# Downscaling craft

Console-generic. The console's numbers (colours, cell size, sprites per line) are in `pxl console show`; its design hints and the
project's rules are in `[style]` and `style.md`. Where a section says "the overlay layer" it means a console whose profile allows one (check
`pxl console show`); on the others fold the detail into the base ramp.

Examples use a made-up palette: `.` transparent, `1` dark outline, `2` body, `3` light/skin, `W` white, `R` bright accent.
Your template has its own keys; `pxl show` prints them. The shapes are generic: adapt, do not paste.

## 1. Read the reference first

Before touching a pixel, look at the big reference and answer in words:
- Which silhouette would I recognise as a black shape? (spikes, ears, tail, hat, a long arm)
- Where is the eye, how big relative to the head, which way does it look?
- Which 2 or 3 colours say "this character"? (shoes, gloves, belly, muzzle)
- Which details are decoration that can go? (gradients, highlights, texture, small buttons)

That is your "must read at 1x" list. Everything else may be cut. A good conversion keeps the read, not the pixels.

## 2. Silhouette and proportions

- Silhouette first: a clean closed shape that already looks like the character in one flat colour.
- When shrinking 2x or more, **exaggerate slightly** what carries identity: head a bit larger, eyes
  larger, hands and feet a pixel bigger. True-to-scale parts look small and timid at 1x.
- **Round stays round**: a round head or body becomes a rounded shape (knock off the corner pixels,
  1-2-3 steps), never a rectangle. Check the silhouette diff for square corners you added.
- **Simplify** busy shapes: five small spikes become three bold ones; a ruffle becomes one step.
- **Move** a part by a pixel when that gives a clean line or keeps a gap open (a 1 px gap between
  legs beats two legs merged into a block).
- Keep the pose line (the main curve of the body) over local accuracy. Being +-1 px off the
  reference is fine if the pose reads better.

## 3. Eyes (the most important feature)

Eyes give the sprite life. Draw them first and deliberately; do not leave them to a detector.
- Draw **both eyes when the reference shows both**. A missing eye makes the face look wrong.
- Smallest readable large eye: a **white shape plus a dark pupil on the gaze side**, about 2 wide x 3 tall.
  At 2 x 2 the pupil is half the eye. At 1 wide, use one dark pixel on a light face.
- The pupil placement is the expression: touching the side the character looks toward = looking
  that way; centred = blank or startled; high = looking up.
- A dark lid/brow cell above or beside the eye gives it weight. Keep at least one white cell visible.
- Avoid a "white block with a divider line": that reads as a wall, not an eye.

```
2x3 looking left   2x3 looking right   2x2 looking left
 1W                 W1                  1W
 1W                 W1                  WW
 WW                 WW
```
(`1` pupil, `W` white. Put a lid or brow row of outline/skin colour above; 3 wide is only for a big head.)

Side-view face, eye near the front edge, pupil on the outer side reads as looking forward:
```
..1111..
.1222W1.
.1222W1.
```
Try two or three variants (move the pupil one cell, change the lid) and view each at 1x before choosing.

## 4. Faces: muzzle, mouth, separators

- A muzzle or snout is usually a different value from the head: a lighter patch with a 1 px dark
  nose at its tip. A flat one-colour face looks blank.
- A mouth is one dark pixel or a short 1 px line. One pixel is enough if it is placed well.
- **Separators**: where head meets body, or chest meets belly, put a deliberate 1 px dark line or a
  value step. Without it parts merge into one lump.
- One pixel of contrast each for cheek, brow, ear is often enough to make the head "turn".

Side view facing right (`3` light muzzle, `1` dark nose and mouth):
```
..1222....
.12223333.
.122333331   <- nose: one dark pixel at the tip
.12233311.   <- mouth: a short dark line
..12222...
```
Separator between face and body:
```
.1333331.   <- light face
.1111111.   <- 1 px dark separator (or a value step)
.1222221.   <- body
```

## 5. Hands, gloves, feet, shoes

They carry identity colours and are tiny, so they go wrong first.
- Keep the **bright identity colour** (`R`) with **one highlight pixel and a darker shade**, even at
  3 to 4 px wide. A flat block of the colour is a lost shoe.
- Add a strap, cuff or sole line as one dark or white pixel row if the reference has it.
- Large hands (big characters, title screens): follow the source's anatomy closely: overall rounded
  glove shape, a separate thumb, individual fingers, knuckle and finger-gap lines exactly where the
  source has them, a cuff ring at the wrist, lit and shaded sides. Never a flat slab or striped ladder.
- Hands: a round 2x2 or 3x3 cluster, outline on the open side; a thumb is one pixel that breaks the
  circle. Gloves are often white or bright: make them stand out from the arm.

```
shoe, side view: highlight W, accent R, shade 1
.WW1..
1RRRR1
1RRR11
.1111.
```

## 6. Limbs in motion

- A limb is a **line** 1 or 2 px wide with a clear direction, not a blob.
- Use even step lengths for diagonals (section 7). A mix of 2s and 3s reads as a smear.
- Keep gaps open: a 1 px gap shows two legs, or an arm swinging away from the body.
- In a walk or run, a limb keeps the same thickness from frame to frame.

## 7. Outlines

- **Closed**, **1 px**, one colour. Outline selectively: where a light area meets a light
  background use a darker line; inside the shape, a value change can replace the line.
- Curves: step lengths that grow then shrink (1,1,2,3 ... 3,2,1,1), never random.
- No **doubled steps** (a pixel touching its neighbour both orthogonally and diagonally).
- No **jaggies** (an L-corner pixel on an otherwise clean diagonal).

