//! Expanding templates into blocks.

use super::tests::{catalog, EXAMPLE};
use super::*;
use crate::fixtures::pk;
use crate::idl::catalog::IdlCatalog;
use crate::ids::{ASSOCIATED_TOKEN_PROGRAM_ID, TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID};
use crate::spl::{associated_token_address, create_ata_idempotent};

fn builtin(id: &str) -> Template {
    builtin_template(id).unwrap().template().unwrap()
}

fn env() -> Env {
    Env {
        wallet: Some(pk(1)),
        fee_payer: Some(pk(2)),
        keypairs: [("mint".to_string(), pk(3))].into(),
    }
}

fn run(template: &Template, answers: &Answers, env: &Env, facts: &Facts) -> Expansion {
    expand(template, answers, env, facts, &catalog()).unwrap()
}

fn instructions(expansion: &Expansion) -> Vec<String> {
    expansion
        .blocks
        .iter()
        .map(|block| block.draft.instruction.clone())
        .collect()
}

/// Builds every block with its program's IDL.
fn build_all(expansion: &mut Expansion) -> Vec<solana_instruction::Instruction> {
    let idls = catalog();
    expansion
        .blocks
        .iter_mut()
        .map(|block| {
            let program = block.draft.program_id.expect("a program");
            let client = idls.program_client(&program).unwrap();
            block
                .draft
                .build(&client)
                .unwrap_or_else(|err| panic!("block {}: {err}", block.number))
        })
        .collect()
}

fn rent(bytes: u64, lamports: u64) -> Facts {
    Facts {
        rent: [(bytes, lamports)].into(),
        ..Facts::default()
    }
}

#[test]
fn blank_answers_leave_blank_fields_and_say_what_is_missing() {
    let template = Template::parse(EXAMPLE).unwrap();
    let answers = Answers::for_template(&template).answer("decimals", "");
    let expansion = run(&template, &answers, &Env::default(), &Facts::default());
    assert_eq!(
        instructions(&expansion),
        ["createAccount", "initializeMint2"]
    );
    assert_eq!(
        expansion.missing,
        [
            Missing::Input("decimals".into()),
            Missing::Wallet,
            Missing::Input("authority".into()),
            Missing::Keypair("mint".into()),
            Missing::FeePayer,
        ]
    );
    // The size is the bare mint's, which needs no answer, so its rent is
    // already needed.
    assert_eq!(expansion.needs, [Fact::Rent(82)]);
    let create = &expansion.blocks[0].draft;
    assert_eq!(create.argument("space"), "82");
    assert_eq!(create.argument("lamports"), "");
    assert_eq!(create.account("newAccount"), None);
    let mint = &expansion.blocks[1].draft;
    assert_eq!(mint.argument("decimals"), "");
    assert_eq!(mint.argument("mintAuthority"), "");
    assert!(!expansion.is_complete());
}

#[test]
fn defaults_fill_unanswered_inputs_and_a_cleared_one_stays_blank() {
    let template = Template::parse(EXAMPLE).unwrap();
    let answers = Answers::for_template(&template);
    let expansion = run(&template, &answers, &env(), &rent(82, 1_461_600));
    assert!(expansion.is_complete(), "{expansion:#?}");
    let mint = &expansion.blocks[1].draft;
    assert_eq!(mint.argument("decimals"), "9");
    assert_eq!(mint.argument("mintAuthority"), pk(1).to_string());
    // Optional and blank: blank, which the IDL reads as none.
    assert_eq!(mint.argument("freezeAuthority"), "");

    let cleared = answers.clone().answer("authority", " ");
    let expansion = run(&template, &cleared, &env(), &rent(82, 1_461_600));
    assert_eq!(expansion.missing, [Missing::Input("authority".into())]);
    assert_eq!(expansion.blocks[1].draft.argument("mintAuthority"), "");
}

