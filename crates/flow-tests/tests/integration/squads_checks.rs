//! An approval that carries what its proposal holds, against the real Squads
//! v4 and clear signing programs.
//!
//! One member stores a transaction, opens its proposal and approves it. The
//! other member's approval is rebuilt the way Review and Sign rebuilds it when
//! asked (`wallet_flows::proposal_checks`), signed, and sent: it lands, and the
//! proposal passes. The other member signs with its own key, through a
//! programmatic signer, or through a Vector account. The same approval
//! carrying anything other than what the proposal holds, or the wrong hash
//! for an upgrade's buffer, or the checks where the program does not run
//! them, is refused by the chain, and no vote lands.
//!
//! The shapes the deployed program does not take (an execution of the same
//! proposal, several members, a durable nonce) are built here with every
//! shape allowed, as Review and Sign would build them for a program that
//! took them, and wait, ignored: each says why the program refuses it. To
//! run one against a program that takes it, put that build in
//! `crates/flow-tests/programs/` (see `scripts/fetch-programs.mjs`), set
//! `wallet_core::squads::ClearSigningAccepts::DEPLOYED` to what it takes, and
//! drop the `#[ignore]`.

use crate::support::node::Node;
use crate::support::vector;
use crate::support::{logs, Chain, SOL};
use solana_instruction::AccountMeta;
use solana_pubkey::Pubkey;
use std::sync::Mutex;
use wallet_core::adapt::{plan_adaptation, AdaptPlan, Adapted};
use wallet_core::program_code::{buffer_hash, BPF_LOADER_UPGRADEABLE_ID};
use wallet_core::programmatic::{create_executor_nonce, programmatic_signer_address};
use wallet_core::squads::{
    compile_with_checks, place_checks, BufferHashCheck, CheckForm, ClearSigningAccepts,
    ProposalCheck, ProposalStatus, Squad, SquadsMember, SquadsSetup, StoredAccount,
    VaultTransaction, PERMISSION_ALL,
};
use wallet_core::{
    apply_local_signatures, checked_in, compile_v1, decode_v1_transaction, explain_transaction,
    parse_nonce_account, DecodedTx, ExplainedIx, InnerKind, Instruction, KeypairSigner, NoIdls,
    NonceSpec, WalletRecord, WalletSigner,
};
use wallet_flows::proposal_checks::{add_checks, WithChecks};
use wallet_flows::FlowError;

/// What a program that took the shapes these ignored tests build would
/// have to take. The deployed one refuses each of them.
const NEXT: ClearSigningAccepts = ClearSigningAccepts::EVERYTHING;

/// What the program running here takes.
const DEPLOYED: ClearSigningAccepts = ClearSigningAccepts::DEPLOYED;

fn no_rebuild(_: &DecodedTx) -> Result<Option<AdaptPlan>, FlowError> {
    Ok(None)
}

struct Desk {
    node: Node,
    squad: Squad,
    vault: Pubkey,
    /// Stores and approves first, and pays.
    a: KeypairSigner,
    /// The member whose approval carries the checks: in use, on another device.
    b: KeypairSigner,
    /// A third member, who approves together with `b` where the multisig
    /// needs three.
    c: KeypairSigner,
}

impl Desk {
    /// A multisig of `a` and `b` that needs both, with three SOL in its vault.
    fn new() -> Self {
        Self::of(2)
    }

    /// A multisig of `a`, `b` and `c` that needs `threshold` of them, `c`
    /// a member only when three are needed.
    fn of(threshold: u16) -> Self {
        Self::with_second(threshold, None)
    }

    /// [`Self::of`], with `second` as the second member in place of `b`'s
    /// key, such as the programmatic signer `b` is the authority of.
    fn with_second(threshold: u16, second: Option<fn(&Pubkey) -> Pubkey>) -> Self {
        let mut chain = Chain::new();
        let (a, b, c) = (
            KeypairSigner::new_random(),
            KeypairSigner::new_random(),
            KeypairSigner::new_random(),
        );
        chain.fund(&a.pubkey(), 10 * SOL);
        chain.fund(&b.pubkey(), SOL);
        let b_member = second.map_or(b.pubkey(), |derive| derive(&b.pubkey()));
        let mut members = vec![a.pubkey(), b_member];
        if threshold > 2 {
            members.push(c.pubkey());
        }
        let setup = SquadsSetup {
            members: members
                .into_iter()
                .map(|key| SquadsMember {
                    key,
                    permissions: PERMISSION_ALL,
                })
                .collect(),
            threshold,
            time_lock: 0,
        };
        let create_key = KeypairSigner::new_random();
        let (squad, create) =
            Squad::create_v4(&create_key.pubkey(), &a.pubkey(), &chain.treasury, &setup);
        chain.send(&[&a, &create_key], vec![create]).unwrap();
        let vault = squad.vault(0);
        chain.fund(&vault, 3 * SOL);
        Self {
            node: Node(Mutex::new(chain)),
            squad,
            vault,
            a,
            b,
            c,
        }
    }

