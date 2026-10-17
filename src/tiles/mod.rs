//! Tile sets for the 4-colour, 8-palette hardware (DESIGN 5.5, 5.6): reduction of 16x16 blocks to 8x8 tiles,
//! statistics, palette fitting and near-duplicate merging.
//!
//! * [`reduce`]: per block, no neighbour ring, flip-equivariant, air/solid mask first, colour vote limited to the
//!   best template palette.
//! * [`stats`]: colours per tile, unique tiles (the checker's rule: a tile equal to another up to mirroring is a
//!   duplicate), the checker's first-fit palette packing and an optimal packing.
//! * [`fit`]: cheapest Oklab remaps so every tile has at most 4 colours and the first-fit packing needs at most 8
//!   palettes.
//! * [`dedupe`]: merges near-duplicate tiles (flips included) until at most N unique tiles remain.
//!
//! A tile grid cell holds a template key; the template's `[palettes]` / `[tiles] air` describe the hardware.

use crate::fsutil::PathSlash;
use crate::cli::TilesArgs;
use crate::color::{self, Oklab, Rgb};
use crate::model::{Grid, Part, PartKind};
use crate::output::{Loc, Out, PxlError, Result};
use crate::template::Template;
use crate::{fsutil, pxl, template};
use serde_json::json;

pub mod dedupe;
pub mod fit;
pub mod palette;
pub mod pattern;
pub mod priority;
pub mod reduce;
pub mod slots;
pub mod stats;

/// Tile edge in pixels.
pub const T: usize = 8;
/// Colour code of a transparent cell (the key `.`).
pub const TRANSP: u32 = 1 << 24;

pub fn code_of(c: Rgb) -> u32 {
    (c.r as u32) << 16 | (c.g as u32) << 8 | c.b as u32
}
pub fn rgb_of(code: u32) -> Rgb {
    Rgb { r: (code >> 16) as u8, g: (code >> 8) as u8, b: code as u8 }
}
pub fn hex_of(code: u32) -> String {
    if code & TRANSP != 0 {
        "transparent".to_string()
    } else {
        color::to_hex(rgb_of(code))
    }
}

/// Key -> colour tables of one part and template.
#[derive(Clone)]
pub struct Colors {
    /// Colour code per key (`TRANSP` for `.` and undefined keys).
    pub code: [u32; 256],
    pub lab: [Oklab; 256],
    /// Per key: the key to use for its colour (palette keys first, then the lowest key).
    pub canon: [u8; 256],
    pub defined: [bool; 256],
}

impl Colors {
    pub fn new(part: &Part, tpl: &Template) -> Colors {
        let mut c = Colors { code: [TRANSP; 256], lab: [Oklab::default(); 256], canon: [0; 256], defined: [false; 256] };
        for k in 0..256usize {
            c.canon[k] = k as u8;
        }
        let in_pal = |k: u8| tpl.palettes.iter().any(|p| p.keys.contains(&k));
        for kd in tpl.keys() {
            if let Some(rgb) = crate::lint::effective_color(part, tpl, kd.key) {
                let k = kd.key as usize;
                c.code[k] = code_of(rgb);
                c.lab[k] = color::srgb_to_oklab(rgb);
                c.defined[k] = true;
            }
        }
        c.defined[b'.' as usize] = true;
        for k in 0..256usize {
            if !c.defined[k] || c.code[k] & TRANSP != 0 {
                continue;
            }
            let mut best = k as u8;
            for j in 0..256usize {
                if c.defined[j] && c.code[j] == c.code[k] {
                    let better = (in_pal(j as u8) && !in_pal(best)) || (in_pal(j as u8) == in_pal(best) && (j as u8) < best);
                    if better {
                        best = j as u8;
                    }
                }
            }
            c.canon[k] = best;
        }
        c
    }
    /// Oklab distance x 100 between two keys (about 1 = barely visible, 5 = clearly visible).
    pub fn de(&self, a: u8, b: u8) -> f64 {
        if self.code[a as usize] == self.code[b as usize] {
            return 0.0;
        }
        color::oklab_dist2(self.lab[a as usize], self.lab[b as usize]).sqrt() * 100.0
    }
}

/// One 8x8 tile as template keys, row major.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Tile {
    pub keys: [u8; 64],
}

impl Tile {
    /// Colour codes of the pixels.
    pub fn px(&self, c: &Colors) -> [u32; 64] {
        let mut o = [0u32; 64];
        for i in 0..64 {
            o[i] = c.code[self.keys[i] as usize];
        }
        o
    }
    /// Distinct keys in first-use order.
    pub fn key_set(&self) -> Vec<u8> {
        let mut v: Vec<u8> = Vec::new();
        for &k in &self.keys {
            if !v.contains(&k) {
                v.push(k);
            }
        }
        v
    }
    pub fn flipped(&self, fx: bool, fy: bool) -> Tile {
        let mut o = *self;
        for y in 0..T {
            for x in 0..T {
                o.keys[y * T + x] = self.keys[(if fy { T - 1 - y } else { y }) * T + if fx { T - 1 - x } else { x }];
            }
        }
        o
    }
    pub fn rows(&self) -> Vec<String> {
        (0..T).map(|y| self.keys[y * T..y * T + T].iter().map(|&k| k as char).collect()).collect()
    }
}

