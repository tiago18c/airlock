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

/// The one transaction the Sign screen is working on.
#[derive(Clone, Copy)]
pub struct SignForm {
    pub wire: Signal<Option<Vec<u8>>>,
    pub source: Signal<Option<TxSource>>,
    /// Log trail id for the transaction on screen. Set when it is received.
    pub flow: Signal<Option<u64>>,
    pub context: Signal<Option<wallet_core::protocol::TxContext>>,
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
    /// A rebuild for a keyless wallet is running.
    pub rebuilding: Signal<bool>,
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

    /// Put `wire` on the screen in place of whatever was there.
    pub fn load(&self, wire: Vec<u8>, from: TxSource, note: Option<wallet_core::Phrase>) {
        self.load_with_lifetime(wire, from, note, None, wallet_core::LookupTables::new());
    }

    /// Like [`load`](Self::load), recording when the blockhash expires when
    /// that is known, and lookup tables when the wire is v0.
    pub fn load_with_lifetime(
        &self,
        wire: Vec<u8>,
        from: TxSource,
        note: Option<wallet_core::Phrase>,
        expiry: Option<BlockhashExpiry>,
        tables: wallet_core::LookupTables,
    ) {
        self.load_prepared(wire, from, note, expiry, tables, Vec::new());
    }

    /// Like [`load_with_lifetime`](Self::load_with_lifetime), with what a
    /// rebuild already read of the Vector accounts.
    pub fn load_prepared(
        &self,
        wire: Vec<u8>,
        from: TxSource,
        note: Option<wallet_core::Phrase>,
        expiry: Option<BlockhashExpiry>,
        tables: wallet_core::LookupTables,
        vector: Vec<wallet_core::relay::VectorContext>,
    ) {
        self.load_prepared_flow(wire, from, note, expiry, tables, vector, None);
    }

    /// Like [`load_prepared`](Self::load_prepared), reusing a flow id when the
    /// transaction was already logged (browser prepare, rebuild, convert).
    #[allow(clippy::too_many_arguments)]
    pub fn load_prepared_flow(
        &self,
        wire: Vec<u8>,
        from: TxSource,
        note: Option<wallet_core::Phrase>,
        expiry: Option<BlockhashExpiry>,
        tables: wallet_core::LookupTables,
        vector: Vec<wallet_core::relay::VectorContext>,
        existing_flow: Option<u64>,
    ) {
        let flow = existing_flow.unwrap_or_else(|| {
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
        let mut this = *self;
        this.wire.set(Some(wire));
        this.source.set(Some(from));
        this.flow.set(Some(flow));
        this.context.set(None);
        this.vector.set(vector);
        this.import.set(String::new());
        this.error.set(None);
        this.note.set(note);
        this.simulation.set(SimState::Idle);
        this.simulated_for.set(None);
        this.tables.set(tables);
        this.clock.reset(expiry);
        this.rebuilt.set(false);
        this.returned.set(false);
        this.batch.set(None);
    }

    /// Put a site's batch on the screen, on its transaction `batch.current`.
    pub fn load_batch(&self, batch: BrowserBatch) {
        let Some(item) = batch.items.get(batch.current).cloned() else {
            return;
        };
        self.load_prepared_flow(
            item.wire,
            batch.source(),
            item.note,
            item.expiry,
            item.tables,
            item.vector,
            Some(item.flow),
        );
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
        let mut this = *self;
        this.wire.set(None);
        this.source.set(None);
        this.flow.set(None);
        this.context.set(None);
        this.vector.set(Vec::new());
        this.import.set(String::new());
        this.error.set(None);
        this.note.set(None);
        this.simulation.set(SimState::Idle);
        this.simulated_for.set(None);
        this.share.set(ShareTab::Everything);
        this.tables.set(wallet_core::LookupTables::new());
        this.clock.reset(None);
        this.rebuilt.set(false);
        this.returned.set(false);
        this.batch.set(None);
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
