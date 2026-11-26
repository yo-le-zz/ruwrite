//! Hot patching: applying edits to the memory of a running process.
//!
//! Implemented on Linux through `/proc/<pid>/mem`, which lets a process with
//! ptrace rights write to pages that are mapped read-only (code, `.rodata`).

use crate::error::{Error, Result};
use crate::patch::Patch;

/// Applies `patches` to the process `pid`, which must be running the
/// executable the patches were prepared for.
///
/// Returns the number of patches actually written (patches whose target
/// already holds the new bytes are skipped, so the call is idempotent).
#[cfg(target_os = "linux")]
pub(crate) fn apply(pid: u32, patches: &[Patch]) -> Result<usize> {
    use crate::error::hex;
    use std::fs::{self, OpenOptions};
    use std::os::unix::fs::{FileExt, MetadataExt};

    // Le binaire réellement exécuté, même s'il a été remplacé sur disque depuis.
    let exe_inode = fs::metadata(format!("/proc/{pid}/exe"))
        .map_err(|e| Error::Live(format!("cannot inspect process {pid}: {e}")))?
        .ino();
    let maps = fs::read_to_string(format!("/proc/{pid}/maps"))
        .map_err(|e| Error::Live(format!("cannot read the memory map of process {pid}: {e}")))?;
    let mappings: Vec<Mapping> = maps
        .lines()
        .filter_map(Mapping::parse)
        .filter(|m| m.inode == exe_inode)
        .collect();

    let mem = OpenOptions::new()
        .read(true)
        .write(true)
        .open(format!("/proc/{pid}/mem"))
        .map_err(|e| {
            Error::Live(format!(
                "cannot open /proc/{pid}/mem for writing: {e} \
                 (the target must belong to you and ptrace must be allowed)"
            ))
        })?;

    // Résolution et vérification de tout avant la moindre écriture.
    let mut writes = Vec::new();
    for patch in patches {
        let len = patch.new.len() as u64;
        let address = mappings
            .iter()
            .find(|m| m.contains(patch.file_offset, len))
            .map(|m| m.start + (patch.file_offset - m.offset))
            .ok_or_else(|| {
                Error::Live(format!(
                    "file offset {:#x} is not mapped in process {pid}",
                    patch.file_offset
                ))
            })?;

        let mut current = vec![0u8; patch.new.len()];
        mem.read_exact_at(&mut current, address)
            .map_err(|e| Error::Live(format!("cannot read {len} bytes at {address:#x}: {e}")))?;
        if current == patch.new {
            continue;
        }
        if current != patch.old {
            return Err(Error::BytesMismatch {
                location: format!("process {pid} at {address:#x}"),
                expected: hex(&patch.old),
                found: hex(&current),
            });
        }
        writes.push((address, patch));
    }

    for (address, patch) in &writes {
        mem.write_all_at(&patch.new, *address)
            .map_err(|e| Error::Live(format!("cannot write at {address:#x}: {e}")))?;
    }
    Ok(writes.len())
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn apply(_pid: u32, _patches: &[Patch]) -> Result<usize> {
    Err(Error::unsupported("hot patching", std::env::consts::OS))
}

/// Une ligne de `/proc/<pid>/maps` : `start-end perms offset dev inode path`.
#[cfg(target_os = "linux")]
struct Mapping {
    start: u64,
    end: u64,
    offset: u64,
    inode: u64,
}

#[cfg(target_os = "linux")]
impl Mapping {
    fn parse(line: &str) -> Option<Mapping> {
        let mut fields = line.split_whitespace();
        let (start, end) = fields.next()?.split_once('-')?;
        let _perms = fields.next()?;
        let offset = fields.next()?;
        let _dev = fields.next()?;
        let inode = fields.next()?;
        Some(Mapping {
            start: u64::from_str_radix(start, 16).ok()?,
            end: u64::from_str_radix(end, 16).ok()?,
            offset: u64::from_str_radix(offset, 16).ok()?,
            inode: inode.parse().ok()?,
        })
    }

    /// `true` si la plage `[file_offset, file_offset + len)` du fichier est
    /// entièrement couverte par ce mapping.
    fn contains(&self, file_offset: u64, len: u64) -> bool {
        file_offset >= self.offset && file_offset + len <= self.offset + (self.end - self.start)
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn parses_a_maps_line() {
        let line = "5581b0c2a000-5581b0c3b000 r-xp 00001000 fd:01 1234567  /usr/bin/demo";
        let m = Mapping::parse(line).unwrap();
        assert_eq!(m.start, 0x5581_b0c2_a000);
        assert_eq!(m.offset, 0x1000);
        assert_eq!(m.inode, 1_234_567);
        assert!(m.contains(0x1000, 0x10));
        assert!(m.contains(0x1000 + 0x10fff, 1));
        assert!(!m.contains(0x1000 + 0x11000, 1));
        assert!(!m.contains(0xfff, 2));
    }

    #[test]
    fn anonymous_mappings_have_no_inode_match() {
        let line = "7ffc1c3f5000-7ffc1c416000 rw-p 00000000 00:00 0   [stack]";
        assert_eq!(Mapping::parse(line).unwrap().inode, 0);
    }

    #[test]
    fn patching_a_missing_process_fails_cleanly() {
        let err = apply(u32::MAX - 1, &[]).unwrap_err();
        assert!(matches!(err, Error::Live(_)));
    }
}
