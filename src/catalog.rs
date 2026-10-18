//! Loading the offset drop: `IC_Offsets.json` plus the generated import files.
//!
//! The point of this module is that a game update should be a file drop, not a
//! rebuild. It finds a drop, parses it, reports which version it targets, and
//! turns a dotted path into a walkable chain. Nothing here knows any offset.
//!
//! Search order, first hit wins:
//!
//! 1. an explicit path (`--imports <dir>`)
//! 2. `$ICRS_IMPORTS`
//! 3. `imports/` next to our own executable
//! 4. the copy vendored into the binary at build time
//!
//! A drop that is present but broken is an error, never a silent fall back to
//! the embedded copy - the whole point is to know what is being read.

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use crate::imports::{self, Imports};
use crate::json;
use crate::offsets::Kind;

pub const OFFSETS_FILE: &str = "IC_Offsets.json";

/// The generated import files, in load order. The overlay is last so it wins;
/// it is optional, the rest are not.
pub const IMPORT_FILES: &[(&str, bool)] = &[
    ("IC_IdleGameManager_Import.ahk", true),
    ("IC_GameSettings_Import.ahk", true),
    ("IC_EngineSettings_Import.ahk", true),
    ("extra.ahk", false),
];

/// The drop vendored into the binary, used when no directory is found.
const EMBEDDED: &[(&str, &str)] = &[
    (OFFSETS_FILE, include_str!("../data/imports/IC_Offsets.json")),
    (
        "IC_IdleGameManager_Import.ahk",
        include_str!("../data/imports/IC_IdleGameManager_Import.ahk"),
    ),
    (
        "IC_GameSettings_Import.ahk",
        include_str!("../data/imports/IC_GameSettings_Import.ahk"),
    ),
    (
        "IC_EngineSettings_Import.ahk",
        include_str!("../data/imports/IC_EngineSettings_Import.ahk"),
    ),
    ("extra.ahk", include_str!("../data/imports/extra.ahk")),
];

/// One top-level object, straight out of `IC_Offsets.json`.
#[derive(Debug, Clone)]
pub struct Root {
    pub name: String,
    pub module_address: u64,
    pub module_offset: Vec<u64>,
    /// Absent for `IdleGameManager`; the other roots write their field offsets
    /// relative to it.
    pub static_offset: u64,
}

/// A chain ready to walk: `base(module_base)`, then `offsets`.
#[derive(Debug, Clone)]
pub struct Chain {
    pub label: String,
    pub path: String,
    pub root: Root,
    pub offsets: Vec<u64>,
    pub kind: Kind,
}

impl Chain {
    pub fn base(&self, module_base: u64) -> u64 {
        module_base + self.root.module_address
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Embedded,
    Directory(PathBuf),
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::Embedded => write!(f, "built-in"),
            Source::Directory(path) => write!(f, "{}", path.display()),
        }
    }
}

#[derive(Debug)]
pub enum Error {
    Read { file: PathBuf, err: io::Error },
    Json { file: String, err: json::Error },
    Imports { file: String, err: imports::Error },
    Missing { file: String, field: String },
    UnknownRoot { root: String, known: Vec<String> },
    UnknownPath { path: String, err: imports::Error },
    UnreadableType { path: String, value_type: String },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Read { file, err } => write!(f, "cannot read {}: {err}", file.display()),
            Error::Json { file, err } => write!(f, "{file}: {err}"),
            Error::Imports { file, err } => write!(f, "{file}: {err}"),
            Error::Missing { file, field } => write!(f, "{file}: missing or malformed {field}"),
            Error::UnknownRoot { root, known } => write!(
                f,
                "the import files hang this path off {root}, which {OFFSETS_FILE} does not \
                 describe (it has: {})",
                known.join(", ")
            ),
            Error::UnknownPath { path, err } => write!(f, "{path}: {err}"),
            Error::UnreadableType { path, value_type } => {
                write!(f, "{path} is declared {value_type}, which has no reader yet")
            }
        }
    }
}

impl std::error::Error for Error {}

/// A parsed offset drop.
#[derive(Debug)]
pub struct Catalog {
    pub source: Source,
    pub platform: Option<String>,
    pub import_version: Option<String>,
    pub pointer_version: Option<String>,
    /// Upstream paths an overlay replaced - should be empty.
    pub overridden: Vec<String>,
    roots: BTreeMap<String, Root>,
    imports: Imports,
}

impl Catalog {
    /// Find and load a drop. `explicit` comes from the command line.
    pub fn discover(explicit: Option<&Path>) -> Result<Self, Error> {
        if let Some(dir) = explicit {
            return Self::from_dir(dir);
        }
        if let Some(dir) = std::env::var_os("ICRS_IMPORTS") {
            return Self::from_dir(Path::new(&dir));
        }
        if let Some(dir) = beside_executable() {
            if dir.join(OFFSETS_FILE).is_file() {
                return Self::from_dir(&dir);
            }
        }
        Self::embedded()
    }

