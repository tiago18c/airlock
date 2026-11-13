//! Bringing IDL files into the catalog, taking them out, and checking one
//! against the chain.

use crate::actions::idl_check::check_idl;
use crate::state::forms::{PendingFiles, PendingImport};
use crate::state::session::Session;
use dioxus::prelude::*;
use solana_pubkey::Pubkey;
use wallet_core::IdlCheck;

#[cfg(not(target_arch = "wasm32"))]
use crate::platform::idl_files::{IdlFiles, IdlFilesError};

/// Ask for one IDL file and import it: a dialog on desktop, which holds
/// the pane's `file_dialog` flag while it is open; in a browser, the page's
/// hidden file field, whose change reads it (see [`read_one`]).
pub fn import_one(session: Session, note: Signal<Option<String>>, error: Signal<Option<String>>) {
    #[cfg(not(target_arch = "wasm32"))]
    import_one_from(
        session,
        note,
        error,
        crate::platform::idl_files::pick_idl_file(),
    );
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (session, note, error);
        let _ = crate::platform::eval("document.getElementById('idl-one').click()");
    }
}

/// Ask for a bundle folder and import its files: a dialog on desktop, as
/// [`import_one`]; in a browser, the page's hidden folder field (see
/// [`read_bundle`]).
pub fn import_folder(
    session: Session,
    note: Signal<Option<String>>,
    error: Signal<Option<String>>,
) {
    #[cfg(not(target_arch = "wasm32"))]
    import_folder_from(
        session,
        note,
        error,
        crate::platform::idl_files::pick_idl_bundle(),
    );
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (session, note, error);
        let _ = crate::platform::eval("document.getElementById('idl-bundle').click()");
    }
}

/// Write every imported IDL into a folder the user picks: a dialog on
/// desktop, as [`import_one`]; in a browser, the page's folder picker.
pub fn export_folder(
    session: Session,
    note: Signal<Option<String>>,
    error: Signal<Option<String>>,
) {
    let files = match session.export_idl_files() {
        Ok(files) => files,
        Err(err) => {
            report(note, error, Err(err.to_string()));
            return;
        }
    };
    #[cfg(not(target_arch = "wasm32"))]
    export_to(session, note, error, files, |files| async move {
        crate::platform::idl_files::save_idl_bundle(&files).await
    });
    #[cfg(target_arch = "wasm32")]
    {
        let (mut note, mut error) = (note, error);
        use base64::engine::general_purpose::STANDARD;
        use base64::Engine;
        let payload: Vec<serde_json::Value> = files
            .iter()
            .map(|(name, bytes)| {
                serde_json::json!({
                    "name": name,
                    "b64": STANDARD.encode(bytes),
                })
            })
            .collect();
        let count = files.len();
        spawn(async move {
            let mut eval = crate::platform::eval(EXPORT_JS);
            if let Err(err) = eval.send(payload) {
                note.set(None);
                error.set(Some(err.to_string()));
                return;
            }
            match eval.recv::<String>().await {
                Ok(message) if message == "ok" => {
                    error.set(None);
                    note.set(Some(format!("Saved {count} files.")));
                }
                Ok(message) => {
                    note.set(None);
                    error.set(Some(message));
                }
                Err(err) => {
                    note.set(None);
                    error.set(Some(err.to_string()));
                }
            }
        });
    }
}

