//! One sample of the game's live state.
//!
//! A tick of any automation should look at exactly one `Snapshot` and nothing
//! else. The AHK script read each value where it happened to need it, so two
//! branches could disagree about the same zone or stack count milliseconds
//! apart, and a read that silently failed became a blank string that compared
//! false. Here a failed read is a `None` plus a named miss, and every decision
//! sees the same numbers.
//!
//! `Sampler` resolves its chains once, then reads them in as few calls to the
//! game as it can - `crate::batch` is where that happens, and why it matters.

use std::time::{Duration, SystemTime};

use crate::game::Ultimate;
#[cfg(windows)]
use crate::batch::{Batch, Walk};
use crate::offsets;

// Sampling needs an attached game, which is Win32 only. A `Snapshot` is plain
// data, so everything that decides anything from one is testable anywhere.
#[cfg(windows)]
use std::io;
#[cfg(windows)]
use std::time::Instant;
#[cfg(windows)]
use crate::catalog::Chain;
#[cfg(windows)]
use crate::game::Game;

#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub at: SystemTime,
    /// How long the sample took, which is also the window during which the
    /// values could disagree with each other.
    pub took: Duration,
    /// Calls made to the game for this sample. Under Proton each one is a
    /// round trip to the wineserver at ~18us, so this is the number that
    /// decides `took` - see `crate::batch`.
    pub reads: usize,
    /// How many of those were spent following pointers rather than reading
    /// values.
    pub pointer_reads: usize,

    pub zone: Option<i64>,
    pub highest_zone: Option<i64>,
    pub quest_left: Option<i64>,
    pub transitioning: Option<bool>,
    pub modron_target: Option<i64>,

    pub steelbones: Option<i64>,
    pub sprint: Option<i64>,
    pub resets: Option<i64>,
    pub gems: Option<i64>,
    pub since_hit: Option<f64>,

    pub ultimates: Vec<Ultimate>,
    /// Ellywick's hand as card types, when she is being tracked.
    pub cards: Option<Vec<i64>>,

    /// Values that did not read this tick, by name. Empty is the healthy case;
    /// anything here means a decision is working with less than it asked for.
    pub misses: Vec<String>,
}

impl Snapshot {
    /// Bosses cleared: they sit on every fifth zone, so integer division.
    pub fn bosses(&self) -> Option<i64> {
        self.highest_zone.map(|zone| zone.div_euclid(5))
    }

    /// Is the party standing on a boss zone right now?
    pub fn on_boss_zone(&self) -> Option<bool> {
        self.zone.map(|zone| zone > 0 && zone % 5 == 0)
    }

    /// How many cards of one type are in hand.
    pub fn cards_of(&self, card_type: i64) -> Option<usize> {
        self.cards
            .as_ref()
            .map(|hand| hand.iter().filter(|card| **card == card_type).count())
    }

    pub fn gem_cards(&self) -> Option<usize> {
        self.cards_of(offsets::effects::GEM_CARD)
    }

    pub fn moon_cards(&self) -> Option<usize> {
        self.cards_of(offsets::effects::MOON_CARD)
    }

    pub fn ultimate(&self, hero_id: i64) -> Option<&Ultimate> {
        self.ultimates.iter().find(|entry| entry.hero_id == hero_id)
    }

    /// Can this champion's ultimate be fired right now?
    pub fn ultimate_ready(&self, hero_id: i64) -> bool {
        self.ultimate(hero_id).is_some_and(|entry| entry.ready() && !entry.queued)
    }

    pub fn healthy(&self) -> bool {
        self.misses.is_empty()
    }

    /// Compact JSON, for logging a tick per line.
    pub fn to_json(&self) -> String {
        let number = |value: Option<i64>| match value {
            Some(value) => value.to_string(),
            None => "null".to_string(),
        };
        let float = |value: Option<f64>| match value {
            Some(value) => format!("{value:.3}"),
            None => "null".to_string(),
        };
        let flag = |value: Option<bool>| match value {
            Some(value) => value.to_string(),
            None => "null".to_string(),
        };
        let seconds = self
            .at
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|since| since.as_secs_f64())
            .unwrap_or(0.0);

        let mut out = String::new();
        out.push_str(&format!(
            "{{\"at\":{seconds:.3},\"took_ms\":{:.1},\"reads\":{}",
            self.took.as_secs_f64() * 1000.0,
            self.reads
        ));
        out.push_str(&format!(",\"zone\":{}", number(self.zone)));
        out.push_str(&format!(",\"highest_zone\":{}", number(self.highest_zone)));
        out.push_str(&format!(",\"bosses\":{}", number(self.bosses())));
        out.push_str(&format!(",\"quest_left\":{}", number(self.quest_left)));
        out.push_str(&format!(",\"transitioning\":{}", flag(self.transitioning)));
        out.push_str(&format!(",\"modron_target\":{}", number(self.modron_target)));
        out.push_str(&format!(",\"steelbones\":{}", number(self.steelbones)));
        out.push_str(&format!(",\"sprint\":{}", number(self.sprint)));
        out.push_str(&format!(",\"resets\":{}", number(self.resets)));
        out.push_str(&format!(",\"gems\":{}", number(self.gems)));
        out.push_str(&format!(",\"since_hit\":{}", float(self.since_hit)));

