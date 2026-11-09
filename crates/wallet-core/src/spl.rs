//! SPL tokens: the accounts a send reads, and the instructions it builds.
//!
//! Token and associated-token have no interface crate here, so the layouts
//! are written out from the programs' documented formats. Token-2022 lays an
//! account out as Token does, then pads it to 165 bytes, adds a byte for the
//! account type, and appends its extensions as type, length, value.

use crate::chain::RawAccount;
use crate::ids::{
    ASSOCIATED_TOKEN_PROGRAM_ID, MEMO_PROGRAM_ID, NATIVE_MINT, NATIVE_MINT_2022, SYSTEM_PROGRAM_ID,
    TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID,
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
const EXT_TRANSFER_FEE_AMOUNT: u16 = 2;
const EXT_CONFIDENTIAL_TRANSFER_ACCOUNT: u16 = 5;
const EXT_DEFAULT_ACCOUNT_STATE: u16 = 6;
const EXT_MEMO_TRANSFER: u16 = 8;
const EXT_NON_TRANSFERABLE: u16 = 9;
const EXT_TRANSFER_HOOK: u16 = 14;
const EXT_CONFIDENTIAL_TRANSFER_FEE_AMOUNT: u16 = 17;
const EXT_PAUSABLE: u16 = 26;
const EXT_PERMISSIONED_BURN: u16 = 28;

/// Where a confidential account's encrypted balances sit in its extension:
/// after `approved` (1) and the ElGamal key (32), the pending balance's low
/// and high halves and the available balance, 64 bytes each. The account
/// can be closed only while all three are the zero ciphertext.
const CONFIDENTIAL_BALANCES: std::ops::Range<usize> = 33..225;

// Token instruction tags.
const IX_CLOSE_ACCOUNT: u8 = 9;
const IX_TRANSFER_CHECKED: u8 = 12;
const IX_BURN_CHECKED: u8 = 15;
/// Token-2022's transfer-fee instructions, and `HarvestWithheldTokensToMint`
/// among them.
const IX_TRANSFER_FEE_EXTENSION: u8 = 26;
const IX_HARVEST_WITHHELD_TOKENS_TO_MINT: u8 = 4;

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
    let mut data = vec![IX_TRANSFER_CHECKED];
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

/// Token `BurnChecked`: tag 15, the amount as u64 LE, then the decimals.
pub fn burn_checked(
    program: Pubkey,
    account: Pubkey,
    mint: Pubkey,
    authority: Pubkey,
    amount: u64,
    decimals: u8,
) -> Instruction {
    let mut data = vec![IX_BURN_CHECKED];
    data.extend_from_slice(&amount.to_le_bytes());
    data.push(decimals);
    Instruction {
        program_id: program,
        accounts: vec![
            AccountMeta::new(account, false),
            AccountMeta::new(mint, false),
            AccountMeta::new_readonly(authority, true),
        ],
        data,
    }
}

/// Token `CloseAccount`: tag 9. The account's lamports go to `destination`.
/// A token account must hold no tokens first; a wrapped SOL account may, and
/// closing it unwraps them.
pub fn close_account(
    program: Pubkey,
    account: Pubkey,
    destination: Pubkey,
    authority: Pubkey,
) -> Instruction {
    Instruction {
        program_id: program,
        accounts: vec![
            AccountMeta::new(account, false),
            AccountMeta::new(destination, false),
            AccountMeta::new_readonly(authority, true),
        ],
        data: vec![IX_CLOSE_ACCOUNT],
    }
}

/// Token-2022 `HarvestWithheldTokensToMint`: moves the transfer fees withheld
/// in `accounts` to their mint. Anyone may send it, and an account still
/// holding withheld fees cannot be closed.
pub fn harvest_withheld_to_mint(mint: Pubkey, accounts: &[Pubkey]) -> Instruction {
    let mut metas = vec![AccountMeta::new(mint, false)];
    metas.extend(
        accounts
            .iter()
            .map(|account| AccountMeta::new(*account, false)),
    );
    Instruction {
        program_id: TOKEN_2022_PROGRAM_ID,
        accounts: metas,
        data: vec![
            IX_TRANSFER_FEE_EXTENSION,
            IX_HARVEST_WITHHELD_TOKENS_TO_MINT,
        ],
    }
}

/// A memo with `text` and no signers. An account that wants a memo on what
/// it receives needs one right before the transfer.
pub fn memo(text: &str) -> Instruction {
    Instruction {
        program_id: MEMO_PROGRAM_ID,
        accounts: Vec::new(),
        data: text.as_bytes().to_vec(),
    }
}

/// Whether `mint` is wrapped SOL, under either token program.
pub fn is_native_mint(mint: &Pubkey) -> bool {
    *mint == NATIVE_MINT || *mint == NATIVE_MINT_2022
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
    /// A Token-2022 mint whose burns need this authority's signature too.
    pub burn_authority: Option<Pubkey>,
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
            burn_authority: None,
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
                    EXT_PERMISSIONED_BURN if value.len() >= 32 => {
                        let authority = Pubkey::try_from(&value[0..32]).ok()?;
                        mint.burn_authority = (authority != Pubkey::default()).then_some(authority);
                    }
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

    /// Why this app cannot burn this mint's tokens, if it cannot.
    pub fn burn_refusal(&self) -> Option<&'static str> {
        if self.paused {
            Some("Burns of this token are paused by its mint.")
        } else if self.burn_authority.is_some() {
            Some("Burning this token needs the approval of an authority its mint names.")
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
    /// Who may close it instead of its owner, when someone is set.
    pub close_authority: Option<Pubkey>,
    /// Transfer fees withheld in it (Token-2022), which must be harvested to
    /// the mint before it can be closed.
    pub withheld: u64,
    /// It holds a confidential balance or withheld confidential fees
    /// (Token-2022), which only a zero-knowledge proof can empty, so it
    /// cannot be closed by this app.
    pub confidential: bool,
}

impl TokenAccount {
    /// Who signs to close it: its close authority, or else its owner.
    pub fn closer(&self) -> Pubkey {
        self.close_authority.unwrap_or(self.owner)
    }

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
            // A COption: a u32 tag, 1 for set, then the key.
            close_authority: match data[129..133] {
                [1, 0, 0, 0] => Some(Pubkey::try_from(&data[133..165]).ok()?),
                _ => None,
            },
            withheld: 0,
            confidential: false,
        };
        if data.len() > ACCOUNT_LEN {
            if account.owner != TOKEN_2022_PROGRAM_ID {
                return None;
            }
            for (kind, value) in extensions(data, ACCOUNT_TYPE_ACCOUNT)? {
                match kind {
                    EXT_MEMO_TRANSFER if !value.is_empty() => {
                        parsed.requires_memo = value[0] != 0;
                    }
                    EXT_TRANSFER_FEE_AMOUNT if value.len() >= 8 => {
                        parsed.withheld = u64::from_le_bytes(value[0..8].try_into().ok()?);
                    }
                    EXT_CONFIDENTIAL_TRANSFER_ACCOUNT => {
                        let balances = value.get(CONFIDENTIAL_BALANCES)?;
                        parsed.confidential |= balances.iter().any(|byte| *byte != 0);
                    }
                    EXT_CONFIDENTIAL_TRANSFER_FEE_AMOUNT => {
                        parsed.confidential |= value.iter().any(|byte| *byte != 0);
                    }
                    _ => {}
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
    fn close_authority_withheld_fees_and_confidential_balances_are_read() {
        let mut data = account_data(pk(1), pk(2), 0, STATE_INITIALIZED);
        data[129] = 1;
        data[133..165].copy_from_slice(pk(7).as_ref());
        let plain = TokenAccount::parse(&raw(TOKEN_PROGRAM_ID, data.clone())).unwrap();
        assert_eq!(plain.close_authority, Some(pk(7)));
        assert_eq!(plain.closer(), pk(7));
        assert_eq!((plain.withheld, plain.confidential), (0, false));

        let mut confidential = vec![0u8; 295];
        let data = with_extensions(
            account_data(pk(1), pk(2), 0, STATE_INITIALIZED),
            ACCOUNT_TYPE_ACCOUNT,
            &[
                (EXT_TRANSFER_FEE_AMOUNT, 42u64.to_le_bytes().to_vec()),
                (EXT_CONFIDENTIAL_TRANSFER_ACCOUNT, confidential.clone()),
            ],
        );
        let parsed = TokenAccount::parse(&raw(TOKEN_2022_PROGRAM_ID, data)).unwrap();
        assert_eq!(parsed.withheld, 42);
        assert_eq!(parsed.close_authority, None);
        assert_eq!(parsed.closer(), pk(2));
        // Zero ciphertexts: configured, but nothing in it.
        assert!(!parsed.confidential);

        // Something in the available balance, after the key and the two
        // halves of the pending balance.
        confidential[33 + 128] = 5;
        let data = with_extensions(
            account_data(pk(1), pk(2), 0, STATE_INITIALIZED),
            ACCOUNT_TYPE_ACCOUNT,
            &[(EXT_CONFIDENTIAL_TRANSFER_ACCOUNT, confidential)],
        );
        assert!(
            TokenAccount::parse(&raw(TOKEN_2022_PROGRAM_ID, data))
                .unwrap()
                .confidential
        );
    }

    #[test]
    fn a_mint_that_names_a_burn_authority_refuses_burns() {
        let data = with_extensions(
            mint_data(0),
            ACCOUNT_TYPE_MINT,
            &[(EXT_PERMISSIONED_BURN, pk(4).as_ref().to_vec())],
        );
        let mint = Mint::parse(&raw(TOKEN_2022_PROGRAM_ID, data)).unwrap();
        assert_eq!(mint.burn_authority, Some(pk(4)));
        assert!(mint.burn_refusal().is_some());
        assert_eq!(mint.refusal(), None);

        let data = with_extensions(
            mint_data(0),
            ACCOUNT_TYPE_MINT,
            &[(EXT_PERMISSIONED_BURN, vec![0u8; 32])],
        );
        let mint = Mint::parse(&raw(TOKEN_2022_PROGRAM_ID, data)).unwrap();
        assert_eq!(mint.burn_refusal(), None);
    }

    #[test]
    fn burn_close_harvest_and_memo_are_laid_out_as_the_programs_read_them() {
        let burn = burn_checked(TOKEN_PROGRAM_ID, pk(1), pk(2), pk(3), 9, 4);
        assert_eq!(burn.data[0], 15);
        assert_eq!(&burn.data[1..9], &9u64.to_le_bytes());
        assert_eq!(burn.data[9], 4);
        let metas: Vec<_> = burn
            .accounts
            .iter()
            .map(|m| (m.pubkey, m.is_writable, m.is_signer))
            .collect();
        assert_eq!(
            metas,
            vec![
                (pk(1), true, false),
                (pk(2), true, false),
                (pk(3), false, true)
            ]
        );

        let close = close_account(TOKEN_2022_PROGRAM_ID, pk(1), pk(5), pk(3));
        assert_eq!(close.program_id, TOKEN_2022_PROGRAM_ID);
        assert_eq!(close.data, vec![9]);
        let metas: Vec<_> = close
            .accounts
            .iter()
            .map(|m| (m.pubkey, m.is_writable, m.is_signer))
            .collect();
        assert_eq!(
            metas,
            vec![
                (pk(1), true, false),
                (pk(5), true, false),
                (pk(3), false, true)
            ]
        );

        let harvest = harvest_withheld_to_mint(pk(2), &[pk(1), pk(6)]);
        assert_eq!(harvest.program_id, TOKEN_2022_PROGRAM_ID);
        assert_eq!(harvest.data, vec![26, 4]);
        assert_eq!(harvest.accounts.len(), 3);
        assert!(harvest
            .accounts
            .iter()
            .all(|m| m.is_writable && !m.is_signer));

        let memo = memo("hi");
        assert_eq!(memo.program_id, MEMO_PROGRAM_ID);
        assert_eq!(memo.data, b"hi");
        assert!(memo.accounts.is_empty());

        assert!(is_native_mint(&NATIVE_MINT));
        assert!(is_native_mint(&NATIVE_MINT_2022));
        assert!(!is_native_mint(&pk(1)));
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
