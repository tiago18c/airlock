use crate::book::address_book::AddressBook;
use crate::book::connections::Connections;
use crate::cluster::Cluster;
use crate::error::VaultError;
use serde::{Deserialize, Serialize};
use solana_hash::Hash;
use wallet_vault::store::{check_saveable, load_json, LoadProblem, VaultStore};

/// Unencrypted app state: cluster, settings and the address book. Signer
/// secrets stay in the vault. Files from before carry `mode` and `airgap`
/// keys; they are ignored.
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
            auto_lock_secs: DEFAULT_AUTO_LOCK_SECS,
            close_to_tray: false,
            tokens_api_key: None,
            connections: Connections::default(),
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

    /// The node answered with its genesis hash: the cluster is the one that
    /// hash belongs to.
    pub fn node_answered(&mut self, genesis: &Hash) {
        self.cluster = self.cluster.reconcile_genesis(genesis);
    }
}

pub struct WorkspaceStore<S: VaultStore> {
    store: S,
    data: Workspace,
    problem: Option<LoadProblem>,
}

impl<S: VaultStore> WorkspaceStore<S> {
    pub fn open(store: S) -> Self {
        let (mut data, problem): (Workspace, _) = load_json(&store, "address book");
        data.book.seed_programs();
        Self {
            store,
            data,
            problem,
        }
    }

    /// Set when the stored workspace could not be read at startup.
    pub fn load_problem(&self) -> Option<&LoadProblem> {
        self.problem.as_ref()
    }

    pub fn data(&self) -> &Workspace {
        &self.data
    }

    pub fn data_mut(&mut self) -> &mut Workspace {
        &mut self.data
    }

    pub fn save(&self) -> Result<(), VaultError> {
        check_saveable(&self.problem)?;
        let json =
            serde_json::to_vec_pretty(&self.data).map_err(|e| VaultError::Serde(e.to_string()))?;
        self.store.save(&json)
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

    #[test]
    fn a_change_that_cannot_be_saved_is_not_kept_in_memory_either() {
        let store = std::sync::Arc::new(wallet_vault::MemoryStore::new());
        let mut workspace = WorkspaceStore::open(store.clone());
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

        let reopened = WorkspaceStore::open(store);
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