        match &self.cards {
            Some(hand) => {
                let cards: Vec<String> = hand
                    .iter()
                    .map(|card| format!("\"{}\"", offsets::effects::card_name(*card)))
                    .collect();
                out.push_str(&format!(",\"cards\":[{}]", cards.join(",")));
            }
            None => out.push_str(",\"cards\":null"),
        }

        let ultimates: Vec<String> = self
            .ultimates
            .iter()
            .map(|entry| {
                format!(
                    "{{\"slot\":{},\"hero\":{},\"key\":{},\"cooldown\":{:.1},\"queued\":{}}}",
                    entry.slot, entry.hero_id, entry.hotkey, entry.cooldown, entry.queued
                )
            })
            .collect();
        out.push_str(&format!(",\"ultimates\":[{}]", ultimates.join(",")));

        let misses: Vec<String> = self.misses.iter().map(|miss| format!("\"{miss}\"")).collect();
        out.push_str(&format!(",\"misses\":[{}]}}", misses.join(",")));
        out
    }
}

#[cfg(windows)]
/// The maximum hand in the Deck of Many Things.
const MAX_CARDS: u64 = 5;

#[cfg(windows)]
/// Chains for the ultimates bar are built up front for this many slots; the
/// live size is read each tick and only that many are sampled.
const MAX_ULTIMATE_SLOTS: u64 = 16;

/// Chains resolved once, and the slots each value comes back in.
///
/// A sample is two passes, because the second depends on the first: the
/// ultimates bar and the hand have to be measured before we know how many
/// entries to read. Both passes share one `Walk`, so the long prefix they have
/// in common is dereferenced once for the whole tick.
#[cfg(windows)]
pub struct Sampler {
    /// Everything whose address does not depend on this tick's own readings.
    core: Batch,
    scalars: Vec<Scalar>,
    bar_size: usize,
    hand_size: Option<usize>,
    /// Chains for every slot the bar might have and every card she might
    /// hold; which of them to read is decided per tick from the counts above.
    ultimates: Vec<UltimateChains>,
    cards: Option<Vec<Chain>>,
    /// Why something is not being sampled, said once at startup rather than
    /// every tick.
    pub notes: Vec<String>,
}

/// A simple value: where it lands in a snapshot, and what to call it if it
/// does not arrive.
#[cfg(windows)]
struct Scalar {
    slot: usize,
    label: &'static str,
    place: Place,
}

/// Which snapshot field a scalar fills.
#[cfg(windows)]
#[derive(Clone, Copy)]
enum Place {
    Zone,
    HighestZone,
    QuestLeft,
    Transitioning,
    ModronTarget,
    Steelbones,
    Sprint,
    Resets,
    Gems,
    SinceHit,
}

#[cfg(windows)]
struct UltimateChains {
    slot: u64,
    hero_id: Chain,
    hotkey: Chain,
    cooldown: Chain,
    queued: Chain,
}

#[cfg(windows)]
impl Sampler {
    /// Resolve everything a sample needs. The champion and effect-key lookups
    /// happen here, once - they are the expensive part, hundreds of reads.
    pub fn new(game: &Game) -> io::Result<Self> {
        use Place::*;

        let module = game.module();
        let mut core = Batch::new();
        let mut scalars = Vec::new();

        for (label, place) in [
            ("zone", Zone),
            ("highest_zone", HighestZone),
            ("quest_left", QuestLeft),
            ("transitioning", Transitioning),
            ("modron_target", ModronTarget),
            ("steelbones", Steelbones),
            ("sprint", Sprint),
            ("resets", Resets),
            ("gems", Gems),
            ("since_hit", SinceHit),
        ] {
            let path = offsets::NAMED
                .iter()
                .find(|want| want.label == label)
                .ok_or_else(|| io::Error::other(format!("no such label: {label}")))?
                .path;
            let chain = game.chain(path, &[])?;
            scalars.push(Scalar { slot: core.push_chain(module, &chain), label, place });
        }

        let bar_size = core.push_chain(module, &game.chain(offsets::ultimates::SIZE, &[])?);

        let mut ultimates = Vec::new();
        for slot in 0..MAX_ULTIMATE_SLOTS {
            let at = [0, slot];
            ultimates.push(UltimateChains {
                slot,
                hero_id: game.chain(offsets::ultimates::HERO_ID, &at)?,
                hotkey: game.chain(offsets::ultimates::HOTKEY, &at)?,
                cooldown: game.chain(offsets::ultimates::COOLDOWN, &at)?,
                queued: game.chain(offsets::ultimates::QUEUED, &at)?,
            });
        }

        let mut notes = Vec::new();
        let (hand_size, cards) = match Self::resolve_cards(game) {
            Ok((size, types)) => (Some(core.push_chain(module, &size)), Some(types)),
            Err(err) => {
                notes.push(format!("cards not tracked: {err}"));
                (None, None)
            }
        };

        Ok(Self { core, scalars, bar_size, hand_size, ultimates, cards, notes })
    }

