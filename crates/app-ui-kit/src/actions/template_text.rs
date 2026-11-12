//! Writing a template as text, in the Templates dialog: what the editor is
//! writing, reading its text as it is typed, and keeping it.
//!
//! Rule 3 holds at the editor: nothing is kept before the person could see
//! what it does. [`read_text`] reads the text as it stands into what the
//! editor shows beside it, on the same screen as Save: why it cannot be kept
//! (too large, or not a template, with the line), else the reader's
//! Questions and Blocks for it, every problem it has on this network, and
//! what its name means (free, its own, a shipped one's, or another of the
//! user's that saving replaces). [`save_text`] keeps the text exactly as it
//! is, through the same checked write as every other template.

use super::kept_templates::{keep, kept_under, refuse_builtin_name, remove_kept};
use super::template_browser::{template_view, TemplateView};
use crate::state::forms::TemplateSource;
pub use crate::state::forms::{EditTarget, TextEdit};
use crate::state::session::Session;
use dioxus::prelude::*;
use std::sync::Arc;
use wallet_core::template::{template_slug, TemplateError, MAX_TEMPLATE_BYTES, STARTER_TEMPLATE};
use wallet_core::Template;

/// The editor on the starter text, for **New template**.
pub fn new_template() -> TextEdit {
    TextEdit::open(EditTarget::New, STARTER_TEMPLATE.to_string())
}

/// The editor on one of the user's, for **Edit**: a template, or a file
/// that is not one yet, to mend. `None` for one of the shipped ones, and for
/// a file whose text could not be read.
pub fn edit_kept(source: &TemplateSource) -> Option<TextEdit> {
    let slug = source.slug()?;
    let text = source.text()?;
    let name = source
        .template()
        .map(|template| template.name)
        .unwrap_or_else(|_| format!("{slug}.toml"));
    Some(TextEdit::open(
        EditTarget::Edit {
            slug: slug.to_string(),
            name,
        },
        text.to_string(),
    ))
}

/// The editor on the text of any template, tied to no kept file, for
/// **Edit a copy**. Its name is then taken, and the editor says so until it
/// is changed.
pub fn edit_copy(source: &TemplateSource) -> Option<TextEdit> {
    let template = source.template().ok()?;
    let text = source.text()?;
    Some(TextEdit::open(
        EditTarget::Copy {
            name: template.name,
        },
        text.to_string(),
    ))
}

/// The editor on a file picked for import, `file` by name: its text, not
/// kept yet. A file that is not text, or is larger than a template can be,
/// opens nothing and says why.
pub fn import_text(file: &str, bytes: Vec<u8>) -> Result<TextEdit, String> {
    if bytes.len() > MAX_TEMPLATE_BYTES {
        return Err(format!(
            "{file} is {} bytes. A template can be at most {MAX_TEMPLATE_BYTES}.",
            bytes.len()
        ));
    }
    let text = String::from_utf8(bytes)
        .map_err(|_| format!("{file} is not text, so it is not a template."))?;
    Ok(TextEdit::open(
        EditTarget::Import {
            file: file.to_string(),
        },
        text,
    ))
}

/// The line of the text a refusal points at, cut where it points: what is
/// before, the character pointed at (empty at the end of the line), and
/// what is after. A character that would hide or reorder text is written
/// as its code, `⟨U+202E⟩`, so the line shows what it holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Excerpt {
    /// From 1.
    pub line: usize,
    pub before: String,
    pub at: String,
    pub after: String,
}

/// Why the text cannot be kept as it stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Larger than a template can be. It is not read.
    TooLarge { bytes: usize },
    /// It does not read as a template: the reader's words, and the line it
    /// points at when it points at one.
    NotATemplate {
        reason: String,
        excerpt: Option<Excerpt>,
    },
}

impl Refusal {
    /// The line it points at, from 1.
    pub fn line(&self) -> Option<usize> {
        match self {
            Self::NotATemplate {
                excerpt: Some(excerpt),
                ..
            } => Some(excerpt.line),
            _ => None,
        }
    }
}

/// What the template's name means for saving it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NameCheck {
    /// No template has it.
    Free,
    /// The name of the template being edited, or another spelling of it.
    Own,
    /// A shipped template's name or id: refused, in these words.
    Builtin(String),
    /// Another of the user's has it, by this name: saving replaces it.
    Replaces(String),
}

/// The text as it stands, read for the editor.
#[derive(Clone, Debug, PartialEq)]
pub enum TextRead {
    Refused(Refusal),
    /// It reads as a template: the reader's view of it, with every problem
    /// it has on this network, and what its name means.
    Reads {
        view: TemplateView,
        name: NameCheck,
    },
}

impl TextRead {
    /// The reader's view, when the text reads as a template.
    pub fn view(&self) -> Option<&TemplateView> {
        match self {
            Self::Reads { view, .. } => Some(view),
            Self::Refused(_) => None,
        }
    }

    /// Whether Save can keep it: it reads, and its name is not a shipped
    /// one's. Problems on this network do not stop it.
    pub fn can_save(&self) -> bool {
        matches!(self, Self::Reads { name, .. } if !matches!(name, NameCheck::Builtin(_)))
    }

    /// The name of the user's template saving replaces.
    pub fn replaces(&self) -> Option<&str> {
        match self {
            Self::Reads {
                name: NameCheck::Replaces(existing),
                ..
            } => Some(existing),
            _ => None,
        }
    }
}

/// "Reads as a template: 2 questions, 1 block."
pub fn reads_as(view: &TemplateView) -> String {
    let count = |n: usize, one: &str, many: &str| {
        if n == 1 {
            format!("1 {one}")
        } else {
            format!("{n} {many}")
        }
    };
    let mut parts = vec![count(view.questions.count(), "question", "questions")];
    if !view.questions.options.is_empty() {
        parts.push(count(view.questions.options.len(), "option", "options"));
    }
    parts.push(count(view.blocks.len(), "block", "blocks"));
    format!("Reads as a template: {}.", parts.join(", "))
}

