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
    /// `Engine.Numeric.Quad`: the game's own 16-byte big number, a 64-bit
    /// mantissa followed by a 64-bit binary exponent. Health and damage at
    /// deep zones outgrow every native type, which is why it exists.
    Quad,
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
            "Int64" => Some(Kind::I64),
            "Quad" => Some(Kind::Quad),
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
    /// Part of `probe`'s one-shot report. The rest are still readable by name
    /// with `read` and `watch`.
    pub probe: bool,
}

/// Shorthand for the common case: no collection indices beyond element 0.
/// Separate arms rather than optional fragments, because a `$(...)?` group has
/// to bind a variable to be repeated in the body.
macro_rules! want {
    (@make $label:literal, $path:literal, $sane:expr, $probe:expr) => {
        Want { label: $label, path: $path, indices: &[], sane: $sane, probe: $probe }
    };
    ($label:literal, $path:literal) => { want!(@make $label, $path, None, false) };
    ($label:literal, $path:literal, probe) => { want!(@make $label, $path, None, true) };
    ($label:literal, $path:literal, sane = $low:literal ..= $high:expr) => {
        want!(@make $label, $path, Some(($low, $high)), false)
    };
    ($label:literal, $path:literal, sane = $low:literal ..= $high:expr, probe) => {
        want!(@make $label, $path, Some(($low, $high)), true)
    };
}

/// Everything readable by name. `probe` reports the flagged ones; `read` and
/// `watch` accept any label here, or any raw path from the drop.
pub const NAMED: &[Want] = &[
    // Where the run is
    want!("zone", "game.gameInstances.ActiveCampaignData.currentAreaID", sane = -1..=100_000, probe),
    want!("highest_zone", "game.gameInstances.ActiveCampaignData.highestAvailableAreaID"),
    want!("quest_left", "game.gameInstances.ActiveCampaignData.currentArea.QuestRemaining"),
    want!("area_hp", "game.gameInstances.ActiveCampaignData.currentArea.Health"),
    want!("transitioning", "game.gameInstances.Controller.areaTransitioner.IsTransitioning_k__BackingField"),
    want!("modron_target", "game.gameInstances.Controller.userData.ModronHandler.modronSaves.targetArea"),

    // Briv
    want!("steelbones", "game.gameInstances.Controller.userData.StatHandler.BrivSteelbonesStacks", sane = 0..=10_000_000, probe),
    want!("sprint", "game.gameInstances.Controller.userData.StatHandler.BrivSprintStacks"),
    want!("resets", "game.gameInstances.Controller.userData.StatHandler.Resets"),

    // Account and session
    want!("gems", "game.gameInstances.Controller.userData.redRubies", sane = 0..=1_000_000_000, probe),
    // Officially UserID and Hash live on GameSettings, whose pointer path is
    // stale; this is the route the AHK script falls back to.
    want!("user_id", "game.gameUser.ID", sane = 1..=i64::MAX, probe),
    want!("hash", "game.gameUser.Hash", probe),
    want!("inited", "game.gameInstances.Controller.userData.inited"),
    want!("last_save", "game.gameInstances.Controller.userData.SaveHandler.lastUserDataSaveTime"),
    want!("heroes", "game.gameInstances.Controller.userData.HeroHandler.heroes.size"),
    want!("ultimates", "game.gameInstances.Screen.uiController.ultimatesBar.ultimateItems.size"),

    // Damage timing
    want!("highest_hit", "game.gameInstances.ActiveCampaignData.highestHitDamage"),
    want!("since_hit", "game.gameInstances.ActiveCampaignData.timeSinceHighestHit"),

    // Expected to fail on V657 - see docs/pointer-chains.md.
    want!("instance_id", "_instance.instanceID", probe),
    want!("web_root", "WebRoot", probe),
];

/// A value worked out from another one rather than read from memory.
#[derive(Debug, Clone, Copy)]
pub struct Derived {
    pub label: &'static str,
    /// The label this is computed from.
    pub from: &'static str,
    pub divide: i64,
    pub why: &'static str,
}

/// Bosses sit on every fifth zone, so integer division gives how many have
/// been cleared: zones 1-4 none, zone 5 one, zones 6-9 still one.
pub const DERIVED: &[Derived] = &[Derived {
    label: "bosses",
    from: "highest_zone",
    divide: 5,
    why: "boss zones are every fifth",
}];

/// Paths used by the ultimates commands. The bar is a `List` of items, each
/// knowing its champion and the number key that fires it.
pub mod ultimates {
    pub const SIZE: &str = "game.gameInstances.Screen.uiController.ultimatesBar.ultimateItems.size";
    pub const HERO_ID: &str =
        "game.gameInstances.Screen.uiController.ultimatesBar.ultimateItems.hero.def.ID";
    pub const HOTKEY: &str =
        "game.gameInstances.Screen.uiController.ultimatesBar.ultimateItems.Hotkey";
    pub const COOLDOWN: &str = "game.gameInstances.Screen.uiController.ultimatesBar.ultimateItems.\
                                ultimateAttack.internalCooldownTimer";
    pub const QUEUED: &str =
        "game.gameInstances.Screen.uiController.ultimatesBar.ultimateItems.ultimateAttack.Queued";
}

