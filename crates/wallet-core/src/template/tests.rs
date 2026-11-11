//! Reading, writing and checking templates.

use super::*;
use crate::idl::catalog::{IdlCatalog, NoIdls};
use crate::idl::network::IdlNetwork;
use crate::ids::{TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID};

const SYSTEM: &str = "11111111111111111111111111111111";
const TOKEN_2022: &str = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";

/// The example of the plan: a Token-2022 mint with a transfer fee option.
pub(super) const EXAMPLE: &str = r#"
format = 1
name = "Create a Token-2022 token"
summary = "A new mint, with the extensions you choose."

[programs]
system = "11111111111111111111111111111111"
token = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb"

[[option]]
name = "transfer_fee"
label = "Transfer fee"
hint = "Keeps a share of every transfer in the receiving account."

[[input]]
name = "decimals"
label = "Decimals"
default = "9"

[[input]]
name = "authority"
label = "Mint authority"
default = "$wallet"

[[input]]
name = "freeze_authority"
label = "Freeze authority"
optional = true

[[input]]
name = "fee"
label = "Fee"
option = "transfer_fee"

[[input]]
name = "max_fee"
label = "Maximum fee"
hint = "The most one transfer can be charged, in the token's base units."
option = "transfer_fee"

[values]
mint = "new-keypair"
space = "token-2022-mint-space $mint"
funded = "token-2022-mint-funded-space $mint"
lamports = "rent-for $funded"

[[block]]
program = "$system"
instruction = "createAccount"
accounts = { payer = "$fee-payer", newAccount = "$mint" }
arguments = { lamports = "$lamports", space = "$space", programAddress = "$token" }

[[block]]
when = "transfer_fee"
program = "$token"
instruction = "initializeTransferFeeConfig"
accounts = { mint = "$mint" }
arguments = { transferFeeBasisPoints = "$fee", maximumFee = "$max_fee", transferFeeConfigAuthority = "$authority", withdrawWithheldAuthority = "$authority" }

[[block]]
program = "$token"
instruction = "initializeMint2"
accounts = { mint = "$mint" }
arguments = { decimals = "$decimals", mintAuthority = "$authority", freezeAuthority = "$freeze_authority" }
"#;

pub(super) fn catalog() -> impl IdlCatalog {
    crate::fixtures::shipped_catalog(IdlNetwork::Devnet)
}

/// A small template around one block, for refusals.
fn one_block(extra: &str, block: &str) -> String {
    format!(
        "format = 1\nname = \"T\"\n[programs]\nsystem = \"{SYSTEM}\"\ntoken = \"{TOKEN_2022}\"\n{extra}\n[[block]]\n{block}\n"
    )
}

fn refused(text: &str) -> (String, String) {
    match Template::parse(text) {
        Err(TemplateError::Invalid { key, expected }) => (key, expected),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn refused_toml(text: &str) -> String {
    match Template::parse(text) {
        Err(TemplateError::Toml(message)) => message,
        other => panic!("expected a TOML refusal, got {other:?}"),
    }
}

const DECIMALS_INPUT: &str = "[[input]]
name = \"decimals\"
label = \"Decimals\"
default = \"9\"
";
const MAX_FEE_INPUT: &str = "[[input]]
name = \"max_fee\"
label = \"Maximum fee\"
hint = \"The most one transfer can be charged, in the token's base units.\"
option = \"transfer_fee\"
";

const MINT_BLOCK: &str =
    "program = \"$token\"\ninstruction = \"initializeMint2\"\naccounts = { mint = \"$m\" }\narguments = { decimals = \"9\", mintAuthority = \"$wallet\" }";
const KEYPAIR: &str = "[values]\nm = \"new-keypair\"";

#[test]
fn the_plan_example_reads_with_its_order_and_references() {
    let template = Template::parse(EXAMPLE).unwrap();
    assert_eq!(template.name, "Create a Token-2022 token");
    let names: Vec<&str> = template.inputs.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "decimals",
            "authority",
            "freeze_authority",
            "fee",
            "max_fee"
        ]
    );
    assert_eq!(
        template.inputs[1].default,
        Some(Term::Ref("wallet".to_string()))
    );
    assert_eq!(
        template.inputs[0].default,
        Some(Term::Literal("9".to_string()))
    );
    assert!(template.inputs[2].optional);
    assert_eq!(template.inputs[3].option.as_deref(), Some("transfer_fee"));
    assert_eq!(template.blocks.len(), 3);
    assert_eq!(template.blocks[1].when.as_deref(), Some("transfer_fee"));
    assert_eq!(template.blocks[0].program, "system");
    assert_eq!(
        template.values["lamports"],
        Call {
            function: Function::RentFor,
            args: vec![Term::Ref("funded".into())]
        }
    );
    assert_eq!(template.keypair_values(), ["mint"]);
    // Each value after the ones it names.
    let order = template.value_order();
    let at = |name| order.iter().position(|n| *n == name).unwrap();
    assert!(at("mint") < at("space"));
    assert!(at("funded") < at("lamports"));
}

