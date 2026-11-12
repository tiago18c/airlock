//! Writing a template back to its file, as [`Template::parse`] reads it.
//!
//! The text is the same for the same template: keys in a fixed order, tables
//! sorted by key, one blank line between sections.

use super::model::*;
use std::collections::BTreeMap;
use std::fmt::Write;

impl Template {
    /// The file [`Template::parse`] reads back to this template.
    pub fn to_toml(&self) -> String {
        let mut out = String::new();
        line(&mut out, "format", "1");
        line(&mut out, "name", &string(&self.name));
        if let Some(summary) = &self.summary {
            line(&mut out, "summary", &string(summary));
        }

        if !self.programs.is_empty() {
            out.push_str("\n[programs]\n");
            for (alias, address) in &self.programs {
                line(&mut out, alias, &string(&address.to_string()));
            }
        }

        for option in &self.options {
            out.push_str("\n[[option]]\n");
            line(&mut out, "name", &string(&option.name));
            line(&mut out, "label", &string(&option.label));
            if let Some(hint) = &option.hint {
                line(&mut out, "hint", &string(hint));
            }
            if option.default {
                line(&mut out, "default", "true");
            }
        }

        for input in &self.inputs {
            out.push_str("\n[[input]]\n");
            line(&mut out, "name", &string(&input.name));
            line(&mut out, "label", &string(&input.label));
            if let Some(hint) = &input.hint {
                line(&mut out, "hint", &string(hint));
            }
            if input.optional {
                line(&mut out, "optional", "true");
            }
            if let Some(default) = &input.default {
                line(&mut out, "default", &string(&default.text()));
            }
            if let Some(option) = &input.option {
                line(&mut out, "option", &string(option));
            }
        }

        if !self.values.is_empty() {
            out.push_str("\n[values]\n");
            for (name, call) in &self.values {
                let mut text = call.function.name().to_string();
                for arg in &call.args {
                    text.push(' ');
                    text.push_str(&arg.text());
                }
                line(&mut out, name, &string(&text));
            }
        }

        for block in &self.blocks {
            out.push_str("\n[[block]]\n");
            if let Some(when) = &block.when {
                line(&mut out, "when", &string(when));
            }
            line(&mut out, "program", &string(&format!("${}", block.program)));
            line(&mut out, "instruction", &string(&block.instruction));
            if !block.accounts.is_empty() {
                line(&mut out, "accounts", &inline(&block.accounts));
            }
            if !block.arguments.is_empty() {
                line(&mut out, "arguments", &inline(&block.arguments));
            }
        }
        out
    }
}

fn line(out: &mut String, key: &str, value: &str) {
    let _ = writeln!(out, "{} = {value}", bare_or_quoted(key));
}

/// A TOML basic string, escaped by the TOML writer.
fn string(text: &str) -> String {
    toml::Value::String(text.to_string()).to_string()
}

fn bare_or_quoted(key: &str) -> String {
    let bare = !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if bare {
        key.to_string()
    } else {
        string(key)
    }
}

fn inline(fields: &BTreeMap<String, Term>) -> String {
    let body: Vec<String> = fields
        .iter()
        .map(|(field, term)| format!("{} = {}", bare_or_quoted(field), string(&term.text())))
        .collect();
    format!("{{ {} }}", body.join(", "))
}
