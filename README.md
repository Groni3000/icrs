# icrs
Idle Champions AHK replacement for Briv gem farming.

## Running

The game has to be open - the probe attaches to `IdleDragons.exe` and reads it.

One command works on both hosts:

```
./scripts/cargo.sh run --release            # probe
./scripts/cargo.sh run --release -- q       # probe, then send a key
```

On Linux it launches the exe inside the game's Proton container; on Windows it
is a plain passthrough to cargo. What follows is what that does per host.

### Windows

```
rustup target add x86_64-pc-windows-gnu     # once
cargo run --release                         # works directly, no wrapper needed
```

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

### Offsets

The V657 Steam drop is compiled in, so there is nothing to install. To use a
different one: `--imports <dir>`, `$ICRS_IMPORTS`, or an `imports/` directory
beside the exe. See [docs/updating-offsets.md](docs/updating-offsets.md).

## Docs

- [How a value is found in memory](docs/pointer-chains.md) - pointer chains,
  the three roots, and how offsets go stale.
- [Updating offsets after a game patch](docs/updating-offsets.md) - where the
  import files live and how they are found.