#[test]
fn a_dollar_is_a_reference_and_two_are_a_literal_dollar() {
    assert_eq!(Term::read("$mint"), Some(Term::Ref("mint".into())));
    assert_eq!(Term::read("$$5"), Some(Term::Literal("$5".into())));
    assert_eq!(Term::read("5$"), Some(Term::Literal("5$".into())));
    assert_eq!(Term::read("$"), None);
    for term in [
        Term::Ref("fee-payer".into()),
        Term::Literal("$5".into()),
        Term::Literal("$$".into()),
        Term::Literal("plain".into()),
    ] {
        assert_eq!(Term::read(&term.text()), Some(term));
    }
}

#[test]
fn written_and_read_back_a_template_is_the_same() {
    let template = Template::parse(EXAMPLE).unwrap();
    let text = template.to_toml();
    let again = Template::parse(&text).unwrap();
    assert_eq!(again, template);
    assert_eq!(
        again.to_toml(),
        text,
        "the same template writes the same text"
    );

    // Literal dollars, quotes and keys that need quoting survive.
    let tricky = one_block(
        "[values]\nm = \"new-keypair\"\n[[input]]\nname = \"n\"\nlabel = \"A \\\"quoted\\\" label\"\nhint = \"Costs $$5\"\ndefault = \"$$5\"",
        &format!("{MINT_BLOCK}\n[[block]]\nprogram = \"$token\"\ninstruction = \"initializeTokenMetadata\"\naccounts = {{ metadata = \"$m\", mint = \"$m\", mintAuthority = \"$wallet\", updateAuthority = \"$wallet\" }}\narguments = {{ name = \"$n\", symbol = \"$$\", uri = \"a b\" }}"),
    );
    let template = Template::parse(&tricky).unwrap();
    assert_eq!(template.inputs[0].default, Some(Term::Literal("$5".into())));
    assert_eq!(Template::parse(&template.to_toml()).unwrap(), template);
}

#[test]
fn every_builtin_template_is_written_as_to_toml_writes_it() {
    for builtin in &BUILTIN_TEMPLATES {
        let template = builtin.template().unwrap();
        assert_eq!(
            template.to_toml(),
            builtin.toml,
            "{} is not in the form to_toml writes",
            builtin.id
        );
    }
}

#[test]
fn a_file_too_large_is_refused_before_it_is_read() {
    let text = format!("{EXAMPLE}\n#{}", "x".repeat(MAX_TEMPLATE_BYTES));
    assert!(matches!(
        Template::parse(&text),
        Err(TemplateError::TooLarge { .. })
    ));
}

#[test]
fn not_toml_is_refused_with_its_line() {
    let message = refused_toml("format = 1\nname = \n");
    assert!(message.starts_with("line 2"), "{message}");
}

fn hidden(text: &str) -> (String, char) {
    match Template::parse(text) {
        Err(TemplateError::Hidden { at, character }) => (at, character),
        other => panic!("expected a hidden character refused, got {other:?}"),
    }
}

