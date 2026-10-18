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
    let base = module + offsets::IDLE_GAME_MANAGER;

    println!("pid        {}", game.pid());
    println!(
        "{:<10} {:#x}  (+{:#x} -> {:#x})",
        offsets::MONO_MODULE,
        module,
        offsets::IDLE_GAME_MANAGER,
        base
    );

    let window = input::Window::for_process(pid)?;
    println!(
        "window     {:#x}{}",
        window.hwnd(),
        if window.is_foreground() { " (foreground)" } else { " (not foreground)" }
    );

    for chain in offsets::ALL {
        match game
            .resolve(base, chain.offsets)
            .and_then(|addr| game.read_i32(addr))
        {
            Ok(value) => println!("{:<10} {value}", chain.label),
            Err(err) => println!("{:<10} unreadable: {err}", chain.label),
        }
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
        println!("sent       {} (vk {:#04x}, scan {})", key.name, key.vk, key.scan);
    }
    Ok(())
}
