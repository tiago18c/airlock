//! What the Add wallet modal keeps while it is open, and saving what it holds.

use super::*;

#[derive(Clone, PartialEq, Eq)]
pub(in crate::ui) struct DerivedRow {
    pub(in crate::ui) index: u32,
    pub(in crate::ui) path: String,
    pub(in crate::ui) pubkey: Pubkey,
    pub(in crate::ui) checked: bool,
}

/// The first accounts derived from a seed phrase, or why that failed.
#[derive(Clone, PartialEq, Eq)]
pub(in crate::ui) struct SeedAccounts {
    pub(in crate::ui) rows: Vec<DerivedRow>,
    pub(in crate::ui) error: Option<String>,
}

/// Accounts past the first batch, and checkbox flips, for one generation of
/// the phrase, passphrase and path. A newer generation ignores this.
#[derive(Clone, PartialEq, Eq, Default)]
pub(in crate::ui) struct SeedTail {
    pub(in crate::ui) generation: u64,
    pub(in crate::ui) rows: Vec<DerivedRow>,
    pub(in crate::ui) flipped: Vec<u32>,
}

/// What the modal's own fields keep besides the shared form.
#[derive(Clone, Copy, PartialEq)]
pub(in crate::ui) struct ModalState {
    /// Extra derived accounts and checkbox flips.
    pub(in crate::ui) tail: Signal<SeedTail>,
    /// Bumped when the phrase, passphrase or path changes, so `tail` no longer applies.
    pub(in crate::ui) generation: Signal<u64>,
    /// The words of a new seed phrase were written down.
    pub(in crate::ui) written: Signal<bool>,
    /// How many words a new seed phrase has.
    pub(in crate::ui) words: Signal<usize>,
    /// A derivation path typed by hand is in use.
    pub(in crate::ui) path_custom: Signal<bool>,
}

/// Drop extra accounts and checkbox flips. Call when the phrase, passphrase or path changes.
pub(in crate::ui) fn invalidate_seed(mut tail: Signal<SeedTail>, mut generation: Signal<u64>) {
    generation += 1;
    tail.set(SeedTail::default());
}

/// First batch only. A phrase that is not 12, 15, 18, 21 or 24 words does not
/// derive, and it is not an error: the field is still being typed.
pub(in crate::ui) fn seed_accounts(
    mode: AddWalletMode,
    creating: bool,
    phrase: &str,
    pass: &str,
    template: &str,
) -> SeedAccounts {
    if mode != AddWalletMode::Seed || creating || !complete_phrase(phrase) {
        return SeedAccounts {
            rows: Vec::new(),
            error: None,
        };
    }
    match derive_batch(phrase, pass, template.trim(), 0, DERIVE_BATCH) {
        Ok(rows) => SeedAccounts { rows, error: None },
        Err(err) => SeedAccounts {
            rows: Vec::new(),
            error: Some(err),
        },
    }
}

fn complete_phrase(phrase: &str) -> bool {
    matches!(phrase.split_whitespace().count(), 12 | 15 | 18 | 21 | 24)
}

/// The first batch plus any later pages, with checkbox flips applied.
pub(in crate::ui) fn shown_seed_rows(
    base: &[DerivedRow],
    tail: &SeedTail,
    generation: u64,
) -> Vec<DerivedRow> {
    let mut rows = base.to_vec();
    if tail.generation != generation {
        return rows;
    }
    rows.extend(tail.rows.iter().cloned());
    for row in &mut rows {
        if tail.flipped.contains(&row.index) {
            row.checked = !row.checked;
        }
    }
    rows
}

/// Where "Derive more" starts, counting only pages for this generation.
pub(in crate::ui) fn next_seed_index(base_len: usize, tail: &SeedTail, generation: u64) -> u32 {
    let extra = if tail.generation == generation {
        tail.rows.len()
    } else {
        0
    };
    (base_len + extra) as u32
}

pub(in crate::ui) fn toggle_seed(mut tail: Signal<SeedTail>, generation: u64, index: u32) {
    let mut tail = tail.write();
    if tail.generation != generation {
        tail.generation = generation;
        tail.rows.clear();
        tail.flipped.clear();
    }
    if let Some(pos) = tail.flipped.iter().position(|item| *item == index) {
        tail.flipped.remove(pos);
    } else {
        tail.flipped.push(index);
    }
}

pub(in crate::ui) fn push_seed_rows(
    mut tail: Signal<SeedTail>,
    generation: u64,
    more: Vec<DerivedRow>,
) {
    let mut tail = tail.write();
    if tail.generation != generation {
        tail.generation = generation;
        tail.rows.clear();
        tail.flipped.clear();
    }
    tail.rows.extend(more);
}

