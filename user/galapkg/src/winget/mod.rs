//! winget manifests in full, for `gala install` to call (ROADMAP PK;
//! ARCHITECTURE "Installing software: the winget resolver"):
//!
//! - [`yaml`]: the YAML subset manifests are written in, with line and
//!   column on every node and error;
//! - [`schema`]: ManifestType, ManifestVersion 1.0.0 to 1.28.0, and in
//!   which schema each field and value appeared;
//! - [`package`]: singleton and multi-file manifests into one
//!   [`Package`] (every installer field resolved, root defaults applied,
//!   locales with fallback), errors and warnings with file, line, column;
//! - [`select`]: installer selection as winget does it for a machine
//!   ([`Machine`]) and the user's options and settings ([`Request`]),
//!   with every rejected installer's reason, and the installer's command
//!   line from InstallerSwitches and the per-type defaults;
//! - [`deps`]: Dependencies into an ordered install plan, cycles
//!   detected;
//! - [`arp`]: AppsAndFeaturesEntries matched against installed
//!   Add/Remove Programs records (what is installed, at what version);
//! - [`version`]: winget's version ordering.
//!
//! Separate from `crate::manifest`/`crate::yaml`, which the install path
//! uses today; the install command switches over as ARCHITECTURE
//! describes. `no_std` with `alloc`, host-tested (`cargo test -p
//! galapkg`), including a corpus of real winget-pkgs manifests
//! (`testdata/winget-corpus`, `corpus_tests`).

pub mod arp;
pub mod deps;
pub mod package;
pub mod schema;
pub mod select;
pub mod version;
pub mod yaml;

#[cfg(test)]
mod corpus_tests;

pub use package::{Architecture, Installer, InstallerType, Package, Scope};
pub use select::{Machine, Request, Selection};