    /// `instructions` paid by `a`, on a recent blockhash.
    fn compiled(&self, instructions: Vec<Instruction>) -> DecodedTx {
        let blockhash = self.node.chain().latest_blockhash();
        let wire = compile_v1(self.a.pubkey(), instructions, None, Some(blockhash))
            .unwrap()
            .wire;
        decode_v1_transaction(&wire).unwrap()
    }

    /// `tx` with what proposal `index` holds, as the next program will take it.
    fn checked(&self, tx: &DecodedTx) -> WithChecks {
        pollster::block_on(add_checks(&self.node, tx, None, NEXT, no_rebuild))
            .unwrap_or_else(|err| panic!("{err}"))
    }

    /// Sign `wire` as every key of `keys`, and send it.
    fn send_as(&self, wire: &[u8], keys: &[&KeypairSigner]) -> Result<(), String> {
        let signers: Vec<&dyn WalletSigner> =
            keys.iter().map(|key| *key as &dyn WalletSigner).collect();
        let signed = apply_local_signatures(wire, &signers).unwrap();
        let sent = self.node.chain().send_wire(&signed);
        sent.map(|_| ()).map_err(|failure| logs(&failure))
    }

    /// Store `instructions` as transaction `index`, open its proposal, and
    /// approve it as `a`.
    fn propose(&self, index: u64, instructions: &[Instruction]) {
        let a = self.a.pubkey();
        self.node
            .chain()
            .send(
                &[&self.a],
                vec![
                    self.squad
                        .create_transaction(index, 0, instructions, &a, &a)
                        .unwrap(),
                    self.squad.create_proposal(index, &a, &a),
                    self.squad.approve(index, &a),
                ],
            )
            .unwrap_or_else(|failure| panic!("{}", logs(&failure)));
    }

    /// `b`'s approval of `index` as the Proposals screen compiles it, paid by `a`.
    fn approval(&self, index: u64) -> DecodedTx {
        let blockhash = self.node.chain().latest_blockhash();
        let wire = compile_v1(
            self.a.pubkey(),
            vec![self.squad.approve(index, &self.b.pubkey())],
            None,
            Some(blockhash),
        )
        .unwrap()
        .wire;
        decode_v1_transaction(&wire).unwrap()
    }

    fn with_checks(&self, index: u64) -> WithChecks {
        pollster::block_on(add_checks(
            &self.node,
            &self.approval(index),
            None,
            ClearSigningAccepts::DEPLOYED,
            no_rebuild,
        ))
        .unwrap_or_else(|err| panic!("{err}"))
    }

    /// Sign `wire` as `a` and `b`, and send it.
    fn send(&self, wire: &[u8]) -> Result<(), String> {
        let signed =
            apply_local_signatures(wire, &[&self.a as &dyn WalletSigner, &self.b]).unwrap();
        let sent = self.node.chain().send_wire(&signed);
        sent.map(|_| ()).map_err(|failure| logs(&failure))
    }

    fn status(&self, index: u64) -> ProposalStatus {
        let data = self.node.chain().data(&self.squad.proposal(index));
        self.squad.parse_proposal(&data).unwrap().status
    }

    /// Whether `member`'s approval of `index` landed.
    fn approved_by(&self, index: u64, member: &Pubkey) -> bool {
        let data = self.node.chain().data(&self.squad.proposal(index));
        let proposal = self.squad.parse_proposal(&data).unwrap();
        proposal.approved.contains(member)
    }

    /// What transaction `index` stores, as the chain holds it.
    fn stored(&self, index: u64) -> VaultTransaction {
        stored_of(&self.node, &self.squad, index)
    }

    fn payment(&self, lamports: u64) -> Instruction {
        solana_system_interface::instruction::transfer(&self.vault, &self.a.pubkey(), lamports)
    }

    /// `check` and `buffers` compiled as the rebuild compiles them, with the
    /// limits a probe gives, for a check written by hand.
    fn compile(&self, check: &ProposalCheck, buffers: &[BufferHashCheck]) -> Vec<u8> {
        let approval = self.approval(check.transaction_index);
        let blockhash = self.node.chain().latest_blockhash();
        let instructions = place_checks(
            None,
            &approval.native_instructions(),
            std::slice::from_ref(check),
            buffers,
        );
        compile_with_checks(self.a.pubkey(), &approval.budget, &instructions, blockhash).unwrap()
    }
}

fn stored_of(node: &Node, squad: &Squad, index: u64) -> VaultTransaction {
    let data = node.chain().data(&squad.transaction(index));
    match squad.parse_transaction(&data) {
        Some(StoredAccount::Vault(stored)) => stored,
        other => panic!("{other:?}"),
    }
}

