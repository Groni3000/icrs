# Vendored offsets

Source: <https://github.com/RLee-EN/BrivMaster-Imports>, `Offsets_P11_V657_A/`.

Platform `11` is Steam (`18` is CNE and reads as Steam, `21` is Epic);
`V657` is the game build these were generated against. The upstream repo
publishes one directory per game version, so updating after a patch means
dropping in a newer directory - not editing code.

These files are generated AutoHotkey source, parsed by `src/imports.rs` for
the offsets only. Two dialects exist and both are accepted: upstream's
`New IBM_GOS(...)` with decimal offsets, and IC Script Hub's
`New GameObjectStructure(...)` with hex.
