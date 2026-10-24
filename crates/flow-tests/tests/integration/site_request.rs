//! A site's sign request for a wallet that has no key, against the real
//! programs.
//!
//! The site was shown a programmatic signer's address or a Squads vault, and
//! built a transaction in which that address pays and signs. The app rebuilds
//! it from what the chain says right now, signs everything with the keys in
//! its vault in one go, and the exact bytes it would hand back to the site
//! are sent.

use crate::support::node::Node;
use crate::support::{logs, Chain, SOL};
use solana_hash::Hash;
use solana_message::v0::{Message as V0Message, MessageAddressTableLookup};
use solana_message::{Message as LegacyMessage, VersionedMessage};
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use solana_transaction::versioned::VersionedTransaction;
use std::sync::Mutex;
use wallet_core::adapt::{
    plan_adaptation, plan_adaptation_with, plan_payer_swap, swap_fee_payer, AdaptError, Adapted,
    AdaptedStep, HeldBack,
};
use wallet_core::programmatic::create_executor_nonce;
use wallet_core::squads::{
    Squad, SquadsMember, SquadsProgram, SquadsSetup, PERMISSION_ALL, PERMISSION_VOTE,
};
use wallet_core::{
    apply_local_signatures, compile_v1, decode_transaction, decode_v1_transaction,
    explain_transaction, parse_wire, quick_sign, sign_locally, ConvertError, IdlStore, Instruction,
    KeypairSigner, LimitFrom, LookupTables, MemoryBodies, MemoryStore, QuickSign, TxFormat,
    WalletId, WalletKind, WalletRecord, WalletSigner,
};
use wallet_flows::request::{replaceable_signers, route, Route};
use wallet_flows::FlowError;

const BOTH: [SquadsProgram; 2] = [SquadsProgram::V4, SquadsProgram::SmartAccount];

/// The app on one device: a chain to talk to and a vault of wallets.
struct App {
    chain: Chain,
    /// Pays every fee. Holds funds.
    hot: KeypairSigner,
    hot_id: WalletId,
    /// Signs authorizations only. Holds nothing.
    authority: KeypairSigner,
    /// The programmatic signer wallet and its address.
    signer_id: WalletId,
    address: Pubkey,
    wallets: Vec<WalletRecord>,
    recipient: Pubkey,
}

impl App {
    fn new() -> Self {
        let mut chain = Chain::new();
        let hot = KeypairSigner::new_random();
        let authority = KeypairSigner::new_random();
        chain.fund(&hot.pubkey(), 10 * SOL);

        let hot_wallet = WalletRecord::from_keypair_seed("Fees", hot.seed_bytes());
        let authority_wallet = WalletRecord::from_keypair_seed("Authority", authority.seed_bytes());
        let mut signer =
            WalletRecord::programmatic("Signer", authority.pubkey(), Some(authority_wallet.id));
        let address = signer.address().unwrap();
        signer.kind.set_fee_payer(Some(hot_wallet.id));

        // The Wallets screen creates the nonce account and stores its address.
        let (nonce, create) = create_executor_nonce(
            &hot.pubkey(),
            &hot.pubkey(),
            "airlock",
            &address,
            chain.nonce_rent(),
        )
        .unwrap();
        chain.send(&[&hot], create.to_vec()).unwrap();
        signer.kind.set_executor_nonce(Some(nonce));

        Self {
            chain,
            hot_id: hot_wallet.id,
            signer_id: signer.id,
            address,
            wallets: vec![hot_wallet, authority_wallet, signer],
            hot,
            authority,
            recipient: KeypairSigner::new_random().pubkey(),
        }
    }

    /// Create a multisig on chain and add it as a wallet acted on by
    /// `member_wallet`. Its vault gets three SOL.
    fn add_squad(
        &mut self,
        program: SquadsProgram,
        members: Vec<SquadsMember>,
        threshold: u16,
        member_wallet: WalletId,
    ) -> Squad {
        let setup = SquadsSetup {
            members,
            threshold,
            time_lock: 0,
        };
        let squad = match program {
            SquadsProgram::V4 => {
                let create_key = KeypairSigner::new_random();
                let (squad, create) = Squad::create_v4(
                    &create_key.pubkey(),
                    &self.hot.pubkey(),
                    &self.chain.treasury,
                    &setup,
                );
                self.chain
                    .send(&[&self.hot, &create_key], vec![create])
                    .unwrap();
                squad
            }
            SquadsProgram::SmartAccount => {
                let config = self.chain.data(&program.program_config());
                let count = SquadsProgram::smart_account_count(&config).unwrap();
                let (squad, create) = Squad::create_smart_account(
                    count + 1,
                    &self.hot.pubkey(),
                    &self.chain.treasury,
                    &setup,
                );
                self.chain.send(&[&self.hot], vec![create]).unwrap();
                squad
            }
        };
        self.chain
            .send(
                &[&self.hot],
                vec![pay(&self.hot.pubkey(), &squad.vault(0), 3 * SOL)],
            )
            .unwrap();

        // Added by address, the way the Wallets screen does: members and
        // threshold come from the chain, not from this record.
        let mut wallet = WalletRecord::squads("Treasury", squad, Vec::new(), threshold);
        if let WalletKind::Squads {
            member_wallet: slot,
            ..
        } = &mut wallet.kind
        {
            *slot = Some(member_wallet);
        }
        wallet.kind.set_fee_payer(Some(self.hot_id));
        self.wallets.push(wallet);
        squad
    }

    /// What a site hands over for the address it was given: that address
    /// pays the fee and signs every instruction.
    fn site_request(&self, from: &Pubkey, instructions: Vec<Instruction>) -> Vec<u8> {
        compile_v1(
            *from,
            instructions,
            None,
            Some(self.chain.latest_blockhash()),
        )
        .unwrap()
        .wire
    }

    /// What the prompt does before it shows anything: plan, read the chain,
    /// rebuild.
    fn adapt(&self, request: &[u8]) -> Result<Adapted, AdaptError> {
        let tx = decode_v1_transaction(request).unwrap();
        let plan = plan_adaptation(&tx, &self.wallets, None)?
            .expect("the request names a wallet without a key");
        let accounts: Vec<_> = plan
            .accounts_to_read()
            .into_iter()
            .filter_map(|address| Some((address, self.chain.account(&address)?)))
            .collect();
        plan.build(&tx, &accounts, self.chain.latest_blockhash())
    }

    /// Whether the prompt offers Sign, judged with the IDLs the app ships.
    fn verdict(&self, wire: &[u8], keys: &[&KeypairSigner]) -> QuickSign {
        let catalog = IdlStore::open(MemoryStore::new(), MemoryBodies::new()).with_builtins();
        let tx = decode_v1_transaction(wire).unwrap();
        let local: Vec<Pubkey> = keys.iter().map(|key| key.pubkey()).collect();
        quick_sign(&tx, &local, &explain_transaction(&catalog, &tx))
    }

    /// Sign with the keys in the vault, then the bytes go out. A press that
    /// signs an authorization signs nothing else, so the transaction takes
    /// a second press.
    fn sign_and_send(&mut self, wire: &[u8], keys: &[&KeypairSigner]) -> crate::support::Landed {
        let signers: Vec<&dyn WalletSigner> =
            keys.iter().map(|key| *key as &dyn WalletSigner).collect();
        let mut signed = sign_locally(wire, &signers).unwrap();
        if signed.authorizations > 0 {
            assert_eq!(
                signed.signatures, 0,
                "an authorization is a press of its own"
            );
            assert!(signed.transaction_next);
            signed = sign_locally(&signed.wire, &signers).unwrap();
        }
        let tx = decode_v1_transaction(&signed.wire).unwrap();
        assert!(tx.fully_signed(), "the presses should finish it");
        self.chain
            .send_wire(&signed.wire)
            .unwrap_or_else(|failure| panic!("{}", logs(&failure)))
    }
}

