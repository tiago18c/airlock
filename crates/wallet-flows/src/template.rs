//! Reading from the node the chain facts a template needs: rent for a size,
//! and a mint's decimals and token program.

use crate::FlowError;
use solana_pubkey::Pubkey;
use std::collections::BTreeSet;
use wallet_core::spl::Mint;
use wallet_core::template::{Fact, Facts};
use wallet_core::SolanaRpc;

/// Answers `needs` from the node: rent one size at a time, as the node
/// answers it, and every mint in one request. A mint that does not exist, or
/// is not a mint of Token or Token-2022, refuses the lot and names it.
pub async fn template_facts(rpc: &dyn SolanaRpc, needs: &[Fact]) -> Result<Facts, FlowError> {
    let mut facts = Facts::default();
    let mut sizes = BTreeSet::new();
    let mut mints = BTreeSet::new();
    for fact in needs {
        match fact {
            Fact::Rent(bytes) => {
                sizes.insert(*bytes);
            }
            Fact::MintDecimals(mint) | Fact::TokenProgram(mint) => {
                mints.insert(*mint);
            }
        }
    }

    for bytes in sizes {
        let len = usize::try_from(bytes).map_err(|_| {
            FlowError::Refused(format!("{bytes} bytes is more than an account can hold."))
        })?;
        facts.rent.insert(bytes, rpc.get_rent_exemption(len).await?);
    }

    let mints: Vec<Pubkey> = mints.into_iter().collect();
    let accounts = rpc.get_accounts(&mints).await?;
    for (mint, account) in mints.iter().zip(accounts) {
        let account = account.ok_or_else(|| {
            FlowError::Refused(format!(
                "There is no account at {mint}. Check the mint address."
            ))
        })?;
        let parsed = Mint::parse(&account).ok_or_else(|| {
            FlowError::Refused(format!(
                "{mint} is not a mint of the Token or Token-2022 program."
            ))
        })?;
        facts.decimals.insert(*mint, parsed.decimals);
        facts.token_programs.insert(*mint, parsed.program);
    }
    Ok(facts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{pk, FakeRpc};
    use wallet_core::{RawAccount, SYSTEM_PROGRAM_ID, TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID};

    fn mint(owner: Pubkey, decimals: u8) -> RawAccount {
        let mut data = vec![0u8; 82];
        data[44] = decimals;
        data[45] = 1;
        RawAccount {
            lamports: 1_461_600,
            data,
            owner,
            executable: false,
            rent_epoch: 0,
        }
    }

    #[test]
    fn rent_and_mints_are_read_with_one_request_for_every_mint() {
        let rpc = FakeRpc::default()
            .with_account(pk(1), mint(TOKEN_PROGRAM_ID, 6))
            .with_account(pk(2), mint(TOKEN_2022_PROGRAM_ID, 9));
        let needs = [
            Fact::Rent(82),
            Fact::MintDecimals(pk(1)),
            Fact::TokenProgram(pk(1)),
            Fact::TokenProgram(pk(2)),
            Fact::Rent(82),
        ];
        let facts = pollster::block_on(template_facts(&rpc, &needs)).unwrap();
        assert_eq!(facts.rent[&82], 2_282_880);
        assert_eq!(facts.decimals[&pk(1)], 6);
        assert_eq!(facts.token_programs[&pk(1)], TOKEN_PROGRAM_ID);
        assert_eq!(facts.token_programs[&pk(2)], TOKEN_2022_PROGRAM_ID);
        assert!(needs.iter().all(|fact| facts.knows(fact)));
        // One rent request and one for both mints.
        assert_eq!(rpc.asked(), 2);
    }

    #[test]
    fn nothing_needed_asks_nothing() {
        let rpc = FakeRpc::default();
        let facts = pollster::block_on(template_facts(&rpc, &[])).unwrap();
        assert_eq!(facts, Facts::default());
        assert_eq!(rpc.asked(), 0);
    }

    #[test]
    fn a_missing_account_or_one_that_is_not_a_mint_is_refused_by_address() {
        let rpc = FakeRpc::default();
        let err =
            pollster::block_on(template_facts(&rpc, &[Fact::MintDecimals(pk(3))])).unwrap_err();
        assert_eq!(
            err,
            FlowError::Refused(format!(
                "There is no account at {}. Check the mint address.",
                pk(3)
            ))
        );

        let rpc = FakeRpc::default().with_account(pk(4), mint(SYSTEM_PROGRAM_ID, 6));
        let err =
            pollster::block_on(template_facts(&rpc, &[Fact::TokenProgram(pk(4))])).unwrap_err();
        assert!(
            err.to_string().contains("is not a mint of the Token"),
            "{err}"
        );
    }

    #[test]
    fn a_node_that_cannot_be_reached_is_an_rpc_error() {
        let rpc = FakeRpc {
            offline: true,
            ..FakeRpc::default()
        };
        let err = pollster::block_on(template_facts(&rpc, &[Fact::Rent(82)])).unwrap_err();
        assert!(matches!(err, FlowError::Rpc(_)), "{err:?}");
    }
}