#[test]
fn a_control_or_bidirectional_character_is_refused_with_its_line() {
    let refused: Vec<char> = (0u32..0x20)
        .chain(0x7f..0xa0)
        .filter_map(char::from_u32)
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .chain('\u{202A}'..='\u{202E}')
        .chain('\u{2066}'..='\u{2069}')
        .chain(['\u{200E}', '\u{200F}', '\u{061C}'])
        .collect();
    assert_eq!(refused.len(), 29 + 33 + 5 + 4 + 3);
    for c in refused {
        // In a comment, in a string and in a key: anywhere in the text.
        for text in [
            format!("{EXAMPLE}\n# a comment {c} here\n"),
            EXAMPLE.replace("A new mint,", &format!("A new{c} mint,")),
            EXAMPLE.replace("max_fee\"\nlabel", &format!("max_fee\"\nla{c}bel")),
        ] {
            let line = text
                .lines()
                .position(|line| line.contains(c))
                .expect("the character is in the text")
                + 1;
            let (at, character) = hidden(&text);
            assert_eq!(
                (at.as_str(), character),
                (format!("line {line}").as_str(), c)
            );
        }
    }
    let message = Template::parse(&EXAMPLE.replace("Create a", "Create\u{202E} a"))
        .unwrap_err()
        .to_string();
    assert_eq!(
        message,
        "line 3: holds U+202E, a bidirectional control. It can make the text read as something other than what it is, so a template may not hold one."
    );
    let message = Template::parse(&format!("{EXAMPLE}\n#\u{7}\n"))
        .unwrap_err()
        .to_string();
    assert!(message.contains("U+0007, a control character"), "{message}");
}

#[test]
fn a_hidden_character_written_as_an_escape_is_refused_by_its_key() {
    let text = EXAMPLE.replace("label = \"Fee\"", "label = \"F\\u202Eee\"");
    assert_eq!(hidden(&text), ("input[4].label".to_string(), '\u{202E}'));
    let text = EXAMPLE.replace(
        "accounts = { mint = \"$mint\" }\narguments = { decimals",
        "accounts = { mint = \"$mint\" }\narguments = { \"dec\\u0007\" = \"1\", decimals",
    );
    assert_eq!(
        hidden(&text),
        ("block[3].arguments.dec\u{7}".to_string(), '\u{7}')
    );
}

#[test]
fn tabs_line_ends_and_other_text_are_read() {
    let text = EXAMPLE.replace('\n', "\r\n").replace(
        "format = 1",
        "format =\t1 # Ünïcödé and 日本語 and émoji 🙂",
    );
    assert_eq!(
        Template::parse(&text).unwrap(),
        Template::parse(EXAMPLE).unwrap()
    );
    // Every shipped file and everything to_toml writes still reads.
    for builtin in &BUILTIN_TEMPLATES {
        let template = builtin.template().unwrap();
        assert_eq!(Template::parse(&template.to_toml()).unwrap(), template);
    }
}

#[test]
fn only_format_1_is_read() {
    assert_eq!(refused("name = \"T\"").0, "format");
    let (key, expected) = refused(&EXAMPLE.replace("format = 1", "format = 2"));
    assert_eq!(key, "format");
    assert!(expected.contains("format = 1"), "{expected}");
    assert_eq!(refused("format = \"1\"\nname = \"T\"").0, "format");
}

#[test]
fn an_unknown_key_anywhere_is_refused_by_name() {
    for (text, key) in [
        (
            EXAMPLE.replace("summary =", "author = \"me\"\nsummary ="),
            "author",
        ),
        (
            EXAMPLE.replace(
                "label = \"Decimals\"",
                "label = \"Decimals\"\ntype = \"u8\"",
            ),
            "type",
        ),
        (
            EXAMPLE.replace(
                "label = \"Transfer fee\"",
                "label = \"Transfer fee\"\nblocks = 1",
            ),
            "blocks",
        ),
        (
            EXAMPLE.replace(
                "instruction = \"initializeMint2\"",
                "instruction = \"initializeMint2\"\nsigners = []",
            ),
            "signers",
        ),
    ] {
        let message = refused_toml(&text);
        assert!(message.contains(key), "{message}");
    }
}

#[test]
fn an_unknown_function_or_wrong_arity_is_refused() {
    let (key, expected) = refused(&EXAMPLE.replace("new-keypair", "fetch-url"));
    assert_eq!(key, "values.mint");
    assert!(expected.contains("fetch-url") && expected.contains("new-keypair"));

    let (key, expected) = refused(&EXAMPLE.replace("rent-for $funded", "rent-for $funded 5"));
    assert_eq!(key, "values.lamports");
    assert!(expected.contains("2 arguments"), "{expected}");

    let (key, _) = refused(&EXAMPLE.replace("new-keypair", "new-keypair $wallet"));
    assert_eq!(key, "values.mint");

    assert_eq!(
        refused(&EXAMPLE.replace("lamports = \"rent-for $funded\"", "lamports = \"\"")).0,
        "values.lamports"
    );
}

