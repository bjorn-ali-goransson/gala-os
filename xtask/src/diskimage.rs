//! `cargo xtask disk-files <image> [C:/path] [--out DIR]`: what a gala disk
//! image (a scenario's `<iso>.disk.img`, kept after a failed run) holds on
//! C:, read with the kernel's own code: the volume (`fs::volume`), the
//! change log (`fs::log`) replayed into a `RamFs` as a boot would, and
//! files' pages read from the data area (`fs::ramfs`, `fs::clusters`;
//! ARCHITECTURE "A disk-backed C:"). It lists the files programs wrote
//! (not the boot's own, which the image does not hold) under the path
//! given, with their sizes, and with `--out` copies them there, so a run
//! that hung or was stopped still leaves its programs' logs to read.

#[path = "../../kernel/src/fs/clusters.rs"]
#[allow(dead_code)]
pub mod clusters;
#[path = "../../kernel/src/fs/log.rs"]
#[allow(dead_code)]
pub mod log;
#[path = "../../kernel/src/fs/ramfs.rs"]
#[allow(dead_code, clippy::all)]
pub mod ramfs;
#[path = "../../kernel/src/fs/volume.rs"]
#[allow(dead_code)]
pub mod volume;

use crate::Result;
use ramfs::{FsError, NodeId, Page, ROOT, RamFs};
use std::fs;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use volume::{Blocks, IoError, Mounted, Volume};

/// The image as a read-only disk (mounting a valid volume writes nothing;
/// one that is not a volume would be formatted: refused here).
struct Image {
    file: fs::File,
    sectors: u64,
}

impl Blocks for Image {
    fn sectors(&self) -> u64 {
        self.sectors
    }
    fn read(&mut self, sector: u64, buf: &mut [u8]) -> std::result::Result<(), IoError> {
        self.file.read_exact_at(buf, sector * volume::SECTOR as u64).map_err(|_| IoError)
    }
    fn write(&mut self, _: u64, _: &[u8]) -> std::result::Result<(), IoError> {
        Err(IoError)
    }
    fn flush(&mut self) -> std::result::Result<(), IoError> {
        Ok(())
    }
}

pub fn disk_files(args: &[String]) -> Result {
    let mut positional = Vec::new();
    let mut out: Option<PathBuf> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--out" {
            out = Some(it.next().ok_or("--out needs a directory")?.into());
        } else {
            positional.push(a.as_str());
        }
    }
    let image = positional.first().ok_or("usage: disk-files <image> [C:/path] [--out DIR]")?;
    let under = positional.get(1).map_or(String::from("\\"), |p| volume_path(p));
    let file = fs::File::open(image).map_err(|e| format!("{image}: {e}"))?;
    let sectors = file.metadata().map_err(|e| e.to_string())?.len() / volume::SECTOR as u64;
    let reader = file.try_clone().map_err(|e| e.to_string())?;
    let (vol, mounted) = Volume::mount(Image { file, sectors })
        .map_err(|e| format!("{image}: not a gala volume ({e:?})"))?;
    let Mounted::Log(ops) = mounted else { return Err(format!("{image}: not a gala volume")) };
    let mut fs = RamFs::new(0).map_err(|e| format!("{e:?}"))?;
    let skipped = ops.iter().filter(|op| fs.apply(op).is_err()).count();
    let (area, generation) = vol.position();
    println!(
        "{image}: log area {area}, generation {generation}, {} records ({skipped} skipped), {} KiB",
        ops.len(),
        vol.used() / 1024
    );
    let data = vol.data_area();
    if let Some((_, count)) = data {
        fs.attach_area(count).map_err(|e| format!("{e:?}"))?;
        if let Some((count, free)) = fs.area_size() {
            println!("data area: {} MiB, {} MiB free", count / 256, free / 256);
        }
    }
    let mut files = Vec::new();
    walk(&fs, ROOT, &mut String::new(), &mut files);
    let under = under.to_ascii_lowercase();
    for (id, path) in files.iter().filter(|(_, p)| p.to_ascii_lowercase().starts_with(&under)) {
        let size = fs.node(*id).map_or(0, |n| n.size());
        println!("{size:>12}  C:{path}");
        if let Some(dir) = &out {
            let bytes = read_file(&mut fs, *id, &reader, data.map_or(0, |(first, _)| first))
                .map_err(|e| format!("C:{path}: {e:?}"))?;
            let target = dir.join(path.trim_start_matches('\\').replace('\\', "/"));
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            fs::write(&target, bytes).map_err(|e| format!("{}: {e}", target.display()))?;
        }
    }
    if let Some(dir) = &out {
        println!("copied to {}", Path::new(dir).display());
    }
    Ok(())
}

/// `C:/a/b` or `\a\b` as the volume's `\a\b`.
fn volume_path(p: &str) -> String {
    let rest = p.strip_prefix("C:").or_else(|| p.strip_prefix("c:")).unwrap_or(p);
    let mut s = rest.replace('/', "\\");
    if !s.starts_with('\\') {
        s.insert(0, '\\');
    }
    s
}

/// Every file below `dir`, with its path.
fn walk(fs: &RamFs, dir: NodeId, path: &mut String, out: &mut Vec<(NodeId, String)>) {
    let Ok(entries) = fs.entries(dir) else { return };
    for (id, name) in entries {
        let len = path.len();
        path.push('\\');
        path.push_str(&String::from_utf16_lossy(name));
        match fs.node(id) {
            Some(n) if n.is_dir() => walk(fs, id, path, out),
            Some(n) if !n.is_symlink() && !n.is_socket() => out.push((id, path.clone())),
            _ => {}
        }
        path.truncate(len);
    }
}

/// File `id`'s contents, its pages on the disk only read from the image.
fn read_file(
    fs: &mut RamFs,
    id: NodeId,
    image: &fs::File,
    first: u64,
) -> std::result::Result<Vec<u8>, FsError> {
    let size = fs.node(id).map_or(0, |n| n.size()) as usize;
    let mut out = vec![0u8; size];
    let mut at = 0;
    while at < size {
        match fs.read(id, at as u64, &mut out[at..]) {
            Ok(0) => break,
            Ok(n) => at += n,
            Err(FsError::NotResident) => {
                let req = fs.take_page_in().ok_or(FsError::NotResident)?;
                let mut page = Box::new(Page([0; ramfs::PAGE]));
                let sector = first + u64::from(req.cluster) * volume::CLUSTER_SECTORS;
                image
                    .read_exact_at(&mut page.0, sector * volume::SECTOR as u64)
                    .map_err(|_| FsError::NotResident)?;
                fs.fill(&req, page);
            }
            Err(e) => return Err(e),
        }
    }
    Ok(out)
}
