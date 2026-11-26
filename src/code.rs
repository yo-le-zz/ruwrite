//! Machine-code generation for the few instruction sequences ruwrite needs.
//!
//! Supported: x86, x86_64 and aarch64. Every function returns the exact bytes
//! to write; callers are responsible for checking that they fit.

use crate::arch::Arch;
use crate::error::{Error, Result};
use crate::value::Value;

const X86_RET: u8 = 0xC3;
const X86_NOP: u8 = 0x90;
const A64_RET: u32 = 0xD65F_03C0;
const A64_NOP: u32 = 0xD503_201F;

fn a64_bytes(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}

/// `ret` instruction (no value).
pub fn ret(arch: &Arch) -> Result<Vec<u8>> {
    match arch {
        Arch::X86 | Arch::X86_64 => Ok(vec![X86_RET]),
        Arch::Aarch64 => Ok(a64_bytes(&[A64_RET])),
        other => Err(Error::unsupported("code generation", other)),
    }
}

/// A run of `nop` instructions covering exactly `len` bytes.
pub fn nop_run(arch: &Arch, len: usize) -> Result<Vec<u8>> {
    match arch {
        Arch::X86 | Arch::X86_64 => Ok(vec![X86_NOP; len]),
        Arch::Aarch64 => {
            if len & 3 != 0 {
                return Err(Error::SizeMismatch {
                    expected: len.next_multiple_of(4),
                    got: len,
                });
            }
            Ok(a64_bytes(&vec![A64_NOP; len / 4]))
        }
        other => Err(Error::unsupported("code generation", other)),
    }
}

/// Code making a function return `value` immediately.
pub fn return_value(arch: &Arch, value: Value) -> Result<Vec<u8>> {
    match arch {
        Arch::X86_64 => x86_64_return(value),
        Arch::X86 => x86_return(value),
        Arch::Aarch64 => aarch64_return(value),
        other => Err(Error::unsupported("code generation", other)),
    }
}

/// Raw bit pattern of a value, sign-extended for signed integers so that the
/// whole register holds a well-formed number.
fn bits(value: Value) -> u64 {
    match value {
        Value::Bool(v) => u64::from(v),
        Value::I8(v) => i64::from(v) as u64,
        Value::I16(v) => i64::from(v) as u64,
        Value::I32(v) => i64::from(v) as u64,
        Value::I64(v) => v as u64,
        Value::U8(v) => u64::from(v),
        Value::U16(v) => u64::from(v),
        Value::U32(v) => u64::from(v),
        Value::U64(v) => v,
        Value::F32(v) => u64::from(v.to_bits()),
        Value::F64(v) => v.to_bits(),
    }
}

fn is_wide(value: Value) -> bool {
    matches!(value, Value::I64(_) | Value::U64(_) | Value::F64(_))
}

fn is_float(value: Value) -> bool {
    matches!(value, Value::F32(_) | Value::F64(_))
}

/// Forme la plus courte pour charger un entier dans `eax` puis rendre la main.
///
/// Les types de 1 octet (`bool`, `u8`, `i8`) sont rendus dans `al` : l'ABI
/// n'impose rien sur les bits hauts, `mov al, imm8` suffit (2 octets).
fn x86_load_eax(value: Value) -> Vec<u8> {
    let b = bits(value);
    if matches!(value, Value::Bool(_) | Value::U8(_) | Value::I8(_)) {
        vec![0xB0, b as u8] // mov al, imm8
    } else if b as u32 == 0 {
        vec![0x31, 0xC0] // xor eax, eax
    } else {
        let mut code = vec![0xB8]; // mov eax, imm32
        code.extend((b as u32).to_le_bytes());
        code
    }
}

fn x86_64_return(value: Value) -> Result<Vec<u8>> {
    let b = bits(value);
    let mut code = if is_wide(value) {
        let mut wide = vec![0x48, 0xB8]; // movabs rax, imm64
        wide.extend(b.to_le_bytes());
        wide
    } else {
        x86_load_eax(value)
    };
    if is_float(value) {
        if is_wide(value) {
            code.extend([0x66, 0x48, 0x0F, 0x6E, 0xC0]); // movq xmm0, rax
        } else {
            code.extend([0x66, 0x0F, 0x6E, 0xC0]); // movd xmm0, eax
        }
    }
    code.push(X86_RET);
    Ok(code)
}

