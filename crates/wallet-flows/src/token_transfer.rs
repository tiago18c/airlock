//! Sending SPL tokens from a wallet to another address.
//!
//! The recipient is usually a wallet, and receives in its associated token
//! account, which is created first when it does not exist yet. A token
//! account of the same mint is sent to as it is.

use crate::compile::{compile_for_route, with_nonce_note};
use crate::error::{FlowError, Prepared};
use crate::transfer::{fee_payer, is_plain_key};
use solana_pubkey::Pubkey;
use wallet_core::spl::{
    associated_token_address, create_ata_idempotent, transfer_checked, Mint, TokenAccount,
};
use wallet_core::{format_token_amount, Phrase, SigningRoute, SolanaRpc, TOKEN_2022_PROGRAM_ID};

/// Where the tokens land.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Destination {
    /// The recipient's associated token account, which exists.
    Existing { account: Pubkey },
    /// The recipient was itself a token account of the mint.
    TokenAccount { account: Pubkey, owner: Pubkey },
    /// The recipient's associated token account, created first for about
    /// `rent` lamports.
    Create { account: Pubkey, rent: u64 },
}

impl Destination {
    pub fn account(&self) -> Pubkey {
        match self {
            Destination::Existing { account }
            | Destination::TokenAccount { account, .. }
            | Destination::Create { account, .. } => *account,
        }
    }
}

/// The mint of a send, and where it lands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub mint: Mint,
    pub destination: Destination,
}

/// Read `mint` and the recipient `to`, and say where tokens sent to `to`
/// land. Refused for a mint this app cannot send, and for a recipient that
/// cannot receive it: another token's account, a mint, a program, or an
/// account that is frozen or wants a memo.
pub async fn resolve(rpc: &dyn SolanaRpc, mint: Pubkey, to: Pubkey) -> Result<Resolved, FlowError> {
    let [mint_account, to_account] = read_two(rpc, mint, to).await?;
    let parsed = mint_account.as_ref().and_then(Mint::parse).ok_or_else(|| {
        FlowError::refused(format!("{mint} is not a token mint on this cluster."))
    })?;
    if let Some(why) = parsed.refusal() {
        return Err(FlowError::refused(why));
    }
    if let Some(account) = &to_account {
        if let Some(held) = TokenAccount::parse(account) {
            if held.mint != mint {
                return Err(FlowError::refused(
                    "The recipient is a token account for another token.",
                ));
            }
            receivable(&held)?;
            return Ok(Resolved {
                mint: parsed,
                destination: Destination::TokenAccount {
                    account: to,
                    owner: held.owner,
                },
            });
        }
        if Mint::parse(account).is_some() {
            return Err(FlowError::refused(
                "The recipient is a token mint, not a wallet.",
            ));
        }
        if account.executable {
            return Err(FlowError::refused(
                "The recipient is a program, not a wallet.",
            ));
        }
    }
    let ata = associated_token_address(&to, &mint, &parsed.program);
    let destination = match rpc.get_account(&ata).await? {
        Some(account) => {
            let held = TokenAccount::parse(&account)
                .filter(|held| held.mint == mint)
                .ok_or_else(|| {
                    FlowError::refused(
                        "The recipient's token account address holds something else.",
                    )
                })?;
            if held.owner != to {
                return Err(FlowError::refused(
                    "The recipient's token account was handed to another owner. Send to that account directly if it is meant.",
                ));
            }
            receivable(&held)?;
            Destination::Existing { account: ata }
        }
        None => {
            if parsed.frozen_by_default {
                return Err(FlowError::refused(
                    "The recipient has no account for this token, and a new one would start frozen.",
                ));
            }
            let rent = rpc.get_rent_exemption(account_len(&parsed)).await?;
            Destination::Create { account: ata, rent }
        }
    };
    Ok(Resolved {
        mint: parsed,
        destination,
    })
}

