//! Owner: compare worker (PLAN.md). `pxl compare` (contact sheets, zoom, `--on`), `pxl diff`, and the sheet
//! renderer reused by `pxl review`.
//!
//! Layout contract (AC-CP3): every cell is `[label row][input | output | diff]`. Labels are drawn only in the
//! label row and in gutters; the three panels are exact nearest-neighbour upscales (`scale` display pixels per
//! reference pixel) of the part's cell window, so panel pixel `(x, y)` shows reference pixel
//! `window_origin + (x / scale, y / scale)` and target cell `(x / (factor*scale), y / (factor*scale))`. The
//! JSON next to the sheets records every panel origin so tools can sample cell centres.

use crate::fsutil::PathSlash;
use crate::cli::*;
use crate::color::{self, Rgb};
use crate::fsutil;
use crate::img::{Rgba, RgbaImage};
use crate::lint::{self, Issue, LintOpts, Severity};
use crate::load;
use crate::metrics::{self, round4, FrameMetrics, TRANSPARENT};
use crate::model::{Part, PartKind, RefFrame};
use crate::output::{Out, PxlError, Result};
use crate::status::{self, StatusEvent};
use crate::template::{self, Template};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

// ------------------------------------------------------------------------------------------------ constants

/// Longest side of any image we write (the size vision models see without downscaling).
pub const MAX_PX: usize = 1568;
pub(crate) const MARGIN: usize = 6;
pub(crate) const GUTTER: usize = 6;
const PANEL_GAP: usize = 2;
pub(crate) const FONT_SCALE: usize = 2;
pub(crate) const LABEL_H: usize = 16;
/// Gutter left of / above a panel that carries cell-coordinate labels (labels never cover art).
pub(crate) const GUT_L: usize = 26;
pub(crate) const GUT_T: usize = 14;
pub(crate) const HEADER_H: usize = 22;
const MIN_PAGE_W: usize = 560;
const MAX_AUTO_SCALE: usize = 16;
/// Pages the automatic sheet scale may use before it falls back to a smaller scale.
const TARGET_PAGES: usize = 3;

pub(crate) const BG: Rgba = Rgba::opaque(0x1c, 0x1d, 0x22);
pub(crate) const PLATE: Rgba = Rgba::opaque(0x27, 0x28, 0x2f);
pub(crate) const TEXT: Rgba = Rgba::opaque(0xdc, 0xde, 0xe4);
pub(crate) const DIM: Rgba = Rgba::opaque(0x8c, 0x8f, 0x9a);
pub(crate) const CHECK_A: Rgba = Rgba::opaque(0x6c, 0x6c, 0x70);
pub(crate) const CHECK_B: Rgba = Rgba::opaque(0x7a, 0x7a, 0x7e);
const D_NONE: Rgba = Rgba::opaque(0x2a, 0x2b, 0x31);
const D_BOTH: Rgba = Rgba::opaque(0xb4, 0xb4, 0xb9);
const D_REF: Rgba = Rgba::opaque(0xe0, 0x3c, 0x3c);
const D_OUT: Rgba = Rgba::opaque(0x3c, 0xc8, 0x64);
pub(crate) const C_GOOD: Rgba = Rgba::opaque(0x5c, 0xd6, 0x7a);
pub(crate) const C_WARN: Rgba = Rgba::opaque(0xe8, 0xc0, 0x50);
pub(crate) const C_BAD: Rgba = Rgba::opaque(0xf0, 0x58, 0x58);
pub(crate) const C_LOST: Rgba = Rgba::opaque(0xd8, 0x6c, 0xf0);

// ------------------------------------------------------------------------------------------------ font (3x5)

const FONT: &[(char, &str)] = &[
    ('0', "### #.# #.# #.# ###"),
    ('1', ".#. ##. .#. .#. ###"),
    ('2', "### ..# ### #.. ###"),
    ('3', "### ..# ### ..# ###"),
    ('4', "#.# #.# ### ..# ..#"),
    ('5', "### #.. ### ..# ###"),
    ('6', "### #.. ### #.# ###"),
    ('7', "### ..# .#. .#. .#."),
    ('8', "### #.# ### #.# ###"),
    ('9', "### #.# ### ..# ###"),
    ('a', ".#. #.# ### #.# #.#"),
    ('b', "##. #.# ##. #.# ##."),
    ('c', ".## #.. #.. #.. .##"),
    ('d', "##. #.# #.# #.# ##."),
    ('e', "### #.. ##. #.. ###"),
    ('f', "### #.. ##. #.. #.."),
    ('g', ".## #.. #.# #.# .##"),
    ('h', "#.# #.# ### #.# #.#"),
    ('i', "### .#. .#. .#. ###"),
    ('j', "..# ..# ..# #.# .#."),
    ('k', "#.# #.# ##. #.# #.#"),
    ('l', "#.. #.. #.. #.. ###"),
    ('m', "#.# ### ### #.# #.#"),
    ('n', "##. #.# #.# #.# #.#"),
    ('o', ".#. #.# #.# #.# .#."),
    ('p', "##. #.# ##. #.. #.."),
    ('q', ".#. #.# #.# ##. .##"),
    ('r', "##. #.# ##. #.# #.#"),
    ('s', ".## #.. .#. ..# ##."),
    ('t', "### .#. .#. .#. .#."),
    ('u', "#.# #.# #.# #.# ###"),
    ('v', "#.# #.# #.# #.# .#."),
    ('w', "#.# #.# ### ### #.#"),
    ('x', "#.# #.# .#. #.# #.#"),
    ('y', "#.# #.# .#. .#. .#."),
    ('z', "### ..# .#. #.. ###"),
    ('_', "... ... ... ... ###"),
    ('.', "... ... ... ... .#."),
    ('%', "#.# ..# .#. #.. #.#"),
    ('-', "... ... ### ... ..."),
    (':', "... .#. ... .#. ..."),
    ('/', "..# ..# .#. #.. #.."),
    ('|', ".#. .#. .#. .#. .#."),
    ('+', "... .#. ### .#. ..."),
    ('(', ".## #.. #.. #.. .##"),
    (')', "##. ..# ..# ..# ##."),
    (',', "... ... ... .#. #.."),
    ('=', "... ### ... ### ..."),
    ('>', "#.. .#. ..# .#. #.."),
    ('<', "..# .#. #.. .#. ..#"),
    ('#', "#.# ### #.# ### #.#"),
    ('x', "#.# #.# .#. #.# #.#"),
    (' ', "... ... ... ... ..."),
];

fn glyph_rows(c: char) -> Option<&'static str> {
    let c = c.to_ascii_lowercase();
    FONT.iter().find(|(k, _)| *k == c).map(|(_, g)| *g)
}

pub(crate) fn text_w(s: &str, scale: usize) -> usize {
    let n = s.chars().count();
    if n == 0 {
        0
    } else {
        n * 4 * scale - scale
    }
}

/// Draw text with the in-house 3x5 font; returns the x after the last glyph. Unknown characters draw a box.
pub(crate) fn draw_text(cv: &mut RgbaImage, x: usize, y: usize, s: &str, scale: usize, col: Rgba) -> usize {
    let mut cx = x;
    for ch in s.chars() {
        let g = glyph_rows(ch).unwrap_or("### #.# #.# #.# ###");
        for (r, row) in g.split(' ').enumerate() {
            for (c, b) in row.bytes().enumerate() {
                if b == b'#' {
                    fill(cv, cx + c * scale, y + r * scale, scale, scale, col);
                }
            }
        }
        cx += 4 * scale;
    }
    cx.saturating_sub(scale)
}

pub(crate) fn fill(cv: &mut RgbaImage, x: usize, y: usize, w: usize, h: usize, col: Rgba) {
    for yy in y..(y + h).min(cv.h) {
        let row = yy * cv.w;
        for xx in x..(x + w).min(cv.w) {
            cv.px[row + xx] = col;
        }
    }
}

