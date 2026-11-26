//! Pending edits and their transactional application.

use std::fmt;

use crate::error::{hex, Error, Result};

/// One byte-level edit of the executable file.
///
/// A patch remembers the bytes it expects to replace, so applying it to an
/// image that does not match (for example a different build of the program)
/// fails cleanly instead of corrupting the binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Patch {
    /// Offset of the first modified byte in the file.
    pub file_offset: u64,
    /// Virtual address of the first modified byte, when known.
    pub address: Option<u64>,
    /// Bytes currently stored there.
    pub old: Vec<u8>,
    /// Bytes to store instead.
    pub new: Vec<u8>,
    /// Human-readable summary of what the edit does.
    pub description: String,
}

impl Patch {
    /// Number of bytes covered by the patch.
    pub fn len(&self) -> usize {
        self.new.len()
    }

    /// `true` when the patch covers no byte.
    pub fn is_empty(&self) -> bool {
        self.new.is_empty()
    }

    fn end(&self) -> u64 {
        self.file_offset + self.new.len() as u64
    }
}

impl fmt::Display for Patch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.address {
            Some(addr) => write!(f, "{addr:#x}")?,
            None => write!(f, "file+{:#x}", self.file_offset)?,
        }
        write!(
            f,
            ": {} -> {}  ({})",
            hex(&self.old),
            hex(&self.new),
            self.description
        )
    }
}

/// Ordered list of edits waiting for a `commit`.
#[derive(Debug, Default, Clone)]
pub(crate) struct PatchSet {
    patches: Vec<Patch>,
}

impl PatchSet {
    pub(crate) fn as_slice(&self) -> &[Patch] {
        &self.patches
    }

    pub(crate) fn clear(&mut self) {
        self.patches.clear();
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.patches.is_empty()
    }

    /// Adds an edit, refusing it if it overlaps one that is already queued.
    pub(crate) fn push(&mut self, patch: Patch) -> Result<()> {
        if patch.old.len() != patch.new.len() {
            return Err(Error::SizeMismatch {
                expected: patch.old.len(),
                got: patch.new.len(),
            });
        }
        for existing in &self.patches {
            if patch.file_offset < existing.end() && existing.file_offset < patch.end() {
                return Err(Error::Overlap {
                    offset: patch.file_offset.max(existing.file_offset),
                    existing: existing.description.clone(),
                });
            }
        }
        self.patches.push(patch);
        Ok(())
    }

    /// Applies every edit to `image`, or none of them.
    pub(crate) fn apply(&self, image: &mut [u8]) -> Result<usize> {
        // Première passe : tout vérifier avant de toucher à l'image.
        for patch in &self.patches {
            let range = checked_range(image.len(), patch.file_offset, patch.new.len())?;
            let found = &image[range];
            if found != patch.old.as_slice() {
                return Err(Error::BytesMismatch {
                    location: format!("file offset {:#x}", patch.file_offset),
                    expected: hex(&patch.old),
                    found: hex(found),
                });
            }
        }
        // Seconde passe : écriture, qui ne peut plus échouer.
        let mut written = 0;
        for patch in &self.patches {
            let range = checked_range(image.len(), patch.file_offset, patch.new.len())?;
            image[range].copy_from_slice(&patch.new);
            written += patch.new.len();
        }
        Ok(written)
    }
}

/// Validates `offset..offset+len` against an image of `image_len` bytes.
pub(crate) fn checked_range(
    image_len: usize,
    offset: u64,
    len: usize,
) -> Result<std::ops::Range<usize>> {
    let out_of_bounds = || Error::OutOfBounds {
        offset,
        length: len as u64,
        image_len: image_len as u64,
    };
    let start = usize::try_from(offset).map_err(|_| out_of_bounds())?;
    let end = start.checked_add(len).ok_or_else(out_of_bounds)?;
    if end > image_len {
        return Err(out_of_bounds());
    }
    Ok(start..end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patch(offset: u64, old: &[u8], new: &[u8]) -> Patch {
        Patch {
            file_offset: offset,
            address: None,
            old: old.to_vec(),
            new: new.to_vec(),
            description: "test".into(),
        }
    }

    #[test]
    fn applies_all_edits() {
        let mut set = PatchSet::default();
        set.push(patch(1, &[1, 2], &[9, 9])).unwrap();
        set.push(patch(4, &[4], &[7])).unwrap();
        let mut image = [0, 1, 2, 3, 4, 5];
        assert_eq!(set.apply(&mut image).unwrap(), 3);
        assert_eq!(image, [0, 9, 9, 3, 7, 5]);
    }

    #[test]
    fn overlapping_edits_are_refused() {
        let mut set = PatchSet::default();
        set.push(patch(2, &[0, 0], &[1, 1])).unwrap();
        assert!(matches!(
            set.push(patch(3, &[0, 0], &[1, 1])),
            Err(Error::Overlap { .. })
        ));
        // Des plages simplement adjacentes restent permises.
        set.push(patch(4, &[0], &[1])).unwrap();
    }

    #[test]
    fn nothing_is_written_when_one_edit_mismatches() {
        let mut set = PatchSet::default();
        set.push(patch(0, &[0], &[9])).unwrap();
        set.push(patch(2, &[77], &[9])).unwrap();
        let mut image = [0, 1, 2];
        assert!(matches!(
            set.apply(&mut image),
            Err(Error::BytesMismatch { .. })
        ));
        assert_eq!(image, [0, 1, 2]);
    }

    #[test]
    fn out_of_bounds_is_detected() {
        let mut set = PatchSet::default();
        set.push(patch(2, &[0, 0], &[1, 1])).unwrap();
        let mut image = [0, 0, 0];
        assert!(matches!(
            set.apply(&mut image),
            Err(Error::OutOfBounds { .. })
        ));
    }

    #[test]
    fn length_change_is_refused() {
        let mut set = PatchSet::default();
        assert!(matches!(
            set.push(patch(0, &[1], &[1, 2])),
            Err(Error::SizeMismatch { .. })
        ));
    }
}
