//! SPL tokens: the accounts a send reads, and the instructions it builds.
//!
//! Token and associated-token have no interface crate here, so the layouts
//! are written out from the programs' documented formats. Token-2022 lays an
//! account out as Token does, then pads it to 165 bytes, adds a byte for the
//! account type, and appends its extensions as type, length, value.

use crate::chain::RawAccount;
use crate::ids::{
    ASSOCIATED_TOKEN_PROGRAM_ID, SYSTEM_PROGRAM_ID, TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID,
};
use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;

/// Size of a Token mint, and of a Token-2022 mint's base.
const MINT_LEN: usize = 82;
/// Size of a Token account, and of a Token-2022 account's base. Token-2022
/// pads a mint with extensions to this size too.
const ACCOUNT_LEN: usize = 165;
/// The account-type byte Token-2022 writes after the padded base.
const ACCOUNT_TYPE_MINT: u8 = 1;
const ACCOUNT_TYPE_ACCOUNT: u8 = 2;

// Token-2022 extension types this module reads.
const EXT_TRANSFER_FEE_CONFIG: u16 = 1;
const EXT_DEFAULT_ACCOUNT_STATE: u16 = 6;
const EXT_MEMO_TRANSFER: u16 = 8;
const EXT_NON_TRANSFERABLE: u16 = 9;
const EXT_TRANSFER_HOOK: u16 = 14;
const EXT_PAUSABLE: u16 = 26;

/// Account state as Token stores it.
const STATE_INITIALIZED: u8 = 1;
const STATE_FROZEN: u8 = 2;

/// Whether `program` is Token or Token-2022.
pub fn is_token_program(program: &Pubkey) -> bool {
    *program == TOKEN_PROGRAM_ID || *program == TOKEN_2022_PROGRAM_ID
}

/// The address the associated-token program derives for a wallet and mint.
pub fn associated_token_address(wallet: &Pubkey, mint: &Pubkey, token_program: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[wallet.as_ref(), token_program.as_ref(), mint.as_ref()],
        &ASSOCIATED_TOKEN_PROGRAM_ID,
    )
    .0
}

/// Associated-token `CreateIdempotent`: creates `wallet`'s account for
/// `mint` unless it exists, with `payer` paying its rent.
pub fn create_ata_idempotent(
    payer: Pubkey,
    wallet: Pubkey,
    mint: Pubkey,
    token_program: Pubkey,
) -> Instruction {
    Instruction {
        program_id: ASSOCIATED_TOKEN_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(payer, true),
            AccountMeta::new(
                associated_token_address(&wallet, &mint, &token_program),
                false,
            ),
            AccountMeta::new_readonly(wallet, false),
            AccountMeta::new_readonly(mint, false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
            AccountMeta::new_readonly(token_program, false),
        ],
        data: vec![1],
    }
}

/// Token `TransferChecked`: tag 12, the amount as u64 LE, then the decimals.
pub fn transfer_checked(
    program: Pubkey,
    source: Pubkey,
    mint: Pubkey,
    destination: Pubkey,
    authority: Pubkey,
    amount: u64,
    decimals: u8,
) -> Instruction {
    let mut data = vec![12u8];
    data.extend_from_slice(&amount.to_le_bytes());
    data.push(decimals);
    Instruction {
        program_id: program,
        accounts: vec![
            AccountMeta::new(source, false),
            AccountMeta::new_readonly(mint, false),
            AccountMeta::new(destination, false),
            AccountMeta::new_readonly(authority, true),
        ],
        data,
    }
}

/// What a send needs to know about a mint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mint {
    /// Token or Token-2022: the program that owns the mint.
    pub program: Pubkey,
    pub decimals: u8,
    /// The larger of the two transfer fees a Token-2022 mint names, in basis
    /// points, when it charges one.
    pub transfer_fee_bps: Option<u16>,
    /// A Token-2022 transfer hook program, which needs accounts this app
    /// does not resolve.
    pub transfer_hook: Option<Pubkey>,
    pub non_transferable: bool,
    pub paused: bool,
    /// New accounts for this mint start frozen.
    pub frozen_by_default: bool,
}

