//! Options at the end of an instruction that the program reads only if
//! bytes are left (Codama's `remainderOptionTypeNode`). None is written as
//! nothing, so a later one filled while an earlier one is blank would be
//! read as the earlier one: the build refuses it and names the field to
//! fill first.

use super::scale_tests::sample_text;
use super::*;
use crate::edit::idl_draft_for;
use crate::fixtures::{pk, shipped_catalog};
use crate::idl::draft::IdlDraft;
use crate::IdlNetwork;

const PUMP: Pubkey = Pubkey::from_str_const("6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P");
const RAYDIUM_AMM_V4: Pubkey =
    Pubkey::from_str_const("675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8");

/// A draft of `instruction` with every account filled and every argument
/// that is not a trailing option given a sample value.
fn filled(client: &ProgramClient, instruction: &str) -> (IdlForm, IdlDraft) {
    try_filled(client, instruction).unwrap_or_else(|field| panic!("{field} has no widget"))
}

/// As [`filled`], or the first argument Build has no widget for.
fn try_filled(client: &ProgramClient, instruction: &str) -> Result<(IdlForm, IdlDraft), String> {
    let form = idl_instruction_form(client, instruction, &IdlBuildInput::default()).unwrap();
    let sources: Vec<&str> = form
        .arguments
        .iter()
        .filter_map(|field| match &field.scale {
            IdlArgScale::FromArgument { argument, .. } => Some(argument.as_str()),
            _ => None,
        })
        .collect();
    let mut draft =
        IdlDraft::for_instruction(client.program_address(), "p", client, Some(instruction));
    for (i, account) in form.accounts.iter().enumerate() {
        if account.source != IdlAccountSource::Derived {
            draft.set_account(&account.name, Some(pk(100 + i as u8)));
        }
    }
    for field in form
        .arguments
        .iter()
        .filter(|field| !field.remainder_option)
    {
        let source = sources.contains(&field.name.as_str());
        let text = sample_text(&field.kind, &field.scale, source)
            .ok_or_else(|| format!("{instruction}.{}", field.name))?;
        draft.set_argument(&field.name, text);
    }
    Ok((form, draft))
}

/// Builds, giving every account the encoder asks for.
fn build(client: &ProgramClient, draft: &mut IdlDraft) -> Result<Instruction, IdlBuildFailure> {
    let mut built = draft.build(client);
    for _ in 0..8 {
        match &built {
            Err(IdlBuildFailure::FillAddress { account, .. }) => {
                draft.set_account(&account.clone(), Some(pk(90)));
                built = draft.build(client);
            }
            _ => break,
        }
    }
    built
}

fn sample(field: &IdlArgField) -> String {
    sample_text(&field.kind, &field.scale, false).unwrap()
}

fn refused(client: &ProgramClient, draft: &mut IdlDraft) -> String {
    match build(client, draft) {
        Ok(built) => panic!("built {:?}", built.data),
        Err(err) => err.to_string(),
    }
}

/// Builds `draft`, opens what it built on Build, and checks the opened
/// draft holds the same trailing options and builds the same bytes.
fn round_trips(client: &ProgramClient, form: &IdlForm, draft: &mut IdlDraft) {
    let built = build(client, draft).unwrap();
    let opened = idl_draft_for(client, "p", &built).expect("opens on Build");
    for field in form.arguments.iter().filter(|field| field.remainder_option) {
        assert_eq!(
            opened.argument(&field.name).trim(),
            draft.argument(&field.name).trim(),
            "{}",
            field.name
        );
    }
    let mut again = opened.clone();
    assert_eq!(build(client, &mut again).unwrap().data, built.data);
}

#[test]
fn pump_create_v2_refuses_a_holder_reward_with_the_options_before_it_blank() {
    let store = shipped_catalog(IdlNetwork::Mainnet);
    let client = store.program_client(&PUMP).unwrap();
    let (form, mut draft) = filled(&client, "createV2");
    let trailing: Vec<&str> = form
        .arguments
        .iter()
        .filter(|field| field.remainder_option)
        .map(|field| field.name.as_str())
        .collect();
    assert_eq!(
        trailing,
        ["isCashbackEnabled", "creatorFeeBps", "isHolderReward"]
    );

    // Before, this built and read back as isCashbackEnabled = true.
    draft.set_argument("isHolderReward", "true".into());
    let err = refused(&client, &mut draft);
    assert_eq!(
        err,
        trailing_option_message("isCashbackEnabled", "isHolderReward"),
    );
    assert!(err.starts_with("Fill isCashbackEnabled first"), "{err}");

    // Before, this failed with "unexpected end of input".
    let (_, mut draft) = filled(&client, "createV2");
    draft.set_argument("creatorFeeBps", "5".into());
    assert!(refused(&client, &mut draft).starts_with("Fill isCashbackEnabled first"));

    // With the one before filled, the next blank one is named.
    draft.set_argument("isCashbackEnabled", "false".into());
    draft.set_argument("creatorFeeBps", String::new());
    draft.set_argument("isHolderReward", "true".into());
    assert!(refused(&client, &mut draft).starts_with("Fill creatorFeeBps first"));

    // In order, it builds and reopens with each value where it was typed.
    draft.set_argument("creatorFeeBps", "5".into());
    round_trips(&client, &form, &mut draft);
}

