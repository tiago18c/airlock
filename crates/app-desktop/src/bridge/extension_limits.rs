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

/// What `const NAME = …;`, or `const NAME: Type = …;`, is set to in
/// `source`, as written.
fn written<'a>(source: &'a str, name: &str) -> &'a str {
    let start = format!("const {name}");
    let from = source
        .match_indices(&start)
        .map(|(at, _)| at + start.len())
        .find(|after| matches!(source[*after..].chars().next(), Some(' ' | ':')))
        .unwrap_or_else(|| panic!("protocol.ts has no constant {name}"));
    let rest = &source[from..];
    let rest = &rest[rest.find("= ").expect("a constant is set") + 2..];
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
fn the_extension_offers_the_intent_methods_the_app_answers_and_waits_as_it_does() {
    let source = protocol_ts();
    let methods = strings(&source, "INTENT_METHODS");
    let ours: Vec<&str> = super::intents::INTENT_METHODS.to_vec();
    assert_eq!(methods, ours);
    // None of them is one the app holds to its older handlers.
    let older = strings(&source, "METHODS");
    assert!(methods.iter().all(|method| !older.contains(method)));
    for method in &methods {
        assert!(super::intents::is_intent_method(method), "{method}");
        assert!(!waits_for_user(method), "{method}");
    }
    // What the extension gives a short time is what the app answers at
    // once; the rest may wait for the person, and gets the long one.
    let at_once = strings(&source, "ANSWERED_AT_ONCE");
    for method in &methods {
        assert_eq!(
            at_once.contains(method),
            !super::intents::may_wait(method),
            "{method}"
        );
    }
    assert!(at_once.iter().all(|method| methods.contains(method)));
}

#[test]
fn the_extension_passes_on_the_pushes_the_app_sends() {
    let pushes = strings(&protocol_ts(), "PUSH_METHODS");
    let sent: Vec<String> = [
        bridge_protocol::reply::accounts_changed("https://a.test", &[]),
        bridge_protocol::reply::resolution_changed("https://a.test", serde_json::json!({})),
    ]
    .iter()
    .map(|frame| {
        let frame: serde_json::Value = serde_json::from_slice(frame).unwrap();
        frame["method"].as_str().unwrap().to_owned()
    })
    .collect();
    assert_eq!(pushes, sent);
}

#[test]
fn a_constant_is_read_as_it_is_written() {
    let source = "const A = 4;\nexport const B = A * 10 * 1000;\nexport const L = [\"x\", \"y\"] as const;\nconst LL: readonly string[] = [\"z\"];";
    assert_eq!(number(source, "A"), 4);
    assert_eq!(number(source, "B"), 40_000);
    assert_eq!(strings(source, "L"), ["x", "y"]);
    assert_eq!(strings(source, "LL"), ["z"]);
}
