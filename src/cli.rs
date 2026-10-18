//! The command line.
//!
//! Every command shares the same two questions - which game process, and which
//! offset drop - so those are global flags. `probe` is the default because
//! running the program with no arguments at all (a double-click) should do the
//! useful thing.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

/// The game's executable, as it appears in the process list.
pub const GAME_EXE: &str = "IdleDragons.exe";

#[derive(Debug, Parser)]
#[command(
    name = "icrs",
    version,
    about = "Read Idle Champions' live state and send input to it",
    long_about = "Reads values out of a running Idle Champions client through \
                  offsets loaded from data, and can send keystrokes to it \
                  without stealing focus.\n\nRun with no arguments to probe \
                  every known value."
)]
pub struct Cli {
    #[command(flatten)]
    pub global: Global,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Args)]
pub struct Global {
    /// Offset drop to use: a directory holding IC_Offsets.json and the import
    /// files. Default: $ICRS_IMPORTS, then imports/ beside this exe, then the
    /// drop built into the binary.
    #[arg(long, value_name = "DIR", global = true)]
    pub imports: Option<PathBuf>,

    /// Game executable to look for.
    #[arg(long, value_name = "NAME", default_value = GAME_EXE, global = true)]
    pub exe: String,

    /// Attach to this process id instead of searching by name.
    #[arg(long, value_name = "PID", global = true)]
    pub pid: Option<u32>,

    /// Wait for Enter before exiting. Automatic when launched from Explorer,
    /// where the console window would otherwise close instantly.
    #[arg(long, global = true)]
    pub pause: bool,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Read every known value once and flag anything implausible (default)
    Probe,

    /// Show the loaded drop, its version against the game's, and each root
    Info,

    /// Read one value by dotted path or by probe label
    Read {
        /// e.g. game.gameInstances.Controller.userData.redRubies, or "gems"
        target: String,

        /// Index for each collection the path crosses, outermost first
        #[arg(long, value_name = "N", value_delimiter = ',')]
        index: Vec<u64>,

        /// Print the value alone, with no label
        #[arg(long)]
        bare: bool,
    },

    /// Print values repeatedly until interrupted
    Watch {
        /// Paths or probe labels. Default: everything `probe` reads.
        targets: Vec<String>,

        /// Milliseconds between reads
        #[arg(long, default_value_t = 1000, value_name = "MS")]
        interval: u64,

        /// Stop after this many reads
        #[arg(long, value_name = "N")]
        count: Option<u64>,
    },

    /// List the ultimates bar: who is in it, which key fires it, cooldown
    Ultimates,

    /// Fire an ultimate, by champion id, champion name, or bar slot
    Ult {
        /// e.g. 83, "ellywick", or --slot 3
        #[arg(required_unless_present = "slot")]
        who: Option<String>,

        /// Bar slot instead of a champion
        #[arg(long, value_name = "N")]
        slot: Option<u64>,

        /// Fire even if the ultimate is still on cooldown
        #[arg(long)]
        force: bool,
    },

    /// Ellywick's hand from the Deck of Many Things
    Cards {
        /// Print only how many Gem cards are in hand
        #[arg(long)]
        gems: bool,

        /// A different champion's deck-style handler, by id
        #[arg(long, value_name = "ID")]
        hero: Option<i64>,
    },

    /// List a champion's effect keys, with the dict entry each sits in
    Effects {
        /// Champion name or id
        who: String,
    },

    /// List the value names `read` and `watch` understand (no game needed)
    Labels,

    /// List the paths the loaded drop knows (no game needed)
    Paths {
        /// Only paths containing this text
        filter: Option<String>,

        /// Show at most this many
        #[arg(long, default_value_t = 40, value_name = "N")]
        limit: usize,
    },

    /// Send keystrokes to the game window without focusing it
    Key {
        /// One or more key names, e.g. esc q w e
        #[arg(required = true)]
        keys: Vec<String>,

        /// Milliseconds between keystrokes
        #[arg(long, default_value_t = 120, value_name = "MS")]
        delay: u64,
    },
}

impl Cli {
    pub fn command(&self) -> &Command {
        self.command.as_ref().unwrap_or(&Command::Probe)
    }
}
