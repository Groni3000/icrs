//! What to do, as a file.
//!
//! A profile is typed fields, not a script. The AHK original mixed its
//! settings into its control flow, so changing one number meant editing the
//! loop; here the loop is fixed and the file only sets thresholds. Every key
//! is known in advance: a misspelled one is an error with a line number
//! rather than a setting that quietly never applies.

use std::fmt;
use std::io;
use std::path::Path;
use std::time::Duration;

use crate::offsets;
use crate::toml::{self, Value};

/// A champion as the profile names it, and the id once we have matched it.
/// Both halves are kept: the id does the work, the name makes the log
/// readable.
#[derive(Debug, Clone, PartialEq)]
pub struct Champion {
    pub asked: String,
    pub id: Option<i64>,
}

impl Champion {
    pub fn id(id: i64) -> Self {
        Self { asked: format!("#{id}"), id: Some(id) }
    }

    pub fn named(name: &str) -> Self {
        Self { asked: name.to_string(), id: None }
    }

    fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Int(id) => Some(Self::id(*id)),
            Value::Text(name) => match name.parse::<i64>() {
                Ok(id) => Some(Self::id(id)),
                Err(_) => Some(Self::named(name)),
            },
            _ => None,
        }
    }

    /// Match a name against the live roster. Ids pass straight through, so a
    /// profile written in ids works with no game attached.
    pub fn resolve(&mut self, roster: &[(i64, String)]) -> Result<i64, Error> {
        if let Some(id) = self.id {
            return Ok(id);
        }
        let wanted = self.asked.to_lowercase();
        let found = roster
            .iter()
            .find(|(_, name)| name.to_lowercase() == wanted)
            .or_else(|| roster.iter().find(|(_, name)| name.to_lowercase().contains(&wanted)));
        match found {
            Some((id, _)) => {
                self.id = Some(*id);
                Ok(*id)
            }
            None => Err(Error::invalid(format!("no champion matching {:?}", self.asked))),
        }
    }
}

impl fmt::Display for Champion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.asked)
    }
}

/// Ellywick's Deck of Many Things.
///
/// The cards she is *holding* are the bonus, and her ultimate throws the whole
/// hand away, so the ultimate is a re-draw and not a payoff: it is worth
/// pressing only once the hand on the table can no longer reach the number of
/// cards you are after.
#[derive(Debug, Clone, PartialEq)]
pub struct Deck {
    pub champion: Champion,
    /// How many cards she can hold at once.
    pub hand_size: i64,
    /// The card type being collected, as `offsets::effects::CARD_TYPES`
    /// numbers them.
    pub want: i64,
    /// How many of them the hand should end up holding.
    pub want_at_least: i64,
    /// The earliest zone at which the ultimate may be pressed. Zero turns the
    /// check off.
    pub zone_at_least: i64,
}

impl Deck {
    pub fn want_name(&self) -> &'static str {
        offsets::effects::card_name(self.want)
    }

    /// Can a hand of `held` cards, `have` of them the wanted type, still get
    /// there if every remaining draw goes our way?
    pub fn still_reachable(&self, held: i64, have: i64) -> bool {
        let draws_left = (self.hand_size - held).max(0);
        have + draws_left >= self.want_at_least
    }
}

/// Briv's stacks.
///
/// The two counts are the same stack at two points in its life: during a run
/// Briv builds Steelbones stacks by tanking, and when the modron core resets
/// the run they are converted into Sprint stacks, which is what does the
/// skipping. So the number worth putting in a file is the Sprint count the
/// *next* run should start with; the Steelbones count to farm for it follows
/// from the conversion ratio.
#[derive(Debug, Clone, PartialEq)]
pub struct Briv {
    pub champion: Champion,
    /// Sprint stacks the next run should begin with.
    pub sprint_at_least: i64,
    /// Whether the feat that converts 1 Steelbones into 1.2 Sprint is taken.
    pub conversion_feat: bool,
}

