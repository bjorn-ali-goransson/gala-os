//! The XML documents of an MSIX/AppX package other than the block map:
//! AppxManifest.xml (Microsoft's "App package manifest schema
//! reference"), AppxMetadata\AppxBundleManifest.xml (the "Bundle
//! manifest schema reference") and OPC's [Content_Types].xml (ECMA-376
//! Part 2, Open Packaging Conventions).
//!
//! Read: Identity, Properties, Dependencies (TargetDeviceFamily,
//! PackageDependency), Resources, Applications (Id, Executable,
//! EntryPoint, the VisualElements basics) and Capabilities (any
//! namespace: foundation, uap*, rescap, DeviceCapability). Everything
//! else (Extensions and the many other namespaces) is ignored, not an
//! error, as IgnorableNamespaces intends. Errors name the file, the
//! element and its line.

use crate::msix::{Error, Identity, Version};
use crate::xml::{self, Element};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Windows 10 and later manifests.
pub const NS_FOUNDATION: &str = "http://schemas.microsoft.com/appx/manifest/foundation/windows10";
/// Windows 8 (2010) and 8.1 (2013) manifests share this root namespace.
pub const NS_2010: &str = "http://schemas.microsoft.com/appx/2010/manifest";
/// The universal app platform namespace (uap); uap2... are
/// `.../uap/windows10/N`.
pub const NS_UAP: &str = "http://schemas.microsoft.com/appx/manifest/uap/windows10";
/// Restricted capabilities (rescap), where runFullTrust lives.
pub const NS_RESCAP: &str =
    "http://schemas.microsoft.com/appx/manifest/foundation/windows10/restrictedcapabilities";
/// The bundle manifest namespace (later bundle schema versions add
/// namespaces for extensions, the root stays in this one).
pub const NS_BUNDLE: &str = "http://schemas.microsoft.com/appx/2013/bundle";
/// [Content_Types].xml's namespace.
pub const NS_CONTENT_TYPES: &str = "http://schemas.openxmlformats.org/package/2006/content-types";

pub const MANIFEST: &str = "AppxManifest.xml";
pub const BUNDLE_MANIFEST: &str = "AppxMetadata/AppxBundleManifest.xml";
pub const CONTENT_TYPES: &str = "[Content_Types].xml";

fn err(file: &str, el: &Element, msg: impl Into<String>) -> Error {
    Error::Manifest { file: file.into(), element: el.qname.clone(), line: el.line, msg: msg.into() }
}

fn required<'e>(file: &str, el: &'e Element, attr: &str) -> Result<&'e str, Error> {
    el.attr(attr).ok_or_else(|| err(file, el, format!("no {attr} attribute")))
}

fn parse_xml(file: &str, bytes: &[u8]) -> Result<Element, Error> {
    xml::parse(bytes).map_err(|e| Error::Xml { file: file.into(), err: e })
}

fn opt(el: &Element, attr: &str) -> Option<String> {
    el.attr(attr).map(String::from)
}

/// Properties.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Properties {
    pub display_name: String,
    pub publisher_display_name: String,
    pub logo: String,
    pub description: Option<String>,
    /// `<Framework>true</Framework>`: a framework package.
    pub framework: bool,
    /// `<ResourcePackage>true</ResourcePackage>`.
    pub resource_package: bool,
}

/// uap:VisualElements (the basics).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VisualElements {
    pub display_name: String,
    pub description: String,
    pub background_color: String,
    pub square150x150_logo: String,
    pub square44x44_logo: String,
}

/// An Application.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Application {
    /// The package-relative application id (PRAID).
    pub id: String,
    pub executable: Option<String>,
    pub entry_point: Option<String>,
    pub visual_elements: Option<VisualElements>,
    pub line: u32,
}

/// A capability: `Capability` in some namespace, or `DeviceCapability`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Capability {
    pub ns: String,
    pub name: String,
    pub device: bool,
}

/// TargetDeviceFamily.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetDeviceFamily {
    pub name: String,
    pub min_version: Version,
    pub max_version_tested: Option<Version>,
}

/// PackageDependency.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageDependency {
    pub name: String,
    pub publisher: String,
    pub min_version: Version,
}

/// AppxManifest.xml.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppxManifest {
    /// The root's namespace (Windows 10 foundation or the 2010 one).
    pub ns: String,
    pub identity: Identity,
    pub properties: Properties,
    pub target_device_families: Vec<TargetDeviceFamily>,
    pub package_dependencies: Vec<PackageDependency>,
    /// Resource Language values.
    pub languages: Vec<String>,
    pub applications: Vec<Application>,
    pub capabilities: Vec<Capability>,
}

