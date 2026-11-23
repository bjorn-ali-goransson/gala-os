import { StrictMode } from "react";
import { hydrateRoot } from "react-dom/client";
import { App } from "./App";
import "./styles.css";

// The markup is already in the page (prerender.mjs); the client only
// attaches behaviour to it, in the language the page was rendered in.
hydrateRoot(
  document.getElementById("root")!,
  <StrictMode>
    <App lang={document.documentElement.lang === "ar" ? "ar" : "en"} />
  </StrictMode>,
);
