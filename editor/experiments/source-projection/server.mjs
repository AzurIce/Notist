import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { resolve, extname, dirname, sep } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(fileURLToPath(import.meta.url));
const language = resolve(root, "../../pkg");
const kernel = resolve(root, "../../pkg-core");
const types = { ".html": "text/html", ".css": "text/css", ".js": "text/javascript", ".mjs": "text/javascript", ".wasm": "application/wasm" };
export async function serveProjection(req, res) {
    try {
      const pathname = decodeURIComponent(new URL(req.url, "http://localhost").pathname);
      const isLanguage = pathname.startsWith("/language/");
      const isKernel = pathname.startsWith("/kernel/");
      const base = isLanguage ? language : isKernel ? kernel : root;
      const path = resolve(base, "." + (isLanguage ? pathname.slice(9) : isKernel ? pathname.slice(7) : pathname === "/" ? "/index.html" : pathname));
      if (!path.startsWith(base + sep) || (!isLanguage && !isKernel && !["index.html", "style.css", "language-worker.js", "dist/app.js", "dist/app.js.map"].includes(path.slice(base.length + 1)))) {
        res.writeHead(404); res.end(); return;
      }
      const body = await readFile(path);
      res.writeHead(200, { "Content-Type": types[extname(path)] || "application/octet-stream", "Cache-Control": "no-store" });
      res.end(body);
    } catch { res.writeHead(404); res.end("Not found"); }
}
export function startServer(port = 4173) {
  const server = createServer(serveProjection);
  return new Promise(resolveReady => server.listen(port, "127.0.0.1", () => resolveReady(server)));
}
if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const server = await startServer(Number(process.env.PORT || 4173));
  console.log(`Notist projection: http://127.0.0.1:${server.address().port}`);
}
