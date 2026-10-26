//! IDLs that ship inside the app, and the programs the build screen pins.
//!
//! Each file in `idls/` is listed once in [`BUILTIN_IDLS`] with the networks
//! it is read on, the document it was made from ([`IdlSource`]) and what was
//! changed in it. A program at different addresses on two networks has a file
//! for each, since the file declares its address and, often, accounts that
//! differ by network too. An address can be listed for one network only,
//! when another network runs a different version.
//!
//! A file made from what a program publishes on chain keeps that document's
//! SHA-256. Checking the program online then tells "the chain still publishes
//! what this file was made from" apart from "the chain publishes something
//! else", although the bytes differ in both cases.
//!
//! A newer file imported by the user replaces the embedded one for that
//! program on that network. Removing the import brings the embedded one back.
//!
//! Some files carry other programs in `additionalPrograms`, and the catalog
//! reads those too: the associated-token program from `token.json` (and
//! `token-2022.json`), the message executor and its nonce program from
//! `ed25519-programmatic-signer.json`. The associated-token entry is fuller
//! than the program's own `idl.json`: its payer, associated account and token
//! program have defaults, so the form derives the address. The Token and
//! Token-2022 entries are the same apart from that token program default, so
//! the Token one, listed first, is the one read.

use crate::idl::catalog::{IdlChainSource, IdlOrigin};
use crate::idl::network::IdlNetwork;
use crate::ids::{
    ASSOCIATED_TOKEN_PROGRAM_ID, COMPUTE_BUDGET_PROGRAM_ID, STAKE_PROGRAM_ID, SYSTEM_PROGRAM_ID,
    TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID,
};
use crate::ids::{SQUADS_SMART_ACCOUNT_PROGRAM_ID, SQUADS_V4_PROGRAM_ID};
use crate::wallets::vector::PROGRAM_ID as VECTOR_PROGRAM_ID;
use solana_pubkey::Pubkey;

pub const ED25519_SIGNER_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("EdSigVfK1DkeMrjFNDMjwfQaJPhPTtX7jW8uPv3oKEgN");

/// Raydium's concentrated-liquidity AMM on devnet.
pub const RAYDIUM_CLMM_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("DRayAUgENGQBKVaX8owNhgzkEDyoHTGVEGHVJT1E9pfH");

/// Raydium's concentrated-liquidity AMM on mainnet.
pub const RAYDIUM_CLMM_MAINNET_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK");

/// Raydium's constant-product AMM on devnet.
pub const RAYDIUM_CP_SWAP_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("DRaycpLY18LhpbydsBWbVJtxpNv9oXPgjRSfpF2bWpYb");

/// Raydium's constant-product AMM on mainnet.
pub const RAYDIUM_CP_SWAP_MAINNET_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C");

/// Raydium's AMM routing program on devnet.
pub const RAYDIUM_ROUTER_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("DRaybByLpbUL57LJARs3j8BitTxVfzBg351EaMr5UTCd");

/// Orca's Whirlpool concentrated-liquidity AMM, on devnet and mainnet.
pub const ORCA_WHIRLPOOL_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc");

/// What an embedded file was made from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdlSource {
    /// The IDL the program publishes on chain.
    Chain(IdlChainSource),
    /// A file in the program's repository. The program publishes no IDL.
    Repository { url: &'static str },
    /// Written by hand: the program publishes no IDL.
    Hand,
}

impl IdlSource {
    pub fn chain(self) -> Option<IdlChainSource> {
        match self {
            Self::Chain(source) => Some(source),
            _ => None,
        }
    }
}

/// One IDL compiled into the binary.
#[derive(Clone, Copy, Debug)]
pub struct BuiltinIdl {
    pub program_id: Pubkey,
    /// `program.name` in the file.
    pub name: &'static str,
    pub json: &'static str,
    /// Where the program runs at `program_id` with this interface.
    pub networks: &'static [IdlNetwork],
    pub source: IdlSource,
    /// What was changed after taking the file from its source.
    pub changes: &'static str,
}

