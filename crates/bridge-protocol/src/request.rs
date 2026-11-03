//! Fields shared by a bridge request the extension and the desktop app both see.

use serde_json::Value;

/// The most transactions one `signTransaction` may carry. The extension
/// refuses more before asking, and the desktop app answers more with
/// `not_implemented`.
pub const MAX_TRANSACTIONS: usize = 16;

/// The most requests one site may have waiting for the user. The extension
/// refuses more before asking, and the desktop app refuses them again, since
/// several browsers can ask for one site.
pub const MAX_PENDING_PER_ORIGIN: usize = 8;

/// The most requests waiting for the user, from every site together.
pub const MAX_PENDING: usize = 32;

/// One frame from the extension, read once. A field that is missing or not of
/// its type reads as empty.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub version: Option<u64>,
    pub id: String,
    pub origin: String,
    pub expires_at: Option<u64>,
    pub method: String,
    pub params: Value,
}

impl Frame {
    /// `None` when the bytes are not JSON.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let mut value: Value = serde_json::from_slice(bytes).ok()?;
        let text = |field| {
            value
                .get(field)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned()
        };
        let (id, origin, method) = (text("id"), text("origin"), text("method"));
        Some(Self {
            version: value.get("v").and_then(Value::as_u64),
            id,
            origin,
            expires_at: value.get("expiresAt").and_then(Value::as_u64),
            method,
            params: value.get_mut("params").map_or(Value::Null, Value::take),
        })
    }
}

/// The four characters shown beside a request, so it can be matched with the
/// one the extension lists.
pub fn request_code(id: &str) -> String {
    id.chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .take(4)
        .collect::<String>()
        .to_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_is_read_field_by_field() {
        let frame = Frame::parse(
            br#"{"v":1,"id":"a","origin":"https://x","expiresAt":9,"method":"connect","params":{"silent":true}}"#,
        )
        .unwrap();
        assert_eq!(frame.version, Some(1));
        assert_eq!(frame.id, "a");
        assert_eq!(frame.origin, "https://x");
        assert_eq!(frame.expires_at, Some(9));
        assert_eq!(frame.method, "connect");
        assert_eq!(frame.params["silent"], true);
    }

    #[test]
    fn what_a_frame_lacks_reads_as_empty() {
        for lacking in [
            &br#"{}"#[..],
            br#"{"v":"1","id":7,"method":[],"origin":null}"#,
            b"[]",
        ] {
            let frame = Frame::parse(lacking).unwrap();
            assert_eq!(frame.version, None);
            assert_eq!(frame.id, "");
            assert_eq!(frame.origin, "");
            assert_eq!(frame.method, "");
            assert_eq!(frame.params, Value::Null);
        }
        assert!(Frame::parse(b"not json").is_none());
    }

    #[test]
    fn request_code_takes_the_first_four_alphanumeric() {
        assert_eq!(request_code("7f3ac0de-11"), "7F3A");
        assert_eq!(request_code("ab"), "AB");
    }
}
