//! Implementation detail of `ruwrite`. Depend on `ruwrite` and use
//! `ruwrite::protect`; nothing here is meant to be used directly.

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, Ident, Item};

const MAGIC: &[u8; 8] = b"RUWPROT2";

/// `RUWPROT2` record: `magic | target_kind(1) | mode(1) | name_encoding(1) |
/// name_len(2, little-endian) | name or hash`. See `ruwrite::protect` for
/// what each field means; this function only lays out the bytes.
fn build_record(target_kind: u8, mode: u8, name: &str, hide_name: bool) -> Vec<u8> {
    let (name_encoding, payload): (u8, Vec<u8>) = if hide_name {
        (1, fnv1a64(name).to_le_bytes().to_vec())
    } else {
        (0, name.as_bytes().to_vec())
    };
    let mut record = Vec::with_capacity(11 + payload.len());
    record.extend_from_slice(MAGIC);
    record.push(target_kind);
    record.push(mode);
    record.push(name_encoding);
    record.extend_from_slice(&(payload.len() as u16).to_le_bytes());
    record.extend_from_slice(&payload);
    record
}

/// Same hash `ruwrite` uses to recognise a hidden name without storing it in
/// the binary. Not cryptographic: it only needs to be stable and cheap.
fn fnv1a64(s: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Declares how `ruwrite` is allowed to treat the function or `static` it is
/// placed on.
///
/// ```ignore
/// # use ruwrite::protect;
/// #[protect(immutable)]
/// fn check_license() -> bool { true }
///
/// #[protect(protected)]
/// static SECRET_KEY: u32 = 0xDEAD_BEEF;
/// ```
///
/// Modes, from least to most restrictive:
///
/// | Mode | Meaning |
/// |------|---------|
/// | `editable` | explicitly not protected; the same as writing nothing, spelled out |
/// | `immutable` | `ruwrite` refuses every edit that touches it |
/// | `protected` | `immutable`, and also included when the program later checks its own integrity (see `Executable::integrity_snapshot`) |
/// | `hide` | `immutable`, and its name is not stored as readable text in the binary and is left out of `Executable::functions()` / `variables()`; see the limitations below |
///
/// `ruwrite` reads this declaration back from the compiled executable, so
/// every edit through it is checked, however it is made: by name, by address,
/// by raw bytes, or by content ([`Executable::find_value`]). It is the
/// program declaring the rule, not `ruwrite` guessing at one — there is
/// deliberately no option to bypass it.
///
/// **What `hide` does and does not do.** It keeps the plain name out of the
/// `.ruwprot` record (a hash is stored instead, so `ruwrite` can still match
/// the right item without ever holding its readable name) and filters it out
/// of `ruwrite`'s own listings. It does not rename the function's debug or
/// symbol-table entry, strip its machine code, or defend against
/// disassembly: a determined reader with `objdump` still sees it. Build
/// without debug info and strip symbols for that.
///
/// Mark a protected function `#[inline(never)]` so the compiler keeps one
/// identifiable copy of it instead of folding it into every caller.
#[proc_macro_attribute]
pub fn protect(attr: TokenStream, item: TokenStream) -> TokenStream {
    let mode_ident = parse_macro_input!(attr as Ident);
    let mode_name = mode_ident.to_string();
    let mode: u8 = match mode_name.as_str() {
        "editable" => 0,
        "immutable" => 1,
        "protected" => 2,
        "hide" => 3,
        other => {
            return syn::Error::new(
                mode_ident.span(),
                format!(
                    "unknown mode `{other}` for `#[ruwrite::protect(..)]`; \
                     expected one of: editable, immutable, protected, hide"
                ),
            )
            .to_compile_error()
            .into();
        }
    };

    let parsed = parse_macro_input!(item as Item);
    let (name, target_kind) = match &parsed {
        Item::Fn(f) => (f.sig.ident.to_string(), 1u8),
        Item::Static(s) => (s.ident.to_string(), 2u8),
        other => {
            return syn::Error::new_spanned(
                other,
                "`#[ruwrite::protect(..)]` can only be placed on a `fn` or a `static`",
            )
            .to_compile_error()
            .into();
        }
    };

    if mode == 0 {
        // `editable` : rien à faire respecter, donc rien à embarquer non plus.
        return quote! { #parsed }.into();
    }

    let record = build_record(target_kind, mode, &name, mode == 3);
    let len = record.len();
    let bytes = record.iter().copied();

    quote! {
        #parsed

        const _: () = {
            #[used]
            #[cfg_attr(
                any(target_os = "macos", target_os = "ios"),
                link_section = "__DATA,__ruwprot"
            )]
            #[cfg_attr(not(any(target_os = "macos", target_os = "ios")), link_section = ".ruwprot")]
            static RUWRITE_RECORD: [u8; #len] = [ #(#bytes),* ];
        };
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_record_layout() {
        let r = build_record(1, 1, "check", false);
        assert_eq!(&r[..8], MAGIC);
        assert_eq!(r[8], 1); // function
        assert_eq!(r[9], 1); // immutable
        assert_eq!(r[10], 0); // plain name
        assert_eq!(&r[11..13], [5, 0]);
        assert_eq!(&r[13..], b"check");
    }

    #[test]
    fn hidden_record_stores_a_hash_not_the_name() {
        let r = build_record(1, 3, "secret_fn", true);
        assert_eq!(r[9], 3); // hide
        assert_eq!(r[10], 1); // hashed
        assert_eq!(&r[11..13], [8, 0]);
        assert_eq!(r.len(), 21);
        assert!(
            r.windows(9).all(|w| w != b"secret_fn"),
            "the plain name must not appear in the record"
        );
    }

    #[test]
    fn hash_is_deterministic() {
        assert_eq!(fnv1a64("check"), fnv1a64("check"));
        assert_ne!(fnv1a64("check"), fnv1a64("checked"));
    }
}
