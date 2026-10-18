//! Probe: attach to the game and read a few live values.

use icrs::{input, mem::Process, offsets, process};
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

fn run() -> std::io::Result<()> {
    let pids = process::find_pids(GAME_EXE)?;
    let Some(&pid) = pids.first() else {
        return Err(std::io::Error::other(format!(
            "{GAME_EXE} is not running (or we are not on its wineserver)"
        )));
    };
    if pids.len() > 1 {
        eprintln!("warning: {} instances running, using pid {pid}", pids.len());
    }

    let game = Process::open(pid)?;
    let module = game.module_base(offsets::MONO_MODULE)?;

    println!("pid         {}", game.pid());
    println!("{:<11} {:#x}", offsets::MONO_MODULE, module);

    let window = input::Window::for_process(pid)?;
    println!(
        "window      {:#x}{}",
        window.hwnd(),
        if window.is_foreground() { " (foreground)" } else { " (not foreground)" }
    );

    // A stale root breaks every chain hanging off it, so the hop trace is
    // printed once per root instead of once per chain.
    let mut traced: Vec<&str> = Vec::new();
    let mut root = "";
    for chain in offsets::ALL {
        if chain.root.name != root {
            root = chain.root.name;
            let hops: Vec<String> =
                chain.root.module_offset.iter().map(|o| format!("{o:#x}")).collect();
            println!(
                "\n{root}  (mono+{:#x} -> {})",
                chain.root.module_address,
                hops.join(" -> ")
            );
        }
        report(&game, chain, module, &mut traced);
    }

    // Optional: icrs <key>  e.g. `cargo run -- esc`
    if let Some(name) = std::env::args().nth(1) {
        let key = input::key_by_name(&name).ok_or_else(|| {
            std::io::Error::other(format!(
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

/// Read one chain and print it; on failure, show where the walk died.
fn report(game: &Process, chain: &offsets::Chain, module_base: u64, traced: &mut Vec<&'static str>) {
    let base = chain.base(module_base);
    let full = chain.full();

    let value = game.resolve(base, &full).and_then(|addr| match chain.kind {
        offsets::Kind::I32 => game.read_i32(addr).map(|v| v.to_string()),
        offsets::Kind::I64 => game.read_i64(addr).map(|v| v.to_string()),
        offsets::Kind::Utf16 => game.read_mono_string(addr).map(|v| format!("{v:?}")),
    });

    match value {
        Ok(value) => println!("  {:<12} {value}", chain.label),
        Err(err) => {
            println!("  {:<12} UNREADABLE: {err}", chain.label);
            if traced.contains(&chain.root.name) {
                return;
            }
            traced.push(chain.root.name);
            for (step, hop) in game.trace(base, &full).iter().enumerate() {
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
