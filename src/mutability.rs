//! Détection de `let` / `let mut` dans le code source d'une variable.
//!
//! DWARF ne dit pas si une variable est `mut`. Quand le fichier source est
//! accessible, la ligne de déclaration le dit ; sinon le statut est inconnu et
//! traité comme « non mutable » par prudence.

use std::fs;
use std::path::{Path, PathBuf};

/// Whether a variable is declared `mut` in the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mutability {
    /// Declared with `let mut`.
    Mutable,
    /// Declared with a plain `let`.
    Immutable,
    /// Source not found or declaration not recognised.
    Unknown,
}

/// Analyse la ligne `line` (1-based) de `source` pour la variable `name`.
pub(crate) fn classify(source: &str, line: u32, name: &str) -> Mutability {
    let Some(text) = source.lines().nth(line.saturating_sub(1) as usize) else {
        return Mutability::Unknown;
    };
    classify_line(text, name)
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Cherche `name` comme identifiant complet dans `text`, à partir de `from`.
fn find_ident(text: &str, name: &str, from: usize) -> Option<usize> {
    let mut start = from;
    while let Some(rel) = text.get(start..)?.find(name) {
        let at = start + rel;
        let before = text[..at].chars().next_back();
        let after = text[at + name.len()..].chars().next();
        if !before.is_some_and(is_ident_char) && !after.is_some_and(is_ident_char) {
            return Some(at);
        }
        start = at + name.len();
    }
    None
}

fn classify_line(text: &str, name: &str) -> Mutability {
    let code = text.split("//").next().unwrap_or(text);
    let Some(let_at) = find_ident(code, "let", 0) else {
        return Mutability::Unknown;
    };
    // Le motif se trouve entre `let` et le `=` / `:` / `;` qui suit.
    let rest = &code[let_at + 3..];
    let end = rest
        .find(['=', ';'])
        .or_else(|| rest.find(':'))
        .unwrap_or(rest.len());
    let pattern = &rest[..end];
    // Les annotations de type (`: i32`) ne font pas partie du motif.
    let pattern = pattern.split(':').next().unwrap_or(pattern);

    let Some(at) = find_ident(pattern, name, 0) else {
        return Mutability::Unknown;
    };
    let before = pattern[..at].trim_end();
    if before.ends_with("mut") && !before[..before.len() - 3].ends_with(is_ident_char) {
        Mutability::Mutable
    } else {
        Mutability::Immutable
    }
}

/// Lit le fichier source en essayant le chemin enregistré puis, si une racine
/// est fournie, la fin du chemin rattachée à cette racine.
pub(crate) fn read_source(recorded: &str, root: Option<&Path>) -> Option<String> {
    if let Ok(text) = fs::read_to_string(recorded) {
        return Some(text);
    }
    let root = root?;
    let components: Vec<&str> = recorded.split('/').filter(|c| !c.is_empty()).collect();
    // On essaie de plus en plus court : `a/b/src/main.rs`, `b/src/main.rs`, ...
    (0..components.len()).find_map(|skip| {
        let candidate: PathBuf = root.join(components[skip..].join("/"));
        fs::read_to_string(candidate).ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_plain_and_mut_declarations() {
        assert_eq!(
            classify_line("    let a: i32 = 4;", "a"),
            Mutability::Immutable
        );
        assert_eq!(
            classify_line("    let mut a: i32 = 4;", "a"),
            Mutability::Mutable
        );
        assert_eq!(classify_line("let mut a = 4;", "a"), Mutability::Mutable);
        assert_eq!(
            classify_line("let a = 4; // mut", "a"),
            Mutability::Immutable
        );
    }

    #[test]
    fn patterns_with_several_bindings() {
        assert_eq!(
            classify_line("let (mut x, y) = f();", "x"),
            Mutability::Mutable
        );
        assert_eq!(
            classify_line("let (mut x, y) = f();", "y"),
            Mutability::Immutable
        );
    }

    #[test]
    fn names_are_matched_as_whole_identifiers() {
        assert_eq!(classify_line("let mut data = 1;", "a"), Mutability::Unknown);
        assert_eq!(
            classify_line("let remutable = 1;", "mutable"),
            Mutability::Unknown
        );
    }

    #[test]
    fn lines_without_a_declaration_are_unknown() {
        assert_eq!(classify_line("fn f(a: i32) {", "a"), Mutability::Unknown);
        assert_eq!(classify("let a = 1;", 5, "a"), Mutability::Unknown);
    }
}