pub(crate) fn paste(cv: &mut RgbaImage, src: &RgbaImage, x: usize, y: usize) {
    for yy in 0..src.h {
        if y + yy >= cv.h {
            break;
        }
        let w = src.w.min(cv.w.saturating_sub(x));
        let d = (y + yy) * cv.w + x;
        cv.px[d..d + w].copy_from_slice(&src.px[yy * src.w..yy * src.w + w]);
    }
}

pub(crate) fn blend(bg: Rgba, c: Rgba) -> Rgba {
    match c.a {
        255 => c,
        0 => bg,
        a => {
            let m = |f: u8, b: u8| ((f as u32 * a as u32 + b as u32 * (255 - a as u32) + 127) / 255) as u8;
            Rgba::opaque(m(c.r, bg.r), m(c.g, bg.g), m(c.b, bg.b))
        }
    }
}

pub(crate) fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max.saturating_sub(1)).collect::<String>() + "."
    }
}

// ------------------------------------------------------------------------------------------------ fast PNG

fn crc_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    for (n, e) in t.iter_mut().enumerate() {
        let mut c = n as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *e = c;
    }
    t
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8], table: &[u32; 256]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut c = 0xFFFF_FFFFu32;
    for &b in &out[start..] {
        c = table[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    out.extend_from_slice(&(c ^ 0xFFFF_FFFF).to_be_bytes());
}

/// Deterministic opaque RGB PNG (filter "Up", zlib level 3): several times faster than `png::encode_rgba`,
/// which matters for the sub-second sheet budget (AC-CP2).
pub fn encode_sheet_png(img: &RgbaImage) -> Vec<u8> {
    let stride = img.w * 3;
    let mut raw = vec![0u8; (stride + 1) * img.h];
    for y in 0..img.h {
        let row = &mut raw[y * (stride + 1)..(y + 1) * (stride + 1)];
        row[0] = 2;
        for x in 0..img.w {
            let p = img.px[y * img.w + x];
            row[1 + x * 3..4 + x * 3].copy_from_slice(&[p.r, p.g, p.b]);
        }
    }
    // Up filter, back to front so the previous row is still unfiltered
    for y in (1..img.h).rev() {
        let (a, b) = raw.split_at_mut(y * (stride + 1));
        let prev = &a[(y - 1) * (stride + 1) + 1..];
        for (i, v) in b[1..=stride].iter_mut().enumerate() {
            *v = v.wrapping_sub(prev[i]);
        }
    }
    let table = crc_table();
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&(img.w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(img.h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr, &table);
    let z = miniz_oxide::deflate::compress_to_vec_zlib(&raw, 3);
    chunk(&mut out, b"IDAT", &z, &table);
    chunk(&mut out, b"IEND", &[], &table);
    out
}

// ------------------------------------------------------------------------------------------------ run options

#[derive(Clone, Debug, Default)]
pub struct RunOpts {
    /// Frames to show (empty = all).
    pub names: Vec<String>,
    /// Review: show the N worst frames by fresh score (after `names` is empty).
    pub worst: Option<usize>,
    /// `Some(0)` = automatic zoom, `Some(n)` = n display pixels per reference pixel.
    pub zoom: Option<usize>,
    pub on: Option<String>,
    pub out_dir: Option<PathBuf>,
    /// File prefix kind: `compare` or `review`.
    pub kind: String,
    /// Write metrics into `<part>.status.json`.
    pub record: bool,
    /// Appended to every page title (review packets: the part's content hash).
    pub stamp: String,
    /// Multi-set parts: render in this set's colours.
    pub set: Option<String>,
}

struct Row {
    idx: usize,
    name: String,
    rf: RefFrame,
    m: FrameMetrics,
    issues: Vec<Issue>,
}

pub struct PartResult {
    pub name: String,
    pub pages: Vec<PathBuf>,
    pub json_path: Option<PathBuf>,
    pub json: Value,
    /// Frame, metrics for the selected frames, worst first.
    pub ranking: Vec<(String, FrameMetrics)>,
    pub warnings: Vec<String>,
}

/// Fallback colour when the part has no (valid) template: a deterministic spread by key.
fn fallback_color(k: u8) -> Rgb {
    let h = (k as u32).wrapping_mul(2654435761);
    Rgb { r: 64 + (h >> 24) as u8 % 160, g: 64 + (h >> 16) as u8 % 160, b: 64 + (h >> 8) as u8 % 160 }
}

pub(crate) fn key_color(part: &Part, tpl: Option<&Template>, k: u8) -> Rgb {
    tpl.and_then(|t| lint::effective_color(part, t, k)).unwrap_or_else(|| fallback_color(k))
}


// ------------------------------------------------------------------------------------------------ panels

pub(crate) fn checker(w: usize, h: usize, size: usize) -> RgbaImage {
    let mut im = RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            im.px[y * w + x] = if ((x / size) + (y / size)) % 2 == 0 { CHECK_A } else { CHECK_B };
        }
    }
    im
}

/// Dim cell-boundary lines on an upscaled panel; `cell` display pixels per target cell.
pub(crate) fn grid_lines(im: &mut RgbaImage, cell: usize) {
    if cell < 6 {
        return;
    }
    for y in 0..im.h {
        for x in 0..im.w {
            if x % cell == 0 || y % cell == 0 {
                let p = &mut im.px[y * im.w + x];
                *p = Rgba::opaque((p.r as u32 * 62 / 100) as u8, (p.g as u32 * 62 / 100) as u8, (p.b as u32 * 62 / 100) as u8);
            }
        }
    }
}

/// Rectangular ring of `t` px (clipped to the image).
pub(crate) fn ring(im: &mut RgbaImage, x: i32, y: i32, w: usize, h: usize, t: usize, c: Rgba) {
    let mut px = |xx: i32, yy: i32| {
        if xx >= 0 && yy >= 0 && (xx as usize) < im.w && (yy as usize) < im.h {
            im.px[yy as usize * im.w + xx as usize] = c;
        }
    };
    for k in 0..t as i32 {
        for i in 0..w as i32 {
            px(x + i, y + k);
            px(x + i, y + h as i32 - 1 - k);
        }
        for j in 0..h as i32 {
            px(x + k, y + j);
            px(x + w as i32 - 1 - k, y + j);
        }
    }
}

/// A copy of `im` inside a gutter: `gl` columns on the left and `gt` rows on top, filled with the plate colour.
/// The art starts at `(gl, gt)`; no art pixel is touched.
pub(crate) fn with_gutter(im: &RgbaImage, gl: usize, gt: usize) -> RgbaImage {
    let mut o = RgbaImage::filled(im.w + gl, im.h + gt, PLATE);
    paste(&mut o, im, gl, gt);
    o
}

/// Cell-coordinate labels for a panel, drawn in a gutter outside the art: x labels in `GUT_T` rows above it, y labels
/// in `GUT_L` columns left of it, for every 4th cell (every 8th, 16th... when the cells are too small to fit the
/// digits). `x0`,`y0` = grid coordinate of the panel's first cell. Returns the image with the gutter; the art
/// starts at `(GUT_L, GUT_T)` and is unchanged (cell centres stay exact).
pub(crate) fn coord_gutter(im: &RgbaImage, x0: usize, y0: usize, cell: usize) -> RgbaImage {
    let cell = cell.max(1);
    let mut o = with_gutter(im, GUT_L, GUT_T);
    let mut step = 4;
    while cell * step < text_w("00", FONT_SCALE) + 4 {
        step *= 2;
    }
    for cx in 0..(im.w / cell) {
        let x = x0 + cx;
        if x % step == 0 {
            draw_text(&mut o, GUT_L + cx * cell + 1, 2, &x.to_string(), FONT_SCALE, TEXT);
        }
    }
    for cy in 0..(im.h / cell) {
        let y = y0 + cy;
        if y % step == 0 {
            let t = y.to_string();
            let w = text_w(&t, FONT_SCALE);
            draw_text(&mut o, GUT_L.saturating_sub(w + 3), GUT_T + cy * cell + 1, &t, FONT_SCALE, DIM);
        }
    }
    o
}

