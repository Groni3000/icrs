//! Where the values live in the game's memory.
//!
//! Provenance: `data/imports/`, a vendored drop of `RLee-EN/BrivMaster-Imports`
//! `Offsets_P11_V657_A` - platform 11 is Steam, V657 the current build. The
//! roots below are `IC_Offsets.json` verbatim; the struct offsets are from the
//! generated import files, which `crate::imports` parses and which
//! `tests/vendored_offsets.rs` checks these constants against.
//!
//! See `docs/pointer-chains.md` for what a chain is and how it goes stale.

/// The Mono runtime module every chain is anchored to.
pub const MONO_MODULE: &str = "mono-2.0-bdwgc.dll";

/// A top-level object the import files hang fields off. There is more than one,
/// and chains rooted in different ones share nothing - a dead chain in one root
/// says nothing about the others.
#[derive(Debug, Clone, Copy)]
pub struct Root {
    pub name: &'static str,
    /// `moduleAddress`: static address within `MONO_MODULE`.
    pub module_address: u64,
    /// `moduleOffset`: the hops from that static address to the object itself.
    pub module_offset: &'static [u64],
}

/// Everything the farm loop reads.
pub const IDLE_GAME_MANAGER: Root = Root {
    name: "IdleGameManager",
    module_address: 0x74D200,
    module_offset: &[0xF20],
};

/// Identity and server details: `instance_id`, `UserID`, `Hash`.
pub const GAME_SETTINGS: Root = Root {
    name: "CrusadersGame.GameSettings",
    module_address: 0xA13548,
    module_offset: &[0x160, 0x868, 0xA0, 0x1B0],
};

/// `WebRoot` - which server the web API lives on.
pub const ENGINE_SETTINGS: Root = Root {
    name: "UnityGameEngine.Core.EngineSettings",
    module_address: 0x74A0F8,
    module_offset: &[0x2A0, 0xA80, 0xB0, 0x498, 0x40],
};

/// `IC_Offsets.json` -> `staticOffset`. Fields on these two roots are written
/// `this.StaticOffset + n` in the import files, so the base is added here.
/// `IdleGameManager` has no static offset.
const GAME_SETTINGS_STATIC: u64 = 0x250;
const ENGINE_SETTINGS_STATIC: u64 = 0x50;

/// How to read the value once the chain lands on its field. These mirror the
/// type names the import files declare.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    I32,
    I64,
    /// `Char` in the import files: a one-byte flag, not a character.
    U8,
    F32,
    F64,
    /// A Mono string: the field holds a pointer to the string object.
    Utf16,
}

impl Kind {
    /// Map an import file's declared type onto a reader.
    pub fn from_value_type(value_type: &str) -> Option<Self> {
        match value_type {
            "Int" => Some(Kind::I32),
            "Int64" | "Quad" => Some(Kind::I64),
            "Char" => Some(Kind::U8),
            "Float" => Some(Kind::F32),
            "Double" => Some(Kind::F64),
            "UTF-16" => Some(Kind::Utf16),
            _ => None,
        }
    }
}

/// A pointer chain: dereference each offset in turn, then read the value.
#[derive(Debug, Clone, Copy)]
pub struct Chain {
    pub label: &'static str,
    pub root: Root,
    /// Offsets from the root object - `root.module_offset` is not repeated here.
    pub offsets: &'static [u64],
    pub kind: Kind,
}

impl Chain {
    /// Where the walk starts, given this launch's module base.
    pub fn base(&self, module_base: u64) -> u64 {
        module_base + self.root.module_address
    }

    /// The whole walk: the root's own hops, then the field offsets.
    pub fn full(&self) -> Vec<u64> {
        [self.root.module_offset, self.offsets].concat()
    }
}

/// Chains through `gameInstances[0]`, which is the shared prefix of every value
/// the farm loop cares about. `0x10`/`0x20` are Mono's `List<T>` layout -
/// `_items` array, then element 0 (element `i` is at `0x20 + i * 8`).
macro_rules! game_chain {
    ($label:literal, $kind:ident, $($offset:expr),+ $(,)?) => {
        Chain {
            label: $label,
            root: IDLE_GAME_MANAGER,
            offsets: &[0xE0, 0xB0, 0x10, 0x20, $($offset),+],
            kind: Kind::$kind,
        }
    };
}

/// `gameInstances[0].ActiveCampaignData.currentAreaID`
pub const CURRENT_ZONE: Chain = game_chain!("zone", I32, 0x28, 0x88);

/// `gameInstances[0].Controller.userData.StatHandler.BrivSteelbonesStacks`
/// (`0x370` on V657 - it was `0x368` on V655 and `0x360` before that).
pub const STEELBONES_STACKS: Chain = game_chain!("steelbones", I32, 0x18, 0xD8, 0x30, 0x370);

