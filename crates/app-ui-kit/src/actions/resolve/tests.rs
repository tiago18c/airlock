//! A `resolve` request on a running window: its notes applied once for the
//! request, with the keys it makes kept as keys of this device.

use super::*;
use crate::state::session::OnlineHooks;
use crate::state::submit::SubmitHandle;
use crate::state::use_session;
use app_core::bridge::{BridgeRequest, SiteCall, SiteTransaction, SiteWire};
use app_core::testing::{kit, Kit};
use app_core::Core;
use dioxus::dioxus_core::{NoOpMutations, ScopeId, VirtualDom};
use dioxus::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;
use wallet_core::intent::{plan_resolution, AccountNote, Notes, Order, Resolution, State, Timing};
use wallet_core::{compile_v1, SolanaRpc, TxFormat, WalletSigner};
use wallet_flows::testing::{pk, FakeRpc};

#[derive(Clone)]
struct Held {
    core: Core,
    rpc: Arc<FakeRpc>,
    session: Rc<Cell<Option<Session>>>,
}

fn app(held: Held) -> Element {
    let core = use_hook(|| CopyValue::new(held.core.clone()));
    let rpc = held.rpc.clone();
    let hooks = OnlineHooks::new(
        move || Ok(rpc.clone() as Arc<dyn SolanaRpc>),
        || {},
        || {},
        || Box::pin(async { Ok(String::new()) }),
    );
    let session = use_session(core, hooks, SubmitHandle::none());
    held.session.set(Some(session));
    rsx! {}
}

/// A running window, with a vault that holds one hot key.
struct Screen {
    kit: Kit,
    dom: VirtualDom,
    session: Session,
    key: Pubkey,
}

const ORIGIN: &str = "https://mint.test";

impl Screen {
    async fn on(rpc: FakeRpc) -> Self {
        let kit = kit();
        kit.core.create_vault("pw").unwrap();
        let wallet = wallet_core::WalletRecord::from_keypair_seed("Hot", [7; 32]);
        let key = wallet.address().unwrap();
        let signers = vec![wallet_core::NewSigner {
            pubkey: key,
            label: wallet.label.clone(),
            origin: wallet_core::Origin::Imported,
            wallet,
        }];
        kit.core
            .enroll(
                "add_signers",
                |data| data.add_signers(&signers),
                |book, _| wallet_core::name_signers(book, &signers),
            )
            .unwrap();
        let session = Rc::new(Cell::new(None));
        let mut dom = VirtualDom::new_with_props(
            app,
            Held {
                core: kit.core.clone(),
                rpc: Arc::new(rpc),
                session: session.clone(),
            },
        );
        dom.rebuild_in_place();
        let session = session.get().unwrap();
        let mut screen = Self {
            kit,
            dom,
            session,
            key,
        };
        screen.settle().await;
        screen
    }

    fn act<R>(&self, then: impl FnOnce() -> R) -> R {
        self.dom.in_scope(ScopeId::ROOT, then)
    }

    async fn settle(&mut self) {
        for _ in 0..4 {
            let _ = tokio::time::timeout(Duration::from_millis(40), self.dom.wait_for_work()).await;
            self.dom.render_immediate(&mut NoOpMutations);
        }
    }

    /// A `resolve` from [`ORIGIN`], queued as the bridge queues one: a
    /// transaction this key pays for that creates `mint`, whose notes let
    /// the wallet replace the mint's key.
    fn queue_mint(&self, request: &str, resolution: &str, mint: Pubkey) -> ResolveCall {
        let create = solana_system_interface::instruction::create_account(
            &self.key,
            &mint,
            1_461_600,
            82,
            &pk(9),
        );
        let wire = compile_v1(
            self.key,
            vec![create],
            None,
            Some(solana_hash::Hash::new_from_array([3; 32])),
        )
        .unwrap()
        .wire;
        let notes = Notes {
            accounts: [(mint, AccountNote::New { replaceable: true })]
                .into_iter()
                .collect(),
            data: Vec::new(),
        };
        let (profile, payer) = self.kit.core.resolution_profile(self.key).unwrap();
        let plan =
            plan_resolution(&notes, &profile, Delivery::Send, Order::Serial, 1, payer).unwrap();
        let call = ResolveCall {
            resolution: resolution.into(),
            account: self.key,
            chain: "solana:devnet".into(),
            transactions: vec![SiteTransaction {
                account: self.key,
                wire: SiteWire::Readable(TxFormat::V1, wire),
                chain: Some("solana:devnet".into()),
                commitment: None,
            }],
            notes,
            order: Order::Serial,
            delivery: Delivery::Send,
            profile,
            plan,
            notes_checked: true,
            message: None,
        };
        self.kit
            .core
            .resolution_create(
                BridgeRequest {
                    id: request.into(),
                    origin: ORIGIN.into(),
                    call: SiteCall::Resolve(Box::new(call.clone())),
                },
                Resolution::new(
                    resolution,
                    self.key,
                    "solana:devnet",
                    Timing::Sync,
                    Delivery::Send,
                    1,
                    0,
                ),
            )
            .unwrap();
        call
    }