/// The instructions the top-level instruction at `at` of `wire` runs, read
/// from its bytes: what a `Passthrough` carries, or what a `Submit`'s
/// `Execute` carries.
fn runs_in(wire: &[u8], at: usize) -> Vec<ExplainedIx> {
    let tx = decode_v1_transaction(wire).unwrap();
    let explained = explain_transaction(&NoIdls, &tx);
    let inner = explained.instructions[at].inner.clone().expect("a carrier");
    match inner.kind {
        InnerKind::Passthrough { .. } => inner.instructions,
        InnerKind::Authorization { .. } => {
            assert_eq!(inner.instructions.len(), 1, "one Execute");
            inner.instructions[0]
                .inner
                .clone()
                .expect("an Execute")
                .instructions
        }
        other => panic!("{other:?}"),
    }
}

/// `wire` signed by `signers` as the Sign screen signs it, sent.
fn sign_and_send(node: &Node, wire: &[u8], signers: &[&dyn WalletSigner]) -> Result<(), String> {
    let signed = crate::support::sign_through(
        Some(node),
        wire,
        signers,
        &mut wallet_flows::sign::VectorSigning::default(),
    )
    .map_err(|err| err.to_string())?;
    node.chain()
        .send_wire(&signed.wire)
        .map(|_| ())
        .map_err(|failure| logs(&failure))
}

/// Asserts that `refused` is the clear signing program's `code` at the
/// transaction's instruction `at`.
#[track_caller]
fn refused_with(refused: &str, at: usize, code: u32) {
    assert!(
        refused.contains(&format!("InstructionError({at}, Custom({code}))")),
        "{refused}"
    );
}

/// The check `with` carries, read back from its bytes.
fn check_in(with: &WithChecks) -> ProposalCheck {
    let tx = decode_v1_transaction(&with.wire).unwrap();
    tx.native_instructions()
        .iter()
        .find_map(ProposalCheck::parse)
        .unwrap()
}

#[test]
fn an_approval_that_carries_what_the_proposal_holds_lands() {
    let desk = Desk::new();
    let payment = desk.payment(SOL);
    desk.propose(1, std::slice::from_ref(&payment));
    let with = desk.with_checks(1);

    // What the device that signs reads from the bytes alone.
    let tx = decode_v1_transaction(&with.wire).unwrap();
    let pinned = checked_in(&explain_transaction(&NoIdls, &tx));
    assert_eq!(pinned.len(), 1);
    assert_eq!(pinned[0].proposal, desk.squad.proposal(1));
    assert!(pinned[0].pins_vote_of(&desk.b.pubkey()));
    assert_eq!(check_in(&with).instructions, vec![payment]);
    println!(
        "approval with checks: {} bytes, CU limit {:?}, loaded accounts limit {:?}",
        with.wire.len(),
        tx.budget.compute_unit_limit,
        tx.budget.loaded_accounts_data_size_limit
    );

    assert_eq!(desk.status(1), ProposalStatus::Active);
    desk.send(&with.wire).unwrap_or_else(|why| panic!("{why}"));
    assert_eq!(desk.status(1), ProposalStatus::Approved);
}

#[test]
fn an_approval_that_carries_anything_else_is_refused_by_the_chain() {
    let desk = Desk::new();
    desk.propose(1, &[desk.payment(SOL)]);
    let honest = check_in(&desk.with_checks(1));

    // Another amount than the one stored.
    let mut amount = honest.clone();
    amount.instructions[0] = desk.payment(2 * SOL);
    let refused = desk.send(&desk.compile(&amount, &[])).unwrap_err();
    // The clear signing program's `dataMismatch`.
    assert!(
        refused.contains("InstructionError(0, Custom(6020))"),
        "{refused}"
    );
    // An instruction left out, and one added.
    let mut fewer = honest.clone();
    fewer.instructions.clear();
    desk.send(&desk.compile(&fewer, &[])).unwrap_err();
    let mut more = honest.clone();
    more.instructions.push(desk.payment(1));
    desk.send(&desk.compile(&more, &[])).unwrap_err();
    // An account's flag other than the one stored.
    let mut flag = honest.clone();
    flag.instructions[0].accounts[1].is_writable = false;
    desk.send(&desk.compile(&flag, &[])).unwrap_err();
    // Something else beside the vote: the program reads the whole transaction.
    let approval = desk.approval(1);
    let blockhash = desk.node.chain().latest_blockhash();
    let mut beside = vec![
        honest.instruction(&[]),
        approval.native_instructions()[0].clone(),
    ];
    beside.push(solana_system_interface::instruction::transfer(
        &desk.a.pubkey(),
        &desk.b.pubkey(),
        1,
    ));
    let extra = compile_v1(desk.a.pubkey(), beside, None, Some(blockhash))
        .unwrap()
        .wire;
    let refused = desk.send(&extra).unwrap_err();
    // The clear signing program's `forbiddenInstruction`.
    assert!(
        refused.contains("InstructionError(0, Custom(6022))"),
        "{refused}"
    );

    assert_eq!(desk.status(1), ProposalStatus::Active);
    // The honest one still lands after all that.
    desk.send(&desk.compile(&honest, &[]))
        .unwrap_or_else(|why| panic!("{why}"));
    assert_eq!(desk.status(1), ProposalStatus::Approved);
}