#[test]
fn a_function_given_the_wrong_kind_of_argument_is_refused() {
    // rent-for takes a size, not an address.
    assert_eq!(
        refused(&EXAMPLE.replace("rent-for $funded", "rent-for $mint")).0,
        "values.lamports"
    );
    assert_eq!(
        refused(&EXAMPLE.replace("rent-for $funded", "rent-for lots")).0,
        "values.lamports"
    );
    assert_eq!(
        refused(&EXAMPLE.replace(
            "token-2022-mint-space $mint",
            "token-2022-mint-space notanaddress"
        ))
        .0,
        "values.space"
    );
}

#[test]
fn a_reference_to_nothing_is_refused_where_it_is() {
    let (key, expected) = refused(&EXAMPLE.replace("\"$decimals\"", "\"$decimal\""));
    assert_eq!(key, "block[3].arguments.decimals");
    assert!(expected.contains("$decimal"), "{expected}");

    assert_eq!(
        refused(&EXAMPLE.replace("rent-for $funded", "rent-for $nothing")).0,
        "values.lamports"
    );
    assert_eq!(
        refused(&EXAMPLE.replace("program = \"$system\"", "program = \"$sys\"")).0,
        "block[1].program"
    );
    // An option holds no value.
    let (key, expected) = refused(&EXAMPLE.replace("\"$max_fee\"", "\"$transfer_fee\""));
    assert_eq!(key, "block[2].arguments.maximumFee");
    assert!(expected.contains("option"), "{expected}");
    // A `$` alone.
    assert_eq!(
        refused(&EXAMPLE.replace("\"$max_fee\"", "\"$\"")).0,
        "block[2].arguments.maximumFee"
    );
}

#[test]
fn a_name_used_twice_is_refused() {
    // An input named like a value.
    let text = EXAMPLE.replace("name = \"fee\"", "name = \"space\"");
    let (key, expected) = refused(&text);
    assert_eq!(key, "values.space");
    assert!(expected.contains("input[4]"), "{expected}");
    // Two inputs.
    assert_eq!(
        refused(&EXAMPLE.replace("name = \"max_fee\"", "name = \"fee\"")).0,
        "input[5].name"
    );
    // An option named like a program alias.
    assert_eq!(
        refused(&EXAMPLE.replace("name = \"transfer_fee\"", "name = \"token\"")).0,
        "option[1].name"
    );
    // A built-in name.
    let (key, expected) = refused(&EXAMPLE.replace("name = \"authority\"", "name = \"wallet\""));
    assert_eq!(key, "input[2].name");
    assert!(expected.contains("built-in"), "{expected}");
    assert_eq!(
        refused(&EXAMPLE.replace("mint = \"new-keypair\"", "fee-payer = \"new-keypair\"")).0,
        "values.fee-payer"
    );
}

#[test]
fn when_and_option_must_name_an_option() {
    assert_eq!(
        refused(&EXAMPLE.replace("when = \"transfer_fee\"", "when = \"fees\"")).0,
        "block[2].when"
    );
    assert_eq!(
        refused(&EXAMPLE.replacen("option = \"transfer_fee\"", "option = \"fees\"", 1)).0,
        "input[4].option"
    );
}

#[test]
fn empty_names_labels_and_bad_names_are_refused() {
    assert_eq!(
        refused(&EXAMPLE.replace("name = \"Create a Token-2022 token\"", "name = \"\"")).0,
        "name"
    );
    assert_eq!(
        refused(&EXAMPLE.replace("label = \"Decimals\"", "label = \" \"")).0,
        "input[1].label"
    );
    assert_eq!(
        refused(&EXAMPLE.replace("name = \"decimals\"", "name = \"9lives\"")).0,
        "input[1].name"
    );
    assert_eq!(
        refused(&EXAMPLE.replace("name = \"decimals\"", "name = \"has space\"")).0,
        "input[1].name"
    );
}