    /// Ellywick's deck, if she is in the formation with the handler loaded:
    /// the chain for the size of her hand, and one per card she could hold.
    fn resolve_cards(game: &Game) -> io::Result<(Chain, Vec<Chain>)> {
        let hero = game.hero_index(offsets::effects::ELLYWICK_HERO_ID)?;
        let entry = game.find_effect(hero, offsets::effects::DECK_KEYS)?;
        let at = [0, hero, entry, 0, 0];
        let mut types = Vec::new();
        for card in 0..MAX_CARDS {
            let at = [0, hero, entry, 0, 0, card];
            types.push(game.chain(offsets::effects::CARD_TYPE, &at)?);
        }
        Ok((game.chain(offsets::effects::CARDS, &at)?, types))
    }

    pub fn tracks_cards(&self) -> bool {
        self.cards.is_some()
    }

    /// Take one sample. Never fails as a whole: whatever cannot be read is
    /// `None` and named in `misses`, because a decision needs to know what it
    /// is missing, not to be denied every other value.
    pub fn take(&self, game: &Game) -> Snapshot {
        let started = Instant::now();
        let at = SystemTime::now();

        let mut snapshot = Snapshot {
            at,
            took: Duration::ZERO,
            reads: 0,
            pointer_reads: 0,
            zone: None,
            highest_zone: None,
            quest_left: None,
            transitioning: None,
            modron_target: None,
            steelbones: None,
            sprint: None,
            resets: None,
            gems: None,
            since_hit: None,
            ultimates: Vec::new(),
            cards: None,
            misses: Vec::new(),
        };

        let mut walk = Walk::new(game.process());
        let core = walk.values(&self.core);

        for scalar in &self.scalars {
            let Some(value) = &core[scalar.slot] else {
                snapshot.misses.push(scalar.label.to_string());
                continue;
            };
            match scalar.place {
                Place::Zone => snapshot.zone = value.as_i64(),
                Place::HighestZone => snapshot.highest_zone = value.as_i64(),
                Place::QuestLeft => snapshot.quest_left = value.as_i64(),
                Place::Transitioning => snapshot.transitioning = value.as_bool(),
                Place::ModronTarget => snapshot.modron_target = value.as_i64(),
                Place::Steelbones => snapshot.steelbones = value.as_i64(),
                Place::Sprint => snapshot.sprint = value.as_i64(),
                Place::Resets => snapshot.resets = value.as_i64(),
                Place::Gems => snapshot.gems = value.as_i64(),
                Place::SinceHit => snapshot.since_hit = value.as_f64(),
            }
        }

        // Second pass: now that the counts are known, read exactly as many
        // bar slots and cards as there actually are.
        let module = game.module();
        let mut rest = Batch::new();

        let bar = core[self.bar_size].as_ref().and_then(|value| value.as_i64());
        let mut bar_slots = Vec::new();
        match bar {
            Some(size) => {
                for chains in self.ultimates.iter().take(size.clamp(0, 64) as usize) {
                    bar_slots.push((
                        chains.slot,
                        rest.push_chain(module, &chains.hero_id),
                        rest.push_chain(module, &chains.hotkey),
                        rest.push_chain(module, &chains.cooldown),
                        rest.push_chain(module, &chains.queued),
                    ));
                }
            }
            None => snapshot.misses.push("ultimates".to_string()),
        }

        let held = self
            .hand_size
            .and_then(|slot| core[slot].as_ref().and_then(|value| value.as_i64()));
        let mut card_slots = Vec::new();
        match (&self.cards, held) {
            (Some(chains), Some(size)) if (0..=MAX_CARDS as i64).contains(&size) => {
                for chain in chains.iter().take(size as usize) {
                    card_slots.push(rest.push_chain(module, chain));
                }
            }
            (Some(_), _) => snapshot.misses.push("cards".to_string()),
            (None, _) => {}
        }

        let rest = walk.values(&rest);

        for (slot, hero_id, hotkey, cooldown, queued) in bar_slots {
            let (Some(hero_id), Some(hotkey), Some(cooldown)) = (
                rest[hero_id].as_ref().and_then(|value| value.as_i64()),
                rest[hotkey].as_ref().and_then(|value| value.as_i64()),
                rest[cooldown].as_ref().and_then(|value| value.as_f64()),
            ) else {
                continue;
            };
            let queued =
                rest[queued].as_ref().and_then(|value| value.as_bool()).unwrap_or(false);
            snapshot.ultimates.push(Ultimate { slot, hero_id, hotkey, cooldown, queued });
        }

        if !card_slots.is_empty() || (self.cards.is_some() && held == Some(0)) {
            let hand: Vec<i64> = card_slots
                .iter()
                .filter_map(|slot| rest[*slot].as_ref().and_then(|value| value.as_i64()))
                .collect();
            if hand.len() == card_slots.len() {
                snapshot.cards = Some(hand);
            } else {
                snapshot.misses.push("cards".to_string());
            }
        }

        snapshot.reads = walk.calls();
        snapshot.pointer_reads = walk.pointers();
        snapshot.took = started.elapsed();
        snapshot
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A snapshot is plain data, so the things decisions will ask of it can be
    /// tested without a game anywhere near.
    fn snapshot() -> Snapshot {
        Snapshot {
            at: SystemTime::UNIX_EPOCH,
            took: Duration::from_millis(3),
            reads: 31,
            pointer_reads: 20,
            zone: Some(270),
            highest_zone: Some(86),
            quest_left: Some(4),
            transitioning: Some(false),
            modron_target: Some(546),
            steelbones: Some(1158),
            sprint: Some(12),
            resets: Some(3992),
            gems: Some(13478),
            since_hit: Some(4.5),
            ultimates: vec![
                Ultimate { slot: 3, hero_id: 58, hotkey: 4, cooldown: -12.0, queued: false },
                Ultimate { slot: 7, hero_id: 83, hotkey: 8, cooldown: 174.0, queued: false },
            ],
            cards: Some(vec![3, 2, 1, 5, 3]),
            misses: Vec::new(),
        }
    }

    #[test]
    fn bosses_are_every_fifth_zone() {
        let mut snap = snapshot();
        for (highest, expected) in [(1, 0), (4, 0), (5, 1), (6, 1), (9, 1), (10, 2), (86, 17)] {
            snap.highest_zone = Some(highest);
            assert_eq!(snap.bosses(), Some(expected), "highest zone {highest}");
        }
    }

    #[test]
    fn boss_zone_detection_excludes_zone_zero() {
        let mut snap = snapshot();
        for (zone, expected) in [(0, false), (1, false), (5, true), (270, true), (271, false)] {
            snap.zone = Some(zone);
            assert_eq!(snap.on_boss_zone(), Some(expected), "zone {zone}");
        }
    }

    #[test]
    fn counts_cards_by_type() {
        let snap = snapshot();
        assert_eq!(snap.gem_cards(), Some(2));
        assert_eq!(snap.moon_cards(), Some(1));
        assert_eq!(snap.cards_of(4), Some(0));
    }

    #[test]
    fn untracked_cards_are_none_not_zero() {
        let mut snap = snapshot();
        snap.cards = None;
        // The difference matters: "no gem cards" and "we are not looking at
        // her hand" must not lead to the same decision.
        assert_eq!(snap.gem_cards(), None);
    }

    #[test]
    fn ultimate_readiness_follows_the_cooldown_sign() {
        let snap = snapshot();
        assert!(snap.ultimate_ready(58), "briv is off cooldown");
        assert!(!snap.ultimate_ready(83), "ellywick has 174s left");
        assert!(!snap.ultimate_ready(999), "champion not in the bar");
    }

    #[test]
    fn a_queued_ultimate_is_not_ready_again() {
        let mut snap = snapshot();
        snap.ultimates[0].queued = true;
        assert!(!snap.ultimate_ready(58));
    }

    #[test]
    fn misses_make_a_snapshot_unhealthy() {
        let mut snap = snapshot();
        assert!(snap.healthy());
        snap.misses.push("zone".to_string());
        assert!(!snap.healthy());
    }

    #[test]
    fn json_carries_the_derived_values_too() {
        let json = snapshot().to_json();
        assert!(json.contains("\"zone\":270"), "{json}");
        assert!(json.contains("\"bosses\":17"), "{json}");
        assert!(json.contains("\"cards\":[\"Gem\",\"Moon\",\"Knight\",\"Flames\",\"Gem\"]"), "{json}");
        assert!(json.contains("\"misses\":[]"), "{json}");
    }
}
