//! The colour theme: which palette tokens.css applies, kept in `localStorage`.

use dioxus::prelude::*;

/// Which palette tokens.css applies. System follows the OS setting.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    System,
    Light,
    Dark,
}

impl Theme {
    pub(crate) const ALL: [Theme; 3] = [Theme::System, Theme::Light, Theme::Dark];

    pub(crate) fn value(self) -> &'static str {
        match self {
            Theme::System => "system",
            Theme::Light => "light",
            Theme::Dark => "dark",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Theme::System => "System",
            Theme::Light => "Light",
            Theme::Dark => "Dark",
        }
    }

    fn parse(raw: &str) -> Self {
        match raw {
            "light" => Theme::Light,
            "dark" => Theme::Dark,
            _ => Theme::System,
        }
    }
}

const THEME_KEY: &str = "airlock.theme";

/// Reads the stored choice once and pins it on `<html>`. Call from the app root.
pub fn use_theme() -> Signal<Theme> {
    let mut theme = use_signal(|| Theme::System);
    use_future(move || async move {
        let stored = crate::platform::eval(format!(
            r#"
            let value = null;
            try {{ value = localStorage.getItem("{THEME_KEY}"); }} catch (_) {{}}
            if (value === "light" || value === "dark") {{
                document.documentElement.dataset.theme = value;
            }}
            return value;
            "#
        ))
        .await;
        if let Ok(value) = stored {
            theme.set(Theme::parse(value.as_str().unwrap_or_default()));
        }
    });
    use_context_provider(|| theme)
}

/// Applies `theme` to `<html>` and stores it.
pub(crate) fn apply(theme: Theme) {
    let value = theme.value();
    let _ = crate::platform::eval(format!(
        r#"
        if ("{value}" === "system") {{
            delete document.documentElement.dataset.theme;
        }} else {{
            document.documentElement.dataset.theme = "{value}";
        }}
        try {{ localStorage.setItem("{THEME_KEY}", "{value}"); }} catch (_) {{}}
        "#
    ));
}