impl AppxManifest {
    /// Whether the package declares `rescap:Capability Name="runFullTrust"`
    /// (a full-trust Win32 program).
    pub fn run_full_trust(&self) -> bool {
        self.capabilities.iter().any(|c| c.ns == NS_RESCAP && c.name == "runFullTrust")
    }

    pub fn parse(file: &str, bytes: &[u8]) -> Result<AppxManifest, Error> {
        let root = parse_xml(file, bytes)?;
        let ns = root.ns.as_str();
        if root.name != "Package" || (ns != NS_FOUNDATION && ns != NS_2010) {
            return Err(err(file, &root, "the root is not a foundation or 2010 manifest Package"));
        }
        let id =
            root.child(ns, "Identity").ok_or_else(|| err(file, &root, "no Identity element"))?;
        let identity = Identity::from_element(file, id, true)?;
        let mut properties = Properties::default();
        if let Some(p) = root.child(ns, "Properties") {
            let text = |n: &str| p.child(ns, n).map(|e| e.text.trim().to_string());
            properties.display_name = text("DisplayName").unwrap_or_default();
            properties.publisher_display_name = text("PublisherDisplayName").unwrap_or_default();
            properties.logo = text("Logo").unwrap_or_default();
            properties.description = text("Description");
            properties.framework = text("Framework").is_some_and(|t| t == "true");
            properties.resource_package = text("ResourcePackage").is_some_and(|t| t == "true");
        } else {
            return Err(err(file, &root, "no Properties element"));
        }
        let mut m = AppxManifest {
            ns: ns.into(),
            identity,
            properties,
            target_device_families: Vec::new(),
            package_dependencies: Vec::new(),
            languages: Vec::new(),
            applications: Vec::new(),
            capabilities: Vec::new(),
        };
        if let Some(d) = root.child(ns, "Dependencies") {
            for t in d.children_named(ns, "TargetDeviceFamily") {
                m.target_device_families.push(TargetDeviceFamily {
                    name: required(file, t, "Name")?.into(),
                    min_version: Version::attr(file, t, "MinVersion")?,
                    max_version_tested: match t.attr("MaxVersionTested") {
                        Some(_) => Some(Version::attr(file, t, "MaxVersionTested")?),
                        None => None,
                    },
                });
            }
            for p in d.children_named(ns, "PackageDependency") {
                m.package_dependencies.push(PackageDependency {
                    name: required(file, p, "Name")?.into(),
                    publisher: required(file, p, "Publisher")?.into(),
                    min_version: Version::attr(file, p, "MinVersion")?,
                });
            }
        }
        if let Some(r) = root.child(ns, "Resources") {
            for res in r.children_named(ns, "Resource") {
                if let Some(l) = res.attr("Language") {
                    m.languages.push(l.into());
                }
            }
        }
        if let Some(apps) = root.child(ns, "Applications") {
            for a in apps.children_named(ns, "Application") {
                let id = required(file, a, "Id")?;
                if id.is_empty() || m.applications.iter().any(|x: &Application| x.id == id) {
                    return Err(err(file, a, format!("Id=\"{id}\" is empty or not unique")));
                }
                // uap:VisualElements in Windows 10 manifests, the root
                // namespace in Windows 8 ones.
                let visual = a
                    .children
                    .iter()
                    .find(|c| {
                        c.name == "VisualElements" && (c.ns == ns || c.ns.starts_with(NS_UAP))
                    })
                    .map(|v| VisualElements {
                        display_name: v.attr("DisplayName").unwrap_or_default().into(),
                        description: v.attr("Description").unwrap_or_default().into(),
                        background_color: v.attr("BackgroundColor").unwrap_or_default().into(),
                        square150x150_logo: v
                            .attr("Square150x150Logo")
                            .or(v.attr("Logo"))
                            .unwrap_or_default()
                            .into(),
                        square44x44_logo: v
                            .attr("Square44x44Logo")
                            .or(v.attr("SmallLogo"))
                            .unwrap_or_default()
                            .into(),
                    });
                m.applications.push(Application {
                    id: id.into(),
                    executable: opt(a, "Executable"),
                    entry_point: opt(a, "EntryPoint"),
                    visual_elements: visual,
                    line: a.line,
                });
            }
        }
        if let Some(caps) = root.child(ns, "Capabilities") {
            for c in &caps.children {
                let device = c.name == "DeviceCapability";
                if c.name == "Capability" || device {
                    m.capabilities.push(Capability {
                        ns: c.ns.clone(),
                        name: required(file, c, "Name")?.into(),
                        device,
                    });
                }
            }
        }
        Ok(m)
    }
}

