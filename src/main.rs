//! `icrs` - read a running Idle Champions client, and send input to it.
//!
//! Every offset comes from the loaded drop (see `icrs::catalog`); this file
//! only decides what to read and how to print it.

use clap::Parser;

use icrs::catalog::{Catalog, Chain};
use icrs::cli::{Cli, Command};
use icrs::mem::Process;
use icrs::offsets::Kind;
use icrs::{input, offsets, process, version};

use std::io::{self, IsTerminal, Write};
use std::process::ExitCode;
use std::time::{Duration, Instant};

fn main() -> ExitCode {
    let cli = Cli::parse();
    // Decided before the work, so a failure still pauses: a double-clicked
    // console window closing on an error message is the worst of both.
    let pause = cli.global.pause || launched_from_explorer();

    let code = match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    };
    if pause {
        wait_for_enter();
    }
    code
}

fn run(cli: &Cli) -> io::Result<()> {
    let catalog = Catalog::discover(cli.global.imports.as_deref())
        .map_err(|err| io::Error::other(err.to_string()))?;

    match cli.command() {
        // The only command that needs neither the game nor a process handle.
        Command::Paths { filter, limit } => {
            let found = catalog.search(filter.as_deref());
            for (path, value_type) in found.iter().take(*limit) {
                println!("{value_type:<8} {path}");
            }
            if found.len() > *limit {
                println!("... {} more (use --limit)", found.len() - *limit);
            } else {
                println!("{} path(s)", found.len());
            }
            Ok(())
        }

        Command::Cards { gems, hero } => {
            let session = Session::attach(cli, &catalog)?;
            let hero_id = hero.unwrap_or(offsets::effects::ELLYWICK_HERO_ID);
            let hero_index = session.hero_index(&catalog, hero_id)?;
            let entry = session.find_effect(&catalog, hero_index, offsets::effects::DECK_KEYS)?;
            let hand = session.cards(&catalog, hero_index, entry)?;
            let gem_count =
                hand.iter().filter(|card| **card == offsets::effects::GEM_CARD).count();

            if *gems {
                println!("{gem_count}");
                return Ok(());
            }
            let named: Vec<&str> =
                hand.iter().map(|card| offsets::effects::card_name(*card)).collect();
            println!("champion #{hero_id}, effect entry {entry}");
            println!("hand        {}", named.join(", "));
            println!("gem cards   {gem_count}");
            Ok(())
        }

        Command::Effects { who } => {
            let session = Session::attach(cli, &catalog)?;
            let hero_id = session.hero_id(&catalog, who)?;
            let hero_index = session.hero_index(&catalog, hero_id)?;
            for (entry, key) in session.effect_keys(&catalog, hero_index)? {
                println!("{entry:>3}  {key}");
            }
            Ok(())
        }

        Command::Labels => {
            for want in offsets::NAMED {
                let mark = if want.probe { "*" } else { " " };
                println!("{mark} {:<14} {}", want.label, want.path);
            }
            for derived in offsets::DERIVED {
                println!(
                    "  {:<14} {} / {} ({})",
                    derived.label, derived.from, derived.divide, derived.why
                );
            }
            println!("\n* = included in `probe`");
            Ok(())
        }

        Command::Ultimates => {
            let session = Session::attach(cli, &catalog)?;
            let bar = session.ultimates(&catalog)?;
            let roster = session.roster(&catalog).unwrap_or_default();
            println!("slot  key  champion                     cooldown");
            for entry in &bar {
                let name = roster
                    .iter()
                    .find(|(id, _)| *id == entry.hero_id)
                    .map(|(_, name)| name.clone())
                    .unwrap_or_else(|| format!("#{}", entry.hero_id));
                let state = if entry.queued {
                    "queued".to_string()
                } else if entry.ready() {
                    "ready".to_string()
                } else {
                    format!("{:.0}s", entry.cooldown)
                };
                println!(
                    "{:>4}  {:>3}  {:<28} {state}",
                    entry.slot,
                    entry.hotkey,
                    format!("{name} (#{})", entry.hero_id)
                );
            }
            Ok(())
        }

        Command::Ult { who, slot, force } => {
            let session = Session::attach(cli, &catalog)?;
            let bar = session.ultimates(&catalog)?;

            let entry = match (who, slot) {
                (_, Some(slot)) => bar.iter().find(|entry| entry.slot == *slot).ok_or_else(|| {
                    io::Error::other(format!("no ultimate in slot {slot}"))
                })?,
                (Some(who), None) => {
                    let hero_id = match who.parse::<i64>() {
                        Ok(id) => id,
                        Err(_) => {
                            let roster = session.roster(&catalog)?;
                            let wanted = who.to_lowercase();
                            roster
                                .iter()
                                .find(|(_, name)| name.to_lowercase() == wanted)
                                .or_else(|| {
                                    roster
                                        .iter()
                                        .find(|(_, name)| name.to_lowercase().contains(&wanted))
                                })
                                .map(|(id, _)| *id)
                                .ok_or_else(|| {
                                    io::Error::other(format!("no champion matching {who:?}"))
                                })?
                        }
                    };
                    bar.iter().find(|entry| entry.hero_id == hero_id).ok_or_else(|| {
                        io::Error::other(format!(
                            "champion {hero_id} has no ultimate in the bar - is it in the formation?"
                        ))
                    })?
                }
                (None, None) => unreachable!("clap requires one of them"),
            };

            if !entry.ready() && !force {
                return Err(io::Error::other(format!(
                    "slot {} is on cooldown for {:.0}s (--force to send anyway)",
                    entry.slot, entry.cooldown
                )));
            }
            let key = input::hotkey(entry.hotkey).ok_or_else(|| {
                io::Error::other(format!("slot {} has hotkey {}, which is not a key this \
                     program knows how to send", entry.slot, entry.hotkey))
            })?;
            session.window.nudge_active()?;
            session.window.tap(key)?;
            println!(
                "fired slot {} (champion #{}) with key {}",
                entry.slot, entry.hero_id, key.name
            );
            Ok(())
        }

        Command::Key { keys, delay } => {
            let pid = find_pid(cli)?;
            let window = input::Window::for_process(pid)?;
            window.nudge_active()?;
            for (position, name) in keys.iter().enumerate() {
                let key = input::key_by_name(name).ok_or_else(|| unknown_key(name))?;
                if position > 0 {
                    std::thread::sleep(Duration::from_millis(*delay));
                }
                window.tap(key)?;
                println!("sent {} (vk {:#04x}, scan {})", key.name, key.vk, key.scan);
            }
            Ok(())
        }

        Command::Info => {
            let session = Session::attach(cli, &catalog)?;
            session.print_header(&catalog);
            println!("\nroots");
            for root in catalog.roots() {
                let base = session.module + root.module_address;
                // Resolving is not enough: the last hop is never dereferenced,
                // so a stale path can "succeed" onto an address that holds
                // nothing. A real Mono object starts with a vtable pointer, so
                // read that as proof of life.
                let state = match session.game.resolve(base, &root.module_offset) {
                    Ok(addr) => match session.game.read_u64(addr) {
                        Ok(vtable) if vtable != 0 => format!("live -> {addr:#x}"),
                        Ok(_) => format!("SUSPECT -> {addr:#x} begins with a null pointer"),
                        Err(err) => format!("DEAD -> {addr:#x} is not readable: {err}"),
                    },
                    Err(err) => format!("DEAD: {err}"),
                };
                println!("  {:<16} mono+{:#x} -> {}", root.name, root.module_address, hops(&root.module_offset));
                println!("  {:<16} {state}", "");
            }
            Ok(())
        }

        Command::Probe => {
            let session = Session::attach(cli, &catalog)?;
            session.print_header(&catalog);

            let mut traced: Vec<String> = Vec::new();
            let mut current_root = String::new();
            for want in offsets::NAMED.iter().filter(|want| want.probe) {
                let target = Target::from(want);
                let chain = match catalog.chain(&target.label, &target.path, &target.indices) {
                    Ok(chain) => chain,
                    Err(err) => {
                        println!("  {:<12} NO CHAIN: {err}", target.label);
                        continue;
                    }
                };
                if chain.root.name != current_root {
                    current_root = chain.root.name.clone();
                    println!(
                        "\n{current_root}  (mono+{:#x} -> {})",
                        chain.root.module_address,
                        hops(&chain.root.module_offset)
                    );
                }
                session.report(&target, &chain, &mut traced);
            }
            Ok(())
        }

        Command::Read { target, index, bare } => {
            let session = Session::attach(cli, &catalog)?;
            let target = Target::parse(target, index);
            let chain = catalog
                .chain(&target.label, &target.path, &target.indices)
                .map_err(|err| io::Error::other(err.to_string()))?;
            let value = target.finish(session.read(&chain)?);
            if *bare {
                println!("{value}");
            } else {
                println!("{:<12} {value}", target.label);
            }
            Ok(())
        }

        Command::Watch { targets, interval, count } => {
            let session = Session::attach(cli, &catalog)?;
            let targets = Target::list(targets);
            let chains: Vec<(Target, io::Result<Chain>)> = targets
                .into_iter()
                .map(|target| {
                    let chain = catalog
                        .chain(&target.label, &target.path, &target.indices)
                        .map_err(|err| io::Error::other(err.to_string()));
                    (target, chain)
                })
                .collect();

            println!(
                "watching {} value(s) every {interval}ms - Ctrl-C to stop",
                chains.len()
            );
            let start = Instant::now();
            let mut reads = 0u64;
            loop {
                let mut line = format!("{:>7.1}s ", start.elapsed().as_secs_f64());
                for (target, chain) in &chains {
                    let shown = match chain {
                        Ok(chain) => match session.read(chain) {
                            Ok(value) => target.finish(value).to_string(),
                            Err(_) => "ERR".to_string(),
                        },
                        Err(_) => "NO CHAIN".to_string(),
                    };
                    line.push_str(&format!(" {}={shown}", target.label));
                }
                println!("{line}");
                io::stdout().flush()?;

                reads += 1;
                if count.is_some_and(|limit| reads >= limit) {
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(*interval));
            }
        }
    }
}

