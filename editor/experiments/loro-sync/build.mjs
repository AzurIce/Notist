import { build } from "esbuild";
import { writeFile } from "node:fs/promises";
const result = await build({ entryPoints: ["client.mjs"], bundle: true, format: "esm", outfile: "dist/client.js", sourcemap: true, metafile: true });
await writeFile("dist/meta.json", JSON.stringify(result.metafile, null, 2));
