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

/// The longest origin a site's request may name.
pub const MAX_ORIGIN_LEN: usize = 255;

/// Whether `origin` is a web origin as a browser reports it: `http` or
/// `https`, `://`, a host, and a port when it is not the scheme's own, with
/// nothing else, written the one way a browser writes it (lowercase, no
/// default port, no leading zeros, IPv4 in four parts, IPv6 in brackets as
/// RFC 5952 writes it). A name in another script arrives as punycode. A
/// name ending in a dot is refused, though a browser keeps the dot: no site
/// a person means to use is written so.
pub fn is_site_origin(origin: &str) -> bool {
    if origin.len() > MAX_ORIGIN_LEN {
        return false;
    }
    let (scheme, rest) = match origin.split_once("://") {
        Some(("https", rest)) => ("https", rest),
        Some(("http", rest)) => ("http", rest),
        _ => return false,
    };
    let (host, port) = if let Some(inner) = rest.strip_prefix('[') {
        let Some((address, after)) = inner.split_once(']') else {
            return false;
        };
        let canonical = address
            .parse::<std::net::Ipv6Addr>()
            .is_ok_and(|ip| ip.to_string() == address);
        if !canonical {
            return false;
        }
        match after {
            "" => (address, None),
            _ => match after.strip_prefix(':') {
                Some(port) => (address, Some(port)),
                None => return false,
            },
        }
    } else {
        let (host, port) = match rest.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (rest, None),
        };
        if !is_host_name(host) {
            return false;
        }
        (host, port)
    };
    let default = if scheme == "https" { 443 } else { 80 };
    !host.is_empty() && port.is_none_or(|port| is_port(port, default))
}

/// A domain name or an IPv4 address, as a browser writes it in an origin.
fn is_host_name(host: &str) -> bool {
    let labels: Vec<&str> = host.split('.').collect();
    let label_ok = |label: &&str| {
        !label.is_empty()
            && label.len() <= 63
            && label
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
    };
    if !labels.iter().all(label_ok) {
        return false;
    }
    // A browser reads a name whose last label is a number as an IPv4
    // address, and writes it back as four decimal parts.
    let last = labels[labels.len() - 1];
    let numeric = last.bytes().all(|b| b.is_ascii_digit())
        || (last.starts_with("0x") && last[2..].bytes().all(|b| b.is_ascii_hexdigit()));
    if !numeric {
        return true;
    }
    labels.len() == 4
        && labels
            .iter()
            .all(|part| part.parse::<u8>().is_ok() && (part.len() == 1 || !part.starts_with('0')))
}

/// A port as a browser writes it: decimal, no leading zero, and left out
/// when it is the scheme's own.
fn is_port(port: &str, default: u16) -> bool {
    let canonical = !port.is_empty()
        && port.bytes().all(|b| b.is_ascii_digit())
        && (port.len() == 1 || !port.starts_with('0'));
    canonical && port.parse::<u16>().is_ok_and(|port| port != default)
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
    fn origins_as_a_browser_writes_them_pass() {
        for origin in [
            "https://example.com",
            "http://example.com",
            "https://app.example.com:8443",
            "http://localhost:8080",
            "http://localhost",
            "http://127.0.0.1:3000",
            "http://[::1]:8080",
            "https://[2001:db8::1]",
            "https://xn--bcher-kva.example",
            "https://my-site_1.example",
            "http://example.com:443",
            "https://example.com:80",
            "http://example.com:0",
            "http://a.b.c.d.e",
        ] {
            assert!(is_site_origin(origin), "{origin}");
        }
        let longest = format!("https://{}example12", "a.".repeat(119));
        assert_eq!(longest.len(), MAX_ORIGIN_LEN);
        assert!(is_site_origin(&longest));
    }

    #[test]
    fn anything_else_is_refused() {
        for origin in [
            "",
            "example.com",
            "https://",
            "https://:443",
            "ftp://example.com",
            "chrome-extension://abcdefghijklmnop",
            "android-app://com.example",
            "null",
            "HTTPS://example.com",
            "https://Example.com",
            "https://example.com/",
            "https://example.com/path",
            "https://example.com?q",
            "https://example.com#f",
            "https://user@example.com",
            "https://user:pw@example.com",
            "https://example.com:443",
            "http://example.com:80",
            "https://example.com:08443",
            "https://example.com:65536",
            "https://example.com:",
            "https://example.com:x",
            "https://example .com",
            " https://example.com",
            "https://example.com ",
            "https://exa\tmple.com",
            "https://example.com\n",
            "https://ex\0ample.com",
            "https://b\u{fc}cher.example",
            "https://example.com.",
            "https://example..com",
            "https://.example.com",
            "http://127.1",
            "http://127.0.0.01",
            "http://0x7f.0.0.1",
            "http://256.0.0.1",
            "http://1.2.3.4.5",
            "http://[::1",
            "http://[0:0:0:0:0:0:0:1]",
            "http://[::1]x",
            "http://[::1]:",
            "http://[nope]",
            "http://[::1]:443:1",
            "http://a:1:2",
        ] {
            assert!(!is_site_origin(origin), "{origin:?}");
        }
        let long = format!("https://{}example123", "a.".repeat(119));
        assert!(long.len() > MAX_ORIGIN_LEN);
        assert!(!is_site_origin(&long));
        let label = format!("https://{}.example", "a".repeat(64));
        assert!(!is_site_origin(&label));
    }

    #[test]
    fn request_code_takes_the_first_four_alphanumeric() {
        assert_eq!(request_code("7f3ac0de-11"), "7F3A");
        assert_eq!(request_code("ab"), "AB");
    }
}
