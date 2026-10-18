//! Probe: attach to the game and read live values through a loaded offset drop.
//!
//! Usage:
//!   icrs [--imports <dir>] [<key>]
//!
//! `<key>` sends one keystroke to the game window (`esc`, `q`, `w`, `e`).

use icrs::catalog::{Catalog, Chain};
use icrs::mem::Process;
use icrs::offsets::{Kind, Want};
use icrs::{input, offsets, process, version};

use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;

const GAME_EXE: &str = "IdleDragons.exe";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

struct Args {
    imports: Option<PathBuf>,
    key: Option<String>,
}

fn parse_args() -> io::Result<Args> {
    let mut args = Args { imports: None, key: None };
    let mut rest = std::env::args().skip(1);
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--imports" => {
                let dir = rest.next().ok_or_else(|| {
                    io::Error::other("--imports needs a directory")
                })?;
                args.imports = Some(PathBuf::from(dir));
            }
            "-h" | "--help" => {
                println!("usage: icrs [--imports <dir>] [<key>]");
                std::process::exit(0);
            }
            other if other.starts_with('-') => {
                return Err(io::Error::other(format!("unknown option {other}")));
            }
            other => args.key = Some(other.to_string()),
        }
    }
    Ok(args)
}

fn run() -> io::Result<()> {
    let args = parse_args()?;

    let pids = process::find_pids(GAME_EXE)?;
    let Some(&pid) = pids.first() else {
        return Err(io::Error::other(format!(
            "{GAME_EXE} is not running (or we are not on its wineserver)"
        )));
    };
    if pids.len() > 1 {
        eprintln!("warning: {} instances running, using pid {pid}", pids.len());
    }

    // Step 1: the game's own version, read off disk - no offsets involved.
    let exe = process::exe_path(pid)?;
    let game_version = version::installed(&exe);

    // Step 2: load an offset drop.
    let mut catalog = load_catalog(args.imports.as_deref())?;

    let game = Process::open(pid)?;
    let module = game.module_base(offsets::MONO_MODULE)?;

    println!("pid         {pid}");
    println!("exe         {}", exe.display());
    match &game_version {
        Ok(found) => println!("game        {found}"),
        Err(err) => println!("game        version unknown: {err}"),
    }
    println!("{:<11} {:#x}", offsets::MONO_MODULE, module);

    // Step 3: say what was loaded and whether it targets this build.
    print_imports_line(&catalog, game_version.as_deref().ok());
    if let Ok(found) = &game_version {
        if let Some(replacement) = offer_replacement(&catalog, found)? {
            catalog = replacement;
            print_imports_line(&catalog, Some(found));
        }
    }

    let window = input::Window::for_process(pid)?;
    println!(
        "window      {:#x}{}",
        window.hwnd(),
        if window.is_foreground() { " (foreground)" } else { " (not foreground)" }
    );

    // Step 4: resolve every wanted path and sanity-check what comes back.
    let mut traced: Vec<String> = Vec::new();
    let mut root = String::new();
    for want in offsets::WANTED {
        let chain = match catalog.chain(want.label, want.path, want.indices) {
            Ok(chain) => chain,
            Err(err) => {
                println!("  {:<12} NO CHAIN: {err}", want.label);
                continue;
            }
        };
        if chain.root.name != root {
            root = chain.root.name.clone();
            println!(
                "\n{root}  (mono+{:#x} -> {})",
                chain.root.module_address,
                chain
                    .root
                    .module_offset
                    .iter()
                    .map(|offset| format!("{offset:#x}"))
                    .collect::<Vec<_>>()
                    .join(" -> ")
            );
        }
        report(&game, want, &chain, module, &mut traced);
    }

    if let Some(name) = args.key {
        let key = input::key_by_name(&name).ok_or_else(|| {
            io::Error::other(format!(
                "unknown key {name:?} (known: {})",
                input::KEYS.iter().map(|k| k.name).collect::<Vec<_>>().join(", ")
            ))
        })?;
        window.nudge_active()?;
        window.tap(key)?;
        println!("\nsent        {} (vk {:#04x}, scan {})", key.name, key.vk, key.scan);
    }
    Ok(())
}

fn load_catalog(explicit: Option<&std::path::Path>) -> io::Result<Catalog> {
    Catalog::discover(explicit).map_err(|err| io::Error::other(err.to_string()))
}

fn print_imports_line(catalog: &Catalog, game_version: Option<&str>) {
    let describe = |value: &Option<String>| value.clone().unwrap_or_else(|| "?".to_string());
    let verdict = match (game_version, catalog.import_version.as_deref()) {
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
}

/// On a version mismatch, offer to load a different drop - but only when
/// someone is there to answer. An unattended run must fail or carry on
/// loudly, never block on input.
fn offer_replacement(catalog: &Catalog, game_version: &str) -> io::Result<Option<Catalog>> {
    let Some(imports) = catalog.import_version.as_deref() else {
        return Ok(None);
    };
    if version::matches(game_version, imports) {
        return Ok(None);
    }
    if !io::stdin().is_terminal() {
        eprintln!(
            "warning: imports target V{imports}, game is {game_version} - continuing anyway; \
             pass --imports <dir> or set ICRS_IMPORTS to use a matching drop"
        );
        return Ok(None);
    }

    print!("path to a V{} offset drop (blank to continue anyway): ", version::feed_form(game_version));
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    let answer = answer.trim();
    if answer.is_empty() {
        return Ok(None);
    }
    Catalog::from_dir(std::path::Path::new(answer))
        .map(Some)
        .map_err(|err| io::Error::other(err.to_string()))
}

/// Read one chain and print it; on failure, show where the walk died.
fn report(game: &Process, want: &Want, chain: &Chain, module_base: u64, traced: &mut Vec<String>) {
    let base = chain.base(module_base);

    let value = game.resolve(base, &chain.offsets).and_then(|addr| match chain.kind {
        Kind::I32 => game.read_i32(addr).map(|v| Read::Int(v as i64)),
        Kind::I64 => game.read_i64(addr).map(Read::Int),
        Kind::U8 => game.read_u8(addr).map(|v| Read::Int(v as i64)),
        Kind::F32 => game.read_f32(addr).map(|v| Read::Float(v as f64)),
        Kind::F64 => game.read_f64(addr).map(Read::Float),
        Kind::Utf16 => game.read_mono_string(addr).map(Read::Text),
    });

    match value {
        Ok(read) => {
            let warning = match (&read, want.sane) {
                (Read::Int(value), Some((low, high))) if !(low..=high).contains(value) => {
                    format!("  <- IMPLAUSIBLE, expected {low}..={high}")
                }
                (Read::Text(text), _) if text.is_empty() => "  <- empty".to_string(),
                _ => String::new(),
            };
            println!("  {:<12} {read}{warning}", want.label);
        }
        Err(err) => {
            println!("  {:<12} UNREADABLE: {err}", want.label);
            // A stale root breaks every chain hanging off it, so the hop trace
            // is printed once per root instead of once per chain.
            if traced.contains(&chain.root.name) {
                return;
            }
            traced.push(chain.root.name.clone());
            for (step, hop) in game.trace(base, &chain.offsets).iter().enumerate() {
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

enum Read {
    Int(i64),
    Float(f64),
    Text(String),
}

impl std::fmt::Display for Read {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Read::Int(value) => write!(f, "{value}"),
            Read::Float(value) => write!(f, "{value}"),
            Read::Text(value) => write!(f, "{value:?}"),
        }
    }
}
