//! Deciding what to do about one snapshot.
//!
//! `decide` is a pure function: a profile, what we have already done, and one
//! sample in; a list of actions out. Nothing in here touches the game, which
//! is the whole point - every rule below can be tested at a chosen moment in
//! a chosen game state, and the loop that sends keys has no judgement of its
//! own to get wrong.
//!
//! Two rules matter more than the rest, and both are rules about *not* acting.
//! A decision stays held for `recast` after it is taken, because the game
//! needs a moment to show the cooldown it just started. And Ellywick's
//! ultimate is only pressed when her hand can no longer reach the cards we
//! want: it discards everything she holds, so pressing it early throws away
//! the very thing we are collecting.

use std::fmt;
use std::time::{Duration, SystemTime};

use crate::input;
use crate::profile::{Champion, Profile};
use crate::snapshot::Snapshot;

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Fire an ultimate by pressing its hotkey in the game's window.
    Ultimate { slot: u64, hero_id: i64, hotkey: i64, champion: String, why: String },
}

impl Action {
    /// The champion an action is about, for the per-champion hold window.
    pub fn hero_id(&self) -> i64 {
        match self {
            Action::Ultimate { hero_id, .. } => *hero_id,
        }
    }

    pub fn why(&self) -> &str {
        match self {
            Action::Ultimate { why, .. } => why,
        }
    }
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Action::Ultimate { slot, champion, hotkey, why, .. } => {
                write!(f, "ultimate: {champion} (slot {slot}, key {hotkey}) - {why}")
            }
        }
    }
}

/// Where Briv's stacks stand against what the next run needs.
///
/// Steelbones is what this run is building; Sprint is what it becomes at the
/// modron reset, and `sprint_target` is the number the profile asked the next
/// run to start with. Nothing presses a key over this: it is the gate the skip
/// cycle will wait on, and until that exists it is what the status line
/// reports.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stacks {
    /// Steelbones stacks tanked so far this run.
    pub steelbones: i64,
    /// Sprint stacks in hand now - left over from the last conversion, and
    /// spent as the run skips zones.
    pub sprint: i64,
    /// Steelbones stacks to finish the run with, after the conversion ratio.
    pub steelbones_needed: i64,
    /// Sprint stacks the next run should start with.
    pub sprint_target: i64,
    pub feat: bool,
}

impl Stacks {
    /// Has this run tanked enough to convert into the Sprint stacks asked for?
    pub fn enough(&self) -> bool {
        self.steelbones >= self.steelbones_needed
    }

    /// What this run's Steelbones would convert into if the core reset now.
    pub fn converts_to(&self) -> i64 {
        if self.feat { self.steelbones * 12 / 10 } else { self.steelbones }
    }

    pub fn progress(&self) -> String {
        let ratio = if self.feat { "x1.2" } else { "1:1" };
        format!(
            "briv: {}/{} steelbones -> {} of {} sprint next run ({ratio}), {} sprint now",
            self.steelbones,
            self.steelbones_needed,
            self.converts_to(),
            self.sprint_target,
            self.sprint
        )
    }
}

/// What one tick concluded: what to do, what it decided not to do, and where
/// Briv's stacks stand. The second half is not decoration - a run that does
/// nothing for an hour should be able to say which threshold it is waiting on.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Decision {
    pub actions: Vec<Action>,
    pub holds: Vec<String>,
    pub stacks: Option<Stacks>,
}

impl Decision {
    fn hold(&mut self, reason: impl Into<String>) {
        self.holds.push(reason.into());
    }

    pub fn is_idle(&self) -> bool {
        self.actions.is_empty()
    }

    /// Has Briv reached the stack thresholds the profile asked for?
    pub fn stacked(&self) -> bool {
        self.stacks.is_some_and(|stacks| stacks.enough())
    }

    /// One line for a status display: what happened, or what we are waiting on.
    pub fn summary(&self) -> String {
        if !self.actions.is_empty() {
            return self.actions.iter().map(Action::to_string).collect::<Vec<_>>().join("; ");
        }
        match self.holds.first() {
            Some(first) => first.clone(),
            None => "nothing to do".to_string(),
        }
    }
}

