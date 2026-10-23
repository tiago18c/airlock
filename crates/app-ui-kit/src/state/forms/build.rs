//! The Build screen: its instruction blocks and transaction settings.

use super::*;

/// One instruction block on the build screen.
#[derive(Clone, PartialEq)]
pub struct IxDraft {
    pub id: u64,
    pub body: IxBody,
}

/// What a block is written as.
#[derive(Clone, PartialEq)]
pub enum IxBody {
    /// Filled in from an IDL.
    Idl(IdlDraft),
    /// A program, its accounts and its bytes, written by hand.
    Raw(RawDraft),
    /// An instruction that carries others, such as a Squads stored
    /// transaction. Each of those is a block of its own, and the carrier is
    /// written again around them.
    Carried {
        carrier: Carrier,
        inner: Vec<IxDraft>,
        /// The block open inside it.
        selected: Option<u64>,
    },
}

impl IxDraft {
    pub fn idl(&self) -> Option<&IdlDraft> {
        match &self.body {
            IxBody::Idl(idl) => Some(idl),
            _ => None,
        }
    }

    /// The pinned tab's name, the program's own name from its IDL, or what
    /// carries the blocks inside.
    pub fn program_label(&self) -> String {
        let program_id = match &self.body {
            IxBody::Idl(idl) => idl.program_id,
            IxBody::Raw(raw) => raw.program_id,
            IxBody::Carried { carrier, .. } => return carrier.label().to_string(),
        };
        if let Some(pinned) = program_id.and_then(|id| pinned_program(&id)) {
            return pinned.label.to_string();
        }
        match (&self.body, program_id) {
            (IxBody::Idl(idl), _) if !idl.program_name.is_empty() => idl.program_name.clone(),
            (IxBody::Raw(_), Some(id)) => short_address(&id),
            _ => "Program".to_string(),
        }
    }

    pub fn instruction_label(&self) -> String {
        match &self.body {
            IxBody::Idl(idl) if idl.instruction.is_empty() => "Choose an instruction".to_string(),
            IxBody::Idl(idl) => idl.instruction.clone(),
            IxBody::Raw(raw) => {
                let hex: String = raw.data.split_whitespace().collect();
                format!("Raw · {} bytes", hex.trim_start_matches("0x").len() / 2)
            }
            IxBody::Carried { carrier, inner, .. } => {
                format!("{} · {} inside", carrier.instruction_label(), inner.len())
            }
        }
    }
}

fn short_address(address: &Pubkey) -> String {
    let text = address.to_string();
    format!("{}…{}", &text[..4], &text[text.len() - 4..])
}

fn find_mut(list: &mut [IxDraft], id: u64) -> Option<&mut IxDraft> {
    for draft in list.iter_mut() {
        if draft.id == id {
            return Some(draft);
        }
        if let IxBody::Carried { inner, .. } = &mut draft.body {
            if let Some(found) = find_mut(inner, id) {
                return Some(found);
            }
        }
    }
    None
}

fn find_ref(list: &[IxDraft], id: u64) -> Option<&IxDraft> {
    list.iter().find_map(|draft| {
        if draft.id == id {
            return Some(draft);
        }
        match &draft.body {
            IxBody::Carried { inner, .. } => find_ref(inner, id),
            _ => None,
        }
    })
}

/// Take block `id` out of whichever carrier holds it, and open its
/// neighbour there. Whether it was found.
fn remove_nested(list: &mut [IxDraft], id: u64) -> bool {
    for draft in list.iter_mut() {
        let IxBody::Carried {
            inner, selected, ..
        } = &mut draft.body
        else {
            continue;
        };
        if inner.iter().any(|d| d.id == id) {
            inner.retain(|d| d.id != id);
            if *selected == Some(id) {
                *selected = inner.first().map(|d| d.id);
            }
            return true;
        }
        if remove_nested(inner, id) {
            return true;
        }
    }
    false
}

/// A keypair made on Build for an account a transaction creates. It lives
/// in memory only, until the app closes, and signs on Sign like a key of
/// this device. Build's Clear leaves it, since a transaction on Sign may
/// still need it.
#[derive(Clone)]
pub struct GeneratedKey {
    pub pubkey: Pubkey,
    seed: zeroize::Zeroizing<[u8; 32]>,
}

impl GeneratedKey {
    pub fn new() -> Self {
        let signer = wallet_core::KeypairSigner::new_random();
        Self {
            pubkey: wallet_core::WalletSigner::pubkey(&signer),
            seed: zeroize::Zeroizing::new(signer.seed_bytes()),
        }
    }

    /// The same key as `signer`, kept the way this form keeps one.
    pub fn from_signer(signer: &wallet_core::KeypairSigner) -> Self {
        Self {
            pubkey: wallet_core::WalletSigner::pubkey(signer),
            seed: zeroize::Zeroizing::new(signer.seed_bytes()),
        }
    }

