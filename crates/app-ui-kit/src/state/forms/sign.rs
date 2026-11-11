//! The Sign screen: the one transaction it is working on.

use super::*;

/// Where the transaction on the Sign screen came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TxSource {
    Build,
    Wallets,
    Stake,
    /// A site asked for a signature through the browser extension.
    Browser {
        id: String,
        origin: String,
    },
    Pasted,
    Scanned,
}

/// How much of the transaction the Sign screen shows.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SignView {
    /// What it does, in sentences.
    Summary,
    /// Everything: limits, accounts, programs, fields and bytes.
    Inspect,
}

/// What the Share section shows.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ShareTab {
    /// The transaction, its note and every signature, in one transfer.
    Everything,
    Transaction,
    Signatures,
    Context,
}

/// What came of pressing Submit, shown over the transaction until the user
/// moves on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Submission {
    /// Being handed to the node.
    Sending,
    /// The node took it, or may have. Its notice says what became of it.
    Sent(solana_signature::Signature),
    /// The node refused it, so it went nowhere.
    Refused(String),
}

#[derive(Clone, PartialEq)]
pub enum SimState {
    /// Not asked: offline, or nothing loaded.
    Idle,
    /// Not asked yet, for the reason given.
    Waiting(String),
    Running,
    Done(wallet_core::simulate::Simulation),
    Failed(String),
}

#[derive(Clone, PartialEq)]
pub struct QrPreview {
    pub svg: String,
    pub index: u16,
    pub count: u16,
    pub version: u8,
    pub modules: usize,
    pub encoded_bytes: usize,
    pub transfer_id: String,
}

/// What came of a signed transaction given back to the site that asked for
/// it, which may send it itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SiteSend {
    /// The node is being watched for it. Submit waits.
    Watching,
    /// The site sent it and it landed: there is nothing left to submit.
    Landed,
    /// It did not land while it was watched. It can be submitted here.
    NotSent,
}

/// One transaction of a batch a site sent, as it stood when the screen last
/// showed it.
#[derive(Clone, Debug, PartialEq)]
pub struct BatchItem {
    pub wire: Vec<u8>,
    pub tables: wallet_core::LookupTables,
    pub vector: Vec<wallet_core::relay::VectorContext>,
    pub expiry: Option<BlockhashExpiry>,
    pub flow: u64,
    pub note: Option<wallet_core::Phrase>,
    /// This device has signed what it can of it. It waits here for the rest
    /// of the batch.
    pub signed: bool,
}

/// The transactions a site asked for in one request, taken one at a time on
/// the Sign screen. None goes back until every one is signed: the site gets
/// them all at once.
#[derive(Clone, Debug, PartialEq)]
pub struct BrowserBatch {
    pub id: String,
    pub origin: String,
    pub items: Vec<BatchItem>,
    /// The one on screen.
    pub current: usize,
}

impl BrowserBatch {
    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn signed(&self) -> usize {
        self.items.iter().filter(|item| item.signed).count()
    }

    pub fn complete(&self) -> bool {
        self.items.iter().all(|item| item.signed)
    }

    /// The first transaction still to sign after the one on screen, going
    /// round to the start.
    pub fn next_unsigned(&self) -> Option<usize> {
        let count = self.items.len();
        (1..=count)
            .map(|step| (self.current + step) % count)
            .find(|&index| !self.items[index].signed)
    }

    /// The source the Sign screen gives each of its transactions.
    pub fn source(&self) -> TxSource {
        TxSource::Browser {
            id: self.id.clone(),
            origin: self.origin.clone(),
        }
    }
}

/// A transaction for the Sign screen, with what is known of it already.
#[derive(Clone, Debug)]
pub struct SignLoad {
    wire: Vec<u8>,
    from: TxSource,
    note: Option<wallet_core::Phrase>,
    expiry: Option<BlockhashExpiry>,
    tables: wallet_core::LookupTables,
    vector: Vec<wallet_core::relay::VectorContext>,
    flow: Option<u64>,
}

