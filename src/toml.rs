//! A minimal TOML reader, enough for a profile.
//!
//! Profiles are hand-written, flat, and small: a few top-level keys and a few
//! `[section]`s of numbers, booleans, strings and string lists. That is a
//! hundred lines of parsing, which is cheaper than a dependency and - more to
//! the point - lets every mistake in the file come back as a line number
//! instead of being silently ignored, which is how the AHK config behaved.
//!
//! Not supported, on purpose: nested tables, arrays of tables, multi-line
//! anything, dates. Anything unrecognised is an error rather than a shrug.

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i64),
    Float(f64),
    Bool(bool),
    Text(String),
    List(Vec<Value>),
}

impl Value {
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Int(_) => "integer",
            Value::Float(_) => "float",
            Value::Bool(_) => "boolean",
            Value::Text(_) => "string",
            Value::List(_) => "list",
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Int(value) => write!(f, "{value}"),
            Value::Float(value) => write!(f, "{value}"),
            Value::Bool(value) => write!(f, "{value}"),
            Value::Text(value) => write!(f, "{value:?}"),
            Value::List(items) => {
                let shown: Vec<String> = items.iter().map(|item| item.to_string()).collect();
                write!(f, "[{}]", shown.join(", "))
            }
        }
    }
}

#[derive(Debug)]
pub struct Error {
    pub line: usize,
    pub reason: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.reason)
    }
}

impl std::error::Error for Error {}

/// One key, with the section it was found in and the line it came from, so a
/// complaint about it can point at the file.
#[derive(Debug, Clone)]
pub struct Entry {
    /// The empty string for keys before any `[section]` header.
    pub section: String,
    pub key: String,
    pub value: Value,
    pub line: usize,
}

#[derive(Debug, Default)]
pub struct Document {
    pub entries: Vec<Entry>,
    /// Section headers in the order they appeared. Kept apart from the
    /// entries so that a header with no keys under it still counts as
    /// present: `[deck]` on its own means "watch the deck, with the
    /// defaults", not "no deck".
    pub sections: Vec<String>,
}

impl Document {
    pub fn get(&self, section: &str, key: &str) -> Option<&Entry> {
        self.entries
            .iter()
            .find(|entry| entry.section == section && entry.key == key)
    }

    pub fn has_section(&self, section: &str) -> bool {
        self.sections.iter().any(|name| name == section)
    }

    /// Every key present, as `section.key` (or just `key` at the top level).
    pub fn dotted_keys(&self) -> Vec<String> {
        self.entries.iter().map(|entry| entry.dotted()).collect()
    }
}

impl Entry {
    pub fn dotted(&self) -> String {
        if self.section.is_empty() {
            self.key.clone()
        } else {
            format!("{}.{}", self.section, self.key)
        }
    }

    fn wrong_type(&self, wanted: &str) -> Error {
        Error {
            line: self.line,
            reason: format!("{} should be {wanted}, not {}", self.dotted(), self.value.type_name()),
        }
    }

    pub fn as_int(&self) -> Result<i64, Error> {
        match self.value {
            Value::Int(value) => Ok(value),
            _ => Err(self.wrong_type("a whole number")),
        }
    }

    pub fn as_bool(&self) -> Result<bool, Error> {
        match self.value {
            Value::Bool(value) => Ok(value),
            _ => Err(self.wrong_type("true or false")),
        }
    }

    pub fn as_text(&self) -> Result<&str, Error> {
        match &self.value {
            Value::Text(value) => Ok(value),
            _ => Err(self.wrong_type("a quoted string")),
        }
    }

    /// A list, whose items are left as values: a profile may legitimately
    /// accept either a champion id or a champion name in the same list.
    pub fn as_list(&self) -> Result<&[Value], Error> {
        match &self.value {
            Value::List(items) => Ok(items),
            _ => Err(self.wrong_type("a list in [brackets]")),
        }
    }

    pub fn error(&self, reason: impl Into<String>) -> Error {
        Error { line: self.line, reason: reason.into() }
    }
}

pub fn parse(text: &str) -> Result<Document, Error> {
    let mut document = Document::default();
    let mut section = String::new();

    for (index, raw) in text.lines().enumerate() {
        let line = index + 1;
        let trimmed = strip_comment(raw.trim(), line)?;
        if trimmed.is_empty() {
            continue;
        }

        if let Some(header) = trimmed.strip_prefix('[') {
            let Some(name) = header.strip_suffix(']') else {
                return Err(Error { line, reason: "section header is missing its ]".into() });
            };
            let name = name.trim();
            if name.is_empty() || name.contains('[') {
                return Err(Error {
                    line,
                    reason: format!("{trimmed:?} is not a section name (arrays of tables are not supported)"),
                });
            }
            section = name.to_string();
            if !document.sections.contains(&section) {
                document.sections.push(section.clone());
            }
            continue;
        }

        let Some((key, value)) = trimmed.split_once('=') else {
            return Err(Error {
                line,
                reason: format!("{trimmed:?} is neither a [section] nor a key = value"),
            });
        };
        let key = key.trim().to_string();
        if key.is_empty() {
            return Err(Error { line, reason: "a value with no key".into() });
        }
        if document.get(&section, &key).is_some() {
            return Err(Error { line, reason: format!("{key} is set twice in this section") });
        }
        let value = parse_value(value.trim(), line)?;
        document.entries.push(Entry { section: section.clone(), key, value, line });
    }

    Ok(document)
}

