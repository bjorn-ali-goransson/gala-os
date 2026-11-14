import { useEffect, useState } from "react";
import {
  architecture,
  decisions,
  layers,
  programs,
  type Layer,
  type Status,
} from "./content";
import { Section } from "./Section";
import { Wall } from "./Wall";

const REPO = "https://github.com/bjorn-ali-goransson/gala-os";
const DOCS = `${REPO}/tree/main/docs`;

const layerOrder: Layer[] = ["core", "nt", "shell", "linux", "project"];

const statusLabel: Record<Status, string> = {
  works: "Runs today",
  building: "Being built",
  planned: "Designed, not built",
};

/** Screenshots of gala running. Drop files in public/shots/ and add them here. */
const shots: { src: string; caption: string }[] = [
  {
    src: "/shots/crash-screen-layout.png",
    caption:
      "A crashed program on shasha's desktop: the report gala leaves behind names the fault, the module and the stack.",
  },
  {
    src: "/shots/crash-screen-art.png",
    caption:
      "The same screen in full. Nothing here is Windows' code — the window, the controls and the text are gala's own.",
  },
];

const toc = [
  { id: "overview", label: "What gala is" },
  { id: "runs", label: "What runs on it" },
  {
    id: "architecture",
    label: "Architecture",
    children: layerOrder.map((l) => ({
      id: `layer-${l}`,
      label: layers[l].name,
    })),
  },
  { id: "decisions", label: "Design decisions" },
  { id: "screens", label: "Screenshots" },
];

function useActiveSection(ids: string[]) {
  const [active, setActive] = useState(ids[0]);
  useEffect(() => {
    const seen = new Map<string, number>();
    const observer = new IntersectionObserver(
      (entries) => {
        for (const e of entries) {
          seen.set(e.target.id, e.intersectionRatio);
        }
        let best = "";
        let bestRatio = 0;
        for (const [id, ratio] of seen) {
          if (ratio > bestRatio) {
            best = id;
            bestRatio = ratio;
          }
        }
        if (best) setActive(best);
      },
      { rootMargin: "-10% 0px -60% 0px", threshold: [0, 0.25, 0.5, 1] },
    );
    for (const id of ids) {
      const el = document.getElementById(id);
      if (el) observer.observe(el);
    }
    return () => observer.disconnect();
  }, [ids.join(",")]);
  return active;
}

function StatusMark({ status }: { status: Status }) {
  return (
    <span className={`mark mark-${status}`} title={statusLabel[status]}>
      <span className="mark-glyph" aria-hidden="true" />
      <span className="mark-text">{statusLabel[status]}</span>
    </span>
  );
}

