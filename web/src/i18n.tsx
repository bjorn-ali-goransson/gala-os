// The site's languages. English is the source: every text is written in
// English in content.ts, App.tsx and Diagrams.tsx, and the Arabic page
// looks each one up in `ar.ts`, a dictionary keyed by the English text.
// A text the dictionary lacks is recorded (`missing`), and prerender.mjs
// fails the build naming every one, so the Arabic page never quietly
// shows English or falls behind a change to the English. Text that is
// rich (inline code, emphasis) is written in both languages side by side
// with `<Both>`, which TypeScript holds to both.
import { createContext, useContext, type ReactNode } from "react";
import { ar } from "./ar";

export type Lang = "en" | "ar";

export const langs: Lang[] = ["en", "ar"];

/** Where each language's page is served. */
export const pagePath: Record<Lang, string> = { en: "/", ar: "/ar/" };

const LangContext = createContext<Lang>("en");

export function LangProvider({ lang, children }: { lang: Lang; children: ReactNode }) {
  return <LangContext.Provider value={lang}>{children}</LangContext.Provider>;
}

export function useLang(): Lang {
  return useContext(LangContext);
}

const missed = new Set<string>();

/** `text` in `lang`. */
export function translate(lang: Lang, text: string): string {
  if (lang === "en") return text;
  const found = ar[text];
  if (found === undefined) {
    missed.add(text);
    return text;
  }
  return found;
}

/** The texts the Arabic page looked up and `ar.ts` did not have. */
export function missing(): string[] {
  return [...missed];
}

/** The page's translator: `t("English text")`. */
export function useT(): (text: string) => string {
  const lang = useLang();
  return (text) => translate(lang, text);
}

/** A rich text written in both languages. */
export function Both({ en, ar: arabic }: { en: ReactNode; ar: ReactNode }) {
  return <>{useLang() === "ar" ? arabic : en}</>;
}

/**
 * The fields of content.ts that are prose, translated; every other field
 * (ids, file names, commands, scenario names, statuses) is the same in
 * every language. A `name` is translated where the dictionary has it and
 * otherwise kept: most are names of programs (Notepad++, GIMP).
 */
const PROSE = new Set([
  "title",
  "intro",
  "what",
  "how",
  "lede",
  "label",
  "detail",
  "caption",
  "body",
  "size",
  "needs",
  "limits",
]);

export function localize<T>(lang: Lang, value: T, key?: string): T {
  if (lang === "en") return value;
  if (typeof value === "string") {
    if (key !== undefined && PROSE.has(key)) return translate(lang, value) as T;
    if (key === "name" || key === "version") return (ar[value] ?? value) as T;
    return value;
  }
  if (Array.isArray(value)) return value.map((v) => localize(lang, v, key)) as T;
  if (value && typeof value === "object") {
    const out: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(value)) out[k] = localize(lang, v, k);
    return out as T;
  }
  return value;
}
