use crate::book::address_book::{AddressBook, ChainState};
use crate::book::connections::Connections;
use crate::cluster::Cluster;
use crate::error::VaultError;
use serde::{Deserialize, Serialize};
use solana_hash::Hash;
use std::cell::RefCell;
use wallet_vault::store::{check_saveable, load_json, LoadProblem, VaultStore};

/// Unencrypted app state: cluster, settings and the address book. Signer
/// secrets stay in the vault. Files from before carry `mode` and `airgap`
/// keys; they are ignored.
///
/// One value here, and three files on disk: see [`WorkspaceStore`].
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub cluster: Cluster,
    /// The custom cluster typed last, kept while a public one is in use so
    /// it can be chosen again without typing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_cluster: Option<Cluster>,
    /// Offer Vector wallets: to add, to show a site and to rebuild for. Off
    /// until it is turned on in Settings. See `vector::offered`.
    #[serde(default)]
    pub vector_wallets: bool,
    /// Offer the hardware wallets whose v1 support is only in a pre-release
    /// build so far. Off until it is turned on in Settings. See
    /// `hardware::offered`.
    #[serde(default)]
    pub prerelease_devices: bool,
    /// Seconds the vault may stay unlocked while the window is hidden, or
    /// idle while visible. Missing in old files → five minutes.
    #[serde(default = "default_auto_lock_secs")]
    pub auto_lock_secs: u64,
    /// Close hides the window (tray) instead of quitting.
    #[serde(default)]
    pub close_to_tray: bool,
    /// Key for the Tokens API, which prices the portfolio. Kept here in
    /// plain text, like the cluster URL, so prices refresh while the vault
    /// is locked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_api_key: Option<String>,
    /// Sites the user chose to remember, and sites they blocked.
    #[serde(default, skip_serializing_if = "Connections::is_empty")]
    pub connections: Connections,
    /// An app with no node has had its network settled: chosen in its
    /// Settings, or moved off the default once. See
    /// [`Workspace::settle_offline_network`].
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub offline_network_chosen: bool,
    pub book: AddressBook,
}

/// Default auto-lock: five minutes.
pub const DEFAULT_AUTO_LOCK_SECS: u64 = 5 * 60;

fn default_auto_lock_secs() -> u64 {
    DEFAULT_AUTO_LOCK_SECS
}

impl Default for Workspace {
    fn default() -> Self {
        Self {
            cluster: Cluster::default(),
            custom_cluster: None,
            vector_wallets: false,
            prerelease_devices: false,
            auto_lock_secs: DEFAULT_AUTO_LOCK_SECS,
            close_to_tray: false,
            tokens_api_key: None,
            connections: Connections::default(),
            offline_network_chosen: false,
            book: AddressBook::default(),
        }
    }
}

impl Workspace {
    /// Use `cluster` from now on. Answers whether it is another node than
    /// before. If it is, what was read from the old one is dropped, so none
    /// of it is shown as this one's.
    pub fn use_cluster(&mut self, cluster: Cluster) -> bool {
        let moved = self.cluster.rpc_url() != cluster.rpc_url();
        if cluster.is_custom() {
            self.custom_cluster = Some(cluster.clone());
        }
        self.cluster = cluster;
        if moved {
            self.book.clear_chain_state();
        }
        moved
    }

    /// An app with no node (Airlock Offline, the phone's offline mode) reads
    /// transactions for the network of `cluster`, chosen in its Settings as
    /// "Read transactions for". There is no node to ask, so all it decides is
    /// which network's IDLs explain a transaction.
    pub fn read_offline_for(&mut self, cluster: Cluster) {
        self.use_cluster(cluster);
        self.offline_network_chosen = true;
    }

    /// Until a network is chosen, an app with no node reads transactions for
    /// mainnet. Before it offered the choice, its cluster was the default,
    /// devnet, which nobody chose: that moves to mainnet, once. A cluster
    /// that is not the default was chosen (in a folder copied from an online
    /// app, say) and is kept. Answers whether the workspace changed.
    pub fn settle_offline_network(&mut self) -> bool {
        if self.offline_network_chosen {
            return false;
        }
        self.offline_network_chosen = true;
        if self.cluster == Cluster::Devnet {
            self.use_cluster(Cluster::MainnetBeta);
        }
        true
    }

