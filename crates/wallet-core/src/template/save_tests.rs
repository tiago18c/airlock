//! Making a template from blocks: what it writes, what it refuses, and that
//! opening it with the blocks' values gives the blocks back.

use super::tests::catalog;
use super::*;
use crate::fixtures::pk;
use crate::idl::catalog::IdlCatalog;
use crate::ids::{
    ASSOCIATED_TOKEN_PROGRAM_ID, SYSTEM_PROGRAM_ID, TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID,
};
use crate::IdlDraft;
use solana_pubkey::Pubkey;
use std::collections::BTreeMap;

const WALLET_KEY: u8 = 1;
const FEE_PAYER_KEY: u8 = 2;
const MADE_HERE: u8 = 3;

fn wallet() -> Option<Pubkey> {
    Some(pk(WALLET_KEY))
}

fn fee_payer() -> Option<Pubkey> {
    Some(pk(FEE_PAYER_KEY))
}

/// A blank block for `instruction`, as Build makes one.
fn draft(program: Pubkey, instruction: &str) -> IdlDraft {
    let idls = catalog();
    let client = idls.program_client(&program).unwrap();
    let name = idls
        .records()
        .iter()
        .find(|record| record.program_id == program)
        .unwrap()
        .name
        .clone();
    IdlDraft::for_instruction(program, &name, &client, Some(instruction))
}

fn plan(drafts: &[IdlDraft]) -> SavePlan {
    SavePlan::new(drafts, &[pk(MADE_HERE)], wallet(), fee_payer(), &catalog()).unwrap()
}

/// Every choice that is not locked set to `choice` where it is offered.
fn all(plan: &SavePlan, choice: Choice) -> BTreeMap<FieldAt, Choice> {
    plan.blocks
        .iter()
        .flat_map(|block| &block.fields)
        .filter(|field| field.offers.contains(&choice))
        .map(|field| (field.at.clone(), choice))
        .collect()
}

fn save(drafts: &[IdlDraft], choices: &BTreeMap<FieldAt, Choice>) -> Saved {
    plan(drafts)
        .template("Saved", "From blocks.", choices, &catalog())
        .unwrap_or_else(|reasons| panic!("{reasons:#?}"))
}

/// Opens `saved` with the blocks' values as its answers, the same wallet,
/// fee payer and keypair, and checks it writes `drafts` back and builds
/// them to the same instructions.
fn round_trip(drafts: &[IdlDraft], saved: &Saved) {
    let template = &saved.template;
    let text = template.to_toml();
    assert_eq!(&Template::parse(&text).unwrap(), template, "{text}");
    check(template, &catalog()).unwrap_or_else(|problems| panic!("{problems:#?}\n{text}"));
    let env = Env {
        wallet: wallet(),
        fee_payer: fee_payer(),
        keypairs: saved.keypairs.clone(),
    };
    let expansion = expand(
        template,
        &saved.answers,
        &env,
        &Facts::default(),
        &catalog(),
    )
    .unwrap();
    assert!(expansion.is_complete(), "{expansion:#?}\n{text}");
    let back: Vec<IdlDraft> = expansion.blocks.into_iter().map(|b| b.draft).collect();
    assert_eq!(back.len(), drafts.len());
    let idls = catalog();
    for (index, (original, back)) in drafts.iter().zip(&back).enumerate() {
        let mut sorted = original.clone();
        sorted.manual.sort();
        let mut back = back.clone();
        back.manual.sort();
        assert_eq!(back, sorted, "block {}\n{text}", index + 1);
        let client = idls.program_client(&original.program_id.unwrap()).unwrap();
        assert_eq!(
            back.build(&client).unwrap(),
            original.clone().build(&client).unwrap(),
            "block {}",
            index + 1
        );
    }
}

/// Saved with every field kept, every field asked, and every address that
/// is the wallet or the fee payer named so, it opens to the same blocks.
fn round_trips_every_way(drafts: &[IdlDraft]) {
    let plan = plan(drafts);
    for choices in [
        BTreeMap::new(),
        all(&plan, Choice::Ask),
        all(&plan, Choice::Wallet),
        all(&plan, Choice::FeePayer),
    ] {
        round_trip(drafts, &save(drafts, &choices));
    }
}

fn transfer_sol() -> IdlDraft {
    let mut draft = draft(SYSTEM_PROGRAM_ID, "transferSol");
    draft.set_account("source", wallet());
    draft.set_account("destination", Some(pk(5)));
    draft.set_argument("amount", "1.5".into());
    draft
}

