//! Squads multisigs: reading one, creating one, and paging through its
//! proposals.

use crate::compile::compile_fresh;
use crate::error::FlowError;
use solana_pubkey::Pubkey;
use std::collections::HashMap;
use wallet_core::proposals::{page_accounts, page_indexes, read_page, ProposalEntry};
use wallet_core::squads::{
    Squad, SquadState, SquadsMember, SquadsProgram, SquadsSetup, PERMISSION_ALL,
};
use wallet_core::{
    apply_local_signatures, stored_in, KeypairSigner, SolanaRpc, StoredTarget, WalletSigner,
};

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

/// What the node says is stored behind `targets`, by address: the raw bytes, to
/// show and to pass on, of what fits what each acts on.
pub async fn read_stored(
    rpc: &dyn SolanaRpc,
    targets: &[StoredTarget],
) -> Result<HashMap<Pubkey, Vec<u8>>, FlowError> {
    // The proposal holds the number that says where its transaction is.
    let proposals: Vec<Pubkey> = targets.iter().map(|target| target.proposal).collect();
    let read = rpc.get_accounts(&proposals).await?;
    let asked: Vec<(StoredTarget, Pubkey)> = targets
        .iter()
        .zip(read)
        .filter_map(|(target, account)| {
            let account =
                account.filter(|account| account.owner == target.squad.program.program_id())?;
            let proposal = target.squad.parse_proposal(&account.data)?;
            Some((*target, target.squad.transaction(proposal.index)))
        })
        .collect();
    let addresses: Vec<Pubkey> = asked.iter().map(|(_, address)| *address).collect();
    let read = rpc.get_accounts(&addresses).await?;
    let mut found = HashMap::new();
    for ((target, address), account) in asked.into_iter().zip(read) {
        let Some(account) =
            account.filter(|account| account.owner == target.squad.program.program_id())
        else {
            continue;
        };
        let one = HashMap::from([(address, account.data)]);
        if stored_in(&target, &one).is_some() {
            found.extend(one);
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{pk, FakeRpc};
    use wallet_core::RawAccount;

    /// A v4 multisig on a local chain, as wallet-core's tests read it: what the
    /// program wrote for its transaction 1, and that transaction's proposal.
    const MULTISIG: &str = "d9a5e44daa78c870fe9cc9c9cd9b18266e9f23d17ba3babdb928f35f539137b7";
    const TRANSACTION: &str = "a8faa264510ea2cfd9a5e44daa78c870fe9cc9c9cd9b18266e9f23d17ba3babdb928f35f539137b735a8380093a2d2386c1b25d1024507dbc5ae3ac66a3e65a3f6f192dee20896d30100000000000000ff00ff0000000001010103000000cd14d8623ad6e0db012f89806e133cc3326376b5c15ff369d4e7df8821d6eb07e2649cd17279322c1945b9c13b9d8f063e52f3723d3e70d81b9f339357aa3670000000000000000000000000000000000000000000000000000000000000000001000000020200000000010c00000002000000443322110000000000000000";
    const PROPOSAL: &str = "1a5ebdbb74883521d9a5e44daa78c870fe9cc9c9cd9b18266e9f23d17ba3babdb928f35f539137b70100000000000000010000000000000000ff0100000035a8380093a2d2386c1b25d1024507dbc5ae3ac66a3e65a3f6f192dee20896d30000000000000000";

    fn written() -> (Squad, StoredTarget, Vec<u8>, Vec<u8>) {
        let address: [u8; 32] = hex::decode(MULTISIG).unwrap().try_into().unwrap();
        let squad = Squad {
            program: SquadsProgram::V4,
            address: Pubkey::new_from_array(address),
        };
        let target = StoredTarget {
            squad,
            proposal: squad.proposal(1),
            transaction: None,
        };
        let stored = hex::decode(TRANSACTION).unwrap();
        (squad, target, stored, hex::decode(PROPOSAL).unwrap())
    }

    fn owned_by(owner: Pubkey, data: Vec<u8>) -> RawAccount {
        RawAccount {
            lamports: 1,
            data,
            owner,
            executable: false,
            rent_epoch: 0,
        }
    }

    fn node(accounts: Vec<(Pubkey, RawAccount)>) -> FakeRpc {
        FakeRpc {
            accounts: accounts.into_iter().collect(),
            ..FakeRpc::default()
        }
    }

    fn read(rpc: &FakeRpc, target: StoredTarget) -> Result<HashMap<Pubkey, Vec<u8>>, FlowError> {
        pollster::block_on(read_stored(rpc, &[target]))
    }

    #[test]
    fn what_a_proposal_holds_is_found_through_the_number_it_keeps() {
        let (squad, target, stored, proposal) = written();
        let program = squad.program.program_id();
        let rpc = node(vec![
            (squad.proposal(1), owned_by(program, proposal)),
            (squad.transaction(1), owned_by(program, stored.clone())),
        ]);
        assert_eq!(
            read(&rpc, target).unwrap(),
            HashMap::from([(squad.transaction(1), stored)])
        );
    }

    #[test]
    fn only_what_the_program_owns_and_what_fits_is_taken() {
        let (squad, target, stored, proposal) = written();
        let program = squad.program.program_id();

        // Another program holds the proposal, or the stored transaction.
        let rpc = node(vec![
            (squad.proposal(1), owned_by(pk(9), proposal.clone())),
            (squad.transaction(1), owned_by(program, stored.clone())),
        ]);
        assert!(read(&rpc, target).unwrap().is_empty());
        let rpc = node(vec![
            (squad.proposal(1), owned_by(program, proposal.clone())),
            (squad.transaction(1), owned_by(pk(9), stored.clone())),
        ]);
        assert!(read(&rpc, target).unwrap().is_empty());

        // A proposal at the address asked about that keeps the number of a
        // proposal somewhere else: what that points at is not what was asked.
        let elsewhere = StoredTarget {
            proposal: pk(5),
            ..target
        };
        let rpc = node(vec![
            (pk(5), owned_by(program, proposal.clone())),
            (squad.transaction(1), owned_by(program, stored.clone())),
        ]);
        assert!(read(&rpc, elsewhere).unwrap().is_empty());

        // The stored transaction is not one this app reads.
        let rpc = node(vec![
            (squad.proposal(1), owned_by(program, proposal.clone())),
            (squad.transaction(1), owned_by(program, proposal)),
        ]);
        assert!(read(&rpc, target).unwrap().is_empty());
    }

    #[test]
    fn a_node_that_has_nothing_is_told_apart_from_one_that_cannot_be_asked() {
        let (_, target, _, _) = written();
        assert!(read(&FakeRpc::default(), target).unwrap().is_empty());
        let down = FakeRpc {
            offline: true,
            ..FakeRpc::default()
        };
        assert!(read(&down, target).is_err());
    }

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