/// What has happened so far. Kept outside `decide` so the decision itself has
/// no memory of its own to drift.
#[derive(Debug, Default, Clone)]
pub struct State {
    fired: Vec<(i64, SystemTime)>,
    pub ticks: u64,
    pub taken: u64,
}

impl State {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn tick(&mut self) {
        self.ticks += 1;
    }

    /// Remember an action as taken at the moment of the sample it came from.
    pub fn record(&mut self, action: &Action, at: SystemTime) {
        let hero_id = action.hero_id();
        match self.fired.iter_mut().find(|(id, _)| *id == hero_id) {
            Some(entry) => entry.1 = at,
            None => self.fired.push((hero_id, at)),
        }
        self.taken += 1;
    }

    pub fn fired_at(&self, hero_id: i64) -> Option<SystemTime> {
        self.fired.iter().find(|(id, _)| *id == hero_id).map(|(_, at)| *at)
    }

    /// How long ago we last acted on this champion. `None` if never - and
    /// also if the clock has gone backwards, which must not read as "just
    /// now" or the hold would last forever.
    pub fn since_fired(&self, hero_id: i64, now: SystemTime) -> Option<Duration> {
        self.fired_at(hero_id).and_then(|at| now.duration_since(at).ok())
    }
}

pub fn decide(profile: &Profile, state: &State, snapshot: &Snapshot) -> Decision {
    let mut decision = Decision::default();

    if profile.guards.require_full_reads && !snapshot.healthy() {
        decision.hold(format!("incomplete sample: {} did not read", snapshot.misses.join(", ")));
        return decision;
    }
    if profile.guards.skip_while_transitioning {
        match snapshot.transitioning {
            Some(true) => {
                decision.hold("moving between zones");
                return decision;
            }
            // A guard that cannot be checked is not a guard that passed.
            None => {
                decision.hold("cannot tell whether we are moving between zones");
                return decision;
            }
            Some(false) => {}
        }
    }

    if let Some(deck) = &profile.deck {
        let want = deck.want_name().to_lowercase();
        match &snapshot.cards {
            None => decision.hold("the deck is not being sampled"),
            Some(hand) => {
                let held = hand.len() as i64;
                let have = hand.iter().filter(|card| **card == deck.want).count() as i64;
                let draws = (deck.hand_size - held).max(0);

                if have >= deck.want_at_least {
                    // Done, and the cards are the bonus: leave them on the
                    // table.
                    decision.hold(format!(
                        "{have} {want} card(s) held, which is the {} we wanted",
                        deck.want_at_least
                    ));
                } else if deck.still_reachable(held, have) {
                    decision.hold(format!(
                        "{have} {want} card(s) in a hand of {held}, {draws} draw(s) left to reach {}",
                        deck.want_at_least
                    ));
                } else if let Some(reason) = zone_gate(deck.zone_at_least, snapshot.zone) {
                    // Hopeless, but not here: parked low to farm stacks, the
                    // gem find a short hand still carries is worth more than
                    // the cooldown spent chasing a better one.
                    decision.hold(reason);
                } else {
                    consider(
                        &mut decision,
                        state,
                        snapshot,
                        profile.ultimates.recast,
                        &deck.champion,
                        format!(
                            "{have} {want} card(s) in a hand of {held} with {draws} draw(s) left: \
                             {} is out of reach, so discard and redraw",
                            deck.want_at_least
                        ),
                    );
                }
            }
        }
    }

    // After the deck, because `Decision::summary` shows the first hold and
    // the stack gate is background: it reports, it never blocks.
    if let Some(briv) = &profile.briv {
        match (snapshot.steelbones, snapshot.sprint) {
            (Some(steelbones), Some(sprint)) => {
                let stacks = Stacks {
                    steelbones,
                    sprint,
                    steelbones_needed: briv.steelbones_needed(),
                    sprint_target: briv.sprint_at_least,
                    feat: briv.conversion_feat,
                };
                if !stacks.enough() {
                    decision.hold(stacks.progress());
                }
                decision.stacks = Some(stacks);
            }
            _ => decision.hold("briv's stack counts did not read"),
        }
    }

    for champion in &profile.ultimates.fire_when_ready {
        consider(
            &mut decision,
            state,
            snapshot,
            profile.ultimates.recast,
            champion,
            "ultimate is ready".to_string(),
        );
    }

    decision
}

