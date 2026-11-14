// llms.txt (https://llmstxt.org): a plain-text brief of the whole site for
// agents that would otherwise have to parse the page. It is generated from
// `content.ts` at build time, so it is the same set of facts the page shows
// and cannot drift from it. Written by prerender.mjs into dist/llms.txt.
import {
  architecture,
  decisions,
  layers,
  programs,
  type Layer,
} from "./content";

const layerOrder: Layer[] = ["core", "nt", "shell", "linux", "project"];

const statusWord = {
  works: "works",
  building: "being built",
  planned: "planned",
} as const;

export function llmsText(): string {
  const out: string[] = [];

  out.push("# gala");
  out.push("");
  out.push(
    "> gala is a from-scratch x86_64 operating system kernel written in Rust " +
      "that runs unmodified Windows programs on its own clean-room system " +
      "DLLs, with a Linux personality on the same core. It is not a Windows " +
      "fork and contains no Microsoft code: compatibility is at the ABI " +
      "boundary only, and every behaviour is established by testing against " +
      "a real Windows build.",
  );
  out.push("");
  out.push(
    "Source and documentation: https://github.com/bjorn-ali-goransson/gala-os",
  );
  out.push("");

  out.push("## What runs on it");
  out.push("");
  for (const p of programs) {
    const name = p.version ? `${p.name} ${p.version}` : p.name;
    out.push(`- **${name}**: ${p.what} (scenarios: ${p.scenarios.join(", ")})`);
  }
  out.push("");

  out.push("## Architecture");
  out.push("");
  for (const l of layerOrder) {
    out.push(`### ${layers[l].name}`);
    out.push("");
    out.push(layers[l].blurb);
    out.push("");
    for (const e of architecture.filter((a) => a.layer === l)) {
      out.push(`- **${e.title}** (${statusWord[e.status]}): ${e.summary}`);
    }
    out.push("");
  }

  out.push("## Design decisions");
  out.push("");
  for (const d of decisions) {
    out.push(`### ${d.title}`);
    out.push("");
    out.push(d.body);
    out.push("");
  }

  return out.join("\n");
}
