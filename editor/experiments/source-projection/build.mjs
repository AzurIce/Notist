import { build } from "esbuild";
await build({ entryPoints: ["app.js"], bundle: true, format: "esm", sourcemap: true, outdir: "dist", external: ["/kernel/*"] });