/// Run `task`, a file dialog and what comes of it, in a task of its own
/// while the pane's `file_dialog` flag is held, so the click that asked for
/// the dialog returns before it opens. Does nothing while one is open
/// already: the dialog is not modal to the window, so the pane disables its
/// buttons while the flag is held, and this refuses a second one should a
/// click get through anyway.
///
/// On desktop dioxus runs an event handler inside the webview's request
/// for the event, holding the window's elements borrowed. A blocking file
/// dialog opened there runs its own message loop on the same thread, and
/// anything that renders the window while it is open, such as a site's
/// request arriving through the browser bridge, panics on the first node
/// it makes ("RefCell already borrowed"); the handler then panics on a
/// scope that is gone (an `unwrap` on `None`) and the app dies.
///
/// The task runs in the scope that owns the flag, the session's, not the
/// pane's: the pane can be left while the dialog is open, and the import or
/// export still finishes and lets go of the flag. What it says then goes
/// nowhere (see [`report`]).
#[cfg(not(target_arch = "wasm32"))]
fn file_dialog(session: Session, task: impl std::future::Future<Output = ()> + 'static) {
    let mut busy = session.forms.program.file_dialog;
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

/// Await `picked`, the file the dialog gives, in a task of its own, and
/// import it.
#[cfg(not(target_arch = "wasm32"))]
fn import_one_from(
    session: Session,
    note: Signal<Option<String>>,
    error: Signal<Option<String>>,
    picked: impl std::future::Future<Output = Option<Result<Vec<u8>, IdlFilesError>>> + 'static,
) {
    file_dialog(session, async move {
        match picked.await {
            None => {}
            Some(Ok(bytes)) => apply_one(&session, &bytes, note, error),
            Some(Err(err)) => report(note, error, Err(err.to_string())),
        }
    });
}

/// Await `picked`, the bundle folder's files, in a task of its own, and
/// import them.
#[cfg(not(target_arch = "wasm32"))]
fn import_folder_from(
    session: Session,
    note: Signal<Option<String>>,
    error: Signal<Option<String>>,
    picked: impl std::future::Future<Output = Option<Result<IdlFiles, IdlFilesError>>> + 'static,
) {
    file_dialog(session, async move {
        match picked.await {
            None => {}
            Some(Ok(files)) => apply_bundle(&session, &files, note, error),
            Some(Err(err)) => report(note, error, Err(err.to_string())),
        }
    });
}

/// Hand `files` to `save`, which asks for a folder and writes them there,
/// in a task of its own, and say where they went.
#[cfg(not(target_arch = "wasm32"))]
fn export_to<F>(
    session: Session,
    note: Signal<Option<String>>,
    error: Signal<Option<String>>,
    files: IdlFiles,
    save: impl FnOnce(IdlFiles) -> F,
) where
    F: std::future::Future<Output = Option<Result<std::path::PathBuf, IdlFilesError>>> + 'static,
{
    let count = files.len();
    let saved = save(files);
    file_dialog(session, async move {
        match saved.await {
            None => {}
            Some(Ok(dir)) => report(
                note,
                error,
                Ok(format!("Saved {count} files to {}.", dir.display())),
            ),
            Some(Err(err)) => report(note, error, Err(err.to_string())),
        }
    });
}

/// Say how an import or an export went: `Ok` in `note`, `Err` in `error`,
/// clearing the other. Says nothing once the pane that showed them has
/// gone, which a file dialog can outlive.
fn report(
    note: Signal<Option<String>>,
    error: Signal<Option<String>>,
    outcome: Result<String, String>,
) {
    let (shown, cleared, text) = match outcome {
        Ok(text) => (note, error, text),
        Err(text) => (error, note, text),
    };
    put(cleared, None);
    put(shown, Some(text));
}

/// Put `value` in `signal`, unless the scope that owned it has gone.
fn put(mut signal: Signal<Option<String>>, value: Option<String>) {
    if let Ok(mut slot) = signal.try_write() {
        *slot = value;
    }
}

/// Import one IDL file, unless it would take the place of an IDL that ships
/// with the app: that waits for the user's yes (see [`confirm_import`]).
pub fn apply_one(
    session: &Session,
    bytes: &[u8],
    note: Signal<Option<String>>,
    error: Signal<Option<String>>,
) {
    let replaces = session.idls.peek().shipped_replaced_by_file(bytes);
    if !replaces.is_empty() {
        ask_first(
            session,
            PendingFiles::One(bytes.to_vec()),
            replaces,
            note,
            error,
        );
        return;
    }
    import_one_now(session, bytes, &[], note, error);
}

/// Hold an import that replaces shipped IDLs until the user says yes.
fn ask_first(
    session: &Session,
    files: PendingFiles,
    replaces: Vec<String>,
    note: Signal<Option<String>>,
    error: Signal<Option<String>>,
) {
    put(note, None);
    put(error, None);
    let mut pending = session.forms.program.pending_import;
    pending.set(Some(PendingImport { files, replaces }));
}

/// The user's answer to an import held by [`ask_first`].
pub fn confirm_import(
    session: &Session,
    replace: bool,
    note: Signal<Option<String>>,
    error: Signal<Option<String>>,
) {
    let mut pending = session.forms.program.pending_import;
    let Some(held) = pending.take() else {
        return;
    };
    if !replace {
        report(note, error, Ok("Nothing was imported.".into()));
        return;
    }
    match held.files {
        PendingFiles::One(bytes) => import_one_now(session, &bytes, &held.replaces, note, error),
        PendingFiles::Bundle(files) => {
            import_bundle_now(session, &files, &held.replaces, note, error)
        }
    }
}

/// What an import replaced, as a sentence to add to its message.
fn replaced_sentence(replaces: &[String]) -> String {
    match replaces {
        [] => String::new(),
        [one] => format!(" It replaces the IDL that ships with Airlock for {one}."),
        [rest @ .., last] => format!(
            " It replaces the IDLs that ship with Airlock for {} and {last}.",
            rest.join(", ")
        ),
    }
}

fn import_one_now(
    session: &Session,
    bytes: &[u8],
    replaces: &[String],
    note: Signal<Option<String>>,
    error: Signal<Option<String>>,
) {
    match session.import_idl(bytes, None) {
        Ok(record) => {
            report(
                note,
                error,
                Ok(format!(
                    "Imported {}.{}",
                    record.name,
                    replaced_sentence(replaces)
                )),
            );
            let mut selected = session.forms.program.selected;
            selected.set(Some(record.program_id));
        }
        Err(err) => report(note, error, Err(err.to_string())),
    }
}

/// Import a bundle, unless it would take the place of IDLs that ship with
/// the app: that waits for the user's yes (see [`confirm_import`]).
pub fn apply_bundle(
    session: &Session,
    files: &[(String, Vec<u8>)],
    note: Signal<Option<String>>,
    error: Signal<Option<String>>,
) {
    let borrowed: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
        .collect();
    let replaces = session.idls.peek().shipped_replaced_by_bundle(&borrowed);
    if !replaces.is_empty() {
        ask_first(
            session,
            PendingFiles::Bundle(files.to_vec()),
            replaces,
            note,
            error,
        );
        return;
    }
    import_bundle_now(session, files, &[], note, error);
}

fn import_bundle_now(
    session: &Session,
    files: &[(String, Vec<u8>)],
    replaces: &[String],
    note: Signal<Option<String>>,
    error: Signal<Option<String>>,
) {
    let borrowed: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
        .collect();
    match session.import_idl_bundle(&borrowed) {
        Ok(report) => {
            let mut text = format!(
                "Imported {} programs.{}",
                report.imported.len(),
                replaced_sentence(replaces)
            );
            if !report.skipped.is_empty() {
                text.push_str(" Skipped: ");
                text.push_str(&report.skipped.join("; "));
            }
            self::report(note, error, Ok(text));
            if let Some(record) = report.imported.first() {
                let mut selected = session.forms.program.selected;
                selected.set(Some(record.program_id));
            }
        }
        Err(err) => self::report(note, error, Err(err.to_string())),
    }
}

pub fn read_one(
    evt: Event<FormData>,
    session: Session,
    mut note: Signal<Option<String>>,
    mut error: Signal<Option<String>>,
) {
    let files = evt.files();
    spawn(async move {
        let Some(file) = files.into_iter().next() else {
            return;
        };
        match file.read_bytes().await {
            Ok(bytes) => apply_one(&session, &bytes, note, error),
            Err(err) => {
                note.set(None);
                error.set(Some(err.to_string()));
            }
        }
    });
}

pub fn check_one(
    session: Session,
    program_id: Pubkey,
    mut checking: Signal<bool>,
    mut note: Signal<Option<String>>,
) {
    let Some(record) = session
        .idl_records()
        .into_iter()
        .find(|record| record.program_id == program_id)
    else {
        return;
    };
    let name = record.name.clone();
    checking.set(true);
    note.set(None);
    spawn(async move {
        note.set(Some(match check_idl(&session, &record).await {
            Ok(check) => check_sentence(&name, &check),
            Err(err) => err.to_string(),
        }));
        checking.set(false);
    });
}

/// Compare every program in `ids` that has a stored IDL with the chain, one
/// after the other, and say how many matched.
pub fn check_many(
    session: Session,
    ids: Vec<Pubkey>,
    mut checking: Signal<bool>,
    mut note: Signal<Option<String>>,
) {
    checking.set(true);
    note.set(None);
    spawn(async move {
        let mut tally = CheckTally::default();
        for id in ids {
            let Some(record) = session
                .idl_records()
                .into_iter()
                .find(|record| record.program_id == id)
            else {
                continue;
            };
            tally.count(check_idl(&session, &record).await);
        }
        note.set(Some(tally.sentence()));
        checking.set(false);
    });
}

/// How a run of checks went.
#[derive(Default, Debug, PartialEq, Eq)]
pub struct CheckTally {
    pub matched: usize,
    /// The chain publishes what the stored file was made from.
    pub sourced: usize,
    pub differed: usize,
    pub failed: usize,
    /// Why the last one that failed did.
    pub last_error: Option<String>,
}

impl CheckTally {
    /// Add one check: how the copies compared, or why they could not be
    /// compared.
    pub fn count(&mut self, check: Result<IdlCheck, impl ToString>) {
        match check {
            Ok(check) if check.same => self.matched += 1,
            Ok(check) if check.source => self.sourced += 1,
            Ok(_) => self.differed += 1,
            Err(err) => {
                self.failed += 1;
                self.last_error = Some(err.to_string());
            }
        }
    }

    pub fn sentence(&self) -> String {
        let mut text = format!("{} match", self.matched);
        if self.sourced > 0 {
            text.push_str(&format!(", {} match their source", self.sourced));
        }
        text.push_str(&format!(", {} differ", self.differed));
        if self.failed > 0 {
            text.push_str(&format!(", {} failed", self.failed));
            if let Some(err) = &self.last_error {
                text.push_str(&format!(" ({err})"));
            }
        }
        text.push_str(". Stored files were not replaced.");
        text
    }
}

pub fn check_sentence(name: &str, check: &IdlCheck) -> String {
    if !check.same && check.source {
        return format!(
            "The remote copy is the IDL {name} was made from, so the stored file is current. It was not replaced."
        );
    }
    let word = if check.same {
        "matches"
    } else {
        "differs from"
    };
    format!("{name} {word} the remote copy. The stored file was not replaced.")
}

pub fn read_bundle(
    evt: Event<FormData>,
    session: Session,
    mut note: Signal<Option<String>>,
    mut error: Signal<Option<String>>,
) {
    let picked = evt.files();
    spawn(async move {
        let mut files = Vec::new();
        for file in picked {
            let name = file.name();
            match file.read_bytes().await {
                Ok(bytes) => files.push((name, bytes.to_vec())),
                Err(err) => {
                    note.set(None);
                    error.set(Some(err.to_string()));
                    return;
                }
            }
        }
        if files.is_empty() {
            note.set(None);
            error.set(Some("That folder has no readable files.".into()));
            return;
        }
        apply_bundle(&session, &files, note, error);
    });
}

#[cfg(target_arch = "wasm32")]
const EXPORT_JS: &str = r#"
try {
    const files = await dioxus.recv();
    if (!window.showDirectoryPicker) {
        dioxus.send("This browser cannot choose a folder.");
    } else {
        const dir = await window.showDirectoryPicker();
        for (const file of files) {
            const handle = await dir.getFileHandle(file.name, { create: true });
            const writable = await handle.createWritable();
            const binary = Uint8Array.from(atob(file.b64), (c) => c.charCodeAt(0));
            await writable.write(binary);
            await writable.close();
        }
        dioxus.send("ok");
    }
} catch (err) {
    dioxus.send(String(err));
}
"#;

#[cfg(test)]
mod tests;