    /// The node answered with its genesis hash: the cluster is the one that
    /// hash belongs to.
    pub fn node_answered(&mut self, genesis: &Hash) {
        self.cluster = self.cluster.reconcile_genesis(genesis);
    }
}

/// What a node said, and which node said it.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChainFile {
    /// Which node. A hash, since a URL can hold an API key.
    node: String,
    #[serde(default)]
    read: ChainState,
}

/// Which node `cluster` is, for [`ChainFile`].
fn node_mark(cluster: &Cluster) -> String {
    hex::encode(crate::sha256_bytes(cluster.rpc_url().as_bytes()))
}

/// What each file was last written with, so that only one that changed is written.
#[derive(Default)]
struct Written {
    user: Option<Vec<u8>>,
    grants: Option<Vec<u8>>,
    chain: Option<Vec<u8>>,
}

fn json_of(value: &impl Serialize) -> Result<Vec<u8>, VaultError> {
    serde_json::to_vec_pretty(value).map_err(|e| VaultError::Serde(e.to_string()))
}

/// A workspace on disk, in three files that do not share a fate. What the user wrote
/// changes when they change it. What a node said changes on every sync, and is
/// worth nothing: damage to it starts it empty and it is read again. Which sites
/// were given what is kept apart from both, and damage to it starts it empty too,
/// so a site has to ask again and nothing else is lost.
///
/// A file from before these were apart holds all of it, and is read as it is. The
/// first save moves it.
pub struct WorkspaceStore<S: VaultStore> {
    user: S,
    grants: S,
    chain: S,
    data: Workspace,
    problem: Option<LoadProblem>,
    grants_problem: Option<LoadProblem>,
    written: RefCell<Written>,
}

impl<S: VaultStore> WorkspaceStore<S> {
    pub fn open(user: S, grants: S, chain: S) -> Self {
        let (mut data, problem): (Workspace, _) = load_json(&user, "address book");
        data.book.seed_programs();
        // Nothing there says what a file from before says, if it says anything.
        let mut written = Written::default();
        let grants_problem = if matches!(grants.load(), Ok(None)) {
            None
        } else {
            let (connections, problem) = load_json(&grants, "list of connected sites");
            data.connections = connections;
            // Until it changes there is nothing to write, which a file that
            // cannot be read yet would refuse.
            written.grants = json_of(&data.connections).ok();
            problem
        };
        // What is read from the chain is worth reading again. Damage to it is
        // not worth saying, and a save writes over it.
        let (read, _): (ChainFile, _) = load_json(&chain, "balances read from the chain");
        if read.node == node_mark(&data.cluster) {
            data.book.restore_chain_state(&read.read);
        }
        Self {
            user,
            grants,
            chain,
            data,
            problem,
            grants_problem,
            written: RefCell::new(written),
        }
    }

    /// Set when the stored workspace could not be read at startup.
    pub fn load_problem(&self) -> Option<&LoadProblem> {
        self.problem.as_ref()
    }

    /// Set when the list of connected sites could not be read at startup.
    pub fn grants_problem(&self) -> Option<&LoadProblem> {
        self.grants_problem.as_ref()
    }

    pub fn data(&self) -> &Workspace {
        &self.data
    }

    pub fn data_mut(&mut self) -> &mut Workspace {
        &mut self.data
    }

