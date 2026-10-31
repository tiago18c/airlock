// Checks that a change to the lockfiles brings in no version published in the
// last week, so a compromised release has time to be noticed and pulled.
//
//   node scripts/check-dep-age.mjs <base-revision>
//
// Compares Cargo.lock and extension/package-lock.json with the base revision
// and asks the registries when each new version was published. A registry that
// cannot answer fails the check: it says nothing about the version's age.
// See docs/releasing.md#updating-dependencies.
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export const MIN_AGE_DAYS = 7;

const DAY_MS = 24 * 60 * 60 * 1000;
const USER_AGENT = "airlock dependency age check (https://github.com/tiago18c/airlock)";
// crates.io asks automated clients for at most one request a second.
const CRATES_IO_DELAY_MS = 1000;

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

/** The crates.io packages in a Cargo.lock. Path and git packages have no publish date. */
export function cargoPackages(lock) {
  return lock
    .split(/^\[\[package\]\]\r?$/m)
    .slice(1)
    .filter((block) => /^source = "registry\+/m.test(block))
    .map((block) => ({
      name: block.match(/^name = "([^"]+)"/m)[1],
      version: block.match(/^version = "([^"]+)"/m)[1],
    }));
}

/** The npm registry packages in a package-lock.json. */
export function npmPackages(lock) {
  return Object.entries(JSON.parse(lock).packages ?? {})
    .filter(([, entry]) => entry.resolved?.startsWith("https://registry.npmjs.org/"))
    .map(([path, entry]) => ({
      name: entry.name ?? path.slice(path.lastIndexOf("node_modules/") + "node_modules/".length),
      version: entry.version,
    }));
}

/** The packages in `after` at a version `before` did not have. */
export function added(before, after) {
  const known = new Set(before.map(({ name, version }) => `${name}@${version}`));
  const seen = new Set();
  return after.filter(({ name, version }) => {
    const key = `${name}@${version}`;
    if (known.has(key) || seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

/** Whether a version published at `published` is still inside the waiting period. */
export function isTooNew(published, now) {
  return now.getTime() - published.getTime() < MIN_AGE_DAYS * DAY_MS;
}

async function getJson(url) {
  for (let attempt = 0; ; attempt++) {
    const response = await fetch(url, { headers: { "User-Agent": USER_AGENT } });
    if (response.ok) return response.json();
    const retry = response.status === 429 || response.status >= 500;
    if (!retry || attempt === 2) throw new Error(`${url} answered ${response.status}`);
    await new Promise((resolve) => setTimeout(resolve, 5000 * (attempt + 1)));
  }
}

async function crateDate({ name, version }) {
  const body = await getJson(`https://crates.io/api/v1/crates/${encodeURIComponent(name)}/${encodeURIComponent(version)}`);
  await new Promise((resolve) => setTimeout(resolve, CRATES_IO_DELAY_MS));
  return body.version.created_at;
}

const npmDocuments = new Map();

async function npmDate({ name, version }) {
  if (!npmDocuments.has(name)) {
    npmDocuments.set(name, await getJson(`https://registry.npmjs.org/${encodeURIComponent(name)}`));
  }
  return npmDocuments.get(name).time[version];
}

/** A file as it is at a revision, or nothing if it did not exist there. */
function atRevision(revision, path) {
  try {
    return execFileSync("git", ["show", `${revision}:${path}`], {
      cwd: root,
      encoding: "utf8",
      maxBuffer: 64 * 1024 * 1024,
      stdio: ["ignore", "pipe", "ignore"],
    });
  } catch {
    return null;
  }
}

const lockfiles = [
  { path: "Cargo.lock", packages: cargoPackages, date: crateDate, empty: "" },
  { path: "extension/package-lock.json", packages: npmPackages, date: npmDate, empty: "{}" },
];

async function main(base) {
  if (!base) throw new Error("usage: check-dep-age.mjs <base-revision>");
  const now = new Date();
  const problems = [];
  for (const { path, packages, date, empty } of lockfiles) {
    const before = packages(atRevision(base, path) ?? empty);
    const after = packages(readFileSync(join(root, path), "utf8"));
    for (const dependency of added(before, after)) {
      const label = `${dependency.name} ${dependency.version}`;
      try {
        const published = new Date(await date(dependency));
        if (Number.isNaN(published.getTime())) throw new Error("no publish date");
        const days = (now.getTime() - published.getTime()) / DAY_MS;
        console.log(`${label}: published ${days.toFixed(1)} days ago`);
        if (isTooNew(published, now)) problems.push(`${label} in ${path} is ${days.toFixed(1)} days old`);
      } catch (error) {
        problems.push(`${label} in ${path} could not be checked: ${error.message}`);
      }
    }
  }
  for (const problem of problems) console.log(`::error::${problem}`);
  if (problems.length > 0) {
    console.log(
      `Versions have to be ${MIN_AGE_DAYS} days old. Run this again once they are. ` +
        "If the update fixes a vulnerability, label the pull request 'security-override'.",
    );
    process.exit(1);
  }
  console.log("No version newer than the waiting period.");
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  await main(process.argv[2]);
}