    /// Prepare `call` as the modal does, `times` times at once.
    async fn resolve(
        &mut self,
        request: &str,
        call: &ResolveCall,
        times: usize,
    ) -> Vec<Result<ResolvedRequest, String>> {
        let out = Rc::new(RefCell::new(Vec::new()));
        for _ in 0..times {
            let (out, session, call, request) =
                (out.clone(), self.session, call.clone(), request.to_owned());
            self.act(|| {
                spawn(async move {
                    let done = resolve_request(session, ORIGIN, &request, &call).await;
                    out.borrow_mut().push(done);
                })
            });
        }
        self.settle().await;
        let done = out.borrow().clone();
        done
    }
}

#[tokio::test]
async fn a_new_mint_gets_one_key_made_here_however_often_the_request_is_prepared() {
    let mut screen = Screen::on(FakeRpc::default()).await;
    let mint = pk(50);
    let call = screen.queue_mint("q1", "r1", mint);

    // Two cards, or a retry and an unlock, prepare it at once.
    let done = screen.resolve("q1", &call, 2).await;
    assert_eq!(done.len(), 2);
    let first = done[0].clone().unwrap();
    assert_eq!(done[1].clone().unwrap(), first);
    let again = screen.resolve("q1", &call, 1).await.remove(0).unwrap();
    assert_eq!(again, first);

    assert_eq!(first.new_keys.len(), 1);
    let (old, made) = first.new_keys[0];
    assert_eq!(old, mint);
    assert_ne!(made, mint);
    assert_eq!(first.replaced.get(&mint), Some(&made));
    // It signs in the mint's place, and with this device's keys.
    let view = wallet_core::parse_wire(&first.wires[0]).unwrap();
    let signers = &view.static_keys[..view.num_required_signatures];
    assert!(signers.contains(&made) && !signers.contains(&mint));
    let local = screen.act(|| screen.session.local_signer_pubkeys());
    assert_eq!(local.iter().filter(|key| **key == made).count(), 1);
    let signing = screen.act(|| screen.session.local_signers_for(&first.wires[0]));
    assert!(signing.iter().any(|signer| signer.pubkey() == made));
    // No guess replaces it again, and nothing is left for the site.
    assert!(!first.kept().replaceable(&made));
    assert!(first.app_signs.is_empty());
    // The site hears which address became which.
    let resolution = screen.kit.core.resolution(ORIGIN, "r1").unwrap();
    assert_eq!(resolution.replaced.get(&mint), Some(&made));
    assert_eq!(resolution.state, State::AwaitingApproval);
}

#[tokio::test]
async fn a_new_account_the_chain_has_seen_fails_the_resolution_and_leaves_the_queue() {
    let mint = pk(51);
    let mut screen = Screen::on(FakeRpc {
        used: vec![mint],
        ..FakeRpc::default()
    })
    .await;
    let call = screen.queue_mint("q2", "r2", mint);
    let done = screen.resolve("q2", &call, 1).await.remove(0);
    let why = done.unwrap_err();
    assert!(why.contains(&mint.to_string()), "{why}");
    let resolution = screen.kit.core.resolution(ORIGIN, "r2").unwrap();
    assert_eq!(resolution.state, State::Failed);
    assert_eq!(resolution.error.unwrap().code, "bad_notes");
    assert!(screen.kit.core.slices().bridge.get().get("q2").is_none());
    // No key was made for it.
    assert!(screen
        .act(|| screen.session.forms.sign.resolved("q2"))
        .is_none());
}

/// A v4 multisig of `member` alone, at `transaction_index`, as the program
/// lays out its account.
fn multisig_account(
    squad: &wallet_core::squads::Squad,
    member: Pubkey,
    transaction_index: u64,
) -> wallet_core::RawAccount {
    let mut data = hex::decode("e07479ba44a14fec").unwrap();
    data.extend_from_slice(&[0u8; 64]);
    data.extend_from_slice(&1u16.to_le_bytes());
    data.extend_from_slice(&0u32.to_le_bytes());
    data.extend_from_slice(&transaction_index.to_le_bytes());
    data.extend_from_slice(&0u64.to_le_bytes());
    data.extend_from_slice(&[0, 255]);
    data.extend_from_slice(&1u32.to_le_bytes());
    data.extend_from_slice(member.as_ref());
    data.push(wallet_core::squads::PERMISSION_ALL);
    wallet_core::RawAccount {
        lamports: 1,
        data,
        owner: squad.program.program_id(),
        executable: false,
        rent_epoch: 0,
    }
}

