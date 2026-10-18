//! The loop: sample, decide, (eventually) act, report.
//!
//! Everything interesting has already happened by the time control reaches
//! here - `snapshot` reads the game, `policy` decides - so this is deliberately
//! thin: it owns the clock, the running totals, and nothing else. Keeping it
//! thin is what makes a dry run meaningful, because the only difference
//! between a dry run and an armed one will be a single call.
//!
//! Dry runs still record their decisions in `State`. That matters: the recast
//! window is what stops a decision repeating before the game shows it, so a
//! dry run that skipped recording would print a press every tick and tell you
//! nothing about the cadence a real run would have.

use std::time::Duration;

use crate::policy::{Decision, State};
use crate::snapshot::Snapshot;

#[cfg(windows)]
use std::io;
#[cfg(windows)]
use crate::game::Game;
#[cfg(windows)]
use crate::policy;
#[cfg(windows)]
use crate::profile::Profile;
#[cfg(windows)]
use crate::snapshot::Sampler;

/// One pass of the loop.
#[derive(Debug, Clone)]
pub struct Tick {
    pub snapshot: Snapshot,
    pub decision: Decision,
    /// Actions actually sent to the game. Empty on a dry run, which is every
    /// run today.
    pub sent: usize,
}

impl Tick {
    /// One JSON object per tick, for logging a run to a file.
    pub fn to_json(&self) -> String {
        let actions: Vec<String> = self
            .decision
            .actions
            .iter()
            .map(|action| format!("{:?}", action.to_string()))
            .collect();
        let holds: Vec<String> = self.decision.holds.iter().map(|hold| format!("{hold:?}")).collect();
        let stacked = self.decision.stacked();
        format!(
            "{{\"sample\":{},\"actions\":[{}],\"sent\":{},\"holds\":[{}],\"stacked\":{stacked}}}",
            self.snapshot.to_json(),
            actions.join(","),
            self.sent,
            holds.join(",")
        )
    }
}

/// Running totals for a run. Pure arithmetic over what each tick produced, so
/// what the footer claims can be checked without a game.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Stats {
    pub ticks: u64,
    /// Actions decided on, whether or not they were sent.
    pub decided: u64,
    pub sent: u64,
    pub reads: u64,
    pub sampling: Duration,
    pub worst_sample: Duration,
    /// Ticks whose sample was missing at least one value.
    pub incomplete: u64,
    /// Ticks that did nothing at all.
    pub idle: u64,
}

impl Stats {
    pub fn note(&mut self, tick: &Tick) {
        self.ticks += 1;
        self.decided += tick.decision.actions.len() as u64;
        self.sent += tick.sent as u64;
        self.reads += tick.snapshot.reads as u64;
        self.sampling += tick.snapshot.took;
        self.worst_sample = self.worst_sample.max(tick.snapshot.took);
        if !tick.snapshot.healthy() {
            self.incomplete += 1;
        }
        if tick.decision.is_idle() {
            self.idle += 1;
        }
    }

    pub fn average_sample(&self) -> Duration {
        self.sampling.checked_div(self.ticks.max(1) as u32).unwrap_or_default()
    }

    pub fn average_reads(&self) -> f64 {
        if self.ticks == 0 {
            return 0.0;
        }
        self.reads as f64 / self.ticks as f64
    }
}

/// An attached game, a profile, and the memory of what has been done to it.
#[cfg(windows)]
pub struct Runtime {
    game: Game,
    sampler: Sampler,
    profile: Profile,
    state: State,
    stats: Stats,
}

#[cfg(windows)]
impl Runtime {
    /// Match the profile's champions against the live roster and resolve every
    /// chain a sample needs. Both are startup costs - hundreds of reads - and
    /// both are paid once.
    pub fn start(game: Game, mut profile: Profile) -> io::Result<Self> {
        let roster = game.roster()?;
        profile.resolve(&roster).map_err(|err| io::Error::other(err.to_string()))?;
        let sampler = Sampler::new(&game)?;
        Ok(Self { game, sampler, profile, state: State::new(), stats: Stats::default() })
    }

    pub fn profile(&self) -> &Profile {
        &self.profile
    }

    pub fn game(&self) -> &Game {
        &self.game
    }

    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    /// What the sampler could not set up, for a one-time startup warning.
    pub fn notes(&self) -> &[String] {
        &self.sampler.notes
    }

    pub fn tick(&mut self) -> Tick {
        let snapshot = self.sampler.take(&self.game);
        let decision = policy::decide(&self.profile, &self.state, &snapshot);

        self.state.tick();
        for action in &decision.actions {
            self.state.record(action, snapshot.at);
        }

        let tick = Tick { snapshot, decision, sent: 0 };
        self.stats.note(&tick);
        tick
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::Action;
    use std::time::SystemTime;

    fn tick(reads: usize, took_ms: u64, actions: usize, misses: &[&str]) -> Tick {
        let snapshot = Snapshot {
            at: SystemTime::UNIX_EPOCH,
            took: Duration::from_millis(took_ms),
            reads,
            pointer_reads: reads / 2,
            zone: Some(270),
            highest_zone: Some(86),
            quest_left: Some(4),
            transitioning: Some(false),
            modron_target: Some(546),
            steelbones: Some(700),
            sprint: Some(25),
            resets: Some(3992),
            gems: Some(13478),
            since_hit: Some(4.5),
            ultimates: Vec::new(),
            cards: Some(vec![3, 1]),
            misses: misses.iter().map(|miss| miss.to_string()).collect(),
        };
        let decision = Decision {
            actions: (0..actions)
                .map(|slot| Action::Ultimate {
                    slot: slot as u64,
                    hero_id: 83,
                    hotkey: 8,
                    champion: "#83".to_string(),
                    why: "test".to_string(),
                })
                .collect(),
            holds: Vec::new(),
            stacks: None,
        };
        Tick { snapshot, decision, sent: 0 }
    }

    #[test]
    fn totals_add_up_over_a_run() {
        let mut stats = Stats::default();
        stats.note(&tick(111, 2, 1, &[]));
        stats.note(&tick(109, 6, 0, &[]));
        stats.note(&tick(80, 4, 0, &["zone"]));

        assert_eq!(stats.ticks, 3);
        assert_eq!(stats.decided, 1);
        assert_eq!(stats.sent, 0, "a dry run sends nothing");
        assert_eq!(stats.idle, 2);
        assert_eq!(stats.incomplete, 1);
        assert_eq!(stats.reads, 300);
        assert_eq!(stats.average_reads(), 100.0);
        assert_eq!(stats.average_sample(), Duration::from_millis(4));
        assert_eq!(stats.worst_sample, Duration::from_millis(6), "the worst is kept, not averaged");
    }

    #[test]
    fn an_empty_run_divides_by_nothing() {
        let stats = Stats::default();
        assert_eq!(stats.average_reads(), 0.0);
        assert_eq!(stats.average_sample(), Duration::ZERO);
    }

    #[test]
    fn a_tick_logs_as_one_json_object() {
        let json = tick(111, 2, 1, &[]).to_json();
        assert!(json.contains("\"sample\":{\"at\":"), "{json}");
        assert!(json.contains("\"reads\":111"), "{json}");
        assert!(json.contains("\"sent\":0"), "{json}");
        assert!(json.contains("ultimate: #83 (slot 0, key 8) - test"), "{json}");
    }
}