fn x86_return(value: Value) -> Result<Vec<u8>> {
    if is_float(value) {
        // Les flottants sont rendus dans st(0) en x86 32 bits : non géré.
        return Err(Error::unsupported("returning a float", "x86"));
    }
    let b = bits(value);
    let mut code = x86_load_eax(value);
    if is_wide(value) {
        code.push(0xBA); // mov edx, imm32 (moitié haute)
        code.extend(((b >> 32) as u32).to_le_bytes());
    }
    code.push(X86_RET);
    Ok(code)
}

/// `movz`/`movk` sequence loading `value` into `x0`/`w0`.
fn a64_load_x0(value: u64, wide: bool) -> Vec<u32> {
    let (movz, movk, halves) = if wide {
        (0xD280_0000u32, 0xF280_0000u32, 4)
    } else {
        (0x5280_0000u32, 0x7280_0000u32, 2)
    };
    let mut words = vec![movz | (((value & 0xFFFF) as u32) << 5)];
    for hw in 1..halves {
        let part = ((value >> (16 * hw)) & 0xFFFF) as u32;
        if part != 0 {
            words.push(movk | ((hw as u32) << 21) | (part << 5));
        }
    }
    words
}

fn aarch64_return(value: Value) -> Result<Vec<u8>> {
    let mut words = a64_load_x0(bits(value), is_wide(value));
    match value {
        Value::F32(_) => words.push(0x1E27_0000), // fmov s0, w0
        Value::F64(_) => words.push(0x9E67_0000), // fmov d0, x0
        _ => {}
    }
    words.push(A64_RET);
    Ok(a64_bytes(&words))
}