impl Mint {
    /// A mint owned by Token or Token-2022. `None` for any other account.
    pub fn parse(account: &RawAccount) -> Option<Self> {
        if !is_token_program(&account.owner) || account.data.len() < MINT_LEN {
            return None;
        }
        let data = &account.data;
        // `is_initialized` follows the authority, supply and decimals.
        if data[45] != 1 {
            return None;
        }
        let mut mint = Mint {
            program: account.owner,
            decimals: data[44],
            transfer_fee_bps: None,
            transfer_hook: None,
            non_transferable: false,
            paused: false,
            frozen_by_default: false,
        };
        if data.len() > MINT_LEN {
            if account.owner != TOKEN_2022_PROGRAM_ID {
                return None;
            }
            for (kind, value) in extensions(data, ACCOUNT_TYPE_MINT)? {
                match kind {
                    EXT_TRANSFER_FEE_CONFIG if value.len() >= 108 => {
                        // Authorities (64), withheld amount (8), then the
                        // older and newer fees: epoch, maximum, basis points.
                        let older = u16::from_le_bytes([value[88], value[89]]);
                        let newer = u16::from_le_bytes([value[106], value[107]]);
                        let bps = older.max(newer);
                        mint.transfer_fee_bps = (bps > 0).then_some(bps);
                    }
                    EXT_DEFAULT_ACCOUNT_STATE if !value.is_empty() => {
                        mint.frozen_by_default = value[0] == STATE_FROZEN;
                    }
                    EXT_NON_TRANSFERABLE => mint.non_transferable = true,
                    EXT_TRANSFER_HOOK if value.len() >= 64 => {
                        let program = Pubkey::try_from(&value[32..64]).ok()?;
                        mint.transfer_hook = (program != Pubkey::default()).then_some(program);
                    }
                    EXT_PAUSABLE if value.len() >= 33 => mint.paused = value[32] != 0,
                    _ => {}
                }
            }
        }
        Some(mint)
    }

    /// Why this mint cannot be sent by this app, if it cannot.
    pub fn refusal(&self) -> Option<&'static str> {
        if self.non_transferable {
            Some("This token cannot be transferred: its mint makes it non-transferable.")
        } else if self.paused {
            Some("Transfers of this token are paused by its mint.")
        } else if self.transfer_hook.is_some() {
            Some("This token runs a transfer hook, which this app cannot send yet.")
        } else {
            None
        }
    }
}

/// What a send needs to know about a token account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenAccount {
    /// Token or Token-2022: the program that owns the account.
    pub program: Pubkey,
    pub mint: Pubkey,
    /// Who may move its tokens.
    pub owner: Pubkey,
    pub amount: u64,
    pub frozen: bool,
    /// Incoming transfers must come with a memo (Token-2022).
    pub requires_memo: bool,
}

impl TokenAccount {
    /// An initialized account owned by Token or Token-2022. `None` for any
    /// other account, a mint included.
    pub fn parse(account: &RawAccount) -> Option<Self> {
        if !is_token_program(&account.owner) || account.data.len() < ACCOUNT_LEN {
            return None;
        }
        let data = &account.data;
        let state = data[108];
        if state != STATE_INITIALIZED && state != STATE_FROZEN {
            return None;
        }
        let mut parsed = TokenAccount {
            program: account.owner,
            mint: Pubkey::try_from(&data[0..32]).ok()?,
            owner: Pubkey::try_from(&data[32..64]).ok()?,
            amount: u64::from_le_bytes(data[64..72].try_into().ok()?),
            frozen: state == STATE_FROZEN,
            requires_memo: false,
        };
        if data.len() > ACCOUNT_LEN {
            if account.owner != TOKEN_2022_PROGRAM_ID {
                return None;
            }
            for (kind, value) in extensions(data, ACCOUNT_TYPE_ACCOUNT)? {
                if kind == EXT_MEMO_TRANSFER && !value.is_empty() {
                    parsed.requires_memo = value[0] != 0;
                }
            }
        }
        Some(parsed)
    }
}