#[test]
fn an_upgrade_lands_only_with_the_hash_of_the_build_in_its_buffer() {
    let desk = Desk::new();
    // A buffer of the upgradeable loader holding a build, its authority the
    // vault, as `solana program write-buffer` leaves it.
    let buffer = KeypairSigner::new_random().pubkey();
    let mut data = vec![1, 0, 0, 0, 1];
    data.extend_from_slice(desk.vault.as_ref());
    data.extend_from_slice(b"\x7fELF the build to deploy");
    data.extend_from_slice(&[0; 4096]);
    let hash = buffer_hash(&data).unwrap();
    desk.node
        .chain()
        .plant(buffer, BPF_LOADER_UPGRADEABLE_ID, data);
    let program = KeypairSigner::new_random().pubkey();
    let upgrade = Instruction {
        program_id: BPF_LOADER_UPGRADEABLE_ID,
        accounts: vec![
            AccountMeta::new(KeypairSigner::new_random().pubkey(), false),
            AccountMeta::new(program, false),
            AccountMeta::new(buffer, false),
            AccountMeta::new(desk.a.pubkey(), false),
            AccountMeta::new_readonly(
                Pubkey::from_str_const("SysvarRent111111111111111111111111111111111"),
                false,
            ),
            AccountMeta::new_readonly(
                Pubkey::from_str_const("SysvarC1ock11111111111111111111111111111111"),
                false,
            ),
            AccountMeta::new_readonly(desk.vault, true),
        ],
        data: 3u32.to_le_bytes().to_vec(),
    };
    desk.propose(1, std::slice::from_ref(&upgrade));
    let with = desk.with_checks(1);
    let tx = decode_v1_transaction(&with.wire).unwrap();
    let native = tx.native_instructions();
    // The buffer's hash first, then what the proposal holds, then the vote.
    assert_eq!(
        BufferHashCheck::parse(&native[0]),
        Some(BufferHashCheck::new(buffer, &hash))
    );
    assert_eq!(with.approval.upgrades.len(), 1);
    assert_eq!(with.approval.upgrades[0].program, program);
    println!(
        "upgrade approval with checks: {} bytes, CU limit {:?}",
        with.wire.len(),
        tx.budget.compute_unit_limit
    );

    // Another build's hash is refused.
    let honest = check_in(&with);
    let wrong = BufferHashCheck::new(buffer, &[7; 32]);
    let refused = desk
        .send(&desk.compile(&honest, std::slice::from_ref(&wrong)))
        .unwrap_err();
    // The clear signing program's `bufferHashMismatch`.
    assert!(
        refused.contains("InstructionError(0, Custom(6033))"),
        "{refused}"
    );
    assert_eq!(desk.status(1), ProposalStatus::Active);

    // Built again: the refused one took the blockhash with it.
    let with = desk.with_checks(1);
    desk.send(&with.wire).unwrap_or_else(|why| panic!("{why}"));
    assert_eq!(desk.status(1), ProposalStatus::Approved);
}

#[test]
#[ignore = "the clear signing program refuses an execution of the proposal beside a check (6023), at the top and in a Submit or Passthrough"]
fn an_approval_and_the_execution_carrying_what_the_proposal_holds_land() {
    let desk = Desk::new();
    let payment = desk.payment(SOL);
    desk.propose(1, std::slice::from_ref(&payment));
    let b = desk.b.pubkey();
    let execute = desk.squad.execute(1, 0, std::slice::from_ref(&payment), &b);
    let with = desk.checked(&desk.compiled(vec![desk.squad.approve(1, &b), execute]));
    assert!(with.approval.pins_vote_of(&b));
    assert!(with.approval.execute.is_some());

    desk.send_as(&with.wire, &[&desk.a, &desk.b])
        .unwrap_or_else(|why| panic!("{why}"));
    assert_eq!(desk.status(1), ProposalStatus::Executed);
    assert_eq!(desk.node.chain().balance(&desk.vault), 2 * SOL);
}

#[test]
#[ignore = "the clear signing program takes one member's vote beside a check: each check refuses another member's vote (6027) and the execution (6023)"]
fn approving_together_and_executing_carrying_what_the_proposal_holds_lands() {
    let desk = Desk::of(3);
    let payment = desk.payment(SOL);
    desk.propose(1, std::slice::from_ref(&payment));
    let (b, c) = (desk.b.pubkey(), desk.c.pubkey());
    let execute = desk.squad.execute(1, 0, std::slice::from_ref(&payment), &b);
    let together = vec![
        desk.squad.approve(1, &b),
        desk.squad.approve(1, &c),
        execute,
    ];
    let with = desk.checked(&desk.compiled(together));
    assert!(with.approval.pins_vote_of(&b) && with.approval.pins_vote_of(&c));
    assert!(with.approval.execute.is_some());

    desk.send_as(&with.wire, &[&desk.a, &desk.b, &desk.c])
        .unwrap_or_else(|why| panic!("{why}"));
    assert_eq!(desk.status(1), ProposalStatus::Executed);
}

