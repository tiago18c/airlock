//! The user's own templates: saving Build's blocks as one, reading them from
//! where they are kept, removing one, and importing and exporting their
//! files.
//!
//! A template is kept as the text of its file, one per template: an imported
//! one as the file read, comments and all, and one saved from Build's blocks
//! as `Template::to_toml` writes it. It goes through `app_core::storage`: `templates/<slug>.toml` in the data
//! folder, or `airlock.template.<slug>` in a browser. A save is checked: one
//! that fails is reported and the list shows only what is kept. The list is
//! read once, the first time Build needs it, into `BuildForm::kept`.

use crate::state::forms::{IxBody, KeptTemplates, TemplateSource};
use crate::state::session::Session;
use dioxus::prelude::*;
use std::collections::BTreeMap;
use std::sync::Arc;
use wallet_core::template::{
    builtin_named, template_slug, Choice, FieldAt, Function, SavePlan, Term,
};
use wallet_core::Template;

/// Read the user's templates, unless they have been read already.
pub fn ensure_kept(session: &Session) {
    if session.forms.build.kept.peek().is_none() {
        load_kept(session);
    }
}

/// Read the user's templates from where they are kept, in place of what
/// was read before. A file that is not a template is listed with why.
pub fn load_kept(session: &Session) {
    let storage = session.core().ports().storage.clone();
    let read = match app_core::storage::stored_templates(storage.as_ref()) {
        Ok(read) => KeptTemplates {
            kept: read
                .kept
                .into_iter()
                .map(|stored| match stored.text {
                    Ok(text) => match Template::parse(&text) {
                        Ok(template) => TemplateSource::Yours {
                            slug: stored.slug,
                            template: Arc::new(template),
                            text: text.into(),
                        },
                        Err(err) => TemplateSource::Broken {
                            slug: stored.slug,
                            why: err.to_string(),
                            text: Some(text.into()),
                        },
                    },
                    Err(why) => TemplateSource::Broken {
                        slug: stored.slug,
                        why,
                        text: None,
                    },
                })
                .collect(),
            left_out: read.left_out,
            error: None,
        },
        Err(err) => KeptTemplates {
            error: Some(format!(
                "Your templates could not be read: {}",
                super::sentence(err)
            )),
            ..KeptTemplates::default()
        },
    };
    let mut kept = session.forms.build.kept;
    kept.set(Some(read));
}

/// The user's templates as read, or none before they are.
pub fn kept_templates(session: &Session) -> KeptTemplates {
    (session.forms.build.kept)().unwrap_or_default()
}

/// Put `source` in the list, in place of one under the same slug.
fn list(session: &Session, source: TemplateSource) {
    let mut kept = session.forms.build.kept;
    let mut kept = kept.write();
    let kept = kept.get_or_insert_with(KeptTemplates::default);
    kept.kept
        .retain(|other| other.slug().is_none() || other.slug() != source.slug());
    kept.kept.push(source);
    kept.kept.sort_by(|a, b| a.slug().cmp(&b.slug()));
}

/// Read Build's blocks for saving as a template. Refuses a block written by
/// hand or one that carries others, by its number: a template holds only
/// instructions filled from an IDL.
pub fn save_plan(session: &Session) -> Result<SavePlan, String> {
    let form = session.forms.build;
    let drafts = form.drafts.peek().clone();
    let mut idl_drafts = Vec::new();
    for (index, draft) in drafts.iter().enumerate() {
        let number = index + 1;
        match &draft.body {
            IxBody::Idl(idl) => idl_drafts.push(idl.clone()),
            IxBody::Raw(_) => {
                return Err(format!(
                    "Block {number} is written by hand, and a template holds only instructions filled from an IDL."
                ))
            }
            IxBody::Carried { .. } => {
                return Err(format!(
                    "Block {number} carries other instructions, and a template holds only instructions filled from an IDL."
                ))
            }
        }
    }
    let keypairs: Vec<_> = form.generated.peek().iter().map(|key| key.pubkey).collect();
    let wallet = session
        .selected_wallet()
        .and_then(|id| session.route_of(id).ok())
        .map(|route| route.address);
    let fee_payer = *form.fee_payer.peek();
    let idls = session.idls.read();
    SavePlan::new(&idl_drafts, &keypairs, wallet, fee_payer, &*idls)
}

/// What came of keeping a template.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kept {
    /// It is kept, under this name.
    Saved(String),
    /// One of the user's is kept under the same slug, by this name. Nothing
    /// was written: keeping it again with `replace` replaces it.
    WouldReplace(String),
}

