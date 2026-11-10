//! Moving everything a wallet holds to another address, as
//! `wallet_flows::migrate` plans and builds it, against the real programs.
//!
//! The wallet holds tokens, an empty token account, wrapped SOL, a stake
//! account and a nonce account. Each step is built, rebuilt around the wallet
//! when Sign would, signed and landed, then the wallet is read again and
//! the next step built, until nothing is left. A hot wallet that pays for
//! itself must end with no account at all: the sweep's amount is exact.

use crate::support::vector::{run, Desk};
use crate::support::{sign_through, Chain, SOL};
use crate::token_plan::{account_data, mint_data, ACCOUNT_LEN};
use solana_pubkey::Pubkey;
use std::collections::BTreeSet;
use wallet_core::migrate::{NonceHeld, Now};
use wallet_core::programmatic::create_executor_nonce;
use wallet_core::spl::{associated_token_address, Mint, TokenAccount};
use wallet_core::squads::{Squad, SquadsMember, SquadsProgram, SquadsSetup, PERMISSION_ALL};
use wallet_core::stake::StakeAccount;
use wallet_core::token_plan::Held;
use wallet_core::{
    signing_route, KeypairSigner, WalletId, WalletRecord, WalletSigner, NATIVE_MINT,
    SYSTEM_PROGRAM_ID, TOKEN_PROGRAM_ID,
};
use wallet_flows::migrate;
use wallet_flows::sign::VectorSigning;
use wallet_flows::testing::{nonce_account, stake_account};

const NOW: Now = Now {
    epoch: 0,
    unix_time: 0,
};

/// What the wallet at `owner` is given to move.
struct Holdings {
    tokens: Vec<Pubkey>,
    /// Mint of the account with tokens in it, and how many.
    full: (Pubkey, u64),
    stake: Pubkey,
    nonce: Pubkey,
}

fn give(chain: &mut Chain, owner: &Pubkey) -> Holdings {
    let mut tokens = Vec::new();
    let full_mint = KeypairSigner::new_random().pubkey();
    chain.plant(full_mint, TOKEN_PROGRAM_ID, mint_data(700));
    for (mint, amount) in [(full_mint, 700), (KeypairSigner::new_random().pubkey(), 0)] {
        if amount == 0 {
            chain.plant(mint, TOKEN_PROGRAM_ID, mint_data(0));
        }
        let account = KeypairSigner::new_random().pubkey();
        chain.plant(
            account,
            TOKEN_PROGRAM_ID,
            account_data(&mint, owner, amount, false),
        );
        tokens.push(account);
    }
    // Wrapped SOL: half a SOL above the rent, which `is_native` records.
    let rent = chain.rent(ACCOUNT_LEN);
    let mut wrapped = account_data(&NATIVE_MINT, owner, SOL / 2, false);
    wrapped[109] = 1;
    wrapped[113..121].copy_from_slice(&rent.to_le_bytes());
    let account = KeypairSigner::new_random().pubkey();
    chain.plant(account, TOKEN_PROGRAM_ID, wrapped);
    chain.fund(&account, SOL / 2);
    tokens.push(account);

    let stake = KeypairSigner::new_random().pubkey();
    let raw = stake_account(*owner, *owner, 2);
    chain.plant(stake, wallet_core::STAKE_PROGRAM_ID, raw.data);
    let nonce = KeypairSigner::new_random().pubkey();
    chain.plant(nonce, SYSTEM_PROGRAM_ID, nonce_account(*owner).0.data);
    Holdings {
        tokens,
        full: (full_mint, 700),
        stake,
        nonce,
    }
}

