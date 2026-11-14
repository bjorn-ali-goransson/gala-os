import { renderToString } from "react-dom/server";
import { App } from "./App";

export function render(): string {
  return renderToString(<App />);
}

// Re-exported so prerender.mjs can write dist/llms.txt from the same data
// the page is built from, without a second copy of the facts.
export { llmsText } from "./llms";
