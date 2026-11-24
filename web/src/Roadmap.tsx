// The roadmap section: docs/ROADMAP.md's status tables, as `roadmap.ts`
// reads them, each item with its state.
import { useT } from "./i18n";
import { roadmap, roadmapStateLabel, type RoadmapState } from "./roadmap";

const STATES: RoadmapState[] = ["done", "progress", "planned", "later", "parked"];

/** The tag colours the rest of the page uses for a state. */
const TAG: Record<RoadmapState, string> = {
  done: "works",
  progress: "building",
  planned: "planned",
  later: "planned",
  parked: "planned",
};

function RoadmapTag({ state }: { state: RoadmapState }) {
  const t = useT();
  return <span className={`tag tag-${TAG[state]}`}>{t(roadmapStateLabel[state])}</span>;
}

export function RoadmapBody() {
  const t = useT();
  const all = roadmap.flatMap((g) => g.items);
  return (
    <>
      <ul className="rm-counts">
        {STATES.filter((s) => all.some((i) => i.state === s)).map((s) => (
          <li key={s}>
            <b>{all.filter((i) => i.state === s).length}</b> <RoadmapTag state={s} />
          </li>
        ))}
      </ul>
      {roadmap.map((g) => (
        <div className="rm-group" key={g.title}>
          <h3 className="sub">{t(g.title)}</h3>
          <ul className="rm-items">
            {g.items.map((i) => (
              <li key={`${i.id}-${i.title}`} className={`rm-${i.state}`}>
                <span className="rm-title">
                  {i.id && <code>{i.id}</code>} {t(i.title)}
                </span>
                <RoadmapTag state={i.state} />
              </li>
            ))}
          </ul>
        </div>
      ))}
    </>
  );
}
