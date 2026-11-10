//! What a Vector wallet's row says of its account, and the steps that move
//! its account along: initializing it, and for a hash-based key, the next
//! key it rotates into.

use super::*;
use crate::ui::transfer::send::PayloadShare;
use std::collections::HashMap;
use wallet_core::protocol::QrObject;
use wallet_core::vector::{
    account_state, leaf_status_note, warn_leaves, SchemeId, VectorAccount, XMSS_LEAVES,
    XMSS_RECOVERY_LEAVES,
};

/// What the chain says of a Vector wallet's account.
#[derive(Clone, PartialEq, Eq)]
enum VectorChain {
    Loading,
    /// No account, or one that is not a complete Vector account yet.
    Uninitialized,
    Ready {
        nonce: String,
        leaves_left: Option<u32>,
    },
    Failed(String),
}

/// Each Vector account read, by address, or why the read failed.
type ReadAccounts = Result<HashMap<Pubkey, Option<VectorAccount>>, String>;

/// What the chain says of every Vector wallet's account, read for all of
/// them in one request when the Wallets pane opens, and again after one is
/// initialized.
#[derive(Clone, Copy)]
pub struct VectorReads {
    /// By address. `None` while reading.
    read: Signal<Option<ReadAccounts>>,
    /// Bumped to read again.
    turn: Signal<u32>,
}

impl VectorReads {
    fn chain(&self, address: &Pubkey) -> VectorChain {
        match &*self.read.read() {
            None => VectorChain::Loading,
            Some(Err(problem)) => VectorChain::Failed(problem.clone()),
            Some(Ok(accounts)) => match accounts.get(address) {
                None => VectorChain::Loading,
                Some(None) => VectorChain::Uninitialized,
                Some(Some(account)) => VectorChain::Ready {
                    nonce: SchemeId::show_nonce(&account.nonce),
                    leaves_left: leaves_left(account),
                },
            },
        }
    }

    fn account(&self, address: &Pubkey) -> Option<VectorAccount> {
        match &*self.read.read() {
            Some(Ok(accounts)) => accounts.get(address).cloned().flatten(),
            _ => None,
        }
    }

    fn again(mut self) {
        self.turn += 1;
    }
}

/// Provide [`VectorReads`] for the Vector wallets among `wallets`.
pub fn use_vector_reads(wallets: &[WalletSummary]) {
    let network = crate::state::use_network();
    let online = network.is_some();
    let addresses: Vec<Pubkey> = wallets
        .iter()
        .filter(|wallet| {
            wallet
                .contract
                .as_ref()
                .and_then(WalletKind::vector_settings)
                .is_some()
        })
        .filter_map(|wallet| wallet.address)
        .collect();
    let mut read = use_signal(|| None);
    let turn = use_signal(|| 0u32);
    // Bumped by each read, so an older answer is dropped.
    let mut generation = use_signal(|| 0u32);
    use_context_provider(|| VectorReads { read, turn });
    use_effect(use_reactive!(|(addresses, online)| {
        let _ = turn();
        let Some(network) = network.filter(|_| online && !addresses.is_empty()) else {
            return;
        };
        let mine = *generation.peek() + 1;
        generation.set(mine);
        read.set(None);
        spawn(async move {
            let found = match network.rpc() {
                Err(err) => Err(err.to_string()),
                Ok(rpc) => wallet_flows::vector::read_accounts(rpc.as_ref(), &addresses)
                    .await
                    .map(|accounts| addresses.iter().copied().zip(accounts).collect())
                    .map_err(|err| err.to_string()),
            };
            if *generation.peek() == mine {
                read.set(Some(found));
            }
        });
    }));
}

