//! MSIX reader tests over the fixtures of testdata/msix (made by
//! make_fixtures.py, cross-checked by check_fixtures.sh with unzip, 7z
//! and sha256sum) and packages built here.

use super::*;
use crate::appxmanifest::NS_RESCAP;
use alloc::vec;

const APP: &[u8] = include_bytes!("../../testdata/msix/app.msix");
const ZIPFILE: &[u8] = include_bytes!("../../testdata/msix/zipfile.msix");
const UNSIGNED: &[u8] = include_bytes!("../../testdata/msix/unsigned.msix");
const BUNDLE: &[u8] = include_bytes!("../../testdata/msix/app.msixbundle");
const PUBLISHER: &str = "CN=Gala Test Publisher, O=Gala, C=US";
const MICROSOFT: &str =
    "CN=Microsoft Corporation, O=Microsoft Corporation, L=Redmond, S=Washington, C=US";

fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/testdata/msix/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn problems(data: &[u8]) -> Vec<String> {
    let p = Package::open(data).unwrap();
    p.verify().unwrap_err().0.iter().map(|e| format!("{e}")).collect()
}

#[test]
fn publisher_id_matches_microsofts_documented_example() {
    // learn.microsoft.com "Check for installed versions of the Windows App
    // SDK runtime": Publisher (as above), PublisherId 8wekyb3d8bbwe,
    // PackageFullName Microsoft.WindowsAppRuntime.1.0_0.318.928.0_x64__8wekyb3d8bbwe.
    assert_eq!(publisher_id(MICROSOFT), "8wekyb3d8bbwe");
    let id = Identity {
        name: "Microsoft.WindowsAppRuntime.1.0".into(),
        publisher: MICROSOFT.into(),
        version: Version([0, 318, 928, 0]),
        architecture: "x64".into(),
        resource_id: None,
        bundle: false,
    };
    assert_eq!(id.full_name(), "Microsoft.WindowsAppRuntime.1.0_0.318.928.0_x64__8wekyb3d8bbwe");
    assert_eq!(id.family_name(), "Microsoft.WindowsAppRuntime.1.0_8wekyb3d8bbwe");
    // Our test publisher: from the same algorithm only (no documented value).
    assert_eq!(publisher_id(PUBLISHER), "dr59fkesnp31w");
}

#[test]
fn package_strings_and_versions() {
    assert!(is_package_string("Gala.TestApp", 3, 50));
    assert!(!is_package_string("ab", 3, 50));
    assert!(!is_package_string("a_b", 1, 50));
    assert!(!is_package_string("con", 1, 50));
    assert!(!is_package_string("CON.app", 1, 50));
    assert!(is_package_string("console", 1, 50));
    assert!(!is_package_string("xn--abc", 1, 50));
    assert!(!is_package_string("a.xn--b", 1, 50));
    assert!(!is_package_string("app.", 1, 50));
    assert_eq!(Version::parse("1.2.3.4"), Some(Version([1, 2, 3, 4])));
    assert_eq!(Version::parse("65535.0.0.0"), Some(Version([65535, 0, 0, 0])));
    assert_eq!(Version::parse("65536.0.0.0"), None);
    assert_eq!(Version::parse("1.2.3"), None);
    assert_eq!(Version::parse("1.2.3.4.5"), None);
    assert_eq!(Version::parse("1.2.+3.4"), None);
    assert!(Version([1, 10, 0, 0]) > Version([1, 9, 9, 9]));
}

