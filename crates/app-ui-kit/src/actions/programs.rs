//! Bringing IDL files into the catalog, taking them out, and checking one
//! against the chain.

use crate::actions::idl_check::check_idl;
use crate::state::session::Session;
use dioxus::prelude::*;
use solana_pubkey::Pubkey;
use wallet_core::IdlCheck;

pub fn import_one(session: Session, note: Signal<Option<String>>, error: Signal<Option<String>>) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut note = note;
        let mut error = error;
        match crate::platform::idl_files::pick_idl_file() {
            None => {}
            Some(Ok(bytes)) => apply_one(&session, &bytes, note, error),
            Some(Err(err)) => {
                note.set(None);
                error.set(Some(err.to_string()));
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (session, note, error);
        let _ = crate::platform::eval("document.getElementById('idl-one').click()");
    }
}

pub fn import_folder(
    session: Session,
    note: Signal<Option<String>>,
    error: Signal<Option<String>>,
) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut note = note;
        let mut error = error;
        match crate::platform::idl_files::pick_idl_bundle() {
            None => {}
            Some(Ok(files)) => apply_bundle(&session, &files, note, error),
            Some(Err(err)) => {
                note.set(None);
                error.set(Some(err.to_string()));
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (session, note, error);
        let _ = crate::platform::eval("document.getElementById('idl-bundle').click()");
    }
}

pub fn export_folder(
    session: Session,
    note: Signal<Option<String>>,
    error: Signal<Option<String>>,
) {
    let mut note = note;
    let mut error = error;
    let files = match session.export_idl_files() {
        Ok(files) => files,
        Err(err) => {
            note.set(None);
            error.set(Some(err.to_string()));
            return;
        }
    };
    #[cfg(not(target_arch = "wasm32"))]
    {
        match crate::platform::idl_files::save_idl_bundle(&files) {
            None => {}
            Some(Ok(dir)) => {
                error.set(None);
                note.set(Some(format!(
                    "Saved {} files to {}.",
                    files.len(),
                    dir.display()
                )));
            }
            Some(Err(err)) => {
                note.set(None);
                error.set(Some(err.to_string()));
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
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

pub fn apply_one(
    session: &Session,
    bytes: &[u8],
    mut note: Signal<Option<String>>,
    mut error: Signal<Option<String>>,
) {
    match session.import_idl(bytes, None) {
        Ok(record) => {
            error.set(None);
            note.set(Some(format!("Imported {}.", record.name)));
            let mut selected = session.forms.program.selected;
            selected.set(Some(record.program_id));
        }
        Err(err) => {
            note.set(None);
            error.set(Some(err.to_string()));
        }
    }
}

pub fn apply_bundle(
    session: &Session,
    files: &[(String, Vec<u8>)],
    mut note: Signal<Option<String>>,
    mut error: Signal<Option<String>>,
) {
    let borrowed: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
        .collect();
    match session.import_idl_bundle(&borrowed) {
        Ok(report) => {
            let mut text = format!("Imported {} programs.", report.imported.len());
            if !report.skipped.is_empty() {
                text.push_str(" Skipped: ");
                text.push_str(&report.skipped.join("; "));
            }
            error.set(None);
            note.set(Some(text));
            if let Some(record) = report.imported.first() {
                let mut selected = session.forms.program.selected;
                selected.set(Some(record.program_id));
            }
        }
        Err(err) => {
            note.set(None);
            error.set(Some(err.to_string()));
        }
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
            tally.count(check_idl(&session, &record).await.map(|check| check.same));
        }
        note.set(Some(tally.sentence()));
        checking.set(false);
    });
}

/// How a run of checks went.
#[derive(Default, Debug, PartialEq, Eq)]
pub struct CheckTally {
    pub matched: usize,
    pub differed: usize,
    pub failed: usize,
    /// Why the last one that failed did.
    pub last_error: Option<String>,
}

impl CheckTally {
    /// Add one check: whether the copies were the same, or why they could
    /// not be compared.
    pub fn count(&mut self, same: Result<bool, impl ToString>) {
        match same {
            Ok(true) => self.matched += 1,
            Ok(false) => self.differed += 1,
            Err(err) => {
                self.failed += 1;
                self.last_error = Some(err.to_string());
            }
        }
    }

    pub fn sentence(&self) -> String {
        let mut text = format!("{} match, {} differ", self.matched, self.differed);
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
mod tests {
    use super::*;

    #[test]
    fn a_run_of_checks_says_how_many_matched_and_why_the_last_failure_failed() {
        let mut tally = CheckTally::default();
        tally.count(Ok::<_, String>(true));
        tally.count(Ok::<_, String>(true));
        tally.count(Ok::<_, String>(false));
        assert_eq!(
            tally.sentence(),
            "2 match, 1 differ. Stored files were not replaced."
        );

        tally.count(Err::<bool, _>("first"));
        tally.count(Err::<bool, _>("Offline"));
        assert_eq!(
            tally.sentence(),
            "2 match, 1 differ, 2 failed (Offline). Stored files were not replaced."
        );
    }
}