struct Panels {
    input: RgbaImage,
    output: RgbaImage,
    diff: RgbaImage,
}

/// Cell window `(x0, y0, w, h)` shown in every panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Crop {
    x0: usize,
    y0: usize,
    w: usize,
    h: usize,
}

/// Bounding window (1 cell padding) of everything either side draws in `rows`; the whole grid when empty.
fn crop_for(part: &Part, rows: &[&Row]) -> Crop {
    let (gw, gh) = part.size;
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
    for r in rows {
        for y in 0..gh {
            for x in 0..gw {
                if r.m.diff[y * gw + x] != 0 {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x + 1);
                    y1 = y1.max(y + 1);
                }
            }
        }
    }
    if x1 == 0 {
        return Crop { x0: 0, y0: 0, w: gw, h: gh };
    }
    let (x0, y0) = (x0.saturating_sub(1), y0.saturating_sub(1));
    let (x1, y1) = ((x1 + 1).min(gw), (y1 + 1).min(gh));
    Crop { x0, y0, w: x1 - x0, h: y1 - y0 }
}

/// Input window (native reference pixels), output cells and diff cells for one frame at `scale`.
fn frame_panels(part: &Part, tpl: Option<&Template>, row: &Row, crop: Crop, scale: usize, grid: bool) -> Panels {
    let g = metrics::geometry(part, &row.rf);
    let f = g.f as usize;
    let (pw, ph) = (crop.w * f, crop.h * f);
    let (ox, oy) = (g.org.0 + (crop.x0 * f) as i32, g.org.1 + (crop.y0 * f) as i32);
    let mut input = checker(pw, ph, 4);
    for y in 0..ph {
        for x in 0..pw {
            let p = row.rf.img.get(ox + x as i32, oy + y as i32);
            if p.a > 0 {
                let d = &mut input.px[y * pw + x];
                *d = blend(*d, p);
            }
        }
    }
    let mut output = checker(pw, ph, 4);
    let frame = &part.frames[row.idx];
    let mut diff = RgbaImage::filled(pw, ph, D_NONE);
    for cy in 0..crop.h {
        for cx in 0..crop.w {
            let (gx, gy) = (cx + crop.x0, cy + crop.y0);
            if gx < frame.grid.w && gy < frame.grid.h {
                let k = frame.grid.cells[gy * frame.grid.w + gx];
                if k != TRANSPARENT {
                    let c = key_color(part, tpl, k);
                    fill(&mut output, cx * f, cy * f, f, f, Rgba::opaque(c.r, c.g, c.b));
                }
            }
            let d = match row.m.diff[gy * g.gw + gx] {
                1 => D_BOTH,
                2 => D_REF,
                3 => D_OUT,
                _ => D_NONE,
            };
            fill(&mut diff, cx * f, cy * f, f, f, d);
        }
    }
    let (mut input, mut output, mut diff) = (input.upscale(scale), output.upscale(scale), diff.upscale(scale));
    for l in row.m.lost.iter().filter(|l| l.kind == "eye") {
        let cell = f * scale;
        let (x0, y0) = ((l.x - 1 - crop.x0 as i32) * cell as i32, (l.y - 1 - crop.y0 as i32) * cell as i32);
        ring(&mut output, x0, y0, (l.w + 2) as usize * cell, (l.h + 2) as usize * cell, (cell / 4).clamp(1, 3), C_LOST);
    }
    if grid {
        for p in [&mut input, &mut output, &mut diff] {
            grid_lines(p, f * scale);
        }
    }
    let output = coord_gutter(&output, crop.x0, crop.y0, f * scale);
    Panels { input: with_gutter(&input, 0, GUT_T), output, diff: with_gutter(&diff, 0, GUT_T) }
}

fn load_background(spec: &str) -> Result<RgbaImage> {
    if spec.starts_with('#') {
        let c = color::parse_hex(spec)?;
        return Ok(RgbaImage::filled(1, 1, Rgba::opaque(c.r, c.g, c.b)));
    }
    let p = Path::new(spec);
    if p.is_file() {
        return crate::png::read_rgba(p);
    }
    Err(PxlError::usage(format!("--on '{}' is neither #rrggbb nor an image file", spec)))
}

/// Output frame over the background at 1x (target pixels).
fn frame_on(part: &Part, tpl: Option<&Template>, row: &Row, bg: &RgbaImage, crop: Crop) -> RgbaImage {
    let frame = &part.frames[row.idx];
    let (w, h) = (crop.w, crop.h);
    let mut im = RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let (gx, gy) = (x + crop.x0, y + crop.y0);
            let b = bg.px[(gy % bg.h) * bg.w + (gx % bg.w)];
            let b = Rgba::opaque(b.r, b.g, b.b);
            let k = if gx < frame.grid.w && gy < frame.grid.h { frame.grid.cells[gy * frame.grid.w + gx] } else { TRANSPARENT };
            im.px[y * w + x] = if k == TRANSPARENT {
                b
            } else {
                let c = key_color(part, tpl, k);
                Rgba::opaque(c.r, c.g, c.b)
            };
        }
    }
    im
}

// ------------------------------------------------------------------------------------------------ sheets

struct CellImg {
    name: String,
    iou: f64,
    errors: u32,
    warnings: u32,
    lost: usize,
    panels: Vec<RgbaImage>,
}

struct Placement {
    page: usize,
    x: usize,
    y: usize,
    panel_x: Vec<usize>,
}

struct SheetPlan {
    cols: usize,
    page_sizes: Vec<(usize, usize)>,
    place: Vec<Placement>,
    cell_w: usize,
    cell_h: usize,
    slot_w: Vec<usize>,
    pages: usize,
}

/// (cols, rows per page) that fit in MAX_PX for cells of `cw` x `ch`; None when a cell does not fit at all.
fn capacity(cw: usize, ch: usize) -> Option<(usize, usize)> {
    let aw = MAX_PX.checked_sub(2 * MARGIN)?;
    let ah = MAX_PX.checked_sub(HEADER_H + 2 * MARGIN)?;
    if cw > aw || ch > ah {
        return None;
    }
    Some(((aw + GUTTER) / (cw + GUTTER), (ah + GUTTER) / (ch + GUTTER)))
}

fn plan_sheet(n: usize, slot_w: Vec<usize>, panel_h: usize) -> Result<SheetPlan> {
    let cell_w = slot_w.iter().sum::<usize>() + PANEL_GAP * (slot_w.len() - 1);
    let cell_h = LABEL_H + panel_h;
    let (cols, rows) = capacity(cell_w, cell_h).ok_or_else(|| {
        PxlError::usage(format!("one cell is {}x{} px, larger than the {} px page limit", cell_w, cell_h, MAX_PX))
            .with_hint("use --zoom for a single large frame, or select fewer/smaller panels")
    })?;
    let cap = cols * rows;
    let pages = (n + cap - 1) / cap.max(1);
    let per = if pages > 0 { (n + pages - 1) / pages } else { 0 };
    let mut place = Vec::new();
    let mut page_sizes = Vec::new();
    let mut offs = Vec::new();
    let mut ox = 0;
    for w in &slot_w {
        offs.push(ox);
        ox += w + PANEL_GAP;
    }
    let mut i = 0;
    for p in 0..pages {
        let cnt = per.min(n - i);
        let cu = cols.min(cnt).max(1);
        let ru = (cnt + cu - 1) / cu;
        let w = (2 * MARGIN + cu * (cell_w + GUTTER) - GUTTER).max(MIN_PAGE_W);
        let h = HEADER_H + MARGIN + ru * (cell_h + GUTTER) - GUTTER + MARGIN;
        page_sizes.push((w, h));
        for k in 0..cnt {
            place.push(Placement {
                page: p,
                x: MARGIN + (k % cu) * (cell_w + GUTTER),
                y: HEADER_H + MARGIN + (k / cu) * (cell_h + GUTTER),
                panel_x: offs.clone(),
            });
        }
        i += cnt;
    }
    Ok(SheetPlan { cols, page_sizes, place, cell_w, cell_h, slot_w, pages })
}

