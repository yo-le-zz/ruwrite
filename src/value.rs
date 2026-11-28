//! Scalar values that can be read from or written to a variable.

use std::fmt;

use crate::arch::Endian;
use crate::error::{Error, Result};

/// Broad category of a scalar type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// `bool`.
    Bool,
    /// Signed integers (`i8` .. `i64`, `isize`, `i128`).
    Signed,
    /// Unsigned integers (`u8` .. `u64`, `usize`, `u128`) and characters.
    Unsigned,
    /// `f32` and `f64`.
    Float,
    /// Anything else (structs, arrays, pointers...): only raw bytes can be written.
    Other,
}

impl Class {
    fn expected(self) -> &'static str {
        match self {
            Class::Bool => "a bool",
            Class::Signed | Class::Unsigned => "an integer",
            Class::Float => "a float",
            Class::Other => "raw bytes",
        }
    }
}

/// Description of the storage of a scalar: its category and its size in bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScalarType {
    /// Name as written in the source (`i32`, `bool`, ...).
    pub name: String,
    /// Category of the type.
    pub class: Class,
    /// Size in bytes.
    pub size: usize,
}

/// A scalar value (`bool`, integers, floats).
///
/// Every primitive converts into a `Value` with `From`, so APIs taking
/// `impl Into<Value>` accept plain literals: `exe.set_static("LIMIT", 5)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value {
    /// `bool`.
    Bool(bool),
    /// `i8`.
    I8(i8),
    /// `i16`.
    I16(i16),
    /// `i32`.
    I32(i32),
    /// `i64`.
    I64(i64),
    /// `u8`.
    U8(u8),
    /// `u16`.
    U16(u16),
    /// `u32`.
    U32(u32),
    /// `u64`.
    U64(u64),
    /// `f32`.
    F32(f32),
    /// `f64`.
    F64(f64),
}

macro_rules! impl_from {
    ($($ty:ty => $variant:ident),* $(,)?) => {
        $(impl From<$ty> for Value {
            fn from(v: $ty) -> Self { Value::$variant(v) }
        })*
    };
}

impl_from!(
    bool => Bool, i8 => I8, i16 => I16, i32 => I32, i64 => I64,
    u8 => U8, u16 => U16, u32 => U32, u64 => U64, f32 => F32, f64 => F64,
);

macro_rules! impl_named_constructor {
    ($($name:ident: $ty:ty => $variant:ident),* $(,)?) => {
        $(
            #[doc = concat!(
                "Builds a `", stringify!($ty), "` value: the number stays separate \\
                 from its type, instead of a literal suffix like `3", stringify!($ty), "`."
            )]
            pub fn $name(n: $ty) -> Value {
                Value::$variant(n)
            }
        )*
    };
}

impl Value {
    /// Builds a `bool` value.
    pub fn bool(b: bool) -> Value {
        Value::Bool(b)
    }

    impl_named_constructor!(
        i8: i8 => I8, i16: i16 => I16, i32: i32 => I32, i64: i64 => I64,
        u8: u8 => U8, u16: u16 => U16, u32: u32 => U32, u64: u64 => U64,
        f32: f32 => F32, f64: f64 => F64,
    );
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Bool(v) => write!(f, "{v}"),
            Value::I8(v) => write!(f, "{v}"),
            Value::I16(v) => write!(f, "{v}"),
            Value::I32(v) => write!(f, "{v}"),
            Value::I64(v) => write!(f, "{v}"),
            Value::U8(v) => write!(f, "{v}"),
            Value::U16(v) => write!(f, "{v}"),
            Value::U32(v) => write!(f, "{v}"),
            Value::U64(v) => write!(f, "{v}"),
            Value::F32(v) => write!(f, "{v}"),
            Value::F64(v) => write!(f, "{v}"),
        }
    }
}

