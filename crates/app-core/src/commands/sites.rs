//! Browser requests and the sites that make them.

use std::sync::Arc;

use bridge_protocol::reply::{SendError, SiteAccount};
use solana_pubkey::Pubkey;
use tracing::{debug, debug_span, info};
use wallet_core::{
    connect_choice, current_first, reconnect_accounts, ShareableAccount, SiteGrant, VaultData,
    VaultError,
};

use super::{locked, Core, Inner, CORE};
use crate::bridge::{
    BridgeAnswer, BridgeRefused, BridgeReply, BridgeRequest, BridgeState, ConnectedSite,
    SendingRequest, SiteChoice, SiteConnect, SiteSign, SitesView, Taken,
};
use crate::events::Event;
use crate::notices::BridgeRequestOutcome;
use crate::slices::shown_wallets;
use crate::storage::AppWorkspace;

impl Core {
    /// Reply to a browser request with what the user decided. Only here, not
    /// on [`PublicCore`]: an answer is the user's approval. Answers whether
    /// the request was waiting and `answer` fits its method.
    pub fn bridge_answer(&self, id: &str, answer: &BridgeAnswer) -> bool {
        self.inner.bridge_answer(id, answer)
    }

    /// Answer a `connect` with the accounts the user chose, the current
    /// wallet moved first, and keep the choice: saved when `remember`, for as
    /// long as the app is open otherwise. Answers whether it was waiting.
    pub fn bridge_connect(
        &self,
        id: &str,
        accounts: Vec<ShareableAccount>,
        remember: bool,
    ) -> bool {
        self.inner.bridge_connect(id, accounts, remember)
    }

    /// Take a send the user approved out of the queue, so it cannot be
    /// rejected while it goes out. None when it is not waiting, is not a send,
    /// or does not have `count` transactions.
    pub fn bridge_begin_send(&self, id: &str, count: usize) -> Option<SendingRequest> {
        self.inner.bridge_begin_send(id, count)
    }

    /// Tell the site how its send went.
    pub fn bridge_send_finished(
        &self,
        sending: SendingRequest,
        results: Vec<Result<[u8; 64], SendError>>,
    ) {
        self.inner.bridge_send_finished(sending, results);
    }

    /// Answer a `signIn` with what `account` signed, and let the site see it,
    /// with what it could already: saved when `remember`, for as long as the
    /// app is open otherwise. Answers whether it was waiting.
    pub fn bridge_sign_in(
        &self,
        id: &str,
        account: Pubkey,
        signed: Vec<u8>,
        signature: [u8; 64],
        remember: bool,
    ) -> bool {
        self.inner
            .bridge_sign_in(id, account, signed, signature, remember)
    }

    /// Turn away the site that made request `id` from now on, and reject
    /// everything it has waiting. Answers whether it was waiting.
    pub fn bridge_block(&self, id: &str) -> bool {
        self.inner.bridge_block(id)
    }

    /// End `origin`'s connection. A remembered site keeps its grant and is
    /// let back in when it next asks to connect.
    pub fn site_disconnect(&self, origin: &str) {
        self.inner.site_disconnect(origin);
    }

    /// Forget what `origin` was given and disconnect it.
    pub fn site_revoke(&self, origin: &str) {
        self.inner.site_revoke(origin);
    }

    /// Turn `origin` away from now on, and reject what it has waiting.
    pub fn site_block(&self, origin: &str) {
        self.inner.site_block(origin);
    }

    pub fn site_unblock(&self, origin: &str) {
        self.inner.site_unblock(origin);
    }

    /// What the connect modal opens with for `origin`.
    pub fn site_choice(&self, origin: &str) -> SiteChoice {
        self.inner.site_choice(origin)
    }
}

impl Inner {
    pub(crate) fn bridge_push(
        &self,
        request: BridgeRequest,
        reply: BridgeReply,
    ) -> Result<(), BridgeRefused> {
        let _span = debug_span!(target: CORE, "command", name = "bridge_push").entered();
        let id = request.id.clone();
        {
            let mut bridge = locked(&self.bridge);
            if let Err(refused) = bridge.push(request, reply) {
                debug!(target: CORE, outcome = "refused", "command");
                return Err(refused);
            }
            self.slices.bridge.publish(Arc::new(bridge.queue().clone()));
        }
        let _ = self.events.send(Event::RequestArrived { id });
        debug!(target: CORE, outcome = "ok", "command");
        Ok(())
    }