fn draw_label(cv: &mut RgbaImage, x: usize, y: usize, cell_w: usize, c: &CellImg) {
    fill(cv, x, y, cell_w, LABEL_H, PLATE);
    let ty = y + (LABEL_H - 5 * FONT_SCALE) / 2;
    // right group, laid out right to left
    let mut parts: Vec<(Rgba, String)> = Vec::new();
    let pct = (c.iou * 100.0).round() as i64;
    let iou_col = if c.iou >= 0.95 { C_GOOD } else if c.iou >= 0.85 { C_WARN } else { C_BAD };
    parts.push((iou_col, format!("{}%", pct)));
    let marks = [(C_BAD, c.errors as usize), (C_WARN, c.warnings as usize), (C_LOST, c.lost)];
    let mut rx = x + cell_w - 3;
    let mut right_w = 0;
    for (col, n) in marks.iter().rev() {
        if *n == 0 {
            continue;
        }
        let t = n.to_string();
        let tw = text_w(&t, FONT_SCALE);
        if rx < x + tw + 24 {
            break;
        }
        rx -= tw;
        draw_text(cv, rx, ty, &t, FONT_SCALE, TEXT);
        rx -= 3 + 6;
        fill(cv, rx, ty + 2, 6, 6, *col);
        fill(cv, rx, ty + 2, 1, 1, PLATE);
        fill(cv, rx + 5, ty + 2, 1, 1, PLATE);
        fill(cv, rx, ty + 7, 1, 1, PLATE);
        fill(cv, rx + 5, ty + 7, 1, 1, PLATE);
        rx -= 5;
        right_w = x + cell_w - 3 - rx;
    }
    let iw = text_w(&parts[0].1, FONT_SCALE);
    rx -= iw;
    draw_text(cv, rx, ty, &parts[0].1, FONT_SCALE, iou_col);
    right_w = right_w.max(x + cell_w - 3 - rx);
    let avail = cell_w.saturating_sub(right_w + 10);
    let max_chars = (avail + FONT_SCALE) / (4 * FONT_SCALE);
    draw_text(cv, x + 3, ty, &truncate_chars(&c.name, max_chars.max(1)), FONT_SCALE, TEXT);
}

fn stamp_of(o: &RunOpts) -> String {
    if o.stamp.is_empty() { String::new() } else { format!(" [{}]", o.stamp) }
}

fn draw_header(cv: &mut RgbaImage, title: &str, legend: bool) {
    let ty = (HEADER_H - 5 * FONT_SCALE) / 2;
    let end = draw_text(cv, MARGIN, ty, title, FONT_SCALE, TEXT);
    if !legend {
        return;
    }
    let items: [(Rgba, &str); 5] = [(D_REF, "diff: input only"), (D_OUT, "output only"), (C_BAD, "limit errors"), (C_WARN, "warnings"), (C_LOST, "lost features")];
    let total: usize = items.iter().map(|(_, t)| 6 + 3 + text_w(t, FONT_SCALE) + 10).sum();
    if cv.w < end + 30 + total + MARGIN {
        return;
    }
    let mut x = cv.w - MARGIN - total + 10;
    for (c, t) in items {
        fill(cv, x, ty + 2, 6, 6, c);
        x += 9;
        x = draw_text(cv, x, ty, t, FONT_SCALE, DIM) + 10;
    }
}

fn render_pages(plan: &SheetPlan, cells: &[CellImg], title: &str, legend: bool) -> Vec<RgbaImage> {
    let idx: Vec<usize> = (0..plan.pages).collect();
    fsutil::par_map(&idx, |&p| {
        let (w, h) = plan.page_sizes[p];
        let mut cv = RgbaImage::filled(w, h, BG);
        let t = format!("{}  page {}/{}", title, p + 1, plan.pages);
        draw_header(&mut cv, &t, legend);
        for (i, pl) in plan.place.iter().enumerate().filter(|(_, pl)| pl.page == p) {
            let c = &cells[i];
            fill(&mut cv, pl.x, pl.y, plan.cell_w, plan.cell_h, PLATE);
            draw_label(&mut cv, pl.x, pl.y, plan.cell_w, c);
            for (k, im) in c.panels.iter().enumerate() {
                paste(&mut cv, im, pl.x + pl.panel_x[k], pl.y + LABEL_H);
            }
        }
        cv
    })
}

// ------------------------------------------------------------------------------------------------ core

fn issue_counts(issues: &[Issue]) -> (u32, u32) {
    let e = issues.iter().filter(|i| i.severity == Severity::Error).count() as u32;
    let w = issues.iter().filter(|i| i.severity == Severity::Warning).count() as u32;
    (e, w)
}

fn metrics_json(m: &FrameMetrics) -> Value {
    json!({
        "iou": round4(m.iou),
        "bbox": {"left": m.bbox[0], "top": m.bbox[1], "right": m.bbox[2], "bottom": m.bbox[3]},
        "bbox_max": m.bbox_max,
        "oklab": m.oklab.map(round4),
        "lost": m.lost.iter().map(|l| json!({"kind": l.kind, "x": l.x, "y": l.y, "w": l.w, "h": l.h, "size": l.size, "dark": l.dark, "contrast": round4(l.contrast)})).collect::<Vec<_>>(),
        "colour_share": m.shares.iter().map(|s| json!({"key": (s.key as char).to_string(), "ref_cells": round4(s.ref_cells), "out_cells": s.out_cells, "deficit": round4(s.deficit())})).collect::<Vec<_>>(),
        "eyes_found": m.eyes_found,
        "eyes_kept": m.eyes_kept,
        "lint_errors": m.errors,
        "lint_warnings": m.warnings,
        "score": round4(m.score),
    })
}

