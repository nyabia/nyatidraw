import { createRequire } from "node:module";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

const requireFromDocs = createRequire(new URL("../../site-docs/package.json", import.meta.url));
const { build } = requireFromDocs("esbuild");
const entry = fileURLToPath(new URL("./entry.js", import.meta.url));
const output = fileURLToPath(new URL("../../target/userscript/nyatidraw.user.js", import.meta.url));
const metadata = await readFile(new URL("./metadata.txt", import.meta.url), "utf8");
const mitLicense = await readFile(new URL("../../LICENSE-MIT", import.meta.url), "utf8");
const result = await build({
  entryPoints: [entry],
  bundle: true,
  format: "iife",
  platform: "browser",
  target: ["es2022"],
  define: { "import.meta.url": JSON.stringify("https://nyabia.github.io/nyatidraw/draw/integration/nyatidraw.js") },
  legalComments: "none",
  write: false,
  logLevel: "warning",
});
const bundle = result.outputFiles[0].text;
if (bundle.includes("import.meta")) throw new Error("Classic userscript bundle still contains import.meta");
await mkdir(fileURLToPath(new URL("../../target/userscript/", import.meta.url)), { recursive: true });
await writeFile(output, `${metadata.trimEnd()}\n\n/*\n${mitLicense.trimEnd()}\n*/\n\n${bundle}`, "utf8");
console.log(`Built ${output}`);
