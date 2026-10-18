//! Which build of the game is installed.
//!
//! The game's own version field lives in `GameSettings`, which is one of the
//! roots whose pointer path is stale (see `docs/pointer-chains.md`) - and
//! reading it from memory would be circular anyway, since the offsets to read
//! it with are the ones we are trying to validate. So it comes off disk:
//! Unity writes `bundleVersion` into `<game>_Data/globalgamemanagers`, which
//! needs no offsets at all.

use std::io;
use std::path::{Path, PathBuf};

/// `globalgamemanagers` is a few hundred KB; anything far larger is not it.
const MAX_FILE: u64 = 32 * 1024 * 1024;

/// Strings Unity writes just before the version cluster in PlayerSettings,
/// tried in order, used to anchor the scan.
const ANCHORS: &[&[u8]] = &[b"public.app-category.games", b"Idle Champions"];

/// How far past the anchor the version cluster can reach.
const WINDOW: usize = 1024;

/// Read the installed game version, e.g. `1.657`, given the game's exe path.
pub fn installed(exe_path: &Path) -> io::Result<String> {
    let path = manager_file(exe_path)?;
    let size = std::fs::metadata(&path)?.len();
    if size > MAX_FILE {
        return Err(io::Error::other(format!(
            "{} is {size} bytes - not a Unity manager file",
            path.display()
        )));
    }
    let bytes = std::fs::read(&path)?;
    from_bytes(&bytes).ok_or_else(|| {
        io::Error::other(format!("no version string found in {}", path.display()))
    })
}

/// `.../IdleDragons.exe` -> `.../IdleDragons_Data/globalgamemanagers`
pub fn manager_file(exe_path: &Path) -> io::Result<PathBuf> {
    let dir = exe_path.parent().ok_or_else(|| {
        io::Error::other(format!("{} has no parent directory", exe_path.display()))
    })?;
    let stem = exe_path.file_stem().and_then(|s| s.to_str()).ok_or_else(|| {
        io::Error::other(format!("{} has no file stem", exe_path.display()))
    })?;
    Ok(dir.join(format!("{stem}_Data")).join("globalgamemanagers"))
}

/// Pull the version out of the binary blob.
///
/// Rather than parse Unity's serialisation, scan for printable runs that look
/// like a version and take the highest. Two things make that necessary: the
/// file also holds the Unity editor version (`2022.3.62f2`, rejected because
/// its first component is a year), and PlayerSettings carries per-platform
/// package versions that are left at Unity's default `1.0` - in the real file
/// two of those sit immediately before `bundleVersion`. The game's version
/// only ever increases, so the highest candidate is the one we want.
///
/// The scan is confined to a window after the product name so that an
/// unrelated number elsewhere in the file cannot win.
fn from_bytes(bytes: &[u8]) -> Option<String> {
    let window = ANCHORS
        .iter()
        .filter_map(|anchor| find(bytes, anchor))
        .min()
        .map(|start| &bytes[start..bytes.len().min(start + WINDOW)]);

    highest_version(window.unwrap_or(bytes))
}

fn highest_version(bytes: &[u8]) -> Option<String> {
    ascii_runs(bytes)
        .filter(|run| looks_like_version(run))
        .max_by_key(|run| components(run))
        .map(str::to_string)
}

/// Numeric comparison, so `1.657` beats `1.0` and `1.656.1`.
fn components(text: &str) -> Vec<u32> {
    text.split('.').map(|part| part.parse().unwrap_or(0)).collect()
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

fn ascii_runs(bytes: &[u8]) -> impl Iterator<Item = &str> {
    bytes
        .split(|&b| !b.is_ascii_graphic())
        .filter(|run| run.len() >= 3)
        .filter_map(|run| std::str::from_utf8(run).ok())
}

/// `1.657` and `1.656.1` yes; the Unity version `2022.3.62f2` no.
fn looks_like_version(text: &str) -> bool {
    let parts: Vec<&str> = text.split('.').collect();
    if !(2..=3).contains(&parts.len()) {
        return false;
    }
    if !parts.iter().all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit())) {
        return false;
    }
    // The game's major version is a single digit; Unity's is a year.
    parts[0].len() <= 2
}

/// Strip the game's constant major version so `1.656.1` compares against the
/// feed's `656.1`.
pub fn feed_form(game_version: &str) -> &str {
    game_version.strip_prefix("1.").unwrap_or(game_version)
}

/// Does the loaded offset drop target the installed game?
pub fn matches(game_version: &str, imports_version: &str) -> bool {
    feed_form(game_version) == imports_version
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real layout, as it appears in `globalgamemanagers`: the Unity
    /// editor version early on, then the product name, the app category, two
    /// default `1.0` package versions, and only then `bundleVersion`.
    #[test]
    fn finds_the_bundle_version_past_the_decoys() {
        let mut blob = Vec::new();
        blob.extend_from_slice(b"\x00\x0e2022.3.62f2\x00\x00");
        blob.extend_from_slice(b"Codename Entertainment\x00");
        blob.extend_from_slice(b"Idle Champions\x00");
        blob.extend_from_slice(b"public.app-category.games\x00");
        blob.extend_from_slice(b"\x03\x001.0\x00\x03\x001.0\x00");
        blob.extend_from_slice(b"\x05\x001.657\x00Simple\x00Good\x00");
        assert_eq!(from_bytes(&blob).as_deref(), Some("1.657"));
    }

    #[test]
    fn accepts_a_minor_version() {
        let blob = b"Idle Champions\x00\x001.0\x00\x001.656.1\x00".to_vec();
        assert_eq!(from_bytes(&blob).as_deref(), Some("1.656.1"));
    }

    #[test]
    fn ignores_numbers_far_from_the_product_name() {
        let mut blob = b"9.99\x00".to_vec();
        blob.extend_from_slice(&[0u8; WINDOW]);
        blob.extend_from_slice(b"Idle Champions\x00\x001.657\x00");
        assert_eq!(from_bytes(&blob).as_deref(), Some("1.657"));
    }

    #[test]
    fn version_shapes() {
        assert!(looks_like_version("1.657"));
        assert!(looks_like_version("1.656.1"));
        assert!(!looks_like_version("2022.3.62"));
        assert!(!looks_like_version("2022.3.62f2"));
        assert!(!looks_like_version("1"));
        assert!(!looks_like_version("1."));
        assert!(!looks_like_version("v1.657"));
    }

    #[test]
    fn compares_against_the_feed() {
        assert!(matches("1.657", "657"));
        assert!(matches("1.656.1", "656.1"));
        assert!(!matches("1.658", "657"));
        assert_eq!(feed_form("1.657"), "657");
    }

    #[test]
    fn derives_the_manager_path() {
        let path = manager_file(Path::new("/games/IdleChampions/IdleDragons.exe")).unwrap();
        assert!(path.ends_with("IdleDragons_Data/globalgamemanagers"), "{}", path.display());
    }
}
