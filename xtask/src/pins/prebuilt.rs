//! The pinned downloads of Windows programs (PREBUILT) and Office's
//! installation source (OFFICE_SOURCE). CI's prebuilt caches are keyed
//! on this directory's files, so a branch that changes no pin reuses
//! them (WORKFLOW "CI build caches").

/// Unmodified prebuilt Windows programs that scenarios run, fetched on
/// first use into target/prebuilt/<name>/ and checked against their
/// SHA-256 (they are not kept in the repository). THIRD_PARTY.md lists
/// them.
pub struct Prebuilt {
    pub name: &'static str,
    pub url: &'static str,
    /// The release's SHA-256, or its SHA-512 (128 hex digits).
    pub checksum: &'static str,
    /// The files used, inside the zip. A release that is one file, not an
    /// archive, names just that file (the URL's last component): the
    /// download is moved there. For an MSI (a `.msi` URL, unpacked
    /// with msitools' msiextract: gala has no Windows Installer), the
    /// directories used, each moved to the release's directory under its
    /// last component (`Program Files/X/Binn` becomes `Binn`).
    pub members: &'static [&'static str],
    /// Files left out of those (unzip -x patterns; for an MSI, paths
    /// removed after the moves, such as `Binn/de`).
    pub excludes: &'static [&'static str],
}

