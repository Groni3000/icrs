//! An attached game: the process, the window, and the loaded offsets.
//!
//! Everything that knows how to pull a value out of the running client lives
//! here, so the command line stays presentation only and `snapshot` can sample
//! the game without going through it.

// Reading another process and posting keystrokes into its window are both
// Win32; the data shapes below are not, so they stay available to tests on any
// host (see `crate::mem`).
#[cfg(windows)]
use std::io;
#[cfg(windows)]
use std::path::{Path, PathBuf};

#[cfg(windows)]
use crate::catalog::{Catalog, Chain};
#[cfg(windows)]
use crate::mem::Process;
#[cfg(windows)]
use crate::offsets::Kind;
#[cfg(windows)]
use crate::value::Value;
#[cfg(windows)]
use crate::{input, offsets, process, version};

/// One entry in the ultimates bar.
#[derive(Debug, Clone, PartialEq)]
pub struct Ultimate {
    pub slot: u64,
    pub hero_id: i64,
    pub hotkey: i64,
    /// Seconds remaining; zero or less means it can be fired.
    pub cooldown: f64,
    pub queued: bool,
}

impl Ultimate {
    pub fn ready(&self) -> bool {
        self.cooldown <= 0.0
    }
}

#[cfg(windows)]
pub struct Game {
    pid: u32,
    exe: PathBuf,
    installed_version: io::Result<String>,
    process: Process,
    module: u64,
    window: input::Window,
    catalog: Catalog,
}

