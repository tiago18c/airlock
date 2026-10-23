//! What a node said about the addresses: balances, token amounts, nonces.

use super::*;

impl AddressBook {
    /// Update a token account that is already in the book. New accounts go to
    /// the holdings cache, not into this list.
    pub fn apply_discovered_token(&mut self, token: DiscoveredTokenAccount) {
        let key = token.pubkey.to_string();
        self.touch(&key, |entry| fill_token(entry, &token));
    }

    pub fn apply_discovered_nonce(&mut self, nonce: DiscoveredNonce) {
        let wallet_id = self.get(&nonce.authority).and_then(|owner| owner.wallet_id);
        let key = nonce.pubkey.to_string();
        if self.touch(&key, |entry| fill_nonce(entry, &nonce)) {
            return;
        }
        self.local.entries.insert(
            key,
            AddressEntry {
                pubkey: nonce.pubkey,
                label: String::new(),
                tags: BTreeSet::from(["nonce".into()]),
                kind: AddressKind::NonceAccount,
                origin: Origin::Discovered,
                owner: Some(nonce.authority),
                mint: None,
                wallet_id,
                hidden: false,
                lamports: None,
                token_amount: None,
                decimals: None,
                nonce_hash: Some(nonce.hash),
                watched: false,
            },
        );
    }

    /// Forget what was read from the chain: balances, token amounts and
    /// nonce values. Labels, tags and the entries themselves stay. For a
    /// change of cluster, where the old values describe another network.
    pub fn clear_chain_state(&mut self) {
        let groups = std::iter::once(&mut self.local).chain(self.imported.iter_mut());
        for entry in groups.flat_map(|group| group.entries.values_mut()) {
            entry.lamports = None;
            entry.token_amount = None;
            entry.nonce_hash = None;
        }
    }

    pub fn set_lamports(&mut self, pubkey: &Pubkey, lamports: u64) -> bool {
        self.touch(&pubkey.to_string(), |entry| entry.lamports = Some(lamports))
    }

    pub fn apply_snapshot(&mut self, snapshot: &crate::chain::ChainSnapshot) {
        for (pubkey, lamports) in &snapshot.balances {
            self.set_lamports(pubkey, *lamports);
        }
        for token in &snapshot.tokens {
            self.apply_discovered_token(token.clone());
        }
        for nonce in &snapshot.nonces {
            self.apply_discovered_nonce(nonce.clone());
        }
    }
}

pub(super) fn fill_token(entry: &mut AddressEntry, token: &DiscoveredTokenAccount) {
    entry.kind = AddressKind::TokenAccount;
    entry.owner = Some(token.owner);
    entry.mint = Some(token.mint);
    entry.token_amount = Some(token.amount);
    entry.decimals = Some(token.decimals);
    if entry.origin != Origin::Imported && entry.origin != Origin::Derived {
        entry.origin = Origin::Discovered;
    }
}

pub(super) fn fill_nonce(entry: &mut AddressEntry, nonce: &DiscoveredNonce) {
    entry.kind = AddressKind::NonceAccount;
    entry.owner = Some(nonce.authority);
    entry.nonce_hash = Some(nonce.hash);
    if entry.origin != Origin::Imported && entry.origin != Origin::Derived {
        entry.origin = Origin::Discovered;
    }
}