#[test]
fn a_block_needs_a_program_reference_and_an_instruction() {
    assert_eq!(
        refused(&one_block(KEYPAIR, "instruction = \"initializeMint2\"")).0,
        "block[1]"
    );
    assert_eq!(
        refused(&one_block(KEYPAIR, "program = \"$token\"")).0,
        "block[1]"
    );
    assert_eq!(
        refused(&one_block(
            KEYPAIR,
            "program = \"$token\"\ninstruction = \"\""
        ))
        .0,
        "block[1]"
    );
    // A literal address is not a program alias.
    assert_eq!(
        refused(&one_block(
            KEYPAIR,
            &MINT_BLOCK.replace("\"$token\"", &format!("\"{TOKEN_2022}\""))
        ))
        .0,
        "block[1].program"
    );
    // Nor is a value that is not a token program.
    assert_eq!(
        refused(&one_block(
            KEYPAIR,
            &MINT_BLOCK.replacen("\"$token\"", "\"$m\"", 1)
        ))
        .0,
        "block[1].program"
    );
    // A field bound twice.
    assert_eq!(
        refused(&one_block(
            KEYPAIR,
            &MINT_BLOCK.replace("decimals = \"9\"", "decimals = \"9\", mint = \"$m\"")
        ))
        .0,
        "block[1].arguments.mint"
    );
}

#[test]
fn values_that_need_each_other_are_refused() {
    let text = one_block(
        "[values]\nm = \"associated-token-account $wallet $n $token\"\nn = \"associated-token-account $wallet $m $token\"",
        MINT_BLOCK,
    );
    let (key, expected) = refused(&text);
    assert!(key == "values.m" || key == "values.n", "{key}");
    assert!(
        expected.contains("$m") && expected.contains("$n"),
        "{expected}"
    );

    let text = one_block(
        "[values]\nm = \"associated-token-account $wallet $m $token\"",
        MINT_BLOCK,
    );
    assert_eq!(refused(&text).0, "values.m");
}

#[test]
fn an_unused_input_or_value_is_refused() {
    let text = EXAMPLE.replace("\"$max_fee\"", "\"5\"");
    assert_eq!(refused(&text).0, "input[5].name");
    let text = one_block(
        "[values]\nm = \"new-keypair\"\nunused = \"new-keypair\"",
        MINT_BLOCK,
    );
    assert_eq!(refused(&text).0, "values.unused");
}

#[test]
fn a_default_may_only_be_a_literal_a_built_in_or_a_program() {
    let text = EXAMPLE.replace("default = \"$wallet\"", "default = \"$mint\"");
    assert_eq!(refused(&text).0, "input[2].default");
    let text = EXAMPLE.replace("default = \"$wallet\"", "default = \"$token\"");
    assert!(Template::parse(&text).is_ok());
}

#[test]
fn too_many_blocks_or_inputs_are_refused() {
    let blocks = (0..=MAX_BLOCKS)
        .map(|_| format!("[[block]]\n{MINT_BLOCK}\n"))
        .collect::<String>();
    let text = format!(
        "format = 1\nname = \"T\"\n[programs]\ntoken = \"{TOKEN_2022}\"\n{KEYPAIR}\n{blocks}"
    );
    assert_eq!(refused(&text).0, "block");

    let inputs = (0..=MAX_INPUTS)
        .map(|n| format!("[[input]]\nname = \"i{n}\"\nlabel = \"I\"\n"))
        .collect::<String>();
    let text = format!("format = 1\nname = \"T\"\n{inputs}");
    assert_eq!(refused(&text).0, "input");
}

// Checking against the IDLs.

fn problems_of(text: &str) -> Vec<Problem> {
    let template = Template::parse(text).unwrap();
    check(&template, &catalog()).expect_err("expected problems")
}

fn has(problems: &[Problem], block: usize, field: Option<&str>, words: &str) {
    assert!(
        problems.iter().any(|problem| problem.block == Some(block)
            && problem.field.as_deref() == field
            && problem.message.contains(words)),
        "no problem at block {block} {field:?} saying {words:?} in {problems:#?}"
    );
}

#[test]
fn the_plan_example_checks_and_gives_its_form() {
    let template = Template::parse(EXAMPLE).unwrap();
    let form = check(&template, &catalog()).unwrap();
    let kinds: Vec<(&str, &InputKind)> = form
        .inputs
        .iter()
        .map(|input| (input.name.as_str(), &input.kind))
        .collect();
    let u8_none = InputKind::Argument {
        kind: crate::IdlArgKind::Integer {
            signed: false,
            float: false,
            bits: 8,
        },
        scale: InputScale::None,
    };
    assert_eq!(kinds[0], ("decimals", &u8_none));
    assert_eq!(kinds[1], ("authority", &InputKind::Address));
    assert_eq!(kinds[2], ("freeze_authority", &InputKind::Address));
    assert_eq!(
        kinds[3],
        (
            "fee",
            &InputKind::Argument {
                kind: crate::IdlArgKind::Integer {
                    signed: false,
                    float: false,
                    bits: 16
                },
                scale: InputScale::Fixed {
                    decimals: 2,
                    unit: Some("%".into())
                }
            }
        )
    );
    assert_eq!(form.inputs[3].option.as_deref(), Some("transfer_fee"));
    assert!(form.inputs[2].optional);
    assert_eq!(form.options.len(), 1);
}