fn analyze(path: &Path, o: &RunOpts) -> Result<(Part, Option<Template>, Vec<Row>, Vec<String>)> {
    let part = crate::pxl::load(path)?;
    if part.kind != PartKind::Sprite {
        return Err(PxlError::not_implemented(&format!("compare for {} parts (M4)", part.kind.name())));
    }
    let mut warnings = Vec::new();
    let tpl = match template::load_for(&part) {
        Ok(t) => Some(t),
        Err(e) => {
            warnings.push(format!("{}: no template ({}); output colours are placeholders, colour error skipped", part.name, e.msg));
            None
        }
    };
    let dir = part.dir();
    // selection
    let sel_idx: Vec<usize> = if o.names.is_empty() {
        (0..part.frames.len()).collect()
    } else {
        let mut v = Vec::new();
        for n in &o.names {
            v.push(part.frame_index(n).ok_or_else(|| unknown_frame(&part, n))?);
        }
        v
    };
    // references for the whole part (lint rules look at neighbours), in parallel
    let all: Vec<usize> = (0..part.frames.len()).collect();
    let loaded: Vec<Result<RefFrame>> = fsutil::par_map(&all, |&i| load::reference(&part, &dir, &part.frames[i]));
    let mut refs: Vec<Option<RefFrame>> = Vec::new();
    let mut first_err: Option<(usize, PxlError)> = None;
    for (i, r) in loaded.into_iter().enumerate() {
        match r {
            Ok(rf) => refs.push(Some(rf)),
            Err(e) => {
                refs.push(None);
                if sel_idx.contains(&i) {
                    if first_err.is_none() {
                        first_err = Some((i, e));
                    }
                } else {
                    // not selected, so compare goes on, but the lint rules that look at neighbours lose this frame: say so
                    warnings.push(load::reference_missing_text(&part.name, &part.frames[i].name, &e.msg, &["shimmer and the other reference rules for its neighbours"]));
                }
            }
        }
    }
    if let Some((_, e)) = first_err {
        return Err(e);
    }
    // lint (counts per frame); failures never block the sheets
    let mut issues: Vec<Issue> = Vec::new();
    let sel_names: Vec<String> = sel_idx.iter().map(|&i| part.frames[i].name.clone()).collect();
    match crate::profile::load_for(&part, None) {
        Ok(prof) => {
            let opts = LintOpts { rules: Vec::new(), frames: if o.worst.is_some() { Vec::new() } else { sel_names }, refs: true, advice: true };
            issues = match &tpl {
                Some(t) => lint::lint_part_refs(&part, t, &prof, &opts, &refs),
                None => lint::lint_part_auto(&part, &prof, &LintOpts { refs: false, ..opts }),
            };
        }
        Err(e) => warnings.push(format!("{}: lint skipped ({})", part.name, e.msg)),
    }
    let by_frame = |name: &str| -> Vec<Issue> {
        issues.iter().filter(|i| i.frame.as_deref() == Some(name) && i.rule != "lost_feature" && i.rule != "lost_eye" && i.rule != "colour_share").cloned().collect()
    };
    let mut rows: Vec<Row> = Vec::new();
    let per: Vec<(usize, Vec<Issue>)> = sel_idx.iter().map(|&i| (i, by_frame(&part.frames[i].name))).collect();
    let computed = fsutil::par_map(&per, |(i, iss)| {
        let rf = refs[*i].clone().unwrap();
        let (e, w) = issue_counts(iss);
        let m = metrics::compute(&part, tpl.as_ref(), &part.frames[*i], &rf, e, w);
        (rf, m)
    });
    for ((i, iss), (rf, m)) in per.into_iter().zip(computed) {
        rows.push(Row { idx: i, name: part.frames[i].name.clone(), rf, m, issues: iss });
    }
    Ok((part, tpl, rows, warnings))
}

fn out_dir_for(path: &Path, o: &RunOpts) -> PathBuf {
    match &o.out_dir {
        Some(d) => d.clone(),
        None => path.parent().map(|p| p.to_path_buf()).unwrap_or_default().join("compare"),
    }
}

fn ranking_of(rows: &[Row]) -> Vec<(String, FrameMetrics)> {
    let mut v: Vec<(String, FrameMetrics)> = rows.iter().map(|r| (r.name.clone(), r.m.clone())).collect();
    v.sort_by(|a, b| b.1.score.partial_cmp(&a.1.score).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(&b.0)));
    v
}

fn write_file(p: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(d) = p.parent() {
        if !d.as_os_str().is_empty() {
            std::fs::create_dir_all(d)?;
        }
    }
    fsutil::atomic_write(p, bytes)
}

fn auto_zoom_scale(pw: usize, ph: usize) -> usize {
    let mut best = 1;
    for s in 1..=64 {
        let w = 2 * MARGIN + 2 * pw * s + GUT_L + GUTTER;
        let h = HEADER_H + 2 * MARGIN + 2 * (LABEL_H + GUT_T + ph * s) + GUTTER;
        if w <= MAX_PX && h <= MAX_PX {
            best = s;
        }
    }
    best
}

fn info_lines(part: &Part, tpl: Option<&Template>, row: &Row, chars: usize, max_lines: usize) -> Vec<(Rgba, String)> {
    let m = &row.m;
    let mut v: Vec<(Rgba, String)> = Vec::new();
    let col = if m.iou >= 0.95 { C_GOOD } else if m.iou >= 0.85 { C_WARN } else { C_BAD };
    v.push((col, format!("iou {:.1}%  score {:.1}", m.iou * 100.0, m.score)));
    v.push((TEXT, format!("bbox dl {} dt {} dr {} db {}", m.bbox[0], m.bbox[1], m.bbox[2], m.bbox[3])));
    v.push((TEXT, match m.oklab {
        Some(e) => format!("oklab err {:.3}", e),
        None => "oklab err n/a".into(),
    }));
    v.push((TEXT, format!("cells ref {} out {}", m.ref_cells, m.out_cells)));
    v.push((DIM, format!("factor {} cell {}x{}", metrics::geometry(part, &row.rf).f, part.size.0, part.size.1)));
    let _ = tpl;
    v.push((C_LOST, format!("lost features {}", m.lost.len())));
    for l in &m.lost {
        if l.kind == "eye" {
            v.push((TEXT, format!(" eye at {},{}: in the reference, no white+pupil in the art", l.x, l.y)));
        } else {
            v.push((DIM, format!(" {},{} {} {}px c{:.2}", l.x, l.y, if l.dark { "dark" } else { "light" }, l.size, l.contrast)));
        }
    }
    for sh in m.shares.iter().filter(|s| s.deficit() >= 1.0) {
        v.push((C_WARN, format!(" share {} {} of ~{:.0} cells", sh.key as char, sh.out_cells, sh.ref_cells)));
    }
    if m.eyes_found > 0 {
        v.push((TEXT, format!("eyes found {} kept {}", m.eyes_found, m.eyes_kept)));
    }
    let (e, w) = issue_counts(&row.issues);
    v.push((if e > 0 { C_BAD } else { C_WARN }, format!("limits: {} errors {} warnings", e, w)));
    let notes = row.issues.iter().filter(|i| i.severity == Severity::Advice).count();
    if notes > 0 {
        v.push((DIM, format!("notes (information): {}", notes)));
    }
    for i in &row.issues {
        let at = match (i.x, i.y) {
            (Some(x), Some(y)) => format!(" {},{}", x, y),
            _ => String::new(),
        };
        v.push((DIM, format!(" {}{} {}", i.rule, at, i.message)));
    }
    let mut out: Vec<(Rgba, String)> = v.into_iter().map(|(c, s)| (c, truncate_chars(&s, chars))).collect();
    if out.len() > max_lines && max_lines > 1 {
        out.truncate(max_lines - 1);
        out.push((DIM, "...".into()));
    }
    out
}