/// Unconditional relative jump placed at `from`, landing on `to`.
pub fn jump(arch: &Arch, from: u64, to: u64) -> Result<Vec<u8>> {
    let out_of_range = Error::JumpOutOfRange { from, to };
    match arch {
        Arch::X86 | Arch::X86_64 => {
            // jmp rel32 : le déplacement est relatif à l'instruction suivante.
            let rel = to as i128 - (from as i128 + 5);
            let rel = i32::try_from(rel).map_err(|_| out_of_range)?;
            let mut code = vec![0xE9];
            code.extend(rel.to_le_bytes());
            Ok(code)
        }
        Arch::Aarch64 => {
            let delta = to as i128 - from as i128;
            if delta % 4 != 0 {
                return Err(out_of_range);
            }
            let imm26 = delta / 4;
            if !(-(1 << 25)..(1 << 25)).contains(&imm26) {
                return Err(out_of_range);
            }
            let word = 0x1400_0000u32 | ((imm26 as u32) & 0x03FF_FFFF);
            Ok(a64_bytes(&[word]))
        }
        other => Err(Error::unsupported("code generation", other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced_x86::{Decoder, DecoderOptions, Mnemonic, Register};

    fn decode(bitness: u32, code: &[u8]) -> Vec<iced_x86::Instruction> {
        let mut decoder = Decoder::new(bitness, code, DecoderOptions::NONE);
        let mut out = Vec::new();
        while decoder.can_decode() {
            out.push(decoder.decode());
        }
        out
    }

    #[test]
    fn x86_64_integer_return_decodes_to_mov_ret() {
        let code = return_value(&Arch::X86_64, Value::I32(-5)).unwrap();
        let insns = decode(64, &code);
        assert_eq!(insns.len(), 2);
        assert_eq!(insns[0].mnemonic(), Mnemonic::Mov);
        assert_eq!(insns[0].op0_register(), Register::EAX);
        assert_eq!(insns[0].immediate(1) as u32, (-5i32) as u32);
        assert_eq!(insns[1].mnemonic(), Mnemonic::Ret);
    }

    #[test]
    fn small_values_use_the_shortest_encoding() {
        assert_eq!(
            return_value(&Arch::X86_64, Value::Bool(true)).unwrap(),
            [0xB0, 0x01, 0xC3]
        );
        assert_eq!(
            return_value(&Arch::X86_64, Value::I32(0)).unwrap(),
            [0x31, 0xC0, 0xC3]
        );
        assert_eq!(
            return_value(&Arch::X86, Value::U8(9)).unwrap(),
            [0xB0, 0x09, 0xC3]
        );
    }

    #[test]
    fn x86_64_wide_return_uses_rax() {
        let code = return_value(&Arch::X86_64, Value::U64(0x1122_3344_5566_7788)).unwrap();
        let insns = decode(64, &code);
        assert_eq!(insns[0].op0_register(), Register::RAX);
        assert_eq!(insns[0].immediate(1), 0x1122_3344_5566_7788);
    }

    #[test]
    fn x86_64_float_return_goes_through_xmm0() {
        let code = return_value(&Arch::X86_64, Value::F32(1.5)).unwrap();
        let insns = decode(64, &code);
        assert_eq!(insns.len(), 3);
        assert_eq!(insns[1].mnemonic(), Mnemonic::Movd);
        assert_eq!(insns[1].op0_register(), Register::XMM0);
        let code = return_value(&Arch::X86_64, Value::F64(2.5)).unwrap();
        let insns = decode(64, &code);
        assert_eq!(insns[1].mnemonic(), Mnemonic::Movq);
    }

    #[test]
    fn x86_32_wide_return_fills_edx_eax() {
        let code = return_value(&Arch::X86, Value::U64(0x1_0000_0002)).unwrap();
        let insns = decode(32, &code);
        assert_eq!(insns.len(), 3);
        assert_eq!(insns[0].immediate(1), 2);
        assert_eq!(insns[1].op0_register(), Register::EDX);
        assert_eq!(insns[1].immediate(1), 1);
        assert!(return_value(&Arch::X86, Value::F32(1.0)).is_err());
    }

    #[test]
    fn x86_jump_is_relative_to_next_instruction() {
        let code = jump(&Arch::X86_64, 0x1000, 0x2000).unwrap();
        assert_eq!(code, [0xE9, 0xFB, 0x0F, 0x00, 0x00]);
        // Saut en arrière : rel32 = 0x1000 - 0x2005 = -0x1005.
        let back = jump(&Arch::X86_64, 0x2000, 0x1000).unwrap();
        assert_eq!(back, [0xE9, 0xFB, 0xEF, 0xFF, 0xFF]);
        assert!(jump(&Arch::X86_64, 0, 1 << 40).is_err());
    }

    #[test]
    fn aarch64_known_encodings() {
        // Valeurs de référence de l'ISA : ret, mov w0,#42, b .+8
        assert_eq!(ret(&Arch::Aarch64).unwrap(), [0xC0, 0x03, 0x5F, 0xD6]);
        let code = return_value(&Arch::Aarch64, Value::I32(42)).unwrap();
        assert_eq!(code, [0x40, 0x05, 0x80, 0x52, 0xC0, 0x03, 0x5F, 0xD6]);
        assert_eq!(
            jump(&Arch::Aarch64, 0x1000, 0x1008).unwrap(),
            [0x02, 0x00, 0x00, 0x14]
        );
        // Saut en arrière : b .-4 (imm26 = -1)
        assert_eq!(
            jump(&Arch::Aarch64, 0x1004, 0x1000).unwrap(),
            [0xFF, 0xFF, 0xFF, 0x17]
        );
    }

    #[test]
    fn aarch64_large_constants_use_movk() {
        let code = return_value(&Arch::Aarch64, Value::U32(0x0001_0002)).unwrap();
        // movz w0,#2 ; movk w0,#1,lsl #16 ; ret
        assert_eq!(code.len(), 12);
        let code = return_value(&Arch::Aarch64, Value::U64(u64::MAX)).unwrap();
        assert_eq!(code.len(), 5 * 4);
    }

    #[test]
    fn unsupported_arch_is_reported() {
        assert!(matches!(
            ret(&Arch::Riscv64),
            Err(Error::Unsupported { .. })
        ));
    }

    #[test]
    fn aarch64_nops_need_alignment() {
        assert_eq!(nop_run(&Arch::Aarch64, 8).unwrap().len(), 8);
        assert!(nop_run(&Arch::Aarch64, 6).is_err());
        assert_eq!(nop_run(&Arch::X86_64, 3).unwrap(), [0x90; 3]);
    }
}