#[test]
fn a_program_with_no_idl_on_the_network_is_named() {
    let template = Template::parse(EXAMPLE).unwrap();
    let problems = check(&template, &NoIdls).unwrap_err();
    assert_eq!(problems.len(), 3);
    has(&problems, 1, None, "No IDL for `$system`");
    has(&problems, 3, None, "No IDL for `$token`");
}

#[test]
fn an_instruction_or_field_the_idl_lacks_is_named() {
    let problems = problems_of(&EXAMPLE.replace("\"initializeMint2\"", "\"initializeMint3\""));
    has(&problems, 3, None, "no instruction `initializeMint3`");

    let problems = problems_of(&EXAMPLE.replace("newAccount = ", "account = "));
    has(&problems, 1, Some("account"), "no account `account`");
    // And the account it did not bind is needed.
    has(&problems, 1, Some("newAccount"), "needs this account");

    let problems = problems_of(&EXAMPLE.replace("programAddress = ", "owner = "));
    has(&problems, 1, Some("owner"), "no argument `owner`");

    // In the wrong table.
    let problems = problems_of(&EXAMPLE.replace(
        "accounts = { mint = \"$mint\" }\narguments = { decimals",
        "accounts = { decimals = \"9\", mint = \"$mint\" }\narguments = { dec",
    ));
    has(
        &problems,
        3,
        Some("decimals"),
        "argument of initializeMint2",
    );
}

#[test]
fn an_unbound_required_field_is_refused_and_idl_defaults_are_not() {
    // `payer` is required; the associated account and token program have
    // IDL defaults.
    let text = format!(
        "format = 1\nname = \"T\"\n[programs]\nata = \"ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL\"\n[[block]]\nprogram = \"$ata\"\ninstruction = \"createAssociatedTokenIdempotent\"\naccounts = {{ owner = \"$wallet\", mint = \"{SYSTEM}\" }}\n"
    );
    let problems = problems_of(&text);
    assert_eq!(problems.len(), 1, "{problems:#?}");
    has(&problems, 1, Some("payer"), "needs this account");

    let problems = problems_of(
        &EXAMPLE
            .replace("decimals = \"$decimals\", ", "")
            .replace(DECIMALS_INPUT, ""),
    );
    has(&problems, 3, Some("decimals"), "needs this argument");
}

#[test]
fn a_required_field_bound_to_an_optional_input_is_refused() {
    let text = EXAMPLE.replace(
        "mintAuthority = \"$authority\", freezeAuthority = \"$freeze_authority\"",
        "mintAuthority = \"$freeze_authority\"",
    );
    let problems = problems_of(&text);
    has(&problems, 3, Some("mintAuthority"), "is optional");
    // Through a value too.
    let text = one_block(
        "[[input]]\nname = \"owner\"\nlabel = \"Owner\"\noptional = true\n[values]\nm = \"associated-token-account $owner $wallet $token\"",
        MINT_BLOCK,
    );
    let problems = problems_of(&text);
    has(&problems, 1, Some("mint"), "`$owner` is optional");
}

#[test]
fn a_required_field_on_an_input_of_another_option_is_refused() {
    // The fee block without its `when`.
    let text = EXAMPLE.replace(
        "when = \"transfer_fee\"
",
        "",
    );
    let problems = problems_of(&text);
    has(
        &problems,
        2,
        Some("transferFeeBasisPoints"),
        "only while `transfer_fee` is on",
    );
}

#[test]
fn bindings_of_one_input_that_disagree_are_refused() {
    // Decimals is a u8; the fee is a percentage.
    let text = EXAMPLE
        .replace("\"$max_fee\"", "\"$fee\"")
        .replace(MAX_FEE_INPUT, "");
    let problems = problems_of(&text);
    assert!(
        problems
            .iter()
            .any(|p| p.block.is_none() && p.message.contains("`$fee` is bound to fields")),
        "{problems:#?}"
    );
    // An address and a number.
    let text = EXAMPLE
        .replace("decimals = \"$decimals\"", "decimals = \"$authority\"")
        .replace(DECIMALS_INPUT, "");
    let problems = problems_of(&text);
    assert!(
        problems
            .iter()
            .any(|p| p.message.contains("`$authority` is bound to fields")),
        "{problems:#?}"
    );
}

