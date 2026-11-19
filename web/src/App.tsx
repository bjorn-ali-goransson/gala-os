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
  shots,
  statusLabel,
  toolGroups,
  windowsPrograms,
  type Capability,
  type Program,
  type Status,
} from "./content";
import { Flow, LayerStack, Progress, TestLoop, WindowPaths } from "./Diagrams";
import { Section } from "./Section";


const nav = [
  { id: "how", label: "How it works" },
  { id: "beyond", label: "Beyond a reboot" },
  { id: "components", label: "Components" },
  { id: "now", label: "In progress" },
  { id: "runs", label: "What runs" },
  { id: "tooling", label: "Tooling" },
  { id: "try", label: "Try it" },
];

function Logo() {
  // The fortress of the boot screen, reduced to its outline.
  return (
    <svg className="logo" viewBox="0 0 40 28" aria-hidden="true">
      <path d="M3 27 L6 8 h-1 l1-3 2 1.5 2-2 2 2 2-2 2 2 2-1.5 1 3 h-1 l0.4 4 L31 17 l0.2-2 h-0.6 l0.6-2 1.4 1 1.4-1.2 1.4 1.2 1.4-1 0.6 2 h-0.6 L38 27 Z" />
    </svg>
  );
}

function StatusTag({ status }: { status: Status }) {
  return <span className={`tag tag-${status}`}>{statusLabel[status]}</span>;
}

function ProgramList({ programs }: { programs: Program[] }) {
  return (
    <ul className="programs">
      {programs.map((p) => (
        <li key={p.name}>
          <p className="program-name">
            {p.name}
            {p.version && <span> {p.version}</span>}
          </p>
          <p className="program-what">{p.what}</p>
          <p className="program-sc">
            {p.scenarios.map((s) => (
              <code key={s}>{s}</code>
            ))}
          </p>
        </li>
      ))}
    </ul>
  );
}

function CapabilityCard({ c }: { c: Capability }) {
  return (
    <li id={`can-${c.id}`}>
      <div className="card-top">
        <h4>{c.name}</h4>
        <StatusTag status={c.status} />
      </div>
      <p>{c.what}</p>
      <p className="card-how">{c.how}</p>
      {c.proof && (
        <p className="program-sc">
          {c.proof.map((s) => (
            <code key={s}>{s}</code>
          ))}
        </p>
      )}
    </li>
  );
}

