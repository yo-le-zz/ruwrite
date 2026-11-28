//! Detection of `mov [memory], immediate` instructions in x86 / x86_64 code.
//!
//! In an unoptimised build, `let a: i32 = 3;` compiles to a store of the
//! constant into the stack slot of `a`. Finding that store is what allows
//! ruwrite to change the initial value of a local variable.

use iced_x86::{ConditionCode, Decoder, DecoderOptions, Instruction, Mnemonic, OpKind, Register};

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

/// Whether a conditional jump reads its `cmp` as a signed or an unsigned
/// comparison. `cascade mode` ([`crate::edit::LocalEditor`]) keeps these
/// consistent with a variable's new signedness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JccFamily {
    /// `jl` / `jge` / `jle` / `jg`.
    Signed,
    /// `jb` / `jae` / `jbe` / `ja`.
    Unsigned,
}

/// A conditional jump found right after a `cmp` against the same stack slot
/// as a local variable.
#[derive(Debug, Clone, Copy)]
pub struct SlotComparison {
    /// Offset, in the scanned code, of the single byte that encodes the
    /// condition: the opcode itself for the short `0x7_` form, the byte
    /// after `0F` for the near `0F 8_` form.
    pub condition_offset: usize,
    /// Current family of the condition.
    pub family: JccFamily,
}

/// Scans `code` for a conditional jump that immediately follows a `cmp`
/// touching the stack slot at `base + disp` (the same test
/// [`scan_imm_stores`] uses to recognise a store to a local).
///
/// Only the direct `cmp [base+disp], ..` form is matched: a comparison done
/// through a register loaded from the slot is not tracked back to it, the
/// same limitation [`scan_imm_stores`] has for stores.
pub fn scan_slot_comparisons(
    code: &[u8],
    ip: u64,
    bitness: u32,
    base: Register,
    disp: i64,
) -> Vec<SlotComparison> {
    let mut decoder = Decoder::with_ip(bitness, code, ip, DecoderOptions::NONE);
    let mut found = Vec::new();
    let mut after_matching_cmp = false;

    while decoder.can_decode() {
        let offset = decoder.position();
        let insn = decoder.decode();
        if insn.is_invalid() {
            after_matching_cmp = false;
            continue;
        }
        if insn.mnemonic() == Mnemonic::Cmp {
            after_matching_cmp = touches_slot(&insn, bitness, base, disp);
            continue;
        }
        if after_matching_cmp {
            if let Some(family) = jcc_family(insn.condition_code()) {
                let condition_offset = match insn.len() {
                    2 => Some(offset),     // 0x7X rel8
                    6 => Some(offset + 1), // 0F 8X rel32
                    _ => None,
                };
                if let Some(condition_offset) = condition_offset {
                    found.push(SlotComparison {
                        condition_offset,
                        family,
                    });
                }
            }
        }
        after_matching_cmp = false;
    }
    found
}

fn touches_slot(insn: &Instruction, bitness: u32, base: Register, disp: i64) -> bool {
    for i in 0..insn.op_count() {
        if insn.op_kind(i) == OpKind::Memory
            && insn.memory_base() == base
            && insn.memory_index() == Register::None
            && signed_displacement(insn.memory_displacement64(), bitness) == disp
        {
            return true;
        }
    }
    false
}

fn jcc_family(cc: ConditionCode) -> Option<JccFamily> {
    match cc {
        ConditionCode::l | ConditionCode::ge | ConditionCode::le | ConditionCode::g => {
            Some(JccFamily::Signed)
        }
        ConditionCode::b | ConditionCode::ae | ConditionCode::be | ConditionCode::a => {
            Some(JccFamily::Unsigned)
        }
        _ => None,
    }
}

/// The byte that flips a conditional jump between the signed and the
/// unsigned reading of the same comparison (`jl` <-> `jb`, `jge` <-> `jae`,
/// `jle` <-> `jbe`, `jg` <-> `ja`), keeping the instruction's length and form
/// (short or near) identical. `None` for a condition with no counterpart in
/// the other family (`je`, `jo`, `js`, ...).
pub fn flip_condition_byte(current: u8) -> Option<u8> {
    const PAIRS: [(u8, u8); 4] = [(0xC, 0x2), (0xD, 0x3), (0xE, 0x6), (0xF, 0x7)];
    let high = current & 0xF0;
    let low = current & 0x0F;
    if high != 0x70 && high != 0x80 {
        return None;
    }
    for &(signed, unsigned) in &PAIRS {
        if low == signed {
            return Some(high | unsigned);
        }
        if low == unsigned {
            return Some(high | signed);
        }
    }
    None
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

    #[test]
    fn finds_a_signed_compare_against_the_slot_short_form() {
        // cmp dword ptr [rbp-4], 5 ; jl +0x10
        let code = [0x83, 0x7D, 0xFC, 0x05, 0x7C, 0x10];
        let found = scan_slot_comparisons(&code, 0, 64, Register::RBP, -4);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].condition_offset, 4);
        assert_eq!(found[0].family, JccFamily::Signed);
    }

    #[test]
    fn finds_an_unsigned_compare_near_form() {
        // cmp dword ptr [rbp-4], 5 ; ja (near, 0F 87 + rel32)
        let code = [0x83, 0x7D, 0xFC, 0x05, 0x0F, 0x87, 0x10, 0x00, 0x00, 0x00];
        let found = scan_slot_comparisons(&code, 0, 64, Register::RBP, -4);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].condition_offset, 5);
        assert_eq!(found[0].family, JccFamily::Unsigned);
    }

    #[test]
    fn ignores_a_compare_against_a_different_slot() {
        // cmp dword ptr [rbp-8], 5 ; jl +0x10 (watching slot at -4)
        let code = [0x83, 0x7D, 0xF8, 0x05, 0x7C, 0x10];
        assert!(scan_slot_comparisons(&code, 0, 64, Register::RBP, -4).is_empty());
    }

    #[test]
    fn ignores_a_jump_with_no_signed_unsigned_counterpart() {
        // cmp dword ptr [rbp-4], 5 ; je +0x10 (equality has no signedness)
        let code = [0x83, 0x7D, 0xFC, 0x05, 0x74, 0x10];
        assert!(scan_slot_comparisons(&code, 0, 64, Register::RBP, -4).is_empty());
    }

    #[test]
    fn flips_every_signed_unsigned_pair_and_rejects_the_rest() {
        assert_eq!(flip_condition_byte(0x7C), Some(0x72)); // jl  -> jb
        assert_eq!(flip_condition_byte(0x72), Some(0x7C)); // jb  -> jl
        assert_eq!(flip_condition_byte(0x7D), Some(0x73)); // jge -> jae
        assert_eq!(flip_condition_byte(0x7E), Some(0x76)); // jle -> jbe
        assert_eq!(flip_condition_byte(0x7F), Some(0x77)); // jg  -> ja
        assert_eq!(flip_condition_byte(0x8C), Some(0x82)); // near jl -> jb
        assert_eq!(flip_condition_byte(0x87), Some(0x8F)); // near ja -> jg
        assert_eq!(flip_condition_byte(0x74), None); // je has no counterpart
        assert_eq!(flip_condition_byte(0x00), None); // unrelated opcode
    }
}
