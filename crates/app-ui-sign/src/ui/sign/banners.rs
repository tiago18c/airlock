//! What stands between the transaction and a signature: a rebuild, a stale
//! blockhash, and who still has to sign.

use super::*;

/// Shown when a required signer is an address no key signs for.
#[component]
pub(in crate::ui) fn RebuildBanner(plan: Result<AdaptPlan, String>, online: bool) -> Element {
    let session = use_context::<Session>();
    let busy = session.forms.sign.rebuilding;
    let plan = match plan {
        Ok(plan) => plan,
        Err(problem) => {
            return rsx! {
                div { class: "callout",
                    p { class: "error", role: "alert", "{problem}" }
                    p { class: "muted",
                        "This transaction asks a wallet without a key to sign, and it cannot be rebuilt for it. It cannot be signed as it stands."
                    }
                }
            }
        }
    };
    let fee_payer = plan.fee_payer;
    rsx! {
        div { class: "callout",
            for (i, asked) in plan.asked().into_iter().enumerate() {
                match asked {
                    AskedWallet::Squads { vault, member, .. } => rsx! {
                        p { key: "{i}",
                            Address { pubkey: vault }
                            " is the vault of a Squads multisig. No key signs a transaction as a vault: its member "
                            Address { pubkey: member }
                            " stores these instructions as a proposal instead, then approves and executes it as far as the multisig allows."
                        }
                    },
                    AskedWallet::Programmatic { authority, address, .. } => rsx! {
                        p { key: "{i}",
                            Address { pubkey: address }
                            " is a programmatic signer. No key signs a transaction as that address: its authority "
                            Address { pubkey: authority }
                            " signs an authorization instead, and a hot wallet relays it."
                        }
                    },
                    AskedWallet::Vector { scheme, account, .. } => rsx! {
                        p { key: "{i}",
                            Address { pubkey: account }
                            " is a Vector wallet. No key signs a transaction as that account: its {scheme.label()} key signs the whole transaction instead, and a hot wallet relays it."
                        }
                    },
                }
            }
            p { class: "muted",
                "Rebuilding moves these instructions, unchanged, inside that proposal or authorization. "
                Address { pubkey: fee_payer }
                " pays the fee. This is the only change this screen makes to a transaction."
            }
            div { class: "btn-row",
                button {
                    class: "primary",
                    disabled: !online || busy(),
                    onclick: move |_| rebuild_loaded(session),
                    if busy() { "Rebuilding…" } else { "Rebuild the transaction" }
                }
                if !online {
                    span { class: "hint",
                        "Needs a connection, to read the multisig or the signer's nonce, and a blockhash."
                    }
                }
            }
        }
    }
}

/// Moves an expired transaction that this device has nothing left to sign
/// to a fresh blockhash, for any transaction not on a durable nonce. Every
/// other move happens on the Sign press.
#[component]
pub(in crate::ui) fn RefreshBlockhash() -> Element {
    let session = use_context::<Session>();
    let busy = use_signal(|| false);
    rsx! {
        button {
            class: "primary",
            disabled: busy(),
            title: "A transaction is only accepted for about 30 seconds after its blockhash. An authorization inside it does not expire.",
            onclick: move |_| refresh_loaded(session, busy),
            if busy() { "Rebuilding…" } else { "Rebuild on a fresh blockhash" }
        }
    }
}

#[component]
fn SignerRow(line: wallet_core::SignerLine, inspect: bool) -> Element {
    // A phone has no width for the chip beside the address: it goes up
    // beside the role.
    let phone = crate::ui::use_phone_layout();
    let role = match line.role {
        SignerRole::FeePayer => "Fee payer",
        SignerRole::Signer => "Signer",
        SignerRole::Authority { .. } => "Authority",
    };
    let state = rsx! { SignerChip { line } };
    rsx! {
        li { class: if phone { "book-row signer-row stacked" } else { "book-row signer-row" },
            if phone {
                div { class: "chip-row signer-head",
                    span { class: "signer-role", "{role}" }
                    {state.clone()}
                }
            }
            div { class: "chip-row",
                if !phone {
                    span { class: "signer-role", "{role}" }
                }
                AddressRow { pubkey: line.authority_chip(), full: inspect,
                    if let SignerRole::Authority { address } = line.role {
                        // A Vector row without the controlling key only knows the
                        // account; do not print it twice around "for".
                        if line.authority_chip() != address {
                            span { class: "muted", "for" }
                            Address { pubkey: address }
                        }
                    }
                    if !phone {
                        {state}
                    }
                }
            }
        }
    }
}

/// Where a signature stands, and for one not made yet, what holds its key.
#[component]
fn SignerChip(line: wallet_core::SignerLine) -> Element {
    match line.state {
        SignerState::Signed => rsx! { span { class: "chip ok", "signed" } },
        SignerState::Invalid => rsx! { span { class: "chip danger", "invalid signature" } },
        SignerState::Local => rsx! { span { class: "chip info", "on this device · not signed" } },
        SignerState::Device => match line.on_device {
            Some(wallet_core::HardwareVendor::SeedVault) => rsx! {
                span { class: "chip kind", "in the Seed Vault · not signed" }
            },
            Some(vendor) => rsx! {
                span { class: "chip info", "on a {vendor.label()} · not signed" }
            },
            None => rsx! { span { class: "chip info", "on a hardware wallet · not signed" } },
        },
        SignerState::Missing => rsx! { span { class: "chip warn", "needed from elsewhere" } },
    }
}

