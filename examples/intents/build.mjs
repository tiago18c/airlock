// Bundles the test page into dist/, with Airlock's design tokens beside it.
import * as esbuild from "esbuild";
import { cpSync, mkdirSync, rmSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(fileURLToPath(import.meta.url));
const repo = join(root, "../..");
const dist = join(root, "dist");

rmSync(dist, { recursive: true, force: true });
mkdirSync(dist, { recursive: true });

await esbuild.build({
  entryPoints: { page: join(root, "src/main.ts") },
  bundle: true,
  outdir: dist,
  format: "esm",
  platform: "browser",
  target: "es2022",
  sourcemap: true,
  // The extension's wallet class is bundled for the dev transport. Its imports
  // resolve here, so the extension needs no install of its own.
  nodePaths: [join(root, "node_modules")],
  loader: { ".png": "dataurl" },
  define: { global: "globalThis", "process.env.NODE_ENV": '"production"' },
  logLevel: "info",
});

cpSync(join(root, "src/index.html"), join(dist, "index.html"));
cpSync(join(root, "src/page.css"), join(dist, "page.css"));
// The page shares the app's design tokens, as the extension's popup does.
cpSync(join(repo, "crates/app-ui-kit/assets/tokens.css"), join(dist, "tokens.css"));
cpSync(join(repo, "apps/airlock/icons/128x128.png"), join(dist, "icon.png"));
