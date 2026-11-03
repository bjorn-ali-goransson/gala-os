#!/usr/bin/env python3
"""Makes pk_install_test's package source (user/pk_install_test/source):
a winget-pkgs tree with gala's test packages and the zips they install
(the `installers` mirror). The tree also holds copies of real winget-pkgs
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
import sys
import zipfile

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
OUT = os.path.join(ROOT, "user", "pk_install_test", "source")
HELLO = open(os.path.join(ROOT, "user", "bin", "hello.exe"), "rb").read()
STAMP = (2026, 11, 3, 0, 0, 0)
URL = "https://packages.gala.invalid/"


def make_zip(name, files):
    path = os.path.join(OUT, "installers", name)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with zipfile.ZipFile(path, "w") as z:
        for entry, data in files:
            info = zipfile.ZipInfo(entry, STAMP)
            info.external_attr = (0o40755 << 16) | 0x10 if data is None else 0o644 << 16
            info.compress_type = zipfile.ZIP_STORED if data is None else zipfile.ZIP_DEFLATED
            z.writestr(info, b"" if data is None else data)
    return hashlib.sha256(open(path, "rb").read()).hexdigest().upper()


def write(rel, text):
    path = os.path.join(OUT, "manifests", rel)
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
    for d in [os.path.join(OUT, "installers"), os.path.join(OUT, "manifests", "g", "GalaTest")]:
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
    # An installer type of a later stage.
    write(
        "g/GalaTest/InnoSetup/2.0/GalaTest.InnoSetup.yaml",
        f"""PackageIdentifier: GalaTest.InnoSetup
PackageVersion: "2.0"
PackageLocale: en-US
Publisher: gala-os
PackageName: InnoSetup
License: Proprietary
ShortDescription: An Inno Setup installer (ROADMAP PK stage 3)
Installers:
- Architecture: x64
  InstallerType: inno
  InstallerUrl: {URL}setup.exe
  InstallerSha256: {"A" * 64}
  InstallerSwitches:
    Silent: /VERYSILENT /SUPPRESSMSGBOXES /NORESTART
ManifestType: singleton
ManifestVersion: 1.6.0
""",
    )
    print("made", OUT, file=sys.stderr)


main()
