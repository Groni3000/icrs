; Local overlay: paths that RLee-EN/BrivMaster-Imports does not generate.
;
; Loaded after the upstream files, so anything here wins. The loader reports
; how many upstream paths an overlay replaces - if that count stops being zero,
; upstream has started generating the path and this file should lose it.
;
; Source: IC Script Hub's own IC_IdleGameManager64_Import.ahk (lines 220-221),
; verified against the live game - ID read 4171405, Hash a 32-character string.
; This is the route the AHK script uses because the GameSettings root, where
; UserID and Hash officially live, has a stale pointer path. See
; docs/pointer-chains.md.

this.game.gameUser.Hash := New GameObjectStructure(this.game.gameUser,"UTF-16", [0x20])
this.game.gameUser.ID := New GameObjectStructure(this.game.gameUser,"Int", [0x60])