#[test]
fn a_finished_expansion_builds_with_rent_written_in_sol() {
    let template = Template::parse(EXAMPLE).unwrap();
    let answers = Answers::for_template(&template)
        .answer("decimals", "6")
        .answer("freeze_authority", &pk(9).to_string());
    let mut expansion = run(&template, &answers, &env(), &rent(82, 1_461_600));
    let create = &expansion.blocks[0];
    assert_eq!(create.draft.program_id, Some(crate::SYSTEM_PROGRAM_ID));
    assert_eq!(create.draft.program_name, "system");
    // Rent is lamports; the field is typed in SOL.
    assert_eq!(create.draft.argument("lamports"), "0.0014616");
    assert_eq!(create.draft.account("payer"), Some(pk(2)));
    let built = build_all(&mut expansion);
    assert_eq!(
        built[0],
        solana_system_interface::instruction::create_account(
            &pk(2),
            &pk(3),
            1_461_600,
            82,
            &TOKEN_2022_PROGRAM_ID
        )
    );
    assert_eq!(built[1].program_id, TOKEN_2022_PROGRAM_ID);
    // initializeMint2: tag 20, decimals, authority, then the freeze option.
    assert_eq!(built[1].data[..2], [20, 6]);
    assert_eq!(&built[1].data[2..34], pk(1).as_ref());
    assert_eq!(built[1].data[34], 1);
    assert_eq!(&built[1].data[35..67], pk(9).as_ref());
}

#[test]
fn rows_list_what_the_template_fixed_and_worked_out() {
    let template = Template::parse(EXAMPLE).unwrap();
    let expansion = run(
        &template,
        &Answers::for_template(&template),
        &env(),
        &rent(82, 1_461_600),
    );
    let create = &expansion.blocks[0];
    assert_eq!(
        create.fixed,
        [Row {
            field: "programAddress".into(),
            text: TOKEN_2022_PROGRAM_ID.to_string(),
            from: Some("token".into()),
        }]
    );
    let worked: Vec<(&str, &str, Option<&str>)> = create
        .worked_out
        .iter()
        .map(|row| (row.field.as_str(), row.text.as_str(), row.from.as_deref()))
        .collect();
    let mint = pk(3).to_string();
    let payer = pk(2).to_string();
    assert_eq!(
        worked,
        [
            ("newAccount", mint.as_str(), Some("mint")),
            ("payer", payer.as_str(), Some("fee-payer")),
            ("lamports", "0.0014616", Some("lamports")),
            ("space", "82", Some("space")),
        ]
    );
    // Inputs are asked, not listed.
    assert!(expansion.blocks[1]
        .worked_out
        .iter()
        .all(|row| row.field == "mint"));
}

#[test]
fn an_option_adds_its_blocks_inputs_and_size() {
    let template = Template::parse(EXAMPLE).unwrap();
    let off = Answers::for_template(&template);
    let expansion = run(&template, &off, &env(), &Facts::default());
    assert_eq!(expansion.needs, [Fact::Rent(82)]);
    assert!(!expansion.missing.contains(&Missing::Input("fee".into())));

    let on = off.turn_on("transfer_fee");
    let expansion = run(&template, &on, &env(), &Facts::default());
    assert_eq!(
        instructions(&expansion),
        [
            "createAccount",
            "initializeTransferFeeConfig",
            "initializeMint2"
        ]
    );
    assert_eq!(
        expansion.missing,
        [
            Missing::Input("fee".into()),
            Missing::Input("max_fee".into())
        ]
    );
    // 165 + 1 + a 4-byte header and 108 bytes of fee config.
    assert_eq!(expansion.blocks[0].draft.argument("space"), "278");
    assert_eq!(expansion.needs, [Fact::Rent(278)]);

    let answered = on.answer("fee", "1.5").answer("max_fee", "5000");
    let mut expansion = run(&template, &answered, &env(), &rent(278, 2_825_760));
    assert!(expansion.is_complete(), "{expansion:#?}");
    // The fee is typed in percent.
    assert_eq!(
        expansion.blocks[1].draft.argument("transferFeeBasisPoints"),
        "1.5"
    );
    let built = build_all(&mut expansion);
    // transferFeeExtension (26), initialize (0), two authorities, then the
    // basis points and the maximum fee.
    let data = &built[1].data;
    assert_eq!(data[..2], [26, 0]);
    assert_eq!(
        &data[data.len() - 10..data.len() - 8],
        &150u16.to_le_bytes()
    );
    assert_eq!(&data[data.len() - 8..], &5000u64.to_le_bytes());
}

