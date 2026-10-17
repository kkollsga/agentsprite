# The inbox contract

```
inbox/
  <group>/                      enemies, player, zone_1, hud ...
    <object>/                   one folder per object (a character, an enemy, a block sheet)
      <object>_00.png           frames in original order, no gaps; source palette, alpha for transparency
      <object>_01.png
      palette.png               swatch strip of the source palette line(s) (optional but useful)
      source.json               where it came from
      contact.png               labelled contact sheet of the frames
```

`pxl ingest inbox/<group> --group <group>` turns each object folder into one asset (numbered files are frames; `--separate` opts out of
grouping). `source.json` is copied to the asset's `input/source.json`, where `pxl qa` reads it.

## Rules for the pictures

- **Original order.** Frame `_NN` is the NN-th entry of the mapping or animation table, not sorted by looks. Gaps are explicit (an
  unused or dropped frame is listed in `source.json`, not silently missing).
- **Source palette.** Index colours rendered to RGB through the real palette line. Do not quantize, dither or reduce.
- **Transparency as alpha.** Index 0 (or the colour key) is transparent; no background colour baked in.
- **One canvas and origin per object.** Every frame of an object has the same canvas size; the object's origin (usually the feet or
  centre) is at the same pixel. Record `canvas` and `origin` in `source.json`.
- **1x.** No scaling in the file. (Integer upscales you cannot avoid, such as an emulator screenshot at 2x, are recorded as `factor`.)

## source.json

```json
{
  "object": "crawler",
  "source": {
    "file": "art/nemesis/Crawler.nem",
    "offset": "0x0000",
    "decoder": "nemesis",
    "mappings": "mappings/Crawler.asm",
    "dplc": null,
    "command": "python3 extract_frames.py art/nemesis/Crawler.nem mappings/Crawler.asm pal/Zone1.bin 1 out/crawler"
  },
  "palette": { "file": "pal/Zone1.bin", "line": 1, "colours": ["#000000", "#e02828", "#901820"] },
  "canvas": { "size": [32, 32], "origin": [16, 28] },
  "factor": 1,
  "zone": "zone_1",
  "art_zone": "zone_1",
  "unused": false,
  "labels": [
    { "name": "crawler_00", "mapping_label": ".walk1", "semantics": "distinct" },
    { "name": "crawler_01", "mapping_label": ".walk2", "semantics": "distinct" },
    { "name": "crawler_02", "mapping_label": ".walk1_r", "semantics": "flip_x", "of": "crawler_00" },
    { "name": "crawler_03", "mapping_label": ".unused", "unused": true }
  ],
  "legal": "provided by the user; use right confirmed 2026-10-17 (decision d0003)",
  "notes": ""
}
```

Fields `pxl qa` acts on: `zone` vs `art_zone` (differ: borrowed art, a `question` task is created), `unused` (asset) and
`labels[].unused` (frames), and the noise score of the pictures. The rest is for humans and for the operator's brief:
`semantics` tells the artist whether a frame is a distinct drawing or the engine's flip of another (do not redraw a flip).
`pxl meta ASSET frame NAME semantics flip_x --of FRAME` carries it into the delivery manifest.

## Handover note

End with a short note to the operator: how many objects and frames, from which files, the palette lines used, anything left out
and why, what QA found, and the legal basis. Put it in `inbox/<group>/EXTRACTION.md`.