impl BuiltinIdl {
    pub fn runs_on(&self, network: IdlNetwork) -> bool {
        self.networks.contains(&network)
    }
}

const EVERYWHERE: &[IdlNetwork] = &IdlNetwork::ALL;
const MAINNET_AND_DEVNET: &[IdlNetwork] = &[IdlNetwork::Mainnet, IdlNetwork::Devnet];
const MAINNET: &[IdlNetwork] = &[IdlNetwork::Mainnet];
const DEVNET: &[IdlNetwork] = &[IdlNetwork::Devnet];

const fn on_chain(network: IdlNetwork, account: IdlOrigin, sha256: &str) -> IdlSource {
    IdlSource::Chain(IdlChainSource {
        network,
        account,
        sha256: hex32(sha256),
    })
}

/// A SHA-256 written as 64 hex digits. A typo fails the build.
const fn hex32(text: &str) -> [u8; 32] {
    const fn digit(byte: u8) -> u8 {
        match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            _ => panic!("not a lower-case hex digit"),
        }
    }
    let bytes = text.as_bytes();
    assert!(bytes.len() == 64, "a SHA-256 is 64 hex digits");
    let mut out = [0u8; 32];
    let mut i = 0;
    while i < 32 {
        out[i] = (digit(bytes[2 * i]) << 4) | digit(bytes[2 * i + 1]);
        i += 1;
    }
    out
}

const DISPLAY_NODES: &str = "Display nodes added by hand.";

const RAYDIUM_CLMM_CHANGES: &str = "Converted from Anchor to Codama. The swap and liquidity \
    instructions declare the remaining accounts they read (tick arrays, the bitmap extension, \
    a swap route), which Anchor IDLs cannot. Token amounts are in base units: the IDL cannot \
    read a mint's decimals. Display nodes added by hand.";

const RAYDIUM_CP_SWAP_CHANGES: &str = "Converted from Anchor to Codama. `updateAmmConfig` \
    declares the new owner it reads as a remaining account. Token amounts are in base units. \
    Display nodes added by hand.";