#[test]
#[ignore = "the clear signing program refuses a System AdvanceNonceAccount beside a top-level check (6022), and a wallet without a key takes no durable nonce"]
fn an_approval_on_a_durable_nonce_carrying_what_the_proposal_holds_lands() {
    let desk = Desk::new();
    desk.propose(1, &[desk.payment(SOL)]);
    let a = desk.a.pubkey();
    let nonce_key = KeypairSigner::new_random();
    let nonce = nonce_key.pubkey();
    let hash = {
        let mut chain = desk.node.chain();
        let create = solana_system_interface::instruction::create_nonce_account(
            &a,
            &nonce,
            &a,
            chain.rent(80),
        );
        chain.send(&[&desk.a, &nonce_key], create).unwrap();
        let account = chain.account(&nonce).unwrap();
        parse_nonce_account(nonce, &account).unwrap().hash
    };
    let spec = NonceSpec {
        account: nonce,
        authority: a,
        hash,
    };
    let vote = desk.squad.approve(1, &desk.b.pubkey());
    let wire = compile_v1(a, vec![vote], Some(spec), None).unwrap().wire;
    let with = desk.checked(&decode_v1_transaction(&wire).unwrap());
    // A durable nonce does not expire with a blockhash.
    assert_eq!(with.last_valid_height, None);
    assert!(
        decode_v1_transaction(&with.wire)
            .unwrap()
            .uses_durable_nonce
    );

    desk.send_as(&with.wire, &[&desk.a, &desk.b])
        .unwrap_or_else(|why| panic!("{why}"));
    assert_eq!(desk.status(1), ProposalStatus::Approved);
}

/// A multisig of `a` and the programmatic signer `b` is the authority of,
/// which needs both, with three SOL in its vault, and the wallets Sign
/// knows: `a`, which pays, and the signer, with its executor nonce.
struct ProgrammaticDesk {
    desk: Desk,
    member: Pubkey,
    wallets: Vec<WalletRecord>,
}

impl ProgrammaticDesk {
    fn new() -> Self {
        let desk = Desk::with_second(2, Some(programmatic_signer_address));
        let member = programmatic_signer_address(&desk.b.pubkey());
        let mut signer = WalletRecord::programmatic("Signer", desk.b.pubkey(), None);
        {
            let mut chain = desk.node.chain();
            let (nonce, create) = create_executor_nonce(
                &desk.a.pubkey(),
                &desk.a.pubkey(),
                "airlock",
                &member,
                chain.nonce_rent(),
            )
            .unwrap();
            chain.send(&[&desk.a], create.to_vec()).unwrap();
            signer.kind.set_executor_nonce(Some(nonce));
        }
        let wallets = vec![
            WalletRecord::from_keypair_seed("Fees", desk.a.seed_bytes()),
            signer,
        ];
        Self {
            desk,
            member,
            wallets,
        }
    }

    /// `instructions` paid by `a`, rebuilt for the signer as Sign rebuilds
    /// a transaction as soon as it arrives.
    fn rebuilt(&self, instructions: Vec<Instruction>) -> Adapted {
        let plain = self.desk.compiled(instructions);
        let plan = plan_adaptation(&plain, &self.wallets, None)
            .unwrap()
            .expect("the signer is asked to sign");
        let (adapted, _) =
            pollster::block_on(wallet_flows::adapt::rebuild(&self.desk.node, &plain, &plan))
                .unwrap_or_else(|err| panic!("{err}"));
        adapted
    }

    /// The member's approval of `index`, as Sign has it, with what the
    /// proposal holds added as Review and Sign adds it.
    fn with_checks(&self, index: u64) -> Result<WithChecks, FlowError> {
        let vote = self.desk.squad.approve(index, &self.member);
        let wrapped = decode_v1_transaction(&self.rebuilt(vec![vote]).wire).unwrap();
        pollster::block_on(add_checks(
            &self.desk.node,
            &wrapped,
            None,
            DEPLOYED,
            |plain| plan_adaptation(plain, &self.wallets, None).map_err(FlowError::refused),
        ))
    }

    /// The check by CPI of the member's approval of `index`.
    fn check(&self, index: u64) -> ProposalCheck {
        ProposalCheck::approving(
            self.desk.squad,
            &self.desk.stored(index),
            self.member,
            CheckForm::Cpi,
        )
    }