fn pay(from: &Pubkey, to: &Pubkey, lamports: u64) -> Instruction {
    solana_system_interface::instruction::transfer(from, to, lamports)
}

fn member(key: Pubkey, permissions: u8) -> SquadsMember {
    SquadsMember { key, permissions }
}

/// `address` is named by the transaction and does not sign it.
fn is_plain_account(wire: &[u8], address: &Pubkey) -> bool {
    decode_v1_transaction(wire)
        .unwrap()
        .accounts
        .iter()
        .any(|account| account.pubkey == *address && !account.signer)
}

fn signer_keys(wire: &[u8]) -> Vec<Pubkey> {
    decode_v1_transaction(wire)
        .unwrap()
        .signers
        .iter()
        .map(|slot| slot.pubkey)
        .collect()
}

#[test]
fn a_request_for_a_programmatic_signer_is_rebuilt_authorized_signed_and_lands() {
    let mut app = App::new();
    app.chain.fund(&app.address, SOL);
    let request = app.site_request(
        &app.address,
        vec![pay(&app.address, &app.recipient, SOL / 2)],
    );

    // As the site built it, nobody can sign it.
    let stuck = app.verdict(&request, &[&app.hot, &app.authority]);
    assert_eq!((stuck.signable, stuck.elsewhere), (0, 1));

    let adapted = app.adapt(&request).unwrap();
    assert_eq!(adapted.fee_payer, app.hot.pubkey());
    assert_eq!(signer_keys(&adapted.wire), vec![app.hot.pubkey()]);
    assert!(is_plain_account(&adapted.wire, &app.address));

    // Every instruction reads and both keys are here: the prompt offers Sign.
    let verdict = app.verdict(&adapted.wire, &[&app.hot, &app.authority]);
    assert_eq!(
        verdict,
        QuickSign {
            unread: 0,
            unvetted: 0,
            elsewhere: 0,
            signable: 2
        }
    );
    // Without the authority key it does not.
    assert!(!app.verdict(&adapted.wire, &[&app.hot]).allowed());

    let before = app.chain.balance(&app.hot.pubkey());
    let (hot, authority) = (
        KeypairSigner::from_seed(&app.hot.seed_bytes()).unwrap(),
        KeypairSigner::from_seed(&app.authority.seed_bytes()).unwrap(),
    );
    let landed = app.sign_and_send(&adapted.wire, &[&hot, &authority]);
    println!(
        "programmatic request: {} bytes asked, {} bytes sent, {} compute units",
        request.len(),
        landed.size,
        landed.compute_units
    );
    assert_eq!(app.chain.balance(&app.recipient), SOL / 2);
    // The address paid what the site asked and not the fee. The hot wallet did.
    assert_eq!(app.chain.balance(&app.address), SOL / 2);
    assert!(app.chain.balance(&app.hot.pubkey()) < before);
    assert_eq!(app.chain.balance(&app.authority.pubkey()), 0);
}

#[test]
fn a_request_for_a_vault_becomes_a_proposal_that_runs_at_once_when_one_approval_is_enough() {
    for program in BOTH {
        let mut app = App::new();
        let squad = app.add_squad(
            program,
            vec![member(app.hot.pubkey(), PERMISSION_ALL)],
            1,
            app.hot_id,
        );
        let vault = squad.vault(0);
        let hot = KeypairSigner::from_seed(&app.hot.seed_bytes()).unwrap();

        // Two requests in a row: each takes the next transaction number.
        for number in 1..=2u64 {
            let request = app.site_request(&vault, vec![pay(&vault, &app.recipient, SOL)]);
            let adapted = app.adapt(&request).unwrap();
            assert_eq!(
                adapted.steps,
                vec![AdaptedStep::Squads {
                    wallet: "Treasury".into(),
                    vault,
                    member: hot.pubkey(),
                    index: number,
                    threshold: 1,
                    voters: vec![hot.pubkey()],
                    executor: Some(hot.pubkey()),
                    held_back: None,
                }],
                "{program:?}"
            );
            assert_eq!(signer_keys(&adapted.wire), vec![hot.pubkey()]);
            assert!(is_plain_account(&adapted.wire, &vault), "{program:?}");

            // The stored payment reads, and so does the proposal around it,
            // with the Squads IDL the app ships. The execution does not count
            // as read: it lists the stored transaction's accounts after the
            // ones its IDL names. So this is still Review, not Sign.
            let verdict = app.verdict(&adapted.wire, &[&hot]);
            assert!(verdict.completes(), "{program:?}");
            assert_eq!(verdict.unread, 1, "{program:?}");

            let landed = app.sign_and_send(&adapted.wire, &[&hot]);
            println!(
                "{program:?} vault request {number}: {} bytes asked, {} bytes sent, {} compute units",
                request.len(),
                landed.size,
                landed.compute_units
            );
            assert_eq!(
                app.chain.balance(&app.recipient),
                number * SOL,
                "{program:?}"
            );
        }
        assert_eq!(app.chain.balance(&vault), SOL, "{program:?}");
    }
}

#[test]
fn a_request_for_a_vault_that_needs_two_approvals_waits_as_an_approved_proposal() {
    for program in BOTH {
        let mut app = App::new();
        let other = KeypairSigner::new_random();
        app.chain.fund(&other.pubkey(), SOL);
        let squad = app.add_squad(
            program,
            vec![
                member(app.hot.pubkey(), PERMISSION_ALL),
                member(other.pubkey(), PERMISSION_ALL),
            ],
            2,
            app.hot_id,
        );
        let vault = squad.vault(0);
        let hot = KeypairSigner::from_seed(&app.hot.seed_bytes()).unwrap();
        let payment = vec![pay(&vault, &app.recipient, SOL)];

        let request = app.site_request(&vault, payment.clone());
        let adapted = app.adapt(&request).unwrap();
        assert!(
            matches!(
                adapted.steps[0],
                AdaptedStep::Squads {
                    index: 1,
                    threshold: 2,
                    ref voters,
                    executor: None,
                    ..
                } if voters.len() == 1
            ),
            "{program:?}"
        );
        app.sign_and_send(&adapted.wire, &[&hot]);
        // Stored and approved once. Nothing has moved.
        assert_eq!(app.chain.balance(&app.recipient), 0, "{program:?}");
        let state = squad.parse_state(&app.chain.data(&squad.address)).unwrap();
        assert_eq!(state.transaction_index, 1);

        // The other member approves and executes what was stored, which is
        // exactly what the site asked for.
        app.chain
            .send(
                &[&other],
                vec![
                    squad.approve(1, &other.pubkey()),
                    squad.execute(1, 0, &payment, &other.pubkey()),
                ],
            )
            .unwrap_or_else(|failure| panic!("{program:?}: {}", logs(&failure)));
        assert_eq!(app.chain.balance(&app.recipient), SOL, "{program:?}");
    }
}