/// The tiles of one frame in sheet order.
#[derive(Clone)]
pub struct Sheet {
    /// Tiles per row.
    pub w: usize,
    pub h: usize,
    pub tiles: Vec<Tile>,
}

impl Sheet {
    pub fn from_grid(g: &Grid) -> Sheet {
        let (w, h) = (g.w / T, g.h / T);
        let mut tiles = Vec::with_capacity(w * h);
        for ty in 0..h {
            for tx in 0..w {
                let mut keys = [0u8; 64];
                for y in 0..T {
                    for x in 0..T {
                        keys[y * T + x] = g.cells[(ty * T + y) * g.w + tx * T + x];
                    }
                }
                tiles.push(Tile { keys });
            }
        }
        Sheet { w, h, tiles }
    }
    /// Pixel position of tile `i`.
    pub fn pos(&self, i: usize) -> (usize, usize) {
        ((i % self.w.max(1)) * T, (i / self.w.max(1)) * T)
    }
    pub fn to_grid(&self) -> Grid {
        let mut g = Grid::new(self.w * T, self.h * T, b'.');
        for (i, t) in self.tiles.iter().enumerate() {
            let (px, py) = self.pos(i);
            for y in 0..T {
                for x in 0..T {
                    g.cells[(py + y) * g.w + px + x] = t.keys[y * T + x];
                }
            }
        }
        g
    }
}

/// Palettes tiles may use: 1 for a part on its own BG palette, else the profile's palettes minus the reserved ones.
pub fn max_palettes(part: &Part, prof: &crate::profile::Profile) -> usize {
    match part.bg_palette {
        Some(_) => 1,
        None => (prof.palettes as usize).saturating_sub(reserved_of(part, prof).len()).max(1),
    }
}

/// BG palettes kept free for a part: its own `bg_reserved`, else the profile's.
pub fn reserved_of<'a>(part: &'a Part, prof: &'a crate::profile::Profile) -> &'a [u8] {
    part.bg_reserved.as_deref().unwrap_or(&prof.bg_reserved)
}

