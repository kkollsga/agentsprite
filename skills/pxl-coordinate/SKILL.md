---
name: pxl-coordinate
description: Run many agents in parallel on pxl parts or assets, on any console: hand out work, set up templates and palettes, track progress, and feed approved frames back so later parts start better. Use when coordinating several pxl agents, setting up or maintaining a pxl.toml project, deriving a colour template, checking leases and stale parts, or managing a snippets library for a character family. For leading a whole project with the user (intake, design language, plan, delivery) use pxl-operator.
---

# pxl-coordinate: parallel agents without collisions

If `pxl --version` fails, `pxl` is not installed: run the installer (`curl -fsSL https://raw.githubusercontent.com/kkollsga/agentsprite/main/install.sh | sh`, Windows: `irm https://raw.githubusercontent.com/kkollsga/agentsprite/main/install.ps1 | iex`) or see the `pxl-operator` skill; `pxl skill install` refreshes these skills.

## Roles

- **Coordinator** (you): owns `pxl.toml` (manifest), `templates/`, snippet library, review and approval.
- **Artist agents**: each runs the `pxl-sprite` skill on one part at a time. They edit only their own part files.
- **Reviewer**: `pxl-review`. Can be the coordinator.

## Commands

```
pxl next --agent A1             # claim the next part needing work; prints PART, task, worst frames
pxl next --agent A1 --tag walk  # filter by tag; --kind sprite|tileset; --dir folder-or-glob
pxl claim PART --agent A1       # claim a specific part
pxl renew PART --agent A1       # extend the lease (every edit with --agent does it)
pxl release PART --state done --note "..." --agent A1   # or --state blocked --note "why"
pxl status                      # state, owner, stale, lint, export freshness, raw (frames/tiles not hand-finished)
pxl status --raw                # list the raw frames and tiles per part
pxl check --hand-finished       # as check, and fail while anything is still automatic output
pxl check                       # lint everything + exact output files; exit 0 only when all pass
pxl build                       # export every part (incremental)
```
Leases expire after 15 minutes idle; the part returns to the queue. A part owned by a live agent
refuses edits from others; read-only commands always work. `STALE` in status means the template or
input changed: `pxl reload PART`. Staleness compares content hashes of the inputs and template, so moving a part
file or upgrading the tool does not make it STALE.

## Step 0: the hardware plan, before any art

The hardware budgets are shared by everything on screen at once, so per-asset passes are not enough. Which budgets exist depends on the
console (`pxl console show`; `pxl console list` shows all nine): consoles with shared palette sets (gb, gbc, nes) budget sprite cells and
palettes per scene; the flat consoles (sms, gg, md, snes, pce, gba) limit each sprite on its own and per line. In a project made with
`pxl new`, `pxl plan` proposes all of this from the inputs (scenes from the groups, VRAM map, group budgets, palette plan, fit report); see
`pxl-operator`. In a hand-written manifest, before the first pixel of a batch:

1. **Palette plan per scene**: shared palette sets in `palettes/` (`pxl palette plan SCENE` builds them from all inputs). Parts used in several
   scenes list several sets (`palette_set = ["zone_1", "zone_3"]`).
2. **VRAM map per scene** in `pxl.toml`: `[scene.NAME]` with `palette_set`, `tileset`, `window` and `groups` (resident cells per group, streamed groups
   per frame). The numbers come from the console's modes (`pxl console show`). GBC example: 8 + 8 palettes, 40 sprites, 10 per line, 348 level
   tiles plus animated slots, 64 window tiles, 178 sprite cells, 1024 DMA bytes per frame.
3. Run `pxl check` and `pxl scene NAME --worst` on the empty plan, then after every batch. `pxl check` is **fail closed** on the shared-set consoles:
   sprite or tile parts without scenes, a sprite part without `palette_set` or in no scene group, or a scene over a shared budget fail it
   (`legacy = true` in pxl.toml is the explicit escape hatch for projects that predate the model). It prints "checked N of M" so a skipped
   part cannot hide, and counts `reference_missing` frames.
4. A client checker that passes is **necessary, not sufficient**: it validates files one at a time. Only the scene checks prove that the
   player, an enemy, a platform and a ring fit on one screen. Deliver with `pxl deliver` (or, in a manifest project, `pxl delivery --template
   DELIVERY_TEMPLATE.md --exceptions FILE --checker OUTPUT.txt`, which fills the checker output, the per-scene budget table, the parts list and the exceptions).