#[test]
fn an_option_on_by_default_starts_on() {
    let text = EXAMPLE.replace(
        "label = \"Transfer fee\"",
        "label = \"Transfer fee\"\ndefault = true",
    );
    let template = Template::parse(&text).unwrap();
    assert!(Answers::for_template(&template).is_on("transfer_fee"));
}

#[test]
fn token_metadata_is_counted_only_in_the_funded_size() {
    let template = builtin("create-token-2022");
    let answers = Answers::for_template(&template)
        .turn_on("metadata")
        .answer("token_name", "Airlock")
        .answer("symbol", "AIR")
        .answer("uri", "https://example.com/air.json");
    let expansion = run(&template, &answers, &env(), &Facts::default());
    // The pointer: 166 + 4 + 64.
    assert_eq!(expansion.blocks[0].draft.argument("space"), "234");
    // Plus the metadata: 4 + 32 + 32 + (4 + 7) + (4 + 3) + (4 + 28) + 4.
    let funded = 234 + 4 + 32 + 32 + 11 + 7 + 32 + 4;
    assert_eq!(expansion.needs, [Fact::Rent(funded)]);

    // The metadata's text is not known yet: the funded size waits for it.
    let blank = Answers::for_template(&template).turn_on("metadata");
    let expansion = run(&template, &blank, &env(), &Facts::default());
    assert_eq!(expansion.blocks[0].draft.argument("space"), "234");
    assert!(expansion.needs.is_empty());
    assert_eq!(expansion.blocks[0].draft.argument("lamports"), "");
}

#[test]
fn an_overridden_derived_account_is_manual_as_builds_change_makes_it() {
    let template = builtin("mint-tokens");
    let mint = pk(7);
    let answers = Answers::for_template(&template)
        .answer("mint", &mint.to_string())
        .answer("amount", "1.5");
    let facts = Facts {
        decimals: [(mint, 6)].into(),
        token_programs: [(mint, TOKEN_2022_PROGRAM_ID)].into(),
        ..Facts::default()
    };
    let mut expansion = run(&template, &answers, &env(), &facts);
    assert!(expansion.is_complete(), "{expansion:#?}");
    let ata = &expansion.blocks[0].draft;
    assert_eq!(ata.program_id, Some(ASSOCIATED_TOKEN_PROGRAM_ID));
    assert!(ata.is_manual("tokenProgram"));
    assert!(!ata.is_manual("payer"));
    assert!(!ata.is_manual("ata"));
    assert_eq!(ata.account("tokenProgram"), Some(TOKEN_2022_PROGRAM_ID));

    let mint_to = &expansion.blocks[1];
    assert_eq!(mint_to.draft.program_id, Some(TOKEN_2022_PROGRAM_ID));
    assert_eq!(mint_to.draft.argument("decimals"), "6");
    assert_eq!(mint_to.draft.argument("amount"), "1.5");
    let account = associated_token_address(&pk(1), &mint, &TOKEN_2022_PROGRAM_ID);
    assert_eq!(mint_to.draft.account("token"), Some(account));

    let form = check(&template, &catalog()).unwrap();
    let amount = form.inputs.iter().find(|i| i.name == "amount").unwrap();
    let InputKind::Argument { scale, .. } = &amount.kind else {
        panic!("an amount");
    };
    assert_eq!(expansion.decimals_of(scale), Some(6));

    let built = build_all(&mut expansion);
    assert_eq!(
        built[0],
        create_ata_idempotent(pk(2), pk(1), mint, TOKEN_2022_PROGRAM_ID)
    );
    // mintToChecked (14), 1.5 at 6 decimals, then the decimals.
    assert_eq!(built[1].data[0], 14);
    assert_eq!(&built[1].data[1..9], &1_500_000u64.to_le_bytes());
    assert_eq!(built[1].data[9], 6);
}