    /// Reply to `id` with what the user decided. Answers whether the request
    /// was waiting and `answer` fits its method.
    pub(crate) fn bridge_answer(self: &Arc<Self>, id: &str, answer: &BridgeAnswer) -> bool {
        let _span = debug_span!(target: CORE, "command", name = "bridge_answer").entered();
        let Some(taken) = self.bridge_take(id, Some(answer)) else {
            debug!(target: CORE, outcome = "not waiting", "command");
            return false;
        };
        let (bytes, outcome) = answer.reply(&taken.request);
        if let Some(reply) = taken.reply {
            reply(bytes);
        }
        self.bridge_finished(&taken.request, outcome);
        true
    }

    pub(crate) fn bridge_begin_send(&self, id: &str, count: usize) -> Option<SendingRequest> {
        let _span = debug_span!(target: CORE, "command", name = "bridge_begin_send").entered();
        // Empty results only to test the fit: one for each transaction.
        let fits = BridgeAnswer::Sent(vec![Err(SendError::Failed(String::new())); count]);
        let taken = self.bridge_take(id, Some(&fits))?;
        Some(SendingRequest {
            request: taken.request,
            reply: taken.reply,
        })
    }

    pub(crate) fn bridge_send_finished(
        self: &Arc<Self>,
        sending: SendingRequest,
        results: Vec<Result<[u8; 64], SendError>>,
    ) {
        let _span = debug_span!(target: CORE, "command", name = "bridge_send_finished").entered();
        let answer = BridgeAnswer::Sent(results);
        let (bytes, outcome) = answer.reply(&sending.request);
        if let Some(reply) = sending.reply {
            reply(bytes);
        }
        self.bridge_finished(&sending.request, outcome);
    }

    /// Drop `id` unanswered: the transport stopped waiting. Answers whether
    /// it was waiting.
    pub(crate) fn bridge_expire(self: &Arc<Self>, id: &str) -> bool {
        let _span = debug_span!(target: CORE, "command", name = "bridge_expire").entered();
        let Some(taken) = self.bridge_take(id, None) else {
            debug!(target: CORE, outcome = "not waiting", "command");
            return false;
        };
        self.bridge_finished(&taken.request, BridgeRequestOutcome::Expired);
        true
    }

    /// The accounts a site can be shown, and the current wallet's address.
    fn site_accounts(&self) -> (Vec<ShareableAccount>, Option<Pubkey>) {
        let vault = locked(&self.vault);
        let list = locked(&self.wallet_list);
        let data = shown_wallets(&vault, &list);
        let current = data.and_then(VaultData::acting_address);
        let workspace = self.workspace.lock();
        (
            wallet_core::shareable_accounts(data, workspace.data()),
            current,
        )
    }

    /// What `origin` was given, remembered or for this run of the app.
    fn site_grant(&self, origin: &str) -> Option<SiteGrant> {
        let remembered = self
            .workspace
            .lock()
            .data()
            .connections
            .grant(origin)
            .cloned();
        remembered.or_else(|| locked(&self.bridge).session(origin).cloned())
    }

    fn site_is_blocked(&self, origin: &str) -> bool {
        self.workspace.lock().data().connections.is_blocked(origin)
    }

    pub(crate) fn site_connect(&self, origin: &str, silent: bool) -> SiteConnect {
        if self.site_is_blocked(origin) {
            return SiteConnect::Blocked;
        }
        let Some(grant) = self.site_grant(origin) else {
            return SiteConnect::Undecided;
        };
        // A site that disconnected stays out until it asks again.
        if grant.ended && silent {
            return SiteConnect::Undecided;
        }
        let (shareable, current) = self.site_accounts();
        let keys: Vec<Pubkey> = shareable.iter().map(|account| account.pubkey).collect();
        let Some(accounts) = reconnect_accounts(&grant.accounts, current, &keys) else {
            return SiteConnect::Undecided;
        };
        if grant.ended {
            self.set_site_ended(origin, false);
        }
        info!(target: "airlock::bridge", origin = %origin, silent, "site reconnected");
        locked(&self.bridge).set_holding(origin, accounts.clone());
        self.publish_sites();
        SiteConnect::Allowed(labelled(accounts, &shareable))
    }

