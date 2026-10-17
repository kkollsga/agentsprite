//! Generates the example inputs in `examples/inputs/`: original pixel art drawn by code (CC0, see examples/LICENSE).
//! "Source style": 16-bit-era shading and palette at 2x the target resolution of a handheld console.
//!
//!   cargo run --example gen_inputs            # rewrites examples/inputs/**.png (deterministic)
//!
//! hero/hero_01..03 (32x48, target 16x24), enemy/crawler_01..02 (32x32), item/gem (32x32), tiles/ground (64x64, a 32x32 tile region),
//! screens/title_WxH (the screen of every console at 2x, e.g. 320x288 for the 160x144 handhelds).

use agentsprite::img::{Rgba, RgbaImage};
use agentsprite::png;
use std::path::Path;

type C = (u8, u8, u8);
const OUTLINE: C = (0x18, 0x18, 0x30);
const SKIN: C = (0xf8, 0xc8, 0xa0);
const SKIN_D: C = (0xd0, 0x90, 0x68);
const FUR: C = (0xf0, 0x90, 0x30);
const FUR_D: C = (0xb0, 0x58, 0x20);
const FUR_L: C = (0xff, 0xc8, 0x70);
const SUIT: C = (0x20, 0x98, 0x98);
const SUIT_D: C = (0x10, 0x60, 0x70);
const SUIT_L: C = (0x60, 0xd0, 0xc0);
const RED: C = (0xe0, 0x28, 0x28);
const RED_D: C = (0x90, 0x18, 0x20);
const RED_L: C = (0xff, 0x78, 0x60);
const WHITE: C = (0xf8, 0xf8, 0xf8);
const GOLD: C = (0xf8, 0xc8, 0x28);
const GOLD_D: C = (0xc0, 0x80, 0x10);
const GOLD_L: C = (0xff, 0xf0, 0x98);
const GREEN: C = (0x48, 0xa8, 0x38);
const GREEN_D: C = (0x28, 0x70, 0x30);
const GREEN_L: C = (0x88, 0xd8, 0x58);
const DIRT: C = (0x98, 0x68, 0x38);
const DIRT_D: C = (0x68, 0x40, 0x20);
const DIRT_L: C = (0xc0, 0x90, 0x58);
const STONE: C = (0x90, 0x98, 0xa8);
const STONE_D: C = (0x58, 0x60, 0x78);

struct Canvas {
    im: RgbaImage,
}

impl Canvas {
    fn new(w: usize, h: usize) -> Canvas {
        Canvas { im: RgbaImage::new(w, h) }
    }
    fn put(&mut self, x: i32, y: i32, c: C) {
        if x >= 0 && y >= 0 && (x as usize) < self.im.w && (y as usize) < self.im.h {
            self.im.px[y as usize * self.im.w + x as usize] = Rgba::opaque(c.0, c.1, c.2);
        }
    }
    fn rect(&mut self, x: i32, y: i32, w: i32, h: i32, c: C) {
        for yy in y..y + h {
            for xx in x..x + w {
                self.put(xx, yy, c);
            }
        }
    }
    /// An ellipse with three tones from the light at the upper left: light, base and dark.
    fn ball(&mut self, cx: f64, cy: f64, rx: f64, ry: f64, base: C, dark: C, light: C) {
        for y in (cy - ry).floor() as i32 - 1..=(cy + ry).ceil() as i32 + 1 {
            for x in (cx - rx).floor() as i32 - 1..=(cx + rx).ceil() as i32 + 1 {
                let (nx, ny) = ((x as f64 + 0.5 - cx) / rx, (y as f64 + 0.5 - cy) / ry);
                if nx * nx + ny * ny <= 1.0 {
                    let l = -(0.6 * nx + 0.8 * ny);
                    self.put(x, y, if l > 0.55 { light } else if l < -0.2 { dark } else { base });
                }
            }
        }
    }
    /// A 1 px outline around everything drawn so far.
    fn outline(&mut self, c: C) {
        let snap = self.im.clone();
        for y in 0..snap.h as i32 {
            for x in 0..snap.w as i32 {
                let here = snap.px[y as usize * snap.w + x as usize].a > 0;
                if here {
                    continue;
                }
                let near = [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|(dx, dy)| {
                    let (nx, ny) = (x + dx, y + dy);
                    nx >= 0 && ny >= 0 && (nx as usize) < snap.w && (ny as usize) < snap.h && snap.px[ny as usize * snap.w + nx as usize].a > 0
                });
                if near {
                    self.put(x, y, c);
                }
            }
        }
    }
    /// An eye: a white block with a pupil on the gaze side (`look` = -1 left, 1 right) and a highlight-free dark pupil.
    fn eye(&mut self, x: i32, y: i32, w: i32, h: i32, look: i32) {
        self.rect(x, y, w, h, WHITE);
        let px = if look >= 0 { x + w - 2 } else { x };
        self.rect(px, y + 1, 2, h - 2, OUTLINE);
    }
}

