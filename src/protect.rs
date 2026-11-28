//! Protection declared by the target program with `#[ruwrite::protect(..)]`
//! or `ruwrite::protect_all!()`.
//!
//! The program embeds small records in a dedicated section; ruwrite reads
//! them when it opens the file and turns them into byte ranges that no edit
//! may touch.

use std::fmt;

use object::{Object, ObjectSection};

const MAGIC: &[u8; 8] = b"RUWPROT2";
const SECTION_NAMES: [&str; 2] = [".ruwprot", "__ruwprot"];

/// How strongly a declared part is protected, from least to most restrictive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtectionMode {
    /// Explicitly not protected (the same as declaring nothing, spelled out).
    Editable,
    /// Every edit touching it is refused.
    Immutable,
    /// `Immutable`, and included in [`Executable::integrity_snapshot`](crate::Executable::integrity_snapshot).
    Protected,
    /// `Immutable`, its name is not stored as readable text in the binary,
    /// and it is left out of [`Executable::functions`](crate::Executable::functions)
    /// and [`Executable::variables`](crate::Executable::variables).
    Hidden,
}

impl fmt::Display for ProtectionMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ProtectionMode::Editable => "editable",
            ProtectionMode::Immutable => "immutable",
            ProtectionMode::Protected => "protected",
            ProtectionMode::Hidden => "hidden",
        })
    }
}

/// What a protection rule covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtectionTarget {
    /// A function, or a group of functions (`name`, `module::name`, `module::*`).
    Function(String),
    /// A `static`.
    Static(String),
    /// A function or static whose plain name was not embedded; only its
    /// hash was, so it can be matched without being readable in the binary.
    HiddenName(u64),
    /// The whole executable.
    Everything,
}

impl fmt::Display for ProtectionTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProtectionTarget::Function(name) => write!(f, "function `{name}`"),
            ProtectionTarget::Static(name) => write!(f, "static `{name}`"),
            ProtectionTarget::HiddenName(_) => f.write_str("a hidden item"),
            ProtectionTarget::Everything => f.write_str("the whole executable"),
        }
    }
}

/// A protection rule declared by the program, and what it matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Protection {
    /// What the rule covers.
    pub target: ProtectionTarget,
    /// How strongly it is protected.
    pub mode: ProtectionMode,
    /// Number of functions or statics found for this rule. `0` means the rule
    /// protects nothing in this build (the item was removed or inlined).
    pub matches: usize,
}