```
good arc: steps 1,2,3        bad arc: doubled corner and L
..11111111..                  ..11111111..
.11........11.                .111......111.
11..........11                .1.1......1.1.   <- L corners
1............1                11..........11
```
Squint: a good line looks smooth when blurred; a bad one shows bumps.

## 8. Clusters, noise, shading

- No **orphans**: a single pixel with no same-colour neighbour is noise unless it is a deliberate
  detail (eye glint, nose). Merge it or remove it.
- No checkerboard, dithering or pillow shading (dark ring around a light middle).
- Choose one **light direction** (top-left is common) and shade the opposite side with one band 1 to 2 px wide.
- 2 or 3 big shapes beat many small ones: big simple shapes read at 1x.

## 8b. Textures

A texture is a repeating unit, not noise. Downscaling turns detailed textures into speckle; redraw them.
Clean does not mean simplified: keep as much of the source's detail as the resolution allows, and match it
closely. Irregularity the source has on purpose (uneven grass heights, varied rock facets, terrace lips,
cracks) stays; only accidental noise goes.
- **Shadows and shading follow the source exactly**: where the source is dark (under ledges, inside each
  checker square's shaded edge, cast shadows of overhangs, the dark side of every facet), the pixel art is
  dark in the same place and in the same proportion. Don't replace the source's shading with a generic
  band or a uniform light direction when the source shows something else. Compare shadow placement panel
  by panel against the reference at zoom.
- Find the unit in the original: checker squares, brick courses, marble veins, metal panels and rivets,
  grass blades, water streaks and foam. Decide its size at target scale (often 4 or 8 px) and draw one
  clean unit.
- Repeat it exactly: `snippet save` the unit and fill with `pxl pattern PART FRAME UNIT --region ... [--mask KEY]`
  (fixed phase, so repeats are identical and neighbours line up), or `stamp` it / copy a region.
- Align it: the same texture continues across neighbouring pieces, tiles and animation frames at the same
  phase. Lock the phase to the object's grid or origin; a scrolling texture moves by whole pattern steps.
- Balance it: base colour plus one or two pattern values, evenly spread. No checkerboard dithering or
  speckle unless the original uses a deliberate dither. Give the textured area a clean edge (outline or
  highlight row) so it doesn't fray.
- Water and waterfalls: vertical streaks with even spacing in 2 or 3 values, a regular white foam or
  highlight rhythm at the top and edges; each frame shifts the streaks evenly so it flows.
- Check textures in a lineup with their neighbours at 1x and zoomed (`pxl lineup A:f B:f --zoom 4`, a tile beside itself for tiles):
  misaligned or unbalanced texture is a defect even when every single frame looks fine.

## 8c. Source-pixel fidelity (the approved style)

When the source is pixel art at an exact multiple of the target (often 2x), build the target from the
source's own pixels rather than from generic units: sample the source at the part's phase (`nearest` or
`pick`), then go over it by hand and restore what halving lost: 1-px details (glints, notches, cracks,
bolt dots, grass tips), seams between checker squares, outlines that broke, and the source's shadow and
highlight placement. Irregularity stays (uneven grass, varied facets). Textures stay aligned because the
phase comes from the source. This keeps the source's character and shadows far better than redrawing
textures as clean units. Check every changed area side by side with the source at zoom.

## 9. Limited palettes (e.g. 3 colours + transparent per layer)

- Give each colour a role: outline (darkest), body (main), light (skin, face, highlights).
- The **overlay (detail) layer** has its own 3 colours. Use it for small bright details that would
  break the base palette: white eyes and gloves, bright shoes, glints. Each overlay key has an
  `under` base colour that shows where the overlay is dropped.
- Put the identity colour of a part (the shoe red) in the overlay rather than squeezing it into the base.
- When two source colours must share one key, choose by role (outline or shade?), not by nearest RGB.

## 10. Consistency across animation

- Draw the eye, shoe and glove once, well. `snippet save` them, `stamp` them in every frame;
  change only what moves (pupil direction, shoe angle).
- Same ground line, origin and body width in every frame; `nudge` to realign.
- Copy unchanged regions from the approved frame (`copy --region`); do not redraw them.
- View all frames in a row (`compare --frames 'walk*'`) to spot a frame that jumps.
- Squash and smear may vary; the eye and the identity colours should not.

## 10b. Consistency across objects

Objects from the same world or maker should look made by one hand: a villain's ship, his capsule and his
bosses share one metal; all minions share one robot look; all items share one finish.
- Same material, same colours: metal light/mid/dark grey, hazard yellow/black, outline colour, light
  colours are the same RGB in every part that shows them (use a shared template via `extends`).
- Same light direction (top-left), same shading depth (one shade band), same highlight style
  (1 px glint vs a band), same hazard-stripe angle and spacing, same outline treatment.
- Variants of one object (intact and damaged, open and closed, the same character's face in a boss,
  an icon and a sign) must be the same drawing with only the difference changed. When two versions
  disagree, pick the better one as the reference and redraw the other from it.
- Before releasing a part, put one frame next to frames of its siblings (a lineup at 1x and zoomed)
  and fix what doesn't match. Check the project's materials notes (e.g. `pxl/templates/MATERIALS.md`).

## 11. When to deviate from the reference

Deviate when the reference at 1x would be:
- a lost feature smaller than one cell that matters: draw it bigger or move it
- a blob (two parts merging): add a separator or open a gap
- noise (texture, gradient): replace by two flat shapes
- a wobbly line: straighten it
Do not deviate for decoration. Mention large departures in the release note.