export function App() {
  return (
    <>
      <a className="skip" href="#main">
        Skip to content
      </a>

      <header className="top">
        <div className="wrap top-in">
          <a className="brand" href="/">
            <Logo />
            <span>gala</span>
            <span className="brand-ar" lang="ar">
              قلعة
            </span>
          </a>
          <nav className="top-nav" aria-label="Sections">
            {nav.map((n) => (
              <a key={n.id} href={`#${n.id}`}>
                {n.label}
              </a>
            ))}
          </nav>
        </div>
      </header>

      <main id="main">
        <div className="hero wrap">
          <p className="eyebrow">A from-scratch operating system in Rust</p>
          <h1>Runs Windows programs. Owes Windows nothing underneath.</h1>
          <p className="lede">
            gala is a new kernel that speaks the Windows system call
            interface and ships its own clean-room <code>ntdll.dll</code>,{" "}
            <code>kernel32.dll</code> and more than eighty other system
            libraries. Windows programs run on it as their makers shipped
            them. A second personality on the same kernel runs Linux
            programs too, on the same desktop.
          </p>
          <p className="hero-links">
            <a className="btn" href="#how">
              How it works
            </a>
            <a className="btn btn-quiet" href="#runs">
              What runs on it
            </a>
            <a className="btn btn-quiet" href="#try">
              Download
            </a>
          </p>
          <figure className="hero-shot">
            <img
              src="/shots/start-menu.webp"
              width={1280}
              height={800}
              alt="gala's desktop with the start menu open: pinned apps, and Ubuntu under Linux distributions."
            />
          </figure>
        </div>

        <div className="wrap">
          <Section
            id="how"
            title="How it works"
            intro="An operating system is built in layers. A program asks the layer below for what it needs, and the request travels down until something can do the work. gala's trick is at one particular layer: the system call boundary."
          >
            <LayerStack />

            <div className="explain">
              <div>
                <h3>Why the system call boundary?</h3>
                <p>
                  A Windows program never talks to the hardware. It calls
                  functions in system DLLs such as <code>kernel32.dll</code>,
                  and those end in <code>ntdll.dll</code>, which asks the
                  kernel through a numbered <em>system call</em>. If gala
                  provides the same DLLs and answers the same system calls
                  the same way, the program cannot tell the difference.
                </p>
              </div>
              <div>
                <h3>Why not copy Windows&rsquo; internals?</h3>
                <p>
                  Because they aren&rsquo;t needed and aren&rsquo;t ours to
                  copy. Below the boundary, gala&rsquo;s memory manager,
                  scheduler, file system and drivers are its own design.
                  That leaves them free to change, for example to be
                  replaced while programs are still running.
                </p>
              </div>
              <div>
                <h3>Where does Linux fit?</h3>
                <p>
                  A personality is a translation layer from one
                  system&rsquo;s calls to the core. gala has two. Linux
                  programs bring their own libraries (glibc, GTK) and make
                  Linux system calls, and the Linux layer answers them with
                  the same core the Windows layer uses.
                </p>
              </div>
            </div>

            <h3 className="dia-title">One system call, step by step</h3>
            <p className="dia-intro">
              What happens when a program reads a file:
            </p>
            <Flow
              label="The path of a ReadFile call through gala"
              steps={[
                { lane: "user", title: "ReadFile", detail: "the program calls kernel32.dll" },
                { lane: "user", title: "NtReadFile", detail: "kernel32 calls into ntdll.dll" },
                { lane: "user", title: "syscall", detail: "ntdll puts the call's number in a register and enters the kernel" },
                { lane: "kernel", title: "Entry and dispatch", detail: "registers saved; the number picks the service" },
                { lane: "kernel", title: "Handle and access", detail: "the handle names a file object; the caller's token is checked" },
                { lane: "kernel", title: "File system", detail: "the data comes from C:, through the disk driver if needed" },
                { lane: "user", title: "Back", detail: "a status code returns and the program carries on" },
              ]}
              caption="Every system call also feeds gala's own diagnostics. A function that gala lacks is listed in a ledger at the end of the run, so no gap stays hidden."
            />

            <h3 className="dia-title">From power-on to the first program</h3>
            <Flow
              label="The boot sequence"
              steps={[
                { title: "Firmware", detail: "UEFI or BIOS" },
                { title: "Limine", detail: "loads the kernel and its files" },
                { title: "Kernel start", detail: "memory, clock, every CPU" },
                { title: "Drivers", detail: "disk, network, screen" },
                { title: "C: assembled", detail: "DLLs, registry, user profiles" },
                { title: "Programs", detail: "services, then the desktop" },
              ]}
            />

            <h3 className="dia-title">Two systems, one desktop</h3>
            <WindowPaths />
          </Section>

          <Section
            id="beyond"
            title="Beyond a reboot"
            intro="Most systems lose everything that is running when the kernel changes or the machine stops. gala is built so that running state can outlive both: written down, carried over and started again."
          >
            <ul className="cards cards-wide">
              {liveCapabilities.map((c) => (
                <CapabilityCard key={c.id} c={c} />
              ))}
            </ul>

            <h3 className="dia-title">A kernel handover, step by step</h3>
            <p className="dia-intro">
              How a new kernel takes over a running machine. The programs
              see a pause, not a restart.
            </p>
            <Flow
              label="The steps of a kernel handover"
              steps={[
                { lane: "old", title: "Load the new kernel", detail: "its image is checked against its signature first" },
                { lane: "old", title: "Quiesce", detail: "every thread stops where it can safely stop; a blocked call is noted for later" },
                { lane: "old", title: "Write the record", detail: "processes, memory, handles, sockets, the clock: versioned data" },
                { lane: "new", title: "Jump", detail: "straight into the new kernel, no firmware or bootloader" },
                { lane: "new", title: "Rebuild", detail: "the record is read, older formats too; memory stays in place" },
                { lane: "user", title: "Carry on", detail: "blocked calls are reissued and connections stay open" },
              ]}
              caption="Anything the record cannot carry yet makes the handover refuse, and the old kernel keeps running, with the reason in its log."
            />

            <h3 className="dia-title">Next: a program that moves between machines</h3>
            <p className="dia-intro">
              The same record, cut down to one program. Planned; the
              commands are those the roadmap names.
            </p>
            <Flow
              label="Freezing a program and starting it on another machine"
              steps={[
                { lane: "here", title: "gala snapshot", detail: "the program and its children freeze at a quiet point" },
                { lane: "here", title: "export", detail: "one file: memory, threads, handles, windows; signed and encrypted" },
                { lane: "there", title: "import", detail: "version, digests and signature checked" },
                { lane: "there", title: "gala restore", detail: "it runs on, its windows on the new desktop" },
              ]}
            />

            <h3 className="dia-title">Making rare bugs ordinary</h3>
            <p className="dia-intro">
              The same idea pointed at bugs: a failure is made to happen on
              demand, and when it happens, it explains itself.
            </p>
            <ul className="cards">
              {debugCapabilities.map((c) => (
                <CapabilityCard key={c.id} c={c} />
              ))}
            </ul>
          </Section>

          <Section
            id="components"
            title="The components"
            intro="The pieces gala is built from, layer by layer. Where a piece is partly done, the done part and the rest are listed apart."
          >
            {componentGroups.map((g) => (
              <div className="group" key={g.id} id={`c-${g.id}`}>
                <div className="group-head">
                  <h3>{g.title}</h3>
                  <p>{g.intro}</p>
                </div>
                <ul className="cards">
                  {g.items.map((c) => (
                    <li key={c.name}>
                      <div className="card-top">
                        <h4>{c.name}</h4>
                        <StatusTag status={c.status} />
                      </div>
                      <p>{c.what}</p>
                      {c.path && <code className="card-path">{c.path}</code>}
                    </li>
                  ))}
                </ul>
              </div>
            ))}
          </Section>

          <Section
            id="now"
            title="In progress now"
            intro="The two large efforts under way. Filled dots are done, a ringed dot is being worked on, and hollow dots come later."
          >
            <div className="focus">
              {focus.map((f) => (
                <article key={f.id} id={`now-${f.id}`}>
                  <h3>{f.title}</h3>
                  <p>{f.lede}</p>
                  <Progress steps={f.steps} />
                </article>
              ))}
            </div>
          </Section>

          <Section
            id="runs"
            title="What runs today"
            intro="Each program below is held to a boot scenario, named under it. The test harness boots gala in a virtual machine, runs the program and checks its output against what real Windows (or real Linux) gives."
          >
            <h3 className="sub">Windows programs, unmodified</h3>
            <ProgramList programs={windowsPrograms} />
            <h3 className="sub">Linux programs</h3>
            <ProgramList programs={linuxPrograms} />

            <h3 className="sub" id="screens">
              On the screen
            </h3>
            <div className="shots">
              {shots.map((s) => (
                <figure key={s.src}>
                  <a href={s.src}>
                    <img src={s.src} width={1280} height={800} alt={`${s.title}: ${s.caption}`} loading="lazy" />
                  </a>
                  <figcaption>
                    <b>{s.title}.</b> {s.caption}
                  </figcaption>
                </figure>
              ))}
            </div>

            <h3 className="sub">Planned</h3>
            <ul className="planned">
              {planned.map((p) => (
                <li key={p.name}>
                  <b>{p.name}</b>
                  <span>{p.what}</span>
                </li>
              ))}
            </ul>
          </Section>

          <Section
            id="tooling"
            title="Tooling"
            intro="Building an operating system is mostly a matter of finding out why something went wrong. Much of gala's code exists to make that fast: one command drives everything, and every failure leaves an explanation behind."
          >
            <h3 className="dia-title">The loop every change follows</h3>
            <TestLoop />

            <div className="toolgroups">
              {toolGroups.map((g) => (
                <div key={g.title} className="toolgroup">
                  <h3>{g.title}</h3>
                  <p>{g.intro}</p>
                  <dl>
                    {g.tools.map((t) => (
                      <div key={t.cmd}>
                        <dt>
                          <code>{t.cmd}</code>
                        </dt>
                        <dd>{t.what}</dd>
                      </div>
                    ))}
                  </dl>
                </div>
              ))}
            </div>

            <div className="twocol">
              <div>
                <h3>Beside the code</h3>
                <dl className="plain">
                  {repoTools.map((t) => (
                    <div key={t.name}>
                      <dt>{t.name}</dt>
                      <dd>{t.what}</dd>
                    </div>
                  ))}
                </dl>
              </div>
              <div>
                <h3>Inside gala</h3>
                <dl className="plain">
                  {insideTools.map((t) => (
                    <div key={t.name}>
                      <dt>
                        <code>{t.name}</code>
                      </dt>
                      <dd>{t.what}</dd>
                    </div>
                  ))}
                </dl>
              </div>
            </div>
          </Section>

          <Section
            id="try"
            title="Try it"
            intro="Bootable images of the latest build, for a virtual machine. They are early developer images: they show the kernel booting and running programs, not a desktop to use yet."
          >
            <ul className="cards cards-wide downloads">
              {images.map((i) => (
                <li key={i.arch} id={`get-${i.arch}`}>
                  <div className="card-top">
                    <h4>{i.arch}</h4>
                    <span className="dl-size">{i.size}</span>
                  </div>
                  <p>{i.what}</p>
                  <p className="dl-links">
                    <a className="btn" href={`${imagesUrl}/${i.file}`}>
                      Download {i.file}
                    </a>
                    <a href={`${imagesUrl}/${i.file}.sha256`}>SHA-256</a>
                    <a href={`${imagesUrl}/${i.file.replace(/\.iso$/, "")}.commit`}>commit</a>
                  </p>
                  <p className="dl-label">Run it in QEMU</p>
                  <pre className="cmd">
                    <code>{i.qemu}</code>
                  </pre>
                  <p className="dl-needs">Needs {i.needs}</p>
                </li>
              ))}
            </ul>

            <div className="explain">
              <div>
                <h3>What you will see</h3>
                <p>{imageDemo}</p>
              </div>
              <div>
                <h3>Check the download</h3>
                <p>
                  Each image has its SHA-256 beside it. In the folder you
                  downloaded both files to:
                </p>
                <pre className="cmd">
                  <code>sha256sum -c gala-x86_64.iso.sha256</code>
                </pre>
                <p>
                  The <code>.commit</code> file names the commit it was
                  built from.
                </p>
              </div>
              <div>
                <h3>On a USB stick</h3>
                <p>
                  The x86_64 image can be written as it is to a USB stick
                  (this erases the stick), for example with{" "}
                  <code>sudo dd if=gala-x86_64.iso of=/dev/sdX bs=4M conv=fsync</code>{" "}
                  on Linux, or with a tool such as Rufus or balenaEtcher.
                  It is tested in QEMU, not on real machines, so hardware
                  support is limited.
                </p>
              </div>
            </div>
          </Section>

          <Section
            id="principles"
            title="Principles"
            intro="The rules every contribution is held to."
          >
            <dl className="principles">
              {decisions.map((d) => (
                <div key={d.title}>
                  <dt>{d.title}</dt>
                  <dd>{d.body}</dd>
                </div>
              ))}
            </dl>
          </Section>
        </div>
      </main>

      <footer className="foot">
        <div className="wrap foot-in">
          <p className="brand">
            <Logo />
            <span>gala</span>
            <span className="brand-ar" lang="ar">
              قلعة
            </span>
          </p>
          <nav aria-label="Links">
            {nav.map((n) => (
              <a key={n.id} href={`#${n.id}`}>
                {n.label}
              </a>
            ))}
          </nav>
          <p className="foot-note">
            The name is the Arabic <i lang="ar-Latn">qal&rsquo;a</i>,
            fortress. No licence is granted to its code.
          </p>
        </div>
      </footer>
    </>
  );
}