/// Refuses a name a built-in template has, and says what keeping `name`
/// would replace.
fn place_for(session: &Session, name: &str, replace: bool) -> Result<Option<Kept>, String> {
    if let Some(builtin) = builtin_named(name) {
        let shipped = builtin
            .template()
            .map(|template| template.name)
            .unwrap_or_else(|_| builtin.id.to_string());
        return Err(format!(
            "“{shipped}” is a built-in template. Give yours another name."
        ));
    }
    let slug = template_slug(name);
    ensure_kept(session);
    let existing = session.forms.build.kept.peek().as_ref().and_then(|kept| {
        kept.kept
            .iter()
            .find(|source| source.slug() == Some(slug.as_str()))
            .cloned()
    });
    Ok(match existing {
        Some(existing) if !replace => Some(Kept::WouldReplace(
            existing
                .template()
                .map(|template| template.name)
                .unwrap_or(slug),
        )),
        _ => None,
    })
}

/// Write `text`, the file `template` was read from, where the user's
/// templates are kept, checked, and list it. A failed write is reported,
/// and the list is left as it was.
fn keep(session: &Session, template: Template, text: &str) -> Result<Kept, String> {
    let slug = template_slug(&template.name);
    let storage = session.core().ports().storage.clone();
    if let Err(err) = app_core::storage::save_template(storage.as_ref(), &slug, text) {
        tracing::warn!(target: "airlock::ui", %slug, error = %err, "could not keep a template");
        return Err(format!(
            "The template was not saved: {}",
            super::sentence(err)
        ));
    }
    let name = template.name.clone();
    list(
        session,
        TemplateSource::Yours {
            slug,
            template: Arc::new(template),
            text: text.into(),
        },
    );
    Ok(Kept::Saved(name))
}

/// Save Build's blocks, read into `plan`, as the template `name`, with each
/// field as `choices` says. One of the user's under the same slug is
/// replaced only with `replace`; a built-in's name is refused. Saved, it is
/// listed and Build's note says so; the blocks stay as they are.
pub fn save_as_template(
    session: &Session,
    plan: &SavePlan,
    name: &str,
    summary: &str,
    choices: &BTreeMap<FieldAt, Choice>,
    replace: bool,
) -> Result<Kept, Vec<String>> {
    let saved = {
        let idls = session.idls.read();
        plan.template(name, summary, choices, &*idls)?
    };
    if let Some(would) = place_for(session, &saved.template.name, replace).map_err(|e| vec![e])? {
        return Ok(would);
    }
    let text = saved.template.to_toml();
    let kept = keep(session, saved.template, &text).map_err(|e| vec![e])?;
    if let Kept::Saved(name) = &kept {
        let mut note = session.forms.build.compiled_note;
        note.set(Some(format!(
            "Saved as the template “{name}”. Open it from Templates."
        )));
    }
    Ok(kept)
}

/// Remove the user's template kept as `slug`.
pub fn remove_kept(session: &Session, slug: &str) -> Result<(), String> {
    let storage = session.core().ports().storage.clone();
    app_core::storage::remove_template(storage.as_ref(), slug)
        .map_err(|err| format!("The template was not removed: {}", super::sentence(err)))?;
    let mut kept = session.forms.build.kept;
    if let Some(kept) = kept.write().as_mut() {
        kept.kept.retain(|source| source.slug() != Some(slug));
    }
    Ok(())
}

/// A file offered for import, read before anything is kept.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportPreview {
    /// The file's text, when it is text. Kept as it is, comments and all.
    pub text: Option<String>,
    /// The template, or why the file is not one.
    pub read: Result<Arc<Template>, String>,
    /// Why it cannot be opened on the network in use. Empty when it can.
    pub problems: Vec<String>,
}

/// Read `bytes` as a template file and check it against the IDLs of the
/// network in use. Nothing is kept.
pub fn preview_import(session: &Session, bytes: &[u8]) -> ImportPreview {
    let text = std::str::from_utf8(bytes).ok().map(str::to_string);
    let read = match &text {
        Some(text) => Template::parse(text).map_err(|err| err.to_string()),
        None => Err("This file is not text, so it is not a template.".to_string()),
    };
    let problems = match &read {
        Ok(template) => {
            let idls = session.idls.read();
            wallet_core::template::check(template, &*idls)
                .err()
                .unwrap_or_default()
                .iter()
                .map(ToString::to_string)
                .collect()
        }
        Err(_) => Vec::new(),
    };
    ImportPreview {
        text,
        read: read.map(Arc::new),
        problems,
    }
}

