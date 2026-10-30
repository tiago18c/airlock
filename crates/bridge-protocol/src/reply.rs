//! The desktop app's replies to the extension, as the bytes of one frame.

use base64::Engine;
use serde_json::{json, Value};

/// A successful reply carrying `result`.
pub fn result(id: &str, result: Value) -> Vec<u8> {
    encode(&json!({ "v": 1, "id": id, "result": result }))
}

/// A failed reply. `code` is what the extension branches on, `message` what
/// it shows.
pub fn error(id: &str, code: &str, message: &str) -> Vec<u8> {
    encode(&json!({
        "v": 1,
        "id": id,
        "error": { "code": code, "message": message }
    }))
}

/// The user turned the request down in the app.
pub fn rejected(id: &str) -> Vec<u8> {
    error(id, "rejected", "Rejected in Airlock.")
}

/// A `connect` answered with exactly these accounts: the 32 bytes of each
/// public key, and its label, which is left out when empty.
pub fn connected(id: &str, accounts: &[([u8; 32], String)]) -> Vec<u8> {
    let accounts: Vec<Value> = accounts
        .iter()
        .map(|(pubkey, label)| {
            let mut account = json!({ "publicKey": base64(pubkey) });
            if !label.is_empty() {
                account["label"] = json!(label);
            }
            account
        })
        .collect();
    result(id, json!({ "accounts": accounts }))
}

/// Not a reply: tells the extension what `origin`'s pages may now see, the
/// account they should act as first. Empty when the site is disconnected.
pub fn accounts_changed(origin: &str, accounts: &[([u8; 32], String)]) -> Vec<u8> {
    let Value::Object(body) =
        serde_json::from_slice::<Value>(&connected("", accounts)).unwrap_or(Value::Null)
    else {
        return Vec::new();
    };
    encode(&json!({
        "v": 1,
        "method": "accountsChanged",
        "origin": origin,
        "params": { "accounts": body.get("result").and_then(|result| result.get("accounts")) },
    }))
}

/// A `signTransaction` answered with its signed transactions, in the order
/// the site sent them.
pub fn signed(id: &str, wires: &[Vec<u8>]) -> Vec<u8> {
    let wires: Vec<String> = wires.iter().map(|wire| base64(wire)).collect();
    result(id, json!({ "signedTransactions": wires }))
}

fn base64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn encode(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap_or_else(|_| {
        br#"{"v":1,"id":"","error":{"code":"internal","message":"could not encode the reply"}}"#
            .to_vec()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(bytes: &[u8]) -> Value {
        serde_json::from_slice(bytes).unwrap()
    }

    #[test]
    fn a_connect_reply_lists_only_the_given_accounts() {
        let reply = read(&connected(
            "c1",
            &[([1; 32], "Treasury".into()), ([2; 32], String::new())],
        ));
        assert_eq!(reply["id"], "c1");
        let accounts = reply["result"]["accounts"].as_array().unwrap();
        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts[0]["label"], "Treasury");
        assert_eq!(accounts[0]["publicKey"], base64(&[1; 32]));
        assert!(accounts[1].get("label").is_none());
    }

    #[test]
    fn a_signed_reply_carries_the_wire_and_a_rejection_its_code() {
        let reply = read(&signed("s1", &[vec![0x81, 1], vec![0x81, 2]]));
        assert_eq!(reply["result"]["signedTransactions"][0], base64(&[0x81, 1]));
        assert_eq!(reply["result"]["signedTransactions"][1], base64(&[0x81, 2]));
        assert_eq!(read(&rejected("s1"))["error"]["code"], "rejected");
    }
}