/// The account of the Vector wallet `id`, at `address`, as the chain has it.
#[component]
pub(super) fn VectorFacts(
    id: WalletId,
    address: Pubkey,
    scheme: SchemeId,
    shown_key: String,
    held_by: String,
    /// The key is kept with the wallet on this device.
    held_here: bool,
) -> Element {
    let session = use_context::<Session>();
    let network = crate::state::use_network();
    let mut error = use_signal(|| None::<String>);
    let note = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);
    let reads = use_context::<VectorReads>();
    let chain = move || reads.chain(&address);
    // The last rotation signed here, shown as a QR until it is closed.
    let mut rotation = use_signal(|| None::<QrObject>);
    let is_xmss = scheme == SchemeId::Xmss;
    let rotates_here = is_xmss && held_here;
    // Every Winternitz transaction names the key the account moves to.
    let is_winternitz = scheme == SchemeId::Winternitz;
    let next_key = session.vector_next_pubkey(id).filter(|_| is_winternitz);
    // The account already holds the key stored as next: it moved on, and the
    // key after it is not here yet.
    let next_is_current = next_key.as_ref().is_some_and(|next| {
        reads
            .account(&address)
            .and_then(|account| account.current_key().map(<[u8]>::to_vec))
            .as_ref()
            == Some(next)
    });
    let mut show_next = use_signal(|| false);
    let online = network.is_some();
    // Leaves this device has used, from its own record: what an airgapped
    // device knows without the chain.
    let used_here = (session.wallets)()
        .summaries
        .iter()
        .find(|wallet| wallet.id == id)
        .and_then(|wallet| wallet.contract.clone())
        .and_then(|kind| match kind.vector_settings()?.authority {
            Authority::Held { next_leaf, .. } => Some(*next_leaf),
            Authority::Wallet { .. } | Authority::Outside => None,
        })
        .filter(|_| rotates_here);
    let kind = (session.wallets)()
        .summaries
        .iter()
        .find(|wallet| wallet.id == id)
        .and_then(|wallet| wallet.contract.clone());
    let phrase = kind
        .as_ref()
        .and_then(WalletKind::vector_phrase)
        .map(|phrase| (phrase.index, phrase.restored));
    let created_with = kind
        .as_ref()
        .and_then(WalletKind::vector_settings)
        .map(|settings| settings.key.to_vec())
        .unwrap_or_default();
    // What the chain says of the account, as a QR for the device that holds
    // the key, which cannot read it.
    let mut show_state = use_signal(|| false);
    let state_object = reads
        .account(&address)
        .map(|read| QrObject::AccountState(account_state(&address, &read, &created_with)));
    // A rotation signed here that the chain does not show yet. Online, the
    // chain says whether it landed.
    let pending = session
        .pending_rotation(&address)
        .filter(|_| rotates_here)
        .filter(|next| {
            reads
                .account(&address)
                .and_then(|account| account.current_key().map(<[u8]>::to_vec))
                .as_ref()
                != Some(next)
        });

    // The same, with its signatures: shown or sent again without signing.
    let kept = session
        .kept_rotation(&address)
        .filter(|_| rotates_here && pending.is_some());
    let kept_nonces = kept.as_ref().map_or(0, |kept| kept.signed.len());
    // How many more times a wallet restored from its phrase can sign its
    // rotation, once the vault is unlocked.
    let restored = phrase.is_some_and(|(_, restored)| restored);
    let recovery_left = restored.then(|| session.recovery_left(id)).flatten();
    // A rotation the device that holds the key gave this one to send, and
    // where the chain says it stands.
    let received = session
        .received_rotation(&address)
        .filter(|_| online && !held_here)
        .and_then(|rotation| {
            let account = reads.account(&address)?;
            let current = account
                .current_key()
                .map_or_else(|| created_with.clone(), <[u8]>::to_vec);
            Some(if current == rotation.next_key {
                Received::Landed
            } else if rotation
                .signed
                .iter()
                .any(|signed| signed.nonce == account.nonce)
            {
                Received::Waiting
            } else {
                Received::Overtaken
            })
        });
    let landed = received == Some(Received::Landed);
    use_effect(use_reactive!(|(address, landed)| {
        if landed {
            session.forget_received_rotation(&address);
        }
    }));
    let send_again = move |_| {
        error.set(None);
        let Some(rotation) = session.received_rotation(&address) else {
            return;
        };
        spawn(async move {
            if let Err(err) = crate::actions::wallets::send_rotation(session, rotation).await {
                error.set(Some(err));
            }
        });
    };
    let show_kept = move |_| {
        if let Some(kept) = session.kept_rotation(&address) {
            rotation.set(Some(QrObject::KeyRotation(kept)));
        }
    };

    use_effect(use_reactive!(|(id, address)| {
        // Read again once the vault is unlocked: the prepared key is in it.
        if !session.is_unlocked() {
            return;
        }
        if let Some(account) = reads.account(&address) {
            install_when_landed(session, id, &account, note, error);
        }
    }));
    let initialize = move |_| {
        error.set(None);
        busy.set(true);
        spawn(async move {
            match crate::actions::wallets::initialize_vector(session, id).await {
                Ok(()) => reads.again(),
                Err(err) => error.set(Some(err)),
            }
            busy.set(false);
        });
    };
    let prepare_next = move |_| {
        error.set(None);
        session.when_unlocked("Unlock the vault to make the next key.", move || {
            if let Err(err) = session.prepare_rotating_next_key(id) {
                error.set(Some(err.to_string()));
            }
        });
    };
    let rotate = move |_| {
        error.set(None);
        session.when_unlocked("Unlock the vault to rotate the key.", move || {
            let made = match session.sign_rotation(id) {
                Ok(made) => made,
                Err(err) => {
                    error.set(Some(err));
                    return;
                }
            };
            if online {
                // This device can send it itself.
                spawn(async move {
                    if let Err(err) = crate::actions::wallets::send_rotation(session, made).await {
                        error.set(Some(err));
                    }
                });
            } else {
                rotation.set(Some(QrObject::KeyRotation(made)));
            }
        });
    };

    rsx! {
        p { class: "wallet-fact",
            span { class: "muted", "Scheme" }
            span { "{scheme.label()}" }
        }
        p { class: "wallet-fact",
            span { class: "muted", "Key" }
            span { class: "mono", "{shown_key}" }
        }
        p { class: "wallet-fact",
            span { class: "muted", "Signs with" }
            span { "{held_by}" }
        }
        if !online && held_here {
            p { class: "note",
                "Initializing the account needs a fee payer and a recent blockhash, so it is done from an online device. Use Export key, add the key there under Add wallet, From another device, and initialize it from that row. This device then signs what that one prepares."
            }
        } else if !online {
            p { class: "muted", "Account state is available when the app is online." }
        }
        if !online {
            if let Some(used) = used_here {
                p { class: "wallet-fact",
                    span { class: "muted", "Leaves used here" }
                    span { "{used} of {XMSS_LEAVES}" }
                }
                if let Some(warning) = leaf_status_note(SchemeId::Xmss, used) {
                    p { class: "warn", "{warning}" }
                }
            }
        }
        if online { match chain() {
            VectorChain::Loading => rsx! {
                p { class: "muted", "Reading the account…" }
            },
            VectorChain::Failed(problem) => rsx! {
                p { class: "warn", "{problem}" }
            },
            VectorChain::Uninitialized => rsx! {
                p { class: "note",
                    "Not initialized on this cluster yet. It can already receive SOL and tokens; initializing keeps what it holds. It cannot send until then."
                }
                div { class: "btn-row",
                    button { disabled: busy(), onclick: initialize,
                        if busy() { "Preparing…" } else { "Initialize" }
                    }
                }
            },
            VectorChain::Ready { nonce, leaves_left } => {
                let used = leaves_left.map(|left| XMSS_LEAVES.saturating_sub(left));
                let leaf_note = used.and_then(|used| leaf_status_note(SchemeId::Xmss, used));
                let due = used.is_some_and(warn_leaves);
                rsx! {
                    p { class: "wallet-fact",
                        span { class: "muted", "Nonce" }
                        span { class: "mono", "{nonce}…" }
                    }
                    if let Some(left) = leaves_left {
                        p { class: "wallet-fact",
                            span { class: "muted", "Leaves left" }
                            span { "{left}" }
                        }
                    }
                    if let Some(note) = leaf_note {
                        p { class: "warn", "{note}" }
                    }
                    if due && !held_here {
                        p { class: "note",
                            "On the device that holds this key, press Rotate key on this wallet's row, then scan or paste what it shows on Sign here. This device sends it, paid by the fee payer."
                        }
                    }
                }
            },
        }}
        if let Some((index, restored)) = phrase {
            p { class: "wallet-fact",
                span { class: "muted", "Recovery phrase" }
                span { "signs with key number {index}" }
            }
            if restored {
                p { class: "warn",
                    "Restored from its recovery phrase. It signs nothing until it has moved to its next key: scan the account's state from the online device on Sign, then press Rotate key and send what it shows at once."
                }
                p { class: "warn",
                    match recovery_left {
                        Some(0) => "No attempt at moving it is left on this device: every leaf kept for recovery has signed. A rotation already signed can still be sent again.".to_string(),
                        Some(1) => "1 attempt at moving it is left on this device.".to_string(),
                        Some(left) => format!("{left} attempts at moving it are left on this device."),
                        None => format!("It has at most {XMSS_RECOVERY_LEAVES} attempts at moving it. Unlock the vault to see how many are left."),
                    }
                    " An attempt is spent only when the account's nonce moved before a rotation landed, because something signed before the key was lost landed first. Send anything like that, or let it go, before you rotate."
                }
                p { class: "warn",
                    "Do not type this phrase into another device until a rotation has landed. A second copy would sign with the same leaves, and two signatures with one leaf give the key away."
                }
            }
        }
        if let Some(received) = received {
            match received {
                Received::Waiting => rsx! {
                    p { class: "warn", role: "status",
                        "The rotation scanned for this wallet has not landed: the account still holds its old key. If its transaction was sent and is gone, send it again."
                    }
                    div { class: "btn-row",
                        button { class: "sm",
                            title: "Build the rotation's transaction again, on a new blockhash, and open it in Sign. The key device does not have to sign again.",
                            onclick: send_again,
                            "Send rotation again"
                        }
                    }
                },
                Received::Overtaken => rsx! {
                    p { class: "warn", role: "alert",
                        "The rotation scanned for this wallet can no longer land: the account's nonce moved first. Show the account state to the device that holds the key, press Rotate key there again, and scan what it shows."
                    }
                },
                Received::Landed => rsx! {
                    p { class: "note", role: "status",
                        "The rotation landed: the account holds its next key. Show the account state to the device that holds the key, so it signs with it."
                    }
                },
            }
        }
        if scheme.is_stateful() && !held_here && online && state_object.is_some() {
            div { class: "btn-row",
                button { class: "sm",
                    title: if is_xmss {
                        "Show what the chain says of this account, for the device that holds its key: where its nonce is, its leaves, and the key it holds. A device restored from a recovery phrase needs it before it can rotate."
                    } else {
                        "Show what the chain says of this account, for the device that holds its key: where its nonce is, and the key it holds. Once it shows a transaction landed, that device moves to its next key and makes the one after it."
                    },
                    onclick: move |_| show_state.set(true),
                    "Show account state"
                }
            }
        }
        if show_state() {
            if let Some(object) = state_object.clone() {
                div { class: "modal-backdrop", onclick: move |_| show_state.set(false),
                    div { class: "modal", onclick: move |evt| evt.stop_propagation(),
                        div { class: "stack",
                            h3 { "Account state" }
                            p { class: "lede",
                                "On the device that holds the key, scan this on Sign or paste the text there. It is read from the chain now, and goes out of date when the account acts."
                            }
                            PayloadShare { objects: vec![object],
                                button { class: "ghost",
                                    onclick: move |_| show_state.set(false),
                                    "Close"
                                }
                            }
                        }
                    }
                }
            }
        }
        if rotates_here {
            p { class: "note",
                "Keep this key on this device only, and never sign with a copy restored from a backup."
            }
            if pending.is_some() && online {
                p { class: "warn", role: "status",
                    "A rotation to the next key was signed here and has not landed. Press Rotate key to send it again: nothing new is signed unless the account's nonce moved."
                }
            } else if pending.is_some() {
                p { class: "warn", role: "status",
                    if kept_nonces == 1 {
                        "A rotation to the next key is signed, and this device has not been shown that it landed."
                    } else {
                        "A rotation to the next key is signed, at each of {kept_nonces} nonces the account may be at, and this device has not been shown that it landed."
                    }
                    " Show it again for the online device as often as needed: that signs nothing. Once it has landed, scan the account's state from the online device on Sign, and this device signs with the next key."
                }
                if kept.is_some() {
                    div { class: "btn-row",
                        button { class: "sm",
                            title: "Show the rotation already signed, for the online device to send. Nothing is signed.",
                            onclick: show_kept,
                            "Show rotation again"
                        }
                    }
                }
            }
            div { class: "btn-row",
                button { class: "sm",
                    title: if online {
                        "Move the account to a new XMSS key now, paid by the fee payer."
                    } else {
                        "Sign the move of the account to a new XMSS key, for the online device to send. Each press spends a leaf or two of the current key."
                    },
                    onclick: rotate,
                    "Rotate key"
                }
            }
        }
        if is_winternitz {
            p { class: "note",
                "Each Winternitz signature spends the key, so every transaction moves the account to its next key. A transaction that fails on chain leaves the account on a key that has signed already, and that key signs nothing else: what the account holds can be stuck."
            }
            if held_here {
                p { class: "note",
                    "Keep this key on this device only, and never sign with a copy restored from a backup."
                }
                if next_key.is_some() {
                    p { class: "note", "A next Winternitz public key is stored for the next transaction." }
                    div { class: "btn-row",
                        button { class: "sm",
                            title: "Show the next public key, for the online device to store before it builds the next transaction.",
                            onclick: move |_| show_next.set(true),
                            "Show next key"
                        }
                    }
                } else {
                    div { class: "btn-row",
                        button { class: "sm",
                            title: "Make the next key on this device and store its public key for the next transaction.",
                            onclick: prepare_next,
                            "Prepare next key"
                        }
                    }
                }
            } else if next_key.is_none() || next_is_current {
                p { class: "note",
                    "Before the next transaction, store the next public key of the device that holds this key: scan or paste what Show next key on its row shows on Sign here. After a transaction landed, give that device this account's state first, so it moves to its next key."
                }
            } else {
                p { class: "note", "A next public key from the device that holds this key is stored for the next transaction." }
            }
        }
        if let Some(text) = note() {
            p { class: "note", role: "status", "{text}" }
        }
        if let Some(err) = error() {
            p { class: "error", "{err}" }
        }
        if show_next() {
            if let Some(key) = next_key.clone() {
                div { class: "modal-backdrop", onclick: move |_| show_next.set(false),
                    div { class: "modal", onclick: move |evt| evt.stop_propagation(),
                        div { class: "stack",
                            h3 { "Next public key" }
                            p { class: "lede",
                                "On the online device, scan this on Sign or paste the text there. It stores the key on this wallet, and the next transaction moves the account to it. It is a public key, never a secret."
                            }
                            PayloadShare { objects: vec![QrObject::PublicKey { scheme: scheme.byte(), key }],
                                button { class: "ghost",
                                    onclick: move |_| show_next.set(false),
                                    "Close"
                                }
                            }
                        }
                    }
                }
            }
        }
        if let Some(object) = rotation() {
            div { class: "modal-backdrop", onclick: move |_| rotation.set(None),
                div { class: "modal", onclick: move |evt| evt.stop_propagation(),
                    div { class: "stack",
                        h3 { "Key rotation" }
                        p { class: "lede",
                            "On the online device, scan this on Sign or paste the text there. It sends the move to the new key, paid by this wallet's fee payer. It carries the new public key and signatures, never a secret."
                        }
                        PayloadShare { objects: vec![object],
                            button { class: "ghost",
                                onclick: move |_| rotation.set(None),
                                "Close"
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Where a rotation this device was given to send stands on chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Received {
    /// The account still holds its old key, at a nonce the rotation was
    /// signed at: it can still land.
    Waiting,
    /// The account's nonce moved first: nothing it carries can land.
    Overtaken,
    /// The account holds the key it names.
    Landed,
}

/// When the account already holds the next key this device made for it,
/// the transaction that rotated it has landed: the next key's seed replaces
/// the current one. Never before, since until then the current seed is the
/// only key that controls the account.
fn install_when_landed(
    session: Session,
    id: WalletId,
    account: &VectorAccount,
    mut note: Signal<Option<String>>,
    mut error: Signal<Option<String>>,
) {
    let Some(current) = account.current_key() else {
        return;
    };
    let prepared = session.vector_prepared_next_key(id);
    if prepared.as_deref() != Some(current) {
        return;
    }
    match session.install_rotation(id, current) {
        Ok(()) => note.set(Some(
            "The account moved to its next key, and this device now signs with it.".into(),
        )),
        Err(err) => error.set(Some(err.to_string())),
    }
}

fn leaves_left(account: &VectorAccount) -> Option<u32> {
    account
        .next_leaf()
        .map(|next| XMSS_LEAVES.saturating_sub(next))
}
