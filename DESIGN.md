# AgentSprite: Design Document

Binary: `pxl` (working name)
Status: Draft 4
Language: Rust
License: MIT

## 1. Purpose

AgentSprite lets AI agents produce **picture perfect pixel art**: true-resolution sprites, tiles and screens
that look as close to the original high resolution art as the target resolution allows, are clean and crisp at
1×, and obey the target hardware's limits exactly.

The workflow is four steps, and the tool is built around them:

```
load  →  adjust  →  compare  →  export
```

1. **Load** high resolution images or source graphics; the tool produces a first reduction as editable text.
2. **Adjust** pixel by pixel, efficiently, with the same colour templates in every file.
3. **Compare** the inputs against the generated pixel art, for all frames at once: original | result |
   silhouette diff at the same physical size, plus metrics.
4. **Export** delivery files: indexed PNG and `.aseprite`.

Steps 2 and 3 repeat until every frame meets the quality bar (§1.1).

The tool is **optimised for many agents running this loop in parallel**, each on its own graphics files, with a
coordinator handing out work and reviewing results (§6).

It works for the classic 2D consoles (GB, GBC, NES, SMS, Game Gear, Mega Drive, SNES, PC Engine, GBA; §5.5): a project
sets its console, and the console's data (palettes, cells, per-line limits, export format) decides what is legal. The first
large production job, a commercial Game Boy Color port, shaped many features; its lessons are in Appendix A. The main text stays
console-generic.

### 1.1 What "picture perfect" means

| Criterion | Measured by |
|---|---|
| **Faithful shape:** silhouette within ±1 px per side of the input at target scale; proportions kept. | `compare` IoU and bbox delta, `ref_scale` lint |
| **Key features survive:** eyes and pupils, faces, mouths, separators between body parts (e.g. face and stomach), hands, feet, 1 px lines, points of spikes. | `compare` lost-features list; review against golden frames |
| **Clean:** no orphan pixels, no jaggies, no checkerboard noise, no blur or scaling artefacts, no dithering unless the original has it. | `orphan`, `jaggy`, `noise` lint |
| **Consistent:** same origin and ground line through an animation; unchanged parts identical between frames; one colour template per part family; same outline approach and light direction everywhere. | `jitter`, `shimmer`, `template` lint |
| **Readable at 1×:** clear against the level backgrounds; player, enemies and hazards distinguishable. | `compare --on <background>` |
| **Seamless tiles:** neighbouring tiles join in every combination the levels use. | `compare` tile pages, lineups |
| **Hardware legal:** colours per layer and per tile, palette counts, RGB555, unique tiles, hardware sprite budget. | profile lint, `check` |

Automatic reduction gives the baseline; the quality comes from the adjust/compare loop on every frame. The
tool's job is to make that loop fast, precise and measurable.

### 1.2 Lessons from the first attempt (Python scripts + Aseprite MCP)

| Lesson | Requirement |
|---|---|
| Driving a GUI editor through MCP added only overhead and failure points. | CLI; `.aseprite` written and read natively. |
| Automatic reduction lost shapes, eyes, separators and thin lines; the first approved frame was hand-drawn. | Pixel-level adjustments must be cheap: coordinate edits, rows, lines and fills, batches, undo, locking. |
| The most useful view was input vs output for every frame at the same size, plus a silhouette diff. | `compare` produces exactly that for all frames, paginated, with metrics. |
| Automatic palettes picked black/blue/white for a blue-and-skin character; colours drifted between files. | Shared colour templates with named roles; parts reference them; drift is a lint error. |
| Parallel agents edited shared scripts and broke each other's output. | No per-project code; agents edit only their own part files. |
| The session died and nine agents' runs ended without reports. | Every command is a pure function of files; `check` shows the exact state. |
| Nine agents ran in parallel with no shared view of progress and no way to hand work over. | Work queue with expiring leases, per-part status files, `pxl status` for the coordinator (§6). |

## 2. Scope

### In scope (v1)

- **Load:**
  - high resolution PNG images (game data is extracted to PNG first: the `pxl-extract` skill), at an integer factor or an explicit target size
- **`.pxl` text parts:** frames, origin, overlay layer, input link per frame, locking.
- **Colour templates** (§4.3) shared across parts.
- **Reduction** with origin- or tile-anchored grids, template colour mapping, and preservation of details and
  thin bands.
- **Pixel-level adjustment commands** (§5.2) with minimal output and undo.
- **`compare`** sheets for all frames and metrics; lint with fix suggestions; GBC profiles matching the client
  checker.
- **Tile sets, levels and screens:**
  - block-aligned reduction and palette fitting
  - unique tiles with flips deduplicated
- **Export:**
  - indexed PNG and `.aseprite` (read and write)
  - project manifest with `build` and `check`

### Out of scope (until the GBC delivery is done)

- Source formats from other games or consoles.
- Fake pixel art grid detection, photo conversion, dithering, automatic shading.
- GIF export, sheet packing, MCP server, Python bindings, GUI.

## 3. Architecture

One crate, one binary.

```
src/
  main.rs        CLI (clap), JSON output, exit codes; no algorithms
  model.rs       Part, Frame, Template, Layer, Profile
  pxl.rs         .pxl parse + canonical serialize
  template.rs    colour templates, colour maps
  color.rs       sRGB/Oklab (deterministic), RGB555 snapping
  png.rs         indexed PNG read/write
  ase/           .aseprite read/write
  load.rs        inputs → reference frames → first reduction
  reduce.rs      reduction algorithm
  edit.rs        pixel-level adjustments, journal/undo
  compare.rs     comparison sheets, metrics
  lint.rs        rules, profiles
  tiles.rs       tile sets, palette fitting, levels
  export.rs      PNG/.aseprite export, project build/check
consoles/        <id>/console.toml, sprite.toml, tile.toml, screen.toml for gb gbc nes sms gg md snes pce gba (compiled in; a project may override from disk)
templates/       shared colour templates (per project)
test-projects/   private data derived from third-party games (git-ignored, never packaged)
examples/        original CC0 inputs, a generator and two demo scripts (single image, project)
skills/          the agent skills (also the Claude Code plugin's skills)
```

| Need | Crate |
|---|---|
| PNG | `png` |
| zlib (`.aseprite`) | `miniz_oxide` |
| CLI | `clap` (derive) |
| Files | `serde`, `toml`, `serde_json` |
| Errors | `anyhow` |

Oklab and k-means are written in house. Colour maths is bit exact on every
platform: a constant sRGB → linear table, a Newton cube root, no `libm` `powf` in output paths, ties broken by
palette index. There is no randomness.

## 4. Files

### 4.1 Inputs

**High resolution PNG:** any image with a transparent or flat background. The part gives either an integer
`factor` (the exact path, the default for pixel art sources) or a `target` size (area reduction in Oklab, for
illustrations).

### 4.2 Part (`.pxl`)

One part = one set of frames that share a canvas, an origin and a colour template (one delivery folder or
group).

```toml
name = "hero"
size = [32, 32]
origin = [17, 16]
template = "templates/hero.toml"       # §4.3

[input]                                 # high resolution source (§4.1)
images = "refs/{frame}.png"             # with factor or target
factor = 2

[reduce]                                # §5.1
factor = 2
phase = [-1, 0]
outline = ["outline"]                   # template roles
thin = true

[[frames]]
name = "01_stand"                       # export name
src = "Stand"                           # picture name, or image path
locked = true                           # adjusted by hand: reload never overwrites it
grid = """
................................
..............11................
...........11222221..1..........
"""
```

Rules:

- Rows must have length `size[0]`, row count `size[1]`. Unknown TOML keys are errors.
- One grid per frame holds every layer; the template says which key belongs to which layer.
- Tile sets: `mode = "tile"` (old: `kind = "tileset"`), `tile = [8, 8]`, grid width a multiple of 8. Screens: `mode = "screen"`,
  160×144.

### 4.3 Colour templates

A template fixes the meaning of every key for a family of parts, so the same character means the same colour
role in every file, and a colour changed once changes everywhere.

```toml
# templates/player.toml
name = "player"

[layers.base]                        # hardware palette 1
"." = { role = "transparent" }
"1" = { role = "outline",   color = "#182078" }
"2" = { role = "body",      color = "#4850b8" }
"3" = { role = "skin",      color = "#f0b090" }

[layers.detail]                      # hardware palette 2 (overlay) → {frame}_detail.png
"W" = { role = "white",     color = "#f8f8f8", under = "3" }
"R" = { role = "shoe",      color = "#f80000", under = "1" }
"r" = { role = "shoe_dark", color = "#900000", under = "1" }

[map]                                # input colour → key, shared by every part using the template
"#000000" = "1"
"#242492" = "2"
"#ffffff" = { key = "W", near = ["#000000", 2] }
"#ff0000" = { key = "R", near = ["#920000", 3] }
```

- **Conventions:**
  - `1` is always the darkest base colour (outline)
  - `.` is always transparent
  - overlay keys are letters
- **Inheritance:** templates may `extends = "templates/gbc-common.toml"` for project-wide conventions, e.g. a
  shared enemy outline colour or the zone palettes.
- **Tile templates** list up to 8 palettes of 4 colours, named per zone (`grass`, `rock`, `sky`, ...). Tiles refer to
  them by key, so tiles in the same zone stay consistent.
- `template suggest <part>` proposes a template from the inputs (scored candidates: lightness tones, Oklab
  clusters, darkest tone plus clusters, weighted medoids). It never overwrites an existing template.

Acceptance:

- AC-TP1: Changing a template colour and running `export` changes that colour in every part using it, and
  nothing else.
- AC-TP2: A part whose grid uses a key missing from its template fails with file, frame, row and column.
- AC-TP3: `template` lint flags parts in the same family that use different templates or local colour
  overrides.

### 4.4 Outputs

**Indexed PNG.**
- PLTE in template order; index 0 transparent through tRNS; all other entries opaque.
- Fixed encoder settings, so output is byte identical across runs and platforms.
- Tile sheets and screens have no transparency.

**`.aseprite`,** read and written per the published file format (Aseprite 1.3):
- **Header:** colour modes indexed, RGBA and grayscale; grid.
- **Layers:** image, group and tilemap layers, with blend mode, opacity and visibility.
- **Cels:** raw, compressed and linked; tilesets.
- **Frames and tags:** frames with durations; tags.
- **Other chunks:** palettes (new and old), slices, user data and properties.
- **Unknown chunks** are kept unchanged on write.

Export writes:
- one layer per template layer
- one frame per `.pxl` frame, one tag per frame name plus the animation tags
- the origin as a slice pivot
- tile sets as tilemap layers

Import reports anything `.pxl` cannot express instead of dropping it silently.

