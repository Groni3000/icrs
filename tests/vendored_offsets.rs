//! The parser, run against the real upstream offset file, must reproduce the
//! chains that were verified by reading the live game.
//!
//! This is the test that makes the offsets safe to treat as data: if a future
//! `BrivMaster-Imports` drop moves something, this fails loudly instead of the
//! program reading a neighbouring field that happens to hold a plausible
//! number.

use icrs::imports::Imports;
use icrs::offsets;

const IDLE_GAME_MANAGER: &str = include_str!("../data/imports/IC_IdleGameManager_Import.ahk");

fn vendored() -> Imports {
    let mut imports = Imports::new();
    imports.load(IDLE_GAME_MANAGER).expect("vendored import file parses");
    imports
}

/// Build a full chain the way a reader must: the root's own module offset,
/// then the parsed struct offsets, with Mono's `List<T>` indexing spliced in
/// where the path crosses a list (`_items` at 0x10, element `i` at 0x20 + 8i).
fn chain_to(imports: &Imports, list_path: &str, leaf_path: &str, index: u64) -> Vec<u64> {
    let list = imports.resolve(list_path).unwrap().fixed(0);
    let leaf = imports.resolve(leaf_path).unwrap().fixed(0);
    let (prefix, tail) = leaf.split_at(list.len());
    assert_eq!(prefix, list.as_slice(), "{leaf_path} does not pass through {list_path}");

    // The root's own hops, then the parsed field offsets with List indexing.
    let mut out = offsets::IDLE_GAME_MANAGER.module_offset.to_vec();
    out.extend_from_slice(&list);
    out.extend_from_slice(&[0x10, 0x20 + index * 8]);
    out.extend_from_slice(tail);
    out
}

#[test]
fn file_is_substantial() {
    // Guard against a truncated or failed download silently parsing to nothing.
    assert!(vendored().len() > 100, "suspiciously few entries parsed");
}

#[test]
fn reproduces_the_verified_chains() {
    let imports = vendored();
    const LIST: &str = "game.gameInstances";

    for (leaf, expected) in [
        (
            "game.gameInstances.ActiveCampaignData.currentAreaID",
            offsets::CURRENT_ZONE,
        ),
        (
            "game.gameInstances.Controller.userData.StatHandler.BrivSteelbonesStacks",
            offsets::STEELBONES_STACKS,
        ),
        (
            "game.gameInstances.Controller.userData.redRubies",
            offsets::GEMS,
        ),
    ] {
        assert_eq!(
            chain_to(&imports, LIST, leaf, 0),
            expected.full(),
            "{} ({leaf})",
            expected.label
        );
    }
}

#[test]
fn value_types_match_what_we_read() {
    let imports = vendored();
    for path in [
        "game.gameInstances.ActiveCampaignData.currentAreaID",
        "game.gameInstances.Controller.userData.StatHandler.BrivSteelbonesStacks",
        "game.gameInstances.Controller.userData.redRubies",
    ] {
        assert_eq!(imports.resolve(path).unwrap().value_type, "Int", "{path}");
    }
}