#[test]
fn facts_are_needed_once_their_argument_is_known_then_used() {
    let template = builtin("mint-tokens");
    let nothing = Answers::for_template(&template);
    let expansion = run(&template, &nothing, &env(), &Facts::default());
    assert!(expansion.needs.is_empty());
    assert_eq!(
        expansion.missing,
        [
            Missing::Input("mint".into()),
            Missing::Input("amount".into())
        ]
    );
    // With no token program known, the token block's program is open.
    assert_eq!(expansion.blocks[1].draft.program_id, None);
    assert_eq!(expansion.blocks[1].draft.instruction, "mintToChecked");

    let mint = pk(7);
    let answers = nothing.answer("mint", &mint.to_string());
    let expansion = run(&template, &answers, &env(), &Facts::default());
    let mut needs = expansion.needs.clone();
    needs.sort();
    assert_eq!(needs, [Fact::MintDecimals(mint), Fact::TokenProgram(mint)]);
    assert_eq!(expansion.blocks[0].draft.account("tokenProgram"), None);

    let facts = Facts {
        decimals: [(mint, 2)].into(),
        token_programs: [(mint, TOKEN_PROGRAM_ID)].into(),
        ..Facts::default()
    };
    let expansion = run(&template, &answers, &env(), &facts);
    assert!(expansion.needs.is_empty());
    assert!(facts.knows(&Fact::MintDecimals(mint)));
    assert_eq!(expansion.blocks[1].draft.program_id, Some(TOKEN_PROGRAM_ID));
    assert_eq!(expansion.blocks[1].draft.argument("decimals"), "2");
    // An amount typed with more places than the mint has is still written as
    // typed: Build says what is wrong with it.
    let too_fine = answers.answer("amount", "0.001");
    let mut expansion = run(&template, &too_fine, &env(), &facts);
    let client = catalog().program_client(&TOKEN_PROGRAM_ID).unwrap();
    let err = expansion.blocks[1].draft.build(&client).unwrap_err();
    assert!(err.to_string().contains("decimal places"), "{err}");
}

#[test]
fn an_unreadable_answer_leaves_what_depends_on_it_blank() {
    let template = builtin("mint-tokens");
    let answers = Answers::for_template(&template).answer("mint", "not a mint");
    let expansion = run(&template, &answers, &env(), &Facts::default());
    assert!(expansion.needs.is_empty());
    let mint_to = &expansion.blocks[1].draft;
    assert_eq!(mint_to.account("mint"), None);
    assert_eq!(mint_to.account("token"), None);
    assert_eq!(mint_to.argument("decimals"), "");
}

#[test]
fn every_function_works_out_its_value() {
    let template = builtin("create-token");
    let answers = Answers::for_template(&template)
        .turn_on("first_supply")
        .answer("amount", "12.5")
        .answer("to", &pk(5).to_string());
    let mut expansion = run(&template, &answers, &env(), &rent(82, 1_461_600));
    assert!(expansion.is_complete(), "{expansion:#?}");
    assert_eq!(
        instructions(&expansion),
        [
            "createAccount",
            "initializeMint2",
            "createAssociatedTokenIdempotent",
            "mintToChecked"
        ]
    );
    // new-keypair, rent-for and associated-token-account.
    assert_eq!(expansion.blocks[0].draft.account("newAccount"), Some(pk(3)));
    assert_eq!(expansion.blocks[0].draft.argument("lamports"), "0.0014616");
    let account = associated_token_address(&pk(5), &pk(3), &TOKEN_PROGRAM_ID);
    assert_eq!(expansion.blocks[3].draft.account("token"), Some(account));
    // The token program is the IDL's default here: nothing manual.
    assert!(expansion.blocks[2].draft.manual.is_empty());
    let built = build_all(&mut expansion);
    assert_eq!(
        built[2],
        create_ata_idempotent(pk(2), pk(5), pk(3), TOKEN_PROGRAM_ID)
    );
    assert_eq!(&built[3].data[1..9], &12_500_000_000u64.to_le_bytes());
    assert_eq!(built[3].accounts[1].pubkey, account);
}