**Manifest `pxl.toml`:** maps parts to delivery paths (`out = "characters/player"` → `{frame}.png`,
`{frame}_detail.png`, `ase = ".../player.aseprite"`).

## 5. Commands (in workflow order)

All coordinates are 0-based `x, y` from the top left, the same as the rulers in `show` and the labels in
`compare`.

### 5.1 Load

- **`load <part.pxl>`**: reads the inputs, renders the high resolution reference for every frame, and writes a
  first reduction into every unlocked frame grid.
- **`load --new <name> --input <image|spec> --size WxH --origin X,Y --template T`**: creates a part.
- **`reload <part> [--frames ...] [--force]`**: regenerates unlocked frames after input, template or reduce
  setting changes.

Reduction (one phase per part, never per frame):
1. **Alpha:** majority of the block, ties decided by ring coverage.
2. **Colour:** the template `map` (with `near` filters), otherwise the nearest key in Oklab. Votes come from the
   block (weight 1) and the ring (weight 0.2). Two bonuses protect features:
   - a key in the block but absent from the ring (an isolated detail)
   - a 1–2 px band that continues through the ring (a separator, gap or line)
3. **Smoothing:** iterated conditional modes; protected cells never flip.
4. **Outline** for the given roles, then orphan cleanup.
5. **Overlay keys** are reduced the same way.

Acceptance:

- AC-LD3: A ×2 nearest-neighbour upscale loads back exactly; shifting the input by `factor` px shifts the output
  by exactly 1 px.
### 5.2 Adjust (pixel by pixel)

Designed for many small, precise edits per frame with as few tokens as possible.

| Command | Example | Effect |
|---|---|---|
| `set` | `pxl set player 01_stand 17,11=W 18,11=W 19,11=1` | individual pixels (key or template role: `19,11=outline`) |
| `row` | `pxl row player 01_stand 16 13 "1222221"` | write a run of pixels starting at x on row y |
| `patch` | `pxl patch player 01_stand 12 6 -` (grid on stdin) | overwrite a rectangle; space = unchanged |
| `line` | `pxl line player 01_stand 9,12 14,12 1` | 1 px line (pixel-art Bresenham, no doubled steps) |
| `rect`, `fill` | `pxl fill player 01_stand 15,17 3` | rectangle; flood fill (4-connected) |
| `replace` | `pxl replace player 01_stand 2=1 --region 9,6,6,8` | swap keys in a region |
| `copy` | `pxl copy player 01_stand 02_wait1 --region 9,6,14,10` | copy pixels between frames (keeps unchanged parts identical) |
| `mirror`, `flip` | `pxl mirror enemy 03_fly x` | symmetric objects |
| `write_grid` | `pxl write_grid player 05_look_up -` | replace a whole frame |
| `apply` | `pxl apply player ops.json` | atomic batch of any of the above, across frames |
| `undo` | `pxl undo player [--steps N]` | revert the last edits (journal next to the part) |
| `clean` | `pxl clean player --orphans --jaggies --dry-run` | report or fix stray pixels and jaggies |

- Every edit prints only what changed: the edited region as a small grid with rulers, plus the changed pixel
  count (JSON with `--json`). The agent sees the effect without re-reading the frame.
- **`show <part> <frame> [--region x,y,w,h] [--ref]`**: the frame as text with rulers. `--ref` adds the input
  next to it as letters at native resolution (rows labelled `12a`/`12b` in target coordinates) for
  pixel-precise redrawing.
- Edits lock the frame (`locked = true`) unless `--no-lock`. The journal (`<part>.journal`) records every edit
  for `undo` and review.

Acceptance:

- AC-AD1: Every edit is atomic; invalid input leaves the part byte identical and names what is wrong (pixel,
  key, out-of-canvas coordinate).
- AC-AD2: No edit changes pixels outside its target; `patch` never changes pixels under spaces.
- AC-AD3: `undo` after N edits restores the part byte identical.
- AC-AD4: Each command finishes in under 20 ms on a 160×144 frame; output for a 1-pixel edit is under 15 lines.

### 5.3 Compare

**`compare <part> [--frames ...|--all] [--on <background>]`** generates the comparison graphics for every frame
:

- **Per frame:**
  - input at native size | generated pixel art upscaled by `factor` | silhouette diff (grey both, red input
    only, green output only)
  - same physical size, aligned by the part's phase
  - frame name and lint markers
- **Sheets:** contact sheets of all frames, paginated so each image's long side is ≤ 1568 px (the size vision
  models see without downscaling); `--zoom` gives one frame large.
- **`--on`:** puts the generated frames over a level background, to judge readability at 1×.
- **Metrics JSON per frame:**
  - silhouette IoU and bbox delta per side
  - mean Oklab error
  - lost features: input details and thin bands with no counterpart, with their coordinates
  - lint issues
  - a part summary that ranks frames worst first, so the agent knows where to work next

Also: `diff <a> <b>` (pixel differences between two versions) and `lint <part>` (§5.5).

Acceptance:

- AC-CP1: A perfectly reduced frame shows no red or green fringe and an empty lost-features list.
- AC-CP2: Sheets for 87 frames are generated in under 1 s; no image exceeds 1568 px on its
  long side.
- AC-CP3: Previews are exact nearest-neighbour upscales; labels and gutters never change cell centres.

### 5.4 Export

- **`export <part> [--out dir]`**: indexed PNGs (base and overlay per frame), tile sheets, screens, `.aseprite`.
- **`build`**: exports every part in `pxl.toml`; incremental.
- **`check`**: lint on every part, plus exact expected file names (including `.aseprite`) and no unlisted
  images; one JSON summary; exit 0 only when everything passes.
- **`import <png|grid.txt|.aseprite>`**: brings existing images, grids or artist edits back into a part.

Acceptance:

- AC-EX1: A golden grid with its template exports PNGs pixel identical to the golden PNGs.
- AC-EX2: Exported `.aseprite` files open in Aseprite 1.3 without warnings;
  `aseprite -b part.aseprite --save-as "{tag}.png"` reproduces the PNG export; read → write keeps every chunk.
- AC-EX3: All outputs are byte identical across runs; `build` writes only declared outputs.
- AC-EX4: `check` over the full project (≈ 800 frames, 6 tile sets, 5 screens) completes in under 1 s.

### 5.5 Consoles and image modes

A **console** is a folder of data. `consoles/<id>/console.toml` holds the hardware facts every image mode shares
(screen size, colour model, palette counts, sprite cell, OAM, VRAM, DMA); `sprite.toml`, `tile.toml` and `screen.toml`
are the **image modes** and inherit every key of `console.toml`. Built-in consoles are compiled into the binary
(`gbc` now); `pxl console list` shows them and `pxl console show [ID]` prints the merged facts per mode.

```toml
# pxl.toml
[project]
console = "gbc"          # pxl console set gbc; a project without it uses gbc and lint / check say so
```

```toml
# a part header
mode = "sprite"          # sprite (default) | tile | screen; `kind = "sprite" | "tileset" | "screen"` is the old name
```

- **Profile = console + mode.** A part's rules are `<console>/<mode>` (`gbc/sprite`, `gbc/tile`, `gbc/screen`); the console
  comes from the nearest `pxl.toml`, the mode from the part. `pxl lint --profile gbc/tile` or a part key
  `profile = "gbc/tile"` picks another one; a path to a .toml file still works.
- **Project overrides.** A file `consoles/<id>/<file>.toml` found above the part (the project folder) overrides single
  keys of the built-in file of that name; tables such as `[budget.enemy]` merge key by key. A console that exists only on disk
  (`consoles/mini/console.toml`) is a console too. Unknown keys are errors. Example: the old BG rules are
  `consoles/gbc/tile.toml` with `unique_tiles_max = 300`, `count_animated = false`, `pattern_unique = false`,
  `bg_reserved = []`, `vram_share = false`.
- **Old names.** `profile = "gbc-sprite"`, `"gbc-tile"`, `"gbc-screen"` still resolve to `gbc/sprite`, `gbc/tile`, `gbc/screen`;
  `lint` and `check` print one deprecation line. A project file `profiles/<name>.toml` still replaces such a profile as a whole
  (keys it omits take the plain defaults). `gbc-tile-legacy` is removed: use the override above.
- **`pxl migrate [--dry-run]`** rewrites an old project: adds `[project] console`, turns `kind` into `mode`
  (`tileset` into `tile`), drops `profile = "gbc-*"` when it equals the part's console and mode, otherwise writes
  `console/mode`, and reports every change. Parts change through the journal (`cmd` = `migrate`, no pixels touched, `pxl undo`
  restores the old text); their recorded status hashes are carried over, so export freshness and approval do not change.
  `profiles/gbc-*.toml` files are kept and reported. A second run changes nothing.
- **Facts in `console.toml`** (GBC): `channel_step = 8` (RGB555), `tile`, `hw_cell = [8, 16]`, `hw_cells_max = 40`,
  `hw_cells_per_row_max = 10`, `palettes = 8` (BG), `obj_palettes = 8`, `colors_per_tile = 4`, `vram_banks`,
  `vram_tiles_per_bank`, `sprite_cells`, `window_tiles`, `vram_shared_tiles`, `obj_tiles_max`, `bg_tiles_max`,
  `dma_bytes_per_frame`, `sheet_width`; a `[console]` table carries name, `screen = [160, 144]` (the `screen` mode takes its
  `size` from it) and the references. Mode files hold what differs: `sprite.toml` (`colors`, `hw_cells_warn`, overlay policy,
  category budgets `[budget.<category>]`), `tile.toml` (`unique_tiles_max = 348`, `count_animated`, `pattern_unique`,
  `bg_reserved = [7]`), `screen.toml` (`unique_tiles_max = 360`).

| Rule | Severity | Detects |
|---|---|---|
| `size`, `palette_size`, `rgb_step`, `alpha`, `shared_palette`, `empty_frame` | error | Format and colour limits |
| `tile_colors`, `tile_palettes`, `unique_tiles` | error | Tile limits |
| `template` | error | Key not in template; local colour override; parts of one family on different templates |
| `hw_cells` | warning | Hardware sprite budget |
| `hw_limit` | error | More than 10 sprite cells in a 16 px band or 40 per frame (hardware limit); palette-set parts: the category limits per real scanline (5.5a) |
| `priority_color0` | error | Priority tile whose see-through part is not colour 0 of its palette (5.6a) |
| `palette_set`, `cell_palette` | error | Palette-set parts: colour not in the set; an 8x16 cell no single slot can hold (5.5a) |
| `orphan`, `jaggy`, `noise` | warning | Stray pixels, doubled diagonals and L corners, checkerboard patches |
| `jitter`, `shimmer` | warning | Origin or ground line moves within a tag; pixels change between frames where the input does not |
| `ref_scale`, `lost_feature` | warning | Bbox off by more than 1 px per side; input details missing |
| `eye_parity`, `identity_colour`, `flat`, `consistency`, `readability` | warning | Design-language gates (5.5g): default only in projects with `gates = true`; block approval without `--justify` |

