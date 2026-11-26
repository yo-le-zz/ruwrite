//! Detection of `mov [memory], immediate` instructions in x86 / x86_64 code.
//!
//! In an unoptimised build, `let a: i32 = 3;` compiles to a store of the
//! constant into the stack slot of `a`. Finding that store is what allows
//! ruwrite to change the initial value of a local variable.

use iced_x86::{Decoder, DecoderOptions, Mnemonic, OpKind, Register};

/// A `mov [mem], imm` instruction.
#[derive(Debug, Clone)]
pub struct ImmStore {
    /// Offset of the instruction in the analysed code.
    pub offset: usize,
    /// Offset of the immediate inside the instruction.
    pub imm_offset: usize,
    /// Number of bytes occupied by the immediate in the encoding.
    pub imm_size: usize,
    /// Number of bytes written to memory.
    pub mem_size: usize,
    /// Base register of the memory operand.
    pub base: Register,
    /// `true` when the memory operand uses an index register.
    pub has_index: bool,
    /// Displacement of the memory operand.
    pub disp: i64,
    /// Immediate, sign-extended to 64 bits.
    pub value: i64,
}

#[cfg(test)]
impl ImmStore {
    /// The stored value truncated to `mem_size` bytes (as raw bits).
    pub fn stored_bits(&self) -> u64 {
        mask(self.value as u64, self.mem_size)
    }
}

/// Keeps the low `size` bytes of `bits`.
pub fn mask(bits: u64, size: usize) -> u64 {
    if size >= 8 {
        bits
    } else {
        bits & ((1u64 << (size * 8)) - 1)
    }
}

/// Scans `code` (loaded at virtual address `ip`) and lists every
/// `mov [memory], immediate`.
pub fn scan_imm_stores(code: &[u8], ip: u64, bitness: u32) -> Vec<ImmStore> {
    let mut decoder = Decoder::with_ip(bitness, code, ip, DecoderOptions::NONE);
    let mut stores = Vec::new();

    while decoder.can_decode() {
        let offset = decoder.position();
        let insn = decoder.decode();
        if insn.is_invalid() || insn.mnemonic() != Mnemonic::Mov {
            continue;
        }
        if insn.op0_kind() != OpKind::Memory || !is_immediate(insn.op1_kind()) {
            continue;
        }
        let constants = decoder.get_constant_offsets(&insn);
        if !constants.has_immediate() {
            continue;
        }
        stores.push(ImmStore {
            offset,
            imm_offset: constants.immediate_offset(),
            imm_size: constants.immediate_size(),
            mem_size: insn.memory_size().size(),
            base: insn.memory_base(),
            has_index: insn.memory_index() != Register::None,
            disp: signed_displacement(insn.memory_displacement64(), bitness),
            value: insn.immediate(1) as i64,
        });
    }
    stores
}

fn is_immediate(kind: OpKind) -> bool {
    matches!(
        kind,
        OpKind::Immediate8
            | OpKind::Immediate16
            | OpKind::Immediate32
            | OpKind::Immediate64
            | OpKind::Immediate8to16
            | OpKind::Immediate8to32
            | OpKind::Immediate8to64
            | OpKind::Immediate32to64
    )
}

/// iced renvoie le déplacement sous forme non signée : on le ré-étend.
fn signed_displacement(raw: u64, bitness: u32) -> i64 {
    if bitness == 32 {
        i64::from(raw as u32 as i32)
    } else {
        raw as i64
    }
}

/// Encode `mov <size> ptr [base + disp], imm` for a stack-slot base register.
///
/// `value` holds the little-endian bytes of the stored value (1, 2, 4 or 8
/// bytes). An 8-byte store only has a sign-extended 32-bit immediate, so the
/// value must fit; otherwise `None` is returned (as for an unsupported base).
pub fn encode_store(base: Register, disp: i32, value: &[u8]) -> Option<Vec<u8>> {
    let rm = match base {
        Register::RSP | Register::ESP => 4u8,
        Register::RBP | Register::EBP => 5u8,
        _ => return None,
    };
    let wide = base.is_gpr64();
    let mut code = Vec::new();
    let imm: &[u8] = match value.len() {
        1 => {
            code.push(0xC6);
            value
        }
        2 => {
            code.extend([0x66, 0xC7]);
            value
        }
        4 => {
            code.push(0xC7);
            value
        }
        8 if wide => {
            let low = i32::from_le_bytes(value[..4].try_into().ok()?);
            let full = i64::from_le_bytes(value.try_into().ok()?);
            if i64::from(low) != full {
                return None;
            }
            code.extend([0x48, 0xC7]);
            &value[..4]
        }
        _ => return None,
    };
    // Déplacement sur 1 octet quand il tient : même taille que le compilateur.
    let short = i8::try_from(disp).ok();
    code.push(
        if short.is_some() {
            0b0100_0000
        } else {
            0b1000_0000
        } | rm,
    ); // reg=/0
    if rm == 4 {
        code.push(0x24); // SIB : base=rsp/esp, sans index
    }
    match short {
        Some(d) => code.push(d as u8),
        None => code.extend(disp.to_le_bytes()),
    }
    code.extend(imm);
    Some(code)
}