/// The hot key of [`Screen::on`], made from the same seed.
fn hot_key() -> Pubkey {
    wallet_core::WalletRecord::from_keypair_seed("Hot", [7; 32])
        .address()
        .unwrap()
}

impl Screen {
    /// Add a multisig acted on and paid for by the hot key. Answers its vault.
    fn add_squad(&self, squad: wallet_core::squads::Squad) -> Pubkey {
        let hot = self.kit.core.with_wallets(|data| {
            data.unwrap()
                .wallets
                .iter()
                .find(|wallet| wallet.address() == Some(self.key))
                .unwrap()
                .id
        });
        let mut wallet = wallet_core::WalletRecord::squads("Treasury", squad, vec![self.key], 1);
        wallet.kind.set_member_wallet(Some(hot));
        wallet.kind.set_fee_payer(Some(hot));
        let vault = wallet.address().unwrap();
        self.kit
            .core
            .enroll(
                "add_contract_wallet",
                |data| data.add_contract_wallet(wallet),
                |_, _| {},
            )
            .unwrap();
        vault
    }

    /// A `resolve` for `vault` whose transaction `i` creates the accounts
    /// `creates[i]`, each marked new and replaceable, queued as the bridge
    /// queues one.
    fn queue_vault(
        &self,
        request: &str,
        resolution: &str,
        vault: Pubkey,
        creates: &[Vec<Pubkey>],
    ) -> ResolveCall {
        let wires: Vec<Vec<u8>> = creates
            .iter()
            .map(|mints| {
                let instructions = mints
                    .iter()
                    .map(|mint| {
                        solana_system_interface::instruction::create_account(
                            &vault,
                            mint,
                            1_461_600,
                            82,
                            &pk(9),
                        )
                    })
                    .collect();
                compile_v1(
                    vault,
                    instructions,
                    None,
                    Some(solana_hash::Hash::new_from_array([3; 32])),
                )
                .unwrap()
                .wire
            })
            .collect();
        let mut all: Vec<Pubkey> = Vec::new();
        for mint in creates.iter().flatten() {
            if !all.contains(mint) {
                all.push(*mint);
            }
        }
        let notes = Notes {
            accounts: all
                .into_iter()
                .map(|mint| (mint, AccountNote::New { replaceable: true }))
                .collect(),
            data: Vec::new(),
        };
        let (profile, payer) = self.kit.core.resolution_profile(vault).unwrap();
        let plan = plan_resolution(
            &notes,
            &profile,
            Delivery::Send,
            Order::Serial,
            wires.len(),
            payer,
        )
        .unwrap();
        let transactions: Vec<SiteTransaction> = wires
            .into_iter()
            .map(|wire| SiteTransaction {
                account: vault,
                wire: SiteWire::Readable(TxFormat::V1, wire),
                chain: Some("solana:devnet".into()),
                commitment: None,
            })
            .collect();
        let count = transactions.len();
        let call = ResolveCall {
            resolution: resolution.into(),
            account: vault,
            chain: "solana:devnet".into(),
            transactions,
            notes,
            order: Order::Serial,
            delivery: Delivery::Send,
            profile: profile.clone(),
            plan,
            notes_checked: true,
            message: None,
        };
        self.kit
            .core
            .resolution_create(
                BridgeRequest {
                    id: request.into(),
                    origin: ORIGIN.into(),
                    call: SiteCall::Resolve(Box::new(call.clone())),
                },
                Resolution::new(
                    resolution,
                    vault,
                    "solana:devnet",
                    profile.timing,
                    Delivery::Send,
                    count,
                    0,
                ),
            )
            .unwrap();
        call
    }
}

/// A multisig at transaction 6, with its account on the node.
async fn multisig_screen() -> (Screen, wallet_core::squads::Squad, Pubkey) {
    let squad = wallet_core::squads::Squad::v4(&pk(60));
    let rpc = FakeRpc {
        accounts: [(squad.address, multisig_account(&squad, hot_key(), 6))]
            .into_iter()
            .collect(),
        ..FakeRpc::default()
    };
    let screen = Screen::on(rpc).await;
    let vault = screen.add_squad(squad);
    (screen, squad, vault)
}

