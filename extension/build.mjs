import * as esbuild from "esbuild";
import { cpSync, mkdirSync, rmSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(fileURLToPath(import.meta.url));
const dist = join(root, "dist");

rmSync(dist, { recursive: true, force: true });
mkdirSync(join(dist, "icons"), { recursive: true });

await esbuild.build({
  entryPoints: {
    background: join(root, "src/background.ts"),
    "content-bridge": join(root, "src/content-bridge.ts"),
    "content-main": join(root, "src/content-main.ts"),
    popup: join(root, "src/popup.ts"),
  },
  bundle: true,
  outdir: dist,
  format: "iife",
  target: "chrome111",
  sourcemap: true,
  loader: { ".png": "dataurl" },
  logLevel: "info",
});

cpSync(join(root, "manifest.json"), join(dist, "manifest.json"));
cpSync(join(root, "src/popup.html"), join(dist, "popup.html"));
cpSync(join(root, "src/popup.css"), join(dist, "popup.css"));
// The popup shares the app's design tokens.
cpSync(join(root, "../crates/app-ui-kit/assets/tokens.css"), join(dist, "tokens.css"));
// The extension carries the base Airlock icon.
for (const size of [16, 32, 48, 128]) {
  cpSync(join(root, `../apps/airlock/icons/${size}x${size}.png`), join(dist, `icons/${size}.png`));
}
