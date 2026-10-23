//! Squads multisigs: reading one, creating one, and paging through its
//! proposals.

use crate::compile::compile_fresh;
use crate::error::FlowError;
use solana_pubkey::Pubkey;
use wallet_core::proposals::{page_accounts, page_indexes, read_page, ProposalEntry};
use wallet_core::squads::{
    Squad, SquadState, SquadsMember, SquadsProgram, SquadsSetup, PERMISSION_ALL,
};
use wallet_core::{apply_local_signatures, KeypairSigner, SolanaRpc, WalletSigner};

/// What a node says about an address that should be a multisig.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lookup {
    Found(SquadState),
    /// The node answered, and the address is not a multisig of that program.
    NotAMultisig,
    /// The node could not be asked. Members and threshold stay unknown.
    Unreachable,
}

/// Read the members and threshold of `squad`. An unreachable node is not an
/// error: a multisig can be added without knowing them.
pub async fn look_up(rpc: Option<&dyn SolanaRpc>, squad: &Squad) -> Lookup {
    let Some(rpc) = rpc else {
        return Lookup::Unreachable;
    };
    match rpc.get_account(&squad.address).await {
        Ok(Some(account)) if account.owner == squad.program.program_id() => squad
            .parse_state(&account.data)
            .map_or(Lookup::Unreachable, Lookup::Found),
        Ok(_) => Lookup::NotAMultisig,
        Err(_) => Lookup::Unreachable,
    }
}

/// A multisig that does not exist yet, and the transaction that creates it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewSquad {
    pub squad: Squad,
    /// Paid by the creator, who still has to sign it. Anything else that has
    /// to sign has signed already.
    pub wire: Vec<u8>,
}

/// Build the transaction that creates a multisig of `members`, each of whom
/// may propose, approve and execute. `creator` pays.
pub async fn create(
    rpc: &dyn SolanaRpc,
    program: SquadsProgram,
    members: &[Pubkey],
    threshold: u16,
    creator: Pubkey,
) -> Result<NewSquad, FlowError> {
    let config = rpc
        .get_account(&program.program_config())
        .await?
        .ok_or_else(|| FlowError::Refused("This program is not set up on this cluster.".into()))?;
    let unreadable = || FlowError::Refused("The program's settings could not be read.".into());
    let treasury = program.treasury(&config.data).ok_or_else(unreadable)?;
    let setup = SquadsSetup {
        members: members
            .iter()
            .map(|key| SquadsMember {
                key: *key,
                permissions: PERMISSION_ALL,
            })
            .collect(),
        threshold,
        time_lock: 0,
    };

    match program {
        SquadsProgram::V4 => {
            // The multisig's address comes from a key used once, here.
            let create_key = KeypairSigner::new_random();
            let (squad, instruction) =
                Squad::create_v4(&create_key.pubkey(), &creator, &treasury, &setup);
            let (wire, _) = compile_fresh(rpc, creator, vec![instruction]).await?;
            let wire = apply_local_signatures(&wire, &[&create_key as &dyn WalletSigner])
                .map_err(FlowError::refused)?;
            Ok(NewSquad { squad, wire })
        }
        SquadsProgram::SmartAccount => {
            let count = SquadsProgram::smart_account_count(&config.data).ok_or_else(unreadable)?;
            let (squad, instruction) =
                Squad::create_smart_account(count + 1, &creator, &treasury, &setup);
            let (wire, _) = compile_fresh(rpc, creator, vec![instruction]).await?;
            Ok(NewSquad { squad, wire })
        }
    }
}

/// Read the multisig, and one page of its transactions starting at `from`,
/// or at the newest when `from` is `None`.
pub async fn load_page(
    rpc: &dyn SolanaRpc,
    squad: &Squad,
    from: Option<u64>,
) -> Result<(SquadState, Vec<ProposalEntry>), FlowError> {
    let state = rpc
        .get_account(&squad.address)
        .await?
        .filter(|account| account.owner == squad.program.program_id())
        .and_then(|account| squad.parse_state(&account.data))
        .ok_or_else(|| {
            FlowError::Refused("The multisig account could not be read on this cluster.".into())
        })?;
    let indexes = page_indexes(from.unwrap_or(state.transaction_index));
    let accounts = rpc.get_accounts(&page_accounts(squad, &indexes)).await?;
    let entries = read_page(squad, &state, &indexes, &accounts);
    Ok((state, entries))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{pk, FakeRpc};

    #[test]
    fn an_address_that_is_not_a_multisig_is_told_apart_from_a_node_that_is_down() {
        let squad = Squad::v4(&pk(9));
        let rpc = FakeRpc::default();
        assert_eq!(
            pollster::block_on(look_up(Some(&rpc), &squad)),
            Lookup::NotAMultisig
        );
        assert_eq!(
            pollster::block_on(look_up(None, &squad)),
            Lookup::Unreachable
        );
        let offline = FakeRpc {
            offline: true,
            ..FakeRpc::default()
        };
        assert_eq!(
            pollster::block_on(look_up(Some(&offline), &squad)),
            Lookup::Unreachable
        );
    }

    #[test]
    fn creating_needs_the_program_to_be_set_up_on_the_cluster() {
        let err = pollster::block_on(create(
            &FakeRpc::default(),
            SquadsProgram::V4,
            &[pk(1)],
            1,
            pk(1),
        ))
        .unwrap_err();
        assert!(err.to_string().contains("not set up on this cluster"));
    }

    #[test]
    fn a_page_needs_the_multisig_account() {
        let err = pollster::block_on(load_page(&FakeRpc::default(), &Squad::v4(&pk(9)), None))
            .unwrap_err();
        assert!(err.to_string().contains("could not be read"));
    }
}