/// The extensions of a Token-2022 account of type `expected`, as type and
/// value. `None` when the account is of another type.
fn extensions(data: &[u8], expected: u8) -> Option<Vec<(u16, &[u8])>> {
    if *data.get(ACCOUNT_LEN)? != expected {
        return None;
    }
    let mut out = Vec::new();
    let mut at = ACCOUNT_LEN + 1;
    while at + 4 <= data.len() {
        let kind = u16::from_le_bytes([data[at], data[at + 1]]);
        let len = usize::from(u16::from_le_bytes([data[at + 2], data[at + 3]]));
        // Type 0 is the unused space at the end.
        if kind == 0 {
            break;
        }
        let value = data.get(at + 4..at + 4 + len)?;
        out.push((kind, value));
        at += 4 + len;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pk(n: u8) -> Pubkey {
        Pubkey::new_from_array([n; 32])
    }

    fn raw(owner: Pubkey, data: Vec<u8>) -> RawAccount {
        RawAccount {
            lamports: 1,
            data,
            owner,
            executable: false,
            rent_epoch: 0,
        }
    }

    fn mint_data(decimals: u8) -> Vec<u8> {
        let mut data = vec![0u8; MINT_LEN];
        data[44] = decimals;
        data[45] = 1;
        data
    }

    fn account_data(mint: Pubkey, owner: Pubkey, amount: u64, state: u8) -> Vec<u8> {
        let mut data = vec![0u8; ACCOUNT_LEN];
        data[0..32].copy_from_slice(mint.as_ref());
        data[32..64].copy_from_slice(owner.as_ref());
        data[64..72].copy_from_slice(&amount.to_le_bytes());
        data[108] = state;
        data
    }

    /// Token-2022 layout: the base padded to 165 bytes, the type, then TLVs.
    fn with_extensions(mut data: Vec<u8>, kind: u8, tlvs: &[(u16, Vec<u8>)]) -> Vec<u8> {
        data.resize(ACCOUNT_LEN, 0);
        data.push(kind);
        for (ext, value) in tlvs {
            data.extend_from_slice(&ext.to_le_bytes());
            data.extend_from_slice(&(value.len() as u16).to_le_bytes());
            data.extend_from_slice(value);
        }
        data
    }

    #[test]
    fn a_plain_mint_and_account_read() {
        let mint = Mint::parse(&raw(TOKEN_PROGRAM_ID, mint_data(6))).unwrap();
        assert_eq!(mint.decimals, 6);
        assert_eq!(mint.program, TOKEN_PROGRAM_ID);
        assert_eq!(mint.refusal(), None);

        let account = TokenAccount::parse(&raw(
            TOKEN_PROGRAM_ID,
            account_data(pk(1), pk(2), 42, STATE_INITIALIZED),
        ))
        .unwrap();
        assert_eq!(
            (account.mint, account.owner, account.amount),
            (pk(1), pk(2), 42)
        );
        assert!(!account.frozen);
        // A mint is not an account, nor the other way round.
        assert!(TokenAccount::parse(&raw(TOKEN_PROGRAM_ID, mint_data(6))).is_none());
        assert!(Mint::parse(&raw(SYSTEM_PROGRAM_ID, mint_data(6))).is_none());
        assert!(
            TokenAccount::parse(&raw(
                TOKEN_PROGRAM_ID,
                account_data(pk(1), pk(2), 42, STATE_FROZEN)
            ))
            .unwrap()
            .frozen
        );
    }

    #[test]
    fn token_2022_extensions_are_read() {
        let mut fee = vec![0u8; 108];
        fee[88..90].copy_from_slice(&50u16.to_le_bytes());
        fee[106..108].copy_from_slice(&100u16.to_le_bytes());
        let mut hook = vec![0u8; 64];
        hook[32..64].copy_from_slice(pk(9).as_ref());
        let data = with_extensions(
            mint_data(9),
            ACCOUNT_TYPE_MINT,
            &[
                (EXT_TRANSFER_FEE_CONFIG, fee),
                (EXT_DEFAULT_ACCOUNT_STATE, vec![STATE_FROZEN]),
                (EXT_TRANSFER_HOOK, hook),
            ],
        );
        let mint = Mint::parse(&raw(TOKEN_2022_PROGRAM_ID, data)).unwrap();
        assert_eq!(mint.transfer_fee_bps, Some(100));
        assert_eq!(mint.transfer_hook, Some(pk(9)));
        assert!(mint.frozen_by_default);
        assert!(mint.refusal().unwrap().contains("hook"));

        // A hook extension with no program set does nothing.
        let data = with_extensions(
            mint_data(9),
            ACCOUNT_TYPE_MINT,
            &[
                (EXT_TRANSFER_HOOK, vec![0u8; 64]),
                (EXT_NON_TRANSFERABLE, vec![]),
            ],
        );
        let mint = Mint::parse(&raw(TOKEN_2022_PROGRAM_ID, data)).unwrap();
        assert_eq!(mint.transfer_hook, None);
        assert!(mint.refusal().unwrap().contains("non-transferable"));

        let data = with_extensions(
            account_data(pk(1), pk(2), 5, STATE_INITIALIZED),
            ACCOUNT_TYPE_ACCOUNT,
            &[(EXT_MEMO_TRANSFER, vec![1])],
        );
        assert!(
            TokenAccount::parse(&raw(TOKEN_2022_PROGRAM_ID, data.clone()))
                .unwrap()
                .requires_memo
        );
        // Extensions only exist under Token-2022.
        assert!(TokenAccount::parse(&raw(TOKEN_PROGRAM_ID, data)).is_none());
    }

    #[test]
    fn instructions_name_the_accounts_in_order() {
        let ata = associated_token_address(&pk(2), &pk(3), &TOKEN_PROGRAM_ID);
        let create = create_ata_idempotent(pk(1), pk(2), pk(3), TOKEN_PROGRAM_ID);
        assert_eq!(create.data, vec![1]);
        assert_eq!(create.accounts[1].pubkey, ata);
        assert!(create.accounts[0].is_signer);

        let transfer = transfer_checked(TOKEN_PROGRAM_ID, pk(4), pk(3), ata, pk(5), 7, 6);
        assert_eq!(transfer.data[0], 12);
        assert_eq!(&transfer.data[1..9], &7u64.to_le_bytes());
        assert_eq!(transfer.data[9], 6);
        assert!(transfer.accounts[3].is_signer);
        // Token-2022 derives a different address for the same wallet and mint.
        assert_ne!(
            ata,
            associated_token_address(&pk(2), &pk(3), &TOKEN_2022_PROGRAM_ID)
        );
    }
}