impl SignLoad {
    pub fn new(wire: Vec<u8>, from: TxSource) -> Self {
        Self {
            wire,
            from,
            note: None,
            expiry: None,
            tables: wallet_core::LookupTables::new(),
            vector: Vec::new(),
            flow: None,
        }
    }

    /// What to say above it. An empty note says nothing.
    pub fn with_note(mut self, note: impl Into<wallet_core::Phrase>) -> Self {
        self.note = Some(note.into()).filter(|note| !note.plain().is_empty());
        self
    }

    /// When its blockhash stops being accepted, when that is known.
    pub fn with_expiry(mut self, expiry: Option<BlockhashExpiry>) -> Self {
        self.expiry = expiry;
        self
    }

    /// This app chose its blockhash, which expires at `height` when known.
    pub fn expiring_at(self, height: Option<u64>) -> Self {
        self.with_expiry(height.map(BlockhashExpiry::exact))
    }

    /// The lookup tables of a v0 wire.
    pub fn with_tables(mut self, tables: wallet_core::LookupTables) -> Self {
        self.tables = tables;
        self
    }

    /// What a rebuild already read of its Vector accounts.
    pub fn with_vector(mut self, vector: Vec<wallet_core::relay::VectorContext>) -> Self {
        self.vector = vector;
        self
    }

    /// Its log flow, when it was logged already (a browser prepare, a rebuild,
    /// a conversion).
    pub fn with_flow(mut self, flow: u64) -> Self {
        self.flow = Some(flow);
        self
    }

    /// The site's request it answers, when a site asked for it.
    pub fn request_id(&self) -> Option<&str> {
        match &self.from {
            TxSource::Browser { id, .. } => Some(id),
            _ => None,
        }
    }
}

/// A site's request that something else took the Sign screen from, as it
/// stood then: its transaction with every signature added to it, its batch
/// and what the screen knew of it. Opened again, it comes back as it was.
#[derive(Clone, Debug, PartialEq)]
pub struct SetAside {
    pub id: String,
    pub origin: String,
    pub wire: Vec<u8>,
    pub note: Option<wallet_core::Phrase>,
    pub expiry: Option<BlockhashExpiry>,
    pub tables: wallet_core::LookupTables,
    pub vector: Vec<wallet_core::relay::VectorContext>,
    pub flow: Option<u64>,
    pub batch: Option<BrowserBatch>,
}

