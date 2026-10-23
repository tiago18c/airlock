// Bundles the tests with esbuild (the same loader settings as the extension build)
// and runs them with Node's built-in test runner.
import * as esbuild from "esbuild";
import { spawnSync } from "node:child_process";
import { readdirSync, rmSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(fileURLToPath(import.meta.url));
const out = join(root, "../dist-test");
rmSync(out, { recursive: true, force: true });

const tests = readdirSync(root).filter((name) => name.endsWith(".test.ts"));
await esbuild.build({
  entryPoints: tests.map((name) => join(root, name)),
  bundle: true,
  outdir: out,
  format: "esm",
  platform: "node",
  outExtension: { ".js": ".mjs" },
  loader: { ".png": "dataurl" },
  logLevel: "warning",
});

const built = tests.map((name) => join(out, name.replace(/\.ts$/, ".mjs")));
const run = spawnSync(process.execPath, ["--test", ...built], { stdio: "inherit" });
process.exit(run.status ?? 1);