fn hero(pose: usize) -> RgbaImage {
    let mut c = Canvas::new(32, 48);
    // legs and shoes: stand, step one way, step the other
    let (lx, rx) = [(10, 18), (7, 21), (12, 16)][pose];
    for (x, flip) in [(lx, 0), (rx, 1)] {
        c.rect(x, 32, 4, 8, SUIT_D);
        c.rect(x, 32, 2, 7, SUIT);
        let sx = x - 2 + flip;
        c.rect(sx, 39, 8, 4, RED);
        c.rect(sx, 39, 8, 1, RED_L);
        c.rect(sx, 42, 8, 1, RED_D);
    }
    // body, arms
    c.ball(16.0, 26.0, 8.5, 8.0, SUIT, SUIT_D, SUIT_L);
    let swing = [0, 3, -3][pose];
    c.ball(8.0 + swing as f64 * 0.5, 27.0, 2.5, 5.5, FUR, FUR_D, FUR_L);
    c.ball(24.0 - swing as f64 * 0.5, 27.0, 2.5, 5.5, FUR, FUR_D, FUR_L);
    // belly patch
    c.ball(17.0, 29.0, 3.5, 4.0, SKIN, SKIN_D, SKIN);
    // head with ears
    c.ball(9.0, 8.0, 3.5, 5.0, FUR, FUR_D, FUR_L);
    c.ball(23.0, 8.0, 3.5, 5.0, FUR, FUR_D, FUR_L);
    c.ball(16.0, 14.0, 10.0, 9.5, FUR, FUR_D, FUR_L);
    c.ball(18.0, 17.0, 7.0, 5.5, SKIN, SKIN_D, SKIN);
    // eyes: two, white with a pupil, looking right
    c.eye(11, 11, 4, 6, 1);
    c.eye(17, 11, 4, 6, 1);
    c.rect(19, 19, 3, 2, OUTLINE); // nose
    c.rect(17, 23, 5, 1, OUTLINE); // mouth
    c.outline(OUTLINE);
    c.im
}

fn crawler(frame: usize) -> RgbaImage {
    let mut c = Canvas::new(32, 32);
    let step = if frame == 0 { 0 } else { 2 };
    for (x, s) in [(5, 0), (12, 1), (19, 0), (25, 1)] {
        let off = if s == 1 { step } else { 2 - step };
        c.rect(x, 24, 3, 4, RED_D);
        c.rect(x - 1, 27 + off.min(1), 5, 2, RED_D);
    }
    c.ball(16.0, 19.0, 13.0, 9.0, RED, RED_D, RED_L);
    c.rect(3, 22, 26, 3, RED_D);
    // spikes
    for x in [8, 14, 20] {
        c.rect(x, 7, 2, 4, GOLD);
        c.rect(x, 7, 1, 3, GOLD_L);
    }
    c.eye(8, 14, 5, 6, -1);
    c.eye(18, 14, 5, 6, -1);
    c.rect(11, 22, 10, 1, OUTLINE);
    c.outline(OUTLINE);
    c.im
}

fn gem() -> RgbaImage {
    let mut c = Canvas::new(32, 32);
    // a faceted diamond: rows of widening width, three tones
    for y in 4..28 {
        let half = if y < 12 { 6 + (y - 4) * 2 / 1 } else { 22 - (y - 12) } / 2;
        for x in 16 - half..16 + half {
            let t = (x - (16 - half)) as f64 / (2 * half).max(1) as f64;
            let col = if y < 12 { if t < 0.35 { GOLD_L } else if t < 0.7 { GOLD } else { GOLD_D } } else if t < 0.5 { GOLD } else { GOLD_D };
            c.put(x, y, col);
        }
    }
    c.rect(12, 8, 2, 2, WHITE);
    c.outline(GOLD_D);
    c.outline(OUTLINE);
    c.im
}