/// In this order on a tie between two files carrying the same additional
/// program: the earlier file's entry is read.
pub const BUILTIN_IDLS: [BuiltinIdl; 15] = [
    BuiltinIdl {
        program_id: SYSTEM_PROGRAM_ID,
        name: "system",
        json: include_str!("../../idls/system.json"),
        networks: EVERYWHERE,
        source: IdlSource::Repository {
            url: "https://github.com/solana-program/system",
        },
        changes: DISPLAY_NODES,
    },
    BuiltinIdl {
        program_id: STAKE_PROGRAM_ID,
        name: "stake",
        json: include_str!("../../idls/stake.json"),
        networks: EVERYWHERE,
        source: IdlSource::Repository {
            url: "https://github.com/solana-program/stake",
        },
        changes: DISPLAY_NODES,
    },
    BuiltinIdl {
        program_id: TOKEN_PROGRAM_ID,
        name: "token",
        json: include_str!("../../idls/token.json"),
        networks: EVERYWHERE,
        source: IdlSource::Repository {
            url: "https://github.com/solana-program/token",
        },
        changes: DISPLAY_NODES,
    },
    BuiltinIdl {
        program_id: TOKEN_2022_PROGRAM_ID,
        name: "token2022",
        json: include_str!("../../idls/token-2022.json"),
        networks: EVERYWHERE,
        source: IdlSource::Repository {
            url: "https://github.com/solana-program/token-2022",
        },
        changes: DISPLAY_NODES,
    },
    BuiltinIdl {
        program_id: COMPUTE_BUDGET_PROGRAM_ID,
        name: "computeBudget",
        json: include_str!("../../idls/compute-budget.json"),
        networks: EVERYWHERE,
        source: IdlSource::Repository {
            url: "https://github.com/solana-program/compute-budget",
        },
        changes: DISPLAY_NODES,
    },
    BuiltinIdl {
        program_id: ED25519_SIGNER_PROGRAM_ID,
        name: "ed25519Signer",
        json: include_str!("../../idls/ed25519-programmatic-signer.json"),
        networks: DEVNET,
        source: on_chain(
            IdlNetwork::Devnet,
            IdlOrigin::ProgramMetadata,
            "4f46c00d60e55bc29ee34157d7fb976f5e5dce3701cbc29b765d573b9ae24dde",
        ),
        changes: "The message executor and its nonce program added to `additionalPrograms`. \
                  Display nodes added by hand.",
    },
    BuiltinIdl {
        program_id: SQUADS_V4_PROGRAM_ID,
        name: "squadsMultisigProgram",
        json: include_str!("../../idls/squads-multisig.json"),
        networks: MAINNET_AND_DEVNET,
        // Converted from `sdk/multisig/idl` of github.com/Squads-Protocol/v4,
        // the 2.1.0 IDL devnet publishes. Mainnet still publishes 2.0.0.
        source: on_chain(
            IdlNetwork::Devnet,
            IdlOrigin::Anchor,
            "04168070401256812adb4df65b0a3259f7ae6438c6ebb796e865b9acd3db809f",
        ),
        changes: "Converted from Anchor to Codama. Display nodes added by hand.",
    },
    BuiltinIdl {
        program_id: SQUADS_SMART_ACCOUNT_PROGRAM_ID,
        name: "squadsSmartAccountProgram",
        json: include_str!("../../idls/squads-smart-account.json"),
        networks: MAINNET_AND_DEVNET,
        // Newer than the Anchor IDL either network publishes.
        source: IdlSource::Repository {
            url: "https://github.com/Squads-Protocol/smart-account-program",
        },
        changes: "Converted from Anchor to Codama (`sdk/smart-account/idl`). Display nodes \
                  added by hand.",
    },
    BuiltinIdl {
        program_id: VECTOR_PROGRAM_ID,
        name: "vector",
        json: include_str!("../../idls/vector.json"),
        networks: DEVNET,
        source: on_chain(
            IdlNetwork::Devnet,
            IdlOrigin::ProgramMetadata,
            "98599236e4cf8f0c30aac6645e8ca8adadefd331a8e3acf89c4c66c26481bd35",
        ),
        changes: DISPLAY_NODES,
    },
    BuiltinIdl {
        program_id: RAYDIUM_CLMM_PROGRAM_ID,
        name: "raydiumClmm",
        json: include_str!("../../idls/raydium-clmm.json"),
        networks: DEVNET,
        source: on_chain(
            IdlNetwork::Devnet,
            IdlOrigin::ProgramMetadata,
            "216dbb0ddadfa73e71a8b1819a04543542bd24e2f4c1ceeae0d779c85bfb8403",
        ),
        changes: RAYDIUM_CLMM_CHANGES,
    },
    BuiltinIdl {
        program_id: RAYDIUM_CP_SWAP_PROGRAM_ID,
        name: "raydiumCpSwap",
        json: include_str!("../../idls/raydium-cp-swap.json"),
        networks: DEVNET,
        source: on_chain(
            IdlNetwork::Devnet,
            IdlOrigin::ProgramMetadata,
            "9fa6136d536592d90ebfea5fd16f1db8d33056d9a5a9f153f6e8468400ae781f",
        ),
        changes: RAYDIUM_CP_SWAP_CHANGES,
    },
    BuiltinIdl {
        program_id: RAYDIUM_ROUTER_PROGRAM_ID,
        name: "raydiumRouter",
        json: include_str!("../../idls/raydium-router.json"),
        networks: DEVNET,
        source: IdlSource::Hand,
        changes: "Data layouts and account orders taken from transactions the program \
                  executed. The route's hops are one remaining-accounts group, since their \
                  accounts depend on each pool program.",
    },
    BuiltinIdl {
        program_id: ORCA_WHIRLPOOL_PROGRAM_ID,
        name: "whirlpool",
        json: include_str!("../../idls/orca-whirlpool.json"),
        // Devnet runs the same version, but its IDL accounts are missing or years old.
        networks: MAINNET_AND_DEVNET,
        source: on_chain(
            IdlNetwork::Mainnet,
            IdlOrigin::ProgramMetadata,
            "9c4ad85a942e27f571c4f33453b0b0f2fef1dde5e9c201b301668068e67ace0d",
        ),
        changes: "Converted from Anchor to Codama. The V2 instructions declare the transfer \
                  hook and supplemental tick array accounts they read. Display nodes added by \
                  hand.",
    },
    BuiltinIdl {
        program_id: RAYDIUM_CLMM_MAINNET_PROGRAM_ID,
        name: "raydiumClmm",
        json: include_str!("../../idls/raydium-clmm-mainnet.json"),
        networks: MAINNET,
        // The devnet file with the program and admin addresses of mainnet:
        // they are all the two networks' IDLs differ in.
        source: on_chain(
            IdlNetwork::Mainnet,
            IdlOrigin::ProgramMetadata,
            "862adbd60703fac5a5b397de8130778bd127d944c909b883f831f5cbbf911c8f",
        ),
        changes: RAYDIUM_CLMM_CHANGES,
    },
    BuiltinIdl {
        program_id: RAYDIUM_CP_SWAP_MAINNET_PROGRAM_ID,
        name: "raydiumCpSwap",
        json: include_str!("../../idls/raydium-cp-swap-mainnet.json"),
        networks: MAINNET,
        // The devnet file with the program, admin and fee receiver addresses
        // of mainnet: they are all the two networks' IDLs differ in.
        source: on_chain(
            IdlNetwork::Mainnet,
            IdlOrigin::ProgramMetadata,
            "d86fe11ffc719fee9832f78a1c7374452ee9ebe2c63d51c0b5e5cb383a8fcfdb",
        ),
        changes: RAYDIUM_CP_SWAP_CHANGES,
    },
];