/// The longest chain: the site asks the vault, the vault's member is a
/// programmatic signer, its authority is a key in this vault, and a hot
/// wallet pays.
#[test]
fn a_request_for_a_vault_whose_member_is_a_programmatic_signer_lands() {
    for program in BOTH {
        let mut app = App::new();
        let other = KeypairSigner::new_random();
        let squad = app.add_squad(
            program,
            vec![
                member(app.address, PERMISSION_ALL),
                member(other.pubkey(), PERMISSION_VOTE),
            ],
            1,
            app.signer_id,
        );
        let vault = squad.vault(0);
        let (hot, authority) = (
            KeypairSigner::from_seed(&app.hot.seed_bytes()).unwrap(),
            KeypairSigner::from_seed(&app.authority.seed_bytes()).unwrap(),
        );

        let request = app.site_request(&vault, vec![pay(&vault, &app.recipient, SOL)]);
        let adapted = app.adapt(&request).unwrap();
        assert_eq!(adapted.steps.len(), 2, "{program:?}");
        assert!(matches!(
            adapted.steps[0],
            AdaptedStep::Squads {
                member,
                executor: Some(executor),
                ..
            } if member == app.address && executor == app.address
        ));
        // The hot wallet alone signs the transaction. The vault and the
        // member's address are accounts in it.
        assert_eq!(signer_keys(&adapted.wire), vec![hot.pubkey()]);
        assert!(is_plain_account(&adapted.wire, &vault), "{program:?}");
        assert!(is_plain_account(&adapted.wire, &app.address), "{program:?}");

        let verdict = app.verdict(&adapted.wire, &[&hot, &authority]);
        assert_eq!((verdict.signable, verdict.elsewhere), (2, 0), "{program:?}");

        let landed = app.sign_and_send(&adapted.wire, &[&hot, &authority]);
        println!(
            "{program:?} vault request through a programmatic member: {} bytes asked, {} bytes sent, {} compute units",
            request.len(),
            landed.size,
            landed.compute_units
        );
        assert_eq!(app.chain.balance(&app.recipient), SOL, "{program:?}");
        assert_eq!(app.chain.balance(&vault), 2 * SOL, "{program:?}");
        assert_eq!(app.chain.balance(&app.address), 0, "{program:?}");
    }
}

/// The wallet record can be stale. What the chain says decides.
#[test]
fn a_member_the_chain_does_not_know_is_refused_before_anything_is_signed() {
    for program in BOTH {
        let mut app = App::new();
        let other = KeypairSigner::new_random();
        // The hot wallet is set as the member here, but is not one on chain.
        let squad = app.add_squad(
            program,
            vec![member(other.pubkey(), PERMISSION_ALL)],
            1,
            app.hot_id,
        );
        let vault = squad.vault(0);
        let request = app.site_request(&vault, vec![pay(&vault, &app.recipient, SOL)]);
        assert_eq!(
            app.adapt(&request),
            Err(AdaptError::NotAMember {
                squad: "Treasury".into(),
                member: app.hot.pubkey(),
            }),
            "{program:?}"
        );
    }
}

/// A site names a sponsor to pay, and the sponsor is not around. The hot
/// wallet set for the signer pays instead, and nothing else changes.
#[test]
fn a_sponsored_request_for_a_programmatic_signer_lands_with_the_hot_wallet_paying() {
    let mut app = App::new();
    app.chain.fund(&app.address, SOL);
    let sponsor = KeypairSigner::new_random();
    app.chain.fund(&sponsor.pubkey(), SOL);
    let (hot, authority) = (
        KeypairSigner::from_seed(&app.hot.seed_bytes()).unwrap(),
        KeypairSigner::from_seed(&app.authority.seed_bytes()).unwrap(),
    );
    let request = app.site_request(
        &sponsor.pubkey(),
        vec![pay(&app.address, &app.recipient, SOL / 2)],
    );

    // Rebuilt as sent, the sponsor's signature is still missing.
    let kept = app.adapt(&request).unwrap();
    assert_eq!(kept.fee_payer, sponsor.pubkey());
    assert_eq!(app.verdict(&kept.wire, &[&hot, &authority]).elsewhere, 1);

    let tx = decode_v1_transaction(&request).unwrap();
    let swap = plan_payer_swap(&tx, &app.wallets, None).unwrap();
    assert_eq!((swap.from, swap.to), (sponsor.pubkey(), hot.pubkey()));
    let swapped = swap_fee_payer(&tx, &swap).unwrap();
    let adapted = app.adapt(&swapped).unwrap();
    assert_eq!(signer_keys(&adapted.wire), vec![hot.pubkey()]);
    assert!(app.verdict(&adapted.wire, &[&hot, &authority]).allowed());

    app.sign_and_send(&adapted.wire, &[&hot, &authority]);
    assert_eq!(app.chain.balance(&app.recipient), SOL / 2);
    // The sponsor was left out entirely.
    assert_eq!(app.chain.balance(&sponsor.pubkey()), SOL);
}

/// The same for an ordinary key: the acting hot wallet takes the fee, and
/// its one signature finishes the transaction.
#[test]
fn a_sponsored_request_for_a_hot_wallet_lands_with_that_wallet_paying() {
    let mut app = App::new();
    let sponsor = KeypairSigner::new_random();
    app.chain.fund(&sponsor.pubkey(), SOL);
    let hot = KeypairSigner::from_seed(&app.hot.seed_bytes()).unwrap();
    let request = app.site_request(
        &sponsor.pubkey(),
        vec![pay(&hot.pubkey(), &app.recipient, SOL)],
    );
    assert!(!app.verdict(&request, &[&hot]).allowed());

    let tx = decode_v1_transaction(&request).unwrap();
    // Nothing is offered unless the app is acting as a hot wallet.
    assert_eq!(plan_payer_swap(&tx, &app.wallets, None), None);
    assert_eq!(
        plan_payer_swap(&tx, &app.wallets, Some(app.signer_id)),
        None
    );
    let swap = plan_payer_swap(&tx, &app.wallets, Some(app.hot_id)).unwrap();
    let swapped = swap_fee_payer(&tx, &swap).unwrap();
    assert_eq!(signer_keys(&swapped), vec![hot.pubkey()]);
    assert!(app.verdict(&swapped, &[&hot]).allowed());

    app.sign_and_send(&swapped, &[&hot]);
    assert_eq!(app.chain.balance(&app.recipient), SOL);
    assert_eq!(app.chain.balance(&sponsor.pubkey()), SOL);
}

/// What Build hands the Sign screen when the app acts as a vault: a hot
/// wallet pays, and the vault signs only its own instruction. The proposal
/// keeps that payer, here one the multisig was not set up with.
#[test]
fn a_transaction_built_here_for_a_vault_keeps_its_payer_and_lands_as_a_proposal() {
    for program in BOTH {
        let mut app = App::new();
        let squad = app.add_squad(
            program,
            vec![member(app.hot.pubkey(), PERMISSION_ALL)],
            1,
            app.hot_id,
        );
        let vault = squad.vault(0);
        let hot = KeypairSigner::from_seed(&app.hot.seed_bytes()).unwrap();
        let purse = KeypairSigner::new_random();
        app.chain.fund(&purse.pubkey(), SOL);
        app.wallets
            .push(WalletRecord::from_keypair_seed("Purse", purse.seed_bytes()));

        let built = app.site_request(&purse.pubkey(), vec![pay(&vault, &app.recipient, SOL)]);
        // As built, the vault's signature is one nobody can make.
        assert_eq!(app.verdict(&built, &[&hot, &purse]).elsewhere, 1);

        let adapted = app.adapt(&built).unwrap();
        assert_eq!(adapted.fee_payer, purse.pubkey(), "{program:?}");
        assert_eq!(
            signer_keys(&adapted.wire),
            vec![purse.pubkey(), hot.pubkey()]
        );
        assert!(is_plain_account(&adapted.wire, &vault), "{program:?}");
        assert!(app.verdict(&adapted.wire, &[&hot, &purse]).completes());

        let before = app.chain.balance(&hot.pubkey());
        app.sign_and_send(&adapted.wire, &[&hot, &purse]);
        assert_eq!(app.chain.balance(&app.recipient), SOL, "{program:?}");
        assert_eq!(app.chain.balance(&vault), 2 * SOL, "{program:?}");
        // The member signed and paid nothing.
        assert_eq!(app.chain.balance(&hot.pubkey()), before, "{program:?}");
    }
}