/// The one transaction the Sign screen is working on.
#[derive(Clone, Copy)]
pub struct SignForm {
    pub wire: Signal<Option<Vec<u8>>>,
    /// Goes up each time the transaction on screen is replaced, or something is
    /// on its way to replace it. A task that takes time remembers it when it
    /// starts, and does nothing with the screen if it has changed: see
    /// [`SignForm::still_on_screen`].
    pub generation: Signal<u64>,
    pub source: Signal<Option<TxSource>>,
    /// Log trail id for the transaction on screen. Set when it is received.
    pub flow: Signal<Option<u64>>,
    pub context: Signal<Option<wallet_core::protocol::TxContext>>,
    /// What a node said is stored behind what the transaction says yes to, by
    /// address. `None` until it has answered. Empty when it had nothing, or
    /// could not be asked.
    pub stored_behind: Signal<Option<std::collections::HashMap<Pubkey, Vec<u8>>>>,
    /// The message whose approvals the user said they have checked elsewhere.
    pub blind_checked: Signal<Option<[u8; 32]>>,
    /// What is known of each Vector account the transaction's `Advance`
    /// lists: from a rebuild, a context note, or both. Signing and placing
    /// their signatures need it.
    pub vector: Signal<Vec<wallet_core::relay::VectorContext>>,
    /// The paste box. It takes a transaction, a signature or a context note.
    pub import: Signal<String>,
    pub error: Signal<Option<String>>,
    pub note: Signal<Option<wallet_core::Phrase>>,
    pub view: Signal<SignView>,
    pub share: Signal<ShareTab>,
    pub simulation: Signal<SimState>,
    /// Checksum of the message `simulation` is about.
    pub simulated_for: Signal<Option<[u8; 32]>>,
    /// Lookup tables for a loaded v0 wire. Empty for legacy and v1.
    pub tables: Signal<wallet_core::LookupTables>,
    pub clock: BlockhashClock,
    pub last_signature: Signal<Option<String>>,
    /// What came of the last press of Submit, while it is on screen.
    pub submission: Signal<Option<Submission>>,
    /// A rebuild for a keyless wallet, or one that adds what a Squads
    /// proposal holds, is running.
    pub rebuilding: Signal<bool>,
    /// Why what a proposal holds could not be added to the message with this
    /// checksum. See `actions::sign_screen::add_checks_loaded`.
    pub checks_refused: Signal<Option<([u8; 32], String)>>,
    /// Checksum of the message last rebuilt without being asked, so a
    /// failed attempt is not repeated. See `SignPanel`.
    pub auto_rebuilt_for: Signal<Option<[u8; 32]>>,
    /// The transaction on screen is one this screen rebuilt for a keyless
    /// wallet, so the steps keep the rebuild behind it.
    pub rebuilt: Signal<bool>,
    /// The transaction on screen went back to the site that asked for it.
    pub returned: Signal<bool>,
    /// The batch the transaction on screen belongs to, when a site asked for
    /// several in one request.
    pub batch: Signal<Option<BrowserBatch>>,
    /// Keys made here in place of one-time keys a site signed with or left
    /// for itself, such as a new mint. Like Build's generated keys, they
    /// live in memory only, until the app closes, and sign as keys of this
    /// device: a transaction can wait for an authorization, or come back
    /// from a request modal, long after it was prepared.
    pub made_here: Signal<Vec<MadeHere>>,
    /// The accounts each site transaction creates for its signers, by the
    /// checksum of its message, as preparing it found them: a program can
    /// create an account by keypair inside its own calls, which only a
    /// simulation shows. Kept in memory, like `made_here`.
    pub created: Signal<Vec<([u8; 32], Vec<Pubkey>)>>,
    /// What came of each transaction given back to a site, by its
    /// signature. Kept in memory, like `made_here`, so it holds for every
    /// transaction of a batch.
    pub site_sends: Signal<Vec<(solana_signature::Signature, SiteSend)>>,
    /// Sites' requests something else took the screen from, kept until they
    /// are opened again or leave the queue. See [`SetAside`].
    pub set_aside: Signal<Vec<SetAside>>,
    /// The site and request id the screen last set aside, for the note that
    /// says so, until that request is on screen again.
    pub displaced: Signal<Option<(String, String)>>,
}

/// A key made here, and the site's one-time key it takes the place of.
#[derive(Clone)]
pub struct MadeHere {
    pub replaces: Pubkey,
    pub key: GeneratedKey,
}

/// When a blockhash stops being accepted, as far as this app knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockhashExpiry {
    pub last_valid_height: u64,
    /// The node gave the height along with the blockhash. Otherwise it is an
    /// estimate for a blockhash someone else chose.
    pub exact: bool,
}

impl BlockhashExpiry {
    pub fn exact(last_valid_height: u64) -> Self {
        Self {
            last_valid_height,
            exact: true,
        }
    }
}

/// A blockhash countdown: when the hash expires and what the node said last.
/// The Sign screen and the request modal each keep one.
#[derive(Clone, Copy, PartialEq)]
pub struct BlockhashClock {
    pub expiry: Signal<Option<BlockhashExpiry>>,
    /// Chain height last read, and the Unix second it was read at.
    pub height: Signal<Option<(u64, i64)>>,
    /// Whether the node still accepts the blockhash, as it said last.
    pub valid: Signal<Option<bool>>,
}

impl BlockhashClock {
    pub fn reset(&self, expiry: Option<BlockhashExpiry>) {
        let mut this = *self;
        this.expiry.set(expiry);
        this.height.set(None);
        this.valid.set(None);
    }

