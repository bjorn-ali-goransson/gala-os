//! What gala's installer writes (ROADMAP IN step 4): the EFI system
//! partition as a FAT32 volume holding a release's files (`fat32`), and
//! those files read from a tar (`tar`). Pure: the caller writes the
//! sectors (`gala setup` through `\\.\PhysicalDrive0`). `no_std` with
//! `alloc`.

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod fat32;
pub mod tar;
