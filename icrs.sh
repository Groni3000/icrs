#!/usr/bin/env bash
# Run icrs on Linux, from a terminal:
#
#   ./icrs.sh                      # probe
#   ./icrs.sh info
#   ./icrs.sh watch zone gems --interval 500
#
# (A file manager will usually need "run in terminal" for this, since the
# output is the whole point and nothing here holds a window open.)
#
# Finds an existing exe before building one, so this is also the right script
# to drop next to a downloaded icrs.exe. On Linux the exe has to run inside the
# game's Proton container, which scripts/run-in-prefix.sh arranges; the game
# must be running, started with STEAM_COMPAT_LAUNCHER_SERVICE=proton.
set -uo pipefail

here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
target="x86_64-pc-windows-gnu"

die() { echo "error: $*" >&2; exit 1; }

exe=""
for candidate in \
    "$here/icrs.exe" \
    "$here/target/$target/release/icrs.exe" \
    "$here/target/$target/debug/icrs.exe"
do
    if [ -f "$candidate" ]; then exe="$candidate"; break; fi
done

if [ -z "$exe" ]; then
    command -v cargo >/dev/null 2>&1 \
        || die "no icrs.exe here and cargo is not installed - build it elsewhere and put icrs.exe next to this script"
    echo "building icrs.exe ..." >&2
    (cd "$here" && cargo build --release) || die "build failed"
    exe="$here/target/$target/release/icrs.exe"
    [ -f "$exe" ] || die "build reported success but $exe is missing"
fi

launcher="$here/scripts/run-in-prefix.sh"
[ -x "$launcher" ] || die "missing $launcher - it is what puts the exe on the game's wineserver"

exec "$launcher" "$exe" "$@"