#[test]
fn opens_and_verifies_the_package() {
    let p = Package::open(APP).unwrap();
    let id = p.identity();
    assert_eq!(id.name, "Gala.TestApp");
    assert_eq!(id.publisher, PUBLISHER);
    assert_eq!(id.version, Version([1, 2, 3, 0]));
    assert_eq!(id.architecture, "x64");
    assert_eq!(id.resource_id, None);
    assert_eq!(id.full_name(), "Gala.TestApp_1.2.3.0_x64__dr59fkesnp31w");
    assert_eq!(id.family_name(), "Gala.TestApp_dr59fkesnp31w");
    assert!(!id.unsigned_publisher());

    let m = &p.manifest;
    assert_eq!(m.properties.display_name, "Gala Test App");
    assert_eq!(m.properties.publisher_display_name, "Gala & Friends");
    assert_eq!(m.properties.logo, "Assets\\My Logo.png");
    assert_eq!(m.languages, vec!["en-us"]);
    assert_eq!(m.applications.len(), 1);
    let app = &m.applications[0];
    assert_eq!(app.id, "App");
    assert_eq!(app.executable.as_deref(), Some("app.exe"));
    assert_eq!(app.entry_point.as_deref(), Some("Windows.FullTrustApplication"));
    let v = app.visual_elements.as_ref().unwrap();
    assert_eq!(v.display_name, "Gala Test App");
    assert_eq!(v.square44x44_logo, "Assets\\My Logo.png");
    assert_eq!(v.background_color, "transparent");
    assert!(m.run_full_trust());
    assert_eq!(m.capabilities.len(), 2);
    assert_eq!(m.capabilities[0].name, "internetClient");
    assert_eq!(m.capabilities[1].ns, NS_RESCAP);
    let t = &m.target_device_families[0];
    assert_eq!(t.name, "Windows.Desktop");
    assert_eq!(t.min_version, Version([10, 0, 17763, 0]));
    assert_eq!(t.max_version_tested, Some(Version([10, 0, 26100, 0])));
    let d = &m.package_dependencies[0];
    assert_eq!(d.name, "Microsoft.VCLibs.140.00.UWPDesktop");
    assert_eq!(publisher_id(&d.publisher), "8wekyb3d8bbwe");
    assert_eq!(d.min_version, Version([14, 0, 30704, 0]));

    assert_eq!(p.block_map.hash_method, blockmap::HashMethod::Sha256);
    assert_eq!(p.signature, Some(39));
    let c = p.verify().unwrap();
    assert_eq!(c, Checked { files: 6, blocks: 8, bytes: 1604 + 208 + 159000 + 65836 + 1851 });
    assert_eq!(c.summary(), "6 files, 8 blocks, 228499 bytes");

    // Files as an installer sees them; the space is %20 in the zip.
    let files = p.files();
    let logo = files.iter().find(|f| f.name == "Assets\\My Logo.png").unwrap();
    assert_eq!(logo.path.as_deref(), Some("Assets\\My Logo.png"));
    assert!(p.archive.find("Assets/My%20Logo.png").is_some());
    assert!(files.iter().find(|f| f.name == "AppxManifest.xml").unwrap().footprint);
    assert!(!files.iter().find(|f| f.name == "app.exe").unwrap().footprint);
    assert_eq!(p.read("Assets\\My Logo.png").unwrap().len(), 208);
    assert_eq!(p.read("data/big.txt").unwrap().len(), 159000);
    assert!(p.read("data/empty.txt").unwrap().is_empty());
    assert_eq!(
        p.read("[Content_Types].xml").err(),
        Some(Error::Unlisted { file: "[Content_Types].xml".into() })
    );
    assert_eq!(p.content_types.lookup("app.exe"), Some("application/x-msdownload"));

    // open() of either kind.
    assert!(matches!(open(APP), Ok(Opened::Package(_))));
    assert!(matches!(open(BUNDLE), Ok(Opened::Bundle(_))));
}

#[test]
fn package_written_by_pythons_zipfile() {
    // Classic zip (no ZIP64, no data descriptors), all stored: no block
    // has a Size, and LfhSize is 30 + the name (zipfile writes no extra).
    let p = Package::open(ZIPFILE).unwrap();
    assert_eq!(p.identity().architecture, "neutral");
    assert_eq!(p.identity().full_name(), "Gala.TestApp_1.2.3.0_neutral__dr59fkesnp31w");
    assert_eq!(p.signature, None);
    assert_eq!(p.verify().unwrap().files, 3);
}

#[test]
fn unsigned_package_has_no_signature() {
    let p = Package::open(UNSIGNED).unwrap();
    assert_eq!(p.signature, None);
    assert!(p.verify().is_ok());
}

