// The roadmap's headlines, read from docs/ROADMAP.md itself at build time
// (its "Status at a glance" tables), so the site cannot fall behind the
// roadmap or disagree with it. Each row's ID, title and state are taken;
// its note (the detail, in the roadmap's own shorthand) is left to the
// roadmap. A table or state this parser does not know fails the build,
// naming it, rather than quietly dropping a row.
import source from "../../docs/ROADMAP.md?raw";

export type RoadmapState = "done" | "progress" | "planned" | "later" | "parked";

export type RoadmapItem = { id: string; title: string; state: RoadmapState };

export type RoadmapGroup = { title: string; items: RoadmapItem[] };

export const roadmapStateLabel: Record<RoadmapState, string> = {
  done: "Done",
  progress: "In progress",
  planned: "Planned",
  later: "Later",
  parked: "Parked",
};

/** A row's State cell as one of the five states. The cell may say more
 * ("done (follow-ups listed)", "steps 1-2 done; step 3 next"): its first
 * word decides, and a cell that names a step done without being done is
 * in progress. */
function state(cell: string, row: string): RoadmapState {
  const c = cell.trim().toLowerCase();
  if (c.startsWith("done")) return "done";
  if (c.startsWith("in progress")) return "progress";
  if (c.startsWith("planned")) return "planned";
  if (c.startsWith("later")) return "later";
  if (c.startsWith("parked")) return "parked";
  if (/\bdone\b/.test(c)) return "progress";
  throw new Error(`docs/ROADMAP.md: the state "${cell}" of row "${row}" is none the website knows`);
}

/** Markdown inline code and emphasis as plain text. */
function plain(s: string): string {
  return s.replace(/`([^`]*)`/g, "$1").replace(/\*\*([^*]*)\*\*/g, "$1").trim();
}

function parse(md: string): RoadmapGroup[] {
  const start = md.indexOf("\n## Status at a glance");
  if (start < 0) throw new Error('docs/ROADMAP.md: no "## Status at a glance" section');
  const end = md.indexOf("\n## ", start + 1);
  const section = md.slice(start, end < 0 ? undefined : end);
  const groups: RoadmapGroup[] = [];
  let group: RoadmapGroup | null = null;
  let columns: string[] | null = null;
  for (const line of section.split("\n")) {
    if (line.startsWith("### ")) {
      group = { title: line.slice(4).trim(), items: [] };
      columns = null;
      continue;
    }
    if (!group || !line.startsWith("|")) continue;
    const cells = line.split("|").slice(1, -1).map((c) => c.trim());
    if (cells.every((c) => /^-+$/.test(c))) continue;
    if (!columns) {
      columns = cells;
      // A table without a State column (the NT core services, listed by
      // test) is a list of what is done, not of items.
      if (columns.includes("State")) groups.push(group);
      continue;
    }
    const at = (name: string) => cells[columns!.indexOf(name)] ?? "";
    if (!columns.includes("State")) continue;
    const title = plain(at("Title"));
    group.items.push({ id: at("ID") === "—" ? "" : at("ID"), title, state: state(at("State"), title) });
  }
  if (groups.length === 0 || groups.some((g) => g.items.length === 0)) {
    throw new Error("docs/ROADMAP.md: a status table the website could not read");
  }
  return groups;
}

export const roadmap: RoadmapGroup[] = parse(source);
