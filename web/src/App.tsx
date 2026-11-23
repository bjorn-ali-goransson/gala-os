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
  usb,
  windowsPrograms,
  type Capability,
  type Program,
  type Status,
} from "./content";
import { Flow, LayerStack, Progress, TestLoop, WindowPaths } from "./Diagrams";
import { Both, LangProvider, localize, pagePath, translate, useLang, useT, type Lang } from "./i18n";
import { Section } from "./Section";


const nav = [
  { id: "how", label: "How it works" },
  { id: "beyond", label: "Beyond a reboot" },
  { id: "components", label: "Components" },
  { id: "now", label: "In progress" },
  { id: "runs", label: "What runs" },
  { id: "tooling", label: "Tooling" },
  { id: "try", label: "Try it" },
  { id: "usb", label: "From a USB stick" },
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
  const t = useT();
  return <span className={`tag tag-${status}`}>{t(statusLabel[status])}</span>;
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

/** Booting a PC from a USB stick: what it needs, the steps, the limits. */
function UsbSection() {
  const lang = useLang();
  const t = useT();
  const u = localize(lang, usb);
  return (
    <Section id="usb" title={t("Run it from a USB stick")} intro={u.intro}>
      <div className="usb-needs">
        <h3>{t("You need")}</h3>
        <ul>
          {u.needs.map((n) => (
            <li key={n}>{n}</li>
          ))}
        </ul>
      </div>
      <ol className="usb-steps">
        {u.steps.map((s) => (
          <li key={s.title}>
            <h3>{s.title}</h3>
            <p>{s.body}</p>
            {s.ways && (
              <div className="usb-ways">
                {s.ways.map((w) => (
                  <div key={w.name} className="usb-way">
                    <h4>{w.name}</h4>
                    <p>{w.how}</p>
                    {w.cmds && (
                      <pre className="cmd">
                        <code>{w.cmds.join("\n")}</code>
                      </pre>
                    )}
                  </div>
                ))}
              </div>
            )}
          </li>
        ))}
      </ol>
      <div className="usb-limits">
        <h3>{t("Limits for now")}</h3>
        <ul>
          {u.limits.map((l) => (
            <li key={l}>{l}</li>
          ))}
        </ul>
      </div>
    </Section>
  );
}

/** The page in `lang`. */
export function App({ lang }: { lang: Lang }) {
  return (
    <LangProvider lang={lang}>
      <Page />
    </LangProvider>
  );
}

/** The link to the page in the other language. */
function LangSwitch() {
  const lang = useLang();
  const other: Lang = lang === "en" ? "ar" : "en";
  return (
    <a className="lang-switch" href={pagePath[other]} hrefLang={other} lang={other}>
      {other === "ar" ? "العربية" : "English"}
    </a>
  );
}

function Page() {
  const lang = useLang();
  const t = useT();
  const L = <T,>(v: T) => localize(lang, v);
  return (
    <>
      <a className="skip" href="#main">
        {t("Skip to content")}
      </a>

      <header className="top">
        <div className="wrap top-in">
          <a className="brand" href={pagePath[lang]}>
            <Logo />
            <span>gala</span>
            <span className="brand-ar" lang="ar">
              قلعة
            </span>
          </a>
          <nav className="top-nav" aria-label={t("Sections")}>
            {nav.map((n) => (
              <a key={n.id} href={`#${n.id}`}>
                {t(n.label)}
              </a>
            ))}
          </nav>
          <LangSwitch />
        </div>
      </header>

      <main id="main">
        <div className="hero wrap">
          <p className="eyebrow">{t("A from-scratch operating system in Rust")}</p>
          <h1>{t("Runs Windows programs. Owes Windows nothing underneath.")}</h1>
          <p className="lede">
            <Both
              en={
                <>
                  gala is a new kernel that speaks the Windows system call
                  interface and ships its own clean-room <code>ntdll.dll</code>,{" "}
                  <code>kernel32.dll</code> and more than eighty other system
                  libraries. Windows programs run on it as their makers shipped
                  them. A second personality on the same kernel runs Linux
                  programs too, on the same desktop.
                </>
              }
              ar={
                <>
                  gala نواة جديدة تتحدث واجهة استدعاءات النظام في Windows،
                  وتأتي بمكتباتها الخاصة المكتوبة من الصفر بلا نسخ:{" "}
                  <code>ntdll.dll</code> و<code>kernel32.dll</code> وأكثر من
                  ثمانين مكتبة نظام أخرى. تعمل عليها برامج Windows كما أصدرها
                  صانعوها، دون أي تعديل. وشخصية ثانية فوق النواة نفسها تشغّل
                  برامج Linux أيضًا، على سطح المكتب نفسه.
                </>
              }
            />
          </p>
          <p className="hero-links">
            <a className="btn" href="#how">
              {t("How it works")}
            </a>
            <a className="btn btn-quiet" href="#runs">
              {t("What runs on it")}
            </a>
            <a className="btn btn-quiet" href="#try">
              {t("Download")}
            </a>
            <a className="btn btn-quiet" href="#usb">
              {t("Boot it from USB")}
            </a>
          </p>
          <figure className="hero-shot">
            <img
              src="/shots/start-menu.webp"
              width={1280}
              height={800}
              alt={t("gala's desktop with the start menu open: pinned apps, and Ubuntu under Linux distributions.")}
            />
          </figure>
        </div>

        <div className="wrap">
          <Section
            id="how"
            title={t("How it works")}
            intro={t(
              "An operating system is built in layers. A program asks the layer below for what it needs, and the request travels down until something can do the work. gala's trick is at one particular layer: the system call boundary.",
            )}
          >
            <LayerStack />

            <div className="explain">
              <div>
                <h3>{t("Why the system call boundary?")}</h3>
                <p>
                  <Both
                    en={
                      <>
                        A Windows program never talks to the hardware. It calls
                        functions in system DLLs such as <code>kernel32.dll</code>,
                        and those end in <code>ntdll.dll</code>, which asks the
                        kernel through a numbered <em>system call</em>. If gala
                        provides the same DLLs and answers the same system calls
                        the same way, the program cannot tell the difference.
                      </>
                    }
                    ar={
                      <>
                        برنامج Windows لا يخاطب العتاد أبدًا. إنه يستدعي دوالّ في
                        مكتبات النظام مثل <code>kernel32.dll</code>، وتنتهي هذه
                        إلى <code>ntdll.dll</code>، التي تطلب من النواة عبر{" "}
                        <em>استدعاء نظام</em> مرقّم. فإذا قدّمت gala المكتبات
                        نفسها، وأجابت عن استدعاءات النظام نفسها بالطريقة نفسها،
                        لم يستطع البرنامج أن يميّز الفرق.
                      </>
                    }
                  />
                </p>
              </div>
              <div>
                <h3>{t("Why not copy Windows' internals?")}</h3>
                <p>
                  {t(
                    "Because they aren't needed and aren't ours to copy. Below the boundary, gala's memory manager, scheduler, file system and drivers are its own design. That leaves them free to change, for example to be replaced while programs are still running.",
                  )}
                </p>
              </div>
              <div>
                <h3>{t("Where does Linux fit?")}</h3>
                <p>
                  {t(
                    "A personality is a translation layer from one system's calls to the core. gala has two. Linux programs bring their own libraries (glibc, GTK) and make Linux system calls, and the Linux layer answers them with the same core the Windows layer uses.",
                  )}
                </p>
              </div>
            </div>

            <h3 className="dia-title">{t("One system call, step by step")}</h3>
            <p className="dia-intro">{t("What happens when a program reads a file:")}</p>
            <Flow
              label={t("The path of a ReadFile call through gala")}
              steps={L([
                { lane: "user", title: "ReadFile", detail: "the program calls kernel32.dll" },
                { lane: "user", title: "NtReadFile", detail: "kernel32 calls into ntdll.dll" },
                { lane: "user", title: "syscall", detail: "ntdll puts the call's number in a register and enters the kernel" },
                { lane: "kernel", title: "Entry and dispatch", detail: "registers saved; the number picks the service" },
                { lane: "kernel", title: "Handle and access", detail: "the handle names a file object; the caller's token is checked" },
                { lane: "kernel", title: "File system", detail: "the data comes from C:, through the disk driver if needed" },
                { lane: "user", title: "Back", detail: "a status code returns and the program carries on" },
              ])}
              caption={t(
                "Every system call also feeds gala's own diagnostics. A function that gala lacks is listed in a ledger at the end of the run, so no gap stays hidden.",
              )}
            />

            <h3 className="dia-title">{t("From power-on to the first program")}</h3>
            <Flow
              label={t("The boot sequence")}
              steps={L([
                { title: "Firmware", detail: "UEFI or BIOS" },
                { title: "Limine", detail: "loads the kernel and its files" },
                { title: "Kernel start", detail: "memory, clock, every CPU" },
                { title: "Drivers", detail: "disk, network, screen" },
                { title: "C: assembled", detail: "DLLs, registry, user profiles" },
                { title: "Programs", detail: "services, then the desktop" },
              ])}
            />

            <h3 className="dia-title">{t("Two systems, one desktop")}</h3>
            <WindowPaths />
          </Section>

          <Section
            id="beyond"
            title={t("Beyond a reboot")}
            intro={t(
              "Most systems lose everything that is running when the kernel changes or the machine stops. gala is built so that running state can outlive both: written down, carried over and started again.",
            )}
          >
            <ul className="cards cards-wide">
              {L(liveCapabilities).map((c) => (
                <CapabilityCard key={c.id} c={c} />
              ))}
            </ul>

            <h3 className="dia-title">{t("A kernel handover, step by step")}</h3>
            <p className="dia-intro">
              {t("How a new kernel takes over a running machine. The programs see a pause, not a restart.")}
            </p>
            <Flow
              label={t("The steps of a kernel handover")}
              steps={L([
                { lane: "old", title: "Load the new kernel", detail: "its image is checked against its signature first" },
                { lane: "old", title: "Quiesce", detail: "every thread stops where it can safely stop; a blocked call is noted for later" },
                { lane: "old", title: "Write the record", detail: "processes, memory, handles, sockets, the clock: versioned data" },
                { lane: "new", title: "Jump", detail: "straight into the new kernel, no firmware or bootloader" },
                { lane: "new", title: "Rebuild", detail: "the record is read, older formats too; memory stays in place" },
                { lane: "user", title: "Carry on", detail: "blocked calls are reissued and connections stay open" },
              ])}
              caption={t(
                "Anything the record cannot carry yet makes the handover refuse, and the old kernel keeps running, with the reason in its log.",
              )}
            />

            <h3 className="dia-title">{t("Next: a program that moves between machines")}</h3>
            <p className="dia-intro">
              {t("The same record, cut down to one program. Planned; the commands are those the roadmap names.")}
            </p>
            <Flow
              label={t("Freezing a program and starting it on another machine")}
              steps={L([
                { lane: "here", title: "gala snapshot", detail: "the program and its children freeze at a quiet point" },
                { lane: "here", title: "export", detail: "one file: memory, threads, handles, windows; signed and encrypted" },
                { lane: "there", title: "import", detail: "version, digests and signature checked" },
                { lane: "there", title: "gala restore", detail: "it runs on, its windows on the new desktop" },
              ])}
            />

            <h3 className="dia-title">{t("Making rare bugs ordinary")}</h3>
            <p className="dia-intro">
              {t(
                "The same idea pointed at bugs: a failure is made to happen on demand, and when it happens, it explains itself.",
              )}
            </p>
            <ul className="cards">
              {L(debugCapabilities).map((c) => (
                <CapabilityCard key={c.id} c={c} />
              ))}
            </ul>
          </Section>

          <Section
            id="components"
            title={t("The components")}
            intro={t(
              "The pieces gala is built from, layer by layer. Where a piece is partly done, the done part and the rest are listed apart.",
            )}
          >
            {L(componentGroups).map((g) => (
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
            title={t("In progress now")}
            intro={t(
              "The two large efforts under way. Filled dots are done, a ringed dot is being worked on, and hollow dots come later.",
            )}
          >
            <div className="focus">
              {L(focus).map((f) => (
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
            title={t("What runs today")}
            intro={t(
              "Each program below is held to a boot scenario, named under it. The test harness boots gala in a virtual machine, runs the program and checks its output against what real Windows (or real Linux) gives.",
            )}
          >
            <h3 className="sub">{t("Windows programs, unmodified")}</h3>
            <ProgramList programs={L(windowsPrograms)} />
            <h3 className="sub">{t("Linux programs")}</h3>
            <ProgramList programs={L(linuxPrograms)} />

            <h3 className="sub" id="screens">
              {t("On the screen")}
            </h3>
            <div className="shots">
              {L(shots).map((s) => (
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

            <h3 className="sub">{t("Planned")}</h3>
            <ul className="planned">
              {L(planned).map((p) => (
                <li key={p.name}>
                  <b>{p.name}</b>
                  <span>{p.what}</span>
                </li>
              ))}
            </ul>
          </Section>

          <Section
            id="tooling"
            title={t("Tooling")}
            intro={t(
              "Building an operating system is mostly a matter of finding out why something went wrong. Much of gala's code exists to make that fast: one command drives everything, and every failure leaves an explanation behind.",
            )}
          >
            <h3 className="dia-title">{t("The loop every change follows")}</h3>
            <TestLoop />

            <div className="toolgroups">
              {L(toolGroups).map((g) => (
                <div key={g.title} className="toolgroup">
                  <h3>{g.title}</h3>
                  <p>{g.intro}</p>
                  <dl>
                    {g.tools.map((tool) => (
                      <div key={tool.cmd}>
                        <dt>
                          <code>{tool.cmd}</code>
                        </dt>
                        <dd>{tool.what}</dd>
                      </div>
                    ))}
                  </dl>
                </div>
              ))}
            </div>

            <div className="twocol">
              <div>
                <h3>{t("Beside the code")}</h3>
                <dl className="plain">
                  {L(repoTools).map((tool) => (
                    <div key={tool.name}>
                      <dt>{tool.name}</dt>
                      <dd>{tool.what}</dd>
                    </div>
                  ))}
                </dl>
              </div>
              <div>
                <h3>{t("Inside gala")}</h3>
                <dl className="plain">
                  {L(insideTools).map((tool) => (
                    <div key={tool.name}>
                      <dt>
                        <code>{tool.name}</code>
                      </dt>
                      <dd>{tool.what}</dd>
                    </div>
                  ))}
                </dl>
              </div>
            </div>
          </Section>

          <Section
            id="try"
            title={t("Try it")}
            intro={t(
              "Bootable images of the latest build, for a virtual machine or a PC. They are early developer images: they show the kernel booting and running programs, not a desktop to use yet.",
            )}
          >
            <ul className="cards cards-wide downloads">
              {L(images).map((i) => (
                <li key={i.arch} id={`get-${i.arch}`}>
                  <div className="card-top">
                    <h4>{i.arch}</h4>
                    <span className="dl-size">{i.size}</span>
                  </div>
                  <p>{i.what}</p>
                  <p className="dl-links">
                    <a className="btn" href={`${imagesUrl}/${i.file}`}>
                      {t("Download")} <span dir="ltr">{i.file}</span>
                    </a>
                    <a href={`${imagesUrl}/${i.file}.sha256`}>SHA-256</a>
                    <a href={`${imagesUrl}/${i.file.replace(/\.iso$/, "")}.commit`}>commit</a>
                  </p>
                  <p className="dl-label">{t("Run it in QEMU")}</p>
                  <pre className="cmd">
                    <code>{i.qemu}</code>
                  </pre>
                  <p className="dl-needs">
                    {t("Needs")} {i.needs}
                  </p>
                </li>
              ))}
            </ul>

            <div className="explain">
              <div>
                <h3>{t("What you will see")}</h3>
                <p>{translate(lang, imageDemo)}</p>
              </div>
              <div>
                <h3>{t("Check the download")}</h3>
                <p>
                  {t("Each image has its SHA-256 beside it. In the folder you downloaded both files to:")}
                </p>
                <pre className="cmd">
                  <code>sha256sum -c gala-x86_64.iso.sha256</code>
                </pre>
                <p>
                  <Both
                    en={
                      <>
                        The <code>.commit</code> file names the commit it was
                        built from.
                      </>
                    }
                    ar={
                      <>
                        ويسمّي ملف <code>.commit</code> الإيداع (commit) الذي
                        بُنيت منه الصورة.
                      </>
                    }
                  />
                </p>
              </div>
              <div>
                <h3>{t("On a real PC")}</h3>
                <p>
                  <Both
                    en={
                      <>
                        The x86_64 image also boots a PC from a USB stick, with
                        nothing installed. The steps are{" "}
                        <a href="#usb">below</a>.
                      </>
                    }
                    ar={
                      <>
                        وصورة x86_64 تُقلع حاسوبًا شخصيًا من ذاكرة USB أيضًا،
                        دون تثبيت أي شيء. الخطوات <a href="#usb">أدناه</a>.
                      </>
                    }
                  />
                </p>
              </div>
            </div>
          </Section>

          <UsbSection />

          <Section id="principles" title={t("Principles")} intro={t("The rules every contribution is held to.")}>
            <dl className="principles">
              {L(decisions).map((d) => (
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
          <nav aria-label={t("Links")}>
            {nav.map((n) => (
              <a key={n.id} href={`#${n.id}`}>
                {t(n.label)}
              </a>
            ))}
          </nav>
          <LangSwitch />
          <p className="foot-note">
            <Both
              en={
                <>
                  The name is the Arabic <i lang="ar-Latn">qal&rsquo;a</i>,
                  fortress. No licence is granted to its code.
                </>
              }
              ar={<>الاسم من الكلمة العربية «قلعة». لا تُمنح أي رخصة لاستخدام شيفرته.</>}
            />
          </p>
        </div>
      </footer>
    </>
  );
}
