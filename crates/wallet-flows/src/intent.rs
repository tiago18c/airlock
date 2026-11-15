//! What resolving an intent asks a node: whether a `new` account is new to
//! the chain, and whether a proposal made for a request has run.

use crate::error::FlowError;
use crate::request::one_time_signers;
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use wallet_core::intent::{proposal_opened, AccountNote, NoteError, Notes};
use wallet_core::squads::ProposalStatus;
use wallet_core::{decode_transaction, parse_wire, LookupTables, SolanaRpc};

/// Check 3's second half: the chain has never seen an account the notes
/// mark `new`. `Ok(Some(_))` names the first one it has seen, `Ok(None)`
/// when none. A node that does not answer is an error; with no node at all
/// the check is skipped, so the caller does not call this.
pub async fn new_accounts_unseen(
    rpc: &dyn SolanaRpc,
    notes: &Notes,
) -> Result<Option<NoteError>, FlowError> {
    let new: Vec<Pubkey> = notes
        .accounts
        .iter()
        .filter(|(_, note)| matches!(note, AccountNote::New { .. }))
        .map(|(address, _)| *address)
        .collect();
    let unseen = one_time_signers(rpc, &new).await?;
    Ok(new
        .into_iter()
        .find(|address| !unseen.contains(address))
        .map(|address| NoteError::SeenOnChain { address }))
}

/// Where a proposal made for a request stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProposalOutcome {
    /// Open for votes, approved and waiting to be executed, or executing.
    Waiting,
    /// It ran. `signature` is the transaction it ran in, when the node's
    /// history of the stored transaction shows one.
    Executed {
        signature: Option<Signature>,
    },
    Rejected,
    Cancelled,
    /// The node holds no proposal account there: the transaction that
    /// creates it has not landed, or it was closed since.
    Missing,
}

