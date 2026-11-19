//! Reading multisigs again through the real session, on a window with no
//! screen drawn, against a node written out in the test.

use super::*;
use crate::state::session::OnlineHooks;
use crate::state::submit::SubmitHandle;
use crate::state::use_session;
use app_core::testing::{kit, Kit};
use app_core::Core;
use dioxus::dioxus_core::{NoOpMutations, ScopeId, VirtualDom};
use dioxus::prelude::*;
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use wallet_core::squads::{SquadsMember, SquadsProgram, PERMISSION_ALL, PERMISSION_VOTE};
use wallet_core::{NewSigner, Origin, SolanaRpc, WalletRecord};
use wallet_flows::testing::{multisig_account, pk, FakeRpc};

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

/// The two hot keys every screen holds, by their seeds.
fn hot(label: &str, seed: u8) -> NewSigner {
    let wallet = WalletRecord::from_keypair_seed(label, [seed; 32]);
    NewSigner {
        pubkey: wallet.address().unwrap(),
        label: label.into(),
        origin: Origin::Imported,
        wallet,
    }
}

fn key_a() -> Pubkey {
    hot("A", 7).pubkey
}

fn key_b() -> Pubkey {
    hot("B", 8).pubkey
}

/// A window over a vault with two hot keys, A and B.
struct Screen {
    kit: Kit,
    dom: VirtualDom,
    session: Session,
    a: WalletId,
    b: WalletId,
}

impl Screen {
    async fn new(rpc: FakeRpc) -> Self {
        let kit = kit();
        kit.core.create_vault("pw").unwrap();
        let signers = vec![hot("A", 7), hot("B", 8)];
        let (a, b) = (signers[0].wallet.id, signers[1].wallet.id);
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
            a,
            b,
        };
        screen.settle().await;
        screen
    }

    fn act<R>(&self, then: impl FnOnce() -> R) -> R {
        self.dom.in_scope(ScopeId::ROOT, then)
    }

    async fn settle(&mut self) {
        for _ in 0..4 {
            let _ = tokio::time::timeout(
                std::time::Duration::from_millis(40),
                self.dom.wait_for_work(),
            )
            .await;
            self.dom.render_immediate(&mut NoOpMutations);
        }
    }

    /// Run `fut`, made with the session, inside the window until it ends.
    async fn run<F>(&mut self, fut: impl FnOnce(Session) -> F) -> F::Output
    where
        F: std::future::Future + 'static,
        F::Output: 'static,
    {
        let out = Rc::new(std::cell::RefCell::new(None));
        let put = out.clone();
        let fut = fut(self.session);
        self.act(|| {
            spawn(async move {
                let done = fut.await;
                *put.borrow_mut() = Some(done);
            })
        });
        for _ in 0..50 {
            self.settle().await;
            if let Some(done) = out.borrow_mut().take() {
                return done;
            }
        }
        panic!("it did not end");
    }

    /// Add `wallet` as it was stored, before any reading.
    fn add(&self, wallet: WalletRecord) -> WalletId {
        let id = wallet.id;
        self.act(|| self.session.add_contract_wallet(wallet))
            .unwrap();
        id
    }

    fn kind(&self, id: WalletId) -> WalletKind {
        self.act(|| {
            self.session
                .wallet_summaries()
                .into_iter()
                .find(|wallet| wallet.id == id)
                .and_then(|wallet| wallet.contract)
                .unwrap()
        })
    }

    fn vault_writes(&self) -> usize {
        self.kit.storage.writes(app_core::storage::VAULT_FILE)
    }
}

fn on_chain(members: &[(Pubkey, u8)], threshold: u16) -> SquadState {
    SquadState {
        threshold,
        time_lock: 0,
        transaction_index: 3,
        stale_transaction_index: 0,
        members: members
            .iter()
            .map(|(key, permissions)| SquadsMember {
                key: *key,
                permissions: *permissions,
            })
            .collect(),
        config_authority: None,
        rent_collector: None,
    }
}

