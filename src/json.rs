//! A minimal JSON reader, enough for `IC_Offsets.json`.
//!
//! That file is machine-generated and flat - objects of numbers, short arrays
//! of numbers, and strings - so a few hundred lines here keeps the crate
//! dependency-free apart from `windows-sys`.

use std::collections::BTreeMap;
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Value>),
    Object(BTreeMap<String, Value>),
}

#[derive(Debug)]
pub struct Error {
    pub at: usize,
    pub reason: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "byte {}: {}", self.at, self.reason)
    }
}

impl std::error::Error for Error {}

impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(map) => map.get(key),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Value::Number(n) if *n >= 0.0 && n.fract() == 0.0 => Some(*n as u64),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(items) => Some(items),
            _ => None,
        }
    }

    /// An array of non-negative integers, which is what every offset list is.
    pub fn as_u64_vec(&self) -> Option<Vec<u64>> {
        self.as_array()?.iter().map(Value::as_u64).collect()
    }

    /// The generator writes a missing number as `""`, so "absent" and "empty
    /// string" mean the same thing and both become `None`.
    pub fn as_optional_u64(&self) -> Option<u64> {
        match self {
            Value::String(s) if s.is_empty() => None,
            Value::String(s) => s.trim().parse().ok(),
            other => other.as_u64(),
        }
    }

    /// `657`, `"657"` and `"656.1"` all have to read the same way, because the
    /// feed is inconsistent about quoting its version fields.
    pub fn as_version_string(&self) -> Option<String> {
        match self {
            Value::Number(n) if n.fract() == 0.0 => Some(format!("{}", *n as i64)),
            Value::Number(n) => Some(format!("{n}")),
            Value::String(s) if !s.is_empty() => Some(s.clone()),
            _ => None,
        }
    }
}

pub fn parse(input: &str) -> Result<Value, Error> {
    let mut parser = Parser { bytes: input.as_bytes(), at: 0 };
    parser.skip_whitespace();
    let value = parser.value()?;
    parser.skip_whitespace();
    if parser.at != parser.bytes.len() {
        return Err(parser.error("trailing content after the top-level value"));
    }
    Ok(value)
}