/// Compare one part: metrics, sheets (or zoom/on pages), JSON, status.
pub fn compare_part(path: &Path, o: &RunOpts) -> Result<PartResult> {
    let (part, tpl, mut rows, mut warnings) = analyze(path, o)?;
    // `--set` changes the rendering colours only; lint and the metrics above come from the part as it is
    let (part, tpl) = match (&o.set, tpl) {
        (Some(sn), Some(t)) if part.palette_set.is_some() => {
            let (p2, t2) = crate::cells::recolour_part(part, t, sn)?;
            (p2, Some(t2))
        }
        (_, t) => (part, t),
    };
    let ranking_all = ranking_of(&rows);
    if let Some(n) = o.worst {
        let keep: Vec<String> = ranking_all.iter().take(n.max(1)).map(|r| r.0.clone()).collect();
        rows.retain(|r| keep.contains(&r.name));
        rows.sort_by(|a, b| {
            let ia = keep.iter().position(|k| *k == a.name);
            let ib = keep.iter().position(|k| *k == b.name);
            ia.cmp(&ib)
        });
    }
    let dir = out_dir_for(path, o);
    let name = part.name.clone();
    let kind = if o.kind.is_empty() { "compare" } else { o.kind.as_str() };
    let f = rows.first().map(|r| metrics::geometry(&part, &r.rf).f as usize).unwrap_or(1);
    let all_rows: Vec<&Row> = rows.iter().collect();
    let crop = crop_for(&part, &all_rows);
    let (pw, ph) = (crop.w * f, crop.h * f);
    let mut pages_out: Vec<PathBuf> = Vec::new();
    let mut page_json: Vec<Value> = Vec::new();
    let mut frame_json: BTreeMap<String, Value> = BTreeMap::new();
    let mut layout = json!({});

    if rows.is_empty() {
        return Err(PxlError::usage("no frames to compare"));
    }

    if let Some(z) = o.zoom {
        // ---------------------------------------------------------------- zoom: one frame per page
        let zcrop = crop_for(&part, &all_rows);
        let (pw, ph) = (zcrop.w * f, zcrop.h * f);
        let fit = auto_zoom_scale(pw, ph);
        let s = if z == 0 {
            fit
        } else if z > fit {
            warnings.push(format!("--zoom {} does not fit in {} px; using {}", z, MAX_PX, fit));
            fit
        } else {
            z
        };
        let rows_ref = &rows;
        let results = fsutil::par_map(&(0..rows.len()).collect::<Vec<_>>(), |&k| {
            let row = &rows_ref[k];
            let p = frame_panels(&part, tpl.as_ref(), row, zcrop, s, true);
            let (w, h) = (2 * MARGIN + 2 * pw * s + GUT_L + GUTTER, HEADER_H + 2 * MARGIN + 2 * (LABEL_H + GUT_T + ph * s) + GUTTER);
            let mut cv = RgbaImage::filled(w, h, BG);
            let title = format!("{}{}  {}  zoom {}x  input | output | diff | metrics", name, stamp_of(o), row.name, s);
            draw_header(&mut cv, &title, false);
            let pos = [(0, 0, "input"), (1, 0, "output"), (0, 1, "diff"), (1, 1, "metrics")];
            let mut origins = Vec::new();
            for (cx, cy, label) in pos {
                let x = MARGIN + cx * (pw * s + GUTTER);
                let y = HEADER_H + MARGIN + cy * (LABEL_H + GUT_T + ph * s + GUTTER);
                let gl = if cx == 1 { GUT_L } else { 0 };
                fill(&mut cv, x, y, pw * s + gl, LABEL_H, PLATE);
                draw_text(&mut cv, x + 3, y + (LABEL_H - 5 * FONT_SCALE) / 2, label, FONT_SCALE, TEXT);
                let img = match label {
                    "input" => Some(&p.input),
                    "output" => Some(&p.output),
                    "diff" => Some(&p.diff),
                    _ => None,
                };
                match img {
                    Some(im) => {
                        paste(&mut cv, im, x, y + LABEL_H);
                        origins.push((label, x + gl, y + LABEL_H + GUT_T));
                    }
                    None => {
                        fill(&mut cv, x, y + LABEL_H, pw * s + gl, GUT_T + ph * s, PLATE);
                        let chars = (pw * s).saturating_sub(8) / (4 * FONT_SCALE);
                        let lines = (ph * s).saturating_sub(8) / (6 * FONT_SCALE + 2);
                        for (i, (c, t)) in info_lines(&part, tpl.as_ref(), row, chars.max(8), lines.max(2)).iter().enumerate() {
                            draw_text(&mut cv, x + 4, y + LABEL_H + GUT_T + 4 + i * (6 * FONT_SCALE + 2), t, FONT_SCALE, *c);
                        }
                    }
                }
            }
            (encode_sheet_png(&cv), (w, h), origins)
        });
        for (row, (bytes, (w, h), origins)) in rows.iter().zip(results) {
            let file = dir.join(format!("{}_zoom_{}.png", name, row.name));
            write_file(&file, &bytes)?;
            let mut pj = json!({"file": file.slash(), "width": w, "height": h, "frames": [row.name]});
            let mut panels = serde_json::Map::new();
            for (l, x, y) in origins {
                panels.insert(l.to_string(), json!([x, y]));
            }
            pj["panels"] = Value::Object(panels);
            page_json.push(pj);
            pages_out.push(file);
        }
        layout = json!({"mode": "zoom", "scale": s, "crop": [zcrop.x0, zcrop.y0, zcrop.w, zcrop.h], "panel_px": [pw * s, ph * s], "coord_gutter": [GUT_L, GUT_T], "factor": f, "max_px": MAX_PX});
    } else if let Some(spec) = &o.on {
        // ---------------------------------------------------------------- over a background
        let bg = load_background(spec)?;
        let plan_for = |s: usize| plan_sheet(rows.len(), vec![crop.w.max(56), GUT_L + pw * s], (GUT_T + ph * s).max(crop.h));
        let s = pick_scale(2, |s| plan_for(s).ok().map(|p| p.pages));
        let plan = plan_for(s)?;
        let rows_ref = &rows;
        let cells: Vec<CellImg> = fsutil::par_map(&(0..rows.len()).collect::<Vec<_>>(), |&k| {
            let row = &rows_ref[k];
            let one = frame_on(&part, tpl.as_ref(), row, &bg, crop);
            let big = coord_gutter(&one.upscale(f * s), crop.x0, crop.y0, f * s);
            CellImg { name: row.name.clone(), iou: row.m.iou, errors: row.m.errors, warnings: row.m.warnings, lost: row.m.lost.len(), panels: vec![one, big] }
        });
        let bg_name = if spec.starts_with('#') { spec.clone() } else { Path::new(spec).file_name().map_or(spec.clone(), |n| n.to_string_lossy().into_owned()) };
        let title = format!("{}{}  on {}  {} frames  1x | x{}", name, stamp_of(o), bg_name, rows.len(), f * s);
        let imgs = render_pages(&plan, &cells, &title, false);
        write_pages(&imgs, &dir, &format!("{}_on", name), &mut pages_out, &mut page_json, &plan, &rows)?;
        layout = json!({"mode": "on", "scale": s, "crop": [crop.x0, crop.y0, crop.w, crop.h], "background": spec, "panel_x": plan.slot_w, "cell_px": [plan.cell_w, plan.cell_h], "factor": f});
        for (r, pl) in rows.iter().zip(&plan.place) {
            frame_json.insert(r.name.clone(), json!({"page": pl.page + 1, "cell": [pl.x, pl.y]}));
        }
    } else {
        // ---------------------------------------------------------------- contact sheets
        let plan_for = |s: usize| plan_sheet(rows.len(), vec![pw * s, GUT_L + pw * s, pw * s], GUT_T + ph * s);
        let s = pick_scale(MAX_AUTO_SCALE, |s| plan_for(s).ok().map(|p| p.pages));
        let plan = plan_for(s)?;
        let rows_ref = &rows;
        let cells: Vec<CellImg> = fsutil::par_map(&(0..rows.len()).collect::<Vec<_>>(), |&k| {
            let row = &rows_ref[k];
            let p = frame_panels(&part, tpl.as_ref(), row, crop, s, false);
            CellImg { name: row.name.clone(), iou: row.m.iou, errors: row.m.errors, warnings: row.m.warnings, lost: row.m.lost.len(), panels: vec![p.input, p.output, p.diff] }
        });
        let title = format!("{}{}  {} frames  {}x  input | output | diff", name, stamp_of(o), rows.len(), s);
        let imgs = render_pages(&plan, &cells, &title, true);
        write_pages(&imgs, &dir, &format!("{}_{}", name, kind), &mut pages_out, &mut page_json, &plan, &rows)?;
        layout = json!({
            "mode": "sheet", "scale": s, "factor": f, "crop": [crop.x0, crop.y0, crop.w, crop.h], "panel_px": [pw * s, ph * s], "cell_px": [plan.cell_w, plan.cell_h], "coord_gutter": [GUT_L, GUT_T],
            "label_h": LABEL_H, "gutter": GUTTER, "margin": MARGIN, "panel_gap": PANEL_GAP, "header_h": HEADER_H,
            "columns": plan.cols, "max_px": MAX_PX,
        });
        for (r, pl) in rows.iter().zip(&plan.place) {
            let g = metrics::geometry(&part, &r.rf);
            let px = |k: usize| json!([pl.x + pl.panel_x[k] + if k == 1 { GUT_L } else { 0 }, pl.y + LABEL_H + GUT_T]);
            frame_json.insert(
                r.name.clone(),
                json!({"page": pl.page + 1, "cell": [pl.x, pl.y], "panels": {"input": px(0), "output": px(1), "diff": px(2)},
                       "window_origin": [g.org.0, g.org.1]}),
            );
        }
    }

    // ---------------------------------------------------------------- metrics json + status
    let ranking = ranking_of(&rows);
    let frames_json: Vec<Value> = rows
        .iter()
        .map(|r| {
            let mut v = metrics_json(&r.m);
            v["frame"] = json!(r.name);
            if let Some(extra) = frame_json.get(&r.name) {
                for (k, x) in extra.as_object().unwrap() {
                    v[k] = x.clone();
                }
            }
            v["issues"] = Value::Array(r.issues.iter().filter(|i| i.severity != Severity::Advice).map(|i| i.to_json()).collect());
            v["notes"] = Value::Array(r.issues.iter().filter(|i| i.severity == Severity::Advice).map(|i| i.to_json()).collect());
            v
        })
        .collect();
    let n = rows.len() as f64;
    let mean_iou = rows.iter().map(|r| r.m.iou).sum::<f64>() / n;
    let json_all = json!({
        "part": name,
        "path": path.slash(),
        "template": tpl.as_ref().map(|t| t.name.clone()),
        "score_formula": metrics::SCORE_FORMULA,
        "ranking_label": RANKING_LABEL,
        "layout": layout,
        "pages": page_json,
        "frames": frames_json,
        "ranking": ranking.iter().map(|(f, m)| json!({"frame": f, "score": round4(m.score), "iou": round4(m.iou)})).collect::<Vec<_>>(),
        "summary": {"frames": rows.len(), "mean_iou": round4(mean_iou), "worst": ranking.first().map(|r| r.0.clone()),
                    "lost_total": rows.iter().map(|r| r.m.lost.len()).sum::<usize>(),
                    "eyes_found": rows.iter().map(|r| r.m.eyes_found).sum::<usize>(),
                    "eyes_kept": rows.iter().map(|r| r.m.eyes_kept).sum::<usize>()},
    });
    let json_path = if o.record || o.zoom.is_some() || o.on.is_some() {
        let tag = if o.zoom.is_some() { "zoom" } else if o.on.is_some() { "on" } else { "compare" };
        let p = dir.join(format!("{}_{}.json", name, tag));
        let mut s = serde_json::to_string_pretty(&json_all).unwrap_or_default();
        s.push('\n');
        write_file(&p, s.as_bytes())?;
        Some(p)
    } else {
        None
    };
    if o.record {
        status::record(path, &part, StatusEvent::Compared);
        let mut mm = BTreeMap::new();
        for r in &rows {
            mm.insert(r.name.clone(), metrics_json(&r.m));
        }
        status::record(path, &part, StatusEvent::Metrics(mm));
    }
    Ok(PartResult { name, pages: pages_out, json_path, json: json_all, ranking, warnings })
}

