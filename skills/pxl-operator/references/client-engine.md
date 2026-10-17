# Learn the client's engine model and pipeline first

Art that is correct per file can still fail in the engine. Every late surprise of the reference project (a dynamic palette
slot collapsed to the first variant everywhere; sprite objects re-coloured into BG by a converter; art borrowed across zones;
frames dropped per act; palettes re-packed so the numbers were wrong) came from not knowing how the client loads the art.
Find out before drawing. Infer first; ask the client's engineers only what you cannot read.

## What to establish

| Question | Where to infer it | Why it matters |
|---|---|---|
| Scene unit: per act, per zone, per screen? Live scene switches? | the client's repo (loader, level format), spec.json, ENGINE.md | budgets are checked against composed real screens, not per object |
| Streaming vs resident art; who allocates VRAM | allocator source, DPLC-like scripts | resident cells count all the time, streamed per frame |
| Palette handling: fixed slots, dynamic slots and their variants, re-packing | converters (`conv_*.py`), palette tables | stable palette ids and per-scene tables in the manifest |
| BG objects and recolouring of sprite art into BG | converters, object lists | dual-use art needs hand-made BG-palette versions |
| OAM order, frames on screen together, repeats and compositions | object layouts, the engine's draw code | repeat/compose/collision rules belong in the manifest, not in prose |
| Frame semantics: distinct drawing or engine flip? | mappings (flip flags) | do not redraw a mirror the engine gets for free |
| Origins and collision footprints | the engine's object tables | origins in the manifest; collision width vs art width |
| Delivery format and checker | spec.json, the checker script, last delivery | `[delivery] spec`, `client_checker`, `package` in `pxl.toml` |
| Can we run their converter or emulator on our package? | tool chain | `pxl deliver --client-pipeline CMD` closes the loop with in-game screenshots |

## How

1. Ask for read access to the client's repo or the converter scripts; read them (they are shorter than the questions).
2. Put what you find in `decisions.md` as assumptions (`pxl decide "..." --by operator`), and the spec in `[delivery]`.
3. Budget against composed real screens when level layouts exist (`pxl scene SCENE --layout`, `pxl plan` line load on real layouts).
4. Before cutting anything for a budget, report what the cut buys under the *real* model.
5. Run the client's pipeline on the early dry-run package and look at the result before production.
