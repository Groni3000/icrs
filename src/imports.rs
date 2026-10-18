//! Parser for the generated offset files (`IC_*_Import.ahk`).
//!
//! The game moves its struct offsets on most updates - Briv's SteelBones
//! counter alone moved twice in three months. Hand-transcribing them is how
//! you end up reading a neighbouring field that happens to hold a plausible
//! number, so the offsets are data here, never code: drop in a newer
//! directory from `RLee-EN/BrivMaster-Imports` and nothing is recompiled.
//!
//! The files are generated AutoHotkey with only three line shapes:
//!
//! ```text
//! this.<path> := New GameObjectStructure(this.<parent>,"<Type>", [<offsets>])
//! this.<path>._CollectionKeyType := "<C# type>"      (also ..._CollectionValType)
//! this.<path>._ArrayDimensions := <n>
//! ```
//!
//! Upstream writes `New IBM_GOS(...)` with decimal offsets; IC Script Hub
//! writes `New GameObjectStructure(...)` with hex. Both are accepted.

use std::collections::BTreeMap;
use std::fmt;

/// One offset term. `GameSettings`-style files express offsets relative to a
/// static base that is only known at run time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Offset {
    Fixed(u64),
    /// `this.StaticOffset + n`
    Static(u64),
}

impl Offset {
    pub fn resolve(self, static_offset: u64) -> u64 {
        match self {
            Offset::Fixed(n) => n,
            Offset::Static(n) => static_offset + n,
        }
    }
}

/// One entry: a field, its parent, and how to reach it from that parent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub parent: String,
    pub value_type: String,
    pub offsets: Vec<Offset>,
    pub collection_key_type: Option<String>,
    pub collection_val_type: Option<String>,
    pub array_dimensions: Option<u32>,
}

/// A path resolved all the way back to whichever root it hangs off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// The root this chain starts from, e.g. `IdleGameManager`. The caller
    /// supplies its base pointer; the import files do not contain one.
    pub root: String,
    pub offsets: Vec<Offset>,
    pub value_type: String,
}

impl Resolved {
    pub fn fixed(&self, static_offset: u64) -> Vec<u64> {
        self.offsets.iter().map(|o| o.resolve(static_offset)).collect()
    }
}

#[derive(Debug)]
pub enum Error {
    Syntax { line: usize, reason: String },
    UnknownPath(String),
    CycleAt(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Syntax { line, reason } => write!(f, "line {line}: {reason}"),
            Error::UnknownPath(path) => write!(f, "no such path: {path}"),
            Error::CycleAt(path) => write!(f, "offset chain loops at: {path}"),
        }
    }
}

impl std::error::Error for Error {}

#[derive(Debug, Default, Clone)]
pub struct Imports {
    nodes: BTreeMap<String, Node>,
}

impl Imports {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse one import file, merging into whatever is already loaded, so a
    /// set of files (IdleGameManager, GameSettings, ...) forms one namespace.
    pub fn load(&mut self, source: &str) -> Result<(), Error> {
        for (index, raw) in source.lines().enumerate() {
            let line = strip_comment(raw).trim();
            if line.is_empty() {
                continue;
            }
            let number = index + 1;
            let Some((lhs, rhs)) = split_assignment(line) else {
                continue; // not an assignment: generator banner, stray brace
            };
            let Some(path) = lhs.strip_prefix("this.") else {
                continue;
            };

            if let Some(base) = path.strip_suffix("._CollectionKeyType") {
                self.attribute(base, number, |node, v| node.collection_key_type = Some(v))?
                    .set(unquote(rhs));
            } else if let Some(base) = path.strip_suffix("._CollectionValType") {
                self.attribute(base, number, |node, v| node.collection_val_type = Some(v))?
                    .set(unquote(rhs));
            } else if let Some(base) = path.strip_suffix("._ArrayDimensions") {
                let dims = parse_number(rhs).ok_or_else(|| Error::Syntax {
                    line: number,
                    reason: format!("array dimensions not a number: {rhs}"),
                })?;
                self.node_mut(base, number)?.array_dimensions = Some(dims as u32);
            } else {
                let node = parse_constructor(rhs, number)?;
                self.nodes.insert(path.to_string(), node);
            }
        }
        Ok(())
    }

