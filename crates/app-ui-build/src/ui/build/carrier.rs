//! A block that carries other instructions: what carries them, and a rail
//! of its own for what is inside. Each inside block is edited like any
//! other, and the carrier is written again around them on compile.

use super::*;

#[component]
pub(in crate::ui) fn CarrierEditor(draft: IxDraft) -> Element {
    let session = use_context::<Session>();
    let form = session.forms.build;
    let id = draft.id;
    let IxBody::Carried { carrier, .. } = draft.body.clone() else {
        return rsx! {};
    };
    let selected = form.selected_in(Some(id));
    let open = form
        .blocks(Some(id))
        .into_iter()
        .find(|inner| Some(inner.id) == selected);
    let title = format!("{} · {}", carrier.label(), carrier.instruction_label());

    rsx! {
        div { class: "ix-editor",
            h3 { "{title}" }
            match carrier.kind {
                CarrierKind::Squads { vault, vault_index } => rsx! {
                    p { class: "hint",
                        "What vault {vault_index} runs once the proposal is approved. The multisig, creator and rent payer stay as they are."
                    }
                    div { class: "chip-row",
                        span { class: "chip", "vault" }
                        Address { pubkey: vault }
                    }
                },
                CarrierKind::Passthrough { signers } => rsx! {
                    p { class: "hint",
                        "What runs as these Vector accounts. The Advance that lists them stays as it is."
                    }
                    for signer in signers {
                        div { class: "chip-row",
                            span { class: "chip",
                                if signer.system { "runs as its system address" } else { "runs as" }
                            }
                            Address { pubkey: signer.address() }
                        }
                    }
                },
                CarrierKind::Programmatic { authorities, nonce_account } => rsx! {
                    p { class: "hint",
                        "What the executor runs once these authorities sign. A change needs their signatures again."
                    }
                    for authority in authorities {
                        div { class: "chip-row",
                            span { class: "chip", "authority" }
                            Address { pubkey: authority }
                        }
                    }
                    div { class: "chip-row",
                        span { class: "chip", "executor nonce" }
                        Address { pubkey: nonce_account }
                    }
                },
            }
            div { class: "ix-nested",
                h4 { "Inside" }
                InstructionRail { parent: Some(id) }
                if let Some(inner) = open {
                    InstructionEditor { key: "{inner.id}", draft: inner }
                } else {
                    p { class: "muted", "Select a block to edit it, or add an instruction inside this one." }
                }
            }
        }
    }
}