fn empty_sigs(n: usize) -> Vec<Signature> {
    vec![Signature::default(); n]
}

fn legacy_transfer(payer: &Pubkey, to: &Pubkey, lamports: u64, blockhash: Hash) -> Vec<u8> {
    let mut message = LegacyMessage::new(&[pay(payer, to, lamports)], Some(payer));
    message.recent_blockhash = blockhash;
    let nsig = usize::from(message.header.num_required_signatures);
    wincode::serialize(&VersionedTransaction {
        signatures: empty_sigs(nsig),
        message: VersionedMessage::Legacy(message),
    })
    .unwrap()
}

fn v0_transfer(
    payer: &Pubkey,
    lamports: u64,
    blockhash: Hash,
    table: Pubkey,
    loaded_index: u8,
) -> Vec<u8> {
    let header = solana_message::MessageHeader {
        num_required_signatures: 1,
        num_readonly_signed_accounts: 0,
        num_readonly_unsigned_accounts: 1,
    };
    let data = {
        let mut data = 2u32.to_le_bytes().to_vec();
        data.extend_from_slice(&lamports.to_le_bytes());
        data
    };
    let ix = solana_message::compiled_instruction::CompiledInstruction {
        program_id_index: 1,
        accounts: vec![0, 2],
        data,
    };
    let message = V0Message {
        header,
        account_keys: vec![*payer, wallet_core::SYSTEM_PROGRAM_ID],
        recent_blockhash: blockhash,
        instructions: vec![ix],
        address_table_lookups: vec![MessageAddressTableLookup {
            account_key: table,
            writable_indexes: vec![loaded_index],
            readonly_indexes: vec![],
        }],
    };
    wincode::serialize(&VersionedTransaction {
        signatures: empty_sigs(1),
        message: VersionedMessage::V0(message),
    })
    .unwrap()
}

#[test]
fn a_legacy_hot_request_is_signed_as_is() {
    let mut chain = Chain::new();
    let hot = KeypairSigner::new_random();
    let recipient = KeypairSigner::new_random().pubkey();
    chain.fund(&hot.pubkey(), 10 * SOL);
    let wire = legacy_transfer(&hot.pubkey(), &recipient, SOL, chain.latest_blockhash());
    assert_eq!(parse_wire(&wire).unwrap().format, TxFormat::Legacy);
    let tx = decode_transaction(&wire, &LookupTables::new()).unwrap();
    let hot_keys = [hot.pubkey()];
    assert_eq!(
        route_offline(&tx, &hot_keys, &hot_keys, false).unwrap(),
        Route::AsIs
    );

    let signed = sign_locally(&wire, &[&hot as &dyn WalletSigner])
        .unwrap()
        .wire;
    assert_eq!(parse_wire(&signed).unwrap().format, TxFormat::Legacy);
    // Only signature bytes change; the message is the same.
    let unsigned = parse_wire(&wire).unwrap();
    let signed_view = parse_wire(&signed).unwrap();
    assert_eq!(unsigned.message, signed_view.message);

    chain
        .send_wire(&signed)
        .unwrap_or_else(|failure| panic!("{}", logs(&failure)));
    assert_eq!(chain.balance(&recipient), SOL);
}

#[test]
fn a_v0_hot_request_keeps_a_sponsor_signature() {
    let hot = KeypairSigner::new_random();
    let sponsor = KeypairSigner::new_random();
    let recipient = KeypairSigner::new_random().pubkey();
    let header = solana_message::MessageHeader {
        num_required_signatures: 2,
        num_readonly_signed_accounts: 1,
        num_readonly_unsigned_accounts: 1,
    };
    let data = {
        let mut data = 2u32.to_le_bytes().to_vec();
        data.extend_from_slice(&SOL.to_le_bytes());
        data
    };
    let message = V0Message {
        header,
        account_keys: vec![
            hot.pubkey(),
            sponsor.pubkey(),
            recipient,
            wallet_core::SYSTEM_PROGRAM_ID,
        ],
        recent_blockhash: Hash::new_from_array([1; 32]),
        instructions: vec![solana_message::compiled_instruction::CompiledInstruction {
            program_id_index: 3,
            accounts: vec![0, 2],
            data,
        }],
        address_table_lookups: vec![],
    };
    let wire = wincode::serialize(&VersionedTransaction {
        signatures: empty_sigs(2),
        message: VersionedMessage::V0(message),
    })
    .unwrap();
    assert_eq!(parse_wire(&wire).unwrap().format, TxFormat::V0);

    let with_sponsor = apply_local_signatures(&wire, &[&sponsor as &dyn WalletSigner]).unwrap();
    let tx = decode_transaction(&with_sponsor, &LookupTables::new()).unwrap();
    assert!(tx.signers[1].valid);
    let hot_keys = [hot.pubkey()];
    assert_eq!(
        route_offline(&tx, &hot_keys, &hot_keys, false).unwrap(),
        Route::AsIs
    );

    let signed = sign_locally(&with_sponsor, &[&hot as &dyn WalletSigner])
        .unwrap()
        .wire;
    let after = decode_transaction(&signed, &LookupTables::new()).unwrap();
    assert!(after.signers[1].valid);
    assert_eq!(after.signers[1].pubkey, sponsor.pubkey());
    assert_eq!(parse_wire(&signed).unwrap().format, TxFormat::V0);
    assert_eq!(
        parse_wire(&with_sponsor).unwrap().message,
        parse_wire(&signed).unwrap().message
    );
}

#[test]
fn a_v0_offline_request_is_converted_to_v1() {
    let offline = KeypairSigner::new_random().pubkey();
    let recipient = KeypairSigner::new_random().pubkey();
    let table = Pubkey::new_from_array([9; 32]);
    let wire = v0_transfer(&offline, SOL, Hash::new_from_array([1; 32]), table, 0);
    let mut tables = LookupTables::new();
    tables.insert(table, vec![recipient]);
    let tx = decode_transaction(&wire, &tables).unwrap();
    assert_eq!(tx.format, TxFormat::V0);
    // The offline key is a wallet of this device, but not a hot one.
    let Route::Converted(converted, _) = route_offline(&tx, &[], &[offline], false).unwrap() else {
        panic!("an offline signer is converted");
    };
    assert_eq!(converted.from, TxFormat::V0);
    let v1 = decode_v1_transaction(&converted.wire).unwrap();
    assert_eq!(v1.format, TxFormat::V1);
    assert!(v1.accounts.iter().any(|a| a.pubkey == recipient));
}