    pub fn get(&self, path: &str) -> Option<&Node> {
        self.nodes.get(path)
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.nodes.keys().map(String::as_str)
    }

    /// Walk from `path` up through its parents, collecting offsets, until a
    /// parent that has no entry of its own - that is the root.
    pub fn resolve(&self, path: &str) -> Result<Resolved, Error> {
        let leaf = self
            .nodes
            .get(path)
            .ok_or_else(|| Error::UnknownPath(path.to_string()))?;

        let mut offsets = Vec::new();
        let mut current = path;
        // The file is generated, but a hand-edit could introduce a loop; cap
        // the walk rather than hang.
        for _ in 0..self.nodes.len() + 1 {
            let node = match self.nodes.get(current) {
                Some(node) => node,
                None => {
                    offsets.reverse();
                    return Ok(Resolved {
                        root: current.to_string(),
                        offsets,
                        value_type: leaf.value_type.clone(),
                    });
                }
            };
            offsets.extend(node.offsets.iter().rev());
            current = &node.parent;
        }
        Err(Error::CycleAt(path.to_string()))
    }

    fn node_mut(&mut self, path: &str, line: usize) -> Result<&mut Node, Error> {
        self.nodes.get_mut(path).ok_or_else(|| Error::Syntax {
            line,
            reason: format!("attribute set on undeclared path: {path}"),
        })
    }

    fn attribute<'a>(
        &'a mut self,
        path: &str,
        line: usize,
        apply: fn(&mut Node, String),
    ) -> Result<Setter<'a>, Error> {
        let node = self.node_mut(path, line)?;
        Ok(Setter { node, apply })
    }
}

/// Tiny helper so the two `_Collection*Type` arms stay one line each.
pub struct Setter<'a> {
    node: &'a mut Node,
    apply: fn(&mut Node, String),
}

impl Setter<'_> {
    fn set(self, value: String) {
        (self.apply)(self.node, value);
    }
}

fn strip_comment(line: &str) -> &str {
    // Offsets and type names never contain ';', so the first one wins.
    match line.find(';') {
        Some(i) => &line[..i],
        None => line,
    }
}

fn split_assignment(line: &str) -> Option<(&str, &str)> {
    let i = line.find(":=")?;
    Some((line[..i].trim(), line[i + 2..].trim()))
}

fn unquote(value: &str) -> String {
    value.trim().trim_matches('"').to_string()
}

/// `New IBM_GOS(this.parent,"Int",[16, 32])`
fn parse_constructor(rhs: &str, line: usize) -> Result<Node, Error> {
    let syntax = |reason: String| Error::Syntax { line, reason };

    let open = rhs.find('(').ok_or_else(|| syntax(format!("not a constructor: {rhs}")))?;
    let close = rhs.rfind(')').ok_or_else(|| syntax("unterminated argument list".into()))?;
    let ctor = rhs[..open].trim_start_matches("New").trim();
    if ctor != "IBM_GOS" && ctor != "GameObjectStructure" {
        return Err(syntax(format!("unknown constructor: {ctor}")));
    }
    let args = &rhs[open + 1..close];

    // parent, "Type", [offsets]
    let first = args.find(',').ok_or_else(|| syntax("missing value type".into()))?;
    let parent = args[..first]
        .trim()
        .strip_prefix("this.")
        .ok_or_else(|| syntax(format!("parent is not a this.* path: {}", &args[..first])))?
        .to_string();

    let rest = &args[first + 1..];
    let quote_open = rest.find('"').ok_or_else(|| syntax("value type not quoted".into()))?;
    let quote_close = rest[quote_open + 1..]
        .find('"')
        .ok_or_else(|| syntax("value type not terminated".into()))?
        + quote_open
        + 1;
    let value_type = rest[quote_open + 1..quote_close].to_string();

    let bracket_open = rest.find('[').ok_or_else(|| syntax("missing offset list".into()))?;
    let bracket_close = rest.rfind(']').ok_or_else(|| syntax("offset list not closed".into()))?;
    let mut offsets = Vec::new();
    for term in rest[bracket_open + 1..bracket_close].split(',') {
        let term = term.trim();
        if term.is_empty() {
            continue; // `[]` - a node that only re-types its parent
        }
        offsets.push(parse_offset(term).ok_or_else(|| syntax(format!("bad offset: {term}")))?);
    }

    Ok(Node {
        parent,
        value_type,
        offsets,
        collection_key_type: None,
        collection_val_type: None,
        array_dimensions: None,
    })
}