    /// The countdown at Unix second `now`, and whether its expiry is exact.
    /// None until the node has been asked for the height.
    pub fn reading(&self, now: i64) -> Option<(wallet_core::ClockReading, bool)> {
        let expiry = (self.expiry)()?;
        let (height, read_at) = (self.height)()?;
        let elapsed = (now - read_at).max(0) as f64;
        let reading = wallet_core::read_clock(
            expiry.last_valid_height,
            height,
            elapsed,
            expiry.exact,
            (self.valid)(),
        );
        Some((reading, expiry.exact))
    }

    /// Whether the node has said the blockhash is gone.
    pub fn expired(&self, now: i64) -> bool {
        self.reading(now)
            .is_some_and(|(reading, _)| reading.expired)
            || (self.valid)() == Some(false)
    }
}

/// A clock with nothing known yet.
pub fn use_blockhash_clock_state() -> BlockhashClock {
    BlockhashClock {
        expiry: use_signal(|| None),
        height: use_signal(|| None),
        valid: use_signal(|| None),
    }
}

impl SignForm {
    /// The site's key and the key made here, for each key made here that
    /// signs `tx`.
    pub fn made_here_in(&self, tx: &wallet_core::DecodedTx) -> Vec<(Pubkey, Pubkey)> {
        self.made_here
            .peek()
            .iter()
            .filter(|made| tx.signers.iter().any(|slot| slot.pubkey == made.key.pubkey))
            .map(|made| (made.replaces, made.key.pubkey))
            .collect()
    }

    /// Remember `created`, the accounts the site's `tx` creates (see
    /// [`wallet_flows::request::accounts_created`]), for the screens that
    /// show it later.
    pub fn keep_created(&self, tx: &wallet_core::DecodedTx, created: Vec<Pubkey>) {
        let message = wallet_core::sha256_bytes(&tx.message);
        let mut kept = self.created;
        kept.with_mut(|kept| {
            kept.retain(|(other, _)| *other != message);
            kept.push((message, created));
        });
    }

    /// The accounts `tx` creates for its signers: what preparing it found,
    /// or else what its own instructions create.
    pub fn created_in(&self, tx: &wallet_core::DecodedTx) -> Vec<Pubkey> {
        let message = wallet_core::sha256_bytes(&tx.message);
        self.created
            .peek()
            .iter()
            .find(|(other, _)| *other == message)
            .map(|(_, created)| created.clone())
            .unwrap_or_else(|| wallet_flows::request::created_accounts(tx, None))
    }

    /// What came of `wire` after it went back to a site, when it did.
    pub fn site_send(&self, wire: &[u8]) -> Option<SiteSend> {
        let signature = wallet_core::parse_wire(wire).ok()?.signature()?;
        (self.site_sends)()
            .iter()
            .find(|(other, _)| *other == signature)
            .map(|(_, send)| *send)
    }

    /// Record `send` for the transaction signed `signature`.
    pub fn set_site_send(&self, signature: solana_signature::Signature, send: SiteSend) {
        let mut sends = self.site_sends;
        sends.with_mut(|sends| {
            sends.retain(|(other, _)| *other != signature);
            sends.push((signature, send));
        });
    }

    /// Keep the site's request on screen, as it stands, when `next` is about
    /// to take its place: anything but another transaction of the same
    /// request. A request already answered keeps nothing.
    fn set_aside_for(&self, next: &TxSource) {
        let Some(TxSource::Browser { id, origin }) = self.source.peek().clone() else {
            return;
        };
        if matches!(next, TxSource::Browser { id: next, .. } if *next == id) {
            return;
        }
        if *self.returned.peek() {
            return;
        }
        let Some(wire) = self.wire.peek().clone() else {
            return;
        };
        self.save_batch_item();
        let kept = SetAside {
            id: id.clone(),
            origin: origin.clone(),
            wire,
            note: self.note.peek().clone(),
            expiry: *self.clock.expiry.peek(),
            tables: self.tables.peek().clone(),
            vector: self.vector.peek().clone(),
            flow: *self.flow.peek(),
            batch: self.batch.peek().clone(),
        };
        let mut set_aside = self.set_aside;
        set_aside.with_mut(|set_aside| {
            set_aside.retain(|other| other.id != id);
            set_aside.push(kept);
        });
        let mut displaced = self.displaced;
        displaced.set(Some((id, origin)));
    }

