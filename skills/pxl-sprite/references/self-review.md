# Self-review: be your own art director

Before you say "done", spend a minute looking as someone who did not draw it. Metrics show where
to look; they are not the verdict.

## What to look at

1. `pxl compare PART --frames FRAME --zoom`: reference | result, large. Read the PNG.
2. The same frame at 1x on the level background: `pxl compare PART --frames FRAME --on "#406080"`.
   Squint or step back; this is what the player sees.
3. Next to the approved frames or your best frame so far: `pxl compare PART --frames A,B`.
4. For animations, all frames in a row: `pxl compare PART --frames 'walk*'`.

## Questions (answer each with the PNG in view)

- Would I recognise this character and pose at 1x, with the label covered?
- Eyes: both present, expressive, pointing the right way, clean white and pupil (not a block)?
- Face: a muzzle, a mouth, a visible separator between face and body? Is the head turned the right way?
- Identity colours (shoes, gloves, belly, accessories): present, with a highlight and a shade?
- Outline: a clean closed line? Anything blocky, doubled, wobbly, accidentally spiky?
- Limbs: lines with direction, or blobs?
- Noise: orphan pixels, checkerboard, flat patches that should have a shape?
- Against the approved frame: same eye, same shoe, same ground line, same light direction?
- If I must criticise one thing, what is it? Fix that, then ask again.

## Habits

- Tick each area (eyes, face, hands, feet, outline, 1x read) from the PNG, not from memory.
- If a lint warning keeps you busy for more than two tries, ask what the picture needs instead.
  Fix the picture. If the picture is right, leave the warning and say so in the note.
- Try one alternative on the weakest area (another eye, another arm). Compare, pick.
- Never judge from the text grid alone; look at the PNG.

## Final review before release (every part, every time)

Do this once more for the whole part, after the last edit, with `pxl compare PART --zoom` pages open
next to the reference. It catches what frame-by-frame work misses.

1. **List the original's major features** in words, from the reference: each eye, each distinct
   colour element (a blue patch, grey fins, a red shell), highlights along edges, the main parts
   (head, body, limbs, tool or weapon). Small details may go; major features may not.
2. **Tick each one in every frame.** Missing one is a defect: an eye that disappeared in one frame,
   a blue element that merged away, fins that lost their grey.
3. **White follows the console's style** (`pxl console show`, `[style] white`, or `style.md`). On most consoles white is for
   highlights: eyes, glints, a leading edge, a shine along a rim; large white areas (legs, bodies, fills) are wrong unless the
   original is white there. On the classic Game Boy white is a main tone (three shades only), so use it actively or the
   sprite goes flat. Grey stays grey: metal legs, fins and wheels are grey with at most a light edge.
4. **Colours by area**: the big colour areas of the original keep their hue (grey stays grey, blue
   stays blue). A highlighted shape (a yellow ball with white rim highlights) keeps its highlights,
   or it becomes a flat blob.
5. **Depth**: round or boxy volumes (a capsule, a ball, a shell, metal casing) need a light side and
   a shaded side, two values of the same hue plus a highlight. A single flat fill reads as a cut-out with
   no depth; the original's shading tells you where light and shadow go.
6. **Prioritise**: you cannot keep every detail at this size. Make the major features as good as
   possible: clean, clear, with their own colours. Spend the remaining effort on the weakest major
   feature (a drill bit, a claw), not on tiny details.
7. Fix what fails, re-check, then release with a note naming any major feature you could not keep
   and why.

## When to stop

Stop when the questions answer yes, or when the remaining flaw can be named in the note ("left
hand is a 2x2 blob, the reference hides it"). Do not stop because numbers are green, and do not
polish an invisible 1 px detail while a limb is still a blob.
Release with a note: what you kept, what you exaggerated, what is still weak.
