# QA before handover

The mistakes below cost the reference project whole rounds of work. Check each one for every object, then say what you checked.

| Check | How | What to do when it fails |
|---|---|---|
| **Noise / garbage** | The picture looks like random pixels or stripes: wrong offset, wrong compression, compressed data shown as tiles. `pxl qa` scores it (over 0.65 with more than 8 colours is flagged). | Find the right offset (the disassembly, a tile viewer). Do not hand over garbage with a note. |
| **Borrowed art** | The mapping's tile numbers point into another zone's or another object's art; the picture looks right only by accident of the palette. | Set `zone` and `art_zone` in `source.json`; the operator gets a question task. |
| **Unused frames** | A frame no animation, object list or level uses (leftover art). | `"unused": true` in `labels`; do not number it into the frames the artist must draw. |
| **Missing frames vs the mapping** | Count entries in the mapping/animation table against your PNGs; account for every one (frame, flip, or unused). | Extract the missing ones, or list them as dropped with a reason. |
| **Consistent canvas and origin** | Open the contact sheet: feet and centre stay put between frames; the canvas is the same. | Re-render with a common canvas and origin. |
| **Right palette line** | Compare the colours with a screenshot or a neighbouring object of the same scene; a wrong line is plausible but off. | Fix the line; record it. |
| **Transparency** | Background is alpha 0, not a colour; no halo. | Re-render with index 0 transparent. |
| **Flip frames** | The mapping reuses a frame with a flip flag. | `semantics: flip_x` and `of` in `labels`; the artist does not redraw it. |

After ingest: `pxl qa` (noise, borrowed art, unused) and `pxl tree`; open the compare images of two or three objects at random.
