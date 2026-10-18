# icrs
Idle Champions AHK replacement for Briv gem farming.

## Running

The game has to be open - the probe attaches to `IdleDragons.exe` and reads it.

Easiest, one launcher per host - both find an existing exe before building one,
so they also work dropped next to a downloaded `icrs.exe`:

```
icrs.bat                            # Windows: double-click, or
icrs.bat watch zone gems --interval 500
./icrs.sh                           # Linux (from a terminal), or
./icrs.sh info
```

`icrs.bat` pauses at the end so a double-clicked window stays readable; set
`ICRS_NO_PAUSE=1` to skip that. `icrs.sh` does not pause - it is meant for a
terminal, and sends the exe into the game's container for you.

Or plain cargo, which works on both hosts:

```
cargo run --release                 # probe
cargo run --release -- info         # any subcommand after --
cargo test
```

### Windows

```
rustup target add x86_64-pc-windows-gnu     # once
```

`cargo run` needs `bash` on PATH, because that is how the runner is spelled
(see `.cargo/config.toml`); Git Bash provides it. Without bash, run the built
exe directly - `target\x86_64-pc-windows-gnu\release\icrs.exe` - or build for
MSVC instead with `--target x86_64-pc-windows-msvc`, which needs no runner.

Double-clicking the exe runs `probe` and then waits for Enter, so the console
window does not vanish.

### Linux (Steam Proton)

The exe has to run inside the game's container to share its wineserver, which
is what `scripts/run-in-prefix.sh` does - cargo calls it for you. It needs the
game started with the launcher service:

```
# Steam -> the game -> Properties -> Launch Options, once:
#     STEAM_COMPAT_LAUNCHER_SERVICE=proton %command%
```

The unit tests touch neither the game nor Wine, so with the game closed they
can still run on the host:

```
cargo test --lib --target x86_64-unknown-linux-gnu      # 27 tests, no game
```

## Commands

```
icrs                     probe (the default)
icrs probe               read every known value once, flagging implausible ones
icrs info                the loaded drop, its version vs the game's, each root's health
icrs read <target>       one value, by probe label or dotted path  (--bare, --index)
icrs watch [targets...]  print repeatedly                          (--interval, --count)
icrs paths [filter]      explore the loaded drop - no game needed  (--limit)
icrs labels              the value names read/watch understand     - no game needed
icrs key <keys...>       send keystrokes without focusing the game (--delay)
icrs ultimates           the ultimates bar: champion, hotkey, cooldown
icrs ult <who>           fire an ultimate by name, id or --slot    (--force)
icrs cards               Ellywick's hand from the Deck of Many Things (--count TYPE)
icrs effects <who>       a champion's effect keys
```

Global: `--imports <dir>`, `--exe <name>`, `--pid <pid>`, `--pause`.

A target is either one of probe's labels (`zone`, `steelbones`, `gems`,
`user_id`, `hash`, `instance_id`, `web_root`) or any path from the drop:

```
$ icrs read gems --bare
13438
$ icrs read game.gameInstances.Controller.userData.ModronHandler.modronSaves.InstanceID
... 1
$ icrs watch zone steelbones --interval 400
    0.0s  zone=1 steelbones=7
```

Champions, their ultimates and their effect handlers:

```
$ icrs ultimates
slot  key  champion                     cooldown
   3    4  Briv (#58)                   ready
   7    8  Ellywick (#83)               ready
$ icrs ult ellywick
fired slot 7 (champion #83) with key 8
$ icrs cards
hand        Gem, Moon, Knight, Flames, Gem
knight      1
moon        1
gem         2
$ icrs cards --count moon
1
$ icrs effects briv | grep haste
 10  briv_unnatural_haste
```

### Offsets

The V657 Steam drop is compiled in, so there is nothing to install. To use a
different one: `--imports <dir>`, `$ICRS_IMPORTS`, or an `imports/` directory
beside the exe. See [docs/updating-offsets.md](docs/updating-offsets.md).

## Docs

- [How a value is found in memory](docs/pointer-chains.md) - pointer chains,
  the three roots, and how offsets go stale.
- [Updating offsets after a game patch](docs/updating-offsets.md) - where the
  import files live and how they are found.