#[test]
fn a_changed_block_is_named() {
    // big.txt's block 1 differs from what the block map hashed; the zip
    // itself is valid (its CRC-32 is of the changed data).
    assert_eq!(
        problems(&fixture("bad_hash.msix")),
        vec![
            "data\\big.txt block 1: hash mismatch: the block map says \
             LYlAveNTsGo6CpIvTXhybyAyS1d6xm0OiLj9ZQkT1rM=, the data hashes to \
             tEmIMuJk6SY43cZh+d0qBysYagNyDwUjb/YHI2GUJIU="
        ]
    );
}

#[test]
fn a_flipped_stored_byte_is_named_before_the_crc() {
    let mut data = APP.to_vec();
    let at = {
        let a = Archive::open(APP).unwrap();
        a.data_offset(a.find("data/blob.bin").unwrap()).unwrap() + 65536 + 7
    };
    data[at] ^= 1;
    let p = problems(&data);
    assert_eq!(p.len(), 1);
    assert!(p[0].starts_with("data\\blob.bin block 1: hash mismatch: the block map says /3N17NcPNbfpC2NS/2HmDplBuZhzSv0cDDlebkjIR5M=, the data hashes to "), "{p:?}");
    // In block 0 of the deflated big.txt: the run no longer inflates to
    // the block, or the block's hash is wrong; either way block 0.
    let mut data = APP.to_vec();
    let at = {
        let a = Archive::open(APP).unwrap();
        a.data_offset(a.find("data/big.txt").unwrap()).unwrap() + 100
    };
    data[at] ^= 0x10;
    let p = problems(&data);
    assert_eq!(p.len(), 1);
    assert!(p[0].starts_with("data\\big.txt block 0: "), "{p:?}");
}

#[test]
fn missing_unlisted_and_extra_files() {
    assert_eq!(
        problems(&fixture("missing_file.msix")),
        vec!["app.exe: listed in the block map (line 3) but not in the package"]
    );
    assert_eq!(
        problems(&fixture("extra_listed.msix")),
        vec!["Assets\\ghost.png: listed in the block map (line 12) but not in the package"]
    );
    assert_eq!(
        problems(&fixture("unlisted_file.msix")),
        vec!["extra.dll: in the package but not in the block map"]
    );
}

#[test]
fn wrong_sizes_are_named() {
    assert_eq!(
        problems(&fixture("wrong_size.msix")),
        vec!["app.exe: Size (uncompressed bytes): the block map says 1605, the package has 1604"]
    );
    assert_eq!(
        problems(&fixture("wrong_lfh.msix")),
        vec![
            "Assets\\My Logo.png: LfhSize (local file header bytes): the block map says 50, \
             the package has 70"
        ]
    );
    assert_eq!(
        problems(&fixture("wrong_block_size.msix")),
        vec![
            "data\\big.txt: compressed size (the sum of the blocks' Size): the block map says \
             1633, the package has 1630"
        ]
    );
}

#[test]
fn opens_the_bundle_and_its_packages() {
    let b = Bundle::open(BUNDLE).unwrap();
    let id = b.identity();
    assert!(id.bundle);
    assert_eq!(id.full_name(), "Gala.TestApp_1.2.3.0_neutral_~_dr59fkesnp31w");
    assert_eq!(id.family_name(), "Gala.TestApp_dr59fkesnp31w");
    assert_eq!(b.manifest.schema_version.as_deref(), Some("5.0"));
    assert_eq!(b.signature, Some(39));
    let names: Vec<&str> = b.manifest.packages.iter().map(|p| p.file_name.as_str()).collect();
    assert_eq!(names, ["Gala.TestApp_1.2.3.0_x64.msix", "Gala.TestApp_1.2.3.0_x86.msix"]);
    assert_eq!(b.manifest.packages[0].kind, "application");
    assert_eq!(b.manifest.packages[0].languages, vec!["en-us"]);
    assert_eq!(b.verify().unwrap().files, 3);
    let x86 = b.package_for("x86").unwrap();
    let p = b.open_package(x86).unwrap();
    assert_eq!(p.identity().full_name(), "Gala.TestApp_1.2.3.0_x86__dr59fkesnp31w");
    assert_eq!(p.verify().unwrap().files, 3);
    assert_eq!(b.package_for("arm64"), None);
    // The inner package's offset: the bundle's bytes from Offset on.
    let x64 = b.package_for("x64").unwrap();
    let start = x64.offset as usize;
    assert!(Package::open(&BUNDLE[start..start + x64.size as usize]).is_ok());
}