/// The embedded IDL of `program_id` on `network`.
pub fn builtin_idl(network: IdlNetwork, program_id: &Pubkey) -> Option<&'static BuiltinIdl> {
    BUILTIN_IDLS
        .iter()
        .find(|idl| idl.program_id == *program_id && idl.runs_on(network))
}

/// A program with its own tab on the build screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PinnedProgram {
    pub label: &'static str,
    pub program_id: Pubkey,
    /// The instruction a new block starts on.
    pub default_instruction: &'static str,
}

pub const PINNED_PROGRAMS: [PinnedProgram; 5] = [
    PinnedProgram {
        label: "System",
        program_id: SYSTEM_PROGRAM_ID,
        default_instruction: "transferSol",
    },
    PinnedProgram {
        label: "Stake",
        program_id: STAKE_PROGRAM_ID,
        default_instruction: "delegateStake",
    },
    PinnedProgram {
        label: "Token",
        program_id: TOKEN_PROGRAM_ID,
        default_instruction: "transferChecked",
    },
    PinnedProgram {
        label: "Token-2022",
        program_id: TOKEN_2022_PROGRAM_ID,
        default_instruction: "transferChecked",
    },
    PinnedProgram {
        label: "ATA",
        program_id: ASSOCIATED_TOKEN_PROGRAM_ID,
        default_instruction: "createAssociatedTokenIdempotent",
    },
];