    pub(crate) fn site_sign(&self, origin: &str, accounts: &[Pubkey]) -> SiteSign {
        if self.site_is_blocked(origin) {
            return SiteSign::Blocked;
        }
        match self.site_grant(origin) {
            Some(grant)
                if !grant.ended && accounts.iter().all(|key| grant.accounts.contains(key)) =>
            {
                // A page from before the app restarted still holds its accounts.
                let mut bridge = locked(&self.bridge);
                if !bridge.holding().contains_key(origin) {
                    bridge.set_holding(origin, grant.accounts);
                    drop(bridge);
                    self.publish_sites();
                }
                SiteSign::Allowed
            }
            _ => SiteSign::NotConnected,
        }
    }

    pub(crate) fn site_disconnected(&self, origin: &str) {
        self.end_site(origin);
        self.publish_sites();
        info!(target: "airlock::bridge", origin = %origin, "site disconnected");
    }

    pub(crate) fn site_disconnect(self: &Arc<Self>, origin: &str) {
        self.end_site(origin);
        self.reject_waiting(origin, true);
        self.publish_sites();
        info!(target: "airlock::bridge", origin = %origin, "site disconnected by the user");
    }

    pub(crate) fn site_revoke(self: &Arc<Self>, origin: &str) {
        locked(&self.bridge).end_session(origin);
        locked(&self.bridge).drop_holding(origin);
        let saved = self.workspace_checked("site_revoke", |workspace| {
            workspace.connections.forget(origin);
        });
        if let Err(err) = &saved {
            self.report_site_save(err);
        }
        self.reject_waiting(origin, true);
        self.publish_sites();
        info!(target: "airlock::bridge", origin = %origin, "site revoked");
    }

    pub(crate) fn site_block(self: &Arc<Self>, origin: &str) {
        let saved = self.workspace_checked("site_block", |workspace| {
            workspace.connections.block(origin);
        });
        if let Err(err) = &saved {
            self.report_site_save(err);
        }
        locked(&self.bridge).end_session(origin);
        locked(&self.bridge).drop_holding(origin);
        self.reject_waiting(origin, false);
        self.publish_sites();
        info!(target: "airlock::bridge", origin = %origin, "site blocked");
    }

    pub(crate) fn site_unblock(&self, origin: &str) {
        let saved = self.workspace_checked("site_unblock", |workspace| {
            workspace
                .connections
                .blocked
                .retain(|known| known != origin);
        });
        if let Err(err) = &saved {
            self.report_site_save(err);
        }
        self.publish_sites();
        info!(target: "airlock::bridge", origin = %origin, "site unblocked");
    }

    /// Reject what `origin` has waiting: all of it, or only what takes a
    /// connection.
    fn reject_waiting(self: &Arc<Self>, origin: &str, only_connected: bool) {
        let waiting = locked(&self.bridge).waiting_from(origin, only_connected);
        for id in waiting {
            self.bridge_answer(&id, &BridgeAnswer::Rejected);
        }
    }

    pub(crate) fn site_pages(&self, pages: std::collections::BTreeSet<String>) {
        locked(&self.bridge).set_pages(pages);
        self.publish_sites();
    }

    pub(crate) fn sites_sync(&self) {
        let (shareable, current) = self.site_accounts();
        let keys: Vec<Pubkey> = shareable.iter().map(|account| account.pubkey).collect();
        let holding = locked(&self.bridge).holding().clone();
        for (origin, held) in holding {
            let granted = self
                .site_grant(&origin)
                .map(|grant| grant.accounts)
                .unwrap_or_default();
            // A wallet the site was not given changes nothing, but one no
            // longer on this device cannot stay with it.
            let accounts = reconnect_accounts(&granted, current, &keys).unwrap_or_else(|| {
                held.iter()
                    .filter(|key| keys.contains(key))
                    .copied()
                    .collect()
            });
            if accounts != held {
                locked(&self.bridge).set_holding(&origin, accounts);
            }
        }
        self.publish_sites();
    }