    /// Sign `wire` as the authority and as `a`, as the Sign screen does, and
    /// send it.
    fn send(&self, wire: &[u8]) -> Result<(), String> {
        sign_and_send(
            &self.desk.node,
            wire,
            &[&self.desk.b as &dyn WalletSigner, &self.desk.a],
        )
    }
}

#[test]
fn a_programmatic_members_approval_carrying_what_the_proposal_holds_lands() {
    let desk = ProgrammaticDesk::new();
    let payment = desk.desk.payment(SOL);
    desk.desk.propose(1, std::slice::from_ref(&payment));
    let member = desk.member;

    let with = desk.with_checks(1).unwrap_or_else(|err| panic!("{err}"));
    assert!(with.adapted.is_some());
    assert!(with.approval.pins_vote_of(&member));
    // The transaction is one `Submit`, and what it runs is the check by CPI
    // for the member and the vote, nothing else.
    let tx = decode_v1_transaction(&with.wire).unwrap();
    assert_eq!(tx.native_instructions().len(), 1);
    let runs = runs_in(&with.wire, 0);
    assert_eq!(runs.len(), 2);
    let check = ProposalCheck::from_data(&runs[0].data).unwrap();
    assert_eq!(check, desk.check(1));
    assert_eq!(check.instructions, vec![payment]);
    assert_eq!(runs[1].data, desk.desk.squad.approve(1, &member).data);
    println!(
        "programmatic approval with checks: {} bytes, CU limit {:?}",
        with.wire.len(),
        tx.budget.compute_unit_limit
    );

    assert_eq!(desk.desk.status(1), ProposalStatus::Active);
    desk.send(&with.wire).unwrap_or_else(|why| panic!("{why}"));
    assert_eq!(desk.desk.status(1), ProposalStatus::Approved);
    assert!(desk.desk.approved_by(1, &member));
}

#[test]
fn a_programmatic_members_approval_carrying_anything_else_is_refused_by_the_chain() {
    let desk = ProgrammaticDesk::new();
    desk.desk.propose(1, &[desk.desk.payment(SOL)]);
    let member = desk.member;
    let vote = desk.desk.squad.approve(1, &member);
    let honest = desk.check(1);

    // Another amount than the one stored: the `Submit` is instruction 0.
    let mut amount = honest.clone();
    amount.instructions[0] = desk.desk.payment(2 * SOL);
    let wire = desk
        .rebuilt(vec![amount.instruction(&[]), vote.clone()])
        .wire;
    assert_eq!(runs_in(&wire, 0).len(), 2);
    refused_with(&desk.send(&wire).unwrap_err(), 0, 6020);

    // Another program's instruction in the list the `Submit` runs.
    let other = solana_system_interface::instruction::transfer(
        &desk.desk.a.pubkey(),
        &desk.desk.c.pubkey(),
        1,
    );
    let wire = desk
        .rebuilt(vec![honest.instruction(&[]), other, vote.clone()])
        .wire;
    assert_eq!(runs_in(&wire, 0).len(), 3);
    refused_with(&desk.send(&wire).unwrap_err(), 0, 6022);

    // The check at the top, beside the `Submit`: the member cannot sign
    // there, so the program refuses it before it reads anything (Anchor's
    // `AccountNotSigner`).
    let carried = desk.rebuilt(vec![vote.clone()]);
    let carried = decode_v1_transaction(&carried.wire).unwrap();
    let mut beside = honest.instruction(&[]);
    beside.accounts[3].is_signer = false;
    let instructions: Vec<Instruction> = std::iter::once(beside)
        .chain(carried.native_instructions())
        .collect();
    let wire = compile_with_checks(
        desk.desk.a.pubkey(),
        &carried.budget,
        &instructions,
        desk.desk.node.chain().latest_blockhash(),
    )
    .unwrap();
    refused_with(&desk.send(&wire).unwrap_err(), 0, 3010);

    // A `verifyProposal` in place of the check by CPI stays at the top,
    // where it refuses the `Submit` beside it.
    let top = ProposalCheck {
        form: CheckForm::TopLevel,
        ..honest.clone()
    };
    let wire = desk.rebuilt(vec![top.instruction(&[]), vote.clone()]).wire;
    assert_eq!(runs_in(&wire, 1).len(), 1);
    refused_with(&desk.send(&wire).unwrap_err(), 0, 6022);

    assert_eq!(desk.desk.status(1), ProposalStatus::Active);
    assert!(!desk.desk.approved_by(1, &member));
    // The honest one still lands after all that.
    let with = desk.with_checks(1).unwrap_or_else(|err| panic!("{err}"));
    desk.send(&with.wire).unwrap_or_else(|why| panic!("{why}"));
    assert!(desk.desk.approved_by(1, &member));
}