pub fn pinned_program(program_id: &Pubkey) -> Option<&'static PinnedProgram> {
    PINNED_PROGRAMS
        .iter()
        .find(|pinned| pinned.program_id == *program_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{
        associated_token_address, create_ata_idempotent, pk, token_transfer_checked, transfer,
    };
    use crate::idl::build::{idl_instruction_form, IdlBuildInput};
    use crate::idl::catalog::{IdlStore, MemoryBodies};
    use crate::idl::draft::IdlDraft;
    use crate::wallets::programmatic::{create_executor_nonce, EXECUTOR_NONCE_PROGRAM_ID};
    use codama_dynamic_client::ProgramClient;
    use codama_nodes::Node;
    use std::sync::Arc;
    use wallet_vault::store::MemoryStore;

    /// Codama's own format, or an Anchor IDL converted to it.
    fn client(idl: &BuiltinIdl) -> ProgramClient {
        if idl.json.contains("\"rootNode\"") {
            return ProgramClient::from_json(idl.json)
                .unwrap_or_else(|err| panic!("{}: {err}", idl.name));
        }
        let root = codama_nodes_from_anchor::root_node_from_anchor(idl.json)
            .unwrap_or_else(|err| panic!("{}: {err}", idl.name));
        ProgramClient::from_root(root).unwrap_or_else(|err| panic!("{}: {err}", idl.name))
    }

    /// Every embedded file of devnet, and the additional programs they carry.
    fn catalog() -> IdlStore<MemoryStore, MemoryBodies> {
        catalog_on(IdlNetwork::Devnet)
    }

    fn catalog_on(network: IdlNetwork) -> IdlStore<MemoryStore, MemoryBodies> {
        IdlStore::open_on(MemoryStore::new(), MemoryBodies::new(), network).with_builtins()
    }

    #[test]
    fn every_embedded_idl_declares_the_listed_program() {
        for idl in &BUILTIN_IDLS {
            let client = client(idl);
            assert_eq!(client.program_address(), idl.program_id, "{}", idl.name);
            assert_eq!(client.root().program.name.to_string(), idl.name);
            assert!(!idl.networks.is_empty(), "{} runs nowhere", idl.name);
        }
    }

    /// One file per program on each network, so which file a network
    /// reads never depends on the order of the list.
    #[test]
    fn no_two_embedded_files_claim_a_program_on_the_same_network() {
        for network in IdlNetwork::ALL {
            let mut seen = std::collections::BTreeSet::new();
            for idl in BUILTIN_IDLS.iter().filter(|idl| idl.runs_on(network)) {
                assert!(
                    seen.insert(idl.program_id),
                    "{} is listed twice on {network}",
                    idl.program_id
                );
            }
        }
    }

    /// An embedded file is never the document it was made from: that would
    /// be a file taken unchanged, which needs no source.
    #[test]
    fn a_source_on_chain_is_not_the_file_itself() {
        for idl in &BUILTIN_IDLS {
            if let Some(source) = idl.source.chain() {
                assert_ne!(
                    source.sha256,
                    crate::idl::catalog::sha256_bytes(idl.json.as_bytes()),
                    "{}",
                    idl.name
                );
                assert!(
                    matches!(
                        source.account,
                        IdlOrigin::ProgramMetadata | IdlOrigin::Anchor
                    ),
                    "{}",
                    idl.name
                );
            }
        }
    }

    #[test]
    fn every_pinned_program_is_in_the_catalog_and_has_its_default_instruction() {
        for network in IdlNetwork::ALL {
            let store = catalog_on(network);
            for pinned in &PINNED_PROGRAMS {
                let client = store
                    .program_client(&pinned.program_id)
                    .unwrap_or_else(|| panic!("{} on {network}", pinned.label));
                assert!(
                    client
                        .instruction_names()
                        .contains(&pinned.default_instruction),
                    "{} has no {}",
                    pinned.label,
                    pinned.default_instruction
                );
            }
        }
    }

    /// The build screen can only offer an instruction it can lay out as a form.
    #[test]
    fn every_instruction_of_every_program_in_the_catalog_has_a_form() {
        let mut failures = Vec::new();
        for network in IdlNetwork::ALL {
            let store = catalog_on(network);
            for record in store.records() {
                let client = store.program_client(&record.program_id).unwrap();
                for name in client.instruction_names() {
                    if let Err(err) = idl_instruction_form(&client, name, &IdlBuildInput::default())
                    {
                        failures.push(format!("{network}: {}.{name}: {err}", record.name));
                    }
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    /// The mainnet Raydium files are the devnet ones with the addresses of
    /// mainnet, which is all the two networks' IDLs differ in.
    #[test]
    fn the_mainnet_raydium_files_differ_from_devnet_by_addresses_alone() {
        let pairs = [
            (
                RAYDIUM_CLMM_PROGRAM_ID,
                RAYDIUM_CLMM_MAINNET_PROGRAM_ID,
                &[(
                    "DRayqG9RXYi8WHgWEmRQGrUWRWbhjYWYkCRJDd6JBBak",
                    "GThUX1Atko4tqhN2NaiTazWSeFWMuiUvfFnyJyUghFMJ",
                )][..],
            ),
            (
                RAYDIUM_CP_SWAP_PROGRAM_ID,
                RAYDIUM_CP_SWAP_MAINNET_PROGRAM_ID,
                &[
                    (
                        "DRayqG9RXYi8WHgWEmRQGrUWRWbhjYWYkCRJDd6JBBak",
                        "GThUX1Atko4tqhN2NaiTazWSeFWMuiUvfFnyJyUghFMJ",
                    ),
                    (
                        "3oE58BKVt8KuYkGxx8zBojugnymWmBiyafWgMrnb6eYy",
                        "DNXgeM9EiiaAbaWvwjHj9fQQLAX5ZsfHyvmYUNRAdNC8",
                    ),
                ][..],
            ),
        ];
        for (devnet, mainnet, swaps) in pairs {
            let mut expected = builtin_idl(IdlNetwork::Devnet, &devnet)
                .unwrap()
                .json
                .replace(&devnet.to_string(), &mainnet.to_string());
            for (from, to) in swaps {
                expected = expected.replace(from, to);
            }
            assert_eq!(
                builtin_idl(IdlNetwork::Mainnet, &mainnet).unwrap().json,
                expected,
                "{mainnet}"
            );
        }
    }

    fn draft(program_id: Pubkey, instruction: &str) -> (Arc<ProgramClient>, IdlDraft) {
        let store = catalog();
        let record = store
            .records()
            .iter()
            .find(|record| record.program_id == program_id)
            .unwrap();
        let client = store.program_client(&program_id).unwrap();
        let draft = IdlDraft::for_instruction(program_id, &record.name, &client, Some(instruction));
        (client, draft)
    }

    fn parsed_name(
        client: &ProgramClient,
        instruction: &solana_instruction::Instruction,
    ) -> String {
        let parsed = client.parse_instruction(instruction).unwrap().unwrap();
        match parsed.path.last() {
            Some(Node::Instruction(node)) => node.name.to_string(),
            _ => String::new(),
        }
    }

    #[test]
    fn the_system_transfer_form_builds_what_the_system_interface_builds() {
        let (client, mut draft) = draft(SYSTEM_PROGRAM_ID, "transferSol");
        draft.set_account("source", Some(pk(1)));
        draft.set_account("destination", Some(pk(2)));
        draft.set_argument("amount", "1500000000".into());
        let built = draft.build(&client).unwrap();
        assert_eq!(built, transfer(pk(1), pk(2), 1_500_000_000));
    }

    #[test]
    fn the_token_transfer_form_builds_the_documented_instruction() {
        for program in [TOKEN_PROGRAM_ID, TOKEN_2022_PROGRAM_ID] {
            let (client, mut draft) = draft(program, "transferChecked");
            draft.set_account("source", Some(pk(1)));
            draft.set_account("mint", Some(pk(2)));
            draft.set_account("destination", Some(pk(3)));
            draft.set_account("authority", Some(pk(4)));
            draft.set_argument("amount", "250".into());
            draft.set_argument("decimals", "6".into());
            let built = draft.build(&client).unwrap();
            assert_eq!(
                built,
                token_transfer_checked(program, pk(1), pk(2), pk(3), pk(4), 250, 6),
                "{program}"
            );
        }
    }

    /// The associated account and the token program have defaults, so the
    /// form asks for the payer, wallet and mint and shows the address.
    #[test]
    fn the_ata_form_derives_the_address_and_builds_the_documented_instruction() {
        let (client, mut draft) = draft(
            ASSOCIATED_TOKEN_PROGRAM_ID,
            "createAssociatedTokenIdempotent",
        );
        draft.set_account("payer", Some(pk(1)));
        draft.set_account("owner", Some(pk(2)));
        draft.set_account("mint", Some(pk(3)));
        let form = idl_instruction_form(&client, &draft.instruction, &draft.input()).unwrap();
        let ata = form
            .accounts
            .iter()
            .find(|field| field.name == "ata")
            .unwrap();
        assert_eq!(
            ata.address,
            Some(associated_token_address(&pk(2), &pk(3), &TOKEN_PROGRAM_ID))
        );
        let built = draft.build(&client).unwrap();
        assert_eq!(
            built,
            create_ata_idempotent(pk(1), pk(2), pk(3), TOKEN_PROGRAM_ID)
        );
    }

    /// The default token program is Token. Changing it derives the
    /// Token-2022 address, and going back to the default undoes that.
    #[test]
    fn the_ata_form_builds_for_token_2022_once_its_token_program_is_changed() {
        let (client, mut draft) = draft(
            ASSOCIATED_TOKEN_PROGRAM_ID,
            "createAssociatedTokenIdempotent",
        );
        draft.set_account("payer", Some(pk(1)));
        draft.set_account("owner", Some(pk(2)));
        draft.set_account("mint", Some(pk(3)));
        draft.set_manual("tokenProgram", true);
        draft.set_account("tokenProgram", Some(TOKEN_2022_PROGRAM_ID));
        assert!(draft.is_manual("tokenProgram"));
        let form = idl_instruction_form(&client, &draft.instruction, &draft.input()).unwrap();
        let ata = form
            .accounts
            .iter()
            .find(|field| field.name == "ata")
            .unwrap();
        assert_eq!(
            ata.address,
            Some(associated_token_address(
                &pk(2),
                &pk(3),
                &TOKEN_2022_PROGRAM_ID
            ))
        );
        assert_eq!(
            draft.build(&client).unwrap(),
            create_ata_idempotent(pk(1), pk(2), pk(3), TOKEN_2022_PROGRAM_ID)
        );

        draft.set_manual("tokenProgram", false);
        assert_eq!(draft.account("tokenProgram"), None);
        assert_eq!(
            draft.build(&client).unwrap(),
            create_ata_idempotent(pk(1), pk(2), pk(3), TOKEN_PROGRAM_ID)
        );
    }

    /// Review reads instructions of an additional program like any other.
    #[test]
    fn instructions_of_additional_programs_are_read_by_name() {
        let store = catalog();
        let ata = store.program_client(&ASSOCIATED_TOKEN_PROGRAM_ID).unwrap();
        for token_program in [TOKEN_PROGRAM_ID, TOKEN_2022_PROGRAM_ID] {
            let create = create_ata_idempotent(pk(1), pk(2), pk(3), token_program);
            assert_eq!(
                parsed_name(&ata, &create),
                "createAssociatedTokenIdempotent"
            );
        }

        let nonce = store.program_client(&EXECUTOR_NONCE_PROGRAM_ID).unwrap();
        let (_, [_, initialize]) =
            create_executor_nonce(&pk(1), &pk(1), "nonce", &pk(2), 1_000_000).unwrap();
        assert_eq!(parsed_name(&nonce, &initialize), "initialize");
    }
}