/// An attached game: the process, its module base, and what we know about it.
struct Session {
    pid: u32,
    exe: std::path::PathBuf,
    game_version: io::Result<String>,
    game: Process,
    module: u64,
    window: input::Window,
}

impl Session {
    fn attach(cli: &Cli, _catalog: &Catalog) -> io::Result<Self> {
        let pid = find_pid(cli)?;
        let exe = process::exe_path(pid)?;
        let game_version = version::installed(&exe);
        let game = Process::open(pid)?;
        let module = game.module_base(offsets::MONO_MODULE)?;
        let window = input::Window::for_process(pid)?;
        Ok(Self { pid, exe, game_version, game, module, window })
    }

    fn print_header(&self, catalog: &Catalog) {
        println!("pid         {}", self.pid);
        println!("exe         {}", self.exe.display());
        match &self.game_version {
            Ok(found) => println!("game        {found}"),
            Err(err) => println!("game        version unknown: {err}"),
        }
        println!("{:<11} {:#x}", offsets::MONO_MODULE, self.module);

        let describe = |value: &Option<String>| value.clone().unwrap_or_else(|| "?".to_string());
        let verdict = match (self.game_version.as_deref().ok(), catalog.import_version.as_deref()) {
            (Some(game), Some(imports)) if version::matches(game, imports) => "matches the game",
            (Some(_), Some(_)) => "DOES NOT MATCH THE GAME",
            _ => "not compared",
        };
        println!(
            "imports     P{} V{} (pointers V{}) from {} - {verdict}",
            describe(&catalog.platform),
            describe(&catalog.import_version),
            describe(&catalog.pointer_version),
            catalog.source,
        );
        println!("            {} paths", catalog.paths());
        if !catalog.overridden.is_empty() {
            println!(
                "            note: the overlay replaced {} upstream path(s): {}",
                catalog.overridden.len(),
                catalog.overridden.join(", ")
            );
        }
        println!(
            "window      {:#x}{}",
            self.window.hwnd(),
            if self.window.is_foreground() { " (foreground)" } else { " (not foreground)" }
        );
    }