- AC-L1: Every rule has 3 positive and 3 negative fixtures.
- AC-L2: JSON with `"schema": 1`; each issue has `rule`, `severity`, `file`, `frame`, `x`, `y`, `message`, and
  `fix` (a ready-to-run `set`/`row`/`apply` command) where possible.
- AC-L3: The `gbc` modes give the same verdicts as an external acceptance checker on its fixtures (optional: `$PXL_CHECK_ASSETS`).

#### Hardware model and the built-in consoles (M6 R2)

The profile keys that describe a console, beyond the GBC ones above (all optional; the defaults are the Game Boy Color):
`colour_bits` (bits per channel; 0 = a fixed list), `channel_step` (the 8-bit step of a channel model: GBC 8, GG 16, MD 32, SMS 64),
`fixed_colours` (NES master palette, GB shades: lint `rgb_step` names the console palette, export and derive snap to the nearest entry),
`colors` (colours per palette line without transparent), `obj_palettes` (palette lines for sprites: lint `palette_lines` limits the colour
layers of a part), `shared_bg_obj`, `hw_cell`, `sprite_size_mode` (`fixed`, `global`, `variable`) with `sprite_sizes` (pixels),
`hw_cells_max` / `hw_cells_per_row_max` (sprites on screen / per scanline), `hw_pixels_per_line_max` (sprite pixels per scanline),
`colors_per_tile` and `palettes` (tile modes; `tile_palettes` packs colour sets into palettes of that many colours), `attr_block` (NES
16x16: lint `attr_block`), `dedupe_flips` and `export_palette`. With `sprite_size_mode = "variable"` (Mega Drive) `sprites::place` covers
each frame with allowed sizes (all 64 offsets of the 8x8 grid, greedy by tiles gained, fewest sprites then least area) and `hw_limit` checks
sprites per frame, per scanline and pixels per scanline of that cover. A console with `export_palette` makes `pxl export` write
`<name>.palette.json` (one line per colour layer, index 0 transparent, `colors + 1` entries per line, every colour in the console's own
value: md `0BBB0GGG0RRR0` word, sms `00BBGGRR`, gg `0BBBBGGGGRRRR`, nes master index, gb shade) and, for variable sizes, `<name>.sprites.json`.
On the SNES a sprite size is one of the two of the chosen OBSEL pair (`sprite_sizes`; override it in `consoles/snes/sprite.toml`), and the
34-sliver limit is modelled as 272 sprite pixels per scanline; the GBA cycle budget is the same pixel limit (n cycles for an OBJ n pixels
wide; rotation/scaling costs 2n+10 and is not modelled); 8bpp GBA is `colors = 255`, `obj_palettes = 1` as an override. SNES, PCE and
GBA assets use one palette per sprite (the flat first version), not per-cell palettes.
The palette-set cell model (`palette_set`, scenes with sprite VRAM and DMA budgets) stays Game Boy Color; the other consoles use the
per-layer model, and their `sprite_cells` / `dma_bytes_per_frame` are ceilings, not budgets (planned for R6).

**Factor 2 and up: the `feature` method.** `image` / `ingest` of a sprite at factor >= 2 write `[reduce] method = "feature"` (`reduce::feature_grid`):
`vote` fills, then (1) *outline*: when the template's outline key draws most of the source silhouette's edge, every cell whose block holds a full
line of it (`f` pixels) becomes outline, so a 1-source-pixel outline (half a cell) survives as a 1-px outline and thin inner lines stay;
(2) *identity colours* (`protect_salient`); (3) *eyes* (`features::stamp_forced`): every detected eye is drawn at its scaled position as a white
rectangle (the reference white at target scale, at least 2 cells, corners cut from 3x3) with a pupil block on the gaze side, whether or not the
vote kept something eye-like. A flat template may now have up to 16 colours (keys 1-9, a-g), so a small saturated area (red shoes) keeps its own
colour; in the cell fold a pixel that is not white never takes a white (light skin became a second highlight) and a grey level does not take the
console's white only because the cell holds three levels. The design gate `eye_parity` takes the console's brightest white (SMS `#c0c0c0`) as its
threshold.

**The first version is hardware-valid and keeps the design** (`image`, `ingest`; `src/autoset.rs`). Profile key `auto_template`: `flat`
(gb, sms, gg, md) derives one layer of at most `colors` colours (the console colour model: channel step or fixed palette; a sprite is one
palette line there, and overlay layers are never added automatically); `set` (gbc, nes) reduces flat with up to 24 colours first, then
**learns the palette slots from the cells** into an asset-local set `<asset>/palettes/<name>.toml` (`console = ".."`, one slot per OBJ
palette, three distinct colours each):
1. *materials*: colours group into hue families (a gap of more than ~28 degrees starts a new family) plus one neutral family; each family
   is a dark / mid / light ramp, the seed of a slot. With too few slots the cheapest merge goes first (Ward cost, Oklab with chroma counted
   three times and the weight raised for saturated colours), so a distinct hue is not folded into a grey while a slot is free.
2. *learning*: every 8x16 cell goes to the slot that holds its colours best, every slot takes the three colours that minimise the weighted
   error of its cells; free slots go to the cell that gains most from a palette of its own while that lowers the total error.
3. *eyes and white*: an eye is a group of at least three near-white pixels with a dark pupil the white surrounds (three or more white
   neighbours); both get a large weight in the cell of the white, with a penalty per white pixel when the slot lacks the white or the pupil.
   White is a light only where the input holds near-white in that cell: a slot that gets white for a cell without near-white pays a
   penalty, so ramps use the material's own lightest colour. `add_guard_keys` gives a source colour that derive mapped onto a near-white
   key its own key. The cell grid is searched over 4x4 offsets and the layout with the lowest total error is kept.
4. every cell is then folded into its slot on the canvas-aligned grid (so each cell uses one slot and the searched placement finds
   zero invalid cells), not by the area-counting M5 fit. A later `reload` uses the M5 fit again.
A single image, or a project without `[project.defaults] category`, has no category: the part gets `[budget]` with the hardware limits
(gbc 10 per scanline, 40 per frame; the NES categories carry only the hardware limits too). Fixed palettes (NES, GB) snap with the
chroma-weighted Oklab distance and a neutral source only maps to a neutral entry. What still fails lint (a figure wider than the
scanline limit) is kept as the best version, printed as `needs hand work: <rule> (Nx): <message>` and recorded as one `fix` task per rule
(`pxl tasks --asset A` also for a single-image asset). Every derive path ends in `snap_template_text`, so no key colour is outside the
console colour model.

| Console | Screen | Colours | Sprite palettes | Sprites (screen / line) | Sprite sizes | Tile palette |
|---|---|---|---|---|---|---|
| gbc | 160x144 | RGB555 | 8 x (3 + transparent) | 40 / 10 | 8x16 cells | 8 BG x 4 |
| gb | 160x144 | 4 fixed shades | 2 x (3 + transparent) | 40 / 10 | 8x8 or 8x16 | 1 x 4 |
| nes | 256x240 | 55 of the 2C02 master palette | 4 x (3 + transparent) | 64 / 8 | 8x8 or 8x16 | 4 x 4, per 16x16 |
| sms | 256x192 | 64 (2 bits per channel) | 1 x (15 + transparent) | 64 / 8 | 8x8 or 8x16 | 2 x 16 |
| gg | 160x144 | 4096 (4 bits per channel) | 1 x (15 + transparent) | 64 / 8 | 8x8 or 8x16 | 2 x 16 |
| md | 320x224 | 512 (3 bits per channel) | 4 shared lines x (15 + transparent) | 80 / 20, 320 px per line | 1-4 x 1-4 tiles | 4 x 16 shared |
| snes | 256x224 | 32768 (BGR555) | 8 x (15 + transparent) | 128 / 32, 272 px (34 slivers) per line | two sizes of one OBSEL pair (default 16x16 / 32x32) | 8 x 16 |
| pce | 256x224 | 512 (9-bit, GRB) | 16 x (15 + transparent) | 64 / 16, 256 px per line | 16 or 32 wide x 16, 32, 64 high | 16 x 16, no BG flips |
| gba | 240x160 | 32768 (BGR555) | 16 x (15 + transparent) (4bpp) or 1 x 255 (8bpp) | 128 / cycle budget 1210 (a normal OBJ of n px costs n) | the 12 OBJ shapes 8x8 to 64x64 | 16 x 16 |

Every fact and its reference is in `consoles/<id>/console.toml` (`pxl console show ID`); facts that could only be checked in a
secondary source or are derived are marked there.

### 5.5x Assets, projects, versions and work lists (M6 R4)