struct Parser<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Parser<'_> {
    fn error(&self, reason: &str) -> Error {
        Error { at: self.at, reason: reason.to_string() }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.at += 1;
        }
    }

    fn expect(&mut self, byte: u8) -> Result<(), Error> {
        if self.peek() == Some(byte) {
            self.at += 1;
            Ok(())
        } else {
            Err(self.error(&format!("expected {:?}", byte as char)))
        }
    }

    fn literal(&mut self, word: &str, value: Value) -> Result<Value, Error> {
        if self.bytes[self.at..].starts_with(word.as_bytes()) {
            self.at += word.len();
            Ok(value)
        } else {
            Err(self.error("not a valid literal"))
        }
    }

    fn value(&mut self) -> Result<Value, Error> {
        match self.peek().ok_or_else(|| self.error("unexpected end of input"))? {
            b'{' => self.object(),
            b'[' => self.array(),
            b'"' => Ok(Value::String(self.string()?)),
            b't' => self.literal("true", Value::Bool(true)),
            b'f' => self.literal("false", Value::Bool(false)),
            b'n' => self.literal("null", Value::Null),
            b'-' | b'0'..=b'9' => self.number(),
            _ => Err(self.error("not a value")),
        }
    }

    fn object(&mut self) -> Result<Value, Error> {
        self.expect(b'{')?;
        let mut map = BTreeMap::new();
        self.skip_whitespace();
        if self.peek() == Some(b'}') {
            self.at += 1;
            return Ok(Value::Object(map));
        }
        loop {
            self.skip_whitespace();
            let key = self.string()?;
            self.skip_whitespace();
            self.expect(b':')?;
            self.skip_whitespace();
            let value = self.value()?;
            map.insert(key, value);
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b'}') => {
                    self.at += 1;
                    return Ok(Value::Object(map));
                }
                _ => return Err(self.error("expected ',' or '}'")),
            }
        }
    }

    fn array(&mut self) -> Result<Value, Error> {
        self.expect(b'[')?;
        let mut items = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b']') {
            self.at += 1;
            return Ok(Value::Array(items));
        }
        loop {
            self.skip_whitespace();
            items.push(self.value()?);
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b']') => {
                    self.at += 1;
                    return Ok(Value::Array(items));
                }
                _ => return Err(self.error("expected ',' or ']'")),
            }
        }
    }

    fn string(&mut self) -> Result<String, Error> {
        self.expect(b'"')?;
        let mut out = String::new();
        loop {
            let byte = self.peek().ok_or_else(|| self.error("unterminated string"))?;
            self.at += 1;
            match byte {
                b'"' => return Ok(out),
                b'\\' => {
                    let escape = self.peek().ok_or_else(|| self.error("unterminated escape"))?;
                    self.at += 1;
                    match escape {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hex = self
                                .bytes
                                .get(self.at..self.at + 4)
                                .and_then(|s| std::str::from_utf8(s).ok())
                                .and_then(|s| u32::from_str_radix(s, 16).ok())
                                .ok_or_else(|| self.error("bad \\u escape"))?;
                            self.at += 4;
                            out.push(char::from_u32(hex).unwrap_or('\u{fffd}'));
                        }
                        _ => return Err(self.error("unknown escape")),
                    }
                }
                _ => {
                    // Multi-byte UTF-8 passes through a byte at a time; the
                    // input was a &str, so the result stays valid.
                    let start = self.at - 1;
                    let end = self
                        .bytes
                        .iter()
                        .skip(self.at)
                        .position(|&b| b == b'"' || b == b'\\')
                        .map(|n| self.at + n)
                        .unwrap_or(self.bytes.len());
                    out.push_str(std::str::from_utf8(&self.bytes[start..end]).map_err(|_| {
                        Error { at: start, reason: "invalid UTF-8 in string".to_string() }
                    })?);
                    self.at = end;
                }
            }
        }
    }

    fn number(&mut self) -> Result<Value, Error> {
        let start = self.at;
        if self.peek() == Some(b'-') {
            self.at += 1;
        }
        while matches!(self.peek(), Some(b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-')) {
            self.at += 1;
        }
        std::str::from_utf8(&self.bytes[start..self.at])
            .ok()
            .and_then(|text| text.parse().ok())
            .map(Value::Number)
            .ok_or_else(|| self.error("not a number"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_shape_of_ic_offsets_json() {
        let value = parse(
            r#"{
                "IdleGameManager": { "moduleAddress": 7655936, "moduleOffset": [3872], "staticOffset": "" },
                "Import_Version_Major": 657,
                "Import_Version_Minor": "",
                "Platform": "11"
            }"#,
        )
        .expect("parses");

        let root = value.get("IdleGameManager").unwrap();
        assert_eq!(root.get("moduleAddress").unwrap().as_u64(), Some(7655936));
        assert_eq!(root.get("moduleOffset").unwrap().as_u64_vec(), Some(vec![3872]));
        assert_eq!(root.get("staticOffset").unwrap().as_optional_u64(), None);
        assert_eq!(
            value.get("Import_Version_Major").unwrap().as_version_string().as_deref(),
            Some("657")
        );
        assert_eq!(value.get("Platform").unwrap().as_str(), Some("11"));
    }

    #[test]
    fn handles_escapes_and_nesting() {
        let value = parse(r#"{"a":["x\n\"y",{"b":true},null,-2.5]}"#).unwrap();
        let items = value.get("a").unwrap().as_array().unwrap();
        assert_eq!(items[0].as_str(), Some("x\n\"y"));
        assert_eq!(items[1].get("b"), Some(&Value::Bool(true)));
        assert_eq!(items[2], Value::Null);
        assert_eq!(items[3], Value::Number(-2.5));
    }

    #[test]
    fn rejects_malformed_input() {
        for bad in [r#"{"a":}"#, r#"{"a":1,}"#, "[1 2]", r#"{"a":1} junk"#, r#""unterminated"#] {
            assert!(parse(bad).is_err(), "should have failed: {bad}");
        }
    }
}
