# Updating offsets after a game patch

Offsets are data, not code. A new game build needs a file drop and no rebuild.

## What to do

1. Grab the matching directory from
   [`RLee-EN/BrivMaster-Imports`](https://github.com/RLee-EN/BrivMaster-Imports) -
   `Offsets_P11_V<version>_A` for Steam (`P18` is CNE, `P21` is Epic).
2. Drop its four files into the `imports/` directory beside the executable.
3. Run `icrs`. The second line of output tells you whether the drop matches the
   installed game:

```
imports     P11 V657 (pointers V654) from built-in - matches the game
            186 paths
```

Nothing is recompiled, and nothing needs editing.

## Where the program looks

First hit wins:

1. `--imports <dir>` on the command line
2. `$ICRS_IMPORTS`
3. `imports/` next to our own executable
4. the copy vendored into the binary at build time (`data/imports/`)

Exe-relative rather than working-directory-relative, so it behaves the same
whether the exe is launched by cargo, by a shortcut, or from a shell somewhere
else. A directory that is present but unreadable or malformed is an error - it
never falls back to the embedded copy silently, because the point of the whole
arrangement is to know which offsets are in use.

## Layout

```
icrs.exe
imports/
  IC_Offsets.json                 ← the roots: static address, hops, static offset
  IC_IdleGameManager_Import.ahk   ← required
  IC_GameSettings_Import.ahk      ← required
  IC_EngineSettings_Import.ahk    ← required
  extra.ahk                       ← optional local overlay
```

Upstream filenames are kept verbatim so a new drop is a copy, not a rename.

## The overlay

`extra.ahk` holds paths the feed does not generate - currently
`game.gameUser.ID` and `game.gameUser.Hash`, the route the AHK script uses
because the `GameSettings` root is stale. It loads last, so it wins.

The loader counts how many upstream paths an overlay replaces and prints a note
when that is not zero; a test asserts it is zero for the vendored drop. If
upstream starts generating a path the overlay also defines, that count goes up
and the overlay entry should be deleted rather than left to shadow fresher data.

## Why the version check is advisory

Two reasons it cannot be the gate:

- The game's version is not readable from memory. It lives in `GameSettings`,
  one of the stale roots, and reading it would be circular anyway - you need
  working offsets to learn which offsets to use. So it is read off disk, from
  `IdleDragons_Data/globalgamemanagers`, where Unity stores `bundleVersion`.
  That file reflects what is *installed*; if Steam patched the game while it was
  running, it can be ahead of the process in front of you.
- Matching versions do not mean working offsets. The V657 drop carries pointer
  data last rescanned at V654, and two of its three roots are dead on V657 - a
  version check passes that setup with a green light.

So the real check is structural: every wanted value is read and compared
against plausible bounds (`offsets::WANTED`), and a chain that breaks prints the
hop it died on. A wrong-but-readable offset shows up as

```
  zone         1798132488  <- IMPLAUSIBLE, expected -1..=100000
```

which is the failure a version comparison cannot see.