pub const PREBUILT: &[Prebuilt] = &[
    Prebuilt {
        name: "caddy",
        url: "https://github.com/caddyserver/caddy/releases/download/v2.8.4/caddy_2.8.4_windows_amd64.zip",
        checksum: "2631fbe1c4f6fb08783ed6dc6ac238d6655b4c923abe2bce1433dbd7fd079630",
        members: &["caddy.exe"],
        excludes: &[],
    },
    // Eclipse Temurin (OpenJDK) 21.0.5+11; the SHA-256 is Adoptium's
    // published one. What `java` needs: the launcher, the DLLs (with
    // Microsoft's C runtime), the server VM, the module image and the
    // configuration (not the class data archives, tools or sources).
    Prebuilt {
        name: "jdk",
        url: "https://github.com/adoptium/temurin21-binaries/releases/download/jdk-21.0.5%2B11/OpenJDK21U-jdk_x64_windows_hotspot_21.0.5_11.zip",
        checksum: "6f09d4a3598542313cca1540106d537c7092a54e415d569f7b928160a90d3128",
        members: &[
            "jdk-21.0.5+11/bin/java.exe",
            "jdk-21.0.5+11/bin/*.dll",
            "jdk-21.0.5+11/lib/modules",
            "jdk-21.0.5+11/lib/jvm.cfg",
            "jdk-21.0.5+11/lib/tzdb.dat",
            "jdk-21.0.5+11/lib/tzmappings",
            "jdk-21.0.5+11/lib/security/*",
            "jdk-21.0.5+11/conf/*",
            "jdk-21.0.5+11/release",
        ],
        excludes: &[],
    },
    // The .NET 8 runtime (Microsoft.NETCore.App and the dotnet host); the
    // SHA-512 is Microsoft's published one (releases.json).
    Prebuilt {
        name: "dotnet",
        url: "https://builds.dotnet.microsoft.com/dotnet/Runtime/8.0.33/dotnet-runtime-8.0.33-win-x64.zip",
        checksum: "ceb56fa91c15aa7c465db8cd42ee4e978589c58885cd1f164f8fcde7231ea15c3cfc0bcddb2e6b452238b3687a3e2bcb3683349b8b92a7172e03154ed280be67",
        members: &[],
        excludes: &[],
    },
    // PowerShell 7.4 (a self-contained .NET 8 application); the SHA-256 is
    // the one in the release's hashes.sha256.
    // CPython 3.13.1's embeddable package for x64 (python.org; the
    // SHA-256 is python.org's published one): python.exe, python313.dll,
    // the standard library as python313.zip and its extension modules.
    // The UCRT it needs is the JDK's (gala has none of its own).
    Prebuilt {
        name: "python",
        url: "https://www.python.org/ftp/python/3.13.1/python-3.13.1-embed-amd64.zip",
        checksum: "7b7923ff0183a8b8fca90f6047184b419b108cb437f75fc1c002f9d2f8bcec16",
        members: &[],
        excludes: &[],
    },
    // CPython 3.13.1's full layout for x64: NuGet's `python` package
    // (the one python.org publishes for build machines; SHA-256 measured,
    // nuget.org verifies its packages by signature): the standard library
    // as files with pip 24.3.1 in site-packages, distlib's script
    // launchers and venv's launcher, which the embeddable one lacks.
    Prebuilt {
        name: "python-full",
        url: "https://api.nuget.org/v3-flatcontainer/python/3.13.1/python.3.13.1.nupkg",
        checksum: "eba9c88a4b0bed9a1b48a18910f10db066eebf27b58443f329775d15984ac2df",
        members: &["tools/*"],
        excludes: &[],
    },
    // numpy 2.2.1's wheel for CPython 3.13 on x64 (PyPI; the SHA-256 is
    // PyPI's published digest): extension modules with OpenBLAS and
    // msvcp140 bundled in numpy.libs. pip installs it in the guest.
    Prebuilt {
        name: "numpy",
        url: "https://files.pythonhosted.org/packages/44/be/0e5cd009d2162e4138d79a5afb3b5d2341f0fe4777ab6e675aa3d4a42e21/numpy-2.2.1-cp313-cp313-win_amd64.whl",
        checksum: "164a829b6aacf79ca47ba4814b130c4020b202522a93d7bff2202bfb33b61c60",
        members: &["numpy-2.2.1-cp313-cp313-win_amd64.whl"],
        excludes: &[],
    },
    Prebuilt {
        name: "pwsh",
        url: "https://github.com/PowerShell/PowerShell/releases/download/v7.4.22/PowerShell-7.4.22-win-x64.zip",
        checksum: "5f89f51860a1a3b07937945663afd7cbd44387361ff9985f5758939cb9bd6359",
        members: &[],
        excludes: &[],
    },
    // Elasticsearch 8.15.3's Windows zip; the SHA-512 is Elastic's
    // published one (the .sha512 next to the zip). Without its bundled JDK
    // (gala's Temurin 21 goes in its place) and the machine learning
    // native programs (ML is disabled), ~250 MB instead of 1 GB unpacked.
    Prebuilt {
        name: "elasticsearch",
        url: "https://artifacts.elastic.co/downloads/elasticsearch/elasticsearch-8.15.3-windows-x86_64.zip",
        checksum: "25eed8feba05e4e398bc9700eaea2042b4166ab3ca75eb9b34a6394d9eebbf7de5eb1c8516f72ed587d0428cfe3966fb45b18587fabc47b1828826d59da4947c",
        members: &[
            "elasticsearch-8.15.3/lib/*",
            "elasticsearch-8.15.3/modules/*",
            "elasticsearch-8.15.3/config/*",
            "elasticsearch-8.15.3/logs/",
            "elasticsearch-8.15.3/plugins/",
        ],
        excludes: &["elasticsearch-8.15.3/modules/x-pack-ml/platform/*"],
    },
    // Win32-OpenSSH 9.8.3.0p2-Preview, Microsoft's port (OpenSSH-Win64.zip
    // from github.com/PowerShell/Win32-OpenSSH releases). No checksum is
    // published with the release: the SHA-256 is the one we measured
    // (THIRD_PARTY.md). The whole directory (sshd, its session process,
    // ssh-keygen, the client, libcrypto, the default configuration).
    Prebuilt {
        name: "openssh",
        url: "https://github.com/PowerShell/Win32-OpenSSH/releases/download/v9.8.3.0p2-Preview/OpenSSH-Win64.zip",
        checksum: "0ca131f3a78f404dc819a6336606caec0db1663a692ccc3af1e90232706ada54",
        members: &["OpenSSH-Win64/*"],
        excludes: &["OpenSSH-Win64/_manifest/*"],
    },
    // PostgreSQL 17.2, EnterpriseDB's Windows binaries zip. EDB publishes
    // no checksum: the SHA-256 is the one we measured (THIRD_PARTY.md).
    // The server and its tools, their DLLs and extension modules and the
    // data initdb copies; not pgAdmin, StackBuilder, the documentation,
    // headers, import libraries or translations.
    Prebuilt {
        name: "postgresql",
        url: "https://get.enterprisedb.com/postgresql/postgresql-17.2-1-windows-x64-binaries.zip",
        checksum: "115eb69fa92430f162bdda2eced073b60fed538715fb3a19ce0bb8c2d8738d01",
        members: &["pgsql/bin/*", "pgsql/lib/*", "pgsql/share/*"],
        excludes: &[
            "pgsql/bin/wx*",
            "pgsql/bin/stackbuilder.exe",
            "pgsql/lib/*.lib",
            "pgsql/lib/*.a",
            "pgsql/share/locale/*",
        ],
    },
    // curl for Windows 8.23.0_8 (curl.se/windows, the official build:
    // llvm-mingw against the UCRT, LibreSSL as its only TLS backend, the
    // Windows ROOT store as its CA list); SHA-256 as curl.se/windows
    // publishes it (which keeps each version at this URL). Only curl.exe
    // (libcurl linked in).
    Prebuilt {
        name: "curl",
        url: "https://curl.se/windows/dl-8.23.0_8/curl-8.23.0_8-win64-mingw.zip",
        checksum: "ca5b0fd6e7a6dbde210281f545c73a89784f2e88104a2cdfe0b73ea3dfed5b3a",
        members: &["curl-8.23.0_8-win64-mingw/bin/curl.exe"],
        excludes: &[],
    },
    // Git for Windows' curl (PortableGit 2.47.1, the release below):
    // curl.exe and its libcurl-4.dll, built with Schannel as its only TLS
    // backend (the libcurl MinGit's git-remote-https loads too), and the
    // DLLs that one imports. curl.se's own Windows build (above) has
    // LibreSSL only.
    Prebuilt {
        name: "gitcurl",
        url: "https://github.com/git-for-windows/git/releases/download/v2.47.1.windows.1/PortableGit-2.47.1-64-bit.7z.exe",
        checksum: "4f3f21f4effcb659566883ee1ed3ae403e5b3d7a0699cee455f6cd765e1ac39c",
        members: &[
            "mingw64/bin/curl.exe",
            "mingw64/bin/libcurl-4.dll",
            "mingw64/bin/libbrotlicommon.dll",
            "mingw64/bin/libbrotlidec.dll",
            "mingw64/bin/libiconv-2.dll",
            "mingw64/bin/libidn2-0.dll",
            "mingw64/bin/libintl-8.dll",
            "mingw64/bin/libpsl-5.dll",
            "mingw64/bin/libssh2-1.dll",
            "mingw64/bin/libunistring-5.dll",
            "mingw64/bin/libzstd.dll",
            "mingw64/bin/zlib1.dll",
        ],
        excludes: &[],
    },
    // MinGit 2.47.1 (Git for Windows' minimal distribution: git and the
    // MSYS2 runtime its scripts need); SHA-256 from the release notes.
    Prebuilt {
        name: "mingit",
        url: "https://github.com/git-for-windows/git/releases/download/v2.47.1.windows.1/MinGit-2.47.1-64-bit.zip",
        checksum: "50b04b55425b5c465d076cdb184f63a0cd0f86f6ec8bb4d5860114a713d2c29a",
        members: &["cmd/*", "etc/*", "mingw64/*", "usr/*"],
        excludes: &[],
    },
    // PortableGit 2.47.1: all of Git for Windows (Git Bash, the MSYS2
    // runtime and tools) without its installer; SHA-256 from the release
    // notes. What bash and git need, not Perl, Tcl/Tk or the docs.
    Prebuilt {
        name: "portablegit",
        url: "https://github.com/git-for-windows/git/releases/download/v2.47.1.windows.1/PortableGit-2.47.1-64-bit.7z.exe",
        checksum: "4f3f21f4effcb659566883ee1ed3ae403e5b3d7a0699cee455f6cd765e1ac39c",
        members: &["bin", "cmd", "etc", "usr/bin", "mingw64/bin", "mingw64/libexec/git-core"],
        excludes: &[],
    },
    // FileZilla Client 3.71.0 for 64-bit Windows: the official installer,
    // as Chocolatey's package carries it (the project's site hands out
    // download links only to browsers, and SourceForge drops old releases).
    // The package's SHA-256 is measured; it carries the 32-bit and the
    // 64-bit installer, and the unpack takes the 64-bit one. An NSIS
    // installer, read by 7-Zip: the program, its DLLs and resources; not
    // the NSIS plugins, translations or docs.
    Prebuilt {
        name: "filezilla",
        url: "https://packages.chocolatey.org/filezilla.3.71.0.nupkg",
        checksum: "fa234cea2f88d73062de1b5b0cac509a295ea63ee13f5a8618839a5552a51780",
        members: &["*.exe", "*.dll", "resources"],
        excludes: &["$PLUGINSDIR", "uninstall.exe"],
    },
    // FileZilla Server 1.12.6 for 64-bit Windows: the official installer,
    // as Chocolatey's package carries it (the project's site hands out
    // download links only to browsers). The package's SHA-256 is the one
    // Scoop's manifest pins; the installer inside is the project's
    // (SHA-256 40b1526d...63ba, in the package's VERIFICATION.txt). An
    // NSIS installer, read by 7-Zip: the server, its tools and DLLs.
    Prebuilt {
        name: "filezilla-server",
        url: "https://packages.chocolatey.org/filezilla.server.1.12.6.nupkg",
        checksum: "af04bfcc64aa7140ad92cd9f602f52714d1ffcee5cc1803e3c80c45ac019fe39",
        members: &["*.exe", "*.dll"],
        excludes: &["$PLUGINSDIR", "uninstall.exe"],
    },
    // SQL Server 2022 Express LocalDB 16.0.1000.6 (SqlLocalDB.msi).
    // Microsoft publishes no checksum for it: the SHA-256 is the one we
    // measured (ROADMAP). Its license allows use to design, develop and
    // test programs (THIRD_PARTY.md). The database engine's directory
    // (LocalDB\Binn) with its English resources only, without the
    // secure-enclave packages.
    Prebuilt {
        name: "sqllocaldb",
        url: "https://download.microsoft.com/download/3/8/d/38de7036-2433-4207-8eae-06e247e17b25/SqlLocalDB.msi",
        checksum: "224d483992ef60368dac70cea174dcfaf43a3ca06ada331c67dc6119a26490f6",
        members: &[
            "Program Files/Microsoft SQL Server/160/LocalDB/Binn",
            "Program Files/Microsoft SQL Server/160/Shared",
        ],
        excludes: &[
            "Binn/aetm-enclave.sgx.pkg",
            "Binn/aetm-enclave.vsm.pkg",
            "Binn/de",
            "Binn/es",
            "Binn/fr",
            "Binn/it",
            "Binn/ja",
            "Binn/ko",
            "Binn/pt",
            "Binn/ru",
            "Binn/zh-Hans",
            "Binn/zh-Hant",
            "Binn/Resources/1028",
            "Binn/Resources/1031",
            "Binn/Resources/1036",
            "Binn/Resources/1040",
            "Binn/Resources/1041",
            "Binn/Resources/1042",
            "Binn/Resources/1046",
            "Binn/Resources/1049",
            "Binn/Resources/2052",
            "Binn/Resources/3082",
            "Binn/Resources/de-DE",
            "Binn/Resources/es-ES",
            "Binn/Resources/fr-FR",
            "Binn/Resources/it-IT",
            "Binn/Resources/ja-JP",
            "Binn/Resources/ko-KR",
            "Binn/Resources/pt-BR",
            "Binn/Resources/ru-RU",
            "Binn/Resources/zh-CN",
            "Binn/Resources/zh-TW",
        ],
    },
    // FlaUI 5.0.0 (MIT), the UI Automation test library flaui_test uses:
    // NuGet packages (zips) from nuget.org, SHA-256 of the .nupkg as
    // downloaded (nuget.org publishes SHA-512 of the package in its
    // catalog; THIRD_PARTY.md). The .NET 8 builds and the license.
    Prebuilt {
        name: "flaui-core",
        url: "https://api.nuget.org/v3-flatcontainer/flaui.core/5.0.0/flaui.core.5.0.0.nupkg",
        checksum: "191cc65ea82036b77f1872e6d4ebf743d3d120895bba2fa5248d126cd6f568a7",
        members: &["lib/net8.0-windows7.0/FlaUI.Core.dll", "LICENSE.txt"],
        excludes: &[],
    },
    Prebuilt {
        name: "flaui-uia3",
        url: "https://api.nuget.org/v3-flatcontainer/flaui.uia3/5.0.0/flaui.uia3.5.0.0.nupkg",
        checksum: "d5d2e083539a04bf6c9053781dfd47332854a76faeb8e9c3c68cc82109f711f2",
        members: &["lib/net8.0-windows7.0/FlaUI.UIA3.dll", "LICENSE.txt"],
        excludes: &[],
    },
    // Interop.UIAutomationClient 10.19041.0 (MIT): the COM interop
    // assembly for UIAutomationClient (generated from Windows' type
    // library) that FlaUI.UIA3 calls through.
    Prebuilt {
        name: "uiautomation-interop",
        url: "https://api.nuget.org/v3-flatcontainer/interop.uiautomationclient/10.19041.0/interop.uiautomationclient.10.19041.0.nupkg",
        checksum: "0d2ed17db2cb13a262f0580ab992db87577651c25a9899053df4114b4d9ff3b1",
        members: &["lib/netcoreapp3.0/Interop.UIAutomationClient.dll", "LICENSE.txt"],
        excludes: &[],
    },
    // Chromium 159.0.8140.0 (BSD-style), the open-source browser Edge is
    // built on: a build of the official snapshot bucket (Win_x64 revision
    // 1733063, refs/heads/main@{#1733063}, commit 20692aad; one
    // whose symbols (chrome-win32-syms.zip) are published too). Google
    // publishes no checksum for snapshots: the SHA-256 is the one we
    // measured (THIRD_PARTY.md). The browser (chrome.exe, chrome.dll,
    // chrome_elf.dll), its data (ICU, V8's snapshot, the resource packs,
    // English only) and the software GL/Vulkan DLLs; not the test
    // program (interactive_ui_tests.exe, 375 MB), the installer or the
    // services.
    Prebuilt {
        name: "chromium",
        url: "https://commondatastorage.googleapis.com/chromium-browser-snapshots/Win_x64/1733063/chrome-win.zip",
        checksum: "084b4343a8db0d3a24060078271d098ce788f9b9e75e202cdc9382521b0de6c9",
        members: &[
            "chrome-win/chrome.exe",
            "chrome-win/chrome.dll",
            "chrome-win/chrome_elf.dll",
            "chrome-win/chrome_wer.dll",
            "chrome-win/eventlog_provider.dll",
            "chrome-win/159.0.8140.0.manifest",
            "chrome-win/icudtl.dat",
            "chrome-win/v8_context_snapshot.bin",
            "chrome-win/resources.pak",
            "chrome-win/chrome_100_percent.pak",
            "chrome-win/locales/en-US.pak",
            "chrome-win/libEGL.dll",
            "chrome-win/libGLESv2.dll",
            "chrome-win/vk_swiftshader.dll",
            "chrome-win/vk_swiftshader_icd.json",
            "chrome-win/vulkan-1.dll",
        ],
        excludes: &[],
    },
    // BR's conformance report: ChromeDriver from the same snapshot (the
    // Win_x64 build's chromedriver_win32.zip holds the x64 driver).
    Prebuilt {
        name: "chromedriver",
        url: "https://commondatastorage.googleapis.com/chromium-browser-snapshots/Win_x64/1733063/chromedriver_win32.zip",
        checksum: "34e5c461af9fadaabf5582b5f3c227b354e64667fdc7c1b03787114a24f015e2",
        members: &["chromedriver_win32/chromedriver.exe"],
        excludes: &[],
    },
    // busybox 1.35.0 for Linux, static (musl), as busybox.net publishes
    // it (a single ELF file, not an archive: the file is the download;
    // no published checksum, the SHA-256 pinned at the first fetch). The
    // Linux personality's milestone 2 target (ROADMAP LX); it runs only
    // inside gala.
    // aarch64's: Alpine's static busybox (musl, static PIE; busybox.net
    // has no aarch64 build), from the release's pinned package (an apk:
    // gzipped tar streams), for the WSL distribution of the aarch64
    // images (make_wsl_rootfs).
    Prebuilt {
        name: "busybox-linux-aarch64",
        url: "https://dl-cdn.alpinelinux.org/alpine/v3.24/main/aarch64/busybox-static-1.37.0-r31.apk",
        checksum: "965777e06b94bf11981d5f4ecdfcd577879f4b0dda544294d1dd3f72b217bc75",
        members: &["bin/busybox.static"],
        excludes: &[],
    },
    Prebuilt {
        name: "busybox-linux",
        url: "https://busybox.net/downloads/binaries/1.35.0-x86_64-linux-musl/busybox",
        checksum: "6e123e7f3202a8c1e9b1f94d8941580a25135382b99e8d3e34fb858bba311348",
        members: &["busybox"],
        excludes: &[],
    },
    // glibc 2.43 as Ubuntu 26.04 LTS (resolute) ships it: the libc6
    // package of the release pocket (2.43-2ubuntu2, never changed once
    // released; the SHA-256 is the one in the release's signed Packages
    // index: THIRD_PARTY.md). A Debian package (an ar
    // archive) unpacked with dpkg-deb; its members moved here by their
    // file names. The program interpreter, libc and libm that dynamically
    // linked Linux programs (user/lx's GLIBC_TESTS) load (ROADMAP LD);
    // they run only inside gala.
    Prebuilt {
        name: "glibc",
        url: "https://archive.ubuntu.com/ubuntu/pool/main/g/glibc/libc6_2.43-2ubuntu2_amd64.deb",
        checksum: "c13775dc0c984403f3fcad229d14507a9f387763bd07ace1e5f93897ee6b8434",
        members: &[
            "usr/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2",
            "usr/lib/x86_64-linux-gnu/libc.so.6",
            "usr/lib/x86_64-linux-gnu/libm.so.6",
        ],
        excludes: &[],
    },
    // The same glibc for aarch64 (the twins of the glibc scenarios:
    // GLIBC_ARM64), the release's arm64 libc6 from ports.ubuntu.com (the
    // SHA-256 in its signed Packages index).
    Prebuilt {
        name: "glibc-arm64",
        url: "https://ports.ubuntu.com/ubuntu-ports/pool/main/g/glibc/libc6_2.43-2ubuntu2_arm64.deb",
        checksum: "6be782d90425acbdf9b7523f791a69a0d3f8770be0750b5cce71d4cdc18397c0",
        members: &[
            "usr/lib/aarch64-linux-gnu/ld-linux-aarch64.so.1",
            "usr/lib/aarch64-linux-gnu/libc.so.6",
            "usr/lib/aarch64-linux-gnu/libm.so.6",
        ],
        excludes: &[],
    },
    // weston 14.0.2's demo clients as Ubuntu 26.04 (resolute) ships them
    // (universe, the release pocket's weston 14.0.2-5; the SHA-256
    // is the one in the release's signed Packages index): only
    // weston-simple-shm, a real libwayland client (ROADMAP LD step 2,
    // scenario lx_simple_shm). It needs libwayland-client and libffi
    // (below) besides glibc. Inside gala only.
    Prebuilt {
        name: "weston",
        url: "https://archive.ubuntu.com/ubuntu/pool/universe/w/weston/weston_14.0.2-5_amd64.deb",
        checksum: "edf3509ee3b2fb0b94d45e3e090be2a5cdf0a18e4ebdfef2160d2e2b1c05090f",
        members: &["usr/bin/weston-simple-shm"],
        excludes: &[],
    },
    // libwayland-client 1.24.0 (Ubuntu 26.04's libwayland-client0
    // 1.24.0-2, main; SHA-256 from the signed Packages index).
    // The member is the soname's symbolic link: its target is kept under
    // the link's name.
    Prebuilt {
        name: "libwayland-client",
        url: "https://archive.ubuntu.com/ubuntu/pool/main/w/wayland/libwayland-client0_1.24.0-2_amd64.deb",
        checksum: "094db0e190b53872e4d5310cd5fa09ca28ee3936386cd09e5f7d13e645aa12ab",
        members: &["usr/lib/x86_64-linux-gnu/libwayland-client.so.0"],
        excludes: &[],
    },
    // libffi 3.5.2 (Ubuntu 26.04's libffi8 3.5.2-4, main; SHA-256
    // from the signed Packages index): libwayland-client's dependency.
    Prebuilt {
        name: "libffi",
        url: "https://archive.ubuntu.com/ubuntu/pool/main/libf/libffi/libffi8_3.5.2-4_amd64.deb",
        checksum: "4a7423124bbd459c0aa38b7389ec7802fa6899654767a95fc27de70967fedd48",
        members: &["usr/lib/x86_64-linux-gnu/libffi.so.8"],
        excludes: &[],
    },
    // The same two for aarch64 (twins of Wayland client scenarios), the
    // release's arm64 packages from ports.ubuntu.com (SHA-256s from its
    // signed Packages index).
    Prebuilt {
        name: "libwayland-client-arm64",
        url: "https://ports.ubuntu.com/ubuntu-ports/pool/main/w/wayland/libwayland-client0_1.24.0-2_arm64.deb",
        checksum: "8459ccec5bdd6aa15dd1e75ea450d97e514f8448e529c4fd08cc036f23f4235a",
        members: &["usr/lib/aarch64-linux-gnu/libwayland-client.so.0"],
        excludes: &[],
    },
    Prebuilt {
        name: "libffi-arm64",
        url: "https://ports.ubuntu.com/ubuntu-ports/pool/main/libf/libffi/libffi8_3.5.2-4_arm64.deb",
        checksum: "cfab73343f076b79df0f4a9d5430319fdb6153760d3e8c2964c448b49edad084",
        members: &["usr/lib/aarch64-linux-gnu/libffi.so.8"],
        excludes: &[],
    },
    // Inno Setup 7.1.1's own installer, x64 edition (jrsoftware.org's
    // GitHub release; the SHA-256 is winget-pkgs' JRSoftware.InnoSetup.7
    // 7.1.1 manifest's): an Inno Setup installer built with
    // SetupArchitecture=x64 (Inno Setup 7), so x64 Setup and uninstaller
    // gala can run without WOW64. Not unpacked: `gala install` runs it
    // (ROADMAP PK stage 3, scenario pk_install_inno); inside gala only.
    Prebuilt {
        name: "innosetup",
        url: "https://github.com/jrsoftware/issrc/releases/download/is-7_1_1/innosetup-7.1.1-x64.exe",
        checksum: "8a5a6711c3766a6e5fb74f280ba0e3551e719ce94538d901fdbd1f8f9920b3f0",
        members: &["innosetup-7.1.1-x64.exe"],
        excludes: &[],
    },
    // The Office Deployment Tool's setup.exe (officecdn.microsoft.com's
    // current one, 16.0.20527.20150, x86; the SHA-256 is the one we
    // measured: Microsoft replaces the file at this URL with each release,
    // so a new one fails the checksum until it is pinned again). Scenario
    // office_install (ROADMAP OF) runs it inside gala; never committed.
    Prebuilt {
        name: "odt",
        url: "https://officecdn.microsoft.com/pr/wsus/setup.exe",
        checksum: "daea2a1c2beef16e88dded58916b9c4d816ff247e76ee5ebb41c3810fee16be1",
        members: &["setup.exe"],
        excludes: &[],
    },
    // Inno Setup 6.5.4's own installer, an x86 Inno Setup installer
    // (jrsoftware.org's GitHub release; the SHA-256 is winget-pkgs'
    // JRSoftware.InnoSetup 6.5.4 manifest's, whose only installer is x86):
    // a 32-bit Setup and uninstaller under WOW64 (ROADMAP WOW stage 4,
    // scenario wow64_inno). Not unpacked; inside gala only.
    Prebuilt {
        name: "innosetup6",
        url: "https://github.com/jrsoftware/issrc/releases/download/is-6_5_4/innosetup-6.5.4.exe",
        checksum: "fa73bf47a4da250d185d07561c2bfda387e5e20db77e4570004cf6a133cc10b1",
        members: &["innosetup-6.5.4.exe"],
        excludes: &[],
    },
    // Mesa 26.2.5 for 64-bit Windows (MIT), pal1000/mesa-dist-win's MSVC
    // release package (the build Mesa's own documentation points Windows
    // users to; static C runtime). No checksum is published with the
    // release: the SHA-256 is the one we measured (THIRD_PARTY.md). GPU G2's
    // software 3D: llvmpipe's OpenGL (`opengl32.dll`, the WGL runtime, and
    // `libgallium_wgl.dll`, the gallium megadriver it loads) and lavapipe's
    // Vulkan ICD (`vulkan_lvp.dll` and its manifest). 26.2.6's MSVC package
    // lacks the OpenGL files, hence 26.2.5.
    Prebuilt {
        name: "mesa",
        url: "https://github.com/pal1000/mesa-dist-win/releases/download/26.2.5/mesa3d-26.2.5-release-msvc.7z",
        checksum: "997123ef4ce8896204ab912d5b54f19372c487ff9835bcefafa2fca23b8953c2",
        members: &[
            "x64/opengl32.dll",
            "x64/libgallium_wgl.dll",
            "x64/vulkan_lvp.dll",
            "x64/lvp_icd.x86_64.json",
        ],
        excludes: &[],
    },
    // The Vulkan Runtime 1.4.341.0 for x64 (LunarG's components zip of
    // the Khronos loader, Apache-2.0 and MIT; the SHA-256 is the one we
    // measured: LunarG publishes none for it): `vulkan-1.dll`, which
    // Windows' GPU drivers install in System32, its PDB (not shipped:
    // `llvm-symbolizer --obj=<dll> <address>` names the loader's source
    // line in a crash report), and `vulkaninfo.exe`.
    Prebuilt {
        name: "vulkan-runtime",
        url: "https://sdk.lunarg.com/sdk/download/1.4.341.0/windows/VulkanRT-X64-1.4.341.0-Components.zip",
        checksum: "20571dcc8292b80a47d68bbbfa4094d87044041f50f4e2e1f326a01e3732b226",
        members: &[
            "VulkanRT-X64-1.4.341.0-Components/x64/vulkan-1.dll",
            "VulkanRT-X64-1.4.341.0-Components/x64/vulkan-1.pdb",
            "VulkanRT-X64-1.4.341.0-Components/x64/vulkaninfo.exe",
            "VulkanRT-X64-1.4.341.0-Components/VulkanRT-License.txt",
        ],
        excludes: &[],
    },
    // DXVK 3.1.1 for x64 (github.com/doitsujin/dxvk, zlib/libpng;
    // THIRD_PARTY.md): Direct3D over Vulkan. It ships beside the program
    // that uses it, never in System32, so that System32's dxgi stays
    // gala's -- the Vulkan loader calls that one (GPU G2, `d3d11_test`).
    // d3d9, d3d8 and d3d10core are in the release too and follow with
    // the programs that want them.
    Prebuilt {
        name: "dxvk",
        url: "https://github.com/doitsujin/dxvk/releases/download/v3.1.1/dxvk-3.1.1.tar.gz",
        checksum: "40565b4a724aadc4433fa4e010b4b23916d9b1f1baeee64e17186db94f54e608",
        members: &["dxvk-3.1.1/x64/d3d11.dll", "dxvk-3.1.1/x64/dxgi.dll"],
        excludes: &[],
    },
];