impl Briv {
    /// The Steelbones stacks to farm before the reset: enough that the
    /// conversion lands on `sprint_at_least`. With the feat each stack is
    /// worth 1.2, so five sixths of the target is enough; without it the
    /// conversion is 1:1.
    pub fn steelbones_needed(&self) -> i64 {
        if self.conversion_feat {
            (self.sprint_at_least * 5 + 5) / 6
        } else {
            self.sprint_at_least
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Ultimates {
    /// Champions whose ultimate should go off whenever it is off cooldown.
    pub fire_when_ready: Vec<Champion>,
    /// How long to leave a champion alone after firing. The game needs a
    /// moment to show the cooldown it just started, and until it does, every
    /// tick would read the same "ready" and fire again.
    pub recast: Duration,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Guards {
    /// Do nothing while the party is moving between zones.
    pub skip_while_transitioning: bool,
    /// Act only on a sample where everything read.
    pub require_full_reads: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    pub name: String,
    pub tick: Duration,
    pub deck: Option<Deck>,
    pub briv: Option<Briv>,
    pub ultimates: Ultimates,
    pub guards: Guards,
}

#[derive(Debug)]
pub struct Error {
    pub line: Option<usize>,
    pub reason: String,
}

impl Error {
    fn invalid(reason: impl Into<String>) -> Self {
        Self { line: None, reason: reason.into() }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "profile line {line}: {}", self.reason),
            None => write!(f, "profile: {}", self.reason),
        }
    }
}

impl std::error::Error for Error {}

impl From<toml::Error> for Error {
    fn from(err: toml::Error) -> Self {
        Self { line: Some(err.line), reason: err.reason }
    }
}

/// Every key a profile may set. Anything else is a typo, and is reported as
/// one.
const SCHEMA: &[(&str, &str)] = &[
    ("", "name"),
    ("", "tick_ms"),
    ("deck", "champion"),
    ("deck", "hand_size"),
    ("deck", "want"),
    ("deck", "want_at_least"),
    ("deck", "zone_at_least"),
    ("briv", "champion"),
    ("briv", "sprint_at_least"),
    ("briv", "conversion_feat"),
    ("ultimates", "fire_when_ready"),
    ("ultimates", "recast_ms"),
    ("guards", "skip_while_transitioning"),
    ("guards", "require_full_reads"),
];

/// A starting point, also what `icrs profile --example` prints.
pub const EXAMPLE: &str = r#"# An icrs profile. Thresholds, not a script: the loop is fixed, this file only
# says where its limits are. Every setting is optional and falls back to the
# value shown; a misspelled one is refused with its line number instead of
# quietly doing nothing.
name = "gem farm"

# How often to look at the game, in milliseconds. One look costs ~12ms.
#   250  -> four looks a second. Reacts to a drawn card within a quarter second.
#   1000 -> once a second. Perfectly fine, just slower to notice a draw.
tick_ms = 250


# --- Ellywick's Deck of Many Things ----------------------------------------
# She draws one card at a time up to a full hand, and the cards she is HOLDING
# are the bonus: gem cards raise gem find, moon cards raise gold find. Her
# ultimate throws the ENTIRE hand away and she starts again from nothing, so
# pressing it is a re-draw, not a payoff.
#
# Hence the one rule here: the ultimate is pressed only when the hand can no
# longer reach the number of cards you want, counting the draws she has left.
# A hand that can still get there is left alone, and a hand that already has
# what you want is never touched.
#
# Delete this whole section to leave her deck alone entirely.
[deck]
# Whose deck. A champion is a name as the game spells it, or an id - `icrs
# ultimates` prints both.
champion = "ellywick"

# How many cards she can hold at once. 5 is the normal maximum, and it is what
# "draws left" is counted against.
hand_size = 5

# The card type you are collecting: "gem", "moon", "knight", "fates" or
# "flames".
#   gem farming      -> "gem"   (gem find)
#   gold formation   -> "moon"  (gold find)
want = "gem"

# How many of that card the hand should end up holding. With want = "gem",
# want_at_least = 3 and hand_size = 5, a whole run of the deck looks like this:
#   0 cards, 0 gems -> left alone: five draws to come, 3 is still reachable
#   1 card,  1 gem  -> left alone: 1 + 4 draws still clears 3
#   4 cards, 1 gem  -> ULTIMATE: even a gem fifth card would only make 2
#   4 cards, 2 gems -> left alone: the fifth card can still make it 3
#   5 cards, 2 gems -> ULTIMATE: the hand is full and short, so redraw
#   5 cards, 3 gems -> left alone for good: this is what we were after
# For a gold-find formation, want = "moon" with want_at_least = 5 means only a
# full hand of moon cards is kept, and anything less is redrawn.
want_at_least = 3

# The earliest zone at which the ultimate may be pressed at all. The hand is
# still watched below it, just never discarded.
#   0   -> no zone check: redraw as soon as a hand is hopeless, anywhere
#   250 -> nothing is pressed before zone 250. This is for parking Briv in one
#          zone to farm Steelbones stacks: a hopeless hand is still carrying
#          some gem find, and while you sit there killing the same boss over
#          and over, keeping it beats spending three minutes of cooldown to
#          chase a better one.
zone_at_least = 0


# --- Briv's stacks ---------------------------------------------------------
# Two names for one stack at two points in its life. During a run Briv builds
# STEELBONES stacks by tanking (never by restarting the client). When the
# modron core resets the run, those are converted into SPRINT stacks - the
# skill is called Unnatural Haste - and the Sprint stacks are what skip zones
# in the run that follows.
#
# So what belongs in a file is the Sprint count the next run should start with.
# The Steelbones count to farm for it is worked out from the conversion ratio,
# not configured.
#
# These thresholds are the gate the skip cycle will wait on. `icrs run` reports
# progress towards them every tick; nothing presses a key over them yet.
#
# Delete this section to stop watching the stacks.
[briv]
champion = "briv"

# Sprint stacks the NEXT run should begin with - the number this run's tanking
# is working towards.
#   1000 -> a short farm cycle
#   8000 -> a long one, many more zones skipped after the reset
sprint_at_least = 1000

# Is the feat that converts 1 Steelbones stack into 1.2 Sprint stacks taken?
# It decides how many Steelbones stacks the run has to finish with:
#   sprint_at_least = 1000, conversion_feat = true  -> farm 834 steelbones (834 x 1.2 = 1000.8)
#   sprint_at_least = 1000, conversion_feat = false -> farm 1000 steelbones (1:1)
conversion_feat = true


# --- Ultimates -------------------------------------------------------------
[ultimates]
# Champions whose ultimate should simply go off whenever it is off cooldown,
# with no condition attached. Names or ids.
#   []                 -> nothing is fired on cooldown alone (the default)
#   ["shandie"]        -> her ultimate every time it comes back
fire_when_ready = []

# How long to leave a champion alone after pressing their key, in milliseconds.
# The client takes a moment to show the cooldown it just started, and until it
# does, every tick reads the same "ready" and would press again.
#   3000 -> three seconds of quiet. Lower it only if you see missed presses.
recast_ms = 3000


# --- Guards ----------------------------------------------------------------
[guards]
# Do nothing while the party is moving between zones - mid-transition the game
# is between two states and a press can land in neither.
skip_while_transitioning = true

# Act only on a sample where every value read. With this off, a tick that
# failed to read the hand still fires the rules that do not need it.
require_full_reads = true
"#;

impl Default for Profile {
    fn default() -> Self {
        Self {
            name: "default".to_string(),
            tick: Duration::from_millis(250),
            deck: Some(Deck {
                champion: Champion::id(offsets::effects::ELLYWICK_HERO_ID),
                hand_size: 5,
                want: offsets::effects::GEM_CARD,
                want_at_least: 3,
                zone_at_least: 0,
            }),
            briv: Some(Briv {
                champion: Champion::named("briv"),
                sprint_at_least: 1000,
                conversion_feat: true,
            }),
            ultimates: Ultimates {
                fire_when_ready: Vec::new(),
                recast: Duration::from_millis(3000),
            },
            guards: Guards { skip_while_transitioning: true, require_full_reads: true },
        }
    }
}

/// A profile and where it came from.
#[derive(Debug)]
pub struct Loaded {
    pub profile: Profile,
    pub source: String,
}

/// The file name looked for when none is given.
pub const FILE: &str = "icrs.toml";

/// Find a profile the way `catalog` finds offsets: what was asked for, then
/// the environment, then the working directory, then next to the executable -
/// and only then the built-in defaults, so a double-clicked binary with no
/// files beside it still runs.
pub fn discover(explicit: Option<&Path>) -> Result<Loaded, Error> {
    if let Some(path) = explicit {
        return Ok(Loaded { profile: Profile::load(path)?, source: path.display().to_string() });
    }
    if let Some(from_env) = std::env::var_os("ICRS_PROFILE") {
        let path = Path::new(&from_env);
        return Ok(Loaded {
            profile: Profile::load(path)?,
            source: format!("{} (ICRS_PROFILE)", path.display()),
        });
    }

    let mut candidates = vec![std::path::PathBuf::from(FILE)];
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        candidates.push(dir.join(FILE));
    }
    for path in candidates {
        if path.is_file() {
            return Ok(Loaded {
                profile: Profile::load(&path)?,
                source: path.display().to_string(),
            });
        }
    }
    Ok(Loaded { profile: Profile::default(), source: "built-in defaults".to_string() })
}

impl Profile {
    pub fn load(path: &Path) -> Result<Self, Error> {
        let text = std::fs::read_to_string(path)
            .map_err(|err: io::Error| Error::invalid(format!("{}: {err}", path.display())))?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self, Error> {
        let document = toml::parse(text)?;

        // Catch typos before reading anything: a profile that parses but does
        // not do what it says is worse than one that refuses to load.
        for entry in &document.entries {
            if !SCHEMA.iter().any(|(section, key)| *section == entry.section && *key == entry.key) {
                let known: Vec<String> = SCHEMA
                    .iter()
                    .filter(|(section, _)| *section == entry.section)
                    .map(|(_, key)| (*key).to_string())
                    .collect();
                let known = if known.is_empty() {
                    format!("no section named [{}]", entry.section)
                } else {
                    format!("known here: {}", known.join(", "))
                };
                return Err(Error {
                    line: Some(entry.line),
                    reason: format!("unknown setting {} ({known})", entry.dotted()),
                });
            }
        }

        let default = Profile::default();
        let fallback_deck = default.deck.clone().expect("the default profile watches the deck");
        let fallback_briv = default.briv.clone().expect("the default profile watches briv");

        let name = match document.get("", "name") {
            Some(entry) => entry.as_text()?.to_string(),
            None => default.name,
        };
        let tick = millis(&document, "", "tick_ms", default.tick, 20..=600_000)?;

        let deck = if document.has_section("deck") {
            let deck = Deck {
                champion: champion(&document, "deck", fallback_deck.champion)?,
                hand_size: integer(&document, "deck", "hand_size", fallback_deck.hand_size, 1..=10)?,
                want: card_type(&document, fallback_deck.want)?,
                want_at_least: integer(
                    &document,
                    "deck",
                    "want_at_least",
                    fallback_deck.want_at_least,
                    1..=10,
                )?,
                zone_at_least: integer(
                    &document,
                    "deck",
                    "zone_at_least",
                    fallback_deck.zone_at_least,
                    0..=10_000_000,
                )?,
            };
            if deck.want_at_least > deck.hand_size {
                let at = document.get("deck", "want_at_least").map(|entry| entry.line);
                return Err(Error {
                    line: at,
                    reason: format!(
                        "want_at_least is {} but the hand only holds {}, so it could never be reached",
                        deck.want_at_least, deck.hand_size
                    ),
                });
            }
            Some(deck)
        } else {
            None
        };

        let briv = if document.has_section("briv") {
            Some(Briv {
                champion: champion(&document, "briv", fallback_briv.champion)?,
                sprint_at_least: integer(
                    &document,
                    "briv",
                    "sprint_at_least",
                    fallback_briv.sprint_at_least,
                    0..=100_000_000,
                )?,
                conversion_feat: flag(
                    &document,
                    "briv",
                    "conversion_feat",
                    fallback_briv.conversion_feat,
                )?,
            })
        } else {
            None
        };

        let fire_when_ready = match document.get("ultimates", "fire_when_ready") {
            Some(entry) => {
                let mut champions = Vec::new();
                for item in entry.as_list()? {
                    let champion = Champion::from_value(item).ok_or_else(|| Error {
                        line: Some(entry.line),
                        reason: format!("{item} is not a champion: use a name in quotes or an id"),
                    })?;
                    champions.push(champion);
                }
                champions
            }
            None => default.ultimates.fire_when_ready,
        };

        Ok(Self {
            name,
            tick,
            deck,
            briv,
            ultimates: Ultimates {
                fire_when_ready,
                recast: millis(
                    &document,
                    "ultimates",
                    "recast_ms",
                    default.ultimates.recast,
                    0..=600_000,
                )?,
            },
            guards: Guards {
                skip_while_transitioning: flag(
                    &document,
                    "guards",
                    "skip_while_transitioning",
                    default.guards.skip_while_transitioning,
                )?,
                require_full_reads: flag(
                    &document,
                    "guards",
                    "require_full_reads",
                    default.guards.require_full_reads,
                )?,
            },
        })
    }

    /// Match every champion named in the profile against the live roster, so
    /// the decisions themselves only ever deal in ids.
    pub fn resolve(&mut self, roster: &[(i64, String)]) -> Result<(), Error> {
        if let Some(deck) = &mut self.deck {
            deck.champion.resolve(roster)?;
        }
        if let Some(briv) = &mut self.briv {
            briv.champion.resolve(roster)?;
        }
        for champion in &mut self.ultimates.fire_when_ready {
            champion.resolve(roster)?;
        }
        Ok(())
    }

    /// Champions this profile might act on, for a startup summary.
    pub fn champions(&self) -> Vec<&Champion> {
        let mut all: Vec<&Champion> = Vec::new();
        if let Some(deck) = &self.deck {
            all.push(&deck.champion);
        }
        if let Some(briv) = &self.briv {
            all.push(&briv.champion);
        }
        all.extend(self.ultimates.fire_when_ready.iter());
        all
    }
}

fn champion(
    document: &toml::Document,
    section: &str,
    fallback: Champion,
) -> Result<Champion, Error> {
    let Some(entry) = document.get(section, "champion") else {
        return Ok(fallback);
    };
    Champion::from_value(&entry.value)
        .ok_or_else(|| Error::from(entry.error("a champion is a name in quotes or an id")))
}

fn card_type(document: &toml::Document, fallback: i64) -> Result<i64, Error> {
    let Some(entry) = document.get("deck", "want") else {
        return Ok(fallback);
    };
    let name = entry.as_text()?;
    offsets::effects::card_type_by_name(name).ok_or_else(|| Error {
        line: Some(entry.line),
        reason: format!(
            "{name:?} is not a card type (known: {})",
            offsets::effects::CARD_TYPES
                .iter()
                .map(|(_, card)| card.to_lowercase())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    })
}

fn integer(
    document: &toml::Document,
    section: &str,
    key: &str,
    fallback: i64,
    sane: std::ops::RangeInclusive<i64>,
) -> Result<i64, Error> {
    let Some(entry) = document.get(section, key) else {
        return Ok(fallback);
    };
    let value = entry.as_int()?;
    if !sane.contains(&value) {
        return Err(Error {
            line: Some(entry.line),
            reason: format!(
                "{} is {value}, which is outside {}..={}",
                entry.dotted(),
                sane.start(),
                sane.end()
            ),
        });
    }
    Ok(value)
}

fn millis(
    document: &toml::Document,
    section: &str,
    key: &str,
    fallback: Duration,
    sane: std::ops::RangeInclusive<i64>,
) -> Result<Duration, Error> {
    let value = integer(document, section, key, fallback.as_millis() as i64, sane)?;
    Ok(Duration::from_millis(value as u64))
}

fn flag(
    document: &toml::Document,
    section: &str,
    key: &str,
    fallback: bool,
) -> Result<bool, Error> {
    match document.get(section, key) {
        Some(entry) => Ok(entry.as_bool()?),
        None => Ok(fallback),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_profile_is_the_example() {
        // profiles/gem-farm.toml is what a user reads first; it is the same
        // text as `icrs profile --example` prints, and this keeps it so.
        assert_eq!(include_str!("../profiles/gem-farm.toml"), EXAMPLE);
    }

    #[test]
    fn the_shipped_example_loads() {
        let profile = Profile::parse(EXAMPLE).unwrap();
        assert_eq!(profile.name, "gem farm");
        assert_eq!(profile.tick, Duration::from_millis(250));

        let deck = profile.deck.unwrap();
        assert_eq!(deck.champion, Champion::named("ellywick"));
        assert_eq!(deck.hand_size, 5);
        assert_eq!(deck.want, offsets::effects::GEM_CARD);
        assert_eq!(deck.want_at_least, 3);
        assert_eq!(deck.zone_at_least, 0);

        let briv = profile.briv.unwrap();
        assert_eq!(briv.champion, Champion::named("briv"));
        assert_eq!(briv.sprint_at_least, 1000);
        assert!(briv.conversion_feat);

        assert!(profile.ultimates.fire_when_ready.is_empty());
        assert!(profile.guards.require_full_reads);
    }

    #[test]
    fn a_section_left_out_turns_that_rule_off() {
        // Defaults have to be usable on their own, and leaving a section out
        // has to mean "do not touch it" - not "use my guesses".
        let parsed = Profile::parse("[briv]\nsprint_at_least = 50").unwrap();
        assert!(parsed.deck.is_none(), "no [deck] section means the deck is not watched");
        assert_eq!(parsed.tick, Profile::default().tick);
        let briv = parsed.briv.unwrap();
        assert_eq!(briv.sprint_at_least, 50);
        assert!(briv.conversion_feat, "the other keys keep their defaults");
    }

    #[test]
    fn the_steelbones_to_farm_follows_from_the_sprint_target() {
        // The file says how many Sprint stacks the next run needs; the stacks
        // to tank for are whatever converts into that.
        let mut briv = Profile::default().briv.unwrap();
        briv.sprint_at_least = 1000;

        briv.conversion_feat = true;
        assert_eq!(briv.steelbones_needed(), 834, "834 x 1.2 = 1000.8, just over");
        assert!(833 * 12 < 1000 * 10, "833 would land short, so 834 is the first that works");

        briv.conversion_feat = false;
        assert_eq!(briv.steelbones_needed(), 1000, "no feat, so 1:1");

        // And a profile asking for nothing asks for no tanking.
        briv.sprint_at_least = 0;
        assert_eq!(briv.steelbones_needed(), 0);
    }

    #[test]
    fn the_zone_gate_is_off_by_default_and_read_when_set() {
        assert_eq!(Profile::parse("[deck]").unwrap().deck.unwrap().zone_at_least, 0);
        let profile = Profile::parse("[deck]\nzone_at_least = 250").unwrap();
        assert_eq!(profile.deck.unwrap().zone_at_least, 250);
    }

    #[test]
    fn reachability_is_what_decides_a_redraw() {
        let deck = Profile::default().deck.unwrap(); // gem, 3 of a hand of 5
        // (cards held, wanted cards held) -> is the target still reachable?
        for (held, have, reachable) in [
            (0, 0, true),  // a fresh hand: five draws to come
            (1, 1, true),  // the feat's guaranteed gem, plus four draws
            (3, 1, true),  // 1 + 2 draws = 3, exactly enough
            (4, 1, false), // even a gem fifth card only makes 2
            (4, 2, true),  // the fifth card can still make it 3
            (5, 2, false), // full hand, short: redraw
            (5, 3, true),  // what we were after
        ] {
            assert_eq!(
                deck.still_reachable(held, have),
                reachable,
                "{have} of {held} cards"
            );
        }
    }

    #[test]
    fn a_target_bigger_than_the_hand_is_refused() {
        // Otherwise the deck rule would fire the ultimate forever.
        let err = Profile::parse("[deck]\nhand_size = 5\nwant_at_least = 6").unwrap_err();
        assert_eq!(err.line, Some(3));
        assert!(err.to_string().contains("could never be reached"), "{err}");
    }

    #[test]
    fn card_types_are_named_not_numbered() {
        let profile = Profile::parse("[deck]\nwant = \"moon\"").unwrap();
        assert_eq!(profile.deck.unwrap().want, offsets::effects::MOON_CARD);
        let err = Profile::parse("[deck]\nwant = \"gems\"").unwrap_err();
        assert!(err.to_string().contains("not a card type"), "{err}");
        assert!(err.to_string().contains("gem, fates"), "{err}");
    }

    #[test]
    fn a_misspelled_setting_is_refused_with_its_line() {
        let err = Profile::parse("[deck]\nwant_at_leats = 1").unwrap_err();
        assert_eq!(err.line, Some(2));
        assert!(err.to_string().contains("unknown setting deck.want_at_leats"), "{err}");
        assert!(err.to_string().contains("want_at_least"), "{err}");
    }

    #[test]
    fn an_unknown_section_says_so() {
        let err = Profile::parse("[dekc]\nx = 1").unwrap_err();
        assert!(err.to_string().contains("no section named [dekc]"), "{err}");
    }

    #[test]
    fn implausible_numbers_are_refused() {
        let err = Profile::parse("[deck]\nhand_size = 99").unwrap_err();
        assert!(err.to_string().contains("outside 1..=10"), "{err}");
        let err = Profile::parse("tick_ms = 1").unwrap_err();
        assert!(err.to_string().contains("outside 20..=600000"), "{err}");
    }

    #[test]
    fn champions_may_be_names_or_ids() {
        let profile =
            Profile::parse("[ultimates]\nfire_when_ready = [\"briv\", 83, \"100\"]").unwrap();
        let wanted = &profile.ultimates.fire_when_ready;
        assert_eq!(wanted[0], Champion::named("briv"));
        assert_eq!(wanted[1].id, Some(83));
        assert_eq!(wanted[2].id, Some(100), "a quoted number is still an id");
    }

    #[test]
    fn names_are_matched_against_the_roster() {
        let roster = vec![(58, "Briv".to_string()), (83, "Ellywick".to_string())];
        let mut profile = Profile::parse(EXAMPLE).unwrap();
        profile.resolve(&roster).unwrap();
        assert_eq!(profile.deck.unwrap().champion.id, Some(83));
        assert_eq!(profile.briv.unwrap().champion.id, Some(58));
    }

    #[test]
    fn a_name_nobody_answers_to_is_an_error() {
        let roster = vec![(58, "Briv".to_string())];
        let mut profile = Profile::parse("[deck]\nchampion = \"ellywik\"").unwrap();
        let err = profile.resolve(&roster).unwrap_err();
        assert!(err.to_string().contains("no champion matching \"ellywik\""), "{err}");
    }

    #[test]
    fn ids_need_no_roster() {
        let mut profile = Profile::parse("[deck]\nchampion = 83").unwrap();
        profile.resolve(&[]).unwrap();
        assert_eq!(profile.deck.unwrap().champion.id, Some(83));
    }
}