/// The words for a text larger than a template can be.
pub fn too_large(bytes: usize) -> String {
    format!("This text is {bytes} bytes. A template can be at most {MAX_TEMPLATE_BYTES}.")
}

/// Read `text`, which the editor is writing as `target`, for what the editor
/// shows beside it: its size first, then whether it reads as a template,
/// then its problems on this network and what its name means.
pub fn read_text(session: &Session, target: &EditTarget, text: &str) -> TextRead {
    if text.len() > MAX_TEMPLATE_BYTES {
        return TextRead::Refused(Refusal::TooLarge { bytes: text.len() });
    }
    let template = match Template::parse(text) {
        Ok(template) => template,
        Err(err) => return TextRead::Refused(refusal(text, &err)),
    };
    let name = name_check(session, target, &template);
    TextRead::Reads {
        view: template_view(session, Arc::new(template), Some(text.to_string())),
        name,
    }
}

fn refusal(text: &str, err: &TemplateError) -> Refusal {
    if let TemplateError::TooLarge { bytes } = err {
        return Refusal::TooLarge { bytes: *bytes };
    }
    let reason = match err {
        // TOML's own words, which the excerpt places.
        TemplateError::Toml { message, .. } => super::sentence(message),
        other => other.to_string(),
    };
    let excerpt = err.place().and_then(|place| {
        let line = text.split('\n').nth(place.line - 1)?;
        let line = line.strip_suffix('\r').unwrap_or(line);
        let mut chars = line.chars();
        let before: String = chars.by_ref().take(place.column - 1).collect();
        let at: String = chars.next().map(String::from).unwrap_or_default();
        Some(Excerpt {
            line: place.line,
            before: shown(&before),
            at: shown(&at),
            after: shown(chars.as_str()),
        })
    });
    Refusal::NotATemplate { reason, excerpt }
}

/// `text` with each character that would hide or reorder what is around it
/// written as its code.
fn shown(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '\t' => c.to_string(),
            '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}'
            | '\u{200E}'
            | '\u{200F}'
            | '\u{061C}' => format!("⟨U+{:04X}⟩", c as u32),
            c if c.is_control() => format!("⟨U+{:04X}⟩", c as u32),
            c => c.to_string(),
        })
        .collect()
}

/// What `template`'s name means for saving it from the editor writing
/// `target`.
fn name_check(session: &Session, target: &EditTarget, template: &Template) -> NameCheck {
    if let Err(why) = refuse_builtin_name(&template.name) {
        return NameCheck::Builtin(why);
    }
    if target.own_slug() == Some(template_slug(&template.name).as_str()) {
        return NameCheck::Own;
    }
    match kept_under(session, &template.name) {
        Some((_, existing)) => NameCheck::Replaces(existing),
        None => NameCheck::Free,
    }
}

/// A text kept from the editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedText {
    /// Its row in the Templates dialog's list.
    pub key: String,
    /// What the dialog says was done.
    pub note: String,
}

/// What came of saving from the editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextSaved {
    Saved(SavedText),
    /// Another of the user's has its name, by this name. Nothing was
    /// written: saving again with `replace` replaces it.
    WouldReplace(String),
}

/// Keep `text`, written as `target`, exactly as it is, comments, order and
/// spacing included, through the same checked write as every other
/// template. It must read as a template and its name must not be a shipped
/// one's; another of the user's under the same slug is replaced only with
/// `replace`. Problems on this network do not stop it: it is listed greyed.
///
/// One of the user's being edited whose name changed is renamed: the new
/// file is written and checked first, and only then is the old one removed.
/// A failed write leaves everything as it was, and says why.
pub fn save_text(
    session: &Session,
    target: &EditTarget,
    text: &str,
    replace: bool,
) -> Result<TextSaved, String> {
    if text.len() > MAX_TEMPLATE_BYTES {
        return Err(too_large(text.len()));
    }
    let template = Template::parse(text).map_err(|err| err.to_string())?;
    let replaced = match name_check(session, target, &template) {
        NameCheck::Builtin(why) => return Err(why),
        NameCheck::Replaces(existing) if !replace => return Ok(TextSaved::WouldReplace(existing)),
        NameCheck::Replaces(_) => true,
        NameCheck::Free | NameCheck::Own => false,
    };
    let name = template.name.clone();
    let slug = template_slug(&name);
    let fits = {
        let idls = session.idls.read();
        wallet_core::template::check(&template, &*idls).is_ok()
    };
    keep(session, template, text)?;
    let mut note = match target {
        EditTarget::Edit { slug: old, .. } if *old == slug => {
            format!("Saved your changes to “{name}”.")
        }
        EditTarget::Edit { name: old, .. } if replaced => {
            format!("Saved “{name}”, which was “{old}”, in place of the one you had.")
        }
        EditTarget::Edit { name: old, .. } => format!("Saved “{name}”, which was “{old}”."),
        _ if replaced => format!("Saved “{name}” in place of the one you had."),
        _ => format!("Saved “{name}”. It is listed with your templates."),
    };
    if let EditTarget::Edit { slug: old, .. } = target {
        if *old != slug {
            if let Err(why) = remove_kept(session, old) {
                note.push_str(&format!(" Its old file, {old}.toml, is still kept. {why}"));
            }
        }
    }
    if !fits {
        note.push_str(" It cannot be used on this network, so it is listed greyed.");
    }
    Ok(TextSaved::Saved(SavedText {
        key: format!("yours:{slug}"),
        note,
    }))
}

#[cfg(test)]
mod tests;
