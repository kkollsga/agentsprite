# Textures as tile units

A texture is a repeating unit drawn once, cleanly, then repeated at a fixed phase. Downscaling turns detail
into speckle; do not keep it. Pick the unit, draw it, fill with `pxl pattern`, then fix the edges by hand.

```
pxl snippet save PART blocks brick --region 0,0,8,8       # a clean 8x8 unit (or a 4x4, 2x2)
pxl pattern PART blocks brick --region 0,0,16,16 --agent A1            # sheet pixels, phase 0,0 = the sheet grid
pxl pattern PART blocks brick --tile 37 --agent A1                     # one tile; phase 0,0 inside the tile
pxl pattern PART blocks brick --tile 37 --mask 2 --agent A1            # only over cells of key 2: the outline stays
pxl pattern PART blocks --unit 0,0,4,4 --region 8,0,8,8 --phase 0,0    # unit cut from the frame itself
```
Do not guess the unit when the original has one: `pxl pattern suggest PART blocks --tiles 37-40 --save brick` (or
`--region x,y,w,h` over a flat stretch of the sheet) measures the repeat in the reference, with its confidence, and saves the
majority unit as the snippet `pattern_brick` already rotated to the default phase; it prints the `pxl pattern` command to run.
Read the unit before you fill: a low confidence or a period that does not divide 8 means pick the unit by eye.

Phase: `cell(x,y) = unit[(y-py) mod h][(x-px) mod w]`. The default phase is 0,0 on tile sets, so any unit whose size
divides 8 lands identically in every tile and every neighbouring part (sprites default to their origin).
`--mask KEY[,KEY]` limits the fill to cells currently of those keys: first draw the shape and its outline, then
pattern the inside. Spaces in a snippet leave the pixel alone.

General rules (craft 8b): base colour plus one or two pattern values, evenly spread, no dithering unless the original
dithers; a clean edge or highlight row so the texture does not fray; same phase across neighbours.

## Per material (at 8x8)

- **Checkered soil (example from a grassy level):** the unit is a 2-colour checker of squares 4x4 (two squares each way per tile) or 4x2 bricks of soil.
  Squares are the same size everywhere; the diagonal must continue across tile borders, so use phase 0,0 for the
  whole wall and never shift one tile. Top row of a ledge: grass with its own unit; the soil pattern starts below it.
- **Brick:** courses 4 px high, bricks 8 px wide, joints 1 px in the dark value, every second course offset by 4.
  Draw the 8x8 unit as two courses; tile it. Corners and the ends of a wall are hand-drawn.
- **Marble:** a light base, one slightly darker vein value, 1 px veins at an even slant (2:1 or 1:1 steps), at most
  two veins per tile, the same slant everywhere. Veins should leave a tile at the same row they enter the next.
- **Metal plating:** panel lines (dark) on an 8 px grid, 1 px highlight on the top and left edge of each panel,
  rivets as single pixels at the same offset in every panel. Mixed-up rivet offsets read as noise.
- **Water (surface and body):** horizontal wave rows in 2 or 3 values with an even rhythm; the body is flat or
  has sparse lighter streaks on a grid; the surface row has a highlight band. Animated water: the pattern shifts
  by a whole step per frame.
- **Waterfall:** vertical streaks, even spacing (every 2 or 4 px), 2 or 3 values, white foam at the top and bottom
  edges in a regular rhythm. Frames shift the streaks down by the same number of rows each frame, wrapping at 8
  (a 4-frame loop shifts by 2 rows per frame); the first row of frame N+1 equals the last row of the shifted frame N.
- **Grass tops and ledges:** the blade rhythm repeats every 4 or 8 px; the left and right ends of the ledge are
  separate hand-drawn tiles.

## Continuity checklist

1. Lineup the family: `pxl lineup PART:blocks --region x,y,w,h --zoom 6`; every tile of the family in one view.
2. Put a tile that tiles with itself beside itself in a lineup crop: a pure texture shows no edge.
3. Animated frames: lineup `PART:waterfall_00,waterfall_01,waterfall_02` and check the even shift.
4. Same material in other parts uses the same unit (same pixel pattern, its own palette only): put them in one lineup.