/// Keep an imported file among the user's templates, as `text`, the file
/// as it was read: kept as written, comments and all, and read again here,
/// so what is kept is what was shown. One under the same slug is replaced
/// only with `replace`; a built-in's name is refused.
pub fn keep_import(session: &Session, text: &str, replace: bool) -> Result<Kept, String> {
    let template = Template::parse(text).map_err(|err| err.to_string())?;
    if let Some(would) = place_for(session, &template.name, replace)? {
        return Ok(would);
    }
    keep(session, template, text)
}

/// The file `source` exports as: its name and its text.
pub fn export_file(source: &TemplateSource) -> Result<(String, String), String> {
    match source {
        TemplateSource::Builtin(id) => wallet_core::builtin_template(id)
            .map(|builtin| (format!("{id}.toml"), builtin.toml.to_string()))
            .ok_or_else(|| format!("No template ships as “{id}”.")),
        TemplateSource::Yours { slug, text, .. } => Ok((format!("{slug}.toml"), text.to_string())),
        TemplateSource::Broken { why, .. } => Err(why.clone()),
    }
}

/// Run `task`, a file dialog and what comes of it, in a task of its own
/// while `busy` is held, so the screen can leave out every button that
/// opens another: the dialog is awaited, not modal to the window, so those
/// buttons could otherwise open a second one beside it. Does nothing while
/// one is open already.
///
/// The task runs in the scope that owns `busy`, not the one that was
/// clicked: the dialog that asked can close while the file dialog is open,
/// and the task must still finish and let go of `busy`. A screen that owns
/// `busy` and goes away takes the task with it.
fn file_dialog(mut busy: Signal<bool>, task: impl std::future::Future<Output = ()> + 'static) {
    if *busy.peek() {
        return;
    }
    busy.set(true);
    dioxus::core::Runtime::current().in_scope(busy.origin_scope(), || {
        spawn(async move {
            task.await;
            busy.set(false);
        })
    });
}

/// Write `source`'s file where the user picks: a dialog on desktop, a
/// download in a browser. Says how it went in `note` or `error`, and holds
/// `busy` while the dialog is open.
pub fn export_template(
    source: &TemplateSource,
    mut note: Signal<Option<String>>,
    mut error: Signal<Option<String>>,
    busy: Signal<bool>,
) {
    let (file_name, text) = match export_file(source) {
        Ok(file) => file,
        Err(err) => {
            note.set(None);
            error.set(Some(err));
            return;
        }
    };
    // The dialog is awaited in a task, never inside the handler that asked
    // for it: see `import_from`.
    #[cfg(not(target_arch = "wasm32"))]
    {
        file_dialog(busy, async move {
            match crate::platform::template_files::save_template_file(&file_name, &text).await {
                None => {}
                Some(Ok(path)) => {
                    error.set(None);
                    note.set(Some(format!("Exported to {}.", path.display())));
                }
                Some(Err(err)) => {
                    note.set(None);
                    error.set(Some(super::sentence(err)));
                }
            }
        });
    }
    #[cfg(target_arch = "wasm32")]
    {
        file_dialog(busy, async move {
            let mut eval = crate::platform::eval(DOWNLOAD_JS);
            let sent = eval.send(serde_json::json!({ "name": file_name, "text": text }));
            let answer = match sent {
                Ok(()) => eval.recv::<String>().await.map_err(|err| err.to_string()),
                Err(err) => Err(err.to_string()),
            };
            match answer {
                Ok(message) if message == "ok" => {
                    error.set(None);
                    note.set(Some(format!("Downloaded {file_name}.")));
                }
                Ok(message) | Err(message) => {
                    note.set(None);
                    error.set(Some(message));
                }
            }
        });
    }
}

/// Ask for a template file to import: a dialog on desktop, which reads it
/// into `preview` and holds `busy` while it is open; in a browser, the
/// page's hidden file field, whose change reads it (see [`read_import`]).
/// A browser's picker is modal to the page and says nothing when it is
/// cancelled, so `busy` is not held there.
pub fn pick_import(
    session: Session,
    preview: Signal<Option<ImportPreview>>,
    error: Signal<Option<String>>,
    busy: Signal<bool>,
) {
    #[cfg(not(target_arch = "wasm32"))]
    import_from(
        session,
        preview,
        error,
        busy,
        crate::platform::template_files::pick_template_file(),
    );
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (session, preview, error, busy);
        let _ = crate::platform::eval("document.getElementById('template-file').click()");
    }
}