#[component]
pub(in crate::ui) fn SignerList(status: SigningStatus, inspect: bool) -> Element {
    let authorities: Vec<_> = status
        .lines
        .iter()
        .cloned()
        .enumerate()
        .filter(|(_, line)| matches!(line.role, SignerRole::Authority { .. }))
        .collect();
    let outer: Vec<_> = status
        .lines
        .iter()
        .cloned()
        .enumerate()
        .filter(|(_, line)| !matches!(line.role, SignerRole::Authority { .. }))
        .collect();
    rsx! {
        if !authorities.is_empty() {
            h4 { "Authorization (intent)" }
            ul { class: "book-list",
                for (i, line) in authorities {
                    SignerRow { key: "{i}", line, inspect }
                }
            }
            h4 { "Transaction" }
        }
        ul { class: "book-list",
            for (i, line) in outer {
                SignerRow { key: "{i}", line, inspect }
            }
        }
    }
}

/// `tick` changes every second while the clock runs, so this redraws.
/// Paste box for an EIP-191 Vector authority whose key is an Ethereum wallet.
#[component]
pub(in crate::ui) fn PersonalSignBox(tx: std::rc::Rc<DecodedTx>, status: SigningStatus) -> Element {
    let session = use_context::<Session>();
    let mut pasted = use_signal(String::new);
    let form = session.forms.sign;
    let known = (form.vector)();
    let vector_here = session.local_vector_accounts();
    let pending = status.lines.iter().find_map(|line| {
        let wallet_core::SignerKey::Authority(wallet_core::relay::AuthorityId::Vector(account)) =
            line.key
        else {
            return None;
        };
        if line.state == SignerState::Signed || vector_here.contains(&account) {
            return None;
        }
        let ctx = wallet_core::relay::AuthContext { vector: &known };
        let digest = wallet_core::relay::personal_sign_digest(&tx, &account, &ctx).ok()?;
        Some((account, digest))
    });
    let Some((account, digest)) = pending else {
        return rsx! {};
    };
    let digest_hex = format!("0x{}", hex::encode(digest));
    rsx! {
        div { class: "callout",
            h4 { "Ethereum personal_sign" }
            p {
                "Sign these 32 bytes with personal_sign (not the hex text), then paste the 65-byte signature."
            }
            p { class: "mono", "{digest_hex}" }
            CopyButton { text: digest_hex.clone() }
            label { "Signature"
                input {
                    r#type: "text",
                    class: "mono",
                    placeholder: "0x… 65-byte signature",
                    value: "{pasted}",
                    oninput: move |evt| pasted.set(evt.value()),
                }
            }
            div { class: "btn-row",
                button {
                    class: "primary",
                    disabled: pasted().trim().is_empty(),
                    onclick: move |_| {
                        apply_personal_sign(session, account, pasted());
                        pasted.set(String::new());
                    },
                    "Add signature"
                }
            }
        }
    }
}

/// The rotations signed on this device that it has not been shown landed,
/// each to show again for the online device. Showing one signs nothing, so
/// a code that was lost on the way, or a transaction that did not land, costs
/// no leaf.
#[component]
pub(in crate::ui) fn KeptRotations() -> Element {
    let session = use_context::<Session>();
    let mut shown = use_signal(|| None::<QrObject>);
    let wallets = (session.wallets)();
    if wallets.kept_rotations.is_empty() {
        return rsx! {};
    }
    rsx! {
        div { class: "callout",
            p {
                "Signed here, and not shown to have landed yet. Show one again for the online device to send, as often as needed: that signs nothing. Scan the account's state from the online device once it has landed."
            }
            for rotation in wallets.kept_rotations.iter().cloned() {
                {
                    let account = Pubkey::new_from_array(rotation.account);
                    let label = wallets
                        .summaries
                        .iter()
                        .find(|wallet| wallet.address == Some(account))
                        .map(|wallet| wallet.label.clone());
                    let signed = rotation.signed.len();
                    rsx! {
                        div { key: "{account}", class: "btn-row",
                            span {
                                "Key rotation of "
                                if let Some(label) = label {
                                    "{label} "
                                }
                                Address { pubkey: account }
                                if signed > 1 {
                                    span { class: "muted", " · signed at {signed} nonces" }
                                }
                            }
                            button { class: "sm",
                                onclick: move |_| shown.set(Some(QrObject::KeyRotation(rotation.clone()))),
                                "Show again"
                            }
                        }
                    }
                }
            }
        }
        if let Some(object) = shown() {
            div { class: "modal-backdrop", onclick: move |_| shown.set(None),
                div { class: "modal", onclick: move |evt| evt.stop_propagation(),
                    div { class: "stack",
                        h3 { "Key rotation" }
                        p { class: "lede",
                            "On the online device, scan this on Sign or paste the text there. It sends the move to the new key, paid by the wallet's fee payer. It carries the new public key and signatures, never a secret."
                        }
                        PayloadShare { objects: vec![object],
                            button { class: "ghost",
                                onclick: move |_| shown.set(None),
                                "Close"
                            }
                        }
                    }
                }
            }
        }
    }
}
