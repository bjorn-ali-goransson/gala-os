#!/usr/bin/env python3
"""Makes pk_install_test's package source (user/pk_install_test/source):
a winget-pkgs tree with gala's test packages and the zips they install
(the `installers` mirror), and user/pk_install_test/served, the files
the in-guest Nginx serves for the download test (pk_install_https:
GalaTest.PkHttps, whose installer is not in the mirror). The tree also holds copies of real winget-pkgs
manifests (manifests/b and manifests/j, MIT, THIRD_PARTY.md), which this
script leaves alone. Run from the repository root:

    python3 -I user/pk_install_test/make_source.py

The packages' program is user/bin/hello.exe (prints "Hello from user
mode!"). Zips are deflated with fixed timestamps, so the output is the
same every run; the SHA-256s in the manifests are computed here.
"""

import hashlib
import os
import shutil
import subprocess
import sys
import zipfile

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
OUT = os.path.join(ROOT, "user", "pk_install_test", "source")
SERVED = os.path.join(ROOT, "user", "pk_install_test", "served")
HELLO = open(os.path.join(ROOT, "user", "bin", "hello.exe"), "rb").read()
STAMP = (2026, 11, 3, 0, 0, 0)
URL = "https://packages.gala.invalid/"
# schannel_test's Nginx (user/schannel_test/nginx.conf): the release URL
# redirects to the file, as GitHub's do; 8444's certificate is untrusted.
NGINX = "https://localhost:8443/"
UNTRUSTED = "https://localhost:8444/"
KEPT = {}


def make_zip(name, files, directory=None):
    path = os.path.join(directory or os.path.join(OUT, "installers"), name)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with zipfile.ZipFile(path, "w") as z:
        for entry, data in files:
            info = zipfile.ZipInfo(entry, STAMP)
            info.external_attr = (0o40755 << 16) | 0x10 if data is None else 0o644 << 16
            info.compress_type = zipfile.ZIP_STORED if data is None else zipfile.ZIP_DEFLATED
            z.writestr(info, b"" if data is None else data)
    return hashlib.sha256(open(path, "rb").read()).hexdigest().upper()


def installer(name, data):
    path = os.path.join(OUT, "installers", name)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as f:
        f.write(data)
    return hashlib.sha256(data).hexdigest().upper()


def nsis_installer(version):
    """pknsis-<version>.exe from nsis/pknsis.nsi with makensis (NSIS 3,
    `apt install nsis`; the build is reproducible). Without makensis the
    checked-in installer is written back unchanged, so the output is the
    same either way."""
    name = f"pknsis-{version}.exe"
    path = os.path.join(OUT, "installers", name)
    makensis = shutil.which("makensis")
    if makensis is None:
        if name not in KEPT:
            sys.exit(
                f"{name} is missing and makensis is not installed to rebuild it "
                f"(apt install nsis); restore it with "
                f"`git checkout -- user/pk_install_test/`"
            )
        print(f"makensis not found: keeping {name}", file=sys.stderr)
        # The bytes, not just the hash: the clean-up in main() has already
        # removed the file they were read from.
        return installer(name, KEPT[name])
    os.makedirs(os.path.dirname(path), exist_ok=True)
    subprocess.run(
        [
            makensis,
            "-V2",
            f"-DVERSION={version}",
            "-DHELLO=" + os.path.join(ROOT, "user", "bin", "hello.exe"),
            "-DOUTFILE=" + path,
            os.path.join(ROOT, "user", "pk_install_test", "nsis", "pknsis.nsi"),
        ],
        check=True,
    )
    return hashlib.sha256(open(path, "rb").read()).hexdigest().upper()


def write(rel, text, root=None):
    path = os.path.join(root or OUT, "manifests", rel)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", newline="\r\n") as f:
        f.write(text)


def pkhello_zip(version):
    d = f"pkhello-{version}/"
    return make_zip(
        f"pkhello-{version}.zip",
        [
            (d, None),
            (d + "pkhello.exe", HELLO),
            (d + "README.txt", f"PkHello {version}\r\nA test package for gala install.\r\n".encode()),
            (d + "doc/", None),
            (d + "doc/LICENSE.txt", b"Test data of gala-os.\r\n" * 8),
        ],
    )