#[test]
fn a_check_by_cpi_at_the_top_is_refused_by_the_chain() {
    // A member that signs with its own key, so the check can be signed at
    // the top: the program wants it run by a `Submit` or a `Passthrough`.
    let desk = Desk::new();
    desk.propose(1, &[desk.payment(SOL)]);
    let b = desk.b.pubkey();
    let check = ProposalCheck::approving(desk.squad, &desk.stored(1), b, CheckForm::Cpi);
    let tx = desk.compiled(vec![check.instruction(&[]), desk.squad.approve(1, &b)]);
    let refused = desk.send(&tx.wire).unwrap_err();
    refused_with(&refused, 0, 6048);
    assert_eq!(desk.status(1), ProposalStatus::Active);
    assert!(!desk.approved_by(1, &b));
}

#[test]
fn a_proposal_too_large_for_a_programmatic_members_approval_leaves_it_as_it_was() {
    let desk = ProgrammaticDesk::new();
    // Data for a program that is not there: the proposal is only voted on.
    let big = Instruction {
        program_id: KeypairSigner::new_random().pubkey(),
        accounts: vec![AccountMeta::new(desk.desk.vault, true)],
        data: vec![7; 3_000],
    };
    desk.desk.propose(1, &[big]);
    // The checks fit beside the vote at the top, but not in a `Submit`.
    let plain = desk.desk.compiled(vec![
        desk.check(1).instruction(&[]),
        desk.desk.squad.approve(1, &desk.member),
    ]);
    assert!(compile_with_checks(
        desk.desk.a.pubkey(),
        &plain.budget,
        &plain.native_instructions(),
        plain.lifetime
    )
    .is_ok());
    let refused = match desk.with_checks(1) {
        Err(FlowError::Refused(text)) => text,
        other => panic!("{other:?}"),
    };
    assert!(
        refused.starts_with("The proposal holds too much to carry in the approval"),
        "{refused}"
    );
}

/// A multisig of the Vector desk's payer and a Vector account, which needs
/// both, with three SOL in its vault and proposal 1, a payment of one SOL,
/// open and approved by the payer.
struct VectorDesk {
    desk: vector::Desk,
    account: Pubkey,
    squad: Squad,
}

impl VectorDesk {
    fn new() -> Self {
        let mut desk = vector::Desk::new();
        let account = desk.add_vector("Member", true).address;
        let payer = desk.payer.pubkey();
        let squad = {
            let mut chain = desk.chain();
            let setup = SquadsSetup {
                members: [payer, account]
                    .map(|key| SquadsMember {
                        key,
                        permissions: PERMISSION_ALL,
                    })
                    .to_vec(),
                threshold: 2,
                time_lock: 0,
            };
            let create_key = KeypairSigner::new_random();
            let (squad, create) =
                Squad::create_v4(&create_key.pubkey(), &payer, &chain.treasury, &setup);
            chain
                .send(&[&desk.payer, &create_key], vec![create])
                .unwrap();
            chain.fund(&squad.vault(0), 3 * SOL);
            let payment =
                solana_system_interface::instruction::transfer(&squad.vault(0), &payer, SOL);
            chain
                .send(
                    &[&desk.payer],
                    vec![
                        squad
                            .create_transaction(1, 0, &[payment], &payer, &payer)
                            .unwrap(),
                        squad.create_proposal(1, &payer, &payer),
                        squad.approve(1, &payer),
                    ],
                )
                .unwrap_or_else(|failure| panic!("{}", logs(&failure)));
            squad
        };
        Self {
            desk,
            account,
            squad,
        }
    }

    fn vote(&self) -> Instruction {
        self.squad.approve(1, &self.account)
    }

    fn check(&self) -> ProposalCheck {
        ProposalCheck::approving(
            self.squad,
            &stored_of(&self.desk.node, &self.squad, 1),
            self.account,
            CheckForm::Cpi,
        )
    }

    /// `instructions` paid by the payer, rebuilt for the account as Sign
    /// rebuilds a transaction as soon as it arrives.
    fn rebuilt(&self, instructions: Vec<Instruction>) -> Adapted {
        self.desk.rebuild(self.desk.payer.pubkey(), instructions)
    }

    /// Sign `wire` as the account and the payer, as the Sign screen does,
    /// and send it.
    fn send(&mut self, adapted: &Adapted, wire: &[u8]) -> Result<(), String> {
        let signed = self.desk.sign(wire, &adapted.context)?;
        self.desk
            .chain()
            .send_wire(&signed.wire)
            .map(|_| ())
            .map_err(|failure| logs(&failure))
    }

    fn approved(&self) -> bool {
        let data = self.desk.chain().data(&self.squad.proposal(1));
        let proposal = self.squad.parse_proposal(&data).unwrap();
        proposal.approved.contains(&self.account)
    }
}

