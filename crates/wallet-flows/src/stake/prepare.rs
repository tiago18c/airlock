//! Preparing a stake operation: its instructions, compiled and described.

use super::read::free_stake_index;
use crate::compile::{compile_for_route, with_nonce_note};
use crate::error::{FlowError, Prepared};
use solana_pubkey::Pubkey;
use wallet_core::stake::{
    authorize_stake, create_stake, deactivate_stake, delegate_stake, merge_stake, split_stake,
    withdraw_stake, StakeAccount, StakeRole,
};
use wallet_core::{format_sol, Phrase, SigningRoute, SolanaRpc};

/// One stake operation, for the wallet whose route is given to [`prepare`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StakeRequest {
    /// A new stake account, delegated to `vote` when one is given.
    New {
        lamports: u64,
        vote: Option<Pubkey>,
    },
    Delegate {
        stake: Pubkey,
        vote: Pubkey,
    },
    Deactivate {
        stake: Pubkey,
    },
    Withdraw {
        stake: Pubkey,
        lamports: u64,
        to: Pubkey,
    },
    /// Move `lamports` out of `account` into a new stake account.
    Split {
        account: StakeAccount,
        lamports: u64,
    },
    /// Merge `source` into `stake`, which closes `source`.
    Merge {
        stake: Pubkey,
        source: Pubkey,
    },
    Authorize {
        stake: Pubkey,
        new_authority: Pubkey,
        role: StakeRole,
    },
}

/// Compile `request` on a fresh blockhash, or the wallet's durable nonce,
/// for the wallet that signs through `route`. `rent_reserve` is what a stake account has to keep to exist.
pub async fn prepare(
    rpc: &dyn SolanaRpc,
    route: &SigningRoute,
    request: StakeRequest,
    rent_reserve: u64,
) -> Result<Prepared, FlowError> {
    let Some((_, payer)) = route.fee_payer else {
        return Err(FlowError::NoFeePayer);
    };
    let owner = route.address;
    let (instructions, note) = match request {
        StakeRequest::New { lamports, vote } => {
            let index = free_stake_index(rpc, &owner).await?;
            let created = create_stake(&owner, index, lamports, rent_reserve, vote.as_ref())
                .map_err(FlowError::refused)?;
            let note = Phrase::default()
                .t("This creates stake account ")
                .a(created.address)
                .t(format!(" with {}", format_sol(lamports)));
            let note = match vote {
                Some(vote) => note.t(" and delegates it to ").a(vote).t("."),
                None => note.t(". It is not delegated yet."),
            };
            (created.instructions, note)
        }
        StakeRequest::Delegate { stake, vote } => (
            delegate_stake(&stake, &owner, &vote),
            Phrase::default()
                .t("This delegates stake account ")
                .a(stake)
                .t(" to ")
                .a(vote)
                .t("."),
        ),
        StakeRequest::Deactivate { stake } => (
            deactivate_stake(&stake, &owner),
            Phrase::default()
                .t("This deactivates stake account ")
                .a(stake)
                .t(". It can be withdrawn once the epoch ends."),
        ),
        StakeRequest::Withdraw {
            stake,
            lamports,
            to,
        } => (
            withdraw_stake(&stake, &owner, &to, lamports),
            Phrase::default()
                .t(format!(
                    "This withdraws {} from stake account ",
                    format_sol(lamports)
                ))
                .a(stake)
                .t(" to ")
                .a(to)
                .t("."),
        ),
        StakeRequest::Split { account, lamports } => {
            let index = free_stake_index(rpc, &owner).await?;
            let split = split_stake(&account, lamports, index).map_err(FlowError::refused)?;
            let note = Phrase::default()
                .t(format!(
                    "This moves {} from stake account ",
                    format_sol(lamports)
                ))
                .a(account.pubkey)
                .t(" into the new stake account ")
                .a(split.address)
                .t(".");
            (split.instructions, note)
        }
        StakeRequest::Merge { stake, source } => (
            merge_stake(&stake, &source, &owner),
            Phrase::default()
                .t("This merges stake account ")
                .a(source)
                .t(" into ")
                .a(stake)
                .t(" and closes it."),
        ),
        StakeRequest::Authorize {
            stake,
            new_authority,
            role,
        } => (
            authorize_stake(&stake, &owner, &new_authority, role),
            Phrase::default()
                .t("This makes ")
                .a(new_authority)
                .t(format!(" the {} of stake account ", role_name(role)))
                .a(stake)
                .t("."),
        ),
    };
    let (wire, _) = compile_for_route(rpc, route, payer, instructions).await?;
    let note = with_nonce_note(note, route);
    Ok(Prepared { wire, note })
}

/// What each authority is called on screen.
pub fn role_name(role: StakeRole) -> &'static str {
    match role {
        StakeRole::Staker => "stake authority",
        StakeRole::Withdrawer => "withdraw authority",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{pk, FakeRpc};
    use wallet_core::stake::{stake_address, STAKE_RENT_LAMPORTS};
    use wallet_core::{decode_v1_transaction, RouteStep, WalletId};

    fn route(owner: Pubkey, payer: Option<Pubkey>) -> SigningRoute {
        SigningRoute {
            address: owner,
            steps: vec![RouteStep::OutsideKey { pubkey: owner }],
            fee_payer: payer.map(|payer| (WalletId::new(), payer)),
            durable_nonce: None,
        }
    }

    #[test]
    fn a_new_stake_goes_to_the_first_free_seed_address_and_the_fee_payer_pays() {
        let (owner, payer) = (pk(1), pk(2));
        let taken = stake_address(&owner, 0).unwrap();
        let rpc = FakeRpc::default().with_balance(taken, 1);
        let prepared = pollster::block_on(prepare(
            &rpc,
            &route(owner, Some(payer)),
            StakeRequest::New {
                lamports: 5_000_000_000,
                vote: None,
            },
            STAKE_RENT_LAMPORTS,
        ))
        .unwrap();
        let created = stake_address(&owner, 1).unwrap();
        assert!(prepared
            .note
            .0
            .contains(&wallet_core::Part::Address(created)));
        assert!(prepared.note.plain().contains("5 SOL"));
        let tx = decode_v1_transaction(&prepared.wire).unwrap();
        assert_eq!(tx.fee_payer(), Some(payer));
        assert!(tx.accounts.iter().any(|account| account.pubkey == created));
    }

    #[test]
    fn a_wallet_nobody_pays_for_is_refused_before_the_node_is_asked() {
        let rpc = FakeRpc::default();
        let err = pollster::block_on(prepare(
            &rpc,
            &route(pk(1), None),
            StakeRequest::Deactivate { stake: pk(9) },
            STAKE_RENT_LAMPORTS,
        ))
        .unwrap_err();
        assert_eq!(err, FlowError::NoFeePayer);
        assert_eq!(rpc.asked(), 0);
    }

    #[test]
    fn an_operation_on_an_account_names_it_and_needs_only_a_blockhash() {
        let rpc = FakeRpc::default();
        let prepared = pollster::block_on(prepare(
            &rpc,
            &route(pk(1), Some(pk(1))),
            StakeRequest::Authorize {
                stake: pk(9),
                new_authority: pk(3),
                role: StakeRole::Withdrawer,
            },
            STAKE_RENT_LAMPORTS,
        ))
        .unwrap();
        assert!(prepared.note.plain().contains("withdraw authority"));
        assert_eq!(rpc.asked(), 1);
    }
}