    /// Write the files that changed, each whatever becomes of the others. What was
    /// read goes first and what the user wrote last: a crash between them leaves a
    /// file that says it is about another node, or the same thing said twice, and
    /// both read correctly. What was read is worth reading again, so a failure to
    /// write it is not worth saying, and the next save tries again.
    pub fn save(&self) -> Result<(), VaultError> {
        let mut written = self.written.borrow_mut();

        let chain = json_of(&ChainFile {
            node: node_mark(&self.data.cluster),
            read: self.data.book.chain_state(),
        })?;
        if written.chain.as_ref() != Some(&chain) && self.chain.save(&chain).is_ok() {
            written.chain = Some(chain);
        }

        let grants = json_of(&self.data.connections)?;
        let grants_saved = if written.grants.as_ref() == Some(&grants) {
            Ok(())
        } else {
            check_saveable(&self.grants_problem)
                .and_then(|()| self.grants.save(&grants))
                .map(|()| written.grants = Some(grants))
        };

        let mut user = self.data.clone();
        user.connections = Connections::default();
        user.book.clear_chain_state();
        let user = json_of(&user)?;
        let user_saved = if written.user.as_ref() == Some(&user) {
            Ok(())
        } else {
            check_saveable(&self.problem)
                .and_then(|()| self.user.save(&user))
                .map(|()| written.user = Some(user))
        };

        grants_saved.and(user_saved)
    }
    /// Change the data and save it. When the save does not happen the data
    /// in memory is what it was before.
    pub fn change(&mut self, f: impl FnOnce(&mut Workspace)) -> Result<(), VaultError> {
        let previous = self.data.clone();
        f(&mut self.data);
        let saved = self.save();
        if saved.is_err() {
            self.data = previous;
        }
        saved
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_pubkey::Pubkey;

    fn pk(n: u8) -> Pubkey {
        Pubkey::new_from_array([n; 32])
    }

    type Shared = std::sync::Arc<wallet_vault::MemoryStore>;

    /// Three files, and what opening them finds.
    struct Files {
        user: Shared,
        grants: Shared,
        chain: Shared,
    }

    impl Files {
        fn new() -> Self {
            Self {
                user: Default::default(),
                grants: Default::default(),
                chain: Default::default(),
            }
        }

        fn open(&self) -> WorkspaceStore<Shared> {
            WorkspaceStore::open(self.user.clone(), self.grants.clone(), self.chain.clone())
        }

        fn text(file: &Shared) -> Option<String> {
            file.load()
                .unwrap()
                .map(|bytes| String::from_utf8(bytes).unwrap())
        }
    }

    fn grant(origin: &str) -> impl FnOnce(&mut Workspace) + '_ {
        move |ws| ws.connections.remember(origin, vec![pk(9)])
    }

    #[test]
    fn each_file_holds_its_own_and_a_restart_puts_them_together() {
        let files = Files::new();
        let mut workspace = files.open();
        workspace
            .change(|ws| {
                ws.book.insert_contact(pk(1), "Alice");
                ws.book.set_lamports(&pk(1), 7);
            })
            .unwrap();
        workspace.change(grant("https://site.example")).unwrap();

        let user = Files::text(&files.user).unwrap();
        assert!(user.contains("Alice"));
        assert!(!user.contains("lamports"), "{user}");
        assert!(!user.contains("site.example"), "{user}");

        let grants = Files::text(&files.grants).unwrap();
        assert!(grants.contains("site.example"));
        assert!(!grants.contains("Alice"));

        let chain = Files::text(&files.chain).unwrap();
        assert!(chain.contains("lamports"));
        assert!(!chain.contains("Alice"));
        assert!(!chain.contains("site.example"));
        // The node's address can hold an API key, and is not written again here.
        assert!(!chain.contains("http"), "{chain}");

        let reopened = files.open();
        let entry = reopened.data().book.get(&pk(1)).unwrap();
        assert_eq!((entry.label.as_str(), entry.lamports), ("Alice", Some(7)));
        assert!(reopened
            .data()
            .connections
            .grant("https://site.example")
            .is_some());
    }

    #[test]
    fn a_sync_writes_only_what_it_read() {
        let files = Files::new();
        let mut workspace = files.open();
        workspace
            .change(|ws| {
                ws.book.insert_contact(pk(1), "Alice");
            })
            .unwrap();
        files.user.refuse_saves(true);
        files.grants.refuse_saves(true);

        // Nothing but a balance changes, so nothing else is written, or this
        // would fail.
        workspace
            .change(|ws| {
                ws.book.set_lamports(&pk(1), 9);
            })
            .unwrap();
        assert!(Files::text(&files.chain).unwrap().contains('9'));
        files.user.refuse_saves(false);
        files.grants.refuse_saves(false);

        // A save that changes nothing writes nothing.
        files.chain.refuse_saves(true);
        workspace.save().unwrap();
    }