#[test]
fn a_vector_members_approval_carrying_what_the_proposal_holds_lands() {
    let mut desk = VectorDesk::new();
    let account = desk.account;
    let payer = desk.desk.payer.pubkey();
    let blockhash = desk.desk.chain().latest_blockhash();
    let wire = compile_v1(payer, vec![desk.vote()], None, Some(blockhash))
        .unwrap()
        .wire;
    // As Sign has it: rebuilt for the member as soon as it arrived.
    let adapted = desk.desk.rebuild_wire(&wire);
    let wrapped = decode_v1_transaction(&adapted.wire).unwrap();
    let wallets = desk.desk.wallets.clone();
    let with = pollster::block_on(add_checks(
        &desk.desk.node,
        &wrapped,
        None,
        DEPLOYED,
        |plain| plan_adaptation(plain, &wallets, None).map_err(FlowError::refused),
    ))
    .unwrap_or_else(|err| panic!("{err}"));
    assert!(with.approval.pins_vote_of(&account));
    // The `Advance` and one `Passthrough`, which runs the check by CPI for
    // the account and the vote, nothing else.
    let tx = decode_v1_transaction(&with.wire).unwrap();
    assert_eq!(tx.native_instructions().len(), 2);
    let runs = runs_in(&with.wire, 1);
    assert_eq!(runs.len(), 2);
    assert_eq!(ProposalCheck::from_data(&runs[0].data), Some(desk.check()));
    assert_eq!(runs[1].data, desk.vote().data);
    println!(
        "Vector approval with checks: {} bytes, CU limit {:?}",
        with.wire.len(),
        tx.budget.compute_unit_limit
    );

    let adapted = with.adapted.clone().unwrap();
    desk.send(&adapted, &with.wire)
        .unwrap_or_else(|why| panic!("{why}"));
    let data = desk.desk.chain().data(&desk.squad.proposal(1));
    assert_eq!(
        desk.squad.parse_proposal(&data).unwrap().status,
        ProposalStatus::Approved
    );
    assert!(desk.approved());
}

#[test]
fn a_vector_members_approval_carrying_anything_else_is_refused_by_the_chain() {
    let mut desk = VectorDesk::new();
    let honest = desk.check();
    let vote = desk.vote();
    let payer = desk.desk.payer.pubkey();
    // The `Advance` is instruction 0, the `Passthrough` 1.

    // Another amount than the one stored.
    let mut amount = honest.clone();
    amount.instructions[0] =
        solana_system_interface::instruction::transfer(&desk.squad.vault(0), &payer, 2 * SOL);
    let adapted = desk.rebuilt(vec![amount.instruction(&[]), vote.clone()]);
    assert_eq!(runs_in(&adapted.wire, 1).len(), 2);
    let wire = adapted.wire.clone();
    refused_with(&desk.send(&adapted, &wire).unwrap_err(), 1, 6020);

    // Another program's instruction in the `Passthrough`: a payment out of
    // the account, which runs there as the Vector program's `Withdraw`.
    let out = solana_system_interface::instruction::transfer(&desk.account, &payer, 1);
    let adapted = desk.rebuilt(vec![honest.instruction(&[]), vote.clone(), out.clone()]);
    assert_eq!(runs_in(&adapted.wire, 1).len(), 3);
    let wire = adapted.wire.clone();
    refused_with(&desk.send(&adapted, &wire).unwrap_err(), 1, 6022);

    // A second `Passthrough`: the account's signature could be used there,
    // outside the list the check reads.
    let between = solana_system_interface::instruction::transfer(
        &payer,
        &KeypairSigner::new_random().pubkey(),
        SOL,
    );
    let adapted = desk.rebuilt(vec![honest.instruction(&[]), vote.clone(), between, out]);
    let tx = decode_v1_transaction(&adapted.wire).unwrap();
    assert_eq!(tx.native_instructions().len(), 4);
    let wire = adapted.wire.clone();
    refused_with(&desk.send(&adapted, &wire).unwrap_err(), 1, 6055);

    // The check at the top, beside the `Advance` and the `Passthrough`: the
    // account cannot sign there, so the program refuses it before it reads
    // anything (Anchor's `AccountNotSigner`).
    let carried = desk.rebuilt(vec![vote.clone()]);
    let tx = decode_v1_transaction(&carried.wire).unwrap();
    let mut beside = honest.instruction(&[]);
    beside.accounts[3].is_signer = false;
    let instructions: Vec<Instruction> = std::iter::once(beside)
        .chain(tx.native_instructions())
        .collect();
    let blockhash = desk.desk.chain().latest_blockhash();
    let wire = compile_with_checks(payer, &tx.budget, &instructions, blockhash).unwrap();
    refused_with(&desk.send(&carried, &wire).unwrap_err(), 0, 3010);

    assert!(!desk.approved());
    // The honest one still lands after all that.
    let adapted = desk.rebuilt(vec![honest.instruction(&[]), vote]);
    let wire = adapted.wire.clone();
    desk.send(&adapted, &wire)
        .unwrap_or_else(|why| panic!("{why}"));
    assert!(desk.approved());
}