/// `gameInstances[0].Controller.userData.redRubies` - gems.
pub const GEMS: Chain = game_chain!("gems", I32, 0x18, 0xD8, 0x2C4);

/// `_instance.instanceID` - needed by the web API, nothing else.
pub const INSTANCE_ID: Chain = Chain {
    label: "instance_id",
    root: GAME_SETTINGS,
    offsets: &[GAME_SETTINGS_STATIC, 0x18],
    kind: Kind::I64,
};

pub const USER_ID: Chain = Chain {
    label: "user_id",
    root: GAME_SETTINGS,
    offsets: &[GAME_SETTINGS_STATIC + 96],
    kind: Kind::I32,
};

pub const PLATFORM: Chain = Chain {
    label: "platform",
    root: GAME_SETTINGS,
    offsets: &[GAME_SETTINGS_STATIC + 208],
    kind: Kind::I32,
};

pub const HASH: Chain = Chain {
    label: "hash",
    root: GAME_SETTINGS,
    offsets: &[GAME_SETTINGS_STATIC + 104],
    kind: Kind::Utf16,
};

pub const WEB_ROOT: Chain = Chain {
    label: "web_root",
    root: ENGINE_SETTINGS,
    offsets: &[ENGINE_SETTINGS_STATIC + 16],
    kind: Kind::Utf16,
};

/// A value to read, named by path rather than by offset. The chain is built
/// from the loaded drop at run time; the constants above exist only as the
/// hand-verified anchors the parser is tested against.
#[derive(Debug, Clone, Copy)]
pub struct Want {
    pub label: &'static str,
    pub path: &'static str,
    /// One index per collection the path crosses, outermost first. Empty means
    /// element 0 everywhere, which is the game instance we always use.
    pub indices: &'static [u64],
    /// Inclusive bounds a healthy read must fall inside. A value outside them
    /// means the chain landed somewhere plausible-looking but wrong, which no
    /// version check can catch.
    pub sane: Option<(i64, i64)>,
}

/// What the probe reads, in display order.
pub const WANTED: &[Want] = &[
    Want {
        label: "zone",
        path: "game.gameInstances.ActiveCampaignData.currentAreaID",
        indices: &[],
        // -1 is the world map, which the AHK script treats as a real state.
        sane: Some((-1, 100_000)),
    },
    Want {
        label: "steelbones",
        path: "game.gameInstances.Controller.userData.StatHandler.BrivSteelbonesStacks",
        indices: &[],
        sane: Some((0, 10_000_000)),
    },
    Want {
        label: "gems",
        path: "game.gameInstances.Controller.userData.redRubies",
        indices: &[],
        sane: Some((0, 1_000_000_000)),
    },
    // Officially these two live on GameSettings, whose pointer path is stale;
    // this is the route the AHK script falls back to.
    Want { label: "user_id", path: "game.gameUser.ID", indices: &[], sane: Some((1, i64::MAX)) },
    Want { label: "hash", path: "game.gameUser.Hash", indices: &[], sane: None },
    // Expected to fail on V657 - see docs/pointer-chains.md.
    Want { label: "instance_id", path: "_instance.instanceID", indices: &[], sane: None },
    Want { label: "web_root", path: "WebRoot", indices: &[], sane: None },
];

/// Every hardcoded chain, in display order.
pub const ALL: &[Chain] = &[
    CURRENT_ZONE,
    STEELBONES_STACKS,
    GEMS,
    INSTANCE_ID,
    USER_ID,
    PLATFORM,
    HASH,
    WEB_ROOT,
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Guards against a chain written by hand instead of via `game_chain!`,
    /// which would be anchored somewhere other than `gameInstances[0]` and
    /// quietly produce plausible-looking nonsense.
    #[test]
    fn game_chains_share_the_game_instance_prefix() {
        const PREFIX: [u64; 5] = [0xF20, 0xE0, 0xB0, 0x10, 0x20];
        for chain in ALL.iter().filter(|c| c.root.name == IDLE_GAME_MANAGER.name) {
            let full = chain.full();
            assert!(
                full.starts_with(&PREFIX),
                "{} does not start at gameInstances[0]: {full:#x?}",
                chain.label
            );
            assert!(
                full.len() > PREFIX.len(),
                "{} has no offsets of its own",
                chain.label
            );
        }
    }

    #[test]
    fn labels_are_unique() {
        for (i, chain) in ALL.iter().enumerate() {
            assert!(
                !ALL[..i].iter().any(|c| c.label == chain.label),
                "duplicate label {}",
                chain.label
            );
        }
    }
}
