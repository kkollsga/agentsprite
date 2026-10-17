---
name: pxl-extract
description: Extract source graphics (a ROM the user provides, a disassembly, community rips and sheets, emulator VRAM/OAM/palette dumps) into clean PNG frames and hand them to pxl in the inbox contract. Use when the user has game data rather than ready images, when asked to rip, dump, decode or extract sprites, tiles, palettes or screens, or when a pxl source QA finding says the source art is wrong.
---

# pxl-extract: get the source graphics out, properly

If `pxl --version` fails, `pxl` is not installed: run the installer (`curl -fsSL https://raw.githubusercontent.com/kkollsga/agentsprite/main/install.sh | sh`, Windows: `irm https://raw.githubusercontent.com/kkollsga/agentsprite/main/install.ps1 | iex`) or see the `pxl-operator` skill; `pxl skill install` refreshes these skills.

pxl reads PNG. It has no ROM reader and no game-format decoders: the extraction is yours, with scripts you write in the scratchpad
or tools you already have. This skill is how you turn any source into the
handover that `pxl ingest` and source QA like best. Extraction is half of the quality: wrong offsets, wrong palettes or art of
another zone are the most expensive mistakes of a project, and they only show late.

**Legal.** Only extract data the user provides and has the right to use (their own game, a licence, a permitted study or
port). Never download or bundle copyrighted ROMs or rips; never commit extracted game art to a public repository; say so in the
handover note. If the right to use is unclear, ask the user once and record the answer (`pxl decide`).

## Sources and tools

| Source | Use |
|---|---|
| Disassembly or ROM art (compressed tiles, mappings, palettes) | decode with a small script you write in the scratchpad, following the format documentation of the console (`references/decoders.md`); write PNG |
| Raw tile data of any console (1/2/4/8 bpp, planar or packed), compressed with LZ77/LZSS/RLE | decode with a small script you write in the scratchpad (never in the project), or an external tool; write PNG |
| Community rips and sprite sheets | check the legal status, then slice them (`pxl ingest --grid WxH | --frames auto`) |
| Emulator dumps (VRAM, OAM, CRAM, screenshots) | debug tools of the emulator (Mesen, BizHawk, Gens/Exodus, mGBA, bsnes); dump tiles, OAM and palettes at one moment, and rebuild the frame from OAM |
| Level data | render the map with the console's tilemap format in your own script |

Prefer the original game data over screenshots: tiles give the exact source palette and indices with no scaling blur.

## The handover: the inbox contract (`references/inbox-contract.md`)

One folder per object: `inbox/<group>/<object>/` holding

- `<object>_NN.png`, frames in the **original order** (`_00` or `_01`, consistent), the **source palette, no reduction**,
  transparency as alpha, one consistent canvas and origin per object;
- the source palette as `palette.png` (a swatch strip) or in `source.json`;
- `source.json`: source file and offset, mapping labels, palette line, frame semantics (distinct or engine-flipped),
  the zone or scene, the unused flag;
- `contact.png`: a labelled contact sheet for a quick human check (`pxl lineup` or your own).

Then `pxl ingest inbox/<group> --group <group>` makes one asset per object (numbered files become frames) and copies
`source.json` to the asset's `input/`. Source QA reads it.

## The loop

1. **Find** the art: identify the format and the offset (header, disassembly labels), decode a tile sheet to PNG and count tiles to find
   the offset a mapping needs.
2. **Match frames by label, never by position.** Mappings tables can start with offset entries; the index shifts when entries are added.
   Print one line per table entry while you render.
3. **Right palette.** Check the palette line against a screenshot or the object's neighbours; a wrong line makes a believable but wrong
   picture. Record the line in `source.json`.
4. **Which zone or scene?** An object's art can be shared or borrowed (the mapping draws from another zone's art). Record `zone` (the
   one the asset is for) and `art_zone` (where the pixels come from). Mark labels the game never uses as `unused`.
5. **Render the frames** to PNG at 1x (nearest-neighbour scaling later, never in the file).
6. **QA before handover** (`references/qa.md`): noise, borrowed art, missing frames vs the mapping, consistent origins. `pxl qa` runs the
   checks on ingested assets.
7. **Contact sheet**, then hand over: tell the operator what was extracted, what was left out and why.

## What not to do

- Do not reduce, recolour, crop to the visible pixels or scale: reduction is pxl's job, and the source palette is the evidence.
- Do not guess offsets silently: when a render is noise, stop and say which file and offset failed.
- Do not draw from unused or borrowed frames; flag them.
- Do not write extraction scripts into the project folder; keep them in the scratchpad and record the command in `source.json`.
