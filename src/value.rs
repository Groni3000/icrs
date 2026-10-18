//! One value read out of the game, whatever its type.

use crate::mem::Quad;
use crate::offsets::Kind;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i64),
    Float(f64),
    Big(Quad),
    Text(String),
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Int(value) => write!(f, "{value}"),
            Value::Float(value) => write!(f, "{value}"),
            Value::Big(value) => write!(f, "{value}"),
            Value::Text(value) => write!(f, "{value:?}"),
        }
    }
}


impl Value {
    /// Pull a value out of bytes already read from the game, which is how a
    /// bulk read turns one block into many values. `Kind::Utf16` is not here:
    /// that field only holds a pointer, and following it is another read.
    pub fn decode(kind: Kind, bytes: &[u8]) -> Option<Self> {
        let wanted = kind.size();
        if bytes.len() < wanted {
            return None;
        }
        let eight = |at: usize| -> [u8; 8] { bytes[at..at + 8].try_into().unwrap() };
        let four = |at: usize| -> [u8; 4] { bytes[at..at + 4].try_into().unwrap() };
        Some(match kind {
            Kind::I32 => Value::Int(i32::from_le_bytes(four(0)) as i64),
            Kind::I64 => Value::Int(i64::from_le_bytes(eight(0))),
            Kind::U8 => Value::Int(bytes[0] as i64),
            Kind::F32 => Value::Float(f32::from_le_bytes(four(0)) as f64),
            Kind::F64 => Value::Float(f64::from_le_bytes(eight(0))),
            Kind::Quad => Value::Big(Quad {
                mantissa: i64::from_le_bytes(eight(0)),
                exponent: i64::from_le_bytes(eight(8)),
            }),
            Kind::Utf16 => return None,
        })
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(value) => Some(*value as f64),
            Value::Float(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            Value::Text(value) => Some(value),
            _ => None,
        }
    }

    /// A `Char` field is the game's bool.
    pub fn as_bool(&self) -> Option<bool> {
        self.as_i64().map(|value| value != 0)
    }
}