/// A Package of a bundle manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundlePackage {
    /// "application" or "resource" (the schema's default).
    pub kind: String,
    pub version: Version,
    pub architecture: String,
    pub resource_id: Option<String>,
    pub file_name: String,
    pub offset: u64,
    pub size: u64,
    pub languages: Vec<String>,
    pub line: u32,
}

/// AppxMetadata\AppxBundleManifest.xml.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundleManifest {
    pub schema_version: Option<String>,
    /// Name, Publisher, Version (a bundle has no architecture; its
    /// ResourceId is `~`).
    pub identity: Identity,
    pub packages: Vec<BundlePackage>,
}

impl BundleManifest {
    pub fn parse(file: &str, bytes: &[u8]) -> Result<BundleManifest, Error> {
        let root = parse_xml(file, bytes)?;
        if !root.is(NS_BUNDLE, "Bundle") {
            return Err(err(file, &root, format!("the root is not {{{NS_BUNDLE}}}Bundle")));
        }
        let id = root
            .child(NS_BUNDLE, "Identity")
            .ok_or_else(|| err(file, &root, "no Identity element"))?;
        let identity = Identity::from_element(file, id, false)?;
        let list = root
            .child(NS_BUNDLE, "Packages")
            .ok_or_else(|| err(file, &root, "no Packages element"))?;
        let mut packages = Vec::new();
        for p in list.children_named(NS_BUNDLE, "Package") {
            let num = |a: &str| -> Result<u64, Error> {
                let v = required(file, p, a)?;
                v.parse().map_err(|_| err(file, p, format!("{a}=\"{v}\" is not a number")))
            };
            let file_name = required(file, p, "FileName")?;
            if file_name.is_empty() || file_name.contains(['<', '>', ':', '"', '|', '?', '*']) {
                return Err(err(file, p, format!("FileName=\"{file_name}\" is not a valid name")));
            }
            let mut languages = Vec::new();
            if let Some(r) = p.child(NS_BUNDLE, "Resources") {
                for res in r.children_named(NS_BUNDLE, "Resource") {
                    if let Some(l) = res.attr("Language") {
                        languages.push(l.into());
                    }
                }
            }
            packages.push(BundlePackage {
                kind: p.attr("Type").unwrap_or("resource").into(),
                version: Version::attr(file, p, "Version")?,
                architecture: p.attr("Architecture").unwrap_or("neutral").into(),
                resource_id: opt(p, "ResourceId"),
                file_name: file_name.into(),
                offset: num("Offset")?,
                size: num("Size")?,
                languages,
                line: p.line,
            });
        }
        if packages.is_empty() {
            return Err(err(file, list, "no Package elements"));
        }
        Ok(BundleManifest { schema_version: opt(&root, "SchemaVersion"), identity, packages })
    }
}

/// [Content_Types].xml: Default (by extension) and Override (by part
/// name) content types.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContentTypes {
    /// (extension, content type).
    pub defaults: Vec<(String, String)>,
    /// (part name with its leading `/`, content type).
    pub overrides: Vec<(String, String)>,
}

impl ContentTypes {
    pub fn parse(file: &str, bytes: &[u8]) -> Result<ContentTypes, Error> {
        let root = parse_xml(file, bytes)?;
        if !root.is(NS_CONTENT_TYPES, "Types") {
            return Err(err(file, &root, format!("the root is not {{{NS_CONTENT_TYPES}}}Types")));
        }
        let mut ct = ContentTypes::default();
        for c in &root.children {
            if c.is(NS_CONTENT_TYPES, "Default") {
                let ext = required(file, c, "Extension")?;
                if ct.defaults.iter().any(|(e, _)| e.eq_ignore_ascii_case(ext)) {
                    return Err(err(file, c, format!("Extension \"{ext}\" given twice")));
                }
                ct.defaults.push((ext.into(), required(file, c, "ContentType")?.into()));
            } else if c.is(NS_CONTENT_TYPES, "Override") {
                let part = required(file, c, "PartName")?;
                if !part.starts_with('/') {
                    return Err(err(file, c, format!("PartName \"{part}\" does not start with /")));
                }
                ct.overrides.push((part.into(), required(file, c, "ContentType")?.into()));
            }
        }
        Ok(ct)
    }

