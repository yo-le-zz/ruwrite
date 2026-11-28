//! Hot patching: applying edits to the memory of a running process.
//!
//! Implemented on Linux through `/proc/<pid>/mem`, which lets a process with
//! ptrace rights write to pages that are mapped read-only (code, `.rodata`).
//!
//! The process may run at a different load address than the file we parsed
//! (ASLR / PIE): the function's or static's address is computed once from the
//! file, then shifted by the process's **load bias** before anything is read
//! or written. See [`load_bias`] for how the bias itself is found: picking
//! the wrong memory mapping there — easy to do if you only look at which
//! mapping's file-offset range contains the byte, since several mappings of
//! the same file can appear to "contain" the same offset once ELF segments
//! are rounded to page boundaries — is what silently patches the wrong
//! address instead of failing loudly.

use crate::error::{Error, Result};
use crate::patch::Patch;

/// Applies `patches` to the process `pid`, which must be running the
/// executable whose bytes are `image` (the same bytes the patches were
/// prepared against).
///
/// Returns the number of patches actually written (patches whose target
/// already holds the new bytes are skipped, so the call is idempotent).
#[cfg(target_os = "linux")]
pub(crate) fn apply(image: &[u8], pid: u32, patches: &[Patch]) -> Result<usize> {
    use crate::error::hex;
    use std::fs::{self, OpenOptions};
    use std::os::unix::fs::{FileExt, MetadataExt};

    let file = object::File::parse(image)
        .map_err(|e| Error::Live(format!("cannot re-parse the image: {e}")))?;
    let segments = load_segments(&file);
    if segments.is_empty() {
        return Err(Error::Live(
            "the executable has no loadable segment (not a normal ELF executable?)".into(),
        ));
    }

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
    if mappings.is_empty() {
        return Err(Error::Live(format!(
            "process {pid} does not appear to be running this executable (no matching mapping)"
        )));
    }

    let bias = load_bias(&segments, &mappings)?;

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
        let vaddr = match patch.address {
            Some(addr) => addr,
            None => vaddr_for_file_offset(&segments, patch.file_offset).ok_or_else(|| {
                Error::Live(format!(
                    "file offset {:#x} has no known virtual address (hot patching needs an \
                     edit made through an address-based API, such as `patch_bytes`)",
                    patch.file_offset
                ))
            })?,
        };
        let address = (vaddr as i64 + bias) as u64;

        let mut current = vec![0u8; patch.new.len()];
        mem.read_exact_at(&mut current, address).map_err(|e| {
            Error::Live(format!(
                "cannot read {} bytes at {address:#x}: {e}",
                patch.new.len()
            ))
        })?;
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
pub(crate) fn apply(_image: &[u8], _pid: u32, _patches: &[Patch]) -> Result<usize> {
    Err(Error::unsupported("hot patching", std::env::consts::OS))
}

/// A `PT_LOAD` segment: where it lives in the file and where it is linked to run.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy)]
struct Segment {
    file_offset: u64,
    file_size: u64,
    vaddr: u64,
}

#[cfg(target_os = "linux")]
fn load_segments(file: &object::File<'_>) -> Vec<Segment> {
    use object::{Object, ObjectSegment};
    let mut segments: Vec<Segment> = file
        .segments()
        .filter_map(|s| {
            let (file_offset, file_size) = s.file_range();
            (file_size > 0).then_some(Segment {
                file_offset,
                file_size,
                vaddr: s.address(),
            })
        })
        .collect();
    segments.sort_by_key(|s| s.file_offset);
    segments
}

/// Virtual address (as linked, before any load bias) of a byte in the file.
#[cfg(target_os = "linux")]
fn vaddr_for_file_offset(segments: &[Segment], file_offset: u64) -> Option<u64> {
    segments
        .iter()
        .find(|s| file_offset >= s.file_offset && file_offset < s.file_offset + s.file_size)
        .map(|s| s.vaddr + (file_offset - s.file_offset))
}

/// Difference between where the process actually runs and where the file
/// says it should run (0 for a non-PIE executable, a random value for a PIE
/// one, because of ASLR).
///
/// Found from a single **anchor segment**: the one with the lowest file
/// offset (normally the segment covering the ELF header itself, at offset 0).
/// Its mapping is identified by an *exact* match on `/proc/<pid>/maps`'
/// `offset` field — not by "does this mapping's range contain the byte",
/// which is what lets several mappings of the same file (text, rodata, data,
/// each its own page-aligned slice) all appear to cover the same offset once
/// segments are rounded to page boundaries, and silently pick the wrong one.
/// An offset of 0 can only be the start of a mapping, so this one mapping is
/// unambiguous; every other patch is then placed relative to it, so the
/// ambiguity never has a chance to come up again.
#[cfg(target_os = "linux")]
fn load_bias(segments: &[Segment], mappings: &[Mapping]) -> Result<i64> {
    let anchor = segments[0];
    let page_offset = anchor.file_offset - anchor.file_offset % PAGE_SIZE_HINT;
    // L'ancre est le segment de plus petit décalage fichier ; son mapping est
    // celui dont le champ `offset` de /proc/pid/maps vaut exactement l'arrondi
    // de ce décalage à la page (en pratique 0, le segment porte l'en-tête ELF).
    let candidates: Vec<&Mapping> = mappings
        .iter()
        .filter(|m| m.offset == page_offset)
        .collect();
    let mapping = match candidates.as_slice() {
        [one] => *one,
        [] => {
            return Err(Error::Live(format!(
                "no memory mapping starts at file offset {page_offset:#x}: the running process \
                 does not match the executable we parsed"
            )))
        }
        _ => {
            return Err(Error::Live(format!(
                "{} memory mappings start at file offset {page_offset:#x}: cannot tell which one \
                 the executable was loaded at",
                candidates.len()
            )))
        }
    };
    // bias = adresse d'exécution réelle - adresse prévue au lien, pour ce même octet.
    let linked = anchor.vaddr + (page_offset - anchor.file_offset) as i64 as u64;
    Ok(mapping.start as i64 - linked as i64)
}

#[cfg(target_os = "linux")]
const PAGE_SIZE_HINT: u64 = 4096;

/// Une ligne de `/proc/<pid>/maps` : `start-end perms offset dev inode path`.
#[cfg(target_os = "linux")]
struct Mapping {
    start: u64,
    offset: u64,
    inode: u64,
}

#[cfg(target_os = "linux")]
impl Mapping {
    fn parse(line: &str) -> Option<Mapping> {
        let mut fields = line.split_whitespace();
        let (start, _end) = fields.next()?.split_once('-')?;
        let _perms = fields.next()?;
        let offset = fields.next()?;
        let _dev = fields.next()?;
        let inode = fields.next()?;
        Some(Mapping {
            start: u64::from_str_radix(start, 16).ok()?,
            offset: u64::from_str_radix(offset, 16).ok()?,
            inode: inode.parse().ok()?,
        })
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
    }

    #[test]
    fn anonymous_mappings_have_no_inode_match() {
        let line = "7ffc1c3f5000-7ffc1c416000 rw-p 00000000 00:00 0   [stack]";
        assert_eq!(Mapping::parse(line).unwrap().inode, 0);
    }

    fn seg(file_offset: u64, file_size: u64, vaddr: u64) -> Segment {
        Segment {
            file_offset,
            file_size,
            vaddr,
        }
    }

    fn mapping(start: u64, offset: u64) -> Mapping {
        Mapping {
            start,
            offset,
            inode: 1,
        }
    }

    #[test]
    fn bias_is_zero_for_a_fixed_load_address() {
        // Exécutable non-PIE : chargé exactement à son adresse de lien.
        let segments = [seg(0, 0x2000, 0x40_0000), seg(0x2000, 0x1000, 0x40_3000)];
        let mappings = [mapping(0x40_0000, 0), mapping(0x40_3000, 0x2000)];
        assert_eq!(load_bias(&segments, &mappings).unwrap(), 0);
    }

    #[test]
    fn bias_is_found_for_a_relocated_pie_binary() {
        let segments = [seg(0, 0x2000, 0), seg(0x2000, 0x1000, 0x3000)];
        // ASLR : chargé à une adresse aléatoire au lieu de 0.
        let mappings = [
            mapping(0x7f00_0000_0000, 0),
            mapping(0x7f00_0000_3000, 0x2000),
        ];
        assert_eq!(load_bias(&segments, &mappings).unwrap(), 0x7f00_0000_0000);
    }

    #[test]
    fn ambiguous_or_missing_anchor_is_reported_instead_of_guessed() {
        let segments = [seg(0, 0x1000, 0)];
        assert!(load_bias(&segments, &[]).is_err());
        let mappings = [mapping(0x1000, 0), mapping(0x5000, 0)];
        assert!(load_bias(&segments, &mappings).is_err());
    }

    #[test]
    fn file_offset_resolves_through_the_right_segment() {
        let segments = [seg(0, 0x2000, 0x40_0000), seg(0x2000, 0x1000, 0x50_0000)];
        assert_eq!(vaddr_for_file_offset(&segments, 0x10), Some(0x40_0010));
        assert_eq!(vaddr_for_file_offset(&segments, 0x2010), Some(0x50_0010));
        assert_eq!(vaddr_for_file_offset(&segments, 0x3000), None);
    }
}
