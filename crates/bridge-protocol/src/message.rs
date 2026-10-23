//! Error envelopes the host can build without understanding the request body.

use serde_json::{json, Value};

pub fn error_reply(request: &[u8], code: &str, message: &str) -> Vec<u8> {
    let id = serde_json::from_slice::<Value>(request)
        .ok()
        .and_then(|value| value.get("id").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_default();
    serde_json::to_vec(&json!({
        "v": 1,
        "id": id,
        "error": { "code": code, "message": message }
    }))
    .unwrap_or_else(|_| {
        br#"{"v":1,"id":"","error":{"code":"internal","message":"could not encode the error"}}"#
            .to_vec()
    })
}

pub fn dev_stub_reply(request: &[u8]) -> Vec<u8> {
    let value = serde_json::from_slice::<Value>(request).unwrap_or(Value::Null);
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let method = value.get("method").and_then(Value::as_str).unwrap_or("");
    let body = if method == "status" {
        json!({
            "v": 1,
            "id": id,
            "result": { "ready": true, "role": "dev-stub" }
        })
    } else {
        json!({
            "v": 1,
            "id": id,
            "error": {
                "code": "not_implemented",
                "message": "bridge-host listen only answers status. Approval happens in the Airlock desktop app."
            }
        })
    };
    serde_json::to_vec(&body)
        .unwrap_or_else(|_| error_reply(request, "internal", "could not encode the reply"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_keeps_the_request_id() {
        let reply = error_reply(
            br#"{"v":1,"id":"abc","method":"connect"}"#,
            "app_not_running",
            "The Airlock desktop app is not running.",
        );
        let value: Value = serde_json::from_slice(&reply).unwrap();
        assert_eq!(value["id"], "abc");
        assert_eq!(value["error"]["code"], "app_not_running");
    }

    #[test]
    fn dev_stub_answers_status_and_rejects_signing() {
        let status = dev_stub_reply(br#"{"v":1,"id":"s","method":"status"}"#);
        let value: Value = serde_json::from_slice(&status).unwrap();
        assert_eq!(value["result"]["role"], "dev-stub");

        let sign = dev_stub_reply(br#"{"v":1,"id":"t","method":"signTransaction"}"#);
        let value: Value = serde_json::from_slice(&sign).unwrap();
        assert_eq!(value["error"]["code"], "not_implemented");
    }
}