/// 64x64 = a 4x4 region of 8x8-pixel tiles at target size. Row 0 grass top, row 1 grass edge over dirt, rows 2-3 dirt with stones.
/// Every texture repeats with a 16 px period (one target tile), so the tiles are the same wherever they sit.
fn ground() -> RgbaImage {
    let mut c = Canvas::new(64, 64);
    for ty in 0..4 {
        for tx in 0..4 {
            let (ox, oy) = (tx * 16, ty * 16);
            match ty {
                0 => {
                    c.rect(ox, oy, 16, 16, GREEN);
                    c.rect(ox, oy, 16, 4, GREEN_L);
                    for k in 0..4 {
                        c.rect(ox + k * 4, oy + 6 + (k % 2) * 2, 2, 4, GREEN_D);
                    }
                    c.rect(ox, oy + 12, 16, 4, GREEN_D);
                }
                1 => {
                    c.rect(ox, oy, 16, 16, DIRT);
                    c.rect(ox, oy, 16, 6, GREEN_D);
                    for k in 0..4 {
                        c.rect(ox + k * 4, oy + 5, 2, 3 + (k % 2) * 2, GREEN_D);
                    }
                    c.rect(ox, oy + 6, 16, 2, DIRT_D);
                    c.rect(ox + 2, oy + 10, 3, 2, DIRT_L);
                    c.rect(ox + 10, oy + 12, 3, 2, DIRT_D);
                }
                _ => {
                    c.rect(ox, oy, 16, 16, DIRT);
                    c.rect(ox + 1, oy + 2, 4, 2, DIRT_L);
                    c.rect(ox + 9, oy + 9, 4, 2, DIRT_D);
                    c.ball(ox as f64 + 11.0, oy as f64 + 4.5, 3.5, 2.5, STONE, STONE_D, WHITE);
                    c.ball(ox as f64 + 4.0, oy as f64 + 12.0, 2.5, 2.0, STONE, STONE_D, WHITE);
                }
            }
        }
    }
    let mut im = c.im;
    limit_blocks(&mut im, 32, 4);
    im
}

const GLYPHS: [(char, [&str; 7]); 8] = [
    ('P', ["####.", "#...#", "#...#", "####.", "#....", "#....", "#...."]),
    ('X', ["#...#", "#...#", ".#.#.", "..#..", ".#.#.", "#...#", "#...#"]),
    ('L', ["#....", "#....", "#....", "#....", "#....", "#....", "#####"]),
    ('D', ["####.", "#...#", "#...#", "#...#", "#...#", "#...#", "####."]),
    ('E', ["#####", "#....", "#....", "####.", "#....", "#....", "#####"]),
    ('M', ["#...#", "##.##", "#.#.#", "#...#", "#...#", "#...#", "#...#"]),
    ('O', [".###.", "#...#", "#...#", "#...#", "#...#", "#...#", ".###."]),
    (' ', [".....", ".....", ".....", ".....", ".....", ".....", "....."]),
];

fn text(c: &mut Canvas, s: &str, x: i32, y: i32, scale: i32, col: C, shade: C) {
    let mut cx = x;
    for ch in s.chars() {
        let g = GLYPHS.iter().find(|g| g.0 == ch).map(|g| g.1).unwrap_or(GLYPHS[7].1);
        for (ry, row) in g.iter().enumerate() {
            for (rx, b) in row.bytes().enumerate() {
                if b == b'#' {
                    c.rect(cx + rx as i32 * scale + scale / 2, y + ry as i32 * scale + scale / 2, scale, scale, shade);
                    c.rect(cx + rx as i32 * scale, y + ry as i32 * scale, scale, scale, col);
                }
            }
        }
        cx += 6 * scale;
    }
}