#[test]
fn a_bundle_offset_that_is_wrong_is_named() {
    let data = fixture("bad_offset.msixbundle");
    let b = Bundle::open(&data).unwrap();
    let p = b.verify().unwrap_err().0;
    assert_eq!(p.len(), 1);
    let msg = format!("{}", p[0]);
    assert!(
        msg.starts_with(
            "bundle package Gala.TestApp_1.2.3.0_x86.msix: the manifest (line 17) says Offset "
        ),
        "{msg}"
    );
    assert!(b.open_package(&b.manifest.packages[1]).is_err());
    assert!(b.open_package(&b.manifest.packages[0]).is_ok());
}

#[test]
fn a_bundle_is_not_a_package() {
    let e = Package::open(BUNDLE).err().unwrap();
    assert_eq!(
        format!("{e}"),
        "AppxManifest.xml (this is a bundle) is missing (every package has one)"
    );
    assert!(Bundle::open(APP).is_err());
}

/// A stored package built here: (name, data) payload files, the block
/// map made by `map(files)` (our own hashes: self-consistency only).
fn build(files: &[(&str, &[u8])], map: impl Fn(&[(&str, &[u8])]) -> String) -> Vec<u8> {
    let mut all: Vec<(&str, &[u8])> = files.to_vec();
    let bm = map(files);
    all.push(("AppxBlockMap.xml", bm.as_bytes()));
    let ct = "<Types xmlns='http://schemas.openxmlformats.org/package/2006/content-types'>\
              <Default Extension='xml' ContentType='text/xml'/>\
              <Default Extension='dll' ContentType='application/x-msdownload'/></Types>";
    all.push(("[Content_Types].xml", ct.as_bytes()));
    zip::writer::build(&all, false)
}

const MANIFEST: &str = "<Package xmlns='http://schemas.microsoft.com/appx/manifest/foundation/windows10'>\n\
    <Identity Name='Built.Here' Publisher='CN=Built, OID.2.25.311729368913984317654407730594956997722=1' \
     Version='2.0.0.0' ProcessorArchitecture='arm64' ResourceId='en-gb'/>\n\
    <Properties><DisplayName>B</DisplayName><PublisherDisplayName>B</PublisherDisplayName><Logo>x</Logo></Properties>\n\
    </Package>";

fn stored_map(method: blockmap::HashMethod, uri: &str, files: &[(&str, &[u8])]) -> String {
    let mut s = format!("<BlockMap xmlns='{}' HashMethod='{uri}'>", blockmap::NS);
    for (name, data) in files {
        s += &format!("<File Name='{name}' Size='{}' LfhSize='{}'>", data.len(), 30 + name.len());
        for chunk in data.chunks(blockmap::BLOCK) {
            s += &format!("<Block Hash='{}'/>", blockmap::base64(&method.hash(chunk)));
        }
        s += "</File>";
    }
    s + "</BlockMap>"
}

#[test]
fn sha384_and_sha512_block_maps() {
    let big = vec![0x5au8; 70000];
    let files: [(&str, &[u8]); 2] = [("AppxManifest.xml", MANIFEST.as_bytes()), ("lib.dll", &big)];
    for (m, uri) in [
        (blockmap::HashMethod::Sha384, "http://www.w3.org/2001/04/xmldsig-more#sha384"),
        (blockmap::HashMethod::Sha512, "http://www.w3.org/2001/04/xmlenc#sha512"),
    ] {
        let z = build(&files, |f| stored_map(m, uri, f));
        let p = Package::open(&z).unwrap();
        assert_eq!(p.block_map.hash_method, m);
        assert_eq!(p.verify().unwrap().blocks, 3);
        let id = p.identity();
        assert!(id.unsigned_publisher());
        assert_eq!(id.resource_id.as_deref(), Some("en-gb"));
        assert!(id.full_name().starts_with("Built.Here_2.0.0.0_arm64_en-gb_"));
    }
    // A SHA-256 hash in a SHA-512 block map is not even parsed.
    let z = build(&files, |f| {
        stored_map(blockmap::HashMethod::Sha256, "http://www.w3.org/2001/04/xmlenc#sha512", f)
    });
    let e = format!("{}", Package::open(&z).err().unwrap());
    assert!(e.starts_with("AppxBlockMap.xml line 1: <Block>: Hash=\""), "{e}");
}

