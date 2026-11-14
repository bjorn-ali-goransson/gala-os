/**
 * The hero: a cross-section through gala, read top to bottom, drawn as the
 * wall of a qal'a — the fortress the project is named after. The strata are
 * the real layers, and the heavy course through the middle is the one place
 * where gala imitates Windows.
 */
export function Wall() {
  return (
    <figure className="wall">
      <svg className="wall-parapet" aria-hidden="true" role="presentation">
        <defs>
          <pattern
            id="merlons"
            patternUnits="userSpaceOnUse"
            width="26"
            height="20"
          >
            <path d="M2 20 L13 2 L24 20 Z" />
          </pattern>
        </defs>
        <rect width="100%" height="20" fill="url(#merlons)" />
      </svg>

      <div className="stratum s-programs" style={{ "--d": "0.30s" } as never}>
        <p className="stratum-name">Programs, as their makers shipped them</p>
        <div className="cells">
          <div className="cell">
            <p className="cell-name">Windows executables</p>
            <ul>
              <li>chrome.exe</li>
              <li>java.exe</li>
              <li>postgres.exe</li>
              <li>nginx.exe</li>
              <li>sshd.exe</li>
              <li>pwsh.exe</li>
            </ul>
          </div>
          <div className="cell cell-narrow">
            <p className="cell-name">Linux binaries</p>
            <ul>
              <li>busybox</li>
              <li>httpd</li>
              <li>wget</li>
            </ul>
          </div>
        </div>
      </div>

      <p className="seam" style={{ "--d": "0.22s" } as never}>
        The ABI boundary
        <span>the only place gala imitates Windows</span>
      </p>

      <div className="stratum s-pers" style={{ "--d": "0.16s" } as never}>
        <div className="cells">
          <div className="cell">
            <p className="cell-name">NT personality</p>
            <ul>
              <li>system call entry</li>
              <li>PE loader</li>
              <li>TEB, PEB</li>
              <li>handles and objects</li>
              <li>50+ clean-room DLLs</li>
            </ul>
          </div>
          <div className="cell cell-narrow">
            <p className="cell-name">Linux personality</p>
            <ul>
              <li>its own syscalls</li>
              <li>ELF loader</li>
            </ul>
          </div>
        </div>
      </div>

      <div className="stratum s-core" style={{ "--d": "0.08s" } as never}>
        <p className="stratum-name">gala&rsquo;s own core</p>
        <ul className="core-cells">
          <li>physical and virtual memory</li>
          <li>threads and the scheduler</li>
          <li>interrupts and one clock</li>
          <li>files, pipes and I/O</li>
          <li>drivers</li>
          <li>TCP/IP</li>
        </ul>
      </div>

      <p className="stratum s-hw" style={{ "--d": "0s" } as never}>
        x86_64, every processor scheduling &middot; virtio disk, network and
        display &middot; UEFI or BIOS
      </p>

      <figcaption>
        Read downwards. Everything under the heavy course is gala&rsquo;s own
        design and free to be rebuilt; everything above it is software that was
        never told it had moved.
      </figcaption>
    </figure>
  );
}
