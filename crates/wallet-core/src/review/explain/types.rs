//! A transaction as it is shown: each program, each instruction, and what a
//! stored IDL says about it.

use super::*;
use solana_hash::Hash;

/// How the stored IDL compares with the last online check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdlPinStatus {
    NoIdl,
    NotChecked,
    Matches,
    Differs,
}

impl IdlPinStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::NoIdl => "no IDL",
            Self::NotChecked => "not checked",
            Self::Matches => "matches",
            Self::Differs => "differs",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgramLine {
    pub program_id: Pubkey,
    pub name: Option<String>,
    pub sha256: Option<[u8; 32]>,
    /// Where the IDL came from: shipped with the app, a file, or the chain.
    pub origin: Option<IdlOrigin>,
    pub status: IdlPinStatus,
    /// Set when a kind 3 note pinned this program id.
    pub context_pin: Option<ContextPinMatch>,
}

/// How a kind 3 pin compares with the IDL stored on this device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextPinMatch {
    Same,
    Differs,
    Missing,
}

/// Account bytes and program pins from a kind 3 note. The Codama display uses
/// the bytes only when [`note_supplies_accounts`] says the pin allows it.
///
/// `schemes` is what is known of each Vector account: from a context note's
/// authorizations, a rebuild, or a wallet record. The `Advance` lays out its
/// accounts by scheme, so the key a Winternitz or XMSS account moves to is
/// read only when the schemes are known. `keys` are the keys accounts hold
/// now, read from the chain, which tell an XMSS account keeping its key from
/// one moving to another.
#[derive(Clone, Debug, Default)]
pub struct DisplayNote {
    pub pins: Vec<(Pubkey, [u8; 32])>,
    pub accounts: HashMap<Pubkey, Vec<u8>>,
    pub schemes: HashMap<Pubkey, crate::wallets::vector::SchemeId>,
    pub keys: HashMap<Pubkey, Vec<u8>>,
}

impl DisplayNote {
    /// Record each account's scheme and current key from `known`, without
    /// replacing one that is already set.
    pub fn with_vector_schemes(mut self, known: &[crate::wallets::relay::VectorContext]) -> Self {
        for context in known {
            self.schemes
                .entry(context.account)
                .or_insert(context.scheme);
            if let Some(key) = &context.key {
                self.keys
                    .entry(context.account)
                    .or_insert_with(|| key.clone());
            }
        }
        self
    }

    /// Record the scheme of each Vector wallet in `wallets`, without
    /// replacing one that is already set. A record's address is derived from
    /// its scheme and key, so it cannot name a scheme its account does not have.
    pub fn with_wallet_schemes(mut self, wallets: &[crate::WalletSummary]) -> Self {
        for wallet in wallets {
            let scheme = wallet
                .contract
                .as_ref()
                .and_then(crate::WalletKind::vector_settings)
                .map(|settings| settings.scheme);
            if let (Some(scheme), Some(address)) = (scheme, wallet.address) {
                self.schemes.entry(address).or_insert(scheme);
            }
        }
        self
    }
}

pub fn note_supplies_accounts(
    program_id: &Pubkey,
    local_sha: Option<[u8; 32]>,
    note: &DisplayNote,
) -> bool {
    match note
        .pins
        .iter()
        .find(|(id, _)| id == program_id)
        .map(|(_, sha)| *sha)
    {
        Some(sha) => local_sha == Some(sha),
        None => true,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdlField {
    pub label: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdlAccountLine {
    pub name: String,
    pub pubkey: Pubkey,
    pub signer: bool,
    pub writable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdlExplanation {
    pub idl_sha256: [u8; 32],
    pub intent: String,
    pub interpolated_intent: Option<String>,
    pub spans: Option<Vec<IntentSpan>>,
    pub fields: Vec<IdlField>,
    pub accounts: Vec<IdlAccountLine>,
    /// Accounts whose bytes an `accountField` injection would read.
    pub accounts_needed: Vec<Pubkey>,
    /// Accounts the instruction carries beyond those its IDL describes, when
    /// the IDL allows none. The program may be reading a different layout than
    /// the one shown, so the account names cannot be trusted.
    pub unnamed_accounts: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExplainedIx {
    pub program_id: Pubkey,
    /// Every account the instruction names, in order.
    pub accounts: Vec<Pubkey>,
    pub data: Vec<u8>,
    /// `None` when no stored IDL parses this instruction.
    pub idl: Option<IdlExplanation>,
    /// The instructions this one carries in its data, when it carries any.
    pub inner: Option<InnerMessage>,
}

/// Instructions found inside another instruction's data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InnerMessage {
    pub kind: InnerKind,
    pub instructions: Vec<ExplainedIx>,
}

/// One key that signs an authorization, and the address it signs as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuthorizingKey {
    pub authority: Pubkey,
    pub address: Pubkey,
    pub signed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InnerKind {
    /// A programmatic signer's `Submit`: what the authorities sign.
    Authorization {
        authorities: Vec<AuthorizingKey>,
        /// Ordinary keys that sign the transaction and are passed along.
        forwarded: Vec<Pubkey>,
    },
    /// The executor's `Execute`: what runs once the nonce is consumed.
    Execution {
        nonce_account: Pubkey,
        nonce_authority: Pubkey,
        nonce_value: Hash,
    },
    /// A transaction stored for a Squads vault to run after approval.
    SquadsTransaction { vault: Pubkey, vault_index: u8 },
    /// A Vector `Passthrough`: what runs as its accounts after the
    /// transaction's `Advance`.
    Passthrough { signers: Vec<RunsAs> },
}

/// One account a `Passthrough` runs as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunsAs {
    /// The Vector account.
    pub account: Pubkey,
    /// It signs as its system PDA, this address, instead of as itself.
    pub system: Option<Pubkey>,
    /// What the transaction's `Advance` does to the key that controls it.
    pub next_key: Option<NextKey>,
}

/// The key a Winternitz or XMSS account signs with after the transaction, as
/// its `Advance` names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NextKey {
    /// A new key takes over the account.
    Replaces(Vec<u8>),
    /// An XMSS key that replaces the one the account holds unless it is that
    /// same key, which is not known here.
    Names(Vec<u8>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExplainedTx {
    pub programs: Vec<ProgramLine>,
    pub instructions: Vec<ExplainedIx>,
}

impl ExplainedTx {
    /// Union of per-instruction account fetches, first-seen order.
    pub fn accounts_needed(&self) -> Vec<Pubkey> {
        let mut out = Vec::new();
        for ix in &self.instructions {
            let Some(idl) = &ix.idl else { continue };
            for pubkey in &idl.accounts_needed {
                if !out.contains(pubkey) {
                    out.push(*pubkey);
                }
            }
        }
        out
    }
}