/// Where the proposal that `wire`, a transaction the wallet composed,
/// opened stands now. Refused when `wire` opens none.
///
/// The transaction it ran in is not kept by the proposal. It is taken as
/// the newest successful transaction in the stored transaction account's
/// history that is not `wire` itself: the execution names that account,
/// and nothing after it does unless the account is closed.
pub async fn follow_proposal(
    rpc: &dyn SolanaRpc,
    wire: &[u8],
) -> Result<ProposalOutcome, FlowError> {
    let tx = decode_transaction(wire, &LookupTables::new()).map_err(FlowError::refused)?;
    let target = proposal_opened(&tx)
        .ok_or_else(|| FlowError::Refused("This transaction opens no Squads proposal.".into()))?;
    let program = target.squad.program.program_id();
    let Some(proposal) = rpc
        .get_account(&target.proposal)
        .await?
        .filter(|account| account.owner == program)
        .and_then(|account| target.squad.parse_proposal(&account.data))
    else {
        return Ok(ProposalOutcome::Missing);
    };
    Ok(match proposal.status {
        ProposalStatus::Draft
        | ProposalStatus::Active
        | ProposalStatus::Approved
        | ProposalStatus::Executing => ProposalOutcome::Waiting,
        ProposalStatus::Rejected => ProposalOutcome::Rejected,
        ProposalStatus::Cancelled => ProposalOutcome::Cancelled,
        ProposalStatus::Executed => {
            let created = parse_wire(wire).ok().and_then(|view| view.signature());
            let stored = target.squad.transaction(proposal.index);
            let history = rpc.get_address_history(&stored, 10, None).await?;
            let signature = history
                .into_iter()
                .find(|item| item.err.is_none() && Some(item.signature) != created)
                .map(|item| item.signature);
            ProposalOutcome::Executed { signature }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{pk, transfer_instruction, FakeRpc};
    use solana_hash::Hash;
    use wallet_core::intent::{tx_role, TxRole};
    use wallet_core::squads::{Squad, SquadsProgram};
    use wallet_core::{compile_v1, AddressSignature, RawAccount};

    fn notes_with_new(addresses: &[Pubkey]) -> Notes {
        Notes {
            accounts: addresses
                .iter()
                .map(|address| (*address, AccountNote::New { replaceable: true }))
                .collect(),
            data: Vec::new(),
        }
    }

    #[test]
    fn a_new_account_the_chain_has_seen_is_named() {
        let rpc = FakeRpc {
            used: vec![pk(3)],
            ..FakeRpc::default()
        };
        let none = pollster::block_on(new_accounts_unseen(&rpc, &notes_with_new(&[pk(2)])));
        assert_eq!(none.unwrap(), None);
        let seen = pollster::block_on(new_accounts_unseen(&rpc, &notes_with_new(&[pk(2), pk(3)])));
        assert_eq!(
            seen.unwrap(),
            Some(NoteError::SeenOnChain { address: pk(3) })
        );
        let down = FakeRpc {
            offline: true,
            ..FakeRpc::default()
        };
        assert!(pollster::block_on(new_accounts_unseen(&down, &notes_with_new(&[pk(2)]))).is_err());
    }

    /// The v4 multisig of `wallet_flows::squads`' tests, its transaction 1's
    /// proposal as the program wrote it, open for votes.
    const MULTISIG: &str = "d9a5e44daa78c870fe9cc9c9cd9b18266e9f23d17ba3babdb928f35f539137b7";
    const PROPOSAL: &str = "1a5ebdbb74883521d9a5e44daa78c870fe9cc9c9cd9b18266e9f23d17ba3babdb928f35f539137b70100000000000000010000000000000000ff0100000035a8380093a2d2386c1b25d1024507dbc5ae3ac66a3e65a3f6f192dee20896d30000000000000000";
    /// Where the status sits in [`PROPOSAL`]: after the tag, the multisig
    /// and the transaction number.
    const STATUS_AT: usize = 8 + 32 + 8;

    fn squad() -> Squad {
        Squad {
            program: SquadsProgram::V4,
            address: Pubkey::new_from_array(hex::decode(MULTISIG).unwrap().try_into().unwrap()),
        }
    }

    /// The proposal with status number `status`: 1 is Active, 2 Rejected,
    /// 5 Executed and 6 Cancelled, each with a timestamp after it.
    fn proposal_with(status: u8) -> RawAccount {
        let mut data = hex::decode(PROPOSAL).unwrap();
        data[STATUS_AT] = status;
        RawAccount {
            lamports: 1,
            data,
            owner: SquadsProgram::V4.program_id(),
            executable: false,
            rent_epoch: 0,
        }
    }

    /// What the wallet composes for a vault send: store it as transaction 1,
    /// open its proposal and approve it, with `member` paying.
    fn proposal_wire(member: Pubkey) -> Vec<u8> {
        let squad = squad();
        let vault = squad.vault(0);
        let store = squad
            .create_transaction(
                1,
                0,
                0,
                &[transfer_instruction(vault, pk(9))],
                &member,
                &member,
            )
            .unwrap();
        let open = squad.create_proposal(1, &member, &member);
        let approve = squad.approve(1, &member);
        compile_v1(
            member,
            vec![store, open, approve],
            None,
            Some(Hash::default()),
        )
        .unwrap()
        .wire
    }

    fn history(signatures: &[(u8, Option<&str>)]) -> Vec<AddressSignature> {
        signatures
            .iter()
            .map(|(byte, err)| AddressSignature {
                signature: Signature::from([*byte; 64]),
                slot: 1,
                block_time: None,
                err: err.map(str::to_string),
            })
            .collect()
    }

    #[test]
    fn a_proposal_is_followed_until_it_runs_with_the_transaction_it_ran_in() {
        let wire = proposal_wire(pk(1));
        let tx = decode_transaction(&wire, &LookupTables::new()).unwrap();
        assert_eq!(tx_role(&tx, &squad().vault(0)), Some(TxRole::Proposal));
        let follow = |rpc: &FakeRpc| pollster::block_on(follow_proposal(rpc, &wire)).unwrap();

        assert_eq!(follow(&FakeRpc::default()), ProposalOutcome::Missing);
        let open = FakeRpc::default().with_account(squad().proposal(1), proposal_with(1));
        assert_eq!(follow(&open), ProposalOutcome::Waiting);
        let rejected = FakeRpc::default().with_account(squad().proposal(1), proposal_with(2));
        assert_eq!(follow(&rejected), ProposalOutcome::Rejected);
        let cancelled = FakeRpc::default().with_account(squad().proposal(1), proposal_with(6));
        assert_eq!(follow(&cancelled), ProposalOutcome::Cancelled);

        // The newest that succeeded, past one that failed.
        let mut executed = FakeRpc::default().with_account(squad().proposal(1), proposal_with(5));
        executed.history.insert(
            squad().transaction(1),
            history(&[(7, Some("InstructionError")), (6, None), (5, None)]),
        );
        assert_eq!(
            follow(&executed),
            ProposalOutcome::Executed {
                signature: Some(Signature::from([6; 64]))
            }
        );
        // Owned by another program, it is not the proposal.
        let mut forged = proposal_with(5);
        forged.owner = pk(4);
        let forged = FakeRpc::default().with_account(squad().proposal(1), forged);
        assert_eq!(follow(&forged), ProposalOutcome::Missing);
    }

    #[test]
    fn a_transaction_that_opens_no_proposal_is_refused() {
        let wire = compile_v1(
            pk(1),
            vec![transfer_instruction(pk(1), pk(2))],
            None,
            Some(Hash::default()),
        )
        .unwrap()
        .wire;
        assert!(pollster::block_on(follow_proposal(&FakeRpc::default(), &wire)).is_err());
    }
}
