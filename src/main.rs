//! `icrs` - read a running Idle Champions client, and send input to it.
//!
//! Every offset comes from the loaded drop (see `icrs::catalog`) and every read
//! goes through `icrs::game`; this file only decides what to ask for and how to
//! print it.

use clap::Parser;

use icrs::catalog::{Catalog, Chain};
use icrs::cli::{Cli, Command};
use icrs::game::Game;
use icrs::profile;
use icrs::snapshot::{Sampler, Snapshot};
use icrs::value::Value;
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

        Command::Profile { path, example } => {
            if *example {
                print!("{}", profile::EXAMPLE);
                return Ok(());
            }
            let loaded = profile::discover(path.as_deref())
                .map_err(|err| io::Error::other(err.to_string()))?;
            show_profile(&loaded);
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

        Command::Cards { count, gems, hero } => {
            let game = attach(cli, catalog)?;
            let hero_id = hero.unwrap_or(offsets::effects::ELLYWICK_HERO_ID);
            let hero_index = game.hero_index(hero_id)?;
            let entry = game.find_effect(hero_index, offsets::effects::DECK_KEYS)?;
            let hand = game.cards(hero_index, entry)?;
            let tally = |card_type: i64| hand.iter().filter(|card| **card == card_type).count();

            // Scriptable single number: --count <type>, or --gems for the one
            // that started all this.
            let wanted = match (count, gems) {
                (Some(name), _) => Some(offsets::effects::card_type_by_name(name).ok_or_else(
                    || {
                        io::Error::other(format!(
                            "unknown card type {name:?} (known: {})",
                            offsets::effects::CARD_TYPES
                                .iter()
                                .map(|(_, card)| *card)
                                .collect::<Vec<_>>()
                                .join(", ")
                        ))
                    },
                )?),
                (None, true) => Some(offsets::effects::GEM_CARD),
                (None, false) => None,
            };
            if let Some(card_type) = wanted {
                println!("{}", tally(card_type));
                return Ok(());
            }

            let named: Vec<&str> =
                hand.iter().map(|card| offsets::effects::card_name(*card)).collect();
            println!("champion #{hero_id}, effect entry {entry}");
            println!("hand        {}", named.join(", "));
            for (card_type, name) in offsets::effects::CARD_TYPES {
                println!("{:<11} {}", name.to_lowercase(), tally(*card_type));
            }
            Ok(())
        }

        Command::Effects { who } => {
            let game = attach(cli, catalog)?;
            let hero_id = game.hero_id(who)?;
            let hero_index = game.hero_index(hero_id)?;
            for (entry, key) in game.effect_keys(hero_index)? {
                println!("{entry:>3}  {key}");
            }
            Ok(())
        }

        Command::Ultimates => {
            let game = attach(cli, catalog)?;
            let bar = game.ultimates()?;
            let roster = game.roster().unwrap_or_default();
            println!("slot  key  champion                     cooldown");
            for entry in &bar {
                println!(
                    "{:>4}  {:>3}  {:<28} {}",
                    entry.slot,
                    entry.hotkey,
                    format!("{} (#{})", champion(&roster, entry.hero_id), entry.hero_id),
                    state_of(entry)
                );
            }
            Ok(())
        }

        Command::Ult { who, slot, force } => {
            let game = attach(cli, catalog)?;
            let bar = game.ultimates()?;

            let entry = match (who, slot) {
                (_, Some(slot)) => bar
                    .iter()
                    .find(|entry| entry.slot == *slot)
                    .ok_or_else(|| io::Error::other(format!("no ultimate in slot {slot}")))?,
                (Some(who), None) => {
                    let hero_id = game.hero_id(who)?;
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
                io::Error::other(format!(
                    "slot {} has hotkey {}, which is not a key this program knows how to send",
                    entry.slot, entry.hotkey
                ))
            })?;
            game.tap(key)?;
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
            let game = attach(cli, catalog)?;
            print_header(&game);
            println!("\nroots");
            for root in game.catalog().roots() {
                let base = game.module() + root.module_address;
                // Resolving is not enough: the last hop is never dereferenced,
                // so a stale path can "succeed" onto an address that holds
                // nothing. A real Mono object starts with a vtable pointer, so
                // read that as proof of life.
                let state = match game.process().resolve(base, &root.module_offset) {
                    Ok(addr) => match game.process().read_u64(addr) {
                        Ok(vtable) if vtable != 0 => format!("live -> {addr:#x}"),
                        Ok(_) => format!("SUSPECT -> {addr:#x} begins with a null pointer"),
                        Err(err) => format!("DEAD -> {addr:#x} is not readable: {err}"),
                    },
                    Err(err) => format!("DEAD: {err}"),
                };
                println!(
                    "  {:<16} mono+{:#x} -> {}",
                    root.name,
                    root.module_address,
                    hops(&root.module_offset)
                );
                println!("  {:<16} {state}", "");
            }
            Ok(())
        }

        Command::Probe => {
            let game = attach(cli, catalog)?;
            print_header(&game);

            let mut traced: Vec<String> = Vec::new();
            let mut current_root = String::new();
            for want in offsets::NAMED.iter().filter(|want| want.probe) {
                let target = Target::from(want);
                let chain = match game.catalog().chain(&target.label, &target.path, &target.indices)
                {
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
                report(&game, &target, &chain, &mut traced);
            }
            Ok(())
        }

        Command::Read { target, index, bare } => {
            let game = attach(cli, catalog)?;
            let target = Target::parse(target, index);
            let chain = game
                .catalog()
                .chain(&target.label, &target.path, &target.indices)
                .map_err(|err| io::Error::other(err.to_string()))?;
            let value = target.finish(game.read(&chain)?);
            if *bare {
                println!("{value}");
            } else {
                println!("{:<12} {value}", target.label);
            }
            Ok(())
        }

        Command::Watch { targets, interval, count } => {
            let game = attach(cli, catalog)?;
            let targets = Target::list(targets);
            let chains: Vec<(Target, io::Result<Chain>)> = targets
                .into_iter()
                .map(|target| {
                    let chain = game
                        .catalog()
                        .chain(&target.label, &target.path, &target.indices)
                        .map_err(|err| io::Error::other(err.to_string()));
                    (target, chain)
                })
                .collect();

            println!("watching {} value(s) every {interval}ms - Ctrl-C to stop", chains.len());
            let start = Instant::now();
            let mut reads = 0u64;
            loop {
                let mut line = format!("{:>7.1}s ", start.elapsed().as_secs_f64());
                for (target, chain) in &chains {
                    let shown = match chain {
                        Ok(chain) => match game.read(chain) {
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

        Command::Bench { count } => {
            let game = attach(cli, catalog)?;
            let reads = (*count).max(1);
            // One ordinary scalar, deep enough to be representative.
            let path = "game.gameInstances.ActiveCampaignData.currentAreaID";
            let chain = game.chain(path, &[])?;
            let hops = chain.offsets.len();
            let address = game.process().resolve(chain.base(game.module()), &chain.offsets)?;

            let per = |elapsed: Duration, ops: u32| elapsed.as_secs_f64() * 1e6 / ops as f64;

            let start = Instant::now();
            for _ in 0..reads {
                game.process().read_i32(address)?;
            }
            let raw = start.elapsed();

            // The same call asking for 4KB instead of 4 bytes: if it costs
            // the same, the price is the call, not the bytes, and the way to
            // go faster is fewer and fatter reads.
            let mut block = vec![0u8; 4096];
            let start = Instant::now();
            for _ in 0..reads {
                game.process().read_into(address & !0xFFF, &mut block)?;
            }
            let bulk = start.elapsed();

            let start = Instant::now();
            for _ in 0..reads {
                game.read(&chain)?;
            }
            let walked = start.elapsed();

            let start = Instant::now();
            for _ in 0..reads {
                game.chain(path, &[])?;
            }
            let resolved = start.elapsed();

            let sampler = Sampler::new(&game)?;
            let ticks = (reads / 20).max(5);
            let start = Instant::now();
            let mut values = 0;
            for _ in 0..ticks {
                let snapshot = sampler.take(&game);
                values = 11 + snapshot.ultimates.len() * 4
                    + snapshot.cards.as_ref().map_or(0, |hand| hand.len() + 1);
            }
            let sampled = start.elapsed();

            println!("pid {} - {reads} repetitions\n", game.pid());
            println!("one 4-byte read at a known address    {:>8.1} us", per(raw, reads));
            println!(
                "one 4096-byte read at a known address  {:>8.1} us  = {:.0}x the bytes for {:.2}x the time",
                per(bulk, reads),
                4096.0 / 4.0,
                per(bulk, reads) / per(raw, reads)
            );
            println!(
                "one pointer walk + read ({hops} hops)        {:>8.1} us  = {:.1} us per hop",
                per(walked, reads),
                per(walked, reads) / (hops + 1) as f64
            );
            println!(
                "building the chain from the imports    {:>8.1} us  (no game access at all)",
                per(resolved, reads)
            );
            println!(
                "\none full sample ({values} values)         {:>8.0} us  = {:.1} reads' worth",
                per(sampled, ticks),
                per(sampled, ticks) / per(raw, reads)
            );
            println!(
                "\nat tick 250ms that is {:.1}% of the time, and a 60fps frame is 16667us",
                per(sampled, ticks) / 2_500.0
            );
            Ok(())
        }

        Command::Snapshot { json } => {
            let game = attach(cli, catalog)?;
            let sampler = Sampler::new(&game)?;
            let snapshot = sampler.take(&game);
            if *json {
                println!("{}", snapshot.to_json());
                return Ok(());
            }
            for note in &sampler.notes {
                println!("note        {note}");
            }
            show(&game, &snapshot);
            Ok(())
        }
    }
}

fn attach(cli: &Cli, catalog: Catalog) -> io::Result<Game> {
    Game::open(find_pid(cli)?, catalog)
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

/// What the game is, what we loaded, and whether the two agree.
fn print_header(game: &Game) {
    let catalog = game.catalog();
    println!("pid         {}", game.pid());
    println!("exe         {}", game.exe().display());
    match game.installed_version() {
        Ok(found) => println!("game        {found}"),
        Err(err) => println!("game        version unknown: {err}"),
    }
    println!("{:<11} {:#x}", offsets::MONO_MODULE, game.module());

    let describe = |value: &Option<String>| value.clone().unwrap_or_else(|| "?".to_string());
    let verdict = match (game.installed_version().ok(), catalog.import_version.as_deref()) {
        (Some(installed), Some(imports)) if version::matches(installed, imports) => {
            "matches the game"
        }
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
        game.window().hwnd(),
        if game.window().is_foreground() { " (foreground)" } else { " (not foreground)" }
    );
}

/// Read one chain for `probe`: print it, and on failure show the hop the walk
/// died on. A stale root breaks every chain hanging off it, so each root is
/// traced only once.
fn report(game: &Game, target: &Target, chain: &Chain, traced: &mut Vec<String>) {
    match game.read(chain).map(|value| target.finish(value)) {
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
            let hops = game.process().trace(chain.base(game.module()), &chain.offsets);
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

/// A profile as loaded, so a typo or a surprising default is visible before
/// anything is sent to the game.
fn show_profile(loaded: &profile::Loaded) {
    let profile = &loaded.profile;
    println!("profile     {}", profile.name);
    println!("source      {}", loaded.source);
    println!("tick        {}ms", profile.tick.as_millis());

    match &profile.deck {
        Some(deck) => {
            println!(
                "deck        {} - keep {} {} card(s) of a hand of {}",
                deck.champion,
                deck.want_at_least,
                deck.want_name().to_lowercase(),
                deck.hand_size
            );
            println!(
                "            the ultimate only fires once {} can no longer be reached",
                deck.want_at_least
            );
            match deck.zone_at_least {
                0 => println!("            at any zone"),
                zone => println!("            and only at zone {zone} or deeper"),
            }
        }
        None => println!("deck        not watched (no [deck] section)"),
    }

    match &profile.briv {
        Some(briv) => {
            println!(
                "briv        {} - start the next run with {} sprint stacks",
                briv.champion, briv.sprint_at_least
            );
            println!(
                "            so tank {} steelbones this run ({})",
                briv.steelbones_needed(),
                if briv.conversion_feat { "x1.2 conversion feat" } else { "1:1, no feat" }
            );
            println!("            reported only: nothing is pressed over these yet");
        }
        None => println!("briv        not watched (no [briv] section)"),
    }

    let ready = &profile.ultimates.fire_when_ready;
    println!(
        "ultimates   fire when ready: {}",
        if ready.is_empty() {
            "(none)".to_string()
        } else {
            ready.iter().map(|champion| champion.to_string()).collect::<Vec<_>>().join(", ")
        }
    );
    println!("            recast {:.1}s", profile.ultimates.recast.as_secs_f64());

    let mut guards: Vec<&str> = Vec::new();
    if profile.guards.skip_while_transitioning {
        guards.push("skip while transitioning");
    }
    if profile.guards.require_full_reads {
        guards.push("require full reads");
    }
    println!(
        "guards      {}",
        if guards.is_empty() { "none".to_string() } else { guards.join(", ") }
    );

    let unresolved: Vec<String> = profile
        .champions()
        .iter()
        .filter(|champion| champion.id.is_none())
        .map(|champion| champion.to_string())
        .collect();
    if !unresolved.is_empty() {
        println!("            {} matched against the roster at run time", unresolved.join(", "));
    }
}

/// One sample, as a table. The same numbers a tick of automation would act on.
fn show(game: &Game, snapshot: &Snapshot) {
    let number = |value: Option<i64>| match value {
        Some(value) => value.to_string(),
        None => "?".to_string(),
    };

    let zone = match (snapshot.zone, snapshot.on_boss_zone()) {
        (Some(zone), Some(true)) => format!("{zone} (boss)"),
        (Some(zone), _) => zone.to_string(),
        (None, _) => "?".to_string(),
    };
    println!("zone        {zone}");
    println!(
        "highest     {}   bosses killed {}",
        number(snapshot.highest_zone),
        number(snapshot.bosses())
    );
    println!("quest left  {}", number(snapshot.quest_left));
    println!(
        "modron      target {}{}",
        number(snapshot.modron_target),
        match snapshot.transitioning {
            Some(true) => ", transitioning",
            Some(false) => "",
            None => ", transition unknown",
        }
    );
    println!("steelbones  {}", number(snapshot.steelbones));
    println!("sprint      {}", number(snapshot.sprint));
    println!("resets      {}", number(snapshot.resets));
    println!("gems        {}", number(snapshot.gems));
    match snapshot.since_hit {
        Some(seconds) => println!("since hit   {seconds:.1}s"),
        None => println!("since hit   ?"),
    }

    match &snapshot.cards {
        Some(hand) if hand.is_empty() => println!("cards       (empty hand)"),
        Some(hand) => {
            let named: Vec<&str> =
                hand.iter().map(|card| offsets::effects::card_name(*card)).collect();
            println!(
                "cards       {}   gem {}, moon {}",
                named.join(", "),
                number(snapshot.gem_cards().map(|count| count as i64)),
                number(snapshot.moon_cards().map(|count| count as i64)),
            );
        }
        None => println!("cards       not tracked"),
    }

    let roster = game.roster().unwrap_or_default();
    println!("ultimates");
    for entry in &snapshot.ultimates {
        println!(
            "  slot {:>2}  key {}  {:<28} {}",
            entry.slot,
            entry.hotkey,
            format!("{} (#{})", champion(&roster, entry.hero_id), entry.hero_id),
            state_of(entry)
        );
    }

    println!("\nsampled in {:.1}ms", snapshot.took.as_secs_f64() * 1000.0);
    if !snapshot.healthy() {
        println!("did not read: {}", snapshot.misses.join(", "));
    }
}

fn champion(roster: &[(i64, String)], hero_id: i64) -> String {
    roster
        .iter()
        .find(|(id, _)| *id == hero_id)
        .map(|(_, name)| name.clone())
        .unwrap_or_else(|| "?".to_string())
}

fn state_of(entry: &icrs::game::Ultimate) -> String {
    if entry.queued {
        "queued".to_string()
    } else if entry.ready() {
        "ready".to_string()
    } else {
        format!("{:.0}s", entry.cooldown)
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
