# Decoders and tools

## Disassembly and ROM formats (no built-in decoder)

pxl itself reads PNG only. For compressed art, mappings and palettes write a decoder in the scratchpad from the format documentation, and
keep the command in `source.json`. Points that matter on every console:

- A palette line selects 16 (or 4) colours; a wrong line gives a believable but wrong picture. Check it against a screenshot.
- Frame names should follow the label of the mapping entry, not its position; local labels lose their dot. Match by label.
- A dynamic pattern loading script means a frame's tiles are loaded piece by piece: apply it or the render is wrong.
- Art loaded at a VRAM tile offset is how mappings number their tiles: a wrong offset gives shifted garbage.

## Other consoles: raw tiles

Write a short decoder (in the scratchpad) from the console's format and write PNG:

| Console | Tile format | Palette |
|---|---|---|
| GB / GBC | 8x8, 2 bpp, 16 bytes, two bitplanes interleaved by row | DMG: 4 shades; GBC: 4 colours of RGB555 per palette |
| NES | 8x8, 2 bpp, 16 bytes: plane 0 then plane 1 | 4 colours per palette from the 64-entry master palette |
| SMS / GG | 8x8, 4 bpp, 32 bytes, 4 interleaved planes per row | 16 colours (SMS 6-bit, GG 12-bit) |
| Mega Drive | 8x8, 4 bpp, 32 bytes, packed nibbles | 16 colours of 9 bits, 4 lines |
| SNES | 8x8, 2/4/8 bpp planar | 15-bit BGR |
| PC Engine | 8x8, 4 bpp, planar in 16-byte pairs (sprites 16x16 blocks) | 9-bit GRB |
| GBA | 8x8, 4 bpp packed or 8 bpp | 15-bit BGR, 16 or 256 colours |

Facts per console are in `pxl console show ID` (it cites its sources). Check a decoder against a known tile (a letter, a border) before
trusting it. Common compressions: LZ77/LZSS (GBA BIOS, many GB/NES games), RLE, Nemesis/Kosinski/Enigma (Mega Drive); identify by the header
or the disassembly, not by trial.

## Emulator dumps

1. Pause at a frame where the objects are on screen. 2. Dump VRAM (tiles), OAM (sprite list: x, y, tile, palette, flip), and CRAM or
palette RAM. 3. Rebuild each object from its OAM entries with the right palette and flips, on a transparent canvas. 4. Record the emulator,
the frame and the dump files in `source.json`. A screenshot is evidence for the palette line, not the source of pixels.

## External tools

Use what is installed and the user allows (tile viewers such as Tile Molester or YY-CHR, Mesen/BizHawk tile viewers, `ffmpeg` and
ImageMagick for conversions). Do not install software without asking. Never put a tool's output into the project without the
`source.json` that says how it was made.
