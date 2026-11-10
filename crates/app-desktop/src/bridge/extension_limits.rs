//! The extension states the app's limits again in `extension/src/protocol.ts`,
//! in another language, so nothing but these tests holds the two together.
//! A limit changed on one side fails here until the other follows.

use std::collections::BTreeMap;
use std::path::Path;

use super::{waits_for_user, DECISION_TIMEOUT};
use bridge_protocol::extension_id::HOST_NAME;
use bridge_protocol::frame::MAX_MESSAGE;
use bridge_protocol::{MAX_PENDING, MAX_PENDING_PER_ORIGIN, MAX_TRANSACTIONS};

fn protocol_ts() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../extension/src/protocol.ts");
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()))
}

/// What `const NAME = …;` is set to in `source`, as written.
fn written<'a>(source: &'a str, name: &str) -> &'a str {
    let start = format!("const {name} = ");
    let from = source
        .find(&start)
        .unwrap_or_else(|| panic!("protocol.ts has no constant {name}"))
        + start.len();
    let rest = &source[from..];
    rest[..rest.find(';').expect("a constant ends in a semicolon")].trim()
}

/// A constant that is a product of whole numbers and of constants before it.
fn number(source: &str, name: &str) -> u64 {
    written(source, name)
        .split('*')
        .map(|factor| {
            let factor = factor.trim();
            factor.parse().unwrap_or_else(|_| number(source, factor))
        })
        .product()
}

/// A constant that is a list of strings.
fn strings(source: &str, name: &str) -> Vec<String> {
    let list = written(source, name);
    let inside = &list[list.find('[').unwrap() + 1..list.rfind(']').unwrap()];
    inside
        .split(',')
        .map(|item| item.trim().trim_matches('"').to_owned())
        .filter(|item| !item.is_empty())
        .collect()
}

#[test]
fn the_extension_states_the_limits_the_app_holds() {
    let source = protocol_ts();
    let theirs: BTreeMap<&str, u64> = [
        "MAX_TRANSACTIONS",
        "MAX_PENDING_PER_ORIGIN",
        "MAX_PENDING",
        "REQUEST_TTL_MS",
    ]
    .into_iter()
    .map(|name| (name, number(&source, name)))
    .collect();
    let ours: BTreeMap<&str, u64> = [
        ("MAX_TRANSACTIONS", MAX_TRANSACTIONS as u64),
        ("MAX_PENDING_PER_ORIGIN", MAX_PENDING_PER_ORIGIN as u64),
        ("MAX_PENDING", MAX_PENDING as u64),
        ("REQUEST_TTL_MS", DECISION_TIMEOUT.as_millis() as u64),
    ]
    .into();
    assert_eq!(theirs, ours);
}

#[test]
fn a_request_the_extension_lets_through_fits_in_a_frame() {
    let source = protocol_ts();
    // The host ends on a frame it cannot read, for every site at once. The
    // envelope around the params is a few hundred bytes.
    let largest = number(&source, "MAX_REQUEST_BYTES");
    assert!(largest + 4096 <= MAX_MESSAGE as u64, "{largest}");
}

#[test]
fn the_extension_starts_the_host_by_the_name_it_is_registered_under() {
    assert_eq!(
        written(&protocol_ts(), "HOST_NAME"),
        format!("\"{HOST_NAME}\"")
    );
}

#[test]
fn the_extension_offers_only_methods_the_app_answers() {
    let methods = strings(&protocol_ts(), "METHODS");
    assert!(methods.len() >= 8, "{methods:?}");
    for method in &methods {
        let answered = waits_for_user(method) || matches!(method.as_str(), "status" | "disconnect");
        assert!(answered, "the app has no handler for {method}");
    }
    // And each one that waits for the user is one a site can reach.
    for method in [
        "connect",
        "signTransaction",
        app_core::bridge::SEND,
        app_core::bridge::SEND_ALL,
        "signMessage",
        "signIn",
    ] {
        assert!(waits_for_user(method));
        assert!(methods.iter().any(|known| known == method), "{method}");
    }
}

#[test]
fn a_constant_is_read_as_it_is_written() {
    let source =
        "const A = 4;\nexport const B = A * 10 * 1000;\nexport const L = [\"x\", \"y\"] as const;";
    assert_eq!(number(source, "A"), 4);
    assert_eq!(number(source, "B"), 40_000);
    assert_eq!(strings(source, "L"), ["x", "y"]);
}
