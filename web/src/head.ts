// The page head's texts per language, written into each page's copy of
// index.html by prerender.mjs (title, descriptions, the address and its
// alternates). English is index.html's own; the Arabic comes from ar.ts
// like the page's.
import { pagePath, translate, type Lang } from "./i18n";

const SITE = "https://gala-os.com";

export type Head = {
  lang: Lang;
  dir: "ltr" | "rtl";
  url: string;
  title: string;
  description: string;
  shortDescription: string;
  imageAlt: string;
  /** Every language's address, for hreflang. */
  alternates: { lang: Lang; url: string }[];
};

export function head(lang: Lang): Head {
  const t = (s: string) => translate(lang, s);
  return {
    lang,
    dir: lang === "ar" ? "rtl" : "ltr",
    url: SITE + pagePath[lang],
    title: t("gala — a Rust kernel that runs Windows programs"),
    description: t(
      "gala is a from-scratch x86_64 operating system kernel in Rust that runs unmodified Windows programs on its own clean-room system DLLs, with a Linux personality on the same core.",
    ),
    shortDescription: t(
      "A from-scratch x86_64 kernel in Rust. Unmodified Windows programs, clean-room system DLLs, a Linux personality on the same core.",
    ),
    imageAlt: t("gala's desktop with the start menu open: pinned Windows apps, and Ubuntu under Linux distributions."),
    alternates: (["en", "ar"] as Lang[]).map((l) => ({ lang: l, url: SITE + pagePath[l] })),
  };
}
