//! Generic types of the 0.30 spec: an argument is read in the scope it is
//! written in, and a template that expands without end, or too far, is
//! refused.

use codama_nodes_from_anchor::{root_node_from_anchor, AnchorIdlError};
use serde_json::{json, Value};

const T: &str = r#"{"generic":"T"}"#;

/// A file with `types` and one instruction whose argument `x` is `argument`.
fn idl(types: &[String], argument: &str) -> String {
    format!(
        r#"{{"address":"4vJ9JU1bJJE96FWSJKvHsmmFADCg4gpZQff4P3bkLKi",
          "metadata":{{"name":"generics","version":"0.1.0","spec":"0.1.0"}},
          "instructions":[{{"name":"run","discriminator":[1,2,3,4,5,6,7,8],"accounts":[],
            "args":[{{"name":"x","type":{argument}}}]}}],
          "types":[{}]}}"#,
        types.join(",")
    )
}

/// A struct template over the type generics `generics`, with `fields` as
/// `(name, type)`.
fn template(name: &str, generics: &[&str], fields: &[(&str, String)]) -> String {
    let generics: Vec<String> = generics
        .iter()
        .map(|generic| match generic.strip_prefix("const ") {
            Some(name) => format!(r#"{{"kind":"const","name":"{name}","type":"usize"}}"#),
            None => format!(r#"{{"kind":"type","name":"{generic}"}}"#),
        })
        .collect();
    let fields: Vec<String> = fields
        .iter()
        .map(|(name, ty)| format!(r#"{{"name":"{name}","type":{ty}}}"#))
        .collect();
    format!(
        r#"{{"name":"{name}","generics":[{}],"type":{{"kind":"struct","fields":[{}]}}}}"#,
        generics.join(","),
        fields.join(",")
    )
}

/// `name` used with the type arguments `arguments`.
fn used(name: &str, arguments: &[&str]) -> String {
    let arguments: Vec<String> = arguments
        .iter()
        .map(|argument| format!(r#"{{"kind":"type","type":{argument}}}"#))
        .collect();
    format!(
        r#"{{"defined":{{"name":"{name}","generics":[{}]}}}}"#,
        arguments.join(",")
    )
}

/// The type of the instruction's argument `x`, as Codama JSON.
fn argument_type(file: &str) -> Value {
    let root = serde_json::to_value(root_node_from_anchor(file).unwrap()).unwrap();
    let arguments = root["program"]["instructions"][0]["arguments"]
        .as_array()
        .unwrap();
    arguments
        .iter()
        .find(|argument| argument["name"] == "x")
        .unwrap()["type"]
        .clone()
}

/// The type of field `name` of a struct type.
fn field<'a>(of: &'a Value, name: &str) -> &'a Value {
    of["fields"]
        .as_array()
        .unwrap_or_else(|| panic!("not a struct: {of}"))
        .iter()
        .find(|field| field["name"] == name)
        .map(|field| &field["type"])
        .unwrap_or_else(|| panic!("no field {name} in {of}"))
}

fn number(format: &str) -> Value {
    json!({ "kind": "numberTypeNode", "format": format, "endian": "le" })
}

/// `Prices<T>` holds `Price<T>`: the inner `T` is the outer one, not itself.
#[test]
fn a_template_passes_its_parameter_on_under_the_same_name() {
    let types = [
        template("Price", &["T"], &[("min", T.into()), ("max", T.into())]),
        template(
            "Prices",
            &["T"],
            &[
                ("index", used("Price", &[T])),
                ("long", used("Price", &[T])),
            ],
        ),
    ];
    let x = argument_type(&idl(&types, &used("Prices", &[r#""u128""#])));
    for price in ["index", "long"] {
        let price = field(&x, price);
        assert_eq!(field(price, "min"), &number("u128"));
        assert_eq!(field(price, "max"), &number("u128"));
    }
}

/// `Outer<A, B>` holds `Inner<B, A>`: each argument is read where it is
/// written, so the names do not cross.
#[test]
fn arguments_are_read_in_the_scope_they_are_written_in() {
    let types = [
        template(
            "Inner",
            &["A", "B"],
            &[
                ("a", r#"{"generic":"A"}"#.into()),
                ("b", r#"{"generic":"B"}"#.into()),
            ],
        ),
        template(
            "Outer",
            &["A", "B"],
            &[(
                "inner",
                used("Inner", &[r#"{"generic":"B"}"#, r#"{"generic":"A"}"#]),
            )],
        ),
    ];
    let x = argument_type(&idl(&types, &used("Outer", &[r#""u8""#, r#""u64""#])));
    let inner = field(&x, "inner");
    assert_eq!(field(inner, "a"), &number("u64"));
    assert_eq!(field(inner, "b"), &number("u8"));
}

/// `Outer<T>` holds `Inner<Option<T>>` and a `T` of its own.
#[test]
fn an_argument_built_from_a_parameter_keeps_its_shape() {
    let types = [
        template("Inner", &["T"], &[("value", T.into())]),
        template(
            "Outer",
            &["T"],
            &[
                ("inner", used("Inner", &[r#"{"option":{"generic":"T"}}"#])),
                ("fixed", used("Inner", &[r#""u8""#])),
                ("own", T.into()),
            ],
        ),
    ];
    let x = argument_type(&idl(&types, &used("Outer", &[r#""u64""#])));
    let value = field(field(&x, "inner"), "value");
    assert_eq!(value["kind"], "optionTypeNode");
    assert_eq!(value["item"], number("u64"));
    assert_eq!(field(field(&x, "fixed"), "value"), &number("u8"));
    assert_eq!(field(&x, "own"), &number("u64"));
}

/// A const generic passed on to another template, in either form a use site
/// may write it.
#[test]
fn a_const_parameter_is_passed_on() {
    let buffer = template(
        "Buffer",
        &["const N"],
        &[("bytes", r#"{"array":["u8",{"generic":"N"}]}"#.into())],
    );
    for passed_on in [
        r#"{"kind":"const","value":"N"}"#,
        r#"{"kind":"type","type":{"generic":"N"}}"#,
    ] {
        let wrapper = format!(
            r#"{{"name":"Wrapper","generics":[{{"kind":"const","name":"N","type":"usize"}}],
              "type":{{"kind":"struct","fields":[{{"name":"buffer",
                "type":{{"defined":{{"name":"Buffer","generics":[{passed_on}]}}}}}}]}}}}"#
        );
        let argument =
            r#"{"defined":{"name":"Wrapper","generics":[{"kind":"const","value":"4"}]}}"#;
        let x = argument_type(&idl(&[buffer.clone(), wrapper], argument));
        let bytes = field(field(&x, "buffer"), "bytes");
        assert_eq!(bytes["kind"], "arrayTypeNode");
        assert_eq!(bytes["count"]["value"], 4, "{passed_on}");
    }
}

fn refused(file: &str) -> AnchorIdlError {
    root_node_from_anchor(file).expect_err("the file is refused")
}

fn is_a_limit(err: &AnchorIdlError) -> bool {
    matches!(
        err,
        AnchorIdlError::GenericExpansionLimit(_) | AnchorIdlError::TypeTooDeep(_)
    )
}

/// A template that holds itself has no end to expand to.
#[test]
fn a_template_that_holds_itself_is_refused() {
    let node = template(
        "Node",
        &["T"],
        &[
            ("value", T.into()),
            ("next", format!(r#"{{"option":{}}}"#, used("Node", &[T]))),
        ],
    );
    let err = refused(&idl(&[node], &used("Node", &[r#""u8""#])));
    assert!(is_a_limit(&err), "{err}");
}

/// Two templates that hold each other, the same.
#[test]
fn templates_that_hold_each_other_are_refused() {
    let types = [
        template("Ping", &["T"], &[("pong", used("Pong", &[T]))]),
        template("Pong", &["T"], &[("ping", used("Ping", &[T]))]),
    ];
    let err = refused(&idl(&types, &used("Ping", &[r#""u8""#])));
    assert!(is_a_limit(&err), "{err}");
}

/// Each template uses the next twice, so the tree doubles at each level:
/// fourteen levels are 16,384 expansions, within the depth a type may nest.
#[test]
fn templates_that_double_at_each_level_are_refused() {
    let levels = 14;
    let mut types: Vec<String> = (0..levels)
        .map(|level| {
            let next = format!("Level{}", level + 1);
            template(
                &format!("Level{level}"),
                &["T"],
                &[("a", used(&next, &[T])), ("b", used(&next, &[T]))],
            )
        })
        .collect();
    types.push(template(
        &format!("Level{levels}"),
        &["T"],
        &[("v", T.into())],
    ));
    let err = refused(&idl(&types, &used("Level0", &[r#""u8""#])));
    assert!(
        matches!(err, AnchorIdlError::GenericExpansionLimit(_)),
        "{err}"
    );
}

/// Each template passes on a pair of its own parameter, so the argument
/// doubles at each level without one more expansion.
#[test]
fn arguments_that_double_at_each_level_are_refused() {
    let levels = 15;
    let pair = used("Pair", &[T, T]);
    let mut types = vec![template(
        "Pair",
        &["A", "B"],
        &[
            ("a", r#"{"generic":"A"}"#.into()),
            ("b", r#"{"generic":"B"}"#.into()),
        ],
    )];
    types.extend((0..levels).map(|level| {
        template(
            &format!("Level{level}"),
            &["T"],
            &[("next", used(&format!("Level{}", level + 1), &[&pair]))],
        )
    }));
    types.push(template(
        &format!("Level{levels}"),
        &["T"],
        &[("v", T.into())],
    ));
    let err = refused(&idl(&types, &used("Level0", &[r#""u8""#])));
    assert!(is_a_limit(&err), "{err}");
}

/// A template of `fields` plain fields, used `uses` times as an instruction's
/// arguments: few expansions, each of them wide.
fn wide(fields: usize, uses: usize) -> String {
    let fields: Vec<(String, String)> = (0..fields)
        .map(|at| (format!("f{at}"), r#""u8""#.to_string()))
        .collect();
    let fields: Vec<(&str, String)> = fields
        .iter()
        .map(|(name, ty)| (name.as_str(), ty.clone()))
        .collect();
    let arguments: Vec<String> = (0..uses)
        .map(|at| {
            format!(
                r#"{{"name":"a{at}","type":{}}}"#,
                used("Wide", &[r#""u8""#])
            )
        })
        .collect();
    format!(
        r#"{{"address":"4vJ9JU1bJJE96FWSJKvHsmmFADCg4gpZQff4P3bkLKi",
          "metadata":{{"name":"generics","version":"0.1.0","spec":"0.1.0"}},
          "instructions":[{{"name":"run","discriminator":[1,2,3,4,5,6,7,8],"accounts":[],
            "args":[{}]}}],
          "types":[{}]}}"#,
        arguments.join(","),
        template("Wide", &["T"], &fields)
    )
}

/// The expansions are few enough, but what they make together is not: a
/// file of 100 KiB that would hold a million fields is refused, and one that
/// holds a tenth of the limit converts.
#[test]
fn templates_that_expand_to_too_many_fields_are_refused() {
    let fits = root_node_from_anchor(&wide(100, 100)).unwrap();
    let root = serde_json::to_value(fits).unwrap();
    assert_eq!(
        root["program"]["instructions"][0]["arguments"]
            .as_array()
            .unwrap()
            .len(),
        // The discriminator, then the hundred arguments.
        101
    );

    let err = refused(&wide(1_000, 1_000));
    assert!(
        matches!(err, AnchorIdlError::GenericExpansionLimit(_)),
        "{err}"
    );
}

/// A chain of templates, each holding the next, converts up to the depth a
/// type may nest and is refused past it.
#[test]
fn templates_nest_as_deep_as_a_type_may_and_no_deeper() {
    let chain = |levels: usize| {
        let mut types: Vec<String> = (0..levels - 1)
            .map(|level| {
                template(
                    &format!("Level{level}"),
                    &["T"],
                    &[("v", used(&format!("Level{}", level + 1), &[T]))],
                )
            })
            .collect();
        types.push(template(
            &format!("Level{}", levels - 1),
            &["T"],
            &[("v", T.into())],
        ));
        idl(&types, &used("Level0", &[r#""u8""#]))
    };
    let mut innermost = argument_type(&chain(15));
    for _ in 0..15 {
        innermost = field(&innermost, "v").clone();
    }
    assert_eq!(innermost, number("u8"));
    assert!(matches!(
        refused(&chain(16)),
        AnchorIdlError::TypeTooDeep(_)
    ));
}