Two ways to work, one layout. An **asset** is a folder with `asset.pxl` (the working file), `input/` (images; the first is the
comparison reference), `versions/`, `compare.png` (input | latest, latest only without an input) and `compare.html` (the image plus
the asset's decisions, open tasks, exceptions, versions). Every command takes the folder, `asset.pxl`, or inside a project the asset name.

- **Single image:** `pxl image FILE.png [--console gbc] [--mode sprite|tile|screen] [--size WxH] [--factor N]` makes `FILE.pxl/` beside the
  picture: the input copied to `input/`, a derived `template.toml` (one flat colour set unless the console allows overlays), a first
  reduction, version v001. The console is in the asset's own header (`console = "gbc"`). `pxl export` writes to `<asset>/export/`.
- **Project:** `pxl new DIR --console ID [--name N]` writes `pxl.toml` (`legacy = true` until scenes are defined, `out_root = "export"`,
  `[project] name, console, strict = true`, `[project.defaults]` mode / size / factor / category / palette_set / template for new assets,
  free-form `[project.settings]` shown by `pxl project show`) and the folders `inbox/ assets/ palettes/ consoles/ templates/ export/ review/`.
  `pxl.toml` keeps one `[parts.NAME]` per asset (path, `out`, tags), so `build`, `check`, scenes and the queue work unchanged.
- **Ingest:** `pxl ingest [inbox|files|globs|folder] [--group G] [--mode M] [--grid WxH | --frames auto] [--name-from file|folder]`: one
  asset per picture, per folder of frames, or per sheet (grid, or connected regions merged within 2 px, bottom-centre aligned). Ingested
  files leave `inbox/`. A changed input is kept as the new `input/<file>` (the old one in `input/history/`), frames that were hand-edited
  or locked keep their art, the rest are reduced again: a new version.
- **Versions:** every write of an asset file goes through `fsutil::atomic_write`, which stores `versions/<name>_vNNN.pxl` (the exact text),
  `.png` (a transparent sheet of all frames) and regenerates `compare.png`, unless the text equals the latest snapshot. Inside a project
  the store then prints the scene delta (`zone_1 sprite VRAM 165 -> 168 / 167 FAIL`, against `.pxl/scene_state.json`; failing rows are
  always printed) and rebuilds that asset's export. `--no-version` (or `PXL_NO_VERSION=1`; `PXL_NO_DELTA`, `PXL_NO_AUTOEXPORT`) switch it off.
  `pxl versions`, `pxl revert ASSET vNNN` (a new version), `pxl import ASSET file` (png, grid text or aseprite; a single-frame asset needs no
  `--frame`). Templates of palette-set assets that live in the asset folder get a key for every colour of the set automatically.
- **Frame sequences:** `pxl ingest` groups single pictures with a common stem and a trailing number (`walker_01.png`, `moto1.png`,
  `moto 2.png`, `walk-03.png`) into one asset, frames ordered by number and named `<stem>_NN`; the output lists every group. A lone numbered
  file stays its own asset; `--separate` turns the grouping off.
- **compare.png:** per frame the input and the latest side by side at the same scale (the latest enlarged by the asset's factor, nearest
  neighbour, and an integer zoom so a frame is at least 128 px tall), a label per frame and a header (name, version, console/mode, factor),
  frames without an input alone, cells wrapped at 2048 px. `compare_1x.png` shows the latest frames at 1x on the console's `preview_bg`
  (override: `[project.settings] preview_bg = "#rrggbb"`).
- **Groups:** the folders between `assets/` and an asset; `pxl group list | add | move ASSET GROUP` (moves the folder, updates path / out / tags,
  rebuilds the export), `pxl tree`.
- **Tasks** live in `<project>/tasks.toml` (a single asset keeps its own): `[[task]]` with stable ids `t0001...`, never reused. `pxl task add`,
  `pxl tasks [--open|--all|--mine|--assign N|--group G]`, `pxl task done ID [--note]`. `pxl rework` and `pxl review note` create `fix` tasks.
  `pxl next` ranks assets with tasks assigned to the caller first, then unassigned tasks, then the old order (priority, worst score, path);
  leases, claim and release are unchanged. `pxl board [--html FILE]` is the overview per group (static HTML, no script).
- **Decisions and exceptions:** `pxl decide "text" [--asset A] [--by user]` appends a dated line to `decisions.md`; `pxl exception add ASSET
  "change" --reason ".."` appends to the asset's `exceptions.md`. Both show in board, brief and the compare page. An asset approved with
  `--agent user` (`approved_by = "user"` in its status) refuses every change without `--force-user-approved`; a forced change adds one open
  `review` task.
- **Brief:** `pxl brief ASSET|GROUP` prints decisions, open tasks, review history, budget rows of the scenes holding the asset, exceptions and
  the paths of compare, latest version, input and review folder.
- **Variants:** `asset.<name>.pxl` beside `asset.pxl` (= `main`), `variants.toml` names the delivered one. `pxl variant add | list | deliver`.
  Deliver points `[parts.NAME]` at the delivered file, registers the others as `NAME__variant` parts exporting to `export/variants/NAME/<v>/`
  and marks them `budget_exempt`; `variants.png` shows them side by side.
- **Reviewer workflow:** `pxl review next` (assets in state `done` or with `review` tasks), `review note ASSET FRAME "text" [--region x,y,w,h]`
  (writes `review/note_NNN.md` and a crop `source | ours`, creates a fix task for the owner; a text with a colour or direction word needs the
  region), `review approve` / `review rework` record the `--agent` identity (required).
- **Manifest commands** (a lease on the file; text edits keep comments; a result that does not parse is not written): `pxl scene set SCENE KEY
  VALUE`, `scene add-part SCENE GROUP PART`, `scene remove-part SCENE PART`, `pxl palette slot set SET SLOT #rrggbb #rrggbb #rrggbb`.
- **No silent zeros:** lint rule `animated_set` (a part in `animated` whose frames lack `_NN` counts no slots), `tile_unreferenced` (a tile
  set that no scene tileset / window and no tile set `animated` names; error with `[project] strict = true`, else a warning), scene rows
  `count <group>` / `BG tiles` / `count window` fail on a 0 count with parts present; `check` ends with `checked N of M part(s), K of S scene(s)`,
  `verify_m5.py` with `checked N of M PNG file(s) in K of L folder(s)`.
- **Exports:** `pxl build` records `<out_root>/.export_manifest.json`; files it names that no part produces any more move to `<out_root>/.removed/`.
- **`pxl migrate --layout assets [--dry-run]`:** moves each manifest part together with its folder (reference images, per-frame cell files,
  compare and preview folders) to `assets/<group>/<name>/`, renames `<stem>.pxl|journal|status.json|lease` to `asset.*`, rewrites the relative
  paths of the moved parts (and of staying parts that point into moved folders), carries the recorded status hashes over (export freshness
  and approvals do not change), journals each text change, stores a baseline version and sets the new `path` in `pxl.toml`; `out` paths and
  exports are untouched. A part whose folder holds another part, or that a scene lists by path, stays and is reported.

### 5.5a Palette sets and cells (M5, shared sprite palettes)

On the GBC the 8 sprite palettes (3 colours + transparent), the 10-per-scanline limit and the 40-sprite table are shared
by everything on screen. A part with `palette_set = "zone_2"` is drawn with plain template keys whose colours come from
one shared set; the palette of every 8x16 cell is derived, never stored. Parts without `palette_set` keep the old
base + detail model unchanged.

- **Set file** `palettes/<name>.toml` (found at or above the part): `include = "global"` pulls slots 0-3 from
  `global.toml`; `[[slots]]` entries have `slot` (0-7), `name`, `colors` (exactly 3 `#rrggbb`, channels multiples of 8,
  all different in a slot) and, for slot 7 only, `variants = { boss = [..], fire = [..] }`. One RGB may appear in several
  slots. Violations are errors naming file and slot.
- **Part header**: `palette_set`, `slots = [0, 1, 4]` (restrict), `category` (`player enemy item hazard effect platform
  scenery boss screen`, default from the profile key `category`, `item`), `variant` (which slot 7 variant the part uses),
  `[budget] per_line = N, per_frame = N` (override the category limits).
- **Cell validity**: a cell (profile `hw_cell`) is valid when one allowed slot holds all its colours (RGB only, layers
  are irrelevant). Category `player` only: up to `multi_slot_max` (2) cells per frame may need two slots; they cost 2
  sprites (base + overlay). Everything else is a `cell_palette` error naming the cell (x,y in frame coordinates), its
  colours, the best slot and the colour to fold.
- **Placement** (`cells::place_frame`, used by lint, `pxl cells` and export): per frame a band offset `oy` in
  0..cell_h-1, per 16 px band a column offset `ox` in 0..cell_w-1, minimising (invalid cells, max sprites on a scanline,
  total sprites); ties go to the smallest offsets. A small DP couples the bands through the two-slot allowance.
  Authored `<frame>.cells.json` in the part folder (`[{"x":..,"y":..,"slot":..,"slot2":..}]`, relative to the origin)
  replaces the search and must cover every opaque pixel. Authored cells may overlap (a grey pole cell over the top rows of a
  gold base): every opaque pixel belongs to the first covering cell, in authored order, whose slot holds its colour; an entry may
  restrict itself with `"region": [x, y, w, h]` (relative to the origin) to take only the pixels in it. Overlapping cells all cost
  sprites on their lines (`hw_limit`), export indexes come from the owning cell, `cells --png` rings the overlaps in white, and a
  pixel no covering cell can hold is a `cell_palette` error. `cells dedupe` leaves frames with overlaps alone. Deterministic; well under 5 ms for a 64x64 frame.
- **Lint**: `palette_set` (colour outside the set, with the nearest set colour; unresolvable set, slot, variant or
  category), `cell_palette`, and `hw_limit` counted per real scanline of the placement against the category table
  (profile `[budget.<category>] per_line, per_frame, overlay_line`): player 4/7 (5 on lines with a two-slot cell),
  enemy 4/8, item/hazard/effect 3/6, platform 6/16, scenery 2/20, boss 6/24, screen 10/30. These rules replace
  `palette_size`, `hw_cells` and the grid `hw_limit` for such parts.
- **`pxl cells PART [FRAME] [--png out.png] [--zoom N]`**: per frame the placement, every cell with its slot and
  colours, invalid cells, the line load; `--png` draws the cell rectangles coloured by slot (invalid cells red) over the
  frame. `--json` gives the same data. Read-only.
- **Export**: per frame an indexed PNG with a 32-entry palette (`slot*4` transparent, `slot*4 + 1..3` the slot colours
  in set order, tRNS 0 for every `slot*4`), `<frame>.cells.json` (x,y relative to the origin, slot, `slot2` for two-slot
  cells) next to it, and `<frame>_detail.png` only when a cell is a two-slot cell (its second-slot pixels). Export
  refuses colours outside the set and invalid cells.

### 5.5b Fit, tile memory, dedupe (M5 phase T2)

- **`pxl palette fit PART [--set S] [--apply] [--frames SEL] [--png out.png --zoom N] [--include-locked]`** remaps a
  part into its set. Per frame the distinct colours are clustered (Oklab distance < 0.03 is drift, one colour); the cell
  layout is searched with a cost that counts *folds* (two colours landing on one slot colour, or a colour moved further
  than 0.14 Oklab), so the layout moves before a colour folds; each cell then takes the slot (for the player: slot pair)
  with the fewest folds and lowest error, and each pixel the colour its cluster was given. Colours already in the set map
  to themselves, so a second run changes nothing. The report gives per frame changed pixels, mean and max dE (Oklab x
  100), the cells with folds (`#from -> #to (px)`) and the resulting layout (sprites, max per line, invalid cells).
  Without `--apply` nothing is written; `--png` is the compare page (before | after | changed pixels, folded cells
  ringed). `--apply` writes the frames as one journaled `write_grid` edit (`undo` works), skips locked frames unless
  `--include-locked`, refuses an approved part (`--force-approved`), does not lock the frames it rewrote, and writes
  `palette_set` to the header when `--set` names a new set.
- **Template keys**: one RGB = one key. A target colour reuses a template key with that exact colour; the others are
  appended to the part's template file as keys of a layer `set` (role `set_rrggbb`). Nothing else in the template
  changes (and the key additions are not undone), so older edits and `undo` stay valid.
- **`reduce` for palette-set parts**: `reduce::reduce_with` ends with the same fit (`fit::fit_to_plan`), limited to the
  set colours the template has keys for: the slot of every cell comes from the set instead of a free palette. This
  covers `load`, `reload`, `candidates`, `pick` and `reduce trial`.
- **Tile memory**: the pattern of a cell is its 2-bit slot indexes (0 transparent), independent of the slot colours; the
  pattern and its X, Y and XY mirrors are one tile. A two-slot cell has two patterns. Parts do not share tiles, a group
  total is the sum of its parts' unique cells. `pxl.toml` declares scenes:

  ```toml
  [scene.zone_2]
  palette_set = "zone_2"
  groups = { common = { budget = 56, parts = ["parts/items", "parts/hazards/*"] }, enemies = { budget = 48, parts = ["parts/enemies"] },
             player = { streamed = true, per_frame = 7, parts = ["player"] } }
  ```

  `parts` entries are manifest part keys, paths, folders or globs (`*` crosses folders). Streamed groups check only the
  largest number of unique cells in one frame. `pxl budget [SCENE|--all] [--top N]` prints group totals, the biggest
  contributors and problems (exit 1 when a group is over); `pxl check` reports the same as rule `tile_budget`
  (project level, only when no part is named) and fails on it.
- **`pxl cells dedupe PART --max N [--apply]`** merges near-identical single-slot cells across frames until the part has
  at most N unique cells: flipped duplicates are already one tile; the cheapest merge (dE of the differing pixels x uses,
  transparent against colour counts 100) goes first, subject to the cell keeping its out-of-canvas pixels transparent.
  The report lists every merge with its pixel difference. `--apply` rewrites the cells (one journaled edit) and writes
  `<frame>.cells.json` for the changed frames so the merged layout stays the placement.
- **Editing feedback**: `pxl show PART FRAME --cells` adds the cell list and a pixel map of the slot per cell; every edit
  command warns (in the echo and in `notes`) when the edit makes a cell need a second slot, makes a cell invalid, or
  pushes a scanline over its category limit.
- **Aseprite**: a palette-set part exports an indexed `.aseprite` with the 32-entry palette, one layer `sprite` and cel
  pixels equal to the shipped PNG indexes (overlay pixels of two-slot cells are merged in).

### 5.5c Parts used in several zones (M5 phase T3b)

`palette_set = ["zone_1", "zone_3"]` declares the sets of a part that is recoloured by each zone's slots at runtime (enemies
take the zone's slots 4-6). The first set is the authoring set: the template colours, the exported image palette and the
lint colours come from it. All sets share one cell layout and one slot per cell, because the export holds slot indexes and
the 1..3 index inside the slot is shared:

- **Slots**: only slots every set defines can carry a cell; the others are dropped from the search (an explicit `slots = [..]`
  naming a slot a set lacks is a `palette_set` error). A pixel's colour in another set is `slot colours[index]` of that set.
- **Lint** (`palette_set`, `cell_palette`, `hw_limit`): the set files must all resolve; the cell validity and the limits come
  from the shared layout; `cell_palette` also fires when two colours of a cell would become the same colour in another set.
- **`palette fit`** assigns each cell's pixels to indexes minimising the mean Oklab error over all sets (folds are counted per
  index, so they are the same in every set) and reports, per frame and set, how many pixels differ from the source, how many
  visibly, mean and max dE. `--set a,b` writes the list to the header.