#[test]
fn every_builtin_template_expands_and_builds_with_everything_on() {
    let mint = pk(7);
    for builtin_template in &BUILTIN_TEMPLATES {
        let template = builtin_template.template().unwrap();
        let mut answers = Answers::for_template(&template);
        for option in &template.options {
            answers = answers.turn_on(&option.name);
        }
        for input in &template.inputs {
            let text = match input.name.as_str() {
                "mint" => mint.to_string(),
                "amount" | "fee" => "1".to_string(),
                "max_fee" => "10".to_string(),
                "token_name" | "symbol" | "uri" => "x".to_string(),
                _ => continue,
            };
            answers = answers.answer(&input.name, &text);
        }
        let mut facts = Facts {
            decimals: [(mint, 6)].into(),
            token_programs: [(mint, TOKEN_PROGRAM_ID)].into(),
            ..Facts::default()
        };
        let first = run(&template, &answers, &env(), &facts);
        for fact in &first.needs {
            if let Fact::Rent(bytes) = fact {
                facts.rent.insert(*bytes, 1_000_000);
            }
        }
        let mut expansion = run(&template, &answers, &env(), &facts);
        assert!(
            expansion.is_complete(),
            "{}: {expansion:#?}",
            builtin_template.id
        );
        let built = build_all(&mut expansion);
        assert_eq!(
            built.len(),
            template.blocks.len(),
            "{}",
            builtin_template.id
        );
    }
}

#[test]
fn an_inputs_default_reads_as_the_form_shows_it() {
    let template = builtin("create-token-2022");
    let env = env();
    assert_eq!(
        template.default_text("decimals", &env).as_deref(),
        Some("9")
    );
    assert_eq!(
        template.default_text("authority", &env),
        Some(pk(1).to_string()),
        "$wallet is the selected wallet's address"
    );
    assert_eq!(template.default_text("freeze_authority", &env), None);
    assert_eq!(template.default_text("no_such_input", &env), None);
    let no_wallet = Env {
        wallet: None,
        ..env.clone()
    };
    assert_eq!(template.default_text("authority", &no_wallet), None);

    let aliased = Template::parse(
        r#"
format = 1
name = "Aliased"

[programs]
token = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"
system = "11111111111111111111111111111111"

[[input]]
name = "owner"
label = "Owner"
default = "$token"

[[input]]
name = "payer"
label = "Payer"
default = "$fee-payer"

[[block]]
program = "$system"
instruction = "transferSol"
accounts = { source = "$payer", destination = "$owner" }
arguments = { amount = "1" }
"#,
    )
    .unwrap();
    assert_eq!(
        aliased.default_text("owner", &no_wallet),
        Some(TOKEN_PROGRAM_ID.to_string())
    );
    assert_eq!(
        aliased.default_text("payer", &no_wallet),
        Some(pk(2).to_string())
    );
}

#[test]
fn a_fact_answered_is_still_one_the_blocks_rest_on() {
    let template = builtin("create-token");
    let answers = Answers::for_template(&template);
    let first = run(&template, &answers, &env(), &Facts::default());
    let rent_fact = match first.needs.as_slice() {
        [fact @ Fact::Rent(_)] => *fact,
        other => panic!("{other:?}"),
    };
    assert_eq!(first.uses, first.needs);

    let Fact::Rent(bytes) = rent_fact else {
        unreachable!()
    };
    let answered = run(&template, &answers, &env(), &rent(bytes, 1_461_600));
    assert!(answered.needs.is_empty());
    assert_eq!(answered.uses, vec![rent_fact]);

    // A mint not known yet rests on nothing: its facts are not asked for.
    let mint = builtin("mint-tokens");
    let blank = run(
        &mint,
        &Answers::for_template(&mint),
        &env(),
        &Facts::default(),
    );
    assert!(blank.uses.is_empty() && blank.needs.is_empty());
}