    fn read(&self, chain: &Chain) -> io::Result<Value> {
        let address = self.game.resolve(chain.base(self.module), &chain.offsets)?;
        match chain.kind {
            Kind::I32 => self.game.read_i32(address).map(|v| Value::Int(v as i64)),
            Kind::I64 => self.game.read_i64(address).map(Value::Int),
            Kind::U8 => self.game.read_u8(address).map(|v| Value::Int(v as i64)),
            Kind::Quad => self.game.read_quad(address).map(Value::Big),
            Kind::F32 => self.game.read_f32(address).map(|v| Value::Float(v as f64)),
            Kind::F64 => self.game.read_f64(address).map(Value::Float),
            Kind::Utf16 => self.game.read_mono_string(address).map(Value::Text),
        }
    }

    /// Read one chain for `probe`: print it, and on failure show the hop the
    /// walk died on. A stale root breaks every chain hanging off it, so each
    /// root is traced only once.
    fn report(&self, target: &Target, chain: &Chain, traced: &mut Vec<String>) {
        match self.read(chain).map(|value| target.finish(value)) {
            Ok(value) => {
                let warning = match (&value, target.sane) {
                    (Value::Int(read), Some((low, high))) if !(low..=high).contains(read) => {
                        format!("  <- IMPLAUSIBLE, expected {low}..={high}")
                    }
                    (Value::Text(text), _) if text.is_empty() => "  <- empty".to_string(),
                    _ => String::new(),
                };
                println!("  {:<12} {value}{warning}", target.label);
            }
            Err(err) => {
                println!("  {:<12} UNREADABLE: {err}", target.label);
                if traced.contains(&chain.root.name) {
                    return;
                }
                traced.push(chain.root.name.clone());
                let hops = self.game.trace(chain.base(self.module), &chain.offsets);
                for (step, hop) in hops.iter().enumerate() {
                    match hop.value {
                        Some(value) => println!(
                            "    hop {step}: [{:#x}] = {value:#x}  +{:#x} -> {:#x}",
                            hop.at,
                            hop.offset,
                            value + hop.offset
                        ),
                        None => println!("    hop {step}: [{:#x}] unreadable", hop.at),
                    }
                }
            }
        }
    }
}