- **Rendering**: `pxl cells PART FRAME --png out.png --set zone_3` and `pxl compare PART --set zone_3` draw the part in that
  set's colours (default: the first set); the metrics of `compare --set` are not recorded.
- **Export**: unchanged (one image of slot indexes with the first set's 32 colours per frame, `cells.json`).
- **`pxl budget`**: a part counts in every scene that lists it; it fits a scene when one of its sets is the scene's set or has
  the same colours in the slots it uses.

### 5.6a BG tile budget, palette-independent tiles, priority, cycles, underwater (M5 phase T3)

- **Budget**: mode `gbc/tile` (`unique_tiles_max = 348`, `count_animated = true`, `pattern_unique = true`,
  `bg_reserved = [7]`); the former rules (300, colours, 8 palettes) are a project override of `consoles/gbc/tile.toml` (5.5). Level tiles + animated
  slots must stay within the limit. An animation set is the frames `<name>_<nn>` of a tile part; the slots the engine
  reserves are the unique tiles of one frame (`anim_slots = "per_frame"`) or frames x tiles (`"all_frames"`). A tile set lists
  the animated parts that belong to it in the header (`animated = ["../x/x.pxl"]`, paths relative to the part). Part keys:
  `max_tiles` (limit of that part, applied to every frame; cards 96) and `bg_palette = 7` (the whole part lives on that
  palette: one palette of 4 colours; it may use a reserved one).
- **Identity**: a tile is its 2-bit index pattern up to X/Y/XY mirrors; the same pattern with another palette is the same tile
  (stats, `dedupe`, lint `unique_tiles`, all `tiles::pattern`). The index of a colour is its rank inside its palette (the
  checker's first-fit packing in sheet order) in the PLTE order of `blocks.png`. The exporter chooses one global PLTE order:
  air colours first (colour 0 of their palette), then by lightness, then a deterministic local search that moves single
  colours to minimise the number of distinct patterns. A checker can verify the count from the PNG alone: pack the colour
  sets first-fit in sheet order, index each colour by its PLTE position inside its palette, canonicalise over the mirrors.
  `tiles stats` prints both counts.
- **Reserved palettes**: `bg_reserved = [7]`; `tiles fit` packs into the rest and lint `tile_palettes` errors above it.
- **Priority**: `[priority] tiles = [..]` in the tile set; `pxl tiles priority PART --set/--clear IDS`
  export writes
  `priority.png` (same size as blocks.png, each priority tile filled white = drawn over sprites); lint `priority_color0`:
  the air of a priority tile must be colour 0 of its palette (one air colour, template `[tiles] air` defined, ids inside the sheet).
- **Cycles**: `[[cycle]]` writes `palette_cycle/<name>.json` = `{"bg_palette": N, "frames_per_step": F, "steps": [{"1": "#..",
  "2": "#.."}]}` (colour index inside the BG palette -> colour). Derived form: `file/line/first/count` (1..4 colours, steps =
  rotations, palette and indexes found from the tile palettes unless `bg_palette` is given); explicit form: `bg_palette`,
  `targets` (indexes 0..3) and `steps` (colours per step). `name` and `frames_per_step` (default 8) are optional. The old
  4x1 swatch PNGs are still written for cycles of exactly four colours.
- **Underwater**: `[[underwater]]` with `kind = "bg"|"obj"`, `palette`, a palette line (`file/line/first/count`) or `colors`,
  and `order`; export writes `palettes_underwater.json` with the palettes in swap order (`order`, else config order).
- **HUD and cards**: tile set parts with `bg_palette = 7`, `[reduce] factor = 1` and a frame named after the output
  (`hud_tiles`, `cards_tiles` -> `hud_tiles.png`), 128 px wide, `max_tiles` for the cap.

### 5.5d Scenes and the shared hardware model (M5 phase T4/T5)

The budgets of the GBC are shared by everything on screen, so they are checked per scene, not per asset.

- **Hardware in the console**: `gbc/sprite` and `gbc/tile` declare `obj_palettes = 8`, BG `palettes = 8` with `bg_reserved = [7]`,
  OAM `hw_cells_max = 40`, `hw_cells_per_row_max = 10`, `vram_banks = 2`, `vram_tiles_per_bank = 384`, `unique_tiles_max = 348`
  (level + animated), `window_tiles = 64`, `sprite_cells = 178`, `dma_bytes_per_frame = 1024` and the overlay policy
  (`overlays = "category"`, `overlay_categories = ["player"]`, `overlay_cap = 2`). A project override `consoles/gbc/<mode>.toml`
  next to the manifest changes single keys for the project checks.
- **Scenes** extend the `[scene.NAME]` of `pxl budget`: `palette_set`, `groups` (as before), plus optional `tileset` (a part key) and
  `window` (part keys, folders or globs for HUD / card tile parts). Top level `legacy = true` opts out of the fail-closed rules.
- **`pxl check`** runs every scene (when no part is named): the palette set exists and defines at most `obj_palettes` slots; each sprite
  group fits its budget; the resident groups plus twice the largest frame of each streamed group fit `sprite_cells`; the streamed groups'
  DMA (unique cells that change between consecutive frames x 32 bytes, worst part per group, summed) fits `dma_bytes_per_frame`; the
  tile set fits `unique_tiles_max` (level + animated slots) and the BG palettes minus the reserved ones; the window parts fit `window_tiles`
  and sit on a reserved BG palette (`bg_palette`). Fail closed: sprite or tile parts without scenes, a sprite part without `palette_set`,
  or in no scene group, are errors (JSON `hardware`, `scenes`). A scene without `tileset` gets a note, not a failure.
- **VRAM rebalancing (T6)**: VRAM is 768 8x8 tiles (2 banks x 384). Sprites address only 0x8000-0x8FFF (256 tiles per bank, so at
  most `obj_tiles_max` = 512 tiles = 256 cells); the background in 8800 mode addresses 0x8800-0x97FF (`bg_tiles_max` = 512 including
  the window). The block 0x8800-0x8FFF (128 tiles per bank, `vram_shared_tiles` = 256) can therefore hold sprite cells or BG tiles. The
  default split (BG 348, window 64, sprites 178 cells = 356 tiles) gives the BG its whole budget; a scene whose tile set uses fewer
  (`bg_used` = pattern-unique level tiles + animated slots, counted like `unique_tiles`) leaves `slack = 348 - bg_used` BG tiles
  that hold `floor(slack / 2)` more sprite cells. With `vram_share = true` (profile) `pxl scene` / `pxl check` report the slack, the
  extra cells and the effective sprite VRAM = min(sprite_cells + extra, obj_tiles_max / 2) and check the resident + double-buffered
  streamed total against it, so group budgets may sum above the default 148 (56 + 48 + 44). An optional `[scene.X] resident_budget = N`
  is validated against that effective total (N + streamed buffers <= effective, resident cells used <= N). A scene with extra cells
  gets a warning ("VRAM split differs from the operator's default"), listed by `pxl delivery` under Open findings next to the VRAM
  table (BG used, window, sprite resident, streamed, total <= 768). Without a linked tileset nothing is lent.
- **Reserved BG palettes per scene**: the profile's `bg_reserved = [7]` (HUD and text) only applies to scenes that have `window` parts
  (a title, ending or special-stage scene may use all 8 BG palettes); `[scene.X] bg_reserved = [..]` overrides it for a scene. A part's
  own lint and `tiles fit` use the profile, or the part key `bg_reserved = [..]` (`[]` = all 8) for a part that never shares the screen
  with a HUD (a screen part can also use the mode `gbc/screen`).
- **Set colours as template keys**: `pxl template add-set TEMPLATE --set NAME [--set NAME2]` appends every colour of the named palette
  sets (following `include`, all slot-7 variants) as keys of `[layers.set]` (role `set_rrggbb`), skipping colours the template already
  has and choosing key characters that do not collide. An existing `[layers.set]` is edited in place, wherever it is; every other byte
  of the file is kept, and the result is validated (loaded) before it is written. `pxl palette fit --apply` does the same for the part's
  sets by default (`--no-all-keys` adds only the colours the fit picks), so hand work has every set colour available.
- **Budget exemption**: a sprite part that ships but never loads in a normal scene (debug-only objects, fallback variants whose BG version ships) says so with the header key `budget_exempt = "<reason>"`. `pxl check` then does not fail "sprite part X is in no scene group" for it (the palette set is still required), prints `exempt: X: reason` (JSON `exempt`), and `pxl delivery` lists it in the exceptions table ("Budget exempt: reason"). An exempt part that is listed in a scene group is contradictory: `check` prints a `warning:` line (JSON `exempt_warnings`).
- **`sprite_palettes.json`** (feedback section 7): a scene called `zone_N`, or any scene with `palettes_out = "<path below out_root>"`,
  exports its palette set as `{"slots": [{"slot", "name", "colors": [3]}, ..., {"slot": 7, "name", "variants": {..}}]}` (following
  `include`, colours upper case). `pxl build` (all parts, no failures) and `pxl export-palettes` write them; `pxl check` expects them
  (missing or out of date is a hardware finding).
- **`scripts/verify_m5.py`** (Python 3 + Pillow, run `python3 -I scripts/verify_m5.py PACKAGE [--json]`) is an independent verifier that
  shares no code with the tool: indexed 32-entry sprite PNGs with `slot*4` transparent; `cells.json` covers every opaque pixel and each
  pixel's slot matches a covering cell (slot2 for `_detail.png`; the origin is searched because PNGs do not store it); sprites per
  scanline (overlaps counted, a second slot counts one more) <= 10 and per frame <= 40; `_detail.png` only under characters/player with
  at most 2 two-slot cells; `sprite_palettes.json` with 8 slots, 15-bit exact colours, slots 0-3 identical across zones and each
  frame's PLTE equal to its zone's slots (folders outside `zones/` are "multi-zone/global": slots 0-3 only); `palette_cycle/*.json`,
  `palettes_underwater.json` fields; `priority.png` size and 1-bit black/white. Sprite folders come from the kind `sprite` rows of
  CHECKLIST.md, else folders with a `*.cells.json` or a 32-entry PNG. It prints PASS/FAIL per rule and per folder; exit 1 on any failure
  or when it found nothing to verify.
- **Overlays**: lint rule `overlay` (error): a frame may not use keys of an overlay layer unless the part's category is in
  `overlay_categories`, and then at most `overlay_cap` overlay cells per frame. Palette-set parts have no overlay layers.
- **`pxl scene NAME [--worst] [--png F] [--zoom N] [--player P] [--objects A,B] [--enemy E] [--ring R]`**: without `--worst` the scene
  report. With it: the player's widest frame, the two widest object frames (one object frame is used twice), the widest enemy frame and a
  ring, all bottom-aligned on one ground line so their cells share scanlines; reports sprites per scanline (limit 10), sprites in OAM
  (40) and OBJ palettes used (8), exit 1 when over. The PNG shows the composition and a bar per line (limit marked).
- **`pxl delivery --template T [--exceptions FILE] [--checker FILE | --package DIR] [--studio ..] [--number ..] [--date ..] [--contact ..]
  [--out DELIVERY.md]`** fills the client's template: header, assets (rows per checklist ID read from the CHECKLIST.md beside the
  template: complete when every part of the folder is approved/done with 0 lint errors, else partial), the tail of the checker output, the
  exceptions table (a 3-column Folder | Change | Reason table is mapped to ID | File | Exception | Agreed by) and, before "Open questions",
  the per-scene budget table and the parts list with state, frames, lint errors and export freshness.

### 5.5e Plan before pixels (M6 R6a)

Palettes and budgets decided late mean every asset is redone (Appendix A, lesson 1). The plan tools put those decisions first.

- **`pxl plan [--scenes a,b] [--apply] [--json]`** reads every asset of the project (its first version) and reports, per scene, before anyone
  draws: the groups with proposed budgets (measured unique cells plus about 12 per cent), sprite VRAM against the effective cells, the
  OAM worst case (the widest frame of every group together; a pessimistic bound, so a warning), DMA, a linked tile set against the BG
  budget, and a **shared palette plan** (below). Without `[scene.*]` tables the scenes are *proposed from the groups* (`--scenes` names
  one per act; default `main`); with tables they are evaluated as they are. Flat consoles (gb, sms, gg, md, snes, pce, gba) have one palette
  per sprite and no scene VRAM model: the plan lists their groups and says so.
- A scene that does not fit lists **options with their costs**: `cut` (animation steps of the biggest assets), `rebalance` (lend unused BG
  tiles to sprites; needs the tile set linked), `bg_conversion` (cells freed against BG tiles used, no flips or priority) and `act_split` (two
  scenes by group halves, and whether each half fits). The user decides once, up front.
- **Dual-use candidates**: objects, platforms and scenery whose widest frame needs half the scanline limit in sprites, or that are a large
  share of an over-full scene, are flagged with the suggestion to make a BG-palette version (8x8 cells, colour-0 policy) as a variant.
- **`pxl plan --apply`** appends the proposed `[scene.NAME]` tables to pxl.toml (`legacy = false`), runs `palette plan --apply` for each,
  and writes `plan.toml` (`state = "proposed"`, a signature of scenes, group budgets, tile set, window and reservations).
  **`pxl plan accept [--by NAME] [--decision "..."]`** signs it off. Scenes that do not fit need `--decision`, which is recorded in
  `decisions.md`. `next` and `claim` refuse while a plan exists that is not accepted, or accepted before the scenes changed, or when
  `[project] plan = "required"` and there is none. Projects without a plan behave as before.
- **`pxl palette plan SCENE [--slots N] [--reserve 7,..] [--without PART] [--parts PATTERN] [--align SET] [--name SET] [--what-if | --apply]`**
  learns one palette set from the cells of all the scene's sprite parts together (the learner of the first version, shared materials get
  one slot), reports the loss per part (mean and maximum dE x100) and the **identity colours lost** (colours over 3 per cent of a part,
  or saturated ones over 0.5 per cent, whose pixels end up more than dE 10 away). Reserved slots (`--reserve`, or `[scene.X.reserve]
  palette_slots`) stay out of the plan and keep their colours from the scene's current set. `--align OTHER` gives a learned slot the number
  of the slot of the other set it matches, so a shared material keeps its slot across scenes. Without `--apply` nothing is written;
  `--what-if` marks a try and refuses `--apply`. With `--apply` it writes `palettes/<set>.toml`, links `palette_set` in the scene and runs
  `palette fit --apply` for each part (assets are reduced into the plan from the start).
