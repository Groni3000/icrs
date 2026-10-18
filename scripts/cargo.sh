#!/usr/bin/env bash
# cargo, for Linux hosts: builds the Windows exe as usual, then launches it
# inside the game's Proton container so it shares a wineserver with
# IdleDragons.exe. Without that, a read or a keystroke cannot reach the game.
#
#   ./scripts/cargo.sh run --release
#   ./scripts/cargo.sh run --release -- q
#   ./scripts/cargo.sh test                  # needs the game open, like run
#
# The same command works on Windows, where it is a plain passthrough to cargo -
# a native .exe needs no runner. So one command line serves both machines.
#
# The runner lives here rather than in .cargo/config.toml because cargo picks a
# runner per target, not per host, and this target is Windows on both hosts -
# on Windows the exe needs no runner and cargo cannot exec a shell script.
#
# Unit tests need neither the game nor Wine; run them on the host instead:
#   cargo test --lib --target x86_64-unknown-linux-gnu
set -euo pipefail

here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"

# On Windows the built exe runs by itself; setting a runner there would hand it
# to a shell script cargo cannot execute (os error 193).
if [[ "${OSTYPE:-}" != msys* && "${OSTYPE:-}" != cygwin* && "${OS:-}" != Windows_NT ]]; then
    export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUNNER="$here/run-in-prefix.sh"
fi

exec cargo "$@"