/// Something to read: either one of the probe's named values or any dotted
/// path from the loaded drop.
struct Target {
    label: String,
    path: String,
    indices: Vec<u64>,
    sane: Option<(i64, i64)>,
    /// Derived values divide what was read - see `offsets::DERIVED`.
    divide: i64,
}

impl Target {
    /// Apply whatever the label implies on top of the raw read.
    fn finish(&self, value: Value) -> Value {
        match value {
            Value::Int(raw) if self.divide > 1 => Value::Int(raw.div_euclid(self.divide)),
            other => other,
        }
    }
}

impl Target {
    fn from(want: &offsets::Want) -> Self {
        Self {
            label: want.label.to_string(),
            path: want.path.to_string(),
            indices: want.indices.to_vec(),
            sane: want.sane,
            divide: 1,
        }
    }

    /// A name is a derived value, then a probe label, then a raw path.
    fn parse(name: &str, indices: &[u64]) -> Self {
        if let Some(derived) = offsets::DERIVED.iter().find(|derived| derived.label == name) {
            if let Some(want) = offsets::NAMED.iter().find(|want| want.label == derived.from) {
                let mut target = Target::from(want);
                target.label = derived.label.to_string();
                target.divide = derived.divide;
                target.sane = None;
                return target;
            }
        }
        match offsets::NAMED.iter().find(|want| want.label == name) {
            Some(want) => {
                let mut target = Target::from(want);
                if !indices.is_empty() {
                    target.indices = indices.to_vec();
                }
                target
            }
            None => Self {
                label: name.to_string(),
                path: name.to_string(),
                indices: indices.to_vec(),
                sane: None,
                divide: 1,
            },
        }
    }