/// Largest integer scale (1..=MAX_AUTO_SCALE) whose sheet needs at most TARGET_PAGES pages; 1 otherwise.
fn pick_scale(max: usize, pages_at: impl Fn(usize) -> Option<usize>) -> usize {
    for s in (1..=max.min(MAX_AUTO_SCALE)).rev() {
        if let Some(p) = pages_at(s) {
            if p <= TARGET_PAGES {
                return s;
            }
        }
    }
    1
}

fn write_pages(imgs: &[RgbaImage], dir: &Path, stem: &str, files: &mut Vec<PathBuf>, pj: &mut Vec<Value>, plan: &SheetPlan, rows: &[Row]) -> Result<()> {
    let enc: Vec<Vec<u8>> = fsutil::par_map(imgs, encode_sheet_png);
    for (i, (im, bytes)) in imgs.iter().zip(enc).enumerate() {
        let file = dir.join(format!("{}_p{:02}.png", stem, i + 1));
        write_file(&file, &bytes)?;
        let frames: Vec<&str> = rows.iter().zip(&plan.place).filter(|(_, p)| p.page == i).map(|(r, _)| r.name.as_str()).collect();
        pj.push(json!({"file": file.slash(), "width": im.w, "height": im.h, "frames": frames}));
        files.push(file);
    }
    Ok(())
}

// ------------------------------------------------------------------------------------------------ commands

/// What the ranking means. It orders frames by difference from the reference; it never judges the art.
pub const RANKING_LABEL: &str = "where to look first (similarity to the reference; not a quality verdict)";

fn fmt_table(res: &PartResult, limit: usize) -> Vec<String> {
    let mut v = vec![format!("{:<24} {:>6} {:>13} {:>6} {:>4} {:>5} {:>7} {:>5}", "frame", "iou%", "bbox l t r b", "oklab", "lost", "lint", "score", "eyes")];
    for (f, m) in res.ranking.iter().take(limit) {
        v.push(format!(
            "{:<24} {:>6.1} {:>3} {:>2} {:>2} {:>2} {:>6} {:>4} {:>2}/{:<2} {:>7.1} {:>5}",
            truncate_chars(f, 24),
            m.iou * 100.0,
            m.bbox[0],
            m.bbox[1],
            m.bbox[2],
            m.bbox[3],
            m.oklab.map_or("n/a".to_string(), |e| format!("{:.3}", e)),
            m.lost.len(),
            m.errors,
            m.warnings,
            m.score,
            format!("{}/{}", m.eyes_kept, m.eyes_found)
        ));
    }
    if res.ranking.len() > limit {
        v.push(format!("... {} more frames (see the json)", res.ranking.len() - limit));
    }
    v
}

pub fn run_compare(a: &CompareArgs, out: &mut Out) -> Result<()> {
    let paths = fsutil::expand_parts(&a.parts)?;
    if paths.is_empty() {
        return Err(PxlError::usage("no parts matched"));
    }
    let mut jparts = Vec::new();
    for p in &paths {
        let part = crate::pxl::load(p)?;
        if part.kind != PartKind::Sprite {
            crate::tileio::compare_tileset(p, &part, a, out)?;
            continue;
        }
        let names: Vec<String> = a.sel.select(&part)?.iter().map(|f| f.name.clone()).collect();
        let all = names.len() == part.frames.len();
        let o = RunOpts {
            names: if all { Vec::new() } else { names },
            worst: None,
            zoom: a.zoom,
            on: a.on.clone(),
            out_dir: a.out.as_ref().map(PathBuf::from),
            kind: "compare".into(),
            record: a.zoom.is_none() && a.on.is_none() && a.set.is_none(),
            stamp: String::new(),
            set: a.set.clone(),
        };
        let res = compare_part(p, &o)?;
        for w in &res.warnings {
            out.warn(w.clone());
        }
        out.line(format!("{}: {} frames, mean iou {:.1}%", res.name, res.ranking.len(), res.json["summary"]["mean_iou"].as_f64().unwrap_or(0.0) * 100.0));
        if let Some(c) = res.json["layout"]["crop"].as_array() {
            let n: Vec<i64> = c.iter().map(|v| v.as_i64().unwrap_or(0)).collect();
            if n.len() == 4 {
                out.line(format!("  crop origin x={} y={} ({}x{} cells); the output panel is labelled with cell coordinates every 4 cells", n[0], n[1], n[2], n[3]));
            }
        }
        for f in &res.pages {
            out.line(format!("  {}", f.slash()));
        }
        if let Some(j) = &res.json_path {
            out.line(format!("  {}", j.slash()));
        }
        out.line(format!("  {}:", RANKING_LABEL));
        for l in fmt_table(&res, 15) {
            out.line(format!("  {}", l));
        }
        jparts.push(res.json);
    }
    if jparts.len() == 1 {
        let one = jparts.pop().unwrap();
        if let Value::Object(m) = one {
            for (k, v) in m {
                out.set(&k, v);
            }
        }
    } else {
        out.set("parts", Value::Array(jparts));
    }
    Ok(())
}