/// A title screen of `w` x `h` source pixels (a `w/2` x `h/2` console screen at 2x): stepped sky, sun, cloud, hills, the title and a strip of
/// ground. Positions are fractions of the canvas, so one drawing serves every console's screen size.
fn title(w: i32, h: i32) -> RgbaImage {
    let mut c = Canvas::new(w as usize, h as usize);
    let sky = [(0x58, 0x88, 0xf8), (0x70, 0xa0, 0xf8), (0x88, 0xb8, 0xf8), (0xa8, 0xd0, 0xf8), (0xc8, 0xe4, 0xf8)];
    let band = (h * 3 / 4 / 5 + 15) / 16 * 16; // bands of whole 8x8 target tiles
    for (i, col) in sky.iter().enumerate() {
        c.rect(0, i as i32 * band, w, band + if i == 4 { h } else { 0 }, *col);
    }
    let scale = ((h as f64 * 0.17 / 7.0).round() as i32 / 2 * 2).max(4); // glyph block size: a title of 17% of the height
    let glyph_w = 6 * scale;
    c.ball(w as f64 * 0.78, h as f64 * 0.18, h as f64 * 0.08, h as f64 * 0.08, GOLD, GOLD_D, GOLD_L);
    let (cx, cy) = (w / 16, h * 46 / 100);
    c.rect(cx, cy + 10, w / 5, 12, WHITE);
    c.rect(cx + 12, cy, w / 9, 12, WHITE);
    c.rect(cx, cy + 18, w / 5, 4, (0xd8, 0xe8, 0xf8));
    c.ball(w as f64 * 0.22, h as f64 * 0.82, w as f64 * 0.28, h as f64 * 0.2, GREEN, GREEN_D, GREEN_L);
    c.ball(w as f64 * 0.75, h as f64 * 0.86, w as f64 * 0.34, h as f64 * 0.18, GREEN, GREEN_D, GREEN_L);
    text(&mut c, "PXL", (w - 3 * glyph_w + scale) / 2, h / 12, scale, WHITE, SUIT_D);
    let s2 = (scale * 7 / 10 / 2 * 2).max(2);
    text(&mut c, "DEMO", (w - 4 * 6 * s2 + s2) / 2, h * 36 / 100, s2, GOLD, GOLD_D);
    let ground = h / 16 * 16 - 32;
    c.rect(0, ground, w, h - ground, DIRT);
    c.rect(0, ground, w, 8, GREEN);
    c.rect(0, ground, w, 3, GREEN_L);
    for k in 0..w / 16 {
        c.rect(k * 16 + 2, ground + 14 + (k % 3) * 4, 4, 2, DIRT_D);
    }
    let mut im = c.im;
    limit_blocks(&mut im, 32, 4);
    im
}

/// Console screens at 1x (width, height): gb, gbc, gg / nes / sms / md / snes, pce / gba.
const SCREENS: [(i32, i32); 6] = [(160, 144), (256, 240), (256, 192), (320, 224), (256, 224), (240, 160)];

/// Keeps the `n` most frequent colours of every `bs` x `bs` block and maps the others to the nearest kept one. The classic limit of
/// "one palette per 16x16 area" (NES attribute blocks, 32x32 here at 2x) is then true for the picture, on every console.
fn limit_blocks(im: &mut RgbaImage, bs: usize, n: usize) {
    let dist = |a: (u8, u8, u8), b: (u8, u8, u8)| -> i32 { (a.0 as i32 - b.0 as i32).pow(2) + (a.1 as i32 - b.1 as i32).pow(2) + (a.2 as i32 - b.2 as i32).pow(2) };
    for by in (0..im.h).step_by(bs) {
        for bx in (0..im.w).step_by(bs) {
            let mut count: Vec<((u8, u8, u8), usize)> = Vec::new();
            for y in by..(by + bs).min(im.h) {
                for x in bx..(bx + bs).min(im.w) {
                    let p = im.px[y * im.w + x];
                    if p.a == 0 {
                        continue;
                    }
                    let c = (p.r, p.g, p.b);
                    match count.iter_mut().find(|e| e.0 == c) {
                        Some(e) => e.1 += 1,
                        None => count.push((c, 1)),
                    }
                }
            }
            if count.len() <= n {
                continue;
            }
            count.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            let keep: Vec<(u8, u8, u8)> = count.iter().take(n).map(|e| e.0).collect();
            for y in by..(by + bs).min(im.h) {
                for x in bx..(bx + bs).min(im.w) {
                    let p = im.px[y * im.w + x];
                    if p.a == 0 {
                        continue;
                    }
                    let c = (p.r, p.g, p.b);
                    if !keep.contains(&c) {
                        let k = *keep.iter().min_by_key(|k| dist(**k, c)).unwrap();
                        im.px[y * im.w + x] = Rgba::opaque(k.0, k.1, k.2);
                    }
                }
            }
        }
    }
}

fn save(root: &Path, rel: &str, im: &RgbaImage) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    png::write_rgba(&p, im).unwrap();
    println!("wrote {}", p.display());
}

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/inputs");
    for i in 0..3 {
        save(&root, &format!("hero/hero_{:02}.png", i + 1), &hero(i));
    }
    for i in 0..2 {
        save(&root, &format!("enemy/crawler_{:02}.png", i + 1), &crawler(i));
    }
    save(&root, "item/gem.png", &gem());
    save(&root, "tiles/ground.png", &ground());
    for (w, h) in SCREENS {
        save(&root, &format!("screens/title_{}x{}.png", w, h), &title(w * 2, h * 2));
    }
}