/// Une plage d'octets du fichier qu'aucune modification ne peut toucher.
#[derive(Debug, Clone)]
pub(crate) struct Guard {
    pub start: u64,
    pub end: u64,
    pub what: String,
    pub rule: String,
    pub mode: ProtectionMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RawTarget {
    Function(String),
    Static(String),
    HiddenName(u64),
    Everything,
}

#[derive(Debug, Clone)]
pub(crate) struct Rule {
    pub target: RawTarget,
    pub mode: ProtectionMode,
}

/// Lit les règles déclarées dans l'exécutable, plus la plage de la section
/// qui les contient.
pub(crate) fn read_rules(file: &object::File<'_>) -> (Vec<Rule>, Option<(u64, u64)>) {
    let mut rules = Vec::new();
    let mut range = None;
    for section in file.sections() {
        let Ok(name) = section.name() else { continue };
        if !SECTION_NAMES.contains(&name) {
            continue;
        }
        if let Some((offset, size)) = section.file_range() {
            range = Some((offset, offset + size));
        }
        if let Ok(data) = section.data() {
            parse_records(data, &mut rules);
        }
    }
    (rules, range)
}

fn parse_records(data: &[u8], rules: &mut Vec<Rule>) {
    let mut pos = 0;
    while pos + 11 <= data.len() {
        if &data[pos..pos + 8] != MAGIC {
            pos += 1; // octets de remplissage entre deux enregistrements
            continue;
        }
        let kind = data[pos + 8];
        let mode_byte = data[pos + 9];
        let name_encoding = data[pos + 10];
        let len = usize::from(u16::from_le_bytes([data[pos + 11], data[pos + 12]]));
        let Some(raw) = data.get(pos + 13..pos + 13 + len) else {
            break;
        };

        let mode = match mode_byte {
            0 => ProtectionMode::Editable,
            1 => ProtectionMode::Immutable,
            2 => ProtectionMode::Protected,
            // Mode inconnu (version plus récente du programme) : le plus
            // restrictif est la seule lecture sûre.
            _ => ProtectionMode::Hidden,
        };

        let target = if name_encoding == 1 && raw.len() == 8 {
            RawTarget::HiddenName(u64::from_le_bytes(raw.try_into().unwrap()))
        } else {
            let name = String::from_utf8_lossy(raw).into_owned();
            match kind {
                1 => RawTarget::Function(name),
                2 => RawTarget::Static(name),
                3 => RawTarget::Everything,
                _ => RawTarget::Everything,
            }
        };
        rules.push(Rule { target, mode });
        pos += 13 + len;
    }
}

/// Un nom qualifié correspond-il au motif ?
///
/// Mêmes règles que la recherche de fonctions : chemin complet, derniers
/// segments, ou préfixe terminé par `*`.
pub(crate) fn name_matches(name: &str, pattern: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix('*') {
        return name.starts_with(prefix) || name.contains(&format!("::{prefix}"));
    }
    name == pattern || name.ends_with(&format!("::{pattern}"))
}

/// Hash utilisé pour les noms cachés (mode `hide`) : identique à celui de la
/// macro, non cryptographique, juste stable.
pub(crate) fn fnv1a64(s: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Same hash as [`fnv1a64`], over raw bytes instead of a name (used for the
/// integrity snapshot).
pub(crate) fn fnv1a64_bytes(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in data {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Dernier segment d'un nom qualifié (`a::b::c` -> `c`), ce que la macro
/// connaît seule au moment de hacher un nom à cacher.
pub(crate) fn last_segment(name: &str) -> &str {
    name.rsplit("::").next().unwrap_or(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(kind: u8, mode: u8, name_encoding: u8, payload: &[u8]) -> Vec<u8> {
        let mut r = Vec::new();
        r.extend(MAGIC);
        r.push(kind);
        r.push(mode);
        r.push(name_encoding);
        r.extend((payload.len() as u16).to_le_bytes());
        r.extend(payload);
        r
    }

    #[test]
    fn parses_concatenated_records_with_padding() {
        let mut data = record(1, 1, 0, b"check");
        data.extend([0, 0, 0]); // remplissage
        data.extend(record(2, 2, 0, b"KEY"));
        let mut rules = Vec::new();
        parse_records(&data, &mut rules);
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].target, RawTarget::Function("check".into()));
        assert_eq!(rules[0].mode, ProtectionMode::Immutable);
        assert_eq!(rules[1].target, RawTarget::Static("KEY".into()));
        assert_eq!(rules[1].mode, ProtectionMode::Protected);
    }

    #[test]
    fn hidden_names_are_read_as_a_hash() {
        let hash = fnv1a64("secret");
        let data = record(1, 3, 1, &hash.to_le_bytes());
        let mut rules = Vec::new();
        parse_records(&data, &mut rules);
        assert_eq!(rules[0].target, RawTarget::HiddenName(hash));
        assert_eq!(rules[0].mode, ProtectionMode::Hidden);
    }

    #[test]
    fn truncated_records_are_safe() {
        let mut rules = Vec::new();
        parse_records(b"RUWPROT2\x01\x01\x00\xff\xffab", &mut rules);
        assert!(rules.is_empty());
    }

    #[test]
    fn name_matching_rules() {
        assert!(name_matches("app::sec::check", "check"));
        assert!(name_matches("app::sec::check", "sec::check"));
        assert!(!name_matches("app::sec::recheck", "check"));
        assert!(name_matches("app::sec::check", "sec::*"));
        assert!(!name_matches("app::other::check", "sec::*"));
    }

    #[test]
    fn last_segment_strips_the_module_path() {
        assert_eq!(last_segment("app::sec::check"), "check");
        assert_eq!(last_segment("check"), "check");
    }
}