    /// The request `id` as it was set aside, taken out of what is kept.
    pub fn take_set_aside(&self, id: &str) -> Option<SetAside> {
        let mut set_aside = self.set_aside;
        let mut taken = None;
        set_aside.with_mut(|set_aside| {
            if let Some(at) = set_aside.iter().position(|kept| kept.id == id) {
                taken = Some(set_aside.remove(at));
            }
        });
        let mut displaced = self.displaced;
        if displaced
            .peek()
            .as_ref()
            .is_some_and(|(other, _)| other == id)
        {
            displaced.set(None);
        }
        taken
    }

    /// Forget what was set aside for requests no longer `waiting`.
    pub fn forget_set_aside(&self, waiting: impl Fn(&str) -> bool) {
        let mut set_aside = self.set_aside;
        if set_aside.peek().iter().any(|kept| !waiting(&kept.id)) {
            set_aside.with_mut(|set_aside| set_aside.retain(|kept| waiting(&kept.id)));
        }
        let mut displaced = self.displaced;
        if displaced
            .peek()
            .as_ref()
            .is_some_and(|(id, _)| !waiting(id))
        {
            displaced.set(None);
        }
    }

    /// Whether replacing the screen would throw away signatures this app
    /// cannot put back: a transaction of the app's own, or one pasted or
    /// scanned, with a signature on it already and not sent. A site's
    /// request is set aside instead (see [`SetAside`]).
    pub fn holds_signed_work(&self) -> bool {
        if matches!(*self.source.peek(), None | Some(TxSource::Browser { .. })) {
            return false;
        }
        if matches!(
            *self.submission.peek(),
            Some(Submission::Sending | Submission::Sent(_))
        ) {
            return false;
        }
        self.wire
            .peek()
            .as_deref()
            .and_then(|wire| wallet_core::parse_wire(wire).ok())
            .is_some_and(|view| view.valid_signatures() > 0)
    }

    /// Which transaction is on screen, for a task about to take time.
    pub fn generation(&self) -> u64 {
        *self.generation.peek()
    }

    /// Whether what was on screen at `generation` still is. Nothing else may
    /// write to the screen, or answer for what is on it. The transaction may
    /// have changed since without this changing, by a signature or a rebuild.
    pub fn still_on_screen(&self, generation: u64) -> bool {
        self.generation() == generation
    }

    /// Something is on its way to replace the transaction on screen, so what is
    /// under way for that one no longer counts. Answers the generation of what
    /// is coming, which is dropped in turn if something newer overtakes it.
    pub fn begin_replacing(&self) -> u64 {
        let next = self.generation() + 1;
        let mut generation = self.generation;
        generation.set(next);
        next
    }

    /// Show `why`, if what it is about is still on screen.
    pub fn report(&self, generation: u64, why: String) {
        if self.still_on_screen(generation) {
            let mut error = self.error;
            error.set(Some(why));
        }
    }

    /// Put a transaction on the screen in place of whatever was there.
    pub fn load(&self, load: SignLoad) {
        let SignLoad {
            wire,
            from,
            note,
            expiry,
            tables,
            vector,
            flow,
        } = load;
        let flow = flow.unwrap_or_else(|| {
            let flow = crate::diagnostics::next_flow_id();
            let (origin, code): (Option<&str>, Option<String>) = match &from {
                TxSource::Browser { id, origin } => (
                    Some(origin.as_str()),
                    Some(bridge_protocol::request_code(id)),
                ),
                _ => (None, None),
            };
            let code_ref = code.as_deref();
            crate::diagnostics::TxLog {
                flow,
                wire: &wire,
                cluster: None,
            }
            .log_received(crate::diagnostics::source_name(&from), origin, code_ref);
            flow
        });
        self.set_aside_for(&from);
        self.reset();
        let mut this = *self;
        this.wire.set(Some(wire));
        this.source.set(Some(from));
        this.flow.set(Some(flow));
        this.vector.set(vector);
        this.note.set(note);
        this.tables.set(tables);
        this.clock.reset(expiry);
    }

