//! The public stores: the workspace with its address book, and token holdings.
//! They live in the core: writes are core commands, reads are the pumped slices.

use super::*;

impl Session {
    /// [`Self::mutate_workspace`] for a caller with nothing to add on failure:
    /// the banner already reports a save that did not happen.
    pub fn edit_workspace(&self, f: impl FnOnce(&mut Workspace)) {
        let _ = self.mutate_workspace(f);
    }

    /// [`Self::mutate_holdings`] for a caller with nothing to add on failure.
    pub fn edit_holdings(&self, f: impl FnOnce(&mut wallet_core::Holdings)) {
        let _ = self.mutate_holdings(f);
    }

    /// Address book, cluster, and mode — always available. A failed save rolls
    /// the change back and records it on `save_error`.
    pub fn mutate_workspace(&self, f: impl FnOnce(&mut Workspace)) -> Result<(), VaultError> {
        let saved = self.command(|core| core.workspace_checked("workspace", f));
        self.report_save(saved)
    }

    /// Token-account cache. A failed save rolls the change back.
    pub fn mutate_holdings(
        &self,
        f: impl FnOnce(&mut wallet_core::Holdings),
    ) -> Result<(), VaultError> {
        let saved = self.command(|core| core.holdings_checked("holdings", f));
        self.report_save(saved)
    }

    /// The custom cluster typed last, kept while a public one is in use.
    pub fn custom_cluster(&self) -> Option<Cluster> {
        // The network slice moves with the workspace, so reading it makes a
        // screen follow the cluster that was last typed.
        let _ = self.network.read();
        self.core()
            .with_workspace(|workspace| workspace.custom_cluster.clone())
    }

    /// The label to show for an address, when something names it.
    pub fn label_of(&self, pubkey: &Pubkey) -> Option<String> {
        (self.directory)().label_of(pubkey)
    }

    /// What to call an address, and what it is.
    pub fn describe(&self, pubkey: &Pubkey) -> wallet_core::Described {
        (self.directory)().describe(pubkey)
    }

    pub fn holding_summary(&self, owner: &Pubkey) -> HoldingSummary {
        (self.holdings_view)().summary(owner)
    }

    pub fn holding_page(&self, owner: &Pubkey, offset: usize, limit: usize) -> Vec<TokenHolding> {
        (self.holdings_view)().page(owner, offset, limit)
    }

    pub fn search_holdings(&self, query: &str, limit: usize) -> Vec<TokenHolding> {
        (self.holdings_view)().search(query, limit)
    }

    pub fn flag_of(&self, account: &Pubkey) -> Option<TokenFlag> {
        (self.holdings_view)().flag_of(account)
    }

    /// Write balances and nonces into the book, and token accounts into holdings.
    /// Only owners whose token list was fetched have their holdings replaced.
    pub fn apply_chain_snapshot(&self, snap: &ChainSnapshot) -> Result<(), VaultError> {
        self.mutate_workspace(|ws| ws.book.apply_snapshot(snap))?;
        self.mutate_holdings(|holdings| holdings.replace_owners(&snap.token_owners, &snap.tokens))
    }

    /// Owners/contacts and known nonce accounts to refresh from chain.
    pub fn refresh_pubkeys(&self) -> (Vec<Pubkey>, Vec<Pubkey>) {
        self.track_wallets();
        let book = (self.book)();
        self.core()
            .with_wallets(|data| wallet_core::refresh_targets(data, &book))
    }
}
