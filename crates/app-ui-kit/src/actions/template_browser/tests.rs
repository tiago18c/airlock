//! What the Templates dialog lists and reads: the filter, the Questions and
//! Blocks tabs of every shipped template and of one with options, and the
//! text of one of the user's kept as it was written.

use super::*;
use crate::actions::kept_templates::{export_file, load_kept};
use crate::actions::template::template_entries;
use crate::actions::template::tests::Screen;
use crate::actions::template_text::{save_text, EditTarget};
use crate::state::forms::TemplateSource;
use std::collections::BTreeSet;
use wallet_core::BUILTIN_TEMPLATES;

fn view(screen: &Screen, source: &TemplateSource) -> TemplateView {
    let template = Arc::new(source.template().unwrap());
    let text = source.text().map(str::to_string);
    screen.act(|| template_view(&screen.session, template, text))
}

/// Every field each block of `template` sets, as the file binds it: block
/// number, accounts or arguments, field.
fn bound(template: &Template) -> BTreeSet<(usize, bool, String)> {
    template
        .blocks
        .iter()
        .enumerate()
        .flat_map(|(index, block)| {
            block
                .accounts
                .keys()
                .map(move |field| (index + 1, true, field.clone()))
                .chain(
                    block
                        .arguments
                        .keys()
                        .map(move |field| (index + 1, false, field.clone())),
                )
        })
        .collect()
}

/// Every field the Blocks tab shows, once each, or a panic.
fn shown(blocks: &[ReaderBlock]) -> BTreeSet<(usize, bool, String)> {
    let mut seen = BTreeSet::new();
    for block in blocks {
        for (account, rows) in [(true, &block.accounts), (false, &block.arguments)] {
            for row in rows {
                assert!(
                    seen.insert((block.number, account, row.field.clone())),
                    "block {} shows {} twice",
                    block.number,
                    row.field
                );
            }
        }
    }
    seen
}

#[tokio::test]
async fn every_field_a_shipped_template_sets_is_on_its_blocks_tab_once() {
    let screen = Screen::offline().await;
    for builtin in &BUILTIN_TEMPLATES {
        let source = TemplateSource::Builtin(builtin.id);
        let shown_view = view(&screen, &source);
        let template = &shown_view.template;
        // On devnet, a mainnet-only template lacks its programs' IDLs and
        // nothing else; its Blocks tab is drawn from the file all the same.
        let mainnet_only = crate::actions::template::tests::MAINNET_ONLY.contains(&builtin.id);
        assert!(
            shown_view.problems.is_empty()
                || mainnet_only
                    && shown_view
                        .problems
                        .iter()
                        .all(|problem| problem.contains("No IDL")),
            "{}: {:?}",
            builtin.id,
            shown_view.problems
        );
        assert_eq!(shown_view.text.as_deref(), Some(builtin.toml));
        assert_eq!(shown_view.blocks.len(), template.blocks.len());
        assert_eq!(shown(&shown_view.blocks), bound(template), "{}", builtin.id);
        // Each row says where its value comes from, as the file binds it.
        for (block, file) in shown_view.blocks.iter().zip(&template.blocks) {
            assert_eq!(block.instruction, file.instruction);
            for row in block.fields() {
                let term = file.binding(&row.field).unwrap();
                match (&row.origin, term) {
                    (FieldOrigin::Asked { input, label }, Term::Ref(name)) => {
                        assert_eq!(input, name);
                        assert_eq!(*label, template.input(name).unwrap().label);
                    }
                    (
                        FieldOrigin::Fixed {
                            alias: Some(alias),
                            address,
                            ..
                        },
                        Term::Ref(name),
                    ) => {
                        assert_eq!(alias, name);
                        assert_eq!(address.as_ref(), template.programs.get(name));
                    }
                    (
                        FieldOrigin::Fixed {
                            text, alias: None, ..
                        },
                        Term::Literal(literal),
                    ) => {
                        assert_eq!(text, literal)
                    }
                    (FieldOrigin::Worked { value, words }, Term::Ref(name)) => {
                        assert!(template.input(name).is_none());
                        assert!(!template.programs.contains_key(name));
                        assert!(!words.is_empty());
                        match value {
                            Some(value) => assert_eq!(value, name),
                            None => assert!(name == WALLET || name == FEE_PAYER),
                        }
                    }
                    other => panic!("{}: {} reads as {other:?}", builtin.id, row.field),
                }
            }
        }
    }
}