/// Show the form for `kind`, to make a wallet when `creating` and to bring
/// one in otherwise. A secret typed or made for another form is wiped.
pub(in crate::ui) fn show(
    form: WalletForm,
    state: ModalState,
    creating: bool,
    kind: AddWalletMode,
) {
    let kind = if creating && !kind.can_create() {
        AddWalletMode::Seed
    } else {
        kind
    };
    if (form.creating)() == creating && (form.mode)() == kind {
        return;
    }
    let (mut secret, mut error) = (form.secret, form.error);
    let (mut now_creating, mut mode) = (form.creating, form.mode);
    let mut written = state.written;
    secret.write().zeroize();
    error.set(None);
    invalidate_seed(state.tail, state.generation);
    written.set(false);
    if creating && kind == AddWalletMode::Seed {
        secret.set(wallet_core::new_phrase((state.words)()));
    }
    now_creating.set(creating);
    mode.set(kind);
}

pub(in crate::ui) fn derive_batch(
    phrase: &str,
    passphrase: &str,
    template: &str,
    start: u32,
    count: u32,
) -> Result<Vec<DerivedRow>, String> {
    let accounts = wallet_core::derive_batch(phrase, passphrase, template, start, count)
        .map_err(|err| err.to_string())?;
    Ok(accounts
        .into_iter()
        .map(|account| DerivedRow {
            checked: account.index == 0,
            index: account.index,
            path: account.path,
            pubkey: account.pubkey,
        })
        .collect())
}

pub(in crate::ui) fn save_wallet(session: Session, form: WalletForm, derived: &[DerivedRow]) {
    let mut error = form.error;
    let mut modal_open = form.modal_open;
    error.set(None);
    let label_text = form.label.read().trim().to_string();
    if label_text.is_empty() {
        error.set(Some("Label is required.".into()));
        return;
    }
    let signers = match (form.mode)() {
        AddWalletMode::Offline
        | AddWalletMode::FromDevice
        | AddWalletMode::Hardware
        | AddWalletMode::Programmatic
        | AddWalletMode::Vector
        | AddWalletMode::Squads => {
            return;
        }
        AddWalletMode::PrivateKey => {
            match NewSigner::from_private_key(&label_text, &form.secret.read()) {
                Ok(signer) => vec![signer],
                Err(err) => {
                    error.set(Some(err.to_string()));
                    return;
                }
            }
        }
        // A new phrase: its first account, with no passphrase.
        AddWalletMode::Seed if (form.creating)() => {
            let first =
                wallet_core::derive_batch(&form.secret.read(), "", SOLANA_PATH_TEMPLATE, 0, 1);
            match first {
                Ok(accounts) => {
                    NewSigner::from_seed_accounts(&label_text, &form.secret.read(), "", &accounts)
                }
                Err(err) => {
                    error.set(Some(err.to_string()));
                    return;
                }
            }
        }
        AddWalletMode::Seed => {
            let chosen: Vec<DerivedAccount> = derived
                .iter()
                .filter(|row| row.checked)
                .map(|row| DerivedAccount {
                    index: row.index,
                    path: row.path.clone(),
                    pubkey: row.pubkey,
                })
                .collect();
            if chosen.is_empty() {
                error.set(Some("Select at least one derived account.".into()));
                return;
            }
            NewSigner::from_seed_accounts(
                &label_text,
                &form.secret.read(),
                &form.passphrase.read(),
                &chosen,
            )
        }
    };
    if let Err(err) = session.add_signers(signers) {
        error.set(Some(err.to_string()));
        return;
    }
    form.clear();
    modal_open.set(false);
    crate::actions::network::start_refresh(session);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_partial_phrase_does_not_derive() {
        let phrase = "one two three four five six seven eight nine ten eleven";
        let got = seed_accounts(AddWalletMode::Seed, false, phrase, "", SOLANA_PATH_TEMPLATE);
        assert!(got.rows.is_empty());
        assert!(got.error.is_none());
    }

    #[test]
    fn twelve_words_list_the_first_batch() {
        let phrase = wallet_core::new_phrase(12);
        let got = seed_accounts(
            AddWalletMode::Seed,
            false,
            &phrase,
            "",
            SOLANA_PATH_TEMPLATE,
        );
        assert!(got.error.is_none(), "{:?}", got.error);
        assert_eq!(got.rows.len(), DERIVE_BATCH as usize);
        assert!(got.rows[0].checked);
        assert!(!got.rows[1].checked);
    }

    #[test]
    fn creating_a_seed_does_not_list_accounts() {
        let phrase = wallet_core::new_phrase(12);
        let got = seed_accounts(AddWalletMode::Seed, true, &phrase, "", SOLANA_PATH_TEMPLATE);
        assert!(got.rows.is_empty());
        assert!(got.error.is_none());
    }
}
