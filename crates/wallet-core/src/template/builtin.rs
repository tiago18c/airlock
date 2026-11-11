//! The templates that ship with the app, compiled into the binary from
//! `templates/*.toml`.

use super::model::Template;
use super::parse::TemplateError;

/// One template that ships with the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuiltinTemplate {
    /// The file's stem. Stable across versions, so a choice can be kept.
    pub id: &'static str,
    pub toml: &'static str,
}

impl BuiltinTemplate {
    /// The template, read from its file. The tests read every one, so this
    /// fails only if a file was changed without running them.
    pub fn template(&self) -> Result<Template, TemplateError> {
        Template::parse(self.toml)
    }
}

/// In the order the picker lists them.
pub static BUILTIN_TEMPLATES: [BuiltinTemplate; 3] = [
    BuiltinTemplate {
        id: "create-token",
        toml: include_str!("../../templates/create-token.toml"),
    },
    BuiltinTemplate {
        id: "create-token-2022",
        toml: include_str!("../../templates/create-token-2022.toml"),
    },
    BuiltinTemplate {
        id: "mint-tokens",
        toml: include_str!("../../templates/mint-tokens.toml"),
    },
];

/// The text **New template** starts from: a short template that reads and
/// checks on every network, with comments that say what each part is. It is
/// not shipped as a template: it is only a first text to change.
pub const STARTER_TEMPLATE: &str = include_str!("../../templates/starter/new-template.toml");

/// The built-in template with this id.
pub fn builtin_template(id: &str) -> Option<&'static BuiltinTemplate> {
    BUILTIN_TEMPLATES.iter().find(|builtin| builtin.id == id)
}