    /// Put a site's batch on the screen, on its transaction `batch.current`.
    pub fn load_batch(&self, batch: BrowserBatch) {
        let Some(item) = batch.items.get(batch.current).cloned() else {
            return;
        };
        self.load(SignLoad {
            wire: item.wire,
            from: batch.source(),
            note: item.note,
            expiry: item.expiry,
            tables: item.tables,
            vector: item.vector,
            flow: Some(item.flow),
        });
        let mut this = *self;
        this.batch.set(Some(batch));
    }

    /// Keep what the screen did to the batch transaction on it: signatures
    /// added, a new blockhash, what is known of its Vector accounts.
    pub fn save_batch_item(&self) {
        let wire = self.wire.peek().clone();
        let vector = self.vector.peek().clone();
        let expiry = *self.clock.expiry.peek();
        let mut batch = self.batch;
        batch.with_mut(|batch| {
            let Some(batch) = batch else {
                return;
            };
            let current = batch.current;
            if let (Some(item), Some(wire)) = (batch.items.get_mut(current), wire) {
                item.wire = wire;
                item.vector = vector;
                item.expiry = expiry;
            }
        });
    }

    /// Show transaction `index` of the batch on screen, keeping the one it
    /// replaces as it stands.
    pub fn show_batch_item(&self, index: usize) {
        self.save_batch_item();
        let Some(mut batch) = self.batch.peek().clone() else {
            return;
        };
        if index >= batch.len() || index == batch.current {
            return;
        }
        batch.current = index;
        self.load_batch(batch);
    }

    /// Keep what a rebuild or a context note said of the Vector accounts,
    /// merging a note over a rebuild so a nonce from the chain wins when
    /// both speak of the same account.
    pub fn set_vector_context(&self, known: Vec<wallet_core::relay::VectorContext>) {
        let mut this = *self;
        this.vector.set(known);
    }

    /// This app moved the transaction to a blockhash that expires at `height`.
    pub fn set_exact_blockhash_height(&self, height: Option<u64>) {
        self.clock.reset(height.map(BlockhashExpiry::exact));
    }

    pub fn clear(&self) {
        self.reset();
        let mut this = *self;
        this.wire.set(None);
        this.source.set(None);
        this.flow.set(None);
        this.share.set(ShareTab::Everything);
    }

    /// Forget what is known of the transaction on screen, and so whatever is
    /// under way for it.
    fn reset(&self) {
        self.begin_replacing();
        let mut this = *self;
        this.context.set(None);
        this.stored_behind.set(None);
        this.blind_checked.set(None);
        this.vector.set(Vec::new());
        this.import.set(String::new());
        this.error.set(None);
        this.note.set(None);
        this.simulation.set(SimState::Idle);
        this.simulated_for.set(None);
        this.tables.set(wallet_core::LookupTables::new());
        this.clock.reset(None);
        this.rebuilt.set(false);
        this.checks_refused.set(None);
        this.returned.set(false);
        this.batch.set(None);
        this.submission.set(None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn batch(signed: &[bool], current: usize) -> BrowserBatch {
        BrowserBatch {
            id: "b".into(),
            origin: "https://example.test".into(),
            items: signed
                .iter()
                .map(|&signed| BatchItem {
                    wire: vec![0x81],
                    tables: wallet_core::LookupTables::new(),
                    vector: Vec::new(),
                    expiry: None,
                    flow: 0,
                    note: None,
                    signed,
                })
                .collect(),
            current,
        }
    }

    #[test]
    fn the_next_transaction_is_the_first_unsigned_after_the_one_on_screen() {
        assert_eq!(batch(&[true, false, false], 0).next_unsigned(), Some(1));
        // Going round to the start, past those already signed.
        assert_eq!(batch(&[false, true, true], 2).next_unsigned(), Some(0));
        assert_eq!(batch(&[true, true, true], 1).next_unsigned(), None);
        assert!(batch(&[true, true], 0).complete());
        assert_eq!(batch(&[true, false, true], 0).signed(), 2);
    }
}