/// Await `picked` in a task of its own and read the file it gives into
/// `preview`, so the click that asked for it returns before the dialog
/// opens.
///
/// On desktop dioxus runs an event handler inside the webview's request
/// for the event, holding the window's elements borrowed. A blocking file
/// dialog opened there runs its own message loop on the same thread, which
/// renders the window while the handler is still running: the first node
/// that render makes panics ("RefCell already borrowed"), and the handler
/// then panics on a scope that is gone (an `unwrap` on `None`), taking the
/// app down. It took a change on screen before the dialog to show, such as
/// the note of an import before it, so a second import crashed and a first
/// did not.
#[cfg(not(target_arch = "wasm32"))]
fn import_from(
    session: Session,
    mut preview: Signal<Option<ImportPreview>>,
    mut error: Signal<Option<String>>,
    busy: Signal<bool>,
    picked: impl std::future::Future<
            Output = Option<Result<Vec<u8>, crate::platform::template_files::TemplateFileError>>,
        > + 'static,
) {
    file_dialog(busy, async move {
        match picked.await {
            None => {}
            Some(Ok(bytes)) => {
                error.set(None);
                preview.set(Some(preview_import(&session, &bytes)));
            }
            Some(Err(err)) => error.set(Some(super::sentence(err))),
        }
    });
}

/// The id of the page's hidden file field a browser imports through.
pub const TEMPLATE_FILE_INPUT: &str = "template-file";

/// Read the file picked in the page's hidden file field into `preview`.
pub fn read_import(
    evt: Event<FormData>,
    session: Session,
    mut preview: Signal<Option<ImportPreview>>,
    mut error: Signal<Option<String>>,
) {
    let files = evt.files();
    spawn(async move {
        let Some(file) = files.into_iter().next() else {
            return;
        };
        if file.size() > wallet_core::template::MAX_TEMPLATE_BYTES as u64 {
            error.set(Some(format!(
                "This file is {} bytes. A template can be at most {}.",
                file.size(),
                wallet_core::template::MAX_TEMPLATE_BYTES
            )));
            return;
        }
        match file.read_bytes().await {
            Ok(bytes) => {
                error.set(None);
                preview.set(Some(preview_import(&session, &bytes)));
            }
            Err(err) => error.set(Some(super::sentence(err))),
        }
    });
}

/// What a reference or a literal stands for, in words: a value by what its
/// function does, an input by its label. A value another value is worked
/// out from is named as the file names it, `$name`.
pub fn term_words(template: &Template, term: &Term) -> String {
    words_of(template, term, false)
}

/// What a reference or a literal stands for, as part of other words: a
/// value by its name, `$mint`, rather than by what works it out.
pub fn name_words(template: &Template, term: &Term) -> String {
    words_of(template, term, true)
}

fn words_of(template: &Template, term: &Term, nested: bool) -> String {
    let name = match term {
        Term::Literal(text) => return text.clone(),
        Term::Ref(name) => name.as_str(),
    };
    if name == wallet_core::template::WALLET {
        return "the wallet in use".to_string();
    }
    if name == wallet_core::template::FEE_PAYER {
        return "the fee payer".to_string();
    }
    if let Some(input) = template.input(name) {
        return format!("“{}”", input.label);
    }
    if let Some(address) = template.programs.get(name) {
        return address.to_string();
    }
    let Some(call) = template.values.get(name).filter(|_| !nested) else {
        return format!("${name}");
    };
    let arg = |at: usize| {
        call.args
            .get(at)
            .map(|arg| words_of(template, arg, true))
            .unwrap_or_default()
    };
    match call.function {
        Function::NewKeypair => "a new keypair, made when it is opened".to_string(),
        Function::AssociatedTokenAccount => {
            format!("the token account of {} for the mint {}", arg(0), arg(1))
        }
        Function::Token2022MintSpace => format!("the size of the mint {}", arg(0)),
        Function::Token2022MintFundedSpace => {
            format!("the size of the mint {} with its metadata", arg(0))
        }
        Function::RentFor => format!("rent for {} bytes, from the node", arg(0)),
        Function::DecimalsOf => format!("the decimals of the mint {}, from the node", arg(0)),
        Function::TokenProgramOf => {
            format!("the token program of the mint {}, from the node", arg(0))
        }
    }
}

/// Whether this device can pick and write template files: not a phone,
/// which has no file picker for the app.
pub fn files_here() -> bool {
    !cfg!(target_os = "android")
}

#[cfg(target_arch = "wasm32")]
const DOWNLOAD_JS: &str = r#"
try {
    const file = await dioxus.recv();
    const blob = new Blob([file.text], { type: "application/toml" });
    const url = URL.createObjectURL(blob);
    const link = document.createElement("a");
    link.href = url;
    link.download = file.name;
    document.body.appendChild(link);
    link.click();
    link.remove();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
    dioxus.send("ok");
} catch (err) {
    dioxus.send(String(err));
}
"#;

#[cfg(test)]
mod tests;
