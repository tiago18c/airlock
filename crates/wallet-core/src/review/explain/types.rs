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
    /// The chain publishes the document the stored file was made from, such
    /// as the Anchor IDL an embedded Codama file was converted from.
    MatchesSource,
    Differs,
}

impl IdlPinStatus {
    /// The status of a stored IDL, from its last online check.
    pub fn of(check: Option<&IdlCheck>) -> Self {
        match check {
            Some(check) if check.same => Self::Matches,
            Some(check) if check.source => Self::MatchesSource,
            Some(_) => Self::Differs,
            None => Self::NotChecked,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::NoIdl => "no IDL",
            Self::NotChecked => "not checked",
            Self::Matches => "matches",
            Self::MatchesSource => "matches its source",
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
    /// The program whose file carries this one's IDL, by that file's name,
    /// when another file carries it.
    pub carried_by: Option<String>,
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
    /// Where the IDL that words this instruction came from: shipped with the
    /// app, a file the person imported, or the chain, where whoever controls
    /// the program wrote it.
    pub origin: IdlOrigin,
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
    /// The bytes of the instruction's data that follow the ones its IDL
    /// describes, when there are any. The program may read them as an
    /// argument a newer version of it added, which nothing here shows.
    pub undescribed_bytes: Vec<u8>,
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
    /// What a Squads smart account's vault runs at once, in this
    /// transaction, approved by the members that sign it.
    SquadsSync {
        vault: Pubkey,
        vault_index: u8,
        signers: Vec<Pubkey>,
    },
    /// A transaction added to a Realms proposal, which the governance runs,
    /// signing as itself or its treasury, once the proposal passes.
    ProposalTransaction {
        governance: Pubkey,
        proposal: Pubkey,
        option_index: u8,
        transaction_index: u16,
    },
    /// A Vector `Passthrough`: what runs as its accounts after the
    /// transaction's `Advance`.
    Passthrough { signers: Vec<RunsAs> },
    /// The Squads clear signing program's `verifyProposal`, or the same check
    /// by CPI, `verifyProposalCpi`: the instructions proposal
    /// `transaction_index` of `multisig` must hold, exactly, for the vote
    /// beside it to land. They run as `vault` once the proposal passes,
    /// so the vault is what signs them, not a signer of this transaction.
    SquadsCheck {
        multisig: Pubkey,
        transaction_index: u64,
        vault: Pubkey,
        vault_index: u8,
        member: Pubkey,
        action: crate::wallets::squads::VoteAction,
    },
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
    /// The accounts whose bytes the sentences would read, each with the
    /// program whose instruction reads it, first-seen order. Instructions
    /// carried inside others count too.
    pub fn accounts_needed(&self) -> Vec<(Pubkey, Pubkey)> {
        fn walk(instructions: &[ExplainedIx], out: &mut Vec<(Pubkey, Pubkey)>) {
            for ix in instructions {
                if let Some(idl) = &ix.idl {
                    for pubkey in &idl.accounts_needed {
                        let pair = (*pubkey, ix.program_id);
                        if !out.contains(&pair) {
                            out.push(pair);
                        }
                    }
                }
                if let Some(inner) = &ix.inner {
                    walk(&inner.instructions, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.instructions, &mut out);
        out
    }
}

/// What of `fetched`, a node's answer for the accounts in `needed` in the same
/// order, the display may read. An IDL describes the accounts of its own
/// program, so an account is kept when the program that reads it owns it:
/// bytes anyone could have written to an account of their own must not set a
/// mint's decimals. A mint is kept for any reader: only the Token and
/// Token-2022 programs can write an initialized mint they own, so its decimals
/// are true whoever reads them, as a swap does for the mints it trades.
pub fn display_accounts(
    needed: &[(Pubkey, Pubkey)],
    fetched: &[Option<crate::chain::RawAccount>],
) -> HashMap<Pubkey, Vec<u8>> {
    needed
        .iter()
        .zip(fetched)
        .filter_map(|((address, reader), account)| {
            let account = account.as_ref()?;
            (account.owner == *reader || is_token_mint(account))
                .then(|| (*address, account.data.clone()))
        })
        .collect()
}

/// Whether `account` is an initialized mint of the Token or Token-2022
/// program. A Token mint is exactly 82 bytes. A Token-2022 mint is 82 bytes,
/// or, with extensions, longer than an account's 165 bytes with account type 1
/// at byte 165; Token-2022 never gives such an account a multisig's 355 bytes.
fn is_token_mint(account: &crate::chain::RawAccount) -> bool {
    const MINT_LEN: usize = 82;
    const ACCOUNT_LEN: usize = 165;
    const MULTISIG_LEN: usize = 355;
    const IS_INITIALIZED: usize = 45;
    const ACCOUNT_TYPE_MINT: u8 = 1;
    let data = &account.data;
    let shaped = if account.owner == crate::ids::TOKEN_PROGRAM_ID {
        data.len() == MINT_LEN
    } else if account.owner == crate::ids::TOKEN_2022_PROGRAM_ID {
        data.len() == MINT_LEN
            || (data.len() > ACCOUNT_LEN
                && data.len() != MULTISIG_LEN
                && data[ACCOUNT_LEN] == ACCOUNT_TYPE_MINT)
    } else {
        false
    };
    shaped && data[IS_INITIALIZED] == 1
}
