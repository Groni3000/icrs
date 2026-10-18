//! One value read out of the game, whatever its type.

use crate::mem::Quad;

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