#[tokio::test]
async fn a_multisigs_new_account_takes_an_ephemeral_signer_of_its_next_proposal() {
    let (mut screen, squad, vault) = multisig_screen().await;
    let (first_mint, second_mint, third_mint) = (pk(52), pk(53), pk(54));
    // Two transactions: the first creates one mint, the second two.
    let call = screen.queue_vault(
        "q3",
        "r3",
        vault,
        &[vec![first_mint], vec![second_mint, third_mint]],
    );
    assert_eq!(call.plan.carried, vec![first_mint, second_mint, third_mint]);
    assert!(call.plan.new_keys.is_empty());

    let done = screen.resolve("q3", &call, 2).await;
    let first = done[0].clone().unwrap();
    assert_eq!(done[1].clone().unwrap(), first);
    assert_eq!(first.derived_at, Some(6));
    // The first transaction is proposal 7, the second proposal 8, its two
    // accounts numbered in the notes' order.
    assert_eq!(
        first.carried,
        vec![
            (first_mint, squad.ephemeral_signer(7, 0)),
            (second_mint, squad.ephemeral_signer(8, 0)),
            (third_mint, squad.ephemeral_signer(8, 1)),
        ]
    );
    for (old, new) in &first.carried {
        assert_eq!(first.replaced.get(old), Some(new));
    }
    // No key was made, and the rebuild may name them inside a proposal.
    assert!(first.new_keys.is_empty());
    let local = screen.act(|| screen.session.local_signer_pubkeys());
    assert!(!local.contains(&squad.ephemeral_signer(7, 0)));
    let carried = screen.act(|| screen.session.forms.sign.carried_addresses());
    assert_eq!(carried.len(), 3);
    let resolution = screen.kit.core.resolution(ORIGIN, "r3").unwrap();
    assert_eq!(
        resolution.replaced.get(&first_mint),
        Some(&squad.ephemeral_signer(7, 0))
    );
    // Each transaction stores a proposal, after the ones before it.
    let ahead = screen.act(|| {
        crate::actions::sign::proposals_ahead(screen.session, &first.wires[..1], &first.wires[1])
    });
    assert_eq!(ahead, 1);
}

#[tokio::test]
async fn a_kept_request_is_derived_again_when_the_proposal_number_moved() {
    let (mut screen, squad, vault) = multisig_screen().await;
    let call = screen.queue_vault("q4", "r4", vault, &[vec![pk(55)]]);
    let first = screen.resolve("q4", &call, 1).await.remove(0).unwrap();
    // What another preparation finds once another proposal was stored.
    let mut moved = first.clone();
    moved.derived_at = Some(7);
    moved.carried = vec![(pk(55), squad.ephemeral_signer(8, 0))];
    let kept = screen.act(|| screen.session.forms.sign.keep_resolved(moved.clone()));
    assert_eq!(kept, moved);
    let carried = screen.act(|| screen.session.forms.sign.carried_addresses());
    assert_eq!(carried, vec![squad.ephemeral_signer(8, 0)]);
    // At the same number, the one kept stays.
    let mut other = moved.clone();
    other.wires.clear();
    let same = screen.act(|| screen.session.forms.sign.keep_resolved(other));
    assert_eq!(same, moved);
}

#[tokio::test]
async fn a_new_account_a_multisig_would_sign_twice_fails_as_unsupported() {
    let (mut screen, _, vault) = multisig_screen().await;
    let mint = pk(56);
    let call = screen.queue_vault("q5", "r5", vault, &[vec![mint], vec![mint]]);
    let why = screen.resolve("q5", &call, 1).await.remove(0).unwrap_err();
    assert!(why.contains("more than one"), "{why}");
    let resolution = screen.kit.core.resolution(ORIGIN, "r5").unwrap();
    assert_eq!(resolution.state, State::Failed);
    let error = resolution.error.unwrap();
    assert_eq!(error.code, "unsupported");
    assert_eq!(
        error.requirement,
        Some(wallet_core::intent::Requirement::NewAccounts)
    );
}

#[tokio::test]
async fn a_multisig_the_node_does_not_hold_leaves_the_request_waiting() {
    let squad = wallet_core::squads::Squad::v4(&pk(61));
    let screen = Screen::on(FakeRpc::default()).await;
    let vault = screen.add_squad(squad);
    let mut screen = screen;
    let call = screen.queue_vault("q6", "r6", vault, &[vec![pk(57)]]);
    // Nothing is derived, and nothing failed: it can be tried again.
    let why = screen.resolve("q6", &call, 1).await.remove(0).unwrap_err();
    assert!(why.contains("not on this network"), "{why}");
    let resolution = screen.kit.core.resolution(ORIGIN, "r6").unwrap();
    assert_eq!(resolution.state, State::AwaitingApproval);
}