#[test]
fn a_value_of_the_wrong_kind_for_its_field_is_refused() {
    let text = EXAMPLE
        .replace("space = \"$space\"", "space = \"$mint\"")
        .replace("programAddress = \"$token\"", "programAddress = \"$space\"");
    let problems = problems_of(&text);
    has(&problems, 1, Some("space"), "holds an address");
    has(&problems, 1, Some("programAddress"), "holds a number");
}

#[test]
fn a_literal_the_field_cannot_read_is_refused() {
    let text = one_block(
        KEYPAIR,
        "program = \"$system\"
instruction = \"createAccount\"
accounts = { payer = \"nobody\", newAccount = \"$m\" }
arguments = { lamports = \"0.0000000001\", space = \"eighty\", programAddress = \"$token\" }",
    );
    let problems = problems_of(&text);
    has(&problems, 1, Some("payer"), "not an address");
    has(&problems, 1, Some("space"), "eighty");
    // More places than the field's unit.
    has(&problems, 1, Some("lamports"), "decimal places");
}

/// `initializeDefaultAccountState.state` links to the enum `accountState`:
/// an input bound to it is a choice of its variants, a literal must be one,
/// and it cannot also be bound to a number.
#[test]
fn an_enum_behind_a_link_is_a_choice_in_a_template() {
    let block = |state: &str| {
        one_block(
            "[[input]]\nname = \"state\"\nlabel = \"State\"\n[[input]]\nname = \"mint\"\nlabel = \"Mint\"",
            &format!(
                "program = \"$token\"\ninstruction = \"initializeDefaultAccountState\"\naccounts = {{ mint = \"$mint\" }}\narguments = {{ state = \"{state}\" }}"
            ),
        )
    };
    let template = Template::parse(&block("$state")).unwrap();
    let form = check(&template, &catalog()).unwrap();
    let state = form.inputs.iter().find(|i| i.name == "state").unwrap();
    assert_eq!(
        state.kind,
        InputKind::Argument {
            kind: crate::IdlArgKind::Enum {
                variants: vec![
                    "uninitialized".into(),
                    "initialized".into(),
                    "frozen".into()
                ],
            },
            scale: InputScale::None,
        }
    );

    let fixed = block("frozen").replace("[[input]]\nname = \"state\"\nlabel = \"State\"\n", "");
    check(&Template::parse(&fixed).unwrap(), &catalog()).unwrap();
    let wrong = block("thawed").replace("[[input]]\nname = \"state\"\nlabel = \"State\"\n", "");
    has(&problems_of(&wrong), 1, Some("state"), "thawed");

    let twice = one_block(
        "[[input]]\nname = \"state\"\nlabel = \"State\"\n[[input]]\nname = \"mint\"\nlabel = \"Mint\"",
        "program = \"$token\"\ninstruction = \"initializeDefaultAccountState\"\naccounts = { mint = \"$mint\" }\narguments = { state = \"$state\" }\n[[block]]\nprogram = \"$token\"\ninstruction = \"initializeMint2\"\naccounts = { mint = \"$mint\" }\narguments = { decimals = \"$state\", mintAuthority = \"$wallet\" }",
    );
    assert!(
        problems_of(&twice)
            .iter()
            .any(|p| p.message.contains("`$state` is bound to fields")),
        "{:#?}",
        problems_of(&twice)
    );
}

#[test]
fn an_amount_scaled_by_an_unbound_argument_is_refused() {
    let text = one_block(
        "[[input]]\nname = \"amount\"\nlabel = \"Amount\"\n[values]\nm = \"new-keypair\"",
        "program = \"$token\"\ninstruction = \"mintToChecked\"\naccounts = { mint = \"$m\", token = \"$wallet\", mintAuthority = \"$wallet\" }\narguments = { amount = \"$amount\" }",
    );
    let problems = problems_of(&text);
    has(&problems, 1, Some("amount"), "Bind `decimals`");
}

#[test]
fn an_extension_the_size_table_does_not_know_is_refused() {
    let text = EXAMPLE.replace(
        "instruction = \"initializeTransferFeeConfig\"\naccounts = { mint = \"$mint\" }\narguments = { transferFeeBasisPoints = \"$fee\", maximumFee = \"$max_fee\", transferFeeConfigAuthority = \"$authority\", withdrawWithheldAuthority = \"$authority\" }",
        "instruction = \"initializePermissionedBurn\"\naccounts = { mint = \"$mint\" }\narguments = { authority = \"$fee\" }",
    ).replace("[[input]]\nname = \"max_fee\"\nlabel = \"Maximum fee\"\nhint = \"The most one transfer can be charged, in the token's base units.\"\noption = \"transfer_fee\"\n", "");
    let problems = problems_of(&text);
    has(&problems, 2, None, "do not know the extension");
}

#[test]
fn a_token_program_of_block_is_checked_against_both_token_programs() {
    let template = builtin_template("mint-tokens").unwrap().template().unwrap();
    let form = check(&template, &catalog()).unwrap();
    let amount = form.inputs.iter().find(|i| i.name == "amount").unwrap();
    assert_eq!(
        amount.kind,
        InputKind::Argument {
            kind: crate::IdlArgKind::Integer {
                signed: false,
                float: false,
                bits: 64
            },
            scale: InputScale::FromArgument {
                block: 2,
                argument: "decimals".into(),
                unit: None
            }
        }
    );

    // An instruction only Token-2022 has fails on Token, and says so once.
    let text = "format = 1\nname = \"T\"\n[[input]]\nname = \"mint\"\nlabel = \"Mint\"\n[values]\ntoken = \"token-program-of $mint\"\n[[block]]\nprogram = \"$token\"\ninstruction = \"initializeNonTransferableMint\"\naccounts = { mint = \"$mint\" }\n";
    let problems = problems_of(text);
    assert_eq!(problems.len(), 1, "{problems:#?}");
    has(&problems, 1, None, "The IDL of token has no instruction");

    // Both programs' IDLs are there.
    assert!(catalog().program_client(&TOKEN_PROGRAM_ID).is_some());
    assert!(catalog().program_client(&TOKEN_2022_PROGRAM_ID).is_some());
}

#[test]
fn every_problem_is_listed_not_just_the_first() {
    let text = EXAMPLE
        .replace("\"initializeMint2\"", "\"initializeMint3\"")
        .replace("newAccount = ", "account = ");
    let problems = problems_of(&text);
    assert!(problems.len() >= 3, "{problems:#?}");
    has(&problems, 3, None, "no instruction");
    has(&problems, 1, Some("account"), "no account");
    assert_eq!(
        problems[0].to_string(),
        "Block 1 (createAccount), account: createAccount has no account `account`."
    );
}

#[test]
fn every_builtin_template_checks_on_every_network_its_programs_run_on() {
    for builtin in &BUILTIN_TEMPLATES {
        let template = builtin
            .template()
            .unwrap_or_else(|err| panic!("{}: {err}", builtin.id));
        assert_eq!(
            Template::parse(&template.to_toml()).unwrap(),
            template,
            "{}",
            builtin.id
        );
        let mut checked_on = 0;
        for network in IdlNetwork::ALL {
            let store = crate::fixtures::shipped_catalog(network);
            let runs_here = template
                .programs
                .values()
                .chain([&TOKEN_PROGRAM_ID, &TOKEN_2022_PROGRAM_ID])
                .all(|program| store.program_client(program).is_some());
            if !runs_here {
                continue;
            }
            check(&template, &store)
                .unwrap_or_else(|problems| panic!("{} on {network}: {problems:#?}", builtin.id));
            checked_on += 1;
        }
        assert_eq!(
            checked_on,
            IdlNetwork::ALL.len(),
            "{}: Token, Token-2022, associated-token and System run everywhere",
            builtin.id
        );
    }
}

#[test]
fn builtin_ids_are_their_file_stems_and_distinct() {
    let ids: std::collections::BTreeSet<&str> =
        BUILTIN_TEMPLATES.iter().map(|builtin| builtin.id).collect();
    assert_eq!(ids.len(), BUILTIN_TEMPLATES.len());
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("templates");
    let mut stems: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
        .map(|path| path.file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    stems.sort();
    let mut listed: Vec<String> = ids.iter().map(|id| id.to_string()).collect();
    listed.sort();
    assert_eq!(stems, listed, "every file in templates/ is listed");
}
