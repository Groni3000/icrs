//! Where the values live in the game's memory.
//!
//! Provenance: these mirror IC Script Hub's `CurrentPointers.json` (the base
//! pointer) and `Imports/IC_IdleGameManager64_Import.ahk` (the struct
//! offsets), cross-checked against `RLee-EN/BrivMaster-Imports`
//! `Offsets_P11_V657_A` - platform 11 is Steam, V657 is the current build.
//!
//! They are hand-transcribed for now. The next step is to parse the import
//! files directly so a game update is a data refresh, not a code change.

/// The Mono runtime module every chain is anchored to.
pub const MONO_MODULE: &str = "mono-2.0-bdwgc.dll";

/// `IdleGameManager.moduleAddress`: offset of the root pointer within the
/// module. The chains below start by dereferencing `module_base + this`.
pub const IDLE_GAME_MANAGER: u64 = 0x74D200;

/// Every chain starts the same way: root -> `game` -> `gameInstances` ->
/// element 0. The last two offsets are Mono's `List<T>` layout - `_items`
/// array at `0x10`, first element at `0x20` (element `i` is `0x20 + i * 8`).
/// The game instance we want is always 0.
macro_rules! chain {
    ($label:literal, $($offset:expr),+ $(,)?) => {
        Chain {
            label: $label,
            offsets: &[0xF20, 0xE0, 0xB0, 0x10, 0x20, $($offset),+],
        }
    };
}

/// A pointer chain: dereference each offset in turn, then read the value.
#[derive(Debug, Clone, Copy)]
pub struct Chain {
    pub label: &'static str,
    pub offsets: &'static [u64],
}

/// `gameInstances[0].ActiveCampaignData.currentAreaID`
pub const CURRENT_ZONE: Chain = chain!("zone", 0x28, 0x88);

/// `gameInstances[0].Controller.userData.StatHandler.BrivSteelbonesStacks`
/// (`0x370` on V657 - it was `0x368` on V655 and `0x360` before that).
pub const STEELBONES_STACKS: Chain = chain!("steelbones", 0x18, 0xD8, 0x30, 0x370);

/// `gameInstances[0].Controller.userData.redRubies` - gems.
pub const GEMS: Chain = chain!("gems", 0x18, 0xD8, 0x2C4);

/// Every chain the probe reports, in display order.
pub const ALL: &[Chain] = &[CURRENT_ZONE, STEELBONES_STACKS, GEMS];

#[cfg(test)]
mod tests {
    use super::*;

    /// Guards against a chain written by hand instead of via `chain!`, which
    /// would be anchored somewhere other than `gameInstances[0]` and quietly
    /// produce plausible-looking nonsense.
    #[test]
    fn chains_share_the_game_instance_prefix() {
        const PREFIX: [u64; 5] = [0xF20, 0xE0, 0xB0, 0x10, 0x20];
        for chain in ALL {
            assert!(
                chain.offsets.starts_with(&PREFIX),
                "{} does not start at gameInstances[0]",
                chain.label
            );
            assert!(
                chain.offsets.len() > PREFIX.len(),
                "{} has no offsets of its own",
                chain.label
            );
        }
    }
}
