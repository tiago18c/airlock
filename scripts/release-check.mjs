// Checks that a release is consistent, and prints its notes.
//
//   node scripts/release-check.mjs 0.2.0            check, print the notes
//   node scripts/release-check.mjs 0.2.0 notes.md   check, write the notes to a file
//
// A release is consistent when every place that states the version says the
// same thing, and CHANGELOG.md has a dated section for it with something in it.
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const read = (path) => readFileSync(join(root, path), "utf8");

/** The body of a Cargo.toml table, such as `package` or `workspace.package`. */
function cargoTable(toml, name) {
  return toml.split(/^\[/m).find((part) => part.startsWith(`${name}]`));
}

/** The `version` in the root Cargo.toml's `[workspace.package]` table. */
export function workspaceVersion(toml) {
  return cargoTable(toml, "workspace.package")?.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
}

/**
 * The `version` in a Cargo.toml `[package]` table. A package that inherits it
 * with `version.workspace = true` has the workspace's version.
 */
export function cargoVersion(toml, workspace) {
  const table = cargoTable(toml, "package");
  if (table && /^version\.workspace\s*=\s*true/m.test(table)) return workspace;
  return table?.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
}

/** The body of the `## [version] - date` section, without its heading. */
export function changelogSection(changelog, version) {
  const lines = changelog.split(/\r?\n/);
  const start = lines.findIndex((line) => line.startsWith(`## [${version}]`));
  if (start < 0) return null;
  const rest = lines.slice(start + 1);
  const end = rest.findIndex((line) => /^## \[/.test(line) || /^\[[^\]]+\]:\s/.test(line));
  const body = (end < 0 ? rest : rest.slice(0, end)).join("\n").trim();
  return { heading: lines[start], body };
}

export function check(version, sources = versionSources()) {
  const problems = [];
  if (!/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/.test(version)) {
    problems.push(`"${version}" is not a version like 1.2.3 or 1.2.3-rc.1.`);
  }
  for (const [where, found] of Object.entries(sources.versions)) {
    if (found !== version) problems.push(`${where} says ${found ?? "nothing"}, not ${version}.`);
  }
  const section = changelogSection(sources.changelog, version);
  if (!section) {
    problems.push(`CHANGELOG.md has no "## [${version}]" section.`);
  } else {
    if (!/ - \d{4}-\d{2}-\d{2}$/.test(section.heading)) {
      problems.push(`The CHANGELOG.md heading "${section.heading}" has no date. Use "## [${version}] - YYYY-MM-DD".`);
    }
    if (!section.body) problems.push(`The CHANGELOG.md section for ${version} is empty.`);
  }
  return { problems, notes: section?.body ?? "" };
}

function versionSources() {
  const workspace = workspaceVersion(read("Cargo.toml"));
  const crate = (path) => cargoVersion(read(path), workspace);
  return {
    versions: {
      "Cargo.toml": workspace,
      "apps/airlock/Cargo.toml": crate("apps/airlock/Cargo.toml"),
      "apps/airlock-offline/Cargo.toml": crate("apps/airlock-offline/Cargo.toml"),
      "crates/wallet-core/Cargo.toml": crate("crates/wallet-core/Cargo.toml"),
      "crates/wallet-flows/Cargo.toml": crate("crates/wallet-flows/Cargo.toml"),
      "crates/wallet-rpc/Cargo.toml": crate("crates/wallet-rpc/Cargo.toml"),
      "crates/wallet-vault/Cargo.toml": crate("crates/wallet-vault/Cargo.toml"),
      "crates/qr-protocol/Cargo.toml": crate("crates/qr-protocol/Cargo.toml"),
      "crates/bridge-host/Cargo.toml": crate("crates/bridge-host/Cargo.toml"),
      "extension/package.json": JSON.parse(read("extension/package.json")).version,
      "extension/manifest.json": JSON.parse(read("extension/manifest.json")).version,
    },
    changelog: read("CHANGELOG.md"),
  };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const [version, output] = process.argv.slice(2);
  if (!version) {
    console.error("usage: node scripts/release-check.mjs <version> [notes-file]");
    process.exit(2);
  }
  const { problems, notes } = check(version.replace(/^v/, ""));
  if (problems.length > 0) {
    for (const problem of problems) console.error(`error: ${problem}`);
    process.exit(1);
  }
  if (output) writeFileSync(output, notes + "\n");
  else console.log(notes);
}
