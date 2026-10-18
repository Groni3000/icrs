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
            for want in offsets::WANTED {
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
            let value = session.read(&chain)?;
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
                            Ok(value) => value.to_string(),
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
            Kind::F32 => self.game.read_f32(address).map(|v| Value::Float(v as f64)),
            Kind::F64 => self.game.read_f64(address).map(Value::Float),
            Kind::Utf16 => self.game.read_mono_string(address).map(Value::Text),
        }
    }

    /// Read one chain for `probe`: print it, and on failure show the hop the
    /// walk died on. A stale root breaks every chain hanging off it, so each
    /// root is traced only once.
    fn report(&self, target: &Target, chain: &Chain, traced: &mut Vec<String>) {
        match self.read(chain) {
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
}

impl Target {
    fn from(want: &offsets::Want) -> Self {
        Self {
            label: want.label.to_string(),
            path: want.path.to_string(),
            indices: want.indices.to_vec(),
            sane: want.sane,
        }
    }

    /// A name is a probe label if one matches, otherwise it is a path.
    fn parse(name: &str, indices: &[u64]) -> Self {
        match offsets::WANTED.iter().find(|want| want.label == name) {
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
            },
        }
    }

    fn list(names: &[String]) -> Vec<Self> {
        if names.is_empty() {
            return offsets::WANTED.iter().map(Target::from).collect();
        }
        names.iter().map(|name| Target::parse(name, &[])).collect()
    }
}

enum Value {
    Int(i64),
    Float(f64),
    Text(String),
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Int(value) => write!(f, "{value}"),
            Value::Float(value) => write!(f, "{value}"),
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
