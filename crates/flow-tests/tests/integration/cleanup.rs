//! Burning and closing the token accounts of a wallet Sign rebuilds for: a
//! Squads vault and a programmatic signer. Each step is made by
//! `wallet_flows::cleanup`, rebuilt around the wallet as the Sign screen
//! does, signed and landed. The steps have to leave room for that rebuild.

use crate::support::vector::{run, Desk};
use crate::support::{sign_through, Chain, SOL};
use crate::token_plan::{account_data, mint_data, ACCOUNT_LEN};
use solana_pubkey::Pubkey;
use std::collections::BTreeMap;
use wallet_core::programmatic::create_executor_nonce;
use wallet_core::squads::{Squad, SquadsMember, SquadsProgram, SquadsSetup, PERMISSION_ALL};
use wallet_core::token_plan::{Choice, Held};
use wallet_core::{
    signing_route, spl::TokenAccount, KeypairSigner, WalletId, WalletRecord, WalletSigner,
    TOKEN_PROGRAM_ID,
};
use wallet_flows::cleanup;
use wallet_flows::sign::VectorSigning;

const ACCOUNTS: usize = 30;

/// `ACCOUNTS` token accounts owned by `owner`, every other one holding
/// tokens, each of its own mint.
fn plant_accounts(chain: &mut Chain, owner: &Pubkey) -> Vec<Pubkey> {
    (0..ACCOUNTS)
        .map(|n| {
            let amount = if n % 2 == 0 { 0 } else { 1_000 };
            let mint = KeypairSigner::new_random().pubkey();
            chain.plant(mint, TOKEN_PROGRAM_ID, mint_data(amount));
            let account = KeypairSigner::new_random().pubkey();
            chain.plant(
                account,
                TOKEN_PROGRAM_ID,
                account_data(&mint, owner, amount, false),
            );
            account
        })
        .collect()
}

/// The accounts as `cleanup::read` gives them. The local node does not list
/// token accounts by owner, so they are read one by one.
fn held(chain: &Chain, accounts: &[Pubkey]) -> Vec<Held> {
    accounts
        .iter()
        .filter_map(|address| {
            let raw = chain.account(address)?;
            let account = TokenAccount::parse(&raw)?;
            let mint = chain
                .account(&account.mint)
                .and_then(|mint| wallet_core::spl::Mint::parse(&mint));
            Some(Held {
                address: *address,
                lamports: raw.lamports,
                account,
                mint,
            })
        })
        .collect()
}

/// Run the cleanup of `accounts` for wallet `id` to the end, a step at a
/// time, reading the accounts again after each step lands, as the app does.
/// `signers` sign beside the fee payer. Returns how many steps it took.
fn burn_and_close_all(
    desk: &mut Desk,
    id: WalletId,
    accounts: &[Pubkey],
    signers: &[&KeypairSigner],
) -> usize {
    let route = signing_route(&desk.wallets, id).unwrap();
    let chosen: BTreeMap<Pubkey, Choice> = accounts
        .iter()
        .map(|account| (*account, Choice::Burn))
        .collect();
    let mut done = 0;
    loop {
        let held = held(&desk.chain(), accounts);
        let plan = cleanup::plan(&route, held, &chosen).unwrap();
        if plan.steps.is_empty() {
            return done;
        }
        assert!(done < ACCOUNTS, "the cleanup does not finish");
        let (prepared, _) = run(cleanup::prepare(&desk.node, &route, &plan, 0, done)).unwrap();
        let adapted = desk.rebuild_wire(&prepared.wire);
        let local: Vec<&dyn WalletSigner> = std::iter::once(&desk.payer as &dyn WalletSigner)
            .chain(signers.iter().map(|key| *key as &dyn WalletSigner))
            .collect();
        let mut vector = VectorSigning {
            keys: &[],
            known: &adapted.context,
            cache: None,
            known_is_current: false,
        };
        let signed = sign_through(Some(&desk.node), &adapted.wire, &local, &mut vector).unwrap();
        desk.land(&signed.wire);
        done += 1;
    }
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
        chain.fund(&squad.vault(0), SOL);
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
fn a_squads_vault_burns_and_closes_its_token_accounts_one_step_at_a_time() {
    for program in [SquadsProgram::V4, SquadsProgram::SmartAccount] {
        let mut desk = Desk::new();
        let (squad, id) = squad_of(&mut desk, program);
        let vault = squad.vault(0);
        let accounts = plant_accounts(&mut desk.chain(), &vault);
        let before = desk.chain().balance(&vault);
        let rent = desk.chain().rent(ACCOUNT_LEN);

        let steps = burn_and_close_all(&mut desk, id, &accounts, &[]);
        assert!(steps > 1, "{program:?}: {steps} steps");
        for account in &accounts {
            assert!(
                desk.chain().account(account).is_none(),
                "{program:?}: {account}"
            );
        }
        // The rent comes back to the vault. The member pays the fees.
        assert_eq!(
            desk.chain().balance(&vault),
            before + rent * ACCOUNTS as u64
        );
    }
}

#[test]
fn a_programmatic_signer_burns_and_closes_its_token_accounts_one_step_at_a_time() {
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
    let accounts = plant_accounts(&mut desk.chain(), &address);
    let rent = desk.chain().rent(ACCOUNT_LEN);

    let steps = burn_and_close_all(&mut desk, id, &accounts, &[&authority]);
    assert!(steps > 1, "{steps} steps");
    for account in &accounts {
        assert!(desk.chain().account(account).is_none(), "{account}");
    }
    assert_eq!(desk.chain().balance(&address), rent * ACCOUNTS as u64);
}
