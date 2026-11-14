// Renders the React page to static HTML and writes it into dist/index.html,
// so the deployed site is real markup that needs no JavaScript to read.
// Run after both `vite build` (client) and `vite build --ssr` (server).
import { readFileSync, writeFileSync, rmSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const templatePath = resolve(here, "dist/index.html");
const template = readFileSync(templatePath, "utf8");

// pathToFileURL: a bare absolute path is not an importable URL on Windows.
const { render } = await import(
  pathToFileURL(resolve(here, "dist-ssr/entry-server.js")).href
);
const markup = render();

const placeholder = "<!--app-html-->";
if (!template.includes(placeholder)) {
  throw new Error(`dist/index.html has no ${placeholder} placeholder to fill`);
}
writeFileSync(templatePath, template.replace(placeholder, markup));

// The SSR bundle is a build artifact, not something to publish.
rmSync(resolve(here, "dist-ssr"), { recursive: true, force: true });

console.log(`prerendered ${markup.length} bytes of markup into dist/index.html`);
