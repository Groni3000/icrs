# How a value is found in memory

Nothing in this program knows a game value's address. Every read is a walk from
a fixed point through a chain of pointers, and each link can move when the game
updates. This is what the walk looks like and where each number comes from.

## The three layers

Reading the current zone:

```
mono-2.0-bdwgc.dll base        found at runtime by name; DIFFERENT EVERY LAUNCH (ASLR)
      + 0x74D200               static address inside that DLL; constant per game version
      -> deref + 0xF20         first pointer hop: the IdleGameManager instance
      -> deref + 0xE0          .game
      -> deref + 0xB0          .gameInstances          (a List<ChampionsGameInstance>)
      -> deref + 0x10          ._items                 (the backing array)
      -> deref + 0x20          [0]                     (element i is at 0x20 + i*8)
      -> deref + 0x28          .ActiveCampaignData
      -> deref + 0x88          .currentAreaID
                               read 4 bytes here = current zone
```

The walk rule is one line, and it is the same rule Script Hub's
`classMemory.pointer()` uses - which is why the published offsets can be used
verbatim:

```
addr = base
for off in offsets:   addr = [addr] + off
```

The last offset lands on the *field*, not the value. `resolve()` returns that
address; the caller reads it as the right type (i32, i64, or a Mono string
pointer).

Layer by layer:

| layer | where it comes from | when it changes |
| --- | --- | --- |
| module base | `module_base()` at startup, looked up by module name | every launch |
| `0x74D200` | `IC_Offsets.json` -> `IdleGameManager.moduleAddress` | rarely (a rescan) |
| `0xF20` | `IC_Offsets.json` -> `IdleGameManager.moduleOffset` | rarely (a rescan) |
| `0xE0`, `0xB0`, `0x28`, `0x88` | `IC_IdleGameManager_Import.ahk`, parsed | most game updates |
| `0x10`, `0x20` | Mono's `List<T>` layout - not in any data file | never |

We derive none of it. The maintainers of
[`RLee-EN/BrivMaster-Imports`](https://github.com/RLee-EN/BrivMaster-Imports)
rescan each game build and publish the results; `data/imports/` is a vendored
copy. See that directory's README for the provenance of the current drop.

## There is not one base - there are three

Chains do not all start from the same place. The feed publishes three
independent roots, and a chain rooted in one shares nothing with the others:

| root | static address | hops to the root object | static offset |
| --- | --- | --- | --- |
| `IdleGameManager` | `0x74D200` | `[0xF20]` | - |
| `CrusadersGame.GameSettings` | `0xA13548` | `[0x160, 0x868, 0xA0, 0x1B0]` | `0x250` |
| `UnityGameEngine.Core.EngineSettings` | `0x74A0F8` | `[0x2A0, 0xA80, 0xB0, 0x498, 0x40]` | `0x50` |

Everything the farm loop needs - zone, Steelbones stacks, gems, formation,
feats - hangs off `IdleGameManager`. The other two roots hold identity and
server details (`instance_id`, `UserID`, `Hash`, `WebRoot`) used to talk to the
game's web API.

The *static offset* is the extra wrinkle on those two roots: their import files
write field offsets as `this.StaticOffset + n` rather than a plain number, so
the parser keeps them symbolic (`Offset::Static(n)`) and applies the root's
static offset last. `IdleGameManager` has no static offset, which is why its
chains are plain numbers throughout.

## How this goes stale

`IC_Offsets.json` versions the two kinds of number separately, and they drift
at different rates:

```json
"Import_Version_Major": 657,     // struct field offsets, regenerated per build
"Pointer_Version_Major": 654     // static addresses, last rescanned at V654
```

So in the current drop the field offsets are generated against the running
build, while the static addresses are three versions old and merely *assumed*
to still hold. A root whose static address has moved produces a chain that
dies at hop 0 or 1 while every other root keeps working.

That is the failure mode to recognise, and it is why `resolve()` reports the
step that failed instead of a bare error. In the AHK script a broken chain
returned a blank string, which then compared as `"" >= 1710` and silently read
false - the same silent-failure class as both production bugs found while
porting. Here, a dead chain names its hop:

```
null pointer at step 3 (+0x1b0) - chain is stale or the game has not loaded
this structure yet
```

Run `cargo run` with the game open to see the whole set resolve, with a
per-hop trace for any chain that does not.

## What the probe shows today (V657, Steam)

`IdleGameManager` resolves - zone, Steelbones stacks and gems all read
correctly. Both other roots are dead, and the traces say why:

```
CrusadersGame.GameSettings
  hop 0: [mono+0xa13548] = 0x6bd463d0   ok
  hop 1: [0x6bd46530]    = 0x6bd46590   ok-looking, but adjacent to its own address
  hop 2: [0x6bd46df8]    = 0x4d8b48c0458b48b8   <- x86 instruction bytes, not a pointer
  hop 3: unreadable

UnityGameEngine.Core.EngineSettings
  hops 0-3 fine, then
  hop 4: [0x6e74a588]    = 0x8b48d88b48d3ff41   <- x86 instruction bytes
  hop 5: unreadable
```

Both walks start fine - so the static addresses are still valid - and then land
on executable code partway down. That is a stale *pointer path*, not a stale
static address: these deep `moduleOffset` chains come from a pointer scan
rather than from class metadata, and the intermediate objects have moved.

The upstream data confirms it. `Pointer_Version_Major` is `654` in both the
V656.1 and the V657 drop, with byte-identical `moduleAddress`/`moduleOffset`
for these two roots - the pointer block has been copied forward unchanged for
three builds while only the field offsets were regenerated. `Offsets_P11_V657_A`
is the newest drop published, so there is no newer scan to pull.

This is also the explanation for the same chain being dead in the AHK script:
nothing is wrong with our transcription or our walk, the published path no
longer holds on V657.

Consequence - narrower than it looks, because three of the four values have
another route:

| value | status |
| --- | --- |
| `UserID` | available as `game.gameUser.ID` under the healthy root |
| `Hash` | available as `game.gameUser.Hash` under the healthy root |
| `WebRoot` | dead, but the server-call class falls back to `master.idlechampions.com` and asks for the play server over the network |
| `instance_id` | dead, no alternative route found yet |

The AHK script already takes the first two routes (see the `ReadUserID` /
`ReadUserHash` patches in `IC_MemoryFunctions_Class.ahk`), which is the proof
that a value lost with a stale root can often be reached through the object
graph instead of by rescanning. So `instance_id` is the single genuinely
missing value, and every server call needs it as a form field.

An untested candidate for it: the game manager exposes
`modronSaves[i].InstanceID` and the active game instance, and the script
compares the two to find the live save. Whether that integer is the same one
the web API wants as `instance_id` has not been checked.

What it costs, concretely: `g_ServerCall.CallEndAdventure()` /
`CallLoadAdventure()` and `CallPreventStackFail` are no-ops. The client half of
a restart still works - `CloseIC` closes the window with a Win32 message and
`OpenIC` relaunches, neither of which needs any of this - so a recovery restart
reopens the game into the same adventure, without saving Briv's stacks across
it. Nothing in the normal farm loop reads these values.
