import { renderToString } from "react-dom/server";
import { App } from "./App";
import { missing, type Lang } from "./i18n";

export function render(lang: Lang): string {
  return renderToString(<App lang={lang} />);
}

// What the Arabic page looked up and ar.ts lacks, for prerender.mjs to
// refuse the build over.
export { missing };
export { head } from "./head";

// Re-exported so prerender.mjs can write dist/llms.txt from the same data
// the page is built from, without a second copy of the facts.
export { llmsText } from "./llms";