#[tokio::test]
async fn the_token_2022_template_reads_with_its_options_and_what_each_block_does() {
    let screen = Screen::offline().await;
    let shown_view = view(&screen, &TemplateSource::Builtin("create-token-2022"));
    let questions = &shown_view.questions;
    assert_eq!(questions.count(), shown_view.template.inputs.len());
    assert_eq!(questions.count(), 12);

    // Asked always, in the order written, with what each holds until it is
    // answered and the widget it takes.
    let always: Vec<&str> = questions.always.iter().map(|q| q.label.as_str()).collect();
    assert_eq!(always, ["Decimals", "Mint authority", "Freeze authority"]);
    assert_eq!(questions.always[0].until.as_deref(), Some("9"));
    assert_eq!(
        questions.always[1].until.as_deref(),
        Some("the wallet in use")
    );
    assert_eq!(questions.always[1].takes.as_deref(), Some("an address"));
    assert!(questions.always[2].optional);
    assert_eq!(questions.always[2].until, None);

    // Then each option, with the questions asked only under it.
    let options: Vec<(&str, usize)> = questions
        .options
        .iter()
        .map(|o| (o.label.as_str(), o.questions.len()))
        .collect();
    assert_eq!(
        options,
        [
            ("Name and symbol", 3),
            ("Transfer fee", 2),
            ("Close authority", 1),
            ("Permanent delegate", 1),
            ("Non-transferable", 0),
            ("Mint a first supply", 2),
        ]
    );
    assert!(questions.options.iter().all(|o| !o.on_at_first));
    let fee = &questions.options[1].questions[0];
    assert_eq!(fee.label, "Fee");
    assert_eq!(fee.takes.as_deref(), Some("a percentage"));
    let name = &questions.options[0].questions[0];
    assert_eq!(name.takes.as_deref(), Some("text"));

    // The blocks, in file order: the one under an option says which, and
    // the token program is named with its address.
    let blocks = &shown_view.blocks;
    assert_eq!(blocks.len(), 10);
    assert_eq!(blocks[0].program, "System");
    assert_eq!(blocks[0].when, None);
    assert_eq!(blocks[1].instruction, "initializeMetadataPointer");
    assert_eq!(blocks[1].when.as_deref(), Some("Name and symbol"));
    assert_eq!(
        blocks[1].program_id,
        Some(wallet_core::TOKEN_2022_PROGRAM_ID)
    );

    // createAccount: the keypair and the fee payer worked out, the program
    // fixed, and the lamports rent from the node, in SOL.
    let create = &blocks[0];
    let row = |block: &ReaderBlock, field: &str| {
        block
            .fields()
            .find(|row| row.field == field)
            .unwrap_or_else(|| panic!("{field}"))
            .clone()
    };
    assert_eq!(
        row(create, "newAccount").origin,
        FieldOrigin::Worked {
            value: Some("mint".into()),
            words: "a new keypair, made when it is opened".into()
        }
    );
    assert_eq!(
        row(create, "payer").origin,
        FieldOrigin::Worked {
            value: None,
            words: "the fee payer".into()
        }
    );
    let lamports = row(create, "lamports");
    assert_eq!(lamports.unit.as_deref(), Some("SOL"));
    assert_eq!(
        lamports.origin,
        FieldOrigin::Worked {
            value: Some("lamports".into()),
            words: "rent for $funded bytes, typed in as the chain has it".into()
        }
    );
    assert!(matches!(
        row(create, "programAddress").origin,
        FieldOrigin::Fixed { alias: Some(ref alias), .. } if alias == "token"
    ));
    // Accounts and arguments apart, accounts in the IDL's order.
    let accounts: Vec<&str> = create.accounts.iter().map(|r| r.field.as_str()).collect();
    assert_eq!(accounts, ["payer", "newAccount"]);
    let arguments: Vec<&str> = create.arguments.iter().map(|r| r.field.as_str()).collect();
    assert_eq!(arguments, ["lamports", "space", "programAddress"]);

    // An asked field names its question.
    assert_eq!(
        row(&blocks[6], "decimals").origin,
        FieldOrigin::Asked {
            input: "decimals".into(),
            label: "Decimals".into()
        }
    );
}

#[tokio::test]
async fn a_block_on_the_token_program_of_a_mint_says_so() {
    let screen = Screen::offline().await;
    let shown_view = view(&screen, &TemplateSource::Builtin("mint-tokens"));
    let mint = &shown_view.blocks[1];
    assert_eq!(mint.program_id, None);
    assert_eq!(mint.program, "The token program of the mint “Mint”");
    assert_eq!(
        mint.fields()
            .find(|row| row.field == "decimals")
            .unwrap()
            .origin,
        FieldOrigin::Worked {
            value: Some("decimals".into()),
            words: "the decimals of the mint “Mint”, typed in as the chain has it".into()
        }
    );
    let amount = &shown_view.questions.always[2];
    assert_eq!(amount.label, "Amount");
    assert_eq!(
        amount.takes.as_deref(),
        Some("a number with as many decimal places as decimals")
    );
}

const COMMENTED: &str = "# Pays a friend a fixed amount.
format = 1
name = \"Pay a friend\"
summary = \"A quarter of a SOL to whoever you name.\"

[programs]
system = \"11111111111111111111111111111111\" # the System Program

[[input]]
name = \"to\"
label = \"To\"

