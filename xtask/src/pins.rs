//! The pinned downloads, each checked against its checksum at use: kept
//! apart from the rest of xtask so that CI's caches of them can be keyed
//! on these files alone (`ci.yml`, WORKFLOW "CI build caches").

pub mod debs;
pub mod prebuilt;
