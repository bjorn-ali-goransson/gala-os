// Renders the React page to static HTML, once per language: English into
// dist/index.html, Arabic into dist/ar/index.html (right to left), so the
// deployed site is real markup that needs no JavaScript to read. Also
// writes the two files crawlers read alongside it, sitemap.xml and
// llms.txt, both generated here so they cannot drift from the page.
// Run after both `vite build` (client) and `vite build --ssr` (server).
//
// The build fails when the Arabic page looked up a text that src/ar.ts
// does not translate, naming each: the Arabic page never quietly shows
// English, nor falls behind a change to the English.
import { readFileSync, writeFileSync, rmSync, mkdirSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const templatePath = resolve(here, "dist/index.html");
const template = readFileSync(templatePath, "utf8");

// pathToFileURL: a bare absolute path is not an importable URL on Windows.
const { render, head, missing, llmsText } = await import(
  pathToFileURL(resolve(here, "dist-ssr/entry-server.js")).href
);

const escape = (s) =>
  s.replaceAll("&", "&amp;").replaceAll('"', "&quot;").replaceAll("<", "&lt;").replaceAll(">", "&gt;");

/** `html` with the one match of `pattern` replaced; anything else fails. */
function once(html, pattern, replacement, what) {
  const count = (html.match(new RegExp(pattern.source, "g")) ?? []).length;
  if (count !== 1) throw new Error(`index.html: expected one ${what}, found ${count}`);
  return html.replace(pattern, () => replacement);
}

function page(lang) {
  const h = head(lang);
  let html = template;
  html = once(html, /<html lang="en">/, `<html lang="${h.lang}" dir="${h.dir}">`, "<html lang>");
  html = once(html, /<title>[^<]*<\/title>/, `<title>${escape(h.title)}</title>`, "<title>");
  const meta = (attr, name, value) => {
    html = once(
      html,
      new RegExp(`<meta\\s+${attr}="${name}"\\s+content="[^"]*"\\s*/>`),
      `<meta ${attr}="${name}" content="${escape(value)}" />`,
      `meta ${name}`,
    );
  };
  meta("name", "description", h.description);
  meta("property", "og:title", h.title);
  meta("property", "og:description", h.shortDescription);
  meta("property", "og:url", h.url);
  meta("property", "og:locale", h.lang);
  meta("property", "og:image:alt", h.imageAlt);
  meta("name", "twitter:title", h.title);
  meta("name", "twitter:description", h.shortDescription);
  html = once(html, /<link rel="canonical" href="[^"]*" \/>/, `<link rel="canonical" href="${h.url}" />`, "canonical");
  const alternates = [
    ...h.alternates.map((a) => `<link rel="alternate" hreflang="${a.lang}" href="${a.url}" />`),
    `<link rel="alternate" hreflang="x-default" href="${h.alternates[0].url}" />`,
  ].join("\n    ");
  html = once(html, /<!--alternates-->/, alternates, "<!--alternates--> placeholder");
  return once(html, /<!--app-html-->/, render(lang), "<!--app-html--> placeholder");
}

const english = page("en");
const arabic = page("ar");
const untranslated = missing();
if (untranslated.length > 0) {
  throw new Error(
    `src/ar.ts has no Arabic for ${untranslated.length} English text(s) the page shows:\n` +
      untranslated.map((s) => `  ${JSON.stringify(s)}`).join("\n"),
  );
}
writeFileSync(templatePath, english);
mkdirSync(resolve(here, "dist/ar"), { recursive: true });
writeFileSync(resolve(here, "dist/ar/index.html"), arabic);

// One URL per language, each naming the other. `lastmod` is the build
// date rather than a date written by hand, which would be wrong the
// moment the page changed.
const lastmod = new Date().toISOString().slice(0, 10);
const pages = head("en").alternates;
const links = pages
  .map((a) => `    <xhtml:link rel="alternate" hreflang="${a.lang}" href="${a.url}" />`)
  .join("\n");
const urls = pages
  .map(
    (p) => `  <url>
    <loc>${p.url}</loc>
${links}
    <lastmod>${lastmod}</lastmod>
    <changefreq>weekly</changefreq>
    <priority>${p.lang === "en" ? "1.0" : "0.9"}</priority>
  </url>`,
  )
  .join("\n");
writeFileSync(
  resolve(here, "dist/sitemap.xml"),
  `<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9" xmlns:xhtml="http://www.w3.org/1999/xhtml">
${urls}
</urlset>
`,
);

const llms = llmsText();
writeFileSync(resolve(here, "dist/llms.txt"), llms);

// The SSR bundle is a build artifact, not something to publish.
rmSync(resolve(here, "dist-ssr"), { recursive: true, force: true });

console.log(
  `prerendered ${english.length} bytes into dist/index.html and ${arabic.length} into dist/ar/index.html, ` +
    `${llms.length} bytes into dist/llms.txt, sitemap lastmod ${lastmod}`,
);