/// A transfer of `amount` of `mint`, in its smallest units, from the wallet
/// whose route is given to `to`, on a fresh blockhash. Returns it and the
/// height at which that blockhash expires.
///
/// The tokens leave the sender's own account of the mint that holds enough,
/// its associated one first. When the recipient's account has to be
/// created, a plain key's fee payer pays its rent. Any other wallet pays it
/// itself, since Sign moves every instruction inside its wrap, where only
/// the wallet signs.
pub async fn prepare(
    rpc: &dyn SolanaRpc,
    route: &SigningRoute,
    mint: Pubkey,
    to: Pubkey,
    amount: u64,
) -> Result<(Prepared, Option<u64>), FlowError> {
    if amount == 0 {
        return Err(FlowError::refused("Enter an amount above zero."));
    }
    let from = route.address;
    if to == from {
        return Err(FlowError::refused("The recipient is this wallet."));
    }
    let payer = fee_payer(route)?;
    let resolved = resolve(rpc, mint, to).await?;
    let program = resolved.mint.program;
    let decimals = resolved.mint.decimals;
    let source = source_account(rpc, from, mint, program, amount, decimals).await?;
    let destination = resolved.destination.account();
    if destination == source {
        return Err(FlowError::refused(
            "The recipient is the account the tokens leave from.",
        ));
    }

    let rent_payer = if is_plain_key(route) { payer } else { from };
    let mut instructions = Vec::with_capacity(2);
    if matches!(resolved.destination, Destination::Create { .. }) {
        instructions.push(create_ata_idempotent(rent_payer, to, mint, program));
    }
    instructions.push(transfer_checked(
        program,
        source,
        mint,
        destination,
        from,
        amount,
        decimals,
    ));
    let (wire, last_valid_height) = compile_for_route(rpc, route, payer, instructions).await?;

    let mut note = Phrase::default()
        .t(format!(
            "This sends {} of ",
            format_token_amount(amount, decimals)
        ))
        .a(mint)
        .t(" from ")
        .a(from)
        .t(" to ")
        .a(to)
        .t(".");
    if let Destination::Create { account, .. } = resolved.destination {
        note = note
            .t(" It first creates the recipient's token account ")
            .a(account)
            .t(", with its rent paid by ")
            .a(rent_payer)
            .t(".");
    }
    if let Some(bps) = resolved.mint.transfer_fee_bps {
        note = note.t(format!(
            " The mint keeps a fee of up to {}% of it, so the recipient gets less.",
            format_token_amount(u64::from(bps), 2)
        ));
    }
    let note = with_nonce_note(note, route);
    Ok((Prepared { wire, note }, last_valid_height))
}

async fn read_two(
    rpc: &dyn SolanaRpc,
    a: Pubkey,
    b: Pubkey,
) -> Result<[Option<wallet_core::RawAccount>; 2], FlowError> {
    let mut read = rpc.get_accounts(&[a, b]).await?.into_iter();
    Ok([read.next().flatten(), read.next().flatten()])
}

/// Refused when `account` cannot take a plain transfer.
fn receivable(account: &TokenAccount) -> Result<(), FlowError> {
    if account.frozen {
        return Err(FlowError::refused(
            "The recipient's token account is frozen.",
        ));
    }
    if account.requires_memo {
        return Err(FlowError::refused(
            "The recipient's token account only takes transfers with a memo, which this app cannot add yet.",
        ));
    }
    Ok(())
}

/// The size of a new associated account for `mint`: Token-2022 adds the
/// account type and the immutable owner extension, and the withheld fee
/// when the mint charges one.
fn account_len(mint: &Mint) -> usize {
    if mint.program != TOKEN_2022_PROGRAM_ID {
        return 165;
    }
    let fee = if mint.transfer_fee_bps.is_some() {
        12
    } else {
        0
    };
    170 + fee
}