fn parse_offset(term: &str) -> Option<Offset> {
    if let Some(rest) = term.strip_prefix("this.StaticOffset") {
        let rest = rest.trim();
        if rest.is_empty() {
            return Some(Offset::Static(0));
        }
        let addend = rest.strip_prefix('+')?.trim();
        return parse_number(addend).map(Offset::Static);
    }
    parse_number(term).map(Offset::Fixed)
}

fn parse_number(text: &str) -> Option<u64> {
    let text = text.trim();
    match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u64::from_str_radix(hex, 16).ok(),
        None => text.parse().ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
; generated banner
this.game:=New IBM_GOS(this.IdleGameManager,"Int",[224])
this.game.gameInstances:=New IBM_GOS(this.game,"List",[176])
this.game.gameInstances._CollectionValType:="CrusadersGame.ChampionsGameInstance"
this.game.gameInstances.grid := New GameObjectStructure(this.game.gameInstances,"Int", [0x30]) ; hex dialect
this.game.gameInstances.grid._ArrayDimensions := 2
this.UserID:=New IBM_GOS(this.CrusadersGame.GameSettings,"Int",[this.StaticOffset+96])
this.anchor:=New IBM_GOS(this.CrusadersGame.GameSettings,"Int",[this.StaticOffset])
"#;

    fn sample() -> Imports {
        let mut imports = Imports::new();
        imports.load(SAMPLE).expect("sample parses");
        imports
    }

    #[test]
    fn parses_both_dialects() {
        let imports = sample();
        // decimal, IBM_GOS
        assert_eq!(imports.get("game").unwrap().offsets, vec![Offset::Fixed(224)]);
        // hex, GameObjectStructure, trailing comment
        assert_eq!(
            imports.get("game.gameInstances.grid").unwrap().offsets,
            vec![Offset::Fixed(0x30)]
        );
    }

    #[test]
    fn captures_collection_metadata() {
        let imports = sample();
        let list = imports.get("game.gameInstances").unwrap();
        assert_eq!(list.value_type, "List");
        assert_eq!(
            list.collection_val_type.as_deref(),
            Some("CrusadersGame.ChampionsGameInstance")
        );
        assert_eq!(imports.get("game.gameInstances.grid").unwrap().array_dimensions, Some(2));
    }

    #[test]
    fn static_offsets_stay_symbolic_until_resolved() {
        let imports = sample();
        assert_eq!(imports.get("UserID").unwrap().offsets, vec![Offset::Static(96)]);
        assert_eq!(imports.get("anchor").unwrap().offsets, vec![Offset::Static(0)]);
        // The static base is a run-time value, so it is applied at the end.
        assert_eq!(imports.resolve("UserID").unwrap().fixed(0x250), vec![0x250 + 96]);
    }

    #[test]
    fn resolve_walks_to_the_root() {
        let imports = sample();
        let resolved = imports.resolve("game.gameInstances.grid").unwrap();
        assert_eq!(resolved.root, "IdleGameManager");
        assert_eq!(resolved.fixed(0), vec![224, 176, 0x30]);
        assert_eq!(resolved.value_type, "Int");

        let settings = imports.resolve("UserID").unwrap();
        assert_eq!(settings.root, "CrusadersGame.GameSettings");
    }

    #[test]
    fn unknown_path_is_an_error_not_an_empty_chain() {
        let imports = sample();
        assert!(matches!(
            imports.resolve("game.nope"),
            Err(Error::UnknownPath(_))
        ));
    }

    #[test]
    fn attribute_on_undeclared_path_is_rejected() {
        let mut imports = Imports::new();
        let err = imports
            .load("this.ghost._ArrayDimensions := 2")
            .unwrap_err();
        assert!(matches!(err, Error::Syntax { line: 1, .. }), "{err}");
    }

    #[test]
    fn bad_offset_is_rejected() {
        let mut imports = Imports::new();
        let err = imports
            .load(r#"this.x:=New IBM_GOS(this.root,"Int",[lol])"#)
            .unwrap_err();
        assert!(matches!(err, Error::Syntax { .. }), "{err}");
    }
}
