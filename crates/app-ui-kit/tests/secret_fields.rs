//! A field that takes key material (a seed phrase, a private key, keys
//! exported from another device) tells the phone's keyboard not to correct,
//! capitalize or learn what is typed, and the desktop's spellchecker not to
//! read it. A keyboard that learns a seed phrase keeps its words in its
//! dictionary, and may sync them.
//!
//! The screens are written in `rsx!`, so this reads their sources: every
//! `textarea` and `input` in the app crates, on the phone and the desktop
//! alike. A field is taken for key material by the words of its label or
//! placeholder. Every other text area that can be typed in is listed in
//! `NOT_KEY_MATERIAL`, so a new one cannot be added without that decision.

use std::fs;
use std::path::{Path, PathBuf};

/// What a field for key material carries, as it is written in `rsx!`.
const QUIET: [&str; 4] = [
    r#"autocomplete: "off""#,
    r#"autocapitalize: "off""#,
    r#"autocorrect: "off""#,
    r#"spellcheck: "false""#,
];

/// Words in a label or placeholder that say a field takes key material.
const KEY_WORDS: &[&str] = &[
    "seed phrase",
    "recovery phrase",
    "private key",
    "secret key",
    "exported keys",
    "mnemonic",
    "the words",
];

/// Text areas a person types in that take no key material: the file, and
/// the label or placeholder that tells the area apart.
const NOT_KEY_MATERIAL: &[(&str, &str)] = &[
    (
        "app-ui-book/src/ui/address_book/transfer.rs",
        "Paste the exported JSON",
    ),
    (
        "app-ui-build/src/ui/build/raw.rs",
        "such as 02000000e803000000000000",
    ),
    ("app-ui-build/src/ui/build/text.rs", "Template text"),
    ("app-ui-sign/src/ui/sign/inbox.rs", "Paste here…"),
    (
        "app-ui-wallets/src/ui/wallets/kinds/squads_form.rs",
        "One address per line",
    ),
];

struct Field {
    file: String,
    line: usize,
    textarea: bool,
    /// The field's own attributes, and the label line just above it.
    text: String,
}

#[test]
fn key_material_fields_keep_the_keyboard_out() {
    let fields = fields();
    let mut problems = Vec::new();
    let mut key_fields = 0;
    for field in &fields {
        let lower = field.text.to_lowercase();
        let password = field.text.contains(r#"r#type: "password""#);
        let key = !password && KEY_WORDS.iter().any(|w| lower.contains(w));
        if key {
            key_fields += 1;
            let missing: Vec<_> = QUIET.iter().filter(|a| !field.text.contains(*a)).collect();
            if !missing.is_empty() {
                problems.push(format!(
                    "{}:{}: a field for key material without {missing:?}",
                    field.file, field.line
                ));
            }
        } else if field.textarea
            && !field.text.contains("readonly: true")
            && !NOT_KEY_MATERIAL
                .iter()
                .any(|(file, name)| field.file.ends_with(file) && field.text.contains(name))
        {
            problems.push(format!(
                "{}:{}: a text area that is neither marked as key material ({:?}) nor listed in NOT_KEY_MATERIAL",
                field.file, field.line, QUIET
            ));
        }
    }
    // The seed phrase, the private key, the exported keys and the Vector
    // recovery phrase: if these are not found, the reading is broken.
    assert!(
        key_fields >= 4,
        "found {key_fields} fields for key material"
    );
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

fn fields() -> Vec<Field> {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut files = Vec::new();
    for entry in fs::read_dir(crates).unwrap() {
        let dir = entry.unwrap().path();
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        if name.starts_with("app-") {
            sources(&dir.join("src"), &mut files);
        }
    }
    sources(&crates.parent().unwrap().join("apps"), &mut files);
    assert!(files.len() > 50, "found {} source files", files.len());

    let mut out = Vec::new();
    for path in files {
        let src = fs::read_to_string(&path).unwrap();
        let file = path.to_string_lossy().replace('\\', "/");
        let lines: Vec<&str> = src.lines().collect();
        for (n, line) in lines.iter().enumerate() {
            let trimmed = line.trim_start();
            let textarea = trimmed.starts_with("textarea {");
            if !textarea && !trimmed.starts_with("input {") {
                continue;
            }
            let mut text = String::new();
            if let Some(above) = lines[..n].iter().rev().find(|l| !l.trim().is_empty()) {
                if above.contains("label {") {
                    text.push_str(above);
                    text.push('\n');
                }
            }
            text.push_str(&block(&lines[n..]));
            out.push(Field {
                file: file.clone(),
                line: n + 1,
                textarea,
                text,
            });
        }
    }
    out
}

/// The element from its first line to its closing brace, skipping braces
/// inside string literals.
fn block(lines: &[&str]) -> String {
    let mut depth = 0i32;
    let mut out = String::new();
    for line in lines {
        out.push_str(line);
        out.push('\n');
        let mut in_str = false;
        let mut escaped = false;
        for c in line.chars() {
            if in_str {
                match c {
                    _ if escaped => escaped = false,
                    '\\' => escaped = true,
                    '"' => in_str = false,
                    _ => {}
                }
                continue;
            }
            match c {
                '"' => in_str = true,
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
        }
        if depth <= 0 {
            break;
        }
    }
    out
}

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            let name = path.file_name().unwrap().to_string_lossy();
            if name != "target" && name != "android" && !name.starts_with('.') {
                sources(&path, out);
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}