fn transfer_checked() -> IdlDraft {
    let mut draft = draft(TOKEN_PROGRAM_ID, "transferChecked");
    draft.set_account("source", Some(pk(10)));
    draft.set_account("mint", Some(pk(11)));
    draft.set_account("destination", Some(pk(12)));
    draft.set_account("authority", wallet());
    draft.set_argument("amount", "2.5".into());
    draft.set_argument("decimals", "6".into());
    draft
}

/// A Token-2022 mint made with a key made here, and its owner's token
/// account, whose token program the IDL would fill as Token's and the
/// person overrode.
fn create_mint() -> Vec<IdlDraft> {
    let mut create = draft(SYSTEM_PROGRAM_ID, "createAccount");
    create.set_account("payer", fee_payer());
    create.set_account("newAccount", Some(pk(MADE_HERE)));
    create.set_argument("lamports", "0.0014616".into());
    create.set_argument("space", "82".into());
    create.set_argument("programAddress", TOKEN_2022_PROGRAM_ID.to_string());
    let mut mint = draft(TOKEN_2022_PROGRAM_ID, "initializeMint2");
    mint.set_account("mint", Some(pk(MADE_HERE)));
    mint.set_argument("decimals", "6".into());
    mint.set_argument("mintAuthority", pk(WALLET_KEY).to_string());
    let mut ata = draft(
        ASSOCIATED_TOKEN_PROGRAM_ID,
        "createAssociatedTokenIdempotent",
    );
    ata.set_account("payer", fee_payer());
    ata.set_account("owner", wallet());
    ata.set_account("mint", Some(pk(MADE_HERE)));
    ata.set_manual("tokenProgram", true);
    ata.set_account("tokenProgram", Some(TOKEN_2022_PROGRAM_ID));
    vec![create, mint, ata]
}

#[test]
fn a_system_transfer_opens_to_the_same_block() {
    round_trips_every_way(&[transfer_sol()]);
}

#[test]
fn a_checked_token_transfer_with_its_scale_opens_to_the_same_block() {
    round_trips_every_way(&[transfer_checked()]);
    // The amount asked and its decimals kept, or both asked.
    let drafts = [transfer_checked()];
    let plan = plan(&drafts);
    let amount = FieldAt::new(0, "amount");
    let decimals = FieldAt::new(0, "decimals");
    for choices in [
        BTreeMap::from([(amount.clone(), Choice::Ask)]),
        BTreeMap::from([(amount.clone(), Choice::Ask), (decimals, Choice::Ask)]),
    ] {
        let saved = save(&drafts, &choices);
        assert_eq!(saved.answers.inputs.get("amount").unwrap(), "2.5");
        round_trip(&drafts, &saved);
    }
    assert_eq!(plan.field(&amount).unwrap().unit, None);
}

#[test]
fn a_mint_with_a_key_made_here_and_an_overridden_account_opens_to_the_same_blocks() {
    let drafts = create_mint();
    round_trips_every_way(&drafts);

    let saved = save(&drafts, &BTreeMap::new());
    let template = &saved.template;
    // One value for the key, wherever it appears.
    assert_eq!(template.keypair_values(), ["newAccount"]);
    assert_eq!(saved.keypairs.get("newAccount"), Some(&pk(MADE_HERE)));
    let key = Term::Ref("newAccount".into());
    assert_eq!(template.blocks[0].accounts.get("newAccount"), Some(&key));
    assert_eq!(template.blocks[1].accounts.get("mint"), Some(&key));
    assert_eq!(template.blocks[2].accounts.get("mint"), Some(&key));
    // The overridden account is written; the ones the IDL derives are not.
    let ata = &template.blocks[2];
    assert_eq!(
        ata.accounts.get("tokenProgram"),
        Some(&Term::Literal(TOKEN_2022_PROGRAM_ID.to_string()))
    );
    assert!(!ata.accounts.contains_key("ata"));
    assert!(!ata.accounts.contains_key("systemProgram"));
    // A blank optional field is left out.
    assert!(!template.blocks[1].arguments.contains_key("freezeAuthority"));
    // Aliases are the programs' names, tidied.
    assert_eq!(
        template.programs.keys().collect::<Vec<_>>(),
        ["associated_token", "system", "token2022"]
    );
}