#[test]
fn a_v0_programmatic_request_is_converted_then_rebuilt() {
    let app = App::new();
    let table = Pubkey::new_from_array([9; 32]);
    let wire = v0_transfer(&app.address, SOL, Hash::new_from_array([1; 32]), table, 0);
    let mut tables = LookupTables::new();
    tables.insert(table, vec![app.recipient]);
    let tx = decode_transaction(&wire, &tables).unwrap();
    let keyless = plan_adaptation(&tx, &app.wallets, None).unwrap().is_some();
    assert!(keyless);
    let hot_keys = [app.hot.pubkey()];
    let Route::Converted(converted, _) = route_offline(&tx, &hot_keys, &hot_keys, keyless).unwrap()
    else {
        panic!("a programmatic signer is converted");
    };
    let adapted = app.adapt(&converted.wire).unwrap();
    assert!(matches!(
        adapted.steps.first(),
        Some(AdaptedStep::Programmatic { .. })
    ));
    let hot = KeypairSigner::from_seed(&app.hot.seed_bytes()).unwrap();
    let authority = KeypairSigner::from_seed(&app.authority.seed_bytes()).unwrap();
    assert!(app.verdict(&adapted.wire, &[&hot, &authority]).completes());
}

#[test]
fn a_v0_with_an_outside_signature_and_offline_key_is_refused() {
    let offline = KeypairSigner::new_random();
    let outsider = KeypairSigner::new_random();
    let recipient = KeypairSigner::new_random().pubkey();
    let mut message = LegacyMessage::new(
        &[pay(&offline.pubkey(), &recipient, SOL)],
        Some(&offline.pubkey()),
    );
    message.account_keys.insert(1, outsider.pubkey());
    message.header.num_required_signatures = 2;
    message.recent_blockhash = Hash::new_from_array([1; 32]);
    let wire = wincode::serialize(&VersionedTransaction {
        signatures: empty_sigs(2),
        message: VersionedMessage::Legacy(message),
    })
    .unwrap();
    let signed = apply_local_signatures(&wire, &[&outsider as &dyn WalletSigner]).unwrap();
    let tx = decode_transaction(&signed, &LookupTables::new()).unwrap();
    assert_eq!(
        route_offline(&tx, &[], &[offline.pubkey()], false).unwrap_err(),
        FlowError::refused(ConvertError::OutsideSignature(outsider.pubkey()))
    );
}

/// What a site sends to make a token: a new account for `mint`, which the
/// site made and has signed with, set up as a mint `payer` controls. With
/// `holder`, the token account of `payer` for the mint is made too, at the
/// address derived from `mint`.
fn new_mint_request(
    chain: &Chain,
    payer: &Pubkey,
    mint: &KeypairSigner,
    holder: bool,
) -> wallet_core::DecodedTx {
    let token = wallet_core::TOKEN_PROGRAM_ID;
    let mut initialize = vec![20u8, 6];
    initialize.extend_from_slice(payer.as_ref());
    initialize.push(0);
    let mut instructions = vec![
        solana_system_interface::instruction::create_account(
            payer,
            &mint.pubkey(),
            chain.rent(82),
            82,
            &token,
        ),
        Instruction {
            program_id: token,
            accounts: vec![solana_instruction::AccountMeta::new(mint.pubkey(), false)],
            data: initialize,
        },
    ];
    if holder {
        let account = wallet_core::spl::associated_token_address(payer, &mint.pubkey(), &token);
        instructions.push(Instruction {
            program_id: wallet_core::ASSOCIATED_TOKEN_PROGRAM_ID,
            accounts: vec![
                solana_instruction::AccountMeta::new(*payer, true),
                solana_instruction::AccountMeta::new(account, false),
                solana_instruction::AccountMeta::new_readonly(*payer, false),
                solana_instruction::AccountMeta::new_readonly(mint.pubkey(), false),
                solana_instruction::AccountMeta::new_readonly(
                    wallet_core::SYSTEM_PROGRAM_ID,
                    false,
                ),
                solana_instruction::AccountMeta::new_readonly(token, false),
            ],
            data: vec![0],
        });
    }
    let mut message = LegacyMessage::new(&instructions, Some(payer));
    message.recent_blockhash = chain.latest_blockhash();
    let wire = wincode::serialize(&VersionedTransaction {
        signatures: empty_sigs(usize::from(message.header.num_required_signatures)),
        message: VersionedMessage::Legacy(message),
    })
    .unwrap();
    let signed = apply_local_signatures(&wire, &[mint as &dyn WalletSigner]).unwrap();
    decode_transaction(&signed, &LookupTables::new()).unwrap()
}

#[test]
fn a_new_mint_for_a_hot_wallet_is_signed_as_the_site_sent_it() {
    let mut chain = Chain::new();
    let hot = KeypairSigner::new_random();
    chain.fund(&hot.pubkey(), 10 * SOL);
    let mint = KeypairSigner::new_random();
    let tx = new_mint_request(&chain, &hot.pubkey(), &mint, true);
    let node = Node(Mutex::new(chain));
    let hot_keys = [hot.pubkey()];
    assert_eq!(
        pollster::block_on(route(Some(&node), &tx, &hot_keys, &hot_keys, false, false)).unwrap(),
        Route::AsIs
    );
    let signed = sign_locally(&tx.wire, &[&hot as &dyn WalletSigner]).unwrap();
    let mut chain = node.0.into_inner().unwrap();
    chain
        .send_wire(&signed.wire)
        .unwrap_or_else(|failure| panic!("{}", logs(&failure)));
    assert_eq!(chain.data(&mint.pubkey()).len(), 82);
}

#[test]
fn a_new_mint_for_an_offline_wallet_gets_a_key_made_here_and_lands() {
    let mut chain = Chain::new();
    let offline = KeypairSigner::new_random();
    chain.fund(&offline.pubkey(), 10 * SOL);
    let mint = KeypairSigner::new_random();
    let tx = new_mint_request(&chain, &offline.pubkey(), &mint, false);
    let node = Node(Mutex::new(chain));

    let Route::Converted(converted, keys) = pollster::block_on(route(
        Some(&node),
        &tx,
        &[],
        &[offline.pubkey()],
        false,
        false,
    ))
    .unwrap() else {
        panic!("an offline payer is converted");
    };
    let fresh = keys.pubkeys();
    assert_eq!(converted.replaced, vec![(mint.pubkey(), fresh[0])]);
    assert_eq!(converted.voided, 1);

    let mut signers = keys.signers();
    signers.push(&offline);
    let signed = sign_locally(&converted.wire, &signers).unwrap();
    let mut chain = node.0.into_inner().unwrap();
    chain
        .send_wire(&signed.wire)
        .unwrap_or_else(|failure| panic!("{}", logs(&failure)));
    assert_eq!(chain.data(&fresh[0]).len(), 82);
    assert!(chain.account(&mint.pubkey()).is_none());
}

#[test]
fn a_new_mint_whose_token_account_is_derived_from_it_cannot_be_reshaped() {
    let mut chain = Chain::new();
    let offline = KeypairSigner::new_random();
    chain.fund(&offline.pubkey(), 10 * SOL);
    let mint = KeypairSigner::new_random();
    let tx = new_mint_request(&chain, &offline.pubkey(), &mint, true);
    let node = Node(Mutex::new(chain));
    let err = pollster::block_on(route(
        Some(&node),
        &tx,
        &[],
        &[offline.pubkey()],
        false,
        false,
    ))
    .unwrap_err();
    let FlowError::Refused(text) = err else {
        panic!("refused, not {err:?}");
    };
    assert!(
        text.starts_with("Reshaping this transaction is not supported for this website."),
        "{text}"
    );
}

