//! Console-native palette and sprite files written next to the PNGs (M6 R2): `<name>.palette.json` lists the palette lines of a
//! part (one line per colour layer, index 0 transparent, `colours + 1` entries each) with every colour in the console's own
//! encoding, and `<name>.sprites.json` the sprite cover of every frame for consoles with per-sprite sizes.

use crate::color::Rgb;
use crate::model::Part;
use crate::profile::Profile;
use serde_json::json;

/// The 2C02 master palette as commonly rendered (index = the 6-bit palette value). The real output varies with the
/// decoder; the structure (64 entries, black at $0F, $0D unusable on some TVs) is from nesdev.org/wiki/PPU_palettes.
pub const NES_MASTER: [u32; 64] = [
    0x7C7C7C, 0x0000FC, 0x0000BC, 0x4428BC, 0x940084, 0xA80020, 0xA81000, 0x881400,
    0x503000, 0x007800, 0x006800, 0x005800, 0x004058, 0x000000, 0x000000, 0x000000,
    0xBCBCBC, 0x0078F8, 0x0058F8, 0x6844FC, 0xD800CC, 0xE40058, 0xF83800, 0xE45C10,
    0xAC7C00, 0x00B800, 0x00A800, 0x00A844, 0x008888, 0x000000, 0x000000, 0x000000,
    0xF8F8F8, 0x3CBCFC, 0x6888FC, 0x9878F8, 0xF878F8, 0xF85898, 0xF87858, 0xFCA044,
    0xF8B800, 0xB8F818, 0x58D854, 0x58F898, 0x00E8D8, 0x787878, 0x000000, 0x000000,
    0xFCFCFC, 0xA4E4FC, 0xB8B8F8, 0xD8B8F8, 0xF8B8F8, 0xF8A4C0, 0xF0D0B0, 0xFCE0A8,
    0xF8D878, 0xD8F878, 0xB8F8B8, 0xB8F8D8, 0x00FCFC, 0xF8D8F8, 0x000000, 0x000000,
];

/// Master palette entries without the repeated blacks, in index order (the first black is $0F).
pub fn nes_unique() -> Vec<(u8, Rgb)> {
    let mut v: Vec<(u8, Rgb)> = Vec::new();
    for (i, c) in NES_MASTER.iter().enumerate() {
        let rgb = Rgb { r: (c >> 16) as u8, g: (c >> 8) as u8, b: *c as u8 };
        if *c == 0 {
            if v.iter().any(|x| x.1 == rgb) {
                continue;
            }
            v.push((0x0F, rgb));
            continue;
        }
        if !v.iter().any(|x| x.1 == rgb) {
            v.push((i as u8, rgb));
        }
    }
    v.sort_by_key(|x| x.0);
    v
}

/// Native value of a colour: the CRAM word (md `0BGR` with bits 1, 5, 9 per channel, gg 12-bit `0BGR`, sms `00BBGGRR`), the
/// master palette index (nes) or the shade 0..3 (gb).
pub fn native(kind: &str, c: Rgb, prof: &Profile) -> Option<u32> {
    let lv = |v: u8, step: u32| (v as u32 / step).min(255 / step);
    match kind {
        "snes" | "gba" => Some((lv(c.b, 8) << 10) | (lv(c.g, 8) << 5) | lv(c.r, 8)),
        "pce" => Some((lv(c.g, 32) << 6) | (lv(c.r, 32) << 3) | lv(c.b, 32)),
        "md" => Some((lv(c.b, 32) << 9) | (lv(c.g, 32) << 5) | (lv(c.r, 32) << 1)),
        "sms" => Some((lv(c.b, 64) << 4) | (lv(c.g, 64) << 2) | lv(c.r, 64)),
        "gg" => Some((lv(c.b, 16) << 8) | (lv(c.g, 16) << 4) | lv(c.r, 16)),
        "nes" => nes_unique().iter().find(|x| x.1 == c).map(|x| x.0 as u32),
        "gb" => prof.fixed_colours.iter().position(|x| *x == c).map(|i| i as u32),
        _ => None,
    }
}

pub fn encoding(kind: &str) -> &'static str {
    match kind {
        "md" => "CRAM word 0000BBB0GGG0RRR0 (3 bits per channel, value = 8-bit channel / 32)",
        "snes" => "CGRAM word 0BBBBBGGGGGRRRRR (BGR555, 5 bits per channel, value = 8-bit channel / 8)",
        "gba" => "palette RAM halfword 0BBBBBGGGGGRRRRR (BGR555, value = 8-bit channel / 8)",
        "pce" => "VCE word 0000000GGGRRRBBB (9-bit, 3 bits per channel, value = 8-bit channel / 32)",
        "sms" => "CRAM byte 00BBGGRR (2 bits per channel, value = 8-bit channel / 64)",
        "gg" => "CRAM word 0000BBBBGGGGRRRR (4 bits per channel, value = 8-bit channel / 16)",
        "nes" => "master palette index $00-$3F (black = $0F)",
        "gb" => "shade 0 (lightest) to 3 (darkest)",
        _ => "",
    }
}

/// `<name>.palette.json`: palettes x (colours + 1) entries.
pub fn palette_json(prof: &Profile, part: &Part, layers: &[(String, Vec<Rgb>)]) -> String {
    let kind = prof.export_palette.as_str();
    let per = prof.colors as usize + 1;
    let lines: Vec<_> = layers
        .iter()
        .enumerate()
        .map(|(li, (name, pal))| {
            let entries: Vec<_> = (0..per)
                .map(|i| {
                    if i == 0 {
                        json!({"index": 0, "transparent": true})
                    } else if let Some(c) = pal.get(i) {
                        json!({"index": i, "rgb": crate::color::to_hex(*c), "native": native(kind, *c, prof), "native_hex": native(kind, *c, prof).map(|n| format!("0x{:04X}", n))})
                    } else {
                        json!({"index": i, "unused": true})
                    }
                })
                .collect();
            json!({"line": li, "layer": name, "entries": entries})
        })
        .collect();
    let v = json!({"console": prof.name.split('/').next().unwrap_or(""), "part": part.name, "encoding": encoding(kind), "colours_per_line": prof.colors, "entries_per_line": per, "lines": lines, "entries": layers.len() * per});
    serde_json::to_string_pretty(&v).unwrap_or_default() + "\n"
}

/// `<name>.sprites.json`: the sprite cover of every frame (variable sprite sizes).
pub fn sprites_json(prof: &Profile, part: &Part) -> String {
    let frames: Vec<_> = part
        .frames
        .iter()
        .map(|f| {
            let p = crate::sprites::place(&f.grid, &prof.sprite_sizes);
            json!({"frame": f.name, "count": p.len(), "sprites": p.iter().map(|s| json!({"x": s.x, "y": s.y, "w": s.w, "h": s.h})).collect::<Vec<_>>()})
        })
        .collect();
    serde_json::to_string_pretty(&json!({"part": part.name, "sizes": prof.sprite_sizes, "frames": frames})).unwrap_or_default() + "\n"
}