impl Value {
    /// Rust name of the type of this value (`"i32"`, `"u8"`, ...).
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Bool(_) => "bool",
            Value::I8(_) => "i8",
            Value::I16(_) => "i16",
            Value::I32(_) => "i32",
            Value::I64(_) => "i64",
            Value::U8(_) => "u8",
            Value::U16(_) => "u16",
            Value::U32(_) => "u32",
            Value::U64(_) => "u64",
            Value::F32(_) => "f32",
            Value::F64(_) => "f64",
        }
    }

    /// Size in bytes of this value's own type (1 for `bool`/`i8`/`u8`, 8 for
    /// `i64`/`u64`/`f64`, ...).
    pub fn size(&self) -> usize {
        match self {
            Value::Bool(_) | Value::I8(_) | Value::U8(_) => 1,
            Value::I16(_) | Value::U16(_) => 2,
            Value::I32(_) | Value::U32(_) | Value::F32(_) => 4,
            Value::I64(_) | Value::U64(_) | Value::F64(_) => 8,
        }
    }

    /// Category of this value.
    pub fn class(&self) -> Class {
        match self {
            Value::Bool(_) => Class::Bool,
            Value::I8(_) | Value::I16(_) | Value::I32(_) | Value::I64(_) => Class::Signed,
            Value::U8(_) | Value::U16(_) | Value::U32(_) | Value::U64(_) => Class::Unsigned,
            Value::F32(_) | Value::F64(_) => Class::Float,
        }
    }

    /// Integer content widened to `i128`, `None` for bools and floats.
    pub fn as_int(&self) -> Option<i128> {
        Some(match *self {
            Value::I8(v) => v.into(),
            Value::I16(v) => v.into(),
            Value::I32(v) => v.into(),
            Value::I64(v) => v.into(),
            Value::U8(v) => v.into(),
            Value::U16(v) => v.into(),
            Value::U32(v) => v.into(),
            Value::U64(v) => v.into(),
            _ => return None,
        })
    }

    /// The type of this value itself (its own name, class and size),
    /// usable as a [`ScalarType`] when no variable description is at hand —
    /// this is what lets a value be recognised in memory without DWARF.
    pub fn own_type(&self) -> ScalarType {
        ScalarType {
            name: self.type_name().into(),
            class: self.class(),
            size: self.size(),
        }
    }

    /// Encodes this value for a destination of the given type.
    ///
    /// Integers are range-checked against the destination (so `2u8` can be
    /// stored in an `i32` slot, but `300` cannot be stored in a `u8` slot).
    pub fn encode(&self, target: &ScalarType, endian: Endian) -> Result<Vec<u8>> {
        let mismatch = || Error::TypeMismatch {
            target: target.name.clone(),
            expected: target.class.expected(),
            got: self.type_name(),
        };
        let out_of_range = || Error::ValueOutOfRange {
            value: self.to_string(),
            target: format!("{} ({} bytes)", target.name, target.size),
        };

        match target.class {
            Class::Bool => match self {
                Value::Bool(v) => Ok(vec![u8::from(*v)]),
                _ => Err(mismatch()),
            },
            Class::Signed | Class::Unsigned => {
                let v = self.as_int().ok_or_else(mismatch)?;
                if target.size == 0 || target.size > 16 {
                    return Err(out_of_range());
                }
                let bits = target.size * 8;
                let (min, max) = if target.class == Class::Signed {
                    (-(1i128 << (bits - 1)), (1i128 << (bits - 1)) - 1)
                } else if bits == 128 {
                    (0, i128::MAX)
                } else {
                    (0, (1i128 << bits) - 1)
                };
                if v < min || v > max {
                    return Err(out_of_range());
                }
                let le = v.to_le_bytes();
                let mut bytes = le[..target.size].to_vec();
                if endian == Endian::Big {
                    bytes.reverse();
                }
                Ok(bytes)
            }
            Class::Float => {
                let mut bytes = match (self, target.size) {
                    (Value::F32(v), 4) => v.to_le_bytes().to_vec(),
                    (Value::F64(v), 8) => v.to_le_bytes().to_vec(),
                    (Value::F64(v), 4) => (*v as f32).to_le_bytes().to_vec(),
                    (Value::F32(v), 8) => f64::from(*v).to_le_bytes().to_vec(),
                    (Value::F32(_) | Value::F64(_), _) => return Err(out_of_range()),
                    _ => return Err(mismatch()),
                };
                if endian == Endian::Big {
                    bytes.reverse();
                }
                Ok(bytes)
            }
            Class::Other => Err(mismatch()),
        }
    }

    /// Decodes bytes read from a variable of the given type.
    pub fn decode(bytes: &[u8], target: &ScalarType, endian: Endian) -> Result<Value> {
        if bytes.len() != target.size {
            return Err(Error::SizeMismatch {
                expected: target.size,
                got: bytes.len(),
            });
        }
        let mut le = bytes.to_vec();
        if endian == Endian::Big {
            le.reverse();
        }
        let unsupported = || Error::TypeMismatch {
            target: target.name.clone(),
            expected: "a scalar of 1, 2, 4 or 8 bytes",
            got: "raw bytes",
        };
        match (target.class, target.size) {
            (Class::Bool, 1) => Ok(Value::Bool(le[0] != 0)),
            (Class::Signed, 1) => Ok(Value::I8(le[0] as i8)),
            (Class::Signed, 2) => Ok(Value::I16(i16::from_le_bytes([le[0], le[1]]))),
            (Class::Signed, 4) => Ok(Value::I32(i32::from_le_bytes(le[..4].try_into().unwrap()))),
            (Class::Signed, 8) => Ok(Value::I64(i64::from_le_bytes(le[..8].try_into().unwrap()))),
            (Class::Unsigned, 1) => Ok(Value::U8(le[0])),
            (Class::Unsigned, 2) => Ok(Value::U16(u16::from_le_bytes([le[0], le[1]]))),
            (Class::Unsigned, 4) => Ok(Value::U32(u32::from_le_bytes(le[..4].try_into().unwrap()))),
            (Class::Unsigned, 8) => Ok(Value::U64(u64::from_le_bytes(le[..8].try_into().unwrap()))),
            (Class::Float, 4) => Ok(Value::F32(f32::from_le_bytes(le[..4].try_into().unwrap()))),
            (Class::Float, 8) => Ok(Value::F64(f64::from_le_bytes(le[..8].try_into().unwrap()))),
            _ => Err(unsupported()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ty(name: &str, class: Class, size: usize) -> ScalarType {
        ScalarType {
            name: name.into(),
            class,
            size,
        }
    }

    #[test]
    fn small_unsigned_fits_in_wider_signed_slot() {
        let bytes = Value::from(2u8)
            .encode(&ty("i32", Class::Signed, 4), Endian::Little)
            .unwrap();
        assert_eq!(bytes, [2, 0, 0, 0]);
    }

    #[test]
    fn out_of_range_is_rejected() {
        let err = Value::from(300).encode(&ty("u8", Class::Unsigned, 1), Endian::Little);
        assert!(matches!(err, Err(Error::ValueOutOfRange { .. })));
        let err = Value::from(-1).encode(&ty("u32", Class::Unsigned, 4), Endian::Little);
        assert!(matches!(err, Err(Error::ValueOutOfRange { .. })));
    }

    #[test]
    fn negative_numbers_are_two_complement() {
        let bytes = Value::from(-2i32)
            .encode(&ty("i16", Class::Signed, 2), Endian::Little)
            .unwrap();
        assert_eq!(bytes, [0xfe, 0xff]);
    }

    #[test]
    fn big_endian_reverses_bytes() {
        let bytes = Value::from(0x0102u16)
            .encode(&ty("u16", Class::Unsigned, 2), Endian::Big)
            .unwrap();
        assert_eq!(bytes, [0x01, 0x02]);
    }

    #[test]
    fn kinds_do_not_mix() {
        let err = Value::from(1.5f32).encode(&ty("i32", Class::Signed, 4), Endian::Little);
        assert!(matches!(err, Err(Error::TypeMismatch { .. })));
        let err = Value::from(1).encode(&ty("bool", Class::Bool, 1), Endian::Little);
        assert!(matches!(err, Err(Error::TypeMismatch { .. })));
    }

    #[test]
    fn floats_round_trip() {
        let t = ty("f32", Class::Float, 4);
        let bytes = Value::from(1.5f32).encode(&t, Endian::Little).unwrap();
        assert_eq!(
            Value::decode(&bytes, &t, Endian::Little).unwrap(),
            Value::F32(1.5)
        );
        let t = ty("f64", Class::Float, 8);
        let bytes = Value::from(-0.25f64).encode(&t, Endian::Big).unwrap();
        assert_eq!(
            Value::decode(&bytes, &t, Endian::Big).unwrap(),
            Value::F64(-0.25)
        );
    }

    #[test]
    fn decode_checks_length() {
        let t = ty("u32", Class::Unsigned, 4);
        assert!(matches!(
            Value::decode(&[1, 2], &t, Endian::Little),
            Err(Error::SizeMismatch { .. })
        ));
    }
}