/// The version of Office the host's cache holds: office_install's
/// products (64-bit, English, Current channel) as the CDN serves them, in
/// the layout of ODT's `setup.exe /download` (`Office\Data\...`), so that
/// config_local.xml's SourcePath finds them. Pinned as PREBUILT is: a
/// newer build is a deliberate change of these lines (the checksums from
/// `cargo xtask office-source --pin`).
pub const OFFICE_SOURCE_VERSION: &str = "16.0.20527.20196";
/// The Current channel's content on the CDN (the client's own log names
/// it: `pr/492350f6-.../Office/Data/v64.cab`).
pub const OFFICE_SOURCE_URL: &str =
    "http://officecdn.microsoft.com/pr/492350f6-3a01-4f97-b9c0-c7c6ddf67d60/Office/Data";
/// The files under `Office\Data` (`{v}`: the version) and their SHA-256s:
/// those the Click-to-Run client fetched for these products in
/// office_install's runs (its log names each). Every name carries the
/// version, which is what makes a checksum meaningful: the channel's
/// unversioned `v64.cab` is a pointer the CDN composes per request, not
/// content, and the cache writes its own (`write_version_pointer`).
pub const OFFICE_SOURCE: &[(&str, &str)] = &[
    ("v64_{v}.cab", "231c411348a65fde60f14081f35d9771ea42b60b62e7fd14bbc1f412be58bbd1"),
    ("{v}/i640.cab", "72b07bef01bc57d9684b380abe01a65910d80411e5bd4cad901c73bcbd87148f"),
    ("{v}/i641033.cab", "1ddca4b1ed0f68b646ff7f367acec502f4306124fadbbbc630b816da2a017802"),
    ("{v}/s640.cab", "d2d0ed663372aa38ce24dfe3af0a1db6814f821eff6cb8bf069bca2560bf759c"),
    ("{v}/s641033.cab", "84892149bac189603809e698014f9b7f0016c26614198116aea21b7ebfbea7f7"),
    (
        "{v}/stream.x64.en-us.dat",
        "6f4c27dddfb1b35dc75fd5e18ba1c403d533aa8c4b18718331b2f2e13e18d59b",
    ),
    (
        "{v}/stream.x64.x-none.dat",
        "e9ec4b9e32b34f0e6debb4006ee99722ac6e765f1472ac27282c13572bd82f6a",
    ),
];
