// Serves dist/ (the web build) for the tests: `node serve.js PORT`.
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../../dist/", import.meta.url));
const types = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript",
  ".wasm": "application/wasm",
  ".css": "text/css",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".ico": "image/x-icon",
};
const port = Number(process.argv[2] || 8095);

createServer(async (req, res) => {
  const path = decodeURIComponent(new URL(req.url, "http://x").pathname);
  const file = normalize(join(root, path.endsWith("/") ? `${path}index.html` : path));
  if (!file.startsWith(root)) {
    res.writeHead(403).end();
    return;
  }
  try {
    const body = await readFile(file);
    res.writeHead(200, { "content-type": types[extname(file)] || "application/octet-stream" }).end(body);
  } catch {
    res.writeHead(404).end("not found (run `npm run build` to build dist/)");
  }
}).listen(port, "127.0.0.1", () => console.log(`serving ${root} on http://127.0.0.1:${port}/`));