#[test]
fn manifest_damage_is_found_at_open() {
    // The manifest is checked against its block map before it is parsed.
    let files: [(&str, &[u8]); 1] = [("AppxManifest.xml", MANIFEST.as_bytes())];
    let good =
        stored_map(blockmap::HashMethod::Sha256, "http://www.w3.org/2001/04/xmlenc#sha256", &files);
    let changed = MANIFEST.replace("2.0.0.0", "3.0.0.0");
    let z = build(&[("AppxManifest.xml", changed.as_bytes())], |_| good.clone());
    let e = format!("{}", Package::open(&z).err().unwrap());
    assert!(e.starts_with("AppxManifest.xml block 0: hash mismatch"), "{e}");
    // A bad manifest names its line.
    let bad = MANIFEST.replace("Version='2.0.0.0'", "Version='2.0'");
    let z = build(&[("AppxManifest.xml", bad.as_bytes())], |f| {
        stored_map(blockmap::HashMethod::Sha256, "http://www.w3.org/2001/04/xmlenc#sha256", f)
    });
    let e = format!("{}", Package::open(&z).err().unwrap());
    assert_eq!(
        e,
        "AppxManifest.xml line 2: <Identity>: Version=\"2.0\" is not a version \
         Major.Minor.Build.Revision (0-65535)"
    );
    // No block map at all.
    let z = zip::writer::build(&[("AppxManifest.xml", MANIFEST.as_bytes())], false);
    assert_eq!(
        format!("{}", Package::open(&z).err().unwrap()),
        "[Content_Types].xml is missing (every package has one)"
    );
    // Not a zip.
    assert!(matches!(Package::open(b"not a package"), Err(Error::Zip { .. })));
}

#[test]
fn duplicate_and_content_type_problems() {
    let files: [(&str, &[u8]); 2] = [("AppxManifest.xml", MANIFEST.as_bytes()), ("x.bin", b"x")];
    let z = build(&files, |f| {
        let s =
            stored_map(blockmap::HashMethod::Sha256, "http://www.w3.org/2001/04/xmlenc#sha256", f);
        let dup = "<File Name='X.BIN' Size='1' LfhSize='35'><Block Hash='LXBKN0ru2BzKV9VWgKFOqcXAkSugbJmeNmrNFrMBhL8='/></File>";
        s.replace("</BlockMap>", &format!("{dup}</BlockMap>"))
    });
    let p = problems(&z);
    assert_eq!(
        p,
        vec![
            "AppxBlockMap.xml line 1: <File>: X.BIN listed twice (first on line 1)".to_string(),
            "x.bin: no content type in [Content_Types].xml".to_string(),
        ]
    );
}

#[test]
fn damage_anywhere_is_an_error_not_a_panic() {
    // gala.exe must not panic on a hostile package: flip bytes and cut
    // the archives short all over; every outcome is a Result.
    for src in [APP, BUNDLE, ZIPFILE] {
        for at in (0..src.len()).step_by(src.len() / 200 + 1) {
            let mut d = src.to_vec();
            d[at] ^= 0xa5;
            let _ = verify_all(&d);
            let _ = verify_all(&src[..at]);
        }
    }
}

fn verify_all(d: &[u8]) -> Result<(), ()> {
    match open(d).map_err(|_| ())? {
        Opened::Package(p) => p.verify().map(|_| ()).map_err(|_| ()),
        Opened::Bundle(b) => {
            let _ = b.verify();
            for p in &b.manifest.packages {
                if let Ok(inner) = b.open_package(p) {
                    let _ = inner.verify();
                }
            }
            Ok(())
        }
    }
}