/// Register used as DWARF frame base, translated to an iced register.
pub fn frame_register(dwarf_reg: u16, bitness: u32) -> Option<Register> {
    match (bitness, dwarf_reg) {
        (64, 6) => Some(Register::RBP),
        (64, 7) => Some(Register::RSP),
        (32, 4) => Some(Register::ESP),
        (32, 5) => Some(Register::EBP),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_dword_store_with_rsp_base() {
        // mov dword ptr [rsp+0x14], 3 ; ret
        let code = [0xC7, 0x44, 0x24, 0x14, 0x03, 0x00, 0x00, 0x00, 0xC3];
        let stores = scan_imm_stores(&code, 0x1000, 64);
        assert_eq!(stores.len(), 1);
        let s = &stores[0];
        assert_eq!(s.offset, 0);
        assert_eq!(s.imm_offset, 4);
        assert_eq!(s.imm_size, 4);
        assert_eq!(s.mem_size, 4);
        assert_eq!(s.base, Register::RSP);
        assert_eq!(s.disp, 0x14);
        assert_eq!(s.value, 3);
    }

    #[test]
    fn finds_byte_store_and_negative_displacement() {
        // mov byte ptr [rbp-4], 2
        let code = [0xC6, 0x45, 0xFC, 0x02];
        let stores = scan_imm_stores(&code, 0, 64);
        assert_eq!(stores.len(), 1);
        assert_eq!(stores[0].mem_size, 1);
        assert_eq!(stores[0].imm_size, 1);
        assert_eq!(stores[0].disp, -4);
        assert_eq!(stores[0].base, Register::RBP);
    }

    #[test]
    fn qword_store_uses_a_sign_extended_imm32() {
        // mov qword ptr [rsp+8], -1
        let code = [0x48, 0xC7, 0x44, 0x24, 0x08, 0xFF, 0xFF, 0xFF, 0xFF];
        let stores = scan_imm_stores(&code, 0, 64);
        assert_eq!(stores[0].mem_size, 8);
        assert_eq!(stores[0].imm_size, 4);
        assert_eq!(stores[0].value, -1);
        assert_eq!(stores[0].stored_bits(), u64::MAX);
    }

    #[test]
    fn ignores_register_moves_and_loads() {
        // mov eax, 3 ; mov eax, [rsp+4] ; mov [rsp+4], eax
        let code = [
            0xB8, 0x03, 0x00, 0x00, 0x00, 0x8B, 0x44, 0x24, 0x04, 0x89, 0x44, 0x24, 0x04,
        ];
        assert!(scan_imm_stores(&code, 0, 64).is_empty());
    }

    #[test]
    fn encoded_stores_decode_back_to_the_same_slot() {
        let cases: [(Register, i32, &[u8]); 4] = [
            (Register::RSP, 0x14, &[9, 0, 0, 0]),
            (Register::RBP, -4, &[2]),
            (
                Register::RSP,
                8,
                &[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF],
            ),
            (Register::RSP, 0x100, &[0x34, 0x12]),
        ];
        for (base, disp, value) in cases {
            let code = encode_store(base, disp, value).unwrap();
            let stores = scan_imm_stores(&code, 0, 64);
            assert_eq!(stores.len(), 1, "{code:02x?}");
            assert_eq!(stores[0].base, base);
            assert_eq!(stores[0].disp, i64::from(disp));
            assert_eq!(stores[0].mem_size, value.len());
            assert_eq!(stores[0].offset, 0);
        }
        assert!(encode_store(Register::RSP, 0, &[0; 8]).is_some());
        assert!(encode_store(Register::RSP, 0, &[0, 0, 0, 0x80, 0, 0, 0, 0]).is_none());
        assert!(encode_store(Register::RAX, 0, &[1]).is_none());
    }

    #[test]
    fn masks_to_the_stored_width() {
        assert_eq!(mask(0x1_2345_6789, 4), 0x2345_6789);
        assert_eq!(mask(u64::MAX, 8), u64::MAX);
        assert_eq!(mask(0x1FF, 1), 0xFF);
    }
}
