// node --test scripts/
import assert from "node:assert/strict";
import { test } from "node:test";
import { cargoVersion, changelogSection, check, workspaceVersion } from "./release-check.mjs";

const changelog = `# Changelog

## [Unreleased]

### Added
- Something not out yet.

## [0.2.0] - 2026-11-03

### Fixed
- A thing.

## [0.1.0]

[Unreleased]: https://example.test/compare/v0.2.0...HEAD
`;

const sources = (versions) => ({ versions, changelog });

test("the package version is read from the package table only", () => {
  const toml = `[workspace]\nmembers = ["."]\n\n[package]\nname = "x"\nversion = "0.2.0"\n\n[dependencies]\nhex = { version = "0.4" }\n`;
  assert.equal(cargoVersion(toml), "0.2.0");
  assert.equal(cargoVersion("[dependencies]\nversion = \"9.9.9\"\n"), undefined);
});

test("a package that inherits its version has the workspace's", () => {
  const root = `[workspace]
members = ["apps/x"]

[workspace.package]
version = "0.3.0"
edition = "2021"

[workspace.dependencies]
hex = { version = "0.4" }
`;
  assert.equal(workspaceVersion(root), "0.3.0");
  assert.equal(cargoVersion(root, workspaceVersion(root)), undefined);
  const member = `[package]
name = "x"
version.workspace = true
`;
  assert.equal(cargoVersion(member, "0.3.0"), "0.3.0");
});

test("a section runs to the next version heading or the link list", () => {
  assert.equal(changelogSection(changelog, "0.2.0").body, "### Fixed\n- A thing.");
  assert.equal(changelogSection(changelog, "0.1.0").body, "");
  assert.equal(changelogSection(changelog, "9.9.9"), null);
});

test("a consistent release passes and yields its notes", () => {
  const result = check("0.2.0", sources({ "Cargo.toml": "0.2.0", "extension/manifest.json": "0.2.0" }));
  assert.deepEqual(result.problems, []);
  assert.equal(result.notes, "### Fixed\n- A thing.");
});

test("every mismatch is reported", () => {
  const { problems } = check("0.1.0", sources({ "Cargo.toml": "0.2.0", "extension/manifest.json": undefined }));
  assert.equal(problems.length, 4);
  assert.match(problems[0], /Cargo.toml says 0.2.0/);
  assert.match(problems[1], /says nothing/);
  assert.match(problems[2], /no date/);
  assert.match(problems[3], /empty/);
  assert.match(check("v2", sources({})).problems[0], /not a version/);
});