- **`pxl budget SCENE --what-if "OP" ...`** shows the scene rows before and after and writes nothing. Operations: `remove PART`, `add PART
  GROUP`, `budget GROUP N`, `reserve GROUP|sprite_cells|bg_tiles N`, `resident N`. Exit 1 when the changed scene is over.
- **Reservations** (`[scene.NAME.reserve]`: `sprite_cells`, `bg_tiles`, `groups = { name = N }`, `palette_slots = [..]`) keep sprite and BG
  producers from eating each other's slack: reserved group cells count in that group's budget (`50+4` in `pxl scene`), `sprite_cells` count
  as used sprite VRAM, `bg_tiles` are never lent to sprites.

### 5.5f Delivery manifest, zones, source QA and delivery tools (M6 R6b)

**The manifest.** `[project] manifest = true` (written by `pxl plan --apply`) makes every full `pxl build` write `<out_root>/manifest.json`, derived
from the parts and never typed. `pxl check` fails with "out of date" when it differs from what the parts now say, so a stale listing cannot ship.
Per part: frames in part order with `size`, `origin`, `semantics` (distinct or engine-flipped with `flip_of`), `dropped`, the exported
`files`, and the hardware `cells` (x, y, slot, slot2; palette-set consoles) or `sprites` (variable sizes); `palette_variant`, `slots_used`,
`residency` (resident or streamed, from the scene group), `zones`, `together`, `collision` against the `visual` size, `repeat`, `compose`,
`forms` (`sprite`, and `bg` with `bg_variant`). Per scene: the resolved OBJ palette table with stable ids `<set>/<slot name>`, colours,
native values (`export_palette` kind, RGB555 for the 5-bit consoles), `used_by`, the slot-7 `variants` (`<id>@<variant>`) with the parts that
use them, the tile set's files and the cycle / underwater files labelled with the scene, the groups, and the layout. Build problems
(unknown meta references, two frames writing one file, a variant the set does not define, a frame in the client spec that is not delivered or
a delivered frame the spec lacks) fail the build.

**`pxl meta ASSET ...`** writes `meta.toml` beside the asset (`<part>.meta.toml` beside a plain part): `set collision WxH[@dx,dy]`, `set repeat
stride=X,Y count=N`, `set residency resident|streamed`, `set bg_variant PART`, `set together F1 F2`, `set zones ..`, `compose add PART DX DY`,
`frame NAME semantics flip_x --of OTHER`, `frame NAME origin X,Y`, `frame NAME dropped true --reason WHY`, `zone-remap SCENE 4=5`. It never
writes what the manifest cannot read. `[delivery]` in pxl.toml: `spec` (a spec listing generated from the manifest), `client_spec` (the client's
`{"parts": {"NAME": {"frames": [..]}}}`, checked at every build), `client_checker`, `client_pipeline`, `package`.

**Dual-use.** A part may name a BG form (`bg_variant`); the manifest lists both forms. `pxl plan` flags objects that probably need one and says
whether the form exists.

