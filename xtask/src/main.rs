//! Build, boot and test driver: `cargo xtask <command>`.
//!
//! Host requirements: `git`, `xorriso`, `qemu-system-x86_64` and an OVMF
//! (UEFI) firmware image. Set `OVMF_CODE` / `OVMF_VARS` to override the search.

use std::env;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::thread;
use std::time::{Duration, Instant};

mod alloc_audit;
mod clockcheck;
mod feed;
mod lint;
mod lx_expect;
mod stub_audit;
mod telemetry;
mod vmtime;
mod wpt;
// The manifest's format and SHA-256 are the kernel's own code (one
// definition for the writer and the reader).
#[path = "../../kernel/src/trust/manifest.rs"]
#[allow(dead_code)]
mod manifest;
mod otel;
mod perfetto;
mod profile;
mod report;
#[path = "../../kernel/src/trust/sha256.rs"]
mod sha256;
mod triage;
mod trust;

/// Pinned Limine release (prebuilt binary branch tag) and its expected commit.
const LIMINE_TAG: &str = "v11.4.1-binary";
const LIMINE_COMMIT: &str = "5be26a73d7b7b4d4477d18be94e1d16e615adf56";
const LIMINE_REPO: &str = "https://github.com/limine-bootloader/limine.git";

/// Unmodified prebuilt Windows programs that scenarios run, fetched on
/// first use into target/prebuilt/<name>/ and checked against their
/// SHA-256 (they are not kept in the repository). THIRD_PARTY.md lists
/// them.
struct Prebuilt {
    name: &'static str,
    url: &'static str,
    /// The release's SHA-256, or its SHA-512 (128 hex digits).
    checksum: &'static str,
    /// The files used, inside the zip. A release that is one file, not an
    /// archive, names just that file (the URL's last component): the
    /// download is moved there. For an MSI (a `.msi` URL, unpacked
    /// with msitools' msiextract: gala has no Windows Installer), the
    /// directories used, each moved to the release's directory under its
    /// last component (`Program Files/X/Binn` becomes `Binn`).
    members: &'static [&'static str],
    /// Files left out of those (unzip -x patterns; for an MSI, paths
    /// removed after the moves, such as `Binn/de`).
    excludes: &'static [&'static str],
}

const PREBUILT: &[Prebuilt] = &[
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
    Prebuilt {
        name: "busybox-linux",
        url: "https://busybox.net/downloads/binaries/1.35.0-x86_64-linux-musl/busybox",
        checksum: "6e123e7f3202a8c1e9b1f94d8941580a25135382b99e8d3e34fb858bba311348",
        members: &["busybox"],
        excludes: &[],
    },
    // glibc 2.39 as Ubuntu 24.04 (noble) ships it: the libc6 package of
    // the release pocket (2.39-0ubuntu8, never changed once released;
    // the SHA-256 is the one we measured, Ubuntu signs the archive's
    // index, not the .deb: THIRD_PARTY.md). A Debian package (an ar
    // archive) unpacked with dpkg-deb; its members moved here by their
    // file names. The program interpreter, libc and libm that dynamically
    // linked Linux programs (user/lx's GLIBC_TESTS) load (ROADMAP LD);
    // they run only inside gala.
    Prebuilt {
        name: "glibc",
        url: "https://archive.ubuntu.com/ubuntu/pool/main/g/glibc/libc6_2.39-0ubuntu8_amd64.deb",
        checksum: "af36c7ac770770fe3d3c10e85d6bc538e76e57570ba7db7d397fb9f654783ef3",
        members: &[
            "usr/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2",
            "usr/lib/x86_64-linux-gnu/libc.so.6",
            "usr/lib/x86_64-linux-gnu/libm.so.6",
        ],
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
];

const KERNEL_TARGET: &str = "x86_64-unknown-none";
/// Freestanding Windows test programs in `user/`, shipped as boot modules.
const USER_TARGET: &str = "x86_64-pc-windows-msvc";
/// 32-bit (x86) test programs, run through WOW64 (ARCHITECTURE "Plan:
/// WOW64"): USER_PROGRAMS_X86.
const USER_TARGET_X86: &str = "i686-pc-windows-msvc";
/// A test DLL: its package, the file it builds, and its name in user/bin.
struct Dll {
    package: &'static str,
    file: &'static str,
    exported_as: &'static str,
}

/// Test DLLs (embedded by test programs or shipped to scenarios; also
/// exported to user/bin).
const USER_DLLS: &[Dll] = &[
    Dll { package: "testdll", file: "testdll.dll", exported_as: "testdll.dll" },
    Dll { package: "testdll2", file: "testdll2.dll", exported_as: "testdll2.dll" },
    Dll { package: "tlsdll", file: "tlsdll.dll", exported_as: "tlsdll.dll" },
    // gala's kernel32.dll, exported under another name like ntdll.
    Dll { package: "kernel32", file: "kernel32.dll", exported_as: "testkernel32.dll" },
    Dll {
        package: "bcryptprimitives",
        file: "bcryptprimitives.dll",
        exported_as: "testbcryptprimitives.dll",
    },
    // gala's ws2_32.dll (Winsock on AFD).
    Dll { package: "ws2_32", file: "ws2_32.dll", exported_as: "testws2_32.dll" },
    // gala's msvcrt.dll (the C runtime of mingw-w64 programs), advapi32.dll
    // and user32.dll (only what Nginx imports).
    Dll { package: "msvcrt", file: "msvcrt.dll", exported_as: "testmsvcrt.dll" },
    Dll { package: "advapi32", file: "advapi32.dll", exported_as: "testadvapi32.dll" },
    Dll { package: "user32", file: "user32.dll", exported_as: "testuser32.dll" },
    // gala's winmm.dll (timer resolution; Go and the JVM load it).
    Dll { package: "winmm", file: "winmm.dll", exported_as: "testwinmm.dll" },
    // comctl32, powrprof, shell32, version: what the JVM imports from them.
    Dll { package: "comctl32", file: "comctl32.dll", exported_as: "testcomctl32.dll" },
    Dll { package: "powrprof", file: "powrprof.dll", exported_as: "testpowrprof.dll" },
    Dll { package: "ole32", file: "ole32.dll", exported_as: "testole32.dll" },
    Dll { package: "bcrypt", file: "bcrypt.dll", exported_as: "testbcrypt.dll" },
    Dll { package: "oleaut32", file: "oleaut32.dll", exported_as: "testoleaut32.dll" },
    Dll { package: "shell32", file: "shell32.dll", exported_as: "testshell32.dll" },
    Dll { package: "version", file: "version.dll", exported_as: "testversion.dll" },
    // psapi.dll: forwarders to kernel32's K32* (JNA's jnidispatch.dll).
    Dll { package: "psapi", file: "psapi.dll", exported_as: "testpsapi.dll" },
    // iphlpapi, secur32, winhttp: what the JDK's net.dll imports.
    Dll { package: "iphlpapi", file: "iphlpapi.dll", exported_as: "testiphlpapi.dll" },
    Dll { package: "secur32", file: "secur32.dll", exported_as: "testsecur32.dll" },
    // gala's schannel.dll: the Schannel (TLS) package behind secur32.
    Dll { package: "schannel", file: "schannel.dll", exported_as: "testschannel.dll" },
    Dll { package: "winhttp", file: "winhttp.dll", exported_as: "testwinhttp.dll" },
    // gala's ncrypt.dll (no key storage providers; bound at startup).
    Dll { package: "ncrypt", file: "ncrypt.dll", exported_as: "testncrypt.dll" },
    // gala's crypt32.dll (certificate stores; only what OpenSSL imports).
    Dll { package: "crypt32", file: "crypt32.dll", exported_as: "testcrypt32.dll" },
    // gala's mswsock.dll (forwarders to ws2_32).
    Dll { package: "mswsock", file: "mswsock.dll", exported_as: "testmswsock.dll" },
    // What SQL Server LocalDB imports from DLLs gala had no other use for
    // (functions not implemented yet: gala_stubs).
    Dll { package: "netapi32", file: "netapi32.dll", exported_as: "testnetapi32.dll" },
    Dll { package: "pdh", file: "pdh.dll", exported_as: "testpdh.dll" },
    Dll { package: "setupapi", file: "setupapi.dll", exported_as: "testsetupapi.dll" },
    Dll { package: "shlwapi", file: "shlwapi.dll", exported_as: "testshlwapi.dll" },
    Dll { package: "rpcrt4", file: "rpcrt4.dll", exported_as: "testrpcrt4.dll" },
    Dll { package: "urlmon", file: "urlmon.dll", exported_as: "testurlmon.dll" },
    Dll { package: "wininet", file: "wininet.dll", exported_as: "testwininet.dll" },
    Dll { package: "userenv", file: "userenv.dll", exported_as: "testuserenv.dll" },
    Dll { package: "xolehlp", file: "xolehlp.dll", exported_as: "testxolehlp.dll" },
    Dll { package: "uxtheme", file: "uxtheme.dll", exported_as: "testuxtheme.dll" },
    Dll { package: "comdlg32", file: "comdlg32.dll", exported_as: "testcomdlg32.dll" },
    Dll { package: "oleacc", file: "oleacc.dll", exported_as: "testoleacc.dll" },
    // gala's UIAutomationCore.dll (UI Automation's client over HWNDs).
    Dll {
        package: "uiautomationcore",
        file: "uiautomationcore.dll",
        exported_as: "testuiautomationcore.dll",
    },
    Dll { package: "mpr", file: "mpr.dll", exported_as: "testmpr.dll" },
    Dll { package: "msimg32", file: "msimg32.dll", exported_as: "testmsimg32.dll" },
    // winspool.drv: no print spooler, no printers (Delphi's VCL imports it:
    // Inno Setup's Setup, PK).
    Dll { package: "winspool", file: "winspool.drv", exported_as: "testwinspool.drv" },
    // DWrite.dll: chrome.dll imports it (BR).
    Dll { package: "dwrite", file: "dwrite.dll", exported_as: "testdwrite.dll" },
    // shcore.dll: the DPI scaling API set (BR).
    Dll { package: "shcore", file: "shcore.dll", exported_as: "testshcore.dll" },
    // dwmapi, wtsapi32, imm32: what chrome.dll asks of them (BR).
    Dll { package: "dwmapi", file: "dwmapi.dll", exported_as: "testdwmapi.dll" },
    // cfgmgr32: the device-tree queries of the Khronos Vulkan loader (GPU
    // G2).
    Dll { package: "cfgmgr32", file: "cfgmgr32.dll", exported_as: "testcfgmgr32.dll" },
    Dll { package: "wtsapi32", file: "wtsapi32.dll", exported_as: "testwtsapi32.dll" },
    Dll { package: "imm32", file: "imm32.dll", exported_as: "testimm32.dll" },
    // msctf.dll: the input scope functions (Delphi's VCL; Inno Setup).
    Dll { package: "msctf", file: "msctf.dll", exported_as: "testmsctf.dll" },
    // The GPU stubs (ROADMAP GPU G0): no Direct3D adapter yet.
    Dll { package: "dxgi", file: "dxgi.dll", exported_as: "testdxgi.dll" },
    Dll { package: "d3d11", file: "d3d11.dll", exported_as: "testd3d11.dll" },
    Dll { package: "d3d12", file: "d3d12.dll", exported_as: "testd3d12.dll" },
    Dll { package: "dcomp", file: "dcomp.dll", exported_as: "testdcomp.dll" },
    // Rich edit controls: one crate (`richedit`) behind both DLLs.
    Dll { package: "riched20", file: "riched20.dll", exported_as: "testriched20.dll" },
    Dll { package: "msftedit", file: "msftedit.dll", exported_as: "testmsftedit.dll" },
    Dll { package: "odbc32", file: "odbc32.dll", exported_as: "testodbc32.dll" },
    Dll { package: "wldap32", file: "wldap32.dll", exported_as: "testwldap32.dll" },
    Dll { package: "wsock32", file: "wsock32.dll", exported_as: "testwsock32.dll" },
    // normaliz.dll: forwarders to kernel32's IDN functions (libcurl).
    Dll { package: "normaliz", file: "normaliz.dll", exported_as: "testnormaliz.dll" },
    Dll { package: "shfolder", file: "shfolder.dll", exported_as: "testshfolder.dll" },
    Dll { package: "kernelbase", file: "kernelbase.dll", exported_as: "testkernelbase.dll" },
    Dll { package: "gdi32", file: "gdi32.dll", exported_as: "testgdi32.dll" },
    Dll { package: "authz", file: "authz.dll", exported_as: "testauthz.dll" },
    Dll { package: "wintrust", file: "wintrust.dll", exported_as: "testwintrust.dll" },
    Dll { package: "webservices", file: "webservices.dll", exported_as: "testwebservices.dll" },
    Dll { package: "dhcpcsvc", file: "dhcpcsvc.dll", exported_as: "testdhcpcsvc.dll" },
    Dll { package: "dbghelp", file: "dbghelp.dll", exported_as: "testdbghelp.dll" },
    Dll { package: "xmllite", file: "xmllite.dll", exported_as: "testxmllite.dll" },
    // gala's clusapi.dll: no failover cluster on this machine.
    Dll { package: "clusapi", file: "clusapi.dll", exported_as: "testclusapi.dll" },
    // security.dll: forwarders to secur32 (SSPI's older name).
    Dll { package: "security", file: "security.dll", exported_as: "testsecurity.dll" },
    Dll { package: "resutils", file: "resutils.dll", exported_as: "testresutils.dll" },
    // gala's wow64.dll: the 64-bit side of every 32-bit process (WOW64's
    // system call thunks, its thread start; ARCHITECTURE "Plan: WOW64").
    Dll { package: "wow64", file: "wow64.dll", exported_as: "testwow64.dll" },
    // Built as ntdll.dll (its export directory says so); exported under
    // another name so user/bin never holds a file called ntdll.dll.
    Dll { package: "testntdll", file: "ntdll.dll", exported_as: "testntdll.dll" },
];

/// gala's 32-bit (x86) DLLs, built for USER_TARGET_X86 and shipped with
/// an `x86/` prefix in EXTRA_MODULES (`("<scenario>", "x86/ntdll.dll",
/// "Windows/SysWOW64")`): the 32-bit ntdll goes in SysWOW64, as on 64-bit
/// Windows (ARCHITECTURE "Plan: WOW64"). Exported to user/bin under their
/// own names (`testntdll32.dll`).
const USER_DLLS_X86: &[Dll] = &[
    // The 32-bit ntdll: testntdll built for i686.
    Dll { package: "testntdll", file: "ntdll.dll", exported_as: "testntdll32.dll" },
    // A 32-bit test DLL (wow64_import_test).
    Dll { package: "testdll32", file: "testdll32.dll", exported_as: "testdll32.dll" },
    // The 32-bit kernel32: gala's kernel32 built for i686, and the
    // kernelbase beside it (the core API sets' host).
    Dll { package: "kernel32", file: "kernel32.dll", exported_as: "testkernel32_32.dll" },
    Dll { package: "kernelbase", file: "kernelbase.dll", exported_as: "testkernelbase32.dll" },
    // The 32-bit msvcrt: gala's msvcrt built for i686.
    Dll { package: "msvcrt", file: "msvcrt.dll", exported_as: "testmsvcrt32.dll" },
    // The 32-bit user32 and gdi32 (WOW64 stage 4; user32 loads gdi32).
    Dll { package: "user32", file: "user32.dll", exported_as: "testuser32_32.dll" },
    Dll { package: "gdi32", file: "gdi32.dll", exported_as: "testgdi32_32.dll" },
    // The 32-bit advapi32 and the bcryptprimitives it imports.
    Dll { package: "advapi32", file: "advapi32.dll", exported_as: "testadvapi32_32.dll" },
    Dll {
        package: "bcryptprimitives",
        file: "bcryptprimitives.dll",
        exported_as: "testbcryptprimitives32.dll",
    },
    // What an x86 Inno Setup installer (Delphi's VCL) loads (WOW64 stage 4).
    Dll { package: "comctl32", file: "comctl32.dll", exported_as: "testcomctl3232.dll" },
    Dll { package: "oleaut32", file: "oleaut32.dll", exported_as: "testoleaut3232.dll" },
    Dll { package: "ole32", file: "ole32.dll", exported_as: "testole3232.dll" },
    Dll { package: "shell32", file: "shell32.dll", exported_as: "testshell3232.dll" },
    Dll { package: "shlwapi", file: "shlwapi.dll", exported_as: "testshlwapi32.dll" },
    Dll { package: "version", file: "version.dll", exported_as: "testversion32.dll" },
    Dll { package: "mpr", file: "mpr.dll", exported_as: "testmpr32.dll" },
    Dll { package: "comdlg32", file: "comdlg32.dll", exported_as: "testcomdlg3232.dll" },
    Dll { package: "uxtheme", file: "uxtheme.dll", exported_as: "testuxtheme32.dll" },
    Dll { package: "imm32", file: "imm32.dll", exported_as: "testimm3232.dll" },
    Dll { package: "dwmapi", file: "dwmapi.dll", exported_as: "testdwmapi32.dll" },
    Dll { package: "msimg32", file: "msimg32.dll", exported_as: "testmsimg3232.dll" },
    Dll { package: "msctf", file: "msctf.dll", exported_as: "testmsctf32.dll" },
    Dll { package: "winspool", file: "winspool.drv", exported_as: "testwinspool32.drv" },
    Dll { package: "wtsapi32", file: "wtsapi32.dll", exported_as: "testwtsapi3232.dll" },
    // winhttp (Inno Setup's downloads) and what it loads.
    Dll { package: "winhttp", file: "winhttp.dll", exported_as: "testwinhttp32.dll" },
    Dll { package: "ws2_32", file: "ws2_32.dll", exported_as: "testws2_32_32.dll" },
    Dll { package: "secur32", file: "secur32.dll", exported_as: "testsecur3232.dll" },
    Dll { package: "shfolder", file: "shfolder.dll", exported_as: "testshfolder32.dll" },
    Dll { package: "bcrypt", file: "bcrypt.dll", exported_as: "testbcrypt32.dll" },
];

/// An EXTRA_MODULES file with the `x86/` prefix: a USER_DLLS_X86 DLL
/// (its file name), else None.
fn x86_dll(file: &str) -> Option<&'static Dll> {
    let name = file.strip_prefix("x86/")?;
    USER_DLLS_X86.iter().find(|d| d.file == name)
}

/// Where the build puts the USER_DLLS_X86 file `file`.
fn built_dll_x86(file: &str) -> PathBuf {
    let release = target_dir().join(USER_TARGET_X86).join("release");
    match USER_DLLS_X86.iter().find(|d| d.file == file) {
        // As built_dll: cargo names it `<package>.dll`.
        Some(d) if !d.file.ends_with(".dll") => release.join(format!("{}.dll", d.package)),
        _ => release.join(file),
    }
}

/// Where the build puts the USER_DLLS file `file`: cargo names a cdylib
/// `<package>.dll`, so a DLL Windows calls otherwise (winspool.drv) is
/// built under its package's name and shipped under its own.
fn built_dll(file: &str) -> PathBuf {
    let release = target_dir().join(USER_TARGET).join("release");
    match USER_DLLS.iter().find(|d| d.file == file) {
        Some(d) if !d.file.ends_with(".dll") => release.join(format!("{}.dll", d.package)),
        _ => release.join(file),
    }
}

/// Directory trees shipped writable (`tarw:`): data a program changes, as
/// an installer leaves it (SQL Server's databases).
const WRITABLE_TREES: &[&str] = &["sql/data", "fzs", "ftp"];

/// Extra boot modules per scenario: (scenario, built DLL file or repository
/// path, directory on C: that the kernel puts it in, passed as the module
/// string).
const EXTRA_MODULES: &[(&str, &str, &str)] = &[
    ("startup_test", "ntdll.dll", "Windows/System32"),
    ("exception_test", "ntdll.dll", "Windows/System32"),
    ("seh_test", "ntdll.dll", "Windows/System32"),
    ("guard_test", "ntdll.dll", "Windows/System32"),
    ("import_test", "ntdll.dll", "Windows/System32"),
    ("import_test", "testdll.dll", "gala"),
    ("fpu_test", "ntdll.dll", "Windows/System32"),
    ("apc_test", "ntdll.dll", "Windows/System32"),
    ("ioapc_test", "ntdll.dll", "Windows/System32"),
    ("sync_test", "ntdll.dll", "Windows/System32"),
    ("symlink_test", "ntdll.dll", "Windows/System32"),
    ("iocp_test", "ntdll.dll", "Windows/System32"),
    ("context_test", "ntdll.dll", "Windows/System32"),
    ("timer_test", "ntdll.dll", "Windows/System32"),
    ("loader_test", "ntdll.dll", "Windows/System32"),
    ("loader_test", "testdll.dll", "gala"),
    ("loader_test", "testdll2.dll", "gala"),
    ("smpsched_test", "ntdll.dll", "Windows/System32"),
    ("smpmem_test", "ntdll.dll", "Windows/System32"),
    ("smpcow_test", "ntdll.dll", "Windows/System32"),
    ("vmcycle_test", "ntdll.dll", "Windows/System32"),
    ("xmodify_test", "ntdll.dll", "Windows/System32"),
    ("smpsem_test", "ntdll.dll", "Windows/System32"),
    ("smprace_test", "ntdll.dll", "Windows/System32"),
    ("smprace_test", "kernel32.dll", "Windows/System32"),
    ("xstate_test", "ntdll.dll", "Windows/System32"),
    ("straddle_test", "ntdll.dll", "Windows/System32"),
    ("statedump_test", "ntdll.dll", "Windows/System32"),
    ("crash_dispatched", "ntdll.dll", "Windows/System32"),
    ("crash_filtered", "ntdll.dll", "Windows/System32"),
    ("safepoint_test", "ntdll.dll", "Windows/System32"),
    ("shootdown_test", "ntdll.dll", "Windows/System32"),
    ("events", "ntdll.dll", "Windows/System32"),
    ("events", "testdll.dll", "gala"),
    ("kernel32_test", "ntdll.dll", "Windows/System32"),
    ("kernel32_test", "kernel32.dll", "Windows/System32"),
    ("kernel32_test", "testdll.dll", "gala"),
    ("kernel32_test", "testdll2.dll", "gala/lib"),
    ("missing_import_test", "ntdll.dll", "Windows/System32"),
    ("missing_import_test", "kernel32.dll", "Windows/System32"),
    ("dpapi_test", "ntdll.dll", "Windows/System32"),
    ("dpapi_test", "kernel32.dll", "Windows/System32"),
    ("dpapi_test", "advapi32.dll", "Windows/System32"),
    ("dpapi_test", "crypt32.dll", "Windows/System32"),
    ("dpapi_test", "bcrypt.dll", "Windows/System32"),
    ("dpapi_test", "bcryptprimitives.dll", "Windows/System32"),
    ("cert_test", "ntdll.dll", "Windows/System32"),
    ("cert_test", "kernel32.dll", "Windows/System32"),
    ("cert_test", "advapi32.dll", "Windows/System32"),
    ("cert_test", "crypt32.dll", "Windows/System32"),
    ("cert_test", "user/schannel_test/certs/ca.cer", "Windows/System32/gala/roots"),
    ("cert_test", "bcrypt.dll", "Windows/System32"),
    ("cert_test", "bcryptprimitives.dll", "Windows/System32"),
    ("cert_test", "ncrypt.dll", "Windows/System32"),
    ("sspi_test", "ntdll.dll", "Windows/System32"),
    ("sspi_test", "kernel32.dll", "Windows/System32"),
    ("sspi_test", "secur32.dll", "Windows/System32"),
    ("sspi_test", "bcryptprimitives.dll", "Windows/System32"),
    ("sspi_test", "advapi32.dll", "Windows/System32"),
    ("sspi_test", "security.dll", "Windows/System32"),
    ("clusapi_test", "ntdll.dll", "Windows/System32"),
    ("clusapi_test", "kernel32.dll", "Windows/System32"),
    ("clusapi_test", "clusapi.dll", "Windows/System32"),
    ("clusapi_test", "resutils.dll", "Windows/System32"),
    ("netapi32_test", "ntdll.dll", "Windows/System32"),
    ("netapi32_test", "kernel32.dll", "Windows/System32"),
    ("netapi32_test", "advapi32.dll", "Windows/System32"),
    ("netapi32_test", "netapi32.dll", "Windows/System32"),
    ("netapi32_test", "bcryptprimitives.dll", "Windows/System32"),
    ("netapi32_test", "bcrypt.dll", "Windows/System32"),
    ("netapi32_test", "secur32.dll", "Windows/System32"),
    ("pdh_test", "ntdll.dll", "Windows/System32"),
    ("pdh_test", "kernel32.dll", "Windows/System32"),
    ("pdh_test", "pdh.dll", "Windows/System32"),
    ("setupapi_test", "ntdll.dll", "Windows/System32"),
    ("setupapi_test", "kernel32.dll", "Windows/System32"),
    ("setupapi_test", "setupapi.dll", "Windows/System32"),
    ("rpcrt4_test", "ntdll.dll", "Windows/System32"),
    ("rpcrt4_test", "kernel32.dll", "Windows/System32"),
    ("rpcrt4_test", "rpcrt4.dll", "Windows/System32"),
    ("rpcrt4_test", "bcryptprimitives.dll", "Windows/System32"),
    ("unimplemented_test", "ntdll.dll", "Windows/System32"),
    ("unimplemented_test", "kernel32.dll", "Windows/System32"),
    ("unimplemented_test", "xolehlp.dll", "Windows/System32"),
    ("kernel32_sys_test", "ntdll.dll", "Windows/System32"),
    ("kernel32_sys_test", "kernel32.dll", "Windows/System32"),
    ("kernel32_sys_test", "testdll.dll", "gala"),
    ("kernel32_thread_test", "ntdll.dll", "Windows/System32"),
    ("handover_k32_test", "ntdll.dll", "Windows/System32"),
    ("handover_k32_test", "kernel32.dll", "Windows/System32"),
    ("kernel32_sync_test", "ntdll.dll", "Windows/System32"),
    ("kernel32_py_test", "ntdll.dll", "Windows/System32"),
    ("kernel32_locale_test", "ntdll.dll", "Windows/System32"),
    ("gui_test", "kernel/assets/wallpapers/dunes.qoi", "Windows/Web/Wallpaper"),
    ("gui_test", "kernel/assets/wallpapers/ribbons.qoi", "Windows/Web/Wallpaper"),
    ("gui_test", "kernel/assets/wallpapers/najdi.qoi", "Windows/Web/Wallpaper"),
    ("gui_test", "kernel/assets/wallpapers/night.qoi", "Windows/Web/Wallpaper"),
    ("gui_test", "ntdll.dll", "Windows/System32"),
    ("gui_test", "kernel32.dll", "Windows/System32"),
    ("gui_test", "user32.dll", "Windows/System32"),
    ("gui_test", "gdi32.dll", "Windows/System32"),
    ("gui_test", "comctl32.dll", "Windows/System32"),
    ("console_window", "kernel/assets/wallpapers/night.qoi", "Windows/Web/Wallpaper"),
    ("sshd_server", "user/sshd/logs-placeholder.txt", "ProgramData/ssh/logs"),
    ("sshd_sftp", "target/prebuilt/openssh/OpenSSH-Win64/sftp-server.exe", "Windows/System32"),
    // Legacy scp (`scp -O`) runs `scp -t`/`scp -f` through the users'
    // shell, so scp.exe must be on its PATH, like sftp-server.exe.
    ("sshd_scp", "target/prebuilt/openssh/OpenSSH-Win64/scp.exe", "Windows/System32"),
    ("sshd_scp", "shlwapi.dll", "Windows/System32"),
    // galatest's private key in its profile, for gala's own ssh.exe.
    ("sshd_client", "user/sshd/client_ed25519", "Users/galatest/.ssh"),
    ("sshd_shell", "user/bin/conhost.exe", "Windows/System32"),
    ("sshd_shell", "gdi32.dll", "Windows/System32"),
    ("conpty_test", "ntdll.dll", "Windows/System32"),
    ("conpty_test", "kernel32.dll", "Windows/System32"),
    ("conpty_test", "user32.dll", "Windows/System32"),
    ("conpty_test", "gdi32.dll", "Windows/System32"),
    ("conpty_test", "user/bin/conhost.exe", "Windows/System32"),
    ("console_window", "ntdll.dll", "Windows/System32"),
    ("console_window", "kernel32.dll", "Windows/System32"),
    ("console_window", "user32.dll", "Windows/System32"),
    ("console_window", "gdi32.dll", "Windows/System32"),
    ("rtl_test", "ntdll.dll", "Windows/System32"),
    ("rtl_test", "kernel32.dll", "Windows/System32"),
    ("disk_test", "ntdll.dll", "Windows/System32"),
    ("disk_test", "kernel32.dll", "Windows/System32"),
    ("disk_test", "advapi32.dll", "Windows/System32"),
    ("disk_test", "secur32.dll", "Windows/System32"),
    ("disk_test", "bcryptprimitives.dll", "Windows/System32"),
    ("disk_test", "bcrypt.dll", "Windows/System32"),
    ("disk_test", "user/access_test/access_test.regseed", "registry"),
    ("persist_test", "ntdll.dll", "Windows/System32"),
    ("persist_test", "kernel32.dll", "Windows/System32"),
    ("persist_test", "advapi32.dll", "Windows/System32"),
    ("persist_test", "secur32.dll", "Windows/System32"),
    ("persist_test", "bcryptprimitives.dll", "Windows/System32"),
    ("persist_test", "bcrypt.dll", "Windows/System32"),
    ("kernel32_system_test", "ntdll.dll", "Windows/System32"),
    ("kernel32_clr_test", "ntdll.dll", "Windows/System32"),
    ("com_test", "ntdll.dll", "Windows/System32"),
    ("bcrypt_test", "ntdll.dll", "Windows/System32"),
    ("console_test", "ntdll.dll", "Windows/System32"),
    ("console_test", "kernel32.dll", "Windows/System32"),
    ("bcrypt_test", "kernel32.dll", "Windows/System32"),
    ("bcrypt_test", "bcrypt.dll", "Windows/System32"),
    ("bcrypt_test", "bcryptprimitives.dll", "Windows/System32"),
    ("com_test", "kernel32.dll", "Windows/System32"),
    ("com_test", "ole32.dll", "Windows/System32"),
    ("com_test", "oleaut32.dll", "Windows/System32"),
    ("com_test", "advapi32.dll", "Windows/System32"),
    ("com_test", "bcryptprimitives.dll", "Windows/System32"),
    ("kernel32_clr_test", "kernel32.dll", "Windows/System32"),
    ("kernel32_clr_test", "testdll.dll", "gala"),
    ("kernel32_clr_test", "user/bin/dotnet_hello.dll", "gala"),
    ("kernel32_clr_test", "user/kernel32_clr_test/x86_native.dll", "gala"),
    ("win32_misc_test", "ntdll.dll", "Windows/System32"),
    ("win32_misc_test", "kernel32.dll", "Windows/System32"),
    ("win32_misc_test", "advapi32.dll", "Windows/System32"),
    ("win32_misc_test", "user32.dll", "Windows/System32"),
    ("win32_misc_test", "gdi32.dll", "Windows/System32"),
    ("win32_misc_test", "ws2_32.dll", "Windows/System32"),
    ("win32_misc_test", "mswsock.dll", "Windows/System32"),
    ("win32_misc_test", "comctl32.dll", "Windows/System32"),
    ("win32_misc_test", "powrprof.dll", "Windows/System32"),
    ("win32_misc_test", "shlwapi.dll", "Windows/System32"),
    ("win32_misc_test", "secur32.dll", "Windows/System32"),
    ("win32_misc_test", "shell32.dll", "Windows/System32"),
    ("win32_misc_test", "shfolder.dll", "Windows/System32"),
    ("win32_misc_test", "winspool.drv", "Windows/System32"),
    ("win32_misc_test", "ole32.dll", "Windows/System32"),
    ("win32_misc_test", "uxtheme.dll", "Windows/System32"),
    ("win32_misc_test", "version.dll", "Windows/System32"),
    ("win32_misc_test", "iphlpapi.dll", "Windows/System32"),
    ("win32_misc_test", "bcryptprimitives.dll", "Windows/System32"),
    // Its 32-bit child (IsWow64Process2), created suspended.
    ("win32_misc_test", "wow64.dll", "Windows/System32"),
    ("kernel32_pool_test", "ntdll.dll", "Windows/System32"),
    ("kernel32_pool_test", "kernel32.dll", "Windows/System32"),
    ("kernel32_pool_test", "winmm.dll", "Windows/System32"),
    ("manifest_test", "ntdll.dll", "Windows/System32"),
    ("manifest_test", "kernel32.dll", "Windows/System32"),
    ("manifest_test", "user32.dll", "Windows/System32"),
    ("manifest_test", "gdi32.dll", "Windows/System32"),
    ("manifest_test", "comctl32.dll", "Windows/System32"),
    ("tls_test", "ntdll.dll", "Windows/System32"),
    ("tls_test", "kernel32.dll", "Windows/System32"),
    ("tls_test", "tlsdll.dll", "gala"),
    ("dirchange_test", "ntdll.dll", "Windows/System32"),
    ("dirchange_test", "kernel32.dll", "Windows/System32"),
    ("afd_poll_test", "ntdll.dll", "Windows/System32"),
    ("afd_poll_test", "kernel32.dll", "Windows/System32"),
    ("afd_poll_test", "ws2_32.dll", "Windows/System32"),
    ("iphlpapi_test", "ntdll.dll", "Windows/System32"),
    ("iphlpapi_test", "kernel32.dll", "Windows/System32"),
    ("iphlpapi_test", "iphlpapi.dll", "Windows/System32"),
    ("keyed_event_test", "ntdll.dll", "Windows/System32"),
    ("keyed_event_test", "kernel32.dll", "Windows/System32"),
    ("registry_test", "ntdll.dll", "Windows/System32"),
    ("registry_test", "kernel32.dll", "Windows/System32"),
    ("registry_test", "advapi32.dll", "Windows/System32"),
    ("registry_test", "bcryptprimitives.dll", "Windows/System32"),
    ("security_test", "ntdll.dll", "Windows/System32"),
    ("security_test", "kernel32.dll", "Windows/System32"),
    ("security_test", "advapi32.dll", "Windows/System32"),
    ("security_test", "secur32.dll", "Windows/System32"),
    ("security_test", "bcryptprimitives.dll", "Windows/System32"),
    ("security_test", "bcrypt.dll", "Windows/System32"),
    ("access_test", "ntdll.dll", "Windows/System32"),
    ("access_test", "kernel32.dll", "Windows/System32"),
    ("access_test", "advapi32.dll", "Windows/System32"),
    ("access_test", "secur32.dll", "Windows/System32"),
    ("access_test", "bcryptprimitives.dll", "Windows/System32"),
    ("access_test", "bcrypt.dll", "Windows/System32"),
    ("access_test", "user/access_test/access_test.regseed", "registry"),
    ("logon_test", "ntdll.dll", "Windows/System32"),
    ("logon_test", "kernel32.dll", "Windows/System32"),
    ("logon_test", "advapi32.dll", "Windows/System32"),
    ("logon_test", "secur32.dll", "Windows/System32"),
    ("logon_test", "bcryptprimitives.dll", "Windows/System32"),
    ("logon_test", "bcrypt.dll", "Windows/System32"),
    ("logon_test", "userenv.dll", "Windows/System32"),
    ("logon_test", "user/access_test/access_test.regseed", "registry"),
    ("system_test", "ntdll.dll", "Windows/System32"),
    ("system_test", "kernel32.dll", "Windows/System32"),
    ("system_test", "advapi32.dll", "Windows/System32"),
    ("system_test", "secur32.dll", "Windows/System32"),
    ("system_test", "bcryptprimitives.dll", "Windows/System32"),
    ("system_test", "bcrypt.dll", "Windows/System32"),
    ("system_test", "user/access_test/access_test.regseed", "registry"),
    ("session_test", "ntdll.dll", "Windows/System32"),
    ("session_test", "kernel32.dll", "Windows/System32"),
    ("session_test", "advapi32.dll", "Windows/System32"),
    ("session_test", "secur32.dll", "Windows/System32"),
    ("session_test", "bcryptprimitives.dll", "Windows/System32"),
    ("session_test", "bcrypt.dll", "Windows/System32"),
    ("session_test", "user32.dll", "Windows/System32"),
    ("session_test", "gdi32.dll", "Windows/System32"),
    ("session_test", "user/access_test/access_test.regseed", "registry"),
    ("presenter_test", "kernel/assets/wallpapers/night.qoi", "Windows/Web/Wallpaper"),
    ("presenter_test", "ntdll.dll", "Windows/System32"),
    ("presenter_test", "kernel32.dll", "Windows/System32"),
    ("presenter_test", "advapi32.dll", "Windows/System32"),
    ("presenter_test", "secur32.dll", "Windows/System32"),
    ("presenter_test", "bcryptprimitives.dll", "Windows/System32"),
    ("presenter_test", "bcrypt.dll", "Windows/System32"),
    ("presenter_test", "user32.dll", "Windows/System32"),
    ("presenter_test", "gdi32.dll", "Windows/System32"),
    ("winsta_test", "ntdll.dll", "Windows/System32"),
    ("winsta_test", "kernel32.dll", "Windows/System32"),
    ("winsta_test", "user32.dll", "Windows/System32"),
    ("lsa_test", "ntdll.dll", "Windows/System32"),
    ("lsa_test", "kernel32.dll", "Windows/System32"),
    ("lsa_test", "advapi32.dll", "Windows/System32"),
    ("lsa_test", "secur32.dll", "Windows/System32"),
    ("lsa_test", "bcryptprimitives.dll", "Windows/System32"),
    ("lsa_test", "bcrypt.dll", "Windows/System32"),
    ("lsa_test", "user/access_test/access_test.regseed", "registry"),
    ("restricted_test", "ntdll.dll", "Windows/System32"),
    ("restricted_test", "kernel32.dll", "Windows/System32"),
    ("restricted_test", "advapi32.dll", "Windows/System32"),
    ("restricted_test", "bcryptprimitives.dll", "Windows/System32"),
    ("restricted_test", "bcrypt.dll", "Windows/System32"),
    ("cmd_test", "ntdll.dll", "Windows/System32"),
    ("cmd_test", "kernel32.dll", "Windows/System32"),
    ("cmd_test", "bcryptprimitives.dll", "Windows/System32"),
    ("cmd_test", "user/bin/cmd.exe", "Windows/System32"),
    ("profile_test", "ntdll.dll", "Windows/System32"),
    ("profile_test", "kernel32.dll", "Windows/System32"),
    ("profile_test", "advapi32.dll", "Windows/System32"),
    ("profile_test", "secur32.dll", "Windows/System32"),
    ("profile_test", "bcryptprimitives.dll", "Windows/System32"),
    ("profile_test", "bcrypt.dll", "Windows/System32"),
    ("profile_test", "userenv.dll", "Windows/System32"),
    ("profile_test", "user/access_test/access_test.regseed", "registry"),
    ("profile_test", "user/profile_test/note.txt", "Users/gala/Documents"),
    ("token_test", "ntdll.dll", "Windows/System32"),
    ("token_test", "kernel32.dll", "Windows/System32"),
    ("token_test", "advapi32.dll", "Windows/System32"),
    ("token_test", "secur32.dll", "Windows/System32"),
    ("token_test", "bcryptprimitives.dll", "Windows/System32"),
    ("token_test", "bcrypt.dll", "Windows/System32"),
    ("advapi32_test", "ntdll.dll", "Windows/System32"),
    ("advapi32_test", "kernel32.dll", "Windows/System32"),
    ("advapi32_test", "advapi32.dll", "Windows/System32"),
    ("advapi32_test", "secur32.dll", "Windows/System32"),
    ("advapi32_test", "bcryptprimitives.dll", "Windows/System32"),
    ("advapi32_test", "bcrypt.dll", "Windows/System32"),
    ("kernel32_system_test", "kernel32.dll", "Windows/System32"),
    ("kernel32_system_test", "psapi.dll", "Windows/System32"),
    ("kernel32_locale_test", "kernel32.dll", "Windows/System32"),
    ("kernel32_sync_test", "kernel32.dll", "Windows/System32"),
    ("kernel32_py_test", "kernel32.dll", "Windows/System32"),
    ("kernel32_thread_test", "kernel32.dll", "Windows/System32"),
    ("xstate_test", "kernel32.dll", "Windows/System32"),
    ("kernel32_misc_test", "ntdll.dll", "Windows/System32"),
    ("kernel32_misc_test", "kernel32.dll", "Windows/System32"),
    ("kernel32_misc_test", "winmm.dll", "Windows/System32"),
    ("kernel32_misc_test", "advapi32.dll", "Windows/System32"),
    ("kernel32_misc_test", "ncrypt.dll", "Windows/System32"),
    ("kernel32_misc_test", "bcryptprimitives.dll", "Windows/System32"),
    ("kernel32_file_test", "ntdll.dll", "Windows/System32"),
    ("kernel32_file_test", "kernel32.dll", "Windows/System32"),
    ("pipe_test", "ntdll.dll", "Windows/System32"),
    ("pipe_test", "kernel32.dll", "Windows/System32"),
    ("pipe_test", "advapi32.dll", "Windows/System32"),
    ("pipe_test", "secur32.dll", "Windows/System32"),
    ("pipe_test", "bcryptprimitives.dll", "Windows/System32"),
    ("pipe_test", "bcrypt.dll", "Windows/System32"),
    ("lock_test", "ntdll.dll", "Windows/System32"),
    ("lock_test", "kernel32.dll", "Windows/System32"),
    ("kernel32_env_test", "ntdll.dll", "Windows/System32"),
    ("kernel32_env_test", "kernel32.dll", "Windows/System32"),
    ("prng_test", "ntdll.dll", "Windows/System32"),
    ("prng_test", "kernel32.dll", "Windows/System32"),
    ("prng_test", "bcryptprimitives.dll", "Windows/System32"),
    ("replay_test", "ntdll.dll", "Windows/System32"),
    ("replay_test", "kernel32.dll", "Windows/System32"),
    ("replay_test", "bcryptprimitives.dll", "Windows/System32"),
    ("record_kernel", "ntdll.dll", "Windows/System32"),
    ("record_kernel", "kernel32.dll", "Windows/System32"),
    ("record_kernel", "bcryptprimitives.dll", "Windows/System32"),
    ("replay_kernel", "ntdll.dll", "Windows/System32"),
    ("replay_kernel", "kernel32.dll", "Windows/System32"),
    ("replay_kernel", "bcryptprimitives.dll", "Windows/System32"),
    // Recorded in gala-um (`um --record=...`): its random numbers.
    ("replay_kernel", "user/replay_test/replay_test.rec", "gala"),
    ("winsock_async_test", "ntdll.dll", "Windows/System32"),
    ("winsock_async_test", "kernel32.dll", "Windows/System32"),
    ("winsock_async_test", "ws2_32.dll", "Windows/System32"),
    ("winsock_async_test", "mswsock.dll", "Windows/System32"),
    ("process_test", "ntdll.dll", "Windows/System32"),
    ("process_test", "kernel32.dll", "Windows/System32"),
    ("commit_test", "ntdll.dll", "Windows/System32"),
    ("commit_test", "kernel32.dll", "Windows/System32"),
    ("quota_test", "ntdll.dll", "Windows/System32"),
    ("quota_test", "kernel32.dll", "Windows/System32"),
    ("quota_test", "ws2_32.dll", "Windows/System32"),
    ("gpu_stub_test", "ntdll.dll", "Windows/System32"),
    ("gpu_stub_test", "kernel32.dll", "Windows/System32"),
    ("gpu_stub_test", "dxgi.dll", "Windows/System32"),
    ("gpu_stub_test", "d3d11.dll", "Windows/System32"),
    ("gpu_stub_test", "d3d12.dll", "Windows/System32"),
    ("gpu_stub_test", "dcomp.dll", "Windows/System32"),
    // gl_test (GPU G2): OpenGL through Mesa's llvmpipe (PREBUILT mesa),
    // its opengl32.dll and the gallium megadriver it loads in System32 as
    // the system's OpenGL (ARCHITECTURE "Plan: graphics and GPU"), with
    // what libgallium_wgl.dll imports.
    ("gl_test", "ntdll.dll", "Windows/System32"),
    ("gl_test", "kernel32.dll", "Windows/System32"),
    ("gl_test", "msvcrt.dll", "Windows/System32"),
    ("gl_test", "user32.dll", "Windows/System32"),
    ("gl_test", "gdi32.dll", "Windows/System32"),
    ("gl_test", "advapi32.dll", "Windows/System32"),
    ("gl_test", "bcryptprimitives.dll", "Windows/System32"),
    ("gl_test", "shell32.dll", "Windows/System32"),
    ("gl_test", "shlwapi.dll", "Windows/System32"),
    ("gl_test", "ole32.dll", "Windows/System32"),
    ("gl_test", "version.dll", "Windows/System32"),
    ("gl_test", "target/prebuilt/mesa/x64/opengl32.dll", "Windows/System32"),
    ("gl_test", "target/prebuilt/mesa/x64/libgallium_wgl.dll", "Windows/System32"),
    ("gl_test", "user/bin/gl_test.exe", "gala"),
    // cfgmgr32_test: the device-tree queries the Vulkan loader makes.
    ("cfgmgr32_test", "ntdll.dll", "Windows/System32"),
    ("cfgmgr32_test", "kernel32.dll", "Windows/System32"),
    ("cfgmgr32_test", "msvcrt.dll", "Windows/System32"),
    ("cfgmgr32_test", "cfgmgr32.dll", "Windows/System32"),
    ("cfgmgr32_test", "user/bin/cfgmgr32_test.exe", "gala"),
    // vk_test (GPU G2): Vulkan through the Khronos loader (PREBUILT
    // vulkan-runtime) and Mesa's lavapipe (PREBUILT mesa), registered
    // under HKLM\SOFTWARE\Khronos\Vulkan\Drivers (user/mesa), all in
    // System32 as a driver installs them.
    ("vk_test", "ntdll.dll", "Windows/System32"),
    ("vk_test", "kernel32.dll", "Windows/System32"),
    ("vk_test", "msvcrt.dll", "Windows/System32"),
    ("vk_test", "user32.dll", "Windows/System32"),
    ("vk_test", "gdi32.dll", "Windows/System32"),
    ("vk_test", "advapi32.dll", "Windows/System32"),
    ("vk_test", "bcryptprimitives.dll", "Windows/System32"),
    ("vk_test", "cfgmgr32.dll", "Windows/System32"),
    // The loader verifies registered drivers through a DXGI factory.
    ("vk_test", "dxgi.dll", "Windows/System32"),
    ("vk_test", "shell32.dll", "Windows/System32"),
    ("vk_test", "shlwapi.dll", "Windows/System32"),
    ("vk_test", "ole32.dll", "Windows/System32"),
    (
        "vk_test",
        "target/prebuilt/vulkan-runtime/VulkanRT-X64-1.4.341.0-Components/x64/vulkan-1.dll",
        "Windows/System32",
    ),
    ("vk_test", "target/prebuilt/mesa/x64/vulkan_lvp.dll", "Windows/System32"),
    ("vk_test", "target/prebuilt/mesa/x64/lvp_icd.x86_64.json", "Windows/System32"),
    ("vk_test", "user/mesa/lavapipe.regseed", "registry"),
    ("vk_test", "user/bin/vk_test.exe", "gala"),
    ("dwrite_test", "ntdll.dll", "Windows/System32"),
    ("dwrite_test", "kernel32.dll", "Windows/System32"),
    ("dwrite_test", "dwrite.dll", "Windows/System32"),
    ("dwrite_test", "fonts/selawik/selawk.ttf", "Windows/Fonts"),
    ("dwrite_test", "fonts/selawik/selawksl.ttf", "Windows/Fonts"),
    ("dwrite_test", "fonts/selawik/selawksb.ttf", "Windows/Fonts"),
    ("dwrite_test", "fonts/selawik/selawkb.ttf", "Windows/Fonts"),
    ("chromium", "fonts/selawik/selawk.ttf", "Windows/Fonts"),
    ("chromium", "fonts/selawik/selawksl.ttf", "Windows/Fonts"),
    ("chromium", "fonts/selawik/selawksb.ttf", "Windows/Fonts"),
    ("chromium", "fonts/selawik/selawkb.ttf", "Windows/Fonts"),
    ("browser_api_test", "ntdll.dll", "Windows/System32"),
    ("browser_api_test", "kernel32.dll", "Windows/System32"),
    ("browser_api_test", "version.dll", "Windows/System32"),
    ("browser_api_test", "ws2_32.dll", "Windows/System32"),
    ("browser_api_test", "advapi32.dll", "Windows/System32"),
    ("browser_api_test", "bcryptprimitives.dll", "Windows/System32"),
    ("browser_api_test", "userenv.dll", "Windows/System32"),
    ("browser_api_test", "shlwapi.dll", "Windows/System32"),
    ("browser_api_test", "netapi32.dll", "Windows/System32"),
    ("browser_api_test", "user32.dll", "Windows/System32"),
    ("browser_api_test", "gdi32.dll", "Windows/System32"),
    ("browser_api_test", "shcore.dll", "Windows/System32"),
    ("browser_api_test", "wtsapi32.dll", "Windows/System32"),
    ("browser_api_test", "dwmapi.dll", "Windows/System32"),
    ("browser_api_test", "imm32.dll", "Windows/System32"),
    ("ipv6_test", "ntdll.dll", "Windows/System32"),
    ("ipv6_test", "kernel32.dll", "Windows/System32"),
    ("ipv6_test", "ws2_32.dll", "Windows/System32"),
    ("hostnet", "ntdll.dll", "Windows/System32"),
    ("hostnet", "kernel32.dll", "Windows/System32"),
    ("hostnet", "ws2_32.dll", "Windows/System32"),
    ("hostnet", "user/hostnet_test/hosts", "Windows/System32/drivers/etc"),
    ("chromium_host", "user/hostnet_test/hosts", "Windows/System32/drivers/etc"),
    ("winsock_test", "ntdll.dll", "Windows/System32"),
    ("winsock_test", "kernel32.dll", "Windows/System32"),
    ("winsock_test", "ws2_32.dll", "Windows/System32"),
    ("sshfwd_test", "ntdll.dll", "Windows/System32"),
    ("sshfwd_test", "kernel32.dll", "Windows/System32"),
    ("sshfwd_test", "ws2_32.dll", "Windows/System32"),
    ("udp_test", "ntdll.dll", "Windows/System32"),
    ("udp_test", "kernel32.dll", "Windows/System32"),
    ("udp_test", "ws2_32.dll", "Windows/System32"),
    ("handover_net_test", "ntdll.dll", "Windows/System32"),
    ("handover_net_test", "kernel32.dll", "Windows/System32"),
    ("handover_net_test", "ws2_32.dll", "Windows/System32"),
    ("go_hello", "ntdll.dll", "Windows/System32"),
    ("go_hello", "kernel32.dll", "Windows/System32"),
    ("go_hello", "bcryptprimitives.dll", "Windows/System32"),
    ("go_goroutines", "ntdll.dll", "Windows/System32"),
    ("go_goroutines", "kernel32.dll", "Windows/System32"),
    ("go_goroutines", "bcryptprimitives.dll", "Windows/System32"),
    ("go_time", "ntdll.dll", "Windows/System32"),
    ("go_time", "kernel32.dll", "Windows/System32"),
    ("go_time", "bcryptprimitives.dll", "Windows/System32"),
    ("go_gc", "ntdll.dll", "Windows/System32"),
    ("go_gc", "kernel32.dll", "Windows/System32"),
    ("go_gc", "bcryptprimitives.dll", "Windows/System32"),
    ("go_os", "ntdll.dll", "Windows/System32"),
    ("go_os", "kernel32.dll", "Windows/System32"),
    ("go_os", "bcryptprimitives.dll", "Windows/System32"),
    ("go_exec", "ntdll.dll", "Windows/System32"),
    ("go_exec", "kernel32.dll", "Windows/System32"),
    ("go_exec", "bcryptprimitives.dll", "Windows/System32"),
    ("go_http", "ntdll.dll", "Windows/System32"),
    ("go_http", "kernel32.dll", "Windows/System32"),
    ("go_http", "bcryptprimitives.dll", "Windows/System32"),
    ("go_http", "ws2_32.dll", "Windows/System32"),
    ("go_http", "mswsock.dll", "Windows/System32"),
    ("nginx", "ntdll.dll", "Windows/System32"),
    ("nginx", "kernel32.dll", "Windows/System32"),
    ("nginx", "ws2_32.dll", "Windows/System32"),
    ("nginx", "mswsock.dll", "Windows/System32"),
    ("nginx", "msvcrt.dll", "Windows/System32"),
    ("nginx", "advapi32.dll", "Windows/System32"),
    ("nginx", "user32.dll", "Windows/System32"),
    ("nginx", "crypt32.dll", "Windows/System32"),
    ("nginx", "bcryptprimitives.dll", "Windows/System32"),
    ("nginx", "user/bin/nginx.exe", "nginx"),
    // The Linux test programs (user/lx, ROADMAP LX): C:\lx\<binary>,
    // which `lxrun=/lx/<binary>` runs.
    ("lx_hello", "user/lx/bin/hello_test", "lx"),
    ("lx_hello_pie", "user/lx/bin/hello_test-pie", "lx"),
    ("lx_write_exit", "user/lx/bin/write_exit_test", "lx"),
    ("lx_write_exit_pie", "user/lx/bin/write_exit_test-pie", "lx"),
    ("lx_brk", "user/lx/bin/brk_test", "lx"),
    ("lx_pipe", "user/lx/bin/pipe_test", "lx"),
    ("lx_brk_pie", "user/lx/bin/brk_test-pie", "lx"),
    ("lx_mmap", "user/lx/bin/mmap_test", "lx"),
    ("lx_mmap_pie", "user/lx/bin/mmap_test-pie", "lx"),
    ("lx_arch_prctl", "user/lx/bin/arch_prctl_test", "lx"),
    ("lx_arch_prctl_pie", "user/lx/bin/arch_prctl_test-pie", "lx"),
    ("lx_auxv", "user/lx/bin/auxv_test", "lx"),
    ("lx_auxv_pie", "user/lx/bin/auxv_test-pie", "lx"),
    ("lx_fileio", "user/lx/bin/fileio_test", "lx"),
    ("lx_fileio_pie", "user/lx/bin/fileio_test-pie", "lx"),
    ("lx_exec", "user/lx/bin/exec_test", "lx"),
    ("lx_exec_pie", "user/lx/bin/exec_test-pie", "lx"),
    ("lx_socket", "user/lx/bin/socket_test", "lx"),
    ("lx_socket_pie", "user/lx/bin/socket_test-pie", "lx"),
    ("lx_memfd", "user/lx/bin/memfd_test", "lx"),
    ("lx_memfd_pie", "user/lx/bin/memfd_test-pie", "lx"),
    ("lx_unix", "user/lx/bin/unix_test", "lx"),
    ("lx_unix_pie", "user/lx/bin/unix_test-pie", "lx"),
    ("lx_thread", "user/lx/bin/thread_test", "lx"),
    ("lx_forkmem", "user/lx/bin/forkmem_test", "lx"),
    ("lx_forkmem_pie", "user/lx/bin/forkmem_test-pie", "lx"),
    ("lx_thread_pie", "user/lx/bin/thread_test-pie", "lx"),
    ("lx_poll", "user/lx/bin/poll_test", "lx"),
    ("lx_poll_pie", "user/lx/bin/poll_test-pie", "lx"),
    ("lx_epoll", "user/lx/bin/epoll_test", "lx"),
    ("lx_epoll_pie", "user/lx/bin/epoll_test-pie", "lx"),
    ("lx_fs", "user/lx/bin/fs_test", "lx"),
    ("lx_fs_pie", "user/lx/bin/fs_test-pie", "lx"),
    // Dynamically linked glibc programs (ROADMAP LD): the program and its
    // libraries in C:\lx, glibc where Ubuntu's ld.so looks for it (its
    // system directory, and /lib64 as PT_INTERP names it).
    ("lx_dyn", "user/lx/bin/dyn_test", "lx"),
    ("lx_dyn", "user/lx/bin/libdyn.so", "lx"),
    ("lx_dyn", "user/lx/bin/libdyn2.so", "lx"),
    ("lx_dyn_pie", "user/lx/bin/dyn_test-pie", "lx"),
    ("lx_dyn_pie", "user/lx/bin/libdyn.so", "lx"),
    ("lx_dyn_pie", "user/lx/bin/libdyn2.so", "lx"),
    ("glibc", "target/prebuilt/glibc/ld-linux-x86-64.so.2", "lib64"),
    ("glibc", "target/prebuilt/glibc/ld-linux-x86-64.so.2", "lib/x86_64-linux-gnu"),
    ("glibc", "target/prebuilt/glibc/libc.so.6", "lib/x86_64-linux-gnu"),
    ("glibc", "target/prebuilt/glibc/libm.so.6", "lib/x86_64-linux-gnu"),
    ("lx_busybox", "target/prebuilt/busybox-linux/busybox", "lx"),
    ("lx_busybox", "user/lx/busybox/shell.sh", "lx"),
    ("lx_httpd", "target/prebuilt/busybox-linux/busybox", "lx"),
    ("lx_httpd", "user/lx/busybox/httpd.sh", "lx"),
    ("winhttp_test", "msvcrt.dll", "Windows/System32"),
    ("winhttp_test", "winhttp.dll", "Windows/System32"),
    ("winhttp_test", "user/bin/winhttp_test.exe", "gala"),
    ("wininet_test", "msvcrt.dll", "Windows/System32"),
    ("wininet_test", "wininet.dll", "Windows/System32"),
    ("wininet_test", "user/bin/wininet_test.exe", "gala"),
    ("gdi32_test", "ntdll.dll", "Windows/System32"),
    ("gdi32_test", "kernel32.dll", "Windows/System32"),
    ("gdi32_test", "msvcrt.dll", "Windows/System32"),
    ("gdi32_test", "user32.dll", "Windows/System32"),
    ("gdi32_test", "gdi32.dll", "Windows/System32"),
    ("gdi32_test", "msimg32.dll", "Windows/System32"),
    ("gdi32_test", "user/bin/gdi32_test.exe", "gala"),
    ("comctl32_test", "ntdll.dll", "Windows/System32"),
    ("comctl32_test", "kernel32.dll", "Windows/System32"),
    ("comctl32_test", "msvcrt.dll", "Windows/System32"),
    ("comctl32_test", "user32.dll", "Windows/System32"),
    ("comctl32_test", "gdi32.dll", "Windows/System32"),
    ("comctl32_test", "comctl32.dll", "Windows/System32"),
    ("comctl32_test", "user/bin/comctl32_test.exe", "gala"),
    ("controls_test", "ntdll.dll", "Windows/System32"),
    ("controls_test", "kernel32.dll", "Windows/System32"),
    ("controls_test", "msvcrt.dll", "Windows/System32"),
    ("controls_test", "user32.dll", "Windows/System32"),
    ("controls_test", "gdi32.dll", "Windows/System32"),
    ("controls_test", "user/bin/controls_test.exe", "gala"),
    ("oleaut32_test", "ntdll.dll", "Windows/System32"),
    ("oleaut32_test", "kernel32.dll", "Windows/System32"),
    ("oleaut32_test", "msvcrt.dll", "Windows/System32"),
    ("oleaut32_test", "ole32.dll", "Windows/System32"),
    ("oleaut32_test", "oleaut32.dll", "Windows/System32"),
    ("oleaut32_test", "advapi32.dll", "Windows/System32"),
    ("oleaut32_test", "bcryptprimitives.dll", "Windows/System32"),
    ("oleaut32_test", "user/bin/oleaut32_test.exe", "gala"),
    ("shlwapi_test", "ntdll.dll", "Windows/System32"),
    ("shlwapi_test", "kernel32.dll", "Windows/System32"),
    ("shlwapi_test", "msvcrt.dll", "Windows/System32"),
    ("shlwapi_test", "shlwapi.dll", "Windows/System32"),
    ("shlwapi_test", "advapi32.dll", "Windows/System32"),
    ("shlwapi_test", "bcryptprimitives.dll", "Windows/System32"),
    ("shlwapi_test", "user/bin/shlwapi_test.exe", "gala"),
    ("ole32_test", "ntdll.dll", "Windows/System32"),
    ("ole32_test", "kernel32.dll", "Windows/System32"),
    ("ole32_test", "msvcrt.dll", "Windows/System32"),
    ("ole32_test", "ole32.dll", "Windows/System32"),
    ("ole32_test", "oleaut32.dll", "Windows/System32"),
    ("ole32_test", "advapi32.dll", "Windows/System32"),
    ("ole32_test", "bcryptprimitives.dll", "Windows/System32"),
    ("ole32_test", "uiautomationcore.dll", "Windows/System32"),
    ("ole32_test", "user32.dll", "Windows/System32"),
    ("ole32_test", "gdi32.dll", "Windows/System32"),
    ("ole32_test", "user/bin/ole32_test.exe", "gala"),
    ("shell32_test", "ntdll.dll", "Windows/System32"),
    ("shell32_test", "kernel32.dll", "Windows/System32"),
    ("shell32_test", "msvcrt.dll", "Windows/System32"),
    ("shell32_test", "shell32.dll", "Windows/System32"),
    ("shell32_test", "user32.dll", "Windows/System32"),
    ("shell32_test", "gdi32.dll", "Windows/System32"),
    ("shell32_test", "shlwapi.dll", "Windows/System32"),
    ("shell32_test", "ole32.dll", "Windows/System32"),
    ("shell32_test", "advapi32.dll", "Windows/System32"),
    ("shell32_test", "bcryptprimitives.dll", "Windows/System32"),
    ("shell32_test", "user/bin/shell32_test.exe", "gala"),
    // comdlg32_test: what Inno Setup's IDE imports from comdlg32
    // (user/comdlg32_test/main.c, mingw); PrintDlgW asks winspool for the
    // default printer.
    ("comdlg32_test", "ntdll.dll", "Windows/System32"),
    ("comdlg32_test", "kernel32.dll", "Windows/System32"),
    ("comdlg32_test", "msvcrt.dll", "Windows/System32"),
    ("comdlg32_test", "comdlg32.dll", "Windows/System32"),
    ("comdlg32_test", "user32.dll", "Windows/System32"),
    ("comdlg32_test", "gdi32.dll", "Windows/System32"),
    ("comdlg32_test", "winspool.drv", "Windows/System32"),
    ("comdlg32_test", "advapi32.dll", "Windows/System32"),
    ("comdlg32_test", "bcryptprimitives.dll", "Windows/System32"),
    ("comdlg32_test", "user/bin/comdlg32_test.exe", "gala"),
    ("dbghelp_test", "ntdll.dll", "Windows/System32"),
    ("dbghelp_test", "kernel32.dll", "Windows/System32"),
    ("dbghelp_test", "msvcrt.dll", "Windows/System32"),
    ("dbghelp_test", "dbghelp.dll", "Windows/System32"),
    ("dbghelp_test", "normaliz.dll", "Windows/System32"),
    ("dbghelp_test", "user/bin/dbghelp_test.exe", "gala"),
    ("richedit_test", "ntdll.dll", "Windows/System32"),
    ("richedit_test", "kernel32.dll", "Windows/System32"),
    ("richedit_test", "msvcrt.dll", "Windows/System32"),
    ("richedit_test", "user32.dll", "Windows/System32"),
    ("richedit_test", "gdi32.dll", "Windows/System32"),
    ("richedit_test", "msftedit.dll", "Windows/System32"),
    ("richedit_test", "riched20.dll", "Windows/System32"),
    ("richedit_test", "user/bin/richedit_test.exe", "gala"),
    ("user32_test", "ntdll.dll", "Windows/System32"),
    ("user32_test", "kernel32.dll", "Windows/System32"),
    ("user32_test", "msvcrt.dll", "Windows/System32"),
    ("user32_test", "msctf.dll", "Windows/System32"),
    ("user32_test", "user32.dll", "Windows/System32"),
    ("user32_test", "gdi32.dll", "Windows/System32"),
    ("user32_test", "ole32.dll", "Windows/System32"),
    ("user32_test", "comctl32.dll", "Windows/System32"),
    ("user32_test", "mpr.dll", "Windows/System32"),
    ("user32_test", "uxtheme.dll", "Windows/System32"),
    ("user32_test", "shell32.dll", "Windows/System32"),
    ("user32_test", "oleaut32.dll", "Windows/System32"),
    ("user32_test", "advapi32.dll", "Windows/System32"),
    ("user32_test", "bcryptprimitives.dll", "Windows/System32"),
    ("user32_test", "imm32.dll", "Windows/System32"),
    ("user32_test", "user/bin/user32_test.exe", "gala"),
    ("windows_across_test", "ntdll.dll", "Windows/System32"),
    ("windows_across_test", "kernel32.dll", "Windows/System32"),
    ("windows_across_test", "msvcrt.dll", "Windows/System32"),
    ("windows_across_test", "user32.dll", "Windows/System32"),
    ("windows_across_test", "gdi32.dll", "Windows/System32"),
    ("windows_across_test", "user/bin/windows_across_test.exe", "gala"),
    // gala_cli_test lowers a token (advapi32) to run `gala` at Low.
    ("gala_cli_test", "advapi32.dll", "Windows/System32"),
    ("gala_cli_test", "bcryptprimitives.dll", "Windows/System32"),
    // pk_install_test: `gala install` writes the registry (advapi32) and
    // installs from a winget-pkgs snapshot, C:\pk_source.
    ("pk_install_test", "ntdll.dll", "Windows/System32"),
    ("pk_install_test", "kernel32.dll", "Windows/System32"),
    ("pk_install_test", "advapi32.dll", "Windows/System32"),
    ("pk_install_test", "bcryptprimitives.dll", "Windows/System32"),
    ("pk_install_test", "user/pk_install_test/source/", "pk_source"),
    // pk_install_nsis: the NSIS installer's imports (ROADMAP PK).
    ("pk_install_nsis", "user32.dll", "Windows/System32"),
    ("pk_install_nsis", "gdi32.dll", "Windows/System32"),
    ("pk_install_nsis", "comctl32.dll", "Windows/System32"),
    ("pk_install_nsis", "ole32.dll", "Windows/System32"),
    ("pk_install_nsis", "shell32.dll", "Windows/System32"),
    ("pk_install_nsis", "shfolder.dll", "Windows/System32"),
    // pk_install_inno: a real Inno Setup installer (PREBUILT innosetup,
    // in the snapshot's installers mirror under its URL's file name) and
    // what its Setup and uninstaller load.
    (
        "pk_install_inno",
        "target/prebuilt/innosetup/innosetup-7.1.1-x64.exe",
        "pk_source/installers",
    ),
    ("pk_install_inno", "user32.dll", "Windows/System32"),
    ("pk_install_inno", "gdi32.dll", "Windows/System32"),
    ("pk_install_inno", "comctl32.dll", "Windows/System32"),
    ("pk_install_inno", "ole32.dll", "Windows/System32"),
    ("pk_install_inno", "oleaut32.dll", "Windows/System32"),
    ("pk_install_inno", "shell32.dll", "Windows/System32"),
    ("pk_install_inno", "bcrypt.dll", "Windows/System32"),
    // What Setup (the program the installer unpacks to %TEMP% and runs)
    // links: Delphi's VCL and Inno's own imports.
    ("pk_install_inno", "mpr.dll", "Windows/System32"),
    ("pk_install_inno", "shlwapi.dll", "Windows/System32"),
    ("pk_install_inno", "winspool.drv", "Windows/System32"),
    ("pk_install_inno", "comdlg32.dll", "Windows/System32"),
    ("pk_install_inno", "version.dll", "Windows/System32"),
    ("pk_install_inno", "wtsapi32.dll", "Windows/System32"),
    ("pk_install_inno", "winhttp.dll", "Windows/System32"),
    ("pk_install_inno", "secur32.dll", "Windows/System32"),
    ("pk_install_inno", "ws2_32.dll", "Windows/System32"),
    ("pk_install_inno", "uxtheme.dll", "Windows/System32"),
    ("pk_install_inno", "imm32.dll", "Windows/System32"),
    ("pk_install_inno", "dwmapi.dll", "Windows/System32"),
    ("pk_install_inno", "crypt32.dll", "Windows/System32"),
    ("pk_install_inno", "msimg32.dll", "Windows/System32"),
    ("pk_install_inno", "msctf.dll", "Windows/System32"),
    // The UCRT (ucrtbase.dll ships with Windows; gala has none of its own:
    // Microsoft's redistributable copy from the JDK, as other scenarios).
    ("pk_install_inno", "target/prebuilt/jdk/jdk-21.0.5+11/bin/ucrtbase.dll", "Windows/System32"),
    // pk_install_https: `gala install` downloads over winhttp from
    // schannel_test's Nginx (MODULE_SETS), which serves the package; the
    // machine root store holds the public CA bundle beside the test CA.
    ("pk_install_https", "winhttp.dll", "Windows/System32"),
    (
        "pk_install_https",
        "user/pk_install_test/served/objects/pkhttps-1.0.0.zip",
        "nginx/html/objects",
    ),
    ("pk_install_https", "user/roots/public-roots.pem", "Windows/System32/gala/roots"),
    // ... and a winget-pkgs tree (directory listings as JSON) for
    // manifests over HTTPS.
    ("pk_install_https", "user/pk_install_test/served/winget-pkgs/", "nginx/html/winget-pkgs"),
    ("uipi_test", "ntdll.dll", "Windows/System32"),
    ("uipi_test", "kernel32.dll", "Windows/System32"),
    ("uipi_test", "msvcrt.dll", "Windows/System32"),
    ("uipi_test", "user32.dll", "Windows/System32"),
    ("uipi_test", "gdi32.dll", "Windows/System32"),
    ("uipi_test", "advapi32.dll", "Windows/System32"),
    ("uipi_test", "bcryptprimitives.dll", "Windows/System32"),
    ("uipi_test", "user/bin/uipi_test.exe", "gala"),
    ("winevent_test", "ntdll.dll", "Windows/System32"),
    ("winevent_test", "kernel32.dll", "Windows/System32"),
    ("winevent_test", "msvcrt.dll", "Windows/System32"),
    ("winevent_test", "user32.dll", "Windows/System32"),
    ("winevent_test", "gdi32.dll", "Windows/System32"),
    ("winevent_test", "bcryptprimitives.dll", "Windows/System32"),
    ("winevent_test", "user/bin/winevent_test.exe", "gala"),
    ("uia_test", "ntdll.dll", "Windows/System32"),
    ("uia_test", "kernel32.dll", "Windows/System32"),
    ("uia_test", "msvcrt.dll", "Windows/System32"),
    ("uia_test", "user32.dll", "Windows/System32"),
    ("uia_test", "gdi32.dll", "Windows/System32"),
    ("uia_test", "ole32.dll", "Windows/System32"),
    ("uia_test", "oleaut32.dll", "Windows/System32"),
    ("uia_test", "bcryptprimitives.dll", "Windows/System32"),
    ("uia_test", "uiautomationcore.dll", "Windows/System32"),
    ("uia_test", "comctl32.dll", "Windows/System32"),
    ("uia_test", "user/bin/uia_test.exe", "gala"),
    ("msvcrt_test", "ntdll.dll", "Windows/System32"),
    ("msvcrt_test", "kernel32.dll", "Windows/System32"),
    ("msvcrt_test", "msvcrt.dll", "Windows/System32"),
    // rand_s loads it (ProcessPrng).
    ("msvcrt_test", "bcryptprimitives.dll", "Windows/System32"),
    ("msvcrt_test", "shell32.dll", "Windows/System32"),
    ("msvcrt_test", "user/bin/msvcrt_test.exe", "gala"),
    ("busybox", "ntdll.dll", "Windows/System32"),
    ("busybox", "kernel32.dll", "Windows/System32"),
    ("busybox", "ws2_32.dll", "Windows/System32"),
    ("busybox", "mswsock.dll", "Windows/System32"),
    ("busybox", "msvcrt.dll", "Windows/System32"),
    ("busybox", "advapi32.dll", "Windows/System32"),
    ("busybox", "user32.dll", "Windows/System32"),
    ("busybox", "bcryptprimitives.dll", "Windows/System32"),
    ("busybox", "user/bin/busybox.exe", "gala"),
    ("busybox_files", "ntdll.dll", "Windows/System32"),
    ("busybox_files", "kernel32.dll", "Windows/System32"),
    ("busybox_files", "ws2_32.dll", "Windows/System32"),
    ("busybox_files", "mswsock.dll", "Windows/System32"),
    ("busybox_files", "msvcrt.dll", "Windows/System32"),
    ("busybox_files", "advapi32.dll", "Windows/System32"),
    ("busybox_files", "user32.dll", "Windows/System32"),
    ("busybox_files", "bcryptprimitives.dll", "Windows/System32"),
    ("busybox_files", "user/bin/busybox.exe", "gala"),
    ("busybox_files", "user/busybox/files.sh", "gala"),
    ("busybox_script", "ntdll.dll", "Windows/System32"),
    ("busybox_script", "kernel32.dll", "Windows/System32"),
    ("busybox_script", "ws2_32.dll", "Windows/System32"),
    ("busybox_script", "mswsock.dll", "Windows/System32"),
    ("busybox_script", "msvcrt.dll", "Windows/System32"),
    ("busybox_script", "advapi32.dll", "Windows/System32"),
    ("busybox_script", "user32.dll", "Windows/System32"),
    ("busybox_script", "bcryptprimitives.dll", "Windows/System32"),
    ("busybox_script", "user/bin/busybox.exe", "gala"),
    ("busybox_script", "user/busybox/script.sh", "gala"),
    ("tcc", "ntdll.dll", "Windows/System32"),
    ("tcc", "kernel32.dll", "Windows/System32"),
    ("tcc", "ws2_32.dll", "Windows/System32"),
    ("tcc", "mswsock.dll", "Windows/System32"),
    ("tcc", "msvcrt.dll", "Windows/System32"),
    ("tcc", "advapi32.dll", "Windows/System32"),
    ("tcc", "user32.dll", "Windows/System32"),
    ("tcc", "bcryptprimitives.dll", "Windows/System32"),
    ("tcc", "user/bin/busybox.exe", "gala"),
    ("tcc", "user/bin/tcc.tar", "gala"),
    ("tcc", "user/tcc/tcc.sh", "gala"),
    ("tcc", "user/tcc/hello.c", "gala"),
    ("tcc_self", "ntdll.dll", "Windows/System32"),
    ("tcc_self", "kernel32.dll", "Windows/System32"),
    ("tcc_self", "ws2_32.dll", "Windows/System32"),
    ("tcc_self", "mswsock.dll", "Windows/System32"),
    ("tcc_self", "msvcrt.dll", "Windows/System32"),
    ("tcc_self", "advapi32.dll", "Windows/System32"),
    ("tcc_self", "user32.dll", "Windows/System32"),
    ("tcc_self", "bcryptprimitives.dll", "Windows/System32"),
    ("tcc_self", "user/bin/busybox.exe", "gala"),
    ("tcc_self", "user/bin/tcc.tar", "gala"),
    ("tcc_self", "user/tcc/tcc.sh", "gala"),
    ("tcc_self", "user/tcc/self.sh", "gala"),
    ("tcc_self", "user/tcc/hello.c", "gala"),
    ("tcc_self", "user/msvcrt_test/main.c", "gala"),
    ("tcc_make", "ntdll.dll", "Windows/System32"),
    ("tcc_make", "kernel32.dll", "Windows/System32"),
    ("tcc_make", "ws2_32.dll", "Windows/System32"),
    ("tcc_make", "mswsock.dll", "Windows/System32"),
    ("tcc_make", "msvcrt.dll", "Windows/System32"),
    ("tcc_make", "advapi32.dll", "Windows/System32"),
    ("tcc_make", "user32.dll", "Windows/System32"),
    ("tcc_make", "bcryptprimitives.dll", "Windows/System32"),
    ("tcc_make", "user/bin/busybox.exe", "gala"),
    ("tcc_make", "user/bin/tcc.tar", "gala"),
    ("tcc_make", "user/tcc/make.sh", "gala"),
    ("tcc_make", "user/tcc/make/Makefile", "gala/make"),
    ("tcc_make", "user/tcc/make/calc.h", "gala/make"),
    ("tcc_make", "user/tcc/make/main.c", "gala/make"),
    ("tcc_make", "user/tcc/make/add.c", "gala/make"),
    ("tcc_make", "user/tcc/make/mul.c", "gala/make"),
    // Win32-OpenSSH, where install-sshd.ps1 puts it.
    ("sshd", "ntdll.dll", "Windows/System32"),
    ("sshd", "kernel32.dll", "Windows/System32"),
    ("sshd", "advapi32.dll", "Windows/System32"),
    ("sshd", "bcrypt.dll", "Windows/System32"),
    ("sshd", "bcryptprimitives.dll", "Windows/System32"),
    ("sshd", "crypt32.dll", "Windows/System32"),
    ("sshd", "secur32.dll", "Windows/System32"),
    ("sshd", "user32.dll", "Windows/System32"),
    ("sshd", "userenv.dll", "Windows/System32"),
    ("sshd", "ws2_32.dll", "Windows/System32"),
    ("sshd", "mswsock.dll", "Windows/System32"),
    ("sshd", "target/prebuilt/openssh/OpenSSH-Win64/", "Program Files/OpenSSH"),
    ("sshd", "user/sshd/sshd_config", "ProgramData/ssh"),
    ("sshd_login", "ntdll.dll", "Windows/System32"),
    ("sshd_login", "kernel32.dll", "Windows/System32"),
    ("sshd_login", "advapi32.dll", "Windows/System32"),
    ("sshd_login", "bcrypt.dll", "Windows/System32"),
    ("sshd_login", "bcryptprimitives.dll", "Windows/System32"),
    ("sshd_login", "crypt32.dll", "Windows/System32"),
    ("sshd_login", "secur32.dll", "Windows/System32"),
    ("sshd_login", "user32.dll", "Windows/System32"),
    ("sshd_login", "userenv.dll", "Windows/System32"),
    ("sshd_login", "ws2_32.dll", "Windows/System32"),
    ("sshd_login", "mswsock.dll", "Windows/System32"),
    ("sshd_login", "target/prebuilt/openssh/OpenSSH-Win64/", "Program Files/OpenSSH"),
    ("sshd_login", "user/sshd/sshd_config", "ProgramData/ssh"),
    ("sshd_login", "user/sshd/authorized_keys", "Users/galatest/.ssh"),
    ("sshd_login", "user/access_test/access_test.regseed", "registry"),
    ("sshd_login", "user/sshd/sshd.regseed", "registry"),
    ("sshd_login", "user/bin/busybox.exe", "gala"),
    ("sshd_login", "msvcrt.dll", "Windows/System32"),
    ("sshd_password", "ntdll.dll", "Windows/System32"),
    ("sshd_password", "kernel32.dll", "Windows/System32"),
    ("sshd_password", "advapi32.dll", "Windows/System32"),
    ("sshd_password", "bcrypt.dll", "Windows/System32"),
    ("sshd_password", "bcryptprimitives.dll", "Windows/System32"),
    ("sshd_password", "crypt32.dll", "Windows/System32"),
    ("sshd_password", "secur32.dll", "Windows/System32"),
    ("sshd_password", "user32.dll", "Windows/System32"),
    ("sshd_password", "userenv.dll", "Windows/System32"),
    ("sshd_password", "ws2_32.dll", "Windows/System32"),
    ("sshd_password", "mswsock.dll", "Windows/System32"),
    ("sshd_password", "target/prebuilt/openssh/OpenSSH-Win64/", "Program Files/OpenSSH"),
    ("sshd_password", "user/sshd/sshd_config", "ProgramData/ssh"),
    ("sshd_password", "user/access_test/access_test.regseed", "registry"),
    ("sshd_password", "user/sshd/sshd.regseed", "registry"),
    ("sshd_password", "user/bin/busybox.exe", "gala"),
    ("sshd_password", "msvcrt.dll", "Windows/System32"),
    ("caddy", "ntdll.dll", "Windows/System32"),
    ("caddy", "kernel32.dll", "Windows/System32"),
    ("caddy", "advapi32.dll", "Windows/System32"),
    ("caddy", "winmm.dll", "Windows/System32"),
    ("caddy", "ncrypt.dll", "Windows/System32"),
    ("caddy", "crypt32.dll", "Windows/System32"),
    ("caddy", "ws2_32.dll", "Windows/System32"),
    ("caddy", "mswsock.dll", "Windows/System32"),
    ("caddy", "bcryptprimitives.dll", "Windows/System32"),
    ("caddy", "target/prebuilt/caddy/caddy.exe", "gala"),
    ("caddy_http", "ntdll.dll", "Windows/System32"),
    ("caddy_http", "kernel32.dll", "Windows/System32"),
    ("caddy_http", "advapi32.dll", "Windows/System32"),
    ("caddy_http", "winmm.dll", "Windows/System32"),
    ("caddy_http", "ncrypt.dll", "Windows/System32"),
    ("caddy_http", "crypt32.dll", "Windows/System32"),
    ("caddy_http", "ws2_32.dll", "Windows/System32"),
    ("caddy_http", "mswsock.dll", "Windows/System32"),
    ("caddy_http", "bcryptprimitives.dll", "Windows/System32"),
    ("caddy_http", "target/prebuilt/caddy/caddy.exe", "gala"),
    ("caddy_http", "user/caddy/site/index.html", "caddy/site"),
    ("java_version", "ntdll.dll", "Windows/System32"),
    ("java_version", "kernel32.dll", "Windows/System32"),
    ("java_version", "advapi32.dll", "Windows/System32"),
    ("java_version", "winmm.dll", "Windows/System32"),
    ("java_version", "version.dll", "Windows/System32"),
    ("java_version", "shell32.dll", "Windows/System32"),
    ("java_version", "powrprof.dll", "Windows/System32"),
    ("java_version", "comctl32.dll", "Windows/System32"),
    ("java_version", "ncrypt.dll", "Windows/System32"),
    ("java_version", "crypt32.dll", "Windows/System32"),
    ("java_version", "ws2_32.dll", "Windows/System32"),
    ("java_version", "mswsock.dll", "Windows/System32"),
    ("java_version", "bcryptprimitives.dll", "Windows/System32"),
    ("java_version", "user32.dll", "Windows/System32"),
    ("java_version", "iphlpapi.dll", "Windows/System32"),
    ("java_version", "secur32.dll", "Windows/System32"),
    ("java_version", "winhttp.dll", "Windows/System32"),
    ("java_version", "msvcrt.dll", "Windows/System32"),
    ("java_version", "target/prebuilt/jdk/jdk-21.0.5+11/", "jdk"),
    ("elasticsearch", "ntdll.dll", "Windows/System32"),
    ("elasticsearch", "kernel32.dll", "Windows/System32"),
    ("elasticsearch", "advapi32.dll", "Windows/System32"),
    ("elasticsearch", "winmm.dll", "Windows/System32"),
    ("elasticsearch", "version.dll", "Windows/System32"),
    ("elasticsearch", "shell32.dll", "Windows/System32"),
    ("elasticsearch", "powrprof.dll", "Windows/System32"),
    ("elasticsearch", "comctl32.dll", "Windows/System32"),
    ("elasticsearch", "ncrypt.dll", "Windows/System32"),
    ("elasticsearch", "crypt32.dll", "Windows/System32"),
    ("elasticsearch", "ws2_32.dll", "Windows/System32"),
    ("elasticsearch", "mswsock.dll", "Windows/System32"),
    ("elasticsearch", "bcryptprimitives.dll", "Windows/System32"),
    ("elasticsearch", "user32.dll", "Windows/System32"),
    ("elasticsearch", "iphlpapi.dll", "Windows/System32"),
    ("elasticsearch", "secur32.dll", "Windows/System32"),
    ("elasticsearch", "winhttp.dll", "Windows/System32"),
    ("elasticsearch", "msvcrt.dll", "Windows/System32"),
    ("elasticsearch", "psapi.dll", "Windows/System32"),
    ("elasticsearch", "target/prebuilt/elasticsearch/elasticsearch-8.15.3/", "es"),
    // The JDK where Elasticsearch looks for its bundled one (C:\\es\\jdk).
    ("elasticsearch", "target/prebuilt/jdk/jdk-21.0.5+11/", "es/jdk"),
    ("elasticsearch", "user/elasticsearch/gala.options", "es/config/jvm.options.d"),
    ("sqllocaldb", "ntdll.dll", "Windows/System32"),
    ("sqllocaldb", "kernel32.dll", "Windows/System32"),
    ("sqllocaldb", "advapi32.dll", "Windows/System32"),
    ("sqllocaldb", "authz.dll", "Windows/System32"),
    ("sqllocaldb", "bcrypt.dll", "Windows/System32"),
    ("sqllocaldb", "bcryptprimitives.dll", "Windows/System32"),
    ("sqllocaldb", "crypt32.dll", "Windows/System32"),
    ("sqllocaldb", "dhcpcsvc.dll", "Windows/System32"),
    ("sqllocaldb", "dbghelp.dll", "Windows/System32"),
    ("sqllocaldb", "xmllite.dll", "Windows/System32"),
    ("sqllocaldb", "clusapi.dll", "Windows/System32"),
    ("sqllocaldb", "resutils.dll", "Windows/System32"),
    ("sqllocaldb", "security.dll", "Windows/System32"),
    ("sqllocaldb", "iphlpapi.dll", "Windows/System32"),
    ("sqllocaldb", "mswsock.dll", "Windows/System32"),
    ("sqllocaldb", "ncrypt.dll", "Windows/System32"),
    ("sqllocaldb", "netapi32.dll", "Windows/System32"),
    ("sqllocaldb", "odbc32.dll", "Windows/System32"),
    ("sqllocaldb", "ole32.dll", "Windows/System32"),
    ("sqllocaldb", "oleaut32.dll", "Windows/System32"),
    ("sqllocaldb", "pdh.dll", "Windows/System32"),
    ("sqllocaldb", "psapi.dll", "Windows/System32"),
    ("sqllocaldb", "rpcrt4.dll", "Windows/System32"),
    ("sqllocaldb", "secur32.dll", "Windows/System32"),
    ("sqllocaldb", "shell32.dll", "Windows/System32"),
    ("sqllocaldb", "shlwapi.dll", "Windows/System32"),
    ("sqllocaldb", "urlmon.dll", "Windows/System32"),
    ("sqllocaldb", "user32.dll", "Windows/System32"),
    ("sqllocaldb", "userenv.dll", "Windows/System32"),
    ("sqllocaldb", "version.dll", "Windows/System32"),
    ("sqllocaldb", "webservices.dll", "Windows/System32"),
    ("sqllocaldb", "winhttp.dll", "Windows/System32"),
    ("sqllocaldb", "wininet.dll", "Windows/System32"),
    ("sqllocaldb", "winmm.dll", "Windows/System32"),
    ("sqllocaldb", "wintrust.dll", "Windows/System32"),
    ("sqllocaldb", "ws2_32.dll", "Windows/System32"),
    ("sqllocaldb", "xolehlp.dll", "Windows/System32"),
    // The database engine as SqlLocalDB.msi installs it, with Microsoft's
    // Visual C++ runtime app-local (the JDK's redistributable copies;
    // LocalDB expects them installed system-wide).
    ("sqllocaldb", "target/prebuilt/sqllocaldb/Binn/", "sql/Binn"),
    ("sqllocaldb", "target/prebuilt/jdk/jdk-21.0.5+11/bin/ucrtbase.dll", "sql/Binn"),
    ("sqllocaldb", "target/prebuilt/jdk/jdk-21.0.5+11/bin/msvcp140.dll", "sql/Binn"),
    ("sqllocaldb", "target/prebuilt/jdk/jdk-21.0.5+11/bin/vcruntime140.dll", "sql/Binn"),
    ("sqllocaldb", "target/prebuilt/jdk/jdk-21.0.5+11/bin/vcruntime140_1.dll", "sql/Binn"),
    ("sqllocaldb", "target/prebuilt/sqllocaldb/Shared/", "sql/Shared"),
    // What the MSI writes to the registry.
    ("sqllocaldb", "user/sqllocaldb/sqllocaldb.regseed", "registry"),
    // The instance's databases: copies of the templates, as SqlLocalDB
    // makes when it creates an instance.
    ("sqllocaldb", "target/prebuilt/sqllocaldb/Binn/Templates/", "sql/data"),
    // PostgreSQL 17.2 as EnterpriseDB ships it, with the Visual C++ runtime
    // app-local (the JDK's redistributable copies; it expects them installed).
    ("gitbash", "ntdll.dll", "Windows/System32"),
    ("gitbash", "kernel32.dll", "Windows/System32"),
    ("gitbash", "advapi32.dll", "Windows/System32"),
    ("gitbash", "bcrypt.dll", "Windows/System32"),
    ("gitbash", "bcryptprimitives.dll", "Windows/System32"),
    ("gitbash", "crypt32.dll", "Windows/System32"),
    ("gitbash", "msvcrt.dll", "Windows/System32"),
    ("gitbash", "secur32.dll", "Windows/System32"),
    ("gitbash", "shell32.dll", "Windows/System32"),
    ("gitbash", "shlwapi.dll", "Windows/System32"),
    ("gitbash", "user32.dll", "Windows/System32"),
    ("gitbash", "ws2_32.dll", "Windows/System32"),
    ("gitbash", "userenv.dll", "Windows/System32"),
    ("gitbash", "netapi32.dll", "Windows/System32"),
    ("gitbash", "target/prebuilt/portablegit/bin/", "git/bin"),
    ("gitbash", "target/prebuilt/portablegit/etc/", "git/etc"),
    ("gitbash", "target/prebuilt/portablegit/usr/bin/", "git/usr/bin"),
    ("filezilla", "kernel/assets/wallpapers/dunes.qoi", "Windows/Web/Wallpaper"),
    ("filezilla", "kernel/assets/wallpapers/ribbons.qoi", "Windows/Web/Wallpaper"),
    ("filezilla", "kernel/assets/wallpapers/najdi.qoi", "Windows/Web/Wallpaper"),
    ("filezilla", "kernel/assets/wallpapers/night.qoi", "Windows/Web/Wallpaper"),
    ("filezilla", "ntdll.dll", "Windows/System32"),
    ("filezilla", "kernel32.dll", "Windows/System32"),
    ("filezilla", "advapi32.dll", "Windows/System32"),
    ("filezilla", "bcrypt.dll", "Windows/System32"),
    ("filezilla", "bcryptprimitives.dll", "Windows/System32"),
    ("filezilla", "crypt32.dll", "Windows/System32"),
    ("filezilla", "msvcrt.dll", "Windows/System32"),
    ("filezilla", "secur32.dll", "Windows/System32"),
    ("filezilla", "shell32.dll", "Windows/System32"),
    ("filezilla", "user32.dll", "Windows/System32"),
    ("filezilla", "gdi32.dll", "Windows/System32"),
    ("filezilla", "comctl32.dll", "Windows/System32"),
    ("filezilla", "ws2_32.dll", "Windows/System32"),
    ("filezilla", "userenv.dll", "Windows/System32"),
    ("filezilla", "shlwapi.dll", "Windows/System32"),
    ("filezilla", "ncrypt.dll", "Windows/System32"),
    ("filezilla", "ole32.dll", "Windows/System32"),
    ("filezilla", "oleaut32.dll", "Windows/System32"),
    ("filezilla", "iphlpapi.dll", "Windows/System32"),
    ("filezilla", "version.dll", "Windows/System32"),
    ("filezilla", "rpcrt4.dll", "Windows/System32"),
    ("filezilla", "netapi32.dll", "Windows/System32"),
    ("filezilla", "powrprof.dll", "Windows/System32"),
    ("filezilla", "winmm.dll", "Windows/System32"),
    ("filezilla", "uxtheme.dll", "Windows/System32"),
    ("filezilla", "comdlg32.dll", "Windows/System32"),
    ("filezilla", "oleacc.dll", "Windows/System32"),
    ("filezilla", "mpr.dll", "Windows/System32"),
    ("filezilla", "msimg32.dll", "Windows/System32"),
    ("filezilla", "msftedit.dll", "Windows/System32"),
    ("filezilla", "riched20.dll", "Windows/System32"),
    ("filezilla", "target/prebuilt/filezilla/", "filezilla"),
    ("filezilla", "target/prebuilt/filezilla-server/", "filezilla-server"),
    ("filezilla", "user/filezilla_server/fzs/", "fzs"),
    ("filezilla", "user/filezilla_server/ftp/", "ftp"),
    ("mingit", "ntdll.dll", "Windows/System32"),
    ("mingit", "kernel32.dll", "Windows/System32"),
    ("mingit", "advapi32.dll", "Windows/System32"),
    ("mingit", "bcrypt.dll", "Windows/System32"),
    ("mingit", "bcryptprimitives.dll", "Windows/System32"),
    ("mingit", "crypt32.dll", "Windows/System32"),
    ("mingit", "msvcrt.dll", "Windows/System32"),
    ("mingit", "secur32.dll", "Windows/System32"),
    ("mingit", "shell32.dll", "Windows/System32"),
    ("mingit", "user32.dll", "Windows/System32"),
    ("mingit", "ws2_32.dll", "Windows/System32"),
    ("mingit", "userenv.dll", "Windows/System32"),
    ("mingit", "shlwapi.dll", "Windows/System32"),
    ("mingit", "target/prebuilt/mingit/", "git"),
    ("mingit", "user/bin/cmd.exe", "Windows/System32"),
    ("postgres", "ntdll.dll", "Windows/System32"),
    ("postgres", "kernel32.dll", "Windows/System32"),
    ("postgres", "advapi32.dll", "Windows/System32"),
    ("postgres", "bcrypt.dll", "Windows/System32"),
    ("postgres", "bcryptprimitives.dll", "Windows/System32"),
    ("postgres", "crypt32.dll", "Windows/System32"),
    ("postgres", "msvcrt.dll", "Windows/System32"),
    ("postgres", "secur32.dll", "Windows/System32"),
    ("postgres", "shell32.dll", "Windows/System32"),
    ("postgres", "user32.dll", "Windows/System32"),
    ("postgres", "ws2_32.dll", "Windows/System32"),
    ("postgres", "mswsock.dll", "Windows/System32"),
    ("postgres", "userenv.dll", "Windows/System32"),
    ("postgres", "wldap32.dll", "Windows/System32"),
    ("postgres", "wsock32.dll", "Windows/System32"),
    ("postgres", "target/prebuilt/postgresql/pgsql/", "pgsql"),
    ("postgres", "target/prebuilt/jdk/jdk-21.0.5+11/bin/ucrtbase.dll", "pgsql/bin"),
    ("postgres", "target/prebuilt/jdk/jdk-21.0.5+11/bin/vcruntime140.dll", "pgsql/bin"),
    ("postgres", "target/prebuilt/jdk/jdk-21.0.5+11/bin/vcruntime140_1.dll", "pgsql/bin"),
    ("postgres", "target/prebuilt/jdk/jdk-21.0.5+11/bin/msvcp140.dll", "pgsql/bin"),
    (
        "postgres",
        "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-crt-convert-l1-1-0.dll",
        "pgsql/bin",
    ),
    (
        "postgres",
        "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-crt-environment-l1-1-0.dll",
        "pgsql/bin",
    ),
    (
        "postgres",
        "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-crt-filesystem-l1-1-0.dll",
        "pgsql/bin",
    ),
    (
        "postgres",
        "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-crt-heap-l1-1-0.dll",
        "pgsql/bin",
    ),
    (
        "postgres",
        "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-crt-locale-l1-1-0.dll",
        "pgsql/bin",
    ),
    (
        "postgres",
        "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-crt-math-l1-1-0.dll",
        "pgsql/bin",
    ),
    (
        "postgres",
        "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-crt-multibyte-l1-1-0.dll",
        "pgsql/bin",
    ),
    (
        "postgres",
        "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-crt-runtime-l1-1-0.dll",
        "pgsql/bin",
    ),
    (
        "postgres",
        "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-crt-stdio-l1-1-0.dll",
        "pgsql/bin",
    ),
    (
        "postgres",
        "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-crt-string-l1-1-0.dll",
        "pgsql/bin",
    ),
    (
        "postgres",
        "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-crt-time-l1-1-0.dll",
        "pgsql/bin",
    ),
    (
        "postgres",
        "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-crt-utility-l1-1-0.dll",
        "pgsql/bin",
    ),
    (
        "postgres",
        "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-crt-conio-l1-1-0.dll",
        "pgsql/bin",
    ),
    (
        "postgres",
        "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-crt-process-l1-1-0.dll",
        "pgsql/bin",
    ),
    ("postgres", "user/postgres/pg_hba.conf", "gala"),
    ("postgres", "user/bin/cmd.exe", "Windows/System32"),
    ("dotnet", "ntdll.dll", "Windows/System32"),
    ("dotnet", "kernel32.dll", "Windows/System32"),
    ("dotnet", "advapi32.dll", "Windows/System32"),
    ("dotnet", "user32.dll", "Windows/System32"),
    ("dotnet", "ws2_32.dll", "Windows/System32"),
    ("dotnet", "mswsock.dll", "Windows/System32"),
    ("dotnet", "bcryptprimitives.dll", "Windows/System32"),
    ("dotnet", "version.dll", "Windows/System32"),
    ("dotnet", "ole32.dll", "Windows/System32"),
    ("dotnet", "bcrypt.dll", "Windows/System32"),
    ("dotnet", "oleaut32.dll", "Windows/System32"),
    ("dotnet", "shell32.dll", "Windows/System32"),
    ("dotnet", "winmm.dll", "Windows/System32"),
    ("dotnet", "msvcrt.dll", "Windows/System32"),
    // P1 step 6: the embeddable CPython (PREBUILT `python`) in C:\python,
    // gala's DLLs it imports, the JDK's UCRT beside it.
    ("python_hello", "target/prebuilt/python/", "python"),
    ("python_hello", "target/prebuilt/jdk/jdk-21.0.5+11/bin/ucrtbase.dll", "python"),
    ("python_hello", "ntdll.dll", "Windows/System32"),
    ("python_hello", "kernel32.dll", "Windows/System32"),
    ("python_hello", "advapi32.dll", "Windows/System32"),
    ("python_hello", "version.dll", "Windows/System32"),
    ("python_hello", "ws2_32.dll", "Windows/System32"),
    ("python_hello", "mswsock.dll", "Windows/System32"),
    ("python_hello", "bcrypt.dll", "Windows/System32"),
    ("python_hello", "bcryptprimitives.dll", "Windows/System32"),
    ("python_hello", "iphlpapi.dll", "Windows/System32"),
    ("python_hello", "rpcrt4.dll", "Windows/System32"),
    ("python_hello", "user32.dll", "Windows/System32"),
    ("python_hello", "msvcrt.dll", "Windows/System32"),
    ("python_hello", "ole32.dll", "Windows/System32"),
    ("python_hello", "oleaut32.dll", "Windows/System32"),
    ("python_hello", "crypt32.dll", "Windows/System32"),
    ("python_script", "user/python/python_test.py", "gala"),
    ("python_async", "user/python/python_async.py", "gala"),
    ("python_mp", "user/python/python_mp.py", "gala"),
    // P1 step 6: pip and venv on the full CPython layout (PREBUILT
    // `python-full`) in C:\python, as python_hello's.
    ("python_pip", "target/prebuilt/python-full/tools/", "python"),
    ("python_pip", "target/prebuilt/jdk/jdk-21.0.5+11/bin/ucrtbase.dll", "python"),
    ("python_pip", "ntdll.dll", "Windows/System32"),
    ("python_pip", "kernel32.dll", "Windows/System32"),
    ("python_pip", "advapi32.dll", "Windows/System32"),
    ("python_pip", "version.dll", "Windows/System32"),
    ("python_pip", "ws2_32.dll", "Windows/System32"),
    ("python_pip", "mswsock.dll", "Windows/System32"),
    ("python_pip", "bcrypt.dll", "Windows/System32"),
    ("python_pip", "bcryptprimitives.dll", "Windows/System32"),
    ("python_pip", "iphlpapi.dll", "Windows/System32"),
    ("python_pip", "rpcrt4.dll", "Windows/System32"),
    ("python_pip", "user32.dll", "Windows/System32"),
    ("python_pip", "msvcrt.dll", "Windows/System32"),
    ("python_pip", "ole32.dll", "Windows/System32"),
    ("python_pip", "oleaut32.dll", "Windows/System32"),
    ("python_pip", "crypt32.dll", "Windows/System32"),
    ("python_pip", "shell32.dll", "Windows/System32"),
    ("python_pip", "shlwapi.dll", "Windows/System32"),
    ("python_pip", "user/python/python_pip.py", "gala"),
    // P1 step 6: numpy (C extensions, OpenBLAS) on the full layout.
    ("python_numpy", "user/python/python_numpy.py", "gala"),
    ("python_numpy", "target/prebuilt/numpy/numpy-2.2.1-cp313-cp313-win_amd64.whl", "gala"),
    ("pwsh", "target/prebuilt/pwsh/", "pwsh"),
    ("pwsh", "ntdll.dll", "Windows/System32"),
    ("pwsh", "kernel32.dll", "Windows/System32"),
    ("pwsh", "advapi32.dll", "Windows/System32"),
    ("pwsh", "user32.dll", "Windows/System32"),
    ("pwsh", "ws2_32.dll", "Windows/System32"),
    ("pwsh", "mswsock.dll", "Windows/System32"),
    ("pwsh", "bcryptprimitives.dll", "Windows/System32"),
    ("pwsh", "version.dll", "Windows/System32"),
    ("pwsh", "ole32.dll", "Windows/System32"),
    ("pwsh", "bcrypt.dll", "Windows/System32"),
    ("pwsh", "secur32.dll", "Windows/System32"),
    ("pwsh", "iphlpapi.dll", "Windows/System32"),
    ("pwsh", "oleaut32.dll", "Windows/System32"),
    ("pwsh", "shell32.dll", "Windows/System32"),
    ("pwsh", "winmm.dll", "Windows/System32"),
    ("pwsh", "msvcrt.dll", "Windows/System32"),
    ("pwsh", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-console-l1-1-0.dll", "pwsh"),
    ("pwsh", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-console-l1-2-0.dll", "pwsh"),
    ("pwsh", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-datetime-l1-1-0.dll", "pwsh"),
    ("pwsh", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-debug-l1-1-0.dll", "pwsh"),
    (
        "pwsh",
        "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-errorhandling-l1-1-0.dll",
        "pwsh",
    ),
    ("pwsh", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-fibers-l1-1-0.dll", "pwsh"),
    ("pwsh", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-fibers-l1-1-1.dll", "pwsh"),
    ("pwsh", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-file-l1-1-0.dll", "pwsh"),
    ("pwsh", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-file-l1-2-0.dll", "pwsh"),
    ("pwsh", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-file-l2-1-0.dll", "pwsh"),
    ("pwsh", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-handle-l1-1-0.dll", "pwsh"),
    ("pwsh", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-heap-l1-1-0.dll", "pwsh"),
    (
        "pwsh",
        "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-interlocked-l1-1-0.dll",
        "pwsh",
    ),
    (
        "pwsh",
        "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-kernel32-legacy-l1-1-1.dll",
        "pwsh",
    ),
    (
        "pwsh",
        "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-libraryloader-l1-1-0.dll",
        "pwsh",
    ),
    (
        "pwsh",
        "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-localization-l1-2-0.dll",
        "pwsh",
    ),
    ("pwsh", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-memory-l1-1-0.dll", "pwsh"),
    ("pwsh", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-namedpipe-l1-1-0.dll", "pwsh"),
    (
        "pwsh",
        "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-processenvironment-l1-1-0.dll",
        "pwsh",
    ),
    (
        "pwsh",
        "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-processthreads-l1-1-0.dll",
        "pwsh",
    ),
    (
        "pwsh",
        "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-processthreads-l1-1-1.dll",
        "pwsh",
    ),
    ("pwsh", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-profile-l1-1-0.dll", "pwsh"),
    ("pwsh", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-rtlsupport-l1-1-0.dll", "pwsh"),
    ("pwsh", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-string-l1-1-0.dll", "pwsh"),
    ("pwsh", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-synch-l1-1-0.dll", "pwsh"),
    ("pwsh", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-synch-l1-2-0.dll", "pwsh"),
    ("pwsh", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-sysinfo-l1-1-0.dll", "pwsh"),
    ("pwsh", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-sysinfo-l1-2-0.dll", "pwsh"),
    ("pwsh", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-timezone-l1-1-0.dll", "pwsh"),
    ("pwsh", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-util-l1-1-0.dll", "pwsh"),
    ("pwsh", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-crt-conio-l1-1-0.dll", "pwsh"),
    ("pwsh", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-crt-convert-l1-1-0.dll", "pwsh"),
    ("pwsh", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-crt-environment-l1-1-0.dll", "pwsh"),
    ("pwsh", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-crt-filesystem-l1-1-0.dll", "pwsh"),
    ("pwsh", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-crt-heap-l1-1-0.dll", "pwsh"),
    ("pwsh", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-crt-locale-l1-1-0.dll", "pwsh"),
    ("pwsh", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-crt-math-l1-1-0.dll", "pwsh"),
    ("pwsh", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-crt-multibyte-l1-1-0.dll", "pwsh"),
    ("pwsh", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-crt-private-l1-1-0.dll", "pwsh"),
    ("pwsh", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-crt-process-l1-1-0.dll", "pwsh"),
    ("pwsh", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-crt-runtime-l1-1-0.dll", "pwsh"),
    ("pwsh", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-crt-stdio-l1-1-0.dll", "pwsh"),
    ("pwsh", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-crt-string-l1-1-0.dll", "pwsh"),
    ("pwsh", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-crt-time-l1-1-0.dll", "pwsh"),
    ("pwsh", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-crt-utility-l1-1-0.dll", "pwsh"),
    ("pwsh", "target/prebuilt/jdk/jdk-21.0.5+11/bin/ucrtbase.dll", "pwsh"),
    // BR stage 1: headless Chromium (PREBUILT) with every gala DLL its
    // imports and delay imports name (delay-loaded ones load on first call).
    ("chromium", "target/prebuilt/chromium/chrome-win/", "chromium"),
    ("chromium", "user/chromium/page.html", "gala"),
    ("chromium", "user/chromium/window.html", "gala"),
    ("chromium", "user/chromium/window2.html", "gala"),
    ("chromium_uia", "user/bin/uia_chromium_test.exe", "gala"),
    (
        "chromium_driver",
        "target/prebuilt/chromedriver/chromedriver_win32/chromedriver.exe",
        "chromium",
    ),
    ("chromium", "ntdll.dll", "Windows/System32"),
    ("chromium", "kernel32.dll", "Windows/System32"),
    ("chromium", "advapi32.dll", "Windows/System32"),
    ("chromium", "bcrypt.dll", "Windows/System32"),
    ("chromium", "bcryptprimitives.dll", "Windows/System32"),
    ("chromium", "crypt32.dll", "Windows/System32"),
    ("chromium", "msvcrt.dll", "Windows/System32"),
    ("chromium", "secur32.dll", "Windows/System32"),
    ("chromium", "shell32.dll", "Windows/System32"),
    ("chromium", "user32.dll", "Windows/System32"),
    ("chromium", "gdi32.dll", "Windows/System32"),
    ("chromium", "comctl32.dll", "Windows/System32"),
    ("chromium", "ws2_32.dll", "Windows/System32"),
    ("chromium", "mswsock.dll", "Windows/System32"),
    ("chromium", "userenv.dll", "Windows/System32"),
    ("chromium", "shlwapi.dll", "Windows/System32"),
    ("chromium", "ncrypt.dll", "Windows/System32"),
    ("chromium", "ole32.dll", "Windows/System32"),
    ("chromium", "oleaut32.dll", "Windows/System32"),
    ("chromium", "iphlpapi.dll", "Windows/System32"),
    ("chromium", "version.dll", "Windows/System32"),
    ("chromium", "rpcrt4.dll", "Windows/System32"),
    ("chromium", "netapi32.dll", "Windows/System32"),
    ("chromium", "powrprof.dll", "Windows/System32"),
    ("chromium", "winmm.dll", "Windows/System32"),
    ("chromium", "uxtheme.dll", "Windows/System32"),
    ("chromium", "comdlg32.dll", "Windows/System32"),
    ("chromium", "oleacc.dll", "Windows/System32"),
    ("chromium", "dbghelp.dll", "Windows/System32"),
    ("chromium", "winhttp.dll", "Windows/System32"),
    ("chromium", "wininet.dll", "Windows/System32"),
    ("chromium", "urlmon.dll", "Windows/System32"),
    ("chromium", "wintrust.dll", "Windows/System32"),
    ("chromium", "pdh.dll", "Windows/System32"),
    ("chromium", "setupapi.dll", "Windows/System32"),
    ("chromium", "dhcpcsvc.dll", "Windows/System32"),
    ("chromium", "uiautomationcore.dll", "Windows/System32"),
    ("chromium", "dwrite.dll", "Windows/System32"),
    ("chromium", "shcore.dll", "Windows/System32"),
    ("chromium", "dwmapi.dll", "Windows/System32"),
    ("chromium", "dxgi.dll", "Windows/System32"),
    ("chromium", "d3d11.dll", "Windows/System32"),
    ("chromium", "d3d12.dll", "Windows/System32"),
    ("chromium", "dcomp.dll", "Windows/System32"),
    ("chromium", "wtsapi32.dll", "Windows/System32"),
    ("chromium", "imm32.dll", "Windows/System32"),
    ("dotnet", "target/prebuilt/dotnet/", "dotnet"),
    ("dotnet", "user/bin/dotnet_hello.dll", "gala"),
    ("dotnet", "user/dotnet_hello/dotnet_hello.runtimeconfig.json", "gala"),
    // flaui_test: FlaUI (PREBUILT) driving flaui_target.exe through gala's
    // UIAutomationCore.dll.
    ("dotnet", "uiautomationcore.dll", "Windows/System32"),
    ("dotnet", "gdi32.dll", "Windows/System32"),
    ("dotnet", "comctl32.dll", "Windows/System32"),
    ("dotnet", "user/bin/flaui_test.dll", "gala"),
    ("dotnet", "user/flaui_test/flaui_test.runtimeconfig.json", "gala"),
    ("dotnet", "user/bin/flaui_target.exe", "gala"),
    // filezilla_flaui's test (with the filezilla set: MODULE_SETS).
    ("dotnet", "user/bin/filezilla_flaui.dll", "gala"),
    ("dotnet", "user/flaui_test/filezilla_flaui.runtimeconfig.json", "gala"),
    ("dotnet", "target/prebuilt/flaui-core/lib/net8.0-windows7.0/FlaUI.Core.dll", "gala"),
    ("dotnet", "target/prebuilt/flaui-uia3/lib/net8.0-windows7.0/FlaUI.UIA3.dll", "gala"),
    (
        "dotnet",
        "target/prebuilt/uiautomation-interop/lib/netcoreapp3.0/Interop.UIAutomationClient.dll",
        "gala",
    ),
    (
        "dotnet",
        "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-console-l1-1-0.dll",
        "dotnet",
    ),
    (
        "dotnet",
        "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-console-l1-2-0.dll",
        "dotnet",
    ),
    (
        "dotnet",
        "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-datetime-l1-1-0.dll",
        "dotnet",
    ),
    ("dotnet", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-debug-l1-1-0.dll", "dotnet"),
    (
        "dotnet",
        "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-errorhandling-l1-1-0.dll",
        "dotnet",
    ),
    ("dotnet", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-fibers-l1-1-0.dll", "dotnet"),
    ("dotnet", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-fibers-l1-1-1.dll", "dotnet"),
    ("dotnet", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-file-l1-1-0.dll", "dotnet"),
    ("dotnet", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-file-l1-2-0.dll", "dotnet"),
    ("dotnet", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-file-l2-1-0.dll", "dotnet"),
    ("dotnet", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-handle-l1-1-0.dll", "dotnet"),
    ("dotnet", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-heap-l1-1-0.dll", "dotnet"),
    (
        "dotnet",
        "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-interlocked-l1-1-0.dll",
        "dotnet",
    ),
    (
        "dotnet",
        "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-kernel32-legacy-l1-1-1.dll",
        "dotnet",
    ),
    (
        "dotnet",
        "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-libraryloader-l1-1-0.dll",
        "dotnet",
    ),
    (
        "dotnet",
        "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-localization-l1-2-0.dll",
        "dotnet",
    ),
    ("dotnet", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-memory-l1-1-0.dll", "dotnet"),
    (
        "dotnet",
        "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-namedpipe-l1-1-0.dll",
        "dotnet",
    ),
    (
        "dotnet",
        "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-processenvironment-l1-1-0.dll",
        "dotnet",
    ),
    (
        "dotnet",
        "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-processthreads-l1-1-0.dll",
        "dotnet",
    ),
    (
        "dotnet",
        "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-processthreads-l1-1-1.dll",
        "dotnet",
    ),
    (
        "dotnet",
        "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-profile-l1-1-0.dll",
        "dotnet",
    ),
    (
        "dotnet",
        "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-rtlsupport-l1-1-0.dll",
        "dotnet",
    ),
    ("dotnet", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-string-l1-1-0.dll", "dotnet"),
    ("dotnet", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-synch-l1-1-0.dll", "dotnet"),
    ("dotnet", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-synch-l1-2-0.dll", "dotnet"),
    (
        "dotnet",
        "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-sysinfo-l1-1-0.dll",
        "dotnet",
    ),
    (
        "dotnet",
        "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-sysinfo-l1-2-0.dll",
        "dotnet",
    ),
    (
        "dotnet",
        "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-timezone-l1-1-0.dll",
        "dotnet",
    ),
    ("dotnet", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-core-util-l1-1-0.dll", "dotnet"),
    ("dotnet", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-crt-conio-l1-1-0.dll", "dotnet"),
    ("dotnet", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-crt-convert-l1-1-0.dll", "dotnet"),
    (
        "dotnet",
        "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-crt-environment-l1-1-0.dll",
        "dotnet",
    ),
    (
        "dotnet",
        "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-crt-filesystem-l1-1-0.dll",
        "dotnet",
    ),
    ("dotnet", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-crt-heap-l1-1-0.dll", "dotnet"),
    ("dotnet", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-crt-locale-l1-1-0.dll", "dotnet"),
    ("dotnet", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-crt-math-l1-1-0.dll", "dotnet"),
    (
        "dotnet",
        "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-crt-multibyte-l1-1-0.dll",
        "dotnet",
    ),
    ("dotnet", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-crt-private-l1-1-0.dll", "dotnet"),
    ("dotnet", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-crt-process-l1-1-0.dll", "dotnet"),
    ("dotnet", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-crt-runtime-l1-1-0.dll", "dotnet"),
    ("dotnet", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-crt-stdio-l1-1-0.dll", "dotnet"),
    ("dotnet", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-crt-string-l1-1-0.dll", "dotnet"),
    ("dotnet", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-crt-time-l1-1-0.dll", "dotnet"),
    ("dotnet", "target/prebuilt/jdk/jdk-21.0.5+11/bin/api-ms-win-crt-utility-l1-1-0.dll", "dotnet"),
    ("dotnet", "target/prebuilt/jdk/jdk-21.0.5+11/bin/ucrtbase.dll", "dotnet"),
    // C++ exceptions with Microsoft's C++ runtime (the JDK's
    // redistributable copies, app-local).
    ("cxx_eh_test", "ntdll.dll", "Windows/System32"),
    ("cxx_eh_test", "kernel32.dll", "Windows/System32"),
    ("cxx_eh_test", "user/bin/cxx_eh_test.exe", "gala"),
    ("cxx_eh_test", "target/prebuilt/jdk/jdk-21.0.5+11/bin/vcruntime140.dll", "gala"),
    ("cxx_eh_test", "target/prebuilt/jdk/jdk-21.0.5+11/bin/ucrtbase.dll", "gala"),
    ("java_hello", "ntdll.dll", "Windows/System32"),
    ("java_hello", "kernel32.dll", "Windows/System32"),
    ("java_hello", "advapi32.dll", "Windows/System32"),
    ("java_hello", "winmm.dll", "Windows/System32"),
    ("java_hello", "version.dll", "Windows/System32"),
    ("java_hello", "shell32.dll", "Windows/System32"),
    ("java_hello", "powrprof.dll", "Windows/System32"),
    ("java_hello", "comctl32.dll", "Windows/System32"),
    ("java_hello", "ncrypt.dll", "Windows/System32"),
    ("java_hello", "crypt32.dll", "Windows/System32"),
    ("java_hello", "ws2_32.dll", "Windows/System32"),
    ("java_hello", "mswsock.dll", "Windows/System32"),
    ("java_hello", "bcryptprimitives.dll", "Windows/System32"),
    ("java_hello", "user32.dll", "Windows/System32"),
    ("java_hello", "iphlpapi.dll", "Windows/System32"),
    ("java_hello", "secur32.dll", "Windows/System32"),
    ("java_hello", "winhttp.dll", "Windows/System32"),
    ("java_hello", "msvcrt.dll", "Windows/System32"),
    ("java_hello", "target/prebuilt/jdk/jdk-21.0.5+11/", "jdk"),
    ("java_hello", "user/java/Hello.java", "gala"),
    ("java_http", "ntdll.dll", "Windows/System32"),
    ("java_http", "kernel32.dll", "Windows/System32"),
    ("java_http", "advapi32.dll", "Windows/System32"),
    ("java_http", "winmm.dll", "Windows/System32"),
    ("java_http", "version.dll", "Windows/System32"),
    ("java_http", "shell32.dll", "Windows/System32"),
    ("java_http", "powrprof.dll", "Windows/System32"),
    ("java_http", "comctl32.dll", "Windows/System32"),
    ("java_http", "ncrypt.dll", "Windows/System32"),
    ("java_http", "crypt32.dll", "Windows/System32"),
    ("java_http", "ws2_32.dll", "Windows/System32"),
    ("java_http", "mswsock.dll", "Windows/System32"),
    ("java_http", "bcryptprimitives.dll", "Windows/System32"),
    ("java_http", "user32.dll", "Windows/System32"),
    ("java_http", "iphlpapi.dll", "Windows/System32"),
    ("java_http", "secur32.dll", "Windows/System32"),
    ("java_http", "winhttp.dll", "Windows/System32"),
    ("java_http", "msvcrt.dll", "Windows/System32"),
    ("java_http", "target/prebuilt/jdk/jdk-21.0.5+11/", "jdk"),
    ("java_http", "user/java/Http.java", "gala"),
    ("java_stress", "ntdll.dll", "Windows/System32"),
    ("java_stress", "kernel32.dll", "Windows/System32"),
    ("java_stress", "advapi32.dll", "Windows/System32"),
    ("java_stress", "winmm.dll", "Windows/System32"),
    ("java_stress", "version.dll", "Windows/System32"),
    ("java_stress", "shell32.dll", "Windows/System32"),
    ("java_stress", "powrprof.dll", "Windows/System32"),
    ("java_stress", "comctl32.dll", "Windows/System32"),
    ("java_stress", "ncrypt.dll", "Windows/System32"),
    ("java_stress", "crypt32.dll", "Windows/System32"),
    ("java_stress", "ws2_32.dll", "Windows/System32"),
    ("java_stress", "mswsock.dll", "Windows/System32"),
    ("java_stress", "bcryptprimitives.dll", "Windows/System32"),
    ("java_stress", "user32.dll", "Windows/System32"),
    ("java_stress", "iphlpapi.dll", "Windows/System32"),
    ("java_stress", "secur32.dll", "Windows/System32"),
    ("java_stress", "winhttp.dll", "Windows/System32"),
    ("java_stress", "msvcrt.dll", "Windows/System32"),
    ("java_stress", "target/prebuilt/jdk/jdk-21.0.5+11/", "jdk"),
    ("java_stress", "user/java/Http.java", "gala"),
    ("java_stress", "user/java/Stress.java", "gala"),
    ("lua", "ntdll.dll", "Windows/System32"),
    ("lua", "kernel32.dll", "Windows/System32"),
    ("lua", "ws2_32.dll", "Windows/System32"),
    ("lua", "mswsock.dll", "Windows/System32"),
    ("lua", "msvcrt.dll", "Windows/System32"),
    ("lua", "advapi32.dll", "Windows/System32"),
    ("lua", "user32.dll", "Windows/System32"),
    ("lua", "bcryptprimitives.dll", "Windows/System32"),
    ("lua", "user/bin/busybox.exe", "gala"),
    ("lua", "user/bin/tcc.tar", "gala"),
    ("lua", "user/bin/lua.tar", "gala"),
    ("lua", "user/lua/lua.sh", "gala"),
    ("lua", "user/lua/test.lua", "gala"),
    ("sqlite", "ntdll.dll", "Windows/System32"),
    ("sqlite", "kernel32.dll", "Windows/System32"),
    ("sqlite", "ws2_32.dll", "Windows/System32"),
    ("sqlite", "mswsock.dll", "Windows/System32"),
    ("sqlite", "msvcrt.dll", "Windows/System32"),
    ("sqlite", "advapi32.dll", "Windows/System32"),
    ("sqlite", "user32.dll", "Windows/System32"),
    ("sqlite", "bcryptprimitives.dll", "Windows/System32"),
    ("sqlite", "user/bin/busybox.exe", "gala"),
    ("sqlite", "user/bin/tcc.tar", "gala"),
    ("sqlite", "user/bin/sqlite-amalgamation.zip", "gala"),
    ("sqlite", "user/sqlite/sqlite.sh", "gala"),
    ("sqlite", "user/sqlite/sqlrun.c", "gala"),
    ("sqlite", "user/sqlite/tcc_win.h", "gala"),
    ("sqlite", "user/sqlite/test.sql", "gala"),
    ("sqlite", "user/sqlite/reopen.sql", "gala"),
    ("sqlite", "user/sqlite/writer.sql", "gala"),
    ("sqlite", "user/sqlite/concurrent.sql", "gala"),
    ("nginx", "user/nginx/conf/nginx.conf", "nginx/conf"),
    ("nginx", "user/nginx/html/index.html", "nginx/html"),
    ("nginx", "user/nginx/logs/keep-logs.txt", "nginx/logs"),
    ("nginx", "user/nginx/temp/keep-temp.txt", "nginx/temp"),
    ("nginx_master", "ntdll.dll", "Windows/System32"),
    ("nginx_master", "kernel32.dll", "Windows/System32"),
    ("nginx_master", "ws2_32.dll", "Windows/System32"),
    ("nginx_master", "mswsock.dll", "Windows/System32"),
    ("nginx_master", "msvcrt.dll", "Windows/System32"),
    ("nginx_master", "advapi32.dll", "Windows/System32"),
    ("nginx_master", "user32.dll", "Windows/System32"),
    ("nginx_master", "crypt32.dll", "Windows/System32"),
    ("nginx_master", "bcryptprimitives.dll", "Windows/System32"),
    ("nginx_master", "user/bin/nginx.exe", "nginx"),
    ("nginx_master", "user/nginx/conf/nginx-master.conf", "nginx/conf"),
    ("nginx_master", "user/nginx/html/index.html", "nginx/html"),
    ("nginx_master", "user/nginx/logs/keep-logs.txt", "nginx/logs"),
    ("nginx_master", "user/nginx/temp/keep-temp.txt", "nginx/temp"),
    ("nginx_tls", "ntdll.dll", "Windows/System32"),
    ("nginx_tls", "kernel32.dll", "Windows/System32"),
    ("nginx_tls", "ws2_32.dll", "Windows/System32"),
    ("nginx_tls", "mswsock.dll", "Windows/System32"),
    ("nginx_tls", "msvcrt.dll", "Windows/System32"),
    ("nginx_tls", "advapi32.dll", "Windows/System32"),
    ("nginx_tls", "user32.dll", "Windows/System32"),
    ("nginx_tls", "bcryptprimitives.dll", "Windows/System32"),
    ("nginx_tls", "crypt32.dll", "Windows/System32"),
    ("nginx_tls", "user/bin/nginx.exe", "nginx"),
    ("nginx_tls", "user/nginx/conf/nginx-tls.conf", "nginx/conf"),
    ("nginx_tls", "user/nginx/conf/gala-test.crt", "nginx/conf"),
    ("nginx_tls", "user/nginx/conf/gala-test.key", "nginx/conf"),
    ("nginx_tls", "user/nginx/html/index.html", "nginx/html"),
    ("nginx_tls", "user/nginx/logs/keep-logs.txt", "nginx/logs"),
    ("nginx_tls", "user/nginx/temp/keep-temp.txt", "nginx/temp"),
    // Schannel (TLS) against Nginx on the loopback address; the test CA
    // in gala's machine root store.
    ("schannel_test", "ntdll.dll", "Windows/System32"),
    ("schannel_test", "kernel32.dll", "Windows/System32"),
    ("schannel_test", "ws2_32.dll", "Windows/System32"),
    ("schannel_test", "mswsock.dll", "Windows/System32"),
    ("schannel_test", "msvcrt.dll", "Windows/System32"),
    ("schannel_test", "advapi32.dll", "Windows/System32"),
    ("schannel_test", "user32.dll", "Windows/System32"),
    ("schannel_test", "bcryptprimitives.dll", "Windows/System32"),
    ("schannel_test", "bcrypt.dll", "Windows/System32"),
    ("schannel_test", "crypt32.dll", "Windows/System32"),
    ("schannel_test", "secur32.dll", "Windows/System32"),
    ("schannel_test", "schannel.dll", "Windows/System32"),
    ("schannel_test", "ncrypt.dll", "Windows/System32"),
    ("schannel_test", "user/bin/nginx.exe", "nginx"),
    ("schannel_test", "user/schannel_test/nginx.conf", "nginx/conf"),
    ("schannel_test", "user/schannel_test/certs/server.crt", "nginx/conf"),
    ("schannel_test", "user/schannel_test/certs/server.key", "nginx/conf"),
    ("schannel_test", "user/schannel_test/certs/untrusted.crt", "nginx/conf"),
    ("schannel_test", "user/schannel_test/certs/untrusted.key", "nginx/conf"),
    ("schannel_test", "user/schannel_test/certs/expired.crt", "nginx/conf"),
    ("schannel_test", "user/schannel_test/certs/expired.key", "nginx/conf"),
    // The CA Nginx checks client certificates against (port 8447).
    ("schannel_test", "user/schannel_test/certs/ca.crt", "nginx/conf"),
    ("schannel_test", "user/schannel_test/certs/ca.cer", "Windows/System32/gala/roots"),
    // Revocation: certificates naming the test CA's CRL (8448 revoked,
    // 8449 not), the CRL in gala's CA store (crypt32 checks offline) and
    // at its distribution point.
    ("schannel_test", "user/schannel_test/certs/revoked.crt", "nginx/conf"),
    ("schannel_test", "user/schannel_test/certs/revoked.key", "nginx/conf"),
    ("schannel_test", "user/schannel_test/certs/notrevoked.crt", "nginx/conf"),
    ("schannel_test", "user/schannel_test/certs/notrevoked.key", "nginx/conf"),
    ("schannel_test", "user/schannel_test/certs/ca.crl", "Windows/System32/gala/ca"),
    ("schannel_test", "user/schannel_test/certs/ca.crl", "nginx/html"),
    ("schannel_test", "user/nginx/html/index.html", "nginx/html"),
    ("schannel_test", "user/nginx/logs/keep-logs.txt", "nginx/logs"),
    ("schannel_test", "user/nginx/temp/keep-temp.txt", "nginx/temp"),
    // curl for Windows, run by busybox sh (user/curl/curl.sh) against
    // schannel_test's Nginx (MODULE_SETS): curl.se's build (PREBUILT
    // curl; the UCRT is the JDK's redistributable ucrtbase.dll, in
    // System32 as on Windows) and Git for Windows' Schannel curl (PREBUILT
    // gitcurl), each as C:\curl.
    ("curl", "target/prebuilt/curl/curl-8.23.0_8-win64-mingw/bin/curl.exe", "curl"),
    ("curl", "target/prebuilt/jdk/jdk-21.0.5+11/bin/ucrtbase.dll", "Windows/System32"),
    ("curl", "normaliz.dll", "Windows/System32"),
    ("curl_common", "iphlpapi.dll", "Windows/System32"),
    ("curl_schannel", "target/prebuilt/gitcurl/mingw64/bin/", "curl"),
    ("curl_common", "wldap32.dll", "Windows/System32"),
    ("curl_common", "user/bin/busybox.exe", "gala"),
    ("curl_common", "user/curl/curl.sh", "gala"),
    // The CA files of curl.sh's --cacert steps.
    ("curl_common", "user/schannel_test/certs/ca.crt", "gala"),
    ("curl_common", "user/schannel_test/certs/untrusted.crt", "gala"),
    // Git's https remotes (MinGit, MODULE_SETS) against the same Nginx,
    // which serves user/git_https/repo.git; busybox runs git.sh.
    ("git_https", "user/git_https/repo.git/", "nginx/html/repo.git"),
    ("git_https", "user/bin/busybox.exe", "gala"),
    ("git_https", "wldap32.dll", "Windows/System32"),
    ("git_https", "iphlpapi.dll", "Windows/System32"),
    ("git_https", "user/git_https/git.sh", "gala"),
    ("go_exit", "ntdll.dll", "Windows/System32"),
    ("go_exit", "kernel32.dll", "Windows/System32"),
    ("go_exit", "bcryptprimitives.dll", "Windows/System32"),
    // WOW64: gala's 64-bit side of 32-bit processes.
    ("wow64_test", "wow64.dll", "Windows/System32"),
    ("wow64_crash", "wow64.dll", "Windows/System32"),
    ("wow64_thread_test", "wow64.dll", "Windows/System32"),
    // A 32-bit program with imports: the 32-bit ntdll's loader.
    ("wow64_import_test", "wow64.dll", "Windows/System32"),
    ("wow64_import_test", "x86/ntdll.dll", "Windows/SysWOW64"),
    ("wow64_import_test", "x86/testdll32.dll", "gala"),
    // Exceptions and APCs in 32-bit code: the 32-bit ntdll's dispatchers.
    ("wow64_seh_test", "wow64.dll", "Windows/System32"),
    ("wow64_seh_test", "x86/ntdll.dll", "Windows/SysWOW64"),
    ("wow64_seh_crash", "wow64.dll", "Windows/System32"),
    ("wow64_seh_crash", "x86/ntdll.dll", "Windows/SysWOW64"),
    // The file system redirector: both ntdlls (told apart by their
    // machine), a hosts file and markers in the exempt directories.
    ("wow64_fs_test", "wow64.dll", "Windows/System32"),
    ("wow64_fs_test", "x86/ntdll.dll", "Windows/SysWOW64"),
    ("wow64_fs_test", "ntdll.dll", "Windows/System32"),
    ("wow64_fs_test", "user/wow64_fs_test/hosts", "Windows/System32/drivers/etc"),
    ("wow64_fs_test", "user/wow64_fs_test/exempt.txt", "Windows/System32/catroot"),
    ("wow64_fs_test", "user/wow64_fs_test/exempt.txt", "Windows/System32/catroot2"),
    ("wow64_fs_test", "user/wow64_fs_test/exempt.txt", "Windows/System32/DriverStore"),
    ("wow64_fs_test", "user/wow64_fs_test/exempt.txt", "Windows/System32/LogFiles"),
    ("wow64_fs_test", "user/wow64_fs_test/exempt.txt", "Windows/System32/spool"),
    // A plain 32-bit program on the 32-bit kernel32.
    ("wow64_k32_test", "wow64.dll", "Windows/System32"),
    ("wow64_k32_test", "x86/ntdll.dll", "Windows/SysWOW64"),
    ("wow64_k32_test", "x86/kernel32.dll", "Windows/SysWOW64"),
    // Its redirector checks: the 64-bit ntdll beside the 32-bit one, the
    // exempt hosts file, the 64-bit cmd it starts through Sysnative.
    ("wow64_k32_test", "ntdll.dll", "Windows/System32"),
    ("wow64_k32_test", "kernel32.dll", "Windows/System32"),
    ("wow64_k32_test", "bcryptprimitives.dll", "Windows/System32"),
    ("wow64_k32_test", "user/bin/cmd.exe", "Windows/System32"),
    ("wow64_k32_test", "user/wow64_fs_test/hosts", "Windows/System32/drivers/etc"),
    // A 32-bit C program (clang-built, checked-in binary) on it.
    ("wow64_c_test", "wow64.dll", "Windows/System32"),
    ("wow64_c_test", "x86/ntdll.dll", "Windows/SysWOW64"),
    ("wow64_c_test", "x86/kernel32.dll", "Windows/SysWOW64"),
    ("wow64_c_test", "user/bin/wow64_c_test.exe", "gala"),
    // A 32-bit C program on the 32-bit msvcrt (mingw-built, checked-in).
    ("wow64_crt_test", "wow64.dll", "Windows/System32"),
    ("wow64_crt_test", "x86/ntdll.dll", "Windows/SysWOW64"),
    ("wow64_crt_test", "x86/kernel32.dll", "Windows/SysWOW64"),
    ("wow64_crt_test", "x86/msvcrt.dll", "Windows/SysWOW64"),
    ("wow64_crt_test", "user/bin/wow64_crt_test.exe", "gala"),
    ("wow64_gui_test", "wow64.dll", "Windows/System32"),
    ("wow64_gui_test", "x86/ntdll.dll", "Windows/SysWOW64"),
    ("wow64_gui_test", "x86/kernel32.dll", "Windows/SysWOW64"),
    ("wow64_gui_test", "x86/msvcrt.dll", "Windows/SysWOW64"),
    ("wow64_gui_test", "x86/user32.dll", "Windows/SysWOW64"),
    ("wow64_gui_test", "x86/gdi32.dll", "Windows/SysWOW64"),
    ("wow64_gui_test", "user/bin/wow64_gui_test.exe", "gala"),
    // wow64_sock_test: a 32-bit Winsock program and the 32-bit ws2_32.
    ("wow64_sock_test", "wow64.dll", "Windows/System32"),
    ("wow64_sock_test", "x86/ntdll.dll", "Windows/SysWOW64"),
    ("wow64_sock_test", "x86/kernel32.dll", "Windows/SysWOW64"),
    ("wow64_sock_test", "x86/msvcrt.dll", "Windows/SysWOW64"),
    ("wow64_sock_test", "x86/ws2_32.dll", "Windows/SysWOW64"),
    ("wow64_sock_test", "user/bin/wow64_sock_test.exe", "gala"),
    // pk_install_inno32: the x86 Inno Setup installer in the snapshot's
    // mirror (with pk_install_test's set and wow64_inno's 32-bit DLLs:
    // MODULE_SETS).
    ("pk_install_inno32", "target/prebuilt/innosetup6/innosetup-6.5.4.exe", "pk_source/installers"),
    // wow64_inno: an x86 Inno Setup installer (PREBUILT innosetup6) and the
    // 32-bit DLLs its Setup loads.
    ("wow64_inno", "wow64.dll", "Windows/System32"),
    ("wow64_inno", "x86/ntdll.dll", "Windows/SysWOW64"),
    ("wow64_inno", "x86/kernel32.dll", "Windows/SysWOW64"),
    ("wow64_inno", "x86/msvcrt.dll", "Windows/SysWOW64"),
    ("wow64_inno", "x86/user32.dll", "Windows/SysWOW64"),
    ("wow64_inno", "x86/gdi32.dll", "Windows/SysWOW64"),
    ("wow64_inno", "x86/advapi32.dll", "Windows/SysWOW64"),
    ("wow64_inno", "x86/bcryptprimitives.dll", "Windows/SysWOW64"),
    ("wow64_inno", "x86/comctl32.dll", "Windows/SysWOW64"),
    ("wow64_inno", "x86/oleaut32.dll", "Windows/SysWOW64"),
    ("wow64_inno", "x86/ole32.dll", "Windows/SysWOW64"),
    ("wow64_inno", "x86/shell32.dll", "Windows/SysWOW64"),
    ("wow64_inno", "x86/shlwapi.dll", "Windows/SysWOW64"),
    ("wow64_inno", "x86/version.dll", "Windows/SysWOW64"),
    ("wow64_inno", "x86/mpr.dll", "Windows/SysWOW64"),
    ("wow64_inno", "x86/comdlg32.dll", "Windows/SysWOW64"),
    ("wow64_inno", "x86/uxtheme.dll", "Windows/SysWOW64"),
    ("wow64_inno", "x86/imm32.dll", "Windows/SysWOW64"),
    ("wow64_inno", "x86/dwmapi.dll", "Windows/SysWOW64"),
    ("wow64_inno", "x86/msimg32.dll", "Windows/SysWOW64"),
    ("wow64_inno", "x86/msctf.dll", "Windows/SysWOW64"),
    ("wow64_inno", "x86/winspool.drv", "Windows/SysWOW64"),
    ("wow64_inno", "x86/wtsapi32.dll", "Windows/SysWOW64"),
    ("wow64_inno", "x86/winhttp.dll", "Windows/SysWOW64"),
    ("wow64_inno", "x86/ws2_32.dll", "Windows/SysWOW64"),
    ("wow64_inno", "x86/secur32.dll", "Windows/SysWOW64"),
    ("wow64_inno", "x86/shfolder.dll", "Windows/SysWOW64"),
    ("wow64_inno", "x86/bcrypt.dll", "Windows/SysWOW64"),
    ("wow64_inno", "target/prebuilt/innosetup6/innosetup-6.5.4.exe", "gala"),
    // A 32-bit program on the 32-bit advapi32.
    ("wow64_adv_test", "wow64.dll", "Windows/System32"),
    ("wow64_adv_test", "x86/ntdll.dll", "Windows/SysWOW64"),
    ("wow64_adv_test", "x86/kernel32.dll", "Windows/SysWOW64"),
    ("wow64_adv_test", "x86/advapi32.dll", "Windows/SysWOW64"),
    ("wow64_adv_test", "x86/bcryptprimitives.dll", "Windows/SysWOW64"),
];
/// Go programs in `user/<name>/`, built for windows/amd64 with this Go
/// toolchain (when `go` is installed; otherwise the checked-in binary in
/// user/bin is used). Reproducible: -trimpath and an empty build id.
const GO_PROGRAMS: &[&str] =
    &["go_hello", "go_goroutines", "go_time", "go_gc", "go_os", "go_exit", "go_http", "go_exec"];
const GO_TOOLCHAIN: &str = "go1.24.7";

const USER_PROGRAMS: &[&str] = &[
    "hello",
    "abi_test",
    "crash",
    "vm_test",
    "wait_test",
    "object_test",
    "thread_test",
    "kuser_test",
    "section_test",
    "file_test",
    "image_test",
    "dir_test",
    "peb_test",
    "startup_test",
    "exception_test",
    "seh_test",
    "guard_test",
    "import_test",
    "args_test",
    "fpu_test",
    "sysinfo_test",
    "threadinfo_test",
    "filemap_test",
    "cow_test",
    "async_test",
    "apc_test",
    "ioapc_test",
    "sync_test",
    "symlink_test",
    "iocp_test",
    "context_test",
    "timer_test",
    "clock_test",
    "handover_test",
    "handover_k32_test",
    "handover_net_test",
    "loader_test",
    "smp_test",
    "smpsched_test",
    "smpmem_test",
    "smpcow_test",
    "vmcycle_test",
    "xmodify_test",
    "smpsem_test",
    "smprace_test",
    "xstate_test",
    "straddle_test",
    "statedump_test",
    "clockcal_test",
    "safepoint_test",
    "shootdown_test",
    "ledger_test",
    "bench",
    "kernel32_test",
    "missing_import_test",
    "unimplemented_test",
    "rpcrt4_test",
    "netapi32_test",
    "pdh_test",
    "setupapi_test",
    "clusapi_test",
    "sspi_test",
    "schannel_test",
    "dpapi_test",
    "cert_test",
    "tds_test",
    "kernel32_sys_test",
    "kernel32_thread_test",
    "kernel32_sync_test",
    "kernel32_py_test",
    "kernel32_locale_test",
    "disk_test",
    "rtl_test",
    "gui_test",
    "conhost",
    "console_window_test",
    "conpty_test",
    "browser_api_test",
    "dwrite_test",
    "gpu_stub_test",
    "persist_test",
    "kernel32_system_test",
    "kernel32_clr_test",
    "com_test",
    "bcrypt_test",
    "console_test",
    "advapi32_test",
    "token_test",
    "security_test",
    "access_test",
    "logon_test",
    "system_test",
    "session_test",
    "presenter_test",
    "winsta_test",
    "lsa_test",
    "restricted_test",
    "cmd",
    "cmd_test",
    "gala",
    "gala_cli_test",
    "pk_install_test",
    "profile_test",
    "registry_test",
    "keyed_event_test",
    "iphlpapi_test",
    "afd_poll_test",
    "dirchange_test",
    "win32_misc_test",
    "kernel32_pool_test",
    "tls_test",
    "manifest_test",
    "kernel32_misc_test",
    "prng_test",
    "replay_test",
    "kernel32_env_test",
    "winsock_test",
    "sshfwd_test",
    "hostnet_test",
    "ipv6_test",
    "winsock_async_test",
    "udp_test",
    "process_test",
    "commit_test",
    "quota_test",
    "kernel32_file_test",
    "pipe_test",
    "lock_test",
];

/// 32-bit test programs (`user/<name>`, built for USER_TARGET_X86, ntrt's
/// x86 runtime), shipped and exported like USER_PROGRAMS.
const USER_PROGRAMS_X86: &[&str] = &[
    "wow64_test",
    "wow64_thread_test",
    "wow64_import_test",
    "wow64_seh_test",
    "wow64_fs_test",
    "wow64_k32_test",
    "wow64_adv_test",
];

/// Where the build puts test program `program` (x64 or x86).
fn built_program(program: &str) -> PathBuf {
    let target = if USER_PROGRAMS_X86.contains(&program) { USER_TARGET_X86 } else { USER_TARGET };
    target_dir().join(target).join("release").join(format!("{program}.exe"))
}
/// Must match `BOOT_OK_MARKER` in `kernel/src/lib.rs`.
const BOOT_OK_MARKER: &str = "gala: boot ok";
/// QEMU exit statuses for `QemuExitCode::{Success, Failure}` (`(code << 1) | 1`).
const QEMU_EXIT_SUCCESS: i32 = 33;
const QEMU_EXIT_FAILURE: i32 = 35;
/// Logged by the kernel when a finished process's memory is still referenced.
const LEAK_MARKER: &str = "address space still referenced";
const BOOT_TEST_TIMEOUT: Duration = Duration::from_secs(120);

/// Scenarios that run whole language runtimes (a JVM, CoreCLR, CPython
/// running pip: python_pip's venv installs pip itself with ensurepip,
/// ~180 s of run time under TCG) or a whole database (PostgreSQL: initdb,
/// then the server's ~20 processes) under TCG: tens of seconds locally,
/// over twice that on a slow CI runner or under the full suite (postgres
/// took ~45 s alone and hit 120 s there). They get this much instead of
/// BOOT_TEST_TIMEOUT.
const SLOW_SCENARIOS: &[&str] = &[
    "java_version",
    "java_hello",
    "java_http",
    "java_stress",
    "dotnet_hello",
    "flaui_test",
    "filezilla_flaui",
    "filezilla_sftp",
    "pwsh_hello",
    "postgres",
    "filezilla",
    "python_pip",
    "python_pip_https",
    "python_numpy",
];
/// Memory for the Elasticsearch scenario's VM.
const ELASTICSEARCH_MEMORY_MIB: u64 = 3072;
/// Elasticsearch starts three JVMs and ~60 modules under TCG.
const ELASTICSEARCH_TIMEOUT: Duration = Duration::from_secs(900);
/// `chromium_wpt`: a directory of web-platform-tests, each a page load
/// and a harness run under TCG (a report run, PENDING: not in the gate).
const WPT_TIMEOUT: Duration = Duration::from_secs(4 * 3600);
const SLOW_TIMEOUT: Duration = Duration::from_secs(300);

/// Scenarios run under an artificial host load (clockcheck::HostLoad:
/// a busy thread on every host CPU) whose guest clock must agree with the
/// host's (`xtask: clock:` line, clockcheck.rs).
const HOST_LOAD_SCENARIOS: &[&str] = &["clockcal_test"];

/// How long scenario `name` may run: in the VM's run time (vmtime.rs:
/// the wall time less what its vCPUs waited for a host CPU), with a wall
/// ceiling of `vmtime::BACKSTOP` times as much.
fn scenario_timeout(name: &str) -> Duration {
    // `XTASK_TIMEOUT=<seconds>`: a hang's state dump without the wait.
    if let Some(secs) = env::var("XTASK_TIMEOUT").ok().and_then(|s| s.parse().ok()) {
        return Duration::from_secs(secs);
    }
    let limit = if SCENARIOS.iter().any(|s| s.name == name && s.exit_status == XTASK_DUMPS) {
        DUMP_SCENARIO_TIMEOUT
    } else if name == "chromium_wpt" {
        WPT_TIMEOUT
    } else if name == "elasticsearch"
        || name == "java_stress"
        || name.starts_with("sqllocaldb")
        || name.starts_with("chromium_")
    {
        ELASTICSEARCH_TIMEOUT
    } else if SLOW_SCENARIOS.contains(&name) {
        SLOW_TIMEOUT
    } else {
        BOOT_TEST_TIMEOUT
    };
    if limit == DUMP_SCENARIO_TIMEOUT {
        return limit;
    }
    // One host thread for both processors (SINGLE_THREAD_TCG) is about
    // half as fast: java_hello and java_http ran out of 300 s in a full
    // suite on a loaded 4-CPU host.
    // A loaded host needs nothing here: the limit counts the VM's run
    // time (under a load of 33 on 4 CPUs sshd_shell's guest had run 58 s
    // when 120 s of wall time were up, 2026-11-03).
    if single_thread_tcg(name) && !Path::new("/dev/kvm").exists() { limit * 2 } else { limit }
}

type Result<T = ()> = std::result::Result<T, String>;

const USAGE: &str = "\
usage: cargo xtask <command> [--release]

commands:
  build        build the kernel and a bootable UEFI ISO (target/<profile>/gala-os.iso)
  run          build, then boot in QEMU with serial on stdio [--headless]
  test         run host unit tests, then um-test (gala-um), replay-test, then the QEMU
               boot tests [--no-boot: all but the boot tests]
  boot-test    only the QEMU boot tests [scenario name filter, or exact names a,b,c]
               (XTASK_SHARD=i/n: the i-th of n shares of the VMs)
  export-tests copy the built test programs to user/bin/ (checked in)
  screenshot   boot and save PNG screenshots of the screen:
               screenshot [--uefi] [--at 1,3,5] [--name N] [kernel cmdline...]
               (default: the boot log, at 3 s; files in target/screenshots/)
  serve        boot a network scenario and keep it running, its port 7
               forwarded to 127.0.0.1:<port> (for a browser):
               serve <nginx|nginx_master|nginx_tls|go_http|...> [port, default 8080]
  um-test      run the program scenarios gala-um supports (runs_in_um) in gala-um,
               the NT layer as a Linux process (docs/GALA_UM.md)
  fault-test   run programs in gala-um with failures injected (failpoint=);
               each must end without a panic, abort or hang:
               fault-test [program filter] [--sweep[=frame|net_tx]: fail each heap
               allocation (frame allocation, packet) of each program in turn]
  sim-test     run multi-threaded programs in gala-um's deterministic simulation
               (um --sim=<seed>) under several seeds, each twice: expected output,
               the same trace for the same seed, different traces across seeds;
               a deadlock is reported with its seed:
               sim-test [program filter] [--seeds N (default 4)] [--seed S: only S]
               [--all: every gala-um program, not only the multi-threaded set]
  replay-test  record programs' inputs in gala-um (um --record=<file>) and replay
               them (um --replay=<file>), live and simulated: the replay must read
               what was recorded
  bench        run user/bench on gala; write target/report/bench.json and
               compare with Windows: bench [<bench-windows.txt from run-all.ps1>]
  lx-expect    run the Linux test programs (user/lx) on this Linux host and compare
               with their expected.txt: lx-expect [--record] [--no-build] [--runs N]
  alloc-audit  find infallible allocations in the kernel's machine code (calls that
               abort on out-of-memory: Vec::clone, collect, ...; clippy cannot see
               them): alloc-audit [kernel ELF] [--all: also growth after try_reserve]
  lint         the analyzers (event resets, wait sites) and cargo fmt --check; the
               pre-commit hook (hooks/, enabled by any xtask run) runs it;
               --full also runs CI's Clippy commands (the pre-push hook)
  feed         the update feed (TUF metadata over content-addressed files; R8):
               feed publish <files dir> <feed dir> [--release N] [--floor F] |
               feed refresh <feed dir> (re-sign snapshot and timestamp) |
               feed renew-root <feed dir> (root N+1, same keys, fresh expiry) |
               feed check <feed> [--over <source>] [--root <trusted root.json>] |
               feed pull <source> <empty dir> | feed push <feed dir> <s3://bucket | dir> |
               feed keygen (a new GALA_FEED_KEYS value on stdout; keys from it when set)
  release-tree the files of a release, as the feed publishes them (kernel, system DLLs,
               Limine, sealed config, manifest; no test programs): release-tree <dir>
  symbolize    resolve the kernel backtraces in a serial log to functions
               and source lines: symbolize <log> [--release]
  triage       what a (failed) boot's log says, in one block: verdict, guest CPUs and
               memory, warnings and errors, panics and faults, programs that did not
               end, failed processes, the ledger's top, the state dump's threads and
               CPUs, each thread's last system calls, the last lines:
               triage <serial log> [<events.jsonl>] (failed logs end with it)
  events       summarize an event stream (XTASK_EVENTS=...): system calls by time,
               failing statuses, each thread's last calls, faults, processes, images:
               events <events.jsonl>
  serve-page   answer one HTTP request on standard input/output with the
               host page (QEMU runs it per connection: HOST_PAGE_SCENARIOS)
  relay <port> one connection on standard input/output to 127.0.0.1:<port>
               (QEMU runs it per connection: chromium_wpt's wptserve)
  wpt-compare  chromium_wpt's report against wpt.fyi's Edge on Windows and
               Chrome on Linux (target/wpt/compare.txt)
  perfetto     a boot's event stream as a Perfetto trace (tracks per thread,
               system calls, IPC flows, samples): <events.jsonl | scenario>
               [--out FILE]; open it in ui.perfetto.dev
  profile      the sampling profiler's report: <events.jsonl | scenario> [--pid N]
               [--tid N] [--from S --to S] [--top N] [--symbols DIR] (boot
               with events=sample,image,thread,process; sample_hz=, default 100)
  otel         convert a boot's event stream and serial log to OpenTelemetry
               (OTLP/JSON files of traces, logs, metrics, posted as batched
               protobuf, OTEL_EXPORTER_OTLP_PROTOCOL; view in Jaeger or an OTel
               collector, docs/WORKFLOW.md, OpenTelemetry): otel <serial log |
               events.jsonl | scenario> [<events.jsonl>] [--out <prefix>]
               [--endpoint http://host:4318 (or OTEL_EXPORTER_OTLP_ENDPOINT)];
               XTASK_OTEL=1 makes boot-test write it for every boot
  docker-test  build the test image (docker/test.Dockerfile) and run
               `cargo xtask test` (or the given xtask arguments) inside it
  report       run every test program on gala; write target/report/gala.{json,md}
  compare      compare a report from Windows (user/bin/run-all.ps1) with gala's:
               compare <windows.json> [<gala.json>]
";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    lint::enable_hook(&root());
    let release = args.iter().any(|a| a == "--release");
    let headless = args.iter().any(|a| a == "--headless");
    let result =
        match args.first().map(String::as_str) {
            Some("build") => build(release, "gala-os", "").map(drop),
            Some("run") => build(release, "gala-os", "").and_then(|iso| run(&iso, headless)),
            // gala-um first: it fails in seconds, before minutes of boots.
            Some("lint") => lint::cli(&root(), args.iter().any(|a| a == "--full")),
            Some("test") => lint::cli(&root(), false)
                .and_then(|()| stub_audit::stub_audit(&root()))
                .and_then(|()| unit_tests())
                .and_then(|()| alloc_audit(release, None, false))
                .and_then(|()| um_test(release))
                .and_then(|()| replay_test(release))
                // `--no-boot`: everything but the boot scenarios (CI boots
                // them in parallel jobs with XTASK_SHARD).
                .and_then(|()| {
                    if args.iter().any(|a| a == "--no-boot") {
                        Ok(())
                    } else {
                        boot_tests(release, None)
                    }
                }),
            Some("boot-test") => boot_tests(release, args.get(1).filter(|a| !a.starts_with("--"))),
            Some("export-tests") => build_kernel(release).and_then(|()| export_tests()),
            Some("report") => gala_report(release),
            Some("docker-test") => docker_test(&args[1..]),
            Some("screenshot") => screenshot(release, &args[1..]),
            Some("bake-fonts") => bake_fonts(),
            Some("bake-icons") => bake_icons(),
            Some("iso") => iso(release, &args[1..]),
            Some("serve") => serve(release, &args[1..]),
            Some("um-test") => um_test(release),
            Some("replay-test") => replay_test(release),
            Some("fault-test") => fault_test(
                release,
                args.get(1).filter(|a| !a.starts_with("--")),
                args.iter().find_map(|a| match a.as_str() {
                    "--sweep" => Some("heap"),
                    _ => a.strip_prefix("--sweep="),
                }),
            ),
            Some("sim-test") => {
                let value = |name: &str| -> Option<String> {
                    let eq = format!("{name}=");
                    args.iter().enumerate().find_map(|(i, a)| match a.strip_prefix(&eq) {
                        Some(v) => Some(v.to_string()),
                        None if a == name => args.get(i + 1).cloned(),
                        None => None,
                    })
                };
                let number = |name: &str| value(name).map(|v| v.parse::<u64>());
                match (number("--seeds").transpose(), number("--seed").transpose()) {
                    (Ok(seeds), Ok(seed)) => sim_test(
                        release,
                        args.get(1).filter(|a| !a.starts_with("--")),
                        seeds.unwrap_or(SIM_SEEDS),
                        seed,
                        args.iter().any(|a| a == "--all"),
                    ),
                    _ => Err("usage: cargo xtask sim-test [filter] [--seeds N] [--seed S] [--all]"
                        .into()),
                }
            }
            Some("bench") => {
                bench(release, args.get(1).filter(|a| !a.starts_with("--")).map(Path::new))
            }
            Some("lx-expect") => lx_expect::lx_expect(&args[1..]),
            Some("qemu") => build_qemu(),
            Some("stub-audit") => stub_audit::stub_audit(&root()),
            Some("alloc-audit") => alloc_audit(
                release,
                args.get(1).filter(|a| !a.starts_with("--")).map(Path::new),
                args.iter().any(|a| a == "--all"),
            ),
            Some("symbolize") => match args.get(1).filter(|a| !a.starts_with("--")) {
                Some(log) => fs::read_to_string(log)
                    .map_err(|e| format!("reading {log}: {e}"))
                    .map(|text| print!("{}", symbolize(release, &text))),
                None => Err("usage: cargo xtask symbolize <serial log> [--release]".into()),
            },
            Some("triage") => match args.get(1).filter(|a| !a.starts_with("--")) {
                Some(log) => triage_command(Path::new(log), args.get(2).map(Path::new)),
                None => Err("usage: cargo xtask triage <serial log> [<events.jsonl>]".into()),
            },
            Some("events") => match args.get(1).filter(|a| !a.starts_with("--")) {
                Some(file) => fs::read_to_string(file)
                    .map_err(|e| format!("reading {file}: {e}"))
                    .map(|text| print!("{}", triage::events_summary(&text))),
                None => Err("usage: cargo xtask events <events.jsonl>".into()),
            },
            Some("feed") => feed::cli(&args[1..]),
            Some("release-tree") => match args.get(1).filter(|a| !a.starts_with("--")) {
                Some(dir) => release_tree(release, Path::new(dir)),
                None => Err("usage: cargo xtask release-tree <dir> [--release]".into()),
            },
            Some("otel") => otel_cli(&args[1..]),
            Some("profile") => profile_cli(&args[1..]),
            Some("perfetto") => perfetto_cli(&args[1..]),
            Some("serve-page") => serve_page(),
            Some("wpt-compare") => wpt::compare(),
            Some("relay") => {
                wpt::relay(args.get(1).map_or("", String::as_str), args.get(2).map(String::as_str))
            }
            Some("compare") => match args.get(1).filter(|a| !a.starts_with("--")) {
                Some(windows) => compare_reports(Path::new(windows), args.get(2).map(Path::new)),
                None => Err("usage: cargo xtask compare <windows.json> [<gala.json>]".into()),
            },
            _ => {
                eprint!("{USAGE}");
                return ExitCode::from(2);
            }
        };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

fn target_dir() -> PathBuf {
    env::var_os("CARGO_TARGET_DIR").map_or_else(|| root().join("target"), PathBuf::from)
}

fn cargo() -> Command {
    let mut cmd = Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    cmd.current_dir(root());
    cmd
}

/// `cargo build` of user crates (release, the Windows target). Paths of
/// crates.io dependencies (panic locations in libm, say) are absolute
/// under the cargo home, which differs between machines: they are
/// remapped to `/cargo` so that the checked-in binaries are the same
/// wherever they are built (CI compares them).
fn user_build() -> Command {
    user_build_for(USER_TARGET)
}

/// `user_build` for `target` (USER_TARGET or USER_TARGET_X86).
fn user_build_for(target: &str) -> Command {
    let mut cmd = cargo();
    cmd.args(["build", "--release", "--target", target]);
    let cargo_home = env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join(".cargo")));
    if let Some(home) = cargo_home {
        // Cargo merges this array with the target's rustflags in
        // .cargo/config.toml.
        let flag = format!("--remap-path-prefix={}=/cargo", home.display());
        cmd.arg("--config").arg(format!("target.{target}.rustflags=[{flag:?}]"));
    }
    cmd
}

/// Runs `cmd`, showing its output only if it fails.
fn run_quiet(cmd: &mut Command) -> Result {
    let out = cmd.output().map_err(|e| format!("failed to spawn {cmd:?}: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    eprint!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    Err(format!("{cmd:?} failed: {}", out.status))
}

fn run_cmd(cmd: &mut Command) -> Result {
    let status = cmd.status().map_err(|e| format!("failed to spawn {cmd:?}: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("{cmd:?} failed: {status}")) }
}

fn unit_tests() -> Result {
    eprintln!("xtask: running kernel and xtask unit tests on the host");
    run_cmd(cargo().args(["test", "--package", "kernel", "--lib"]))?;
    // The handover formats as the `handover_next` build has them (R9 (d)).
    run_cmd(
        cargo()
            .args(["test", "--package", "kernel", "--lib", "--features", "handover_next"])
            .arg("handover::"),
    )?;
    run_cmd(cargo().args(["test", "--package", "xtask"]))?;
    // The Linux personality's ABI data, checked against the user/lx
    // transcripts recorded on Linux (ROADMAP LX).
    run_cmd(cargo().args(["test", "--package", "lx_abi"]))?;
    // AFD's request layouts for both pointer sizes (kernel/afd_abi).
    run_cmd(cargo().args(["test", "--package", "afd_abi"]))?;
    // The Wayland wire format (kernel/wl_abi; ROADMAP LD step 2).
    run_cmd(cargo().args(["test", "--package", "wl_abi"]))?;
    // `gala install`'s package manager (manifests, zip, the install
    // transaction; ROADMAP PK).
    run_cmd(cargo().args(["test", "--package", "galapkg"]))?;
    // Schannel's TLS engine (both sides in memory; ROADMAP TLS).
    run_cmd(cargo().args(["test", "--package", "schannel_tls"]))?;
    // The update feed's TUF metadata and the client's refusals (R8).
    run_cmd(cargo().args(["test", "--package", "tuf"]))
}

/// `cargo xtask alloc-audit`: disassembles the kernel (the debug build
/// unless `elf` names one) and fails on infallible allocations
/// (`alloc_audit`). Skipped with a warning without `objdump`.
fn alloc_audit(release: bool, elf: Option<&Path>, all: bool) -> Result {
    let elf = match elf {
        Some(path) => path.to_path_buf(),
        None => {
            let mut cmd = cargo();
            cmd.args([
                "build",
                "--package",
                "kernel",
                "--bin",
                "kernel",
                "--target",
                KERNEL_TARGET,
            ]);
            // The kernel the boot tests run (its trust key too).
            trust::kernel_env(&mut cmd)?;
            if release {
                cmd.arg("--release");
            }
            run_cmd(&mut cmd)?;
            let profile = if release { "release" } else { "debug" };
            target_dir().join(KERNEL_TARGET).join(profile).join("kernel")
        }
    };
    let output = match Command::new("objdump")
        .args(["-d", "--no-show-raw-insn", "-C"])
        .arg(&elf)
        .stderr(Stdio::inherit())
        .output()
    {
        Ok(o) if o.status.success() => o,
        Ok(o) => return Err(format!("objdump {} failed: {}", elf.display(), o.status)),
        Err(e) => {
            eprintln!("xtask: alloc-audit skipped (no objdump: {e})");
            return Ok(());
        }
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let findings = alloc_audit::audit(&text);
    let failing = findings.iter().filter(|f| !f.excused).count();
    print!("{}", alloc_audit::describe(&findings, all));
    eprintln!(
        "xtask: alloc-audit {}: {} kernel calls into library code that may abort on \
         out-of-memory, {failing} not reserved for first",
        elf.display(),
        findings.len()
    );
    if failing == 0 {
        Ok(())
    } else {
        Err(format!(
            "alloc-audit: {failing} infallible allocations in the kernel (listed above; \
             use the fallible APIs, docs/WORKFLOW.md \"Fault injection\")"
        ))
    }
}

/// Builds the kernel and packs it with Limine into a UEFI-bootable ISO named
/// `name.iso` whose boot entry passes `cmdline` to the kernel.
fn build(release: bool, name: &str, cmdline: &str) -> Result<PathBuf> {
    build_kernel(release)?;
    make_iso(release, name, cmdline, &[])
}

fn build_kernel(release: bool) -> Result {
    // Every kernel xtask builds trusts the test key (R8; `XTASK_UNSIGNED`).
    let mut cmd = cargo();
    cmd.args(["build", "--package", "kernel", "--bin", "kernel", "--target", KERNEL_TARGET]);
    trust::kernel_env(&mut cmd)?;
    if release {
        cmd.arg("--release");
    }
    run_cmd(&mut cmd)?;
    // User programs are always optimized: without a CRT there is no
    // `memcpy` etc. for unoptimized code to call. DLLs that programs embed
    // are built first.
    let mut cmd = user_build();
    for dll in USER_DLLS {
        cmd.args(["--package", dll.package]);
    }
    run_cmd(&mut cmd)?;
    let mut cmd = user_build();
    for program in USER_PROGRAMS {
        cmd.args(["--package", program]);
    }
    run_cmd(&mut cmd)?;
    let mut cmd = user_build_for(USER_TARGET_X86);
    for dll in USER_DLLS_X86 {
        cmd.args(["--package", dll.package]);
    }
    run_cmd(&mut cmd)?;
    let mut cmd = user_build_for(USER_TARGET_X86);
    for program in USER_PROGRAMS_X86 {
        cmd.args(["--package", program]);
    }
    run_cmd(&mut cmd)?;
    build_go_programs()
}

/// Builds GO_PROGRAMS into the user target directory (or copies the
/// checked-in binaries when there is no Go toolchain).
fn build_go_programs() -> Result {
    let out_dir = target_dir().join(USER_TARGET).join("release");
    fs::create_dir_all(&out_dir).map_err(|e| format!("creating {out_dir:?}: {e}"))?;
    let have_go = Command::new("go").arg("version").output().is_ok_and(|o| o.status.success());
    for program in GO_PROGRAMS {
        let exe = out_dir.join(format!("{program}.exe"));
        if !have_go {
            copy(&root().join("user/bin").join(format!("{program}.exe")), &exe)?;
            continue;
        }
        run_quiet(
            Command::new("go")
                .args(["build", "-trimpath", "-buildvcs=false", "-ldflags=-buildid=", "-o"])
                .arg(&exe)
                .arg(".")
                .current_dir(root().join("user").join(program))
                .env("GOOS", "windows")
                .env("GOARCH", "amd64")
                .env("CGO_ENABLED", "0")
                .env("GOTOOLCHAIN", GO_TOOLCHAIN)
                .env("GOFLAGS", "-mod=mod"),
        )?;
    }
    Ok(())
}

/// Where the server kernel (built without the desktop) goes.
/// R9 (d): scenarios that hand over between the default kernel and the
/// `handover_next` build (a stand-in for the next release, with a changed
/// state format), with whether they boot the next build; the other kernel
/// is shipped as a boot module (`kernel-next`, `kernel-gala`) for
/// `handover_image=`.
const HANDOVER_BUILD_SCENARIOS: &[(&str, bool)] = &[
    ("handover_upgrade", false),
    ("handover_upgrade_uefi", false),
    ("handover_downgrade_refused", true),
];

/// Where the `handover_next` kernel goes (its own target directory).
fn next_target_dir() -> PathBuf {
    target_dir().join("next")
}

/// Builds the `handover_next` kernel (R9 (d)) and returns its path.
fn build_next_kernel(release: bool) -> Result<PathBuf> {
    let mut cmd = cargo();
    cmd.args(["build", "--package", "kernel", "--bin", "kernel", "--target", KERNEL_TARGET])
        .args(["--features", "handover_next", "--target-dir"])
        .arg(next_target_dir());
    trust::kernel_env(&mut cmd)?;
    if release {
        cmd.arg("--release");
    }
    run_cmd(&mut cmd)?;
    let profile = if release { "release" } else { "debug" };
    Ok(next_target_dir().join(KERNEL_TARGET).join(profile).join("kernel"))
}

fn make_iso(release: bool, name: &str, cmdline: &str, extra: &[(&str, &str)]) -> Result<PathBuf> {
    make_signed_iso(release, name, cmdline, extra, &trust::signing_for(name))
}

/// `make_iso` with the image signed as `signing` says (R8: the manifest's
/// release, files changed on purpose for a refusal scenario).
fn make_signed_iso(
    release: bool,
    name: &str,
    cmdline: &str,
    extra: &[(&str, &str)],
    signing: &trust::Signing,
) -> Result<PathBuf> {
    // The disk is shared with other worktrees and fills up: say so plainly
    // instead of a copy or xorriso failing half-way.
    check_disk_space(name, MIN_FREE_MIB)?;
    make_iso_unchecked(release, name, cmdline, extra, signing, None).map_err(|e| {
        if e.contains("No space left") || e.contains("os error 28") {
            format!("{e}\n{}", disk_hint(name, None))
        } else {
            e
        }
    })
}

/// Free space a new image needs at least before it is started (its tree,
/// then the ISO: twice its files).
const MIN_FREE_MIB: u64 = 300;

/// What to do when the disk is full (WORKFLOW "Disk space").
fn disk_hint(name: &str, need: Option<u64>) -> String {
    let free = free_disk_mib(&target_dir()).map_or("?".into(), |m| format!("{m} MiB"));
    let need = need.map_or(String::new(), |n| format!(", ~{n} MiB needed"));
    format!(
        "xtask: the disk is (nearly) full making {name}.iso: {free} free on the disk of \
         {}{need}. Delete images (target/*/*.iso, kept for failed boots), \
         target/debug/incremental, and other worktrees' target/*/test-* and *.iso \
         (docs/WORKFLOW.md, \"Disk space\"); `du -sh target/* .claude/worktrees/*/target` \
         shows who uses it.",
        target_dir().display()
    )
}

/// Fails early when less than `need` MiB are free.
fn check_disk_space(name: &str, need: u64) -> Result {
    match free_disk_mib(&target_dir()) {
        Some(free) if free < need => Err(disk_hint(name, Some(need))),
        _ => Ok(()),
    }
}

/// Bytes of the files under `dir`.
fn tree_size(dir: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(dir) else { return 0 };
    entries
        .flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => tree_size(&e.path()),
            _ => e.metadata().map_or(0, |m| m.len()),
        })
        .sum()
}

/// Stages the image's tree (kernel, test programs, `extra`, Limine, the
/// sealed config and manifest) and makes the ISO from it; with `tree`,
/// stages into that directory without the test programs and stops there
/// (`release-tree`: what a release publishes).
fn make_iso_unchecked(
    release: bool,
    name: &str,
    cmdline: &str,
    extra: &[(&str, &str)],
    signing: &trust::Signing,
    tree: Option<&Path>,
) -> Result<PathBuf> {
    let profile = if release { "release" } else { "debug" };
    let mut kernel = target_dir().join(KERNEL_TARGET).join(profile).join("kernel");
    // R9 (d): the other build as a boot module, to hand over to.
    let builds = name
        .strip_prefix("test-")
        .and_then(|n| HANDOVER_BUILD_SCENARIOS.iter().find(|s| s.0 == n).map(|s| s.1));
    let mut other_kernel = None;
    if let Some(boots_next) = builds {
        let next = build_next_kernel(release)?;
        other_kernel = Some(if boots_next {
            ("kernel-gala", std::mem::replace(&mut kernel, next))
        } else {
            ("kernel-next", next)
        });
    }
    let limine = fetch_limine()?;
    let out = target_dir().join(profile);
    let iso_root = tree.map_or_else(|| out.join(format!("{name}-iso-root")), Path::to_path_buf);
    if iso_root.exists() {
        fs::remove_dir_all(&iso_root).map_err(|e| format!("cleaning {iso_root:?}: {e}"))?;
    }
    let efi_boot = iso_root.join("EFI/BOOT");
    let boot = iso_root.join("boot");
    for dir in [&efi_boot, &boot.join("limine")] {
        fs::create_dir_all(dir).map_err(|e| format!("creating {dir:?}: {e}"))?;
    }
    copy_kernel(&kernel, &boot.join("kernel"))?;
    let mut modules = String::new();
    if let Some((module, path)) = &other_kernel {
        copy_kernel(path, &boot.join(module))?;
        modules += &format!("    module_path: boot():/boot/{module}\n");
    }
    let programs = if tree.is_some() { &[][..] } else { USER_PROGRAMS };
    let others = if tree.is_some() { &[][..] } else { GO_PROGRAMS };
    let x86 = if tree.is_some() { &[][..] } else { USER_PROGRAMS_X86 };
    for program in programs.iter().chain(others).chain(x86) {
        copy(&built_program(program), &boot.join(format!("{program}.exe")))?;
        modules += &format!("    module_path: boot():/boot/{program}.exe\n");
    }
    for (file, dir) in extra {
        // A path with a slash is a file in the repository (checked-in
        // binaries, configuration); otherwise a built DLL.
        if file.starts_with("target/prebuilt/") {
            fetch_prebuilt(file.trim_end_matches('/'))?;
        }
        // A path ending in a slash is a directory tree, shipped as C:\<dir>
        // with its subdirectories: packed into one USTAR archive the kernel
        // unpacks (Limine runs out of memory-map entries with hundreds of
        // modules; the .NET runtime has that many files).
        if file.ends_with('/') {
            let mut files = Vec::new();
            tree_files(&root().join(file), Path::new(""), &mut files)?;
            let archive = format!("{}.tree.tar", dir.replace('/', "_"));
            write_tar(&root().join(file), &files, &boot.join(&archive))?;
            // Data directories are writable (`tarw:`), program files not.
            let kind = if WRITABLE_TREES.contains(dir) { "tarw" } else { "tar" };
            modules += &format!(
                "    module_path: boot():/boot/{archive}\n    module_string: {kind}:{dir}\n"
            );
            continue;
        }
        // A 32-bit DLL: under boot/x86 (its name is often a 64-bit one's;
        // the kernel names the file after the path's last part).
        if let Some(dll) = x86_dll(file) {
            let x86 = boot.join("x86");
            fs::create_dir_all(&x86).map_err(|e| format!("creating {x86:?}: {e}"))?;
            copy(&built_dll_x86(dll.file), &x86.join(dll.file))?;
            modules += &format!(
                "    module_path: boot():/boot/x86/{}\n    module_string: {dir}\n",
                dll.file
            );
            continue;
        }
        let (source, name) = match file.rsplit_once('/') {
            Some((_, name)) => (root().join(file), name),
            None => (built_dll(file), *file),
        };
        copy(&source, &boot.join(name))?;
        modules += &format!("    module_path: boot():/boot/{name}\n    module_string: {dir}\n");
    }
    copy(&limine.join("BOOTX64.EFI"), &efi_boot.join("BOOTX64.EFI"))?;
    for file in ["limine-uefi-cd.bin", "limine-bios-cd.bin", "limine-bios.sys"] {
        copy(&limine.join(file), &boot.join("limine").join(file))?;
    }
    let config = format!(
        "timeout: 0\nquiet: yes\n\n/gala-os\n    protocol: limine\n    path: boot():/boot/kernel\n    cmdline: {cmdline}\n{modules}"
    );
    // R8: the manifest, the config's hashes, the enrolled executables.
    trust::seal(&iso_root, &config, signing)?;
    if tree.is_some() {
        return Ok(iso_root);
    }

    let iso = out.join(format!("{name}.iso"));
    // The ISO is as large as its tree.
    check_disk_space(name, (tree_size(&iso_root) >> 20) + 64)?;
    run_quiet(
        Command::new("xorriso")
            .args(["-as", "mkisofs", "-R", "-r", "-J", "-quiet"])
            .args(["-b", "boot/limine/limine-bios-cd.bin", "-no-emul-boot", "-boot-load-size", "4"])
            .args(["-boot-info-table"])
            .args(["--efi-boot", "boot/limine/limine-uefi-cd.bin"])
            .args(["-efi-boot-part", "--efi-boot-image", "--protective-msdos-label"])
            .arg(&iso_root)
            .arg("-o")
            .arg(&iso),
    )?;
    // Hybrid image: bootable by BIOS (Limine's stages) and UEFI.
    run_quiet(Command::new(limine.join("limine")).arg("bios-install").arg(&iso))?;
    // The tree is in the image now (it can be as large: disk space).
    let _ = fs::remove_dir_all(&iso_root);
    if name == "gala-os" {
        eprintln!("xtask: built {}", iso.display());
    }
    Ok(iso)
}

/// Writes the files `files` (relative to `dir`) as a USTAR archive (the
/// POSIX.1-1988 format; the kernel reads it, kernel/src/fs/tar.rs).
fn write_tar(dir: &Path, files: &[PathBuf], out: &Path) -> Result {
    let mut tar = Vec::new();
    for rel in files {
        let is_dir = dir.join(rel).is_dir();
        let mut path = rel.to_string_lossy().replace('\\', "/");
        let data = if is_dir {
            path.push('/');
            Vec::new()
        } else {
            fs::read(dir.join(rel)).map_err(|e| format!("reading {rel:?}: {e}"))?
        };
        // `prefix/name`: the name holds 100 bytes, the prefix 155.
        let (prefix, name) = if path.len() <= 100 {
            ("", path.as_str())
        } else {
            path.char_indices()
                .filter(|&(i, c)| c == '/' && i <= 155 && path.len() - i - 1 <= 100)
                .map(|(i, _)| (&path[..i], &path[i + 1..]))
                .next()
                .ok_or(format!("path too long for a tar archive: {path}"))?
        };
        let mut h = [0u8; 512];
        h[..name.len()].copy_from_slice(name.as_bytes());
        h[100..108].copy_from_slice(b"0000444\0");
        h[108..116].copy_from_slice(b"0000000\0");
        h[116..124].copy_from_slice(b"0000000\0");
        h[124..136].copy_from_slice(format!("{:011o}\0", data.len()).as_bytes());
        h[136..148].copy_from_slice(b"00000000000\0");
        h[156] = if is_dir { b'5' } else { b'0' };
        h[257..263].copy_from_slice(b"ustar\0");
        h[263..265].copy_from_slice(b"00");
        h[345..345 + prefix.len()].copy_from_slice(prefix.as_bytes());
        h[148..156].copy_from_slice(b"        ");
        let sum: u32 = h.iter().map(|&b| u32::from(b)).sum();
        h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
        tar.extend_from_slice(&h);
        tar.extend_from_slice(&data);
        tar.resize(tar.len().div_ceil(512) * 512, 0);
    }
    tar.resize(tar.len() + 1024, 0);
    fs::write(out, tar).map_err(|e| format!("writing {out:?}: {e}"))
}

/// The files and directories under `dir` (paths relative to it),
/// recursively, each directory before its contents.
fn tree_files(dir: &Path, rel: &Path, out: &mut Vec<PathBuf>) -> Result {
    let entries =
        fs::read_dir(dir.join(rel)).map_err(|e| format!("reading {dir:?}/{rel:?}: {e}"))?;
    let mut names: Vec<_> = entries.filter_map(|e| e.ok()).map(|e| e.file_name()).collect();
    names.sort();
    for name in names {
        // xtask's own files (the .unpacked-<hash> marker).
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let path = rel.join(&name);
        if dir.join(&path).is_dir() {
            // The directory itself too (an empty one would be lost).
            out.push(path.clone());
            tree_files(dir, &path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

/// Clones the pinned Limine binary release into the target directory once
/// (once per xtask run too: ISOs are built in parallel, and concurrent
/// clones into one directory fail).
/// Makes sure the prebuilt file `path` (`target/prebuilt/<name>/<file>`)
/// is there: downloads its release (curl), checks the SHA-256 and unzips
/// the members (unzip).
fn fetch_prebuilt(path: &str) -> Result {
    // ISOs are built in parallel, and two scenarios may need the same
    // release: one fetch at a time, so none sees a half-written file.
    static FETCH: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = FETCH.lock().unwrap_or_else(|e| e.into_inner());
    let full = root().join(path);
    let name = path.split('/').nth(2).ok_or(format!("bad prebuilt path {path}"))?;
    let p = PREBUILT.iter().find(|p| p.name == name).ok_or(format!("no prebuilt {name:?}"))?;
    let dir = root().join("target/prebuilt").join(p.name);
    // Written once the release is unpacked: a partial unpack (an
    // interrupted run) is redone.
    // Keyed by the selection too, so a changed member list unpacks again.
    let selection = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (p.members, p.excludes).hash(&mut h);
        h.finish()
    };
    let done = dir.join(format!(".unpacked-{}-{selection:016x}", p.checksum));
    if full.exists() && done.exists() {
        return Ok(());
    }
    // Another release (a re-pinned version) or a partial unpack: start
    // from nothing, or the old release's files stay beside the new ones
    // (and ship with a directory module).
    if dir.exists() {
        eprintln!("xtask: {}: no unpack of the pinned release; clearing {dir:?}", p.name);
        fs::remove_dir_all(&dir).map_err(|e| format!("clearing {dir:?}: {e}"))?;
    }
    fs::create_dir_all(&dir).map_err(|e| format!("creating {dir:?}: {e}"))?;
    let zip = dir.join("download.zip");
    eprintln!("xtask: fetching {} ({})", p.name, p.url);
    run_cmd(Command::new("curl").args(["-sSfL", "-o"]).arg(&zip).arg(p.url))?;
    // SHA-256 or SHA-512 (Microsoft publishes the latter), by length.
    let tool = if p.checksum.len() == 128 { "sha512sum" } else { "sha256sum" };
    let sum = Command::new(tool).arg(&zip).output().map_err(|e| format!("{tool}: {e}"))?;
    let sum = String::from_utf8_lossy(&sum.stdout);
    if sum.split_whitespace().next() != Some(p.checksum) {
        let _ = fs::remove_file(&zip);
        return Err(format!("{}: SHA-256 mismatch ({sum})", p.url));
    }
    // A Chocolatey package (a zip) that carries the installer in tools/:
    // that installer is the archive. A NuGet library package (nuget.org)
    // is a plain zip, unpacked as one.
    let chocolatey = p.url.ends_with(".nupkg") && p.url.contains("chocolatey.org");
    let zip = if chocolatey {
        let inner = dir.join("nupkg");
        let _ = fs::remove_dir_all(&inner);
        run_cmd(
            Command::new("unzip")
                .args(["-q", "-o"])
                .arg(&zip)
                .arg("tools/*.exe")
                .arg("-d")
                .arg(&inner),
        )?;
        let _ = fs::remove_file(&zip);
        // A package may carry a 32-bit and a 64-bit installer (FileZilla's
        // does): the 64-bit one, by name, whatever order the directory
        // lists them in; otherwise the first by name.
        let tools = inner.join("tools");
        let mut exes: Vec<PathBuf> = fs::read_dir(&tools)
            .map_err(|e| format!("reading {tools:?}: {e}"))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "exe"))
            .collect();
        let is_64 = |p: &PathBuf| {
            let n = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
            n.contains("win64") || n.contains("x64")
        };
        exes.sort_by_key(|p| (!is_64(p), p.clone()));
        eprintln!(
            "xtask: {}: installers in tools/: {:?}, using the first",
            p.name,
            exes.iter().filter_map(|p| p.file_name()).collect::<Vec<_>>()
        );
        let exe = exes.into_iter().next().ok_or(format!("{}: no installer in tools/", p.url))?;
        let moved = dir.join("download.exe");
        fs::rename(&exe, &moved).map_err(|e| format!("moving {exe:?}: {e}"))?;
        let _ = fs::remove_dir_all(&inner);
        moved
    } else {
        zip
    };
    if p.members.len() == 1 && p.url.rsplit('/').next() == Some(p.members[0]) {
        // Not an archive: the download is the one file used.
        let to = dir.join(p.members[0]);
        fs::rename(&zip, &to).map_err(|e| format!("moving {zip:?} to {to:?}: {e}"))?;
    } else if p.url.ends_with(".msi") {
        unpack_msi(p, &zip, &dir)?;
    } else if p.url.ends_with(".deb") {
        unpack_deb(p, &zip, &dir)?;
    } else if p.url.ends_with(".7z")
        || p.url.ends_with(".7z.exe")
        || p.url.ends_with("-setup.exe")
        || chocolatey
    {
        // A 7-Zip archive (mesa-dist-win's), a 7-Zip self-extracting
        // archive (Git for Windows' PortableGit) or an NSIS installer
        // (FileZilla): 7-Zip reads it without running it. Members are
        // files, directories or wildcards.
        let mut cmd = Command::new("7z");
        cmd.args(["x", "-y", "-bd", "-bso0"]).arg(format!("-o{}", dir.display())).arg(&zip);
        cmd.args(p.members);
        for exclude in p.excludes {
            cmd.arg(format!("-x!{exclude}"));
        }
        run_cmd(&mut cmd)
            .map_err(|e| format!("{e} (7z is in p7zip-full: apt install p7zip-full)"))?;
    } else {
        let mut cmd = Command::new("unzip");
        cmd.args(["-q", "-o"]).arg(&zip).args(p.members).arg("-d").arg(&dir);
        if !p.excludes.is_empty() {
            cmd.arg("-x").args(p.excludes);
        }
        run_cmd(&mut cmd)?;
    }
    let _ = fs::remove_file(&zip);
    if !full.exists() {
        return Err(format!("{path} is not in {}", p.url));
    }
    write(&done, p.url)
}

/// Unpacks the Debian package `deb` (dpkg-deb, its data archive as the
/// package would install it) and moves `p`'s member files into `dir`
/// under their file names (`usr/lib/x86_64-linux-gnu/libc.so.6` becomes
/// `libc.so.6`).
fn unpack_deb(p: &Prebuilt, deb: &Path, dir: &Path) -> Result {
    let tree = dir.join("deb");
    if tree.exists() {
        fs::remove_dir_all(&tree).map_err(|e| format!("cleaning {tree:?}: {e}"))?;
    }
    fs::create_dir_all(&tree).map_err(|e| format!("creating {tree:?}: {e}"))?;
    run_quiet(Command::new("dpkg-deb").arg("-x").arg(deb).arg(&tree))
        .map_err(|e| format!("{e} (dpkg-deb is in dpkg)"))?;
    for member in p.members {
        let last = member.rsplit('/').next().unwrap_or(member);
        fs::rename(tree.join(member), dir.join(last))
            .map_err(|e| format!("{member} in {}: {e}", p.url))?;
    }
    fs::remove_dir_all(&tree).map_err(|e| format!("cleaning {tree:?}: {e}"))
}

/// Unpacks the MSI `msi` (msitools' msiextract, the files laid out as
/// Windows Installer would install them) and moves `p`'s member
/// directories into `dir`, then removes the excludes.
fn unpack_msi(p: &Prebuilt, msi: &Path, dir: &Path) -> Result {
    let tree = dir.join("msi");
    if tree.exists() {
        fs::remove_dir_all(&tree).map_err(|e| format!("cleaning {tree:?}: {e}"))?;
    }
    fs::create_dir_all(&tree).map_err(|e| format!("creating {tree:?}: {e}"))?;
    run_quiet(Command::new("msiextract").arg("-C").arg(&tree).arg(msi))
        .map_err(|e| format!("{e} (msiextract is in msitools: apt install msitools)"))?;
    for member in p.members {
        let last = member.rsplit('/').next().unwrap_or(member);
        let to = dir.join(last);
        if to.exists() {
            fs::remove_dir_all(&to).map_err(|e| format!("cleaning {to:?}: {e}"))?;
        }
        fs::rename(tree.join(member), &to).map_err(|e| format!("{member} in {}: {e}", p.url))?;
    }
    for exclude in p.excludes {
        let path = dir.join(exclude);
        let removed =
            if path.is_dir() { fs::remove_dir_all(&path) } else { fs::remove_file(&path) };
        removed.map_err(|e| format!("excluding {exclude}: {e}"))?;
    }
    fs::remove_dir_all(&tree).map_err(|e| format!("cleaning {tree:?}: {e}"))
}

fn fetch_limine() -> Result<PathBuf> {
    static LIMINE: std::sync::OnceLock<Result<PathBuf>> = std::sync::OnceLock::new();
    LIMINE.get_or_init(fetch_limine_once).clone()
}

fn fetch_limine_once() -> Result<PathBuf> {
    // Under the repository's own target/ even with CARGO_TARGET_DIR set, so
    // the host and the Docker image (target/docker) share one checkout.
    let dir = root().join("target").join(format!("limine-{LIMINE_TAG}"));
    if !dir.join("BOOTX64.EFI").exists() {
        if dir.exists() {
            fs::remove_dir_all(&dir).map_err(|e| format!("cleaning {dir:?}: {e}"))?;
        }
        eprintln!("xtask: fetching Limine {LIMINE_TAG}");
        run_cmd(
            Command::new("git")
                .args(["clone", "--quiet", "--depth", "1", "--branch", LIMINE_TAG, LIMINE_REPO])
                .arg(&dir),
        )?;
    }
    if !dir.join("limine").exists() {
        // The host tool that installs Limine's BIOS stages into an ISO.
        run_quiet(Command::new("make").arg("-C").arg(&dir))?;
    }
    let head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&dir)
        .output()
        .map_err(|e| format!("git rev-parse: {e}"))?;
    let head = String::from_utf8_lossy(&head.stdout);
    if head.trim() != LIMINE_COMMIT {
        return Err(format!(
            "Limine checkout {dir:?} is at {}, expected {LIMINE_COMMIT}; delete it and retry",
            head.trim()
        ));
    }
    Ok(dir)
}

struct Firmware {
    code: PathBuf,
    vars: Option<PathBuf>,
}

fn find_ovmf() -> Result<Firmware> {
    const CANDIDATES: &[(&str, &str)] = &[
        ("/usr/share/OVMF/OVMF_CODE_4M.fd", "/usr/share/OVMF/OVMF_VARS_4M.fd"),
        ("/usr/share/OVMF/OVMF_CODE.fd", "/usr/share/OVMF/OVMF_VARS.fd"),
        ("/usr/share/edk2/x64/OVMF_CODE.4m.fd", "/usr/share/edk2/x64/OVMF_VARS.4m.fd"),
        ("/usr/share/edk2/ovmf/OVMF_CODE.fd", "/usr/share/edk2/ovmf/OVMF_VARS.fd"),
        ("/usr/share/qemu/edk2-x86_64-code.fd", "/usr/share/qemu/edk2-i386-vars.fd"),
        (
            "/opt/homebrew/share/qemu/edk2-x86_64-code.fd",
            "/opt/homebrew/share/qemu/edk2-i386-vars.fd",
        ),
    ];
    if let Some(code) = env::var_os("OVMF_CODE") {
        let vars = env::var_os("OVMF_VARS").map(PathBuf::from);
        return Ok(Firmware { code: code.into(), vars });
    }
    CANDIDATES
        .iter()
        .find(|(code, _)| Path::new(code).exists())
        .map(|(code, vars)| Firmware {
            code: code.into(),
            vars: Path::new(vars).exists().then(|| vars.into()),
        })
        .ok_or_else(|| "no OVMF firmware found; install OVMF or set OVMF_CODE".into())
}

/// The firmware a VM boots with. ISOs are hybrid, so either works.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Boot {
    /// QEMU's SeaBIOS: reaches the kernel about 5x faster than OVMF under
    /// software emulation, so boot tests use it by default.
    Bios,
    /// OVMF (UEFI), the path real machines take.
    Uefi,
}

/// Lines the event stream (debug console) of a scenario must contain.
/// What scenarios' event streams must hold (an entry starting with `!`:
/// must not hold).
const EVENT_EXPECT: &[(&str, &[&str])] = &[
    (
        // The sampling profiler: timer ticks sample the running thread
        // (`by`), its mode and instruction pointer, and in the kernel the
        // return addresses of its frames.
        "events_sample",
        &[
            r#""ev":"sample","mode":"user","rip":"#,
            r#""ev":"sample","mode":"kernel","rip":"#,
            r#","stack":[18446744"#,
            // sampleprobe's half second with interrupts off overflows the
            // CPU's ring: its last sample says how many were lost after it.
            r#"],"lost":"#,
        ],
    ),
    (
        // `events_match=` records the system calls of processes whose
        // command line contains the text: process_test's children
        // (`process_test child ...`), not the parent.
        "events_match",
        &[r#"!"ev":"syscall","pid":4,"#, r#""ev":"syscall","pid":12,"#],
    ),
    (
        // `events_pid=` limits system-call events to those processes:
        // process_test's own (pid 4), none of its children's; a wait
        // names the object it waits on.
        "events_pid",
        &[
            r#""ev":"syscall","pid":4,"#,
            r#"!"ev":"syscall","pid":12,"#,
            r#"!"ev":"syscall","pid":20,"#,
            r#""ev":"wait","pid":4,"#,
            r#""obj":"Process","oid":"#,
        ],
    ),
    (
        // RS step B2: each window is registered in its own session, and
        // HWNDs asked for across sessions are named.
        "session_test",
        &[
            r#""ev":"window","op":"create""#,
            r#""ev":"window","op":"describe""#,
            r#""session":0,"class":"GalaSessionTestS0""#,
            r#""session":1,"class":"GalaSessionTestS1""#,
            r#""ev":"window","op":"other session""#,
        ],
    ),
    (
        // SD: the kernel without a screen or `presenter=` keeps session 2's
        // windows with nothing showing them.
        "noscreen_windows",
        &[
            r#""ev":"presenter","op":"live","session":2,"presenter":"none","w":1280,"h":800,"bare":"no screen and no presenter=""#,
            r#""ev":"window","op":"create""#,
        ],
    ),
    (
        // SD: so does the server profile, with a screen.
        "server_profile_windows",
        &[
            r#""ev":"presenter","op":"live","session":2,"presenter":"none","w":1280,"h":800,"bare":"server profile""#,
            r#""ev":"window","op":"create""#,
        ],
    ),
    (
        // RS step B3a's presenters: session 2's desktop goes live shown
        // by nothing, then a Test frame is attached and detached; the
        // console moves to session 2 and back.
        "presenter_test",
        &[
            r#""ev":"presenter","op":"live","session":2,"presenter":"none","w":1280,"h":800"#,
            r#""ev":"presenter","op":"attach","session":2,"presenter":"test","was":"none""#,
            r#""ev":"presenter","op":"attach","session":2,"presenter":"none","was":"test""#,
            r#""ev":"presenter","op":"console","session":2,"from":1,"was":"test","kept":1"#,
            r#""ev":"presenter","op":"console","session":1,"from":2,"was":"none","kept":1"#,
        ],
    ),
    (
        // RS step B3a: each process's window station and desktop, and
        // where the choice came from.
        "winsta_test",
        &[
            r#""ev":"winsta","op":"connect","pid":"#,
            r#""station":"WinSta0","desktop":"Default","from":"logon""#,
            r#""from":"startupinfo""#,
            r#""from":"parent""#,
        ],
    ),
    (
        "sshd_server",
        &[
            r#""ev":"tcp accept""#,
            r#""local":"10.0.2.15:22""#,
            r#""ev":"logon""#,
            r#""user":"galatest""#,
        ],
    ),
    (
        // ssh -L: sshd-session connects to sshfwd_test over loopback;
        // ssh -R: it accepts sshfwd_test's connection on its listener.
        "sshd_forward",
        &[
            r#""ev":"tcp connect""#,
            r#""remote":"127.0.0.1:7000""#,
            r#""ev":"tcp accept""#,
            r#""local":"127.0.0.1:7001""#,
        ],
    ),
    (
        "sshd_shell",
        &[
            r#""ev":"console create","id":1,"#,
            r#""pseudo":true,"cols":80,"rows":25"#,
            r#""ev":"console close","id":1,"pseudo":true"#,
        ],
    ),
    (
        // OT2: a child's process start names its creator (process and
        // thread); every event names the thread whose work emitted it
        // ("by"; 0 for kernel threads).
        "events_process",
        &[
            r#""by":0,"ev":"process start","pid":4,"#,
            r#""ppid":0,"ptid":0"#,
            r#""by":8,"ev":"process start","#,
            r#""ppid":4,"ptid":8"#,
            r#""by":8,"ev":"image","pid":4,"#,
        ],
    ),
    (
        // OT3: pipes and completion ports, so data can be followed from a
        // write to the read that took it and to the thread that dequeued
        // its completion.
        "events_ipc",
        &[
            r#""ev":"pipe create","pipe":"#,
            r#""ev":"pipe connect","pipe":"#,
            r#""ev":"pipe write","pipe":"#,
            r#""ev":"pipe read","pipe":"#,
            r#""ev":"pipe disconnect","pipe":"#,
            r#""ev":"iocp post","port":"#,
            r#""ev":"iocp dequeue","port":"#,
            r#","queued_by":"#,
        ],
    ),
    (
        "events",
        &[
            r#""ev":"process start","pid":4,"session":1,"name":"import_test.exe","personality":"nt""#,
            r#""ev":"image","pid":4,"name":"import_test.exe""#,
            r#""name":"\\Device\\HarddiskVolume1\\gala\\testdll.dll""#,
            r#""ev":"syscall","pid":4,"tid":8,"nr":8,"name":"NtWriteFile""#,
            r#""ev":"thread exit","pid":4,"tid":8,"status":0"#,
            r#""ev":"process exit","pid":4,"session":1,"status":0,"personality":"nt""#,
        ],
    ),
];

/// Scenarios still being brought up: `cargo xtask boot-test <name>` runs
/// them, `cargo xtask test` (and CI) does not, until they pass.
const PENDING: &[&str] = &[
    // BR's conformance report: a report run of hours, not a gate check.
    "chromium_wpt",
    "java_stress",
    "elasticsearch",
    "filezilla",
    "filezilla_flaui",
    "filezilla_sftp",
    "gitbash",
    "mingit",
    "sqllocaldb",
    "sqllocaldb_persist",
    "sshd",
];

/// Boot tests that boot through UEFI, so that path stays covered.
const UEFI_SCENARIOS: &[&str] = &[
    "presenter_test",
    "chromium_window",
    "boot_config_edited",
    "handover_uefi",
    "handover_upgrade_uefi",
    "handover_idle_uefi",
    "handover_k32_uefi",
    "handover_net_uefi",
    "server_profile",
    "server_profile_windows",
    "gala_cli",
    "hello",
    "selftest",
    "fault-pagefault",
    "gui_test",
    "virtio_gpu",
    "virtio_gpu_reset",
    "filezilla",
    "console_window",
];

/// Scenarios whose screen is a virtio-gpu (modern only, no VGA: the
/// firmware's framebuffer is not shown once the kernel runs) instead of
/// QEMU's standard VGA; ARCHITECTURE "Plan: graphics and GPU".
const GPU_SCENARIOS: &[&str] = &["virtio_gpu", "virtio_gpu_reset"];

/// What xtask's host-side client does in a net scenario.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum NetClient {
    /// Sends NET_ECHO_BYTES and expects them back (`net_client`).
    Echo,
    /// GETs `/` and expects this body (`http_client`).
    Http(&'static str),
    /// The page, gzip and a rewrite (`nginx_client`).
    Nginx,
    /// The page over TLS (`tls_client`, with the host's curl).
    Tls,
    /// GETs `/` and expects a 200 whose body contains all of these.
    Contains(&'static [&'static str]),
    /// Logs on with the host's ssh and user/sshd/client_ed25519 as
    /// `galatest` and runs `echo` (`ssh_client`).
    Ssh,
    /// The same with galatest's password (`ssh_client`, through
    /// SSH_ASKPASS) and no key.
    SshPassword,
    /// sshd as a server (`ssh_server_client`): three logins one after
    /// another, then two at once, each running a command.
    SshServer,
    /// An interactive shell over a pty (`ssh_shell_client`, `ssh -tt`):
    /// typed commands, line editing, Ctrl+C, exit.
    SshShell,
    /// File transfer with the host's sftp (`sftp_client`): put, ls, get.
    Sftp,
    /// The host's scp both ways with `-p`, over SFTP and the legacy
    /// protocol (`scp_client`): bytes and times.
    Scp,
    /// `ssh -L` and `ssh -R` to and from user/sshfwd_test inside gala
    /// (`ssh_forward_client`).
    SshForward,
    /// gala's own ssh.exe logs on to gala's sshd over loopback, from a
    /// session the host's ssh opened (`ssh_inner_client`).
    SshInner,
    /// The host's psql runs `select 40+2` (`psql_client`).
    Psql,
    /// Logs on to an FTP server as `gala` and asks for the directory
    /// (`ftp_client`: the control connection only).
    Ftp,
    /// A WebDriver session with ChromeDriver (`webdriver_client`):
    /// headless Chromium loads page.html, a script reads it.
    WebDriver,
    /// web-platform-tests through ChromeDriver (`wpt_client`).
    Wpt,
    /// handover_net_test's line protocol across a kernel handover
    /// (`handover_net_client`).
    HandoverNet,
    /// UDP datagrams to the guest's port 7, echoed (`udp_client`), and a
    /// name resolved by the guest through QEMU's DNS (10.0.2.3).
    Udp,
}

/// Scenarios that get a virtio-net card on QEMU's user-mode network, with
/// host port `<free port>` forwarded to the guest's TCP port 7, the serial
/// line after which the guest accepts connections there, and the client
/// xtask then runs.
const NET_SCENARIOS: &[(&str, &str, NetClient)] = &[
    ("net_echo", "net: echo listening on port 7", NetClient::Echo),
    ("winsock_test", "winsock_test: listening on port 7", NetClient::Echo),
    ("udp_echo", "udp_test: listening on port 7", NetClient::Udp),
    ("handover_net", "handover_net_test: listening on port 7", NetClient::HandoverNet),
    ("handover_net_uefi", "handover_net_test: listening on port 7", NetClient::HandoverNet),
    ("go_http", "go_http: listening on port 7", NetClient::Http(GO_HTTP_PAGE)),
    ("net_reset", "go_http: listening on port 7", NetClient::Http(GO_HTTP_PAGE)),
    ("nginx", "using the \"select\" event method", NetClient::Nginx),
    ("caddy_http", "Caddy serving static files on :7", NetClient::Http(CADDY_PAGE)),
    ("java_http", "java_http: listening on port 7", NetClient::Http(JAVA_HTTP_PAGE)),
    ("nginx_master", "start worker process", NetClient::Nginx),
    ("nginx_tls", "using the \"select\" event method", NetClient::Tls),
    ("sshd_login", "Server listening on", NetClient::Ssh),
    ("sshd_password", "Server listening on", NetClient::SshPassword),
    ("sshd_server", "Server listening on", NetClient::SshServer),
    ("sshd_shell", "Server listening on", NetClient::SshShell),
    ("sshd_sftp", "Server listening on", NetClient::Sftp),
    ("sshd_scp", "Server listening on", NetClient::Scp),
    ("sshd_forward", "Server listening on", NetClient::SshForward),
    ("sshd_client", "Server listening on", NetClient::SshInner),
    ("postgres", "database system is ready to accept connections", NetClient::Psql),
    ("filezilla_server", "[FTP Server] Listening on 0.0.0.0:7", NetClient::Ftp),
    ("chromium_driver", "ChromeDriver was started successfully", NetClient::WebDriver),
    ("chromium_wpt", "ChromeDriver was started successfully", NetClient::Wpt),
    // Elasticsearch's root endpoint: the node's JSON description.
    (
        "elasticsearch",
        "] started {",
        NetClient::Contains(&[
            "\"cluster_name\" : \"elasticsearch\"",
            "\"number\" : \"8.15.3\"",
            "You Know, for Search",
        ]),
    ),
];

/// Scenarios whose program serves until stopped: once the net client has
/// finished, xtask stops the VM (QEMU_EXIT_SUCCESS cannot come) and the
/// scenario passes if the client succeeded.
const XTASK_STOPS: i32 = -1;
/// Scenarios that are meant to time out (a program that hangs on
/// purpose): xtask asks for the state dump, which must have the expected
/// lines.
const XTASK_DUMPS: i32 = -2;
/// How long they run before that.
const DUMP_SCENARIO_TIMEOUT: Duration = Duration::from_secs(25);
/// What Limine prints when it has panicked (R8: a file or config whose
/// hash does not match): it waits for a key, or halts.
const LIMINE_HALTED: &[&str] = &["Press a key to return to menu", "System halted."];

/// `text` without terminal control sequences (ESC [ ... final byte):
/// Limine on the UEFI console places every character with one.
fn plain_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) {
                    break;
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}
const STOPPED_LINE: &str = "xtask: stopped the VM after the client";

/// The guest's port a net scenario's host port is forwarded to: sshd's
/// own (22) for the sshd server scenarios, else 7.
fn guest_port(name: &str) -> u16 {
    if SSH_PORT_22.contains(&name) { 22 } else { 7 }
}

/// The scenario an ISO was made for (`test-<name>.iso`, `serve-<name>.iso`,
/// and an `XTASK_REPEAT` copy `test-<name>.r<i>.iso`). Without the repeat
/// suffix a repeat of sshd_shell was its own unknown scenario: its port
/// forward went to the guest's port 7 instead of sshd's 22 (every
/// connection reset), and a GPU scenario lost its display.
fn iso_scenario(iso: &Path) -> &str {
    let stem = iso.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let stem = match stem.rsplit_once(".r") {
        Some((name, i)) if !i.is_empty() && i.bytes().all(|b| b.is_ascii_digit()) => name,
        _ => stem,
    };
    stem.strip_prefix("test-").or_else(|| stem.strip_prefix("serve-")).unwrap_or(stem)
}

/// Scenarios whose sshd listens on its configured port 22.
const SSH_PORT_22: &[&str] =
    &["sshd_server", "sshd_shell", "sshd_sftp", "sshd_scp", "sshd_forward", "sshd_client"];

/// The listening marker and client of a NET_SCENARIOS entry.
fn net_marker(name: &str) -> Option<(&'static str, NetClient)> {
    NET_SCENARIOS.iter().find(|(n, ..)| *n == name).map(|&(_, m, c)| (m, c))
}

/// The page go_http serves (user/go_http/main.go).
/// What user/java/Http.java serves.
const JAVA_HTTP_PAGE: &str = "hello from Java on gala\n";
const GO_HTTP_PAGE: &str = "hello from gala, a fortress for Windows programs\n";
/// The page Nginx serves: its own start page (user/nginx/html/index.html,
/// from the Nginx release), compared byte for byte.
const NGINX_PAGE: &str = include_str!("../../user/nginx/html/index.html");

/// The page Caddy serves in caddy_http (user/caddy/site/index.html).
const CADDY_PAGE: &str = include_str!("../../user/caddy/site/index.html");

/// Bytes the client sends through the echo server.
const NET_ECHO_BYTES: usize = 65536;

/// The firmware for scenario `name` (`XTASK_FIRMWARE=bios|uefi` overrides).
fn scenario_boot(name: &str) -> Boot {
    match env::var("XTASK_FIRMWARE").as_deref() {
        Ok("uefi") => Boot::Uefi,
        Ok("bios") => Boot::Bios,
        _ if UEFI_SCENARIOS.contains(&name) => Boot::Uefi,
        _ => Boot::Bios,
    }
}

/// Scenarios (substrings of their image names) whose programs rewrite
/// code that another processor runs: JIT compilers (HotSpot, the CLR) and
/// xmodify_test. Without KVM they run with one host thread emulating both
/// processors (`-accel tcg,thread=single`): QEMU's multi-threaded TCG (8.2,
/// and still in 10.2: QEMU issue 4669) can keep a translation of code made
/// while another processor was writing it, so the old instructions run on
/// after the write. JITs patch code other threads run (inline caches,
/// HotSpot's GC rewriting the object addresses embedded in compiled code).
/// xmodify_test fails about 1 boot in 10 with `-accel tcg,thread=multi`.
/// (The intermittent Java heap corruption is not this: WORKFLOW "Known
/// intermittent".)
const SINGLE_THREAD_TCG: &[&str] = &[
    "java_",
    "elasticsearch",
    "dotnet_",
    "pwsh_",
    "flaui",
    "xmodify_test",
    "straddle_test",
    "chromium_",
];

fn single_thread_tcg(name: &str) -> bool {
    SINGLE_THREAD_TCG.iter().any(|s| name.contains(s))
}

/// The QEMU gala is tested with: QEMU_VERSION built by `cargo xtask qemu`
/// into `~/.cache/gala-qemu/<version>`. QEMU 8.1.0-8.2.5 (Ubuntu 24.04's
/// 8.2.2) loses writes to the second page of a translated block that
/// spans two pages, which corrupted JIT-compiled code (the JVM's heap
/// "corruption", WORKFLOW "Known intermittent"); 8.2.6 fixed it.
const QEMU_VERSION: &str = "11.1.3";
const QEMU_SHA256: &str = "5a8289c7940dc77d3873c81a6c29ba3afe1446fc9d36e77fa1658cdbecb84b22";

fn qemu_prefix() -> PathBuf {
    let home = env::var_os("HOME").map_or_else(|| PathBuf::from("/root"), PathBuf::from);
    home.join(".cache/gala-qemu").join(QEMU_VERSION)
}

/// `XTASK_QEMU` (a path), else the pinned build if present, else the one
/// on PATH (with a warning once: it may be a QEMU with the 8.2.x bug).
fn qemu_binary() -> PathBuf {
    if let Some(p) = env::var_os("XTASK_QEMU") {
        return PathBuf::from(p);
    }
    static WARNED: std::sync::Once = std::sync::Once::new();
    let pinned = qemu_prefix().join("bin/qemu-system-x86_64");
    if pinned.is_file() {
        WARNED.call_once(|| eprintln!("xtask: QEMU {QEMU_VERSION} ({})", pinned.display()));
        return pinned;
    }
    WARNED.call_once(|| {
        eprintln!(
            "xtask: QEMU {QEMU_VERSION} not built (`cargo xtask qemu`); using qemu-system-x86_64 from PATH"
        );
    });
    PathBuf::from("qemu-system-x86_64")
}

/// `cargo xtask qemu`: downloads QEMU_VERSION from download.qemu.org,
/// checks QEMU_SHA256, and builds only the x86_64 system emulator (with
/// user networking) into `qemu_prefix()`. Build dependencies: WORKFLOW
/// "QEMU".
fn build_qemu() -> Result {
    let prefix = qemu_prefix();
    let bin = prefix.join("bin/qemu-system-x86_64");
    if bin.is_file() {
        eprintln!("xtask: QEMU {QEMU_VERSION} already built at {}", bin.display());
        return Ok(());
    }
    let work = target_dir().join("qemu-build");
    fs::create_dir_all(&work).map_err(|e| format!("{}: {e}", work.display()))?;
    let tarball = work.join(format!("qemu-{QEMU_VERSION}.tar.xz"));
    let url = format!("https://download.qemu.org/qemu-{QEMU_VERSION}.tar.xz");
    eprintln!("xtask: fetching {url}");
    run_cmd(Command::new("curl").args(["-sSfL", "-o"]).arg(&tarball).arg(&url))?;
    let sum =
        Command::new("sha256sum").arg(&tarball).output().map_err(|e| format!("sha256sum: {e}"))?;
    let sum = String::from_utf8_lossy(&sum.stdout);
    if sum.split_whitespace().next() != Some(QEMU_SHA256) {
        let _ = fs::remove_file(&tarball);
        return Err(format!("{url}: SHA-256 mismatch ({sum})"));
    }
    run_cmd(Command::new("tar").arg("xf").arg(&tarball).arg("-C").arg(&work))?;
    let src = work.join(format!("qemu-{QEMU_VERSION}"));
    let build = work.join("build");
    fs::create_dir_all(&build).map_err(|e| format!("{}: {e}", build.display()))?;
    run_cmd(
        Command::new(src.join("configure"))
            .current_dir(&build)
            .args(["--target-list=x86_64-softmmu", "--enable-slirp", "--disable-docs"])
            .args(["--disable-werror", "--disable-gtk", "--disable-sdl"])
            .arg(format!("--prefix={}", prefix.display())),
    )?;
    let jobs = std::thread::available_parallelism().map_or(2, |n| n.get()).to_string();
    run_cmd(Command::new("make").current_dir(&build).args(["-j", &jobs]))?;
    run_cmd(Command::new("make").current_dir(&build).arg("install"))?;
    let _ = fs::remove_dir_all(&work);
    eprintln!("xtask: QEMU {QEMU_VERSION} built at {}", bin.display());
    Ok(())
}

fn qemu(iso: &Path, boot: Boot) -> Result<Command> {
    // 512 MiB, plus twice the image for images with large programs (the
    // boot modules stay in memory, and programs like a JVM map them).
    let image_mib = fs::metadata(iso).map_or(0, |m| m.len() >> 20);
    let mut memory = if image_mib > 64 { 512 + 2 * image_mib } else { 512 };
    // Elasticsearch: a 256 MiB heap pre-touched, its launcher's JVM, and
    // the files it writes on C: (in memory) need more than that; with
    // ~1.5 GiB it ran out of frames (Elastic asks for several GiB).
    // SQL Server LocalDB asks for about 1 GB, and its 280 MB of files are
    // in memory too. PostgreSQL: its default shared memory (128 MB of
    // buffers), a process per backend and helper, and its cluster on C:
    // ran the 750 MB machine out of frames.
    if iso.file_name().is_some_and(|n| {
        ["elasticsearch", "sqllocaldb", "postgres", "chromium"]
            .iter()
            .any(|s| n.to_string_lossy().contains(s))
    }) {
        memory = memory.max(ELASTICSEARCH_MEMORY_MIB);
    }
    let extra = std::env::var("XTASK_QEMU_ARGS").unwrap_or_default();
    let mut cmd = Command::new(qemu_binary());
    // Two processors (one for `onecpu_*` scenarios: telemetry::vm_cpus).
    let cpus = telemetry::vm_cpus(iso).to_string();
    cmd.args(["-machine", "q35", "-cpu", "max", "-m", &format!("{memory}M"), "-smp", &cpus]);
    // KVM when available, otherwise software emulation; an `-accel` in
    // XTASK_QEMU_ARGS replaces both (`-accel tcg,thread=multi` reproduces
    // what SINGLE_THREAD_TCG avoids).
    if !extra.split_whitespace().any(|a| a == "-accel") {
        let tcg = if iso.file_name().is_some_and(|n| single_thread_tcg(&n.to_string_lossy())) {
            "tcg,thread=single"
        } else {
            "tcg"
        };
        cmd.args(["-accel", "kvm", "-accel", tcg]);
    }
    cmd.args(["-no-reboot"])
        // No default network card; NET_SCENARIOS add their own.
        .args(["-nic", "none"]);
    if boot == Boot::Uefi {
        let fw = find_ovmf()?;
        cmd.arg("-drive")
            .arg(format!("if=pflash,format=raw,unit=0,readonly=on,file={}", fw.code.display()));
        if let Some(vars) = fw.vars {
            // The firmware writes its variable store; give it a private copy.
            let copy_path = iso.with_extension("ovmf-vars.fd");
            copy(&vars, &copy_path)?;
            cmd.arg("-drive")
                .arg(format!("if=pflash,format=raw,unit=1,file={}", copy_path.display()));
        }
    }
    cmd.arg("-cdrom").arg(iso);
    // Extra arguments for debugging, e.g. "-d int,cpu_reset -D qemu.log".
    cmd.args(extra.split_whitespace());
    Ok(cmd)
}

fn run(iso: &Path, headless: bool) -> Result {
    let mut cmd = qemu(iso, Boot::Uefi)?;
    cmd.args(["-serial", "stdio"]);
    if headless {
        cmd.args(["-display", "none"]);
    }
    run_cmd(&mut cmd)
}

/// A boot test: a kernel command line and the expected outcome.
struct Scenario {
    name: &'static str,
    cmdline: &'static str,
    exit_status: i32,
    /// Lines (substrings) that must appear in the serial log.
    expect: &'static [&'static str],
}

const SCENARIOS: &[Scenario] = &[
    // Fault injection (kernel/src/failpoint.rs): programs run while every
    // n-th kernel heap allocation (or frame allocation) fails. They end
    // however they end; the kernel must survive (no panic, no hang, every
    // address space freed). `cargo xtask fault-test` does the same in
    // gala-um, with many more rules.
    Scenario {
        name: "failpoint_heap",
        cmdline: "qemu_exit run=hello.exe,file_test.exe,kernel32_test.exe,process_test.exe \
                  failpoint=heap:every:211",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["run: end process_test.exe", "failpoint: heap ", "gala: boot ok"],
    },
    Scenario {
        name: "failpoint_frame",
        cmdline: "qemu_exit run=hello.exe,file_test.exe,kernel32_test.exe,process_test.exe \
                  failpoint=frame:every:53",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["run: end process_test.exe", "failpoint: frame ", "gala: boot ok"],
    },
    Scenario {
        name: "failpoint_random",
        cmdline: "qemu_exit run=hello.exe,file_test.exe,kernel32_test.exe,process_test.exe \
                  failpoint=heap:random:5,frame:random:10 failseed=7",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["run: end process_test.exe", "failpoint: heap ", "gala: boot ok"],
    },
    Scenario {
        // Every other disk request fails while persist_test runs and its
        // changes are written to the disk log (`persist`).
        name: "failpoint_disk",
        cmdline: "qemu_exit persist run=persist_test.exe failpoint=disk_io:every:2",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["run: end persist_test.exe", "failpoint: disk_io ", "gala: boot ok"],
    },
    // Added latency (live mocking, R7): every disk request waits 3 ms
    // first; the program works as before, only slower.
    Scenario {
        name: "failpoint_disk_delay",
        cmdline: "qemu_exit persist run=persist_test.exe failpoint=disk_io:delay:3",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "persist_test: first boot",
            "exited with status 0x00000000",
            "delayed by 3 ms",
            "gala: boot ok",
        ],
    },
    Scenario {
        // C++ exception handling as MSVC compiles it, with vcruntime140
        // (user/cxx_eh_test: clang-built, checked-in binary).
        name: "cxx_eh_test",
        cmdline: "qemu_exit run=cxx_eh_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["cxx_eh_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "timer_test",
        cmdline: "qemu_exit run=timer_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["timer_test: ok", "exited with status 0x00000000"],
    },
    // Clock control (ROADMAP R7; kernel/src/clockctl.rs): GalaClockControl
    // freezes, scales and advances the clock; every view of it and the
    // waits in progress follow. Off without `clock=control`; `clock_boot`
    // starts frozen at 2038-01-19 03:14:08 UTC from the boot flag.
    Scenario {
        name: "clock_test",
        cmdline: "qemu_exit clock=control run=clock_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["clock_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "clock_off",
        cmdline: "qemu_exit run=clock_test.exe args=off",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["clock_test: ok (off)", "exited with status 0x00000000"],
    },
    Scenario {
        name: "clock_boot",
        cmdline: "qemu_exit clock=control,freeze,system:2147483648 run=clock_test.exe args=boot",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["clock_test: ok (boot)", "exited with status 0x00000000"],
    },
    Scenario {
        name: "loader_test",
        cmdline: "qemu_exit run=loader_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["loader_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "smp_test",
        // QEMU runs with -smp 2 (see `qemu`).
        cmdline: "qemu_exit run=smp_test.exe args=cpus=2",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["smp: 2 CPUs online", "smp_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "smpsched_test",
        cmdline: "qemu_exit run=smpsched_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["smpsched_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "smpmem_test",
        cmdline: "qemu_exit run=smpmem_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["smpmem_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // Copy-on-write faults on one page from two processors at once.
        name: "smpcow_test",
        cmdline: "qemu_exit run=smpcow_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["smpcow_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // Commit, decommit, recommit and protect while another processor
        // writes the pages around them.
        name: "vmcycle_test",
        cmdline: "qemu_exit run=vmcycle_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["vmcycle_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // Code rewritten where an instruction straddles a page boundary
        // (a JIT patching an object address in compiled code): the next
        // call must run the new instruction. Fails under QEMU 8.1.0-8.2.5
        // TCG (the Java corruption, WORKFLOW "QEMU"); xtask's pinned QEMU
        // passes it.
        name: "straddle_test",
        cmdline: "qemu_exit run=straddle_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["straddle_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // Code rewritten on one processor and run on the other (a JIT's
        // garbage collector patching compiled code).
        name: "xmodify_test",
        cmdline: "qemu_exit run=xmodify_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["xmodify_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // The clock under a busy host (ARCHITECTURE "Interrupts and
        // time", calibration): booted with every host CPU kept busy
        // (HOST_LOAD_SCENARIOS) and `calstall`, which stalls the boot CPU
        // inside the calibration as a descheduled vCPU would be; the
        // guest's time must still pass at the host's rate: xtask times
        // the program's two `uptime_ms=` marks (clockcheck.rs) and the
        // kernel checks itself against the RTC.
        name: "clockcal_test",
        cmdline: "qemu_exit calstall run=clockcal_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "clockcal: stall injected",
            "tsc: ",
            // Agrees, or inconclusive when the host is so busy that no
            // poll lands near a second edge; xtask's timing judges.
            "clock: RTC check: ",
            "clockcal_test: ok",
            "exited with status 0x00000000",
            "xtask: clock: clockcal_test: guest",
            "xtask: clock: clock: guest",
        ],
    },
    Scenario {
        // The RTC check's recalibration: `calwrong` makes the first
        // calibration 20x low (guest time runs 20x fast); the kernel's
        // check against the CMOS RTC must say so, recalibrate, and the
        // program (which waits for the check under `calwrong`) must then
        // see the host's rate.
        name: "clockcal_recal",
        cmdline: "qemu_exit calwrong run=clockcal_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "clockcal: calibration made 20x low",
            "clock: WRONG: the TSC was calibrated at",
            "clock: recalibrated: TSC",
            "clockcal_test: ok",
            "exited with status 0x00000000",
            "xtask: clock: clockcal_test: guest",
        ],
    },
    Scenario {
        // A boot test that times out explains itself: statedump_test hangs
        // on purpose (one thread waits for ever, one spins in user code);
        // at the timeout xtask injects an NMI and the kernel's state dump
        // must show every CPU, the flight recorder, both threads and the
        // statistics (kernel/src/statedump.rs). In gala-um the same dump
        // comes on SIGQUIT (um-test checks the lines both have).
        name: "statedump_test",
        cmdline: "qemu_exit run=statedump_test.exe",
        exit_status: XTASK_DUMPS,
        expect: &[
            "statedump_test: waiting",
            "statedump: begin (NMI",
            "statedump: cpu 0 ",
            "statedump: cpu 1 ",
            "statedump: locks: scheduler free",
            "statedump: heap ",
            "flight: the last",
            "statedump: NMI part done",
            "statedump: threads",
            "NtWaitForSingleObject from rip",
            "Event signal 0",
            "in user mode at rip",
            // The keyed-event waiter: its caller and its key.
            "NtWaitForKeyedEvent from rip",
            "(statedump_test.exe+0x",
            "(value there 0x6a1a); queued for it: (waiters, releasers, all entries) Some((1, 0, 1))",
            "statedump: end",
            "the guest's state dump (NMI) took",
        ],
    },
    Scenario {
        // A semaphore released and waited on from two processors at once.
        name: "smpsem_test",
        cmdline: "qemu_exit run=smpsem_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["smpsem_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // Races found by the hygiene sweep (2026-11-03): WaitAll with an
        // object twice, a timer's expiry vs. a re-arm, a name vs. its
        // handle count, a process's handles vs. its signal.
        name: "smprace_test",
        cmdline: "qemu_exit run=smprace_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["smprace_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // N1a: a kernel TCP echo server on port 7 behind QEMU's user-mode
        // network; xtask connects through a host port forward (see
        // `net_client`) and appends its verdict to the log.
        name: "net_echo",
        cmdline: "qemu_exit net_echo",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "net: virtio-net",
            "net: link up, 10.0.2.15/24",
            "net_echo: echoed 65536 bytes",
            "xtask: net client: echo ok",
        ],
    },
    Scenario {
        // N1b: Winsock over gala's ws2_32 and AFD: loopback checks, then
        // (`serve`) an echo server on port 7 for xtask's client.
        // The far ends of `sshd_forward` without ssh: both ends of a
        // loopback connection in one process (sshfwd_test without
        // arguments, as run-all.ps1 runs it on Windows).
        name: "sshfwd_test",
        cmdline: "qemu_exit run=sshfwd_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["sshfwd_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "winsock_test",
        cmdline: "qemu_exit run=winsock_test.exe args=serve",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "winsock_test: echoed 65536 bytes",
            "xtask: net client: echo ok",
            "winsock_test: ok",
            "exited with status 0x00000000",
        ],
    },
    Scenario {
        // N3 step 1: IPv6 sockets over ::1 (ARCHITECTURE "Plan: IPv6").
        name: "ipv6_test",
        cmdline: "qemu_exit run=ipv6_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["ipv6_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // A page from the host network: hostnet_test fetches
        // http://10.0.2.100/, which QEMU's user network hands to xtask's
        // `serve-page` on the host (HOST_PAGE_SCENARIOS).
        name: "hostnet",
        cmdline: "qemu_exit run=hostnet_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "hostnet_test: ok",
            "exited with status 0x00000000",
            "xtask: serve-page: \"GET / HTTP/1.0\"",
        ],
    },
    Scenario {
        // The event stream (see EVENT_EXPECT) while import_test runs.
        name: "events",
        cmdline: "qemu_exit events=all run=import_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["import_test: ok", "events: "],
    },
    Scenario {
        // The event stream's process tree and threads (see EVENT_EXPECT)
        // while process_test starts its children.
        name: "events_process",
        cmdline: "qemu_exit events=process,thread,image run=process_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["process_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // The event stream's pipe and completion-port events (see
        // EVENT_EXPECT) while pipe_test runs.
        name: "events_ipc",
        cmdline: "qemu_exit events=ipc,thread run=pipe_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["pipe_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // Profiler samples while bench runs (see EVENT_EXPECT).
        name: "events_sample",
        // `sampleprobe`: the boot thread spins with interrupts off before
        // the program; samples (NMIs from the other CPU) must land inside.
        cmdline: "qemu_exit events=sample,image,thread,process sampleprobe run=bench.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["sampleprobe: ok", "bench: done", "exited with status 0x00000000"],
    },
    Scenario {
        // System-call events of processes chosen by their command line
        // (see EVENT_EXPECT).
        name: "events_match",
        cmdline: "qemu_exit events=syscall,process events_match=child run=process_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["process_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // System-call events of one process only (see EVENT_EXPECT).
        name: "events_pid",
        cmdline: "qemu_exit events=syscall,process events_pid=4 run=process_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["process_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // Smoke test only; `cargo xtask bench` collects the numbers.
        name: "bench",
        cmdline: "qemu_exit run=bench.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["bench: null_syscall ns_per_op", "bench: done", "exited with status 0x00000000"],
    },
    Scenario {
        name: "kernel32_test",
        cmdline: "qemu_exit run=kernel32_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "kernel32_test: hello from kernel32",
            "kernel32_test: ok",
            "exited with status 0x00000000",
        ],
    },
    Scenario {
        name: "missing_import_test",
        cmdline: "qemu_exit run=missing_import_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "missing_import_test.exe finished with status 0xc0000139",
            r#"ledger: {"service":"kernel32.dll!GalaNoSuchFunctionOne","what":"missing export""#,
            r#"ledger: {"service":"kernel32.dll!GalaNoSuchFunctionTwo","what":"missing export""#,
        ],
    },
    Scenario {
        name: "dpapi_test",
        cmdline: "qemu_exit run=dpapi_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["dpapi_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "cert_test",
        cmdline: "qemu_exit run=cert_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["cert_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "sspi_test",
        cmdline: "qemu_exit run=sspi_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["sspi_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "clusapi_test",
        cmdline: "qemu_exit run=clusapi_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["clusapi_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "netapi32_test",
        cmdline: "qemu_exit run=netapi32_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["netapi32_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "pdh_test",
        cmdline: "qemu_exit run=pdh_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["pdh_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "setupapi_test",
        cmdline: "qemu_exit run=setupapi_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["setupapi_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "rpcrt4_test",
        cmdline: "qemu_exit run=rpcrt4_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["rpcrt4_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "unimplemented_test",
        cmdline: "qemu_exit run=unimplemented_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "unimplemented_test.exe finished with status 0xc0000002",
            r#"ledger: {"service":"xolehlp.dll!DtcGetTransactionManagerExW","what":"not implemented""#,
        ],
    },
    Scenario {
        name: "kernel32_sys_test",
        cmdline: "qemu_exit run=kernel32_sys_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["kernel32_sys_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // P1 (JDK): a compatibility manifest declaring Windows 10 gets the
        // real version from GetVersionEx (java.exe has one).
        name: "manifest_test",
        cmdline: "qemu_exit run=manifest_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["manifest_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // P1 (JDK): static TLS (the PE TLS directory) for the executable
        // and a DLL loaded later; TLS callbacks and thread notifications.
        name: "tls_test",
        cmdline: "qemu_exit run=tls_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["tls_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // P1 (JDK): the thread pool and what else msvcp140 imports.
        name: "kernel32_pool_test",
        cmdline: "qemu_exit run=kernel32_pool_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["kernel32_pool_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // P1 (JDK): user32, ws2_32 ordinals, comctl32, powrprof, shell32,
        // version.
        name: "win32_misc_test",
        cmdline: "qemu_exit run=win32_misc_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["win32_misc_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // P1 (JDK): SIDs, ACLs, security descriptors, the process token.
        name: "advapi32_test",
        cmdline: "qemu_exit run=advapi32_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["advapi32_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // Security model S1 (docs/SECURITY.md): access tokens as kernel
        // objects (user/token_test).
        name: "token_test",
        cmdline: "qemu_exit run=token_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["token_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // Security model S2 (docs/SECURITY.md): security descriptors kept
        // on files, keys and kernel objects (user/security_test).
        name: "security_test",
        cmdline: "qemu_exit run=security_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["security_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // Security model S3 (docs/SECURITY.md): access checks at open,
        // with a second account (user/access_test; its account seeded).
        name: "access_test",
        cmdline: "qemu_exit run=access_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["access_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // Security model S4 (docs/SECURITY.md): per-user processes and
        // pipe impersonation (user/logon_test; its account seeded).
        name: "logon_test",
        cmdline: "qemu_exit run=logon_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["logon_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // Security model S4 (docs/SECURITY.md): a program started as
        // SYSTEM (`as=system`, the services' account; user/system_test).
        name: "system_test",
        cmdline: "qemu_exit as=system run=system_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["system_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // Sessions, ROADMAP RS steps B1 and B2 (user/session_test): run
        // as SYSTEM (session 0), it starts a child in session 1 and checks
        // the session ids and the per-session named objects; then a
        // second child in session 1 and the parent each make a window
        // the other cannot find, enumerate or message (the log says
        // where the HWND asked for is).
        name: "session_test",
        cmdline: "qemu_exit as=system events=process,window run=session_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "session: 1 made, named objects in \\Sessions\\1\\BaseNamedObjects",
            "session 0): starting at",
            "session 1): starting at",
            "desktop: session 0 keeps its windows (1280x800), shown by none (no screen and no presenter=)",
            "desktop: session 1 keeps its windows (1280x800), shown by none (no screen and no presenter=)",
            "in session 0: not visible across sessions",
            "in session 1: not visible across sessions",
            "session_test: ok",
            "exited with status 0x00000000",
        ],
    },
    Scenario {
        // Presenters, ROADMAP RS step B3a (user/presenter_test): as SYSTEM
        // (session 0) it starts a child in session 2, which nothing
        // shows; the child's window is kept with its pixels, a Test
        // presenter attached renders it to memory, which the parent reads
        // back (GalaSession) and clicks (injected input), detached and
        // attached again it shows what the child painted meanwhile. Then
        // the console switch: a second child's window in session 1 on
        // the screen, `gala attach 2` puts session 2 there, back with
        // GalaSession; the screen check (SCREEN_SCENARIOS) sees each and
        // clicks through PS/2. UEFI: a machine with a screen, which shows
        // session 1.
        name: "presenter_test",
        cmdline: "qemu_exit as=system wallpaper=night events=window run=presenter_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "session: 2 made, named objects in \\Sessions\\2\\BaseNamedObjects",
            "desktop: session 2 is live (1280x800), shown by none (the console shows session 1)",
            "presenter_test: child painted",
            "presenter: test frame for session 2: 1280x800 at",
            "desktop: session 2 is now shown by test (1280x800; was none)",
            "presenter_test: frame shows the window",
            "presenter_test: injected click reached the window",
            "desktop: session 2 is now shown by none (1280x800; was test)",
            "presenter_test: kept while not shown",
            "input for session 2 refused: 0xc0000184 (only a desktop a Test presenter shows",
            "presenter_test: session 1 shows its window",
            "presenter_test: a click on the console reached session 1",
            "desktop: the console now shows session 2 (1280x800; was session 1, which keeps its 1 windows shown by none)",
            "presenter_test: the console shows session 2",
            "presenter_test: a click on the console reached session 2",
            "desktop: the console now shows session 1 (1280x800; was session 2, which keeps its 1 windows shown by none)",
            "presenter_test: the console shows session 1 again",
            "presenter_test: a click reached session 1 again",
            "presenter_test: ok",
            "exited with status 0x00000000",
        ],
    },
    Scenario {
        // Window stations and desktops, ROADMAP RS step B3a
        // (user/winsta_test): the boot user's program and its children
        // are on WinSta0\Default of session 1.
        name: "winsta_test",
        cmdline: "qemu_exit events=window run=winsta_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "is on WinSta0\\Default (its logon session's)",
            "is on WinSta0\\Default (STARTUPINFO.lpDesktop)",
            "is on WinSta0\\Default (its parent's, pid",
            "asked for window station \"dummy name\": 0xc0000034",
            "winsta_test: ok",
            "exited with status 0x00000000",
        ],
    },
    Scenario {
        // The same program run as SYSTEM in session 0, as services are:
        // the SYSTEM logon session's window station, not visible.
        name: "winsta_service",
        cmdline: "qemu_exit as=system run=winsta_test.exe args=service",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "is on Service-0x0-3e7$\\Default (its logon session's)",
            "winsta_test: ok",
            "exited with status 0x00000000",
        ],
    },
    Scenario {
        // Security model S4 (docs/SECURITY.md): LSA logons as sshd makes
        // them, run as SYSTEM (user/lsa_test).
        name: "lsa_test",
        cmdline: "qemu_exit as=system run=lsa_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["lsa_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // Security model S5 (docs/SECURITY.md): restricted tokens and
        // integrity levels as PostgreSQL uses them, run as SYSTEM
        // (user/restricted_test).
        name: "restricted_test",
        cmdline: "qemu_exit as=system run=restricted_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["restricted_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // gala's cmd.exe (user/cmd): /c with its quote rules,
        // redirections, echo, an unknown command (user/cmd_test).
        name: "cmd_test",
        cmdline: "qemu_exit run=cmd_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["cmd_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // Security model S4 (docs/SECURITY.md): user profiles, loaded as
        // SYSTEM (user/profile_test).
        name: "profile_test",
        cmdline: "qemu_exit as=system run=profile_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["profile_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // P1 (Elasticsearch, Java's WatchService): directory change
        // notification.
        name: "dirchange_test",
        cmdline: "qemu_exit run=dirchange_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["dirchange_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // P1 (Elasticsearch, Java NIO): AFD polls as wepoll makes them.
        name: "afd_poll_test",
        cmdline: "qemu_exit run=afd_poll_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["afd_poll_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // P1 (Elasticsearch): network interfaces and addresses (Java's
        // NetworkInterface).
        name: "iphlpapi_test",
        cmdline: "qemu_exit run=iphlpapi_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["iphlpapi_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // P1 (Elasticsearch): keyed events (the JDK's wepoll selector).
        name: "keyed_event_test",
        cmdline: "qemu_exit run=keyed_event_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["keyed_event_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // P1 (PowerShell, .NET, JDK): the registry (advapi32's Reg* and
        // ntdll's Nt*Key over the kernel's key tree).
        name: "registry_test",
        cmdline: "qemu_exit run=registry_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["registry_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // P1 (PowerShell): the console (CONOUT$, CONIN$) behind redirected
        // standard handles.
        name: "console_test",
        cmdline: "qemu_exit run=console_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["console_test: split-line", "console_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // P1 (PowerShell): CNG random numbers and hashes (.NET's crypto).
        name: "bcrypt_test",
        cmdline: "qemu_exit run=bcrypt_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["bcrypt_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // P1 (.NET): the COM basics CoreCLR imports (ole32, oleaut32).
        name: "com_test",
        cmdline: "qemu_exit run=com_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["com_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // P1 (.NET): what CoreCLR imports from kernel32.
        name: "kernel32_clr_test",
        cmdline: "qemu_exit run=kernel32_clr_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["kernel32_clr_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // P1 (JDK): memory, process, file and version functions.
        name: "kernel32_system_test",
        cmdline: "qemu_exit run=kernel32_system_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["kernel32_system_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // BR stage 1: the Win32 functions Chromium's start-up calls that
        // gala lacked (mitigation policies, process/thread information,
        // WaitOnAddress, CompareObjectHandles, GetAddrInfoExW, ...).
        name: "browser_api_test",
        cmdline: "qemu_exit run=browser_api_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["browser_api_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // GPU G0: the Direct3D stubs fail cleanly (no adapter).
        name: "gpu_stub_test",
        cmdline: "qemu_exit run=gpu_stub_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["gpu_stub_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // The PnP configuration manager's queries (cfgmgr32): gala has no
        // device tree, only its root.
        name: "cfgmgr32_test",
        cmdline: "qemu_exit run=cfgmgr32_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["cfgmgr32_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // GPU G2: Vulkan on Mesa's lavapipe through the Khronos loader (a
        // triangle read back, then presented with VK_KHR_win32_surface),
        // on a headless desktop whose frame the screen check reads.
        name: "vk_test",
        cmdline: "qemu_exit presenter=test run=vk_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "vk_test: device llvmpipe",
            "vk_test: read back 0x336699 0xffcc00",
            "vk_test: presented",
            "xtask: screen ok",
            "vk_test: ok",
            "exited with status 0x00000000",
        ],
    },
    Scenario {
        // GPU G2: OpenGL on Mesa's llvmpipe (WGL: a pixel format, a
        // context, a frame read back and presented), on a headless desktop
        // (`presenter=test`, BIOS) whose frame the screen check reads.
        name: "gl_test",
        cmdline: "qemu_exit presenter=test run=gl_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "gl_test: renderer llvmpipe",
            "gl_test: presented",
            "xtask: screen ok",
            "gl_test: ok",
            "exited with status 0x00000000",
        ],
    },
    Scenario {
        // BR stage 1: DirectWrite's system font collection (gala's
        // dwrite.dll over Selawik), as Chromium's browser and Skia use it.
        name: "dwrite_test",
        cmdline: "qemu_exit run=dwrite_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["dwrite_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // Pseudo consoles (ConPTY): conpty_test plays a terminal for a
        // child console program attached to a pseudo console (gala's
        // conhost.exe in pseudo-console mode, in System32 as on Windows):
        // line editing, size and resize, key events from VT input,
        // Ctrl+C, ClosePseudoConsole.
        name: "conpty_test",
        cmdline: "qemu_exit run=conpty_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["conpty_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // A console program in a console window (gala's conhost.exe draws
        // its session): plain text, VT colours and UTF-8.
        name: "console_window",
        cmdline: "qemu_exit wallpaper=night run=console_window_test.exe console=console_window_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "console_window_test: printed",
            "xtask: screen ok",
            "console_window_test: read hi",
            "console_window_test: console ok",
            "console_window_test: key x",
            "console_window_test: raw z",
            "console_window_test: ctrl-c",
            "console_window_test: done",
            "exited with status 0x00000000",
        ],
    },
    Scenario {
        // The window manager's first milestone: a Win32 window painted on
        // the desktop (UEFI: a framebuffer); SCREEN_SCENARIOS checks the
        // screen.
        name: "gui_test",
        // The night wallpaper, so the screen check knows its colours.
        cmdline: "qemu_exit wallpaper=night run=gui_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "gui_test: painted",
            "gui_test: click",
            "gui_test: typed hi",
            "gui_test: keys typed while busy came in order",
            "gui_test: menu 202",
            "gui_test: combo picked Blue",
            "xtask: screen ok",
            "gui_test: ok",
            "exited with status 0x00000000",
        ],
    },
    Scenario {
        // gui_test on a headless desktop (ROADMAP RS B3a): BIOS, no
        // screen; `presenter=test` shows the console session's desktop in
        // a frame in memory, which the same screen check reads (QEMU's
        // memsave at the address the kernel logs) instead of screendumps;
        // the keyboard and mouse still come through PS/2.
        name: "gui_test_headless",
        cmdline: "qemu_exit wallpaper=night presenter=test run=gui_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "desktop: session 1 is live (1280x800), shown by test",
            "presenter: test frame for session 1: 1280x800 at",
            "gui_test: painted",
            "gui_test: click",
            "gui_test: typed hi",
            "gui_test: keys typed while busy came in order",
            "gui_test: menu 202",
            "gui_test: combo picked Blue",
            "xtask: screen ok",
            "gui_test: ok",
            "exited with status 0x00000000",
        ],
    },
    Scenario {
        // gui_test with the screen on virtio-gpu (`display=virtio`; QEMU
        // gets `-vga none -device virtio-gpu-pci`: GPU_SCENARIOS), the same
        // screen check (SCREEN_SCENARIOS). ARCHITECTURE "Plan: graphics and
        // GPU", stage G1.
        name: "virtio_gpu",
        cmdline: "qemu_exit wallpaper=night display=virtio run=gui_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "display: virtio-gpu at",
            "1280x800",
            "gui_test: painted",
            "gui_test: click",
            "gui_test: typed hi",
            "gui_test: keys typed while busy came in order",
            "gui_test: menu 202",
            "gui_test: combo picked Blue",
            "xtask: screen ok",
            "gui_test: ok",
            "exited with status 0x00000000",
            "display: virtio-gpu 1280x800:",
        ],
    },
    Scenario {
        // The same with the device reset under the driver after its 30th
        // flush (invariant 4): the driver sets it up again from the memory
        // it kept, and the screen check still passes.
        name: "virtio_gpu_reset",
        cmdline: "qemu_exit wallpaper=night display=virtio gpureset=30 run=gui_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "display: virtio-gpu at",
            "display: resetting virtio-gpu under the driver (gpureset=30)",
            "display: virtio-gpu was reset or gave up",
            "gui_test: painted",
            "xtask: screen ok",
            "gui_test: ok",
            "exited with status 0x00000000",
            "1 resets",
        ],
    },
    Scenario {
        // ROADMAP SD "CLI parity": a GUI program driven by the `gala`
        // command alone (start, list, snap, maximize/restore, move,
        // minimize/activate, wallpaper, ps, close, kill); SCREEN_SCENARIOS
        // checks shasha once the window is snapped left.
        name: "gala_cli",
        cmdline: "qemu_exit wallpaper=night run=gala_cli_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "gala_cli_test target: up",
            "gala_cli_test target: snapped left",
            "xtask: screen ok",
            "gala_cli_test target: key g",
            "gala_cli_test target: maximized",
            "gala_cli_test target: restored",
            "gala_cli_test target: snapped right",
            "gala_cli_test target: moved",
            "gala_cli_test target: minimized",
            "desktop: wallpaper ribbons",
            "gala_cli_test target: SC_CLOSE",
            "gala_cli_test target: WM_CLOSE",
            "gala_cli_test target: ok",
            "gala_cli_test: ok",
            "exited with status 0x00000000",
        ],
    },
    Scenario {
        // ROADMAP PK stages 1 and 2: `gala install` of a zip package from
        // a local winget-pkgs snapshot (C:\pk_source), its command run
        // from PATH through its shim, listed, upgraded, uninstalled; a
        // conflicting install rolled back, a bad SHA-256 refused. On the
        // winget resolver: the installer selected as winget would, a
        // dependency installed first, `--scope`, an upgrade keeping the
        // installed scope. The install log is copied to the serial log
        // (`logtee:`).
        name: "pk_install_zip",
        cmdline: "qemu_exit logtee=C:/Users/gala/AppData/Local/gala/logs run=pk_install_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "installed id=GalaTest.PkHello version=1.0.0",
            "rollback remove file",
            "upgraded id=GalaTest.PkHello from=1.0.0 to=1.1.0",
            "uninstalled key=GalaTest.PkHello_Microsoft.Winget.Source_8wekyb3d8bbwe",
            "plan install GalaTest.PkLib 1.0.0 installer 1 (for GalaTest.PkDep)",
            "installed id=GalaTest.PkLib version=1.0.0",
            "upgraded id=GalaTest.PkScoped from=1.0.0 to=1.1.0",
            "error requires-admin administrator=no",
            "pk_install_test: ok",
            "exited with status 0x00000000",
        ],
    },
    Scenario {
        // ROADMAP PK: a machine-scope portable install (winget's
        // %ProgramFiles%\WinGet layout, the HKLM entry, the machine's
        // PATH) with administrator rights (`as=system`): installed, run,
        // listed, upgraded keeping machine scope, uninstalled. The zip
        // run checks the boot user is refused one (exit 6).
        name: "pk_install_machine",
        cmdline: "qemu_exit as=system logtee=C:/Users/gala/AppData/Local/gala/logs run=pk_install_test.exe args=machine",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "elevation administrator=yes",
            "registered key=HKLM\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\GalaTest.PkScoped_Microsoft.Winget.Source_8wekyb3d8bbwe scope=machine",
            "upgraded id=GalaTest.PkScoped from=1.0.0 to=1.1.0",
            "pk_install_test: ok",
            "exited with status 0x00000000",
        ],
    },
    Scenario {
        // ROADMAP PK stage 3: `gala install` of a real Inno Setup
        // installer (Inno Setup 7.1.1's own, x64; winget-pkgs' manifest
        // JRSoftware.InnoSetup.7) from the snapshot's mirror: run with
        // winget's Inno switches (--silent: /VERYSILENT, the manifest's
        // /CURRENTUSER for --scope user), found through its ProductCode's
        // uninstall entry, its compiler run, listed, uninstalled through
        // its QuietUninstallString.
        name: "pk_install_inno",
        cmdline: "qemu_exit logtee=C:/Users/gala/AppData/Local/gala/logs run=pk_install_test.exe args=inno",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "innosetup-7.1.1-x64.exe\" /SP- /VERYSILENT /SUPPRESSMSGBOXES /NORESTART",
            "found-by=ProductCode",
            "installed id=JRSoftware.InnoSetup.7 version=7.1.1",
            "arp-entry-gone key=HKCU",
            "pk_install_test: ok",
            "exited with status 0x00000000",
        ],
    },
    Scenario {
        // WOW64 stage 4 under ROADMAP PK: `gala install` of an x86 Inno
        // Setup installer (Inno Setup 6.5.4's own; winget-pkgs' manifest
        // JRSoftware.InnoSetup, whose installers are all x86, chosen on
        // x64 as winget does) from the snapshot's mirror: its 32-bit Setup,
        // compiler, IDE (/ASSOC) and uninstaller under WOW64, otherwise as
        // pk_install_inno.
        name: "pk_install_inno32",
        cmdline: "qemu_exit logtee=C:/Users/gala/AppData/Local/gala/logs run=pk_install_test.exe args=inno32",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "innosetup-6.5.4.exe\" /SP- /VERYSILENT /SUPPRESSMSGBOXES /NORESTART",
            "found-by=ProductCode",
            "installed id=JRSoftware.InnoSetup version=6.5.4",
            "arp-entry-gone key=HKCU",
            "pk_install_test: ok",
            "exited with status 0x00000000",
        ],
    },
    Scenario {
        // ROADMAP PK stage 3: `gala install` of a real NSIS installer
        // (user/pk_install_test/nsis, x64) from the snapshot: run with /S
        // (and /D=), found through its uninstall entry, upgraded by the
        // next version's installer, uninstalled through its
        // UninstallString with /S (gala waits for the entry to go); then
        // installed without gala, matched to its package by name and
        // publisher (winget list) and upgraded.
        name: "pk_install_nsis",
        cmdline: "qemu_exit logtee=C:/Users/gala/AppData/Local/gala/logs run=pk_install_test.exe args=nsis",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "pknsis-1.0.0.exe\" /S",
            "found-by=the one new entry",
            "upgraded id=GalaTest.PkNsis from=1.0.0 to=1.1.0",
            "arp-entry-gone key=HKCU",
            "matched key=GalaTest.PkNsis by=NameAndPublisher",
            "pk_install_test: ok",
            "exited with status 0x00000000",
        ],
    },
    Scenario {
        // ROADMAP PK and TLS step 4: `gala install` downloads an
        // installer over HTTPS (winhttp on Schannel) from schannel_test's
        // Nginx in the guest: the release URL redirects (302) to the
        // file, as GitHub's do; SHA-256 checked, installed, run,
        // uninstalled; an untrusted certificate and an unreachable
        // github.com fail with the reason; the public CA bundle is in
        // crypt32's ROOT store. Manifests over HTTPS: a winget-pkgs tree
        // Nginx serves (JSON listings); the default source, GitHub's API,
        // is not reachable from the VM.
        name: "pk_install_https",
        cmdline: "qemu_exit start=nginx.exe start_args=-p%20C:/nginx/%20-c%20conf/nginx.conf \
                  logtee=C:/Users/gala/AppData/Local/gala/logs run=pk_install_test.exe args=https",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "fetch status=200 url=https://localhost:8443/objects/pkhttps-1.0.0.zip redirected-from=https://localhost:8443/releases/download/v1.0.0/pkhttps-1.0.0.zip",
            "installed id=GalaTest.PkHttps version=1.0.0",
            "fetch error step=WinHttpSendRequest code=12175",
            "source read url=https://localhost:8443/winget-pkgs/manifests/g/GalaTest/PkHttps/1.0.0/GalaTest.PkHttps.yaml",
            "pk_install_test: ok",
            "exited with status 0x00000000",
        ],
    },
    Scenario {
        // ntdll's run-time library and the calls Git Bash's msys-2.0.dll
        // imports.
        name: "rtl_test",
        cmdline: "qemu_exit run=rtl_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["rtl_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // D1: raw sector I/O on a virtio-blk disk (DISK_SCENARIOS attaches
        // it and checks what the program wrote). As SYSTEM: raw disks are
        // for administrators.
        name: "disk_test",
        cmdline: "qemu_exit as=system run=disk_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["disk_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // R1 invariant 4: disk_test with the device reset under the
        // driver at the third request; the driver sets it up again and
        // posts the request again (DISK_SCENARIOS checks the sectors).
        name: "disk_reset",
        cmdline: "qemu_exit diskreset=3 as=system run=disk_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "disk: the device was reset or gave up; setting it up again",
            "disk_test: ok",
            "exited with status 0x00000000",
        ],
    },
    Scenario {
        // R4: the crash record kept across reboots (DISK_SCENARIOS: the
        // first boot panics on purpose and leaves it; this one prints and
        // clears it, then runs normally).
        name: "crash_record",
        cmdline: "qemu_exit persist crashtest run=hello.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "crash: a record from the previous boot",
            "crash: KERNEL PANIC",
            "crashtest: a deliberate panic",
            "crash: flight: the last",
            "exited with status 0x00000000",
        ],
    },
    Scenario {
        // D2: files kept across a reboot (`persist`: C: is kept on the
        // disk); DISK_SCENARIOS boots it twice, this is the second boot.
        name: "persist_test",
        cmdline: "qemu_exit persist run=persist_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["persist_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // P1 (JDK): locale and time (en-US, UTC).
        name: "kernel32_locale_test",
        cmdline: "qemu_exit run=kernel32_locale_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["kernel32_locale_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // P1 (JDK): SRW locks, condition variables, InitOnce, FLS, SLists,
        // semaphores.
        name: "kernel32_sync_test",
        cmdline: "qemu_exit run=kernel32_sync_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["kernel32_sync_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // P1 step 6: what CPython imports from kernel32 (PathCch, Pss
        // snapshots, CopyFile2, SetWaitableTimerEx, pipe state, ...).
        name: "kernel32_py_test",
        cmdline: "qemu_exit run=kernel32_py_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["kernel32_py_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "xstate_test",
        cmdline: "qemu_exit run=xstate_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["xstate_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // A JVM-style safepoint (poll fault, vectored handler, values in
        // registers and on the stack rewritten while stopped) on two
        // processors: no value of an older epoch may survive.
        name: "safepoint_test",
        cmdline: "qemu_exit run=safepoint_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["safepoint_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // TLB shootdowns when the changing thread moves to another
        // processor right after the change: a reader on processor 1 must
        // never keep a decommitted page's old translation.
        name: "shootdown_test",
        cmdline: "qemu_exit run=shootdown_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["shootdown_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "kernel32_thread_test",
        cmdline: "qemu_exit run=kernel32_thread_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["kernel32_thread_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "kernel32_misc_test",
        cmdline: "qemu_exit run=kernel32_misc_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["kernel32_misc_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "prng_test",
        cmdline: "qemu_exit run=prng_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["prng_test: ok", "exited with status 0x00000000"],
    },
    // Record/replay's inputs (kernel/src/replay.rs): random numbers, the
    // performance counter, the system time; `cargo xtask replay-test`
    // records and replays it in gala-um.
    Scenario {
        name: "replay_test",
        cmdline: "qemu_exit run=replay_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["replay_test: ok", "exited with status 0x00000000"],
    },
    // The kernel records the program's inputs on the debug console
    // (`record`); xtask joins them into test-record_kernel.rec.
    Scenario {
        name: "record_kernel",
        cmdline: "qemu_exit record run=replay_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "replay_test: ok",
            "replay: recorded ",
            "xtask: recording ",
            "exited with status 0x00000000",
        ],
    },
    // ... and replays a recording made in gala-um
    // (user/replay_test/replay_test.rec): the same random numbers.
    Scenario {
        name: "replay_kernel",
        cmdline: "qemu_exit replay=replay_test.rec run=replay_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "replay: replaying replay_test.rec",
            "replay_test: random f337cbf22a6ac1871480fcd88e3bf539",
            "replay_test: random c476c1ed3c",
            "replay_test: ok",
            "replay: replayed 0 of 172 clock, 2 of 2 random, 1 of 1 boot time records; 0 inputs live",
            "exited with status 0x00000000",
        ],
    },
    Scenario {
        // A real Go program (user/go_hello): ROADMAP G2.
        name: "go_hello",
        cmdline: "qemu_exit run=go_hello.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["Hello from Go on gala!", "go_hello.exe finished with status 0x00000000"],
    },
    Scenario {
        name: "go_goroutines",
        cmdline: "qemu_exit run=go_goroutines.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["go_goroutines: ok", "go_goroutines.exe finished with status 0x00000000"],
    },
    Scenario {
        name: "go_time",
        cmdline: "qemu_exit run=go_time.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["go_time: ok", "go_time.exe finished with status 0x00000000"],
    },
    Scenario {
        name: "go_gc",
        cmdline: "qemu_exit run=go_gc.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["go_gc: ok", "go_gc.exe finished with status 0x00000000"],
    },
    Scenario {
        name: "go_os",
        cmdline: "qemu_exit run=go_os.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["go_os: ok", "go_os.exe finished with status 0x00000000"],
    },
    Scenario {
        // H2: os/exec with pipes (the program runs itself as children).
        name: "go_exec",
        cmdline: "qemu_exit run=go_exec.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["go_exec: ok", "go_exec.exe finished with status 0x00000000"],
    },
    Scenario {
        // R6: the commit charge, the commit limit and job memory limits.
        name: "commit_test",
        cmdline: "qemu_exit run=commit_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["commit_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // R6: pool and handle quotas: a process leaking handles, then
        // objects, fails at its limits (logged) while another runs on.
        name: "quota_test",
        cmdline: "qemu_exit quota=handles:3000,nonpaged:96K run=quota_test.exe args=3000%2098304",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "quota_test: handle limit at 3000 handles",
            "over its handle limit: 3000 in use + 1 > limit 3000",
            "over its nonpaged pool:",
            "quota_test: port quota at ",
            "quota_test: ok",
            "exited with status 0x00000000",
        ],
    },
    Scenario {
        // R6: sockets' buffers are charged to their process: connections
        // with 1 MiB buffers until one is refused (WSAENOBUFS) while a
        // child connects its own; closing gives the charge back;
        // ProcessQuotaLimits' pool limits need SeIncreaseQuotaPrivilege.
        name: "quota_sockets",
        cmdline: "qemu_exit quota=nonpaged:13M run=quota_test.exe args=sockets%2013631488",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "quota_test: socket quota at 3 connections",
            "over its nonpaged pool:",
            "quota_test: ok",
            "exited with status 0x00000000",
        ],
    },
    Scenario {
        // N2b: CreateProcessW (the test starts itself as a child).
        name: "process_test",
        cmdline: "qemu_exit run=process_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "process_test: child says hello through the inherited console",
            "process_test: ok",
            "exited with status 0x00000000",
        ],
    },
    Scenario {
        // UDP over gala's ws2_32 and AFD datagram endpoints (loopback).
        name: "udp_test",
        cmdline: "qemu_exit run=udp_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["udp_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // udp_test's `serve`: datagrams from the host through QEMU's
        // network (a forwarded UDP port), echoed; a name resolved through
        // QEMU's DNS (`udp_client`).
        name: "udp_echo",
        cmdline: "qemu_exit run=udp_test.exe args=serve",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "udp_test: listening on port 7",
            "xtask: net client: udp ok",
            "udp_test: echoed ",
            "udp_test: ok",
            "exited with status 0x00000000",
        ],
    },
    Scenario {
        // N1c: overlapped Winsock with a completion port (what Go uses).
        // events=net,sample: the sampling interrupts and the net events
        // written under the stack's lock preempt connect's thread often
        // enough that the FD_CONNECT rounds hit the moment the endpoint
        // once lost its handshake's report (every run without the fix).
        name: "winsock_async_test",
        cmdline: "qemu_exit events=net,sample run=winsock_async_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["winsock_async_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // R1 invariant 4: go_http with the network card reset under the
        // driver after the 3rd frame it sent; the driver sets it up again
        // (lost frames are TCP's to send again) and the page still comes.
        name: "net_reset",
        cmdline: "qemu_exit netreset=3 run=go_http.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "net: the device was reset or gave up; setting it up again",
            "xtask: net client: http ok",
            "go_http: ok",
            "exited with status 0x00000000",
        ],
    },
    Scenario {
        // N1c: net/http on gala: a loopback GET, then a page for the host.
        name: "go_http",
        cmdline: "qemu_exit run=go_http.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "go_http: loopback GET ok",
            "go_http: served /",
            "xtask: net client: http ok",
            "go_http: ok",
            "exited with status 0x00000000",
        ],
    },
    Scenario {
        // N2a: Nginx (x64, mingw-w64 build in user/bin), one process,
        // serving a static page; xtask stops the VM after the GET.
        name: "nginx",
        cmdline: "qemu_exit run=nginx.exe args=-p%20C:/nginx/%20-c%20conf/nginx.conf",
        exit_status: XTASK_STOPS,
        expect: &[
            "using the \"select\" event method",
            "nginx/1.30.6",
            "xtask: net client: http ok",
        ],
    },
    Scenario {
        // H2: busybox-w32's sh (user/busybox/build.sh) runs
        //   sh -c "echo hello | cat; echo pipe=$?; echo abc | sed s/b/X/;
        //          echo count=$(printf 'x\ny\nx\n' | grep -c x); exit 3"
        // Pipelines and $(...) are child processes joined by pipes (no fork
        // on Windows). Expected as on Windows (POSIX sh semantics, the same
        // binary): the four lines (pipe=0: cat succeeded), then status 3.
        name: "busybox",
        cmdline: "qemu_exit run=busybox.exe args=sh%20-c%20%22echo%20hello%20%7C%20cat;%20echo%20pipe=$?;%20echo%20abc%20%7C%20sed%20s/b/X/;%20echo%20count=$(printf%20'x%5Cny%5Cnx%5Cn'%20%7C%20grep%20-c%20x);%20exit%203%22",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "hello",
            "pipe=0",
            "aXc",
            "count=2",
            "busybox.exe (pid 4) exited with status 0x00000003",
        ],
    },
    Scenario {
        // W3: gala's gdi32 against documented behaviour, for what
        // wxWidgets (FileZilla) uses (user/gdi32_test/main.c, mingw).
        name: "gdi32_test",
        cmdline: "qemu_exit run=gdi32_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["gdi32_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // W3: gala's comctl32 (image lists) against documented behaviour,
        // for what wxWidgets (FileZilla) uses (user/comctl32_test/main.c,
        // mingw).
        name: "comctl32_test",
        cmdline: "qemu_exit run=comctl32_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["comctl32_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // W3: user32's built-in controls (STATIC, BUTTON, EDIT) and
        // DrawTextW against documented behaviour, for what wxWidgets
        // (FileZilla) creates (user/controls_test/main.c, mingw).
        name: "controls_test",
        cmdline: "qemu_exit run=controls_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["controls_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // AG: oleaut32's dates, currency and string conversions and
        // VarCmp against documented behaviour, for what wxWidgets
        // (FileZilla Server's GUI) and SQL Server LocalDB import
        // (user/oleaut32_test/main.c, mingw).
        name: "oleaut32_test",
        cmdline: "qemu_exit run=oleaut32_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["oleaut32_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // AG: shlwapi's path and string functions against documented
        // behaviour, for what SQL Server LocalDB and wxWidgets import
        // (user/shlwapi_test/main.c, mingw).
        name: "shlwapi_test",
        cmdline: "qemu_exit run=shlwapi_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["shlwapi_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // AG: ole32's PROPVARIANT and STGMEDIUM clean-up, the
        // IRunnableObject helpers and proxy blankets against documented
        // behaviour, for what wxWidgets and SQL Server LocalDB import
        // (user/ole32_test/main.c, mingw).
        name: "ole32_test",
        cmdline: "qemu_exit run=ole32_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["ole32_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // AG: shell32's ShellExecuteExW, dropped files, DragAcceptFiles
        // and SHFileOperationW against documented behaviour, for what
        // wxWidgets imports, and shell links ([MS-SHLLINK] files) for
        // Inno Setup's shortcuts (user/shell32_test/main.c, mingw).
        name: "shell32_test",
        cmdline: "qemu_exit run=shell32_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["shell32_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // PK stage 3: what Inno Setup's IDE (ISIDE.exe /assoc) imports from
        // comdlg32: GetFileTitleW and the dialogs' parameter checks against
        // documented behaviour (user/comdlg32_test/main.c, mingw).
        name: "comdlg32_test",
        cmdline: "qemu_exit run=comdlg32_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["comdlg32_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // AG: dbghelp's ImageNtHeader and SymSrvGetFileIndexInfoW (what
        // SQL Server LocalDB's SqlDumper imports) and normaliz.dll's IDN
        // forwarders (libcurl) against documented behaviour
        // (user/dbghelp_test/main.c, mingw).
        name: "dbghelp_test",
        cmdline: "qemu_exit run=dbghelp_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["dbghelp_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // W3: rich edit controls (msftedit.dll's RICHEDIT50W, riched20.dll's
        // RichEdit20W/A) against documented behaviour, for wxWidgets'
        // wxTE_RICH2 text controls (FileZilla's message log;
        // user/richedit_test/main.c, mingw).
        name: "richedit_test",
        cmdline: "qemu_exit run=richedit_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["richedit_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // W3: gala's user32 against documented behaviour, for what
        // wxWidgets (FileZilla) uses (user/user32_test/main.c, mingw).
        // Without a screen (BIOS, gala-um) its windows are kept on a
        // desktop shown by none, as by the server kernel (ROADMAP SD).
        name: "user32_test",
        cmdline: "qemu_exit run=user32_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "desktop: session 1 keeps its windows (1280x800), shown by none (no screen and no presenter=)",
            "user32_test: ok",
            "exited with status 0x00000000",
        ],
    },
    Scenario {
        // ROADMAP SD: on a machine with no screen and no `presenter=` (a
        // BIOS boot) the kernel keeps windows with nothing showing them
        // (server_profile_windows: the same with a screen). Nothing can
        // show the desktop here, so frames, injected input and the Test
        // and Console presenters are refused.
        name: "noscreen_windows",
        cmdline: "qemu_exit as=system events=window run=presenter_test.exe args=server",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "session: 2 made, named objects in \\Sessions\\2\\BaseNamedObjects",
            "desktop: session 2 keeps its windows (1280x800), shown by none (no screen and no presenter=)",
            "presenter_test: child painted",
            "desktop: session 2 cannot be shown by test: no screen and no presenter=",
            "presenter_test: the window is kept with nothing showing it",
            "presenter_test: the window was closed",
            "presenter_test: ok",
            "exited with status 0x00000000",
        ],
    },
    Scenario {
        // ROADMAP SD: the server profile with a screen (UEFI) keeps
        // windows as the server kernel does and never draws them: the
        // Test and Console presenters are refused.
        name: "server_profile_windows",
        cmdline: "qemu_exit profile=server as=system events=window run=presenter_test.exe args=server",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "profile: server",
            "desktop: session 2 keeps its windows (1280x800), shown by none (server profile)",
            "presenter_test: child painted",
            "desktop: session 2 cannot be shown by test: server profile",
            "presenter_test: the window is kept with nothing showing it",
            "presenter_test: the window was closed",
            "presenter_test: ok",
            "exited with status 0x00000000",
        ],
    },
    Scenario {
        // ROADMAP SD: the server profile never brings up the graphical
        // interface, even with a screen (UEFI: a framebuffer): windows
        // are kept as by the server kernel (shown by none), nothing is
        // drawn, and `console=` starts no console window.
        name: "server_profile",
        cmdline: "qemu_exit profile=server console=user32_test.exe \
                  run=user32_test.exe,gala_cli_test.exe args=,server",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "profile: server",
            "console: no console window for user32_test.exe (server profile)",
            "desktop: session 1 keeps its windows (1280x800), shown by none (server profile)",
            "user32_test: ok",
            "gala_cli_test: ok",
            "exited with status 0x00000000",
        ],
    },
    Scenario {
        // Windows across processes (the kernel's window registry): a
        // child process's windows found and read by its parent
        // (user/windows_across_test/main.c, mingw).
        name: "windows_across_test",
        cmdline: "qemu_exit run=windows_across_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["windows_across_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // User Interface Privilege Isolation: a Low integrity child's
        // messages to its Medium parent's window are blocked but those
        // that always pass and those ChangeWindowMessageFilter(Ex) opens
        // (user/uipi_test/main.c, mingw).
        name: "uipi_test",
        cmdline: "qemu_exit run=uipi_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["uipi_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // WinEvents: SetWinEventHook/UnhookWinEvent/NotifyWinEvent on this
        // thread and across processes (a child's window created, shown,
        // renamed, destroyed), as documented (user/winevent_test/main.c,
        // mingw).
        name: "winevent_test",
        cmdline: "qemu_exit run=winevent_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "winevent_test: own events ok",
            "winevent_test: ok",
            "exited with status 0x00000000",
        ],
    },
    Scenario {
        // UI Automation's client (gala's UIAutomationCore.dll) over the
        // window tree: elements, conditions, Invoke/Value/Toggle
        // (user/uia_test/main.c, mingw).
        name: "uia_test",
        cmdline: "qemu_exit run=uia_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["uia_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // H3: gala's msvcrt against documented msvcrt behaviour
        // (user/msvcrt_test/main.c, built by mingw against msvcrt).
        name: "msvcrt_test",
        cmdline: "qemu_exit run=msvcrt_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["msvcrt_test: _putws", "msvcrt_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // R9 (a): kernel handover to the same build, twice, with no
        // programs running: each kernel writes the handover record, parks
        // the other CPU and enters a fresh copy of itself, which starts
        // from the record. A marker page survives both, the clock goes on
        // without going back, every CPU comes back each time.
        name: "handover_same",
        cmdline: "qemu_exit handover=2",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            // R8: each kernel checks the manifest and the image it hands
            // over to (its own file).
            "manifest: release 1 verified",
            "manifest: the next kernel is /boot/kernel of release 1",
            "handover: generation 0 -> 1: record ",
            "handover: generation 1 (from generation 0), record version 1",
            "handover: clock monotonic",
            "handover: generation 1: 2 of 2 CPUs online again",
            "survived 1 handover(s)",
            "handover: generation 1 -> 2: record ",
            "handover: generation 2 (from generation 1), record version 1",
            "handover: generation 2: 2 of 2 CPUs online again",
            "survived 2 handover(s)",
            "gala: boot ok",
        ],
    },
    Scenario {
        // R9 (b): a program whose threads are all blocked in waits (a
        // sleep, a timer, an event, an alertable wait) asks for a kernel
        // handover (GalaRequestHandover); the kernel hands over once all
        // are blocked, the new kernel rebuilds the process (its pages
        // kept in their frames, its handles, objects and threads) and
        // the threads make their calls again; the program finds its
        // memory, handles and ids as they were, the sleep ending at its
        // deadline. `run=` does not start it again.
        name: "handover_idle",
        cmdline: "qemu_exit handover=1 run=handover_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "handover_test: ready",
            "handover: carrying 1 process(es), 4 thread(s)",
            "handover: generation 0 -> 1: record ",
            "handover: generation 1 (from generation 0), record version 1",
            "handover: resumed process handover_test.exe",
            "run: handover_test.exe goes on from the previous kernel",
            "handover_test: slept",
            "handover_test: ok",
            "exited with status 0x00000000",
            "gala: boot ok",
        ],
    },
    Scenario {
        // R9 (b), twice: the process that came across asks again (its
        // workers have exited by then: handles to exited threads come
        // across too) and goes on in a third kernel.
        name: "handover_idle_twice",
        cmdline: "qemu_exit handover=2 run=handover_test.exe args=twice",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "handover: carrying 1 process(es), 4 thread(s)",
            "handover: generation 1 (from generation 0)",
            // Bounded by the measured pause in the test itself, not by
            // a fixed window here.
            "handover_test: slept",
            "handover: carrying 1 process(es), 1 thread(s)",
            "handover: generation 2 (from generation 1)",
            "handover: resumed process handover_test.exe",
            "handover_test: slept again",
            "handover_test: ok",
            "exited with status 0x00000000",
            "gala: boot ok",
        ],
    },
    // One CPU (ARCHITECTURE "Processors"): `onecpu_*` boot with `-smp 1`
    // (telemetry::vm_cpus) and never share a VM. The kernel must run as
    // well on one CPU as on two: a CPU count, a handover (no CPU to park),
    // a few SMP programs and the benchmarks.
    Scenario {
        name: "onecpu_smp",
        cmdline: "qemu_exit run=smp_test.exe args=cpus=1",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["smp: 1 CPUs online", "smp_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "onecpu_handover",
        cmdline: "qemu_exit handover=2 run=handover_test.exe args=twice",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "parking 0 CPU(s)",
            "handover: generation 1: 1 of 1 CPUs online again",
            "handover: generation 2: 1 of 1 CPUs online again",
            "handover_test: slept again",
            "handover_test: ok",
            "gala: boot ok",
        ],
    },
    Scenario {
        name: "onecpu_programs",
        cmdline: "qemu_exit run=thread_test.exe,smpsched_test.exe,smpmem_test.exe,smpcow_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "thread_test: ok",
            "smpsched_test: one processor, concurrency checks skipped",
            "smpsched_test: ok",
            "smpmem_test: ok",
            "smpcow_test: ok",
            "gala: boot ok",
        ],
    },
    Scenario {
        name: "onecpu_bench",
        cmdline: "qemu_exit run=bench.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["smp: 1 CPUs online", "bench: done", "exited with status 0x00000000"],
    },
    // APs the bootloader missed (ARCHITECTURE "Processors"). `smptest=hide`:
    // the kernel ignores the APs Limine started and starts them itself
    // from the MADT (INIT, startup IPIs, its own trampoline).
    Scenario {
        name: "smp_missed",
        cmdline: "qemu_exit smptest=hide run=smp_test.exe args=cpus=2",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "smp: MADT: 2 enabled CPU(s): 0 1",
            "smp: 2 CPUs online (1 reported by the bootloader, 2 in the MADT",
            "the kernel started 1 itself",
            "smp_test: ok",
            "exited with status 0x00000000",
        ],
    },
    // `smptest=late`: as above, but first the AP runs Limine's trampoline
    // again, as one that answers Limine's startup IPIs after Limine gave up
    // does. Without the identity map the kernel adds to Limine's tables
    // that AP triple-faults and QEMU ends with status 0, without a word
    // (the silent ends seen under load). Then a handover, with the CPU the
    // kernel started.
    Scenario {
        name: "smp_late",
        cmdline: "qemu_exit smptest=late handover=1 run=handover_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "low 4 GiB identity-mapped in the bootloader's tables",
            "it ran the bootloader's trampoline again and parked",
            "smp: 2 CPUs online (1 reported by the bootloader, 2 in the MADT",
            "parking 1 CPU(s)",
            "handover: generation 1: 2 of 2 CPUs online again",
            "handover_test: ok",
            "gala: boot ok",
        ],
    },
    Scenario {
        // R9 (b): what a handover cannot carry yet makes it refuse, with
        // the reason, before anything is stopped for good: the program
        // (holding a handle to the NUL device) goes on in the same kernel.
        // Then the boot's own handover (no programs) is done, and the
        // program runs again in the new kernel, which takes no requests.
        name: "handover_refused",
        cmdline: "qemu_exit handover=1 run=handover_test.exe args=refuse",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "handover: refused: process 4 has a device handle",
            "handover_test: ok (refused)",
            "handover: generation 1 (from generation 0)",
            "handover_test: ok (no handover allowed)",
            "gala: boot ok",
        ],
    },
    Scenario {
        // R9 (c): a program on gala's ntdll and kernel32 (DLL views) asks
        // for a handover while it works: two threads compute in user mode
        // (interrupted, their full user context carried), one is blocked
        // in ReadFile on a pipe, one writes to the console between short
        // sleeps, one waits with a user APC queued, one sleeps alertably
        // for a timer's completion routine, one spins for a critical
        // section. Every thread is stopped at a safe point; the new kernel
        // goes on with all of them and the program finds its results as
        // if nothing had happened.
        name: "handover_k32",
        cmdline: "qemu_exit handover=1 run=handover_k32_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "handover_k32_test: ready",
            "handover: carrying 1 process(es), 10 thread(s)",
            "handover: generation 1 (from generation 0)",
            "handover: resumed process handover_k32_test.exe",
            "handover_k32_test: slept",
            "handover_k32_test: writer wrote lines:",
            "handover_k32_test: ok",
            "exited with status 0x00000000",
            "gala: boot ok",
        ],
    },
    Scenario {
        // R9 (c), twice: the kernel32 program goes on across a second
        // handover (its DLL views, pipes and kernel32 state again).
        name: "handover_k32_twice",
        cmdline: "qemu_exit handover=2 run=handover_k32_test.exe args=twice",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "handover: carrying 1 process(es), 10 thread(s)",
            "handover: generation 1 (from generation 0)",
            "handover_k32_test: writer wrote lines:",
            "handover: carrying 1 process(es), 1 thread(s)",
            "handover: generation 2 (from generation 1)",
            "handover_k32_test: slept again",
            "handover_k32_test: ok",
            "exited with status 0x00000000",
            "gala: boot ok",
        ],
    },
    Scenario {
        // R9 (d): a handover to a newer kernel build with a changed state
        // format (the `handover_next` build, shipped as the boot module
        // kernel-next: PROCESSES version 4 with a new field, the new WRITER
        // section, the CLOCK record's in-memory structure laid out
        // differently), with the kernel32 program working across it. The
        // running kernel reads the next one's version note and checks the
        // record against it first; the new kernel reads PROCESSES version
        // 3 with the new field's default. The program asks again and goes
        // on across a second handover, gala-next to gala-next (PROCESSES
        // version 4 both ways).
        name: "handover_upgrade",
        cmdline: "qemu_exit handover=2 handover_image=kernel-next run=handover_k32_test.exe \
                  args=twice",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "handover_k32_test: ready",
            // R8: the next build is in the signed manifest.
            "manifest: the next kernel is /boot/kernel-next of release 1",
            "handover: gala hands over to gala-next (record framing 1-1, image layout 1, \
             extended state 0xe7, 36 section kinds; with programs)",
            "handover: carrying 1 process(es), 10 thread(s)",
            "handover: generation 1 (from generation 0), record version 1",
            "handover: this kernel is gala-next; the record was written by an older kernel \
             (no WRITER section)",
            "handover: section PROCESSES version 3 read as version 4 (new fields take their \
             defaults)",
            "handover: process handover_k32_test.exe has come through 1 handover(s) (counted \
             from here: an older PROCESSES version)",
            "handover: resumed process handover_k32_test.exe",
            "handover_k32_test: writer wrote lines:",
            "handover: gala-next hands over to gala-next",
            "handover: generation 2 (from generation 1), record version 1",
            "handover: this kernel is gala-next; the record was written by gala-next",
            "handover: process handover_k32_test.exe has come through 2 handover(s)",
            "handover_k32_test: slept again",
            "handover_k32_test: ok",
            "exited with status 0x00000000",
            "gala: boot ok",
        ],
    },
    Scenario {
        // handover_upgrade booted by UEFI (UEFI_SCENARIOS), once.
        name: "handover_upgrade_uefi",
        cmdline: "qemu_exit handover=1 handover_image=kernel-next run=handover_k32_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "firmware: UEFI x86_64",
            "handover: gala hands over to gala-next",
            "handover: this kernel is gala-next",
            "handover: section PROCESSES version 3 read as version 4",
            "handover: resumed process handover_k32_test.exe",
            "handover_k32_test: ok",
            "exited with status 0x00000000",
            "gala: boot ok",
        ],
    },
    Scenario {
        // R9 (d): back from the newer build to the older one (the boot
        // module kernel-gala) while the program runs: gala-next writes
        // PROCESSES version 4, which needs a version-4 reader, and the
        // older kernel's note says it reads version 3, so gala-next refuses
        // before stopping anything and the program goes on in it. With no
        // programs left, the boot's own handover to the older build is
        // done: it skips the WRITER section it does not know.
        name: "handover_downgrade_refused",
        cmdline: "qemu_exit handover=1 handover_image=kernel-gala run=handover_k32_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "handover_k32_test: ready",
            "handover: gala-next hands over to gala (record framing 1-1, image layout 1, \
             extended state 0xe7, 35 section kinds; with programs)",
            "handover: refused: the next kernel (gala) reads PROCESSES up to version 3; this \
             kernel writes version 4, which needs a reader of version 4",
            "handover_k32_test: slept",
            "handover_k32_test: ok",
            "exited with status 0x00000000",
            "handover: gala-next hands over to gala (record framing 1-1, image layout 1, \
             extended state 0xe7, 35 section kinds)",
            "handover: generation 1 (from generation 0), record version 1",
            "handover: this kernel is gala; this build does not read the writer's name",
            "handover: skipped section 33 (unknown to this kernel)",
            "gala: boot ok",
        ],
    },
    Scenario {
        // R8 (ARCHITECTURE "Chain of trust", step 4): the boot's signed
        // manifest does not list the image to hand over to (the boot
        // module kernel-unsigned: the kernel with 8 bytes appended, a valid
        // image Limine loaded by its config's hash), so the kernel refuses
        // before stopping anything and goes on.
        name: "handover_unsigned_refused",
        cmdline: "qemu_exit handover=1 handover_image=kernel-unsigned",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "manifest: release 1 verified (rollback floor 1, ",
            "handover: refused: the next kernel's image is not trusted: it is not in the signed \
             manifest of release 1 (SHA-256 ",
            "gala: boot ok",
        ],
    },
    Scenario {
        // R9 (e): a server with established TCP connections across a
        // kernel handover (user/handover_net_test, on ws2_32): xtask's
        // client talks to it through QEMU's port forward before, during
        // and after the handover; a second connection waits unaccepted in
        // the listener's backlog with a line unread across it; a third is
        // accepted after it on the same listening socket. A bound UDP port
        // keeps a datagram queued across it and gets new ones after it. The
        // program's line counter goes on, no connection is reset.
        name: "handover_net",
        cmdline: "qemu_exit handover=1 run=handover_net_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "handover_net_test: listening on port 7",
            "handover: carrying 1 process(es)",
            "socket(s): 2 TCP connection(s), 1 listener(s), 1 UDP socket(s); the network is \
             stopped",
            "handover: generation 1 (from generation 0)",
            "handover: resumed process handover_net_test.exe",
            "socket(s) resumed; the network was stopped for",
            "handover_net_test: connection 1: 15 line(s), 15 in all",
            "handover_net_test: connection 2: 2 line(s), 17 in all",
            "handover_net_test: connection 3: 3 line(s), 20 in all",
            "handover_net_test: ok",
            "xtask: net client: handover ok",
            "exited with status 0x00000000",
            "gala: boot ok",
        ],
    },
    Scenario {
        // R8 steps 2-3: hello.exe changed after limine.conf was written
        // with its BLAKE2B: Limine refuses it (hash_mismatch_panic) and
        // never starts the kernel. xtask stops the VM at Limine's panic.
        name: "boot_tampered",
        cmdline: "qemu_exit run=hello.exe",
        exit_status: XTASK_STOPS,
        expect: &["PANIC: Blake2b hash for URI `boot():/boot/hello.exe` does not match!"],
    },
    Scenario {
        // R8 step 2: limine.conf changed after its BLAKE2B was enrolled in
        // BOOTX64.EFI: Limine refuses the config (booted by UEFI, from
        // the image's EFI system partition).
        name: "boot_config_edited",
        cmdline: "qemu_exit run=hello.exe",
        exit_status: XTASK_STOPS,
        expect: &["CHECKSUM MISMATCH FOR CONFIG FILE"],
    },
    Scenario {
        // R8 step 6: an earlier boot of release 2 (its own image, signed
        // as release 2 with floor 2) raised the disk's rollback floor;
        // this boot of release 1 is refused once the volume is mounted.
        name: "rollback_refused",
        cmdline: "qemu_exit persist",
        exit_status: QEMU_EXIT_FAILURE,
        expect: &[
            "manifest: release 1 verified",
            "rollback: refused: release 1 is below the rollback floor 2",
        ],
    },
    Scenario {
        // handover_net_test on its own (as on Windows, run-all.ps1): its
        // `self` thread plays the client over loopback, with no handover
        // allowed (the answer to `handover` says so).
        name: "handover_net_self",
        cmdline: "qemu_exit run=handover_net_test.exe args=self",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "handover_net_test: connection 3: 3 line(s), 20 in all",
            "handover_net_test: self client ok",
            "handover_net_test: ok",
        ],
    },
    Scenario {
        // handover_net booted by UEFI (UEFI_SCENARIOS).
        name: "handover_net_uefi",
        cmdline: "qemu_exit handover=1 run=handover_net_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "firmware: UEFI x86_64",
            "socket(s): 2 TCP connection(s), 1 listener(s), 1 UDP socket(s); the network is \
             stopped",
            "handover: resumed process handover_net_test.exe",
            "handover_net_test: connection 3: 3 line(s), 20 in all",
            "handover_net_test: ok",
            "xtask: net client: handover ok",
            "gala: boot ok",
        ],
    },
    Scenario {
        // handover_k32 booted by UEFI (UEFI_SCENARIOS).
        name: "handover_k32_uefi",
        cmdline: "qemu_exit handover=1 run=handover_k32_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "firmware: UEFI x86_64",
            "handover: carrying 1 process(es), 10 thread(s)",
            "handover: resumed process handover_k32_test.exe",
            "handover_k32_test: ok",
            "exited with status 0x00000000",
            "gala: boot ok",
        ],
    },
    Scenario {
        // A CPU that takes the park IPI too late (`parkdelay`: 3 s, past
        // the boot CPU's 2 s wait; a vCPU descheduled that long, or a wait
        // cut short by a clock calibrated low, as handover_idle_uefi once
        // was): the handover is abandoned cleanly (the late CPU finds the
        // gate closed and goes on, the boot CPU's timer and interrupts
        // come back), the program goes on in this kernel, and the boot's
        // own handover afterwards works.
        name: "handover_park_late",
        cmdline: "qemu_exit handover=1 parkdelay run=handover_test.exe args=late",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "handover: parkdelay",
            "handover: no other CPU parked within 2000 ms (0 of 1); the handover is abandoned",
            "handover_test: ok (late)",
            "exited with status 0x00000000",
            "handover: generation 1 (from generation 0)",
            "handover_test: ok (no handover allowed)",
            "gala: boot ok",
        ],
    },
    Scenario {
        // handover_idle booted by UEFI (UEFI_SCENARIOS).
        name: "handover_idle_uefi",
        cmdline: "qemu_exit handover=1 run=handover_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "firmware: UEFI x86_64",
            "handover: carrying 1 process(es), 4 thread(s)",
            "handover: resumed process handover_test.exe",
            "handover_test: ok",
            "exited with status 0x00000000",
            "gala: boot ok",
        ],
    },
    Scenario {
        // handover_same booted by UEFI (UEFI_SCENARIOS): the GOP
        // framebuffer is handed over too, and its console goes on.
        name: "handover_uefi",
        cmdline: "qemu_exit handover=2",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "firmware: UEFI x86_64",
            "handover: generation 2 (from generation 1), record version 1",
            "framebuffer: 1280x800",
            "handover: generation 2: 2 of 2 CPUs online again",
            "survived 2 handover(s)",
            "gala: boot ok",
        ],
    },
    Scenario {
        // R9 (a) with a mounted volume (`persist`, DISK_SCENARIOS): the
        // volume's log position is handed over and found again by the next
        // kernel; persist_test runs in each kernel (between handovers, not
        // across them): the first writes its files, the next ones find
        // them on the disk.
        name: "handover_disk",
        cmdline: "qemu_exit persist handover=2 run=persist_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "persist_test: first boot",
            "handover: generation 1 (from generation 0)",
            "handover: volume continues",
            "handover: generation 2: 2 of 2 CPUs online again",
            "persist_test: ok",
            "survived 2 handover(s)",
            "gala: boot ok",
        ],
    },
    Scenario {
        // R4: the watchdog for a thread that stays in the kernel: the boot
        // thread spins 25 s (`spintest`); it is reported after 20 s.
        name: "kernel_watchdog",
        cmdline: "qemu_exit spintest run=hello.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "spintest: the boot thread spins",
            "has been in the kernel for 20 s without blocking or reaching user mode",
            "spintest: done",
            "exited with status 0x00000000",
        ],
    },
    Scenario {
        // R4: the hang watchdog. `hangtest` makes CPU 1 spin with
        // interrupts off for 9 s, 3 s after its timer starts; another CPU
        // reports it after 5 s with its flight-recorder events, and the
        // system goes on (busybox sleeps through it).
        name: "watchdog",
        cmdline: "qemu_exit hangtest run=busybox.exe args=sleep%2012",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "hangtest: CPU 1 spins with interrupts off",
            "watchdog: CPU 1 has taken no timer interrupt for 5 s",
            "flight: the last",
            // Its own backtrace, by NMI (kernel/src/statedump.rs).
            "watchdog: CPU 1 kernel rip",
            "watchdog: CPU 1 backtrace:",
            "hangtest: CPU 1 goes on",
            "exited with status 0x00000000",
        ],
    },
    Scenario {
        // H3: busybox utilities over files (user/busybox/files.sh): mkdir,
        // ls, find, grep -r, wc, cp -r, cmp, md5sum, tar c/t/x, rm -r.
        // Expected as on Windows (the same binary and script).
        name: "busybox_files",
        cmdline: "qemu_exit run=busybox.exe args=sh%20C:/gala/files.sh",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "ls: 1.txt b ",
            "ls -l: 1 file",
            "find: a a/1.txt a/b a/b/2.txt a/b/3.txt ",
            "find -type f: 3",
            "grep -r: a/b/3.txt:four",
            "wc -l: 2",
            "cp -r: c/1.txt c/b/2.txt c/b/3.txt ",
            "cmp: same",
            "md5sum: 5bbf5a52328e7439ae6e719dfe712200  a/1.txt",
            "tar tf: a/ a/1.txt a/b/ a/b/2.txt a/b/3.txt ",
            "tar xf: three four ",
            "rm -r: []",
            "files: ok",
            "busybox.exe (pid 4) exited with status 0x00000000",
        ],
    },
    Scenario {
        // H3: a shell script from a file (user/busybox/script.sh):
        // functions, loops, case, here-documents, command substitution,
        // arithmetic, subshells, exit status 4. Expected as on Windows.
        name: "busybox_script",
        cmdline: "qemu_exit run=busybox.exe args=sh%20C:/gala/script.sh",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "hello, gala",
            "status: 3",
            "for: 15",
            "while: 4",
            "case: text number other",
            "heredoc: 2 lines, line 2",
            "subst: c,b,a,",
            "args: 3 y",
            "if: elif",
            "subshell: 5",
            "script: ok",
            "busybox.exe (pid 4) exited with status 0x00000004",
        ],
    },
    Scenario {
        // H4: a C compiler on gala (user/tcc/tcc.sh): busybox's tar unpacks
        // TinyCC (user/tcc/build.sh), tcc builds its runtime library
        // (libtcc1.a, the startup code) and compiles user/tcc/hello.c,
        // which then runs. Expected as on Windows.
        name: "tcc",
        cmdline: "qemu_exit run=busybox.exe args=sh%20C:/gala/tcc.sh%20C:/gala/tcc.tar",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "unpacked=include/stdio.h",
            "tcc version 0.9.28rc mob:e23dabe (x86_64 Windows)",
            "libtcc1=lib/libtcc1.a",
            "compiled=hello.exe",
            "hello from tcc: sum of squares=385",
            "argc=4 last=three len=5",
            "status=7",
            "busybox.exe (pid 4) exited with status 0x00000000",
        ],
    },
    Scenario {
        // P1 (pending): Win32-OpenSSH's server, the next program for the
        // security model (docs/SECURITY.md): ssh-keygen -A makes the host
        // keys in %ProgramData%\ssh, sshd -t checks the configuration and
        // the keys (their ACLs), sshd -d would serve one connection.
        name: "sshd",
        cmdline: "qemu_exit cwd=C:/Program%20Files/OpenSSH run=ssh-keygen.exe,sshd.exe args=-A,-t",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "ssh-keygen.exe finished with status 0x00000000",
            "sshd.exe finished with status 0x00000000",
        ],
    },
    Scenario {
        // Security model S4 (pending): sshd as SYSTEM (as=system, as the
        // service runs) serving one connection (-d) on port 7; the host's
        // ssh logs on as galatest with a key (user/sshd/client_ed25519,
        // its authorized_keys in the profile) and runs echo.
        name: "sshd_login",
        cmdline: "qemu_exit as=system wait=all cwd=C:/Program%20Files/OpenSSH run=ssh-keygen.exe,sshd.exe args=-A,-ddd%20-e%20-p%207",
        exit_status: XTASK_STOPS,
        expect: &["xtask: net client: ssh ok"],
    },
    Scenario {
        // PostgreSQL 17.2 (EnterpriseDB's Windows binaries, PREBUILT):
        // initdb makes a cluster in C:\\pgdata (trust authentication; it
        // restarts itself with a restricted token, as pg_ctl does), then
        // postgres serves it on port 7 with our pg_hba.conf (the host
        // connects through QEMU's network); the host's psql queries it.
        name: "postgres",
        cmdline: "qemu_exit run=initdb.exe,postgres.exe args=-D%20C:/pgdata%20-U%20postgres%20-A%20trust%20-E%20UTF8%20--no-locale,-D%20C:/pgdata%20-p%207%20-c%20listen_addresses=*%20-c%20hba_file=C:/gala/pg_hba.conf",
        exit_status: XTASK_STOPS,
        expect: &["Success. You can now start the database server", "xtask: net client: psql ok"],
    },
    Scenario {
        // FileZilla Server 1.12.6, a console program, with its settings in
        // C:\fzs (user/filezilla_server: FTP on port 7, plain FTP allowed,
        // a user `gala` without a password whose home is C:\ftp); it
        // serves until stopped. The host logs on and lists nothing more
        // (QEMU forwards one port: no data connections).
        name: "filezilla_server",
        cmdline: "qemu_exit run=filezilla-server/filezilla-server.exe args=--config-dir%20C:/fzs",
        exit_status: XTASK_STOPS,
        expect: &["xtask: net client: ftp ok"],
    },
    Scenario {
        // W3: FileZilla 3.71.0 itself (wxWidgets 3.2): its main window and
        // Welcome dialog on the desktop, clicked closed (SCREEN_SCENARIOS).
        name: "filezilla",
        cmdline: "qemu_exit wallpaper=dunes \
                  start=filezilla-server/filezilla-server.exe start_args=--config-dir%20C:/fzs \
                  console=filezilla-server.exe run=filezilla/filezilla.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["xtask: screen ok", "exited with status 0x00000000"],
    },
    Scenario {
        // W3 and UI Automation: FileZilla 3.71.0 driven by FlaUI 5.0.0 on
        // .NET 8 (user/flaui_test/FileZillaFlaUI.cs) through gala's
        // UIAutomationCore, with FileZilla Server started first: the
        // Welcome dialog, Quickconnect, the connection's dialogs, the
        // remote and local file lists, File > Exit. No pixels: the same
        // test runs on Windows.
        name: "filezilla_flaui",
        cmdline: "qemu_exit \
                  start=filezilla-server/filezilla-server.exe start_args=--config-dir%20C:/fzs \
                  console=filezilla-server.exe run=dotnet.exe args=C:/gala/filezilla_flaui.dll",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "window=FileZilla",
            "welcome=Welcome to FileZilla OK",
            "quickconnect=127.0.0.1 gala 7",
            "dialog=Remember passwords? OK",
            "dialog=Unknown certificate OK",
            "remote=..,hello.txt",
            "local=2 ..,",
            "grid=Filename,Filesize,Filetype,Last modified,Permissions,Owner/Group rows=2",
            "row=hello.txt ",
            "events=title opened=Remember passwords?,Unknown certificate",
            "selected=hello.txt True",
            "menu=File Exit",
            "exited=True",
            "filezilla_flaui: ok",
            "dotnet.exe finished with status 0x00000000",
        ],
    },
    Scenario {
        // W3: FileZilla 3.71.0's SFTP engine (its own libfzssh) against
        // Win32-OpenSSH's sshd: ssh-keygen -A makes the host keys, then
        // sshd serves as a service (`start_as=system`: SYSTEM, session 0)
        // while the boot user's FlaUI test (filezilla_flaui.dll `sftp`)
        // Quickconnects as galatest with its password, accepts the
        // unknown host key and lists C:\ftp over SFTP (sftp-server.exe as
        // galatest), then File > Exit.
        name: "filezilla_sftp",
        cmdline: "qemu_exit logtee=C:/ProgramData/ssh/logs events=net,logon \
                  start_as=system start=ssh-keygen.exe,sshd.exe start_args=-A, \
                  start_wait=ssh-keygen.exe run=dotnet.exe args=C:/gala/filezilla_flaui.dll%20sftp",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "start: ssh-keygen.exe (pid ",
            ") finished with status 0x00000000",
            "start: sshd.exe (pid ",
            "welcome=Welcome to FileZilla OK",
            "quickconnect=sftp://127.0.0.1/C:/ftp galatest 22",
            "dialog=Remember passwords? OK",
            "dialog=Unknown host key OK",
            "logtee: sshd.log: ",
            "Accepted password for galatest",
            "remote=..,hello.txt",
            "grid=Filename,Filesize,Filetype,Last modified,Permissions,Owner/Group rows=2",
            "row=hello.txt ",
            "events=title opened=Remember passwords?,Unknown host key",
            "selected=hello.txt True",
            "menu=File Exit",
            "exited=True",
            "filezilla_flaui: ok",
            "dotnet.exe finished with status 0x00000000",
        ],
    },
    Scenario {
        // P1 (pending): Git Bash (PortableGit 2.47.1): bash on the MSYS2
        // runtime, a pipeline (a forked child per command).
        name: "gitbash",
        cmdline: "qemu_exit run=git/usr/bin/bash.exe args=-c%20%22echo%20hi%20from%20bash%20|%20cat%22",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["hi from bash"],
    },
    Scenario {
        // P1 (pending): MinGit 2.47.1, run as users run it (cmd\git.exe,
        // which starts mingw64\bin\git.exe): its version, then a new
        // repository in the user's profile (git checks that its user owns
        // it): init, a file written by cmd.exe, add, commit, log (without
        // the pager: gala's console is a terminal, and MinGit has no less).
        name: "mingit",
        cmdline: "qemu_exit cwd=C:/Users/gala \
                  run=cmd/git.exe,cmd/git.exe,cmd.exe,cmd/git.exe,cmd/git.exe,cmd/git.exe \
                  args=--version,init%20repo,/c%20echo%20hello%20from%20gala>repo\\hello.txt,\
                  -C%20repo%20add%20hello.txt,\
                  -C%20repo%20-c%20user.name=gala%20-c%20user.email=gala@example.com%20commit%20-m%20first,\
                  --no-pager%20-C%20repo%20log%20--format=log:%25s",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "git version 2.47.1.windows.1",
            "Initialized empty Git repository in C:/Users/gala/repo/.git/",
            "(root-commit)",
            "1 file changed, 1 insertion(+)",
            "log:first",
        ],
    },
    Scenario {
        // sshd as a server (ROADMAP SSH step 1): started at boot as SYSTEM
        // without -d, on its configured port 22, restarted if it ends
        // (`restart=`), logging to %ProgramData%\ssh\logs (sshd_config's
        // SyslogFacility LOCAL0), which `logtee=` copies to the serial log.
        // The host logs on with a key three times, then twice at once,
        // each running a command; the logs and the event stream (logons,
        // connections) show them.
        name: "sshd_server",
        cmdline: "qemu_exit as=system logtee=C:/ProgramData/ssh/logs restart=sshd.exe events=net,logon cwd=C:/Program%20Files/OpenSSH run=ssh-keygen.exe,sshd.exe args=-A,",
        exit_status: XTASK_STOPS,
        expect: &[
            "xtask: net client: ssh server ok",
            "logtee: sshd.log: ",
            "Accepted publickey for galatest",
        ],
    },
    Scenario {
        // File transfer (ROADMAP SSH step 3): the host's sftp uses sshd's
        // sftp subsystem (sftp-server.exe): uploads a file of 200 KiB,
        // lists the directory, downloads the file again (compared byte for
        // byte), removes it.
        name: "sshd_sftp",
        cmdline: "qemu_exit as=system logtee=C:/ProgramData/ssh/logs events=net,logon,process cwd=C:/Program%20Files/OpenSSH run=ssh-keygen.exe,sshd.exe args=-A,",
        exit_status: XTASK_STOPS,
        expect: &["xtask: net client: sftp ok"],
    },
    Scenario {
        // File transfer with scp (ROADMAP SSH step 3): the host's scp -p
        // copies a file to gala and back, once over SFTP (scp's default:
        // sftp-server.exe) and once with the legacy protocol (`-O`: sshd
        // runs `scp -t` / `scp -f`, gala's System32\scp.exe, through the
        // shell). Both directions keep the bytes and the times scp -p
        // carries (gala's own `stat` sees the uploaded times).
        name: "sshd_scp",
        cmdline: "qemu_exit as=system logtee=C:/ProgramData/ssh/logs events=net,logon,process cwd=C:/Program%20Files/OpenSSH run=ssh-keygen.exe,sshd.exe args=-A,",
        exit_status: XTASK_STOPS,
        expect: &["xtask: net client: scp ok"],
    },
    Scenario {
        // Port forwarding (ROADMAP SSH step 4). `ssh -L`: the host's
        // connection to a local socket becomes a direct-tcpip channel;
        // sshd-session connects out over loopback to sshfwd_test serving
        // 127.0.0.1:7000 (started by the same ssh's command) and the bytes
        // come back echoed. `ssh -R`: sshd-session listens on gala's
        // 127.0.0.1:7001 (tcpip-forward); sshfwd_test connects to it and
        // its bytes travel to an echo server on the host and back.
        name: "sshd_forward",
        cmdline: "qemu_exit as=system logtee=C:/ProgramData/ssh/logs events=net,logon,process cwd=C:/Program%20Files/OpenSSH run=ssh-keygen.exe,sshd.exe args=-A,",
        exit_status: XTASK_STOPS,
        expect: &["xtask: net client: ssh forward ok"],
    },
    Scenario {
        // The client (ROADMAP SSH step 5): from a session the host's ssh
        // opened, gala's own ssh.exe (Win32-OpenSSH's, unmodified) logs on
        // to gala's sshd over 127.0.0.1 with galatest's key from its
        // profile, twice: first accepting the new host key (written to
        // ~/.ssh/known_hosts), then checking it strictly against that file.
        name: "sshd_client",
        cmdline: "qemu_exit as=system logtee=C:/ProgramData/ssh/logs events=net,logon,process cwd=C:/Program%20Files/OpenSSH run=ssh-keygen.exe,sshd.exe args=-A,",
        exit_status: XTASK_STOPS,
        expect: &[
            "xtask: net client: ssh client ok",
            "Accepted publickey for galatest from 127.0.0.1 port",
        ],
    },
    Scenario {
        // `restart=` (the services' supervisor, as sshd_server uses it): a
        // program that ends is started again, here at most twice.
        name: "service_restart",
        cmdline: "qemu_exit run=hello.exe restart=hello.exe:2",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "service: hello.exe ended; restarting it (1 of 2)",
            "service: hello.exe ended; restarting it (2 of 2)",
            "service: hello.exe ended 2 times; not restarted again",
            "gala: boot ok",
        ],
    },
    Scenario {
        // An interactive shell over ssh (ROADMAP SSH step 2): the host's
        // `ssh -tt` asks for a pty, so sshd-session runs the shell (busybox
        // sh, DefaultShellArguments) in a pseudo console: `conhost.exe
        // --headless ... -- <shell>` (gala's, in System32). xtask types
        // like a terminal and reads the VT output: a command's result,
        // a line edited with DEL, Ctrl+C interrupting `sleep`, `exit`.
        // `conpty=trace` logs both directions of the pseudo console.
        name: "sshd_shell",
        cmdline: "qemu_exit as=system logtee=C:/ProgramData/ssh/logs conpty=trace events=net,logon,console cwd=C:/Program%20Files/OpenSSH run=ssh-keygen.exe,sshd.exe args=-A,",
        exit_status: XTASK_STOPS,
        expect: &["xtask: net client: ssh shell ok", "conpty "],
    },
    Scenario {
        // The same login with galatest's password (Win32-OpenSSH checks it
        // with LogonUserExExW, LOGON32_LOGON_NETWORK_CLEARTEXT); no key.
        name: "sshd_password",
        cmdline: "qemu_exit as=system wait=all cwd=C:/Program%20Files/OpenSSH run=ssh-keygen.exe,sshd.exe args=-A,-ddd%20-e%20-p%207",
        exit_status: XTASK_STOPS,
        expect: &["xtask: net client: ssh ok"],
    },
    Scenario {
        // Unmodified prebuilt programs: Caddy 2.8.4's official Windows
        // release (Go 1.22, fetched and checked by xtask, PREBUILT) prints
        // its version and lists its modules. Expected as on Windows.
        name: "caddy",
        cmdline: "qemu_exit run=caddy.exe,caddy.exe args=version,list-modules",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "v2.8.4 h1:q3pe0wpBj1OcHFZ3n/1nl4V4bxBrYoSoab7rL9BMYNk=",
            "http.handlers.file_server",
            "Standard modules: 121",
            "caddy.exe finished with status 0x00000000",
        ],
    },
    Scenario {
        // P1: the unmodified Caddy release as a static file server
        // (`caddy file-server`), the host fetching its page; xtask stops
        // the VM after the GET. Caddy logs JSON to stderr, as on Windows.
        name: "caddy_http",
        cmdline: "qemu_exit run=caddy.exe args=file-server%20--listen%20:7%20--root%20C:/caddy/site",
        exit_status: XTASK_STOPS,
        expect: &[
            "\"msg\":\"server running\"",
            "Caddy serving static files on :7",
            "xtask: net client: http ok",
        ],
    },
    Scenario {
        // P1: the JDK's HTTP server (NIO selector: wepoll over AFD) serving
        // the host; xtask stops the VM after the GET.
        name: "java_http",
        cmdline: "qemu_exit run=java.exe args=C:/gala/Http.java dump=C:/hs_err_pid4.log",
        exit_status: XTASK_STOPS,
        expect: &["java_http: listening on port 7", "xtask: net client: http ok"],
    },
    Scenario {
        // P1: a Java program on the unmodified JDK: `java Hello.java`
        // (the source launcher compiles it in memory with jdk.compiler).
        // Expected as on Windows 11 (2 processors under QEMU).
        // Pending: javac 48 times in one JVM (user/java/Stress.java), a
        // stress run for the Java heap corruption (WORKFLOW "Known intermittent").
        name: "java_stress",
        cmdline: "qemu_exit run=java.exe args=C:/gala/Stress.java%2048 dump=C:/hs_err_pid4.log",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["java_stress: 48 compiles, 0 failed", "java.exe finished with status 0x00000000"],
    },
    Scenario {
        name: "java_hello",
        cmdline: "qemu_exit run=java.exe args=C:/gala/Hello.java dump=C:/hs_err_pid4.log",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "Hello from Java on gala",
            "os.name=Windows 11",
            "os.version=10.0",
            "os.arch=amd64",
            "user.name=gala",
            "user.home=C:\\Users\\gala",
            "java.home=C:\\jdk",
            "processors=2",
            "counts={brown=1, dog=1, end=1, fox=1, jumps=1, lazy=1, over=1, quick=1, the=3}",
            "sum=499999500000",
            "pi=3.14159",
            "java.exe finished with status 0x00000000",
        ],
    },
    Scenario {
        // P1: the unmodified .NET 8 runtime (PREBUILT): the dotnet host
        // lists its runtimes, as on Windows. Microsoft's redistributable
        // UCRT (from the JDK) sits next to dotnet.exe, as self-contained
        // .NET apps deploy it.
        name: "dotnet_info",
        cmdline: "qemu_exit run=dotnet.exe args=--list-runtimes",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "Microsoft.NETCore.App 8.0.33 [C:\\dotnet\\shared\\Microsoft.NETCore.App]",
            "dotnet.exe finished with status 0x00000000",
        ],
    },
    Scenario {
        // BR stage 1: headless Chromium (PREBUILT) starts, loads
        // about:blank and prints its DOM: the browser process, its
        // children (renderer, network service) over Mojo, V8.
        name: "chromium_version",
        cmdline: "qemu_exit run=chrome.exe args=--headless%20--no-sandbox%20--disable-gpu%20--no-first-run%20--user-data-dir=C:\\tmp\\chr%20--enable-logging=stderr%20--v=1%20--dump-dom%20about:blank dump=C:/chromium/debug.log,C:/tmp/chr/chrome_debug.log",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "<html><head></head><body></body></html>",
            "chrome.exe finished with status 0x00000000",
        ],
    },
    Scenario {
        // BR stage 1: a local page whose script changes it (so V8 ran),
        // its DOM printed after the script.
        name: "chromium_dump",
        cmdline: "qemu_exit run=chrome.exe args=--headless%20--no-sandbox%20--disable-gpu%20--no-first-run%20--user-data-dir=C:\\tmp\\chr%20--enable-logging=stderr%20--dump-dom%20file:///C:/gala/page.html dump=C:/chromium/debug.log,C:/tmp/chr/chrome_debug.log",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "<p id=\"static\">static text</p>",
            "<p id=\"dynamic\">V8 ran: 42</p>",
            "chrome.exe finished with status 0x00000000",
        ],
    },
    Scenario {
        // BR stage 1: an https:// page from Nginx in the guest (OpenSSL,
        // schannel_test's configuration: a certificate issued by the test
        // CA, a trusted root on the machine), its DOM printed: Winsock,
        // the network service, BoringSSL and certificate verification
        // through crypt32's chain engine.
        name: "chromium_https",
        cmdline: "qemu_exit start=nginx.exe start_args=-p%20C:/nginx/%20-c%20conf/nginx.conf \
                  run=chrome.exe args=--headless%20--no-sandbox%20--disable-gpu%20--no-first-run%20--user-data-dir=C:\\tmp\\chr%20--enable-logging=stderr%20--dump-dom%20https://127.0.0.1:8443/ \
                  dump=C:/chromium/debug.log,C:/tmp/chr/chrome_debug.log",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["<h1>Welcome to nginx!</h1>", "chrome.exe finished with status 0x00000000"],
    },
    Scenario {
        // BR stage 1: a page from the host network by name:
        // http://gala-host.test/ (the scenario's hosts file names
        // 10.0.2.100, which QEMU's user network hands to xtask's
        // `serve-page` on the host: HOST_PAGE_SCENARIOS); the network
        // service's sockets leave the guest through virtio-net.
        name: "chromium_host",
        cmdline: "qemu_exit run=chrome.exe args=--headless%20--no-sandbox%20--disable-gpu%20--no-first-run%20--user-data-dir=C:\\tmp\\chr%20--enable-logging=stderr%20--dump-dom%20http://gala-host.test/ \
                  dump=C:/chromium/debug.log,C:/tmp/chr/chrome_debug.log",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "<h1>hello from the host</h1>",
            "chrome.exe finished with status 0x00000000",
            "xtask: serve-page: \"GET / HTTP/1.1\"",
        ],
    },
    Scenario {
        // BR stage 2: Chromium with a window (software rendering) on the
        // desktop: a page with a green background and a magenta box,
        // checked on the screen (SCREEN_SCENARIOS), then closed with
        // Alt+F4. --disable-direct-composition: a stopgap until GPU
        // G2/G4; --ipc-connection-timeout=120: a stopgap until guest time
        // follows the emulated CPU's work (ROADMAP "Known issues and
        // stopgaps").
        name: "chromium_window",
        cmdline: "qemu_exit run=chrome.exe args=--no-sandbox%20--disable-gpu%20--disable-direct-composition%20--no-first-run%20--user-data-dir=C:\\tmp\\chr%20--enable-logging=stderr%20--ipc-connection-timeout=120%20--window-position=0%2C0%20--window-size=800%2C600%20file:///C:/gala/window.html \
                  dump=C:/chromium/debug.log,C:/tmp/chr/chrome_debug.log",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "gala window painted",
            "gala text focused",
            "gala typed gala",
            "gala link followed",
            "xtask: screen ok",
            "chrome.exe finished with status 0x00000000",
        ],
    },
    Scenario {
        // BR stage 2 and UIA: a UI Automation client
        // (user/uia_chromium_test, mingw) starts Chromium on window.html
        // (chromium_window's flags), finds the page's link in Chromium's
        // accessibility tree (its UIA provider, through gala's
        // UIAutomationCore) by ControlType and Name, invokes it
        // (window2.html's title) and closes Chromium with WM_CLOSE.
        name: "chromium_uia",
        cmdline: "qemu_exit run=uia_chromium_test.exe dump=C:/chromium/debug.log,C:/tmp/chr/chrome_debug.log",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "uia_chromium_test: link found",
            "uia_chromium_test: link followed",
            "uia_chromium_test: ok",
            "exited with status 0x00000000",
        ],
    },
    Scenario {
        // BR's conformance report, step 1: ChromeDriver (PREBUILT, the
        // snapshot's) serves WebDriver on port 7; xtask (the host) opens a
        // session on headless Chromium, loads page.html, reads its title
        // and text through a script and ends the session
        // (`webdriver_client`), then asks ChromeDriver to shut down, so
        // the run ends and `dump=` copies ChromeDriver's log and
        // Chromium's (ChromeDriver does not pass Chromium its stderr).
        name: "chromium_driver",
        cmdline: "qemu_exit run=chromedriver.exe args=--port=7%20--allowed-ips=%20--verbose%20--log-path=C:\\chromium\\chromedriver.log \
                  dump=C:/chromium/chromedriver.log,C:/chromium/chrome_driven.log",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["xtask: net client: webdriver ok"],
    },
    Scenario {
        // BR's conformance report, step 2: web-platform-tests (WPT_DIRS
        // at WPT_COMMIT, fetched by xtask) served by WPT's own server on
        // the host (wpt.rs), run one page at a time in headless Chromium
        // through ChromeDriver (`wpt::client`); each test's line goes into
        // the serial log (`xtask: wpt: ...`), the results to target/wpt/.
        // Each session's Chrome logs to its own file (a FATAL in one
        // stays when the next session starts), the first eight dumped.
        name: "chromium_wpt",
        cmdline: "qemu_exit run=chromedriver.exe args=--port=7%20--allowed-ips=%20--log-path=C:\\chromium\\chromedriver.log \
                  dump=C:/chromium/chromedriver.log,C:/chromium/chrome_wpt-0.log,C:/chromium/chrome_wpt-1.log,\
                  C:/chromium/chrome_wpt-2.log,C:/chromium/chrome_wpt-3.log,C:/chromium/chrome_wpt-4.log,\
                  C:/chromium/chrome_wpt-5.log,C:/chromium/chrome_wpt-6.log,C:/chromium/chrome_wpt-7.log",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["xtask: wpt: summary", "xtask: net client: wpt ok"],
    },
    Scenario {
        // P1 step 6: the embeddable CPython 3.13.1 (PREBUILT `python`)
        // evaluating one expression; expected as on Windows.
        name: "python_hello",
        cmdline: "qemu_exit run=python.exe args=-c%20print(6*7)",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["\n42\r", "python.exe finished with status 0x00000000"],
    },
    Scenario {
        // P1 step 6: CPython running user/python/python_test.py: threads,
        // sockets, select, a subprocess, hashlib/zlib/json/sqlite3/ctypes;
        // each check as on Windows.
        name: "python_script",
        cmdline: "qemu_exit run=python.exe args=C:\\gala\\python_test.py",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["python_test: ok", "python.exe finished with status 0x00000000"],
    },
    Scenario {
        // P1 step 6: asyncio's proactor loop (I/O completion ports through
        // _overlapped): TCP and UDP over loopback, a subprocess's pipes,
        // the executor, tasks; user/python/python_async.py, as on Windows.
        name: "python_async",
        cmdline: "qemu_exit run=python.exe args=C:\\gala\\python_async.py",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["python_async: ok", "python.exe finished with status 0x00000000"],
    },
    Scenario {
        // P1 step 6: pip and venv (the full CPython layout): pip installs a
        // wheel the script builds, its console-script launcher runs,
        // `python -m venv` and the venv's launcher;
        // user/python/python_pip.py, as on Windows.
        name: "python_pip",
        cmdline: "qemu_exit run=python.exe args=C:\\gala\\python_pip.py",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["python_pip: ok", "python.exe finished with status 0x00000000"],
    },
    Scenario {
        // P1 step 6: pip from a package index over https: Nginx (from
        // schannel_test, MODULE_SETS) serves the index python_pip.py
        // writes; the test CA is in the machine's root store.
        name: "python_pip_https",
        cmdline: "qemu_exit start=nginx.exe start_args=-p%20C:/nginx/%20-c%20conf/nginx.conf \
                  run=python.exe args=C:\\gala\\python_pip.py%20https",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "python_pip: pip install over https ok",
            "python_pip: ok",
            "python.exe finished with status 0x00000000",
        ],
    },
    Scenario {
        // P1 step 6: a package with C extensions: pip installs numpy's
        // wheel from the file; arithmetic, random, LAPACK through its
        // OpenBLAS, a threaded matrix product, FFT;
        // user/python/python_numpy.py, as on Windows.
        name: "python_numpy",
        cmdline: "qemu_exit run=python.exe args=C:\\gala\\python_numpy.py",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["python_numpy: ok", "python.exe finished with status 0x00000000"],
    },
    Scenario {
        // P1 step 6: multiprocessing (spawned children over inherited
        // pipes): Process, Queue, Pool, a shared Value and Lock, Pipe,
        // shared_memory; user/python/python_mp.py, as on Windows.
        name: "python_mp",
        cmdline: "qemu_exit run=python.exe args=C:\\gala\\python_mp.py",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["python_mp: ok", "python.exe finished with status 0x00000000"],
    },
    Scenario {
        // P1: PowerShell 7.4 (fetched by xtask, PREBUILT), a self-contained
        // .NET 8 application, running one command.
        name: "pwsh_hello",
        cmdline: "qemu_exit run=pwsh.exe args=-NoProfile%20-NonInteractive%20-Command%20Write-Output%20(1+2);%20$PSVersionTable.PSVersion.ToString()",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["\n3\r", "7.4.22", "pwsh.exe finished with status 0x00000000"],
    },
    Scenario {
        // P1: a managed program (user/dotnet_hello, built by its build.sh)
        // on the unmodified .NET 8 runtime: CoreCLR, the JIT, the GC,
        // managed exceptions (one from an access violation), a thread.
        name: "dotnet_hello",
        cmdline: "qemu_exit run=dotnet.exe args=C:/gala/dotnet_hello.dll",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "hello from .NET 8.0.33",
            "os=Microsoft Windows NT 10.0.26100.0",
            "64bit=True processors=2",
            "the=4",
            "sum=499999500000",
            "caught NullReferenceException",
            "caught boom",
            "thread=42",
            "gc=ok",
            "pi=3.14159",
            "dotnet.exe finished with status 0x00000000",
        ],
    },
    Scenario {
        // UI Automation step 5: FlaUI 5.0.0 (FlaUI.UIA3, PREBUILT) on the
        // .NET 8 runtime drives another program (user/flaui_test: the C#
        // test and flaui_target.exe, built by its build.sh) through gala's
        // UIAutomationCore.dll: CUIAutomation8, the element tree, Invoke,
        // Value and Toggle; FlaUI's Window API (the Window and Transform
        // patterns, the title bar, Window.Close).
        name: "flaui_test",
        cmdline: "qemu_exit run=dotnet.exe args=C:/gala/flaui_test.dll",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "timeouts=20000/2000",
            "timeout=30000",
            "window=FlaUI Target",
            "pid=ok",
            "opened=FlaUI Target",
            "focus=True",
            "cached=1 Press 101",
            "button=Press Button",
            "title=FlaUI Pressed",
            "namechange=FlaUI Pressed",
            "text=hello",
            "valuechange=hello",
            "checked=False",
            "checked=True",
            "togglechange=On",
            "items=3 alpha.txt,beta.txt,gamma.txt",
            "selected=beta.txt True False",
            "selectedevent=beta.txt",
            "grid=3x2 RowMajor headers=Name,Size header=Name,Size",
            "cell=22 1,1 104 Size",
            "row=gamma.txt,333",
            "window=True True False False Normal modal=0",
            "titlebar= Minimize,Maximize,Close",
            "moved=100,80",
            "state=Maximized",
            "state=Normal 100,80",
            "closed=True",
            "closedevent=True",
            "flaui_test: ok",
            "dotnet.exe finished with status 0x00000000",
        ],
    },
    Scenario {
        // P1 step 5 (pending): SQL Server 2022 LocalDB's database engine,
        // started directly as a console program (-c: not as a service),
        // as SqlLocalDB starts an instance: the instance ID the MSI
        // registered (-S), an instance name (-s), and the documented
        // startup options naming the master database's files and the
        // error log (sqlservr also echoes its log to the console), until
        // it is ready for clients. The first start of a new instance, as
        // SqlUserInstance.dll does it (its strings: `-m -K -T1617 -w5`
        // before `-c`): it configures the user instance, relocates the
        // system databases next to master and regenerates the service
        // master key (-w: it would end itself after that many minutes
        // without clients, -w1 here).
        name: "sqllocaldb",
        // tds_test (user/tds_test) starts sqlservr.exe with those
        // arguments, connects over the named pipe with TDS, logs in with
        // integrated security, queries and shuts the instance down.
        cmdline: "qemu_exit run=tds_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "SQL Server is now ready for client connections",
            "Starting up database 'tempdb'",
            "tds_test: connected",
            "tds_test: logged in",
            "tds_test: row: [Microsoft SQL Server 2022",
            "[GALA\\gala]",
            "Server shut down by request from login GALA\\gala",
            "SQL Server is terminating this process",
            "tds_test: second start",
            "tds_test: ok",
        ],
    },
    Scenario {
        // LocalDB across a reboot (pending, like sqllocaldb; `persist`):
        // the first boot (DISK_SCENARIOS) creates galadb with a row and
        // shuts the instance down, this one starts it again and reads the
        // row back.
        name: "sqllocaldb_persist",
        cmdline: "qemu_exit persist run=tds_test.exe args=persist",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["tds_test: start after a reboot", "tds_test: row: [kept]", "tds_test: ok"],
    },
    Scenario {
        // P1 step 4 (pending): Elasticsearch 8.15 on the JDK, started as its
        // elasticsearch.bat does (gala has no cmd.exe): the CLI launcher,
        // which starts the server as a second java.exe. Single node, no
        // security, no machine learning; 256 MB heap (gala.options).
        name: "elasticsearch",
        cmdline: "qemu_exit cwd=C:/es run=java.exe args=-Xms4m%20-Xmx64m%20-XX:+UseSerialGC%20-Dcli.name=server%20-Dcli.script=C:/es/bin/elasticsearch.bat%20-Dcli.libs=C:/es/lib/tools/server-cli%20-Des.path.home=C:/es%20-Des.path.conf=C:/es/config%20-Des.distribution.type=zip%20-cp%20C:/es/lib/*;C:/es/lib/cli-launcher/*%20org.elasticsearch.launcher.CliToolLauncher%20-E%20discovery.type=single-node%20-E%20xpack.security.enabled=false%20-E%20xpack.ml.enabled=false%20-E%20http.host=0.0.0.0%20-E%20http.port=7",
        exit_status: XTASK_STOPS,
        expect: &["] started {", "xtask: net client: http ok"],
    },
    Scenario {
        // P1: the unmodified Temurin JDK 21 (fetched and checked by xtask,
        // PREBUILT): `java -version`, as on Windows (to stderr).
        name: "java_version",
        cmdline: "qemu_exit run=java.exe args=-version",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "openjdk version \"21.0.5\" 2024-10-15 LTS",
            "OpenJDK Runtime Environment Temurin-21.0.5+11 (build 21.0.5+11-LTS)",
            "OpenJDK 64-Bit Server VM Temurin-21.0.5+11 (build 21.0.5+11-LTS, mixed mode",
            "java.exe finished with status 0x00000000",
        ],
    },
    Scenario {
        // H5: TinyCC builds itself on gala (user/tcc/self.sh): after the
        // tcc scenario's steps, tcc.exe compiles TinyCC's sources into
        // tcc2.exe, which compiles hello.c and msvcrt_test's main.c; both
        // run. Expected as on Windows.
        name: "tcc_self",
        cmdline: "qemu_exit run=busybox.exe args=sh%20C:/gala/self.sh%20C:/gala/tcc.tar%20C:/gala/main.c",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "status=7",
            "tcc2: tcc version 0.9.28rc mob:e23dabe (x86_64 Windows)",
            "status2=7",
            "msvcrt_test: ok",
            "msvcrt_test status=0",
            "busybox.exe (pid 4) exited with status 0x00000000",
        ],
    },
    Scenario {
        // H6: busybox's make builds a C program with tcc on gala
        // (user/tcc/make.sh, user/tcc/make/): every object, then only what
        // a touched source needs, then nothing, then a failing rule stops
        // make. Expected as on Windows.
        name: "tcc_make",
        cmdline: "qemu_exit run=busybox.exe args=sh%20C:/gala/make.sh%20C:/gala/tcc.tar",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "build: ../tcc/tcc.exe -B../tcc -c main.c",
            "build: ../tcc/tcc.exe -B../tcc -c add.c",
            "build: ../tcc/tcc.exe -B../tcc -c mul.c",
            "build: ../tcc/tcc.exe -B../tcc -o calc.exe main.o add.o mul.o",
            "calc: add(2, 3)=5 mul(4, 5)=20",
            "after touch: ../tcc/tcc.exe -B../tcc -c add.c",
            "after touch: ../tcc/tcc.exe -B../tcc -o calc.exe main.o add.o mul.o",
            "after touch: 2 commands",
            "again: make: 'calc.exe' is up to date",
            "broken: tcc: error: file 'missing.c' not found",
            "broken status=2",
            "busybox.exe (pid 4) exited with status 0x00000000",
        ],
    },
    Scenario {
        // H7: Lua 5.4.7 (user/lua/build.sh) built on gala by its own
        // makefile under busybox's make with tcc (user/lua/lua.sh), then
        // running user/lua/test.lua. Expected as on Windows (msvcrt's
        // three-digit exponents included).
        name: "lua",
        cmdline: "qemu_exit run=busybox.exe args=sh%20C:/gala/lua.sh%20C:/gala/tcc.tar%20C:/gala/lua.tar",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "built: 37 commands, lua.exe luac.exe",
            "Lua 5.4.7  Copyright (C) 1994-2024 Lua.org, PUC-Rio",
            "upper: HELLO, GALA len=11",
            "gsub: HeLLo, gaLa",
            "find: 8,11",
            "rep: ab-ab-ab",
            "gmatch: eno owt eerht",
            "sort: 1 3 5 7 9",
            "sort desc: 9 7 5 3 1",
            "counts: a=3 b=2 c=1",
            "closures: 3 1",
            "int: 3 1 1024.0 9223372036854775807",
            "float: 0.33333333333333 0.3 1e+100 2.5e-005",
            "format: 3.142   2.3 1.234568e+004 0.0001 ff",
            "tonumber: 16 100.0 nil",
            "math: -4 4 4 4.0",
            "pcall: false boom",
            "error table: 42",
            "runtime error: false attempt to index a nil value",
            "coroutine: 10 15 dead",
            "io: 2 lines",
            "lua: ok",
            "lua status=0",
            "busybox.exe (pid 4) exited with status 0x00000000",
        ],
    },
    Scenario {
        // H9: the SQLite amalgamation (user/sqlite/build.sh) compiled on
        // gala by tcc with a small driver (user/sqlite/sqlrun.c), which builds and queries
        // a file database (user/sqlite/test.sql), and a second process
        // reopens it (reopen.sql); then two processes write it at once
        // (writer.sql, file locks). Expected as on Windows.
        name: "sqlite",
        cmdline: "qemu_exit run=busybox.exe args=sh%20C:/gala/sqlite.sh%20C:/gala/tcc.tar%20C:/gala/sqlite-amalgamation.zip",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "built: sqlrun.exe",
            "version: [3.54.1]",
            "count: 1000 sum: 500500 total: 750750.0",
            "groups: zero=142,one=143,two=143,three=143,four=143,five=143,six=143",
            "avg: 750.000",
            "indexed: 143 n103 999",
            "in transaction: 10",
            "after rollback: 1000",
            "printf:    42|ab   |0003.142|1.234568e+04|1,234,567",
            "strings: GALA on gala a+b+c 5",
            r#"json: {"n":3,"list":[1,2.5,"x"]}"#,
            "first session status=0",
            "reopened: 1000 six",
            "integrity: ok",
            "pages: 1",
            "second session status=0",
            "writer a status=0",
            "writer b status=0",
            "concurrent: 80 rows, 40 distinct, sum 1640",
            "integrity after writers: ok",
            "busybox.exe (pid 4) exited with status 0x00000000",
        ],
    },
    Scenario {
        // N2b: Nginx's normal mode: the master starts a worker process
        // (CreateProcess) that serves the page.
        name: "nginx_master",
        cmdline: "qemu_exit run=nginx.exe args=-p%20C:/nginx/%20-c%20conf/nginx-master.conf",
        exit_status: XTASK_STOPS,
        expect: &["start worker process", "xtask: net client: http ok"],
    },
    Scenario {
        // N2d: Nginx with TLS (OpenSSL); the host's curl GETs over HTTPS.
        name: "nginx_tls",
        cmdline: "qemu_exit run=nginx.exe args=-p%20C:/nginx/%20-c%20conf/nginx-tls.conf",
        exit_status: XTASK_STOPS,
        expect: &["xtask: net client: tls ok"],
    },
    Scenario {
        // TLS step 1: WinHTTP over Schannel against Nginx in the guest
        // (schannel_test's configuration, with plain HTTP on 8080).
        name: "winhttp_test",
        cmdline: "qemu_exit start=nginx.exe start_args=-p%20C:/nginx/%20-c%20conf/nginx.conf \
                  run=winhttp_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["winhttp_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // TLS step 2: WinINet's HTTP functions (on winhttp's engine,
        // user/http_client) against the same Nginx.
        name: "wininet_test",
        cmdline: "qemu_exit start=nginx.exe start_args=-p%20C:/nginx/%20-c%20conf/nginx.conf \
                  run=wininet_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["wininet_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // TLS step 3: curl for Windows (curl.se's build, PREBUILT curl),
        // unmodified, against the same Nginx (user/curl/curl.sh: plain
        // HTTP, HTTPS, an untrusted and an expired certificate). That build
        // has LibreSSL as its only TLS backend; it trusts the Windows ROOT
        // store (crypt32's CertOpenSystemStoreA: gala's machine roots).
        name: "curl",
        cmdline: "qemu_exit start=nginx.exe start_args=-p%20C:/nginx/%20-c%20conf/nginx.conf \
                  run=busybox.exe args=sh%20C:/gala/curl.sh",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "version: curl 8.23.0 (x86_64-w64-mingw32) libcurl/8.23.0 LibreSSL/4.4.0",
            "http: exit 0 <title>Welcome to nginx!</title>",
            "https: exit 0 <title>Welcome to nginx!</title>",
            "https ip: exit 0 200 896",
            "untrusted: exit 60",
            "expired: exit 60",
            "cacert: exit 0 <title>Welcome to nginx!</title>",
            "cacert untrusted: exit 60",
            "cacert other: exit 60",
            "cacert expired: exit 60",
            // LibreSSL's curl does not check revocation.
            "revocation: exit 0 <title>Welcome to nginx!</title>",
            "revoked: exit 0",
            "revoked no-revoke: exit 0",
            "curl.sh: done",
            "exited with status 0x00000000",
        ],
    },
    Scenario {
        // TLS step 3: curl on Schannel: Git for Windows' curl (PREBUILT
        // gitcurl, the libcurl MinGit's https remotes use), unmodified,
        // through secur32 and schannel.dll, validating against gala's
        // machine root store; the same script (its --cacert steps check
        // the chain in curl, through crypt32's chain engine).
        name: "curl_schannel",
        cmdline: "qemu_exit start=nginx.exe start_args=-p%20C:/nginx/%20-c%20conf/nginx.conf \
                  run=busybox.exe args=sh%20C:/gala/curl.sh",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "version: curl 8.11.0 (x86_64-w64-mingw32) libcurl/8.11.0 Schannel",
            "http: exit 0 <title>Welcome to nginx!</title>",
            "https: exit 0 <title>Welcome to nginx!</title>",
            "https ip: exit 0 200 896",
            "curl: (60) schannel: ",
            "untrusted: exit 60",
            "expired: exit 35",
            "cacert: exit 0 <title>Welcome to nginx!</title>",
            "cacert untrusted: exit 60",
            "cacert other: exit 60",
            "cacert expired: exit 60",
            // Revocation (SCH_CRED_REVOCATION_CHECK_CHAIN without
            // --ssl-no-revoke), from the CRL in gala's CA store.
            "revocation: exit 0 <title>Welcome to nginx!</title>",
            "CRYPT_E_REVOKED (0x80092010) - The certificate is revoked.",
            "revoked: exit 35",
            "revoked no-revoke: exit 0",
            "resumption: exit 0 . r",
            "resumption tls1.2: exit 0 . r",
            "curl.sh: done",
            "exited with status 0x00000000",
        ],
    },
    Scenario {
        // TLS step 3: Git's https remotes: MinGit (PREBUILT), unmodified,
        // with Schannel (http.sslBackend), cloning user/git_https/repo.git
        // from the same Nginx over Git's dumb HTTP protocol
        // (user/git_https/git.sh), and refusing an untrusted certificate.
        name: "git_https",
        cmdline: "qemu_exit start=nginx.exe start_args=-p%20C:/nginx/%20-c%20conf/nginx.conf \
                  cwd=C:/Users/gala run=busybox.exe args=sh%20C:/gala/git.sh",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "ls-remote: exit 0 58e30b428a549e95e6456011efecb254e82a9265\tHEAD",
            "clone: exit 0",
            "file: hello over https",
            "log: served by nginx",
            "SEC_E_UNTRUSTED_ROOT",
            "untrusted: exit 128",
            "git.sh: done",
            "exited with status 0x00000000",
        ],
    },
    Scenario {
        // Schannel (secur32 + schannel.dll): TLS 1.3 and 1.2 clients
        // against Nginx (OpenSSL) on the loopback address, certificate
        // validation, and a server (AcceptSecurityContext) in-process.
        name: "schannel_test",
        cmdline: "qemu_exit start=nginx.exe start_args=-p%20C:/nginx/%20-c%20conf/nginx.conf \
                  run=schannel_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["schannel_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "go_exit",
        cmdline: "qemu_exit run=go_exit.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["go_exit: exiting with 3", "go_exit.exe finished with status 0x00000003"],
    },
    Scenario {
        name: "kernel32_env_test",
        cmdline: "qemu_exit run=kernel32_env_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["kernel32_env_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "kernel32_file_test",
        cmdline: "qemu_exit run=kernel32_file_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["kernel32_file_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // H1: anonymous pipes (CreatePipe, PeekNamedPipe, EOF, inheritance).
        name: "pipe_test",
        cmdline: "qemu_exit run=pipe_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["pipe_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // H10: byte-range file locks (LockFileEx, UnlockFileEx, LockFile).
        name: "lock_test",
        cmdline: "qemu_exit run=lock_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["lock_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "ledger_test",
        cmdline: "qemu_exit run=ledger_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "ledger_test: ok",
            "exited with status 0x00000000",
            r#"ledger: {"service":"?","what":"unknown service","detail":4095,"hits":1}"#,
            r#"ledger: {"service":"NtQueryInformationProcess","what":"info class","detail":32512,"hits":2}"#,
        ],
    },
    Scenario {
        name: "context_test",
        cmdline: "qemu_exit run=context_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["context_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "iocp_test",
        cmdline: "qemu_exit run=iocp_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["iocp_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "symlink_test",
        cmdline: "qemu_exit run=symlink_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["symlink_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "sync_test",
        cmdline: "qemu_exit run=sync_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["sync_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "ioapc_test",
        cmdline: "qemu_exit run=ioapc_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["ioapc_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "apc_test",
        cmdline: "qemu_exit run=apc_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["apc_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "async_test",
        cmdline: "qemu_exit run=async_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["async_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "cow_test",
        cmdline: "qemu_exit run=cow_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["cow_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "filemap_test",
        cmdline: "qemu_exit run=filemap_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["filemap_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "threadinfo_test",
        cmdline: "qemu_exit run=threadinfo_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["threadinfo_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "sysinfo_test",
        cmdline: "qemu_exit run=sysinfo_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["sysinfo_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "fpu_test",
        cmdline: "qemu_exit run=fpu_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["fpu_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "args_test",
        // `args=` is percent-encoded: %20 space, %22 quote, %25 percent.
        cmdline: "qemu_exit run=args_test.exe args=alpha%20%22beta%20gamma%22",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["args_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "trace",
        cmdline: "qemu_exit trace run=hello.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "trace: pid 4 tid 8 NtWriteFile(",
            ") -> 0x00000000",
            "NtTerminateProcess(0xffffffffffffffff, 0x0)",
            "Hello from user mode!",
        ],
    },
    Scenario {
        name: "import_test",
        cmdline: "qemu_exit run=import_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["import_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "guard_test",
        cmdline: "qemu_exit run=guard_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["guard_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // P1: frame-based exception handling (unwind data, language
        // handlers, RtlVirtualUnwind/RtlUnwindEx/RtlRaiseException).
        name: "seh_test",
        cmdline: "qemu_exit run=seh_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["seh_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "exception_test",
        cmdline: "qemu_exit run=exception_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["exception_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "startup_test",
        cmdline: "qemu_exit run=startup_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["startup_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "peb_test",
        cmdline: "qemu_exit run=peb_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["peb_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // WOW64 (ARCHITECTURE "Plan: WOW64"): a 32-bit program in
        // compatibility mode, its TEB32, PEB32 and parameters, NT services
        // through gala's wow64.dll.
        name: "wow64_test",
        cmdline: "qemu_exit run=wow64_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "personality nt, 32-bit x86 (WOW64)",
            "wow64_test: ok",
            "exited with status 0x00000000",
        ],
    },
    Scenario {
        // WOW64: 32-bit threads (NtCreateThread with the x86 CONTEXT, their
        // TEB32s, exit), their contexts (CS 0x23) when suspended in 32-bit
        // code or in a system call, NtSetContextThread.
        name: "wow64_thread_test",
        cmdline: "qemu_exit run=wow64_thread_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["wow64_thread_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // WOW64 stage 3: a 32-bit program with imports, bound by the
        // 32-bit ntdll (SysWOW64) before its entry point: a relocated DLL,
        // TLS and its callbacks, DllMain, the loader's services.
        name: "wow64_import_test",
        cmdline: "qemu_exit run=wow64_import_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "personality nt, 32-bit x86 (WOW64)",
            "wow64_import_test: ok",
            "exited with status 0x00000000",
        ],
    },
    Scenario {
        // WOW64 stage 3: exceptions in 32-bit code through the 32-bit
        // ntdll's dispatcher (x86 SEH frames, RtlUnwind, RtlRaiseException,
        // vectored handlers) and user APCs with 32-bit routines.
        name: "wow64_seh_test",
        cmdline: "qemu_exit run=wow64_seh_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["wow64_seh_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // WOW64 stage 3: the file system redirector (System32, Sysnative,
        // an exemption, turned off per thread) and the 32-bit environment.
        name: "wow64_fs_test",
        cmdline: "qemu_exit run=wow64_fs_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["wow64_fs_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // WOW64 stage 3: a plain 32-bit program on gala's 32-bit kernel32
        // (SysWOW64): files, threads, what a 32-bit process is told.
        name: "wow64_k32_test",
        cmdline: "qemu_exit run=wow64_k32_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["wow64_k32_test: hello", "wow64_k32_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // WOW64 stage 3: a plain 32-bit C program (user/wow64_c_test:
        // clang-built, checked-in binary; stdcall imports through an
        // import library, not large-address-aware) on the 32-bit kernel32.
        name: "wow64_c_test",
        cmdline: "qemu_exit run=wow64_c_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["wow64_c_test: hello", "wow64_c_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // WOW64 stage 4: a 32-bit GUI program (user/wow64_gui_test, mingw's
        // i686 compiler, checked-in binary) on the 32-bit user32 and gdi32:
        // a window, its messages with 32-bit WPARAM/LPARAM, painting read
        // back, destruction (ARCHITECTURE "Plan: WOW64").
        name: "wow64_gui_test",
        cmdline: "qemu_exit run=wow64_gui_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["wow64_gui_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // WOW64: Winsock from a 32-bit program (user/wow64_sock_test,
        // mingw's i686 compiler, checked-in binary): the x86 WSADATA,
        // ADDRINFOA, WSABUF and fd_set, TCP and UDP on the loopback
        // address through AFD (whose structures the kernel reads in the
        // caller's layout).
        name: "wow64_sock_test",
        cmdline: "qemu_exit run=wow64_sock_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["wow64_sock_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // WOW64 stage 4: an x86 Inno Setup installer (Inno Setup 6.5.4's
        // own, PREBUILT innosetup6) installs silently for the current user:
        // its 32-bit setup loader, the Setup it extracts and runs on gala's
        // 32-bit DLLs (the files, the Start menu's shortcuts through the
        // 32-bit shell32's ShellLink, the uninstall entry), then the
        // installed 32-bit Compil32.exe /ASSOC; Inno's own log is dumped.
        name: "wow64_inno",
        cmdline: "qemu_exit run=innosetup-6.5.4.exe args=/SP-%20/VERYSILENT%20/SUPPRESSMSGBOXES%20/NORESTART%20/CURRENTUSER%20/LOG=C:\\inno.log dump=C:/inno.log",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "64-bit install mode: No",
            "Programs\\Inno Setup 6\\Compil32.exe",
            "Successfully created the icon.",
            "Creating new uninstall key: HKEY_CURRENT_USER",
            "Installation process succeeded.",
            "Process exit code: 0",
            "innosetup-6.5.4.exe (pid 4) exited with status 0x00000000",
        ],
    },
    Scenario {
        // WOW64 stage 3: a 32-bit C program on the 32-bit msvcrt
        // (user/wow64_crt_test: mingw's i686 startup code, checked-in
        // binary): printf/scanf's 4-byte va_list slots, setjmp/longjmp's
        // x86 jump buffer, FILE and lconv layouts, the x87 control word,
        // the 32-bit time_t, atexit.
        name: "wow64_crt_test",
        cmdline: "qemu_exit run=wow64_crt_test.exe args=one%20%22two%20three%22",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "wow64_crt_test: hello 32 bit 1.50",
            "wow64_crt_test: ok",
            "wow64_crt_test: atexit ran",
            "exited with status 0x00000000",
        ],
    },
    Scenario {
        // WOW64 stage 3: a 32-bit program on the 32-bit advapi32
        // (user/wow64_adv_test): the registry and its 32-bit view, tokens,
        // SIDs, security descriptors and ACLs in their x86 layouts,
        // CryptGenRandom.
        name: "wow64_adv_test",
        cmdline: "qemu_exit run=wow64_adv_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["wow64_adv_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        // WOW64 stage 3: an exception no 32-bit handler takes, through the
        // 32-bit ntdll: the crash report's 32-bit registers.
        name: "wow64_seh_crash",
        cmdline: "qemu_exit run=wow64_seh_test.exe args=crash",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "wow64_seh_test: writing to address 0",
            "crash report: wow64_seh_test.exe (pid 4) ended by an unhandled exception \
             0xc0000005 (access violation) in thread",
            "writing address 0x0",
            "crash report:   personality nt, 32-bit x86 (WOW64), session ",
            "ebx=0x12345678",
            "process wow64_seh_test.exe (pid 4) exited with status 0xc0000005",
            BOOT_OK_MARKER,
        ],
    },
    Scenario {
        // WOW64: a 32-bit program's unhandled exception, its crash report
        // with the 32-bit registers.
        name: "wow64_crash",
        cmdline: "qemu_exit run=wow64_test.exe args=crash",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "wow64_test: writing to address 0",
            "crash report: wow64_test.exe (pid 4) ended by an unhandled exception 0xc0000005 \
             (access violation) in thread",
            "(wow64_test.exe+0x",
            "writing address 0x0",
            "crash report:   personality nt, 32-bit x86 (WOW64), session ",
            "crash report:   eax=",
            "crash report:   eip=",
            "crash report:   [esp+0x00]",
            "process wow64_test.exe (pid 4) exited with status 0xc0000005",
            BOOT_OK_MARKER,
        ],
    },
    Scenario {
        name: "dir_test",
        cmdline: "qemu_exit run=dir_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["dir_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "image_test",
        cmdline: "qemu_exit run=image_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["image_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "file_test",
        cmdline: "qemu_exit run=file_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["file_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "section_test",
        cmdline: "qemu_exit run=section_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["section_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "kuser_test",
        cmdline: "qemu_exit run=kuser_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["kuser_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "thread_test",
        cmdline: "qemu_exit run=thread_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["thread_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "object_test",
        cmdline: "qemu_exit run=object_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["object_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "wait_test",
        cmdline: "qemu_exit run=wait_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["wait_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "vm_test",
        cmdline: "qemu_exit run=vm_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["vm_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "abi_test",
        cmdline: "qemu_exit run=abi_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["abi_test: write ok", "abi_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "crash",
        cmdline: "qemu_exit run=crash.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "crash: writing to address 0",
            "user exception: page fault (#PF)",
            // The crash report (nt::crashreport).
            "crash report: crash.exe (pid 4) ended by an unhandled exception 0xc0000005 \
             (access violation) in thread",
            "(crash.exe+0x",
            "writing address 0x0",
            // The personality (ROADMAP LX seam 2) and session.
            "crash report:   personality nt, session ",
            "crash report:   rax=",
            "crash report:   [rsp+0x00]",
            "process crash.exe (pid 4) exited with status 0xc0000005",
            BOOT_OK_MARKER,
        ],
    },
    Scenario {
        // The same crash with ntdll: the exception goes to its dispatcher,
        // finds no handler and ends the process (as on Windows), and the
        // crash report says where.
        name: "crash_dispatched",
        cmdline: "qemu_exit run=crash.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "crash: writing to address 0",
            "crash report: crash.exe (pid 4) ended by an unhandled exception 0xc0000005 \
             (access violation) in thread",
            "(crash.exe+0x",
            "writing address 0x0",
            "process crash.exe (pid 4) exited with status 0xc0000005",
            BOOT_OK_MARKER,
        ],
    },
    Scenario {
        // The same crash taken by a top-level filter that ends the process
        // with its own code (Crashpad's 0xffff7002 when its handler cannot
        // snapshot): the report still describes the exception.
        name: "crash_filtered",
        cmdline: "qemu_exit run=crash.exe args=filter",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "crash: the top-level filter ends the process",
            "crash report: crash.exe (pid 4) ended with status 0xffff7002 after an unhandled \
             exception 0xc0000005 (access violation) in thread",
            "(crash.exe+0x",
            "writing address 0x0",
            "process crash.exe (pid 4) exited with status 0xffff7002",
            BOOT_OK_MARKER,
        ],
    },
    // The Linux personality (ROADMAP LX milestone 1): each user/lx program,
    // static and static-pie, with the arguments and environment `cargo
    // xtask lx-expect` gives it on Linux.
    Scenario {
        // user/lx/hello_test, static (ET_EXEC): its transcript must be its
        // expected.txt (real Linux's; `lx_expect::check_boot`).
        name: "lx_hello",
        cmdline: "qemu_exit lxrun=/lx/hello_test lxargs=./hello_test%20first%20%27%27%20%27third%20arg%27 lxenv=A=1%20EMPTY=%20%27LONGER_NAME=some%20value%27",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["personality linux", "run: end /lx/hello_test", BOOT_OK_MARKER],
    },
    Scenario {
        // user/lx/hello_test, static-pie (ET_DYN): its transcript must be its
        // expected.txt (real Linux's; `lx_expect::check_boot`).
        name: "lx_hello_pie",
        cmdline: "qemu_exit lxrun=/lx/hello_test-pie lxargs=./hello_test%20first%20%27%27%20%27third%20arg%27 lxenv=A=1%20EMPTY=%20%27LONGER_NAME=some%20value%27",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["personality linux", "run: end /lx/hello_test-pie", BOOT_OK_MARKER],
    },
    Scenario {
        // user/lx/write_exit_test, static (ET_EXEC): its transcript must be its
        // expected.txt (real Linux's; `lx_expect::check_boot`).
        name: "lx_write_exit",
        cmdline: "qemu_exit lxrun=/lx/write_exit_test lxargs=./write_exit_test%20first%20%27%27%20%27third%20arg%27 lxenv=A=1%20EMPTY=%20%27LONGER_NAME=some%20value%27",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["personality linux", "run: end /lx/write_exit_test", BOOT_OK_MARKER],
    },
    Scenario {
        // user/lx/write_exit_test, static-pie (ET_DYN): its transcript must be its
        // expected.txt (real Linux's; `lx_expect::check_boot`).
        name: "lx_write_exit_pie",
        cmdline: "qemu_exit lxrun=/lx/write_exit_test-pie lxargs=./write_exit_test%20first%20%27%27%20%27third%20arg%27 lxenv=A=1%20EMPTY=%20%27LONGER_NAME=some%20value%27",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["personality linux", "run: end /lx/write_exit_test-pie", BOOT_OK_MARKER],
    },
    Scenario {
        // user/lx/pipe_test (LX milestone 2): its transcript must be its
        // expected.txt (real Linux's; `lx_expect::check_boot`).
        name: "lx_pipe",
        cmdline: "qemu_exit lxrun=/lx/pipe_test lxargs=./pipe_test",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["personality linux", "run: end /lx/pipe_test", BOOT_OK_MARKER],
    },
    Scenario {
        // user/lx/forkmem_test (LD step 1): fork's memory, its transcript
        // real Linux's (`lx_expect::check_boot`); a fork shares its
        // private pages copy-on-write and copies none (the fork line).
        name: "lx_forkmem",
        cmdline: "qemu_exit lxrun=/lx/forkmem_test lxargs=./forkmem_test%20first%20%27%27%20%27third%20arg%27 lxenv=A=1%20EMPTY=%20%27LONGER_NAME=some%20value%27",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "personality linux",
            "pages shared copy-on-write, 0 copied",
            "run: end /lx/forkmem_test",
            BOOT_OK_MARKER,
        ],
    },
    Scenario {
        // user/lx/forkmem_test, static-pie: as `lx_forkmem`.
        name: "lx_forkmem_pie",
        cmdline: "qemu_exit lxrun=/lx/forkmem_test-pie lxargs=./forkmem_test%20first%20%27%27%20%27third%20arg%27 lxenv=A=1%20EMPTY=%20%27LONGER_NAME=some%20value%27",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "personality linux",
            "pages shared copy-on-write, 0 copied",
            "run: end /lx/forkmem_test-pie",
            BOOT_OK_MARKER,
        ],
    },
    Scenario {
        // user/lx/brk_test, static (ET_EXEC): its transcript must be its
        // expected.txt (real Linux's; `lx_expect::check_boot`).
        name: "lx_brk",
        cmdline: "qemu_exit lxrun=/lx/brk_test lxargs=./brk_test%20first%20%27%27%20%27third%20arg%27 lxenv=A=1%20EMPTY=%20%27LONGER_NAME=some%20value%27",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["personality linux", "run: end /lx/brk_test", BOOT_OK_MARKER],
    },
    Scenario {
        // user/lx/brk_test, static-pie (ET_DYN): its transcript must be its
        // expected.txt (real Linux's; `lx_expect::check_boot`).
        name: "lx_brk_pie",
        cmdline: "qemu_exit lxrun=/lx/brk_test-pie lxargs=./brk_test%20first%20%27%27%20%27third%20arg%27 lxenv=A=1%20EMPTY=%20%27LONGER_NAME=some%20value%27",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["personality linux", "run: end /lx/brk_test-pie", BOOT_OK_MARKER],
    },
    Scenario {
        // user/lx/mmap_test, static (ET_EXEC): its transcript must be its
        // expected.txt (real Linux's; `lx_expect::check_boot`).
        name: "lx_mmap",
        cmdline: "qemu_exit lxrun=/lx/mmap_test lxargs=./mmap_test%20first%20%27%27%20%27third%20arg%27 lxenv=A=1%20EMPTY=%20%27LONGER_NAME=some%20value%27",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["personality linux", "run: end /lx/mmap_test", BOOT_OK_MARKER],
    },
    Scenario {
        // user/lx/mmap_test, static-pie (ET_DYN): its transcript must be its
        // expected.txt (real Linux's; `lx_expect::check_boot`).
        name: "lx_mmap_pie",
        cmdline: "qemu_exit lxrun=/lx/mmap_test-pie lxargs=./mmap_test%20first%20%27%27%20%27third%20arg%27 lxenv=A=1%20EMPTY=%20%27LONGER_NAME=some%20value%27",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["personality linux", "run: end /lx/mmap_test-pie", BOOT_OK_MARKER],
    },
    Scenario {
        // user/lx/arch_prctl_test, static (ET_EXEC): its transcript must be its
        // expected.txt (real Linux's; `lx_expect::check_boot`).
        name: "lx_arch_prctl",
        cmdline: "qemu_exit lxrun=/lx/arch_prctl_test lxargs=./arch_prctl_test%20first%20%27%27%20%27third%20arg%27 lxenv=A=1%20EMPTY=%20%27LONGER_NAME=some%20value%27",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["personality linux", "run: end /lx/arch_prctl_test", BOOT_OK_MARKER],
    },
    Scenario {
        // user/lx/arch_prctl_test, static-pie (ET_DYN): its transcript must be its
        // expected.txt (real Linux's; `lx_expect::check_boot`).
        name: "lx_arch_prctl_pie",
        cmdline: "qemu_exit lxrun=/lx/arch_prctl_test-pie lxargs=./arch_prctl_test%20first%20%27%27%20%27third%20arg%27 lxenv=A=1%20EMPTY=%20%27LONGER_NAME=some%20value%27",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["personality linux", "run: end /lx/arch_prctl_test-pie", BOOT_OK_MARKER],
    },
    Scenario {
        // user/lx/auxv_test, static (ET_EXEC): its transcript must be its
        // expected.txt (real Linux's; `lx_expect::check_boot`).
        name: "lx_auxv",
        cmdline: "qemu_exit lxrun=/lx/auxv_test lxargs=./auxv_test%20first%20%27%27%20%27third%20arg%27 lxenv=A=1%20EMPTY=%20%27LONGER_NAME=some%20value%27",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["personality linux", "run: end /lx/auxv_test", BOOT_OK_MARKER],
    },
    Scenario {
        // user/lx/dyn_test, linked dynamically against glibc (ROADMAP LD):
        // started through its interpreter, ld.so, which loads libdyn.so
        // and glibc's libc from the image; its transcript must be its
        // expected.txt (real Linux's).
        name: "lx_dyn",
        cmdline: "qemu_exit lxrun=/lx/dyn_test lxargs=./dyn_test%20first%20%27%27%20%27third%20arg%27 lxenv=A=1%20EMPTY=%20%27LONGER_NAME=some%20value%27",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["personality linux", "run: end /lx/dyn_test", BOOT_OK_MARKER],
    },
    Scenario {
        // user/lx/dyn_test as a PIE with an interpreter.
        name: "lx_dyn_pie",
        cmdline: "qemu_exit lxrun=/lx/dyn_test-pie lxargs=./dyn_test%20first%20%27%27%20%27third%20arg%27 lxenv=A=1%20EMPTY=%20%27LONGER_NAME=some%20value%27",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["personality linux", "run: end /lx/dyn_test-pie", BOOT_OK_MARKER],
    },
    Scenario {
        // user/lx/auxv_test, static-pie (ET_DYN): its transcript must be its
        // expected.txt (real Linux's; `lx_expect::check_boot`).
        name: "lx_auxv_pie",
        cmdline: "qemu_exit lxrun=/lx/auxv_test-pie lxargs=./auxv_test%20first%20%27%27%20%27third%20arg%27 lxenv=A=1%20EMPTY=%20%27LONGER_NAME=some%20value%27",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["personality linux", "run: end /lx/auxv_test-pie", BOOT_OK_MARKER],
    },
    Scenario {
        // user/lx/fileio_test (LX milestone 2): files and directories; its
        // transcript must be its expected.txt (real Linux's;
        // `lx_expect::check_boot`).
        name: "lx_fileio",
        cmdline: "qemu_exit lxrun=/lx/fileio_test lxargs=./fileio_test",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["personality linux", "run: end /lx/fileio_test", BOOT_OK_MARKER],
    },
    Scenario {
        // user/lx/fileio_test as a static-pie.
        name: "lx_fileio_pie",
        cmdline: "qemu_exit lxrun=/lx/fileio_test-pie lxargs=./fileio_test",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["personality linux", "run: end /lx/fileio_test-pie", BOOT_OK_MARKER],
    },
    Scenario {
        // user/lx/exec_test (LX milestone 2): execve, #! scripts, getppid,
        // uname; its transcript must be its expected.txt (real Linux's;
        // `lx_expect::check_boot`).
        name: "lx_exec",
        cmdline: "qemu_exit lxrun=/lx/exec_test lxargs=./exec_test",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["personality linux", "run: end /lx/exec_test", BOOT_OK_MARKER],
    },
    Scenario {
        // user/lx/exec_test as a static-pie (it execs itself by argv[0]).
        name: "lx_exec_pie",
        cmdline: "qemu_exit lxrun=/lx/exec_test-pie lxargs=./exec_test-pie",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["personality linux", "run: end /lx/exec_test-pie", BOOT_OK_MARKER],
    },
    Scenario {
        // user/lx/socket_test (LX, H11): TCP sockets over loopback on the
        // kernel's stack, poll; its transcript must be its expected.txt
        // (real Linux's; `lx_expect::check_boot`).
        name: "lx_socket",
        cmdline: "qemu_exit lxrun=/lx/socket_test lxargs=./socket_test",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["personality linux", "run: end /lx/socket_test", BOOT_OK_MARKER],
    },
    Scenario {
        // user/lx/socket_test as a static-pie.
        name: "lx_socket_pie",
        cmdline: "qemu_exit lxrun=/lx/socket_test-pie lxargs=./socket_test",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["personality linux", "run: end /lx/socket_test-pie", BOOT_OK_MARKER],
    },
    Scenario {
        // user/lx/memfd_test (LD step 2): memfd_create, mappings every
        // process shares, the seals; its transcript must be its
        // expected.txt (real Linux's; `lx_expect::check_boot`).
        name: "lx_memfd",
        cmdline: "qemu_exit lxrun=/lx/memfd_test lxargs=./memfd_test",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["personality linux", "run: end /lx/memfd_test", BOOT_OK_MARKER],
    },
    Scenario {
        // user/lx/memfd_test as a static-pie.
        name: "lx_memfd_pie",
        cmdline: "qemu_exit lxrun=/lx/memfd_test-pie lxargs=./memfd_test",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["personality linux", "run: end /lx/memfd_test-pie", BOOT_OK_MARKER],
    },
    Scenario {
        // user/lx/unix_test (LD step 2): AF_UNIX sockets, names, SCM_RIGHTS
        // from a forked child, socket pairs of every kind; its transcript
        // must be its expected.txt (real Linux's; `lx_expect::check_boot`).
        name: "lx_unix",
        cmdline: "qemu_exit lxrun=/lx/unix_test lxargs=./unix_test",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["personality linux", "run: end /lx/unix_test", BOOT_OK_MARKER],
    },
    Scenario {
        // user/lx/unix_test as a static-pie.
        name: "lx_unix_pie",
        cmdline: "qemu_exit lxrun=/lx/unix_test-pie lxargs=./unix_test",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["personality linux", "run: end /lx/unix_test-pie", BOOT_OK_MARKER],
    },
    Scenario {
        // user/lx/thread_test (LX): clone of threads, futex, signals per
        // thread, musl's pthreads; its transcript must be its expected.txt
        // (real Linux's; `lx_expect::check_boot`).
        name: "lx_thread",
        cmdline: "qemu_exit lxrun=/lx/thread_test lxargs=./thread_test",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["personality linux", "run: end /lx/thread_test", BOOT_OK_MARKER],
    },
    Scenario {
        // user/lx/thread_test as a static-pie.
        name: "lx_thread_pie",
        cmdline: "qemu_exit lxrun=/lx/thread_test-pie lxargs=./thread_test",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["personality linux", "run: end /lx/thread_test-pie", BOOT_OK_MARKER],
    },
    Scenario {
        // user/lx/poll_test (LX): poll, select and epoll on pipes; its
        // transcript must be its expected.txt (real Linux's;
        // `lx_expect::check_boot`).
        name: "lx_poll",
        cmdline: "qemu_exit lxrun=/lx/poll_test lxargs=./poll_test",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["personality linux", "run: end /lx/poll_test", BOOT_OK_MARKER],
    },
    Scenario {
        // user/lx/poll_test as a static-pie.
        name: "lx_poll_pie",
        cmdline: "qemu_exit lxrun=/lx/poll_test-pie lxargs=./poll_test",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["personality linux", "run: end /lx/poll_test-pie", BOOT_OK_MARKER],
    },
    Scenario {
        // user/lx/epoll_test (LX): epoll on sockets, epoll fds, blocking
        // waits woken by a thread or a signal; its transcript must be its
        // expected.txt (real Linux's; `lx_expect::check_boot`).
        name: "lx_epoll",
        cmdline: "qemu_exit lxrun=/lx/epoll_test lxargs=./epoll_test",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["personality linux", "run: end /lx/epoll_test", BOOT_OK_MARKER],
    },
    Scenario {
        // user/lx/epoll_test as a static-pie.
        name: "lx_epoll_pie",
        cmdline: "qemu_exit lxrun=/lx/epoll_test-pie lxargs=./epoll_test",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["personality linux", "run: end /lx/epoll_test-pie", BOOT_OK_MARKER],
    },
    Scenario {
        // user/lx/fs_test (LX seam 4): symbolic and hard links, unlink of
        // an open file, names differing only in case, the stat family and
        // statx, renameat2; its transcript must be its expected.txt (real
        // Linux's; `lx_expect::check_boot`).
        name: "lx_fs",
        cmdline: "qemu_exit lxrun=/lx/fs_test lxargs=./fs_test",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["personality linux", "run: end /lx/fs_test", BOOT_OK_MARKER],
    },
    Scenario {
        // user/lx/fs_test as a static-pie.
        name: "lx_fs_pie",
        cmdline: "qemu_exit lxrun=/lx/fs_test-pie lxargs=./fs_test",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["personality linux", "run: end /lx/fs_test-pie", BOOT_OK_MARKER],
    },
    Scenario {
        // A static Linux busybox (ROADMAP LX milestone 2): its shell runs
        // user/lx/busybox/shell.sh (pipes, redirections, the coreutils-style
        // applets through execve); the transcript must be
        // user/lx/busybox/lx_busybox.txt (`lx_expect::check_boot`).
        name: "lx_busybox",
        cmdline: "qemu_exit lxrun=/lx/busybox lxargs=busybox%20sh%20/lx/shell.sh lxenv=PATH=/lx",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["personality linux", "run: end /lx/busybox", BOOT_OK_MARKER],
    },
    Scenario {
        // busybox httpd serves a directory on 127.0.0.1:8080 and busybox
        // wget fetches files from it (ROADMAP H11 on the Linux side: Linux
        // sockets on the kernel's TCP stack); user/lx/busybox/httpd.sh,
        // the transcript must be user/lx/busybox/lx_httpd.txt.
        name: "lx_httpd",
        cmdline: "qemu_exit lxrun=/lx/busybox lxargs=busybox%20sh%20/lx/httpd.sh lxenv=PATH=/lx",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["personality linux", "run: end /lx/busybox", BOOT_OK_MARKER],
    },
    Scenario {
        name: "hello",
        cmdline: "qemu_exit run=hello.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "Hello from user mode!",
            "process hello.exe (pid 4) exited with status 0x00000000",
            BOOT_OK_MARKER,
        ],
    },
    Scenario {
        name: "selftest",
        cmdline: "qemu_exit selftest",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[BOOT_OK_MARKER, "selftest: "],
    },
    Scenario {
        name: "fault-ud",
        cmdline: "qemu_exit fault=ud",
        exit_status: QEMU_EXIT_FAILURE,
        expect: &["KERNEL PANIC", "CPU exception: invalid opcode (#UD)", "backtrace: 0xffffffff8"],
    },
    Scenario {
        name: "fault-pagefault",
        cmdline: "qemu_exit fault=pagefault",
        exit_status: QEMU_EXIT_FAILURE,
        expect: &[
            "CPU exception: page fault (#PF) error=0x0",
            "cr2=0x0000000000000008",
            "backtrace: 0xffffffff8",
        ],
    },
    Scenario {
        name: "fault-stackoverflow",
        cmdline: "qemu_exit fault=stackoverflow",
        exit_status: QEMU_EXIT_FAILURE,
        expect: &["CPU exception: double fault (#DF)"],
    },
];

/// Boots every scenario (matching `filter`) headless, in parallel.
/// Programs per shared VM at most (more VMs than CPUs only queue).
const MAX_BATCH: usize = 12;

/// One VM of a boot test run: a scenario of its own, or several
/// compatible scenarios whose programs run one after another.
enum Vm<'a> {
    Single(&'a Scenario),
    Batch(Vec<&'a Scenario>),
}

/// ` events=<list>` when `XTASK_EVENTS` asks for the event stream in every
/// boot (written to `<iso>.events.jsonl`).
fn events_flag() -> String {
    let events = env::var("XTASK_EVENTS").map(|e| format!(" events={e}")).unwrap_or_default();
    // `XTASK_RECORD=1`: every boot records its programs' inputs
    // (`<iso>.rec`; kernel/src/replay.rs).
    let record = if env::var_os("XTASK_RECORD").is_some() { " record" } else { "" };
    // `XTASK_CMDLINE="checkuser tlbparanoid"`: more kernel command-line
    // words for every boot (diagnostic flags, ARCHITECTURE "Kernel
    // command line").
    let extra = env::var("XTASK_CMDLINE").map(|c| format!(" {c}")).unwrap_or_default();
    format!("{events}{record}{extra}")
}

/// Scenarios that ship named sets of EXTRA_MODULES instead of their own
/// (the same set: they share a VM); a scenario listed more than once
/// ships the union of its sets.
const MODULE_SETS: &[(&str, &str)] = &[
    ("events_process", "process_test"),
    ("events_ipc", "pipe_test"),
    ("events_pid", "process_test"),
    ("events_match", "process_test"),
    ("quota_sockets", "quota_test"),
    ("pk_install_zip", "pk_install_test"),
    ("pk_install_machine", "pk_install_test"),
    ("pk_install_inno", "pk_install_test"),
    ("pk_install_inno", "pk_install_inno"),
    ("pk_install_inno32", "pk_install_test"),
    ("pk_install_inno32", "wow64_inno"),
    ("pk_install_inno32", "pk_install_inno32"),
    ("pk_install_nsis", "pk_install_test"),
    ("pk_install_nsis", "pk_install_nsis"),
    ("pk_install_https", "pk_install_test"),
    ("pk_install_https", "schannel_test"),
    ("pk_install_https", "pk_install_https"),
    ("onecpu_programs", "smpsched_test"),
    ("kernel_watchdog", "hello"),
    ("service_restart", "hello"),
    ("server_profile", "user32_test"),
    ("noscreen_windows", "presenter_test"),
    ("server_profile_windows", "presenter_test"),
    ("gala_cli", "gui_test"),
    ("virtio_gpu", "gui_test"),
    ("gui_test_headless", "gui_test"),
    ("virtio_gpu_reset", "gui_test"),
    ("gala_cli", "gala_cli_test"),
    ("net_reset", "go_http"),
    ("disk_reset", "disk_test"),
    ("handover_disk", "persist_test"),
    ("handover_k32", "handover_k32_test"),
    ("handover_k32_uefi", "handover_k32_test"),
    ("handover_net", "handover_net_test"),
    ("handover_net_uefi", "handover_net_test"),
    ("handover_net_self", "handover_net_test"),
    ("udp_echo", "udp_test"),
    ("handover_k32_twice", "handover_k32_test"),
    ("handover_upgrade", "handover_k32_test"),
    ("handover_upgrade_uefi", "handover_k32_test"),
    ("handover_downgrade_refused", "handover_k32_test"),
    ("crash_record", "hello"),
    ("watchdog", "busybox"),
    ("failpoint_heap", "kernel32_test"),
    ("failpoint_frame", "kernel32_test"),
    ("failpoint_random", "kernel32_test"),
    ("failpoint_disk", "persist_test"),
    ("failpoint_disk_delay", "persist_test"),
    ("dotnet_info", "dotnet"),
    ("filezilla_server", "filezilla"),
    ("sshd_server", "sshd_login"),
    ("sshd_server", "sshd_server"),
    ("sshd_shell", "sshd_login"),
    ("sshd_shell", "sshd_server"),
    ("sshd_shell", "sshd_shell"),
    ("sshd_sftp", "sshd_login"),
    ("sshd_sftp", "sshd_server"),
    ("sshd_sftp", "sshd_sftp"),
    ("sshd_scp", "sshd_login"),
    ("sshd_scp", "sshd_server"),
    ("sshd_scp", "sshd_sftp"),
    ("sshd_scp", "sshd_scp"),
    ("sshd_forward", "sshd_login"),
    ("sshd_forward", "sshd_server"),
    ("sshd_client", "sshd_login"),
    ("sshd_client", "sshd_server"),
    ("sshd_client", "sshd_client"),
    ("dotnet_hello", "dotnet"),
    ("flaui_test", "dotnet"),
    ("filezilla_flaui", "dotnet"),
    ("filezilla_flaui", "filezilla"),
    ("filezilla_sftp", "dotnet"),
    ("filezilla_sftp", "filezilla"),
    ("filezilla_sftp", "sshd_login"),
    ("filezilla_sftp", "sshd_server"),
    ("filezilla_sftp", "sshd_sftp"),
    ("pwsh_hello", "pwsh"),
    ("python_pip_https", "python_pip"),
    ("python_numpy", "python_pip"),
    ("python_numpy", "python_numpy"),
    ("python_pip_https", "schannel_test"),
    ("python_script", "python_hello"),
    ("python_script", "python_script"),
    ("python_async", "python_hello"),
    ("python_async", "python_async"),
    ("python_mp", "python_hello"),
    ("python_mp", "python_mp"),
    ("chromium_version", "chromium"),
    ("chromium_dump", "chromium"),
    ("chromium_https", "chromium"),
    ("chromium_host", "chromium"),
    ("chromium_window", "chromium"),
    ("chromium_uia", "chromium"),
    ("chromium_uia", "chromium_uia"),
    ("chromium_driver", "chromium"),
    ("chromium_driver", "chromium_driver"),
    ("chromium_wpt", "chromium"),
    ("chromium_wpt", "chromium_driver"),
    ("chromium_host", "chromium_host"),
    ("chromium_https", "schannel_test"),
    ("winhttp_test", "winhttp_test"),
    ("winhttp_test", "schannel_test"),
    ("wininet_test", "wininet_test"),
    ("wininet_test", "schannel_test"),
    ("curl", "curl"),
    ("curl", "curl_common"),
    ("curl", "schannel_test"),
    ("curl_schannel", "curl_schannel"),
    ("curl_schannel", "curl_common"),
    ("curl_schannel", "schannel_test"),
    ("git_https", "git_https"),
    ("git_https", "mingit"),
    ("git_https", "schannel_test"),
    ("sqllocaldb_persist", "sqllocaldb"),
    ("winsta_service", "winsta_test"),
    ("lx_dyn", "lx_dyn"),
    ("lx_dyn", "glibc"),
    ("lx_dyn_pie", "lx_dyn_pie"),
    ("lx_dyn_pie", "glibc"),
];

/// The boot modules (file, directory) scenario `name` ships besides the
/// test programs.
fn extra_modules(name: &str) -> Vec<(&'static str, &'static str)> {
    let mut sets: Vec<&str> = MODULE_SETS.iter().filter(|s| s.0 == name).map(|s| s.1).collect();
    if sets.is_empty() {
        sets.push(name);
    }
    let mut modules: Vec<_> =
        EXTRA_MODULES.iter().filter(|m| sets.contains(&m.0)).map(|m| (m.1, m.2)).collect();
    // kernelbase is part of the system wherever kernel32 is, as on
    // Windows: it hosts the core API sets (ARCHITECTURE "API sets").
    for (kernel32, kernelbase, dir) in [
        ("kernel32.dll", "kernelbase.dll", "Windows/System32"),
        ("x86/kernel32.dll", "x86/kernelbase.dll", "Windows/SysWOW64"),
    ] {
        if modules.contains(&(kernel32, dir)) {
            modules.push((kernelbase, dir));
        }
    }
    modules.sort_unstable();
    modules.dedup();
    modules
}

/// `(program file, args)` if the scenario can share a VM: it only runs one
/// program (no other kernel flags), expects success, boots by BIOS, and
/// expects nothing that depends on running first (process ids, a
/// session's desktop going live: logged once per boot).
fn batchable(s: &Scenario) -> Option<(&'static str, &'static str)> {
    if s.exit_status != QEMU_EXIT_SUCCESS
        || scenario_boot(s.name) != Boot::Bios
        || s.expect.iter().any(|e| e.contains("pid ") || e.starts_with("desktop: session "))
        || net_marker(s.name).is_some()
        || HOST_PAGE_SCENARIOS.contains(&s.name)
        || disk_scenario(s.name).is_some()
        || s.name.starts_with("onecpu_")
    {
        return None;
    }
    let (mut program, mut args) = (None, "");
    for word in s.cmdline.split_whitespace() {
        match word.split_once('=') {
            _ if word == "qemu_exit" => {}
            Some(("run", p)) if !p.contains(',') => program = Some(p),
            Some(("args", a)) if !a.contains(',') => args = a,
            _ => return None,
        }
    }
    program.map(|p| (p, args))
}

/// Groups the selected scenarios into VMs: batchable ones with identical
/// boot modules share VMs (spread over the host CPUs), the rest boot alone.
fn plan_vms<'a>(selected: &[&'a Scenario]) -> Vec<Vm<'a>> {
    use std::collections::BTreeMap;
    let mut groups: BTreeMap<Vec<(&str, &str)>, Vec<&Scenario>> = BTreeMap::new();
    let mut vms = Vec::new();
    for &s in selected {
        match batchable(s) {
            // Its own image, whose name picks the emulation (`qemu`).
            Some(_) if single_thread_tcg(s.name) => vms.push(Vm::Single(s)),
            Some(_) if env::var_os("XTASK_NO_BATCH").is_none() => {
                groups.entry(extra_modules(s.name)).or_default().push(s)
            }
            _ => vms.push(Vm::Single(s)),
        }
    }
    let cpus = thread::available_parallelism().map_or(2, |n| n.get());
    let batched: usize = groups.values().map(Vec::len).sum();
    let size = batched.div_ceil(cpus).clamp(1, MAX_BATCH);
    for group in groups.into_values() {
        for chunk in group.chunks(size) {
            vms.push(match chunk {
                [one] => Vm::Single(one),
                _ => Vm::Batch(chunk.to_vec()),
            });
        }
    }
    vms
}

/// `XTASK_SHARD=i/n` (1-based): only every n-th VM, starting at the i-th,
/// so n machines boot the whole set between them (CI's boot jobs). The VMs
/// are dealt out after `plan_vms`, so a batch stays in one shard; the
/// shard says what it took.
fn shard(vms: Vec<Vm<'_>>) -> Result<Vec<Vm<'_>>> {
    let Ok(spec) = env::var("XTASK_SHARD") else {
        return Ok(vms);
    };
    let parsed = spec
        .split_once('/')
        .and_then(|(i, n)| Some((i.parse::<usize>().ok()?, n.parse::<usize>().ok()?)));
    let Some((i, n)) = parsed.filter(|&(i, n)| i >= 1 && i <= n) else {
        return Err(format!("XTASK_SHARD={spec:?}: expected i/n with 1 <= i <= n"));
    };
    let total = vms.len();
    let mine: Vec<Vm<'_>> =
        vms.into_iter().enumerate().filter(|(k, _)| k % n == i - 1).map(|(_, vm)| vm).collect();
    let scenarios: usize = mine
        .iter()
        .map(|vm| match vm {
            Vm::Single(_) => 1,
            Vm::Batch(group) => group.len(),
        })
        .sum();
    eprintln!("xtask: shard {i} of {n}: {} of {total} VMs ({scenarios} scenarios)", mine.len());
    Ok(mine)
}

fn boot_tests(release: bool, filter: Option<&String>) -> Result {
    RELEASE.store(release, std::sync::atomic::Ordering::Relaxed);
    let t0 = Instant::now();
    build_kernel(release)?;
    let built = t0.elapsed();
    // With `XTASK_REPEAT`, a scenario's exact name selects it alone (it
    // repeats one scenario: handover_upgrade's name is part of
    // handover_upgrade_uefi's); otherwise the filter is a substring.
    let exact = env::var_os("XTASK_REPEAT").is_some()
        && filter.is_some_and(|f| SCENARIOS.iter().any(|s| s.name == f.as_str()));
    let selected: Vec<&Scenario> = SCENARIOS
        .iter()
        .filter(|s| match filter {
            // Pending scenarios run only when named exactly.
            None => !PENDING.contains(&s.name),
            Some(f) if exact => s.name == f.as_str(),
            // `name$`: that scenario only (`hello$`, not java_hello too).
            Some(f) if f.ends_with('$') => s.name == f.trim_end_matches('$'),
            // `a,b,c`: exactly these scenarios.
            Some(f) if f.contains(',') => f.split(',').any(|n| n == s.name),
            // A pending scenario runs when named exactly; others match
            // by substring.
            Some(f) => {
                s.name == f.as_str() || (s.name.contains(f.as_str()) && !PENDING.contains(&s.name))
            }
        })
        .collect();
    if selected.is_empty() {
        return Err("no boot test scenario matches the filter".into());
    }
    let vms = shard(plan_vms(&selected))?;
    // Tests read the serial log; the screen console stays on only where
    // the scenario boots by UEFI, which keeps that code covered.
    let screen = |s: &Scenario| {
        let screen = if scenario_boot(s.name) == Boot::Uefi { "" } else { " noscreen" };
        format!("{screen}{}", events_flag())
    };
    let make = |vm: &Vm| match vm {
        Vm::Single(s) => make_iso(
            release,
            &format!("test-{}", s.name),
            &format!("{}{}", s.cmdline, screen(s)),
            &extra_modules(s.name),
        ),
        Vm::Batch(group) => {
            let (programs, args): (Vec<_>, Vec<_>) =
                group.iter().filter_map(|s| batchable(s)).unzip();
            let cmdline = format!(
                "qemu_exit noscreen{} run={} args={}",
                events_flag(),
                programs.join(","),
                args.join(",")
            );
            make_iso(
                release,
                &format!("batch-{}", group[0].name),
                &cmdline,
                &extra_modules(group[0].name),
            )
        }
    };
    if let Some(times) = env::var("XTASK_REPEAT").ok().and_then(|n| n.parse::<usize>().ok()) {
        let isos = run_parallel(&vms, |vm| make(vm));
        let isos: Vec<PathBuf> = isos.into_iter().collect::<Result<_>>()?;
        return repeat_boots(&vms, &isos, times);
    }
    // Each VM's image is made just before it boots and deleted after a
    // pass, so only the images of the VMs running at once are on the disk
    // (making them all first filled the disk: about 50 MB each).
    let iso_time = std::sync::Mutex::new(Duration::ZERO);
    let t2 = Instant::now();
    let results = run_parallel(&vms, |vm| {
        let t = Instant::now();
        let iso = make(vm)?;
        if let Ok(mut total) = iso_time.lock() {
            *total += t.elapsed();
        }
        let result = boot_vm(vm, &iso);
        // A passing VM's image is not needed again (a failed one is kept
        // to boot by hand).
        if result.1.iter().all(|(_, v)| v.is_ok()) {
            let _ = fs::remove_file(&iso);
        }
        Ok(result)
    });
    let isos_made = iso_time.into_inner().unwrap_or_default();
    let booted = t2.elapsed();
    let mut failed = 0;
    let (mut firmware, mut kernel) = (Duration::ZERO, Duration::ZERO);
    let mut history = Vec::new();
    for (vm, result) in vms.iter().zip(results) {
        // A VM that could not be made or booted fails its scenarios (its
        // error was once dropped here, and the run passed without them).
        let (time, verdicts) = match result {
            Ok(r) => r,
            Err(e) => {
                let names: Vec<&str> = match vm {
                    Vm::Single(s) => vec![s.name],
                    Vm::Batch(group) => group.iter().map(|s| s.name).collect(),
                };
                for name in names {
                    failed += 1;
                    eprintln!("xtask: boot test {name:<20} FAILED: {e}");
                    history.push(HistoryRun {
                        scenario: name,
                        ok: false,
                        secs: 0.0,
                        shared: false,
                        reason: e.to_string(),
                    });
                }
                continue;
            }
        };
        firmware += time.to_banner;
        kernel += time.total.saturating_sub(time.to_banner);
        let shared = matches!(vm, Vm::Batch(_));
        for (scenario, verdict) in &verdicts {
            history.push(HistoryRun {
                scenario: scenario.name,
                ok: verdict.is_ok(),
                secs: time.total.as_secs_f64(),
                shared,
                reason: verdict
                    .as_ref()
                    .err()
                    .map(|e| e.split(" (serial log: ").next().unwrap_or(e).to_string())
                    .unwrap_or_default(),
            });
        }
        for (scenario, verdict) in verdicts {
            match verdict {
                Ok(()) if shared => eprintln!("xtask: boot test {:<20} ok", scenario.name),
                Ok(()) => eprintln!(
                    "xtask: boot test {:<20} ok  ({:.1}s to kernel, {:.1}s in kernel)",
                    scenario.name,
                    time.to_banner.as_secs_f64(),
                    time.total.saturating_sub(time.to_banner).as_secs_f64()
                ),
                Err(e) => {
                    failed += 1;
                    eprintln!("xtask: boot test {:<20} FAILED: {e}", scenario.name);
                }
            }
        }
        if let Vm::Batch(group) = vm {
            eprintln!(
                "xtask:   (shared VM of {} programs: {:.1}s to kernel, {:.1}s in kernel)",
                group.len(),
                time.to_banner.as_secs_f64(),
                time.total.saturating_sub(time.to_banner).as_secs_f64()
            );
        }
    }
    let summary = format!(
        "build {:.1}s, ISOs {:.1}s summed, boots {:.1}s wall (with the ISOs) ({} scenarios in {} VMs; summed: {:.1}s firmware+bootloader, {:.1}s kernel)",
        built.as_secs_f64(),
        isos_made.as_secs_f64(),
        booted.as_secs_f64(),
        selected.len(),
        vms.len(),
        firmware.as_secs_f64(),
        kernel.as_secs_f64()
    );
    eprintln!("xtask: timing: {summary}");
    let _ = fs::create_dir_all(report_dir());
    let _ = write(&report_dir().join("timing.txt"), &format!("{summary}\n"));
    // The suite's health over runs: flaky and slower scenarios.
    record_history(&history);
    let names: Vec<&str> = selected.iter().map(|s| s.name).collect();
    let health = fs::read_to_string(history_path()).unwrap_or_default();
    for line in history_findings(&health, &names) {
        eprintln!("xtask: health: {line}");
    }
    if failed == 0 {
        Ok(())
    } else {
        Err(format!("{failed} of {} boot tests failed", selected.len()))
    }
}

/// One scenario's outcome for target/report/history.jsonl.
struct HistoryRun {
    scenario: &'static str,
    ok: bool,
    /// The VM's whole time (a shared VM's for each of its scenarios).
    secs: f64,
    shared: bool,
    reason: String,
}

fn history_path() -> PathBuf {
    report_dir().join("history.jsonl")
}

/// Runs kept in the history (older ones are dropped when it grows past
/// twice this).
const HISTORY_KEEP: usize = 20_000;

/// Appends this run's outcomes to target/report/history.jsonl (one JSON
/// line per scenario: time, commit, host load, outcome, duration).
fn record_history(runs: &[HistoryRun]) {
    if runs.is_empty() {
        return;
    }
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let commit = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(root())
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    let load = fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|l| l.split_whitespace().next().map(str::to_string))
        .unwrap_or_else(|| "0".into());
    let mut text = String::new();
    for r in runs {
        let reason: String = r
            .reason
            .chars()
            .take(200)
            .map(|c| match c {
                '"' | '\\' => '\'',
                c if c.is_control() => ' ',
                c => c,
            })
            .collect();
        text += &format!(
            "{{\"t\":{t},\"commit\":\"{commit}\",\"load\":{load},\"scenario\":\"{}\",\"ok\":{},\"secs\":{:.1},\"shared\":{},\"reason\":\"{reason}\"}}\n",
            r.scenario, r.ok, r.secs, r.shared
        );
    }
    let path = history_path();
    let old = fs::read_to_string(&path).unwrap_or_default();
    let mut lines: Vec<&str> = old.lines().collect();
    if lines.len() > 2 * HISTORY_KEEP {
        lines.drain(..lines.len() - HISTORY_KEEP);
    }
    let mut all = lines.join("\n");
    if !all.is_empty() {
        all.push('\n');
    }
    all += &text;
    let _ = fs::create_dir_all(report_dir());
    let _ = fs::write(&path, all);
}

/// Runs of one scenario looked at for flakiness.
const HISTORY_WINDOW: usize = 30;

/// What the history says about `scenarios` (this run's): those that both
/// passed and failed in their last HISTORY_WINDOW runs (flaky, with the
/// last failure's reason), and those whose latest run (alone in its VM)
/// took over 1.5 times, and 10 s more than, the median of their previous
/// ten passes.
fn history_findings(history: &str, scenarios: &[&str]) -> Vec<String> {
    use std::collections::BTreeMap;
    use triage::field;
    // Per scenario: (ok, secs, shared, reason, load).
    type Run<'a> = (bool, f64, bool, &'a str, &'a str);
    let mut runs: BTreeMap<&str, Vec<Run>> = BTreeMap::new();
    for line in history.lines() {
        let Some(name) = field(line, "scenario") else { continue };
        let ok = field(line, "ok") == Some("true");
        let secs = field(line, "secs").and_then(|v| v.parse().ok()).unwrap_or(0.0);
        let shared = field(line, "shared") == Some("true");
        let reason = field(line, "reason").unwrap_or("");
        let load = field(line, "load").unwrap_or("?");
        runs.entry(name).or_default().push((ok, secs, shared, reason, load));
    }
    let mut out = Vec::new();
    for name in scenarios {
        let Some(all) = runs.get(name) else { continue };
        let recent = &all[all.len().saturating_sub(HISTORY_WINDOW)..];
        let failures = recent.iter().filter(|r| !r.0).count();
        if failures > 0 && failures < recent.len() {
            let last = recent.iter().rev().find(|r| !r.0).map_or("", |r| r.3);
            out.push(format!(
                "{name} is flaky: failed {failures} of its last {} runs (last failure: {})",
                recent.len(),
                last.chars().take(120).collect::<String>()
            ));
        }
        let Some((latest, earlier)) = all.split_last() else { continue };
        if latest.2 || !latest.0 {
            continue;
        }
        let mut passes: Vec<f64> =
            earlier.iter().rev().filter(|r| r.0 && !r.2).take(10).map(|r| r.1).collect();
        if passes.len() < 3 {
            continue;
        }
        passes.sort_by(f64::total_cmp);
        let median = passes[passes.len() / 2];
        if latest.1 > 1.5 * median && latest.1 - median > 10.0 {
            out.push(format!(
                "{name} is slower: {:.1}s against a median of {median:.1}s over its last {} \
                 passes (host load {})",
                latest.1,
                passes.len(),
                latest.4
            ));
        }
    }
    out
}

/// `XTASK_REPEAT=<n> cargo xtask boot-test <name>`: boots one scenario's
/// image `n` times, one VM per two host CPUs at once (`XTASK_REPEAT_JOBS`
/// sets how many; more load brings races out, and time limits too). Each boot has its own copy of the image (a hard
/// link) and log; a failed boot keeps its log as
/// `test-<name>.r<i>.serial.log` (its error report is printed), a passed
/// one is deleted. Ends with the count of failures.
fn repeat_boots(vms: &[Vm], isos: &[PathBuf], times: usize) -> Result {
    let ([vm @ Vm::Single(s)], [iso]) = (vms, isos) else {
        return Err("XTASK_REPEAT needs a filter that selects one scenario of its own VM".into());
    };
    let copies: Vec<PathBuf> = (0..times.max(1))
        .map(|i| {
            let copy = iso.with_extension(format!("r{i}.iso"));
            let _ = fs::remove_file(&copy);
            fs::hard_link(iso, &copy).or_else(|_| fs::copy(iso, &copy).map(drop))?;
            Ok(copy)
        })
        .collect::<std::io::Result<_>>()
        .map_err(|e| format!("copying {}: {e}", iso.display()))?;
    let t = Instant::now();
    // One VM per two host CPUs (each has two processors): more only
    // queue, and a JVM then runs into its scenario's time limit.
    let jobs = env::var("XTASK_REPEAT_JOBS")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| thread::available_parallelism().map_or(1, |n| n.get().div_ceil(2)));
    let results = run_parallel_on(jobs, &copies, |copy| {
        let (_, verdicts) = boot_vm(vm, copy);
        let verdict = verdicts.into_iter().next().map_or(Ok(()), |(_, v)| v);
        let _ = fs::remove_file(copy);
        match &verdict {
            Ok(()) => {
                let _ = fs::remove_file(copy.with_extension("serial.log"));
                let _ = fs::remove_file(copy.with_extension("events.jsonl"));
                eprintln!("xtask: repeat {}: ok", copy.display());
            }
            Err(e) => eprintln!(
                "xtask: repeat {}: FAILED: {e} (log {})",
                copy.display(),
                copy.with_extension("serial.log").display()
            ),
        }
        Ok(verdict)
    });
    let failed = results.iter().filter(|r| !matches!(r, Ok(Ok(())))).count();
    eprintln!(
        "xtask: {}: {failed} of {} boots failed ({:.0}s)",
        s.name,
        copies.len(),
        t.elapsed().as_secs_f64()
    );
    let _ = fs::remove_file(iso);
    if failed == 0 { Ok(()) } else { Err(format!("{failed} of {} boots failed", copies.len())) }
}

/// Runs `f` on every job, at most one per host CPU at a time (each job is
/// an emulator that needs a CPU: running them all at once starves them on
/// small CI machines). Results are in job order.
fn run_parallel<J: Sync, T: Send>(
    jobs: &[J],
    f: impl Fn(&J) -> Result<T> + Sync,
) -> Vec<Result<T>> {
    // `XTASK_VMS=<n>`: at most n at a time (a shared, busy host).
    let workers = env::var("XTASK_VMS").ok().and_then(|n| n.parse().ok()).filter(|&n| n > 0);
    run_parallel_on(
        workers.unwrap_or_else(|| thread::available_parallelism().map_or(2, |n| n.get())),
        jobs,
        f,
    )
}

/// `run_parallel` with at most `workers` jobs at a time.
fn run_parallel_on<J: Sync, T: Send>(
    workers: usize,
    jobs: &[J],
    f: impl Fn(&J) -> Result<T> + Sync,
) -> Vec<Result<T>> {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let workers = workers.clamp(1, jobs.len().max(1));
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Option<Result<T>>>> = Mutex::new(jobs.iter().map(|_| None).collect());
    thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(job) = jobs.get(i) else { break };
                    let result = f(job);
                    if let Ok(mut r) = results.lock() {
                        r[i] = Some(result);
                    }
                }
            });
        }
    });
    results
        .into_inner()
        .unwrap_or_default()
        .into_iter()
        .map(|r| r.unwrap_or_else(|| Err("did not run".into())))
        .collect()
}

/// Program scenarios gala-um does not run, and why (docs/GALA_UM.md).
const NOT_IN_UM: &[(&str, &str)] = &[
    ("wow64_test", "32-bit code (WOW64): its segments are the kernel's GDT"),
    ("wow64_thread_test", "32-bit code (WOW64): its segments are the kernel's GDT"),
    ("wow64_import_test", "32-bit code (WOW64): its segments are the kernel's GDT"),
    ("wow64_seh_test", "32-bit code (WOW64): its segments are the kernel's GDT"),
    ("wow64_fs_test", "32-bit code (WOW64): its segments are the kernel's GDT"),
    ("wow64_k32_test", "32-bit code (WOW64): its segments are the kernel's GDT"),
    ("wow64_c_test", "32-bit code (WOW64): its segments are the kernel's GDT"),
    ("wow64_crt_test", "32-bit code (WOW64): its segments are the kernel's GDT"),
    ("wow64_gui_test", "32-bit code (WOW64): its segments are the kernel's GDT"),
    ("wow64_inno", "32-bit code (WOW64): its segments are the kernel's GDT"),
    ("wow64_sock_test", "32-bit code (WOW64): its segments are the kernel's GDT"),
    ("pk_install_inno32", "32-bit code (WOW64): its segments are the kernel's GDT"),
    ("wow64_adv_test", "32-bit code (WOW64): its segments are the kernel's GDT"),
    ("win32_misc_test", "a child process (its 32-bit wow64_test.exe, IsWow64Process2)"),
    ("conpty_test", "child processes: the console host and the child"),
    ("process_test", "child processes (one NT process per Linux process)"),
    ("quota_test", "child processes (one NT process per Linux process)"),
    ("smprace_test", "round 4 starts child processes (one NT process per Linux process)"),
    ("windows_across_test", "child processes (one NT process per Linux process)"),
    ("uipi_test", "a child process (one NT process per Linux process)"),
    ("shell32_test", "a child process (ShellExecuteExW runs it)"),
    ("winevent_test", "a child process (one NT process per Linux process)"),
    ("uia_test", "a child process (one NT process per Linux process)"),
    ("logon_test", "child processes (one NT process per Linux process)"),
    ("system_test", "child processes; run as SYSTEM (a boot option)"),
    ("session_test", "child processes; run as SYSTEM (a boot option)"),
    ("presenter_test", "a child process; run as SYSTEM (a boot option)"),
    ("winsta_test", "child processes (one NT process per Linux process)"),
    ("bench", "measures the platform: gala-um's numbers are the host's"),
    ("go_exec", "child processes (one NT process per Linux process)"),
    ("busybox", "child processes (pipelines are child processes)"),
    ("cmd_test", "child processes (one NT process per Linux process)"),
    ("gui_test", "needs a screen (the desktop draws on the framebuffer)"),
    ("gala_cli", "needs a screen (shasha) and child processes"),
    ("console_window", "needs a screen (the console window is on the desktop)"),
];

/// The kernel flags gala-um applies to a scenario's run (`um_command`),
/// and how. A scenario with any other flag (`start=`, `as=`, `events=`,
/// `wallpaper=`, ...) does not run in gala-um: `um_command` would drop
/// the flag and run a different test (GALA_UM "What runs in gala-um").
const UM_FLAGS: &[(&str, &str)] = &[
    ("qemu_exit", "the run ends with the program"),
    ("run", "the program (um's program argument)"),
    ("args", "its command line"),
    ("clock", "um --clock="),
    ("quota", "um --quota="),
    ("calstall", "stalls the kernel's clock calibration; gala-um has none"),
];

/// The first kernel flag of `s` that gala-um does not apply (UM_FLAGS).
fn um_unapplied_flag(s: &Scenario) -> Option<&'static str> {
    s.cmdline.split_whitespace().find(|w| {
        let key = w.split_once('=').map_or(*w, |(k, _)| k);
        !UM_FLAGS.iter().any(|(f, _)| *f == key)
    })
}

/// Why gala-um does not run scenario `s`, or None if it does: it runs a
/// program's own scenario (its program and expected lines, only the flags
/// of UM_FLAGS), except network scenarios (they need a network card and
/// xtask's client), disk scenarios (no virtio-blk in a Linux process) and
/// NOT_IN_UM.
fn um_gap(s: &Scenario) -> Option<String> {
    if scenario_program(s).is_none() {
        return Some("not a program's own scenario".into());
    }
    if net_marker(s.name).is_some() || HOST_PAGE_SCENARIOS.contains(&s.name) {
        return Some("network scenario (needs a network card and xtask's client)".into());
    }
    if disk_scenario(s.name).is_some() {
        return Some("disk scenario (no virtio-blk in a Linux process)".into());
    }
    if let Some((_, why)) = NOT_IN_UM.iter().find(|(n, _)| *n == s.name) {
        return Some(why.to_string());
    }
    um_unapplied_flag(s)
        .map(|flag| format!("kernel flag {flag} (gala-um does not apply it, UM_FLAGS)"))
}

/// Whether gala-um runs scenario `s` (`um_gap` says why not).
fn runs_in_um(s: &Scenario) -> bool {
    um_gap(s).is_none()
}

/// Expected lines that only a QEMU boot has: the boot marker, the NMI
/// part of a state dump and xtask's own line about it, a user rip from
/// the timer tick.
fn kernel_only(line: &str) -> bool {
    line == BOOT_OK_MARKER
        || ["statedump: begin", "statedump: cpu", "statedump: locks", "statedump: heap"]
            .iter()
            .any(|p| line.starts_with(p))
        || line.starts_with("statedump: NMI")
        || line.starts_with("flight:")
        || line == "in user mode at rip"
        || line.starts_with("the guest's state dump")
        // The machine's clock: calibration, the RTC check, xtask's timing.
        || ["clockcal:", "tsc: ", "clock: ", "xtask: clock"].iter().any(|p| line.starts_with(p))
}

/// Seconds a gala-um program may run (they take milliseconds).
const UM_TIMEOUT_SECS: u32 = 30;

/// `cargo xtask um-test`: builds um/ for the host and runs the program of
/// each scenario that `runs_in_um` in it, checking the QEMU scenario's
/// `expect` lines (except the boot marker).
fn um_test(release: bool) -> Result {
    let um = build_um(release)?;
    let mut failed = Vec::new();
    let all = Instant::now();
    let selected: Vec<&Scenario> = SCENARIOS.iter().filter(|s| runs_in_um(s)).collect();
    for s in &selected {
        let started = Instant::now();
        // A program that hangs on purpose gets SIGQUIT (its state dump)
        // after a few seconds.
        let secs = if s.exit_status == XTASK_DUMPS { 3 } else { UM_TIMEOUT_SECS };
        let (code, mut log) = run_um_with(&um, s, &[], secs)?;
        if matches!(code, Some(124 | 137)) {
            log += &format!(
                "\nxtask: gala-um timed out after {secs}s (SIGQUIT asked for a state dump)\n"
            );
        }
        let missing: Vec<_> =
            s.expect.iter().filter(|e| !kernel_only(e) && !log.contains(**e)).collect();
        if missing.is_empty() {
            eprintln!(
                "xtask: um test {:<20} ok  ({:.1} ms)",
                s.name,
                started.elapsed().as_secs_f64() * 1e3
            );
        } else {
            eprintln!("xtask: um test {:<20} FAILED, missing {missing:?}; output:\n{log}", s.name);
            failed.push(s.name);
        }
    }
    eprintln!(
        "xtask: um-test: {} programs in {:.2} s",
        selected.len(),
        all.elapsed().as_secs_f64()
    );
    // What it left out, each with its reason (um_gap), so that a program
    // missing here is explained rather than silently absent.
    for s in SCENARIOS.iter().filter(|s| scenario_program(s).is_some()) {
        if let Some(why) = um_gap(s) {
            eprintln!("xtask: um-test: not run: {:<20} {why}", s.name);
        }
    }
    if failed.is_empty() { Ok(()) } else { Err(format!("um tests failed: {failed:?}")) }
}

/// Builds um/ for the host; returns the binary.
fn build_um(release: bool) -> Result<PathBuf> {
    let mut build = cargo();
    build.args(["build", "--package", "um"]);
    if release {
        build.arg("--release");
    }
    run_quiet(&mut build)?;
    Ok(target_dir().join(if release { "release" } else { "debug" }).join("um"))
}

/// Runs scenario `s`'s program in gala-um (binary `um`) as its boot test
/// would run: its boot modules from user/bin, its `args=`, two
/// processors; returns stdout and stderr.
fn run_um(um: &Path, s: &Scenario) -> Result<String> {
    run_um_with(um, s, &[], UM_TIMEOUT_SECS).map(|(_, log)| log)
}

/// `run_um` with more options for um (`--failpoint=...`) and a timeout;
/// returns the exit status (None if a signal ended it) and the output.
fn run_um_with(
    um: &Path,
    s: &Scenario,
    options: &[String],
    timeout_secs: u32,
) -> Result<(Option<i32>, String)> {
    let out = um_command(um, s, options, timeout_secs)?
        .output()
        .map_err(|e| format!("running {}: {e}", um.display()))?;
    let log =
        format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    Ok((out.status.code(), log))
}

/// The command `run_um_with` runs (printed to repeat a failure by hand).
fn um_command(um: &Path, s: &Scenario, options: &[String], timeout_secs: u32) -> Result<Command> {
    let name = s.name;
    let program = scenario_program(s).ok_or(format!("{name}: not a program scenario"))?;
    // Never a run without part of its setup (a server `start=` starts, a
    // token `as=` gives): it would fail in a way that says nothing.
    if let Some(flag) = um_unapplied_flag(s) {
        return Err(format!("{name}: gala-um does not apply kernel flag {flag} (UM_FLAGS)"));
    }
    // The scenario's `args=` (percent-encoded), as one command-line string.
    let args = s.cmdline.split_whitespace().find_map(|w| w.strip_prefix("args=")).unwrap_or("");
    // A hung program fails instead of hanging the run (Linux only, as
    // gala-um itself).
    let mut cmd = Command::new("timeout");
    // At the timeout SIGQUIT first: gala-um prints a state dump (every
    // thread's system call and wait, the ledger; kernel/src/statedump.rs),
    // then SIGKILL 5 s later (exit status 137).
    cmd.args(["-s", "QUIT", "-k", "5"]).arg(timeout_secs.to_string());
    // As many processors as the QEMU boot tests (-smp 2).
    cmd.arg(um).arg("--cpus=2").args(options);
    // Clock control, as the kernel's `clock=`.
    if let Some(spec) = s.cmdline.split_whitespace().find_map(|w| w.strip_prefix("clock=")) {
        cmd.arg(format!("--clock={spec}"));
    }
    // Pool and handle limits, as the kernel's `quota=`.
    if let Some(spec) = s.cmdline.split_whitespace().find_map(|w| w.strip_prefix("quota=")) {
        cmd.arg(format!("--quota={spec}"));
    }
    // Its boot modules, from user/bin (checked in) as `--module=`.
    for (file, dir) in extra_modules(name) {
        // The program itself is added by um.
        if file.ends_with(&format!("/{program}.exe")) {
            continue;
        }
        if file.starts_with("target/prebuilt/") {
            // As for the boot tests: fetched (and checked) on first use.
            fetch_prebuilt(file.trim_end_matches('/'))?;
        }
        let (host, file_name) = match file.rsplit_once('/') {
            Some(_) if let Some(dll) = x86_dll(file) => {
                (root().join("user/bin").join(dll.exported_as), dll.file)
            }
            Some((_, n)) => (root().join(file), n),
            None => {
                let dll = USER_DLLS
                    .iter()
                    .find(|d| d.file == file)
                    .ok_or(format!("{name}: no DLL {file}"))?;
                (root().join("user/bin").join(dll.exported_as), file)
            }
        };
        cmd.arg(format!("--module={dir}/{file_name}={}", host.display()));
    }
    cmd.arg(root().join("user/bin").join(format!("{program}.exe"))).arg(percent_decode(args));
    Ok(cmd)
}

/// `cmd` as a shell command line.
fn shell_line(cmd: &Command) -> String {
    let quote = |a: &std::ffi::OsStr| {
        let a = a.to_string_lossy();
        let plain = |c: char| c.is_ascii_alphanumeric() || "-_=/.,:".contains(c);
        if !a.is_empty() && a.chars().all(plain) {
            a.into_owned()
        } else {
            format!("'{}'", a.replace('\'', r"'\''"))
        }
    };
    let words = std::iter::once(cmd.get_program()).chain(cmd.get_args());
    words.map(quote).collect::<Vec<_>>().join(" ")
}

/// Programs `cargo xtask fault-test` runs in gala-um (scenario names;
/// each must `runs_in_um`). Add programs as they pass.
const FAULT_PROGRAMS: &[&str] = &[
    "hello",
    "abi_test",
    "file_test",
    "kernel32_test",
    "thread_test",
    "wait_test",
    "object_test",
    "section_test",
    "vm_test",
    "registry_test",
    "pipe_test",
    "winsock_async_test",
    "iocp_test",
    "ioapc_test",
    "async_test",
    "apc_test",
    "timer_test",
    "sync_test",
    "dir_test",
    "filemap_test",
    "cow_test",
    "guard_test",
    "exception_test",
    "loader_test",
    "kernel32_thread_test",
    "kernel32_file_test",
    "go_hello",
];

/// Programs whose whole heap sweep is part of a plain `fault-test` (each
/// found a bug: hello NtTerminateProcess failing, pipe_test lost pipe
/// completions, exception_test the copy-on-write access violation).
const FAULT_SWEEPS: &[&str] = &["hello", "pipe_test", "exception_test"];

/// Rules each fault-test program runs under (`random` ones once per
/// seed in FAULT_SEEDS).
const FAULT_RULES: &[&str] = &[
    "heap:every:2",
    "heap:every:3",
    "heap:every:7",
    "heap:every:31",
    "heap:every:97",
    "heap:every:499",
    "heap:random:20",
    "frame:every:3",
    "frame:every:61",
    "frame:random:20",
    "net_tx:every:3",
    "heap:random:5,frame:random:5,net_tx:random:100",
];
const FAULT_SEEDS: &[u64] = &[1, 2, 3];
/// A fault-test run may take this long (a hang is a failure).
const FAULT_TIMEOUT_SECS: u32 = 10;

/// What a fault-test run must not show: the NT layer panicked, or an
/// infallible allocation failed (Rust's message for it).
const FAULT_CRASH_MARKERS: &[&str] = &["panicked at", "KERNEL PANIC", "memory allocation of"];

/// Load errors a valid program must not get from an injected failure.
const FAULT_MISREPORTS: &[&str] = &[": Pe(", ": BadImageBase"];

/// Judges one gala-um run with failures injected: the program ends (any
/// exit status: 0 or 1 from the program, 2 when um could not start it),
/// the NT layer neither panics, aborts nor hangs.
fn judge_fault_run(code: Option<i32>, log: &str) -> Result {
    if let Some(m) = FAULT_CRASH_MARKERS.iter().find(|m| log.contains(**m)) {
        return Err(format!("crashed ({m:?})"));
    }
    // The programs are valid images: an injected failure reported as a bad
    // one is a wrong status (hello.exe once failed with BadPeSignature).
    if let Some(m) = FAULT_MISREPORTS.iter().find(|m| log.contains(**m)) {
        return Err(format!("out of memory reported as {m:?}"));
    }
    match code {
        Some(0..=2) => Ok(()),
        Some(124 | 137) => {
            Err(format!("hung (killed after {FAULT_TIMEOUT_SECS} s; its state dump is in the log)"))
        }
        Some(c) if c > 128 => Err(format!("killed by signal {}", c - 128)),
        Some(c) => Err(format!("um exited with {c}")),
        None => Err("killed by a signal".into()),
    }
}

/// `cargo xtask fault-test [program filter] [--sweep[=<point>]]`: runs
/// FAULT_PROGRAMS in gala-um under each of FAULT_RULES
/// (kernel/src/failpoint.rs) and checks that each run ends without a
/// panic, an abort or a hang. With `--sweep`, each program instead runs
/// once per occurrence of the point (`heap` by default, or `frame`...),
/// with only that one failing (`<point>:nth:k` for every k).
fn fault_test(release: bool, filter: Option<&String>, sweep: Option<&str>) -> Result {
    let um = build_um(release)?;
    let programs: Vec<&Scenario> = FAULT_PROGRAMS
        .iter()
        .filter(|p| filter.is_none_or(|f| p.contains(f.as_str())))
        .map(|p| {
            SCENARIOS
                .iter()
                .find(|s| s.name == *p && runs_in_um(s))
                .ok_or(format!("fault test: {p} is not a gala-um scenario"))
        })
        .collect::<Result<_>>()?;
    if programs.is_empty() {
        return Err("no fault-test program matches the filter".into());
    }
    // `point:nth:k` for every occurrence k of `point` in a clean run of `s`.
    let sweep_jobs = |s: &'static Scenario, point: &str| -> Result<Vec<(&Scenario, String, u64)>> {
        // Count the occurrences (a rule that never fires).
        let count = vec![format!("--failpoint={point}:nth:{}", u64::MAX)];
        let (_, log) = run_um_with(&um, s, &count, FAULT_TIMEOUT_SECS)?;
        let n: u64 = log
            .split(&format!("failpoint: {point} "))
            .nth(1)
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|n| n.parse().ok())
            .ok_or(format!("{}: no {point} count in its output:\n{log}", s.name))?;
        eprintln!("xtask: fault test {}: {n} {point} occurrences to fail one by one", s.name);
        Ok((1..=n).map(|k| (s, format!("{point}:nth:{k}"), 0)).collect())
    };
    let mut jobs: Vec<(&Scenario, String, u64)> = Vec::new();
    for &s in &programs {
        if let Some(point) = sweep {
            jobs.extend(sweep_jobs(s, point)?);
            continue;
        }
        for rule in FAULT_RULES {
            if rule.contains("random") {
                jobs.extend(FAULT_SEEDS.iter().map(|&seed| (s, rule.to_string(), seed)));
            } else {
                jobs.push((s, rule.to_string(), 0));
            }
        }
        // The sweeps that found bugs, kept as their regression tests.
        if FAULT_SWEEPS.contains(&s.name) {
            jobs.extend(sweep_jobs(s, "heap")?);
        }
    }
    let started = Instant::now();
    let results = run_parallel(&jobs, |(s, rule, seed)| {
        let options = [format!("--failpoint={rule}"), format!("--failseed={seed}")];
        let (code, log) = run_um_with(&um, s, &options, FAULT_TIMEOUT_SECS)?;
        Ok(judge_fault_run(code, &log).map_err(|e| (e, log)))
    });
    let mut failed = 0;
    let dir = target_dir().join("failed");
    for ((s, rule, seed), result) in jobs.iter().zip(results) {
        let verdict = result.and_then(|r| {
            r.map_err(|(e, log)| {
                let file =
                    dir.join(format!("fault-{}-{}-{seed}.log", s.name, rule.replace(':', "_")));
                let _ = fs::create_dir_all(&dir).and_then(|()| fs::write(&file, &log));
                let tail: Vec<&str> = log.lines().rev().take(12).collect();
                let tail: Vec<&str> = tail.into_iter().rev().collect();
                format!("{e}; log {}:\n    {}", file.display(), tail.join("\n    "))
            })
        });
        if let Err(e) = verdict {
            failed += 1;
            eprintln!("xtask: fault test {:<16} {rule} seed {seed}: FAILED: {e}", s.name);
        }
    }
    eprintln!(
        "xtask: fault-test: {} runs of {} programs in {:.1} s, {failed} failed",
        jobs.len(),
        programs.len(),
        started.elapsed().as_secs_f64()
    );
    if failed == 0 { Ok(()) } else { Err(format!("{failed} fault-test runs failed")) }
}

/// Programs `cargo xtask replay-test` records and replays in gala-um
/// (scenario names; each must `runs_in_um`), and the prefix of the lines
/// that show their inputs (they must repeat in the replay).
const REPLAY_PROGRAMS: &[(&str, &str)] = &[("replay_test", "replay_test: ")];

/// `cargo xtask replay-test`: runs each REPLAY_PROGRAMS program in gala-um
/// with `--record=<file>` twice (live inputs: the lines must differ),
/// then with `--replay=<file>` (the settings and inputs from the file:
/// the lines must be the recorded ones), without and with `--sim` (whose
/// trace must repeat too).
fn replay_test(release: bool) -> Result {
    let um = build_um(release)?;
    let started = Instant::now();
    let mut failed = Vec::new();
    for (program, prefix) in REPLAY_PROGRAMS {
        let s = SCENARIOS
            .iter()
            .find(|s| s.name == *program && runs_in_um(s))
            .ok_or(format!("replay-test: {program} is not a gala-um scenario"))?;
        for sim in [None, Some(3u64)] {
            let label = sim.map_or(String::from("live"), |seed| format!("sim {seed}"));
            let file = |run: u32| target_dir().join(format!("replay-{program}-{label}-{run}.rec"));
            let wanted = |log: &str| -> Vec<String> {
                log.lines()
                    .filter(|l| l.starts_with(prefix) || l.starts_with("sim: seed"))
                    .map(str::to_string)
                    .collect()
            };
            let mut recorded = Vec::new();
            for run in 0..2 {
                let mut options = vec![format!("--record={}", file(run).display())];
                options.extend(sim.map(|seed| format!("--sim={seed}")));
                let (code, log) = run_um_with(&um, s, &options, UM_TIMEOUT_SECS)?;
                if code != Some(0) || !log.contains("replay: recorded") {
                    failed.push(format!("{program} ({label}): recording failed:\n{log}"));
                }
                recorded.push(wanted(&log));
            }
            // Live inputs differ between runs (the simulation's seed fixes
            // them, so its two recordings agree).
            if sim.is_none() && recorded[0] == recorded[1] {
                failed.push(format!("{program} ({label}): two live runs read the same inputs"));
            }
            if sim.is_some() && recorded[0] != recorded[1] {
                failed.push(format!("{program} ({label}): one seed, different runs"));
            }
            let replay = format!("--replay={}", file(0).display());
            let (code, log) = run_um_with(&um, s, &[replay], UM_TIMEOUT_SECS)?;
            let replayed = wanted(&log);
            if code != Some(0) || replayed != recorded[0] || !log.contains("replay: replayed") {
                failed.push(format!(
                    "{program} ({label}): the replay read {replayed:?}, recorded {:?}:\n{log}",
                    recorded[0]
                ));
            } else {
                eprintln!(
                    "xtask: replay test {program:<16} {label:<6} ok ({} lines)",
                    replayed.len()
                );
            }
            for run in 0..2 {
                let _ = fs::remove_file(file(run));
            }
        }
    }
    eprintln!("xtask: replay-test: {:.2} s", started.elapsed().as_secs_f64());
    if failed.is_empty() {
        Ok(())
    } else {
        for f in &failed {
            eprintln!("xtask: replay test FAILED: {f}");
        }
        Err(format!("{} replay tests failed", failed.len()))
    }
}

/// Programs `cargo xtask sim-test` runs in gala-um's deterministic
/// simulation, and whether they are multi-threaded (their traces must
/// then differ across seeds). Each must `runs_in_um`.
const SIM_PROGRAMS: &[(&str, bool)] = &[
    ("hello", false),
    ("lock_test", true),
    ("kernel32_sync_test", true),
    ("pipe_test", true),
    ("thread_test", true),
    ("sync_test", true),
    ("keyed_event_test", true),
    ("kernel32_thread_test", true),
    ("smpsched_test", true),
    ("smpmem_test", true),
    // Few interleavings (the second thread makes its few system calls
    // while the first mostly waits): a few seeds may all give one trace.
    ("iocp_test", false),
    ("apc_test", false),
    // System calls from one thread only (the NT layer's helper threads,
    // which the seed also schedules, make none).
    ("wait_test", false),
    ("timer_test", false),
    // Clock control on the simulation's virtual clock (frozen, advanced,
    // 1000 times faster) with three waiting threads.
    ("clock_test", false),
    ("ioapc_test", false),
    ("smp_test", false),
    // Go: the runtime's own threads (sysmon, the garbage collector's
    // workers) make system calls of their own. Its random numbers
    // (ProcessPrng) come from the seed through GalaRandom (R7); its
    // spin-waits read the time from KUSER_SHARED_DATA, which the
    // simulation traps, and the garbage collector's spin loops are
    // preempted at a canonical point (ARCHITECTURE "Deterministic
    // simulation").
    ("go_hello", true),
    ("go_goroutines", true),
    ("go_time", true),
    ("go_gc", true),
    ("go_os", true),
    ("go_exit", true),
];
/// Seeds per program by default (1..=N), each run twice.
const SIM_SEEDS: u64 = 4;
/// A simulated run may take this long (a hang is a failure).
const SIM_TIMEOUT_SECS: u32 = 60;
/// um's exit status for a deadlock found by the simulation.
const SIM_DEADLOCK_EXIT: i32 = 4;

/// The trace hash of a simulated run's `sim: seed S trace H` line.
fn sim_trace(log: &str) -> Option<&str> {
    log.lines()
        .find_map(|l| l.strip_prefix("sim: seed ")?.split(" trace ").nth(1)?.split(' ').next())
}

/// `cargo xtask sim-test [filter] [--seeds N] [--seed S] [--all]`: runs
/// SIM_PROGRAMS (with `--all`, every gala-um program) under gala-um's deterministic simulation with seeds
/// 1..=N (or only S), each seed twice, and checks (a) the scenario's
/// expected output, (b) that both runs of a seed have the same trace
/// hash, (c) that a multi-threaded program's traces differ across seeds.
/// A deadlock (um exits with 4) is reported with its seed and the
/// threads' waits; `um --sim=<seed>` repeats it.
fn sim_test(
    release: bool,
    filter: Option<&String>,
    seeds: u64,
    only: Option<u64>,
    all: bool,
) -> Result {
    let um = build_um(release)?;
    let mut programs: Vec<(&Scenario, bool)> = SIM_PROGRAMS
        .iter()
        .map(|&(p, threaded)| {
            SCENARIOS
                .iter()
                .find(|s| s.name == p && runs_in_um(s))
                .map(|s| (s, threaded))
                .ok_or(format!("sim test: {p} is not a gala-um scenario"))
        })
        .collect::<Result<_>>()?;
    if all {
        // Every gala-um program (traces need not differ across seeds).
        let more = SCENARIOS.iter().filter(|s| runs_in_um(s));
        let more: Vec<_> =
            more.filter(|s| !programs.iter().any(|(p, _)| p.name == s.name)).collect();
        programs.extend(more.into_iter().map(|s| (s, false)));
    }
    programs.retain(|(s, _)| filter.is_none_or(|f| s.name.contains(f.as_str())));
    if programs.is_empty() {
        return Err("no sim-test program matches the filter".into());
    }
    let seed_list: Vec<u64> = match only {
        Some(s) => vec![s],
        None => (1..=seeds.max(1)).collect(),
    };
    let jobs: Vec<(&Scenario, u64, u32)> = programs
        .iter()
        .flat_map(|&(s, _)| seed_list.iter().flat_map(move |&seed| [(s, seed, 0), (s, seed, 1)]))
        .collect();
    let started = Instant::now();
    let results = run_parallel(&jobs, |(s, seed, _)| {
        run_um_with(&um, s, &[format!("--sim={seed}")], SIM_TIMEOUT_SECS)
    });
    let dir = target_dir().join("failed");
    let mut failures = Vec::new();
    // (program, seed) -> (trace hash, its `sim:` line, log file) of its runs.
    type Runs = Vec<(String, String, PathBuf)>;
    let mut traces: std::collections::BTreeMap<(&str, u64), Runs> = Default::default();
    let mut warnings = Vec::new();
    for ((s, seed, run), result) in jobs.iter().zip(results) {
        let (code, log) = result?;
        let file = dir.join(format!("sim-{}-{seed}-{run}.log", s.name));
        let mut fail = |why: String| {
            let _ = fs::create_dir_all(&dir).and_then(|()| fs::write(&file, &log));
            let tail: Vec<&str> = log.lines().rev().take(30).collect();
            let tail: Vec<&str> = tail.into_iter().rev().collect();
            let repeat = um_command(&um, s, &[format!("--sim={seed}")], SIM_TIMEOUT_SECS)
                .map_or_else(|e| e, |c| shell_line(&c));
            failures.push(format!(
                "{} seed {seed}: {why}; log {}; repeat with:\n    {repeat}\n    {}",
                s.name,
                file.display(),
                tail.join("\n    ")
            ));
        };
        if code == Some(SIM_DEADLOCK_EXIT) {
            fail("DEADLOCK".into());
            continue;
        }
        let missing: Vec<_> =
            s.expect.iter().filter(|e| **e != BOOT_OK_MARKER && !log.contains(**e)).collect();
        if !missing.is_empty() {
            fail(format!("exit {code:?}, missing {missing:?}"));
            continue;
        }
        match sim_trace(&log) {
            Some(h) => {
                let line = log.lines().find(|l| l.starts_with("sim: seed ")).unwrap_or("");
                let _ = fs::create_dir_all(&dir).and_then(|()| fs::write(&file, &log));
                let entry = (h.to_string(), line.to_string(), file.clone());
                traces.entry((s.name, *seed)).or_default().push(entry);
            }
            None => fail("no `sim: seed S trace H` line".into()),
        }
    }
    for &(s, threaded) in &programs {
        let mut distinct = std::collections::BTreeSet::new();
        for &seed in &seed_list {
            let Some(runs) = traces.get(&(s.name, seed)) else { continue };
            if let [(a, line_a, log_a), (b, line_b, log_b)] = &runs[..]
                && a != b
            {
                let what = format!(
                    "{} seed {seed}: two runs, two traces:\n    {line_a} ({})\n    {line_b} ({})",
                    s.name,
                    log_a.display(),
                    log_b.display()
                );
                // The watchdog preempted a thread spinning in program code
                // (it depends on the host's timing): a limit of the
                // simulation, tolerated for `--all`'s other programs. The
                // SIM_PROGRAMS must repeat.
                if (line_a.contains("watchdog") || line_b.contains("watchdog"))
                    && !SIM_PROGRAMS.iter().any(|(p, _)| *p == s.name)
                {
                    warnings.push(what);
                } else {
                    failures.push(format!("{what}\n    not deterministic"));
                }
            }
            distinct.extend(runs.iter().map(|r| r.0.clone()));
        }
        eprintln!(
            "xtask: sim test {:<20} {} seeds, {} distinct traces",
            s.name,
            seed_list.len(),
            distinct.len()
        );
        if threaded && seed_list.len() > 1 && distinct.len() == 1 {
            failures.push(format!(
                "{}: the same trace under every seed: the seed does not change the interleaving",
                s.name
            ));
        }
    }
    for w in &warnings {
        eprintln!("xtask: sim test: not reproducible (watchdog preemptions): {w}");
    }
    for f in &failures {
        eprintln!("xtask: sim test FAILED: {f}");
    }
    eprintln!(
        "xtask: sim-test: {} runs of {} programs in {:.1} s, {} failures",
        jobs.len(),
        programs.len(),
        started.elapsed().as_secs_f64(),
        failures.len()
    );
    if failures.is_empty() { Ok(()) } else { Err(format!("{} sim-test failures", failures.len())) }
}

/// Decodes `%XX` escapes (the kernel's `args=` encoding).
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let hex = b.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok());
        match (b[i], hex.and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(v)) => {
                out.push(v);
                i += 3;
            }
            (c, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The program a scenario runs, if it is that program's own scenario.
fn scenario_program(scenario: &Scenario) -> Option<&'static str> {
    let program = scenario.cmdline.split_whitespace().find_map(|w| w.strip_prefix("run="))?;
    let program = program.strip_suffix(".exe")?;
    (program == scenario.name).then_some(program)
}

fn report_dir() -> PathBuf {
    target_dir().join("report")
}

/// Runs every test program's scenario and records each outcome.
fn gala_report(release: bool) -> Result {
    build_kernel(release)?;
    let selected: Vec<(&Scenario, &str)> = SCENARIOS
        .iter()
        .filter(|s| s.exit_status != XTASK_DUMPS)
        .filter_map(|s| scenario_program(s).map(|p| (s, p)))
        .collect();
    let mut isos = Vec::new();
    for (scenario, _) in &selected {
        isos.push(make_iso(
            release,
            &format!("report-{}", scenario.name),
            scenario.cmdline,
            &extra_modules(scenario.name),
        )?);
    }
    // The verdicts do not matter here: outcomes are read from the logs.
    let jobs: Vec<(&Scenario, &PathBuf)> = selected.iter().map(|(s, _)| *s).zip(&isos).collect();
    let _ = run_parallel(&jobs, |(scenario, iso)| boot_scenario_quiet(scenario, iso));
    let mut results = Vec::new();
    let mut ledger = Vec::new();
    for ((_, program), iso) in selected.iter().zip(&isos) {
        let serial = read_log(&iso.with_extension("serial.log"));
        results.push(report::outcome_from_serial(program, &serial));
        report::merge_ledger(&mut ledger, program, &serial);
    }
    let dir = report_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("creating {dir:?}: {e}"))?;
    write(&dir.join("ledger.md"), &report::ledger_markdown(&ledger))?;
    eprintln!(
        "xtask: wrote {} ({} missing features)",
        dir.join("ledger.md").display(),
        ledger.len()
    );
    // The same programs in gala-um: a different outcome is a bug in one
    // of the two platforms.
    let um = build_um(release)?;
    let mut um_results = Vec::new();
    for (scenario, program) in &selected {
        um_results.push(if runs_in_um(scenario) {
            report::outcome_from_serial(program, &run_um(&um, scenario)?)
        } else {
            report::Outcome {
                program: program.to_string(),
                skipped: um_gap(scenario),
                ..Default::default()
            }
        });
    }
    write(&dir.join("gala-um.json"), &report::to_json("gala-um", &um_results))?;
    write(&dir.join("gala-um.md"), &report::markdown("gala-um", &um_results))?;
    write(&dir.join("gala.json"), &report::to_json("gala", &results))?;
    let mut table = report::markdown("gala", &results);
    table += "\n## gala (QEMU) and gala-um\n\n";
    let mut different = 0;
    for (name, q, u, verdict) in report::compare(&results, &um_results) {
        if verdict == report::Verdict::Different {
            different += 1;
            let exit = |o: Option<report::Outcome>| o.and_then(|o| o.exit).unwrap_or(0);
            table += &format!(
                "- DIFFERENT: {name}: QEMU {:#010x}, gala-um {:#010x}\n",
                exit(q),
                exit(u)
            );
        }
    }
    if different == 0 {
        table += "Every program that ran on both ended the same.\n";
    }
    write(&dir.join("gala.md"), &table)?;
    print!("{table}");
    eprintln!(
        "xtask: wrote {} and gala-um.json ({different} programs differ between QEMU and gala-um)",
        dir.join("gala.json").display()
    );
    Ok(())
}

/// Prints how each program fared on Windows and on gala; fails if any
/// program that ran on both ended differently.
fn compare_reports(windows: &Path, gala: Option<&Path>) -> Result {
    let read = |p: &Path| -> Result<(String, Vec<report::Outcome>)> {
        let text = fs::read_to_string(p).map_err(|e| format!("reading {p:?}: {e}"))?;
        report::from_json(&text).map_err(|e| format!("{p:?}: {e}"))
    };
    let gala_path = gala.map_or_else(|| report_dir().join("gala.json"), Path::to_path_buf);
    let (_, w) = read(windows)?;
    let (_, g) = read(&gala_path)?;
    let show = |o: &Option<report::Outcome>| match o {
        Some(report::Outcome { exit: Some(e), .. }) => format!("{e:#010x}"),
        Some(report::Outcome { skipped: Some(s), .. }) => format!("skipped ({s})"),
        _ => "-".to_string(),
    };
    // How long each ran (ms), and how many times Windows' time gala took.
    let ms = |o: &Option<report::Outcome>| o.as_ref().and_then(|o| o.ms);
    let time = |t: Option<u64>| t.map_or_else(|| "-".to_string(), |t| t.to_string());
    println!("| program | windows | gala | | windows ms | gala ms | gala/windows |");
    println!("|---|---|---|---|---|---|---|");
    let mut different = 0;
    let (mut win_total, mut gala_total) = (0u64, 0u64);
    for (name, win, gal, verdict) in report::compare(&w, &g) {
        let mark = match verdict {
            report::Verdict::Same => "same",
            report::Verdict::Different => {
                different += 1;
                "DIFFERENT"
            }
            report::Verdict::Missing => "missing",
        };
        let ratio = match (ms(&win), ms(&gal)) {
            (Some(a), Some(b)) => {
                win_total += a;
                gala_total += b;
                format!("{:.1}", b as f64 / a.max(1) as f64)
            }
            _ => "-".to_string(),
        };
        println!(
            "| {name} | {} | {} | {mark} | {} | {} | {ratio} |",
            show(&win),
            show(&gal),
            time(ms(&win)),
            time(ms(&gal))
        );
    }
    if win_total > 0 {
        println!(
            "\ntimes of the programs timed on both: windows {win_total} ms, gala {gala_total} ms \
             ({:.1}x)",
            gala_total as f64 / win_total as f64
        );
    }
    if different == 0 { Ok(()) } else { Err(format!("{different} programs differ")) }
}

/// `boot_scenario` without printing logs (for reports).
/// How long a boot took: until the kernel's banner appeared on the serial
/// console (firmware and bootloader), and in total.
#[derive(Clone, Copy, Default)]
struct BootTime {
    to_banner: Duration,
    total: Duration,
}

/// The kernel's first serial line (see `kernel_main`).
const KERNEL_BANNER: &str = "gala-os kernel";

/// What QEMU prints (its standard error) when it cannot set up a host
/// port forward (the port was taken by another process meanwhile).
const QEMU_FORWARD_FAILED: &str = "host forwarding rule";

/// Boots `iso` (with `boot` firmware) until QEMU exits or `timeout`;
/// returns the exit status (None on timeout), the serial log and timing.
/// A net scenario whose QEMU could not forward its host port (another
/// process took it between the lease and QEMU's bind) boots again with
/// another port, up to three times.
fn run_vm(
    iso: &Path,
    boot: Boot,
    timeout: Duration,
    net: Option<(&'static str, NetClient)>,
    stop_after_client: bool,
    disk: Option<&Path>,
    screen: Option<&'static ScreenCheck>,
) -> Result<(Option<std::process::ExitStatus>, String, BootTime)> {
    let mut tries = 0;
    loop {
        let run = run_vm_once(iso, boot, timeout, net, stop_after_client, disk, screen)?;
        let stderr = fs::read_to_string(iso.with_extension("qemu.stderr")).unwrap_or_default();
        tries += 1;
        if net.is_none() || tries == 3 || !stderr.contains(QEMU_FORWARD_FAILED) {
            return Ok(run);
        }
        eprintln!(
            "xtask: {}: QEMU could not forward its host port ({}); booting again with another",
            iso.display(),
            stderr.trim()
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn run_vm_once(
    iso: &Path,
    boot: Boot,
    timeout: Duration,
    net: Option<(&'static str, NetClient)>,
    stop_after_client: bool,
    disk: Option<&Path>,
    screen: Option<&'static ScreenCheck>,
) -> Result<(Option<std::process::ExitStatus>, String, BootTime)> {
    let log = iso.with_extension("serial.log");
    let _ = fs::remove_file(&log);
    let events = iso.with_extension("events.jsonl");
    let _ = fs::remove_file(&events);
    let mut cmd = qemu(iso, boot)?;
    // The monitor: screendumps and input for screen checks, and the NMI
    // that asks for a state dump when the boot times out. A short path
    // (a Unix socket's is limited to 107 bytes; worktrees are deep).
    let socket = monitor_socket();
    let _ = fs::remove_file(&socket);
    let monitor = format!("unix:{},server,nowait", socket.display());
    cmd.args(["-display", "none", "-monitor", &monitor])
        .arg("-serial")
        .arg(format!("file:{}", log.display()))
        // The kernel's event stream (`events=` on its command line).
        .arg("-debugcon")
        .arg(format!("file:{}", events.display()))
        .args(["-device", "isa-debug-exit,iobase=0xf4,iosize=0x04"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        // QEMU's own complaints (a port it cannot forward, a bad option):
        // in the failed log's telemetry.
        .stderr(
            fs::File::create(iso.with_extension("qemu.stderr"))
                .map_or_else(|_| Stdio::null(), Stdio::from),
        );
    // Early-boot telemetry (telemetry.rs): QEMU's resets and guest errors,
    // SeaBIOS's log, the kernel's phase markers, QMP for the vCPU count.
    let qemu_log = iso.with_extension("qemu.log");
    let bios_log = iso.with_extension("bios.log");
    let phases_log = iso.with_extension("phases.log");
    for f in [&qemu_log, &bios_log, &phases_log] {
        let _ = fs::remove_file(f);
    }
    let qmp = telemetry::qmp_socket();
    let _ = fs::remove_file(&qmp);
    // `XTASK_QEMU_LOG=int,cpu_reset`: more of QEMU's log (`-d int` is
    // every exception with the registers: large).
    let what = env::var("XTASK_QEMU_LOG").unwrap_or_else(|_| "cpu_reset,guest_errors".into());
    cmd.args(["-d", &what, "-D"])
        .arg(&qemu_log)
        .arg("-chardev")
        .arg(format!("file,id=gala_bios,path={}", bios_log.display()))
        .args([
            "-device",
            &format!("isa-debugcon,iobase={},chardev=gala_bios", telemetry::BIOS_PORT),
        ])
        .arg("-chardev")
        .arg(format!("file,id=gala_phases,path={}", phases_log.display()))
        .args([
            "-device",
            &format!("isa-debugcon,iobase={},chardev=gala_phases", telemetry::PHASE_PORT),
        ])
        .arg("-qmp")
        .arg(format!("unix:{},server=on,wait=off", qmp.display()));
    let host_before = telemetry::host_state();
    // Held until this boot (and its client) is over: no other VM of this
    // xtask gets the port meanwhile.
    let lease = if net.is_some() { Some(PortLease::new()?) } else { None };
    let port = lease.as_ref().map(|l| l.0);
    let (marker, kind) = net.unwrap_or(("", NetClient::Echo));
    if let Some(port) = port {
        if HOST_PAGE_SCENARIOS.contains(&iso_scenario(iso)) {
            // Both: the forwarded port and the host's pages (chromium_wpt).
            let _ = fs::remove_file(iso.with_extension("serve.log"));
            let guestfwd = host_page_fwd(&mut cmd, &iso.with_extension("serve.log"))?;
            add_net_with(&mut cmd, port, guest_port(iso_scenario(iso)), &guestfwd);
        } else {
            add_net_to(&mut cmd, port, guest_port(iso_scenario(iso)));
        }
    } else if HOST_PAGE_SCENARIOS.contains(&iso_scenario(iso)) {
        let _ = fs::remove_file(iso.with_extension("serve.log"));
        add_host_page_net(&mut cmd, &iso.with_extension("serve.log"))?;
    }
    if let Some(disk) = disk {
        add_disk(&mut cmd, disk);
    }
    if GPU_SCENARIOS.contains(&iso_scenario(iso)) {
        add_gpu(&mut cmd);
    }
    let mut child = cmd.spawn().map_err(|e| format!("failed to spawn QEMU: {e}"))?;
    let qmp_thread = {
        let qmp = qmp.clone();
        thread::spawn(move || telemetry::qmp_cpu_count(&qmp))
    };
    let start = Instant::now();
    let mut to_banner = None;
    let mut client = None;
    let mut stopped = false;
    let mut screen_verdict: Option<String> = None;
    let mut dump_note = None;
    // The guest's `uptime_ms=` marks against the host's time, timed by a
    // thread of their own (clockcheck.rs).
    let clock_marks = clockcheck::Watcher::start(&log, start);
    // The limit counts the VM's run time (vmtime.rs).
    let mut vm_clock = vmtime::VmClock::new(child.id(), start);
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break Some(status);
        }
        if let Some(check) = screen.filter(|_| screen_verdict.is_none())
            && fs::read(&log)
                .is_ok_and(|b| b.windows(check.marker.len()).any(|w| w == check.marker.as_bytes()))
        {
            let failed = check_screen(&socket, iso, check).err();
            screen_verdict = Some(match &failed {
                None => "xtask: screen ok\n".into(),
                Some(e) => format!("xtask: screen: {e}\n"),
            });
            // A failed screen check ends the run (it cannot pass any more):
            // the guest's state dump first, as at a timeout.
            if let Some(e) = failed {
                let why = format!("stopped: the screen check failed ({e})");
                dump_note =
                    Some(ask_for_state_dump(&socket, &log, &mut child, &mut vm_clock, &why));
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
        if let Some(port) = port.filter(|_| client.is_none())
            && fs::read(&log).is_ok_and(|b| b.windows(marker.len()).any(|w| w == marker.as_bytes()))
        {
            let serve_log = iso.with_extension("serve.log");
            let monitor = socket.clone();
            client = Some(thread::spawn(move || match kind {
                NetClient::Echo => net_client(port),
                NetClient::Http(page) => http_client(port, page),
                NetClient::Nginx => nginx_client(port),
                NetClient::Tls => tls_client(port),
                NetClient::Contains(parts) => contains_client(port, parts),
                NetClient::Ssh => ssh_client(port, false),
                NetClient::SshPassword => ssh_client(port, true),
                NetClient::SshServer => ssh_server_client(port),
                NetClient::SshShell => ssh_shell_client(port),
                NetClient::Sftp => sftp_client(port),
                NetClient::Scp => scp_client(port),
                NetClient::SshForward => ssh_forward_client(port),
                NetClient::SshInner => ssh_inner_client(port),
                NetClient::Psql => psql_client(port),
                NetClient::Ftp => ftp_client(port),
                NetClient::WebDriver => webdriver_client(port),
                NetClient::Wpt => wpt::client(port, &serve_log, &monitor),
                NetClient::HandoverNet => handover_net_client(port),
                NetClient::Udp => udp_client(port),
            }));
        }
        // A scenario that ends in the bootloader (R8: Limine refusing a
        // file) is stopped once Limine has said why.
        if stop_after_client
            && port.is_none()
            && fs::read(&log).is_ok_and(|b| {
                let text = plain_text(&String::from_utf8_lossy(&b));
                LIMINE_HALTED.iter().any(|m| text.contains(m))
            })
        {
            let _ = child.kill();
            let _ = child.wait();
            stopped = true;
            break None;
        }
        if stop_after_client && client.as_ref().is_some_and(|c| c.is_finished()) {
            // Let the server log the request, then stop it.
            thread::sleep(Duration::from_millis(500));
            let _ = child.kill();
            let _ = child.wait();
            stopped = true;
            break None;
        }
        if to_banner.is_none()
            && fs::read(&log).is_ok_and(|b| {
                b.windows(KERNEL_BANNER.len()).any(|w| w == KERNEL_BANNER.as_bytes())
            })
        {
            to_banner = Some(start.elapsed());
        }
        let now = vm_clock.sample();
        // A VM that ended while this loop waited (a screen check, which
        // can take minutes under load) ended within its time.
        if now.over(timeout).is_some()
            && let Some(status) = child.try_wait().map_err(|e| e.to_string())?
        {
            break Some(status);
        }
        if let Some(over) = now.over(timeout) {
            // Before killing it: ask the guest what it is doing.
            let what = match over {
                vmtime::Over::RunTime => format!("{}s of run time", timeout.as_secs()),
                vmtime::Over::Backstop => format!(
                    "{}s of wall time (the ceiling: {}x the limit of {}s of run time)",
                    timeout.as_secs() * u64::from(vmtime::BACKSTOP),
                    vmtime::BACKSTOP,
                    timeout.as_secs()
                ),
            };
            let why = format!("timed out after {what}; {}", now.describe());
            dump_note = Some(ask_for_state_dump(&socket, &log, &mut child, &mut vm_clock, &why));
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        thread::sleep(Duration::from_millis(50));
    };
    let _ = fs::remove_file(&socket);
    let total = start.elapsed();
    let vm_time = vm_clock.sample();
    let mut serial = read_log(&log);
    if let Some(note) = dump_note {
        serial += &note;
    }
    if let Some(line) = clock_marks.finish() {
        serial += &line;
    }
    serial += &format!(
        "xtask: vm time: {} (limit {}s of run time)\n",
        vm_time.describe(),
        timeout.as_secs()
    );
    // A `record` boot's recording (kernel/src/replay.rs), from the debug
    // console's `"ev":"rec"` lines.
    if let Some(line) = extract_recording(&events, &iso.with_extension("rec")) {
        serial += &line;
    }
    if stopped {
        if port.is_none() {
            // Limine's own output (boot-chain scenarios) as plain text.
            serial = plain_text(&serial);
        }
        serial += STOPPED_LINE;
        serial += "\n";
    }
    if let Some(verdict) = screen_verdict {
        serial += &verdict;
    }
    // The host page's requests (`serve-page`), as they came.
    if let Ok(served) = fs::read_to_string(iso.with_extension("serve.log")) {
        serial += &served;
    }
    let vcpus = qmp_thread.join().ok().flatten();
    let _ = fs::remove_file(&qmp);
    serial += &boot_telemetry(BootFacts {
        iso,
        status,
        stopped,
        vcpus,
        host_before: &host_before,
        serial: &serial,
        logs: [&qemu_log, &bios_log, &phases_log],
    });
    if let Some(port) = port {
        // Which forward this VM had (VMs at once each have their own).
        serial += &format!(
            "xtask: net: host 127.0.0.1:{port} forwarded to the guest's port {} (TCP and UDP)\n",
            guest_port(iso_scenario(iso))
        );
    }
    if let Some(client) = client {
        let verdict = client.join().unwrap_or_else(|_| Err("client panicked".into()));
        serial += &match verdict {
            Ok(()) => format!(
                "xtask: net client: {} ok\n",
                match kind {
                    NetClient::Echo => "echo",
                    NetClient::Tls => "tls",
                    NetClient::Ssh | NetClient::SshPassword => "ssh",
                    NetClient::SshServer => "ssh server",
                    NetClient::SshShell => "ssh shell",
                    NetClient::Sftp => "sftp",
                    NetClient::Scp => "scp",
                    NetClient::SshForward => "ssh forward",
                    NetClient::SshInner => "ssh client",
                    NetClient::Psql => "psql",
                    NetClient::Ftp => "ftp",
                    NetClient::HandoverNet => "handover",
                    NetClient::Udp => "udp",
                    NetClient::WebDriver => "webdriver",
                    NetClient::Wpt => "wpt",
                    _ => "http",
                }
            ),
            Err(e) => format!("xtask: net client: {e}\n"),
        };
    }
    Ok((status, serial, BootTime { to_banner: to_banner.unwrap_or(total), total }))
}

/// What `run_vm` knows about a finished boot, for `boot_telemetry`.
struct BootFacts<'a> {
    iso: &'a Path,
    status: Option<std::process::ExitStatus>,
    stopped: bool,
    vcpus: Option<usize>,
    host_before: &'a str,
    serial: &'a str,
    /// QEMU's log, SeaBIOS's, the kernel's phase markers.
    logs: [&'a Path; 3],
}

/// The `xtask:` lines appended to a boot's serial log (telemetry.rs): how
/// QEMU ended, its vCPUs, the host; after an abnormal end (not the
/// kernel's own exit code, not stopped by xtask) also the last phase
/// markers and the tail of QEMU's log, so a silent death has a story.
fn boot_telemetry(f: BootFacts) -> String {
    let want = telemetry::vm_cpus(f.iso);
    let mut out = format!(
        "xtask: QEMU ended: {}\nxtask: vCPUs: {} of {want} asked\nxtask: host at start: {}\nxtask: host at end: {}\n",
        telemetry::exit_reason(f.status, f.stopped),
        f.vcpus.map_or("? (QMP did not answer)".to_string(), |n| n.to_string()),
        f.host_before,
        telemetry::host_state()
    );
    if let Some(n) = telemetry::cpus_online(f.serial).filter(|&n| n < want) {
        out += &format!("xtask: {}\n", fewer_cpus(n, want));
    }
    let code = f.status.and_then(|s| s.code());
    let normal = f.stopped || matches!(code, Some(QEMU_EXIT_SUCCESS | QEMU_EXIT_FAILURE));
    if !normal {
        let [qemu_log, _, phases] = f.logs;
        let last = telemetry::tail(phases, 12);
        out += &format!(
            "xtask: last boot phases ({}):\n{}\n",
            phases.display(),
            if last.is_empty() { "(none)".to_string() } else { last }
        );
        out += &format!(
            "xtask: QEMU log tail ({}):\n{}\n",
            qemu_log.display(),
            telemetry::tail(qemu_log, 30)
        );
    }
    let stderr = f.iso.with_extension("qemu.stderr");
    let complaints = telemetry::tail(&stderr, 10);
    if !complaints.trim().is_empty() {
        out += &format!("xtask: QEMU's standard error ({}):\n{complaints}\n", stderr.display());
    }
    out
}

/// The failure reason for a VM that got fewer CPUs than it asked for.
fn fewer_cpus(got: u32, want: u32) -> String {
    format!("VM got {got} of {want} CPUs (host overloaded?)")
}

/// A fresh short path for a VM's monitor socket.
fn monitor_socket() -> PathBuf {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    env::temp_dir().join(format!("gala-{}-{n}.mon", std::process::id()))
}

/// Marks of the guest's state dump (kernel/src/statedump.rs).
const STATEDUMP_BEGIN: &str = "statedump: begin";
const STATEDUMP_NMI_DONE: &str = "statedump: NMI part done";
const STATEDUMP_END: &str = "statedump: end";
/// How long a timed-out guest gets to finish its state dump (TCG on a
/// loaded host prints slowly).
const STATEDUMP_WAIT: Duration = Duration::from_secs(20);

/// A boot that timed out: injects an NMI through the monitor (QEMU's
/// `nmi` reaches every CPU; the kernel prints a state dump: each CPU's
/// rip and backtrace, held locks, memory, the flight recorder, then every
/// thread's system call and wait), waits for it to end, and says so in a
/// line for the log (docs/OBSERVABILITY.md, "State dump").
fn ask_for_state_dump(
    socket: &Path,
    log: &Path,
    child: &mut std::process::Child,
    vm_clock: &mut vmtime::VmClock,
    why: &str,
) -> String {
    use std::io::Write as _;
    let seen = |mark: &str| {
        fs::read(log).is_ok_and(|b| b.windows(mark.len()).any(|w| w == mark.as_bytes()))
    };
    // Kept open while the guest dumps (QEMU may drop a command whose
    // connection closed before it read it).
    let monitor = std::os::unix::net::UnixStream::connect(socket).and_then(|mut m| {
        m.write_all(b"nmi\n")?;
        m.flush()?;
        Ok(m)
    });
    let sent = monitor.as_ref().map(|_| ()).map_err(|e| e.to_string());
    let asked = Instant::now();
    let run_at_ask = vm_clock.sample().run_ms();
    // STATEDUMP_WAIT of the VM's run time, as its limit (vmtime.rs).
    let mut waited_enough = || {
        let ran = vm_clock.sample().run_ms().saturating_sub(run_at_ask);
        ran > STATEDUMP_WAIT.as_millis() as u64
            || asked.elapsed() > STATEDUMP_WAIT * vmtime::BACKSTOP
    };
    if sent.is_ok() {
        while !waited_enough() && !seen(STATEDUMP_END) {
            if child.try_wait().ok().flatten().is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
    let wait = STATEDUMP_WAIT.as_secs();
    match sent {
        Err(e) => format!("\nxtask: {why}; no state dump (monitor: {e})\n"),
        Ok(()) if seen(STATEDUMP_END) => format!(
            "\nxtask: {why}; the guest's state dump (NMI) took {:.1}s\n",
            asked.elapsed().as_secs_f64()
        ),
        Ok(()) if seen(STATEDUMP_NMI_DONE) => format!(
            "\nxtask: {why}; the state dump's NMI part came, its threads part \
             did not within {wait}s of run time (the boot thread could not run: the scheduler or a lock stuck?)\n"
        ),
        Ok(()) if seen(STATEDUMP_BEGIN) => format!(
            "\nxtask: {why}; the state dump began but did not end within {wait}s of run time\n"
        ),
        Ok(()) => format!(
            "\nxtask: {why}; the guest did not answer the NMI within {wait}s of run time \
             (no NMI handler yet: early boot, or the machine is wedged)\n"
        ),
    }
}

/// Joins the `{"ev":"rec","hex":...}` lines of an event stream into the
/// recording they carry (a `record` boot; kernel/src/replay.rs), written
/// to `out`; returns the line saying so, None without such lines.
fn extract_recording(events: &Path, out: &Path) -> Option<String> {
    let text = fs::read_to_string(events).ok()?;
    let mut bytes = Vec::new();
    for line in text.lines().filter(|l| l.contains(r#""ev":"rec""#)) {
        let hex = line.split(r#""hex":""#).nth(1)?.split('"').next()?;
        for i in (0..hex.len()).step_by(2) {
            bytes.push(u8::from_str_radix(hex.get(i..i + 2)?, 16).ok()?);
        }
    }
    if bytes.is_empty() {
        return None;
    }
    let magic = bytes.starts_with(b"GALAREC\0");
    fs::write(out, &bytes).ok()?;
    Some(format!(
        "xtask: recording {} ({} bytes{})\n",
        out.display(),
        bytes.len(),
        if magic { "" } else { ", no GALAREC header" }
    ))
}

/// What a scenario's screen must show once its serial log has `marker`
/// (screendump through QEMU's monitor; pixels on the guest's screen), in
/// steps: each sends its monitor commands (input), waits for the guest to
/// draw, takes a screendump and checks its pixels.
struct ScreenCheck {
    marker: &'static str,
    steps: &'static [ScreenStep],
}

struct ScreenStep {
    /// A serial-log line (substring) the guest must have printed before
    /// the input is sent ("" for none): the program is done drawing
    /// what the input acts on, however slowly a busy host runs it.
    after: &'static str,
    /// QEMU monitor commands sent first (`mouse_move`, `mouse_button`,
    /// `sendkey`), 250 ms apart (time for the guest to read the device, also
    /// when the host is busy). Keep each move within 255 pixels per axis:
    /// QEMU queues a few PS/2 packets of at most 255 and hands the rest of a
    /// larger move over with later events.
    input: &'static [&'static str],
    pixels: &'static [PixelCheck],
}

#[derive(Clone, Copy)]
enum PixelCheck {
    /// The pixel at (x, y) is this 0xRRGGBB colour.
    At(u32, u32, u32),
    /// Some pixel in the rectangle (x, y, w, h) is close to this colour
    /// (anti-aliased text, scaled images).
    AnyIn(u32, u32, u32, u32, u32),
}

/// gui_test's window: 400 x 300 at (100, 100); the desktop's frame is a
/// 1-pixel outline and a 32-pixel caption (Windows 11's metrics), so its
/// client area starts at (101, 133) (kernel/src/nt/desktop). The screen is
/// 1280 x 800: the wallpaper, the taskbar in the bottom 48 rows with the
/// start button (Gala's fortress) and the window's button centred, the
/// clock at the right; the start menu (600 x 640) opens centred above it
/// at (340, 100). Shaded colours are checked as "some pixel near".
const GUI_TEST_SCREEN: ScreenCheck = ScreenCheck {
    marker: "gui_test: painted",
    steps: &[
        ScreenStep {
            after: "",
            input: &[],
            pixels: &[
                PixelCheck::AnyIn(4, 4, 12, 12, 0x060e2b), // the night wallpaper's corner
                PixelCheck::At(300, 110, 0xeef2f7),        // the active caption
                PixelCheck::At(101 + 100, 133 + 70, 0xff0000), // FillRect's red
                PixelCheck::At(101 + 300, 133 + 200, 0xffffff), // COLOR_WINDOW
                PixelCheck::AnyIn(101 + 20, 133 + 140, 140, 16, 0x0000ff), // the text
                PixelCheck::AnyIn(880, 780, 40, 12, 0xe0e7f3), // the taskbar
                PixelCheck::AnyIn(600, 760, 32, 30, 0x167fd6), // the fortress
                PixelCheck::AnyIn(640, 788, 48, 8, 0x005fb8), // the active window's pill
                PixelCheck::AnyIn(648, 760, 32, 24, 0xff8000), // its class icon
                PixelCheck::At(121, 117, 0xff8000),        // and in its caption
            ],
        },
        ScreenStep {
            after: "",
            // From the middle to the start button (615, 776), a click.
            input: &["mouse_move 0 255", "mouse_move -25 121", "mouse_button 1", "mouse_button 0"],
            pixels: &[
                PixelCheck::AnyIn(590, 600, 20, 20, 0xf2f4f8), // the start menu
                PixelCheck::AnyIn(392, 182, 60, 18, 0x1b1b1b), // "Pinned"
                PixelCheck::AnyIn(398, 226, 32, 32, 0x4a80f0), // "Hello window"
            ],
        },
        ScreenStep {
            after: "",
            // Escape closes the menu; to client (250, 50) at (351, 183)
            // and a click: the program's green square.
            input: &[
                "sendkey esc",
                "mouse_move -132 -198",
                "mouse_move -132 -198",
                "mouse_move 0 -197",
                "mouse_button 1",
                "mouse_button 0",
            ],
            pixels: &[
                PixelCheck::AnyIn(343, 175, 16, 16, 0x00c000), // the green mark
                PixelCheck::At(101 + 300, 133 + 200, 0xffffff), // the menu closed
            ],
        },
        ScreenStep {
            after: "",
            // Typing goes to the active window: "hi" in black.
            input: &["sendkey h", "sendkey i"],
            pixels: &[PixelCheck::AnyIn(101 + 20, 133 + 200, 24, 16, 0x000000)],
        },
        ScreenStep {
            after: "",
            // Keys typed while the program is busy: `q` keeps its window
            // procedure in Sleep (3 s) while x and y come; it says
            // whether their messages came in Windows' order.
            input: &["sendkey q", "sendkey x", "sendkey y"],
            pixels: &[],
        },
        ScreenStep {
            after: "gui_test: keys typed while busy",
            // A right click at client (250, 50): the popup menu
            // (One, Two) at (351, 183), over the window.
            input: &["mouse_button 2", "mouse_button 0"],
            pixels: &[
                PixelCheck::At(470, 200, 0xf9f9f9),            // the menu
                PixelCheck::AnyIn(386, 190, 30, 14, 0x1b1b1b), // "One"
            ],
        },
        ScreenStep {
            after: "",
            // Down to "Two" (381, 222) and a click: TrackPopupMenu
            // returns 202, a blue square at the point; the menu
            // goes.
            input: &["mouse_move 30 39", "mouse_button 1", "mouse_button 0"],
            pixels: &[
                PixelCheck::AnyIn(345, 177, 12, 12, 0x0000ff), // the blue mark
                PixelCheck::At(470, 200, 0xffffff),            // the menu gone
            ],
        },
        ScreenStep {
            after: "",
            // The edit control at client (220, 230) - (370, 256):
            // a click in it (396, 376) and "ok" typed show there, with
            // the focused edit's accent underline.
            input: &[
                "mouse_move 15 154",
                "mouse_button 1",
                "mouse_button 0",
                "sendkey o",
                "sendkey k",
            ],
            pixels: &[
                PixelCheck::AnyIn(326, 368, 30, 16, 0x1b1b1b), // "ok"
                PixelCheck::AnyIn(323, 386, 140, 3, 0x005fb8), // the underline
            ],
        },
        ScreenStep {
            after: "",
            // The drop-down list combo box at client (220, 20) -
            // (370, 46), screen (321, 153) - (471, 179): a click on
            // it (396, 166) drops its list, a popup under it
            // (y 179..257, items of 24 rows from 182): light grey,
            // "Red" selected (the selected item's tint and accent
            // pill), "Blue" in the third row.
            input: &["mouse_move 0 -210", "mouse_button 1", "mouse_button 0"],
            pixels: &[
                PixelCheck::At(468, 220, 0xf9f9f9),            // the list
                PixelCheck::At(321, 220, 0xc8c8c8),            // its border
                PixelCheck::AnyIn(325, 186, 4, 16, 0x005fb8),  // the pill
                PixelCheck::AnyIn(335, 232, 40, 20, 0x1b1b1b), // "Blue"
            ],
        },
        ScreenStep {
            after: "",
            // The pointer on "Blue" (396, 242) lights it.
            input: &["mouse_move 0 76"],
            pixels: &[
                PixelCheck::At(332, 242, 0xe6e8eb), // the hover
                PixelCheck::At(332, 194, 0xeeeff1), // "Red", selected
            ],
        },
        ScreenStep {
            after: "",
            // A click picks it: the list goes, the program paints
            // its magenta square at client (380, 60) - (392, 72).
            input: &["mouse_button 1", "mouse_button 0"],
            pixels: &[
                PixelCheck::At(487, 199, 0xff00ff),            // the mark
                PixelCheck::At(468, 220, 0xffffff),            // the list gone
                PixelCheck::AnyIn(331, 156, 40, 20, 0x1b1b1b), // "Blue" shown
            ],
        },
        ScreenStep {
            after: "",
            // The pointer rests on the edit (396, 376): after the
            // initial delay its tooltip shows 20 rows below the
            // pointer, at (396, 396): a grey border, a light face,
            // "Type here" in dark text from (405, 401).
            input: &["mouse_move 0 134"],
            pixels: &[
                PixelCheck::At(396, 405, 0xc8c8c8),            // the border
                PixelCheck::At(398, 398, 0xf9f9f9),            // the face
                PixelCheck::AnyIn(405, 400, 50, 18, 0x1b1b1b), // "Type here"
            ],
        },
        ScreenStep {
            after: "",
            // Off the edit, into the client area (396, 276): the
            // tip goes (the window's white under it again).
            input: &["mouse_move 0 -100"],
            pixels: &[PixelCheck::At(398, 398, 0xffffff)],
        },
        ScreenStep {
            after: "",
            // The close button (x 453..498 of the caption): the program
            // ends (WM_SYSCOMMAND SC_CLOSE; no screendump).
            input: &["mouse_move 80 -159", "mouse_button 1", "mouse_button 0"],
            pixels: &[],
        },
    ],
};

/// Scenarios with screen checks.
const SCREEN_SCENARIOS: &[(&str, ScreenCheck)] = &[
    (
        // Chromium's window at (0, 0), 800 x 600, its page (window.html)
        // below Chromium's own tab strip and toolbar: the green body, the
        // magenta box at page (100, 100) - (300, 200) and the gold link
        // block at page (400, 100) - (600, 200). Then dragged by its own
        // tab strip, "gala" typed into the text field (page (100, 250)),
        // the link clicked (window2.html: a blue body), and Alt+F4.
        "chromium_window",
        ScreenCheck {
            marker: "gala window painted",
            steps: &[
                ScreenStep {
                    after: "",
                    input: &[],
                    pixels: &[
                        PixelCheck::AnyIn(20, 400, 60, 60, 0x2e8b57), // the body
                        PixelCheck::AnyIn(120, 200, 160, 160, 0xff00ff), // the box
                    ],
                },
                ScreenStep {
                    after: "",
                    // A press on the tab strip's empty part (500, 20):
                    // Chromium's WM_NCHITTEST says HTCAPTION, so user32
                    // asks the desktop to drag the window.
                    input: &["mouse_move -140 -200", "mouse_move 0 -180", "mouse_button 1"],
                    pixels: &[],
                },
                ScreenStep {
                    after: "desktop: dragging window",
                    // Dragged by (100, 100): the box moves with it, and
                    // the whole new area is redrawn: the page's green at
                    // its bottom-right corner, drawn last (the window spans
                    // 100..900 x 100..700 now).
                    input: &["mouse_move 100 100", "mouse_button 0"],
                    pixels: &[
                        PixelCheck::AnyIn(220, 300, 160, 160, 0xff00ff),
                        PixelCheck::AnyIn(520, 300, 160, 160, 0xffd700),
                        PixelCheck::At(880, 600, 0x2e8b57),
                    ],
                },
                ScreenStep {
                    after: "",
                    // A click in the text field (page (100, 250) -
                    // (300, 290): 200..400 x 493..533 on the screen after
                    // the drag; the pointer is at (600, 120)): it takes
                    // the focus.
                    input: &["mouse_move -300 393", "mouse_button 1", "mouse_button 0"],
                    pixels: &[],
                },
                ScreenStep {
                    after: "gala text focused",
                    // Typing with no IME (imm32 has no input contexts):
                    // the keys' WM_KEYDOWN and WM_CHAR reach the page.
                    input: &["sendkey g", "sendkey a", "sendkey l", "sendkey a"],
                    pixels: &[],
                },
                ScreenStep {
                    after: "gala typed gala",
                    // A click on the link block's middle (the pointer is at
                    // (300, 513); the block spans 500..700 x 343..443 now,
                    // below the toolbar and the --no-sandbox infobar):
                    // Chromium follows it.
                    input: &["mouse_move 300 -123", "mouse_button 1", "mouse_button 0"],
                    pixels: &[],
                },
                ScreenStep {
                    after: "gala link followed",
                    // window2.html's blue where window.html's box was.
                    input: &[],
                    pixels: &[PixelCheck::AnyIn(220, 300, 160, 160, 0x1e90ff)],
                },
                ScreenStep {
                    after: "",
                    // Alt+F4: keys reach Chromium once its window is
                    // activated (user32's WM_ACTIVATE gives it the focus).
                    input: &["sendkey alt-f4"],
                    pixels: &[],
                },
            ],
        },
    ),
    ("gui_test", GUI_TEST_SCREEN),
    // The same program and checks with the screen on virtio-gpu (stage G1
    // of ARCHITECTURE "Plan: graphics and GPU"): QEMU's screendump shows
    // the scanout, so the pixels got there by TRANSFER_TO_HOST_2D and
    // RESOURCE_FLUSH; the second resets the device midway (gpureset=).
    ("virtio_gpu", GUI_TEST_SCREEN),
    ("virtio_gpu_reset", GUI_TEST_SCREEN),
    // The same on a headless desktop (`presenter=test`, a BIOS boot with
    // no screen): the pixels are the Test presenter's frame in the
    // guest's memory (`dump_frame`), not a screendump.
    ("gui_test_headless", GUI_TEST_SCREEN),
    (
        // GPU G2: vk_test's frame (lavapipe), presented through
        // VK_KHR_win32_surface (Mesa's software swapchain blits it into
        // the window's DC) into its 400 x 300 window at (100, 100), client
        // 398 x 266 from (101, 133): the clear colour in the corner, the
        // triangle's inside; then a click there ends the program.
        "vk_test",
        ScreenCheck {
            marker: "vk_test: presented",
            steps: &[
                ScreenStep {
                    after: "",
                    input: &[],
                    pixels: &[
                        PixelCheck::At(101 + 4, 133 + 4, 0x336699),
                        PixelCheck::At(101 + 199, 133 + 173, 0xffcc00),
                        PixelCheck::At(101 + 30, 133 + 240, 0x336699),
                    ],
                },
                ScreenStep {
                    after: "",
                    input: &[
                        "mouse_move -255 -94",
                        "mouse_move -85 0",
                        "mouse_button 1",
                        "mouse_button 0",
                    ],
                    pixels: &[],
                },
            ],
        },
    ),
    (
        // GPU G2: gl_test's frame (llvmpipe through Mesa's WGL), presented
        // by SwapBuffers into its 400 x 300 window at (100, 100), client
        // 398 x 266 from (101, 133): the clear colour in the client's
        // corner, the triangle's at (199, 173) of the client; then a click
        // there ends the program (the pointer starts at (640, 400)).
        "gl_test",
        ScreenCheck {
            marker: "gl_test: presented",
            steps: &[
                ScreenStep {
                    after: "",
                    input: &[],
                    pixels: &[
                        PixelCheck::At(101 + 4, 133 + 4, 0x336699),
                        PixelCheck::At(101 + 199, 133 + 173, 0xffcc00),
                        PixelCheck::At(101 + 30, 133 + 240, 0x336699),
                    ],
                },
                ScreenStep {
                    after: "",
                    input: &[
                        "mouse_move -255 -94",
                        "mouse_move -85 0",
                        "mouse_button 1",
                        "mouse_button 0",
                    ],
                    pixels: &[],
                },
            ],
        },
    ),
    (
        // RS B3a's console switch (user/presenter_test): session 1's
        // yellow window (client from (601, 133)) on the screen, clicked;
        // after `gala attach 2` session 2's (client from (101, 133), blue,
        // with the magenta mark of the click injected earlier at (301,
        // 233)) is there instead, clicked twice through PS/2 (input
        // follows the console); back on session 1 its window and mark are
        // shown again, and a click reaches it. Marks are 12 x 12 around
        // the point, checked up and left of it (the pointer is drawn down
        // and right). Each desktop keeps its own pointer: moves start
        // from (0, 0) (four moves of -255 reach it from anywhere).
        "presenter_test",
        ScreenCheck {
            marker: "presenter_test: session 1 shows its window",
            steps: &[
                ScreenStep {
                    after: "",
                    input: &[],
                    pixels: &[
                        PixelCheck::At(651, 183, 0xffff00), // session 1's client
                        PixelCheck::At(800, 110, 0xeef2f7), // its active caption
                    ],
                },
                ScreenStep {
                    after: "",
                    // To client (200, 150) at (801, 283), a click.
                    input: &[
                        "mouse_move -255 -255",
                        "mouse_move -255 -255",
                        "mouse_move -255 -255",
                        "mouse_move -255 -255",
                        "mouse_move 255 255",
                        "mouse_move 255 28",
                        "mouse_move 255 0",
                        "mouse_move 36 0",
                        "mouse_button 1",
                        "mouse_button 0",
                    ],
                    pixels: &[],
                },
                ScreenStep {
                    after: "presenter_test: the console shows session 2",
                    input: &[],
                    pixels: &[
                        PixelCheck::At(151, 183, 0x0000ff), // session 2's client
                        PixelCheck::At(297, 229, 0xff00ff), // the injected click's mark
                    ],
                },
                ScreenStep {
                    after: "",
                    // To client (250, 150) at (351, 283), a click: its mark.
                    input: &[
                        "mouse_move -255 -255",
                        "mouse_move -255 -255",
                        "mouse_move -255 -255",
                        "mouse_move -255 -255",
                        "mouse_move 255 255",
                        "mouse_move 96 28",
                        "mouse_button 1",
                        "mouse_button 0",
                    ],
                    pixels: &[PixelCheck::At(347, 279, 0xff00ff)],
                },
                ScreenStep {
                    after: "presenter_test: a click on the console reached session 2",
                    // A second click: the program goes on (switches back).
                    input: &["mouse_button 1", "mouse_button 0"],
                    pixels: &[],
                },
                ScreenStep {
                    after: "presenter_test: the console shows session 1 again",
                    input: &[],
                    pixels: &[
                        PixelCheck::At(651, 183, 0xffff00), // session 1's client, kept
                        PixelCheck::At(797, 279, 0xff00ff), // and the first click's mark
                    ],
                },
                ScreenStep {
                    after: "",
                    // Session 1's pointer is where it was: 40 down, a click.
                    input: &["mouse_move 0 40", "mouse_button 1", "mouse_button 0"],
                    pixels: &[],
                },
            ],
        },
    ),
    (
        // `gala snap left` put gala_cli_test's window (client area
        // RGB 2e 8b 57) on the left half of the work area (640 x 752 on
        // the 1280 x 800 screen): its caption at the top, square
        // corners (docked); the night wallpaper on the right. Then `g`
        // typed into it (it renames itself, which the test waits for).
        "gala_cli",
        ScreenCheck {
            marker: "gala_cli_test: snapped left",
            steps: &[
                ScreenStep {
                    after: "",
                    input: &[],
                    pixels: &[
                        PixelCheck::At(300, 12, 0xeef2f7),             // the active caption
                        PixelCheck::At(0, 0, 0x9ca3ad), // a square corner: the outline
                        PixelCheck::At(20, 60, 0x2e8b57), // the client area, top left
                        PixelCheck::At(620, 740, 0x2e8b57), // and bottom right
                        PixelCheck::AnyIn(900, 300, 40, 40, 0x060e2b), // the wallpaper
                    ],
                },
                ScreenStep { after: "", input: &["sendkey g"], pixels: &[] },
            ],
        },
    ),
    (
        // The console window: conhost's grid at (60, 60) with the
        // program's lines: light grey text, Campbell green and bright red.
        "console_window",
        ScreenCheck {
            marker: "console_window_test: printed",
            steps: &[
                ScreenStep {
                    after: "",
                    input: &[],
                    pixels: &[
                        PixelCheck::AnyIn(66, 98, 300, 14, 0xcccccc), // "Hello from ..."
                        PixelCheck::AnyIn(66, 112, 300, 22, 0x13a10e), // green
                        PixelCheck::AnyIn(66, 112, 400, 22, 0xe74856), // bright red
                    ],
                },
                ScreenStep {
                    after: "",
                    // "hi" and Enter in the window: echoed on the fifth line
                    // (y 174..190) by the console's cooked input.
                    input: &["sendkey h", "sendkey i", "sendkey ret"],
                    pixels: &[PixelCheck::AnyIn(66, 174, 30, 16, 0xcccccc)],
                },
                ScreenStep {
                    after: "",
                    // "ok" and Enter (ReadConsoleW), then "x" (one raw key
                    // record): the program's answer on the ninth line.
                    input: &["sendkey o", "sendkey k", "sendkey ret", "sendkey x"],
                    pixels: &[PixelCheck::AnyIn(66, 250, 300, 16, 0xcccccc)],
                },
                ScreenStep {
                    after: "",
                    // Win+Right (Snap): the console fills the right half of
                    // the work area (its black down to the taskbar), its
                    // grid follows and keeps the text, now at the top left
                    // of that half.
                    input: &["sendkey meta_l-right"],
                    pixels: &[
                        PixelCheck::At(1250, 700, 0x0c0c0c),
                        PixelCheck::At(660, 700, 0x0c0c0c),
                        PixelCheck::AnyIn(646, 38, 300, 16, 0xcccccc), // "Hello from ..."
                    ],
                },
                ScreenStep {
                    after: "",
                    // Raw input: "z" without Enter, then Ctrl+C (a
                    // CTRL_C_EVENT, not a key): the program's answers on
                    // the 11th and 12th lines (19 pixels each).
                    input: &["sendkey z", "sendkey ctrl-c"],
                    pixels: &[
                        PixelCheck::AnyIn(646, 38 + 10 * 19, 300, 16, 0xcccccc),
                        PixelCheck::AnyIn(646, 38 + 11 * 19, 300, 16, 0xcccccc),
                    ],
                },
                ScreenStep {
                    // After the last of them: wheel events that came while
                    // the lines were still printing scrolled from another
                    // bottom (step 5 failed under load, 2026-11-04).
                    after: "line 39",
                    // Forty more lines pushed the first ones off the top;
                    // the wheel turned up (six notches, three lines each)
                    // scrolls back to them: the green line shows again.
                    input: &[
                        "mouse_move 0 0 1",
                        "mouse_move 0 0 1",
                        "mouse_move 0 0 1",
                        "mouse_move 0 0 1",
                        "mouse_move 0 0 1",
                        "mouse_move 0 0 1",
                    ],
                    pixels: &[PixelCheck::AnyIn(646, 38 + 19, 300, 16, 0x13a10e)],
                },
                ScreenStep {
                    after: "",
                    // Done looking: "q" (raw input) ends the program. It
                    // used to end 8 s after its last line, which a screen
                    // check on a loaded host outlasted (step 5: the
                    // monitor gone, 2026-11-04).
                    input: &["sendkey q"],
                    pixels: &[],
                },
            ],
        },
    ),
    (
        // FileZilla's main window with its Welcome dialog (wx's modal loop:
        // MsgWaitForMultipleObjects): the accent Quickconnect and OK
        // buttons; then OK closes the dialog, File on the menu bar opens
        // its menu and Exit ends FileZilla.
        "filezilla",
        ScreenCheck {
            marker: "desktop: shown window 'Welcome to FileZilla'",
            steps: &[
                ScreenStep {
                    after: "",
                    input: &[],
                    pixels: &[
                        PixelCheck::AnyIn(660, 118, 80, 16, 0x005fb8), // Quickconnect
                        PixelCheck::AnyIn(730, 512, 60, 20, 0x005fb8), // OK
                    ],
                },
                ScreenStep {
                    after: "",
                    // To OK (763, 522) and a click: the dialog (0xf0f0f0)
                    // goes, the file lists (white) under it show.
                    input: &["mouse_move 122 122", "mouse_button 1", "mouse_button 0"],
                    pixels: &[
                        PixelCheck::At(560, 470, 0xffffff),
                        PixelCheck::At(470, 520, 0xffffff),
                        // The status bar's green LED (AlphaBlend; it was
                        // on black, then washed out).
                        PixelCheck::AnyIn(1208, 716, 10, 4, 0x015001),
                    ],
                },
                ScreenStep {
                    after: "",
                    // From OK (763, 522) to the toolbar's Refresh button
                    // (206, 93): after the initial delay its tooltip (the
                    // toolbar's tooltip control, the text from wx's
                    // TTN_NEEDTEXT) shows 20 rows below the pointer: the
                    // tip's light face inside its border.
                    input: &["mouse_move -255 -255", "mouse_move -255 -174", "mouse_move -47 0"],
                    pixels: &[PixelCheck::At(208, 115, 0xf9f9f9)],
                },
                ScreenStep {
                    after: "",
                    // Back to (763, 522), where the next step starts.
                    input: &["mouse_move 255 255", "mouse_move 255 174", "mouse_move 47 0"],
                    pixels: &[],
                },
                ScreenStep {
                    after: "",
                    // Quickconnect to FileZilla Server (started first):
                    // Host (136, 126) "127.0.0.1", Username (318) "gala",
                    // Password (499) "gala" (the server's user needs none;
                    // FileZilla asks for one otherwise), Port (620) "7",
                    // then the Quickconnect button (702).
                    input: &[
                        "mouse_move -209 -132",
                        "mouse_move -209 -132",
                        "mouse_move -209 -132",
                        "mouse_button 1",
                        "mouse_button 0",
                        "sendkey 1",
                        "sendkey 2",
                        "sendkey 7",
                        "sendkey dot",
                        "sendkey 0",
                        "sendkey dot",
                        "sendkey 0",
                        "sendkey dot",
                        "sendkey 1",
                        "mouse_move 182 0",
                        "mouse_button 1",
                        "mouse_button 0",
                        "sendkey g",
                        "sendkey a",
                        "sendkey l",
                        "sendkey a",
                        "mouse_move 181 0",
                        "mouse_button 1",
                        "mouse_button 0",
                        "sendkey g",
                        "sendkey a",
                        "sendkey l",
                        "sendkey a",
                        "mouse_move 121 0",
                        "mouse_button 1",
                        "mouse_button 0",
                        "sendkey 7",
                        "mouse_move 82 0",
                        "mouse_button 1",
                        "mouse_button 0",
                    ],
                    // The first connection asks whether to remember
                    // passwords: its OK button.
                    pixels: &[PixelCheck::AnyIn(730, 491, 60, 20, 0x005fb8)],
                },
                ScreenStep {
                    after: "",
                    // OK (763, 501): FileZilla connects to the server.
                    input: &[
                        "mouse_move 61 188",
                        "mouse_move 0 187",
                        "mouse_button 1",
                        "mouse_button 0",
                    ],
                    // Then, after the TLS handshake, the server's
                    // self-signed certificate: the "Unknown certificate"
                    // dialog's OK (y 632-652).
                    pixels: &[PixelCheck::AnyIn(800, 636, 60, 12, 0x005fb8)],
                },
                ScreenStep {
                    after: "",
                    // OK (833, 642): FileZilla trusts it, logs on and lists
                    // the remote directory (C:\ftp): "hello.txt" at the left
                    // of the remote list.
                    input: &["mouse_move 70 141", "mouse_button 1", "mouse_button 0"],
                    pixels: &[PixelCheck::AnyIn(650, 405, 150, 40, 0x1b1b1b)],
                },
                ScreenStep {
                    after: "",
                    // Win+Right (Snap): FileZilla on the right half of the
                    // screen (its Quickconnect button there); at the left
                    // the server's console window (conhost, at (60, 60))
                    // shows with its log: light grey on Campbell black.
                    // FileZilla lays itself out again: its local and remote
                    // panes split the half (sash gravity 0.5), so the remote
                    // list's "Filename" header and "hello.txt" are in the
                    // right quarter (with the old layout, or a repaint not
                    // yet shown, nothing dark is there).
                    input: &["sendkey meta_l-right"],
                    pixels: &[
                        PixelCheck::AnyIn(640, 60, 640, 120, 0x005fb8),
                        PixelCheck::At(500, 400, 0x0c0c0c),
                        PixelCheck::AnyIn(66, 98, 400, 400, 0xcccccc),
                        PixelCheck::AnyIn(966, 356, 44, 14, 0x1b1b1b),
                        PixelCheck::AnyIn(970, 405, 40, 15, 0x1b1b1b),
                    ],
                },
                ScreenStep {
                    after: "",
                    // A click in the console (300, 387) activates it, and
                    // Win+Left snaps it to the left half: the server's log
                    // next to FileZilla.
                    input: &[
                        "mouse_move -255 -255",
                        "mouse_move -255 0",
                        "mouse_move -23 0",
                        "mouse_button 1",
                        "mouse_button 0",
                        "sendkey meta_l-left",
                    ],
                    pixels: &[
                        PixelCheck::At(20, 700, 0x0c0c0c),
                        PixelCheck::At(620, 700, 0x0c0c0c),
                        PixelCheck::AnyIn(6, 38, 600, 400, 0xcccccc),
                        PixelCheck::AnyIn(640, 60, 640, 120, 0x005fb8),
                    ],
                },
                ScreenStep {
                    after: "",
                    // The pointer to the corner (0, 0), then to "File" on
                    // FileZilla's menu bar (663, 44) and a click: its menu
                    // opens.
                    input: &[
                        "mouse_move -255 -255",
                        "mouse_move -255 -255",
                        "mouse_move -255 -255",
                        "mouse_move 255 0",
                        "mouse_move 255 0",
                        "mouse_move 153 44",
                        "mouse_button 1",
                        "mouse_button 0",
                    ],
                    pixels: &[
                        PixelCheck::At(980, 200, 0xf9f9f9),            // the menu
                        PixelCheck::AnyIn(680, 281, 24, 14, 0x1b1b1b), // "Exit"
                        PixelCheck::AnyIn(680, 90, 60, 14, 0x9d9d9d),  // a disabled item
                    ],
                },
                ScreenStep {
                    after: "",
                    // Down to "Exit" (692, 288) and a click: WM_COMMAND,
                    // FileZilla ends (status 0).
                    input: &["mouse_move 29 244", "mouse_button 1", "mouse_button 0"],
                    pixels: &[],
                },
            ],
        },
    ),
];

fn screen_check(name: &str) -> Option<&'static ScreenCheck> {
    SCREEN_SCENARIOS.iter().find(|(n, _)| *n == name).map(|(_, c)| c)
}

/// Takes a screendump through the monitor at `socket` (kept as
/// `<iso>.screen.png`) and checks `check`'s pixels.
fn check_screen(socket: &Path, iso: &Path, check: &ScreenCheck) -> Result {
    use std::io::Write;
    let mut monitor = None;
    for _ in 0..100 {
        if let Ok(s) = std::os::unix::net::UnixStream::connect(socket) {
            monitor = Some(s);
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    let mut monitor = monitor.ok_or("cannot connect to the QEMU monitor")?;
    let log = iso.with_extension("serial.log");
    for (n, step) in check.steps.iter().enumerate() {
        if !step.after.is_empty() {
            wait_for_line(&log, step.after).map_err(|e| format!("step {n}: {e}"))?;
        }
        for command in step.input {
            // Each command, then time for the guest to read the device
            // (QEMU's PS/2 queue is short: a click queued behind a long
            // move is dropped).
            // A press waits longer: under load (the full suite) the guest
            // drains the queue slowly and a press behind moves is lost.
            if command.starts_with("mouse_button 1") {
                thread::sleep(Duration::from_millis(750));
            }
            writeln!(monitor, "{command}").map_err(|e| e.to_string())?;
            thread::sleep(Duration::from_millis(250));
        }
        check_screen_step(&mut monitor, iso, n, step).map_err(|e| format!("step {n}: {e}"))?;
    }
    Ok(())
}

/// Waits for `line` in the serial log `log`. A give-up only (the VM's
/// own limit is not checked while a screen check runs): two minutes.
fn wait_for_line(log: &Path, line: &str) -> Result {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(120) {
        if fs::read(log).is_ok_and(|b| b.windows(line.len()).any(|w| w == line.as_bytes())) {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
    Err(format!("no {line:?} in the serial log within 120 s"))
}

fn check_screen_step(
    monitor: &mut std::os::unix::net::UnixStream,
    iso: &Path,
    n: usize,
    step: &ScreenStep,
) -> Result {
    if step.pixels.is_empty() {
        return Ok(());
    }
    // Let the guest finish drawing; under load (the full suite) a redraw
    // can take longer, and a step may wait for the program (a TLS handshake
    // under TCG takes seconds), so a failed check is retried for 30 s.
    let mut tries = 0;
    loop {
        thread::sleep(Duration::from_millis(500));
        match dump_and_check(monitor, iso, n, step) {
            Err(_) if tries < 60 => tries += 1,
            r => return r,
        }
    }
}

/// Whether scenario `name`'s screen is a Test presenter's frame in the
/// guest's memory (`presenter=test` on its command line) rather than a
/// screen QEMU shows.
fn headless_screen(name: &str) -> bool {
    SCENARIOS
        .iter()
        .find(|s| s.name == name)
        .is_some_and(|s| s.cmdline.split_whitespace().any(|w| w == "presenter=test"))
}

/// Where the console session's Test frame is: (address, width, height)
/// from the serial log's last `presenter: test frame for session 1:` line
/// (the kernel logs it when it makes the frame; ARCHITECTURE
/// "Presenters (B3a)").
fn frame_place(log: &Path) -> Result<(u64, usize, usize)> {
    const LINE: &str = "presenter: test frame for session 1: ";
    let text = fs::read_to_string(log).map_err(|e| format!("reading {log:?}: {e}"))?;
    let rest = text
        .lines()
        .rev()
        .find_map(|l| l.split_once(LINE).map(|(_, r)| r))
        .ok_or("no `presenter: test frame for session 1:` line in the serial log")?;
    // "1280x800 at 0xffff... (4096000 bytes, ...)"
    let parse = || -> Option<(u64, usize, usize)> {
        let (size, rest) = rest.split_once(" at 0x")?;
        let (w, h) = size.split_once('x')?;
        let address = rest.split_whitespace().next()?;
        Some((u64::from_str_radix(address, 16).ok()?, w.parse().ok()?, h.parse().ok()?))
    };
    parse().ok_or_else(|| format!("cannot read the frame's place from {rest:?}"))
}

/// The guest's Test frame as a PPM at `ppm`: its 0x00RRGGBB pixels read
/// with the monitor's `memsave` (virtual addresses: the kernel's heap is
/// mapped in every address space) and turned into RGB.
fn dump_frame(monitor: &mut std::os::unix::net::UnixStream, iso: &Path, ppm: &Path) -> Result {
    use std::io::Write;
    let (address, w, h) = frame_place(&iso.with_extension("serial.log"))?;
    let raw = ppm.with_extension("frame");
    let _ = fs::remove_file(&raw);
    let size = 4 * w * h;
    // The file name quoted: unquoted, the monitor reads its slashes as
    // part of the size's expression.
    writeln!(monitor, "memsave {address:#x} {size} \"{}\"", raw.display())
        .map_err(|e| e.to_string())?;
    let mut data = Vec::new();
    for _ in 0..100 {
        thread::sleep(Duration::from_millis(50));
        if fs::metadata(&raw).is_ok_and(|m| m.len() as usize == size) {
            data = fs::read(&raw).map_err(|e| format!("reading {raw:?}: {e}"))?;
            break;
        }
    }
    let _ = fs::remove_file(&raw);
    if data.len() != size {
        // What the monitor said (an address it could not read: "Invalid
        // addr ...").
        let said = monitor_reply(monitor);
        return Err(format!(
            "memsave of the frame at {address:#x} gave no {size} bytes; the monitor: {said:?}"
        ));
    }
    let mut out = format!("P6\n{w} {h}\n255\n").into_bytes();
    for p in data.chunks_exact(4) {
        out.extend_from_slice(&[p[2], p[1], p[0]]);
    }
    fs::write(ppm, out).map_err(|e| format!("writing {ppm:?}: {e}"))
}

/// What the monitor has written and not been read (its replies: the last
/// three lines), waiting at most 200 ms.
fn monitor_reply(monitor: &mut std::os::unix::net::UnixStream) -> String {
    use std::io::Read;
    let _ = monitor.set_read_timeout(Some(Duration::from_millis(200)));
    let mut said = Vec::new();
    let mut buf = [0u8; 4096];
    while let Ok(n) = monitor.read(&mut buf) {
        if n == 0 {
            break;
        }
        said.extend_from_slice(&buf[..n]);
    }
    let _ = monitor.set_read_timeout(None);
    // Its last lines, without the line editor's cursor moves.
    let text = String::from_utf8_lossy(&said).replace("\u{1b}[D", "").replace("\u{1b}[K", "");
    let lines: Vec<&str> =
        text.lines().map(str::trim).filter(|l| !l.is_empty() && *l != "(qemu)").collect();
    lines[lines.len().saturating_sub(3)..].join(" | ")
}

fn dump_and_check(
    monitor: &mut std::os::unix::net::UnixStream,
    iso: &Path,
    n: usize,
    step: &ScreenStep,
) -> Result {
    use std::io::Write;
    let ppm = iso.with_extension(format!("screen{n}.ppm"));
    let _ = fs::remove_file(&ppm);
    if headless_screen(iso_scenario(iso)) {
        dump_frame(monitor, iso, &ppm)?;
    } else {
        writeln!(monitor, "screendump {}", ppm.display()).map_err(|e| e.to_string())?;
        let mut previous = 0;
        for _ in 0..100 {
            thread::sleep(Duration::from_millis(50));
            let len = fs::metadata(&ppm).map_or(0, |m| m.len());
            if len > 0 && len == previous {
                break;
            }
            previous = len;
        }
    }
    let (w, h, rgb) = read_ppm(&ppm)?;
    let _ = ppm_to_png(&ppm, &iso.with_extension(format!("screen{n}.png")));
    let at = |x: u32, y: u32| -> Option<u32> {
        if x as usize >= w || y as usize >= h {
            return None;
        }
        let i = 3 * (y as usize * w + x as usize);
        Some(u32::from(rgb[i]) << 16 | u32::from(rgb[i + 1]) << 8 | u32::from(rgb[i + 2]))
    };
    let close = |a: u32, b: u32| {
        (0..3).all(|k| ((a >> (8 * k)) & 0xff).abs_diff((b >> (8 * k)) & 0xff) <= 0x40)
    };
    for &p in step.pixels {
        match p {
            PixelCheck::At(x, y, want) => {
                let got = at(x, y);
                if got != Some(want) {
                    return Err(format!("pixel ({x}, {y}) is {got:06x?}, not {want:06x}"));
                }
            }
            PixelCheck::AnyIn(x, y, rw, rh, want) => {
                let found = (y..y + rh)
                    .any(|yy| (x..x + rw).any(|xx| at(xx, yy).is_some_and(|c| close(c, want))));
                if !found {
                    return Err(format!("no pixel near {want:06x} in ({x}, {y}, {rw}, {rh})"));
                }
            }
        }
    }
    Ok(())
}

/// A binary PPM (P6, 8-bit): width, height and the RGB bytes.
fn read_ppm(ppm: &Path) -> Result<(usize, usize, Vec<u8>)> {
    let data = fs::read(ppm).map_err(|e| format!("reading {ppm:?}: {e}"))?;
    let mut fields = Vec::new();
    let mut i = 0;
    while fields.len() < 4 {
        while data.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        let start = i;
        while data.get(i).is_some_and(|b| !b.is_ascii_whitespace()) {
            i += 1;
        }
        if start == i {
            return Err(format!("{ppm:?}: truncated PPM header"));
        }
        fields.push(String::from_utf8_lossy(&data[start..i]).into_owned());
    }
    let (w, h): (usize, usize) = match (fields[0].as_str(), fields[1].parse(), fields[2].parse()) {
        ("P6", Ok(w), Ok(h)) if fields[3] == "255" => (w, h),
        _ => return Err(format!("{ppm:?}: not an 8-bit P6 PPM")),
    };
    let pixels = data.get(i + 1..i + 1 + w * h * 3).ok_or(format!("{ppm:?}: truncated PPM"))?;
    Ok((w, h, pixels.to_vec()))
}

/// A virtio-net card on QEMU's user network, host `port` forwarded to the
/// guest's TCP and UDP port 7.
fn add_net(cmd: &mut Command, port: u16) {
    add_net_to(cmd, port, 7);
}

/// The same with the guest's port `guest` (sshd's 22).
fn add_net_to(cmd: &mut Command, port: u16, guest: u16) {
    add_net_with(cmd, port, guest, "");
}

/// add_net_to with more `-netdev user` options (`extra`: ",guestfwd=...").
fn add_net_with(cmd: &mut Command, port: u16, guest: u16, extra: &str) {
    cmd.arg("-netdev")
        .arg(format!(
            "user,id=n0,hostfwd=tcp:127.0.0.1:{port}-:{guest},hostfwd=udp:127.0.0.1:{port}-:{guest}{extra}"
        ))
        // The legacy (0.9.5) interface: one I/O BAR (see docs/ARCHITECTURE.md).
        .args(["-device", "virtio-net-pci,netdev=n0,disable-legacy=off"]);
}

/// Scenarios whose guest fetches `http://10.0.2.100/` from the host
/// (`add_host_page_net`).
const HOST_PAGE_SCENARIOS: &[&str] = &["hostnet", "chromium_host", "chromium_wpt"];

/// The page `serve-page` serves (user/hostnet_test checks it byte for
/// byte).
const HOST_PAGE: &str = "<html><body><h1>hello from the host</h1></body></html>\n";

/// A virtio-net card on QEMU's user network whose address 10.0.2.100
/// port 80 is forwarded, a connection at a time, to `xtask serve-page` on
/// the host (QEMU runs the program for each connection, its standard
/// input and output the connection): a page from the host network with
/// no host port to lease or to tell the guest.
fn add_host_page_net(cmd: &mut Command, log: &Path) -> Result {
    let guestfwd = host_page_fwd(cmd, log)?;
    cmd.arg("-netdev")
        .arg(format!("user,id=n0{guestfwd}"))
        .args(["-device", "virtio-net-pci,netdev=n0,disable-legacy=off"]);
    Ok(())
}

/// The `-netdev user` option that sends 10.0.2.100:80 to `xtask
/// serve-page` (",guestfwd=..."), its log set in QEMU's environment
/// (serve-page inherits it); for chromium_wpt, wptserve's ports instead
/// (`wpt::guestfwd`), the log still the client's.
fn host_page_fwd(cmd: &mut Command, log: &Path) -> Result<String> {
    let exe = env::current_exe().map_err(|e| format!("xtask's own path: {e}"))?;
    let exe = exe.display().to_string();
    if exe.contains(',') || exe.contains(' ') {
        return Err(format!("xtask's path {exe:?} cannot go in a -netdev option"));
    }
    // serve-page's standard error is the connection too: it logs each
    // request to `log`, which the run adds to the serial log.
    cmd.env("XTASK_SERVE_PAGE_LOG", log);
    if log.to_string_lossy().contains("chromium_wpt") {
        return wpt::guestfwd();
    }
    Ok(format!(",guestfwd=tcp:10.0.2.100:80-cmd:{exe} serve-page"))
}

/// `cargo xtask serve-page` (QEMU runs it for each connection to
/// 10.0.2.100:80): reads the request's head from standard input, answers
/// with HOST_PAGE and exits (closing the connection). Each request is
/// appended to `XTASK_SERVE_PAGE_LOG` (the run adds it to the serial
/// log), as `xtask: serve-page: "<request line>"`: standard output and
/// error are both the connection.
fn serve_page() -> Result {
    use std::io::{Read, Write};
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    let mut stdin = std::io::stdin().lock();
    while !head.ends_with(b"\r\n\r\n") && head.len() < 64 * 1024 {
        match stdin.read(&mut byte) {
            Ok(1) => head.push(byte[0]),
            _ => break,
        }
    }
    let first =
        String::from_utf8_lossy(head.split(|&b| b == b'\r').next().unwrap_or(&[])).into_owned();
    if let Ok(path) = env::var("XTASK_SERVE_PAGE_LOG")
        && let Ok(mut log) = fs::OpenOptions::new().append(true).create(true).open(path)
    {
        let _ = writeln!(log, "xtask: serve-page: {first:?}");
    }
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{HOST_PAGE}",
        HOST_PAGE.len()
    );
    let mut out = std::io::stdout().lock();
    out.write_all(response.as_bytes()).and_then(|()| out.flush()).map_err(|e| e.to_string())
}

/// A virtio-gpu (modern virtio-pci) as the only display, at 1280 x 800
/// (its preferred mode, which GET_DISPLAY_INFO and the EDID report).
fn add_gpu(cmd: &mut Command) {
    cmd.args(["-vga", "none", "-device", "virtio-gpu-pci,xres=1280,yres=800,edid=on"]);
}

/// A virtio-blk disk backed by the raw image `path` (the legacy
/// interface, as for the network card).
fn add_disk(cmd: &mut Command, path: &Path) {
    cmd.arg("-drive")
        .arg(format!("if=none,id=d0,format=raw,file={}", path.display()))
        .args(["-device", "virtio-blk-pci,drive=d0,disable-legacy=off"]);
}

/// A scenario that gets a disk.
struct DiskScenario {
    name: &'static str,
    /// Bytes.
    size: usize,
    /// What to put on the fresh (zeroed) image.
    setup: fn(&mut [u8]),
    /// What the image must hold after the last boot (its first MiB).
    check: fn(&[u8]) -> Result,
    /// Boots of the same image before the judged one, each with the
    /// lines its serial log must have (files kept across a reboot).
    earlier_boots: &'static [&'static [&'static str]],
    /// The earlier boots end with the kernel's failure status (a panic
    /// on purpose: the crash record's scenario), not success.
    earlier_fail: bool,
}

const DISK_SCENARIOS: &[DiskScenario] = &[
    DiskScenario {
        name: "disk_test",
        size: 16 << 20,
        setup: disk_test_setup,
        check: disk_test_check,
        earlier_boots: &[],
        earlier_fail: false,
    },
    DiskScenario {
        // R1 invariant 4: the device is reset under the driver mid-test.
        name: "disk_reset",
        size: 16 << 20,
        setup: disk_test_setup,
        check: disk_test_check,
        earlier_boots: &[],
        earlier_fail: false,
    },
    DiskScenario {
        // LocalDB's data files stay on the disk (twice: two log areas).
        name: "sqllocaldb_persist",
        size: 1 << 30,
        setup: |_| {},
        check: |_| Ok(()),
        earlier_boots: &[&["tds_test: created", "exited with status 0x00000000"]],
        earlier_fail: false,
    },
    DiskScenario {
        name: "failpoint_disk",
        size: 32 << 20,
        setup: |_| {},
        check: |_| Ok(()),
        earlier_boots: &[],
        earlier_fail: false,
    },
    DiskScenario {
        name: "failpoint_disk_delay",
        size: 32 << 20,
        setup: |_| {},
        check: |_| Ok(()),
        earlier_boots: &[],
        earlier_fail: false,
    },
    DiskScenario {
        // R9 (a): the volume handed over between kernels.
        name: "handover_disk",
        size: 32 << 20,
        setup: |_| {},
        check: |_| Ok(()),
        earlier_boots: &[],
        earlier_fail: false,
    },
    DiskScenario {
        // R8 step 6: the earlier boot is release 2 (trust::EARLIER_SIGNING).
        name: "rollback_refused",
        size: 32 << 20,
        setup: |_| {},
        check: |_| Ok(()),
        earlier_boots: &[&[
            "manifest: release 2 verified (rollback floor 2, ",
            "rollback: the disk's floor is release 2 (was 0), highest release seen 2",
            "gala: boot ok",
        ]],
        earlier_fail: false,
    },
    DiskScenario {
        name: "persist_test",
        size: 32 << 20,
        setup: |_| {},
        check: |_| Ok(()),
        earlier_boots: &[&["persist_test: first boot", "exited with status 0x00000000"]],
        earlier_fail: false,
    },
    DiskScenario {
        // R4: the first boot panics on purpose (`crashtest`, no record yet)
        // and leaves its crash record on the disk; this boot prints it.
        name: "crash_record",
        size: 32 << 20,
        setup: |_| {},
        check: |_| Ok(()),
        earlier_boots: &[&[
            "KERNEL PANIC",
            "crashtest: a deliberate panic",
            "crash: the record is on the disk",
        ]],
        earlier_fail: true,
    },
];

fn disk_scenario(name: &str) -> Option<&'static DiskScenario> {
    DISK_SCENARIOS.iter().find(|d| d.name == name)
}

/// disk_test reads this at the start of sector 1.
fn disk_test_setup(image: &mut [u8]) {
    let text = b"gala disk_test sector 1";
    image[512..512 + text.len()].copy_from_slice(text);
}

/// disk_test wrote sector 2: its text, then each byte's offset (mod 256).
fn disk_test_check(image: &[u8]) -> Result {
    let text = b"written by disk_test";
    let sector = &image[1024..1536];
    let ok = sector.starts_with(text)
        && sector.iter().enumerate().skip(text.len()).all(|(i, &b)| b == i as u8);
    if ok {
        Ok(())
    } else {
        Err("sector 2 of the disk image does not hold what disk_test wrote".into())
    }
}

/// Makes scenario `name`'s fresh disk image next to `iso`, if it has one.
fn new_disk(name: &str, iso: &Path) -> Result<Option<PathBuf>> {
    let Some(d) = disk_scenario(name) else { return Ok(None) };
    // What setup writes goes in the first MiB; the rest is a sparse file.
    let mut image = vec![0u8; d.size.min(1 << 20)];
    (d.setup)(&mut image);
    let path = iso.with_extension("disk.img");
    let made = fs::File::create(&path).and_then(|f| {
        use std::io::Write;
        f.set_len(d.size as u64)?;
        (&f).write_all(&image)
    });
    made.map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(Some(path))
}

/// Checks scenario `name`'s disk image after its run.
fn check_disk(name: &str, path: &Path) -> Result {
    let Some(d) = disk_scenario(name) else { return Ok(()) };
    // The first MiB (what checks look at; images can be large).
    let mut image = vec![0u8; d.size.min(1 << 20)];
    fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut image))
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    (d.check)(&image)
}

/// Boots scenario `name`'s earlier boots on its disk (see DiskScenario),
/// keeping each one's serial log as `<iso>.boot<n>.serial.log`.
fn earlier_boots(name: &str, iso: &Path, boot: Boot, timeout: Duration, disk: &Path) -> Result {
    let Some(d) = disk_scenario(name) else { return Ok(()) };
    // R8: earlier boots of another release (its own image, signed so).
    let earlier = match trust::EARLIER_SIGNING.iter().find(|(n, _)| *n == name) {
        Some((_, signing)) => {
            let s = SCENARIOS.iter().find(|s| s.name == name).ok_or("no such scenario")?;
            let release = RELEASE.load(std::sync::atomic::Ordering::Relaxed);
            let cmdline = format!("{} noscreen", s.cmdline);
            let earlier_name = format!("earlier-{name}");
            Some(make_signed_iso(release, &earlier_name, &cmdline, &extra_modules(name), signing)?)
        }
        None => None,
    };
    let result = boot_earlier(d, earlier.as_deref().unwrap_or(iso), iso, boot, timeout, disk);
    if let Some(earlier) = earlier {
        let _ = fs::remove_file(earlier);
    }
    result
}

fn boot_earlier(
    d: &DiskScenario,
    image: &Path,
    iso: &Path,
    boot: Boot,
    timeout: Duration,
    disk: &Path,
) -> Result {
    for (n, expect) in d.earlier_boots.iter().enumerate() {
        let (status, serial, _) = run_vm(image, boot, timeout, None, false, Some(disk), None)?;
        let kept = iso.with_extension(format!("boot{}.serial.log", n + 1));
        let _ = fs::write(&kept, &serial);
        let missing: Vec<&str> = expect.iter().copied().filter(|e| !serial.contains(e)).collect();
        let want = if d.earlier_fail { QEMU_EXIT_FAILURE } else { QEMU_EXIT_SUCCESS };
        if status.and_then(|s| s.code()) != Some(want) || !missing.is_empty() {
            return Err(format!(
                "boot {} of {} failed (lacks {missing:?}; serial log: {})",
                n + 1,
                d.earlier_boots.len() + 1,
                kept.display()
            ));
        }
    }
    Ok(())
}

/// `cargo xtask serve <scenario> [port]`: boots a NET_SCENARIOS entry as
/// the boot test does, but forwards a fixed host port and keeps the VM
/// running (serial log on stdout) until it exits or is killed.
fn serve(release: bool, args: &[String]) -> Result {
    let mut args = args.iter().filter(|a| !a.starts_with("--"));
    let usage = "usage: cargo xtask serve <network scenario> [port]";
    let name = args.next().ok_or(usage)?;
    let port = match args.next() {
        Some(p) => p.parse().map_err(|_| usage)?,
        None => 8080,
    };
    // `serve sshd`: sshd as a server (sshd_server) on host port 2222.
    let (name, port) = match name.as_str() {
        "sshd" => ("sshd_server", if port == 8080 { 2222 } else { port }),
        n => (n, port),
    };
    let s = SCENARIOS
        .iter()
        .find(|s| s.name == name && net_marker(s.name).is_some())
        .ok_or_else(|| format!("{name} is not a network scenario"))?;
    build_kernel(release)?;
    let cmdline = format!("{} noscreen", s.cmdline);
    let iso = make_iso(release, &format!("serve-{name}"), &cmdline, &extra_modules(s.name))?;
    let mut cmd = qemu(&iso, scenario_boot(s.name))?;
    cmd.args(["-serial", "stdio", "-display", "none"]);
    add_net_to(&mut cmd, port, guest_port(s.name));
    eprintln!("xtask: serving {name} on 127.0.0.1:{port}");
    run_cmd(&mut cmd)
}

/// Ports leased to this xtask's running VMs (see `PortLease`).
static LEASED_PORTS: std::sync::Mutex<Vec<u16>> = std::sync::Mutex::new(Vec::new());

/// A host port for one VM's forward (TCP and UDP), held until dropped:
/// no other VM of this xtask gets it meanwhile. Taken below the kernel's
/// ephemeral range (`ip_local_port_range`), from a random start: a port
/// from `bind(0)` was free when asked but is an ephemeral port, which any
/// outgoing connection (another VM's net client, another agent's) could
/// take before QEMU binds it. Another process on the host can still
/// take it first; QEMU then cannot forward it and `run_vm` boots again
/// with another port.
struct PortLease(u16);

impl PortLease {
    fn new() -> Result<Self> {
        let low = fs::read_to_string("/proc/sys/net/ipv4/ip_local_port_range")
            .ok()
            .and_then(|r| r.split_whitespace().next().and_then(|p| p.parse::<u16>().ok()))
            .unwrap_or(32768);
        let (first, span) = if low > 12000 { (low - 12000, 12000u32) } else { (1024, 8000) };
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos())
            ^ std::process::id().wrapping_mul(2_654_435_761);
        let mut leased = LEASED_PORTS.lock().unwrap_or_else(|e| e.into_inner());
        for i in 0..span {
            let port = first + (seed.wrapping_add(i.wrapping_mul(7919)) % span) as u16;
            if leased.contains(&port) {
                continue;
            }
            // Free for TCP and for UDP (both are forwarded).
            if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
                && std::net::UdpSocket::bind(("127.0.0.1", port)).is_ok()
            {
                leased.push(port);
                return Ok(PortLease(port));
            }
        }
        Err(format!("no port in {first}..{} free for both TCP and UDP", u32::from(first) + span))
    }
}

impl Drop for PortLease {
    fn drop(&mut self) {
        let mut leased = LEASED_PORTS.lock().unwrap_or_else(|e| e.into_inner());
        leased.retain(|&p| p != self.0);
        // The net client's files (`client_dir`).
        let _ = fs::remove_dir_all(root().join("target/net-client").join(self.0.to_string()));
    }
}

/// A free host port for `screenshot` (its only VM, for the whole run).
fn free_port() -> Result<u16> {
    let lease = PortLease::new()?;
    let port = lease.0;
    std::mem::forget(lease);
    Ok(port)
}

/// The host side of `net_echo`: sends NET_ECHO_BYTES of a pattern to the
/// guest's port 7 (writing and reading at the same time, so neither side's
/// window fills up), then half-closes and expects the same bytes back.
fn net_client(port: u16) -> Result {
    use std::io::Write;
    let pattern = |i: usize| (i * 7 + i / 251) as u8;
    let mut stream =
        std::net::TcpStream::connect(("127.0.0.1", port)).map_err(|e| format!("connect: {e}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(60))).map_err(|e| e.to_string())?;
    let mut writer = stream.try_clone().map_err(|e| e.to_string())?;
    let sender = thread::spawn(move || -> Result {
        let data: Vec<u8> = (0..NET_ECHO_BYTES).map(pattern).collect();
        for chunk in data.chunks(4096) {
            writer.write_all(chunk).map_err(|e| format!("send: {e}"))?;
        }
        writer.shutdown(std::net::Shutdown::Write).map_err(|e| format!("shutdown: {e}"))
    });
    let mut echoed = Vec::new();
    let read = stream.read_to_end(&mut echoed);
    sender.join().unwrap_or_else(|_| Err("sender panicked".into()))?;
    read.map_err(|e| format!("receive after {} bytes: {e}", echoed.len()))?;
    if echoed.len() != NET_ECHO_BYTES {
        return Err(format!("echoed {} bytes, sent {NET_ECHO_BYTES}", echoed.len()));
    }
    match echoed.iter().enumerate().find(|&(i, &b)| b != pattern(i)) {
        Some((i, b)) => Err(format!("byte {i} is {b:#x}, sent {:#x}", pattern(i))),
        None => Ok(()),
    }
}

/// The host side of the handover_net scenarios (user/handover_net_test):
/// a conversation of numbered lines on one connection before, across and
/// after a kernel handover (the line `handover` asks for it), a second
/// connection opened before the handover that waits in the listener's
/// backlog with its first line unread, a third opened after it. Every
/// answer must come, in order, and no connection may be reset.
fn handover_net_client(port: u16) -> Result {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpStream;
    type Conn = (TcpStream, BufReader<TcpStream>);
    let open = |what: &str| -> Result<Conn> {
        let s =
            TcpStream::connect(("127.0.0.1", port)).map_err(|e| format!("{what}: connect: {e}"))?;
        s.set_read_timeout(Some(Duration::from_secs(60))).map_err(|e| e.to_string())?;
        let r = BufReader::new(s.try_clone().map_err(|e| e.to_string())?);
        Ok((s, r))
    };
    let send = |c: &mut Conn, line: &str| -> Result {
        c.0.write_all(format!("{line}\n").as_bytes()).map_err(|e| format!("send {line:?}: {e}"))
    };
    let expect = |c: &mut Conn, want: &str| -> Result {
        let mut got = String::new();
        c.1.read_line(&mut got).map_err(|e| format!("waiting for {want:?}: {e}"))?;
        if got.trim_end() == want { Ok(()) } else { Err(format!("expected {want:?}, got {got:?}")) }
    };
    let ask = |c: &mut Conn, line: &str, want: &str| -> Result {
        send(c, line)?;
        expect(c, want)
    };
    // The server closes after `bye`: a FIN, never a reset.
    let closed = |c: &mut Conn, what: &str| -> Result {
        let mut rest = String::new();
        match c.1.read_line(&mut rest) {
            Ok(0) => Ok(()),
            Ok(_) => Err(format!("{what}: more after bye: {rest:?}")),
            Err(e) => Err(format!("{what}: closed with an error (reset?): {e}")),
        }
    };
    // Datagrams to the program's UDP port 7 (the same host port, UDP).
    let udp = std::net::UdpSocket::bind("127.0.0.1:0").map_err(|e| format!("udp: {e}"))?;
    udp.connect(("127.0.0.1", port)).map_err(|e| format!("udp: {e}"))?;
    udp.set_read_timeout(Some(Duration::from_secs(30))).map_err(|e| e.to_string())?;
    let mut first = open("first connection")?;
    ask(&mut first, "hello", "1 hello")?;
    ask(&mut first, "before", "2 before")?;
    // The second connection: established, not accepted, a line unread.
    let mut second = open("second connection")?;
    send(&mut second, "early")?;
    // A datagram that waits, unread, in the program's UDP socket across
    // the handover.
    udp.send(b"early-udp").map_err(|e| format!("udp send: {e}"))?;
    thread::sleep(Duration::from_millis(500));
    ask(&mut first, "handover", "3 handover requested")?;
    // Sent at once: it reaches a kernel that is stopping or starting.
    ask(&mut first, "during", "4 during")?;
    for i in 0..10 {
        thread::sleep(Duration::from_millis(300));
        ask(&mut first, &format!("line {i}"), &format!("{} line {i}", 5 + i))?;
    }
    ask(&mut first, "bye", "15 bye")?;
    closed(&mut first, "first connection")?;
    expect(&mut second, "16 early")?;
    ask(&mut second, "bye", "17 bye")?;
    closed(&mut second, "second connection")?;
    let mut third = open("third connection")?;
    ask(&mut third, "after", "18 after")?;
    // One more datagram after the handover; then both are answered.
    udp.send(b"late-udp").map_err(|e| format!("udp send: {e}"))?;
    thread::sleep(Duration::from_millis(500));
    ask(&mut third, "udp", "19 udp 2")?;
    for want in ["udp early-udp", "udp late-udp"] {
        let mut buf = [0u8; 64];
        let n = udp.recv(&mut buf).map_err(|e| format!("waiting for {want:?}: {e}"))?;
        if &buf[..n] != want.as_bytes() {
            return Err(format!("expected {want:?}, got {:?}", String::from_utf8_lossy(&buf[..n])));
        }
    }
    ask(&mut third, "bye", "20 bye")?;
    closed(&mut third, "third connection")
}

/// The host side of `udp_echo` (user/udp_test's `serve`): datagrams of
/// several sizes to the guest's port 7 through QEMU's UDP forward, each
/// echoed whole (sent again while no answer comes: UDP may lose one, and
/// the guest's first answer may wait for ARP); then `dns example.com`,
/// which the guest resolves through QEMU's DNS (10.0.2.3, forwarding to
/// the host's resolver): an IPv4 address when the host resolves the name
/// too, skipped (logged) when the host has no DNS; then `bye`.
fn udp_client(port: u16) -> Result {
    let s = std::net::UdpSocket::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    s.connect(("127.0.0.1", port)).map_err(|e| format!("connect: {e}"))?;
    let timeout =
        |ms| s.set_read_timeout(Some(Duration::from_millis(ms))).map_err(|e| e.to_string());
    timeout(500)?;
    // Sends `data` until an answer equal to `want` comes (answers to
    // earlier tries are skipped).
    let ask = |data: &[u8], want: Option<&[u8]>, tries: usize| -> Result<Vec<u8>> {
        let mut buf = vec![0u8; 4096];
        for _ in 0..tries {
            s.send(data).map_err(|e| format!("send: {e}"))?;
            loop {
                match s.recv(&mut buf) {
                    Ok(n) if want.is_none_or(|w| w == &buf[..n]) => return Ok(buf[..n].to_vec()),
                    Ok(_) => continue,
                    Err(e)
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::WouldBlock
                                | std::io::ErrorKind::TimedOut
                                | std::io::ErrorKind::ConnectionRefused
                        ) =>
                    {
                        break;
                    }
                    Err(e) => return Err(format!("recv: {e}")),
                }
            }
        }
        Err(format!("no answer to a {}-byte datagram", data.len()))
    };
    for (i, n) in [1usize, 2, 100, 512, 1000, 1400].into_iter().enumerate() {
        let data: Vec<u8> = (0..n).map(|j| (j * 7 + i) as u8 | 0x80).collect();
        ask(&data, Some(&data), 40).map_err(|e| format!("{e} (echo)"))?;
    }
    use std::net::ToSocketAddrs;
    let host_resolves = ("example.com", 80).to_socket_addrs().is_ok_and(|mut a| a.next().is_some());
    timeout(20_000)?;
    let answer = loop {
        // Skip late echoes still on their way.
        let a = ask(b"dns example.com", None, 1)?;
        if a.starts_with(b"dns ") {
            break String::from_utf8_lossy(&a).into_owned();
        }
    };
    timeout(500)?;
    let address = answer.strip_prefix("dns ").and_then(|a| a.parse::<std::net::Ipv4Addr>().ok());
    match (address, host_resolves) {
        (Some(_), _) => {}
        (None, false) => {
            eprintln!("xtask: udp_echo: no DNS on the host; the guest said {answer:?}")
        }
        (None, true) => return Err(format!("dns example.com: {answer:?}")),
    }
    // The guest answers `bye` once and ends; if that one answer is lost
    // (UDP through QEMU's user network under load), no later try is
    // answered. The scenario checks the guest's own `udp_test: ok` and
    // its echo count, so a lost last answer is said, not failed.
    if ask(b"bye", Some(b"bye"), 10).is_err() {
        eprintln!("xtask: udp_echo: no answer to bye (the guest may have ended after a lost one)");
    }
    Ok(())
}

/// The host side of an HTTP scenario: GET / with HTTP/1.1, expect 200 and
/// `page` as the body.
fn http_client(port: u16, page: &str) -> Result {
    let (head, body) = http_retrying(port, "/", "")?;
    expect_page(&head, &body, page)
}

/// The Nginx scenarios' client: the page, the page gzip-compressed (zlib)
/// and the page through a rewrite (PCRE2).
fn nginx_client(port: u16) -> Result {
    let (head, body) = http_retrying(port, "/", "")?;
    expect_page(&head, &body, NGINX_PAGE).map_err(|e| format!("GET /: {e}"))?;
    let (head, body) = http_request(port, "/", "Accept-Encoding: gzip\r\n")?;
    if !head.lines().any(|l| l.eq_ignore_ascii_case("Content-Encoding: gzip")) {
        return Err(format!("gzip: no Content-Encoding: gzip in {head:?}"));
    }
    // A gzip member: magic 1f 8b, method 8 (deflate), and a trailer whose
    // last four bytes are the uncompressed length.
    let isize =
        body.len().checked_sub(4).map(|i| u32::from_le_bytes(body[i..].try_into().unwrap()));
    if !body.starts_with(&[0x1f, 0x8b, 8]) || isize != Some(NGINX_PAGE.len() as u32) {
        return Err(format!(
            "gzip: body is not a gzip of the page: {:02x?}",
            &body[..body.len().min(16)]
        ));
    }
    let (head, body) = http_request(port, "/old", "")?;
    expect_page(&head, &body, NGINX_PAGE).map_err(|e| format!("GET /old (rewrite): {e}"))
}

/// The TLS scenario's client: GET / over HTTPS with the host's curl (the
/// certificate is self-signed: -k), retried while the server is not
/// listening yet (curl's connect, TLS-handshake and empty-reply errors).
/// What `ssh_client` asks sshd's session to print.
const SSH_ECHO: &str = "gala-ssh-ok";

/// The host side of `sshd_login`: the host's ssh with the test key
/// user/sshd/client_ed25519 (galatest's authorized key) logs on as
/// `galatest` and runs `echo gala-ssh-ok`; its verbose trace says how far
/// it got.
fn psql_client(port: u16) -> Result {
    let out = Command::new("timeout")
        .arg("120")
        .arg("psql")
        .args(["-h", "127.0.0.1", "-p", &port.to_string(), "-U", "postgres", "-d", "postgres"])
        .args(["-w", "-tAc", "select 40+2"])
        .env("PGCONNECT_TIMEOUT", "60")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("psql: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    if out.status.success() && stdout.trim() == "42" {
        return Ok(());
    }
    Err(format!(
        "psql exit {:?}, stdout {stdout:?}, stderr {:?}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    ))
}

/// One WebDriver command (W3C WebDriver over HTTP/1.1): its status code
/// and body. ChromeDriver checks the Host header (localhost or an IP
/// literal); `timeout` covers commands that start Chromium under TCG.
fn webdriver(
    port: u16,
    method: &str,
    path: &str,
    body: &str,
    timeout: u64,
) -> Result<(u32, String)> {
    use std::io::Write;
    let mut stream =
        std::net::TcpStream::connect(("127.0.0.1", port)).map_err(|e| format!("connect: {e}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(timeout))).map_err(|e| e.to_string())?;
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).map_err(|e| format!("send: {e}"))?;
    // ChromeDriver keeps the connection open whatever the request says:
    // read until the body is complete (its Content-Length, or a chunked
    // body's last chunk), not until the connection closes.
    let mut response = Vec::new();
    let mut buf = [0u8; 16384];
    let (head, mut raw) = loop {
        let n = match stream.read(&mut buf) {
            Ok(0) => 0,
            Ok(n) => n,
            Err(e) if response.is_empty() => {
                return Err(format!("{method} {path}: no response: {e}"));
            }
            Err(e) => return Err(format!("{method} {path}: response cut: {e}")),
        };
        response.extend_from_slice(&buf[..n]);
        if let Some(split) = response.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&response[..split]).into_owned();
            let body = &response[split + 4..];
            let length = head.lines().find_map(|l| {
                let (k, v) = l.split_once(':')?;
                k.trim()
                    .eq_ignore_ascii_case("Content-Length")
                    .then(|| v.trim().parse::<usize>().ok())?
            });
            let chunked =
                head.lines().any(|l| l.eq_ignore_ascii_case("Transfer-Encoding: chunked"));
            let complete = match length {
                Some(len) => body.len() >= len,
                None if chunked => body.ends_with(b"0\r\n\r\n"),
                None => n == 0,
            };
            if complete || n == 0 {
                let mut raw = body.to_vec();
                if let Some(len) = length {
                    raw.truncate(len);
                }
                break (head, raw);
            }
        } else if n == 0 {
            return Err(format!("{method} {path}: no end of headers"));
        }
    };
    if head.lines().any(|l| l.eq_ignore_ascii_case("Transfer-Encoding: chunked")) {
        raw = dechunk(&raw).ok_or("bad chunked body")?;
    }
    let status = head.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    Ok((status, String::from_utf8_lossy(&raw).into_owned()))
}

/// The JSON string value of `"key":"..."` in `body` (no escapes: the
/// values read here have none).
fn json_string<'a>(body: &'a str, key: &str) -> Option<&'a str> {
    let at = body.find(&format!("\"{key}\""))?;
    let rest = body[at + key.len() + 2..].trim_start().strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    Some(&rest[..rest.find('"')?])
}

/// The host side of `chromium_driver`: ChromeDriver's /status, then a
/// session on headless Chromium (software rendering, no sandbox, its
/// own profile), page.html loaded from C:, a script returning its title
/// and its script-written paragraph, and the session ended (Chromium
/// exits). Each step's failure names the step and the driver's answer.
fn webdriver_client(port: u16) -> Result {
    let verdict = webdriver_session(port);
    // ChromeDriver ends (its /shutdown), and with it the run: `dump=`
    // then copies the logs, whatever the verdict.
    let _ = webdriver(port, "GET", "/shutdown", "", 60);
    verdict
}

fn webdriver_session(port: u16) -> Result {
    let mut ready = Err("no answer".to_string());
    for _ in 0..60 {
        ready = webdriver(port, "GET", "/status", "", 60);
        if matches!(ready, Ok((200, ref b)) if b.contains("\"ready\":true")) {
            break;
        }
        thread::sleep(Duration::from_millis(500));
    }
    match ready {
        Ok((200, ref b)) if b.contains("\"ready\":true") => {}
        other => return Err(format!("GET /status: {other:?}")),
    }
    // Stopgap (ROADMAP BR, "Chrome's start-up"): browserStartupTimeout
    // 180 s instead of ChromeDriver's 60 s. Measured: the browser's UI
    // thread answers DevTools' /json/version only after its start-up work
    // (a CPU-bound UI thread: image page faults and relocation, lockdep in
    // the test kernel, Chrome's own code), 40-70 s after launch under TCG
    // with the gate's load. Removed once Chrome starts within 60 s there.
    let capabilities = r#"{"capabilities":{"alwaysMatch":{"goog:chromeOptions":{"browserStartupTimeout":180000,"binary":"C:\\chromium\\chrome.exe","args":["--headless","--no-sandbox","--disable-gpu","--no-first-run","--user-data-dir=C:\\tmp\\chrd","--enable-logging","--log-file=C:\\chromium\\chrome_driven.log","--ipc-connection-timeout=120"]}}}}"#;
    let (status, body) = webdriver(port, "POST", "/session", capabilities, 600)?;
    let Some(id) = json_string(&body, "sessionId").filter(|_| status == 200) else {
        return Err(format!("POST /session: {status} {body}"));
    };
    let id = id.to_string();
    let step = |method: &str, path: &str, body: &str| -> Result<String> {
        let (status, answer) = webdriver(port, method, &format!("/session/{id}{path}"), body, 300)?;
        if status != 200 {
            return Err(format!("{method} /session/{{id}}{path}: {status} {answer}"));
        }
        Ok(answer)
    };
    step("POST", "/url", r#"{"url":"file:///C:/gala/page.html"}"#)?;
    let script = r#"{"script":"return document.title + '|' + document.getElementById('dynamic').textContent","args":[]}"#;
    let answer = step("POST", "/execute/sync", script)?;
    if json_string(&answer, "value") != Some("gala page|V8 ran: 42") {
        return Err(format!("the script's value: {answer}"));
    }
    step("DELETE", "", "")?;
    Ok(())
}

/// The host side of `filezilla_server`: the FTP control connection (RFC
/// 959): the greeting (220), USER gala (230 at once: no password, or 331
/// then PASS), PWD (257 "/"), QUIT (221).
fn ftp_client(port: u16) -> Result {
    use std::io::{BufRead, BufReader, Write};
    let stream = (0..60)
        .find_map(|_| {
            std::net::TcpStream::connect(("127.0.0.1", port))
                .map_err(|_| thread::sleep(Duration::from_millis(500)))
                .ok()
        })
        .ok_or("connect: refused for 30 s")?;
    stream.set_read_timeout(Some(Duration::from_secs(60))).map_err(|e| e.to_string())?;
    let mut writer = stream.try_clone().map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(stream);
    // A reply: its code and last line (multi-line replies end with "ddd ").
    let mut reply = || -> std::result::Result<(u32, String), String> {
        let mut line = String::new();
        loop {
            line.clear();
            if reader.read_line(&mut line).map_err(|e| format!("read: {e}"))? == 0 {
                return Err("connection closed".into());
            }
            let b = line.as_bytes();
            if b.len() >= 4 && b[..3].iter().all(u8::is_ascii_digit) && b[3] == b' ' {
                return Ok((line[..3].parse().unwrap_or(0), line.trim_end().to_string()));
            }
        }
    };
    let (code, text) = reply()?;
    if code != 220 {
        return Err(format!("greeting: {text}"));
    }
    let mut send = |cmd: &str| {
        writer.write_all(format!("{cmd}\r\n").as_bytes()).map_err(|e| format!("send: {e}"))
    };
    send("USER gala")?;
    let (mut code, mut text) = reply()?;
    if code == 331 {
        send("PASS gala")?;
        (code, text) = reply()?;
    }
    if code != 230 {
        return Err(format!("login: {text}"));
    }
    send("PWD")?;
    let (code, text) = reply()?;
    if code != 257 || !text.contains("\"/\"") {
        return Err(format!("PWD: {text}"));
    }
    send("QUIT")?;
    let (code, text) = reply()?;
    if code != 221 {
        return Err(format!("QUIT: {text}"));
    }
    Ok(())
}

/// The host-side net client's own files (ssh's key copy and trace, the
/// sftp transfer, the askpass script) for the VM whose forward is host
/// `port`: VMs that run at once (`XTASK_REPEAT_JOBS`, the suite) have
/// distinct ports, so their clients never share a file (one client
/// rewriting the key while another's ssh read it, one trace written by
/// two). Removed by `boot_vm` after the run.
fn client_dir(port: u16) -> Result<PathBuf> {
    let dir = root().join("target/net-client").join(port.to_string());
    fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    Ok(dir)
}

/// The test key (user/sshd/client_ed25519) as ssh wants it: a 0600 copy
/// (ssh refuses a private key others may read), in `client_dir(port)`.
fn ssh_key(port: u16) -> Result<PathBuf> {
    let key = client_dir(port)?.join("client_ed25519");
    if !key.exists() {
        fs::copy(root().join("user/sshd/client_ed25519"), &key).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&key, fs::Permissions::from_mode(0o600))
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(key)
}

/// galatest's password (user/access_test/access_test.regseed has its hash).
const SSH_PASSWORD: &str = "Gala-Test-1";

/// The host's ssh as galatest with the test key, running `command` on
/// gala; its standard output, or why it failed (the end of its -v trace).
fn ssh_key_run(port: u16, command: &[&str]) -> Result<String> {
    let key = ssh_key(port)?;
    let out = Command::new("timeout")
        .arg("120")
        .arg("ssh")
        .arg("-v")
        .arg("-i")
        .arg(&key)
        .args(["-o", "BatchMode=yes", "-o", "IdentitiesOnly=yes"])
        .args(["-p", &port.to_string()])
        .args(["-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=/dev/null"])
        .args(["-o", "ConnectTimeout=60"])
        .arg("galatest@127.0.0.1")
        .args(command)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("ssh: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    if out.status.success() {
        return Ok(stdout);
    }
    let trace = String::from_utf8_lossy(&out.stderr);
    let mut tail: Vec<&str> = trace.lines().rev().take(25).collect();
    tail.reverse();
    Err(format!("ssh exit {:?}, stdout {stdout:?}:\n{}", out.status.code(), tail.join("\n")))
}

/// The host side of `sshd_shell`: the host's ssh with a pty forced
/// (`-tt`; its standard input is a pipe, so the pty is 80 x 24 or the
/// server's default) plays a terminal, expect-style: what it types goes to
/// the shell as a terminal sends it (CR for Enter, DEL for Backspace, 0x03
/// for Ctrl+C), what the shell prints comes back as VT text. Checks: a
/// command runs (`echo gala-$((6*7))` prints gala-42, which the typed
/// line does not contain), DEL edits the line before Enter, Ctrl+C ends a
/// running `sleep 60` and the prompt comes back, `exit` ends the session.
fn ssh_shell_client(port: u16) -> Result {
    use std::io::{Read, Write};
    use std::sync::{Arc, Mutex};
    let key = ssh_key(port)?;
    let trace = client_dir(port)?.join("ssh.trace");
    let mut ssh = Command::new("timeout")
        .arg("300")
        .arg("ssh")
        .arg("-tt")
        .arg("-v")
        .arg("-i")
        .arg(&key)
        .args(["-o", "BatchMode=yes", "-o", "IdentitiesOnly=yes"])
        .args(["-p", &port.to_string()])
        .args(["-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=/dev/null"])
        .args(["-o", "ConnectTimeout=60"])
        .arg("galatest@127.0.0.1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(fs::File::create(&trace).map_err(|e| e.to_string())?)
        .spawn()
        .map_err(|e| format!("ssh: {e}"))?;
    let mut input = ssh.stdin.take().ok_or("ssh: no stdin")?;
    let mut output = ssh.stdout.take().ok_or("ssh: no stdout")?;
    let seen = Arc::new(Mutex::new(Vec::<u8>::new()));
    let reader = {
        let seen = seen.clone();
        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = output.read(&mut buf) {
                if n == 0 {
                    break;
                }
                seen.lock().unwrap_or_else(|e| e.into_inner()).extend_from_slice(&buf[..n]);
            }
        })
    };
    let len = || seen.lock().unwrap_or_else(|e| e.into_inner()).len();
    // Waits for `want` after offset `from`; where it ends.
    let expect = |want: &str, from: usize, secs: u64| -> Result<usize> {
        let until = Instant::now() + Duration::from_secs(secs);
        loop {
            {
                let all = seen.lock().unwrap_or_else(|e| e.into_inner());
                let hay = &all[from.min(all.len())..];
                if let Some(i) = hay.windows(want.len()).position(|w| w == want.as_bytes()) {
                    return Ok(from + i + want.len());
                }
            }
            if Instant::now() > until {
                return Err(format!("no {want:?} within {secs} s"));
            }
            thread::sleep(Duration::from_millis(50));
        }
    };
    let mut steps = || -> Result {
        let mut send = |bytes: &[u8]| input.write_all(bytes).map_err(|e| format!("send: {e}"));
        // The shell is up when it has printed something (its prompt).
        let until = Instant::now() + Duration::from_secs(90);
        while len() == 0 {
            if Instant::now() > until {
                return Err("no prompt within 90 s".into());
            }
            thread::sleep(Duration::from_millis(100));
        }
        thread::sleep(Duration::from_millis(500));
        let at = len();
        send(b"echo gala-$((6*7))\r")?;
        let at = expect("gala-42\r\n", at, 30).map_err(|e| format!("a command: {e}"))?;
        // DEL takes the X back before Enter.
        send(b"echo gala-$((40+3))X\x7f\r")?;
        let at = expect("gala-43\r\n", at, 30).map_err(|e| format!("line editing: {e}"))?;
        // Ctrl+C ends a running command; the shell goes on.
        send(b"sleep 60\r")?;
        thread::sleep(Duration::from_secs(3));
        let started = Instant::now();
        send(b"\x03")?;
        thread::sleep(Duration::from_secs(1));
        // Typed once: the shell's control handler takes Ctrl+C while
        // `sleep` runs (a late one, a lost wake-up, dropped this line:
        // ARCHITECTURE "Waits").
        send(b"echo gala-$((50-6))\r")?;
        let at = expect("gala-44\r\n", at, 30).map_err(|e| format!("Ctrl+C: {e}"))?;
        if started.elapsed() > Duration::from_secs(25) {
            return Err("Ctrl+C: the prompt came back only after sleep ended".into());
        }
        let _ = at;
        send(b"exit\r")?;
        let until = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = ssh.try_wait().map_err(|e| e.to_string())? {
                return if status.success() {
                    Ok(())
                } else {
                    Err(format!("exit: ssh ended with {status}"))
                };
            }
            if Instant::now() > until {
                return Err("exit: the session did not end within 30 s".into());
            }
            thread::sleep(Duration::from_millis(100));
        }
    };
    let result = steps();
    let _ = ssh.kill();
    let _ = reader.join();
    result.map_err(|e| {
        // What the terminal showed (escaped) and the end of ssh's trace.
        let all = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let shown: String = all[all.len().saturating_sub(1500)..]
            .iter()
            .map(|&b| match b {
                b'\n' => "\\n\n".to_string(),
                0x20..=0x7e => (b as char).to_string(),
                _ => format!("\\x{b:02x}"),
            })
            .collect();
        let trace = fs::read_to_string(&trace).unwrap_or_default();
        let mut tail: Vec<&str> = trace.lines().rev().take(15).collect();
        tail.reverse();
        format!("{e}\nterminal:\n{shown}\nssh -v:\n{}", tail.join("\n"))
    })
}

/// The host side of `sshd_sftp`: the host's sftp in batch mode with the
/// test key: `put` a 200 KiB file, `ls`, `get` it back, `rm` it; the
/// download must equal the upload and the listing name the file.
fn sftp_client(port: u16) -> Result {
    let key = ssh_key(port)?;
    let dir = client_dir(port)?.join("sftp");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let data: Vec<u8> = (0..200 * 1024u32).map(|i| (i * 31 + i / 7) as u8).collect();
    fs::write(dir.join("up.bin"), &data).map_err(|e| e.to_string())?;
    fs::write(
        dir.join("batch"),
        "put up.bin gala-up.bin\nls -l\nget gala-up.bin down.bin\nrm gala-up.bin\nls\n",
    )
    .map_err(|e| e.to_string())?;
    let out = Command::new("timeout")
        .arg("180")
        .arg("sftp")
        .arg("-v")
        .arg("-i")
        .arg(&key)
        .args(["-o", "BatchMode=yes", "-o", "IdentitiesOnly=yes"])
        .args(["-P", &port.to_string()])
        .args(["-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=/dev/null"])
        .args(["-o", "ConnectTimeout=60"])
        .arg("-b")
        .arg("batch")
        .arg("galatest@127.0.0.1")
        .current_dir(&dir)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("sftp: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let fail = |what: String| {
        let trace = String::from_utf8_lossy(&out.stderr);
        let mut tail: Vec<&str> = trace.lines().rev().take(20).collect();
        tail.reverse();
        Err(format!("{what}; stdout {stdout:?}:\n{}", tail.join("\n")))
    };
    if !out.status.success() {
        // What the users' shell sees (the subsystem runs through it).
        let seen = ssh_key_run(port, &["echo PATH=$PATH; command -v sftp-server.exe"])
            .unwrap_or_else(|e| e);
        return fail(format!("sftp exit {:?} (the shell: {seen:?})", out.status.code()));
    }
    if !stdout.contains("gala-up.bin") {
        return fail("the listing has no gala-up.bin".into());
    }
    match fs::read(dir.join("down.bin")) {
        Ok(got) if got == data => Ok(()),
        Ok(got) => fail(format!("downloaded {} bytes, not the {} uploaded", got.len(), data.len())),
        Err(e) => fail(format!("no download: {e}")),
    }
}

/// The host's ssh (or scp, `program`) with the test key and the options
/// every ssh client here uses; the caller adds the destination.
fn ssh_tool(program: &str, port: u16, key: &Path, secs: u32) -> Command {
    let mut cmd = Command::new("timeout");
    cmd.arg(secs.to_string())
        .arg(program)
        .arg("-v")
        .arg("-i")
        .arg(key)
        .args(["-o", "BatchMode=yes", "-o", "IdentitiesOnly=yes"])
        .args([if program == "scp" { "-P" } else { "-p" }, &port.to_string()])
        .args(["-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=/dev/null"])
        .args(["-o", "ConnectTimeout=60"])
        .stdin(Stdio::null());
    cmd
}

/// The last `lines` lines of a client's `-v` trace.
fn trace_tail(trace: &[u8], lines: usize) -> String {
    let trace = String::from_utf8_lossy(trace);
    let mut tail: Vec<&str> = trace.lines().rev().take(lines).collect();
    tail.reverse();
    tail.join("\n")
}

/// The times `scp_client` gives its file (2021-03-04 05:06:07 UTC and
/// 2020-09-13 12:26:40 UTC): far from now, so a copy that drops them shows.
const SCP_MTIME: u64 = 1_614_834_367;
const SCP_ATIME: u64 = 1_600_000_000;

/// The host side of `sshd_scp`: the host's scp -p copies a 300 KiB file
/// with fixed times to gala and back, over SFTP (scp's default) and over
/// the legacy protocol (`-O`). Checks: gala's `stat` sees the upload's size
/// and both times (scp -p sets them with SFTP SETSTAT or the legacy `T`
/// line, i.e. SetFileTime there), the download equals the upload byte for
/// byte and has the modification time gala holds (the one sent).
fn scp_client(port: u16) -> Result {
    let key = ssh_key(port)?;
    let dir = client_dir(port)?.join("scp");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let data: Vec<u8> = (0..300 * 1024u32).map(|i| (i * 7 + i / 251) as u8).collect();
    let up = dir.join("up.bin");
    fs::write(&up, &data).map_err(|e| e.to_string())?;
    // Set before each upload: reading the file moves the host's access
    // time (relatime: it is older than the modification time).
    let set_times = || -> Result {
        let at = |secs: u64| std::time::UNIX_EPOCH + Duration::from_secs(secs);
        let times = fs::FileTimes::new().set_modified(at(SCP_MTIME)).set_accessed(at(SCP_ATIME));
        fs::File::options()
            .write(true)
            .open(&up)
            .and_then(|f| f.set_times(times))
            .map_err(|e| format!("setting up.bin's times: {e}"))
    };
    let scp = |legacy: bool, from: &str, to: &str| -> Result {
        let mut cmd = ssh_tool("scp", port, &key, 120);
        if legacy {
            cmd.arg("-O");
        }
        let out = cmd
            .arg("-p")
            .args([from, to])
            .current_dir(&dir)
            .output()
            .map_err(|e| format!("scp: {e}"))?;
        if out.status.success() {
            return Ok(());
        }
        Err(format!(
            "scp {from} {to}: exit {:?}, stdout {:?}:\n{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            trace_tail(&out.stderr, 20)
        ))
    };
    for (mode, legacy) in [("sftp", false), ("legacy", true)] {
        let remote = format!("gala-scp-{mode}.bin");
        let down = format!("down-{mode}.bin");
        set_times()?;
        scp(legacy, "up.bin", &format!("galatest@127.0.0.1:{remote}"))
            .map_err(|e| format!("{mode} upload: {e}"))?;
        let seen = ssh_key_run(port, &[&format!("stat -c '%s %Y %X' {remote}")])
            .map_err(|e| format!("{mode} upload: stat: {e}"))?;
        let want = format!("{} {SCP_MTIME} {SCP_ATIME}", data.len());
        if seen.trim() != want {
            return Err(format!(
                "{mode} upload: gala's stat says {:?} (size, modified, accessed), not {want:?}",
                seen.trim()
            ));
        }
        scp(legacy, &format!("galatest@127.0.0.1:{remote}"), &down)
            .map_err(|e| format!("{mode} download: {e}"))?;
        let got = fs::read(dir.join(&down)).map_err(|e| format!("{mode} download: {e}"))?;
        if got != data {
            let first = got.iter().zip(&data).position(|(a, b)| a != b);
            return Err(format!(
                "{mode} download: {} bytes, not the {} uploaded (first difference at {first:?})",
                got.len(),
                data.len()
            ));
        }
        let modified = fs::metadata(dir.join(&down))
            .and_then(|m| m.modified())
            .map_err(|e| e.to_string())?
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        if modified != SCP_MTIME {
            return Err(format!(
                "{mode} download: modified at {modified}, not {SCP_MTIME} (scp -p keeps gala's time)"
            ));
        }
        ssh_key_run(port, &[&format!("rm {remote}")])
            .map_err(|e| format!("{mode}: removing {remote}: {e}"))?;
    }
    Ok(())
}

/// sshfwd_test's port for `ssh -L` (it serves there) and sshd's for
/// `ssh -R` (sshfwd_test connects there), inside gala.
const FORWARD_LOCAL_PORT: u16 = 7000;
const FORWARD_REMOTE_PORT: u16 = 7001;
/// The bytes each direction of `ssh_forward_client` carries.
const FORWARD_BYTES: usize = 200 * 1024;

/// Which leg of `ssh -L` lost bytes, from what sshfwd_test said it
/// served: on the way in (the host's ssh, sshd-session's channel or gala's
/// loopback connection to sshfwd_test; the scenario's `events=net` has
/// each connection's byte counts at its shutdown and close) or on the way
/// back.
fn forward_leg(said: &[String]) -> String {
    let served = said.iter().find_map(|l| {
        l.split("sshfwd_test: served ").nth(1)?.split(' ').next()?.parse::<usize>().ok()
    });
    match served {
        Some(n) if n < FORWARD_BYTES => format!(
            " (sshfwd_test received {n} of {FORWARD_BYTES}: lost on the way in, before \
             sshfwd_test; the tcp shutdown/close events count each connection's bytes)"
        ),
        Some(n) => format!(" (sshfwd_test received and echoed {n}: lost on the way back)"),
        None => String::new(),
    }
}

/// The host side of `sshd_forward`. `ssh -L <socket>:127.0.0.1:7000`
/// whose command starts `sshfwd_test serve 7000` in gala: once it says it
/// listens, the host writes FORWARD_BYTES to the local socket, shuts its
/// side down and must read them all back (sshd-session's direct-tcpip
/// channel connects over gala's loopback). Then `ssh -R
/// 7001:127.0.0.1:<echo>` whose command runs `sshfwd_test connect 7001`:
/// sshd-session listens on gala's 127.0.0.1:7001 (tcpip-forward), and what
/// sshfwd_test sends there reaches an echo server on the host and comes
/// back (sshfwd_test compares it).
fn ssh_forward_client(port: u16) -> Result {
    use std::io::{BufRead, BufReader, Read, Write};
    let key = ssh_key(port)?;
    let dir = client_dir(port)?;
    let data: Vec<u8> = (0..FORWARD_BYTES).map(|i| (i * 31 + i / 7) as u8).collect();

    // -L: the local end is a Unix socket (no host port to lease).
    let sock = dir.join("forward.sock");
    let _ = fs::remove_file(&sock);
    let trace = dir.join("ssh-L.trace");
    let mut ssh = ssh_tool("ssh", port, &key, 180)
        .args(["-o", "ExitOnForwardFailure=yes", "-L"])
        .arg(format!("{}:127.0.0.1:{FORWARD_LOCAL_PORT}", sock.display()))
        .arg("galatest@127.0.0.1")
        .arg(format!("C:/gala/sshfwd_test.exe serve {FORWARD_LOCAL_PORT}"))
        .stdout(Stdio::piped())
        .stderr(fs::File::create(&trace).map_err(|e| e.to_string())?)
        .spawn()
        .map_err(|e| format!("ssh -L: {e}"))?;
    let stdout = ssh.stdout.take().ok_or("ssh -L: no stdout")?;
    let (lines_tx, lines_rx) = std::sync::mpsc::channel::<String>();
    let reader = thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(std::result::Result::ok) {
            let _ = lines_tx.send(line);
        }
    });
    let mut said = Vec::new();
    let local = (|| -> Result {
        // sshfwd_test listens once it says so.
        let until = Instant::now() + Duration::from_secs(90);
        loop {
            let left = until.saturating_duration_since(Instant::now());
            match lines_rx.recv_timeout(left) {
                Ok(line) => {
                    let listening = line.contains("sshfwd_test: listening on 127.0.0.1:");
                    said.push(line);
                    if listening {
                        break;
                    }
                }
                Err(_) => return Err("sshfwd_test did not say it listens within 90 s".into()),
            }
        }
        let stream = std::os::unix::net::UnixStream::connect(&sock)
            .map_err(|e| format!("connecting to ssh's local socket: {e}"))?;
        stream.set_read_timeout(Some(Duration::from_secs(60))).map_err(|e| e.to_string())?;
        let mut writer = stream.try_clone().map_err(|e| e.to_string())?;
        let sent = data.clone();
        let sender = thread::spawn(move || -> Result {
            writer.write_all(&sent).map_err(|e| format!("send: {e}"))?;
            writer.shutdown(std::net::Shutdown::Write).map_err(|e| format!("shutdown: {e}"))
        });
        let mut back = Vec::new();
        let read = (&stream).read_to_end(&mut back);
        sender.join().unwrap_or_else(|_| Err("sender panicked".into()))?;
        read.map_err(|e| format!("receive after {} bytes: {e}", back.len()))?;
        if back != data {
            let first = back.iter().zip(&data).position(|(a, b)| a != b);
            return Err(format!(
                "{} bytes came back, not the {} sent (first difference at {first:?})",
                back.len(),
                data.len()
            ));
        }
        // The command ends once its connection has; so does ssh.
        let until = Instant::now() + Duration::from_secs(60);
        loop {
            if let Some(status) = ssh.try_wait().map_err(|e| e.to_string())? {
                if !status.success() {
                    return Err(format!("ssh ended with {status}"));
                }
                break;
            }
            if Instant::now() > until {
                return Err("ssh did not end within 60 s of the connection's end".into());
            }
            thread::sleep(Duration::from_millis(100));
        }
        Ok(())
    })();
    let _ = ssh.kill();
    let _ = ssh.wait();
    let _ = reader.join();
    said.extend(lines_rx.try_iter());
    let _ = fs::remove_file(&sock);
    let served = format!("sshfwd_test: served {FORWARD_BYTES} bytes to 127.0.0.1");
    let local = local.and_then(|()| {
        if said.iter().any(|l| l.contains(&served))
            && said.iter().any(|l| l.contains("sshfwd_test: ok"))
        {
            Ok(())
        } else {
            Err(format!("sshfwd_test did not say {served:?}"))
        }
    });
    local.map_err(|e| {
        let trace = fs::read(&trace).unwrap_or_default();
        format!(
            "ssh -L: {e}{}; sshfwd_test said {said:?}\nssh -v:\n{}",
            forward_leg(&said),
            trace_tail(&trace, 20)
        )
    })?;

    // -R: an echo server on the host for sshd's forwarded-tcpip channel.
    let echo = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| format!("echo: {e}"))?;
    let echo_port = echo.local_addr().map_err(|e| e.to_string())?.port();
    let echoer = thread::spawn(move || -> Result<usize> {
        let (mut conn, _) = echo.accept().map_err(|e| format!("echo accept: {e}"))?;
        conn.set_read_timeout(Some(Duration::from_secs(60))).map_err(|e| e.to_string())?;
        let mut buf = [0u8; 16384];
        let mut total = 0;
        loop {
            let n = conn.read(&mut buf).map_err(|e| format!("echo read after {total}: {e}"))?;
            if n == 0 {
                break;
            }
            conn.write_all(&buf[..n]).map_err(|e| format!("echo write: {e}"))?;
            total += n;
        }
        conn.shutdown(std::net::Shutdown::Write).map_err(|e| format!("echo shutdown: {e}"))?;
        Ok(total)
    });
    let out = ssh_tool("ssh", port, &key, 180)
        .args(["-o", "ExitOnForwardFailure=yes", "-R"])
        .arg(format!("{FORWARD_REMOTE_PORT}:127.0.0.1:{echo_port}"))
        .arg("galatest@127.0.0.1")
        .arg(format!("C:/gala/sshfwd_test.exe connect {FORWARD_REMOTE_PORT} {FORWARD_BYTES}"))
        .output()
        .map_err(|e| format!("ssh -R: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    if !echoer.is_finished() {
        // Nothing was forwarded: end the echo server's accept.
        let _ = std::net::TcpStream::connect(("127.0.0.1", echo_port));
    }
    let echoed = echoer.join().unwrap_or_else(|_| Err("echo server panicked".into()));
    let forwarded = format!(
        "sshfwd_test: forwarded {FORWARD_BYTES} bytes through 127.0.0.1:{FORWARD_REMOTE_PORT}"
    );
    let remote = if !out.status.success() {
        Err(format!("ssh exit {:?}", out.status.code()))
    } else if !stdout.contains(&forwarded) {
        Err(format!("sshfwd_test did not say {forwarded:?}"))
    } else {
        match echoed {
            Ok(n) if n == FORWARD_BYTES => Ok(()),
            Ok(n) => Err(format!("the host's echo server saw {n} bytes")),
            Err(e) => Err(e),
        }
    };
    remote.map_err(|e| {
        format!("ssh -R: {e}; stdout {stdout:?}\nssh -v:\n{}", trace_tail(&out.stderr, 20))
    })
}

/// The host side of `sshd_client`: the host's ssh runs gala's ssh.exe in
/// gala, which logs on to gala's sshd at 127.0.0.1 as galatest with the key
/// in galatest's profile and runs `echo`. The first login accepts the host
/// key (StrictHostKeyChecking=accept-new writes ~/.ssh/known_hosts), the
/// second must find it there (StrictHostKeyChecking=yes); known_hosts must
/// then name 127.0.0.1's ed25519 key. A failing ssh.exe's `-v` log (`-E`)
/// is printed in its place.
fn ssh_inner_client(port: u16) -> Result {
    for (i, check) in [(1, "accept-new"), (2, "yes")] {
        let word = format!("gala-inner-{i}");
        let line = format!(
            // Single quotes: sshd hands the command to `sh -c` in double ones.
            "'C:/Program Files/OpenSSH/ssh.exe' -v -E inner-{i}.log \
             -i C:/Users/galatest/.ssh/client_ed25519 -o BatchMode=yes \
             -o StrictHostKeyChecking={check} galatest@127.0.0.1 echo {word}; \
             s=$?; [ $s = 0 ] || tail -n 30 inner-{i}.log; exit $s"
        );
        let out = ssh_key_run(port, &[&line]).map_err(|e| format!("gala's ssh, login {i}: {e}"))?;
        if !out.contains(&word) {
            return Err(format!("gala's ssh, login {i}: stdout {out:?}"));
        }
    }
    let known = ssh_key_run(port, &["cat C:/Users/galatest/.ssh/known_hosts"])
        .map_err(|e| format!("reading known_hosts: {e}"))?;
    if !known.lines().any(|l| l.starts_with("127.0.0.1 ssh-ed25519 ")) {
        return Err(format!("known_hosts has no 127.0.0.1 ssh-ed25519 line: {known:?}"));
    }
    Ok(())
}

/// The host side of `sshd_server`: three logins one after another, each
/// running a command, then two at once (both connected before either
/// ends: each sleeps a second), all through the one sshd.
fn ssh_server_client(port: u16) -> Result {
    for i in 1..=3 {
        let word = format!("gala-ssh-{i}");
        let out = ssh_key_run(port, &["echo", &word]).map_err(|e| format!("login {i}: {e}"))?;
        if !out.contains(&word) {
            return Err(format!("login {i}: stdout {out:?}"));
        }
    }
    let together: Vec<_> = (1..=2)
        .map(|i| {
            thread::spawn(move || {
                let word = format!("gala-ssh-together-{i}");
                let line = format!("sleep 1; echo {word}");
                let out =
                    ssh_key_run(port, &[&line]).map_err(|e| format!("login together {i}: {e}"))?;
                if out.contains(&word) {
                    Ok(())
                } else {
                    Err(format!("login together {i}: stdout {out:?}"))
                }
            })
        })
        .collect();
    for t in together {
        t.join().unwrap_or_else(|_| Err("ssh thread panicked".into()))?;
    }
    Ok(())
}

fn ssh_client(port: u16, password: bool) -> Result {
    let mut ssh = Command::new("timeout");
    ssh.arg("120").arg("ssh").arg("-v");
    if password {
        // ssh asks SSH_ASKPASS (forced) for the password: a script.
        let askpass = client_dir(port)?.join("askpass");
        fs::write(&askpass, format!("#!/bin/sh\necho '{SSH_PASSWORD}'\n"))
            .map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&askpass, fs::Permissions::from_mode(0o700))
                .map_err(|e| e.to_string())?;
        }
        ssh.env("SSH_ASKPASS", &askpass)
            .env("SSH_ASKPASS_REQUIRE", "force")
            .env("DISPLAY", ":0")
            .stdin(std::process::Stdio::null())
            .args(["-o", "PubkeyAuthentication=no", "-o", "PreferredAuthentications=password"])
            .args(["-o", "NumberOfPasswordPrompts=1"]);
    } else {
        // ssh refuses a private key others may read: a 0600 copy.
        let key = ssh_key(port)?;
        ssh.arg("-i").arg(&key).args(["-o", "BatchMode=yes", "-o", "IdentitiesOnly=yes"]);
    }
    let out = ssh
        .args(["-p", &port.to_string()])
        .args(["-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=/dev/null"])
        .args(["-o", "ConnectTimeout=60"])
        .args(["galatest@127.0.0.1", "echo", SSH_ECHO])
        .output()
        .map_err(|e| format!("ssh: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    if out.status.success() && stdout.contains(SSH_ECHO) {
        return Ok(());
    }
    let trace = String::from_utf8_lossy(&out.stderr);
    let mut tail: Vec<&str> = trace.lines().rev().take(25).collect();
    tail.reverse();
    Err(format!("ssh exit {:?}, stdout {stdout:?}:\n{}", out.status.code(), tail.join("\n")))
}

fn tls_client(port: u16) -> Result {
    let url = format!("https://127.0.0.1:{port}/");
    let mut last = String::new();
    for _ in 0..50 {
        let out = Command::new("curl")
            .args(["-skv", "--max-time", "60", "--noproxy", "*", &url])
            .output()
            .map_err(|e| format!("curl: {e}"))?;
        match out.status.code() {
            Some(0) => {
                return if out.stdout == NGINX_PAGE.as_bytes() {
                    Ok(())
                } else {
                    Err(format!("body {:?}", String::from_utf8_lossy(&out.stdout)))
                };
            }
            // Could not connect, TLS handshake failed, empty reply, reset.
            Some(code @ (7 | 35 | 52 | 56)) => {
                last = format!("curl exit {code}: {}", String::from_utf8_lossy(&out.stderr));
                thread::sleep(Duration::from_millis(200));
            }
            code => {
                // The end of curl's trace (-v) says where TLS stopped.
                let trace = String::from_utf8_lossy(&out.stderr);
                let mut tail: Vec<&str> = trace.lines().rev().take(12).collect();
                tail.reverse();
                return Err(format!("curl exit {code:?}:\n{}", tail.join("\n")));
            }
        }
    }
    Err(last)
}

/// GET / and expect a 200 whose body contains each of `parts`.
fn contains_client(port: u16, parts: &[&str]) -> Result {
    let (head, body) = http_retrying(port, "/", "")?;
    if !head.starts_with("HTTP/1.1 200 OK") {
        return Err(format!("status line {:?}", head.lines().next()));
    }
    let text = String::from_utf8_lossy(&body);
    match parts.iter().find(|p| !text.contains(*p)) {
        Some(p) => Err(format!("body lacks {p:?}: {text:?}")),
        None => Ok(()),
    }
}

fn expect_page(head: &str, body: &[u8], page: &str) -> Result {
    if !head.starts_with("HTTP/1.1 200 OK") {
        return Err(format!("status line {:?}", head.lines().next()));
    }
    if body != page.as_bytes() {
        return Err(format!("body {:?}", String::from_utf8_lossy(body)));
    }
    Ok(())
}

/// `http_request`, retried while nothing at all comes back, for up to
/// 10 s: a server may log that it started before it listens (Nginx's
/// master logs before its worker opens the socket).
fn http_retrying(port: u16, path: &str, headers: &str) -> Result<(String, Vec<u8>)> {
    let mut last = String::new();
    for _ in 0..50 {
        match http_request(port, path, headers) {
            Err(e) if e.starts_with("no response") => {
                last = e;
                thread::sleep(Duration::from_millis(200));
            }
            other => return other,
        }
    }
    Err(last)
}

/// One HTTP/1.1 GET (`headers`: extra header lines, CRLF-terminated);
/// the response head and the body (de-chunked). "no response: ..." if the
/// connection gave nothing back.
fn http_request(port: u16, path: &str, headers: &str) -> Result<(String, Vec<u8>)> {
    use std::io::Write;
    let mut stream =
        std::net::TcpStream::connect(("127.0.0.1", port)).map_err(|e| format!("connect: {e}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(60))).map_err(|e| e.to_string())?;
    let request =
        format!("GET {path} HTTP/1.1\r\nHost: gala\r\n{headers}Connection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).map_err(|e| format!("send: {e}"))?;
    let mut response = Vec::new();
    let read = stream.read_to_end(&mut response);
    if response.is_empty() {
        let why = read.err().map_or("closed".to_string(), |e| e.to_string());
        return Err(format!("no response: {why}"));
    }
    read.map_err(|e| format!("receive: {e}"))?;
    let split = response.windows(4).position(|w| w == b"\r\n\r\n").ok_or("no end of headers")?;
    let head = String::from_utf8_lossy(&response[..split]).into_owned();
    let mut body = response[split + 4..].to_vec();
    if head.lines().any(|l| l.eq_ignore_ascii_case("Transfer-Encoding: chunked")) {
        body = dechunk(&body).ok_or("bad chunked body")?;
    }
    Ok((head, body))
}

/// The data of a chunked body (sizes in hex, each chunk CRLF-terminated).
fn dechunk(mut b: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let line_end = b.windows(2).position(|w| w == b"\r\n")?;
        let size_text = std::str::from_utf8(&b[..line_end]).ok()?;
        let size = usize::from_str_radix(size_text.split(';').next()?.trim(), 16).ok()?;
        b = &b[line_end + 2..];
        if size == 0 {
            return Some(out);
        }
        out.extend_from_slice(b.get(..size)?);
        b = b.get(size + 2..)?;
    }
}

/// Judges one scenario from the exit status and its part of the log.
/// Judges a run; on failure the serial log is kept in
/// `target/failed/<scenario>.log` (rare failures need it), with its
/// backtraces symbolized now (a later build moves the addresses).
fn judge(
    scenario: &Scenario,
    status: Option<i32>,
    timed_out: bool,
    serial: &str,
    run: &RunFacts,
) -> Result {
    judge_log(scenario, status, timed_out, serial).map_err(|e| {
        let dir = target_dir().join("failed");
        // A repeat's boot (`test-<name>.r<i>.iso`) keeps its own: boots
        // at once must not overwrite each other's.
        let name = match run.events.as_deref().and_then(repeat_index) {
            Some(i) => format!("{}.r{i}", scenario.name),
            None => scenario.name.to_string(),
        };
        let path = dir.join(format!("{name}.log"));
        let symbols = symbolize(RELEASE.load(std::sync::atomic::Ordering::Relaxed), serial);
        let events = run.events.as_deref().and_then(|p| fs::read_to_string(p).ok());
        // The event stream (when it was on) next to the log.
        if let Some(text) = events.as_deref().filter(|t| !t.is_empty()) {
            let _ = fs::create_dir_all(&dir)
                .and_then(|()| fs::write(dir.join(format!("{name}.events.jsonl")), text));
        }
        let facts = triage::Facts {
            scenario: scenario.name,
            verdict: Some(&e),
            secs: run.secs,
            host: Some(host_facts()),
        };
        let summary = triage::triage(serial, events.as_deref(), &facts);
        let kept = format!("{serial}\n{symbols}\n{summary}");
        match fs::create_dir_all(&dir).and_then(|()| fs::write(&path, kept)) {
            Ok(()) => format!("{e} (serial log: {}; it ends with a triage)", path.display()),
            Err(_) => e,
        }
    })
}

/// A serial log as text. Not `read_to_string`: a program's bytes need
/// not be UTF-8, and one invalid byte would make the whole log read as
/// empty (a failed boot's kept log then holds only xtask's lines); invalid
/// bytes become U+FFFD instead.
fn read_log(path: &Path) -> String {
    fs::read(path).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default()
}

/// The repeat number of a file of an `XTASK_REPEAT` boot
/// (`test-<name>.r<i>.<ext>`).
fn repeat_index(path: &Path) -> Option<usize> {
    let name = path.file_name()?.to_str()?;
    name.split('.').find_map(|part| part.strip_prefix('r')?.parse().ok())
}

/// What xtask knows of a boot besides its log (for the triage).
#[derive(Default)]
struct RunFacts {
    events: Option<PathBuf>,
    secs: Option<f64>,
}

/// The host as a failed log should say: processors, load, free disk.
fn host_facts() -> String {
    let cpus = thread::available_parallelism().map_or(0, |n| n.get());
    let load = fs::read_to_string("/proc/loadavg")
        .map(|l| l.split_whitespace().take(3).collect::<Vec<_>>().join(" "))
        .unwrap_or_else(|_| "?".into());
    let disk = free_disk_mib(&target_dir())
        .map_or("?".into(), |m| format!("{:.1} GiB", m as f64 / 1024.0));
    format!("{cpus} CPUs, load {load}, {disk} free on the target disk")
}

/// The kernel build a boot ran: profile and the checkout's commit.
fn kernel_build() -> String {
    let release = RELEASE.load(std::sync::atomic::Ordering::Relaxed);
    let commit = Command::new("git")
        .args(["describe", "--always", "--dirty"])
        .current_dir(root())
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    format!("{} {commit}", if release { "release" } else { "debug" })
}

/// Converts a boot that just ended to OTLP/JSON (`otel_export_at`).
fn otel_export(
    serial: &str,
    events: &str,
    scenario: &str,
    prefix: &Path,
    endpoint: Option<&str>,
) -> Result<String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    otel_export_at(serial, events, scenario, prefix, endpoint, now)
}

/// Converts a boot to OTLP/JSON: `<prefix>.traces.json` (one request per
/// line, `otel::SPANS_PER_REQUEST` spans each), `<prefix>.logs.json`,
/// `<prefix>.metrics.json`; posts them to `endpoint` when given. The
/// boot's time 0 in wall time is `end_unix_ns` minus its last event's
/// time. Returns the report (`otel:` lines) and where the files went.
fn otel_export_at(
    serial: &str,
    events: &str,
    scenario: &str,
    prefix: &Path,
    endpoint: Option<&str>,
    end_unix_ns: u64,
) -> Result<String> {
    let last = events
        .lines()
        .filter_map(|l| triage::field(l, "t").and_then(|t| t.parse::<u64>().ok()))
        .max()
        .unwrap_or(0);
    let build = kernel_build();
    // Function names for the profile: the kernel's from its ELF, a
    // program's from `XTASK_SYMBOLS=<dir>` (its modules and symbols, e.g.
    // target/prebuilt/chromium-syms).
    let kernel = target_dir().join("x86_64-unknown-none/debug/kernel");
    let dir = env::var_os("XTASK_SYMBOLS").map(PathBuf::from);
    let symbolize = |module: &str, offsets: &[u64]| match (module, &dir) {
        ("kernel", _) => profile::llvm_symbolize_kernel(&kernel, offsets),
        (_, Some(dir)) => profile::llvm_symbolize(dir, module, offsets),
        _ => vec![None; offsets.len()],
    };
    let export = otel::convert(&otel::Boot {
        serial,
        events,
        scenario,
        build: &build,
        start_unix_ns: end_unix_ns.saturating_sub(last),
        symbols: Some(&symbolize),
    });
    let name = |signal: &str| {
        let mut p = prefix.as_os_str().to_owned();
        p.push(format!(".{signal}.json"));
        PathBuf::from(p)
    };
    let write = |signal: &str, bodies: &[String]| -> Result<PathBuf> {
        let path = name(signal);
        let mut text = bodies.join("\n");
        text.push('\n');
        fs::write(&path, text).map_err(|e| format!("writing {}: {e}", path.display()))?;
        Ok(path)
    };
    let traces = write("traces", &export.traces)?;
    write("logs", &export.logs)?;
    write("metrics", std::slice::from_ref(&export.metrics))?;
    let profiles = !export.profiles.is_empty();
    if profiles {
        write("profiles", std::slice::from_ref(&export.profiles))?;
    }
    let mut said = export.report.lines();
    said += &format!(
        "otel: wrote {} (and .logs.json, .metrics.json{}; OTLP/JSON, one request per line)\n",
        traces.display(),
        if profiles { ", .profiles.json" } else { "" }
    );
    if let Some(endpoint) = endpoint {
        match otel::post(endpoint, &export) {
            Ok(sent) => said += &format!("otel: sent to {endpoint}: {sent}\n"),
            Err(e) => said += &format!("otel: sending to {endpoint} FAILED: {e}\n"),
        }
    }
    Ok(said)
}

/// `cargo xtask perfetto <events.jsonl | scenario> [--out FILE]`: the
/// boot as a Perfetto trace (xtask/src/perfetto.rs).
fn perfetto_cli(args: &[String]) -> Result {
    let Some(what) = args.first().filter(|a| !a.starts_with("--")) else {
        return Err("usage: cargo xtask perfetto <events.jsonl | scenario> [--out FILE]".into());
    };
    let path = if Path::new(what).exists() {
        PathBuf::from(what)
    } else {
        target_dir().join("debug").join(format!("test-{what}.events.jsonl"))
    };
    let text = fs::read_to_string(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    let out = args
        .iter()
        .position(|a| a == "--out")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
        .unwrap_or_else(|| path.with_extension("perfetto.json"));
    let trace = perfetto::convert(&text);
    fs::write(&out, &trace).map_err(|e| format!("writing {}: {e}", out.display()))?;
    eprintln!(
        "perfetto: wrote {} ({}); open it in https://ui.perfetto.dev (Open trace file; it stays local)",
        out.display(),
        perfetto::summary(&trace)
    );
    Ok(())
}

/// `cargo xtask profile <events.jsonl | scenario> [--pid N] [--tid N]
/// [--from S --to S] [--top N] [--symbols DIR]`: the sampling profiler's
/// report (xtask/src/profile.rs).
fn profile_cli(args: &[String]) -> Result {
    let flag =
        |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let num = |name: &str| flag(name).and_then(|v| v.parse::<u64>().ok());
    let secs = |name: &str| flag(name).and_then(|v| v.parse::<f64>().ok());
    let Some(what) = args.first().filter(|a| !a.starts_with("--")) else {
        return Err("usage: cargo xtask profile <events.jsonl | scenario> [--pid N] [--tid N] \
                    [--from S --to S] [--top N] [--symbols DIR] [--release]"
            .into());
    };
    // A release boot's stream and kernel (`boot-test --release`): asked
    // for, or a stream under target/release.
    let release = args.iter().any(|a| a == "--release")
        || Path::new(what).components().any(|c| c.as_os_str() == "release");
    let profile_dir = if release { "release" } else { "debug" };
    let path = if Path::new(what).exists() {
        PathBuf::from(what)
    } else {
        target_dir().join(profile_dir).join(format!("test-{what}.events.jsonl"))
    };
    let text = fs::read_to_string(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    if profile::count(&text) == 0 {
        return Err(format!(
            "{}: no sample events (boot with events=sample, or XTASK_EVENTS=sample,image,thread,process)",
            path.display()
        ));
    }
    let options = profile::Options {
        pid: num("--pid"),
        tid: num("--tid"),
        window: secs("--from").map(|a| (a, secs("--to").unwrap_or(f64::MAX))),
        top: num("--top").unwrap_or(15) as usize,
    };
    let symbols = flag("--symbols").map(PathBuf::from);
    // The kernel's own addresses: its ELF (the debug build boot tests
    // use, or the release one).
    let kernel = target_dir().join("x86_64-unknown-none").join(profile_dir).join("kernel");
    let symbolize = |module: &str, offsets: &[u64]| match (module, &symbols) {
        ("kernel", _) => profile::llvm_symbolize_kernel(&kernel, offsets),
        (_, Some(dir)) => profile::llvm_symbolize(dir, module, offsets),
        _ => vec![None; offsets.len()],
    };
    print!("{}", profile::report(&text, &options, &symbolize));
    Ok(())
}

/// `cargo xtask otel`'s arguments: <what> [<events>] [--out <prefix>]
/// [--endpoint <url>].
fn otel_cli(args: &[String]) -> Result {
    let flag =
        |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let positional: Vec<&String> = args
        .iter()
        .enumerate()
        .filter(|(i, a)| {
            !a.starts_with("--")
                && !(*i > 0 && matches!(args[*i - 1].as_str(), "--out" | "--endpoint"))
        })
        .map(|(_, a)| a)
        .collect();
    match positional.first() {
        Some(what) => otel_command(
            what,
            positional.get(1).map(Path::new),
            flag("--out").map(PathBuf::from),
            flag("--endpoint").or_else(|| env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok()),
        ),
        None => Err("usage: cargo xtask otel <serial log | events.jsonl | scenario> \
                     [<events.jsonl>] [--out <prefix>] [--endpoint <url>]"
            .into()),
    }
}

/// `cargo xtask otel <serial log | events.jsonl | scenario> [events]`.
fn otel_command(
    what: &str,
    events: Option<&Path>,
    out: Option<PathBuf>,
    endpoint: Option<String>,
) -> Result {
    let given = Path::new(what);
    let stem_of = |name: &str| -> String {
        name.strip_suffix(".events.jsonl")
            .or_else(|| name.strip_suffix(".serial.log"))
            .or_else(|| name.strip_suffix(".log"))
            .unwrap_or(name)
            .to_string()
    };
    // The serial log and the stream: a file and its sibling, or the
    // newest logs of a scenario.
    let (serial_path, events_path) = if given.is_file() {
        let name = given.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let stem = stem_of(name);
        let sibling = |ext: &str| given.with_file_name(format!("{stem}{ext}"));
        if name.ends_with(".events.jsonl") {
            let serial =
                [sibling(".serial.log"), sibling(".log")].into_iter().find(|p| p.is_file());
            (serial, Some(given.to_path_buf()))
        } else {
            (Some(given.to_path_buf()), Some(sibling(".events.jsonl")))
        }
    } else {
        let candidates = [
            target_dir().join(format!("debug/test-{what}.serial.log")),
            target_dir().join(format!("release/test-{what}.serial.log")),
            target_dir().join(format!("failed/{what}.log")),
        ];
        let newest = candidates
            .into_iter()
            .filter(|p| p.is_file())
            .max_by_key(|p| fs::metadata(p).and_then(|m| m.modified()).ok())
            .ok_or_else(|| {
                format!(
                    "{what}: not a file, and no log of scenario {what} in \
                     target/{{debug,release}}/test-{what}.serial.log or target/failed/{what}.log \
                     (a batched scenario: give its batch-*.serial.log)"
                )
            })?;
        let events = if newest.parent().is_some_and(|d| d.ends_with("failed")) {
            newest.with_file_name(format!("{what}.events.jsonl"))
        } else {
            newest.with_file_name(format!("test-{what}.events.jsonl"))
        };
        (Some(newest), Some(events))
    };
    let events_path = events.map(Path::to_path_buf).or(events_path).filter(|p| p.is_file());
    let read =
        |p: &Path| fs::read_to_string(p).map_err(|e| format!("reading {}: {e}", p.display()));
    let serial = serial_path.as_deref().map(read).transpose()?.unwrap_or_default();
    let events = events_path.as_deref().map(read).transpose()?.unwrap_or_default();
    if serial.is_empty() && events.is_empty() {
        return Err(format!("{what}: no serial log and no event stream to convert"));
    }
    let base = serial_path.clone().or(events_path.clone()).unwrap_or_default();
    let stem = stem_of(base.file_name().and_then(|n| n.to_str()).unwrap_or("boot"));
    let scenario = stem.strip_prefix("test-").unwrap_or(&stem);
    let prefix = out.unwrap_or_else(|| base.with_file_name(format!("{stem}.otlp")));
    // The boot ended about when its newest file was last written.
    let end = [serial_path.as_deref(), events_path.as_deref()]
        .into_iter()
        .flatten()
        .filter_map(|p| fs::metadata(p).and_then(|m| m.modified()).ok())
        .max()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos() as u64);
    eprintln!(
        "otel: serial log {}, event stream {}",
        serial_path.as_ref().map_or("(none)".into(), |p| p.display().to_string()),
        events_path
            .as_ref()
            .map_or("(none or empty: boot with XTASK_EVENTS=all for spans)".into(), |p| p
                .display()
                .to_string())
    );
    let said = otel_export_at(&serial, &events, scenario, &prefix, endpoint.as_deref(), end)?;
    eprint!("{said}");
    if said.contains("FAILED") { Err("sending the OTLP export failed".into()) } else { Ok(()) }
}

/// `cargo xtask triage <log> [events]`: the triage of any log; the
/// event stream next to it is found by name when not given.
fn triage_command(log: &Path, events: Option<&Path>) -> Result {
    let serial = fs::read_to_string(log).map_err(|e| format!("reading {}: {e}", log.display()))?;
    let name = log.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let guess = |suffix: &str| {
        let stem = name.strip_suffix(suffix)?;
        Some(log.with_file_name(format!("{stem}.events.jsonl")))
    };
    let events_path = events
        .map(Path::to_path_buf)
        .or_else(|| guess(".serial.log").or_else(|| guess(".log")).filter(|p| p.exists()));
    let events = events_path.as_deref().and_then(|p| fs::read_to_string(p).ok());
    // A failed log already ends with the triage xtask wrote: print the
    // fresh one only (the scenario's name from the file name).
    let serial = serial.split("===== triage (xtask").next().unwrap_or(&serial);
    let scenario =
        name.trim_end_matches(".log").trim_end_matches(".serial").trim_start_matches("test-");
    let facts = triage::Facts { scenario, ..triage::Facts::default() };
    print!("{}", triage::triage(serial, events.as_deref(), &facts));
    Ok(())
}

/// Free space (MiB) on the file system holding `dir` (`df`), if known.
fn free_disk_mib(dir: &Path) -> Option<u64> {
    let mut probe = dir.to_path_buf();
    while !probe.exists() {
        probe = probe.parent()?.to_path_buf();
    }
    let out = Command::new("df").args(["-Pk"]).arg(&probe).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let kib: u64 = text.lines().nth(1)?.split_whitespace().nth(3)?.parse().ok()?;
    Some(kib / 1024)
}

fn judge_log(scenario: &Scenario, status: Option<i32>, timed_out: bool, serial: &str) -> Result {
    // A guest whose clock disagreed with the host's (clockcheck.rs): the
    // clock scenarios fail on it; any other failure says so, since wrong
    // time explains early timeouts and watchdogs.
    if HOST_LOAD_SCENARIOS.contains(&scenario.name)
        && let Some(line) = serial.lines().find(|l| l.starts_with(clockcheck::WRONG))
    {
        return Err(line.to_string());
    }
    judge_log_inner(scenario, status, timed_out, serial).map_err(|e| {
        if serial.contains(clockcheck::WRONG) {
            format!("{e} (and the guest's clock was wrong: `{}` line)", clockcheck::WRONG)
        } else {
            e
        }
    })
}

fn judge_log_inner(
    scenario: &Scenario,
    status: Option<i32>,
    timed_out: bool,
    serial: &str,
) -> Result {
    let missing: Vec<&str> =
        scenario.expect.iter().copied().filter(|e| !serial.contains(e)).collect();
    if scenario.exit_status == XTASK_DUMPS {
        return match () {
            _ if !timed_out => Err(format!("expected to time out, QEMU exited with {status:?}")),
            _ if !missing.is_empty() => Err(format!("the state dump lacks {missing:?}")),
            _ => Ok(()),
        };
    }
    if scenario.exit_status == XTASK_STOPS && serial.contains(STOPPED_LINE) {
        return if missing.is_empty() {
            Ok(())
        } else {
            Err(format!("serial log lacks {missing:?}"))
        };
    }
    // The guest may finish on its own (a server serving one connection)
    // between the client's success and xtask stopping the VM: a clean exit
    // with everything expected (the client's line among it) passes too.
    if scenario.exit_status == XTASK_STOPS
        && status == Some(QEMU_EXIT_SUCCESS)
        && !timed_out
        && missing.is_empty()
        && serial.contains(BOOT_OK_MARKER)
        && !serial.contains(LEAK_MARKER)
    {
        return Ok(());
    }
    // A failed screen check stopped the VM (run_vm): that, not a timeout.
    if let Some(line) = serial.lines().find(|l| l.starts_with("xtask: screen: ")) {
        return Err(line.trim_start_matches("xtask: ").to_string());
    }
    match status {
        _ if timed_out => Err(format!(
            "timed out after {}s of run time (vmtime.rs)",
            scenario_timeout(scenario.name).as_secs()
        )),
        s if s != Some(scenario.exit_status) => {
            Err(format!("QEMU exited with {s:?}, expected status {}", scenario.exit_status))
        }
        _ if !missing.is_empty() => Err(format!("serial log lacks {missing:?}")),
        _ if scenario.exit_status == QEMU_EXIT_SUCCESS && serial.contains(LEAK_MARKER) => {
            Err("a process address space was not freed".into())
        }
        // A user/lx program: its output and exit status exactly as real
        // Linux's (its expected.txt).
        _ => lx_expect::check_boot(scenario.name, scenario.cmdline, serial),
    }
}

/// The part of a shared VM's log that belongs to the `nth` (from 0) run of
/// `program`, and whether it ran to its end.
fn program_section<'a>(serial: &'a str, program: &str, nth: usize) -> Option<(&'a str, bool)> {
    let begin = format!("run: begin {program}");
    let mut start = 0;
    for k in 0..=nth {
        let skip = if k == 0 { 0 } else { begin.len() };
        start += skip + find_line(&serial[start + skip..], &begin)?;
    }
    let rest = &serial[start..];
    Some(match find_line(rest, &format!("run: end {program}")) {
        Some(i) => (&rest[..i], true),
        None => (rest, false),
    })
}

/// Where the first line ending in `text` (CRLF or LF) is in `serial`.
fn find_line(serial: &str, text: &str) -> Option<usize> {
    serial
        .match_indices(text)
        .find(|(i, _)| matches!(serial.as_bytes().get(i + text.len()), Some(b'\r' | b'\n')))
        .map(|(i, _)| i)
}

/// `name -> ns per operation` from `bench: <name> ns_per_op <n>` lines.
fn bench_results(text: &str) -> Vec<(String, u64)> {
    text.lines()
        .filter_map(|l| l.split_once("bench: ").map(|(_, rest)| rest))
        .filter_map(|rest| {
            let mut words = rest.split_whitespace();
            let name = words.next()?;
            (words.next()? == "ns_per_op").then_some(())?;
            Some((name.to_string(), words.next()?.parse().ok()?))
        })
        .collect()
}

/// Boots `bench.exe` on gala, writes target/report/bench.json and prints
/// the numbers, next to Windows' if a run-all.ps1 bench file is given.
fn bench(release: bool, windows: Option<&Path>) -> Result {
    build_kernel(release)?;
    let iso = make_iso(release, "bench", "qemu_exit noscreen run=bench.exe", &[])?;
    let (status, serial, _) = run_vm(&iso, Boot::Bios, BOOT_TEST_TIMEOUT, None, false, None, None)?;
    if status.and_then(|s| s.code()) != Some(QEMU_EXIT_SUCCESS) || !serial.contains("bench: done") {
        return Err(format!(
            "bench did not finish; see {}",
            iso.with_extension("serial.log").display()
        ));
    }
    let gala = bench_results(&serial);
    let windows = match windows {
        Some(path) => {
            bench_results(&fs::read_to_string(path).map_err(|e| format!("reading {path:?}: {e}"))?)
        }
        None => Vec::new(),
    };
    let accel = if Path::new("/dev/kvm").exists() { "kvm" } else { "tcg" };
    let mut json = format!("{{\n  \"platform\": \"gala qemu-{accel}\",\n  \"results\": [\n");
    for (i, (name, ns)) in gala.iter().enumerate() {
        let comma = if i + 1 < gala.len() { "," } else { "" };
        json += &format!("    {{\"name\": \"{name}\", \"ns_per_op\": {ns}}}{comma}\n");
    }
    json += "  ]\n}\n";
    fs::create_dir_all(report_dir()).map_err(|e| e.to_string())?;
    write(&report_dir().join("bench.json"), &json)?;
    println!("| benchmark | gala (qemu-{accel}) ns/op | Windows ns/op |\n|---|---|---|");
    for (name, ns) in &gala {
        let win = windows.iter().find(|(n, _)| n == name).map_or("".into(), |(_, w)| w.to_string());
        println!("| {name} | {ns} | {win} |");
    }
    eprintln!("xtask: wrote {}", report_dir().join("bench.json").display());
    Ok(())
}

/// Whether this run builds release kernels (for finding the ELF).
static RELEASE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The `backtrace:` lines of a kernel log, resolved to function names and
/// source lines with `llvm-addr2line` or `addr2line` (empty if there are
/// none or no tool is installed).
fn symbolize(release: bool, serial: &str) -> String {
    let profile = if release { "release" } else { "debug" };
    let elf = target_dir().join(KERNEL_TARGET).join(profile).join("kernel");
    let mut out = String::new();
    for (what, line) in serial.lines().filter_map(|l| l.split_once("backtrace:")) {
        // Whose backtrace (a state dump has one per CPU).
        let what = what.trim_start_matches(['[', ' ']).trim();
        let addresses: Vec<&str> = line.split_whitespace().collect();
        if addresses.is_empty() {
            continue;
        }
        let resolved = ["llvm-addr2line", "addr2line"].iter().find_map(|tool| {
            Command::new(tool)
                .args(["-f", "-C", "-i", "-p", "-e"])
                .arg(&elf)
                .args(&addresses)
                .output()
                .ok()
                .filter(|o| o.status.success())
        });
        match resolved {
            Some(o) => {
                out += &format!(
                    "{what}{}backtrace (symbolized):\n",
                    if what.is_empty() { "" } else { " " }
                );
                for frame in String::from_utf8_lossy(&o.stdout).lines() {
                    out += &format!("  {frame}\n");
                }
            }
            None => out += "backtrace: no addr2line found to symbolize it\n",
        }
    }
    out
}

/// Boots one VM and judges each of its scenarios.
fn boot_vm<'a>(vm: &Vm<'a>, iso: &Path) -> (BootTime, Vec<(&'a Scenario, Result)>) {
    let (scenarios, timeout): (Vec<&Scenario>, Duration) = match vm {
        Vm::Single(s) => (vec![*s], scenario_timeout(s.name)),
        Vm::Batch(group) => (
            group.clone(),
            group.iter().map(|s| scenario_timeout(s.name)).max().unwrap_or(BOOT_TEST_TIMEOUT)
                + Duration::from_secs(20) * group.len() as u32,
        ),
    };
    let boot = scenario_boot(scenarios[0].name);
    let net = match vm {
        Vm::Single(s) => net_marker(s.name),
        Vm::Batch(_) => None,
    };
    let stop = matches!(vm, Vm::Single(s) if s.exit_status == XTASK_STOPS);
    let single = match vm {
        Vm::Single(s) => Some(s.name),
        Vm::Batch(_) => None,
    };
    let disk = match single.map(|name| new_disk(name, iso)).transpose() {
        Ok(d) => d.flatten(),
        Err(e) => {
            return (BootTime::default(), scenarios.iter().map(|&s| (s, Err(e.clone()))).collect());
        }
    };
    if let (Some(name), Some(d)) = (single, disk.as_deref())
        && let Err(e) = earlier_boots(name, iso, boot, timeout, d)
    {
        return (BootTime::default(), scenarios.iter().map(|&s| (s, Err(e.clone()))).collect());
    }
    let screen = single.and_then(screen_check);
    let want_cpus = telemetry::vm_cpus(iso);
    // The clock under a busy host: every host CPU kept busy while it runs.
    let _host_load =
        single.filter(|n| HOST_LOAD_SCENARIOS.contains(n)).map(|_| clockcheck::HostLoad::start());
    let mut attempt = 0;
    let (status, serial, time) = loop {
        let (status, serial, time) =
            match run_vm(iso, boot, timeout, net, stop, disk.as_deref(), screen) {
                Ok(r) => r,
                Err(e) => {
                    return (
                        BootTime::default(),
                        scenarios.iter().map(|&s| (s, Err(e.clone()))).collect(),
                    );
                }
            };
        // A boot that got fewer CPUs than asked for (Limine's AP start-up
        // timed out and the kernel could not start the AP either) did not
        // test what it should: once more (not with a disk: its state moved
        // on).
        let got = telemetry::cpus_online(&serial);
        if attempt == 0 && disk.is_none() && got.is_some_and(|n| n < want_cpus) {
            // Keep the short boot's logs: the next boot overwrites them.
            for ext in ["serial.log", "phases.log", "qemu.log", "bios.log"] {
                let _ = fs::copy(
                    iso.with_extension(ext),
                    iso.with_extension(format!("short-cpus.{ext}")),
                );
            }
            let _ = fs::write(iso.with_extension("short-cpus.serial.log"), &serial);
            eprintln!(
                "xtask: {}: {}; booting it once more (logs kept as *.short-cpus.*)",
                iso.display(),
                fewer_cpus(got.unwrap_or(0), want_cpus)
            );
            attempt += 1;
            continue;
        }
        break (status, serial, time);
    };
    // XTASK_OTEL: the boot as OpenTelemetry, next to its logs.
    if env::var_os("XTASK_OTEL").is_some() {
        let label = match vm {
            Vm::Single(s) => s.name.to_string(),
            Vm::Batch(_) => iso_scenario(iso).to_string(),
        };
        let events = fs::read_to_string(iso.with_extension("events.jsonl")).unwrap_or_default();
        let endpoint = env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok();
        match otel_export(
            &serial,
            &events,
            &label,
            &iso.with_extension("otlp"),
            endpoint.as_deref(),
        ) {
            Ok(said) => eprint!("{}", said.replace("otel:", &format!("xtask: otel {label}:"))),
            Err(e) => eprintln!("xtask: otel {label}: {e}"),
        }
    }
    let short = telemetry::cpus_online(&serial).filter(|&n| n < want_cpus);
    let code = status.and_then(|s| s.code());
    let facts = RunFacts {
        events: Some(iso.with_extension("events.jsonl")),
        secs: Some(time.total.as_secs_f64()),
    };
    let verdicts: Vec<(&Scenario, Result)> = match vm {
        Vm::Single(s) => {
            let events = fs::read_to_string(iso.with_extension("events.jsonl")).unwrap_or_default();
            let wanted = EVENT_EXPECT.iter().filter(|(n, _)| *n == s.name).flat_map(|(_, e)| *e);
            // An entry starting with '!' must not be in the stream.
            let missing: Vec<&str> = wanted
                .copied()
                .filter(|e| match e.strip_prefix('!') {
                    Some(absent) => events.contains(absent),
                    None => !events.contains(e),
                })
                .collect();
            let verdict = judge(s, code, status.is_none(), &serial, &facts)
                .and_then(|()| {
                    if missing.is_empty() {
                        Ok(())
                    } else {
                        Err(format!("event stream lacks (or, for a '!' entry, has) {missing:?}"))
                    }
                })
                .and_then(|()| disk.as_deref().map_or(Ok(()), |d| check_disk(s.name, d)));
            vec![(*s, verdict)]
        }
        Vm::Batch(group) => group
            .iter()
            .enumerate()
            .map(|(i, &s)| {
                let program = batchable(s).map_or("", |(p, _)| p);
                // Several scenarios may run the same program (dotnet.exe):
                // the n-th of them has its n-th section.
                let nth = group[..i]
                    .iter()
                    .filter(|&&o| batchable(o).map_or("", |(p, _)| p) == program)
                    .count();
                // Boot-time lines (before the first program) belong to all.
                let preamble = &serial[..serial.find("run: begin ").unwrap_or(0)];
                let verdict = match program_section(&serial, program, nth) {
                    // Finished: judge its section as if it had its own VM.
                    Some((section, true)) => judge(
                        s,
                        Some(QEMU_EXIT_SUCCESS),
                        false,
                        &format!("{preamble}{section}"),
                        &facts,
                    ),
                    // Started but the VM ended (or hung) in it.
                    Some((section, false)) => {
                        judge(s, code, status.is_none(), &format!("{preamble}{section}"), &facts)
                            .and(Err("the shared VM ended during this program".into()))
                    }
                    None => Err("did not run (the shared VM ended before it)".into()),
                };
                (s, verdict)
            })
            .collect(),
    };
    // A failure on a VM short of CPUs says so first.
    let verdicts = verdicts
        .into_iter()
        .map(|(s, v)| {
            (
                s,
                v.map_err(|e| {
                    short.map_or(e.clone(), |n| format!("{}: {e}", fewer_cpus(n, want_cpus)))
                }),
            )
        })
        .collect::<Vec<_>>();
    let show = env::var_os("XTASK_VERBOSE").is_some() || verdicts.iter().any(|(_, v)| v.is_err());
    if show {
        eprintln!(
            "----- {} serial output ({}) -----\n{}\n-----",
            scenarios.iter().map(|s| s.name).collect::<Vec<_>>().join(", "),
            iso.with_extension("serial.log").display(),
            serial.trim_end()
        );
        eprint!("{}", symbolize(RELEASE.load(std::sync::atomic::Ordering::Relaxed), &serial));
        // The triage of each failed scenario (also at the end of its kept log).
        let events = fs::read_to_string(iso.with_extension("events.jsonl")).ok();
        for (s, v) in verdicts.iter().filter(|(_, v)| v.is_err()) {
            let verdict = v.as_ref().err().map(String::as_str);
            let host = Some(host_facts());
            let facts = triage::Facts { scenario: s.name, verdict, secs: facts.secs, host };
            eprint!("{}", triage::triage(&serial, events.as_deref(), &facts));
        }
    }
    (time, verdicts)
}

/// Boots a scenario alone (for reports), without printing its log.
fn boot_scenario_quiet(scenario: &Scenario, iso: &Path) -> Result {
    let disk = new_disk(scenario.name, iso)?;
    if let Some(d) = disk.as_deref() {
        let boot = scenario_boot(scenario.name);
        earlier_boots(scenario.name, iso, boot, scenario_timeout(scenario.name), d)?;
    }
    let (status, serial, time) = run_vm(
        iso,
        scenario_boot(scenario.name),
        scenario_timeout(scenario.name),
        net_marker(scenario.name),
        scenario.exit_status == XTASK_STOPS,
        disk.as_deref(),
        screen_check(scenario.name),
    )?;
    let facts = RunFacts {
        events: Some(iso.with_extension("events.jsonl")),
        secs: Some(time.total.as_secs_f64()),
    };
    judge(scenario, status.and_then(|s| s.code()), status.is_none(), &serial, &facts)?;
    disk.as_deref().map_or(Ok(()), |d| check_disk(scenario.name, d))
}

/// Boot modules for an ISO with kernel command line `cmdline`: gala's
/// system DLLs in System32 (programs may look only there), the public CA
/// bundle in the machine root store, the test DLLs next to the programs,
/// Nginx's files if it runs, and `scenario`'s files
/// from the repository (scripts, archives) if one is named.
fn image_modules(
    cmdline: &[&str],
    scenario: Option<&str>,
) -> Result<Vec<(&'static str, &'static str)>> {
    let mut modules: Vec<(&str, &str)> = USER_DLLS
        .iter()
        .map(|d| (d.file, if is_test_dll(d.file) { "gala" } else { "Windows/System32" }))
        .collect();
    modules.extend(system_modules().into_iter().filter(|(file, _)| file.contains('/')));
    if cmdline.iter().any(|w| w.contains("nginx.exe")) {
        let nginx = if cmdline.iter().any(|w| w.contains("nginx-master")) {
            "nginx_master"
        } else if cmdline.iter().any(|w| w.contains("nginx-tls")) {
            "nginx_tls"
        } else {
            "nginx"
        };
        modules.extend(extra_modules(nginx).into_iter().filter(|(file, _)| file.contains('/')));
    }
    if let Some(scenario) = scenario {
        let files = extra_modules(scenario);
        if files.is_empty() {
            return Err(format!("no scenario {scenario:?} with extra modules"));
        }
        modules.extend(files.into_iter().filter(|(file, _)| file.contains('/')));
    }
    Ok(modules)
}

/// The test DLLs among USER_DLLS (shipped to C:\gala, not System32).
fn is_test_dll(file: &str) -> bool {
    file.starts_with("testdll") || file == "tlsdll.dll"
}

/// The system a full image ships besides the kernel (`cargo xtask iso`'s,
/// and a release's: `release-tree`): every DLL of USER_DLLS but the test
/// DLLs in C:\Windows\System32, and the machine root store's public CAs
/// (THIRD_PARTY.md: Mozilla's, as curl.se publishes them), which Schannel
/// and crypt32's ROOT store read: a full image trusts the public web, as
/// Windows does.
fn system_modules() -> Vec<(&'static str, &'static str)> {
    let mut modules: Vec<(&str, &str)> = USER_DLLS
        .iter()
        .filter(|d| !is_test_dll(d.file))
        .map(|d| (d.file, "Windows/System32"))
        .collect();
    modules.push(("user/roots/public-roots.pem", "Windows/System32/gala/roots"));
    modules
}

/// `cargo xtask release-tree <dir> [--release]`: the files of a release
/// (what the update feed publishes, ARCHITECTURE "The update feed"): the
/// image tree `make_iso` stages, with the kernel, system_modules, Limine,
/// the sealed config and the signed manifest, and no test programs, test
/// DLLs or scenario modules. The kernel command line is empty, as
/// `cargo xtask build`'s. `dir` must not exist or be empty.
fn release_tree(release: bool, dir: &Path) -> Result {
    if fs::read_dir(dir).is_ok_and(|mut d| d.next().is_some()) {
        return Err(format!("{}: not empty (release-tree writes a new tree)", dir.display()));
    }
    // The kernel and the DLLs; no test programs.
    let mut cmd = cargo();
    cmd.args(["build", "--package", "kernel", "--bin", "kernel", "--target", KERNEL_TARGET]);
    trust::kernel_env(&mut cmd)?;
    if release {
        cmd.arg("--release");
    }
    run_cmd(&mut cmd)?;
    let mut cmd = user_build();
    for dll in USER_DLLS {
        cmd.args(["--package", dll.package]);
    }
    run_cmd(&mut cmd)?;
    let modules = system_modules();
    make_iso_unchecked(release, "release", "", &modules, &trust::DEFAULT, Some(dir))?;
    let files = tree_size(dir);
    let mut count = Vec::new();
    tree_files(dir, Path::new(""), &mut count)?;
    let count = count.iter().filter(|p| dir.join(p).is_file()).count();
    eprintln!(
        "xtask: release tree {}: {count} files, {files} bytes (kernel, {} system modules, \
         Limine, limine.conf, gala.manifest)",
        dir.display(),
        modules.len()
    );
    Ok(())
}

#[cfg(test)]
mod release_tree_tests {
    use super::*;

    /// A release is the system: gala's DLLs in System32 and the root
    /// store, no test DLL or program.
    #[test]
    fn release_modules_are_the_system() {
        let m = system_modules();
        for dll in ["ntdll.dll", "kernel32.dll", "kernelbase.dll", "user32.dll", "ws2_32.dll"] {
            assert!(m.contains(&(dll, "Windows/System32")), "{dll}");
        }
        assert!(m.contains(&("user/roots/public-roots.pem", "Windows/System32/gala/roots")));
        for (file, _) in &m {
            assert!(!is_test_dll(file) && !file.ends_with(".exe"), "{file}");
            assert!(!USER_PROGRAMS.iter().any(|p| file.starts_with(p)), "{file}");
        }
        // `cargo xtask iso` ships the same system, and the test DLLs apart.
        let iso = image_modules(&[], None).unwrap();
        assert!(m.iter().all(|f| iso.contains(f)));
        assert!(iso.contains(&("testdll.dll", "gala")));
    }
}

/// What `cargo xtask iso` runs without a command line: a few programs that
/// show the C runtime-free, Rust and Go layers on any machine.
const DEMO_CMDLINE: &str = "run=hello.exe,go_hello.exe,go_goroutines.exe";

/// `cargo xtask iso [--name N] [--scenario S] [--file PATH[=DIR]]...
/// [kernel cmdline...]`: a bootable hybrid ISO (BIOS and UEFI; written as
/// is to a USB stick, it boots too) with every program and DLL, `S`'s
/// files, the host files given, and the command line (`S`'s, or
/// DEMO_CMDLINE if none). Prints where it is.
fn iso(release: bool, args: &[String]) -> Result {
    let mut name = String::from("demo");
    let mut cmdline = Vec::new();
    let mut scenario: Option<String> = None;
    // --file PATH[=DIR]: a file of the host's, as C:\DIR\<its name> (DIR
    // gala by default).
    let mut files: Vec<(String, String)> = Vec::new();
    let mut it = args.iter().filter(|a| *a != "--release");
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--name" => name = it.next().ok_or("--name needs a value")?.clone(),
            "--scenario" => scenario = Some(it.next().ok_or("--scenario needs a name")?.clone()),
            "--file" => {
                let spec = it.next().ok_or("--file needs a path")?;
                let (path, dir) = spec.split_once('=').unwrap_or((spec, "gala"));
                let full = fs::canonicalize(path).map_err(|e| format!("{path}: {e}"))?;
                files.push((full.display().to_string(), dir.to_string()));
            }
            _ => cmdline.push(arg.as_str()),
        }
    }
    if cmdline.is_empty() && scenario.is_none() {
        cmdline.push(DEMO_CMDLINE);
    }
    if let Some(s) = scenario.as_deref().filter(|_| cmdline.is_empty()) {
        // The scenario's own command line, without the test-only exit.
        let found = SCENARIOS.iter().find(|x| x.name == s).ok_or(format!("no scenario {s:?}"))?;
        cmdline.extend(found.cmdline.split_whitespace().filter(|w| *w != "qemu_exit"));
    }
    build_kernel(release)?;
    let mut modules: Vec<(&str, &str)> = image_modules(&cmdline, scenario.as_deref())?;
    modules.extend(files.iter().map(|(path, dir)| (path.as_str(), dir.as_str())));
    let path = make_iso(release, &format!("gala-{name}"), &cmdline.join(" "), &modules)?;
    println!("xtask: wrote {} (kernel command line: {})", path.display(), cmdline.join(" "));
    println!("xtask: QEMU: qemu-system-x86_64 -cpu max -m 1G -smp 2 -cdrom {}", path.display());
    println!(
        "xtask: USB stick (erases it): sudo dd if={} of=/dev/sdX bs=4M conv=fsync",
        path.display()
    );
    Ok(())
}

/// Boots an ISO with the given kernel command line (all test programs and
/// test DLLs as boot modules, ntdll in System32) and saves the screen as
/// PNG at each requested time, through QEMU's monitor (`screendump`).
fn screenshot(release: bool, args: &[String]) -> Result {
    let mut boot = Boot::Bios;
    let mut times = vec![3.0f64];
    let mut name = String::from("gala");
    let mut cmdline = Vec::new();
    let mut with_files: Option<String> = None;
    let mut it = args.iter().filter(|a| *a != "--release");
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--uefi" => boot = Boot::Uefi,
            "--at" => {
                let list = it.next().ok_or("--at needs a list of seconds")?;
                times = list
                    .split(',')
                    .map(|t| t.parse().map_err(|_| format!("bad time {t:?}")))
                    .collect::<Result<_>>()?;
            }
            "--name" => name = it.next().ok_or("--name needs a value")?.clone(),
            "--scenario" => {
                with_files = Some(it.next().ok_or("--scenario needs a name")?.clone());
            }
            _ => cmdline.push(arg.as_str()),
        }
    }
    build_kernel(release)?;
    let modules = image_modules(&cmdline, with_files.as_deref())?;
    let iso = make_iso(release, &format!("shot-{name}"), &cmdline.join(" "), &modules)?;
    let dir = target_dir().join("screenshots");
    fs::create_dir_all(&dir).map_err(|e| format!("creating {dir:?}: {e}"))?;
    let socket = iso.with_extension("monitor.sock");
    let _ = fs::remove_file(&socket);
    let mut cmd = qemu(&iso, boot)?;
    cmd.args(["-display", "none", "-serial", "null", "-monitor"])
        .arg(format!("unix:{},server,nowait", socket.display()))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Echo servers (`net_echo`, winsock_test's `serve`): a card, and the
    // client once the guest listens (no serial log here, so retry). Not
    // before 3 s: until the guest's network is up, QEMU holds the host
    // connection open and the client would wait out its read timeout;
    // after that a closed port is refused at once.
    let nginx = cmdline.iter().any(|w| w.contains("nginx.exe"));
    let tls = cmdline.iter().any(|w| w.contains("nginx-tls"));
    let http = nginx || cmdline.iter().any(|w| w.contains("go_http"));
    if cmdline.iter().any(|w| *w == "net_echo" || w.contains("serve")) || http {
        let port = free_port()?;
        add_net(&mut cmd, port);
        thread::spawn(move || {
            let mut result = Err(String::new());
            thread::sleep(Duration::from_millis(2500));
            for _ in 0..20 {
                thread::sleep(Duration::from_millis(500));
                result = if tls {
                    tls_client(port)
                } else if nginx {
                    nginx_client(port)
                } else if http {
                    http_client(port, GO_HTTP_PAGE)
                } else {
                    net_client(port)
                };
                if result.is_ok() {
                    break;
                }
            }
            if let Err(e) = result {
                eprintln!("xtask: net client: {e}");
            }
        });
    }
    let mut child = cmd.spawn().map_err(|e| format!("failed to spawn QEMU: {e}"))?;
    let start = Instant::now();
    let result = (|| {
        let mut monitor = None;
        for _ in 0..100 {
            if let Ok(s) = std::os::unix::net::UnixStream::connect(&socket) {
                monitor = Some(s);
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        let mut monitor = monitor.ok_or("cannot connect to the QEMU monitor")?;
        for &t in &times {
            let at = Duration::from_secs_f64(t);
            if let Some(wait) = at.checked_sub(start.elapsed()) {
                thread::sleep(wait);
            }
            let ppm = dir.join(format!("{name}-{t}s.ppm"));
            let _ = fs::remove_file(&ppm);
            use std::io::Write;
            writeln!(monitor, "screendump {}", ppm.display()).map_err(|e| e.to_string())?;
            for _ in 0..100 {
                if fs::metadata(&ppm).is_ok_and(|m| m.len() > 0) {
                    break;
                }
                thread::sleep(Duration::from_millis(20));
            }
            thread::sleep(Duration::from_millis(100));
            let png = dir.join(format!("{name}-{t}s.png"));
            ppm_to_png(&ppm, &png)?;
            let _ = fs::remove_file(&ppm);
            eprintln!("xtask: wrote {}", png.display());
        }
        Ok(())
    })();
    let _ = child.kill();
    let _ = child.wait();
    result
}

/// Converts a binary PPM (P6, 8-bit) to PNG (stored, uncompressed deflate
/// blocks: no dependencies, files are larger but every viewer reads them).
fn ppm_to_png(ppm: &Path, png: &Path) -> Result {
    let data = fs::read(ppm).map_err(|e| format!("reading {ppm:?}: {e}"))?;
    // Header: "P6" whitespace width whitespace height whitespace 255 single whitespace.
    let mut fields = Vec::new();
    let mut i = 0;
    while fields.len() < 4 {
        while data.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        let start = i;
        while data.get(i).is_some_and(|b| !b.is_ascii_whitespace()) {
            i += 1;
        }
        if start == i {
            return Err(format!("{ppm:?}: truncated PPM header"));
        }
        fields.push(String::from_utf8_lossy(&data[start..i]).into_owned());
    }
    let (w, h): (usize, usize) = match (fields[0].as_str(), fields[1].parse(), fields[2].parse()) {
        ("P6", Ok(w), Ok(h)) if fields[3] == "255" => (w, h),
        _ => return Err(format!("{ppm:?}: not an 8-bit P6 PPM")),
    };
    let pixels = data.get(i + 1..i + 1 + w * h * 3).ok_or(format!("{ppm:?}: truncated PPM"))?;
    // Scanlines with filter byte 0.
    let mut raw = Vec::with_capacity(h * (w * 3 + 1));
    for row in pixels.chunks(w * 3) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut zlib = vec![0x78, 0x01];
    for (n, block) in raw.chunks(65535).enumerate() {
        let last = (n + 1) * 65535 >= raw.len();
        zlib.push(u8::from(last));
        let len = block.len() as u16;
        zlib.extend_from_slice(&len.to_le_bytes());
        zlib.extend_from_slice(&(!len).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in &raw {
        a = (a + u32::from(byte)) % 65521;
        b = (b + a) % 65521;
    }
    zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit RGB
    for (kind, body) in [(b"IHDR", &ihdr), (b"IDAT", &zlib), (b"IEND", &Vec::new())] {
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        let start = out.len();
        out.extend_from_slice(kind);
        out.extend_from_slice(body);
        let crc = crc32(&out[start..]);
        out.extend_from_slice(&crc.to_be_bytes());
    }
    fs::write(png, out).map_err(|e| format!("writing {png:?}: {e}"))
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// The image `docker-test` builds and runs.
const DOCKER_IMAGE: &str = "gala-os-test";

/// Builds the test image and runs xtask inside it with the repository
/// mounted (`args` default to `test`), using KVM when the host has it.
fn docker_test(args: &[String]) -> Result {
    // Behind a proxy (as in sandboxes): forward its settings, trust its CA,
    // and share the host's network when the proxy listens on localhost.
    const PROXY_VARS: [&str; 6] =
        ["http_proxy", "https_proxy", "no_proxy", "HTTP_PROXY", "HTTPS_PROXY", "NO_PROXY"];
    let proxies: Vec<(&str, String)> =
        PROXY_VARS.iter().filter_map(|&v| env::var(v).ok().map(|value| (v, value))).collect();
    let host_network =
        proxies.iter().any(|(_, v)| v.contains("127.0.0.1") || v.contains("localhost"));
    let ca_dir = root().join("docker/extra-ca");
    if let Some(ca) = env::var_os("XTASK_DOCKER_CA").or_else(|| env::var_os("SSL_CERT_FILE")) {
        copy(Path::new(&ca), &ca_dir.join("extra.crt"))?;
    }
    let mut build = Command::new("docker");
    build
        .args(["build", "--quiet", "-t", DOCKER_IMAGE, "-f"])
        .arg(root().join("docker/test.Dockerfile"));
    if host_network {
        build.args(["--network", "host"]);
    }
    for (var, value) in &proxies {
        build.arg("--build-arg").arg(format!("{var}={value}"));
    }
    run_cmd(build.arg(root().join("docker")))?;
    let mut cmd = Command::new("docker");
    cmd.args(["run", "--rm", "-v"]).arg(format!("{}:/work", root().display()));
    if host_network {
        cmd.args(["--network", "host"]);
    }
    for (var, value) in &proxies {
        cmd.arg("-e").arg(format!("{var}={value}"));
    }
    if Path::new("/dev/kvm").exists() {
        cmd.args(["--device", "/dev/kvm"]);
    }
    for var in ["XTASK_VERBOSE", "XTASK_QEMU_ARGS", "XTASK_FIRMWARE", "XTASK_NO_BATCH"] {
        if let Ok(value) = env::var(var) {
            cmd.arg("-e").arg(format!("{var}={value}"));
        }
    }
    cmd.args([DOCKER_IMAGE, "cargo", "xtask"]);
    if args.is_empty() {
        cmd.arg("test");
    } else {
        cmd.args(args);
    }
    run_cmd(&mut cmd)
}

/// Checked-in files a test program opens beside itself, exported to
/// user/bin so that it finds them there on Windows as in C:\gala.
const EXPORTED_TEST_DATA: &[(&str, &str)] =
    &[("user/kernel32_clr_test/x86_native.dll", "x86_native.dll")];

/// Copies the test programs to `user/bin/`, where they are checked in so
/// they can be run on real Windows without a Rust toolchain.
fn export_tests() -> Result {
    let dir = root().join("user/bin");
    fs::create_dir_all(&dir).map_err(|e| format!("creating {dir:?}: {e}"))?;
    for program in USER_PROGRAMS.iter().chain(GO_PROGRAMS).chain(USER_PROGRAMS_X86) {
        copy(&built_program(program), &dir.join(format!("{program}.exe")))?;
    }
    for dll in USER_DLLS {
        let file = built_dll(dll.file);
        copy(&file, &dir.join(dll.exported_as))?;
    }
    for dll in USER_DLLS_X86 {
        copy(&built_dll_x86(dll.file), &dir.join(dll.exported_as))?;
    }
    for (file, exported_as) in EXPORTED_TEST_DATA {
        copy(&root().join(file), &dir.join(exported_as))?;
    }
    eprintln!("xtask: exported {} test programs to {}", USER_PROGRAMS.len(), dir.display());
    // pk_install_test's package source zips user/bin/hello.exe, so it is
    // derived from what was just exported: remake it (deterministic), or
    // a change to ntrt leaves the zips holding an old hello.exe.
    run_cmd(
        Command::new("python3").arg("-I").arg(root().join("user/pk_install_test/make_source.py")),
    )?;
    Ok(())
}

fn copy(from: &Path, to: &Path) -> Result {
    fs::copy(from, to).map(drop).map_err(|e| format!("copying {from:?} to {to:?}: {e}"))
}

/// Copies a kernel ELF onto an image without its debug information
/// (`objcopy --strip-debug`; symbols and the GALA note stay). The image's
/// kernel is hashed by Limine at every boot and by the kernel before a
/// handover (R8): 3 MB instead of 30 for a debug build. Backtraces are
/// symbolized on the host from the build's own ELF, which keeps it.
fn copy_kernel(from: &Path, to: &Path) -> Result {
    run_quiet(Command::new("objcopy").arg("--strip-debug").arg(from).arg(to))
}

fn write(path: &Path, contents: &str) -> Result {
    fs::write(path, contents).map_err(|e| format!("writing {path:?}: {e}"))
}

#[cfg(test)]
mod constants {
    /// The kernel's NTSTATUS values (kernel/src/nt/status.rs) against
    /// mingw-w64's ntstatus.h, when it is installed (Ubuntu:
    /// mingw-w64-common; CI has none and skips). A wrong value is a
    /// silent ABI bug: STATUS_OBJECT_NAME_EXISTS was 0x40000035 here, and
    /// a test expected it, until this check found it.
    /// The NT layer reaches the machine only through nt/platform.rs (the
    /// gala-um seam, docs/GALA_UM.md).
    #[test]
    fn nt_uses_the_machine_only_through_platform() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../kernel/src/nt");
        let banned = [
            "crate::arch",
            "crate::sched",
            "crate::time",
            "crate::disk",
            "crate::mm::PAGE_SIZE",
            "crate::mm::frame",
            "crate::mm::paging",
            "crate::mm::free_frame",
            "crate::mm::allocate_frame",
            "crate::mm::hhdm_offset",
        ];
        let mut found = Vec::new();
        // nt/ and its subdirectories.
        let mut files = Vec::new();
        let mut dirs = vec![dir.clone()];
        while let Some(d) = dirs.pop() {
            for entry in std::fs::read_dir(&d).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    dirs.push(path);
                } else {
                    files.push(path);
                }
            }
        }
        for path in files {
            if path.file_name().is_some_and(|n| n == "platform.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            for (i, line) in text.lines().enumerate() {
                if banned.iter().any(|b| line.contains(b)) {
                    found.push(format!("{}:{}: {}", path.display(), i + 1, line.trim()));
                }
            }
        }
        assert!(found.is_empty(), "use nt::platform instead:\n{}", found.join("\n"));
    }

    /// `net` is core, below every personality: it reaches threads and time
    /// through `crate::runtime`, never through the NT layer (ROADMAP LX
    /// seam 6; a Linux personality would build its sockets on it too).
    #[test]
    fn net_does_not_depend_on_nt() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../kernel/src/net");
        let mut found = Vec::new();
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let text = std::fs::read_to_string(&path).unwrap();
            for (i, line) in text.lines().enumerate() {
                if line.contains("crate::nt") || line.contains("super::super::nt") {
                    found.push(format!("{}:{}: {}", path.display(), i + 1, line.trim()));
                }
            }
        }
        assert!(
            found.is_empty(),
            "net/ names the NT layer; use crate::runtime:\n{}",
            found.join("\n")
        );
    }

    /// The AF_UNIX sockets are core objects (ARCHITECTURE "AF_UNIX
    /// sockets", seam 5): no personality's types in them, so NT and Linux
    /// views can share one socket.
    #[test]
    fn unix_sockets_do_not_depend_on_a_personality() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../kernel/src/unix.rs");
        let text = std::fs::read_to_string(&path).unwrap();
        let found: Vec<String> = text
            .lines()
            .enumerate()
            .filter(|(_, l)| l.contains("crate::nt") || l.contains("crate::lx"))
            .map(|(i, l)| format!("{}:{}: {}", path.display(), i + 1, l.trim()))
            .collect();
        assert!(found.is_empty(), "unix.rs names a personality:\n{}", found.join("\n"));
    }

    #[test]
    fn ntstatus_values_match_mingw() {
        let header = std::path::Path::new("/usr/share/mingw-w64/include/ntstatus.h");
        let Ok(header) = std::fs::read_to_string(header) else {
            eprintln!("skipped: no mingw-w64 ntstatus.h");
            return;
        };
        let mut defined = std::collections::HashMap::new();
        for line in header.lines() {
            // #define STATUS_NAME ((NTSTATUS)0x....)
            let Some(rest) = line.strip_prefix("#define STATUS_") else { continue };
            let Some((name, value)) = rest.split_once(char::is_whitespace) else { continue };
            let Some(hex) = value.trim().strip_prefix("((NTSTATUS)0x") else { continue };
            let hex = hex.trim_end_matches(')').trim_end_matches('L');
            if let Ok(v) = u32::from_str_radix(hex, 16) {
                defined.insert(name.to_string(), v);
            }
        }
        let source =
            std::fs::read_to_string(super::root().join("kernel/src/nt/status.rs")).unwrap();
        let mut checked = 0;
        for line in source.lines() {
            // pub const NAME: NtStatus = NtStatus(0x....);
            let Some(rest) = line.trim().strip_prefix("pub const ") else { continue };
            let Some((name, value)) = rest.split_once(": NtStatus = NtStatus(0x") else { continue };
            let hex: String = value
                .chars()
                .take_while(|c| c.is_ascii_hexdigit() || *c == '_')
                .filter(|&c| c != '_')
                .collect();
            let ours = u32::from_str_radix(&hex, 16).unwrap();
            if let Some(&theirs) = defined.get(name) {
                assert_eq!(ours, theirs, "STATUS_{name}: kernel {ours:#x}, ntstatus.h {theirs:#x}");
                checked += 1;
            }
        }
        assert!(checked > 50, "only {checked} statuses compared");
    }

    /// sim-test's programs are gala-um scenarios, and it reads the trace
    /// line um prints.
    #[test]
    fn sim_test_programs_and_trace_line() {
        use super::*;
        for name in SIM_PROGRAMS.iter().map(|(p, _)| *p) {
            assert!(
                SCENARIOS.iter().any(|s| s.name == name && runs_in_um(s)),
                "{name} is not a gala-um scenario"
            );
        }
        // Every Go program gala-um runs is simulated and must repeat from
        // its seed (Go's runtime is our most demanding threaded program).
        for name in GO_PROGRAMS {
            if SCENARIOS.iter().any(|s| s.name == *name && runs_in_um(s)) {
                assert!(
                    SIM_PROGRAMS.iter().any(|(p, threaded)| p == name && *threaded),
                    "{name} runs in gala-um but is not in SIM_PROGRAMS"
                );
            }
        }
        let log = "x\nsim: seed 7 trace 3145af8e34765682 (214 scheduling points, 110 ms virtual)\n";
        assert_eq!(sim_trace(log), Some("3145af8e34765682"));
        assert_eq!(sim_trace("sim: seed 7: DEADLOCK at 5 ms virtual"), None);
    }
}

/// The desktop's text: Selawik (Microsoft's open-source stand-in for
/// Segoe UI, fonts/selawik) rasterized here, on the host, so the kernel
/// only copies coverage (no font engine in the kernel). One file per face
/// and size in kernel/assets/ui-<size>-<weight>.glyphs:
/// "GLY1", size, ascent, descent (i16 each, pixels), then for each
/// character 0x20..=0xFF: advance (i16, 1/16 pixel), x and y of the
/// bitmap's top-left from the pen at the baseline (i16, y down), width
/// and height (u16), its offset into the coverage bytes (u32); then the
/// coverage bytes.
fn bake_fonts() -> Result {
    let faces: &[(u32, &str, &str)] = &[
        (12, "regular", "selawk.ttf"),
        (12, "semibold", "selawksb.ttf"),
        (14, "semibold", "selawksb.ttf"),
        (20, "semibold", "selawksb.ttf"),
        (28, "semilight", "selawksl.ttf"),
    ];
    for &(size, weight, file) in faces {
        let ttf = fs::read(root().join("fonts/selawik").join(file))
            .map_err(|e| format!("{file}: {e}"))?;
        let font = fontdue::Font::from_bytes(ttf, fontdue::FontSettings::default())
            .map_err(|e| e.to_string())?;
        let px = size as f32;
        let line = font.horizontal_line_metrics(px).ok_or("no line metrics")?;
        let mut table = Vec::new();
        let mut data = Vec::new();
        for c in 0x20u32..=0xff {
            let ch = char::from_u32(c).unwrap_or('?');
            let (m, bitmap) = font.rasterize(ch, px);
            let advance = (m.advance_width * 16.0).round() as i16;
            // fontdue: ymin is the bitmap's bottom relative to the
            // baseline (up); the top-left y, down from the baseline:
            let top = -(m.ymin + m.height as i32);
            table.extend_from_slice(&advance.to_le_bytes());
            table.extend_from_slice(&(m.xmin as i16).to_le_bytes());
            table.extend_from_slice(&(top as i16).to_le_bytes());
            table.extend_from_slice(&(m.width as u16).to_le_bytes());
            table.extend_from_slice(&(m.height as u16).to_le_bytes());
            table.extend_from_slice(&(data.len() as u32).to_le_bytes());
            data.extend_from_slice(&bitmap);
        }
        let mut out = b"GLY1".to_vec();
        out.extend_from_slice(&(size as i16).to_le_bytes());
        out.extend_from_slice(&(line.ascent.round() as i16).to_le_bytes());
        out.extend_from_slice(&((-line.descent).round() as i16).to_le_bytes());
        out.extend_from_slice(&table);
        out.extend_from_slice(&data);
        let path = root().join(format!("kernel/assets/ui-{size}-{weight}.glyphs"));
        fs::write(&path, &out).map_err(|e| format!("{path:?}: {e}"))?;
        eprintln!("xtask: {} ({} bytes)", path.display(), out.len());
    }
    Ok(())
}

/// The desktop's pictures from SVG (kernel/assets/*.svg), rendered here
/// with resvg so the kernel only blends coverage: each to square alpha
/// masks (the picture centred, `size` x `size` bytes) in
/// kernel/assets/<name>-<size>.alpha; the kernel colours them.
fn bake_icons() -> Result {
    let icons: &[(&str, &[u32])] = &[("fortress", &[24, 96, 160])];
    for &(name, sizes) in icons {
        let svg = fs::read(root().join(format!("kernel/assets/{name}.svg")))
            .map_err(|e| format!("{name}.svg: {e}"))?;
        let tree = resvg::usvg::Tree::from_data(&svg, &resvg::usvg::Options::default())
            .map_err(|e| e.to_string())?;
        let view = tree.size();
        for &size in sizes {
            let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size).ok_or("pixmap")?;
            let scale = size as f32 / view.width().max(view.height());
            let dx = (size as f32 - view.width() * scale) / 2.0;
            let dy = (size as f32 - view.height() * scale) / 2.0;
            let transform = resvg::tiny_skia::Transform::from_row(scale, 0.0, 0.0, scale, dx, dy);
            resvg::render(&tree, transform, &mut pixmap.as_mut());
            let alpha: Vec<u8> = pixmap.pixels().iter().map(|p| p.alpha()).collect();
            let path = root().join(format!("kernel/assets/{name}-{size}.alpha"));
            fs::write(&path, &alpha).map_err(|e| format!("{path:?}: {e}"))?;
            eprintln!("xtask: {} ({} bytes)", path.display(), alpha.len());
        }
    }
    // shell32's file icons (the system image list): colour, as 32-bit
    // pixels with straight alpha (B, G, R, A bytes), 16 and 32 px.
    for name in ["document", "folder", "folder-open", "program", "drive"] {
        let svg = fs::read(root().join(format!("user/shell32/assets/{name}.svg")))
            .map_err(|e| format!("{name}.svg: {e}"))?;
        let tree = resvg::usvg::Tree::from_data(&svg, &resvg::usvg::Options::default())
            .map_err(|e| e.to_string())?;
        let view = tree.size();
        for size in [16u32, 32] {
            let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size).ok_or("pixmap")?;
            let scale = size as f32 / view.width().max(view.height());
            let transform = resvg::tiny_skia::Transform::from_scale(scale, scale);
            resvg::render(&tree, transform, &mut pixmap.as_mut());
            let mut out = Vec::with_capacity((size * size * 4) as usize);
            for p in pixmap.pixels() {
                let c = p.demultiply();
                out.extend_from_slice(&[c.blue(), c.green(), c.red(), c.alpha()]);
            }
            let path = root().join(format!("user/shell32/assets/{name}-{size}.bgra"));
            fs::write(&path, &out).map_err(|e| format!("{path:?}: {e}"))?;
            eprintln!("xtask: {} ({} bytes)", path.display(), out.len());
        }
    }
    // The wordmark (قلعة, "qal'a"): cropped to its letters, 112 pixels
    // wide (the wallpaper's mark) and 220 (the boot splash), as
    // "<width u16><height u16>" then the alpha bytes.
    let svg = fs::read(root().join("kernel/assets/wordmark.svg"))
        .map_err(|e| format!("wordmark.svg: {e}"))?;
    let tree = resvg::usvg::Tree::from_data(&svg, &resvg::usvg::Options::default())
        .map_err(|e| e.to_string())?;
    let bbox = tree.root().abs_bounding_box();
    for (width, file) in [(112u32, "wordmark.alpha"), (220, "wordmark-220.alpha")] {
        let scale = width as f32 / bbox.width();
        let height = (bbox.height() * scale).ceil() as u32;
        let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height).ok_or("pixmap")?;
        let transform = resvg::tiny_skia::Transform::from_row(
            scale,
            0.0,
            0.0,
            scale,
            -bbox.x() * scale,
            -bbox.y() * scale,
        );
        resvg::render(&tree, transform, &mut pixmap.as_mut());
        let mut out = Vec::new();
        out.extend_from_slice(&(width as u16).to_le_bytes());
        out.extend_from_slice(&(height as u16).to_le_bytes());
        out.extend(pixmap.pixels().iter().map(|p| p.alpha()));
        let path = root().join("kernel/assets").join(file);
        fs::write(&path, &out).map_err(|e| format!("{path:?}: {e}"))?;
        eprintln!("xtask: {} ({width} x {height})", path.display());
    }
    Ok(())
}

#[cfg(test)]
mod judging {
    use super::*;

    /// A log with a byte that is not UTF-8 is still read whole (the
    /// invalid byte as U+FFFD), not as empty.
    #[test]
    fn a_log_with_an_invalid_byte_is_read_whole() {
        let path = std::env::temp_dir().join(format!("xtask-read-log-{}.log", std::process::id()));
        fs::write(&path, b"first line\ncheck \xb0 failed\nlast line\n").unwrap();
        let text = read_log(&path);
        let _ = fs::remove_file(&path);
        assert!(text.starts_with("first line\n"));
        assert!(text.contains("check \u{fffd} failed"));
        assert!(text.ends_with("last line\n"));
    }

    #[test]
    fn a_server_that_ends_after_the_client_passes() {
        let s = Scenario {
            name: "x",
            cmdline: "",
            exit_status: XTASK_STOPS,
            expect: &["xtask: net client: ssh ok"],
        };
        let ok = format!("xtask: net client: ssh ok\n{BOOT_OK_MARKER}");
        assert!(judge_log(&s, Some(QEMU_EXIT_SUCCESS), false, &ok).is_ok());
        // Without the client's line, or after a failure exit, it fails.
        assert!(judge_log(&s, Some(QEMU_EXIT_SUCCESS), false, BOOT_OK_MARKER).is_err());
        assert!(judge_log(&s, Some(QEMU_EXIT_FAILURE), false, &ok).is_err());
    }
}

#[cfg(test)]
mod health {
    use super::*;

    fn line(name: &str, ok: bool, secs: f64, shared: bool) -> String {
        format!(
            r#"{{"t":1,"commit":"abc","load":2.5,"scenario":"{name}","ok":{ok},"secs":{secs},"shared":{shared},"reason":"timed out after 300s"}}"#
        )
    }

    #[test]
    fn flaky_and_slower_scenarios_are_found() {
        let mut h = Vec::new();
        for i in 0..10 {
            h.push(line("java_http", i != 4, 40.0, false));
            h.push(line("steady", true, 10.0, false));
            h.push(line("slow", true, 20.0, false));
            h.push(line("shared", i != 2, 5.0, true));
        }
        h.push(line("slow", true, 45.0, false));
        h.push(line("steady", true, 11.0, false));
        let found = history_findings(&h.join("\n"), &["java_http", "steady", "slow", "shared"]);
        assert_eq!(found.len(), 3, "{found:?}");
        assert!(found[0].starts_with("java_http is flaky: failed 1 of its last 10 runs"));
        assert!(found[0].contains("timed out after 300s"));
        assert!(found[1].starts_with("slow is slower: 45.0s against a median of 20.0s"));
        assert!(found[1].contains("host load 2.5"));
        assert!(found[2].starts_with("shared is flaky"));
        // Only this run's scenarios.
        assert!(history_findings(&h.join("\n"), &["steady"]).is_empty());
    }

    #[test]
    fn a_dump_scenario_passes_only_on_a_timeout_with_its_lines() {
        let s = Scenario {
            name: "x",
            cmdline: "",
            exit_status: XTASK_DUMPS,
            expect: &["statedump: end"],
        };
        assert!(judge_log(&s, None, true, "statedump: end").is_ok());
        assert!(judge_log(&s, None, true, "statedump: begin").is_err());
        assert!(judge_log(&s, Some(QEMU_EXIT_SUCCESS), false, "statedump: end").is_err());
    }
}

#[cfg(test)]
mod repeats {
    use super::*;

    /// A repeat's copy of an image is the same scenario (its port
    /// forward, display, ...) and keeps its own failed log.
    #[test]
    fn a_repeat_copy_is_its_scenario() {
        let iso = |n: &str| PathBuf::from(format!("/t/debug/{n}"));
        assert_eq!(iso_scenario(&iso("test-sshd_shell.iso")), "sshd_shell");
        assert_eq!(iso_scenario(&iso("test-sshd_shell.r0.iso")), "sshd_shell");
        assert_eq!(iso_scenario(&iso("test-sshd_shell.r11.iso")), "sshd_shell");
        assert_eq!(iso_scenario(&iso("serve-go_http.iso")), "go_http");
        assert_eq!(guest_port(iso_scenario(&iso("test-sshd_shell.r3.iso"))), 22);
        assert_eq!(iso_scenario(&iso("test-handover_k32_twice.iso")), "handover_k32_twice");
        assert_eq!(repeat_index(&iso("test-udp_echo.r4.events.jsonl")), Some(4));
        assert_eq!(repeat_index(&iso("test-udp_echo.events.jsonl")), None);
    }

    /// Leases held at once never share a port.
    #[test]
    fn port_leases_are_distinct() {
        let leases: Vec<PortLease> = (0..8).map(|_| PortLease::new().unwrap()).collect();
        let mut ports: Vec<u16> = leases.iter().map(|l| l.0).collect();
        ports.sort_unstable();
        ports.dedup();
        assert_eq!(ports.len(), 8);
    }
}

#[cfg(test)]
mod um_scenarios {
    use super::*;

    /// A scenario whose setup gala-um cannot give (a server `start=`
    /// starts) does not run there, and the reason names the flag:
    /// schannel_test once ran without its Nginx and waited 60 s for it,
    /// which was taken for a TLS hang in gala-um.
    #[test]
    fn a_flag_gala_um_does_not_apply_keeps_a_scenario_out() {
        let s = Scenario {
            name: "tls_client_test",
            cmdline: "qemu_exit start=nginx.exe start_args=-p%20C:/nginx/ run=tls_client_test.exe",
            exit_status: QEMU_EXIT_SUCCESS,
            expect: &["tls_client_test: ok"],
        };
        assert!(!runs_in_um(&s));
        assert!(um_gap(&s).is_some_and(|why| why.contains("start=nginx.exe")), "{:?}", um_gap(&s));
        let um = Path::new("um");
        assert!(um_command(um, &s, &[], 1).is_err_and(|e| e.contains("start=nginx.exe")));
        // Flags gala-um applies keep it in.
        let s = Scenario { cmdline: "qemu_exit clock=pause run=tls_client_test.exe", ..s };
        assert!(runs_in_um(&s), "{:?}", um_gap(&s));
    }

    /// Every scenario gala-um runs is run with all of its kernel flags,
    /// and schannel_test, which needs Nginx (`start=`), is kept out by
    /// that rule, not by a list.
    #[test]
    fn um_scenarios_have_only_flags_um_applies() {
        for s in SCENARIOS.iter().filter(|s| runs_in_um(s)) {
            assert_eq!(um_unapplied_flag(s), None, "{}", s.name);
        }
        let schannel = SCENARIOS.iter().find(|s| s.name == "schannel_test").unwrap();
        assert!(um_gap(schannel).is_some_and(|why| why.starts_with("kernel flag start=nginx.exe")));
    }
}