    pub fn signer(&self) -> wallet_core::KeypairSigner {
        wallet_core::KeypairSigner::from_seed(self.seed.as_ref())
            .expect("a stored seed is 32 bytes")
    }
}

impl Default for GeneratedKey {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy)]
pub struct BuildForm {
    pub drafts: Signal<Vec<IxDraft>>,
    pub selected: Signal<Option<u64>>,
    pub next_id: Signal<u64>,
    pub fee_payer: Signal<Option<Pubkey>>,
    /// The wallet whose fee payer `fee_payer` was last defaulted from.
    pub payer_for: Signal<Option<wallet_core::WalletId>>,
    /// Bumped by `clear` so address pickers drop text that is not in `selected`.
    pub picker_reset: Signal<u64>,
    pub advanced_open: Signal<bool>,
    pub cu_limit: Signal<String>,
    pub cu_price: Signal<String>,
    pub loaded_accounts: Signal<String>,
    pub use_nonce: Signal<bool>,
    pub nonce: Signal<Option<Pubkey>>,
    pub nonce_authority: Signal<Option<Pubkey>>,
    pub nonce_hash: Signal<String>,
    pub blockhash: Signal<String>,
    pub error: Signal<Option<String>>,
    pub compiled_note: Signal<Option<String>>,
    /// Keypairs made here for accounts. See [`GeneratedKey`].
    pub generated: Signal<Vec<GeneratedKey>>,
}

impl BuildForm {
    pub fn clear(&self) {
        let mut drafts = self.drafts;
        let mut selected = self.selected;
        let mut next_id = self.next_id;
        let mut fee_payer = self.fee_payer;
        let mut payer_for = self.payer_for;
        let mut picker_reset = self.picker_reset;
        let mut advanced_open = self.advanced_open;
        let mut cu_limit = self.cu_limit;
        let mut cu_price = self.cu_price;
        let mut loaded_accounts = self.loaded_accounts;
        let mut use_nonce = self.use_nonce;
        let mut nonce = self.nonce;
        let mut nonce_authority = self.nonce_authority;
        let mut nonce_hash = self.nonce_hash;
        let mut blockhash = self.blockhash;
        let mut error = self.error;
        let mut compiled_note = self.compiled_note;
        drafts.set(Vec::new());
        selected.set(None);
        next_id.set(1);
        fee_payer.set(None);
        payer_for.set(None);
        picker_reset.set(picker_reset() + 1);
        advanced_open.set(false);
        cu_limit.set(String::new());
        cu_price.set(String::new());
        loaded_accounts.set(String::new());
        use_nonce.set(false);
        nonce.set(None);
        nonce_authority.set(None);
        nonce_hash.set(String::new());
        blockhash.set(String::new());
        error.set(None);
        compiled_note.set(None);
    }

    /// The lifetime and budget fields, as typed.
    pub fn settings(&self) -> TxSettings {
        TxSettings {
            use_nonce: (self.use_nonce)(),
            nonce: (self.nonce)(),
            nonce_authority: (self.nonce_authority)(),
            nonce_hash: (self.nonce_hash)(),
            blockhash: (self.blockhash)(),
            cu_limit: (self.cu_limit)(),
            cu_price: (self.cu_price)(),
            loaded_accounts: (self.loaded_accounts)(),
        }
    }

    fn take_id(&self) -> u64 {
        let mut next_id = self.next_id;
        let id = *next_id.peek();
        next_id.set(id + 1);
        id
    }

    /// The blocks at the top, or inside carrier `parent`.
    pub fn blocks(&self, parent: Option<u64>) -> Vec<IxDraft> {
        let drafts = self.drafts.read();
        let Some(parent) = parent else {
            return drafts.clone();
        };
        match find_ref(&drafts, parent).map(|draft| &draft.body) {
            Some(IxBody::Carried { inner, .. }) => inner.clone(),
            _ => Vec::new(),
        }
    }

    /// The block open at the top, or inside carrier `parent`.
    pub fn selected_in(&self, parent: Option<u64>) -> Option<u64> {
        let Some(parent) = parent else {
            return (self.selected)();
        };
        match find_ref(&self.drafts.read(), parent).map(|draft| &draft.body) {
            Some(IxBody::Carried { selected, .. }) => *selected,
            _ => None,
        }
    }

    pub fn select(&self, parent: Option<u64>, id: u64) {
        match parent {
            None => {
                let mut selected = self.selected;
                selected.set(Some(id));
            }
            Some(parent) => self.patch(parent, |draft| {
                if let IxBody::Carried { selected, .. } = &mut draft.body {
                    *selected = Some(id);
                }
            }),
        }
    }

