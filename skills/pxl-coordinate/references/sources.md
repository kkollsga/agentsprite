# Setting up a part from images

How to describe where a part's high resolution reference comes from (`[input]`), how frames pick their
source (`[[frames]]`), how the reduction is configured (`[reduce]`) and how the part is declared in the project
manifest (`[parts.X]`). Every `toml` block below is parsed by the test suite, so you can copy it.

pxl reads PNG (and Aseprite) pictures only. Game data (ROMs, disassemblies, rips, emulator dumps) is turned into
pictures first: see the `pxl-extract` skill for the inbox contract (one folder per object with the frame PNGs and a
`source.json`).

## 1. Look at the pictures

```text
pxl image refs/robot_01.png --console gbc     # first version of one picture
pxl intake                                    # infer sizes, palettes and assets from an inbox folder
pxl show PART FRAME --ref                     # the reference next to the grid
```

Frame pictures of one object must share one canvas size and one object origin (the point that stays put between
frames: feet, centre of mass). If they do not, fix that in the pictures, not in the part.

## 2. `[input]`

```toml
name = "robot"
size = [32, 32]
origin = [16, 28]

[input]
images = "refs/{frame}.png"
ref_origin = [34, 48]
factor = 2

[reduce]
factor = 2

[[frames]]
name = "01_stand"
src = "robot_01"
grid = """
................................
................................
................................
................................
................................
................................
................................
................................
................................
................................
................................
................................
................................
................................
................................
................................
................................
................................
................................
................................
................................
................................
................................
................................
................................
................................
................................
................................
................................
................................
................................
................................
"""
```

| key | meaning |
| --- | --- |
| `images` | a pattern like `refs/{frame}.png`; `{frame}` is the frame's `src`, else its name. |
| `ref_origin` | object origin inside the reference picture, `[x, y]`. Default: `origin * factor`. |
| `factor` | integer scale of the reference relative to the target (also used for lint). |
| `target` | explicit target size `[w, h]` for area reduction instead of `factor`. |

A frame whose `src` ends in `.png` reads that picture directly.

## 3. `[[frames]]`

* `name`: file-safe name (no spaces or slashes); order prefixes `01_` and trailing numbers drive tags.
* `src`: picture name (with `images`) or a `.png` path. Default: the frame name.
* `locked`: `true` once approved; locked frames are never changed by reloads.
* `grid`: the cell rows.
* `phase = [x, y]` on a frame replaces `[reduce] phase` for that frame only (reference pixels, as below). Use it for
  a frame whose art sits one source pixel off the grid of its neighbours (a sprite drawn with an odd offset); `show`,
  `compare`, `reduce`, `candidates` and the raw-output audit all read the frame's own phase, and changing it makes the
  part stale like any other input.

```toml
name = "offset"
size = [8, 8]
origin = [4, 4]

[input]
images = "refs/{frame}.png"
factor = 2

[reduce]
factor = 2

[[frames]]
name = "a"
grid = """
........
........
........
........
........
........
........
........
"""

[[frames]]
name = "b"
phase = [1, 0]
grid = """
........
........
........
........
........
........
........
........
"""
```

## 4. `[reduce]`

```toml
name = "tiny"
size = [8, 8]

[reduce]
method = "vote"
factor = 2
phase = [0, 0]
outline = ["outline"]
thin = true
exemplars = ["approved/tiny_idle.txt"]

[[frames]]
name = "a"
grid = """
........
........
........
........
........
........
........
........
"""
```

| key | meaning |
| --- | --- |
| `method` | `vote` (default), `nearest`, `contour`, `exemplar`, `auto`. |
| `factor` | reduction factor (reference pixels per target pixel); falls back to `input.factor`. |
| `phase` | `[x, y]` shift of the reduction grid in reference pixels (a frame can override it with its own `phase`, section 3). |
| `outline` | template roles that get an outline. |
| `thin` | keep one pixel lines (limbs, antennae). |
| `exemplars` | extra approved grids (text files), paired with their frame by file stem. |

## 5. Manifest `[parts.X]`

`pxl.toml` is searched as `./pxl.toml`, then `./pxl/pxl.toml`, then the same two names in each parent
folder, so it is found from the project root and from any subfolder. Paths in it are relative to the folder
that holds the manifest.

```toml manifest
out_root = "../build/pxl_out"

[parts.robot]
path = "parts/enemies/robot.pxl"
priority = 5
tags = ["enemy", "robot"]
out = "enemies/robot"
ase = "enemies/robot/robot.aseprite"
png = "{frame}.png"
overlay = "{frame}_{layer}.png"

[parts.spring]
path = "parts/items/spring.pxl"

[review]
siblings = [["robot", "spring"]]
```

| key | meaning |
| --- | --- |
| `path` | the `.pxl` file (required). |
| `priority` | queue order, higher first (default 0). |
| `tags` | queue tags for `pxl next --tag`. |
| `out` | folder of the exported PNGs (default `<part folder>/<part stem>_export`). |
| `ase` | the `.aseprite` file (default `<out>/<part name>.aseprite`). |
| `png`, `overlay` | file name patterns of the base layer and further layers. |
| `out_root` (top level) | prefix of every relative `out` and `ase`. |
| `[review] siblings` (top level) | part names that `pxl review` puts side by side in the lineup: a list of names (one group) or a list of such lists. A part file can say the same with `[review] siblings = ["name", ...]`. Parts of one `family` or one template base are siblings without being listed. |

## 6. Checklist

1. Put the pictures where `images` points; keep one canvas size and one origin per object.
2. Write `[input]`, create the part (`pxl load PART`), look at `pxl show PART FRAME --ref`.
3. `pxl template suggest PART --derive`; check identity colours (a body colour, a lamp) have their own key.
4. Declare the part in `pxl.toml`; `pxl status` shows `TODO/ALL` (frames not locked / frames).