export function App() {
  const ids = toc.flatMap((t) => [t.id, ...(t.children?.map((c) => c.id) ?? [])]);
  const active = useActiveSection(ids);

  return (
    <>
      <a className="skip" href="#overview">
        Skip to content
      </a>

      <header className="masthead">
        <div className="shell masthead-in">
          <a className="wordmark" href="/">
            gala
            <span className="wordmark-ar" lang="ar" aria-hidden="true">
              قلعة
            </span>
          </a>
          <nav className="masthead-nav" aria-label="Elsewhere">
            <a href={DOCS}>Documentation</a>
            <a href={REPO}>Source</a>
          </nav>
        </div>
      </header>

      <main>
        <div className="hero">
          <div className="shell hero-in">
            <h1 className="hero-title">
              An operating system that runs Windows programs
              <span className="hero-title-sub">
                and owes Windows nothing underneath
              </span>
            </h1>
            <p className="hero-lede">
              gala is a from-scratch x86_64 kernel written in Rust. It
              implements the Windows system call interface and ships its own
              clean-room <code>ntdll.dll</code>, <code>kernel32.dll</code> and
              about fifty other system libraries, so Windows executables run on
              it exactly as their makers shipped them. Below that boundary,
              every line is gala's own design.
            </p>
            <Wall />
          </div>
        </div>

        <div className="shell layout">
          <nav className="rail" aria-label="On this page">
            <ul>
              {toc.map((t) => (
                <li key={t.id}>
                  <a
                    href={`#${t.id}`}
                    className={active === t.id ? "is-active" : undefined}
                  >
                    {t.label}
                  </a>
                  {t.children && (
                    <ul className="rail-sub">
                      {t.children.map((c) => (
                        <li key={c.id}>
                          <a
                            href={`#${c.id}`}
                            className={
                              active === c.id ? "is-active" : undefined
                            }
                          >
                            {c.label}
                          </a>
                        </li>
                      ))}
                    </ul>
                  )}
                </li>
              ))}
            </ul>
          </nav>

          <div className="body">
            <Section id="overview" title="What gala is">
              <p>
                A kernel, a set of system libraries and a graphical shell,
                written from nothing over the last years in Rust. The goal is
                plain: take a program built for Windows &mdash; by MSVC,
                mingw-w64, Go or Rust, or downloaded from its vendor as a
                release &mdash; and run it unchanged, on a kernel whose
                internals belong to this project.
              </p>
              <p>
                That is already true for a surprising amount of software. A Java
                virtual machine compiles and runs a program. PostgreSQL makes a
                cluster and serves queries over the network. OpenSSH accepts a
                login over a pseudo console, with line editing and Ctrl+C.
                TinyCC compiles itself on gala into a second compiler that
                behaves the same. Chromium opens a window and answers
                ChromeDriver.
              </p>
              <p>
                The name comes from the Arabic <i lang="ar">qal&rsquo;a</i>,
                fortress &mdash; and it is a play on Windows. It belongs to a
                family of foundational software built and controlled in Saudi
                Arabia rather than rented from elsewhere, which is also where
                its values come from: collect nothing that is not needed, keep
                the trusted core small, and say plainly what the system does.
              </p>
              <div className="pillars">
                <div>
                  <h3>One kernel, two personalities</h3>
                  <p>
                    Windows binaries go through the NT personality; Linux
                    binaries go through their own system call layer. Both sit on
                    the same memory manager, scheduler and drivers.
                  </p>
                </div>
                <div>
                  <h3>Updated without a reboot</h3>
                  <p>
                    The system image and the kernel itself are designed to be
                    replaced underneath running programs. The handover already
                    works with programs blocked in waits, in I/O and on open TCP
                    connections.
                  </p>
                </div>
                <div>
                  <h3>Servers and desktops, undivided</h3>
                  <p>
                    A machine with no screen never starts a graphical interface.
                    A desktop gets <b>shasha</b>, gala&rsquo;s shell &mdash; and
                    everything shasha does can be done from a terminal instead.
                  </p>
                </div>
              </div>
            </Section>

            <Section
              id="runs"
              title="What runs on it"
              intro="Each program below is held to a boot scenario: the test harness starts gala in a virtual machine, runs the program, and compares what comes out against what real Windows gives for the same binary. The names in the margin are those scenarios."
            >
              <ul className="programs">
                {programs.map((p) => (
                  <li key={p.name}>
                    <div className="program-head">
                      <h3>
                        {p.name}
                        {p.version && (
                          <span className="program-ver"> {p.version}</span>
                        )}
                      </h3>
                      <p>{p.what}</p>
                    </div>
                    <ul className="scenarios" aria-label="Boot scenarios">
                      {p.scenarios.map((s) => (
                        <li key={s}>
                          <code>{s}</code>
                        </li>
                      ))}
                    </ul>
                  </li>
                ))}
              </ul>
              <p className="aside">
                Underneath them sits the surface those programs actually use,
                checked call by call: processes and threads, virtual memory and
                sections, the synchronisation primitives, structured exception
                handling and unwinding, a case-insensitive file system with
                Windows share semantics, the registry, access tokens and
                security descriptors, sessions, consoles and pseudo consoles,
                Winsock, and windows on the screen.
              </p>
            </Section>

            <Section
              id="architecture"
              title="Architecture"
              intro="Every section of the design, by the layer it belongs to. The whole reference is in the repository; this is its map."
            >
              {layerOrder.map((l) => (
                <div className="layer" id={`layer-${l}`} key={l}>
                  <div className="layer-head">
                    <h3>{layers[l].name}</h3>
                    <p>{layers[l].blurb}</p>
                  </div>
                  <ul className="entries">
                    {architecture
                      .filter((e) => e.layer === l)
                      .map((e) => (
                        <li key={e.title}>
                          <div className="entry-top">
                            <h4>{e.title}</h4>
                            <StatusMark status={e.status} />
                          </div>
                          <p>{e.summary}</p>
                        </li>
                      ))}
                  </ul>
                </div>
              ))}
            </Section>

            <Section
              id="decisions"
              title="Design decisions"
              intro="The choices that shape everything else, and the reasons they were made."
            >
              <dl className="decisions">
                {decisions.map((d) => (
                  <div key={d.title}>
                    <dt>{d.title}</dt>
                    <dd>{d.body}</dd>
                  </div>
                ))}
              </dl>
            </Section>

            <Section id="screens" title="Screenshots">
              <div className="shots">
                {shots.map((s) => (
                  <figure key={s.src}>
                    <img src={s.src} alt={s.caption} loading="lazy" />
                    <figcaption>{s.caption}</figcaption>
                  </figure>
                ))}
              </div>
            </Section>
          </div>
        </div>
      </main>

      <footer className="foot">
        <div className="shell foot-in">
          <p className="foot-name">
            gala
            <span lang="ar"> قلعة</span>
          </p>
          <nav aria-label="Links">
            <a href={REPO}>Source</a>
            <a href={DOCS}>Documentation</a>
            <a href={`${REPO}/blob/main/docs/CLEAN_ROOM.md`}>Clean-room rules</a>
            <a href={`${REPO}/blob/main/docs/ROADMAP.md`}>Roadmap</a>
          </nav>
          <p className="foot-note">
            The source is public so that it can be read. No licence is granted;
            see the repository.
          </p>
        </div>
      </footer>
    </>
  );
}
