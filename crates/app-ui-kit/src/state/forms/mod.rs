use dioxus::prelude::*;
use solana_pubkey::Pubkey;
use wallet_core::edit::{Carrier, OpenedIx, OpenedTx, RawDraft};
use wallet_core::{pinned_program, AddressKind, IdlDraft, TxSettings, SOLANA_PATH_TEMPLATE};
use zeroize::Zeroize;

mod build;
mod cleanup;
mod contact;
mod migrate;
mod send;
mod sign;
mod stake;
mod wallet;

pub use build::*;
pub use cleanup::*;
pub use contact::*;
pub use migrate::*;
pub use send::*;
pub use sign::*;
pub use stake::*;
pub use wallet::*;

#[derive(Clone, Copy)]
pub struct ProgramForm {
    pub selected: Signal<Option<Pubkey>>,
    /// The part of the IDL shown, or the original JSON.
    pub section: Signal<IdlSection>,
    /// A file dialog of the Programs pane is open: Import IDL, Import
    /// bundle and Export IDLs are disabled until it returns. Kept here, not
    /// on the pane, so that leaving the pane while one is open neither lets
    /// a second one open nor drops what the first one picks.
    pub file_dialog: Signal<bool>,
}

/// A tab of a program's IDL.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IdlSection {
    #[default]
    Instructions,
    Accounts,
    Types,
    Pdas,
    Events,
    Errors,
    /// The file as it was imported.
    Raw,
}

/// The History screen: which wallet it shows, and the transaction open.
#[derive(Clone, Copy)]
pub struct HistoryForm {
    /// Only this wallet's transactions. `None` shows every wallet's.
    pub wallet: Signal<Option<Pubkey>>,
    /// The transaction open in detail.
    pub selected: Signal<Option<solana_signature::Signature>>,
    pub view: Signal<SignView>,
}

#[derive(Clone, Copy)]
pub struct Forms {
    pub wallet: WalletForm,
    pub contact: ContactForm,
    pub build: BuildForm,
    pub sign: SignForm,
    pub stake: StakeForm,
    pub send: SendForm,
    pub cleanup: CleanupForm,
    pub migrate: MigrateForm,
    pub program: ProgramForm,
    pub history: HistoryForm,
}

pub fn use_forms() -> Forms {
    Forms {
        wallet: WalletForm {
            label: use_signal(String::new),
            secret: use_signal(String::new),
            passphrase: use_signal(String::new),
            path_template: use_signal(|| SOLANA_PATH_TEMPLATE.to_string()),
            mode: use_signal(|| AddWalletMode::PrivateKey),
            creating: use_signal(|| false),
            error: use_signal(|| None::<String>),
            modal_open: use_signal(|| false),
        },
        contact: ContactForm {
            filter: use_signal(String::new),
            kind: use_signal(|| None::<wallet_core::Group>),
            new_kind: use_signal(|| AddressKind::Account),
            pubkey: use_signal(String::new),
            label: use_signal(String::new),
            tags: use_signal(String::new),
            error: use_signal(|| None::<String>),
            modal_open: use_signal(|| false),
            editing: use_signal(|| None::<Pubkey>),
            editing_group: use_signal(String::new),
            group: use_signal(|| wallet_core::LOCAL_GROUP_ID.to_string()),
            page: use_signal(|| 0usize),
        },
        build: BuildForm {
            drafts: use_signal(Vec::<IxDraft>::new),
            selected: use_signal(|| None::<u64>),
            next_id: use_signal(|| 1u64),
            fee_payer: use_signal(|| None::<Pubkey>),
            picker_reset: use_signal(|| 0u64),
            advanced_open: use_signal(|| false),
            cu_limit: use_signal(String::new),
            cu_price: use_signal(String::new),
            loaded_accounts: use_signal(String::new),
            use_nonce: use_signal(|| false),
            nonce: use_signal(|| None::<Pubkey>),
            nonce_authority: use_signal(|| None::<Pubkey>),
            nonce_hash: use_signal(String::new),
            blockhash: use_signal(String::new),
            error: use_signal(|| None::<String>),
            compiled_note: use_signal(|| None::<String>),
            payer_for: use_signal(|| None::<wallet_core::WalletId>),
            generated: use_signal(Vec::<GeneratedKey>::new),
        },
        sign: SignForm {
            wire: use_signal(|| None::<Vec<u8>>),
            generation: use_signal(|| 0u64),
            source: use_signal(|| None::<TxSource>),
            flow: use_signal(|| None::<u64>),
            context: use_signal(|| None),
            stored_behind: use_signal(|| None),
            blind_checked: use_signal(|| None),
            vector: use_signal(Vec::new),
            import: use_signal(String::new),
            error: use_signal(|| None::<String>),
            note: use_signal(|| None::<wallet_core::Phrase>),
            view: use_signal(|| SignView::Summary),
            share: use_signal(|| ShareTab::Everything),
            simulation: use_signal(|| SimState::Idle),
            simulated_for: use_signal(|| None::<[u8; 32]>),
            tables: use_signal(wallet_core::LookupTables::new),
            clock: use_blockhash_clock_state(),
            last_signature: use_signal(|| None::<String>),
            submission: use_signal(|| None::<Submission>),
            rebuilding: use_signal(|| false),
            checks_refused: use_signal(|| None),
            auto_rebuilt_for: use_signal(|| None::<[u8; 32]>),
            rebuilt: use_signal(|| false),
            returned: use_signal(|| false),
            batch: use_signal(|| None),
            made_here: use_signal(Vec::<MadeHere>::new),
            created: use_signal(Vec::new),
            site_sends: use_signal(Vec::new),
            set_aside: use_signal(Vec::new),
            displaced: use_signal(|| None),
        },
        stake: StakeForm {
            snapshot: use_signal(|| None),
            loaded_for: use_signal(|| None::<String>),
            loading: use_signal(|| false),
            error: use_signal(|| None::<String>),
            rewards: use_signal(Vec::new),
            reward_epochs: use_signal(Vec::<u64>::new),
            rewards_loading: use_signal(|| false),
            rewards_error: use_signal(|| None::<String>),
        },
        send: SendForm {
            open: use_signal(|| None::<SendOpen>),
        },
        cleanup: CleanupForm {
            open: use_signal(|| None),
            run: use_signal(|| None),
            notice: use_signal(|| None),
        },
        migrate: MigrateForm {
            open: use_signal(|| None),
            run: use_signal(|| None),
            notice: use_signal(|| None),
            finished: use_signal(|| None),
        },
        program: ProgramForm {
            selected: use_signal(|| None::<Pubkey>),
            section: use_signal(IdlSection::default),
            file_dialog: use_signal(|| false),
        },
        history: HistoryForm {
            wallet: use_signal(|| None::<Pubkey>),
            selected: use_signal(|| None),
            view: use_signal(|| SignView::Summary),
        },
    }
}
