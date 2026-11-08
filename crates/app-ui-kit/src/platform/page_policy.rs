//! The page's Content Security Policy, which every shell that hosts the app in a
//! WebView puts in its head: desktop and Android.

/// What the page may load, and where it may send anything: the app's own files, and the
/// websocket on the loopback that Dioxus sends it edits on, on a port it picks at launch.
/// A window that shows what strangers wrote should run nothing they wrote, and have as
/// few ways as can be managed of sending anything out. `docs/browser-bridge.md` has what
/// this leaves.
///
/// `'unsafe-inline'` is only for starting: Dioxus's start-up script is inline, with a key
/// made at each launch, so it cannot be named. [`AFTER_START_UP`] takes it away. Its
/// `eval` builds a function from text, so `'unsafe-eval'` stays.
pub const CONTENT_POLICY: &str = "default-src 'none'; \
    script-src 'self' 'unsafe-inline' 'unsafe-eval'; \
    style-src 'self' 'unsafe-inline'; \
    img-src 'self' data: blob:; \
    font-src 'self' data:; \
    media-src 'self' blob: mediastream:; \
    connect-src 'self' ws://127.0.0.1:*; \
    base-uri 'none'; \
    form-action 'none'";

/// Added to the page once its own scripts have been read, and before anything is drawn.
/// Policies add up, so from then on no inline script, handler or speculation rule runs.
pub const AFTER_START_UP: &str = "script-src 'self' 'unsafe-eval'";

/// The page's policy, and what adds [`AFTER_START_UP`] to it. Dioxus's scripts are read
/// before `DOMContentLoaded`, and it draws nothing until `load`.
pub fn page_head() -> String {
    format!(
        r#"<meta http-equiv="Content-Security-Policy" content="{CONTENT_POLICY}">
<script>
addEventListener("DOMContentLoaded", () => {{
    const policy = document.createElement("meta");
    policy.httpEquiv = "Content-Security-Policy";
    policy.content = "{AFTER_START_UP}";
    document.head.append(policy);
}});
</script>"#
    )
}

#[cfg(test)]
mod tests {
    use super::{page_head, AFTER_START_UP, CONTENT_POLICY};

    /// Each directive, and the places it allows.
    fn directives() -> Vec<(&'static str, Vec<&'static str>)> {
        CONTENT_POLICY
            .split(';')
            .map(|directive| {
                let mut words = directive.split_whitespace();
                (words.next().unwrap(), words.collect())
            })
            .collect()
    }

    #[test]
    fn nothing_is_allowed_unless_it_is_named_and_nothing_named_is_elsewhere() {
        let directives = directives();
        assert_eq!(directives[0], ("default-src", vec!["'none'"]));
        for (name, sources) in &directives {
            for source in sources {
                let here = source.starts_with('\'')
                    || ["data:", "blob:", "mediastream:", "ws://127.0.0.1:*"].contains(source);
                assert!(here, "{name} allows {source}");
            }
        }
    }

    #[test]
    fn what_could_carry_something_out_is_named_and_held_to_this_device() {
        let directives = directives();
        let of = |wanted: &str| {
            directives
                .iter()
                .find(|(name, _)| *name == wanted)
                .unwrap_or_else(|| panic!("{wanted}"))
                .1
                .clone()
        };
        assert_eq!(of("connect-src"), ["'self'", "ws://127.0.0.1:*"]);
        assert_eq!(of("form-action"), ["'none'"]);
        assert_eq!(of("base-uri"), ["'none'"]);
        assert!(!of("img-src").iter().any(|source| source.contains("http")));
    }

    #[test]
    fn inline_scripts_run_only_until_the_page_has_been_read() {
        assert!(!AFTER_START_UP.contains("unsafe-inline"));
        let head = page_head();
        let at_first = head.find(CONTENT_POLICY).unwrap();
        let waits = head.find("DOMContentLoaded").unwrap();
        let after = head.find(AFTER_START_UP).unwrap();
        assert!(at_first < waits && waits < after);
    }
}
