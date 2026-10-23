//! The order of this crate's modules, from the bottom. A module may use one
//! in its own layer or below, never above, so the modules form no loop and
//! a layer can later become a crate of its own.
//!
//! `book` and `wallets` are groups, so each of their modules has its own
//! place: the address book is a store that wallets read, while `describe`
//! names addresses with what the wallets know.

use std::collections::BTreeMap;
use std::path::Path;

const LAYERS: &[&[&str]] = &[
    &["ids", "units", "cluster"],
    &["chain", "error", "keys", "phrase"],
    &["tx", "market", "vector", "stake", "spl"],
    &["idl", "transport", "address_book", "holdings", "workspace"],
    &["squads", "programmatic", "offline_nonce"],
    &["record", "record_kind", "record_keys", "vault", "listing"],
    &["import", "roster", "route", "proposals", "relay"],
    &["vector_wallets", "adapt", "enroll", "shared_keys"],
    &["describe", "portfolio", "review", "explorer", "edit"],
];

/// Modules whose own files only list what their children export.
const GROUPS: &[&str] = &["book", "wallets"];

fn layer(unit: &str) -> Option<usize> {
    LAYERS.iter().position(|layer| layer.contains(&unit))
}

/// Names a `pub use` list exports, each with the module it comes from.
fn reexports(text: &str) -> BTreeMap<String, String> {
    let mut names = BTreeMap::new();
    for statement in text.split(';') {
        let Some(at) = statement.find("pub use ") else {
            continue;
        };
        let used = statement[at + "pub use ".len()..].trim();
        let (path, list) = match used.find("::{") {
            Some(brace) => (&used[..brace], used[brace + 3..].trim_end_matches('}')),
            None => match used.rfind("::") {
                Some(sep) => (&used[..sep], &used[sep + 2..]),
                None => continue,
            },
        };
        let module = path.rsplit("::").next().unwrap_or(path);
        for name in list.split(',') {
            let name = name.split(" as ").last().unwrap_or(name).trim();
            if !name.is_empty() {
                names.insert(name.to_string(), module.to_string());
            }
        }
    }
    names
}

/// Drops comments, `pub use` lists and a trailing `mod tests`, which do not
/// make one module need another.
fn code_of(text: &str) -> String {
    let text = text.replace("\r\n", "\n");
    let text = match text.find("#[cfg(test)]\nmod tests") {
        Some(at) => &text[..at],
        None => &text,
    };
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect();
    lines
        .join("\n")
        .split_inclusive(';')
        .filter(|statement| !statement.trim_start().starts_with("pub use "))
        .collect()
}

fn unit_of(rel: &Path) -> Option<String> {
    let parts: Vec<String> = rel
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    let file = parts.last()?;
    if file == "lib.rs"
        || file == "layers.rs"
        || file == "fixtures.rs"
        || file == "tests.rs"
        || file.ends_with("_tests.rs")
        || parts.iter().any(|part| part == "tests")
    {
        return None;
    }
    let first = parts[0].trim_end_matches(".rs");
    if GROUPS.contains(&first) {
        let child = parts.get(1)?.trim_end_matches(".rs");
        return (child != "mod").then(|| child.to_string());
    }
    Some(first.to_string())
}

fn visit(dir: &Path, found: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            visit(&path, found);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            found.push(path);
        }
    }
}

#[test]
fn no_module_uses_one_above_it() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let read = |path: &Path| std::fs::read_to_string(path).unwrap();
    let root = reexports(&read(&src.join("lib.rs")));
    let groups: BTreeMap<&str, BTreeMap<String, String>> = GROUPS
        .iter()
        .map(|group| (*group, reexports(&read(&src.join(group).join("mod.rs")))))
        .collect();
    // `crate::<segments>` to the module it reaches.
    let target = |segments: &[&str]| -> Option<String> {
        let (first, rest) = segments.split_first()?;
        let mut unit = root.get(*first).cloned().unwrap_or(first.to_string());
        let mut name = if root.contains_key(*first) {
            Some(first.to_string())
        } else {
            rest.first().map(|second| second.to_string())
        };
        while let Some(children) = groups.get(unit.as_str()) {
            let next = name.take()?;
            unit = children.get(&next).cloned().unwrap_or(next);
        }
        Some(unit)
    };

    let mut files = Vec::new();
    visit(&src, &mut files);
    let mut unplaced = Vec::new();
    let mut upward = Vec::new();
    for path in files {
        let rel = path.strip_prefix(&src).unwrap();
        let Some(unit) = unit_of(rel) else {
            continue;
        };
        let Some(own) = layer(&unit) else {
            unplaced.push(format!("{} ({unit})", rel.display()));
            continue;
        };
        let code = code_of(&read(&path));
        for (at, _) in code.match_indices("crate::") {
            // The path's first two names; it ends at anything but `::`.
            let mut segments = Vec::new();
            for segment in code[at + "crate::".len()..].split("::").take(2) {
                let end = segment
                    .find(|c: char| !(c.is_alphanumeric() || c == '_'))
                    .unwrap_or(segment.len());
                if end > 0 {
                    segments.push(&segment[..end]);
                }
                if end < segment.len() {
                    break;
                }
            }
            let Some(reached) = target(&segments) else {
                continue;
            };
            if layer(&reached).is_some_and(|theirs| theirs > own) {
                upward.push(format!(
                    "{} ({unit}) uses crate::{}",
                    rel.display(),
                    segments.join("::")
                ));
            }
        }
    }
    assert!(unplaced.is_empty(), "modules with no layer: {unplaced:#?}");
    upward.sort();
    upward.dedup();
    assert!(
        upward.is_empty(),
        "modules that use one above them: {upward:#?}"
    );
}
