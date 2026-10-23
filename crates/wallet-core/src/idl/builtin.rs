//! IDLs that ship inside the app, and the programs the build screen pins.
//!
//! The files in `idls/` are copied unchanged from each program's repository
//! (`idl.json` on `main` of `github.com/solana-program/<name>`). The two
//! Squads files are the Anchor IDLs their programs publish, from
//! `sdk/multisig/idl` of `github.com/Squads-Protocol/v4` and
//! `sdk/smart-account/idl` of `github.com/Squads-Protocol/smart-account-program`,
//! converted to Codama with `codama-nodes-from-anchor` and given display
//! nodes by hand. `vector.json` is the Codama IDL of our Vector fork, with
//! display nodes added by hand too. `raydium-clmm.json` is the Anchor IDL the
//! devnet Raydium CLMM program publishes in its program-metadata account,
//! converted the same way. Its display nodes are added by hand, and so are the
//! remaining accounts its swap and liquidity instructions read (tick arrays,
//! the bitmap extension, a swap route), which Anchor IDLs cannot declare. Its
//! token amounts are in base units: the IDL cannot read a mint's decimals.
//! `raydium-cp-swap.json` is the devnet Raydium CP swap program's IDL from
//! its program-metadata account, converted and given display nodes the same
//! way, with its amounts in base units too. `updateAmmConfig` also declares
//! the new owner it reads as a remaining account.
//! `raydium-router.json` is written by hand: the devnet Raydium routing
//! program publishes no IDL. Its data layouts and account orders are the ones
//! of transactions the program executed, and the route's hops are one
//! remaining-accounts group, since their accounts depend on each pool program.
//! A newer file imported
//! by the user replaces the embedded one for that program. Removing the
//! import brings the embedded one back.
//!
//! Some files carry other programs in `additionalPrograms`, and the catalog
//! reads those too: the associated-token program from `token.json` (and
//! `token-2022.json`), the message executor and its nonce program from
//! `ed25519-programmatic-signer.json`. The associated-token entry is fuller
//! than the program's own `idl.json`: its payer, associated account and token
//! program have defaults, so the form derives the address. The Token and
//! Token-2022 entries are the same apart from that token program default, so
//! the Token one, listed first, is the one read.

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

/// Raydium's constant-product AMM on devnet.
pub const RAYDIUM_CP_SWAP_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("DRaycpLY18LhpbydsBWbVJtxpNv9oXPgjRSfpF2bWpYb");

/// Raydium's AMM routing program on devnet.
pub const RAYDIUM_ROUTER_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("DRaybByLpbUL57LJARs3j8BitTxVfzBg351EaMr5UTCd");

/// One IDL compiled into the binary.
#[derive(Clone, Copy, Debug)]
pub struct BuiltinIdl {
    pub program_id: Pubkey,
    /// `program.name` in the file.
    pub name: &'static str,
    pub json: &'static str,
}

/// In this order on a tie between two files carrying the same additional
/// program: the earlier file's entry is read.
pub const BUILTIN_IDLS: [BuiltinIdl; 12] = [
    BuiltinIdl {
        program_id: SYSTEM_PROGRAM_ID,
        name: "system",
        json: include_str!("../../idls/system.json"),
    },
    BuiltinIdl {
        program_id: STAKE_PROGRAM_ID,
        name: "stake",
        json: include_str!("../../idls/stake.json"),
    },
    BuiltinIdl {
        program_id: TOKEN_PROGRAM_ID,
        name: "token",
        json: include_str!("../../idls/token.json"),
    },
    BuiltinIdl {
        program_id: TOKEN_2022_PROGRAM_ID,
        name: "token2022",
        json: include_str!("../../idls/token-2022.json"),
    },
    BuiltinIdl {
        program_id: COMPUTE_BUDGET_PROGRAM_ID,
        name: "computeBudget",
        json: include_str!("../../idls/compute-budget.json"),
    },
    BuiltinIdl {
        program_id: ED25519_SIGNER_PROGRAM_ID,
        name: "ed25519Signer",
        json: include_str!("../../idls/ed25519-programmatic-signer.json"),
    },
    BuiltinIdl {
        program_id: SQUADS_V4_PROGRAM_ID,
        name: "squadsMultisigProgram",
        json: include_str!("../../idls/squads-multisig.json"),
    },
    BuiltinIdl {
        program_id: SQUADS_SMART_ACCOUNT_PROGRAM_ID,
        name: "squadsSmartAccountProgram",
        json: include_str!("../../idls/squads-smart-account.json"),
    },
    BuiltinIdl {
        program_id: VECTOR_PROGRAM_ID,
        name: "vector",
        json: include_str!("../../idls/vector.json"),
    },
    BuiltinIdl {
        program_id: RAYDIUM_CLMM_PROGRAM_ID,
        name: "raydiumClmm",
        json: include_str!("../../idls/raydium-clmm.json"),
    },
    BuiltinIdl {
        program_id: RAYDIUM_CP_SWAP_PROGRAM_ID,
        name: "raydiumCpSwap",
        json: include_str!("../../idls/raydium-cp-swap.json"),
    },
    BuiltinIdl {
        program_id: RAYDIUM_ROUTER_PROGRAM_ID,
        name: "raydiumRouter",
        json: include_str!("../../idls/raydium-router.json"),
    },
];

pub fn builtin_idl(program_id: &Pubkey) -> Option<&'static BuiltinIdl> {
    BUILTIN_IDLS
        .iter()
        .find(|idl| idl.program_id == *program_id)
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

    /// Every embedded file, and the additional programs they carry.
    fn catalog() -> IdlStore<MemoryStore, MemoryBodies> {
        IdlStore::open(MemoryStore::new(), MemoryBodies::new()).with_builtins()
    }

    #[test]
    fn every_embedded_idl_declares_the_listed_program() {
        for idl in &BUILTIN_IDLS {
            let client = client(idl);
            assert_eq!(client.program_address(), idl.program_id, "{}", idl.name);
            assert_eq!(client.root().program.name.to_string(), idl.name);
        }
    }

    #[test]
    fn every_pinned_program_is_in_the_catalog_and_has_its_default_instruction() {
        let store = catalog();
        for pinned in &PINNED_PROGRAMS {
            let client = store
                .program_client(&pinned.program_id)
                .expect(pinned.label);
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

    /// The build screen can only offer an instruction it can lay out as a form.
    #[test]
    fn every_instruction_of_every_program_in_the_catalog_has_a_form() {
        let store = catalog();
        let mut failures = Vec::new();
        for record in store.records() {
            let client = store.program_client(&record.program_id).unwrap();
            for name in client.instruction_names() {
                if let Err(err) = idl_instruction_form(&client, name, &IdlBuildInput::default()) {
                    failures.push(format!("{}.{name}: {err}", record.name));
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
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