    /// The content type of the zip item `zip_name` (an Override for its
    /// part name, else the Default for its extension; ASCII
    /// case-insensitive, as OPC compares part names).
    pub fn lookup(&self, zip_name: &str) -> Option<&str> {
        let part = format!("/{}", zip_name.trim_start_matches('/'));
        if let Some((_, t)) = self.overrides.iter().find(|(p, _)| p.eq_ignore_ascii_case(&part)) {
            return Some(t);
        }
        let last = part.rsplit('/').next().unwrap_or("");
        let ext = last.rsplit_once('.').map(|(_, e)| e)?;
        self.defaults.iter().find(|(e, _)| e.eq_ignore_ascii_case(ext)).map(|(_, t)| t.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str =
        r#"<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10">"#;

    fn manifest(body: &str) -> Result<AppxManifest, Error> {
        AppxManifest::parse(MANIFEST, format!("{HEAD}\n{body}\n</Package>").as_bytes())
    }

    #[test]
    fn errors_name_file_element_and_line() {
        let e = manifest("<Identity Name='Ab' Publisher='CN=x' Version='1.0.0.0'/>").unwrap_err();
        assert_eq!(
            format!("{e}"),
            "AppxManifest.xml line 2: <Identity>: Name=\"Ab\" is not a package string of 3 to 50 \
             characters (letters, digits, '.', '-')"
        );
        let e =
            manifest("<Identity Name='Abc' Publisher='CN=x' Version='1.0.70000.0'/>").unwrap_err();
        assert!(format!("{e}").contains("Version=\"1.0.70000.0\""), "{e}");
        let e = manifest("<Identity Name='Abc' Version='1.0.0.0'/>").unwrap_err();
        assert!(format!("{e}").contains("no Publisher attribute"), "{e}");
        let e = manifest(
            "<Identity Name='Abc' Publisher='CN=x' Version='1.0.0.0' ProcessorArchitecture='mips'/>",
        )
        .unwrap_err();
        assert!(format!("{e}").contains("ProcessorArchitecture=\"mips\""), "{e}");
        let e = manifest("<Identity Name='Abc' Publisher='CN=x' Version='1.0.0.0'/>").unwrap_err();
        assert!(format!("{e}").contains("no Properties"), "{e}");
        let e = AppxManifest::parse(MANIFEST, b"<Package/>").unwrap_err();
        assert!(format!("{e}").contains("the root is not"), "{e}");
        let e = manifest("<Identity Name='Abc' Publisher='CN=x' Version='1'").unwrap_err();
        assert!(matches!(e, Error::Xml { .. }), "{e}");
    }

    #[test]
    fn windows8_manifest() {
        let doc = br##"<Package xmlns="http://schemas.microsoft.com/appx/2010/manifest">
  <Identity Name="Old.App" Publisher="CN=Old" Version="1.0.0.0"/>
  <Properties><DisplayName>Old</DisplayName><PublisherDisplayName>Old</PublisherDisplayName><Logo>l.png</Logo></Properties>
  <Applications><Application Id="A" StartPage="index.html"><VisualElements DisplayName="Old" Logo="l.png" SmallLogo="s.png" Description="d" BackgroundColor="#000000" ForegroundText="light"/></Application></Applications>
  <Capabilities><Capability Name="internetClient"/><DeviceCapability Name="webcam"/></Capabilities>
</Package>"##;
        let m = AppxManifest::parse(MANIFEST, doc).unwrap();
        assert_eq!(m.identity.architecture, "neutral");
        let v = m.applications[0].visual_elements.as_ref().unwrap();
        assert_eq!(
            (v.square150x150_logo.as_str(), v.square44x44_logo.as_str()),
            ("l.png", "s.png")
        );
        assert_eq!(m.capabilities.len(), 2);
        assert!(m.capabilities[1].device);
        assert!(!m.run_full_trust());
    }

    #[test]
    fn content_types_lookup() {
        let doc = br#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="PNG" ContentType="image/png"/>
<Override PartName="/AppxManifest.xml" ContentType="application/vnd.ms-appx.manifest+xml"/>
</Types>"#;
        let ct = ContentTypes::parse(CONTENT_TYPES, doc).unwrap();
        assert_eq!(ct.lookup("appxmanifest.XML"), Some("application/vnd.ms-appx.manifest+xml"));
        assert_eq!(ct.lookup("Assets/My%20Logo.png"), Some("image/png"));
        assert_eq!(ct.lookup("x.xml"), None);
        assert_eq!(ct.lookup("noext"), None);
    }
}