    #[test]
    fn a_file_from_before_they_were_apart_is_read_as_it_is_and_moved_by_the_first_save() {
        let files = Files::new();
        // As an earlier build wrote it: all of it in one file.
        let mut old = Workspace::default();
        old.book.insert_contact(pk(1), "Alice");
        old.book.set_lamports(&pk(1), 7);
        grant("https://site.example")(&mut old);
        files.user.save(&json_of(&old).unwrap()).unwrap();

        let workspace = files.open();
        let entry = workspace.data().book.get(&pk(1)).unwrap();
        assert_eq!((entry.label.as_str(), entry.lamports), ("Alice", Some(7)));
        assert!(workspace
            .data()
            .connections
            .grant("https://site.example")
            .is_some());
        assert_eq!(Files::text(&files.grants), None);

        workspace.save().unwrap();
        let user = Files::text(&files.user).unwrap();
        assert!(!user.contains("lamports"), "{user}");
        assert!(!user.contains("site.example"), "{user}");
        assert!(Files::text(&files.grants).unwrap().contains("site.example"));
        assert!(Files::text(&files.chain).unwrap().contains("lamports"));

        let reopened = files.open();
        let entry = reopened.data().book.get(&pk(1)).unwrap();
        assert_eq!((entry.label.as_str(), entry.lamports), ("Alice", Some(7)));
        assert!(reopened
            .data()
            .connections
            .grant("https://site.example")
            .is_some());
    }

    #[test]
    fn damage_to_what_was_read_loses_only_that_and_says_nothing() {
        let files = Files::new();
        let mut workspace = files.open();
        workspace
            .change(|ws| {
                ws.book.insert_contact(pk(1), "Alice");
                ws.book.set_lamports(&pk(1), 7);
                grant("https://site.example")(ws);
            })
            .unwrap();
        files.chain.damage(b"not json");

        let mut reopened = files.open();
        let entry = reopened.data().book.get(&pk(1)).unwrap();
        assert_eq!((entry.label.as_str(), entry.lamports), ("Alice", None));
        assert!(reopened
            .data()
            .connections
            .grant("https://site.example")
            .is_some());
        assert!(reopened.load_problem().is_none());
        assert!(reopened.grants_problem().is_none());
        // And the next save writes over it.
        reopened
            .change(|ws| {
                ws.book.set_lamports(&pk(1), 8);
            })
            .unwrap();
        assert_eq!(
            files.open().data().book.get(&pk(1)).unwrap().lamports,
            Some(8)
        );
    }

    #[test]
    fn damage_to_the_grants_makes_sites_ask_again_and_loses_nothing_else() {
        let files = Files::new();
        let mut workspace = files.open();
        workspace
            .change(|ws| {
                ws.book.insert_contact(pk(1), "Alice");
                grant("https://site.example")(ws);
            })
            .unwrap();
        files.grants.damage(b"not json");

        let reopened = files.open();
        assert!(reopened
            .data()
            .connections
            .grant("https://site.example")
            .is_none());
        assert_eq!(reopened.data().book.get(&pk(1)).unwrap().label, "Alice");
        assert!(reopened.grants_problem().is_some());
        assert!(reopened.load_problem().is_none());
    }

    #[test]
    fn damage_to_what_the_user_wrote_leaves_the_others_alone() {
        let files = Files::new();
        let mut workspace = files.open();
        workspace.change(grant("https://site.example")).unwrap();
        files.user.damage(b"not json");

        let reopened = files.open();
        assert!(reopened.load_problem().is_some());
        assert!(reopened
            .data()
            .connections
            .grant("https://site.example")
            .is_some());
    }

