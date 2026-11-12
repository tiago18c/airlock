//! An argument typed as a link to one of its program's own defined types:
//! an enum whose variants hold nothing is a choice on Build, anything else
//! stays a field nobody can fill, and a link never loops.

use super::scale_tests::sample_text;
use super::*;
use crate::edit::idl_draft_for;
use crate::fixtures::{pk, shipped_catalog};
use crate::idl::draft::IdlDraft;
use crate::{IdlNetwork, TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID};
use codama_nodes::{
    DefinedTypeNode, EnumEmptyVariantTypeNode, EnumStructVariantTypeNode, EnumTypeNode,
    NumberTypeNode, OptionTypeNode, ProgramLinkNode, StructFieldTypeNode, StructTypeNode, U8,
};

fn choice(variants: &[&str]) -> IdlArgKind {
    IdlArgKind::Enum {
        variants: variants.iter().map(|v| v.to_string()).collect(),
    }
}

fn kind_of(client: &ProgramClient, instruction: &str, argument: &str) -> IdlArgKind {
    idl_instruction_form(client, instruction, &IdlBuildInput::default())
        .unwrap()
        .arguments
        .into_iter()
        .find(|field| field.name == argument)
        .unwrap_or_else(|| panic!("no argument {instruction}.{argument}"))
        .kind
}

fn shipped(program_id: Pubkey) -> std::sync::Arc<ProgramClient> {
    shipped_catalog(IdlNetwork::Devnet)
        .program_client(&program_id)
        .unwrap()
}

#[test]
fn an_enum_behind_a_link_is_a_choice() {
    let token_2022 = shipped(TOKEN_2022_PROGRAM_ID);
    assert_eq!(
        kind_of(&token_2022, "initializeDefaultAccountState", "state"),
        choice(&["uninitialized", "initialized", "frozen"])
    );
    let token = shipped(TOKEN_PROGRAM_ID);
    assert_eq!(
        kind_of(&token, "setAuthority", "authorityType"),
        choice(&[
            "mintTokens",
            "freezeAccount",
            "accountOwner",
            "closeAccount"
        ])
    );
}

/// Token-2022's `DefaultAccountStateInstruction::Initialize`: the extension's
/// tag 28, its own 0, then the state as the program numbers it.
#[test]
fn each_default_account_state_builds_the_bytes_the_program_reads() {
    let client = shipped(TOKEN_2022_PROGRAM_ID);
    let mint = pk(3);
    for (index, state) in ["uninitialized", "initialized", "frozen"]
        .into_iter()
        .enumerate()
    {
        let mut draft = IdlDraft::for_instruction(
            TOKEN_2022_PROGRAM_ID,
            "token2022",
            &client,
            Some("initializeDefaultAccountState"),
        );
        draft.set_account("mint", Some(mint));
        draft.set_argument("state", state.to_string());
        let built = draft.build(&client).unwrap();
        assert_eq!(built.data, vec![28, 0, index as u8], "{state}");
        assert_eq!(built.accounts.len(), 1);
        assert_eq!(built.accounts[0].pubkey, mint);

        let opened = idl_draft_for(&client, "token2022", &built).unwrap();
        assert_eq!(opened.argument("state"), state);
        assert_eq!(opened.clone().build(&client).unwrap(), built);
    }
}

#[test]
fn a_variant_the_enum_does_not_have_is_refused() {
    let client = shipped(TOKEN_2022_PROGRAM_ID);
    let mut draft = IdlDraft::for_instruction(
        TOKEN_2022_PROGRAM_ID,
        "token2022",
        &client,
        Some("initializeDefaultAccountState"),
    );
    draft.set_account("mint", Some(pk(3)));
    draft.set_argument("state", "thawed".to_string());
    assert!(draft.build(&client).is_err());
}