/// Review images for one part (used by `pxl review`): the worst frames, as paginated sheets.
pub fn review_part(path: &Path, frames: &[String], worst: usize, out_dir: Option<&Path>) -> Result<PartResult> {
    let o = RunOpts {
        names: frames.to_vec(),
        worst: if frames.is_empty() { Some(worst) } else { None },
        zoom: None,
        on: None,
        out_dir: out_dir.map(|d| d.to_path_buf()),
        kind: "review".into(),
        stamp: String::new(),
        record: false,
        set: None,
    };
    compare_part(path, &o)
}

// ------------------------------------------------------------------------------------------------ diff

struct Change {
    x: usize,
    y: usize,
    from: u8,
    to: u8,
}

fn diff_grids(a: &crate::model::Grid, b: &crate::model::Grid) -> (Vec<Change>, bool) {
    let (w, h) = (a.w.max(b.w), a.h.max(b.h));
    let mut v = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let ka = if x < a.w && y < a.h { a.cells[y * a.w + x] } else { b'?' };
            let kb = if x < b.w && y < b.h { b.cells[y * b.w + x] } else { b'?' };
            if ka != kb {
                v.push(Change { x, y, from: ka, to: kb });
            }
        }
    }
    (v, a.w != b.w || a.h != b.h)
}

pub fn run_diff(a: &DiffArgs, out: &mut Out) -> Result<()> {
    let pa = fsutil::resolve_part(&a.a)?;
    let part_a = crate::pxl::load(&pa)?;
    // pairs of (label, grid a, grid b)
    let mut pairs: Vec<(String, &crate::model::Grid, &crate::model::Grid)> = Vec::new();
    let part_b;
    let mut only: Vec<String> = Vec::new();
    match fsutil::resolve_part(&a.b) {
        Ok(pb) => {
            part_b = Some(crate::pxl::load(&pb)?);
            let pb = part_b.as_ref().unwrap();
            let names: Vec<String> = if a.sel.frames.is_empty() || a.sel.all {
                part_a.frames.iter().map(|f| f.name.clone()).collect()
            } else {
                a.sel.select(&part_a)?.iter().map(|f| f.name.clone()).collect()
            };
            for n in &names {
                match (part_a.frame(n), pb.frame(n)) {
                    (Some(fa), Some(fb)) => pairs.push((n.clone(), &fa.grid, &fb.grid)),
                    _ => only.push(format!("{} (only in a)", n)),
                }
            }
            if a.sel.frames.is_empty() || a.sel.all {
                for f in &pb.frames {
                    if part_a.frame(&f.name).is_none() {
                        only.push(format!("{} (only in b)", f.name));
                    }
                }
            }
        }
        Err(_) => {
            // `diff <part> <frameA> --frames <frameB>`: two frames of one part
            let fa = part_a.frame(&a.b).ok_or_else(|| {
                PxlError::usage(format!("'{}' is neither a part file nor a frame of {}", a.b, part_a.name))
                    .with_hint("pxl diff <a.pxl> <b.pxl>, or pxl diff <part> <frameA> --frames <frameB>")
            })?;
            let other = a.sel.frames.first().ok_or_else(|| PxlError::usage("give the second frame with --frames <frameB>"))?;
            let fb = part_a.frame(other).ok_or_else(|| unknown_frame(&part_a, other))?;
            part_b = None;
            pairs.push((format!("{} -> {}", fa.name, fb.name), &fa.grid, &fb.grid));
        }
    }
    let _ = &part_b;
    let mut total = 0usize;
    let mut jframes = Vec::new();
    for (label, ga, gb) in &pairs {
        let (ch, resized) = diff_grids(ga, gb);
        total += ch.len();
        let bbox = if ch.is_empty() {
            None
        } else {
            Some((
                ch.iter().map(|c| c.x).min().unwrap(),
                ch.iter().map(|c| c.y).min().unwrap(),
                ch.iter().map(|c| c.x).max().unwrap(),
                ch.iter().map(|c| c.y).max().unwrap(),
            ))
        };
        if ch.is_empty() {
            out.line(format!("{}: identical", label));
        } else {
            let (x0, y0, x1, y1) = bbox.unwrap();
            out.line(format!("{}: {} cells differ, bbox x {}..{} y {}..{}{}", label, ch.len(), x0, x1, y0, y1, if resized { " (grid sizes differ)" } else { "" }));
            // text grid of the changed region: new key where changed, '-' where equal
            let mut line = format!("     ");
            for x in x0..=x1 {
                line.push_str(&format!("{}", x % 10));
            }
            out.line(line);
            for y in y0..=y1 {
                let mut s = format!("{:>4} ", y);
                for x in x0..=x1 {
                    match ch.iter().find(|c| c.x == x && c.y == y) {
                        Some(c) => s.push(c.to as char),
                        None => s.push('-'),
                    }
                }
                out.line(s);
            }
            for c in ch.iter().take(30) {
                out.line(format!("  ({},{}) {} -> {}", c.x, c.y, c.from as char, c.to as char));
            }
            if ch.len() > 30 {
                out.line(format!("  ... {} more", ch.len() - 30));
            }
        }
        jframes.push(json!({
            "frame": label,
            "changed": ch.len(),
            "bbox": bbox.map(|b| json!({"x0": b.0, "y0": b.1, "x1": b.2, "y1": b.3})),
            "size_differs": resized,
            "changes": ch.iter().map(|c| json!({"x": c.x, "y": c.y, "from": (c.from as char).to_string(), "to": (c.to as char).to_string()})).collect::<Vec<_>>(),
        }));
    }
    for o in &only {
        out.line(format!("{}", o));
    }
    out.line(format!("{} cells differ in {} frames", total, jframes.iter().filter(|f| f["changed"].as_u64().unwrap_or(0) > 0).count()));
    out.set("frames", Value::Array(jframes));
    out.set("total_changed", json!(total));
    out.set("unmatched", json!(only));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_covers_label_chars() {
        for c in "0123456789abcdefghijklmnopqrstuvwxyz_.%-: /|".chars() {
            assert!(glyph_rows(c).is_some(), "missing glyph {}", c);
        }
        for (_, g) in FONT {
            assert_eq!(g.split(' ').count(), 5);
            assert!(g.split(' ').all(|r| r.len() == 3));
        }
    }

    #[test]
    fn png_is_decodable() {
        let mut im = RgbaImage::filled(5, 4, Rgba::opaque(1, 2, 3));
        im.set(2, 2, Rgba::opaque(9, 8, 7));
        let bytes = encode_sheet_png(&im);
        let d = png::Decoder::new(std::io::Cursor::new(bytes));
        let mut r = d.read_info().unwrap();
        let mut buf = vec![0; r.output_buffer_size()];
        let info = r.next_frame(&mut buf).unwrap();
        assert_eq!((info.width, info.height), (5, 4));
        assert_eq!(&buf[(2 * 5 + 2) * 3..(2 * 5 + 2) * 3 + 3], &[9, 8, 7]);
    }

    #[test]
    fn capacity_respects_limit() {
        let (c, r) = capacity(196, 80).unwrap();
        assert!(2 * MARGIN + c * (196 + GUTTER) - GUTTER <= MAX_PX);
        assert!(HEADER_H + 2 * MARGIN + r * (80 + GUTTER) - GUTTER <= MAX_PX);
        assert!(capacity(MAX_PX, 10).is_none());
    }
}
