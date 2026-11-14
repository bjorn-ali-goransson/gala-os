// Renders the React page to static HTML and writes it into dist/index.html,
// so the deployed site is real markup that needs no JavaScript to read.
// Also writes the two files crawlers read alongside it, sitemap.xml and
// llms.txt, both generated here so they cannot drift from the page.
// Run after both `vite build` (client) and `vite build --ssr` (server).
import { readFileSync, writeFileSync, rmSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const SITE = "https://gala-os.com/";

const here = dirname(fileURLToPath(import.meta.url));
const templatePath = resolve(here, "dist/index.html");
const template = readFileSync(templatePath, "utf8");

// pathToFileURL: a bare absolute path is not an importable URL on Windows.
const { render, llmsText } = await import(
  pathToFileURL(resolve(here, "dist-ssr/entry-server.js")).href
);
const markup = render();

const placeholder = "<!--app-html-->";
if (!template.includes(placeholder)) {
  throw new Error(`dist/index.html has no ${placeholder} placeholder to fill`);
}
writeFileSync(templatePath, template.replace(placeholder, markup));

// One page, so one URL. `lastmod` is the build date rather than a date
// written by hand, which would be wrong the moment the page changed.
const lastmod = new Date().toISOString().slice(0, 10);
writeFileSync(
  resolve(here, "dist/sitemap.xml"),
  `<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url>
    <loc>${SITE}</loc>
    <lastmod>${lastmod}</lastmod>
    <changefreq>weekly</changefreq>
    <priority>1.0</priority>
  </url>
</urlset>
`,
);

const llms = llmsText();
writeFileSync(resolve(here, "dist/llms.txt"), llms);

// The SSR bundle is a build artifact, not something to publish.
rmSync(resolve(here, "dist-ssr"), { recursive: true, force: true });

console.log(
  `prerendered ${markup.length} bytes of markup into dist/index.html, ` +
    `${llms.length} bytes into dist/llms.txt, sitemap lastmod ${lastmod}`,
);
