// The page's diagrams. They are HTML and CSS rather than images, so they
// follow the theme, scale to a phone's width and can be read by a screen
// reader or a crawler as text.

import { Fragment } from "react";
import { useT } from "./i18n";

/** The layers of gala, read from the top (programs) down to the machine. */
export function LayerStack() {
  const t = useT();
  return (
    <figure className="dia stack" aria-label={t("The layers of gala, from programs down to the machine")}>
      <div className="stack-row">
        <p className="stack-label">{t("Programs, unchanged")}</p>
        <div className="stack-cells">
          <div className="cell cell-win">
            <b>{t("Windows programs")}</b>
            <span>chrome.exe · java.exe · postgres.exe · WINWORD.EXE</span>
          </div>
          <div className="cell cell-lx">
            <b>{t("Linux programs")}</b>
            <span>gimp · inkscape · busybox</span>
          </div>
        </div>
      </div>

      <div className="stack-row">
        <p className="stack-label">{t("Libraries in user mode")}</p>
        <div className="stack-cells">
          <div className="cell cell-win">
            <b>{t("gala's clean-room DLLs")}</b>
            <span>kernel32 · user32 · gdi32 · ws2_32 · ole32 · … → ntdll</span>
          </div>
          <div className="cell cell-lx">
            <b>{t("The distribution's own")}</b>
            <span>glibc · GTK · libwayland</span>
          </div>
        </div>
      </div>

      <div className="stack-boundary">
        <span>{t("The ABI boundary: system calls")}</span>
        <small>{t("the only place where gala imitates anything")}</small>
      </div>

      <div className="stack-row">
        <p className="stack-label">{t("Personalities")}</p>
        <div className="stack-cells">
          <div className="cell cell-win">
            <b>{t("NT layer")}</b>
            <span>{t("handles · processes · registry · security · sockets")}</span>
          </div>
          <div className="cell cell-lx">
            <b>{t("Linux layer")}</b>
            <span>{t("system calls · signals · epoll · Wayland server")}</span>
          </div>
        </div>
      </div>

      <div className="stack-row">
        <p className="stack-label">{t("Shared services")}</p>
        <div className="stack-cells stack-cells-even">
          <div className="cell">
            <b>shasha</b>
            <span>{t("windows on the screen")}</span>
          </div>
          <div className="cell">
            <b>{t("Files")}</b>
            <span>{t("C: and the disk log")}</span>
          </div>
          <div className="cell">
            <b>{t("Network")}</b>
            <span>TCP/IP</span>
          </div>
          <div className="cell">
            <b>{t("Sessions")}</b>
            <span>{t("users and desktops")}</span>
          </div>
        </div>
      </div>

      <div className="stack-row">
        <p className="stack-label">{t("The core")}</p>
        <div className="stack-cells">
          <div className="cell cell-core">
            <b>{t("Memory · threads and scheduler · one clock · drivers · handover")}</b>
          </div>
        </div>
      </div>

      <div className="stack-row">
        <p className="stack-label">{t("The architecture seam")}</p>
        <div className="stack-cells">
          <div className="cell">
            <b>x86_64</b>
            <span className="tag tag-works">{t("works")}</span>
          </div>
          <div className="cell">
            <b>aarch64</b>
            <span className="tag tag-building">{t("in progress")}</span>
          </div>
        </div>
      </div>

      <div className="stack-row">
        <p className="stack-label">{t("The machine")}</p>
        <div className="stack-cells">
          <div className="cell cell-hw">
            <span>{t("A PC or a virtual machine: UEFI or BIOS · virtio disk, network and display · PS/2")}</span>
          </div>
        </div>
      </div>
      <figcaption>
        {t(
          "Windows programs are on the left and Linux programs on the right. Both go down to the same core. Everything below the boundary is gala's own design.",
        )}
      </figcaption>
    </figure>
  );
}

export type FlowStep = { title: string; detail?: string; lane?: string };

/**
 * Steps joined by arrows: across on a wide screen, down on a narrow one.
 * A step whose lane differs from the one before starts a new band, so a
 * flow that crosses from user mode into the kernel shows where it does.
 */