#[test]
fn the_blocks_a_built_in_template_shows_open_to_the_same_blocks() {
    let mint = pk(7);
    for builtin in &BUILTIN_TEMPLATES {
        let template = builtin.template().unwrap();
        let mut answers = Answers::for_template(&template);
        for option in &template.options {
            answers = answers.turn_on(&option.name);
        }
        for input in &template.inputs {
            let text = match input.name.as_str() {
                "mint" => mint.to_string(),
                "amount" | "fee" => "1".to_string(),
                "max_fee" => "10".to_string(),
                "token_name" | "symbol" | "uri" | "nft_name" => "x".to_string(),
                "royalty" => "5.5".to_string(),
                "collection" => pk(23).to_string(),
                "recipient" => pk(24).to_string(),
                "cliff_amount" | "per_period" => "1".to_string(),
                "period" => "60".to_string(),
                "stream_name" => "Payroll".to_string(),
                "ticket" => pk(25).to_string(),
                "collection_name" => "x".to_string(),
                "freeze_authority" => pk(8).to_string(),
                "program" => pk(20).to_string(),
                "buffer" => pk(21).to_string(),
                "idl_buffer" => pk(22).to_string(),
                "repository" | "commit" | "version" => "x".to_string(),
                "build_args" => r#"["--library-name", "x"]"#.to_string(),
                "deploy_slot" => "123".to_string(),
                _ => continue,
            };
            answers = answers.answer(&input.name, &text);
        }
        let env = Env {
            wallet: wallet(),
            fee_payer: fee_payer(),
            keypairs: [
                ("mint".to_string(), pk(MADE_HERE)),
                ("stream".to_string(), pk(31)),
                ("ticket".to_string(), pk(32)),
                ("asset".to_string(), pk(33)),
                ("collection".to_string(), pk(34)),
            ]
            .into(),
        };
        let mut facts = Facts {
            decimals: [(mint, 6)].into(),
            token_programs: [(mint, TOKEN_PROGRAM_ID)].into(),
            ..Facts::default()
        };
        let first = expand(&template, &answers, &env, &facts, &catalog()).unwrap();
        for fact in &first.needs {
            if let Fact::Rent(bytes) = fact {
                facts.rent.insert(*bytes, 1_000_000);
            }
        }
        let shown = expand(&template, &answers, &env, &facts, &catalog()).unwrap();
        assert!(shown.is_complete(), "{}", builtin.id);
        let drafts: Vec<IdlDraft> = shown.blocks.into_iter().map(|b| b.draft).collect();
        round_trips_every_way(&drafts);
    }
}

#[test]
fn asked_fields_holding_one_value_are_one_question() {
    // The wallet is the transfer's source and the token transfer's
    // authority: both addresses, one value.
    let drafts = [transfer_sol(), transfer_checked()];
    let plan = plan(&drafts);
    let choices = BTreeMap::from([
        (FieldAt::new(0, "source"), Choice::Ask),
        (FieldAt::new(1, "authority"), Choice::Ask),
        (FieldAt::new(0, "amount"), Choice::Ask),
        (FieldAt::new(1, "amount"), Choice::Ask),
    ]);
    let inputs = plan.inputs(&choices);
    let names: Vec<&str> = inputs.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, ["source", "amount", "amount_2"]);
    assert_eq!(
        inputs[0].fields,
        [FieldAt::new(0, "source"), FieldAt::new(1, "authority")]
    );
    assert_eq!(inputs[0].label, "Source");
    // Two amounts with different values and units stay apart, and their
    // labels say which block.
    assert_eq!(inputs[1].label, "Amount");
    assert_eq!(inputs[2].label, "Amount (block 2)");
    let saved = save(&drafts, &choices);
    assert_eq!(saved.template.inputs.len(), 3);
    round_trip(&drafts, &saved);
}

#[test]
fn equal_values_of_different_kinds_are_asked_apart() {
    // A public key argument and an account holding one address are both
    // addresses: one question. A number is not.
    let mut first = draft(SYSTEM_PROGRAM_ID, "transferSol");
    first.set_account("source", wallet());
    first.set_account("destination", Some(pk(5)));
    first.set_argument("amount", "82".into());
    let mut create = draft(SYSTEM_PROGRAM_ID, "createAccount");
    create.set_account("payer", wallet());
    create.set_account("newAccount", Some(pk(6)));
    create.set_argument("lamports", "1".into());
    create.set_argument("space", "82".into());
    create.set_argument("programAddress", pk(5).to_string());
    let drafts = [first, create];
    let plan = plan(&drafts);
    let mut choices = BTreeMap::new();
    for at in [
        FieldAt::new(0, "destination"),
        FieldAt::new(1, "programAddress"),
        FieldAt::new(0, "amount"),
        FieldAt::new(1, "space"),
    ] {
        choices.insert(at, Choice::Ask);
    }
    let inputs = plan.inputs(&choices);
    assert_eq!(inputs.len(), 3, "{inputs:#?}");
    assert_eq!(inputs[0].fields.len(), 2, "the two addresses are joined");
    round_trip(&drafts, &save(&drafts, &choices));
}