    pub fn from_dir(dir: &Path) -> Result<Self, Error> {
        let read = |name: &str| -> Result<String, Error> {
            let file = dir.join(name);
            std::fs::read_to_string(&file).map_err(|err| Error::Read { file, err })
        };

        let offsets = read(OFFSETS_FILE)?;
        let mut files = Vec::new();
        for (name, required) in IMPORT_FILES {
            match read(name) {
                Ok(text) => files.push(((*name).to_string(), text)),
                Err(err) if *required => return Err(err),
                Err(_) => {}
            }
        }
        Self::build(Source::Directory(dir.to_path_buf()), &offsets, &files)
    }

    pub fn embedded() -> Result<Self, Error> {
        let offsets = EMBEDDED
            .iter()
            .find(|(name, _)| *name == OFFSETS_FILE)
            .map(|(_, text)| *text)
            .expect("the embedded drop always contains IC_Offsets.json");
        let files: Vec<(String, String)> = EMBEDDED
            .iter()
            .filter(|(name, _)| *name != OFFSETS_FILE)
            .map(|(name, text)| ((*name).to_string(), (*text).to_string()))
            .collect();
        Self::build(Source::Embedded, offsets, &files)
    }

    fn build(source: Source, offsets: &str, files: &[(String, String)]) -> Result<Self, Error> {
        let root_data = json::parse(offsets).map_err(|err| Error::Json {
            file: OFFSETS_FILE.to_string(),
            err,
        })?;

        let mut roots = BTreeMap::new();
        for (name, value) in root_entries(&root_data) {
            let missing = |field: &str| Error::Missing {
                file: OFFSETS_FILE.to_string(),
                field: format!("{name}.{field}"),
            };
            let module_address = value
                .get("moduleAddress")
                .and_then(json::Value::as_u64)
                .ok_or_else(|| missing("moduleAddress"))?;
            let module_offset = value
                .get("moduleOffset")
                .and_then(json::Value::as_u64_vec)
                .ok_or_else(|| missing("moduleOffset"))?;
            let static_offset = value
                .get("staticOffset")
                .and_then(json::Value::as_optional_u64)
                .unwrap_or(0);
            roots.insert(
                name.clone(),
                Root { name, module_address, module_offset, static_offset },
            );
        }

        let mut imports = Imports::new();
        let mut overridden = Vec::new();
        for (name, text) in files {
            let load = imports.load(text).map_err(|err| Error::Imports {
                file: name.clone(),
                err,
            })?;
            overridden.extend(load.replaced);
        }

        let version = |key: &str| {
            root_data.get(key).and_then(json::Value::as_version_string)
        };
        let import_version = join_version(version("Import_Version_Major"), version("Import_Version_Minor"));
        let pointer_version = join_version(version("Pointer_Version_Major"), version("Pointer_Version_Minor"));

        Ok(Self {
            source,
            platform: root_data.get("Platform").and_then(json::Value::as_version_string),
            import_version,
            pointer_version,
            overridden,
            roots,
            imports,
        })
    }

    pub fn roots(&self) -> impl Iterator<Item = &Root> {
        self.roots.values()
    }

    pub fn paths(&self) -> usize {
        self.imports.len()
    }

    /// Paths containing `filter` (case-insensitive) with their declared type,
    /// so the drop can be explored without the game running.
    pub fn search(&self, filter: Option<&str>) -> Vec<(&str, &str)> {
        let needle = filter.map(str::to_lowercase);
        self.imports
            .paths()
            .filter(|path| match &needle {
                Some(needle) => path.to_lowercase().contains(needle),
                None => true,
            })
            .filter_map(|path| {
                self.imports.get(path).map(|node| (path, node.value_type.as_str()))
            })
            .collect()
    }

    /// Turn a dotted path into a chain, indexing collections with `indices`
    /// (outermost first, 0 where omitted).
    pub fn chain(&self, label: &str, path: &str, indices: &[u64]) -> Result<Chain, Error> {
        if let Some(parent) = path.strip_suffix(".size") {
            if self.imports.get(path).is_none() {
                return self.collection_size(label, parent, indices);
            }
        }

        let resolved = self
            .imports
            .resolve_at(path, indices)
            .map_err(|err| Error::UnknownPath { path: path.to_string(), err })?;

        let root = self.root_for(&resolved.root)?;
        let kind = Kind::from_value_type(&resolved.value_type).ok_or_else(|| {
            Error::UnreadableType {
                path: path.to_string(),
                value_type: resolved.value_type.clone(),
            }
        })?;

        let mut offsets = root.module_offset.clone();
        offsets.extend(resolved.fixed(root.static_offset));
        Ok(Chain {
            label: label.to_string(),
            path: path.to_string(),
            root: root.clone(),
            offsets,
            kind,
        })
    }

