// llms.txt (https://llmstxt.org): a plain-text brief of the whole site for
// agents that would otherwise have to parse the page. It is generated from
// `content.ts` at build time, so it is the same set of facts the page shows
// and cannot drift from it. Written by prerender.mjs into dist/llms.txt.
import {
  componentGroups,
  debugCapabilities,
  decisions,
  focus,
  imageDemo,
  images,
  imagesUrl,
  insideTools,
  linuxPrograms,
  liveCapabilities,
  planned,
  repoTools,
  statusLabel,
  toolGroups,
  usb,
  windowsPrograms,
  type Program,
} from "./content";
import { coverage, pct } from "./Coverage";
import { roadmap, roadmapStateLabel } from "./roadmap";

function programs(out: string[], list: Program[]) {
  for (const p of list) {
    const name = p.version ? `${p.name} ${p.version}` : p.name;
    out.push(`- **${name}**: ${p.what} (scenarios: ${p.scenarios.join(", ")})`);
  }
  out.push("");
}

export function llmsText(): string {
  const out: string[] = [];

  out.push("# gala");
  out.push("");
  out.push(
    "> gala is a from-scratch operating system kernel written in Rust " +
      "that runs unmodified Windows programs on its own clean-room system " +
      "DLLs, with a Linux personality on the same core. It is not a Windows " +
      "fork and contains no Microsoft code: compatibility is at the ABI " +
      "boundary only, and every behaviour is established by testing against " +
      "a real Windows build. x86_64 works; an ARM64 port is in progress.",
  );
  out.push("");

  out.push("## In progress now");
  out.push("");
  for (const f of focus) {
    out.push(`### ${f.title}`);
    out.push("");
    out.push(f.lede);
    out.push("");
    for (const s of f.steps) {
      const detail = s.detail ? `: ${s.detail}` : "";
      out.push(`- ${s.label} (${statusLabel[s.status].toLowerCase()})${detail}`);
    }
    out.push("");
  }

  out.push("## Beyond a reboot");
  out.push("");
  for (const c of [...liveCapabilities, ...debugCapabilities]) {
    const proof = c.proof ? ` (checked by: ${c.proof.join(", ")})` : "";
    out.push(`- **${c.name}** (${statusLabel[c.status].toLowerCase()}): ${c.what} ${c.how}${proof}`);
  }
  out.push("");

  out.push("## Components");
  out.push("");
  for (const g of componentGroups) {
    out.push(`### ${g.title}`);
    out.push("");
    out.push(g.intro);
    out.push("");
    for (const c of g.items) {
      const path = c.path ? ` [${c.path}]` : "";
      out.push(
        `- **${c.name}** (${statusLabel[c.status].toLowerCase()})${path}: ${c.what}`,
      );
    }
    out.push("");
  }

  out.push("## What runs on it");
  out.push("");
  out.push("### Windows programs, unmodified");
  out.push("");
  programs(out, windowsPrograms);
  out.push("### Linux programs");
  out.push("");
  programs(out, linuxPrograms);

  out.push("## Planned");
  out.push("");
  for (const p of planned) out.push(`- **${p.name}**: ${p.what}`);
  out.push("");

  out.push("## API coverage");
  out.push("");
  const c = coverage;
  out.push(
    `gala's ${c.gala.dlls} DLLs export ${c.gala.real} of the ${c.windows.exports} named functions of ` +
      `the ${c.windows.dlls} 64-bit DLLs in ${c.windows.product}'s System32 (build ${c.windows.build}), ` +
      `${pct(c.gala.real, c.windows.exports, 1)}%, legacy included, and ${c.gala.stubs} more as stubs that only ` +
      `report failure. Of the Windows functions each program below imports (static imports, resolved before ` +
      `the program runs; then with its delay imports, resolved on first call), gala exports for real:`,
  );
  out.push("");
  for (const p of c.programs) {
    out.push(
      `- **${p.name}**: ${pct(p.static.real, p.static.imports)}% of static imports ` +
        `(${p.static.real} of ${p.static.imports}), ${pct(p.real, p.imports)}% with delay imports (${p.real} of ${p.imports})`,
    );
  }
  out.push("");
  out.push(
    "Read from import and export tables only: functions found with GetProcAddress and COM methods are not " +
      "seen, and an export that exists may do only part of its job. " +
      `Measured ${c.measured}, gala at commit ${c.galaCommit}.`,
  );
  out.push("");

  out.push("## Roadmap");
  out.push("");
  for (const g of roadmap) {
    out.push(`### ${g.title}`);
    out.push("");
    for (const i of g.items) {
      const id = i.id ? `${i.id}: ` : "";
      out.push(`- ${id}${i.title} (${roadmapStateLabel[i.state].toLowerCase()})`);
    }
    out.push("");
  }

  out.push("## Tooling");
  out.push("");
  for (const g of toolGroups) {
    out.push(`### ${g.title}`);
    out.push("");
    out.push(g.intro);
    out.push("");
    for (const t of g.tools) out.push(`- \`${t.cmd}\`: ${t.what}`);
    out.push("");
  }
  out.push("### Beside the code");
  out.push("");
  for (const t of repoTools) out.push(`- **${t.name}**: ${t.what}`);
  out.push("");
  out.push("### Inside gala");
  out.push("");
  for (const t of insideTools) out.push(`- \`${t.name}\`: ${t.what}`);
  out.push("");

  out.push("## Try it");
  out.push("");
  out.push(imageDemo);
  out.push("");
  for (const i of images) {
    out.push(`- **${i.arch}** (${i.size}): ${imagesUrl}/${i.file} (SHA-256: ${imagesUrl}/${i.file}.sha256). ${i.what} Needs ${i.needs}`);
    out.push("");
    out.push("  ```sh");
    for (const line of i.qemu.split("\n")) out.push(`  ${line}`);
    out.push("  ```");
    out.push("");
  }

  out.push("## Run it from a USB stick");
  out.push("");
  out.push(usb.intro);
  out.push("");
  out.push("You need:");
  for (const n of usb.needs) out.push(`- ${n}`);
  out.push("");
  usb.steps.forEach((step, i) => {
    out.push(`### ${i + 1}. ${step.title}`);
    out.push("");
    out.push(step.body);
    out.push("");
    for (const w of step.ways ?? []) {
      out.push(`- **${w.name}**: ${w.how}`);
      if (w.cmds) {
        out.push("");
        out.push("  ```sh");
        for (const c of w.cmds) out.push(`  ${c}`);
        out.push("  ```");
      }
      out.push("");
    }
  });
  out.push("Limits for now:");
  for (const l of usb.limits) out.push(`- ${l}`);
  out.push("");

  out.push("## Principles");
  out.push("");
  for (const d of decisions) {
    out.push(`### ${d.title}`);
    out.push("");
    out.push(d.body);
    out.push("");
  }

  return out.join("\n");
}