/// The sender's account of `mint` that `amount` leaves from: its associated
/// one when it holds enough, otherwise the one that holds the most.
async fn source_account(
    rpc: &dyn SolanaRpc,
    from: Pubkey,
    mint: Pubkey,
    program: Pubkey,
    amount: u64,
    decimals: u8,
) -> Result<Pubkey, FlowError> {
    let held: Vec<_> = rpc
        .get_token_accounts_by_owner(&from)
        .await?
        .into_iter()
        .filter(|account| account.mint == mint)
        .collect();
    let ata = associated_token_address(&from, &mint, &program);
    if let Some(account) = held
        .iter()
        .find(|account| account.pubkey == ata && account.amount >= amount)
    {
        return Ok(account.pubkey);
    }
    let largest = held.iter().max_by_key(|account| account.amount);
    match largest {
        Some(account) if account.amount >= amount => Ok(account.pubkey),
        Some(account) => {
            let total: u64 = held.iter().map(|account| account.amount).sum();
            Err(FlowError::refused(if held.len() > 1 && total >= amount {
                format!(
                    "No single account of this wallet holds that much: the largest holds {}. Send at most that.",
                    format_token_amount(account.amount, decimals)
                )
            } else {
                format!(
                    "This wallet holds {} of this token.",
                    format_token_amount(total, decimals)
                )
            }))
        }
        None => Err(FlowError::refused("This wallet holds none of this token.")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{mint_account, pk, token_account, FakeRpc};
    use wallet_core::{
        decode_v1_transaction, DiscoveredTokenAccount, RouteStep, WalletId,
        ASSOCIATED_TOKEN_PROGRAM_ID, COMPUTE_BUDGET_PROGRAM_ID, TOKEN_PROGRAM_ID,
    };

    const MINT: u8 = 50;

    fn key_route(owner: Pubkey, payer: Option<Pubkey>) -> SigningRoute {
        SigningRoute {
            address: owner,
            steps: vec![RouteStep::OutsideKey { pubkey: owner }],
            fee_payer: payer.map(|payer| (WalletId::new(), payer)),
            durable_nonce: None,
        }
    }

    /// A wallet Sign wraps around, paid for by `payer`.
    fn wrapped_route(owner: Pubkey, payer: Pubkey) -> SigningRoute {
        SigningRoute {
            address: owner,
            steps: Vec::new(),
            fee_payer: Some((WalletId::new(), payer)),
            durable_nonce: None,
        }
    }

    /// `from` holds `amount` in its associated account of the mint.
    fn node(program: Pubkey, from: Pubkey, amount: u64) -> FakeRpc {
        let mint = pk(MINT);
        let ata = associated_token_address(&from, &mint, &program);
        FakeRpc::default()
            .with_account(mint, mint_account(program, 6))
            .with_tokens(
                from,
                vec![DiscoveredTokenAccount {
                    pubkey: ata,
                    owner: from,
                    mint,
                    amount,
                    decimals: 6,
                }],
            )
    }

    fn programs(prepared: &Prepared) -> Vec<Pubkey> {
        let tx = decode_v1_transaction(&prepared.wire).unwrap();
        tx.instructions
            .iter()
            .map(|ix| ix.program_id)
            .filter(|program| *program != COMPUTE_BUDGET_PROGRAM_ID)
            .collect()
    }

    #[test]
    fn a_recipient_without_an_account_gets_one_first() {
        let (from, to) = (pk(1), pk(2));
        let rpc = node(TOKEN_PROGRAM_ID, from, 5_000_000);
        let (prepared, _) = pollster::block_on(prepare(
            &rpc,
            &key_route(from, None),
            pk(MINT),
            to,
            1_500_000,
        ))
        .unwrap();
        assert_eq!(
            programs(&prepared),
            vec![ASSOCIATED_TOKEN_PROGRAM_ID, TOKEN_PROGRAM_ID]
        );
        let note = prepared.note.plain();
        assert!(note.contains("1.5 of"), "{note}");
        assert!(
            note.contains("creates the recipient's token account"),
            "{note}"
        );
    }

    #[test]
    fn an_existing_account_is_sent_to_as_it_is() {
        let (from, to) = (pk(1), pk(2));
        let ata = associated_token_address(&to, &pk(MINT), &TOKEN_2022_PROGRAM_ID);
        let rpc = node(TOKEN_2022_PROGRAM_ID, from, 10)
            .with_account(ata, token_account(TOKEN_2022_PROGRAM_ID, pk(MINT), to, 0));
        let resolved = pollster::block_on(resolve(&rpc, pk(MINT), to)).unwrap();
        assert_eq!(resolved.destination, Destination::Existing { account: ata });
        let (prepared, _) =
            pollster::block_on(prepare(&rpc, &key_route(from, None), pk(MINT), to, 10)).unwrap();
        assert_eq!(programs(&prepared), vec![TOKEN_2022_PROGRAM_ID]);
    }

    #[test]
    fn a_token_account_of_the_mint_receives_directly() {
        let (from, account, owner) = (pk(1), pk(3), pk(4));
        let rpc = node(TOKEN_PROGRAM_ID, from, 10)
            .with_account(account, token_account(TOKEN_PROGRAM_ID, pk(MINT), owner, 0));
        let resolved = pollster::block_on(resolve(&rpc, pk(MINT), account)).unwrap();
        assert_eq!(
            resolved.destination,
            Destination::TokenAccount { account, owner }
        );
    }

    #[test]
    fn recipients_that_cannot_receive_are_refused() {
        let from = pk(1);
        // Another token's account.
        let rpc = node(TOKEN_PROGRAM_ID, from, 10)
            .with_account(pk(3), token_account(TOKEN_PROGRAM_ID, pk(51), pk(4), 0))
            .with_account(pk(5), mint_account(TOKEN_PROGRAM_ID, 0));
        let err = pollster::block_on(resolve(&rpc, pk(MINT), pk(3))).unwrap_err();
        assert!(err.to_string().contains("another token"));
        // A mint.
        let err = pollster::block_on(resolve(&rpc, pk(MINT), pk(5))).unwrap_err();
        assert!(err.to_string().contains("mint"));
        // Not a mint at all.
        let err = pollster::block_on(resolve(&rpc, pk(6), pk(2))).unwrap_err();
        assert!(err.to_string().contains("not a token mint"));
        // An associated account handed to someone else.
        let to = pk(2);
        let ata = associated_token_address(&to, &pk(MINT), &TOKEN_PROGRAM_ID);
        let rpc = node(TOKEN_PROGRAM_ID, from, 10)
            .with_account(ata, token_account(TOKEN_PROGRAM_ID, pk(MINT), pk(7), 0));
        let err = pollster::block_on(resolve(&rpc, pk(MINT), to)).unwrap_err();
        assert!(err.to_string().contains("another owner"));
    }

    #[test]
    fn a_wrapped_wallet_pays_the_rent_itself() {
        let (vault, hot, to) = (pk(1), pk(9), pk(2));
        let rpc = node(TOKEN_PROGRAM_ID, vault, 10);
        let (prepared, _) =
            pollster::block_on(prepare(&rpc, &wrapped_route(vault, hot), pk(MINT), to, 10))
                .unwrap();
        let tx = decode_v1_transaction(&prepared.wire).unwrap();
        assert_eq!(tx.fee_payer(), Some(hot));
        let create = token_instructions(&tx)[0];
        assert_eq!(create.accounts[0], vault);

        // A plain key's fee payer pays it.
        let (prepared, _) = pollster::block_on(prepare(
            &rpc_for_key(),
            &key_route(pk(1), Some(hot)),
            pk(MINT),
            to,
            10,
        ))
        .unwrap();
        let tx = decode_v1_transaction(&prepared.wire).unwrap();
        let create = token_instructions(&tx)[0];
        assert_eq!(create.accounts[0], hot);
    }

    fn token_instructions(tx: &wallet_core::DecodedTx) -> Vec<&wallet_core::RawInstruction> {
        tx.instructions
            .iter()
            .filter(|ix| ix.program_id != COMPUTE_BUDGET_PROGRAM_ID)
            .collect()
    }

    fn rpc_for_key() -> FakeRpc {
        node(TOKEN_PROGRAM_ID, pk(1), 10)
    }

    #[test]
    fn the_source_must_hold_enough_in_one_account() {
        let (from, to) = (pk(1), pk(2));
        let mint = pk(MINT);
        let held = |pubkey: Pubkey, amount: u64| DiscoveredTokenAccount {
            pubkey,
            owner: from,
            mint,
            amount,
            decimals: 6,
        };
        let rpc = FakeRpc::default()
            .with_account(mint, mint_account(TOKEN_PROGRAM_ID, 6))
            .with_tokens(from, vec![held(pk(20), 3), held(pk(21), 4)]);
        let route = key_route(from, None);
        // The larger account pays when the associated one is not there.
        let (prepared, _) = pollster::block_on(prepare(&rpc, &route, mint, to, 4)).unwrap();
        let tx = decode_v1_transaction(&prepared.wire).unwrap();
        let transfer = *token_instructions(&tx).last().unwrap();
        assert_eq!(transfer.accounts[0], pk(21));

        let err = pollster::block_on(prepare(&rpc, &route, mint, to, 6)).unwrap_err();
        assert!(err.to_string().contains("No single account"), "{err}");
        let err = pollster::block_on(prepare(&rpc, &route, mint, to, 8)).unwrap_err();
        assert!(err.to_string().contains("holds 0.000007"), "{err}");
        assert!(pollster::block_on(prepare(&rpc, &route, mint, to, 0)).is_err());
        assert!(pollster::block_on(prepare(&rpc, &route, mint, from, 1)).is_err());
    }
}
