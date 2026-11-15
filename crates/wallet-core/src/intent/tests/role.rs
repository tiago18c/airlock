//! The role of a composed transaction, and a message signed as a memo.

use super::*;
use crate::fixtures::transfer;
use crate::ids::MEMO_PROGRAM_ID;
use crate::tx::decode::{
    decode_transaction, decode_v1_transaction, parse_wire, LookupTables, TxFormat,
};
use crate::wallets::programmatic::{programmatic_signer_address, ExecutorNonce};
use crate::wallets::relay::wrap_for_programmatic;
use crate::wallets::squads::{Squad, SquadsProgram};

fn v1(payer: Pubkey, instructions: Vec<Instruction>) -> DecodedTx {
    decode_v1_transaction(
        &compile_v1(payer, instructions, None, Some(blockhash(1)))
            .unwrap()
            .wire,
    )
    .unwrap()
}

#[test]
fn a_transaction_the_account_signs_is_direct() {
    let tx = v1(pk(1), vec![transfer(pk(2), pk(3), 5)]);
    assert_eq!(tx_role(&tx, &pk(2)), Some(TxRole::Direct));
    assert_eq!(tx_role(&tx, &pk(1)), Some(TxRole::Direct));
    assert_eq!(tx_role(&tx, &pk(3)), None);
}

#[test]
fn each_step_of_a_proposal_has_its_role() {
    let member = pk(1);
    for program in [SquadsProgram::V4, SquadsProgram::SmartAccount] {
        let squad = match program {
            SquadsProgram::V4 => Squad::v4(&pk(90)),
            SquadsProgram::SmartAccount => Squad::smart_account(9),
        };
        let vault = squad.vault(0);
        let send = [transfer(vault, pk(9), 5)];
        let store = squad
            .create_transaction(4, 0, 0, &send, &member, &member)
            .unwrap();
        let open = squad.create_proposal(4, &member, &member);
        let approve = squad.approve(4, &member);
        let execute = squad.execute(4, 0, &send, &member);

        let proposal = v1(member, vec![store.clone(), open.clone(), approve.clone()]);
        assert_eq!(
            tx_role(&proposal, &vault),
            Some(TxRole::Proposal),
            "{program:?}"
        );
        assert_eq!(
            proposal_opened(&proposal).map(|target| (target.squad, target.proposal)),
            Some((squad, squad.proposal(4)))
        );
        let at_once = v1(member, vec![store, open, approve.clone(), execute.clone()]);
        assert_eq!(tx_role(&at_once, &vault), Some(TxRole::Carrier));
        assert_eq!(
            tx_role(&v1(member, vec![approve]), &vault),
            Some(TxRole::Vote)
        );
        let execution = v1(member, vec![execute]);
        assert_eq!(tx_role(&execution, &vault), Some(TxRole::Execution));
        assert_eq!(proposal_opened(&execution), None);
    }
}

#[test]
fn a_smart_account_that_runs_at_once_is_a_carrier() {
    let squad = Squad::smart_account(9);
    let vault = squad.vault(0);
    let sync = squad
        .execute_sync(0, &[transfer(vault, pk(9), 5)], &[pk(1)])
        .unwrap();
    assert_eq!(
        tx_role(&v1(pk(1), vec![sync]), &vault),
        Some(TxRole::Carrier)
    );
}

#[test]
fn a_programmatic_signers_submit_is_a_carrier() {
    let authority = pk(6);
    let acts_as = programmatic_signer_address(&authority);
    let site = v1(acts_as, vec![transfer(acts_as, pk(2), 5)]);
    let wire = wrap_for_programmatic(
        &site,
        &[authority],
        ExecutorNonce {
            account: pk(7),
            authority,
            value: blockhash(2),
        },
        pk(8),
        blockhash(3),
    )
    .unwrap();
    let wrapped = decode_v1_transaction(&wire).unwrap();
    assert_eq!(tx_role(&wrapped, &acts_as), Some(TxRole::Carrier));
}

#[test]
fn a_message_is_a_memo_when_it_is_short_text() {
    assert_eq!(
        memo_text(b"sign in to app.example: 42"),
        Ok("sign in to app.example: 42")
    );
    assert_eq!(memo_text(b""), Err(MemoError::Empty));
    assert_eq!(memo_text(&[0xff, 0xfe]), Err(MemoError::NotText));
    assert_eq!(memo_text(&[b'a'; 513]), Err(MemoError::TooLong(513)));
    assert!(memo_text(&[b'a'; 512]).is_ok());
}

#[test]
fn the_memo_transaction_signs_the_message_as_the_account() {
    let account = pk(2);
    // Legacy for a key here, so any node shows the proof by its signature;
    // v1 for every other account, which is rebuilt or signed elsewhere.
    assert_eq!(memo_format(true), TxFormat::Legacy);
    assert_eq!(memo_format(false), TxFormat::V1);
    let v1_wire =
        memo_transaction(account, "hello", pk(1), Some(blockhash(1)), TxFormat::V1).unwrap();
    assert_eq!(parse_wire(&v1_wire).unwrap().format, TxFormat::V1);
    let v1_tx = decode_transaction(&v1_wire, &LookupTables::new()).unwrap();
    assert!(is_memo_of(&v1_tx, &account, b"hello"));
    assert_eq!(v1_tx.fee_payer(), Some(pk(1)));
    let wire = memo_transaction(
        account,
        "hello",
        pk(1),
        Some(blockhash(1)),
        TxFormat::Legacy,
    )
    .unwrap();
    assert_eq!(parse_wire(&wire).unwrap().format, TxFormat::Legacy);
    let tx = decode_transaction(&wire, &LookupTables::new()).unwrap();
    assert_eq!(tx.fee_payer(), Some(pk(1)));
    assert!(tx.signers.iter().any(|slot| slot.pubkey == account));
    assert_eq!(tx.instructions.len(), 1);
    assert_eq!(tx.instructions[0].program_id, MEMO_PROGRAM_ID);
    assert_eq!(tx.instructions[0].accounts, vec![account]);
    assert_eq!(tx.instructions[0].data, b"hello");
    assert_eq!(tx_role(&tx, &account), Some(TxRole::Direct));

    assert!(is_memo_of(&tx, &account, b"hello"));
    assert!(!is_memo_of(&tx, &account, b"hello!"));
    assert!(!is_memo_of(&tx, &pk(3), b"hello"));
    // Anything beside it, and it is not just a memo.
    let more = v1(
        pk(1),
        vec![
            memo_instruction(account, "hello"),
            transfer(account, pk(9), 5),
        ],
    );
    assert!(!is_memo_of(&more, &account, b"hello"));
}

#[test]
fn a_memo_carried_for_a_programmatic_signer_is_still_the_memo() {
    let authority = pk(6);
    let acts_as = programmatic_signer_address(&authority);
    let memo = decode_transaction(
        &memo_transaction(acts_as, "hello", acts_as, Some(blockhash(1)), TxFormat::V1).unwrap(),
        &LookupTables::new(),
    )
    .unwrap();
    let wire = wrap_for_programmatic(
        &memo,
        &[authority],
        ExecutorNonce {
            account: pk(7),
            authority,
            value: blockhash(2),
        },
        pk(8),
        blockhash(3),
    )
    .unwrap();
    let wrapped = decode_v1_transaction(&wire).unwrap();
    assert!(is_memo_of(&wrapped, &acts_as, b"hello"));
}
