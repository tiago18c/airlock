// Serves dist/ on the loopback only. A content script does not run on file://,
// so the page needs a server to meet the extension.
//
//   node serve.mjs [port]      (default 8124, or PORT)
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { dirname, extname, join, normalize, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

export const DIST = join(dirname(fileURLToPath(import.meta.url)), "dist");
const host = "127.0.0.1";

const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".map": "application/json; charset=utf-8",
  ".png": "image/png",
};

/** Serves `root` on 127.0.0.1:`port` (0 for any). Resolves with the server
 * once it listens; `server.address().port` is the port taken. */
export function serve(port, root = DIST) {
  const server = createServer(async (request, response) => {
    if (request.method !== "GET" && request.method !== "HEAD") {
      response.writeHead(405).end();
      return;
    }
    const path = decodeURIComponent(new URL(request.url ?? "/", "http://localhost").pathname);
    const file = normalize(join(root, path.endsWith("/") ? `${path}index.html` : path));
    // Nothing outside dist/ is served, whatever the path says.
    if (file !== root && !file.startsWith(root + sep)) {
      response.writeHead(403).end();
      return;
    }
    try {
      const body = await readFile(file);
      response.writeHead(200, {
        "content-type": TYPES[extname(file)] ?? "application/octet-stream",
        "cache-control": "no-store",
      });
      response.end(request.method === "HEAD" ? undefined : body);
    } catch {
      response.writeHead(404, { "content-type": "text/plain; charset=utf-8" }).end("Not found. Run npm run build first.\n");
    }
  });
  return new Promise((done, fail) => {
    server.once("error", fail);
    server.listen(port, host, () => done(server));
  });
}

// Run only as `node serve.mjs`, not when imported.
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const port = Number(process.argv[2] ?? process.env.PORT ?? 8124);
  const server = await serve(port);
  console.log(`Serving ${DIST} at http://${host}:${server.address().port}/`);
}