    #[test]
    fn a_file_that_cannot_be_written_does_not_stop_the_others() {
        let files = Files::new();
        let mut workspace = files.open();
        workspace
            .change(|ws| {
                ws.book.insert_contact(pk(1), "Alice");
            })
            .unwrap();

        // What was read cannot be written: nothing else is held up, nothing says
        // so, and the next save tries again.
        files.chain.refuse_saves(true);
        workspace
            .change(|ws| {
                ws.book.set_label(&pk(1), "Alice B");
                ws.book.set_lamports(&pk(1), 7);
            })
            .unwrap();
        assert!(Files::text(&files.user).unwrap().contains("Alice B"));
        assert!(!Files::text(&files.chain).unwrap().contains("lamports"));
        files.chain.refuse_saves(false);
        workspace.save().unwrap();
        assert!(Files::text(&files.chain).unwrap().contains("lamports"));

        // The grants cannot be written: a name still is, and it is said.
        files.grants.refuse_saves(true);
        let refused = workspace.change(|ws| {
            ws.book.set_label(&pk(1), "Alice C");
            grant("https://site.example")(ws);
        });
        assert!(refused.is_err());
        assert!(Files::text(&files.user).unwrap().contains("Alice C"));
        // What was refused is not kept in memory, and the next save puts the file
        // back in step with it.
        assert_eq!(workspace.data().book.get(&pk(1)).unwrap().label, "Alice B");
        files.grants.refuse_saves(false);
        workspace.save().unwrap();
        assert!(Files::text(&files.user).unwrap().contains("Alice B"));

        // What the user wrote cannot be written: a grant still is.
        files.user.refuse_saves(true);
        let refused = workspace.change(|ws| {
            ws.book.set_label(&pk(1), "Alice D");
            grant("https://other.example")(ws);
        });
        assert!(refused.is_err());
        assert!(Files::text(&files.grants)
            .unwrap()
            .contains("other.example"));
    }

    #[test]
    fn grants_that_cannot_be_read_leave_the_book_working_and_are_not_written_over() {
        let files = Files::new();
        let mut workspace = files.open();
        workspace.change(grant("https://site.example")).unwrap();

        files.grants.refuse_reads(true);
        let mut reopened = files.open();
        assert!(reopened.grants_problem().is_some());
        assert!(reopened
            .data()
            .connections
            .grant("https://site.example")
            .is_none());
        // A change to the book is saved, and says nothing of the grants.
        reopened
            .change(|ws| {
                ws.book.insert_contact(pk(1), "Alice");
            })
            .unwrap();
        // A change to the grants is refused, rather than writing over them.
        assert!(reopened.change(grant("https://other.example")).is_err());

        files.grants.refuse_reads(false);
        let readable = files.open();
        assert!(readable
            .data()
            .connections
            .grant("https://site.example")
            .is_some());
        assert_eq!(readable.data().book.get(&pk(1)).unwrap().label, "Alice");
    }

    #[test]
    fn what_another_node_said_is_not_taken_for_this_ones() {
        let files = Files::new();
        let mut workspace = files.open();
        workspace
            .change(|ws| {
                ws.book.insert_contact(pk(1), "Alice");
                ws.book.set_lamports(&pk(1), 7);
            })
            .unwrap();
        // A crash that wrote what the user chose, and not what follows from it.
        let mut moved: Workspace =
            serde_json::from_str(&Files::text(&files.user).unwrap()).unwrap();
        moved.cluster = custom("http://localhost:8899");
        files.user.save(&json_of(&moved).unwrap()).unwrap();

        let reopened = files.open();
        assert_eq!(reopened.data().book.get(&pk(1)).unwrap().lamports, None);
    }

    #[test]
    fn a_change_that_cannot_be_saved_is_not_kept_in_memory_either() {
        let files = Files::new();
        let store = files.user.clone();
        let mut workspace = files.open();
        workspace
            .change(|ws| {
                ws.book.insert_contact(pk(1), "Alice");
            })
            .unwrap();

        store.refuse_saves(true);
        let refused = workspace.change(|ws| {
            ws.book.insert_contact(pk(2), "Bob");
            ws.close_to_tray = true;
        });
        assert!(refused.is_err());
        assert!(workspace.data().book.get(&pk(2)).is_none());
        assert!(!workspace.data().close_to_tray);

        let reopened = files.open();
        assert_eq!(reopened.data().book.get(&pk(1)).unwrap().label, "Alice");
        assert!(reopened.data().book.get(&pk(2)).is_none());
    }

    fn custom(url: &str) -> Cluster {
        Cluster::Custom {
            name: "mine".into(),
            rpc_url: url.into(),
            known: None,
        }
    }

    #[test]
    fn another_node_drops_what_was_read_from_the_old_one_and_keeps_the_names() {
        let mut data = Workspace::default();
        data.book.insert_contact(pk(1), "Alice");
        data.book.set_lamports(&pk(1), 7);
        let start = data.cluster.clone();

        // The same node again changes nothing.
        assert!(!data.use_cluster(start.clone()));
        assert_eq!(data.book.get(&pk(1)).unwrap().lamports, Some(7));

        assert!(data.use_cluster(custom("http://localhost:8899")));
        let entry = data.book.get(&pk(1)).unwrap();
        assert_eq!((entry.label.as_str(), entry.lamports), ("Alice", None));
    }

