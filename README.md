# icrs
Idle Champions AHK replacement for Briv gem farming.

## Running

The game has to be open - the probe attaches to `IdleDragons.exe` and reads it.

One command works on both hosts:

```
./scripts/cargo.sh run --release            # probe
./scripts/cargo.sh run --release -- info    # any subcommand after --
```

On Linux it launches the exe inside the game's Proton container; on Windows it
is a plain passthrough to cargo. What follows is what that does per host.

### Windows

```
rustup target add x86_64-pc-windows-gnu     # once
cargo run --release                         # works directly, no wrapper needed
```

Double-clicking the exe runs `probe` and then waits for Enter, so the console
window does not vanish.

Or run the built exe straight: `target\x86_64-pc-windows-gnu\release\icrs.exe`.

### Linux (Steam Proton)

The exe has to run inside the game's container to share its wineserver, so
cargo needs a runner. `scripts/cargo.sh` sets one up for a single invocation:

```
# Steam -> the game -> Properties -> Launch Options, once:
#     STEAM_COMPAT_LAUNCHER_SERVICE=proton %command%
./scripts/cargo.sh run --release
./scripts/cargo.sh run --release -- q
```

Plain `cargo run` works on Windows but not on Linux, which is why the runner is
not in `.cargo/config.toml`: cargo picks a runner per *target*, and this target
is Windows on both hosts.

The unit tests touch neither the game nor Wine, so they can run on the host
with no container at all:

```
cargo test --lib --target x86_64-unknown-linux-gnu      # 27 tests, game closed
./scripts/cargo.sh test                                 # everything, game open
```

## Commands

```
icrs                     probe (the default)
icrs probe               read every known value once, flagging implausible ones
icrs info                the loaded drop, its version vs the game's, each root's health
icrs read <target>       one value, by probe label or dotted path  (--bare, --index)
icrs watch [targets...]  print repeatedly                          (--interval, --count)
icrs paths [filter]      explore the loaded drop - no game needed  (--limit)
icrs key <keys...>       send keystrokes without focusing the game (--delay)
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

### Offsets

The V657 Steam drop is compiled in, so there is nothing to install. To use a
different one: `--imports <dir>`, `$ICRS_IMPORTS`, or an `imports/` directory
beside the exe. See [docs/updating-offsets.md](docs/updating-offsets.md).

## Docs

- [How a value is found in memory](docs/pointer-chains.md) - pointer chains,
  the three roots, and how offsets go stale.
- [Updating offsets after a game patch](docs/updating-offsets.md) - where the
  import files live and how they are found.