/// What `migrate::read` would find. The local node lists neither token
/// accounts by owner nor program accounts, so they are read one by one.
fn snapshot(desk: &Desk, id: WalletId, to: Pubkey, holdings: &Holdings) -> migrate::Snapshot {
    let route = signing_route(&desk.wallets, id).unwrap();
    let tokens: Vec<Held> = {
        let chain = desk.chain();
        holdings
            .tokens
            .iter()
            .filter_map(|address| {
                let raw = chain.account(address)?;
                let account = TokenAccount::parse(&raw)?;
                let mint = chain.account(&account.mint).and_then(|m| Mint::parse(&m));
                Some(Held {
                    address: *address,
                    lamports: raw.lamports,
                    account,
                    mint,
                })
            })
            .collect()
    };
    let mut snapshot = run(migrate::snapshot_with(&desk.node, &route, to, tokens, NOW)).unwrap();
    let chain = desk.chain();
    snapshot.stakes = chain
        .account(&holdings.stake)
        .and_then(|raw| StakeAccount::parse(holdings.stake, &raw))
        .filter(|stake| stake.staker == route.address || stake.withdrawer == route.address)
        .into_iter()
        .collect();
    snapshot.nonces = chain
        .account(&holdings.nonce)
        .map(|raw| NonceHeld {
            address: holdings.nonce,
            lamports: raw.lamports,
        })
        .into_iter()
        .collect();
    snapshot
}

/// Run the move of wallet `id` to `to` to the end. `rebuild` when Sign would
/// wrap each step; `signers` sign beside the fee payer. Returns the steps.
fn move_everything(
    desk: &mut Desk,
    id: WalletId,
    to: Pubkey,
    holdings: &Holdings,
    rebuild: bool,
    signers: &[&KeypairSigner],
) -> usize {
    let route = signing_route(&desk.wallets, id).unwrap();
    let mut done = 0;
    loop {
        let snap = snapshot(desk, id, to, holdings);
        let choices = migrate::default_choices(&route, &snap, &BTreeSet::new());
        let plan = migrate::plan(&route, &snap, &choices).unwrap();
        let Some((prepared, _)) =
            run(migrate::prepare(&desk.node, &route, &snap, &plan, done)).unwrap()
        else {
            return done;
        };
        assert!(done < 20, "the move does not finish");
        let (wire, context) = if rebuild {
            let adapted = desk.rebuild_wire(&prepared.wire);
            (adapted.wire, adapted.context)
        } else {
            (prepared.wire, Vec::new())
        };
        let local: Vec<&dyn WalletSigner> = std::iter::once(&desk.payer as &dyn WalletSigner)
            .chain(signers.iter().map(|key| *key as &dyn WalletSigner))
            .collect();
        let mut vector = VectorSigning {
            keys: &[],
            known: &context,
            cache: None,
            known_is_current: false,
        };
        let signed = sign_through(Some(&desk.node), &wire, &local, &mut vector)
            .unwrap_or_else(|err| panic!("step {}: {err}", done + 1));
        desk.land(&signed.wire);
        done += 1;
    }
}

/// Everything arrived at `to`: the tokens, both stake authorities, and the
/// SOL; and the token and nonce accounts are closed.
fn assert_moved(desk: &Desk, to: Pubkey, holdings: &Holdings) {
    let chain = desk.chain();
    for account in &holdings.tokens {
        assert!(chain.account(account).is_none(), "{account} closed");
    }
    let (mint, amount) = holdings.full;
    let received = associated_token_address(&to, &mint, &TOKEN_PROGRAM_ID);
    let received = TokenAccount::parse(&chain.account(&received).unwrap()).unwrap();
    assert_eq!((received.owner, received.amount), (to, amount));
    let stake =
        StakeAccount::parse(holdings.stake, &chain.account(&holdings.stake).unwrap()).unwrap();
    assert_eq!((stake.staker, stake.withdrawer), (to, to));
    assert!(chain.account(&holdings.nonce).is_none(), "nonce closed");
}