/// Drop a trailing `# comment`, leaving `#` inside quotes alone.
fn strip_comment(line: &str, number: usize) -> Result<&str, Error> {
    let mut quote: Option<char> = None;
    for (at, character) in line.char_indices() {
        match (quote, character) {
            (None, '"' | '\'') => quote = Some(character),
            (Some(open), _) if character == open => quote = None,
            (None, '#') => return Ok(line[..at].trim_end()),
            _ => {}
        }
    }
    if quote.is_some() {
        return Err(Error { line: number, reason: "a quoted string is never closed".into() });
    }
    Ok(line)
}

fn parse_value(text: &str, line: usize) -> Result<Value, Error> {
    let fail = |reason: String| Error { line, reason };

    if text.is_empty() {
        return Err(fail("a key with no value".into()));
    }
    if let Some(inner) = text.strip_prefix('[') {
        let Some(inner) = inner.strip_suffix(']') else {
            return Err(fail("a list is missing its ]".into()));
        };
        let mut items = Vec::new();
        for part in split_items(inner) {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            items.push(parse_value(part, line)?);
        }
        return Ok(Value::List(items));
    }
    if text.len() >= 2 {
        let bytes = text.as_bytes();
        let first = bytes[0] as char;
        if (first == '"' || first == '\'') && bytes[bytes.len() - 1] as char == first {
            return Ok(Value::Text(text[1..text.len() - 1].to_string()));
        }
    }
    match text {
        "true" => return Ok(Value::Bool(true)),
        "false" => return Ok(Value::Bool(false)),
        _ => {}
    }
    if let Ok(value) = text.parse::<i64>() {
        return Ok(Value::Int(value));
    }
    if let Ok(value) = text.parse::<f64>() {
        return Ok(Value::Float(value));
    }
    Err(fail(format!(
        "{text:?} is not a number, true/false, a quoted string, or a [list]"
    )))
}

/// Split a list body on commas that are not inside quotes.
fn split_items(inner: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut quote: Option<char> = None;
    let mut start = 0;
    for (at, character) in inner.char_indices() {
        match (quote, character) {
            (None, '"' | '\'') => quote = Some(character),
            (Some(open), _) if character == open => quote = None,
            (None, ',') => {
                parts.push(&inner[start..at]);
                start = at + 1;
            }
            _ => {}
        }
    }
    parts.push(&inner[start..]);
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
# a profile
name = "gem farm"      # inline comment
tick_ms = 250

[deck]
champion = "ellywick"
fire_at_gems = 1
hold_until_moons = 0

[ultimates]
fire_when_ready = ["briv", 83]
recast_ms = 3000

[guards]
skip_while_transitioning = true
"#;

    #[test]
    fn reads_sections_keys_and_types() {
        let document = parse(SAMPLE).unwrap();
        assert_eq!(document.get("", "name").unwrap().as_text().unwrap(), "gem farm");
        assert_eq!(document.get("", "tick_ms").unwrap().as_int().unwrap(), 250);
        assert_eq!(document.get("deck", "fire_at_gems").unwrap().as_int().unwrap(), 1);
        assert!(document.get("guards", "skip_while_transitioning").unwrap().as_bool().unwrap());
        assert_eq!(
            document.get("ultimates", "fire_when_ready").unwrap().as_list().unwrap(),
            &[Value::Text("briv".into()), Value::Int(83)]
        );
        assert!(document.has_section("deck"));
        assert!(!document.has_section("decks"));
    }

    #[test]
    fn a_key_knows_where_it_came_from() {
        let document = parse(SAMPLE).unwrap();
        let entry = document.get("deck", "champion").unwrap();
        assert_eq!(entry.dotted(), "deck.champion");
        assert_eq!(entry.line, 7);
        // Asking for the wrong type complains with that line number, because
        // "it did nothing" is the worst answer a config file can give.
        let err = entry.as_int().unwrap_err();
        assert_eq!(err.line, 7);
        assert!(err.to_string().contains("deck.champion should be a whole number"), "{err}");
    }

    #[test]
    fn a_section_with_no_keys_is_still_a_section() {
        let document = parse("[deck]\n# nothing but a comment").unwrap();
        assert!(document.has_section("deck"));
        assert!(document.entries.is_empty());
    }

    #[test]
    fn a_hash_inside_quotes_is_not_a_comment() {
        let document = parse("name = \"farm #1\"").unwrap();
        assert_eq!(document.get("", "name").unwrap().as_text().unwrap(), "farm #1");
    }

    #[test]
    fn the_same_key_twice_is_an_error() {
        let err = parse("[deck]\nfire_at_gems = 1\nfire_at_gems = 2").unwrap_err();
        assert_eq!(err.line, 3);
        assert!(err.to_string().contains("set twice"), "{err}");
    }

    #[test]
    fn the_same_key_in_two_sections_is_fine() {
        let document = parse("[a]\nx = 1\n[b]\nx = 2").unwrap();
        assert_eq!(document.get("a", "x").unwrap().as_int().unwrap(), 1);
        assert_eq!(document.get("b", "x").unwrap().as_int().unwrap(), 2);
    }

    #[test]
    fn malformed_lines_say_which_line() {
        for (text, line) in [
            ("name = \"unclosed", 1),
            ("[deck\nx = 1", 1),
            ("just some words", 1),
            ("name = \"ok\"\nfire_at_gems =", 2),
            ("name = \"ok\"\nlist = [1, 2", 2),
            ("name = \"ok\"\nvalue = yes", 2),
        ] {
            let err = parse(text).unwrap_err();
            assert_eq!(err.line, line, "{text:?} -> {err}");
        }
    }

    #[test]
    fn floats_and_negatives_survive() {
        let document = parse("a = -3\nb = 1.5").unwrap();
        assert_eq!(document.get("", "a").unwrap().value, Value::Int(-3));
        assert_eq!(document.get("", "b").unwrap().value, Value::Float(1.5));
    }
}