    #[test]
    fn the_custom_cluster_typed_last_is_kept_while_a_public_one_is_in_use() {
        let mut data = Workspace::default();
        let mine = custom("http://localhost:8899");
        data.use_cluster(mine.clone());
        data.use_cluster(Cluster::Devnet);
        assert_eq!(data.cluster, Cluster::Devnet);
        assert_eq!(data.custom_cluster, Some(mine));
    }

    #[test]
    fn an_app_with_no_node_reads_mainnet_until_a_network_is_chosen() {
        // A new vault, or one from before the choice: on the default, devnet.
        let mut data = Workspace::default();
        assert!(data.settle_offline_network());
        assert_eq!(data.cluster, Cluster::MainnetBeta);
        // Once settled, it is never moved again.
        assert!(!data.settle_offline_network());

        // Devnet chosen in Settings stays devnet, across a restart.
        data.read_offline_for(Cluster::Devnet);
        let json = serde_json::to_string(&data).unwrap();
        let mut loaded: Workspace = serde_json::from_str(&json).unwrap();
        assert!(!loaded.settle_offline_network());
        assert_eq!(loaded.cluster, Cluster::Devnet);
    }

    #[test]
    fn a_network_someone_chose_before_the_choice_was_offered_is_kept() {
        for chosen in [Cluster::Testnet, custom("http://localhost:8899")] {
            let mut data = Workspace {
                cluster: chosen.clone(),
                ..Workspace::default()
            };
            data.settle_offline_network();
            assert_eq!(data.cluster, chosen);
        }
        // An online app's file says nothing of it.
        let json = serde_json::to_string(&Workspace::default()).unwrap();
        assert!(!json.contains("offlineNetworkChosen"));
    }

    #[test]
    fn a_probe_answer_names_the_cluster() {
        let mut data = Workspace {
            cluster: Cluster::Devnet,
            ..Workspace::default()
        };
        // A hash no public cluster has, on a public URL: the URL is kept as a custom one.
        data.node_answered(&Hash::new_from_array([7; 32]));
        assert!(data.cluster.is_custom());
        assert_eq!(data.cluster.rpc_url(), Cluster::Devnet.rpc_url());
    }

    #[test]
    fn a_workspace_from_before_keeps_loading_without_its_mode() {
        let mut json = serde_json::to_value(Workspace::default()).unwrap();
        json["mode"] = serde_json::json!("offline");
        json["airgap"] = serde_json::json!(true);
        let data: Workspace = serde_json::from_value(json).unwrap();
        assert_eq!(data.cluster, Cluster::default());
    }

    #[test]
    fn workspace_json_keeps_imported_groups() {
        let mut data = Workspace::default();
        data.book.insert_contact(pk(1), "Alice");
        let exported = data.book.export_json("Laptop").unwrap();
        data.book.import_json(&exported).unwrap();
        let json = serde_json::to_string_pretty(&data).unwrap();
        assert!(json.contains("Laptop"));
        assert!(json.contains("Alice"));
        assert!(json.contains(&format!("\"{}\"", pk(1))));
        assert!(!json.contains("[1,1,1,1"));
        let loaded: Workspace = serde_json::from_str(&json).unwrap();
        assert_eq!(
            loaded
                .book
                .rows("", None)
                .iter()
                .filter(|row| row.imported)
                .count(),
            1
        );
        assert_eq!(loaded.book.get(&pk(1)).unwrap().label, "Alice");
    }

    #[test]
    fn old_workspace_json_gets_auto_lock_and_close_defaults() {
        let json = r#"{"cluster":{"kind":"devnet"},"mode":"online","book":{"local":{"id":"local","label":"This device","entries":{}},"imports":[]}}"#;
        let loaded: Workspace = serde_json::from_str(json).unwrap();
        assert_eq!(loaded.auto_lock_secs, DEFAULT_AUTO_LOCK_SECS);
        assert!(!loaded.close_to_tray);
    }
}