    /// Add a block at the end, at the top or inside carrier `parent`, and
    /// open it.
    pub fn add_instruction(&self, parent: Option<u64>, body: IxBody) -> u64 {
        let id = self.take_id();
        let draft = IxDraft { id, body };
        match parent {
            None => {
                let mut drafts = self.drafts;
                drafts.write().push(draft);
            }
            Some(parent) => self.patch(parent, |carrier| {
                if let IxBody::Carried { inner, .. } = &mut carrier.body {
                    inner.push(draft);
                }
            }),
        }
        self.select(parent, id);
        id
    }

    /// Take block `id` out, wherever it is.
    pub fn remove_instruction(&self, id: u64) {
        let mut drafts = self.drafts;
        let mut list = drafts.write();
        if list.iter().any(|d| d.id == id) {
            list.retain(|d| d.id != id);
            let mut selected = self.selected;
            if *selected.peek() == Some(id) {
                selected.set(list.first().map(|d| d.id));
            }
            return;
        }
        remove_nested(&mut list, id);
    }

    /// Move a block within the top, or within carrier `parent`.
    pub fn move_instruction(&self, parent: Option<u64>, from: usize, to: usize) {
        let shift = |list: &mut Vec<IxDraft>| {
            if from == to || from >= list.len() || to >= list.len() {
                return;
            }
            let item = list.remove(from);
            list.insert(to, item);
        };
        match parent {
            None => {
                let mut drafts = self.drafts;
                shift(&mut drafts.write());
            }
            Some(parent) => self.patch(parent, |carrier| {
                if let IxBody::Carried { inner, .. } = &mut carrier.body {
                    shift(inner);
                }
            }),
        }
    }

    /// Change block `id`, wherever it is.
    pub fn patch(&self, id: u64, f: impl FnOnce(&mut IxDraft)) {
        let mut drafts = self.drafts;
        let mut list = drafts.write();
        if let Some(draft) = find_mut(&mut list, id) {
            f(draft);
        }
    }

    /// Change the IDL fields of block `id`, when it is filled from an IDL.
    pub fn patch_idl(&self, id: u64, f: impl FnOnce(&mut IdlDraft)) {
        self.patch(id, |draft| {
            if let IxBody::Idl(idl) = &mut draft.body {
                f(idl);
            }
        });
    }

    /// Change the raw fields of block `id`, when it is written by hand.
    pub fn patch_raw(&self, id: u64, f: impl FnOnce(&mut RawDraft)) {
        self.patch(id, |draft| {
            if let IxBody::Raw(raw) = &mut draft.body {
                f(raw);
            }
        });
    }

    /// Put a transaction read back by `wallet_core::edit` on the form, in
    /// place of what was there. `wallet` is the wallet whose fee payer and
    /// nonce the form would otherwise put over the ones read.
    pub fn load(&self, opened: OpenedTx, wallet: Option<wallet_core::WalletId>) {
        self.clear();
        let drafts: Vec<IxDraft> = opened
            .instructions
            .into_iter()
            .map(|ix| self.draft_of(ix))
            .collect();
        let mut this = *self;
        this.selected.set(drafts.first().map(|d| d.id));
        this.drafts.set(drafts);
        this.fee_payer.set(opened.fee_payer);
        this.payer_for.set(wallet);
        let text = |value: Option<u64>| value.map(|v| v.to_string()).unwrap_or_default();
        this.cu_limit.set(text(opened.cu_limit.map(u64::from)));
        this.cu_price.set(text(opened.cu_price_micro_lamports));
        this.loaded_accounts
            .set(text(opened.loaded_accounts.map(u64::from)));
        if let Some(nonce) = &opened.nonce {
            this.use_nonce.set(true);
            this.nonce.set(Some(nonce.account));
            this.nonce_authority.set(Some(nonce.authority));
            this.nonce_hash.set(nonce.hash.to_string());
        }
        this.advanced_open.set(
            opened.nonce.is_some()
                || opened.cu_limit.is_some()
                || opened.cu_price_micro_lamports.is_some()
                || opened.loaded_accounts.is_some(),
        );
    }

    fn draft_of(&self, opened: OpenedIx) -> IxDraft {
        let id = self.take_id();
        let body = match opened {
            OpenedIx::Idl(idl) => IxBody::Idl(idl),
            OpenedIx::Raw(raw) => IxBody::Raw(raw),
            OpenedIx::Carried { carrier, inner } => {
                let inner: Vec<IxDraft> = inner.into_iter().map(|ix| self.draft_of(ix)).collect();
                IxBody::Carried {
                    carrier,
                    selected: inner.first().map(|d| d.id),
                    inner,
                }
            }
        };
        IxDraft { id, body }
    }
}