    fn list(names: &[String]) -> Vec<Self> {
        if names.is_empty() {
            return offsets::NAMED.iter().map(Target::from).collect();
        }
        names.iter().map(|name| Target::parse(name, &[])).collect()
    }
}

enum Value {
    Int(i64),
    Float(f64),
    Big(icrs::mem::Quad),
    Text(String),
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Int(value) => write!(f, "{value}"),
            Value::Float(value) => write!(f, "{value}"),
            Value::Big(value) => write!(f, "{value}"),
            Value::Text(value) => write!(f, "{value:?}"),
        }
    }
}

fn find_pid(cli: &Cli) -> io::Result<u32> {
    if let Some(pid) = cli.global.pid {
        return Ok(pid);
    }
    let pids = process::find_pids(&cli.global.exe)?;
    let Some(&pid) = pids.first() else {
        return Err(io::Error::other(format!(
            "{} is not running (or we are not on its wineserver)",
            cli.global.exe
        )));
    };
    if pids.len() > 1 {
        eprintln!(
            "warning: {} instances running, using pid {pid} (pass --pid to choose)",
            pids.len()
        );
    }
    Ok(pid)
}

fn unknown_key(name: &str) -> io::Error {
    io::Error::other(format!(
        "unknown key {name:?} (known: {})",
        input::KEYS.iter().map(|key| key.name).collect::<Vec<_>>().join(", ")
    ))
}

fn hops(offsets: &[u64]) -> String {
    offsets.iter().map(|offset| format!("{offset:#x}")).collect::<Vec<_>>().join(" -> ")
}

/// True when we are the only process on this console, which is what happens
/// when Explorer starts us: the window is ours alone and closes with us.
#[cfg(windows)]
fn launched_from_explorer() -> bool {
    use windows_sys::Win32::System::Console::GetConsoleProcessList;

    let mut pids = [0u32; 2];
    // SAFETY: the buffer and its length are handed over together.
    let count = unsafe { GetConsoleProcessList(pids.as_mut_ptr(), pids.len() as u32) };
    count == 1
}

#[cfg(not(windows))]
fn launched_from_explorer() -> bool {
    false
}

fn wait_for_enter() {
    if !io::stdin().is_terminal() {
        return;
    }
    print!("\npress Enter to close ");
    let _ = io::stdout().flush();
    let mut discard = String::new();
    let _ = io::stdin().read_line(&mut discard);
}

/// One entry in the ultimates bar.
struct Ultimate {
    slot: u64,
    hero_id: i64,
    hotkey: i64,
    cooldown: f64,
    queued: bool,
}

impl Ultimate {
    fn ready(&self) -> bool {
        self.cooldown <= 0.0
    }
}

impl Session {
    /// Read the ultimates bar. Slots exist for champions without ultimates
    /// too, so entries that do not resolve are skipped rather than reported.
    fn ultimates(&self, catalog: &Catalog) -> io::Result<Vec<Ultimate>> {
        let size = self.read_named(catalog, offsets::ultimates::SIZE, &[])?;
        let size = match size {
            Value::Int(size) if (0..=64).contains(&size) => size as u64,
            other => return Err(io::Error::other(format!("implausible bar size: {other}"))),
        };

        let mut bar = Vec::new();
        for slot in 0..size {
            let at = [0, slot];
            let (Ok(Value::Int(hero_id)), Ok(Value::Int(hotkey))) = (
                self.read_named(catalog, offsets::ultimates::HERO_ID, &at),
                self.read_named(catalog, offsets::ultimates::HOTKEY, &at),
            ) else {
                continue;
            };
            let cooldown = match self.read_named(catalog, offsets::ultimates::COOLDOWN, &at) {
                Ok(Value::Float(seconds)) => seconds,
                _ => continue,
            };
            let queued = matches!(
                self.read_named(catalog, offsets::ultimates::QUEUED, &at),
                Ok(Value::Int(1))
            );
            bar.push(Ultimate { slot, hero_id, hotkey, cooldown, queued });
        }
        Ok(bar)
    }