/// A champion's effect keys, and Ellywick's deck inside one of them.
///
/// `effectKeysByHashedKeyName` is a `Dictionary<int, List<EffectKey>>` whose
/// keys are hashes we cannot reproduce, so the entries are walked and their
/// `def.Key` strings compared - the same approach Script Hub takes. The
/// indices for these paths are, in order: game instance, hero, dict entry,
/// effect-key list element, handler, and for a card, the card.
pub mod effects {
    pub const COUNT: &str =
    "game.gameInstances.Controller.userData.HeroHandler.heroes.effects.\
     effectKeysByHashedKeyName.size";

    pub const KEY: &str =
    "game.gameInstances.Controller.userData.HeroHandler.heroes.effects.\
     effectKeysByHashedKeyName.List.parentEffectKeyHandler.def.Key";

    pub const HANDLERS: &str =
    "game.gameInstances.Controller.userData.HeroHandler.heroes.effects.\
     effectKeysByHashedKeyName.List.parentEffectKeyHandler.activeEffectHandlers.size";

    pub const CARDS: &str =
    "game.gameInstances.Controller.userData.HeroHandler.heroes.effects.\
     effectKeysByHashedKeyName.List.parentEffectKeyHandler.activeEffectHandlers.\
     cardsInHand.size";

    pub const CARD_TYPE: &str =
    "game.gameInstances.Controller.userData.HeroHandler.heroes.effects.\
     effectKeysByHashedKeyName.List.parentEffectKeyHandler.activeEffectHandlers.\
     cardsInHand.CardType";

    /// Ellywick, and the effect key her deck lives under. The game calls it
    /// `deck_of_many_things`; Script Hub looks for `call_of_the_feywild`, so
    /// both are tried.
    pub const ELLYWICK_HERO_ID: i64 = 83;
    pub const DECK_KEYS: &[&str] =
        &["ellywick_deck_of_many_things", "ellywick_call_of_the_feywild"];

    /// `CardType` as the game numbers it. Gem raises the gem find, Moon the
    /// gold find - the two worth holding a hand of.
    pub const CARD_TYPES: &[(i64, &str)] =
        &[(1, "Knight"), (2, "Moon"), (3, "Gem"), (4, "Fates"), (5, "Flames")];

    pub const GEM_CARD: i64 = 3;

    pub fn card_name(card_type: i64) -> &'static str {
        CARD_TYPES
            .iter()
            .find(|(number, _)| *number == card_type)
            .map(|(_, name)| *name)
            .unwrap_or("?")
    }

    /// Accepts "moon", "Moon", or the raw number.
    pub fn card_type_by_name(name: &str) -> Option<i64> {
        if let Ok(number) = name.parse::<i64>() {
            return CARD_TYPES.iter().find(|(n, _)| *n == number).map(|(n, _)| *n);
        }
        CARD_TYPES
            .iter()
            .find(|(_, card)| card.eq_ignore_ascii_case(name))
            .map(|(number, _)| *number)
    }
}

/// The champion roster, for turning a name into an id.
pub mod heroes {
    pub const SIZE: &str = "game.gameInstances.Controller.userData.HeroHandler.heroes.size";
    pub const ID: &str = "game.gameInstances.Controller.userData.HeroHandler.heroes.def.ID";
    pub const NAME: &str = "game.gameInstances.Controller.userData.HeroHandler.heroes.def.name";
    pub const BENCHED: &str = "game.gameInstances.Controller.userData.HeroHandler.heroes.Benched";
}

/// Every hardcoded chain, in display order.
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

#[cfg(test)]
mod path_tests {
    use super::*;

    /// The long effect paths are written with line continuations, which strip
    /// leading whitespace - but only if every continuation line is indented.
    /// A stray space would make the path silently wrong.
    #[test]
    fn effect_paths_have_no_whitespace() {
        for path in [
            effects::COUNT,
            effects::KEY,
            effects::HANDLERS,
            effects::CARDS,
            effects::CARD_TYPE,
            ultimates::COOLDOWN,
        ] {
            assert!(!path.contains(' '), "{path:?} contains a space");
            assert!(!path.contains('\n'), "{path:?} contains a newline");
        }
        assert!(effects::CARD_TYPE.ends_with(".cardsInHand.CardType"), "{}", effects::CARD_TYPE);
        assert!(effects::COUNT.ends_with("effectKeysByHashedKeyName.size"), "{}", effects::COUNT);
    }

    #[test]
    fn derived_values_point_at_real_labels() {
        for derived in DERIVED {
            assert!(
                NAMED.iter().any(|want| want.label == derived.from),
                "{} is derived from {}, which is not a label",
                derived.label,
                derived.from
            );
            assert!(derived.divide > 0, "{} divides by {}", derived.label, derived.divide);
        }
    }
}