**Zone completeness and real screens.** `[scene.X] layout = "layouts/x.json"` (JSON with `objects` id to part and `acts`, or CSV
`act,object,x,y`; `pxl scene set X layout FILE`). `pxl check` and `pxl scene X --layout [--act N] [--png F]` report every placed object that
has no part, or whose part is not drawn for this zone (not in the scene's groups and its set differs in a slot it uses) unless
`zone-remap SCENE` covers every differing slot and targets slots the scene's set defines. `--layout` also composes each act with the widest
frame of every object on a sliding 160x144 window and reports the worst scanline and frame against the hardware limits. (No objpos reader yet:
convert to the generic formats.)

**Source QA** (`pxl qa [ASSET] [--dry-run]`, and at ingest): a noise score (share of neighbouring pixel pairs that differ, over more than 8
colours, above 0.65) flags garbage references; `input/source.json` (copied by ingest from beside the picture, the `pxl-extract` contract) flags
`zone` != `art_zone` and `unused` labels. Each finding becomes one `question` task and sets the asset to `blocked`, so `next` does not hand it
out as a draw job.

**Delivery tools.** `pxl sheet GROUP [--set S] [--on #rrggbb] [--zoom N]`: contact sheet. `pxl package`: assembles `package/` from the
manifest (non-delivered variants and dropped frames stay out; `package.json` lists every file with hash and size, the dropped frames with
reasons, the asset exceptions); files of an earlier package that no longer belong are removed. `pxl verify-package [DIR]`: files present,
hashes unchanged, nothing unlisted, frame PNG sizes, console colours, every opaque pixel in a listed cell, sprites per scanline and frame, palette
tables (count, ids, colours), with counts of what was checked; the console comes from the manifest. `pxl deliver [--client-checker CMD]
[--client-pipeline CMD] [--allow-client-fail]`: our `check`, the package and the verifier (a failure refuses and snapshots nothing), then the
client commands in the package folder with `PXL_PACKAGE`, `PXL_MANIFEST` and `PXL_REPORT_DIR` (images written there are kept); a failing
client command refuses unless allowed; the snapshot is `deliveries/N/` (manifest, package, report.json). `pxl changes --since N [--to M]`
lists parts and frames added or dropped, size, origin, footprint, cells, semantics, variant, residency and palette colour changes.

**Lint** (`near_duplicate`, `stacked`; on by default in projects with `manifest = true`, anywhere with `--rules`): two colours of one palette
(template layer or set slot) less than dE 4 apart (warning); stacked cells (an authored overlap of at least half a cell) outside the profile's
overlay categories whose covering cells do not all hold the pixel's colour (error: it does not survive a flatten).

**Per-console style.** `[style]` in each console.toml (and in pxl.toml, which overrides keys): `white` (`active` | `highlights` | `balanced`),
`outline`, `min_value_spread`, `value_map` (`stretch` | `colour`) and `reason`; `pxl console show` prints it. The Game Boy is `active` + `stretch`:
its first version maps source colours by luminance with a contrast stretch per sprite (the darkest material black, the lightest white, the
range cut at 0.2 / 0.5 / 0.8 after stretching), keeps a light-only sprite from turning black, uses OBP0 (black, dark grey, white) and OBP1 (black,
light grey, white) as a two-slot set, and folds each 8x16 cell into the slot and a monotone level map that keeps its materials apart.

### 5.5g Operator and design-language tool support (M6 R7)

The agent runs a project as the experienced operator (PLAN_M6). The tool supports it; a project made by `pxl new` has `[project] gates = true`
(`--no-gates` opts out; a project without the key behaves exactly as before, including its lint results and `pxl check --hand-finished`).

- **`pxl intake [--request TEXT|FILE] [--interview] [--answers FILE]`** is non-interactive by default. It infers, from the request, the client spec
  (`[delivery] client_spec` / `inbox/spec.json` `console`) and the ingested inputs: console (the longest alias first: "game boy color" before "game boy"), image modes (counted),
  fidelity (faithful unless the request asks for a reinterpretation), cadence (`as_work_lands` | `per_batch` | `daily` | `weekly`), trade-off order (default: cut
  animation steps, fold colours, narrow sizes, convert to BG, counter-propose), exceptions (`never` unless the request allows asking), deadline, and the
  **identity list per asset** (eyes per frame from `features::detect`, saturated colours of 0.5 % to 12 % of the figure grouped by hue, the main materials).
  It writes `brief.toml` (`[brief]`, `[assumptions]` with `assumed` | `confirmed`, `[modes]`, `[identity.NAME]`) and one `[operator]` line per item to `decisions.md`
  (`(assumption KEY)`, replaced on a rerun). `pxl decide "why" --set KEY=VALUE --by user` changes an item and confirms it (a rerun leaves it). Only a high-impact
  uninferable item becomes a question: the request names another console than the project (options keep / switch), or it asks for both a faithful and a
  reinterpreted look (options with the asset's compare image). Questions are open decisions with options and a recommendation. `--interview` asks each item on
  stdin; `--answers` applies a TOML file as the user's decisions.
- **Decisions with options** (`decisions.toml`, `src/decisions.rs`): `pxl ask TOPIC --option A=img[|label] --option B=img --recommend A "why" [--asset X]` needs two
  options and a recommendation, copies the images to `review/decisions/dNNNN/` and writes the page `review/decisions/dNNNN.html` (the recommended option marked).
  `pxl decide dNNNN A` closes it (a dated line in `decisions.md`), `pxl decisions [--all]` lists them. `pxl task add ASSET "text" --after dNNNN` stays blocked
  (`tasks` marks it, `claim` does not hand it out, `next` skips an asset whose every open task waits). Board, brief and report show open decisions and the
  unconfirmed assumptions.
- **Design language** (`src/style.rs`, `src/insight.rs`): `pxl style analyze [ASSET..]` measures per asset and over the project (`.pxl/style_analysis.json`):
  outline presence, colour and thickness (output pixels), tones per material (hue families of 0.3 rad plus a neutral family; lightness bands separated by more than
  0.05 holding 6 %), dark / mid / light ramp per material, light direction (correlation of lightness with position), highlight style (none, spots, areas), dithering
  (checkerboard share), palette size, eyes (count per frame, white size in cells). `pxl style propose [--force]` writes `style.md`: the measured input style, one
  line per hardware limit with its effect on this input (colours per palette line, sprite palette lines, cell palette granularity, factor, sprites per scanline and
  size mode, background attribute granularity, fixed greys or master palette), the **rules** (`outline`, `eyes` with minimum sizes, `identity`, `ramps`, `light`,
  `white`, `highlight`, `dither`, `fold`, `readability`; white and outline follow the console `[style]`), the identity list and the shared ramps. `pxl style approve` hashes
  the rules (editing them afterwards clears the approval), and writes `[style] outline = "none"` to pxl.toml when the input has no outline. State is in `style.toml`.
- **Style frames and milestones**: `pxl style mark ASSET [FRAMES]` (`--remove`) names the reference assets; compare.html and the review packet show the nearest one (same
  group, else same mode). Milestones (`pxl milestone status`): `plan accepted` (plan.toml current), `style approved` (style.md approved and every style frame in state
  approved), `dry run passed` (`pxl deliver --dry-run`, valid while the style frames' latest versions are unchanged). With `gates = true` `next` and `claim` refuse
  without an accepted plan and hand out the style frames only until all three hold.
- **`pxl deliver --dry-run`** builds, runs `check` on the style frames, assembles a package under `.pxl/dryrun/package` and verifies it with the delivery verifier, runs the
  client's checker / pipeline when given, keeps the style frames' compare images in `.pxl/dryrun/report_images/` and snapshots nothing. (Rendering in-engine colours from the
  manifest alone is not done yet.)
- **Quality gates** (`src/design.rs`; lint rules, warnings; on every lint of a gated project, anywhere with `--rules`; validated by `tests/gates_harness.rs`):
  - `eye_parity`: per frame, every source eye whose white fills at least three full cells (smaller whites are glints at this resolution; advice `lost_eye` still lists them)
    must be drawn. Colour based, so it works with any template or palette-set keys (`features::eye_drawn`): a connected near-white area of 40 % of the reference white
    around it and a dark pupil cell by the reference pupil. `fix` is the stamped eye (not for palette-set parts, whose keys are the set colours).
  - `identity_colour`: each identity colour of the asset (brief.toml, else detected) that the source frame holds in at least three cells (not darker than L 0.35) must have its hue
    family in the art (within 0.4 rad, not much greyer; a very light key colour is carried by the console's white). Skipped on grey consoles.
  - `flat`: a material with three tones (two when it fills 40 cells) in the source that is one colour in the art; a single-colour block of a hue the source lacks over 70 % of the figure
    (not for runtime-recoloured parts with a `variant`); a value range under half the source's and under the console `min_value_spread`.
  - `consistency`: a chromatic colour of the character that is in at least two thirds of the frames, missing from a frame whose own source holds that colour family in six cells;
    a material whose source ramps match across sibling assets and style frames while the art ramps differ.
  - `readability`: judged against the source frame itself: the tone span under the floor and under half the source's (two or more colours, 40 cells or more), and a silhouette that
    sits on the console preview background where the source does not.
  In a gated project `review approve` and `approve` refuse while any of them warns, unless `--justify "why"` is given; the justification is kept in the verdict.
  `pxl review checklist ASSET` generates the checklist from style.md; `review note ... --rule ID` quotes a rule (validated against style.md).
- **`pxl report [--html [FILE]] [--if-due] [--peek]`**: milestones, done, in review, in work, new images since the last report (`review/.report.json`), pending decisions with
  recommendations, scene budget status, unconfirmed assumptions. `--if-due` follows the brief cadence.

### 5.6 Tiles, levels and screens

- **`tiles fit <tileset>`**: assigns tiles to the template's palettes; proposes the cheapest Oklab remaps when
  they do not fit.
- **`tiles stats <tileset>`**: colours per tile, unique count (flips deduplicated), duplicate groups.
- **`[reduce] thin = true`** (tile sets and screens): a 2-2 tie between two colours goes to the colour that is rarer in
  the block, so 1 px lines (inner channels, strokes) survive the halving.
- **`tiles dedupe <tileset> --max N [--apply]`**: merges near-duplicate tiles (flips included), invisible merges first.
- **Screens** (`mode = "screen"`, 160×144, `[reduce] factor = 2`): a screen is a tile set whose reference is one picture of the
  screen's native size (`[input] images`); it reduces, fits, dedupes and compares like a tile set (8 palettes × 4, no transparency).
  A screen has no unique-tile limit unless `[screen] unique_max` says so (mode `gbc/screen`: 360).
  `pxl template suggest --derive` works on screens (no air key). The exported file is named after the frame
  (`background`, `splash`, `background_2`).
- **Animated tile parts**: a tile set part whose frames are called `<name>_<nn>`; listed in `animated = [..]` of the tile set they
  share the budget with, exported to `animated/`.

Acceptance:

- AC-T1: After `tiles fit`, `tile_palettes` passes.

## 6. Parallel agent loops

Many agents run the load → adjust → compare → export loop at the same time, each on different graphics files.
A coordinator fans work out, watches progress and reviews. The tool supports this without a server: all
coordination happens through small files and atomic file operations.

### 6.1 Unit of work

- **The part file is the unit of work.** One `.pxl` file (with its journal and status file) is owned by at most
  one agent at a time. Part commands read the shared inputs, templates, profiles and manifest, but never write
  them.
- **Shared files are read-only during loops.** Templates, profiles and the manifest change only through the
  coordinator (`pxl template set ...`). When a template changes, every part using it shows as `stale` in
  `pxl status` and is reloaded or re-exported by its owner.
- **Atomic writes.** Every write goes to a temporary file in the same directory and is renamed into place, so a
  killed agent never leaves a half-written part, PNG or `.aseprite`.
- **No hidden state.** Outputs depend only on the part, its template, profile and inputs. Content hashes in the
  status file detect staleness. There is no daemon and no global cache that agents could fight over.

### 6.2 Work queue and leases

| Command | Effect |
|---|---|
| `pxl next --agent A [--kind sprite\|tileset] [--tag zone_1]` | atomically claims the next part that needs work (highest priority, worst metrics first) and prints its path, task and current metrics |
| `pxl claim <part> --agent A` | claims a specific part |
| `pxl renew <part> --agent A` | extends the lease (done automatically by every edit command run with `--agent`) |
| `pxl release <part> --agent A [--state done\|blocked --note "..."]` | gives the part back with a state and a note for the coordinator |
| `pxl status [--json] [--stale] [--blocked]` | the whole project at a glance: per part the state, owner, lease age, frames remaining, worst frame metrics, lint counts, export freshness |

- Leases are files (`<part>.lease`) created with exclusive create, so two agents claiming the same part cannot
  both succeed. A lease expires after a configurable idle time (default 15 min). A crashed agent's part goes back
  to the queue on its own.
- Commands that modify a part refuse to run if another agent holds a live lease (`--agent` identifies the
  caller). Read-only commands (`show`, `compare`, `lint`) always work.
- Part states: `todo` → `in_progress` → `done` (passes lint and the agent's own review) → `approved` (set by the
  coordinator) or `rework` (with review notes). `blocked` carries a note (e.g. "wrong art offset, needs
  coordinator").

### 6.3 Status and handback

- Each part has a `<part>.status.json` (written by `load`, edits, `compare`, `lint`, `export`): state, owner,
  per-frame metrics, lint summary, locked frames, input/template/output hashes, notes. This is the handback:
  nothing depends on an agent's final message arriving.
- `pxl review [--state done] [--family F] [--out dir]` writes a review packet per part (`dir/<part>/`): compare
  pages (input | output | diff) of every frame, the worst frames zoomed, a lineup with the sibling parts (same
  `family`, same template base, or listed in `[review] siblings` of the part or manifest; each item at 1x and
  zoomed), the raw (not hand-finished) frames and `review.md` (the checklist from
  `skills/pxl-review/references/checklist.md`, open notes, a per-frame table). `pxl approve <part> [--note]` /
  `pxl rework <part> [--frames SEL] --note "..."` record the verdict in the status file (`review` entries with the
  frames and their grid hashes at that time). A per-frame note is open until its frame is edited; `pxl status
  --rework` lists the open ones, `pxl status --metrics` counts rework rounds per part and frame and groups the notes
  by the first checklist keyword they mention.
- `[input] tile_base` is added to every mapping piece's tile index (mod 2048); `[[frames]] phase = [x, y]`
  replaces `[reduce] phase` for that frame (folded into the reference origin by `load::reference`).
- `pxl pattern suggest PART FRAME [--region | --tile N | --tiles RANGES] [--max-period N] [--save NAME]` measures the
  repeating unit of the reference (texture.rs): smallest period within a margin of the best agreement, majority unit
  over all repeats, saved as the snippet `pattern_NAME` at the default phase of `pxl pattern`.

### 6.4 Multi-file commands

- Every read or batch command accepts several parts or a glob (`pxl compare 'parts/enemies/*.pxl'`,
  `pxl lint parts/`, `pxl load parts/zones/zone_1/*.pxl`, `pxl export --all`). These run across files on all CPU
  cores (scoped threads), with results reported per part.
- `pxl build` and `pxl check` skip parts whose hashes have not changed.

Acceptance:

- AC-PA1: 20 concurrent `pxl claim` calls on the same part: exactly one succeeds; the others get a clear "held
  by A since ..." error.
- AC-PA2: Killing an agent process at any point leaves every file valid (atomic renames); after lease expiry the
  part is claimable again with its last complete state.
- AC-PA3: 20 processes working on 20 different parts concurrently produce output byte identical to running
  them one after another.
- AC-PA4: `pxl status --json` on the full project (≈ 60 parts) completes in under 100 ms.
- AC-PA5: A template change marks exactly the parts using it as `stale`.
- AC-PA6: Cold start of any command is under 10 ms, so agents can call the tool in tight loops.

## 7. Interface conventions

- `pxl <command> [args] [--json]`
- AC-I1: With `--json`, exactly one JSON object on stdout, nothing else.
- AC-I2: Exit codes: 0 success, 1 validation or lint error, 2 usage error, 3 I/O error.
- AC-I3: Errors name the file, frame, row and column, and suggest the fix.
- AC-I4: `pxl guide` prints the workflow (§8), the quality bar (§1.1) and the adjust command table (§5.2) on one
  page, so a fresh agent needs nothing else.

## 8. Agent workflow

Per part:

0. **Claim:** `pxl next --agent A` (or `claim`); work only on the part it returns.
1. **Load:** `pxl load part.pxl`. Check `compare` once to confirm the inputs render correctly (right art,
   offset, palette line, template mapping).
2. **Compare:** `pxl compare part.pxl --all`. Read the sheets and the worst-first ranking.
3. **Adjust:** for the worst frame, `pxl show part F --ref`, then `set` / `row` / `line` / `copy` / `apply`.
   Each edit echoes the changed region.
4. **Compare again** for that frame (`--frames F --zoom`) and run `lint`; repeat 3–4 until the frame meets §1.1
   and has no warnings. Move to the next frame in the ranking.
5. **Export:** `pxl export part.pxl`, then `pxl release part --state done`; pick up the next part with
   `pxl next`.

Coordinator loop:
- Start N agents, each looping `next` → load/adjust/compare → export → `release`.
- Watch progress with `pxl status`.
- Review `done` parts with `pxl review`, then `approve` them or send them to `rework` with notes; reworked parts
  go back into the queue.
- Resolve `blocked` parts.
- Finish when `pxl check` passes and every part is `approved`.

## 9. Testing

  - end to end: a fresh agent with only `pxl guide` brings 6 frames to approved quality
- **Parallel:** a stress test with 20 agent processes looping `next`/edit/`release` on generated parts, killed
  at random points (AC-PA1–3).
- **Adjust commands:** property tests for atomicity, locality and undo (AC-AD1–3).
- **Lint fixtures:** 3 positive and 3 negative per rule.
- **Golden files** for every command; `cargo test` regenerates them when an environment variable is set.
- **Examples** (`examples/`, original CC0 art): `tests/r5_release.rs` runs the single-image and project scripts on gbc, nes and md, the
  quality and gates harnesses measure the automatic first version, and every console has a small end-to-end project.
- **No silent skips:** a gate or compare that has no source image prints `reference_missing` (part and frame), and `check` counts "checked N of M".

## 10. Milestones

| Milestone | Scope | Exit criteria |
|---|---|---|
| M1 Load | PNG input, `.pxl`, templates, reduction, `show --ref` | AC-LD*, AC-TP1/2 |
| M2 Adjust + compare + parallel | all §5.2 commands, journal/undo, `compare` sheets and metrics, lint, leases, `next`/`status`/`review`, multi-file commands | AC-AD*, AC-CP*, AC-L1/2, AC-PA* |
| M3 Export | indexed PNG, `.aseprite` read/write, `pxl.toml`, `build`, `check`, profiles | AC-EX*, AC-L3, AC-TP3 |
| M4 Tiles and screens | tile sets, `tiles *`, screen parts | AC-T* |

After M3 the sprite work (≈ 770 frames) can run. After M4 the whole delivery can run.

## 11. Open questions

1. One overlay layer per part enough, or allow several (each costs hardware sprites)?
2. Palette levels: linear 3-bit → 8-bit expansion (used so far) or measured Mega Drive output levels? It changes
   which GBC colours are nearest.
3. Should the journal be kept permanently (full edit history per frame) or trimmed after `export`?
4. Screens: is 160×112 content inside 160×144 (with fill) right for every screen, or do some need a crop?
5. Lease timeout: is 15 minutes idle right for agent loops, or should leases be renewed by a heartbeat
   independent of edits?

## Appendix A. Lessons from a commercial port

The first production job was a Game Boy Color port of a Mega Drive game: about 770 sprite frames, 6 level tile sets and 5 screens,
delivered to an engine team that loads them with its own converters. It shaped several features; none of it is needed to use the tool
on another console.

**What went wrong, and the feature that prevents it**

| Lesson | Feature |
|---|---|
| Budgets were checked per asset; the delivery was returned. Palettes were chosen per part, then redone for shared sets. | The fail-closed scene model, `pxl plan`, `pxl palette plan`, `budget --what-if`, reservations (§5.5d, §5.5e) |
| Silent zeros: unlinked animated sets, frame names without `_NN` counted 0 tiles, the verifier skipped half the folders. | Lint for unreferenced tile parts and unlinked animations; "checked N of M" in every verifier; `reference_missing` |
| Stale exports blocked approvals and old outputs nearly shipped. | A store rebuilds its export; `build` removes outputs no part produces any more (`export/.removed/`) |
| Art drawn from the wrong source (noise, another zone's art, unused frames). | Source QA at ingest (`pxl qa`), question tasks |
| Decisions, alternatives and exceptions lived in memory and notes. | `decisions.md`, variants, `exception add`, `pxl brief`, `pxl board` |
| Reviewer notes went through the coordinator; one reviewer direction contradicted the source. | `pxl review next | note | approve` with region crops; verify a direction at zoom before relaying it |
| Agents approved bad designs (one eye, removed key colours, flat shading) until the user corrected them. | The design language (`style.md`) and the lint gates `eye_parity`, `identity_colour`, `flat`, `consistency`, `readability` (§5.5g) |
| The engine team never edited the art, but re-coloured and re-packed it in their converters. | The machine-readable delivery manifest: palette variants and stable palette ids, repeat and compose rules, collision vs visual size, dual-use BG art, zone completeness (§5.5f) |

The operator playbook that came out of it is the `pxl-operator` skill: infer, assume and show; ask only what cannot be inferred; learn
the client's engine model first; plan before pixels; style frames before production; an early delivery dry run; record every decision.