## Setting up well

1. **Inputs and parts**: `pxl load` from the source art (see `pxl load --help`), one part per character or
   object family, frames ordered by tags (stand, walk, jump).
   Game data (ROMs, disassemblies, dumps) is turned into PNG first (`pxl skill show pxl-extract`). Part setup: see
   `pxl skill show pxl-coordinate --ref sources` ("Setting up a part from images": `[input]`,
   `[[frames]]`, `[reduce]`, `[parts.X]`).
   Commands take a manifest key wherever they take a part (`pxl build enemy_07`).
   **Families are explicit.** The `template` lint compares template choice only between parts that declare the
   same `family = "name"` (in the part header, or `[parts.enemy_07] family = "enemies"` in `pxl.toml`). Give each
   object/group its own palette and family; parts without a family are never compared with each other.
   `pxl lock PART FRAMES|--all` / `pxl unlock` protect approved frames from `pxl reload`/`load` (only `--force` overrides).
   `pxl.toml` is found as `./pxl.toml`, `./pxl/pxl.toml`, then in each parent folder.
2. **Palette first.** Colours are shared by template, and wrong palettes make every frame worse.
   - `pxl template suggest PART --derive` prints a full derivation from the native-resolution inputs:
     colours per layer, overlay, an explicit `[map]`, an error report (try `--layers 2`, `--mode real|optimised`).
   - Identity protection: a saturated hue only replaces a template colour with a smaller own share, never a neutral covering >= 20% of the weight (grey casing stays grey), and the colour it uses is one of its own source colours.
   - Compare candidates with `pxl template suggest PART --compare A B`; write with `--write` (never overwrites existing templates).
   - A good template: base = outline/body/light, overlay = identity colours (shoes, gloves, white eyes).
   - Check that identity colours (bright shoes, gloves) have their own key; a lost shoe colour is a template problem, not an artist problem.
   - Templates are changed by the coordinator (`pxl template set`), never by artists.
   - **Template changes (a key was removed or renamed):** parts still storing the old key fail lint/export/reload. Rewrite the stored
     grids per part: `pxl template remap PART OLD=NEW [OLD=NEW ...] [--frames SEL] [--dry-run]` (NEW may be a role name; OLD need
     not exist any more; lock flags are kept; `pxl undo` reverts). `pxl reload --force` / `load --force` regenerate the selected
     frames even when they hold old keys, but frames not regenerated with unknown keys are refused with the remap command to run.
3. **Hardest frame first**: one agent does the key pose by hand, to the review bar; approve it. That frame is the style reference.

## Feedback loop

Locked and approved frames feed `exemplar` and `auto` reductions:
```
pxl reload PART --method auto    # re-reduce unlocked frames from approved/locked ones
pxl reload PART --try vote,contour,exemplar,auto   # compare methods, writes nothing
```
After approving the first good frame, reload sibling parts so their starting points inherit its style, then
dispatch the next agents. Re-running after each approval improves later frames.

## Snippets library

`pxl snippet save PART FRAME eye --region x,y,w,h` stores `templates/snippets/NAME.txt`, shared by the whole
template family (the `snippets/` folder next to the template). Curate it: save the approved eye (both facings by `stamp --flip x`), shoe, glove, muzzle.
Tell agents the snippet names; they `stamp` and adjust. A shared eye drawing is how a character looks consistent across 80 frames.

## Material language across parts

The project's design language is `style.md` (`pxl style propose`, approved by the user; the console's `[style]` is the default). Every brief to an agent
repeats the general rules, which hold on every console: eyes white with a pupil (as many as the source), identity colours survive, depth through
light and shade, draw for the cells, no flat fills, consistency across frames and siblings, and verify every claim at zoom.

Write a short materials note per project (shared colours for metal, hazard stripes, outlines, lights;
light direction; highlight style) and shared base templates (`extends`) per family (e.g. the villain's
machinery, enemy robots, items). Review families with a lineup of frames from different parts side by
side; inconsistency between related objects is a review defect like a lost eye.

## Tile sets and zones