    /// Champion ids and names, for turning "ellywick" into 83.
    fn roster(&self, catalog: &Catalog) -> io::Result<Vec<(i64, String)>> {
        let Value::Int(size) = self.read_named(catalog, offsets::heroes::SIZE, &[])? else {
            return Err(io::Error::other("hero list size is not a number"));
        };

        let mut roster = Vec::new();
        for index in 0..size.clamp(0, 1024) as u64 {
            let at = [0, index];
            if let (Ok(Value::Int(id)), Ok(Value::Text(name))) = (
                self.read_named(catalog, offsets::heroes::ID, &at),
                self.read_named(catalog, offsets::heroes::NAME, &at),
            ) {
                roster.push((id, name));
            }
        }
        Ok(roster)
    }

    /// Where a champion sits in the hero list, which is what every
    /// per-champion path is indexed by.
    fn hero_index(&self, catalog: &Catalog, hero_id: i64) -> io::Result<u64> {
        let Value::Int(size) = self.read_named(catalog, offsets::heroes::SIZE, &[])? else {
            return Err(io::Error::other("hero list size is not a number"));
        };
        for index in 0..size.clamp(0, 1024) as u64 {
            if let Ok(Value::Int(id)) =
                self.read_named(catalog, offsets::heroes::ID, &[0, index])
                && id == hero_id
            {
                return Ok(index);
            }
        }
        Err(io::Error::other(format!("no champion with id {hero_id} in the hero list")))
    }

    /// Accepts a champion id or a name.
    fn hero_id(&self, catalog: &Catalog, who: &str) -> io::Result<i64> {
        if let Ok(id) = who.parse::<i64>() {
            return Ok(id);
        }
        let wanted = who.to_lowercase();
        let roster = self.roster(catalog)?;
        roster
            .iter()
            .find(|(_, name)| name.to_lowercase() == wanted)
            .or_else(|| roster.iter().find(|(_, name)| name.to_lowercase().contains(&wanted)))
            .map(|(id, _)| *id)
            .ok_or_else(|| io::Error::other(format!("no champion matching {who:?}")))
    }

    /// Every effect key on a champion, with the dictionary entry it occupies.
    fn effect_keys(&self, catalog: &Catalog, hero: u64) -> io::Result<Vec<(u64, String)>> {
        let Value::Int(count) = self.read_named(catalog, offsets::effects::COUNT, &[0, hero])?
        else {
            return Err(io::Error::other("effect key count is not a number"));
        };

        let mut keys = Vec::new();
        for entry in 0..count.clamp(0, 256) as u64 {
            if let Ok(Value::Text(key)) =
                self.read_named(catalog, offsets::effects::KEY, &[0, hero, entry, 0])
                && !key.is_empty()
            {
                keys.push((entry, key));
            }
        }
        Ok(keys)
    }

    /// The dictionary entry holding one of `candidates`, in preference order.
    fn find_effect(
        &self,
        catalog: &Catalog,
        hero: u64,
        candidates: &[&str],
    ) -> io::Result<u64> {
        let keys = self.effect_keys(catalog, hero)?;
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
    fn cards(&self, catalog: &Catalog, hero: u64, entry: u64) -> io::Result<Vec<i64>> {
        let at = [0, hero, entry, 0, 0];
        let Value::Int(size) = self.read_named(catalog, offsets::effects::CARDS, &at)? else {
            return Err(io::Error::other("card count is not a number"));
        };
        if !(0..=16).contains(&size) {
            return Err(io::Error::other(format!("implausible card count: {size}")));
        }

        let mut hand = Vec::new();
        for card in 0..size as u64 {
            let at = [0, hero, entry, 0, 0, card];
            if let Value::Int(card_type) =
                self.read_named(catalog, offsets::effects::CARD_TYPE, &at)?
            {
                hand.push(card_type);
            }
        }
        Ok(hand)
    }

    fn read_named(&self, catalog: &Catalog, path: &str, indices: &[u64]) -> io::Result<Value> {
        let chain = catalog
            .chain(path, path, indices)
            .map_err(|err| io::Error::other(err.to_string()))?;
        self.read(&chain)
    }
}