def main():
    # Installers built by tools that may be missing are kept across the
    # clean-up below.
    for name in ["pknsis-1.0.0.exe", "pknsis-1.1.0.exe"]:
        path = os.path.join(OUT, "installers", name)
        if os.path.exists(path):
            KEPT[name] = open(path, "rb").read()
    for d in [os.path.join(OUT, "installers"), os.path.join(OUT, "manifests", "g", "GalaTest"), SERVED]:
        if os.path.isdir(d):
            shutil.rmtree(d)
    h10 = pkhello_zip("1.0.0")
    h11 = pkhello_zip("1.1.0")
    # 1.0.0: one singleton manifest, installer fields at the root.
    write(
        "g/GalaTest/PkHello/1.0.0/GalaTest.PkHello.yaml",
        f"""# A test package of gala-os (pk_install_test).
PackageIdentifier: GalaTest.PkHello
PackageVersion: 1.0.0
PackageLocale: en-US
Publisher: gala-os
PackageName: PkHello
License: Proprietary
ShortDescription: Says hello (gala's install test)
Moniker: pkhello
Tags:
- test
- hello
InstallerType: zip
NestedInstallerType: portable
NestedInstallerFiles:
- RelativeFilePath: pkhello-1.0.0\\pkhello.exe
  PortableCommandAlias: pkhello
Installers:
- Architecture: x64
  InstallerUrl: {URL}pkhello-1.0.0.zip
  InstallerSha256: {h10}
ManifestType: singleton
ManifestVersion: 1.6.0
""",
    )
    # 1.1.0: the multi-file form, as winget-pkgs has it today.
    base = "g/GalaTest/PkHello/1.1.0/GalaTest.PkHello"
    write(
        base + ".yaml",
        """PackageIdentifier: GalaTest.PkHello
PackageVersion: 1.1.0
DefaultLocale: en-US
ManifestType: version
ManifestVersion: 1.12.0
""",
    )
    write(
        base + ".installer.yaml",
        f"""PackageIdentifier: GalaTest.PkHello
PackageVersion: 1.1.0
InstallerType: zip
NestedInstallerType: portable
Installers:
- Architecture: x86
  NestedInstallerFiles:
  - RelativeFilePath: pkhello-1.1.0/pkhello.exe
    PortableCommandAlias: pkhello
  InstallerUrl: {URL}pkhello-1.1.0-x86.zip
  InstallerSha256: {"0" * 64}
- Architecture: x64
  NestedInstallerFiles:
  - RelativeFilePath: pkhello-1.1.0/pkhello.exe
    PortableCommandAlias: pkhello
  InstallerUrl: {URL}pkhello-1.1.0.zip
  InstallerSha256: {h11}
ManifestType: installer
ManifestVersion: 1.12.0
""",
    )
    write(
        base + ".locale.en-US.yaml",
        """PackageIdentifier: GalaTest.PkHello
PackageVersion: 1.1.0
PackageLocale: en-US
Publisher: gala-os
PackageName: PkHello
License: Proprietary
ShortDescription: |-
  Says hello (gala's install test),
  version 1.1.0
Moniker: pkhello
Tags: [test, hello]
ManifestType: defaultLocale
ManifestVersion: 1.12.0
""",
    )
    # A package whose command is pkhello too: its install fails after
    # its files are written and is rolled back.
    hc = make_zip(
        "conflict-1.0.0.zip",
        [("conflict/pkhello.exe", HELLO), ("conflict/NOTES.txt", b"conflict\r\n")],
    )
    write(
        "g/GalaTest/Conflict/1.0.0/GalaTest.Conflict.yaml",
        f"""PackageIdentifier: GalaTest.Conflict
PackageVersion: 1.0.0
PackageLocale: en-US
Publisher: gala-os
PackageName: Conflict
License: Proprietary
ShortDescription: Wants the command pkhello, which PkHello has
Installers:
- Architecture: neutral
  InstallerType: zip
  NestedInstallerType: portable
  NestedInstallerFiles:
  - RelativeFilePath: conflict/pkhello.exe
    PortableCommandAlias: pkhello
  InstallerUrl: {URL}conflict-1.0.0.zip
  InstallerSha256: {hc}
ManifestType: singleton
ManifestVersion: 1.6.0
""",
    )
    # A download that is not what the manifest says.
    write(
        "g/GalaTest/BadHash/1.0.0/GalaTest.BadHash.yaml",
        f"""PackageIdentifier: GalaTest.BadHash
PackageVersion: 1.0.0
PackageLocale: en-US
Publisher: gala-os
PackageName: BadHash
License: Proprietary
ShortDescription: Its installer's SHA-256 is wrong
Installers:
- Architecture: x64
  InstallerType: zip
  NestedInstallerType: portable
  NestedInstallerFiles:
  - RelativeFilePath: pkhello-1.0.0/pkhello.exe
    PortableCommandAlias: badhash
  InstallerUrl: {URL}pkhello-1.0.0.zip
  InstallerSha256: {h10[:-4]}0000
ManifestType: singleton
ManifestVersion: 1.6.0
""",
    )
    # A stand-in for an Inno Setup installer: hello.exe, which ignores the
    # switches and registers nothing, so the install fails with "no
    # uninstall entry" (exit 4). A real Inno Setup installer is the
    # pk_install_inno scenario's (Inno Setup's own, a PREBUILT download,
    # with winget-pkgs' manifest in manifests/j, which this script leaves
    # alone); it needs WOW64 (every Inno Setup checks for x86 support).
    # The host tests run Inno's switches against a fake.
    hi = installer("innosetup-2.0.exe", HELLO)
    write(
        "g/GalaTest/InnoSetup/2.0/GalaTest.InnoSetup.yaml",
        f"""PackageIdentifier: GalaTest.InnoSetup
PackageVersion: "2.0"
PackageLocale: en-US
Publisher: gala-os
PackageName: InnoSetup
License: Proprietary
ShortDescription: A stand-in Inno Setup installer that registers nothing
Installers:
- Architecture: x64
  InstallerType: inno
  InstallerUrl: {URL}innosetup-2.0.exe
  InstallerSha256: {hi}
ManifestType: singleton
ManifestVersion: 1.6.0
""",
    )
    # An installer type of a later stage.
    write(
        "g/GalaTest/Msix/1.0.0/GalaTest.Msix.yaml",
        f"""PackageIdentifier: GalaTest.Msix
PackageVersion: 1.0.0
PackageLocale: en-US
Publisher: gala-os
PackageName: Msix
License: Proprietary
ShortDescription: An MSIX package (ROADMAP PK stage 4)
Installers:
- Architecture: x64
  InstallerType: msix
  InstallerUrl: {URL}msix-1.0.0.msix
  InstallerSha256: {"A" * 64}
ManifestType: singleton
ManifestVersion: 1.6.0
""",
    )
    # A real NSIS installer (nsis/pknsis.nsi, built with makensis for
    # x64): 1.0.0 and 1.1.0, per-user, its ARP entry GalaTest.PkNsis.
    for version in ["1.0.0", "1.1.0"]:
        h = nsis_installer(version)
        write(
            f"g/GalaTest/PkNsis/{version}/GalaTest.PkNsis.yaml",
            f"""# A test package of gala-os (pk_install_test nsis).
PackageIdentifier: GalaTest.PkNsis
PackageVersion: {version}
PackageLocale: en-US
Publisher: gala-os
PackageName: PkNsis
License: Proprietary
ShortDescription: Says hello, installed by an NSIS installer
Installers:
- Architecture: x64
  InstallerType: nullsoft
  Scope: user
  InstallerUrl: {URL}pknsis-{version}.exe
  InstallerSha256: {h}
ManifestType: singleton
ManifestVersion: 1.6.0
""",
        )
    # Dependencies (winget installs a package's PackageDependencies
    # first, each once; learn.microsoft.com "install command"): PkDep
    # needs PkLib 1.0.0 or later; PkDepMissing needs a package the source
    # does not have.
    for ident, deps in [
        ("PkLib", None),
        ("PkDep", ("GalaTest.PkLib", "1.0.0")),
        ("PkDepMissing", ("GalaTest.NoSuchLib", None)),
    ]:
        cmd = ident.lower()
        hz = make_zip(
            f"{cmd}-1.0.0.zip",
            [(f"{cmd}-1.0.0/{cmd}.exe", HELLO), (f"{cmd}-1.0.0/README.txt", f"{ident} 1.0.0\r\n".encode())],
        )
        dep_yaml = ""
        if deps:
            dep_yaml = f"  Dependencies:\n    PackageDependencies:\n    - PackageIdentifier: {deps[0]}\n"
            if deps[1]:
                dep_yaml += f"      MinimumVersion: {deps[1]}\n"
        write(
            f"g/GalaTest/{ident}/1.0.0/GalaTest.{ident}.yaml",
            f"""PackageIdentifier: GalaTest.{ident}
PackageVersion: 1.0.0
PackageLocale: en-US
Publisher: gala-os
PackageName: {ident}
License: Proprietary
ShortDescription: A test package with dependencies (gala's install test)
Installers:
- Architecture: x64
  InstallerType: zip
  NestedInstallerType: portable
  NestedInstallerFiles:
  - RelativeFilePath: {cmd}-1.0.0\\{cmd}.exe
    PortableCommandAlias: {cmd}
  InstallerUrl: {URL}{cmd}-1.0.0.zip
  InstallerSha256: {hz}
{dep_yaml}ManifestType: singleton
ManifestVersion: 1.6.0
""",
        )
    # Scope (`--scope`; an upgrade keeps the installed scope): a machine
    # and a user installer, the machine one listed first. The files are
    # the same; the URLs tell which installer was used.
    for version in ["1.0.0", "1.1.0"]:
        files = [
            (f"pkscoped-{version}/pkscoped.exe", HELLO),
            (f"pkscoped-{version}/README.txt", f"PkScoped {version}\r\n".encode()),
        ]
        hm = make_zip(f"pkscoped-{version}-machine.zip", files)
        hu = make_zip(f"pkscoped-{version}-user.zip", files)
        write(
            f"g/GalaTest/PkScoped/{version}/GalaTest.PkScoped.yaml",
            f"""PackageIdentifier: GalaTest.PkScoped
PackageVersion: {version}
PackageLocale: en-US
Publisher: gala-os
PackageName: PkScoped
License: Proprietary
ShortDescription: A user and a machine installer (gala's install test)
InstallerType: zip
NestedInstallerType: portable
NestedInstallerFiles:
- RelativeFilePath: pkscoped-{version}\\pkscoped.exe
  PortableCommandAlias: pkscoped
Installers:
- Architecture: x64
  Scope: machine
  InstallerUrl: {URL}pkscoped-{version}-machine.zip
  InstallerSha256: {hm}
- Architecture: x64
  Scope: user
  InstallerUrl: {URL}pkscoped-{version}-user.zip
  InstallerSha256: {hu}
ManifestType: singleton
ManifestVersion: 1.6.0
""",
        )
    # Downloaded over HTTPS (pk_install_https): the zip is served by
    # Nginx, not mirrored, so `gala install` fetches it through winhttp,
    # following the release URL's redirect, and checks its SHA-256.
    hh = make_zip(
        "pkhttps-1.0.0.zip",
        [("pkhttps-1.0.0/pkhttps.exe", HELLO), ("pkhttps-1.0.0/README.txt", b"PkHttps 1.0.0\r\n")],
        os.path.join(SERVED, "objects"),
    )
    # PkHttps's manifest is also served by Nginx as a winget-pkgs tree
    # (served/winget-pkgs, directory listings as JSON): `--source
    # https://localhost:8443/winget-pkgs` reads it over HTTPS.
    for ident, url, what, roots in [
        (
            "PkHttps",
            f"{NGINX}releases/download/v1.0.0/pkhttps-1.0.0.zip",
            "Downloaded over HTTPS",
            [None, os.path.join(SERVED, "winget-pkgs")],
        ),
        ("PkUntrusted", f"{UNTRUSTED}objects/pkhttps-1.0.0.zip", "Served with an untrusted certificate", [None]),
    ]:
        for root in roots:
            write(
                f"g/GalaTest/{ident}/1.0.0/GalaTest.{ident}.yaml",
                f"""PackageIdentifier: GalaTest.{ident}
PackageVersion: 1.0.0
PackageLocale: en-US
Publisher: gala-os
PackageName: {ident}
License: Proprietary
ShortDescription: {what} (gala's install test)
InstallerType: zip
NestedInstallerType: portable
NestedInstallerFiles:
- RelativeFilePath: pkhttps-1.0.0\\pkhttps.exe
  PortableCommandAlias: {ident.lower()}
Installers:
- Architecture: x64
  InstallerUrl: {url}
  InstallerSha256: {hh}
ManifestType: singleton
ManifestVersion: 1.6.0
""",
                root,
            )
    print("made", OUT, "and", SERVED, file=sys.stderr)


main()