#[test]
fn a_hot_wallet_moves_everything_and_is_left_with_no_account() {
    let mut desk = Desk::new();
    let source = desk.payer.pubkey();
    let id = desk.wallets[0].id;
    let to = KeypairSigner::new_random().pubkey();
    let holdings = give(&mut desk.chain(), &source);
    let nonce_rent = desk.chain().balance(&holdings.nonce);

    let steps = move_everything(&mut desk, id, to, &holdings, false, &[]);
    assert_eq!(steps, 2, "the tokens, then everything else");
    assert_moved(&desk, to, &holdings);
    // Exact to the lamport: the wallet's account is gone.
    assert_eq!(desk.chain().balance(&source), 0);
    // It got the SOL, the unwrapped SOL and the nonce's rent, less the fees
    // and the receiving account's rent the wallet paid.
    assert!(desk.chain().balance(&to) > 10 * SOL - SOL / 100 + nonce_rent - SOL / 50);
}

fn squad_of(desk: &mut Desk, program: SquadsProgram) -> (Squad, WalletId) {
    let member = desk.payer.pubkey();
    let setup = SquadsSetup {
        members: vec![SquadsMember {
            key: member,
            permissions: PERMISSION_ALL,
        }],
        threshold: 1,
        time_lock: 0,
    };
    let squad = {
        let mut chain = desk.chain();
        let squad = match program {
            SquadsProgram::V4 => {
                let create_key = KeypairSigner::new_random();
                let (squad, create) =
                    Squad::create_v4(&create_key.pubkey(), &member, &chain.treasury, &setup);
                chain
                    .send(&[&desk.payer, &create_key], vec![create])
                    .unwrap();
                squad
            }
            SquadsProgram::SmartAccount => {
                let config = chain.data(&program.program_config());
                let count = SquadsProgram::smart_account_count(&config).unwrap();
                let (squad, create) =
                    Squad::create_smart_account(count + 1, &member, &chain.treasury, &setup);
                chain.send(&[&desk.payer], vec![create]).unwrap();
                squad
            }
        };
        chain.fund(&squad.vault(0), 2 * SOL);
        squad
    };
    let mut wallet = WalletRecord::squads("Treasury", squad, Vec::new(), 1);
    wallet.kind.set_member_wallet(Some(desk.wallets[0].id));
    wallet.kind.set_fee_payer(Some(desk.wallets[0].id));
    let id = wallet.id;
    desk.wallets.push(wallet);
    (squad, id)
}

#[test]
fn a_squads_vault_moves_everything_through_its_proposals() {
    for program in [SquadsProgram::V4, SquadsProgram::SmartAccount] {
        let mut desk = Desk::new();
        let (squad, id) = squad_of(&mut desk, program);
        let vault = squad.vault(0);
        let to = KeypairSigner::new_random().pubkey();
        let holdings = give(&mut desk.chain(), &vault);

        let steps = move_everything(&mut desk, id, to, &holdings, true, &[]);
        assert!(steps >= 2, "{program:?}: {steps} steps");
        assert_moved(&desk, to, &holdings);
        // Another wallet pays, so the vault sends everything.
        assert_eq!(desk.chain().balance(&vault), 0, "{program:?}");
    }
}

#[test]
fn a_programmatic_signer_moves_everything_through_its_authorizations() {
    let mut desk = Desk::new();
    let authority = KeypairSigner::new_random();
    let mut signer = WalletRecord::programmatic("Signer", authority.pubkey(), None);
    let address = signer.address().unwrap();
    let (nonce, create) = create_executor_nonce(
        &desk.payer.pubkey(),
        &desk.payer.pubkey(),
        "airlock",
        &address,
        desk.chain().nonce_rent(),
    )
    .unwrap();
    desk.chain().send(&[&desk.payer], create.to_vec()).unwrap();
    signer.kind.set_executor_nonce(Some(nonce));
    signer.kind.set_fee_payer(Some(desk.wallets[0].id));
    let id = signer.id;
    desk.wallets.push(signer);
    desk.chain().fund(&address, SOL);
    let to = KeypairSigner::new_random().pubkey();
    let holdings = give(&mut desk.chain(), &address);

    let steps = move_everything(&mut desk, id, to, &holdings, true, &[&authority]);
    assert!(steps >= 2, "{steps} steps");
    assert_moved(&desk, to, &holdings);
    assert_eq!(desk.chain().balance(&address), 0);
}
