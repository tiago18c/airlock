//! A multisig's configuration read again from the real programs, and kept on
//! its wallet record.
//!
//! Each program creates a multisig with a config (settings) authority, a
//! time lock and, on v4, a rent collector. The authority then changes the
//! threshold and adds a member, as a controlled multisig allows without a
//! vote. Reading the account again, as the app does at start and from
//! Details, brings the record up to date with what the program wrote.

use crate::support::node::Node;
use crate::support::{logs, Chain, SOL};
use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;
use std::sync::Mutex;
use wallet_core::squads::{
    Squad, SquadConfig, SquadsMember, SquadsProgram, SquadsSetup, PERMISSION_ALL, PERMISSION_VOTE,
};
use wallet_core::{KeypairSigner, WalletKind, WalletRecord, WalletSigner, SYSTEM_PROGRAM_ID};
use wallet_flows::squads::{look_up_all, Lookup};

const BOTH: [SquadsProgram; 2] = [SquadsProgram::V4, SquadsProgram::SmartAccount];

/// Create a multisig of `members` that needs two, controlled by `authority`.
fn create(
    chain: &mut Chain,
    program: SquadsProgram,
    payer: &KeypairSigner,
    members: &[Pubkey],
    authority: Pubkey,
    rent_collector: Option<Pubkey>,
) -> Squad {
    let setup = SquadsSetup {
        members: members
            .iter()
            .map(|key| SquadsMember {
                key: *key,
                permissions: PERMISSION_ALL,
            })
            .collect(),
        threshold: 2,
        time_lock: 30,
        config_authority: Some(authority),
        rent_collector,
    };
    match program {
        SquadsProgram::V4 => {
            let create_key = KeypairSigner::new_random();
            let (squad, create) = Squad::create_v4(
                &create_key.pubkey(),
                &payer.pubkey(),
                &chain.treasury,
                &setup,
            );
            chain
                .send(&[payer, &create_key], vec![create])
                .unwrap_or_else(|failure| panic!("{}", logs(&failure)));
            squad
        }
        SquadsProgram::SmartAccount => {
            let config = chain.data(&program.program_config());
            let count = SquadsProgram::smart_account_count(&config).unwrap();
            let (squad, create) =
                Squad::create_smart_account(count + 1, &payer.pubkey(), &chain.treasury, &setup);
            chain
                .send(&[payer], vec![create])
                .unwrap_or_else(|failure| panic!("{}", logs(&failure)));
            squad
        }
    }
}

/// An instruction the config authority signs alone: the IDL's discriminator,
/// then `args`, with the authority paying for any room it takes.
fn as_authority(
    squad: &Squad,
    authority: &Pubkey,
    discriminator: &str,
    args: &[u8],
) -> Instruction {
    let program = squad.program.program_id();
    let mut accounts = vec![
        AccountMeta::new(squad.address, false),
        AccountMeta::new_readonly(*authority, true),
        AccountMeta::new(*authority, true),
        AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
    ];
    if squad.program == SquadsProgram::SmartAccount {
        accounts.push(AccountMeta::new_readonly(program, false));
    }
    let mut data: Vec<u8> = (0..discriminator.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&discriminator[at..at + 2], 16).unwrap())
        .collect();
    data.extend_from_slice(args);
    // No memo.
    data.push(0);
    Instruction {
        program_id: program,
        accounts,
        data,
    }
}

fn change_threshold(squad: &Squad, authority: &Pubkey, threshold: u16) -> Instruction {
    let discriminator = match squad.program {
        SquadsProgram::V4 => "8d2a0f7ea95c3eb5",
        SquadsProgram::SmartAccount => "338d4e85462f5f7c",
    };
    as_authority(squad, authority, discriminator, &threshold.to_le_bytes())
}

pub(crate) fn add_member(
    squad: &Squad,
    authority: &Pubkey,
    member: &Pubkey,
    permissions: u8,
) -> Instruction {
    let discriminator = match squad.program {
        SquadsProgram::V4 => "01dbd76cb8e5d608",
        SquadsProgram::SmartAccount => "50c6e49a07ea6338",
    };
    let mut args = member.to_bytes().to_vec();
    args.push(permissions);
    as_authority(squad, authority, discriminator, &args)
}

fn read(node: &Node, squads: &[Squad]) -> Vec<Lookup> {
    pollster::block_on(look_up_all(Some(node), squads))
}

#[test]
fn a_change_the_authority_makes_on_chain_reaches_the_record_when_it_is_read_again() {
    for program in BOTH {
        let mut chain = Chain::new();
        let payer = KeypairSigner::new_random();
        let authority = KeypairSigner::new_random();
        chain.fund(&payer.pubkey(), 10 * SOL);
        chain.fund(&authority.pubkey(), SOL);
        let members: Vec<Pubkey> = (0..3)
            .map(|_| KeypairSigner::new_random().pubkey())
            .collect();
        // The smart account program keeps no rent collector.
        let collector =
            (program == SquadsProgram::V4).then(|| KeypairSigner::new_random().pubkey());
        let squad = create(
            &mut chain,
            program,
            &payer,
            &members,
            authority.pubkey(),
            collector,
        );
        let node = Node(Mutex::new(chain));

        // Added, and read: everything the program wrote is on the record.
        let Lookup::Found(state) = read(&node, &[squad]).remove(0) else {
            panic!("{program:?}: the multisig did not read");
        };
        assert_eq!(
            state.config_authority,
            Some(authority.pubkey()),
            "{program:?}"
        );
        assert_eq!(state.rent_collector, collector, "{program:?}");
        let mut wallet = WalletRecord::squads("Team", squad, Vec::new(), 0);
        assert!(
            wallet
                .kind
                .set_squad_config(&SquadConfig::from(&state))
                .first_read
        );
        assert_eq!(wallet.summary().split(" · ").last(), Some("2 of 3"));

        // The authority raises the threshold and adds a member who may only
        // vote, with no proposal.
        let newcomer = KeypairSigner::new_random().pubkey();
        {
            let mut chain = node.chain();
            let sent = chain.send(
                &[&authority],
                vec![
                    add_member(&squad, &authority.pubkey(), &newcomer, PERMISSION_VOTE),
                    change_threshold(&squad, &authority.pubkey(), 3),
                ],
            );
            if let Err(failure) = sent {
                panic!("{program:?}: {}", logs(&failure));
            }
        }

        // Read again, with another multisig that is not on this chain.
        let missing = Squad::v4(&KeypairSigner::new_random().pubkey());
        let found = read(&node, &[squad, missing]);
        assert_eq!(found[1], Lookup::NotAMultisig, "{program:?}");
        let Lookup::Found(state) = &found[0] else {
            panic!("{program:?}: {:?}", found[0]);
        };
        let changes = wallet.kind.set_squad_config(&SquadConfig::from(state));
        assert_eq!(changes.threshold, Some((2, 3)), "{program:?}");
        assert_eq!(changes.added, vec![newcomer], "{program:?}");
        assert!(changes.removed.is_empty(), "{program:?}");
        let WalletKind::Squads {
            threshold,
            time_lock,
            config_authority,
            ..
        } = &wallet.kind
        else {
            panic!("a multisig");
        };
        assert_eq!(
            (*threshold, *time_lock, *config_authority),
            (3, 30, Some(authority.pubkey())),
            "{program:?}"
        );
        assert!(wallet
            .kind
            .squad_members()
            .contains(&(newcomer, Some(PERMISSION_VOTE))));
        // A config change makes what was proposed before it stale.
        assert!(
            state.stale_transaction_index >= state.transaction_index,
            "{program:?}"
        );
    }
}
