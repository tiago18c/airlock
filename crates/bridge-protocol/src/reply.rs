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

/// An account a site is shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SiteAccount {
    pub pubkey: [u8; 32],
    /// Left out of the reply when empty.
    pub label: String,
    /// A key on this device can sign a message as this account. No key signs
    /// as a programmatic signer or a Squads vault.
    pub signs_messages: bool,
}

fn accounts_json(accounts: &[SiteAccount]) -> Vec<Value> {
    accounts
        .iter()
        .map(|site| {
            let mut account = json!({ "publicKey": base64(&site.pubkey) });
            if !site.label.is_empty() {
                account["label"] = json!(site.label);
            }
            if site.signs_messages {
                account["signsMessages"] = json!(true);
            }
            account
        })
        .collect()
}

/// A `connect` answered with exactly these accounts.
pub fn connected(id: &str, accounts: &[SiteAccount]) -> Vec<u8> {
    result(id, json!({ "accounts": accounts_json(accounts) }))
}

/// A `signAndSendTransaction` answered with the signature of each transaction
/// sent, in the order the site sent them.
pub fn sent(id: &str, signatures: &[[u8; 64]]) -> Vec<u8> {
    let signatures: Vec<String> = signatures.iter().map(|one| base64(one)).collect();
    result(id, json!({ "signatures": signatures }))
}

/// A `signAndSendAllTransactions` answered, one result for each transaction:
/// its signature, or why it did not go through.
pub fn sent_each(id: &str, results: &[Result<[u8; 64], String>]) -> Vec<u8> {
    let results: Vec<Value> = results
        .iter()
        .map(|one| match one {
            Ok(signature) => json!({ "signature": base64(signature) }),
            Err(message) => json!({ "error": { "code": "send_failed", "message": message } }),
        })
        .collect();
    result(id, json!({ "results": results }))
}

/// The user approved sending, and it did not go through.
pub fn send_failed(id: &str, message: &str) -> Vec<u8> {
    error(id, "send_failed", message)
}

/// A `signMessage` answered: the bytes signed, and the signature over them.
pub fn message_signed(id: &str, signed: &[u8], signature: &[u8; 64]) -> Vec<u8> {
    result(
        id,
        json!({ "signedMessage": base64(signed), "signature": base64(signature) }),
    )
}

/// A `signIn` answered: `account` signed `signed`, and `accounts` are what the
/// site may see now, `account` first.
pub fn signed_in(
    id: &str,
    account: &[u8; 32],
    accounts: &[SiteAccount],
    signed: &[u8],
    signature: &[u8; 64],
) -> Vec<u8> {
    result(
        id,
        json!({
            "account": base64(account),
            "accounts": accounts_json(accounts),
            "signedMessage": base64(signed),
            "signature": base64(signature),
        }),
    )
}

/// Not a reply: tells the extension what `origin`'s pages may now see, the
/// account they should act as first. Empty when the site is disconnected.
pub fn accounts_changed(origin: &str, accounts: &[SiteAccount]) -> Vec<u8> {
    encode(&json!({
        "v": 1,
        "method": "accountsChanged",
        "origin": origin,
        "params": { "accounts": accounts_json(accounts) },
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
            &[
                SiteAccount {
                    pubkey: [1; 32],
                    label: "Treasury".into(),
                    signs_messages: true,
                },
                SiteAccount {
                    pubkey: [2; 32],
                    label: String::new(),
                    signs_messages: false,
                },
            ],
        ));
        assert_eq!(reply["id"], "c1");
        let accounts = reply["result"]["accounts"].as_array().unwrap();
        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts[0]["label"], "Treasury");
        assert_eq!(accounts[0]["publicKey"], base64(&[1; 32]));
        assert!(accounts[1].get("label").is_none());
        assert_eq!(accounts[0]["signsMessages"], true);
        assert!(accounts[1].get("signsMessages").is_none());
    }

    #[test]
    fn a_signed_reply_carries_the_wire_and_a_rejection_its_code() {
        let reply = read(&signed("s1", &[vec![0x81, 1], vec![0x81, 2]]));
        assert_eq!(reply["result"]["signedTransactions"][0], base64(&[0x81, 1]));
        assert_eq!(reply["result"]["signedTransactions"][1], base64(&[0x81, 2]));
        assert_eq!(read(&rejected("s1"))["error"]["code"], "rejected");
    }
}