#[test]
fn the_wallet_and_the_fee_payer_are_offered_only_where_they_are_held() {
    let drafts = [transfer_sol()];
    let plan = plan(&drafts);
    let source = plan.field(&FieldAt::new(0, "source")).unwrap();
    assert_eq!(source.offers, [Choice::Keep, Choice::Ask, Choice::Wallet]);
    let destination = plan.field(&FieldAt::new(0, "destination")).unwrap();
    assert_eq!(destination.offers, [Choice::Keep, Choice::Ask]);
    let saved = save(
        &drafts,
        &BTreeMap::from([(FieldAt::new(0, "source"), Choice::Wallet)]),
    );
    assert_eq!(
        saved.template.blocks[0].accounts.get("source"),
        Some(&Term::Ref(WALLET.into()))
    );
    let refused = plan
        .template(
            "T",
            "",
            &BTreeMap::from([(FieldAt::new(0, "destination"), Choice::FeePayer)]),
            &catalog(),
        )
        .unwrap_err();
    assert_eq!(
        refused,
        ["Block 1 (transferSol), destination: it does not hold the fee payer."]
    );
}

#[test]
fn a_blank_field_the_program_needs_must_be_asked() {
    let mut blank = transfer_sol();
    blank.set_account("destination", None);
    blank.set_argument("amount", String::new());
    let drafts = [blank];
    let plan = plan(&drafts);
    let destination = plan.field(&FieldAt::new(0, "destination")).unwrap();
    assert_eq!(destination.locked, Some(Locked::MustAsk));
    assert_eq!(destination.default_choice(), Choice::Ask);
    assert!(destination.offers.is_empty());
    // By default both are asked, and saved so.
    let saved = save(&drafts, &BTreeMap::new());
    assert_eq!(
        saved
            .template
            .inputs
            .iter()
            .map(|i| i.name.as_str())
            .collect::<Vec<_>>(),
        ["destination", "amount"]
    );
    // Kept instead, it is refused by name.
    let refused = plan
        .template(
            "T",
            "",
            &BTreeMap::from([(FieldAt::new(0, "amount"), Choice::Keep)]),
            &catalog(),
        )
        .unwrap_err();
    assert_eq!(
        refused,
        ["Block 1 (transferSol), amount: it is blank, and the program needs it. Ask for it each time, or fill it in on Build."]
    );
}

#[test]
fn a_blank_optional_field_is_left_out_or_asked() {
    let mut mint = draft(TOKEN_PROGRAM_ID, "initializeMint2");
    mint.set_account("mint", Some(pk(4)));
    mint.set_argument("decimals", "6".into());
    mint.set_argument("mintAuthority", pk(WALLET_KEY).to_string());
    let drafts = [mint];
    let plan = plan(&drafts);
    let freeze = FieldAt::new(0, "freezeAuthority");
    let field = plan.field(&freeze).unwrap();
    assert!(field.is_blank() && field.optional && field.locked.is_none());
    let kept = save(&drafts, &BTreeMap::new());
    assert!(!kept.template.blocks[0]
        .arguments
        .contains_key("freezeAuthority"));
    let asked = save(&drafts, &BTreeMap::from([(freeze, Choice::Ask)]));
    let input = asked.template.input("freezeAuthority").unwrap();
    assert!(input.optional);
    assert_eq!(input.label, "Freeze authority");
    round_trip(&drafts, &asked);
}