#[test]
fn pump_buy_refuses_a_partial_fill_with_track_volume_blank() {
    let store = shipped_catalog(IdlNetwork::Mainnet);
    let client = store.program_client(&PUMP).unwrap();
    let (form, mut draft) = filled(&client, "buy");
    // Before, this built and read back as trackVolume = true.
    draft.set_argument("partialFill", "true".into());
    assert_eq!(
        refused(&client, &mut draft),
        trailing_option_message("trackVolume", "partialFill")
    );
    // Neither, the first alone, and both all build and reopen as typed.
    for (track, partial) in [("", ""), ("true", ""), ("false", "true")] {
        draft.set_argument("trackVolume", track.into());
        draft.set_argument("partialFill", partial.into());
        round_trips(&client, &form, &mut draft);
    }
}

#[test]
fn raydium_amm_v4_withdraw_refuses_a_floor_on_the_second_token_alone() {
    let store = shipped_catalog(IdlNetwork::Mainnet);
    let client = store.program_client(&RAYDIUM_AMM_V4).unwrap();
    let (form, mut draft) = filled(&client, "withdraw");
    let pc = form
        .arguments
        .iter()
        .find(|field| field.name == "minPcAmount")
        .unwrap();
    // A floor typed for the second token would land on the first.
    draft.set_argument("minPcAmount", sample(pc));
    assert_eq!(
        refused(&client, &mut draft),
        trailing_option_message("minCoinAmount", "minPcAmount")
    );
    let coin = form
        .arguments
        .iter()
        .find(|field| field.name == "minCoinAmount")
        .unwrap();
    draft.set_argument("minCoinAmount", sample(coin));
    round_trips(&client, &form, &mut draft);
}

/// Every instruction of the shipped IDLs, on every network, that ends in
/// two or more trailing options refuses each later one filled with an
/// earlier one blank, naming the first blank one, and builds and reopens
/// with all of them filled.
#[test]
fn every_shipped_instruction_with_trailing_options_refuses_them_out_of_order() {
    let mut seen = std::collections::BTreeSet::new();
    let mut skipped = std::collections::BTreeSet::new();
    let mut failures = Vec::new();
    for network in IdlNetwork::ALL {
        let store = shipped_catalog(network);
        for record in store.records() {
            let client = store.program_client(&record.program_id).unwrap();
            for instruction in client.instruction_names() {
                let Ok(form) =
                    idl_instruction_form(&client, instruction, &IdlBuildInput::default())
                else {
                    continue;
                };
                let trailing: Vec<&IdlArgField> = form
                    .arguments
                    .iter()
                    .filter(|field| field.remainder_option)
                    .collect();
                if trailing.len() < 2 {
                    continue;
                }
                let name = format!("{}.{instruction}", record.name);
                // Another argument Build has no widget for keeps it off
                // Build altogether.
                let Ok((_, base)) = try_filled(&client, instruction) else {
                    skipped.insert(name);
                    continue;
                };
                seen.insert(name.clone());
                // Those before `blank` filled, `blank` up to `later` blank,
                // and `later` filled.
                for (later, field) in trailing.iter().enumerate().skip(1) {
                    for blank in 0..later {
                        let mut draft = base.clone();
                        for (i, other) in trailing.iter().enumerate() {
                            if i < blank || i == later {
                                draft.set_argument(&other.name, sample(other));
                            }
                        }
                        let first_blank = trailing[blank].name.as_str();
                        let want = trailing_option_message(first_blank, &field.name);
                        match build(&client, &mut draft) {
                            Err(IdlBuildFailure::Message(got)) if got == want => {}
                            other => failures.push(format!(
                                "{network}: {name}: {} blank, {} filled: {other:?}",
                                first_blank, field.name
                            )),
                        }
                    }
                }
                let mut all = base.clone();
                for field in &trailing {
                    all.set_argument(&field.name, sample(field));
                }
                if let Err(err) = build(&client, &mut all) {
                    failures.push(format!("{network}: {name}: all filled: {err}"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    eprintln!("checked {seen:?}\nno widget {skipped:?}");
    for name in [
        "pump.buy",
        "pump.buyExactSolIn",
        "pump.createV2",
        "raydiumAmm.withdraw",
        "hadron.initialize",
        "manifest.defrag",
    ] {
        assert!(seen.contains(name), "{name} not checked: {seen:?}");
    }
    // Each instruction left out is listed: one more is a decision made
    // with the IDL that brings it, one fewer a widget Build gained.
    assert_eq!(
        skipped.into_iter().collect::<Vec<_>>(),
        ["pumpAmm.createPool"]
    );
}