/// A Token-2022 mint with a metadata pointer and metadata, as a site sent it
/// to an offline wallet: v0, with the fee payer's slot and the new mint's
/// slot both left empty. The pointer names the mint in its data.
const SITE_MINT_REQUEST: &str = "AgAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAgAIAAwUkEZaHEHKVzelIqc9oJsDIClot4bYNOL4VgZjoiT6szW1+pyamtli0+I8TVZT5LA2H79IyqE+6iI5yL1z/6lwjAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAGp9UXGSxcUSGMyUw9SvF/WNruCJuh/UTj29mKAAAAAAbd9uHudY/eGEJdvORszdq2GvxNg7kNJ/69+SjYoYv8rqsRK+r1uV3fwXqLpc0vNX/b4/Jqzm8cX+kZimSZVXEEAgIAATQAAAAA2M0iAAAAAADqAAAAAAAAAAbd9uHudY/eGEJdvORszdq2GvxNg7kNJ/69+SjYoYv8BAEBQicAJBGWhxBylc3pSKnPaCbAyApaLeG2DTi+FYGY6Ik+rM1tfqcmprZYtPiPE1WU+SwNh+/SMqhPuoiOci9c/+pcIwQCAQNDAAYkEZaHEHKVzelIqc9oJsDIClot4bYNOL4VgZjoiT6szQEkEZaHEHKVzelIqc9oJsDIClot4bYNOL4VgZjoiT6szQQEAQABABfS4R6iWLhNjQEAAAB0AQAAAHQBAAAAdAA=";

#[test]
fn a_sites_unsigned_token_2022_mint_for_an_offline_wallet_gets_a_key_made_here_and_lands() {
    use base64::Engine;
    let mut chain = Chain::new();
    let offline = KeypairSigner::new_random();
    chain.fund(&offline.pubkey(), 10 * SOL);
    let sent = base64::engine::general_purpose::STANDARD
        .decode(SITE_MINT_REQUEST)
        .unwrap();
    let view = parse_wire(&sent).unwrap();
    let (site_payer, site_mint) = (view.static_keys[0], view.static_keys[1]);
    // The same request on this chain, paid by a key the test holds.
    let wire = wallet_core::renew(
        &sent,
        chain.latest_blockhash(),
        &[(site_payer, offline.pubkey())],
    )
    .unwrap();
    let tx = decode_transaction(&wire, &LookupTables::new()).unwrap();
    assert!(tx.signers.iter().all(|slot| !slot.filled));
    let node = Node(Mutex::new(chain));

    let Route::Converted(converted, keys) = pollster::block_on(route(
        Some(&node),
        &tx,
        &[],
        &[offline.pubkey()],
        false,
        false,
    ))
    .unwrap() else {
        panic!("an offline payer is converted");
    };
    let fresh = keys.pubkeys();
    assert_eq!(converted.replaced, vec![(site_mint, fresh[0])]);

    let mut signers = keys.signers();
    signers.push(&offline);
    let signed = sign_locally(&converted.wire, &signers).unwrap();
    let mut chain = node.0.into_inner().unwrap();
    chain
        .send_wire(&signed.wire)
        .unwrap_or_else(|failure| panic!("{}", logs(&failure)));
    let mint = chain
        .account(&fresh[0])
        .expect("the mint is made at the new key");
    assert_eq!(mint.owner, wallet_core::TOKEN_2022_PROGRAM_ID);
    // The metadata pointer names the new mint, not the site's.
    let data = chain.data(&fresh[0]);
    assert!(data.windows(32).any(|window| window == fresh[0].as_ref()));
    assert!(!data.windows(32).any(|window| window == site_mint.as_ref()));
}

#[test]
fn a_sites_unsigned_token_2022_mint_for_a_hot_wallet_is_signed_as_sent_and_the_site_adds_the_mint()
{
    use base64::Engine;
    let mut chain = Chain::new();
    let hot = KeypairSigner::new_random();
    chain.fund(&hot.pubkey(), 10 * SOL);
    let sent = base64::engine::general_purpose::STANDARD
        .decode(SITE_MINT_REQUEST)
        .unwrap();
    let view = parse_wire(&sent).unwrap();
    // The same request on this chain, for a payer and a mint the test holds.
    let mint = KeypairSigner::new_random();
    let wire = wallet_core::renew(
        &sent,
        chain.latest_blockhash(),
        &[
            (view.static_keys[0], hot.pubkey()),
            (view.static_keys[1], mint.pubkey()),
        ],
    )
    .unwrap();
    let tx = decode_transaction(&wire, &LookupTables::new()).unwrap();
    let node = Node(Mutex::new(chain));
    let hot_keys = [hot.pubkey()];
    assert_eq!(
        pollster::block_on(route(Some(&node), &tx, &hot_keys, &hot_keys, false, true)).unwrap(),
        Route::AsIs
    );

    // This device signs the site's own bytes, then the site adds its mint.
    let signed = apply_local_signatures(&wire, &[&hot as &dyn WalletSigner]).unwrap();
    assert_eq!(
        parse_wire(&signed).unwrap().message,
        parse_wire(&wire).unwrap().message
    );
    let finished = apply_local_signatures(&signed, &[&mint as &dyn WalletSigner]).unwrap();
    let mut chain = node.0.into_inner().unwrap();
    chain
        .send_wire(&finished)
        .unwrap_or_else(|failure| panic!("{}", logs(&failure)));
    let made = chain
        .account(&mint.pubkey())
        .expect("the site's mint is made");
    assert_eq!(made.owner, wallet_core::TOKEN_2022_PROGRAM_ID);
}

impl App {
    /// The mint request a site sent, on this chain and asking `payer` to pay,
    /// routed the way the app prepares it.
    fn route_site_mint(&mut self, payer: Pubkey) -> Route {
        use base64::Engine;
        let sent = base64::engine::general_purpose::STANDARD
            .decode(SITE_MINT_REQUEST)
            .unwrap();
        let site_payer = parse_wire(&sent).unwrap().static_keys[0];
        let wire = wallet_core::renew(&sent, self.chain.latest_blockhash(), &[(site_payer, payer)])
            .unwrap();
        let tx = decode_transaction(&wire, &LookupTables::new()).unwrap();
        let known: Vec<Pubkey> = self
            .wallets
            .iter()
            .filter_map(WalletRecord::address)
            .collect();
        let keyless =
            plan_adaptation_with(&tx, &self.wallets, None, &replaceable_signers(&tx, &known))
                .unwrap()
                .is_some();
        assert!(keyless);
        let hot_keys = [self.hot.pubkey(), self.authority.pubkey()];
        let node = Node(Mutex::new(std::mem::replace(&mut self.chain, Chain::new())));
        let routed = pollster::block_on(route(Some(&node), &tx, &hot_keys, &known, keyless, false));
        self.chain = node.0.into_inner().unwrap();
        routed.unwrap()
    }

    /// [`Self::adapt`], with keys made here for the request.
    fn adapt_with(&self, request: &[u8], made_here: &[Pubkey]) -> Result<Adapted, AdaptError> {
        let tx = decode_v1_transaction(request).unwrap();
        let plan = plan_adaptation_with(&tx, &self.wallets, None, made_here)?
            .expect("the request names a wallet without a key");
        let accounts: Vec<_> = plan
            .accounts_to_read()
            .into_iter()
            .filter_map(|address| Some((address, self.chain.account(&address)?)))
            .collect();
        plan.build(&tx, &accounts, self.chain.latest_blockhash())
    }
}