/// Why the zone forbids pressing anything, if it does.
fn zone_gate(at_least: i64, zone: Option<i64>) -> Option<String> {
    if at_least <= 0 {
        return None;
    }
    match zone {
        Some(zone) if zone >= at_least => None,
        Some(zone) => Some(format!("in zone {zone}, holding the hand until zone {at_least}")),
        None => Some(format!("zone did not read, so the zone {at_least} gate cannot be checked")),
    }
}

/// Fire this champion's ultimate, or record why not.
fn consider(
    decision: &mut Decision,
    state: &State,
    snapshot: &Snapshot,
    recast: Duration,
    champion: &Champion,
    why: String,
) {
    let Some(hero_id) = champion.id else {
        decision.hold(format!("{champion} has not been matched to a champion yet"));
        return;
    };
    if decision.actions.iter().any(|action| action.hero_id() == hero_id) {
        return;
    }
    let Some(entry) = snapshot.ultimate(hero_id) else {
        decision.hold(format!("{champion} is not in the ultimates bar"));
        return;
    };
    if entry.queued {
        decision.hold(format!("{champion}'s ultimate is already queued"));
        return;
    }
    if !entry.ready() {
        decision.hold(format!("{champion}'s ultimate is {:.0}s from ready", entry.cooldown));
        return;
    }
    if let Some(since) = state.since_fired(hero_id, snapshot.at)
        && since < recast
    {
        decision.hold(format!(
            "fired {champion} {:.1}s ago, giving the game {:.1}s to catch up",
            since.as_secs_f64(),
            recast.as_secs_f64()
        ));
        return;
    }
    if input::hotkey(entry.hotkey).is_none() {
        decision.hold(format!(
            "{champion} is on hotkey {}, which this program cannot send",
            entry.hotkey
        ));
        return;
    }
    decision.actions.push(Action::Ultimate {
        slot: entry.slot,
        hero_id,
        hotkey: entry.hotkey,
        champion: champion.to_string(),
        why,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::Ultimate;
    use crate::offsets::effects::{GEM_CARD, MOON_CARD};
    use crate::profile::{Briv, Deck};

    const ELLYWICK: i64 = 83;
    const BRIV: i64 = 58;
    const KNIGHT: i64 = 1;

    /// Just the deck rule, so its holds are the only ones in the list.
    fn deck_profile() -> Profile {
        let mut profile = Profile::default();
        profile.deck = Some(Deck {
            champion: Champion::id(ELLYWICK),
            hand_size: 5,
            want: GEM_CARD,
            want_at_least: 3,
            zone_at_least: 0,
        });
        profile.briv = None;
        profile
    }

    /// Just the stack gate.
    fn briv_profile() -> Profile {
        let mut profile = Profile::default();
        profile.deck = None;
        profile.briv = Some(Briv {
            champion: Champion::id(BRIV),
            sprint_at_least: 1000,
            conversion_feat: false,
        });
        profile
    }

    fn at(seconds: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
    }

    fn snapshot(cards: Vec<i64>) -> Snapshot {
        Snapshot {
            at: at(1000),
            took: Duration::from_millis(12),
            reads: 31,
            pointer_reads: 20,
            zone: Some(270),
            highest_zone: Some(86),
            quest_left: Some(4),
            transitioning: Some(false),
            modron_target: Some(546),
            steelbones: Some(1158),
            sprint: Some(25),
            resets: Some(3992),
            gems: Some(13478),
            since_hit: Some(4.5),
            ultimates: vec![
                Ultimate { slot: 3, hero_id: BRIV, hotkey: 4, cooldown: -3.0, queued: false },
                Ultimate { slot: 7, hero_id: ELLYWICK, hotkey: 8, cooldown: -1.0, queued: false },
            ],
            cards: Some(cards),
            misses: Vec::new(),
        }
    }

    // --- the deck ----------------------------------------------------------

    #[test]
    fn a_fresh_hand_is_never_discarded() {
        // The ultimate throws the hand away, so pressing it on an empty hand
        // (or on the first guaranteed gem card) wastes a three-minute cooldown
        // and collects nothing.
        for hand in [vec![], vec![GEM_CARD], vec![GEM_CARD, KNIGHT, MOON_CARD]] {
            let held = hand.len();
            let decision = decide(&deck_profile(), &State::new(), &snapshot(hand));
            assert!(decision.is_idle(), "discarded a hand of {held}: {:?}", decision.actions);
        }
    }

    #[test]
    fn a_hopeless_hand_is_discarded() {
        // Four cards, one gem: even a gem as the fifth card only makes two,
        // and we want three. Redraw.
        let decision =
            decide(&deck_profile(), &State::new(), &snapshot(vec![GEM_CARD, KNIGHT, KNIGHT, MOON_CARD]));
        assert_eq!(decision.actions.len(), 1);
        let Action::Ultimate { hero_id, hotkey, slot, .. } = &decision.actions[0];
        assert_eq!((*hero_id, *hotkey, *slot), (ELLYWICK, 8, 7));
        assert_eq!(
            decision.actions[0].why(),
            "1 gem card(s) in a hand of 4 with 1 draw(s) left: 3 is out of reach, so discard and redraw"
        );
    }

    #[test]
    fn two_gems_in_four_wait_for_the_fifth_card() {
        let hand = vec![GEM_CARD, GEM_CARD, KNIGHT, MOON_CARD];
        let decision = decide(&deck_profile(), &State::new(), &snapshot(hand));
        assert!(decision.is_idle());
        assert_eq!(decision.holds, ["2 gem card(s) in a hand of 4, 1 draw(s) left to reach 3"]);
    }

    #[test]
    fn a_fifth_card_that_misses_is_a_redraw() {
        let hand = vec![GEM_CARD, GEM_CARD, KNIGHT, MOON_CARD, KNIGHT];
        let decision = decide(&deck_profile(), &State::new(), &snapshot(hand));
        assert_eq!(decision.actions.len(), 1, "a full hand of 2 gems cannot improve");
    }

    #[test]
    fn a_finished_hand_is_left_alone_for_good() {
        let hand = vec![GEM_CARD, GEM_CARD, GEM_CARD, KNIGHT, MOON_CARD];
        let decision = decide(&deck_profile(), &State::new(), &snapshot(hand));
        assert!(decision.is_idle());
        assert_eq!(decision.holds, ["3 gem card(s) held, which is the 3 we wanted"]);
    }

    #[test]
    fn a_gold_find_profile_wants_a_full_hand_of_moons() {
        let mut profile = deck_profile();
        let deck = profile.deck.as_mut().unwrap();
        deck.want = MOON_CARD;
        deck.want_at_least = 5;

        // One miss in the first four cards already makes five moons impossible.
        let decision = decide(&profile, &State::new(), &snapshot(vec![MOON_CARD, MOON_CARD, GEM_CARD]));
        assert_eq!(decision.actions.len(), 1, "{:?}", decision.holds);

        // An all-moon hand of three is still on course.
        let decision =
            decide(&profile, &State::new(), &snapshot(vec![MOON_CARD, MOON_CARD, MOON_CARD]));
        assert!(decision.is_idle());

        // And a full moon hand is kept.
        let decision = decide(&profile, &State::new(), &snapshot(vec![MOON_CARD; 5]));
        assert!(decision.is_idle());
        assert_eq!(decision.holds, ["5 moon card(s) held, which is the 5 we wanted"]);
    }

    #[test]
    fn an_untracked_deck_is_not_an_empty_hand() {
        let mut sample = snapshot(vec![GEM_CARD]);
        sample.cards = None;
        let decision = decide(&deck_profile(), &State::new(), &sample);
        assert!(decision.is_idle());
        assert_eq!(decision.holds, ["the deck is not being sampled"]);
    }

    /// The regression this whole layer exists for. The game does not show the
    /// new cooldown immediately, so the next sample still says "ready" with
    /// the same cards in hand - and a decision with no memory fires again,
    /// and again, every tick.
    #[test]
    fn does_not_fire_again_before_the_game_catches_up() {
        let profile = deck_profile();
        let mut state = State::new();
        let hopeless = vec![GEM_CARD, KNIGHT, KNIGHT, MOON_CARD];

        let first = snapshot(hopeless.clone());
        let decision = decide(&profile, &state, &first);
        assert_eq!(decision.actions.len(), 1, "the first tick fires");
        state.record(&decision.actions[0], first.at);

        // 300ms later: the hand and the cooldown both still read exactly as
        // they did, because the client has not processed the key yet.
        let mut stale = snapshot(hopeless.clone());
        stale.at = first.at + Duration::from_millis(300);
        let decision = decide(&profile, &state, &stale);
        assert!(decision.is_idle(), "fired twice: {:?}", decision.actions);
        assert!(decision.holds[0].starts_with("fired #83 0.3s ago"), "{:?}", decision.holds);

        // Still held at the edge of the window...
        let mut edge = snapshot(hopeless.clone());
        edge.at = first.at + profile.ultimates.recast - Duration::from_millis(1);
        assert!(decide(&profile, &state, &edge).is_idle());

        // ...and allowed again once it has passed, since a hand still holding
        // the same hopeless cards means the first press did not land.
        let mut after = snapshot(hopeless);
        after.at = first.at + profile.ultimates.recast;
        assert_eq!(decide(&profile, &state, &after).actions.len(), 1);
    }

    #[test]
    fn one_press_per_hand_without_needing_a_timer() {
        // The press that lands empties the hand, and an empty hand can reach
        // the target again - so the rule itself stops at one ultimate per
        // hand, with the recast window only covering the lag.
        let profile = deck_profile();
        let mut state = State::new();
        let first = snapshot(vec![GEM_CARD, KNIGHT, KNIGHT, MOON_CARD]);
        let decision = decide(&profile, &state, &first);
        state.record(&decision.actions[0], first.at);

        let mut discarded = snapshot(vec![]);
        discarded.at = first.at + Duration::from_secs(60);
        let decision = decide(&profile, &state, &discarded);
        assert!(decision.is_idle(), "pressed again on the fresh hand");
    }

    // --- briv's stacks -----------------------------------------------------

    #[test]
    fn reports_progress_towards_the_sprint_target() {
        let mut sample = snapshot(vec![]);
        sample.steelbones = Some(700);
        sample.sprint = Some(12);
        let decision = decide(&briv_profile(), &State::new(), &sample);
        assert!(!decision.stacked());
        assert_eq!(
            decision.holds,
            ["briv: 700/1000 steelbones -> 700 of 1000 sprint next run (1:1), 12 sprint now"]
        );
        assert_eq!(decision.stacks.unwrap().steelbones_needed, 1000);
    }

    #[test]
    fn the_conversion_feat_lowers_the_stacks_to_tank_for() {
        // 1 Steelbones becomes 1.2 Sprint, so 1000 Sprint next run only needs
        // 834 Steelbones tanked in this one.
        let mut profile = briv_profile();
        profile.briv.as_mut().unwrap().conversion_feat = true;

        let mut sample = snapshot(vec![]);
        sample.steelbones = Some(834);
        sample.sprint = Some(3);
        let decision = decide(&profile, &State::new(), &sample);
        assert!(decision.stacked(), "834 x 1.2 clears 1000: {:?}", decision.holds);
        assert_eq!(decision.stacks.unwrap().converts_to(), 1000);

        sample.steelbones = Some(833);
        let decision = decide(&profile, &State::new(), &sample);
        assert!(!decision.stacked(), "833 x 1.2 = 999.6, just short");
        assert_eq!(
            decision.holds,
            ["briv: 833/834 steelbones -> 999 of 1000 sprint next run (x1.2), 3 sprint now"]
        );
    }

    #[test]
    fn sprint_stacks_in_hand_are_reported_not_required() {
        // The run spends Sprint stacks as it skips; being out of them is not a
        // reason to say this run has not tanked enough.
        let mut sample = snapshot(vec![]);
        sample.steelbones = Some(1000);
        sample.sprint = Some(0);
        let decision = decide(&briv_profile(), &State::new(), &sample);
        assert!(decision.stacked());
        assert_eq!(decision.stacks.unwrap().sprint, 0);
    }

    #[test]
    fn the_stack_gate_presses_nothing() {
        // Deliberate: the thresholds gate the zone-skip cycle, which does not
        // exist yet. Reaching them must not become a keystroke by accident.
        let mut sample = snapshot(vec![]);
        sample.steelbones = Some(100_000);
        sample.sprint = Some(100);
        let decision = decide(&briv_profile(), &State::new(), &sample);
        assert!(decision.stacked());
        assert!(decision.actions.is_empty());
    }

    #[test]
    fn unreadable_stacks_are_not_zero_stacks() {
        let mut sample = snapshot(vec![]);
        sample.steelbones = None;
        let decision = decide(&briv_profile(), &State::new(), &sample);
        assert!(!decision.stacked());
        assert_eq!(decision.stacks, None);
        assert_eq!(decision.holds, ["briv's stack counts did not read"]);
    }

    // --- the zone gate -----------------------------------------------------

    #[test]
    fn a_hopeless_hand_is_kept_below_the_zone_gate() {
        let mut profile = deck_profile();
        profile.deck.as_mut().unwrap().zone_at_least = 250;

        let hopeless = vec![GEM_CARD, KNIGHT, KNIGHT, MOON_CARD];
        let mut sample = snapshot(hopeless.clone());
        sample.zone = Some(120);
        let decision = decide(&profile, &State::new(), &sample);
        assert!(decision.is_idle(), "redrew while parked low");
        assert_eq!(decision.holds, ["in zone 120, holding the hand until zone 250"]);

        // At the gate it is allowed, and past it too.
        sample.zone = Some(250);
        assert_eq!(decide(&profile, &State::new(), &sample).actions.len(), 1);
        sample.zone = Some(999);
        assert_eq!(decide(&profile, &State::new(), &sample).actions.len(), 1);
    }

    #[test]
    fn zero_turns_the_zone_gate_off() {
        let mut sample = snapshot(vec![GEM_CARD, KNIGHT, KNIGHT, MOON_CARD]);
        sample.zone = Some(1);
        assert_eq!(decide(&deck_profile(), &State::new(), &sample).actions.len(), 1);
    }

    #[test]
    fn an_unreadable_zone_blocks_the_gate_it_cannot_check() {
        let mut profile = deck_profile();
        profile.deck.as_mut().unwrap().zone_at_least = 250;
        profile.guards.require_full_reads = false;

        let mut sample = snapshot(vec![GEM_CARD, KNIGHT, KNIGHT, MOON_CARD]);
        sample.zone = None;
        sample.misses.push("zone".to_string());
        let decision = decide(&profile, &State::new(), &sample);
        assert!(decision.is_idle());
        assert_eq!(decision.holds, ["zone did not read, so the zone 250 gate cannot be checked"]);
    }

    // --- everything else ---------------------------------------------------

    #[test]
    fn the_real_cooldown_holds_the_deck_too() {
        let mut sample = snapshot(vec![GEM_CARD, KNIGHT, KNIGHT, MOON_CARD]);
        sample.ultimates[1].cooldown = 174.0;
        let decision = decide(&deck_profile(), &State::new(), &sample);
        assert!(decision.is_idle());
        assert_eq!(decision.holds, ["#83's ultimate is 174s from ready"]);
    }

    #[test]
    fn a_queued_ultimate_is_left_alone() {
        let mut sample = snapshot(vec![GEM_CARD, KNIGHT, KNIGHT, MOON_CARD]);
        sample.ultimates[1].queued = true;
        let decision = decide(&deck_profile(), &State::new(), &sample);
        assert!(decision.is_idle());
        assert_eq!(decision.holds, ["#83's ultimate is already queued"]);
    }

    #[test]
    fn a_champion_out_of_the_formation_is_a_hold_not_a_crash() {
        let mut sample = snapshot(vec![GEM_CARD, KNIGHT, KNIGHT, MOON_CARD]);
        sample.ultimates.retain(|entry| entry.hero_id != ELLYWICK);
        let decision = decide(&deck_profile(), &State::new(), &sample);
        assert_eq!(decision.holds, ["#83 is not in the ultimates bar"]);
    }

    #[test]
    fn an_unresolved_name_never_becomes_an_action() {
        let mut profile = deck_profile();
        profile.deck.as_mut().unwrap().champion = Champion::named("ellywick");
        let sample = snapshot(vec![GEM_CARD, KNIGHT, KNIGHT, MOON_CARD]);
        let decision = decide(&profile, &State::new(), &sample);
        assert!(decision.is_idle());
        assert_eq!(decision.holds, ["ellywick has not been matched to a champion yet"]);
    }

    #[test]
    fn a_hotkey_we_cannot_send_is_a_hold() {
        let mut sample = snapshot(vec![GEM_CARD, KNIGHT, KNIGHT, MOON_CARD]);
        sample.ultimates[1].hotkey = 11;
        let decision = decide(&deck_profile(), &State::new(), &sample);
        assert_eq!(decision.holds, ["#83 is on hotkey 11, which this program cannot send"]);
    }

    #[test]
    fn fire_when_ready_covers_other_champions() {
        let mut profile = deck_profile();
        profile.deck = None;
        profile.ultimates.fire_when_ready = vec![Champion::id(BRIV)];
        let decision = decide(&profile, &State::new(), &snapshot(vec![]));
        assert_eq!(decision.actions.len(), 1);
        assert_eq!(decision.actions[0].hero_id(), BRIV);
    }

    #[test]
    fn one_champion_named_twice_acts_once() {
        let mut profile = deck_profile();
        profile.ultimates.fire_when_ready = vec![Champion::id(ELLYWICK), Champion::id(ELLYWICK)];
        let decision = decide(&profile, &State::new(), &snapshot(vec![GEM_CARD]));
        assert_eq!(decision.actions.len(), 1, "{:?}", decision.actions);
    }

    #[test]
    fn guards_stop_everything() {
        let mut sample = snapshot(vec![GEM_CARD, KNIGHT, KNIGHT, MOON_CARD]);
        sample.transitioning = Some(true);
        let decision = decide(&deck_profile(), &State::new(), &sample);
        assert!(decision.is_idle());
        assert_eq!(decision.holds, ["moving between zones"]);

        // An unreadable guard is not a satisfied guard.
        sample.transitioning = None;
        sample.misses.push("transitioning".to_string());
        let mut profile = deck_profile();
        profile.guards.require_full_reads = false;
        let decision = decide(&profile, &State::new(), &sample);
        assert_eq!(decision.holds, ["cannot tell whether we are moving between zones"]);
    }

    #[test]
    fn an_incomplete_sample_is_not_acted_on() {
        let mut sample = snapshot(vec![GEM_CARD, KNIGHT, KNIGHT, MOON_CARD]);
        sample.misses.push("zone".to_string());
        let decision = decide(&deck_profile(), &State::new(), &sample);
        assert!(decision.is_idle());
        assert_eq!(decision.holds, ["incomplete sample: zone did not read"]);

        // Unless the profile says to go ahead anyway.
        let mut profile = deck_profile();
        profile.guards.require_full_reads = false;
        assert_eq!(decide(&profile, &State::new(), &sample).actions.len(), 1);
    }

    #[test]
    fn state_counts_what_it_was_told() {
        let mut state = State::new();
        let sample = snapshot(vec![GEM_CARD, KNIGHT, KNIGHT, MOON_CARD]);
        let decision = decide(&deck_profile(), &state, &sample);
        state.tick();
        state.record(&decision.actions[0], sample.at);
        assert_eq!((state.ticks, state.taken), (1, 1));
        assert_eq!(state.fired_at(ELLYWICK), Some(sample.at));
        assert_eq!(state.fired_at(BRIV), None);
        // A clock that jumps backwards must not read as "just fired".
        assert_eq!(state.since_fired(ELLYWICK, at(0)), None);
    }

    #[test]
    fn the_summary_says_what_happened_or_what_it_waits_on() {
        let hopeless = snapshot(vec![GEM_CARD, KNIGHT, KNIGHT, MOON_CARD]);
        let decision = decide(&deck_profile(), &State::new(), &hopeless);
        assert!(decision.summary().starts_with("ultimate: #83 (slot 7, key 8) - 1 gem card(s)"));

        let decision = decide(&deck_profile(), &State::new(), &snapshot(vec![GEM_CARD]));
        assert_eq!(
            decision.summary(),
            "1 gem card(s) in a hand of 1, 4 draw(s) left to reach 3"
        );
    }
}