One agent per zone tileset part (tile-range leases do not exist): `pxl next --kind tileset --agent NAME`, brief with the
skill `pxl-tiles`. Order for a zone: you set up the part and its template (`pxl template suggest PART --derive`), `pxl load`,
then the agent runs stats, fit, dedupe, `compare`, and hand-finishes every used tile. Animated parts (waterfall, flowers,
lava, torches) and fonts/screens are separate parts, one agent each, after the zone's blocks, so they match its palette and units.
Give neighbouring pieces (zones sharing a material, the screens that reuse a font) the same texture units: save them as snippets
in the family's `snippets/` folder and tell agents the names.

Gate for tile sets before approval: `pxl status --raw` (nothing raw), `pxl check --hand-finished` (opt-in, fails while any tile or
frame still equals the automatic output; plain `pxl check` stays usable mid-work), `pxl lint` 0 errors, within the console's unique-tile budget, then the client's checker PASS. Review textures with `pxl lineup` across zones. Sprites also obey the hardware limits (`hw_limit` error: over the console's cells per row band and per frame), so a wide or tall multi-part sprite must be split before it is a deliverable.

Everything is hand-finished, sprites included: after reviewing a part, run `pxl status --raw`; any frame that still equals its automatic
output goes back with a note.

## Sprite palettes and tile memory (palette-set projects)

In a project made with `pxl new` do not write these files: `pxl plan --apply` proposes the scenes and the shared set, `pxl scene set SCENE
group.player.budget N` changes a group budget, `pxl palette slot set | remove` edits a slot and `pxl asset set ASSET category player` allows the
player's stacked (two-slot) cells. For a hand-written manifest declare palette sets in `palettes/` and scenes in `pxl.toml` (`[scene.zone_2] palette_set = ".."`, `groups = { common = { budget = 56, parts = [".."] }, player = { streamed = true, per_frame = 7, parts = [".."] } }`; parts are keys, folders or globs).
`pxl budget SCENE|--all` reports unique 8x16 cells per group (flips free) and the biggest contributors; `pxl check` carries the same as
rule `tile_budget`. Parts enter a set with `pxl palette fit` (the fit moves the cell layout before it folds a colour), `reduce` for a
palette-set part picks the slot per cell, and `pxl cells dedupe` trims a part to a cell budget.

## Projects of assets: tasks, briefs, review (`pxl guide project`)

A project made with `pxl new` keeps one folder per asset (`assets/<group>/<name>/`): every write stores a version, refreshes `compare.png`
and prints the scene budget delta; never hand-edit `pxl.toml`, palettes or scenes (`pxl scene set | add-part | remove-part`,
`pxl palette slot set | remove`, `pxl group move`, `pxl asset set`, `pxl project set`). Hand work out with `pxl task add ASSET "text" --assign NAME`, `pxl next --agent A` (assigned
tasks first) and `pxl brief ASSET` (decisions, tasks, review history, budgets, paths: a fresh agent starts there). Record what the user
decides with `pxl decide "text" --asset A --by user`; an asset the user approved (`--agent user`) needs `--force-user-approved` to change.
Reviewers use `pxl review next | note | approve | rework` with their `--agent`; a note about a colour or direction carries `--region x,y,w,h`
(the crop `source | ours` is saved, a fix task is made). `pxl board` is your overview; escalate only round 3 and user decisions.

## Running the project as the operator (gated projects)

`pxl new` sets `[project] gates = true`: milestones (plan accepted, style approved, dry run passed) gate `pxl next`, and the design-language lint
gates (`eye_parity`, `identity_colour`, `flat`, `consistency`, `readability`) block `pxl review approve` unless the reviewer writes `--justify "why"`.
The whole operator method (infer, assume and record, show, ask only what cannot be inferred with options, images and a recommendation; intake, style
frames, plan, early dry run, reports, delivery) is the skill `pxl-operator`; extracting source graphics is `pxl-extract`.

## Dispatching agents

Brief each artist agent with: PART (or `pxl next`), their `--agent` name, the skill `pxl-sprite`,
the style reference frame (the approved style frames), `style.md`, and the note "you own the looks; lint is a hint". Run 4 to 8 at once; more
than one agent per part is not allowed. When an agent releases, review it (`pxl-review`) before its frames become exemplars.

## Checks before delivery

`pxl status` (no stale, no blocked, raw 0), `pxl check --hand-finished` exit 0, review every part, `pxl build`.
