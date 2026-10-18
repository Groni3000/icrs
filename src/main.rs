use icrs::process;

const GAME_EXE: &str = "IdleDragons.exe";

fn main() -> std::process::ExitCode {
    match process::find_pids(GAME_EXE) {
        Ok(pids) if pids.is_empty() => {
            eprintln!("{GAME_EXE} is not running (or we are not on its wineserver)");
            std::process::ExitCode::FAILURE
        }
        Ok(pids) => {
            for pid in &pids {
                println!("{GAME_EXE} pid={pid}");
            }
            std::process::ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("error: {err}");
            std::process::ExitCode::FAILURE
        }
    }
}