[[block]]
program = \"$system\"
instruction = \"transferSol\"
accounts = { source = \"$wallet\", destination = \"$to\" }
arguments = { amount = \"0.25\" }
";

/// Keep `text` as an imported file is kept from the editor.
fn keep_import(screen: &Screen, text: &str) {
    let importing = EditTarget::Import {
        file: "import.toml".into(),
    };
    let kept = screen.act(|| save_text(&screen.session, &importing, text, false));
    assert!(kept.is_ok(), "{kept:?}");
}

#[tokio::test]
async fn an_imported_template_is_kept_listed_and_exported_as_it_was_written() {
    let screen = Screen::offline().await;
    keep_import(&screen, COMMENTED);

    // Kept byte for byte, comments and all.
    let stored = screen
        .kit
        .storage
        .read(&app_core::storage::template_file("pay-a-friend"))
        .unwrap();
    assert_eq!(stored, COMMENTED.as_bytes());

    // Listed with its text, and read again from the store the same way.
    for reload in [false, true] {
        if reload {
            screen.act(|| load_kept(&screen.session));
        }
        let entries = screen.act(|| template_entries(&screen.session));
        let mine = entries.last().unwrap();
        assert_eq!(mine.name, "Pay a friend");
        assert_eq!(mine.source.text(), Some(COMMENTED));
        assert_eq!(
            export_file(&mine.source).unwrap(),
            ("pay-a-friend.toml".into(), COMMENTED.into())
        );
        let shown_view = view(&screen, &mine.source);
        assert_eq!(shown_view.text.as_deref(), Some(COMMENTED));
        let amount = shown_view.blocks[0]
            .fields()
            .find(|r| r.field == "amount")
            .unwrap()
            .clone();
        assert_eq!(amount.unit.as_deref(), Some("SOL"));
        assert!(matches!(amount.origin, FieldOrigin::Fixed { ref text, .. } if text == "0.25"));
    }
}

#[tokio::test]
async fn a_broken_file_keeps_its_text_to_show() {
    let screen = Screen::offline().await;
    let text = "format = 1\nname = \"Half\"\n# no blocks yet\nsurprise = 1\n";
    screen
        .kit
        .storage
        .put(&app_core::storage::template_file("half"), text.as_bytes());
    screen
        .kit
        .storage
        .put(&app_core::storage::template_file("bytes"), &[0xff, 0xfe]);
    screen.act(|| load_kept(&screen.session));
    let entries = screen.act(|| template_entries(&screen.session));
    let broken: Vec<_> = entries.iter().filter(|e| e.is_broken()).collect();
    assert_eq!(broken.len(), 2);
    assert_eq!(broken[0].name, "bytes.toml");
    assert_eq!(broken[0].source.text(), None);
    assert_eq!(broken[1].name, "half.toml");
    assert_eq!(broken[1].source.text(), Some(text));
    assert!(broken[1].problem.as_deref().unwrap().contains("surprise"));
}

#[tokio::test]
async fn the_filter_reads_names_and_summaries() {
    let screen = Screen::offline().await;
    let entries = screen.act(|| template_entries(&screen.session));
    let names = |filter: &str| -> Vec<String> {
        entries
            .iter()
            .filter(|entry| template_matches(entry, filter))
            .map(|entry| entry.name.clone())
            .collect()
    };
    assert_eq!(names("").len(), BUILTIN_TEMPLATES.len());
    assert_eq!(names("  "), names(""));
    assert_eq!(names("2022"), ["Create a Token-2022 token"]);
    assert_eq!(
        names("MINT"),
        [
            "Create a token",
            "Create a Token-2022 token",
            "Mint tokens",
            "Add a name and symbol to a token",
            "Create an NFT",
            "Create a compressed NFT tree",
            "Mint a compressed NFT",
            "Create a Raydium pool",
        ]
    );
    // "extensions" is only in a summary.
    assert_eq!(names("extensions"), ["Create a Token-2022 token"]);
    assert!(names("bridge").is_empty());
}

#[tokio::test]
async fn the_dialog_opens_on_the_first_template_that_can_be_used() {
    let screen = Screen::offline().await;
    let elsewhere = COMMENTED.replace(
        "11111111111111111111111111111111\" #",
        &format!("{}\" #", Pubkey::new_from_array([9; 32])),
    );
    let mut entries = screen.act(|| template_entries(&screen.session));
    keep_import(&screen, &elsewhere);
    let all = screen.act(|| template_entries(&screen.session));
    let greyed = all.last().unwrap().clone();
    assert!(greyed.problem.is_some());
    assert_eq!(greyed.problems.len(), 1, "{:?}", greyed.problems);
    assert!(
        greyed.problems[0].contains("No IDL"),
        "{:?}",
        greyed.problems
    );
    entries.insert(0, greyed.clone());
    assert_eq!(first_usable(&entries).unwrap().name, "Create a token");
    assert_eq!(
        first_usable(std::slice::from_ref(&greyed)).unwrap().name,
        greyed.name
    );
    assert_eq!(first_usable(&[]), None);

    // A template that does not fit still reads: its blocks, its problems,
    // and no widget for its questions, which its fields would decide.
    let shown_view = view(&screen, &greyed.source);
    assert_eq!(shown_view.problems, greyed.problems);
    assert_eq!(shown_view.blocks[0].program, "A program with no IDL here");
    assert_eq!(shown_view.questions.always[0].takes, None);
}