#[tokio::test]
async fn every_multisig_follows_the_chain_and_a_chosen_member_that_left_stays_chosen() {
    let (a, b) = (key_a(), key_b());
    let squad = Squad::v4(&pk(40));
    let smart = Squad::smart_account(41);
    let missing = Squad::v4(&pk(42));
    let mut now = on_chain(&[(b, PERMISSION_ALL), (pk(3), PERMISSION_VOTE)], 2);
    now.time_lock = 3_600;
    now.rent_collector = Some(pk(9));
    let rpc = FakeRpc::default()
        .with_account(squad.address, multisig_account(SquadsProgram::V4, &now))
        .with_account(
            smart.address,
            multisig_account(
                SquadsProgram::SmartAccount,
                &on_chain(&[(a, PERMISSION_ALL)], 1),
            ),
        );
    let mut screen = Screen::new(rpc).await;

    // As stored: A acted, B approved with it, and A was a member.
    let mut team = WalletRecord::squads("Team", squad, vec![a, pk(2)], 1);
    team.kind.set_member_wallet(Some(screen.a));
    team.kind.set_approves_with(vec![screen.b]);
    let team = screen.add(team);
    // Imported without its members: nobody acts for it yet.
    let fresh = screen.add(WalletRecord::squads("Fresh", smart, Vec::new(), 0));
    let gone = screen.add(WalletRecord::squads("Gone", missing, vec![a], 1));
    let gone_before = screen.kind(gone);
    screen.settle().await;
    let unread = screen.vault_writes();

    screen.run(|session| refresh_quietly(session, None)).await;
    screen.settle().await;
    assert!(screen.vault_writes() > unread, "what changed is saved");

    let kind = screen.kind(team);
    assert_eq!(
        kind.squad_members(),
        vec![(b, Some(PERMISSION_ALL)), (pk(3), Some(PERMISSION_VOTE))]
    );
    let WalletKind::Squads {
        threshold,
        time_lock,
        rent_collector,
        member_wallet,
        approves_with,
        ..
    } = &kind
    else {
        panic!("a multisig");
    };
    assert_eq!(
        (*threshold, *time_lock, *rent_collector),
        (2, 3_600, Some(pk(9)))
    );
    // A is no longer a member and is still the one chosen: it is shown as
    // such, not replaced behind the person's back.
    assert_eq!(*member_wallet, Some(screen.a));
    assert_eq!(*approves_with, vec![screen.b]);
    // What it would sign says why it cannot.
    let route = screen.act(|| screen.session.route_of(team));
    assert!(route.unwrap_err().to_string().contains("is not a member"));

    // Read for the first time: the local member acts.
    let WalletKind::Squads {
        member_wallet,
        threshold,
        ..
    } = screen.kind(fresh)
    else {
        panic!("a multisig");
    };
    assert_eq!((member_wallet, threshold), (Some(screen.a), 1));

    // Not on this cluster: kept as it was.
    assert_eq!(screen.kind(gone), gone_before);

    // Read again with nothing new: the vault is not written.
    let writes = screen.vault_writes();
    screen.run(|session| refresh_quietly(session, None)).await;
    screen.settle().await;
    assert_eq!(screen.vault_writes(), writes);
}

#[tokio::test]
async fn a_node_that_does_not_answer_changes_nothing_and_says_so_when_asked() {
    let squad = Squad::v4(&pk(40));
    let down = FakeRpc {
        offline: true,
        ..FakeRpc::default()
    };
    let mut screen = Screen::new(down).await;
    let id = screen.add(WalletRecord::squads("Team", squad, vec![pk(1)], 1));
    let before = screen.kind(id);
    let writes = screen.vault_writes();
    screen.run(|session| refresh_quietly(session, None)).await;
    screen.settle().await;
    assert_eq!(screen.kind(id), before);
    assert_eq!(screen.vault_writes(), writes);

    let said = screen
        .run(|session| refresh_by_hand(session, id))
        .await
        .unwrap_err();
    assert!(said.contains("did not answer"), "{said}");
    assert_eq!(screen.kind(id), before);
}

#[tokio::test]
async fn a_refresh_by_hand_says_what_changed_or_that_nothing_did() {
    let a = key_a();
    let squad = Squad::v4(&pk(40));
    let elsewhere = Squad::v4(&pk(41));
    let rpc = FakeRpc::default().with_account(
        squad.address,
        multisig_account(
            SquadsProgram::V4,
            &on_chain(&[(a, PERMISSION_ALL), (pk(3), PERMISSION_ALL)], 2),
        ),
    );
    let mut screen = Screen::new(rpc).await;
    let id = screen.add(WalletRecord::squads("Team", squad, vec![a], 1));
    let other = screen.add(WalletRecord::squads("Other", elsewhere, vec![a], 1));
    screen.settle().await;

    let said = screen
        .run(|session| refresh_by_hand(session, id))
        .await
        .unwrap();
    assert!(
        said.starts_with("The threshold went from 1 to 2."),
        "{said}"
    );
    assert!(said.contains("became a member"), "{said}");
    screen.settle().await;
    let said = screen
        .run(|session| refresh_by_hand(session, id))
        .await
        .unwrap();
    assert_eq!(said, "Read again from Devnet. Nothing changed.");

    let said = screen
        .run(|session| refresh_by_hand(session, other))
        .await
        .unwrap_err();
    assert!(said.starts_with("This multisig is not on Devnet"), "{said}");
}