    /// `<collection>.size`: how many elements it holds.
    ///
    /// The import files never declare this - it is Mono's own layout, `_size`
    /// at `0x18` for a `List<T>` and `count` at `0x40` for a
    /// `Dictionary<K,V>` - but Script Hub spells it `.size` and so do we,
    /// because every loop over a collection needs it.
    fn collection_size(&self, label: &str, parent: &str, indices: &[u64]) -> Result<Chain, Error> {
        let resolved = self
            .imports
            .resolve_at(parent, indices)
            .map_err(|err| Error::UnknownPath { path: parent.to_string(), err })?;
        let size_offset = match resolved.value_type.as_str() {
            "List" => 0x18,
            "Dict" => 0x40,
            other => {
                return Err(Error::UnreadableType {
                    path: parent.to_string(),
                    value_type: format!("{other} - .size is defined for List and Dict"),
                });
            }
        };

        let root = self.root_for(&resolved.root)?;
        let mut offsets = root.module_offset.clone();
        offsets.extend(resolved.fixed(root.static_offset));
        offsets.push(size_offset);
        Ok(Chain {
            label: label.to_string(),
            path: format!("{parent}.size"),
            root: root.clone(),
            offsets,
            kind: Kind::I32,
        })
    }

    /// `CrusadersGame.GameSettings` in the import files is `GameSettings` in
    /// `IC_Offsets.json`, so match on the last segment.
    fn root_for(&self, import_root: &str) -> Result<&Root, Error> {
        let key = import_root.rsplit('.').next().unwrap_or(import_root);
        self.roots.get(key).ok_or_else(|| Error::UnknownRoot {
            root: import_root.to_string(),
            known: self.roots.keys().cloned().collect(),
        })
    }
}

/// The root objects are the object-valued members of the top level; the rest
/// of the file is version metadata.
fn root_entries(data: &json::Value) -> Vec<(String, &json::Value)> {
    match data {
        json::Value::Object(map) => map
            .iter()
            .filter(|(_, value)| matches!(value, json::Value::Object(_)))
            .map(|(name, value)| (name.clone(), value))
            .collect(),
        _ => Vec::new(),
    }
}

fn join_version(major: Option<String>, minor: Option<String>) -> Option<String> {
    match (major, minor) {
        (Some(major), Some(minor)) => Some(format!("{major}.{minor}")),
        (Some(major), None) => Some(major),
        _ => None,
    }
}

fn beside_executable() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join("imports"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_drop_loads() {
        let catalog = Catalog::embedded().expect("embedded drop is valid");
        assert_eq!(catalog.source, Source::Embedded);
        assert_eq!(catalog.platform.as_deref(), Some("11"));
        assert_eq!(catalog.import_version.as_deref(), Some("657"));
        assert_eq!(catalog.pointer_version.as_deref(), Some("654"));
        assert!(catalog.paths() > 100, "only {} paths parsed", catalog.paths());
    }

    #[test]
    fn the_overlay_does_not_shadow_upstream() {
        let catalog = Catalog::embedded().unwrap();
        assert!(
            catalog.overridden.is_empty(),
            "extra.ahk now replaces upstream paths, drop them from it: {:?}",
            catalog.overridden
        );
    }

    #[test]
    fn all_three_roots_are_described() {
        let catalog = Catalog::embedded().unwrap();
        let names: Vec<&str> = catalog.roots().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["EngineSettings", "GameSettings", "IdleGameManager"]);
        let manager = catalog.root_for("IdleGameManager").unwrap();
        assert_eq!(manager.module_address, 0x74D200);
        assert_eq!(manager.module_offset, [0xF20]);
        assert_eq!(manager.static_offset, 0);
    }

    #[test]
    fn builds_the_verified_chains_from_data_alone() {
        let catalog = Catalog::embedded().unwrap();
        for want in crate::offsets::NAMED {
            catalog
                .chain(want.label, want.path, want.indices)
                .unwrap_or_else(|err| panic!("{}: {err}", want.label));
        }

        let zone = catalog.chain("zone", "game.gameInstances.ActiveCampaignData.currentAreaID", &[]).unwrap();
        assert_eq!(zone.root.name, "IdleGameManager");
        assert_eq!(zone.offsets, crate::offsets::CURRENT_ZONE.full());
        assert_eq!(zone.kind, Kind::I32);
    }

    #[test]
    fn static_offsets_are_applied_from_the_json() {
        let catalog = Catalog::embedded().unwrap();
        let chain = catalog.chain("instance_id", "_instance.instanceID", &[]).unwrap();
        assert_eq!(chain.root.name, "GameSettings");
        // moduleOffset, then staticOffset (0x250), then instanceID at 0x18.
        assert_eq!(chain.offsets, crate::offsets::INSTANCE_ID.full());
        assert_eq!(chain.kind, Kind::I64);
    }

    #[test]
    fn a_missing_directory_is_an_error_not_a_fallback() {
        let err = Catalog::from_dir(Path::new("/nonexistent/imports")).unwrap_err();
        assert!(matches!(err, Error::Read { .. }), "{err}");
    }
}