#[test]
fn what_cannot_be_saved_is_refused_with_every_reason() {
    let drafts = [transfer_sol()];
    let saving = plan(&drafts);
    let long = "x".repeat(81);
    let refused = saving
        .template(" ", &"y".repeat(301), &BTreeMap::new(), &catalog())
        .unwrap_err();
    assert_eq!(
        refused,
        [
            "Give the template a name.",
            "The summary is longer than 300 characters."
        ]
    );
    let refused = saving
        .template(&long, "", &BTreeMap::new(), &catalog())
        .unwrap_err();
    assert_eq!(refused, ["The name is longer than 80 characters."]);

    // A value the IDL does not read is the IDL's refusal, by block and field.
    let mut wrong = transfer_sol();
    wrong.set_argument("amount", "1.5.5".into());
    let refused = plan(&[wrong])
        .template("T", "", &BTreeMap::new(), &catalog())
        .unwrap_err();
    assert_eq!(refused.len(), 1);
    assert!(
        refused[0].starts_with("Block 1 (transferSol), amount: "),
        "{refused:?}"
    );
}

#[test]
fn blocks_that_are_not_ready_cannot_be_read() {
    let idls = catalog();
    let read = |drafts: &[IdlDraft]| SavePlan::new(drafts, &[], None, None, &idls).unwrap_err();
    assert_eq!(read(&[]), "There are no blocks on Build to save.");
    assert_eq!(
        read(&[transfer_sol(), IdlDraft::empty()]),
        "Block 2 has no program yet."
    );
    let mut nameless = transfer_sol();
    nameless.instruction.clear();
    assert_eq!(read(&[nameless]), "Block 1 has no instruction yet.");
    let mut unknown = transfer_sol();
    unknown.program_id = Some(pk(99));
    assert_eq!(
        read(&[unknown]),
        "Block 1's program has no IDL on this network."
    );
}

#[test]
fn the_template_is_named_and_summed_up_as_typed() {
    let saved = save(&[transfer_sol()], &BTreeMap::new());
    assert_eq!(saved.template.name, "Saved");
    assert_eq!(saved.template.summary.as_deref(), Some("From blocks."));
    let bare = plan(&[transfer_sol()])
        .template("  Pay rent ", "  ", &BTreeMap::new(), &catalog())
        .unwrap();
    assert_eq!(bare.template.name, "Pay rent");
    assert_eq!(bare.template.summary, None);
}

#[test]
fn a_field_name_reads_as_words() {
    assert_eq!(words("mintAuthority"), "Mint authority");
    assert_eq!(words("newAccount"), "New account");
    assert_eq!(words("freeze_authority"), "Freeze authority");
    assert_eq!(words("amount"), "Amount");
    assert_eq!(words("token2022Program"), "Token2022 program");
}

#[test]
fn a_slug_is_short_plain_and_never_a_path_or_a_built_in() {
    assert_eq!(template_slug("Pay the rent"), "pay-the-rent");
    assert_eq!(
        template_slug("  Weekly payroll: Q3/Q4!  "),
        "weekly-payroll-q3-q4"
    );
    assert_eq!(template_slug("../../etc/passwd"), "etc-passwd");
    assert_eq!(template_slug("C:\\Windows\\x.toml"), "c-windows-x-toml");
    assert_eq!(template_slug(""), "template");
    assert_eq!(template_slug("★★★"), "template");
    assert_eq!(template_slug("Café au lait"), "caf-au-lait");
    assert_eq!(template_slug("Create token"), "create-token-yours");
    assert_eq!(template_slug("mint tokens"), "mint-tokens-yours");
    assert_eq!(template_slug("CON"), "con-yours");
    let long = template_slug(&"word ".repeat(40));
    assert!(long.len() <= MAX_SLUG_CHARS, "{long}");
    assert!(!long.ends_with('-'));
    for slug in [
        "pay-the-rent",
        "template",
        "create-token-yours",
        long.as_str(),
    ] {
        assert!(is_template_slug(slug), "{slug}");
    }
    for not in [
        "",
        "create-token",
        "Pay",
        "a--b",
        "-a",
        "a-",
        "a.toml",
        "a/b",
        "con",
        "a.toml.bak",
    ] {
        assert!(!is_template_slug(not), "{not}");
    }
}

#[test]
fn a_built_in_templates_name_or_id_is_taken() {
    let named = |name: &str| builtin_named(name).map(|builtin| builtin.id);
    assert_eq!(named("Create a token"), Some("create-token"));
    assert_eq!(named("  create a TOKEN "), Some("create-token"));
    assert_eq!(named("mint-tokens"), Some("mint-tokens"));
    assert_eq!(named("Mint tokens"), Some("mint-tokens"));
    assert_eq!(named("Create token"), Some("create-token"));
    assert_eq!(named("Create a token for payroll"), None);
    assert_eq!(named("Pay the rent"), None);
}