/// Colour codes of the template's air keys.
pub fn air_codes(tpl: &Template, cols: &Colors) -> Vec<u32> {
    let mut v: Vec<u32> = tpl.tile_air.iter().map(|&k| cols.code[k as usize]).filter(|c| c & TRANSP == 0).collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// Unique tile limit of a part: `[screen] unique_max`, else the profile's `unique_tiles_max`.
pub fn unique_limit(part: &Part, prof: &crate::profile::Profile) -> usize {
    part.max_tiles.or(part.screen.as_ref().and_then(|s| s.unique_max)).unwrap_or(prof.unique_tiles_max) as usize
}

/// Frame index to work on: `--frame`, else `blocks`, else the first.
pub fn pick_frame(part: &Part, want: Option<&str>) -> Result<usize> {
    match want {
        Some(n) => part.frame_index(n).ok_or_else(|| crate::cli::unknown_frame(part, n)),
        None => Ok(part.frame_index("blocks").unwrap_or(0)),
    }
    .and_then(|i| if part.frames.is_empty() { Err(PxlError::validation("part has no frames")) } else { Ok(i) })
}

/// Everything the tile commands and rules need about one frame.
pub struct Loaded {
    pub part: Part,
    pub tpl: Template,
    pub cols: Colors,
    pub fi: usize,
    pub sheet: Sheet,
    /// Palette-independent tile identity (profile `pattern_unique`).
    pub pattern: bool,
    /// Colour codes of the template's air keys.
    pub air: Vec<u32>,
}

pub fn load_frame(part: Part, tpl: Template, fi: usize) -> Result<Loaded> {
    if part.kind == PartKind::Sprite {
        return Err(PxlError::usage(format!("{} is a sprite; tile commands work on tile sets and screens", part.name)));
    }
    let g = &part.frames[fi].grid;
    if g.w % T != 0 || g.h % T != 0 {
        return Err(PxlError::validation(format!("frame {} is {}x{}, not a multiple of 8", part.frames[fi].name, g.w, g.h)).with_loc(Loc::file(part.path.slash())));
    }
    let cols = Colors::new(&part, &tpl);
    let sheet = Sheet::from_grid(g);
    let pattern = crate::profile::load_for(&part, None).map(|p| p.pattern_unique).unwrap_or(false);
    let air = air_codes(&tpl, &cols);
    Ok(Loaded { part, tpl, cols, fi, sheet, pattern, air })
}

fn open(a: &TilesArgs) -> Result<(std::path::PathBuf, Loaded)> {
    let path = fsutil::resolve_part(&a.tileset)?;
    let part = pxl::load(&path)?;
    let tpl = template::load_for(&part)?;
    let fi = pick_frame(&part, a.frame.as_deref())?;
    Ok((path, load_frame(part, tpl, fi)?))
}

fn tile_xy(s: &Sheet, i: usize) -> String {
    let (x, y) = s.pos(i);
    format!("#{} ({},{})", i, x, y)
}

/// `pxl tiles stats`
pub fn run_stats(a: &TilesArgs, out: &mut Out) -> Result<()> {
    let (path, l) = open(a)?;
    let mut st = stats::stats(&l.sheet, &l.cols);
    if l.pattern {
        st.groups = pattern::groups(&l.sheet, &l.cols, &l.air, true);
        st.unique = st.groups.len();
    }
    let fname = l.part.frames[l.fi].name.clone();
    out.line(format!("{} frame {}: {} tiles ({} per row)", path.slash(), fname, l.sheet.tiles.len(), l.sheet.w));
    let hist: Vec<String> = st.color_hist.iter().enumerate().filter(|(_, n)| **n > 0).map(|(c, n)| format!("{} colours: {}", c, n)).collect();
    out.line(format!("colours per tile: {}", hist.join(", ")));
    for &t in st.over4.iter().take(20) {
        out.line(format!("  tile {} uses {} colours (max 4)", tile_xy(&l.sheet, t), stats::colors_of(&l.tiles_px(t)).len()));
    }
    if st.over4.len() > 20 {
        out.line(format!("  ... {} more tiles over 4 colours", st.over4.len() - 20));
    }
    out.line(format!(
        "unique tiles ({}): {} of {}",
        if l.pattern { "same index pattern up to mirroring, whatever the palette" } else { "mirrors count as duplicates" },
        st.unique,
        l.sheet.tiles.len()
    ));
    if l.pattern {
        let by_colour = stats::unique_groups(&l.sheet, &l.cols).len();
        out.line(format!("  by pixel colours (old rule) {}: palette-independent counting saves {}", by_colour, by_colour.saturating_sub(st.unique)));
        out.set("unique_by_colours", json!(by_colour));
    }
    let prof = crate::profile::load_for(&l.part, None)?;
    let anim = crate::tiles::slots::animated_slots(&l.part, &prof);
    if prof.count_animated || anim.total > 0 {
        out.line(format!("animated tile slots: {} ({}); level tiles + animated slots = {} (limit {})", anim.total, anim.describe(), st.unique + anim.total, unique_limit(&l.part, &prof)));
        out.set("animated_slots", json!(anim.total));
    }
    let dups: Vec<&Vec<usize>> = st.groups.iter().filter(|g| g.len() > 1).collect();
    out.line(format!("duplicate groups: {} ({} tiles are copies)", dups.len(), dups.iter().map(|g| g.len() - 1).sum::<usize>()));
    for g in dups.iter().take(a.top.unwrap_or(12)) {
        let shown: Vec<String> = g.iter().take(8).map(|t| t.to_string()).collect();
        let more = if g.len() > 8 { format!(" = ... ({} copies in all)", g.len()) } else { String::new() };
        out.line(format!("  tiles {}{} are the same tile", shown.join(" = "), more));
    }
    let maxp = max_palettes(&l.part, &prof);
    out.line(format!(
        "palettes: checker first-fit {} (max {}{}){}, optimal {}{}",
        st.greedy.bins.len(),
        maxp,
        if maxp < prof.palettes as usize && l.part.bg_palette.is_none() { format!(": BG palette {} reserved", reserved_of(&l.part, &prof).iter().map(|b| b.to_string()).collect::<Vec<_>>().join(",")) } else { String::new() },
        if st.greedy.bins.len() > maxp { format!(", the first tile that opens palette {} is {}", maxp + 1, tile_xy(&l.sheet, st.greedy.opener[maxp])) } else { String::new() },
        if st.optimal_exact { "" } else { "<=" },
        st.optimal
    ));
    if st.greedy.bins.len() > maxp {
        out.line(format!("fix: pxl tiles fit {} --apply", a.tileset));
    }
    out.set("frame", json!(fname));
    out.set("tiles", json!(l.sheet.tiles.len()));
    out.set("colors_per_tile", json!(st.color_hist));
    out.set("over_4_colors", json!(st.over4));
    out.set("unique", json!(st.unique));
    out.set("duplicate_groups", json!(dups));
    out.set("palettes_greedy", json!(st.greedy.bins.len()));
    out.set("palettes_optimal", json!(st.optimal));
    out.set("palettes_optimal_exact", json!(st.optimal_exact));
    out.set("palette_openers", json!(st.greedy.opener));
    out.set("transparent_tiles", json!(st.transparent));
    Ok(())
}

impl Loaded {
    pub fn tiles_px(&self, t: usize) -> [u32; 64] {
        self.sheet.tiles[t].px(&self.cols)
    }
}

/// `pxl tiles fit`
pub fn run_fit(a: &TilesArgs, out: &mut Out) -> Result<()> {
    let (_path, l) = open(a)?;
    let max_pal = max_palettes(&l.part, &crate::profile::load_for(&l.part, None)?);
    let plan = fit::plan(&l, max_pal);
    fit::report(&l, &plan, a, out)?;
    Ok(())
}

/// `pxl tiles dedupe`
pub fn run_dedupe(a: &TilesArgs, out: &mut Out) -> Result<()> {
    let (_path, l) = open(a)?;
    dedupe::run(&l, a, out)
}
