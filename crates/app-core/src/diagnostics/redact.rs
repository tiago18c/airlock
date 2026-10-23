//! Strip credentials from URLs before they go into a log line.

use url::Url;

/// Scheme, host, port and path only. User info and the query string are dropped
/// (many RPC providers put an API key there).
pub fn redact_url(raw: &str) -> String {
    let Ok(mut url) = Url::parse(raw) else {
        return "(unparseable url)".into();
    };
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    url.into()
}

#[cfg(test)]
mod tests {
    use super::redact_url;

    #[test]
    fn drops_userinfo_and_query() {
        assert_eq!(
            redact_url("https://user:pass@api.example.com:443/rpc?api-key=SMOKE-KEY"),
            "https://api.example.com/rpc"
        );
    }

    #[test]
    fn keeps_scheme_host_port_path() {
        assert_eq!(
            redact_url("http://127.0.0.1:8899/"),
            "http://127.0.0.1:8899/"
        );
    }

    #[test]
    fn plain_url_unchanged() {
        assert_eq!(
            redact_url("https://api.devnet.solana.com/"),
            "https://api.devnet.solana.com/"
        );
    }
}
