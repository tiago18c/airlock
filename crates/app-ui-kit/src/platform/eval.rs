//! One door for running JavaScript in the page, so every call is logged.

use dioxus::prelude::document;

/// Run `script` in the page. Logs at `trace` under `perf` (T1).
pub fn eval(script: impl AsRef<str>) -> document::Eval {
    let script = script.as_ref();
    tracing::trace!(
        target: "perf",
        script_len = script.len(),
        "eval"
    );
    document::eval(script)
}

/// Copy `text` to the clipboard. Falls back to a throwaway field when the
/// clipboard API is refused (insecure context).
pub fn copy_text(text: &str) -> document::Eval {
    let quoted = serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into());
    // The clipboard API needs a secure page. Where it is refused,
    // fall back to selecting the text in a throwaway field.
    eval(format!(
        r#"const text = {quoted};
        try {{ await navigator.clipboard.writeText(text); }}
        catch (err) {{
            const field = document.createElement("textarea");
            field.value = text;
            document.body.appendChild(field);
            field.select();
            document.execCommand("copy");
            field.remove();
        }}"#
    ))
}