/// The arguments of `client` whose type is a link, or an option of one, that
/// now reads as a choice.
fn linked_choices(client: &ProgramClient) -> Vec<(String, String)> {
    let program = &client.root().program;
    let mut found = Vec::new();
    for instruction in &program.instructions {
        for argument in &instruction.arguments {
            let Some(field) = argument_field(program, instruction, argument) else {
                continue;
            };
            let was = classify(&argument.r#type);
            if was != field.kind && enum_inside(&field.kind) {
                found.push((instruction.name.to_string(), argument.name.to_string()));
            }
        }
    }
    found
}

fn enum_inside(kind: &IdlArgKind) -> bool {
    match kind {
        IdlArgKind::Enum { .. } => true,
        IdlArgKind::Option(inner) => enum_inside(inner),
        _ => false,
    }
}

fn raw_inside(kind: &IdlArgKind) -> bool {
    match kind {
        IdlArgKind::Raw { .. } => true,
        IdlArgKind::Option(inner) => raw_inside(inner),
        _ => false,
    }
}

/// Builds `instruction` with every field filled and each linked choice set
/// to each of its variants in turn, opens each on Build and builds it again.
fn round_trip_choices(
    client: &ProgramClient,
    program: &str,
    instruction: &str,
    choices: &[&str],
) -> Result<usize, String> {
    let form = idl_instruction_form(client, instruction, &IdlBuildInput::default())?;
    let sources: Vec<&str> = form
        .arguments
        .iter()
        .filter_map(|field| match &field.scale {
            IdlArgScale::FromArgument { argument, .. } => Some(argument.as_str()),
            _ => None,
        })
        .collect();
    let mut base =
        IdlDraft::for_instruction(client.program_address(), program, client, Some(instruction));
    for (i, account) in form.accounts.iter().enumerate() {
        if account.source != IdlAccountSource::Derived {
            base.set_account(&account.name, Some(pk(100 + i as u8)));
        }
    }
    for field in &form.arguments {
        let source = sources.contains(&field.name.as_str());
        let text = sample_text(&field.kind, &field.scale, source)
            .ok_or_else(|| format!("{} has no widget", field.name))?;
        base.set_argument(&field.name, text);
    }
    let mut checked = 0;
    for field in form
        .arguments
        .iter()
        .filter(|field| choices.contains(&field.name.as_str()))
    {
        let variants = match &field.kind {
            IdlArgKind::Enum { variants } => variants.clone(),
            IdlArgKind::Option(inner) => match inner.as_ref() {
                IdlArgKind::Enum { variants } => variants.clone(),
                _ => continue,
            },
            _ => continue,
        };
        for variant in variants {
            let mut draft = base.clone();
            draft.set_argument(&field.name, variant.clone());
            let mut built = draft.build(client);
            for _ in 0..4 {
                match &built {
                    Err(IdlBuildFailure::FillAddress { account, .. }) => {
                        draft.set_account(&account.clone(), Some(pk(90)));
                        built = draft.build(client);
                    }
                    _ => break,
                }
            }
            let built = built.map_err(|err| format!("{variant}: does not build: {err}"))?;
            let opened = idl_draft_for(client, program, &built)
                .ok_or_else(|| format!("{variant}: does not open on Build"))?;
            if opened.argument(&field.name) != variant {
                return Err(format!(
                    "{} chose `{variant}`, opens as `{}`",
                    field.name,
                    opened.argument(&field.name)
                ));
            }
            let again = opened
                .clone()
                .build(client)
                .map_err(|err| format!("{variant}: opened, does not build: {err}"))?;
            if again != built {
                return Err(format!("{variant}: opened, builds other bytes"));
            }
            checked += 1;
        }
    }
    Ok(checked)
}

/// Every argument of the shipped IDLs that links to a plain enum of its own
/// program is a choice, on every network, and each of its variants builds,
/// opens on Build with that variant, and builds the same bytes again.
#[test]
fn every_linked_choice_of_the_shipped_idls_opens_as_chosen_and_builds_the_same() {
    let mut fields = std::collections::BTreeSet::new();
    let mut variants = 0;
    let mut failures = Vec::new();
    let mut skipped = Vec::new();
    for network in IdlNetwork::ALL {
        let store = shipped_catalog(network);
        for record in store.records() {
            let client = store.program_client(&record.program_id).unwrap();
            let found = linked_choices(&client);
            let mut by_instruction: std::collections::BTreeMap<&str, Vec<&str>> =
                Default::default();
            for (instruction, argument) in &found {
                fields.insert(format!("{}.{instruction}.{argument}", record.name));
                by_instruction
                    .entry(instruction.as_str())
                    .or_default()
                    .push(argument.as_str());
            }
            for (instruction, choices) in by_instruction {
                let form =
                    idl_instruction_form(&client, instruction, &IdlBuildInput::default()).unwrap();
                // Another field Build has no widget for keeps it off Build.
                if form.arguments.iter().any(|field| raw_inside(&field.kind)) {
                    skipped.push(format!("{}.{instruction}", record.name));
                    continue;
                }
                match round_trip_choices(&client, &record.name, instruction, &choices) {
                    Ok(count) => variants += count,
                    Err(err) => {
                        failures.push(format!("{network}: {}.{instruction}: {err}", record.name))
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert!(
        fields.contains("token2022.initializeDefaultAccountState.state"),
        "{fields:?}"
    );
    assert!(fields.contains("token.setAuthority.authorityType"));
    assert!(
        fields.len() >= 10,
        "only {} linked choices: {fields:?}",
        fields.len()
    );
    assert!(variants >= 30, "only {variants} variants built");
    // An instruction is skipped when another of its fields has no widget on
    // Build, so its choices are not checked here. Each one is listed, counted
    // once whatever the number of networks it is read on: one more is then a
    // decision made with the IDL that brings it, and one fewer is a widget
    // Build gained.
    const SKIPPED: [&str; 15] = [
        "bankineco.executeTrancheDepositFeeExempt",
        "bankineco.executeTrancheWithdrawFeeExempt",
        "bubblegum.mintV2",
        "bubblegum.updateAssetDataV2",
        "kaminoLending.idlMissingTypes",
        "programMetadata.initialize",
        "raydiumLaunchpad.initializeV2",
        "raydiumLaunchpad.initializeWithToken2022",
        "tokenMetadata.updateAsAuthorityItemDelegateV2",
        "tokenMetadata.updateAsUpdateAuthorityV2",
        "vault.initializeStrategy",
        "vaults.liquidateDex",
        "vaults.liquidatePerfectDex",
        "vaults.operateDex",
        "vaults.operatePerfectDex",
    ];
    skipped.sort();
    skipped.dedup();
    assert_eq!(skipped, SKIPPED);
}

// What an author can write: a program whose types the form reads by hand,
// with no client in between, so a type the client would refuse still
// reaches the form.

fn plain_enum() -> TypeNode {
    EnumTypeNode::new(vec![
        EnumEmptyVariantTypeNode::new("on").into(),
        EnumEmptyVariantTypeNode::new("off").into(),
    ])
    .into()
}

fn link(name: &str) -> TypeNode {
    DefinedTypeLinkNode::new(name).into()
}

fn program_with(types: Vec<(&str, TypeNode)>) -> ProgramNode {
    let mut program = ProgramNode::new("evil", "4vJ9JU1bJJE96FWSJKvHsmmFADCg4gpZQff4P3bkLKi");
    program.defined_types = types
        .into_iter()
        .map(|(name, ty)| DefinedTypeNode::new(name, ty))
        .collect();
    program
}

fn raw(note: &str) -> IdlArgKind {
    IdlArgKind::Raw {
        note: note.to_string(),
    }
}

#[test]
fn a_link_is_followed_one_link_further_and_no_more() {
    let program = program_with(vec![
        ("switch", plain_enum()),
        ("alias", link("switch")),
        ("aliasOfAlias", link("alias")),
    ]);
    let on_off = choice(&["on", "off"]);
    assert_eq!(classify_in(&program, &link("switch")), on_off);
    assert_eq!(classify_in(&program, &link("alias")), on_off);
    assert_eq!(
        classify_in(&program, &link("aliasOfAlias")),
        raw("definedTypeLinkNode")
    );
    // An optional one is an optional choice.
    assert_eq!(
        classify_in(&program, &OptionTypeNode::new(link("switch")).into()),
        IdlArgKind::Option(Box::new(on_off))
    );
    // Seeds and scales do not follow links.
    assert_eq!(classify(&link("switch")), raw("definedTypeLinkNode"));
}

#[test]
fn a_link_to_anything_but_a_plain_enum_of_this_program_is_not_filled() {
    let mut elsewhere = DefinedTypeLinkNode::new("switch");
    elsewhere.program = Some(ProgramLinkNode::new("other"));
    let mut here = DefinedTypeLinkNode::new("switch");
    here.program = Some(ProgramLinkNode::new("evil"));
    let program = program_with(vec![
        ("switch", plain_enum()),
        ("number", NumberTypeNode::le(U8).into()),
        (
            "record",
            StructTypeNode::new(vec![StructFieldTypeNode::new("x", NumberTypeNode::le(U8))]).into(),
        ),
        (
            "withFields",
            EnumTypeNode::new(vec![
                EnumEmptyVariantTypeNode::new("none").into(),
                EnumStructVariantTypeNode::new(
                    "some",
                    StructTypeNode::new(vec![StructFieldTypeNode::new(
                        "x",
                        NumberTypeNode::le(U8),
                    )]),
                )
                .into(),
            ])
            .into(),
        ),
    ]);
    let unread = raw("definedTypeLinkNode");
    assert_eq!(classify_in(&program, &here.into()), choice(&["on", "off"]));
    assert_eq!(classify_in(&program, &elsewhere.into()), unread);
    assert_eq!(classify_in(&program, &link("missing")), unread);
    assert_eq!(classify_in(&program, &link("number")), unread);
    assert_eq!(classify_in(&program, &link("record")), unread);
    assert_eq!(classify_in(&program, &link("withFields")), unread);
}

#[test]
fn types_that_link_to_themselves_end_unfilled() {
    let program = program_with(vec![
        ("loop", link("loop")),
        ("ping", link("pong")),
        ("pong", link("ping")),
        ("maybeLoop", OptionTypeNode::new(link("maybeLoop")).into()),
    ]);
    let unread = raw("definedTypeLinkNode");
    assert_eq!(classify_in(&program, &link("loop")), unread);
    assert_eq!(classify_in(&program, &link("ping")), unread);
    assert_eq!(classify_in(&program, &link("maybeLoop")), unread);
}