export function Flow({
  steps,
  label,
  caption,
}: {
  steps: FlowStep[];
  label: string;
  caption?: string;
}) {
  const t = useT();
  return (
    <figure className="dia flow" aria-label={label}>
      <ol>
        {steps.map((s, i) => {
          const crosses = i > 0 && s.lane && s.lane !== steps[i - 1].lane;
          return (
            <Fragment key={s.title}>
              {i > 0 && (
                <li
                  className={`flow-arrow${crosses ? " flow-cross" : ""}`}
                  aria-hidden="true"
                >
                  <span />
                </li>
              )}
              <li className={`flow-step${s.lane ? ` lane-${s.lane}` : ""}`}>
                {s.lane && (i === 0 || crosses) && (
                  <span className="flow-lane">{t(laneName[s.lane] ?? s.lane)}</span>
                )}
                <b>{s.title}</b>
                {s.detail && <span>{s.detail}</span>}
              </li>
            </Fragment>
          );
        })}
      </ol>
      {caption && <figcaption>{caption}</figcaption>}
    </figure>
  );
}

const laneName: Record<string, string> = {
  user: "user mode",
  kernel: "kernel",
  host: "your machine",
  win: "Windows 11",
  gala: "gala",
  old: "running kernel",
  new: "new kernel",
  here: "this machine",
  there: "another machine",
};

/** How a window from either personality reaches the screen. */
export function WindowPaths() {
  const t = useT();
  return (
    <figure className="dia paths" aria-label={t("How Windows and Linux programs get a window on the same desktop")}>
      <div className="paths-lanes">
        <ol className="paths-lane">
          <li className="chip chip-win">Notepad, Word, FileZilla</li>
          <li className="chip">user32 · gdi32 · Direct2D</li>
          <li className="chip">{t("system calls")}</li>
        </ol>
        <ol className="paths-lane">
          <li className="chip chip-lx">GIMP, Inkscape</li>
          <li className="chip">GTK · libwayland</li>
          <li className="chip">{t("a Unix socket to gala's Wayland server")}</li>
        </ol>
        <ol className="paths-lane">
          <li className="chip chip-lx">{t("xterm and other X11 programs")}</li>
          <li className="chip">Xwayland</li>
          <li className="chip">{t("the same Wayland server")}</li>
        </ol>
      </div>
      <div className="paths-merge" aria-hidden="true" />
      <ol className="paths-tail">
        <li className="chip chip-strong">{t("shasha: one window list, one taskbar, one focus")}</li>
        <li className="chip">{t("framebuffer or virtio-gpu")}</li>
      </ol>
      <figcaption>
        {t(
          "Three routes lead to the same window list. The taskbar, the task manager and UI Automation treat every window alike, whichever system it came from.",
        )}
      </figcaption>
    </figure>
  );
}

/** The test-first loop every piece of work follows. */
export function TestLoop() {
  const t = useT();
  const steps = [
    { n: "1", title: "Write a test program", detail: "a small Windows .exe in user/<feature>_test" },
    { n: "2", title: "Run it on Windows 11", detail: "its output is the expected result" },
    { n: "3", title: "Run it on gala", detail: "in gala-um (seconds) and booted in QEMU" },
    { n: "4", title: "See it fail, then implement", detail: "until gala's output equals Windows'" },
    { n: "5", title: "Check it in", detail: "the scenario joins the suite CI runs on every change" },
  ];
  return (
    <figure className="dia loop" aria-label={t("The test-first loop")}>
      <ol>
        {steps.map((s) => (
          <li key={s.n}>
            <span className="loop-n">{s.n}</span>
            <b>{t(s.title)}</b>
            <span>{t(s.detail)}</span>
          </li>
        ))}
      </ol>
      <p className="loop-back" aria-hidden="true">
        ↺ {t("the same binary runs on both systems, and the two outputs are compared")}
      </p>
    </figure>
  );
}

export type Track = { label: string; status: "works" | "building" | "planned"; detail?: string };

/** A line of milestones, done ones filled. */
export function Progress({ steps }: { steps: Track[] }) {
  const t = useT();
  return (
    <ol className="progress">
      {steps.map((s) => (
        <li key={s.label} className={`progress-${s.status}`}>
          <span className="progress-dot" aria-hidden="true" />
          <span className="progress-text">
            <b>{s.label}</b>
            {s.detail && <span>{s.detail}</span>}
          </span>
          <span className="sr-only">
            ({t(s.status === "works" ? "done" : s.status === "building" ? "in progress" : "planned")})
          </span>
        </li>
      ))}
    </ol>
  );
}