#[test]
fn a_sites_unsigned_token_2022_mint_for_a_programmatic_signer_lands_after_its_authorization() {
    let mut app = App::new();
    app.chain.fund(&app.address, 10 * SOL);
    let Route::Converted(converted, keys) = app.route_site_mint(app.address) else {
        panic!("a programmatic signer is converted");
    };
    let fresh = keys.pubkeys();
    let adapted = app.adapt_with(&converted.wire, &fresh).unwrap();
    assert!(signer_keys(&adapted.wire).contains(&fresh[0]));

    // The authorization is signed on its own, as when it comes from another
    // device, and the transaction afterwards with the key kept here.
    let authorized = sign_locally(&adapted.wire, &[&app.authority as &dyn WalletSigner]).unwrap();
    assert_eq!(authorized.authorizations, 1);
    let mut signers = keys.signers();
    signers.push(&app.hot);
    let signed = sign_locally(&authorized.wire, &signers).unwrap();
    assert_eq!(signed.dropped, 0);
    assert!(decode_v1_transaction(&signed.wire).unwrap().fully_signed());
    app.chain
        .send_wire(&signed.wire)
        .unwrap_or_else(|failure| panic!("{}", logs(&failure)));
    let mint = app.chain.account(&fresh[0]).expect("the mint is made");
    assert_eq!(mint.owner, wallet_core::TOKEN_2022_PROGRAM_ID);
}

#[test]
fn a_sites_unsigned_token_2022_mint_for_a_vault_executes_at_once_with_the_key_made_here() {
    for program in BOTH {
        let mut app = App::new();
        let squad = app.add_squad(
            program,
            vec![member(app.hot.pubkey(), PERMISSION_ALL)],
            1,
            app.hot_id,
        );
        let vault = squad.vault(0);
        let Route::Converted(converted, keys) = app.route_site_mint(vault) else {
            panic!("a vault is converted");
        };
        let fresh = keys.pubkeys();
        let adapted = app.adapt_with(&converted.wire, &fresh).unwrap();
        assert!(
            matches!(
                adapted.steps[0],
                AdaptedStep::Squads {
                    executor: Some(_),
                    held_back: None,
                    ..
                }
            ),
            "{program:?}"
        );
        assert_eq!(
            signer_keys(&adapted.wire),
            vec![app.hot.pubkey(), fresh[0]],
            "{program:?}"
        );
        let mut signers = keys.signers();
        signers.push(&app.hot);
        let signed = sign_locally(&adapted.wire, &signers).unwrap();
        app.chain
            .send_wire(&signed.wire)
            .unwrap_or_else(|failure| panic!("{program:?}: {}", logs(&failure)));
        let mint = app.chain.account(&fresh[0]).expect("the mint is made");
        assert_eq!(
            mint.owner,
            wallet_core::TOKEN_2022_PROGRAM_ID,
            "{program:?}"
        );
    }
}

#[test]
fn a_sites_mint_for_a_vault_that_cannot_execute_at_once_is_not_supported() {
    for program in BOTH {
        let mut app = App::new();
        let other = KeypairSigner::new_random();
        let squad = app.add_squad(
            program,
            vec![
                member(app.hot.pubkey(), PERMISSION_ALL),
                member(other.pubkey(), PERMISSION_ALL),
            ],
            2,
            app.hot_id,
        );
        let Route::Converted(converted, keys) = app.route_site_mint(squad.vault(0)) else {
            panic!("a vault is converted");
        };
        let fresh = keys.pubkeys();
        assert_eq!(
            app.adapt_with(&converted.wire, &fresh).unwrap_err(),
            AdaptError::NotAtOnce {
                squad: "Treasury".into(),
                signer: fresh[0],
                held: HeldBack::Votes { missing: 1 },
            },
            "{program:?}"
        );
    }
}

/// The bytes a site would send: a v0 transaction loading accounts from
/// `lookups`, each a table with its writable and readonly indexes.
fn v0_wire(
    payer: &Pubkey,
    statics: Vec<Pubkey>,
    readonly_unsigned: u8,
    instructions: Vec<solana_message::compiled_instruction::CompiledInstruction>,
    lookups: Vec<(Pubkey, Vec<u8>, Vec<u8>)>,
    blockhash: Hash,
) -> Vec<u8> {
    let message = V0Message {
        header: solana_message::MessageHeader {
            num_required_signatures: 1,
            num_readonly_signed_accounts: 0,
            num_readonly_unsigned_accounts: readonly_unsigned,
        },
        account_keys: [vec![*payer], statics].concat(),
        recent_blockhash: blockhash,
        instructions,
        address_table_lookups: lookups
            .into_iter()
            .map(
                |(account_key, writable_indexes, readonly_indexes)| MessageAddressTableLookup {
                    account_key,
                    writable_indexes,
                    readonly_indexes,
                },
            )
            .collect(),
    };
    wincode::serialize(&VersionedTransaction {
        signatures: empty_sigs(1),
        message: VersionedMessage::V0(message),
    })
    .unwrap()
}

/// [`route`] with no node, the way the app routes while offline.
fn route_offline(
    tx: &wallet_core::DecodedTx,
    hot: &[Pubkey],
    known: &[Pubkey],
    keyless: bool,
) -> Result<Route, FlowError> {
    pollster::block_on(route(None, tx, hot, known, keyless, false))
}

fn transfer_data(lamports: u64) -> Vec<u8> {
    let mut data = 2u32.to_le_bytes().to_vec();
    data.extend_from_slice(&lamports.to_le_bytes());
    data
}

/// The tables `wire` names, read from the chain the way the app reads them.
fn tables_on(chain: &Chain, wire: &[u8]) -> LookupTables {
    let mut tables = LookupTables::new();
    for table in wallet_core::lookup_tables_needed(wire).unwrap() {
        let addresses = wallet_core::parse_lookup_table_addresses(&chain.data(&table)).unwrap();
        tables.insert(table, addresses);
    }
    tables
}

#[test]
fn a_v0_hot_request_that_loads_from_a_table_is_signed_as_is_and_lands() {
    let mut chain = Chain::new();
    let hot = KeypairSigner::new_random();
    chain.fund(&hot.pubkey(), 10 * SOL);
    let recipient = KeypairSigner::new_random().pubkey();
    let table = chain.plant_lookup_table(&[recipient]);
    let wire = v0_wire(
        &hot.pubkey(),
        vec![wallet_core::SYSTEM_PROGRAM_ID],
        1,
        vec![solana_message::compiled_instruction::CompiledInstruction {
            program_id_index: 1,
            accounts: vec![0, 2],
            data: transfer_data(SOL),
        }],
        vec![(table, vec![0], vec![])],
        chain.latest_blockhash(),
    );

    let tables = tables_on(&chain, &wire);
    let tx = decode_transaction(&wire, &tables).unwrap();
    assert_eq!(tx.instructions[0].accounts, vec![hot.pubkey(), recipient]);
    let hot_keys = [hot.pubkey()];
    assert_eq!(
        route_offline(&tx, &hot_keys, &hot_keys, false).unwrap(),
        Route::AsIs
    );

    // Signing needs only the message, not the tables.
    let signed = sign_locally(&wire, &[&hot as &dyn WalletSigner]).unwrap();
    assert_eq!(signed.signatures, 1);
    let after = decode_transaction(&signed.wire, &tables).unwrap();
    assert_eq!(after.format, TxFormat::V0);
    assert!(after.fully_signed());

    chain
        .send_wire(&signed.wire)
        .unwrap_or_else(|failure| panic!("{}", logs(&failure)));
    assert_eq!(chain.balance(&recipient), SOL);
}

