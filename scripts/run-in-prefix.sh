#!/usr/bin/env bash
# Run a Windows .exe inside the Proton container of a running Steam game, so it
# shares a wineserver with that game. Needed on Linux only; on Windows the exe
# runs directly.
#
# Why: ReadProcessMemory and PostMessage only reach the game from a process on
# the SAME wineserver. Tools that spawn their own prefix (protontricks-launch,
# a bare `wine`) land in a different one and see nothing.
#
# Requires the game's Steam launch options to contain:
#     STEAM_COMPAT_LAUNCHER_SERVICE=proton %command%
# and the game to be running - that is what publishes the D-Bus service we
# attach to.
#
# Usage:
#   run-in-prefix.sh [--appid N] <program.exe> [args...]
#   run-in-prefix.sh [--appid N] --env            # dump the container's env
#   run-in-prefix.sh [--appid N] --shell '<cmd>'  # run a shell command in it
#
# Env overrides: ICRS_APPID, STEAM_ROOT, SLR_DIR
set -uo pipefail

APPID="${ICRS_APPID:-627690}"   # 627690 = Idle Champions

die() { echo "error: $*" >&2; exit 1; }

# On Windows there is no container and no wineserver to join: the exe is native,
# so run it and get out of the way. This branch is why cargo's runner can point
# here on both hosts.
case "${OSTYPE:-}${OS:-}" in
  msys*|cygwin*|*Windows_NT) exec "$@" ;;
esac

case "${1:-}" in
  --appid) APPID="${2:?--appid needs a value}"; shift 2 ;;
esac
[ $# -ge 1 ] || die "usage: $(basename "$0") [--appid N] <program.exe> [args...]"

# --- locate the Steam installation -----------------------------------------
find_steam_root() {
    [ -n "${STEAM_ROOT:-}" ] && { echo "$STEAM_ROOT"; return; }
    local c
    for c in "$HOME/.steam/root" "$HOME/.steam/steam" \
             "$HOME/.steam/debian-installation" \
             "$HOME/.local/share/Steam" \
             "$HOME/.var/app/com.valvesoftware.Steam/data/Steam"; do
        [ -d "$c/steamapps" ] && { readlink -f "$c"; return; }
    done
}
STEAM_ROOT=$(find_steam_root)
[ -n "$STEAM_ROOT" ] || die "could not find a Steam installation (set STEAM_ROOT)"

# --- locate the launch client ----------------------------------------------
# Steam ships several runtimes (_soldier, _sniper, _4, ...); any of their
# launch clients can talk to the service, so take the newest one present.
find_launch_client() {
    [ -n "${SLR_DIR:-}" ] && { echo "$SLR_DIR/pressure-vessel/bin/steam-runtime-launch-client"; return; }
    local d
    # shellcheck disable=SC2012
    for d in $(ls -d "$STEAM_ROOT"/steamapps/common/SteamLinuxRuntime_* 2>/dev/null | sort -rV); do
        [ -x "$d/pressure-vessel/bin/steam-runtime-launch-client" ] && {
            echo "$d/pressure-vessel/bin/steam-runtime-launch-client"; return; }
    done
}
LC=$(find_launch_client)
[ -n "$LC" ] && [ -x "$LC" ] || die "steam-runtime-launch-client not found under $STEAM_ROOT (set SLR_DIR)"

# --- is the game's service up? ---------------------------------------------
if ! "$LC" --list 2>/dev/null | grep -q "App${APPID}\b"; then
    cat >&2 <<MSG
error: no launcher service for app $APPID.

  1. Steam -> the game -> Properties -> Launch Options:
         STEAM_COMPAT_LAUNCHER_SERVICE=proton %command%
  2. Fully close the game, start it again from Steam.
     (The service only exists while the game runs.)

Services currently available:
$("$LC" --list 2>/dev/null | sed 's/^/    /')
MSG
    exit 1
fi

# Keep our own working directory inside the container: without it the exe
# starts wherever the service happens to sit, and a relative path typed in the
# shell (a profile, an imports folder) does not exist from in there.
run_in() { "$LC" --inside-app="$APPID" --directory="$PWD" -- "$@"; }

case "${1:-}" in
  --env)   run_in /bin/sh -c 'env | sort'; exit $? ;;
  --shell) shift; run_in /bin/sh -c "$*"; exit $? ;;
esac

# --- find Proton's wine, from inside the container -------------------------
# The host has no matching wine build; these paths only resolve in there.
WINE=$(run_in /bin/sh -c '
  for c in "$WINELOADER" "$PROTON_DIST_PATH/bin/wine" \
           "$STEAM_COMPAT_TOOL_PATHS/files/bin/wine"; do
    [ -n "$c" ] && [ -x "$c" ] && { echo "$c"; exit 0; }
  done
  command -v wine 2>/dev/null' 2>/dev/null | tr -d '\r' | head -1)
[ -n "$WINE" ] || die "could not locate wine inside the container (try: $0 --env)"

EXE=$(readlink -f "$1"); shift
[ -f "$EXE" ] || die "not found: $EXE"

# Not run_in: exec cannot replace the process with a shell function.
exec "$LC" --inside-app="$APPID" --directory="$PWD" -- "$WINE" "$EXE" "$@"
