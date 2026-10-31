// node --test scripts/
import assert from "node:assert/strict";
import { test } from "node:test";
import { added, cargoPackages, isTooNew, npmPackages } from "./check-dep-age.mjs";

const cargoLock = `version = 4

[[package]]
name = "airlock"
version = "0.1.0"
dependencies = [
 "serde",
]

[[package]]
name = "serde"
version = "1.0.200"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "00"

[[package]]
name = "vector-core"
version = "0.1.0"
source = "git+https://github.com/tiago18c/vector?rev=abc#abc"
`;

const npmLock = JSON.stringify({
  packages: {
    "": { name: "extension", version: "0.1.0" },
    "node_modules/esbuild": {
      version: "0.25.0",
      resolved: "https://registry.npmjs.org/esbuild/-/esbuild-0.25.0.tgz",
    },
    "node_modules/@types/chrome": {
      version: "0.3.6",
      resolved: "https://registry.npmjs.org/@types/chrome/-/chrome-0.3.6.tgz",
    },
    "node_modules/a/node_modules/b": {
      version: "2.0.0",
      resolved: "https://registry.npmjs.org/b/-/b-2.0.0.tgz",
    },
    "node_modules/alias": {
      name: "real-name",
      version: "1.0.0",
      resolved: "https://registry.npmjs.org/real-name/-/real-name-1.0.0.tgz",
    },
    "node_modules/local": { resolved: "../local", link: true },
  },
});

test("only crates from a registry are read from Cargo.lock", () => {
  assert.deepEqual(cargoPackages(cargoLock), [{ name: "serde", version: "1.0.200" }]);
});

test("a Cargo.lock checked out with CRLF reads the same", () => {
  assert.deepEqual(cargoPackages(cargoLock.replaceAll("\n", "\r\n")), cargoPackages(cargoLock));
});

test("only registry packages are read from package-lock.json, under their real names", () => {
  assert.deepEqual(npmPackages(npmLock), [
    { name: "esbuild", version: "0.25.0" },
    { name: "@types/chrome", version: "0.3.6" },
    { name: "b", version: "2.0.0" },
    { name: "real-name", version: "1.0.0" },
  ]);
});

test("a lockfile that did not exist has no packages", () => {
  assert.deepEqual(cargoPackages(""), []);
  assert.deepEqual(npmPackages("{}"), []);
});

test("a package counts when its version is new to the lockfile, once", () => {
  const before = [{ name: "a", version: "1.0.0" }, { name: "b", version: "1.0.0" }];
  const after = [
    { name: "a", version: "1.0.0" },
    { name: "b", version: "1.1.0" },
    { name: "b", version: "1.1.0" },
    { name: "c", version: "0.1.0" },
  ];
  assert.deepEqual(added(before, after), [
    { name: "b", version: "1.1.0" },
    { name: "c", version: "0.1.0" },
  ]);
});

test("a version is too new until seven full days have passed", () => {
  const now = new Date("2026-10-30T12:00:00Z");
  assert.equal(isTooNew(new Date("2026-10-23T12:00:01Z"), now), true);
  assert.equal(isTooNew(new Date("2026-10-23T12:00:00Z"), now), false);
  assert.equal(isTooNew(new Date("2026-01-01T00:00:00Z"), now), false);
});
