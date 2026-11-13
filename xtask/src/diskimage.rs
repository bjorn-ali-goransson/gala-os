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
#[path = "../../kernel/src/fs/pagetable.rs"]
#[allow(dead_code)]
pub mod pagetable;
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

/// A disk image read by offset: a raw file, or (`disk-files` on a
/// resumed run's kept layer, `target/snapshots/<name>/runs/*.qcow2`) a
/// qcow2 file over its raw backing file, read-only: uncompressed
/// clusters from the layer, the rest from the backing file (machine
/// snapshots' layers are qcow2 over the snapshot's raw `disk.img`).
enum Source {
    Raw(fs::File),
    Qcow2 { file: fs::File, l1: Vec<u64>, cluster_bits: u32, size: u64, backing: Option<fs::File> },
}

impl Source {
    fn open(path: &str) -> std::result::Result<Source, String> {
        let file = fs::File::open(path).map_err(|e| format!("{path}: {e}"))?;
        let mut head = [0u8; 72];
        if file.read_exact_at(&mut head, 0).is_err() || &head[..4] != b"QFI\xfb" {
            return Ok(Source::Raw(file));
        }
        let be32 = |o: usize| u32::from_be_bytes(head[o..o + 4].try_into().unwrap());
        let be64 = |o: usize| u64::from_be_bytes(head[o..o + 8].try_into().unwrap());
        let (backing_at, backing_len) = (be64(8), be32(16) as usize);
        let (cluster_bits, size, crypt) = (be32(20), be64(24), be32(32));
        let (l1_len, l1_at) = (be32(36) as usize, be64(40));
        if crypt != 0 {
            return Err(format!("{path}: an encrypted qcow2 image"));
        }
        let mut raw = vec![0u8; l1_len * 8];
        file.read_exact_at(&mut raw, l1_at).map_err(|e| format!("{path}: {e}"))?;
        let l1 = raw.chunks(8).map(|c| u64::from_be_bytes(c.try_into().unwrap())).collect();
        let backing = if backing_at == 0 {
            None
        } else {
            let mut name = vec![0u8; backing_len];
            file.read_exact_at(&mut name, backing_at).map_err(|e| format!("{path}: {e}"))?;
            let name = String::from_utf8_lossy(&name).into_owned();
            let full = Path::new(path).parent().unwrap_or(Path::new(".")).join(&name);
            Some(fs::File::open(&full).map_err(|e| format!("{}: {e}", full.display()))?)
        };
        Ok(Source::Qcow2 { file, l1, cluster_bits, size, backing })
    }

    fn len(&self) -> std::io::Result<u64> {
        match self {
            Source::Raw(f) => Ok(f.metadata()?.len()),
            Source::Qcow2 { size, .. } => Ok(*size),
        }
    }

    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> std::io::Result<()> {
        let Source::Qcow2 { file, l1, cluster_bits, backing, .. } = self else {
            let Source::Raw(f) = self else { unreachable!() };
            return f.read_exact_at(buf, offset);
        };
        let cluster = 1u64 << cluster_bits;
        let per_l2 = cluster / 8;
        let mut done = 0;
        while done < buf.len() {
            let at = offset + done as u64;
            let within = at & (cluster - 1);
            let n = ((cluster - within) as usize).min(buf.len() - done);
            let part = &mut buf[done..done + n];
            let index = at >> cluster_bits;
            let l2_at =
                l1.get((index / per_l2) as usize).copied().unwrap_or(0) & 0x00ff_ffff_ffff_fe00;
            let mut entry = 0u64;
            if l2_at != 0 {
                let mut e = [0u8; 8];
                file.read_exact_at(&mut e, l2_at + (index % per_l2) * 8)?;
                entry = u64::from_be_bytes(e);
            }
            if entry & (1 << 62) != 0 {
                return Err(std::io::Error::other("a compressed qcow2 cluster"));
            }
            let data = entry & 0x00ff_ffff_ffff_fe00;
            if entry & 1 != 0 {
                part.fill(0);
            } else if data != 0 {
                file.read_exact_at(part, data + within)?;
            } else if let Some(b) = backing {
                b.read_exact_at(part, at)?;
            } else {
                part.fill(0);
            }
            done += n;
        }
        Ok(())
    }
}

/// The image as a disk: read-only for `disk-files` (mounting a valid
/// volume writes nothing; one that is not a volume would be formatted:
/// refused there), writable for `inject` (always raw).
struct Image {
    file: Source,
    sectors: u64,
    writable: bool,
}

