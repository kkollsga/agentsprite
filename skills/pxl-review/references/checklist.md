# Review checklist

Judge on the PNGs (compare pages, the 1x sheet on the level colour, the sibling lineup), never on numbers.
A "no" is a rework note for the named frames. Notes name the frame, the place (x,y or "left eye") and the fix.

## Finish
- [ ] **Hand-finished**: no frame or tile is still raw method output (`pxl status --raw` lists none).
- [ ] **Clean**: no specks, orphan pixels, stair blobs or noisy shading; outlines closed, 1 px, even steps.

## Features (every frame)
- [ ] **Eyes**: count them — every eye the original shows is there (two eyes or two goggle lenses stay two,
      in every frame and every place the character appears), with white and a pupil that reads as a pupil
      (not merged into the outline), looking the right way.
- [ ] **Faces**: muzzle, nose, mouth and expression as in the original (an open mouth stays open).
- [ ] **Defining details** survive: nostrils, separators between body parts, hands, feet, spikes, the
      object's signature parts (a drill bit, a claw, a propeller).
- [ ] **Major features complete**: list the original's major features in words and tick each one. Small
      details may go; major features may not.

## Shape
- [ ] **Silhouette** stays inside the original's shape (about 1 px); the pose reads the same.
- [ ] **Round stays round**: no boxy heads, bodies or feet; no square corners the original doesn't have.

## Colour
- [ ] **Identity colours** survive: bright shoes, gloves, red coats, blue glass, a ball that shows state.
- [ ] **White follows the style rule** (`[style] white`, `style.md`): on most consoles only eyes, glints and edge highlights, large
      white fills (legs, bodies) are wrong unless the original is white there; on the classic Game Boy white is a main tone.
- [ ] **Big colour areas keep their hue**: grey stays grey, blue stays blue.
- [ ] **Highlighted shapes keep their highlights**: a round yellow shape with white rim glints must not
      become a flat blob.
- [ ] **Fidelity**: as much of the source's detail as the resolution allows; deliberate irregularity
      (uneven grass, varied facets, lips, cracks) kept; not simplified into generic shapes.
- [ ] **Shadows match the source**: dark areas, cast shadows and shaded edges sit where the source has them,
      in the same proportion (check side by side at zoom).
- [ ] **Value contrast**: shapes separate by brightness, not only by hue: dark outlines/separations
      between features, light highlights and glints, and a palette whose colours spread across dark, mid
      and light. Squint at 1x: if the face or object turns into one mid-tone blob, it fails.
- [ ] **Depth**: volumes have a light side, a shade side and a highlight; no flat single-value fills.
- [ ] **Special frames** keep their look (a burnt, black-and-white frame stays charred).

## Cells and budgets (consoles with shared palette sets: gb, gbc, nes)
- [ ] **No palette seams**: look at `pxl cells PART FRAME --png c.png --zoom 6` next to the frame: where a cell border meets a colour
      change, the two cells' ramps must still read as one surface (same hue family, outline colours that differ per cell are a
      deliberate shade, a hard step from one hue to another across a border is a seam). View it in every zone's colours for
      multi-set parts (`--set ZONE`).
- [ ] **Full ramp per cell**: a cell that shows only two of its slot's three colours looks flat; shoes, eyes and icons keep their
      highlight and shade inside their own cell.
- [ ] **Cell-aware drawing**: identity colours (shoes, gloves) sit in their own band, icons in their own cell, shared bodies are reused (`pxl cells dedupe`
      report: no merge with a visible difference), repeating segments are repeated cells.
- [ ] **Per-scene budgets**: `pxl check` is green for every scene that lists the part (`pxl scene NAME`: palettes, sprite VRAM,
      DMA, BG tiles); `pxl scene NAME --worst` stays within the console's per-line and per-frame sprite limits (`pxl console show`). A client checker that passes is necessary, not
      sufficient: it checks assets one by one; the scene check is the shared hardware.

## Textures
- [ ] Textures (waterfall, water, checker soil, bricks, marble, metal plating, grass) are clean repeating
      units, not downscale noise; balanced (one or two pattern values); same phase across neighbouring
      pieces, tiles and animation frames; water flows evenly frame to frame.

## Consistency
- [ ] **Variants are one drawing**: intact and damaged, open and closed, the same character in several
      places (boss, icon, sign). When they disagree, the better one is the reference.
- [ ] **Families share materials**: same metal, stripes, outline, light direction and highlight style across
      related objects (check the sibling lineup).
- [ ] **Animation**: same eye, glove, shoe and ground line across frames; unchanged parts identical.

## Compromises
- [ ] Every palette compromise or overlay is recorded in DELIVERY_NOTES.md; decisions already made by the
      user are respected (don't reopen them, don't silently deviate).
