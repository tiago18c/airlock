//! Expanding templates into blocks.

use super::tests::{catalog, EXAMPLE};
use super::*;
use crate::fixtures::pk;
use crate::idl::catalog::IdlCatalog;
use crate::ids::{ASSOCIATED_TOKEN_PROGRAM_ID, TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID};
use crate::spl::{associated_token_address, create_ata_idempotent};
use solana_pubkey::Pubkey;

fn builtin(id: &str) -> Template {
    builtin_template(id).unwrap().template().unwrap()
}

fn env() -> Env {
    Env {
        wallet: Some(pk(1)),
        fee_payer: Some(pk(2)),
        keypairs: [
            ("mint".to_string(), pk(3)),
            ("stream".to_string(), pk(31)),
            ("ticket".to_string(), pk(32)),
            ("asset".to_string(), pk(33)),
            ("collection".to_string(), pk(34)),
            ("tree".to_string(), pk(35)),
        ]
        .into(),
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

/// A stream's escrow and its accounts as a create on mainnet named them,
/// and amounts typed in the mint's units once its decimals are known.
#[test]
fn a_stream_derives_its_escrow_and_takes_amounts_in_the_tokens_units() {
    let template = builtin("create-stream");
    let mint = Pubkey::from_str_const("2hVjgoesyZSbSGXEQY83hCjC19Grge8ud2kyTN2Rpump");
    let stream = Pubkey::from_str_const("BYw33zBVjfANmDt1cWHLGqJiJcpjLwUujsoDp2D28sMG");
    let sender = Pubkey::from_str_const("2k8Hgck8w3mQ2kZy2akG4GfpxTHMT3XaHjCEJtRf62Jm");
    let env = Env {
        wallet: Some(sender),
        fee_payer: Some(sender),
        keypairs: [("stream".to_string(), stream)].into(),
    };
    let answers = Answers::for_template(&template)
        .answer("recipient", &sender.to_string())
        .answer("mint", &mint.to_string())
        .answer("amount", "65955934.550769")
        .answer("per_period", "0.000001")
        .answer("period", "1");
    // The decimals are a fact: until they are known, nothing is scaled.
    let waiting = run(&template, &answers, &env, &Facts::default());
    assert!(waiting.needs.contains(&Fact::MintDecimals(mint)));
    assert_eq!(waiting.blocks[0].draft.argument("netAmountDeposited"), "");
    let facts = Facts {
        decimals: [(mint, 6)].into(),
        token_programs: [(mint, TOKEN_2022_PROGRAM_ID)].into(),
        ..Facts::default()
    };
    let expansion = run(&template, &answers, &env, &facts);
    assert!(expansion.is_complete(), "{expansion:#?}");
    let create = &expansion.blocks[0].draft;
    let account = |name: &str| create.account(name).unwrap().to_string();
    assert_eq!(
        account("escrowTokens"),
        "FUXfokqb8p1w1ZphpdgUmW3PCjCq5qgP2dZmtxrfkRZD"
    );
    assert_eq!(
        account("senderTokens"),
        "GvGWqdqUYEsTY7FD7VhswqHq4aTKZz8rrvqNMmpNunwH"
    );
    assert_eq!(
        account("streamflowTreasuryTokens"),
        "F6Dpjn554aGTtPXkhoShFLnHTeDV2i1LuzcxiCvVdzAA"
    );
    assert_eq!(account("partnerTokens"), account("senderTokens"));
    assert_eq!(create.argument("netAmountDeposited"), "65955934550769");
    assert_eq!(create.argument("amountPerPeriod"), "1");
    assert_eq!(create.argument("cliffAmount"), "0");
    // Each amount is typed with the mint's decimals.
    let form = check(&template, &catalog()).unwrap();
    let amount = form.inputs.iter().find(|i| i.name == "amount").unwrap();
    let InputKind::Argument { scale, .. } = &amount.kind else {
        panic!("{amount:?}")
    };
    assert_eq!(
        *scale,
        InputScale::FromValue {
            value: "decimals".to_string(),
            unit: None
        }
    );
    assert_eq!(expansion.decimals_of(scale), Some(6));
    assert_eq!(waiting.decimals_of(scale), None);
}

/// A tree's size is a choice of the shapes the programs take, its account
/// is sized from the shape and canopy, and a canopy Bubblegum refuses is
/// named on its input.
#[test]
fn a_tree_is_sized_from_its_shape_and_canopy_and_a_bad_canopy_is_named() {
    let template = builtin("create-nft-tree");
    let form = check(&template, &catalog()).unwrap();
    let shape = form.inputs.iter().find(|i| i.name == "shape").unwrap();
    let InputKind::Argument {
        kind: crate::IdlArgKind::Enum { variants },
        ..
    } = &shape.kind
    else {
        panic!("{shape:?}")
    };
    assert_eq!(variants.len(), 34);
    assert_eq!(variants[0], "8 NFTs (depth 3, buffer 8)");
    assert!(variants.contains(&"16,384 NFTs (depth 14, buffer 64)".to_string()));

    // The default shape, no canopy: 31,800 bytes, and their rent.
    let answers = Answers::for_template(&template);
    let first = run(&template, &answers, &env(), &Facts::default());
    assert_eq!(first.needs, [Fact::Rent(31_800)]);
    let expansion = run(&template, &answers, &env(), &rent(31_800, 222_000_000));
    assert!(expansion.is_complete(), "{expansion:#?}");
    let create = &expansion.blocks[1].draft;
    assert_eq!(create.argument("maxDepth"), "14");
    assert_eq!(create.argument("maxBufferSize"), "64");
    assert_eq!(
        create.account("treeAuthority"),
        Some(
            Pubkey::find_program_address(&[pk(35).as_ref()], &super::expand::BUBBLEGUM_PROGRAM_ID)
                .0
        )
    );

    // A depth of 24 needs a canopy of at least 7.
    let deep = answers
        .clone()
        .answer("shape", "16,777,216 NFTs (depth 24, buffer 64)")
        .answer("canopy", "3");
    let expansion = run(&template, &deep, &env(), &Facts::default());
    assert!(expansion.missing.contains(&Missing::Invalid {
        input: "canopy".to_string(),
        message: "A tree of depth 24 takes a canopy of 7 to 23.".to_string(),
    }));
    assert!(expansion.needs.is_empty(), "no size, so no rent to ask for");
    let deep = deep.answer("canopy", "7");
    let expansion = run(&template, &deep, &env(), &Facts::default());
    assert!(expansion.missing.is_empty(), "{:?}", expansion.missing);
}

/// The shipped IDLs of the first network that offers every block of
/// `template`, mainnet first.
fn catalog_for(template: &Template) -> impl IdlCatalog {
    use crate::idl::network::IdlNetwork;
    for network in [IdlNetwork::Mainnet, IdlNetwork::Devnet] {
        let store = crate::fixtures::shipped_catalog(network);
        if check(template, &store).is_ok_and(|form| form.unavailable.is_empty()) {
            return store;
        }
    }
    panic!("{} is offered whole on no network", template.name)
}

/// The Token Metadata accounts of a mint, a royalty in percent, the creator
/// list and the collection claim, worked out as Metaplex's clients do: the
/// addresses are those of an NFT on mainnet.
#[test]
fn an_nft_derives_its_metadata_and_edition_and_writes_its_creator() {
    let template = builtin("create-nft");
    let mint = Pubkey::from_str_const("77dS5XKvzP6bto59gWzyK3VUWY8qkv3Hdw8VyxttAzz8");
    let env = Env {
        keypairs: [("mint".to_string(), mint)].into(),
        ..env()
    };
    let answers = Answers::for_template(&template)
        .turn_on("in_collection")
        .answer("nft_name", "Ape")
        .answer("symbol", "APE")
        .answer("uri", "https://e.x/a.json")
        .answer("royalty", "5.5")
        .answer("collection", &pk(9).to_string());
    let expansion = run(&template, &answers, &env, &rent(82, 1_461_600));
    assert!(expansion.is_complete(), "{expansion:#?}");
    let create = &expansion.blocks[2].draft;
    assert_eq!(
        create.account("metadata"),
        Some(Pubkey::from_str_const(
            "H9bVaTZhsRMoc1F7QmsnmTtqYY7QkG1FdWz3RbVuP47E"
        ))
    );
    assert_eq!(
        create.account("masterEdition"),
        Some(Pubkey::from_str_const(
            "9C13d9vXB2c1a964XrMZTZ3phVgdkFWJ31dha8PCdu1m"
        ))
    );
    assert_eq!(create.argument("assetData.sellerFeeBasisPoints"), "550");
    assert_eq!(
        create.argument("assetData.creators"),
        format!(r#"[{{"address":"{}","verified":true,"share":100}}]"#, pk(1))
    );
    assert_eq!(
        create.argument("assetData.collection"),
        format!(r#"{{"verified":false,"key":"{}"}}"#, pk(9))
    );
    assert_eq!(
        expansion.values.get("royalty_points").map(String::as_str),
        Some("550")
    );

    // A royalty with more places than percent has, or none, leaves the
    // field blank; with no collection the claim is blank, so none.
    let answers = answers.answer("royalty", "5.555");
    let expansion = run(&template, &answers, &env, &rent(82, 1_461_600));
    assert_eq!(
        expansion.blocks[2]
            .draft
            .argument("assetData.sellerFeeBasisPoints"),
        ""
    );
    let answers = Answers::for_template(&template).answer("royalty", "0");
    let expansion = run(&template, &answers, &env, &rent(82, 1_461_600));
    assert_eq!(
        expansion.blocks[2].draft.argument("assetData.collection"),
        ""
    );
    // The royalty is typed in percent, with two places.
    let form = check(&template, &catalog()).unwrap();
    let royalty = form.inputs.iter().find(|i| i.name == "royalty").unwrap();
    assert_eq!(
        royalty.kind,
        InputKind::Argument {
            kind: crate::IdlArgKind::Integer {
                signed: false,
                float: false,
                bits: 64
            },
            scale: InputScale::Fixed {
                decimals: 2,
                unit: None
            },
        }
    );
}

#[test]
fn every_builtin_template_expands_and_builds_with_everything_on() {
    let mint = pk(7);
    for builtin_template in &BUILTIN_TEMPLATES {
        let template = builtin_template.template().unwrap();
        let idls = catalog_for(&template);
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
                "tree" => pk(26).to_string(),
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
        let mut facts = Facts {
            decimals: [(mint, 6)].into(),
            token_programs: [(mint, TOKEN_PROGRAM_ID)].into(),
            ..Facts::default()
        };
        let first = expand(&template, &answers, &env(), &facts, &idls).unwrap();
        for fact in &first.needs {
            if let Fact::Rent(bytes) = fact {
                facts.rent.insert(*bytes, 1_000_000);
            }
        }
        let mut expansion = expand(&template, &answers, &env(), &facts, &idls).unwrap();
        assert!(
            expansion.is_complete(),
            "{}: {expansion:#?}",
            builtin_template.id
        );
        let built: Vec<_> = expansion
            .blocks
            .iter_mut()
            .map(|block| {
                let client = idls
                    .program_client(&block.draft.program_id.unwrap())
                    .unwrap();
                block.draft.build(&client).unwrap_or_else(|err| {
                    panic!("{} block {}: {err}", builtin_template.id, block.number)
                })
            })
            .collect();
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

#[test]
fn the_starter_expands_to_a_transfer_that_builds() {
    let template = Template::parse(STARTER_TEMPLATE).unwrap();
    // Unanswered, the amount takes its default and only the address is
    // missing.
    let answers = Answers::for_template(&template);
    let blank = run(&template, &answers, &env(), &Facts::default());
    assert_eq!(blank.missing, [Missing::Input("to".into())]);
    let answers = answers.answer("to", &pk(9).to_string());
    let mut expansion = run(&template, &answers, &env(), &Facts::default());
    assert!(expansion.missing.is_empty() && expansion.needs.is_empty());
    assert_eq!(instructions(&expansion), ["transferSol"]);
    let built = build_all(&mut expansion);
    assert_eq!(
        built,
        [solana_system_interface::instruction::transfer(
            &pk(1),
            &pk(9),
            10_000_000
        )]
    );
}

#[test]
fn upgrading_a_program_derives_its_program_data_and_idl_account_and_needs_no_fact() {
    let template = builtin("upgrade-program");
    let program = pk(20);
    let program_data = Pubkey::find_program_address(
        &[program.as_ref()],
        &crate::program_code::BPF_LOADER_UPGRADEABLE_ID,
    )
    .0;
    let answers = Answers::for_template(&template)
        .answer("program", &program.to_string())
        .answer("buffer", &pk(21).to_string());
    let mut expansion = run(&template, &answers, &env(), &Facts::default());
    assert!(expansion.is_complete(), "{expansion:#?}");
    assert!(expansion.needs.is_empty() && expansion.uses.is_empty());
    assert_eq!(instructions(&expansion), ["upgrade"]);
    let built = build_all(&mut expansion);
    let accounts: Vec<Pubkey> = built[0].accounts.iter().map(|meta| meta.pubkey).collect();
    // Program data, program, buffer, spill (the wallet), rent, clock, authority.
    assert_eq!(
        accounts,
        [
            program_data,
            program,
            pk(21),
            pk(1),
            solana_sdk_ids::sysvar::rent::ID,
            solana_sdk_ids::sysvar::clock::ID,
            pk(1),
        ]
    );
    assert_eq!(built[0].data, 3u32.to_le_bytes());

    let answers = answers
        .turn_on("idl")
        .turn_on("verified_build")
        .answer("idl_buffer", &pk(22).to_string())
        .answer("repository", "https://example.com/p")
        .answer("commit", "abc")
        .answer("version", "0.4.11")
        .answer("deploy_slot", "9");
    let mut expansion = run(&template, &answers, &env(), &Facts::default());
    assert!(expansion.is_complete(), "{expansion:#?}");
    assert_eq!(
        instructions(&expansion),
        ["upgrade", "setData", "close", "trim", "update"]
    );
    let idl_account = crate::idl::catalog::program_metadata_idl_address(&program).unwrap();
    let set_data = &expansion.blocks[1].draft;
    assert_eq!(set_data.account("metadata"), Some(idl_account));
    assert_eq!(set_data.account("programData"), Some(program_data));
    assert_eq!(set_data.argument("compression"), "zlib");
    // The closed IDL buffer pays for the IDL account's new size, and trim
    // sends what is left over to the refund address.
    assert_eq!(
        expansion.blocks[2].draft.account("destination"),
        Some(idl_account)
    );
    assert_eq!(
        expansion.blocks[3].draft.account("destination"),
        Some(pk(1))
    );
    assert_eq!(expansion.blocks[4].draft.argument("params.args"), "");
    let built = build_all(&mut expansion);
    // setData: encoding utf8, zlib, json, direct, and no data.
    assert_eq!(built[1].data, [3, 1, 2, 1, 0]);
}