impl Blocks for Image {
    fn sectors(&self) -> u64 {
        self.sectors
    }
    fn read(&mut self, sector: u64, buf: &mut [u8]) -> std::result::Result<(), IoError> {
        self.file.read_exact_at(buf, sector * volume::SECTOR as u64).map_err(|_| IoError)
    }
    fn write(&mut self, sector: u64, buf: &[u8]) -> std::result::Result<(), IoError> {
        match &self.file {
            Source::Raw(f) if self.writable => {
                f.write_all_at(buf, sector * volume::SECTOR as u64).map_err(|_| IoError)
            }
            _ => Err(IoError),
        }
    }
    fn flush(&mut self) -> std::result::Result<(), IoError> {
        match &self.file {
            Source::Raw(f) if self.writable => f.sync_data().map_err(|_| IoError),
            _ => Ok(()),
        }
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
    let file = Source::open(image)?;
    let sectors = file.len().map_err(|e| e.to_string())? / volume::SECTOR as u64;
    let reader = Source::open(image)?;
    let (vol, mounted) = Volume::mount(Image { file, sectors, writable: false })
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
    image: &Source,
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

/// Puts the host directory `source` (its files and directories, all of
/// it) on a fresh disk image as C:`under` (`\\OfficeSource`): formats the
/// volume as the kernel would (`fs::volume`), writes each file's data to
/// consecutive clusters of the data area and appends the records a guest
/// would have written for them (Create, then Clusters naming the pages'
/// clusters), so the guest's mount replays them and reads the pages from
/// the disk as it needs them (ARCHITECTURE "A disk-backed C:"). Files are
/// streamed (a MiB at a time): the host needs no room beyond the image's
/// own. Returns the files and bytes written.
pub fn inject(image: &Path, source: &Path, under: &str) -> Result<(u64, u64)> {
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(image)
        .map_err(|e| format!("{}: {e}", image.display()))?;
    let sectors = file.metadata().map_err(|e| e.to_string())?.len() / volume::SECTOR as u64;
    let writer = file.try_clone().map_err(|e| e.to_string())?;
    let (mut vol, mounted) =
        Volume::mount(Image { file: Source::Raw(file), sectors, writable: true })
            .map_err(|e| format!("{}: {e:?}", image.display()))?;
    if !matches!(mounted, Mounted::Formatted) {
        return Err(format!("{}: not a fresh image", image.display()));
    }
    let (first, count) = vol.data_area().ok_or("the volume has no data area")?;
    // NT time (100 ns since 1601) of now.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() * 10_000_000 + 116_444_736_000_000_000);
    let wide = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
    let create = |path: &str, directory: bool| log::Op::Create {
        path: wide(path),
        directory,
        attributes: if directory { 0x10 } else { 0x20 },
        created: now,
        written: now,
        mode: None,
        case_sensitive: false,
    };
    let append = |vol: &mut Volume<Image>, ops: &[log::Op]| {
        vol.append(ops).map_err(|e| format!("{}: the change log: {e:?}", image.display()))
    };
    let mut next: u32 = 0;
    let (mut files, mut bytes) = (0u64, 0u64);
    let mut stack = vec![(source.to_path_buf(), under.to_string())];
    append(&mut vol, &[create(under, true)])?;
    let mut chunk = vec![0u8; 1 << 20];
    while let Some((dir, path)) = stack.pop() {
        let mut entries: Vec<_> = fs::read_dir(&dir)
            .map_err(|e| format!("{}: {e}", dir.display()))?
            .filter_map(|e| e.ok())
            .collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let name = entry.file_name().to_string_lossy().into_owned();
            let child = format!("{path}\\{name}");
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            if kind.is_dir() {
                append(&mut vol, &[create(&child, true)])?;
                stack.push((entry.path(), child));
                continue;
            }
            let mut src = fs::File::open(entry.path()).map_err(|e| e.to_string())?;
            let size = src.metadata().map_err(|e| e.to_string())?.len();
            let pages = size.div_ceil(ramfs::PAGE as u64);
            if u64::from(next) + pages > u64::from(count) {
                return Err(format!("{}: the data area is full", image.display()));
            }
            let start = next;
            let mut at = 0u64;
            while at < size {
                use std::io::Read;
                // As much as the chunk holds (reads may come short).
                let mut n = 0;
                while n < chunk.len() {
                    match src.read(&mut chunk[n..]).map_err(|e| e.to_string())? {
                        0 => break,
                        k => n += k,
                    }
                }
                if n == 0 {
                    return Err(format!("{}: shorter than its size", entry.path().display()));
                }
                // Whole pages: the last one's tail is zeros.
                let whole = n.next_multiple_of(ramfs::PAGE);
                chunk[n..whole].fill(0);
                let cluster = u64::from(start) + at / ramfs::PAGE as u64;
                let sector = first + cluster * volume::CLUSTER_SECTORS;
                writer
                    .write_all_at(&chunk[..whole], sector * volume::SECTOR as u64)
                    .map_err(|e| format!("{}: {e}", image.display()))?;
                at += n as u64;
            }
            next += pages as u32;
            let mut ops = vec![create(&child, false)];
            let mut runs = Vec::new();
            let mut page = 0u64;
            while page < pages {
                let n = (pages - page).min(u64::from(u32::MAX));
                runs.push(log::Run { page, cluster: start + page as u32, count: n as u32 });
                page += n;
            }
            if pages > 0 {
                ops.push(log::Op::Clusters { path: wide(&child), size, written: now, runs });
            }
            append(&mut vol, &ops)?;
            files += 1;
            bytes += size;
        }
    }
    writer.sync_data().map_err(|e| e.to_string())?;
    Ok((files, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What `inject` puts on a fresh image reads back as a guest's mount
    /// would replay it (the same code): the tree and every byte.
    #[test]
    fn injected_files_read_back() {
        let dir = std::env::temp_dir().join(format!("gala-inject-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let src = dir.join("src");
        fs::create_dir_all(src.join("Office").join("Data")).unwrap();
        let big: Vec<u8> = (0..(3 << 20) + 123).map(|i| (i * 7 % 251) as u8).collect();
        fs::write(src.join("Office").join("Data").join("stream.dat"), &big).unwrap();
        fs::write(src.join("small.txt"), b"hello").unwrap();
        fs::write(src.join("empty"), b"").unwrap();
        let image = dir.join("disk.img");
        fs::File::create(&image).unwrap().set_len(64 << 20).unwrap();
        let (files, bytes) = inject(&image, &src, "\\OfficeSource").unwrap();
        assert_eq!((files, bytes), (3, big.len() as u64 + 5));

        let file = fs::File::open(&image).unwrap();
        let sectors = file.metadata().unwrap().len() / volume::SECTOR as u64;
        let reader = Source::Raw(file.try_clone().unwrap());
        let (vol, mounted) =
            Volume::mount(Image { file: Source::Raw(file), sectors, writable: false }).unwrap();
        let Mounted::Log(ops) = mounted else { panic!("not formatted") };
        let mut fs = RamFs::new(0).unwrap();
        assert!(ops.iter().all(|op| fs.apply(op).is_ok()));
        let (first, count) = vol.data_area().unwrap();
        fs.attach_area(count).unwrap();
        let mut found = Vec::new();
        walk(&fs, ROOT, &mut String::new(), &mut found);
        found.sort_by(|a, b| a.1.cmp(&b.1));
        let names: Vec<&str> = found.iter().map(|(_, p)| p.as_str()).collect();
        assert_eq!(
            names,
            [
                "\\OfficeSource\\Office\\Data\\stream.dat",
                "\\OfficeSource\\empty",
                "\\OfficeSource\\small.txt"
            ]
        );
        let read = |fs: &mut RamFs, i: usize| read_file(fs, found[i].0, &reader, first).unwrap();
        assert_eq!(read(&mut fs, 0), big);
        assert_eq!(read(&mut fs, 1), b"");
        assert_eq!(read(&mut fs, 2), b"hello");
        let _ = fs::remove_dir_all(&dir);
    }

    /// A qcow2 layer reads its own clusters, its zero clusters as zeros
    /// and the rest from its raw backing file (a resumed run's kept
    /// layer over the snapshot's disk.img), across cluster boundaries.
    #[test]
    fn qcow2_layer_reads_over_its_backing_file() {
        let dir = std::env::temp_dir().join(format!("gala-qcow2-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("base.raw"), vec![0xaau8; 2048]).unwrap();
        // 512-byte clusters: the header, the L1 table, the L2 table, one
        // data cluster.
        let mut q = vec![0u8; 2048];
        let put32 =
            |q: &mut Vec<u8>, at: usize, v: u32| q[at..at + 4].copy_from_slice(&v.to_be_bytes());
        let put64 =
            |q: &mut Vec<u8>, at: usize, v: u64| q[at..at + 8].copy_from_slice(&v.to_be_bytes());
        q[..4].copy_from_slice(b"QFI\xfb");
        put32(&mut q, 4, 3);
        put64(&mut q, 8, 200);
        put32(&mut q, 16, 8);
        q[200..208].copy_from_slice(b"base.raw");
        put32(&mut q, 20, 9);
        put64(&mut q, 24, 2048);
        put32(&mut q, 36, 1);
        put64(&mut q, 40, 512);
        put64(&mut q, 512, 1024 | 1 << 63);
        put64(&mut q, 1024 + 8, 1536 | 1 << 63);
        put64(&mut q, 1024 + 16, 1);
        q[1536..].fill(0x55);
        let layer = dir.join("layer.qcow2");
        fs::write(&layer, &q).unwrap();
        let src = Source::open(layer.to_str().unwrap()).unwrap();
        assert_eq!(src.len().unwrap(), 2048);
        let mut all = vec![0u8; 2048];
        src.read_exact_at(&mut all, 0).unwrap();
        assert!(all[..512].iter().all(|&b| b == 0xaa));
        assert!(all[512..1024].iter().all(|&b| b == 0x55));
        assert!(all[1024..1536].iter().all(|&b| b == 0));
        assert!(all[1536..].iter().all(|&b| b == 0xaa));
        let mut part = vec![0u8; 1000];
        src.read_exact_at(&mut part, 300).unwrap();
        assert_eq!(part, all[300..1300]);
        let _ = fs::remove_dir_all(&dir);
    }
}