#[cfg(windows)]
impl Game {
    /// Attach to a running client.
    pub fn open(pid: u32, catalog: Catalog) -> io::Result<Self> {
        let exe = process::exe_path(pid)?;
        let installed_version = version::installed(&exe);
        let process = Process::open(pid)?;
        let module = process.module_base(offsets::MONO_MODULE)?;
        let window = input::Window::for_process(pid)?;
        Ok(Self { pid, exe, installed_version, process, module, window, catalog })
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn exe(&self) -> &Path {
        &self.exe
    }

    /// The version of the files on disk, which is the only place it can be
    /// read from - see `crate::version`.
    pub fn installed_version(&self) -> Result<&str, &io::Error> {
        self.installed_version.as_deref()
    }

    pub fn module(&self) -> u64 {
        self.module
    }

    pub fn process(&self) -> &Process {
        &self.process
    }

    pub fn window(&self) -> &input::Window {
        &self.window
    }

    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// Resolve a path into a reusable chain. Worth doing once and keeping:
    /// resolving walks the import tree, reading does not.
    pub fn chain(&self, path: &str, indices: &[u64]) -> io::Result<Chain> {
        self.catalog
            .chain(path, path, indices)
            .map_err(|err| io::Error::other(err.to_string()))
    }

    /// Follow a chain and read what it lands on.
    pub fn read(&self, chain: &Chain) -> io::Result<Value> {
        let address = self.process.resolve(chain.base(self.module), &chain.offsets)?;
        match chain.kind {
            Kind::I32 => self.process.read_i32(address).map(|v| Value::Int(v as i64)),
            Kind::I64 => self.process.read_i64(address).map(Value::Int),
            Kind::U8 => self.process.read_u8(address).map(|v| Value::Int(v as i64)),
            Kind::Quad => self.process.read_quad(address).map(Value::Big),
            Kind::F32 => self.process.read_f32(address).map(|v| Value::Float(v as f64)),
            Kind::F64 => self.process.read_f64(address).map(Value::Float),
            Kind::Utf16 => self.process.read_mono_string(address).map(Value::Text),
        }
    }

    /// Resolve and read in one go. Convenient for one-off reads; for anything
    /// in a loop, keep the `Chain`.
    pub fn read_path(&self, path: &str, indices: &[u64]) -> io::Result<Value> {
        self.read(&self.chain(path, indices)?)
    }

    fn read_int(&self, path: &str, indices: &[u64]) -> io::Result<i64> {
        self.read_path(path, indices)?
            .as_i64()
            .ok_or_else(|| io::Error::other(format!("{path} did not read as a number")))
    }

    /// Champion ids and names, in hero-list order, so the position in this
    /// list is the index every per-champion path wants.
    pub fn roster(&self) -> io::Result<Vec<(i64, String)>> {
        let size = self.read_int(offsets::heroes::SIZE, &[])?;

        let mut roster = Vec::new();
        for index in 0..size.clamp(0, 1024) as u64 {
            let at = [0, index];
            let (Ok(id), Ok(name)) = (
                self.read_path(offsets::heroes::ID, &at),
                self.read_path(offsets::heroes::NAME, &at),
            ) else {
                continue;
            };
            if let (Some(id), Some(name)) = (id.as_i64(), name.as_text()) {
                roster.push((id, name.to_string()));
            }
        }
        Ok(roster)
    }

    /// Where a champion sits in the hero list.
    pub fn hero_index(&self, hero_id: i64) -> io::Result<u64> {
        let size = self.read_int(offsets::heroes::SIZE, &[])?;
        for index in 0..size.clamp(0, 1024) as u64 {
            if self.read_int(offsets::heroes::ID, &[0, index]).ok() == Some(hero_id) {
                return Ok(index);
            }
        }
        Err(io::Error::other(format!("no champion with id {hero_id} in the hero list")))
    }

    /// A champion id from an id or a name.
    pub fn hero_id(&self, who: &str) -> io::Result<i64> {
        if let Ok(id) = who.parse::<i64>() {
            return Ok(id);
        }
        let wanted = who.to_lowercase();
        let roster = self.roster()?;
        roster
            .iter()
            .find(|(_, name)| name.to_lowercase() == wanted)
            .or_else(|| roster.iter().find(|(_, name)| name.to_lowercase().contains(&wanted)))
            .map(|(id, _)| *id)
            .ok_or_else(|| io::Error::other(format!("no champion matching {who:?}")))
    }

    /// Every effect key on a champion, with the dictionary entry it sits in.
    pub fn effect_keys(&self, hero: u64) -> io::Result<Vec<(u64, String)>> {
        let count = self.read_int(offsets::effects::COUNT, &[0, hero])?;
        let mut keys = Vec::new();
        for entry in 0..count.clamp(0, 256) as u64 {
            if let Ok(Value::Text(key)) =
                self.read_path(offsets::effects::KEY, &[0, hero, entry, 0])
                && !key.is_empty()
            {
                keys.push((entry, key));
            }
        }
        Ok(keys)
    }

    /// The dictionary entry holding one of `candidates`, in preference order.
    pub fn find_effect(&self, hero: u64, candidates: &[&str]) -> io::Result<u64> {
        let keys = self.effect_keys(hero)?;
        for wanted in candidates {
            if let Some((entry, _)) = keys.iter().find(|(_, key)| key == wanted) {
                return Ok(*entry);
            }
        }
        Err(io::Error::other(format!(
            "none of {candidates:?} among this champion's effect keys ({})",
            keys.iter().map(|(_, key)| key.as_str()).collect::<Vec<_>>().join(", ")
        )))
    }

    /// The card types in hand, in order.
    pub fn cards(&self, hero: u64, entry: u64) -> io::Result<Vec<i64>> {
        let at = [0, hero, entry, 0, 0];
        let size = self.read_int(offsets::effects::CARDS, &at)?;
        if !(0..=16).contains(&size) {
            return Err(io::Error::other(format!("implausible card count: {size}")));
        }
        let mut hand = Vec::new();
        for card in 0..size as u64 {
            let at = [0, hero, entry, 0, 0, card];
            if let Ok(card_type) = self.read_int(offsets::effects::CARD_TYPE, &at) {
                hand.push(card_type);
            }
        }
        Ok(hand)
    }

    /// The ultimates bar. Slots whose fields do not resolve are skipped, since
    /// the bar is sized for the formation, not for what is in it.
    pub fn ultimates(&self) -> io::Result<Vec<Ultimate>> {
        let size = self.read_int(offsets::ultimates::SIZE, &[])?;
        if !(0..=64).contains(&size) {
            return Err(io::Error::other(format!("implausible bar size: {size}")));
        }

        let mut bar = Vec::new();
        for slot in 0..size as u64 {
            let at = [0, slot];
            let (Ok(hero_id), Ok(hotkey)) = (
                self.read_int(offsets::ultimates::HERO_ID, &at),
                self.read_int(offsets::ultimates::HOTKEY, &at),
            ) else {
                continue;
            };
            let Some(cooldown) =
                self.read_path(offsets::ultimates::COOLDOWN, &at).ok().and_then(|v| v.as_f64())
            else {
                continue;
            };
            let queued = self
                .read_path(offsets::ultimates::QUEUED, &at)
                .ok()
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            bar.push(Ultimate { slot, hero_id, hotkey, cooldown, queued });
        }
        Ok(bar)
    }

    /// Press a key in the game's window without focusing it.
    pub fn tap(&self, key: input::Key) -> io::Result<()> {
        self.window.nudge_active()?;
        self.window.tap(key)
    }
}