    /// The labels of `keys`, for the bridge to tell a site.
    pub(crate) fn site_labels(&self, keys: Vec<Pubkey>) -> Vec<SiteAccount> {
        let (shareable, _) = self.site_accounts();
        labelled(keys, &shareable)
    }

    fn publish_sites(&self) {
        let workspace = self.workspace.lock();
        let bridge = locked(&self.bridge);
        let view = sites_view(&workspace, &bridge);
        self.slices.sites.publish(Arc::new(view));
    }

    /// Take `origin` off what it holds. A remembered site keeps its grant.
    fn end_site(&self, origin: &str) {
        locked(&self.bridge).end_session(origin);
        locked(&self.bridge).drop_holding(origin);
        let remembered = self
            .workspace
            .lock()
            .data()
            .connections
            .grant(origin)
            .is_some();
        if remembered {
            self.set_site_ended(origin, true);
        }
    }

    /// Saved now rather than deferred: this runs on the bridge's own thread,
    /// which cannot start the saver task.
    fn set_site_ended(&self, origin: &str, ended: bool) {
        let saved = self.workspace_checked("site_ended", |workspace| {
            workspace.connections.set_ended(origin, ended);
        });
        if let Err(err) = &saved {
            self.report_site_save(err);
        }
    }

    pub(crate) fn site_choice(&self, origin: &str) -> SiteChoice {
        let (shareable, current) = self.site_accounts();
        let keys: Vec<Pubkey> = shareable.iter().map(|account| account.pubkey).collect();
        let grant = self.site_grant(origin);
        let remembered = self
            .workspace
            .lock()
            .data()
            .connections
            .grant(origin)
            .is_some();
        SiteChoice {
            choice: connect_choice(
                &keys,
                current,
                grant.as_ref().map(|grant| grant.accounts.as_slice()),
            ),
            remembered,
        }
    }

    pub(crate) fn bridge_connect(
        self: &Arc<Self>,
        id: &str,
        accounts: Vec<ShareableAccount>,
        remember: bool,
    ) -> bool {
        let _span = debug_span!(target: CORE, "command", name = "bridge_connect").entered();
        let (_, current) = self.site_accounts();
        let keys: Vec<Pubkey> = accounts.iter().map(|account| account.pubkey).collect();
        let ordered = current_first(keys, current);
        let answer = BridgeAnswer::Connect(labelled(ordered.clone(), &accounts));
        let Some(taken) = self.bridge_take(id, Some(&answer)) else {
            debug!(target: CORE, outcome = "not waiting", "command");
            return false;
        };
        let origin = taken.request.origin.clone();
        // A request with no origin has nothing a later one could match.
        if !origin.is_empty() {
            locked(&self.bridge).set_holding(&origin, ordered.clone());
            self.keep_site(&origin, ordered, remember);
            self.publish_sites();
        }
        let (bytes, outcome) = answer.reply(&taken.request);
        if let Some(reply) = taken.reply {
            reply(bytes);
        }
        self.bridge_finished(&taken.request, outcome);
        true
    }

    pub(crate) fn bridge_sign_in(
        self: &Arc<Self>,
        id: &str,
        account: Pubkey,
        signed: Vec<u8>,
        signature: [u8; 64],
        remember: bool,
    ) -> bool {
        let _span = debug_span!(target: CORE, "command", name = "bridge_sign_in").entered();
        let Some(origin) = locked(&self.bridge)
            .queue()
            .get(id)
            .map(|request| request.origin.clone())
        else {
            debug!(target: CORE, outcome = "not waiting", "command");
            return false;
        };
        let (shareable, _) = self.site_accounts();
        // The account that signed in comes first: it is who the site now sees.
        let mut keys = vec![account];
        if let Some(grant) = self.site_grant(&origin) {
            keys.extend(grant.accounts.into_iter().filter(|key| *key != account));
        }
        let answer = BridgeAnswer::SignedIn {
            account: account.to_bytes(),
            accounts: labelled(keys.clone(), &shareable),
            signed,
            signature,
        };
        let Some(taken) = self.bridge_take(id, Some(&answer)) else {
            debug!(target: CORE, outcome = "not waiting", "command");
            return false;
        };
        if !origin.is_empty() {
            locked(&self.bridge).set_holding(&origin, keys.clone());
            self.keep_site(&origin, keys, remember);
            self.publish_sites();
        }
        let (bytes, outcome) = answer.reply(&taken.request);
        if let Some(reply) = taken.reply {
            reply(bytes);
        }
        self.bridge_finished(&taken.request, outcome);
        true
    }