#[test]
fn a_v0_request_on_two_tables_is_converted_with_the_accounts_it_named() {
    let mut chain = Chain::new();
    let offline = KeypairSigner::new_random();
    chain.fund(&offline.pubkey(), 10 * SOL);
    let (first, second) = (
        KeypairSigner::new_random().pubkey(),
        KeypairSigner::new_random().pubkey(),
    );
    let bystander = KeypairSigner::new_random().pubkey();
    chain.fund(&bystander, SOL);
    // The first table has a readonly entry, so a decoder that does not put
    // every writable account first would swap `second` and `bystander`.
    let table_a = chain.plant_lookup_table(&[first, bystander]);
    let table_b = chain.plant_lookup_table(&[second]);
    // 0 payer, 1 system program, 2 first, 3 second, 4 bystander.
    let wire = v0_wire(
        &offline.pubkey(),
        vec![wallet_core::SYSTEM_PROGRAM_ID],
        1,
        vec![
            solana_message::compiled_instruction::CompiledInstruction {
                program_id_index: 1,
                accounts: vec![0, 2],
                data: transfer_data(SOL),
            },
            solana_message::compiled_instruction::CompiledInstruction {
                program_id_index: 1,
                accounts: vec![0, 3, 4],
                data: transfer_data(2 * SOL),
            },
        ],
        vec![(table_a, vec![0], vec![1]), (table_b, vec![0], vec![])],
        chain.latest_blockhash(),
    );

    let tx = decode_transaction(&wire, &tables_on(&chain, &wire)).unwrap();
    assert_eq!(
        tx.instructions[1].accounts,
        vec![offline.pubkey(), second, bystander]
    );
    let Route::Converted(converted, _) =
        route_offline(&tx, &[], &[offline.pubkey()], false).unwrap()
    else {
        panic!("an offline signer is converted");
    };
    assert_eq!((converted.loaded, converted.tables), (3, 2));
    // With no node and no limit from the site, two transfers get what the
    // runtime gives two System instructions, and that is enough.
    assert_eq!(
        (converted.cu_limit, converted.limit_from),
        (6_000, LimitFrom::Runtime)
    );

    // The offline device signs the v1 rewrite, and it moves what the site asked.
    let signed = sign_locally(&converted.wire, &[&offline as &dyn WalletSigner]).unwrap();
    chain
        .send_wire(&signed.wire)
        .unwrap_or_else(|failure| panic!("{}", logs(&failure)));
    assert_eq!(chain.balance(&first), SOL);
    assert_eq!(chain.balance(&second), 2 * SOL);
    assert_eq!(chain.balance(&bystander), SOL);
}

#[test]
fn with_a_node_a_rewrite_asks_for_what_it_uses_at_the_sites_price_and_lands() {
    let mut chain = Chain::new();
    let offline = KeypairSigner::new_random();
    chain.fund(&offline.pubkey(), 10 * SOL);
    let recipient = KeypairSigner::new_random().pubkey();
    let price = Instruction {
        program_id: wallet_core::COMPUTE_BUDGET_PROGRAM_ID,
        accounts: Vec::new(),
        data: [vec![3u8], 50_000u64.to_le_bytes().to_vec()].concat(),
    };
    let mut message = LegacyMessage::new(
        &[price, pay(&offline.pubkey(), &recipient, SOL)],
        Some(&offline.pubkey()),
    );
    message.recent_blockhash = chain.latest_blockhash();
    let wire = wincode::serialize(&VersionedTransaction {
        signatures: empty_sigs(1),
        message: VersionedMessage::Legacy(message),
    })
    .unwrap();
    let tx = decode_transaction(&wire, &LookupTables::new()).unwrap();
    let node = Node(Mutex::new(chain));

    let Route::Converted(converted, _) = pollster::block_on(route(
        Some(&node),
        &tx,
        &[],
        &[offline.pubkey()],
        false,
        false,
    ))
    .unwrap() else {
        panic!("an offline signer is converted");
    };
    assert_eq!(converted.limit_from, LimitFrom::Simulated);
    // Far less than the 3,000 the runtime would have given the transfer.
    assert!(converted.cu_limit < 1_000, "{}", converted.cu_limit);
    let budget = decode_v1_transaction(&converted.wire).unwrap().budget;
    let priority = budget.priority_fee.expect("the site's price is kept");
    assert_eq!(
        u128::from(priority),
        (50_000u128 * u128::from(converted.cu_limit)).div_ceil(1_000_000)
    );
    assert!(budget.loaded_accounts_data_size_limit.unwrap() < 1024 * 1024);

    let signed = sign_locally(&converted.wire, &[&offline as &dyn WalletSigner]).unwrap();
    let mut chain = node.0.into_inner().unwrap();
    let landed = chain
        .send_wire(&signed.wire)
        .unwrap_or_else(|failure| panic!("{}", logs(&failure)));
    assert!(landed.compute_units <= u64::from(converted.cu_limit));
    assert_eq!(chain.balance(&recipient), SOL);
    assert_eq!(
        chain.balance(&offline.pubkey()),
        10 * SOL - SOL - wallet_core::LAMPORTS_PER_SIGNATURE - priority
    );
}

/// Issue #64: the approving member pays and receives. A compiled message
/// marks the fee payer as a signer in every instruction that names it; the
/// wrap must still store a plain transfer to that member and land.
#[test]
fn a_send_to_the_fee_payer_member_lands_as_a_proposal() {
    for program in BOTH {
        let mut app = App::new();
        let squad = app.add_squad(
            program,
            vec![member(app.hot.pubkey(), PERMISSION_ALL)],
            1,
            app.hot_id,
        );
        app.wallets
            .last_mut()
            .unwrap()
            .kind
            .set_fee_payer(Some(app.hot_id));
        let vault = squad.vault(0);
        let hot = KeypairSigner::from_seed(&app.hot.seed_bytes()).unwrap();
        let before = app.chain.balance(&hot.pubkey());

        let built = app.site_request(&hot.pubkey(), vec![pay(&vault, &hot.pubkey(), SOL)]);
        assert_eq!(app.verdict(&built, &[&hot]).elsewhere, 1, "{program:?}");

        let adapted = app
            .adapt(&built)
            .unwrap_or_else(|e| panic!("{program:?}: {e}"));
        assert_eq!(adapted.fee_payer, hot.pubkey(), "{program:?}");
        assert_eq!(
            signer_keys(&adapted.wire),
            vec![hot.pubkey()],
            "{program:?}"
        );
        assert!(is_plain_account(&adapted.wire, &vault), "{program:?}");
        assert!(
            app.verdict(&adapted.wire, &[&hot]).completes(),
            "{program:?}"
        );

        app.sign_and_send(&adapted.wire, &[&hot]);
        assert_eq!(app.chain.balance(&vault), 2 * SOL, "{program:?}");
        // Member received SOL from the vault and paid the outer fee.
        assert!(
            app.chain.balance(&hot.pubkey()) > before,
            "{program:?}: member should have received the transfer"
        );
    }
}