    /// Keep what `origin` was given. When it cannot be saved the site still
    /// has it until the app closes, and the failure is reported.
    fn keep_site(&self, origin: &str, accounts: Vec<Pubkey>, remember: bool) {
        let was_remembered = self
            .workspace
            .lock()
            .data()
            .connections
            .grant(origin)
            .is_some();
        let saved = if remember {
            let kept = accounts.clone();
            self.workspace_checked("site_remember", move |workspace| {
                workspace.connections.remember(origin, kept);
            })
        } else if was_remembered {
            self.workspace_checked("site_forget", |workspace| {
                workspace.connections.forget(origin);
            })
        } else {
            Ok(())
        };
        let mut bridge = locked(&self.bridge);
        bridge.end_session(origin);
        if let Err(err) = &saved {
            self.report_site_save(err);
        }
        if !remember || saved.is_err() {
            bridge.set_session(origin, accounts);
        }
    }

    fn report_site_save(&self, err: &VaultError) {
        let _ = self.events.send(Event::SaveFailed {
            store: self.workspace.name(),
            error: err.to_string(),
        });
    }

    pub(crate) fn bridge_block(self: &Arc<Self>, id: &str) -> bool {
        let _span = debug_span!(target: CORE, "command", name = "bridge_block").entered();
        let Some(origin) = locked(&self.bridge)
            .queue()
            .get(id)
            .map(|request| request.origin.clone())
        else {
            debug!(target: CORE, outcome = "not waiting", "command");
            return false;
        };
        if origin.is_empty() {
            self.bridge_answer(id, &BridgeAnswer::Rejected);
        } else {
            self.site_block(&origin);
        }
        true
    }

    fn bridge_take(&self, id: &str, answer: Option<&BridgeAnswer>) -> Option<Taken> {
        let mut bridge = locked(&self.bridge);
        let taken = bridge.take(id, answer)?;
        self.slices.bridge.publish(Arc::new(bridge.queue().clone()));
        Some(taken)
    }

    fn bridge_finished(self: &Arc<Self>, request: &BridgeRequest, outcome: BridgeRequestOutcome) {
        info!(
            target: "airlock::bridge",
            code = %request.code(),
            method = %request.method(),
            origin = %request.origin,
            outcome = outcome.as_str(),
            "request finished"
        );
        self.notices_set_bridge_outcome(&request.id, &request.origin, request.method(), outcome);
        debug!(target: CORE, outcome = "ok", "command");
    }
}

pub(super) fn sites_view(workspace: &AppWorkspace, bridge: &BridgeState) -> SitesView {
    let connections = &workspace.data().connections;
    SitesView {
        connected: bridge
            .connected()
            .map(|(origin, accounts)| ConnectedSite {
                origin: origin.clone(),
                accounts: accounts.clone(),
                remembered: connections.grant(origin).is_some(),
            })
            .collect(),
        authorized: connections.sites.clone(),
        blocked: connections.blocked.clone(),
        holding: bridge.holding().clone(),
    }
}

/// `keys` as a site is sent them, each with the label it has here.
fn labelled(keys: Vec<Pubkey>, accounts: &[ShareableAccount]) -> Vec<SiteAccount> {
    keys.into_iter()
        .map(|key| {
            let known = accounts.iter().find(|account| account.pubkey == key);
            SiteAccount {
                pubkey: key.to_bytes(),
                label: known
                    .map(|account| account.label.clone())
                    .unwrap_or_default(),
                signs_messages: known.is_some_and(|account| !account.contract),
            }
        })
        .collect()
}
