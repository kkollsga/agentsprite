//! Shared sprite palette sets and per-cell palettes (M5 phase T1, DESIGN "Palette sets and cells").
//!
//! On the GBC every sprite palette is shared by everything on screen: 8 OBJ slots of 3 colours (+ transparent). A part
//! with `palette_set = "zone_2"` is drawn with plain template keys whose colours all come from that set
//! (`palettes/zone_2.toml`, found above the part). Nothing else is stored: the slot of every 8x16 cell is derived.
//!
//! * **Palette set file** (`palettes/<name>.toml`): `include = "global"` pulls slots 0-3 from `global.toml`; then one
//!   `[[slots]]` per own slot with `slot = N`, optional `name`, `colors = ["#..", "#..", "#.."]` (three RGB555
//!   colours, channels multiples of 8, all different) and, for slot 7 only, `variants = { boss = [..], fire = [..] }`.
//!   The same RGB may sit in several slots.
//! * **Cell validity**: a cell (profile `hw_cell`, 8x16) is valid when one allowed slot holds every colour in it. Only
//!   the RGB values count (base/detail layers of the template are irrelevant). For the category `player` up to
//!   `multi_slot_max` (2) cells per frame may instead need two slots; such a cell costs 2 sprites (base + overlay).
//!   Any other cell that needs more than one slot is a `cell_palette` error.
//! * **Placement**: the engine places cells freely, so per frame we search a band offset `oy` (0..cell_h-1) and per
//!   16 px band a column offset `ox` (0..cell_w-1) minimising (invalid cells, max sprites on a scanline, total
//!   sprites); ties go to the smallest offsets (so the canvas-anchored grid wins when nothing is better). An authored
//!   `<frame>.cells.json` next to the part (`[{"x":-8,"y":-16,"slot":0}]`, relative to the origin, optional `slot2`)
//!   replaces the search; it must cover every opaque pixel.
//! * `hw_limit` counts the real scanlines of that placement against the category limits of the profile.

use crate::fsutil::PathSlash;
use crate::color::{self, Rgb};
use crate::fsutil;
use crate::model::Part;
use crate::output::{Loc, Out, PxlError, Result};
use crate::profile::{self, Profile};
use crate::template::Template;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

// ------------------------------------------------------------------------------------------------ palette set

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotDef {
    pub slot: u8,
    pub name: String,
    pub colors: Vec<Rgb>,
    pub variants: BTreeMap<String, Vec<Rgb>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaletteSet {
    pub name: String,
    pub path: PathBuf,
    /// Always 8 entries.
    pub slots: Vec<Option<SlotDef>>,
    /// The set file and every file it includes (for change hashes).
    pub files: Vec<PathBuf>,
}

fn serr(path: &Path, msg: impl Into<String>) -> PxlError {
    PxlError::validation(msg).with_loc(Loc::file(path.slash()))
}

fn colour_list(path: &Path, ctx: &str, v: &toml::Value, prof: &Profile) -> Result<Vec<Rgb>> {
    let a = v.as_array().ok_or_else(|| serr(path, format!("{}: `colors` must be a list of three \"#rrggbb\" strings", ctx)))?;
    if a.len() != 3 {
        return Err(serr(path, format!("{}: needs exactly 3 colours (colour 0 is transparent), got {}", ctx, a.len())));
    }
    let mut out: Vec<Rgb> = Vec::new();
    for e in a {
        let s = e.as_str().ok_or_else(|| serr(path, format!("{}: colour must be a string like \"#4850c0\"", ctx)))?;
        let c = color::parse_hex(s).map_err(|e| serr(path, format!("{}: {}", ctx, e.msg)))?;
        if !prof.legal(c) {
            let why = if prof.fixed_colours.is_empty() { format!("not representable in RGB555 (channels must be multiples of {})", prof.channel_step) } else { "not in the console palette".to_string() };
            return Err(serr(path, format!("{}: colour {} is {}", ctx, color::to_hex(c), why)).with_hint(format!("nearest legal colour: {}", color::to_hex(prof.snap(c)))));
        }
        if out.contains(&c) {
            return Err(serr(path, format!("{}: colour {} appears twice in one slot", ctx, color::to_hex(c))));
        }
        out.push(c);
    }
    Ok(out)
}

/// Parse the text of a palette set file. `dir` resolves `include`.
pub fn parse_set(name: &str, text: &str, path: &Path, depth: usize) -> Result<PaletteSet> {
    let root: toml::Table = text.parse().map_err(|e: toml::de::Error| serr(path, format!("invalid palette set TOML: {}", e.message())))?;
    for k in root.keys() {
        if !["include", "name", "slots", "console"].contains(&k.as_str()) {
            return Err(serr(path, format!("unknown key '{}'", k)).with_hint("allowed: console, include, name, [[slots]]"));
        }
    }
    // the colours a set may hold are those of the console: its `console = ".."` key, else the project's
    let dir_of_set = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    let cid = root.get("console").and_then(|c| c.as_str()).map(String::from).unwrap_or_else(|| crate::console::project_console(dir_of_set).id);
    let prof = crate::console::resolve(&cid, "sprite", Some(dir_of_set)).or_else(|_| crate::console::resolve(crate::console::DEFAULT_CONSOLE, "sprite", None))?;
    let prof = &prof;
    let mut set = PaletteSet { name: name.to_string(), path: path.to_path_buf(), slots: vec![None; 8], files: vec![path.to_path_buf()] };
    let mut own: Vec<u8> = Vec::new();
    for sv in root.get("slots").map(|v| v.as_array().cloned()).unwrap_or(Some(Vec::new())).ok_or_else(|| serr(path, "`slots` must be an array of tables: [[slots]]"))? {
        let t = sv.as_table().ok_or_else(|| serr(path, "`slots` must be an array of tables: [[slots]]"))?;
        for k in t.keys() {
            if !["slot", "name", "colors", "variants"].contains(&k.as_str()) {
                return Err(serr(path, format!("slot entry: unknown key '{}'", k)).with_hint("allowed: slot, name, colors, variants"));
            }
        }
        let n = t.get("slot").and_then(|v| v.as_integer()).filter(|n| (0..8).contains(n)).ok_or_else(|| serr(path, "slot entry needs `slot = N` with N in 0..7"))? as u8;
        if own.contains(&n) {
            return Err(serr(path, format!("slot {} is defined twice", n)));
        }
        own.push(n);
        let ctx = format!("slot {}", n);
        let colors = match t.get("colors") {
            Some(v) => colour_list(path, &ctx, v, prof)?,
            None => Vec::new(),
        };
        let mut variants = BTreeMap::new();
        if let Some(v) = t.get("variants") {
            if n != 7 {
                return Err(serr(path, format!("{}: only slot 7 (the dynamic slot) may have variants", ctx)));
            }
            let vt = v.as_table().ok_or_else(|| serr(path, format!("{}: `variants` must be a table: variants = {{ boss = [..] }}", ctx)))?;
            for (vn, vv) in vt {
                variants.insert(vn.clone(), colour_list(path, &format!("{} variant '{}'", ctx, vn), vv, prof)?);
            }
        }
        if colors.is_empty() && variants.is_empty() {
            return Err(serr(path, format!("{}: needs `colors` (3 colours) or `variants`", ctx)));
        }
        let sname = t.get("name").and_then(|v| v.as_str()).map(String::from).unwrap_or_else(|| format!("slot {}", n));
        set.slots[n as usize] = Some(SlotDef { slot: n, name: sname, colors, variants });
    }
    if let Some(inc) = root.get("include") {
        let inc = inc.as_str().ok_or_else(|| serr(path, "`include` must be a set name, e.g. include = \"global\""))?;
        if depth >= 3 {
            return Err(serr(path, "`include` chain too deep (cycle?)"));
        }
        let dir = path.parent().unwrap_or_else(|| Path::new("."));
        let ipath = dir.join(format!("{}.toml", inc));
        if !ipath.is_file() {
            return Err(serr(path, format!("include '{}': {} not found", inc, ipath.slash())));
        }
        let itext = fsutil::read_string(&ipath)?;
        let iset = parse_set(inc, &itext, &ipath, depth + 1)?;
        // slots 0-3 are global; the dynamic slot 7 comes along unless the including set defines its own
        for s in [0usize, 1, 2, 3, 7] {
            if let Some(d) = &iset.slots[s] {
                if set.slots[s].is_some() && s == 7 {
                    continue;
                }
                if set.slots[s].is_some() {
                    return Err(serr(path, format!("slot {} is defined here and comes from include '{}' (slots 0-3 are global)", s, inc)));
                }
                set.slots[s] = Some(d.clone());
            }
        }
        set.files.extend(iset.files);
    }
    Ok(set)
}

/// Every colour of a set in slot order, the slot-7 variants included (no duplicates).
pub fn all_colours(set: &PaletteSet) -> Vec<Rgb> {
    let mut v: Vec<Rgb> = Vec::new();
    for d in set.slots.iter().flatten() {
        for c in d.colors.iter().chain(d.variants.values().flatten()) {
            if !v.contains(c) {
                v.push(*c);
            }
        }
    }
    v
}

/// Load `palettes/<name>.toml` found at or above `from_dir`.
pub fn load_set(name: &str, from_dir: &Path) -> Result<PaletteSet> {
    let rel = format!("palettes/{}.toml", name);
    let f = fsutil::find_up(from_dir, &rel).ok_or_else(|| {
        PxlError::validation(format!("palette set '{}' not found (looked for {} at or above {})", name, rel, from_dir.slash())).with_hint("create palettes/<name>.toml next to pxl.toml (see DESIGN, Palette sets)")
    })?;
    let text = fsutil::read_string(&f)?;
    parse_set(name, &text, &f, 0)
}

// ------------------------------------------------------------------------------------------------ resolved set

/// The set as one part sees it: allowed slots, colour bits and the limits of its category.
#[derive(Clone, Debug)]
pub struct Resolved {
    pub set: PaletteSet,
    /// Allowed slots in ascending order: (slot, name, its 3 colours).
    pub slots: Vec<(u8, String, [Rgb; 3])>,
    /// Distinct colours of the allowed slots (at most 24); the bit of a colour is its index.
    pub palette: Vec<Rgb>,
    /// Per allowed slot: bit mask of its colours in `palette`.
    pub masks: Vec<u32>,
    pub category: String,
    pub per_line: u32,
    pub overlay_line: u32,
    pub per_frame: u32,
    /// Cells per frame that may need two slots (0 unless the category is `player`).
    pub multi_max: u32,
    pub cell: (usize, usize),
    /// Further sets of a multi-set part, with the colours of every kept slot (same order as `slots`).
    pub extras: Vec<ExtraSet>,
}

/// One more palette set of a part that is recoloured at runtime (`palette_set = ["zone_1", "zone_3"]`).
#[derive(Clone, Debug)]
pub struct ExtraSet {
    pub name: String,
    pub set: PaletteSet,
    pub cols: Vec<[Rgb; 3]>,
}

fn slot_colours(d: &SlotDef, variant: &Option<String>) -> Option<[Rgb; 3]> {
    let c: Option<&Vec<Rgb>> = match variant {
        Some(v) if d.variants.contains_key(v) => d.variants.get(v),
        _ if !d.colors.is_empty() => Some(&d.colors),
        _ => None,
    };
    c.map(|c| [c[0], c[1], c[2]])
}

impl Resolved {
    /// Name and colours of every set the part is drawn in: the authoring set first.
    pub fn all_sets(&self) -> Vec<(String, Vec<[Rgb; 3]>)> {
        let mut v = vec![(self.set.name.clone(), self.slots.iter().map(|s| s.2).collect::<Vec<_>>())];
        v.extend(self.extras.iter().map(|e| (e.name.clone(), e.cols.clone())));
        v
    }
    pub fn slot_index(&self, slot: u8) -> Option<usize> {
        self.slots.iter().position(|s| s.0 == slot)
    }
    pub fn colour_bit(&self, c: Rgb) -> Option<usize> {
        self.palette.iter().position(|p| *p == c)
    }
    /// Nearest colour of the allowed slots (Oklab): (slot, colour).
    pub fn nearest(&self, c: Rgb) -> Option<(u8, Rgb)> {
        let o = color::srgb_to_oklab(c);
        let mut best: Option<(f64, u8, Rgb)> = None;
        for (s, _, cols) in &self.slots {
            for &k in cols {
                let d = color::oklab_dist2(o, color::srgb_to_oklab(k));
                if best.map_or(true, |b| d < b.0) {
                    best = Some((d, *s, k));
                }
            }
        }
        best.map(|b| (b.1, b.2))
    }
    pub fn slot_label(&self, slot: u8) -> String {
        match self.slot_index(slot) {
            Some(i) => format!("{} ({})", slot, self.slots[i].1),
            None => slot.to_string(),
        }
    }
}

/// Resolve the palette set, slots, variant and category of a part.
pub fn resolve(part: &Part, prof: &Profile) -> Result<Resolved> {
    let name = part.palette_set.as_deref().ok_or_else(|| PxlError::usage("part has no palette_set"))?;
    let set = load_set(name, &part.dir())?;
    let ferr = |m: String| PxlError::validation(m).with_loc(Loc::file(part.path.slash()));
    let mut chosen: Vec<u8> = if part.slots.is_empty() { (0..8).filter(|s| set.slots[*s as usize].is_some()).collect() } else { part.slots.clone() };
    chosen.sort();
    chosen.dedup();
    if let Some(v) = &part.variant {
        if !set.slots.iter().flatten().any(|s| s.variants.contains_key(v)) {
            return Err(ferr(format!("variant '{}' is not defined in palette set '{}'", v, set.name)));
        }
    }
    let mut slots: Vec<(u8, String, [Rgb; 3])> = Vec::new();
    for s in chosen {
        let Some(d) = &set.slots[s as usize] else {
            return Err(ferr(format!("slot {} is not defined in palette set '{}'", s, set.name)).with_hint("slots = [..] may only list slots the set defines"));
        };
        let cols: Option<&Vec<Rgb>> = match &part.variant {
            Some(v) if d.variants.contains_key(v) => d.variants.get(v),
            _ if !d.colors.is_empty() => Some(&d.colors),
            _ => None,
        };
        match cols {
            Some(c) => slots.push((s, d.name.clone(), [c[0], c[1], c[2]])),
            None if !part.slots.is_empty() => {
                return Err(ferr(format!("slot {} of '{}' only has variants ({}); set `variant = \"..\"` on the part", s, set.name, d.variants.keys().cloned().collect::<Vec<_>>().join(", "))));
            }
            None => {}
        }
    }
    if slots.is_empty() {
        return Err(ferr(format!("palette set '{}' leaves no usable slot for this part", set.name)));
    }
    // more sets: only slots every set defines can carry a cell (the slot index is shared)
    let mut extras: Vec<ExtraSet> = Vec::new();
    for en in &part.palette_more {
        let es = load_set(en, &part.dir())?;
        extras.push(ExtraSet { name: en.clone(), set: es, cols: Vec::new() });
    }
    if !extras.is_empty() {
        let mut keep: Vec<bool> = vec![true; slots.len()];
        for e in extras.iter_mut() {
            for (i, (s, _, _)) in slots.iter().enumerate() {
                match e.set.slots[*s as usize].as_ref().and_then(|d| slot_colours(d, &part.variant)) {
                    Some(c) => e.cols.push(c),
                    None => {
                        if !part.slots.is_empty() {
                            return Err(ferr(format!("slot {} is not defined in palette set '{}' (every set of `palette_set` must define the part's slots)", s, e.name)));
                        }
                        keep[i] = false;
                        e.cols.push([Rgb::new(0, 0, 0); 3]);
                    }
                }
            }
        }
        fn filt<T>(v: &mut Vec<T>, keep: &[bool]) {
            let mut i = 0;
            v.retain(|_| {
                i += 1;
                keep[i - 1]
            });
        }
        filt(&mut slots, &keep);
        for e in extras.iter_mut() {
            filt(&mut e.cols, &keep);
        }
        if slots.is_empty() {
            return Err(ferr(format!("no slot is defined in all of the sets {}", part.palette_sets().join(", "))));
        }
    }
    let mut palette: Vec<Rgb> = Vec::new();
    for (_, _, cols) in &slots {
        for c in cols {
            if !palette.contains(c) {
                palette.push(*c);
            }
        }
    }
    let masks = slots.iter().map(|(_, _, cols)| cols.iter().fold(0u32, |m, c| m | 1 << palette.iter().position(|p| p == c).unwrap())).collect();
    let category = part.category.clone().unwrap_or_else(|| prof.default_category.clone());
    if !profile::CATEGORIES.contains(&category.as_str()) {
        return Err(ferr(format!("unknown category '{}'", category)).with_hint(format!("categories: {}", profile::CATEGORIES.join(", "))));
    }
    let b = prof.budgets.get(&category).copied().ok_or_else(|| ferr(format!("profile '{}' has no [budget.{}]", prof.name, category)))?;
    let per_line = part.budget.per_line.unwrap_or(b.per_line);
    let per_frame = part.budget.per_frame.unwrap_or(b.per_frame);
    let overlay_line = if part.budget.per_line.is_some() { per_line } else { b.overlay_line.max(per_line) };
    let multi_max = if category == "player" { prof.multi_slot_max } else { 0 };
    Ok(Resolved { set, slots, palette, masks, category, per_line, overlay_line, per_frame, multi_max, cell: (prof.hw_cell.0 as usize, prof.hw_cell.1 as usize), extras })
}

// ------------------------------------------------------------------------------------------------ frame pixels

/// Pixel ids of a frame: 255 transparent, 254 opaque but outside the set, else the index into `Resolved::palette`.
pub struct Pix {
    pub w: usize,
    pub h: usize,
    pub ids: Vec<u8>,
    pub rgb: Vec<Option<Rgb>>,
}

pub(crate) const T: u8 = 255;
pub(crate) const OUT: u8 = 254;

/// Colours of the frame's pixels (template colours with the part's overrides, snapped to `step`).
pub fn frame_pix(part: &Part, tpl: &Template, res: &Resolved, fi: usize, step: u32) -> Pix {
    frame_pix_of(part, tpl, res, &part.frames[fi].grid, step)
}

/// [`frame_pix`] for any grid of the part's canvas (before/after an edit).
pub fn frame_pix_of(part: &Part, tpl: &Template, res: &Resolved, g: &crate::model::Grid, step: u32) -> Pix {
    let mut cache: [Option<Option<Rgb>>; 256] = [None; 256];
    let mut rgb = vec![None; g.w * g.h];
    let mut ids = vec![T; g.w * g.h];
    for (i, &k) in g.cells.iter().enumerate() {
        if k == b'.' {
            continue;
        }
        let c = *cache[k as usize].get_or_insert_with(|| crate::lint::effective_color(part, tpl, k).map(|c| color::snap_step(c, step.max(1))));
        let c = c.unwrap_or(Rgb::new(0, 0, 0));
        rgb[i] = Some(c);
        ids[i] = res.colour_bit(c).map_or(OUT, |b| b as u8);
    }
    Pix { w: g.w, h: g.h, ids, rgb }
}

// ------------------------------------------------------------------------------------------------ placement

#[derive(Clone, Debug)]
pub struct Cell {
    pub x: i32,
    pub y: i32,
    pub w: usize,
    pub h: usize,
    /// Primary slot (for an invalid cell: the slot that holds most of its colours).
    pub slot: u8,
    /// Second slot of a two-slot (overlay) cell.
    pub slot2: Option<u8>,
    pub invalid: bool,
    /// Why an invalid cell is invalid, when it is not a plain slot conflict.
    pub note: Option<String>,
    /// Opaque pixels owned by the cell.
    pub opaque: usize,
    /// In-set colours with their pixel counts.
    pub colors: Vec<(Rgb, usize)>,
    pub outside: usize,
}

impl Cell {
    /// Hardware sprites the cell costs.
    pub fn cost(&self) -> u32 {
        if self.slot2.is_some() {
            2
        } else {
            1
        }
    }
}

#[derive(Clone, Debug)]
pub struct Placement {
    pub authored: bool,
    /// Band offset of the search (None for an authored layout).
    pub oy: Option<i32>,
    pub cells: Vec<Cell>,
    /// Sprites on every scanline of the canvas.
    pub line_load: Vec<u32>,
    /// Scanlines that carry a two-slot cell.
    pub line_multi: Vec<bool>,
    pub sprites: u32,
    pub max_line: u32,
    pub invalid: usize,
    pub multi: usize,
    /// Opaque pixels no cell covers (authored layouts).
    pub uncovered: Vec<(i32, i32)>,
    /// Cell index (into `cells`) every pixel belongs to, -1 for transparent or uncovered pixels.
    pub owner: Vec<i32>,
    /// Opaque pixels that no covering cell can hold (authored layouts with overlapping cells).
    pub unholdable: Vec<(i32, i32)>,
    /// Pixels covered by more than one cell.
    pub overlap_px: usize,
}

pub(crate) const OPQ: u32 = 1 << 31;

enum Fit {
    Single(usize),
    Pair(usize, usize),
    Bad(usize),
}

fn fit(res: &Resolved, m: u32, multi_ok: bool) -> Fit {
    let m = m & !OPQ;
    if m == 0 {
        return Fit::Single(0);
    }
    if let Some(i) = res.masks.iter().position(|sm| m & !sm == 0) {
        return Fit::Single(i);
    }
    if multi_ok {
        let mut best: Option<(u32, usize, usize)> = None;
        for a in 0..res.masks.len() {
            for b in a + 1..res.masks.len() {
                if m & !(res.masks[a] | res.masks[b]) == 0 {
                    let (ca, cb) = ((m & res.masks[a]).count_ones(), (m & res.masks[b]).count_ones());
                    let (p, s, cov) = if cb > ca { (b, a, cb) } else { (a, b, ca) };
                    if best.map_or(true, |x| cov > x.0) {
                        best = Some((cov, p, s));
                    }
                }
            }
        }
        if let Some((_, p, s)) = best {
            return Fit::Pair(p, s);
        }
    }
    let mut bi = 0;
    let mut bc = 0;
    for (i, sm) in res.masks.iter().enumerate() {
        let c = (m & sm).count_ones();
        if c > bc {
            bc = c;
            bi = i;
        }
    }
    Fit::Bad(bi)
}

/// Best layout by search: band offset and cell positions (x, y).
///
/// Per band offset the bands are independent except for the per-frame allowance of two-slot cells, so a small DP over
/// the number of two-slot cells (capped just above the allowance) picks the column offset of every band; the
/// objective is (invalid cells incl. two-slot cells over the allowance, max sprites on a band, total sprites).
fn search(res: &Resolved, ids: &[u8], w: usize, h: usize) -> (i32, Vec<(i32, i32)>) {
    let multi_ok = res.multi_max > 0;
    search_by(res.cell, res.multi_max as usize, (res.per_line, res.overlay_line, res.per_frame), ids, w, h, &|m| match fit(res, m, multi_ok) {
        Fit::Single(_) => (0, 1, 0),
        Fit::Pair(..) => (0, 2, 1),
        Fit::Bad(_) => (1, 1, 0),
    })
}

/// The placement search with a pluggable cell evaluator: `eval(mask)` gives (badness, sprites, two-slot cells) of a cell
/// whose pixel ids OR to `mask` (bit 31 = opaque). Pixel ids are bit numbers (< 31); `T` = transparent, `OUT` = no bit.
pub(crate) fn search_by(cell: (usize, usize), multi_max: usize, lim: (u32, u32, u32), ids: &[u8], w: usize, h: usize, eval: &dyn Fn(u32) -> (usize, u32, usize)) -> (i32, Vec<(i32, i32)>) {
    let (cw, ch) = (cell.0 as i32, cell.1 as i32);
    let multi_ok = multi_max > 0;
    let cap = if multi_ok { multi_max + 8 } else { 0 };
    // accumulated (invalid cells, per-line excess, total sprites, max per band)
    type Acc = ((usize, u32, u32, u32), Vec<(i32, i32)>);
    // final: (invalid, limit violations, two-slot cells over the allowance, total sprites, max per line)
    let mut best: Option<((usize, u32, usize, u32, u32), i32, Vec<(i32, i32)>)> = None;
    let line_limit = |has_multi: bool| if has_multi { lim.1.max(lim.0) } else { lim.0 };
    let mut colmask = vec![0u32; w];
    for oy in 0..ch {
        let mut y0 = if oy == 0 { 0 } else { oy - ch };
        let mut state: Vec<Option<Acc>> = vec![None; cap + 1];
        state[0] = Some(((0, 0, 0, 0), Vec::new()));
        while y0 < h as i32 {
            for (x, cm) in colmask.iter_mut().enumerate() {
                let mut m = 0u32;
                for y in y0.max(0)..(y0 + ch).min(h as i32) {
                    let id = ids[y as usize * w + x];
                    if id != T {
                        m |= OPQ;
                        if id != OUT {
                            m |= 1 << id;
                        }
                    }
                }
                *cm = m;
            }
            // candidates of this band: one per column offset (invalid, sprites, two-slot cells, cell x list)
            let mut cands: Vec<(usize, u32, usize, Vec<i32>)> = Vec::new();
            for ox in 0..cw {
                let mut xs = if ox == 0 { 0 } else { ox - cw };
                let (mut i2, mut s2, mut m2) = (0usize, 0u32, 0usize);
                let mut xl: Vec<i32> = Vec::new();
                while xs < w as i32 {
                    let m = colmask[xs.max(0) as usize..(xs + cw).min(w as i32) as usize].iter().fold(0u32, |a, b| a | b);
                    if m & OPQ != 0 {
                        xl.push(xs);
                        let (b, sp, mu) = eval(m);
                        i2 += b;
                        s2 += sp;
                        m2 += mu;
                    }
                    xs += cw;
                }
                if xl.is_empty() {
                    cands.clear();
                    cands.push((0, 0, 0, xl));
                    break;
                }
                cands.push((i2, s2, m2, xl));
            }
            let mut next: Vec<Option<Acc>> = vec![None; cap + 1];
            for (si, st) in state.iter().enumerate() {
                let Some(((inv, lex, tot, maxl), cells)) = st else { continue };
                for (i2, s2, m2, xl) in &cands {
                    let ns = (si + m2).min(cap);
                    let ex = if lim.0 > 0 { s2.saturating_sub(line_limit(*m2 > 0)) } else { 0 };
                    let key = (inv + i2, lex + ex, tot + s2, (*maxl).max(*s2));
                    if next[ns].as_ref().map_or(true, |n| key < n.0) {
                        let mut c = cells.clone();
                        c.extend(xl.iter().map(|x| (*x, y0)));
                        next[ns] = Some((key, c));
                    }
                }
            }
            state = next;
            y0 += ch;
        }
        for (si, st) in state.iter().enumerate() {
            let Some(((inv, lex, tot, maxl), cells)) = st else { continue };
            let excess = si.saturating_sub(multi_max);
            let fex = if lim.2 > 0 { tot.saturating_sub(lim.2) } else { 0 };
            let key = (*inv, lex + fex, excess, *tot, *maxl);
            if best.as_ref().map_or(true, |b| key < b.0) {
                best = Some((key, oy, cells.clone()));
            }
        }
    }
    best.map_or((0, Vec::new()), |b| (b.1, b.2))
}

struct Rect {
    x: i32,
    y: i32,
    slot: Option<u8>,
    slot2: Option<u8>,
    /// Authored cells only take the pixels inside this frame rectangle (explicit assignment of overlapped pixels).
    region: Option<(i32, i32, i32, i32)>,
}

fn evaluate(res: &Resolved, pix: &Pix, rects: &[Rect], authored: bool, oy: Option<i32>) -> Placement {
    let (cw, ch) = (res.cell.0 as i32, res.cell.1 as i32);
    let (w, h) = (pix.w, pix.h);
    let mut owner: Vec<i32> = vec![-1; w * h];
    let mut cells: Vec<Cell> = Vec::new();
    let mut masks: Vec<u32> = Vec::new();
    // Each opaque pixel belongs to exactly one covering cell, even when authored cells overlap: the first one (in the
    // authored order) whose slot(s) hold the pixel's colour; a cell with an explicit `region` only takes pixels inside it.
    // A pixel no covering cell can hold goes to the first covering cell (which then reports it).
    let mut unholdable: Vec<(i32, i32)> = Vec::new();
    let mut overlap_px = 0usize;
    let holds = |r: &Rect, id: u8| -> bool {
        if id >= OUT || r.slot.is_none() {
            return true;
        }
        let bit = 1u32 << id;
        [r.slot, r.slot2].into_iter().flatten().any(|s| res.slot_index(s).map_or(false, |i| res.masks[i] & bit != 0))
    };
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let i = y as usize * w + x as usize;
            let id = pix.ids[i];
            if id == T {
                continue;
            }
            let cover: Vec<usize> = rects
                .iter()
                .enumerate()
                .filter(|(_, r)| x >= r.x && x < r.x + cw && y >= r.y && y < r.y + ch && r.region.map_or(true, |(rx, ry, rw, rh)| x >= rx && x < rx + rw && y >= ry && y < ry + rh))
                .map(|(k, _)| k)
                .collect();
            if cover.len() > 1 {
                overlap_px += 1;
            }
            if let Some(&k) = cover.iter().find(|&&k| holds(&rects[k], id)) {
                owner[i] = k as i32;
            } else if let Some(&k) = cover.first() {
                owner[i] = k as i32;
                if cover.len() > 1 {
                    unholdable.push((x, y));
                }
            }
        }
    }
    for (ri, r) in rects.iter().enumerate() {
        let mut m = 0u32;
        let mut counts = vec![0usize; res.palette.len()];
        let (mut opq, mut outside) = (0usize, 0usize);
        for y in r.y.max(0)..(r.y + ch).min(h as i32) {
            for x in r.x.max(0)..(r.x + cw).min(w as i32) {
                let i = y as usize * w + x as usize;
                let id = pix.ids[i];
                if id == T || owner[i] != ri as i32 {
                    continue;
                }
                opq += 1;
                m |= OPQ;
                if id == OUT {
                    outside += 1;
                } else {
                    m |= 1 << id;
                    counts[id as usize] += 1;
                }
            }
        }
        masks.push(m);
        let colors: Vec<(Rgb, usize)> = counts.iter().enumerate().filter(|(_, n)| **n > 0).map(|(i, n)| (res.palette[i], *n)).collect();
        let mut c = Cell { x: r.x, y: r.y, w: cw as usize, h: ch as usize, slot: res.slots[0].0, slot2: None, invalid: false, note: None, opaque: opq, colors, outside };
        let bits = m & !OPQ;
        let given = r.slot.map(|s| res.slot_index(s));
        match given {
            Some(None) => {
                c.slot = r.slot.unwrap();
                c.invalid = true;
                c.note = Some(format!("slot {} is not an allowed slot of this part", c.slot));
            }
            Some(Some(i)) => {
                c.slot = res.slots[i].0;
                if bits & !res.masks[i] == 0 {
                    // single slot as authored
                } else if let Some(j) = r.slot2.and_then(|s| res.slot_index(s)) {
                    if bits & !(res.masks[i] | res.masks[j]) == 0 && res.multi_max > 0 {
                        c.slot2 = Some(res.slots[j].0);
                    } else {
                        c.invalid = true;
                    }
                } else {
                    c.invalid = true;
                }
            }
            None => match fit(res, m, res.multi_max > 0) {
                Fit::Single(i) => c.slot = res.slots[i].0,
                Fit::Pair(a, b) => {
                    c.slot = res.slots[a].0;
                    c.slot2 = Some(res.slots[b].0);
                }
                Fit::Bad(i) => {
                    c.slot = res.slots[i].0;
                    c.invalid = true;
                }
            },
        }
        cells.push(c);
    }
    // two-slot cells beyond the per-frame allowance become invalid (the ones with the fewest second-slot pixels)
    let mut pairs: Vec<(usize, usize)> = cells
        .iter()
        .enumerate()
        .filter(|(_, c)| c.slot2.is_some())
        .map(|(i, c)| {
            let pi = res.slot_index(c.slot).unwrap();
            let sec: usize = c.colors.iter().filter(|(col, _)| res.masks[pi] & (1 << res.colour_bit(*col).unwrap()) == 0).map(|x| x.1).sum();
            (i, sec)
        })
        .collect();
    if pairs.len() > res.multi_max as usize {
        pairs.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        for &(i, _) in &pairs[res.multi_max as usize..] {
            cells[i].slot2 = None;
            cells[i].invalid = true;
            cells[i].note = Some(format!("needs two slots but the frame already has {} such cells (at most {})", res.multi_max, res.multi_max));
        }
    }
    let mut uncovered = Vec::new();
    for i in 0..w * h {
        if pix.ids[i] != T && owner[i] < 0 {
            uncovered.push(((i % w) as i32, (i / w) as i32));
        }
    }
    let mut line_load = vec![0u32; h];
    let mut line_multi = vec![false; h];
    for c in &cells {
        for y in c.y.max(0)..(c.y + ch).min(h as i32) {
            line_load[y as usize] += c.cost();
            line_multi[y as usize] |= c.slot2.is_some();
        }
    }
    let sprites = cells.iter().map(|c| c.cost()).sum();
    let max_line = line_load.iter().copied().max().unwrap_or(0);
    let invalid = cells.iter().filter(|c| c.invalid).count();
    let multi = cells.iter().filter(|c| c.slot2.is_some()).count();
    Placement { authored, oy, cells, line_load, line_multi, sprites, max_line, invalid, multi, uncovered, owner, unholdable, overlap_px }
}

/// Pixel ids for colours already chosen (after a remap).
pub fn pix_from_rgb(res: &Resolved, w: usize, h: usize, rgb: Vec<Option<Rgb>>) -> Pix {
    let ids = rgb.iter().map(|c| c.map_or(T, |c| res.colour_bit(c).map_or(OUT, |b| b as u8))).collect();
    Pix { w, h, ids, rgb }
}

/// Path of the authored layout of a frame: `<part folder>/<frame>.cells.json`.
pub fn sidecar_path(part: &Part, frame: &str) -> PathBuf {
    part.dir().join(format!("{}.cells.json", frame))
}

fn read_sidecar(part: &Part, frame: &str) -> Option<std::result::Result<Vec<Rect>, String>> {
    let p = sidecar_path(part, frame);
    if !p.is_file() {
        return None;
    }
    let parse = || -> std::result::Result<Vec<Rect>, String> {
        let text = std::fs::read_to_string(&p).map_err(|e| format!("cannot read {}: {}", p.slash(), e))?;
        let v: Value = serde_json::from_str(&text).map_err(|e| format!("{} is not valid JSON: {}", p.slash(), e))?;
        let a = v.as_array().ok_or_else(|| format!("{} must be a list: [{{\"x\":-8,\"y\":-16,\"slot\":0}}, ..]", p.slash()))?;
        let mut out = Vec::new();
        for (i, e) in a.iter().enumerate() {
            let get = |k: &str| e.get(k).and_then(|x| x.as_i64());
            let (Some(x), Some(y)) = (get("x"), get("y")) else {
                return Err(format!("{} entry {}: needs integer `x` and `y` (relative to the origin)", p.slash(), i));
            };
            let slot = |k: &str| get(k).filter(|s| (0..8).contains(s)).map(|s| s as u8);
            let region = match e.get("region").and_then(|r| r.as_array()) {
                Some(r) if r.len() == 4 && r.iter().all(|v| v.is_i64()) => {
                    let n: Vec<i32> = r.iter().map(|v| v.as_i64().unwrap() as i32).collect();
                    Some((n[0] + part.origin.0, n[1] + part.origin.1, n[2], n[3]))
                }
                Some(_) => return Err(format!("{} entry {}: `region` must be [x, y, w, h] relative to the origin", p.slash(), i)),
                None => None,
            };
            out.push(Rect { x: x as i32 + part.origin.0, y: y as i32 + part.origin.1, slot: slot("slot"), slot2: slot("slot2"), region });
        }
        Ok(out)
    };
    Some(parse())
}

/// Placement of one frame: the authored `<frame>.cells.json` if present, else the best searched layout.
pub fn place_frame(part: &Part, res: &Resolved, pix: &Pix, fi: usize) -> std::result::Result<Placement, String> {
    let name = &part.frames[fi].name;
    if let Some(r) = read_sidecar(part, name) {
        let rects = r?;
        return Ok(evaluate(res, pix, &rects, true, None));
    }
    let (oy, pos) = search(res, &pix.ids, pix.w, pix.h);
    let rects: Vec<Rect> = pos.into_iter().map(|(x, y)| Rect { x, y, slot: None, slot2: None, region: None }).collect();
    Ok(evaluate(res, pix, &rects, false, Some(oy)))
}

// ------------------------------------------------------------------------------------------------ findings (lint)

pub struct Finding {
    pub rule: &'static str,
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub msg: String,
    pub fix: Option<String>,
}

fn hexs(cols: &[(Rgb, usize)]) -> String {
    cols.iter().map(|(c, _)| color::to_hex(*c)).collect::<Vec<_>>().join(" ")
}

/// How to repair an invalid cell: the slot that holds most of its colours and the colour to fold into it.
pub fn fold_hint(res: &Resolved, c: &Cell) -> String {
    let Some(si) = res.slot_index(c.slot) else { return String::new() };
    let have = res.slots[si].2;
    let missing: Vec<(Rgb, usize)> = c.colors.iter().filter(|(col, _)| !have.contains(col)).cloned().collect();
    let held = c.colors.len() - missing.len();
    let mut s = format!("best slot {} holds {} of {} colours", res.slot_label(c.slot), held, c.colors.len());
    if let Some(&(m, n)) = missing.iter().min_by_key(|x| x.1) {
        let o = color::srgb_to_oklab(m);
        let near = have.iter().min_by(|a, b| color::oklab_dist2(o, color::srgb_to_oklab(**a)).partial_cmp(&color::oklab_dist2(o, color::srgb_to_oklab(**b))).unwrap()).copied();
        s.push_str(&format!("; fold {} ({} px) into {}", color::to_hex(m), n, near.map_or("a slot colour".into(), color::to_hex)));
        if missing.len() > 1 {
            s.push_str(&format!(" (still {} colour(s) outside the slot, then fold those too)", missing.len() - 1));
        }
    }
    s
}

/// `palette_set`, `cell_palette` and `hw_limit` issues of one frame.
pub fn frame_findings(part: &Part, tpl: &Template, res: &Resolved, prof: &Profile, fi: usize, enabled: &dyn Fn(&str) -> bool) -> Vec<Finding> {
    let mut out = Vec::new();
    let f = &part.frames[fi];
    let g = &f.grid;
    let pix = frame_pix(part, tpl, res, fi, prof.channel_step);
    let allowed: Vec<String> = res.slots.iter().map(|s| s.0.to_string()).collect();
    if enabled("palette_set") {
        let mut seen: Vec<(Rgb, usize, usize)> = Vec::new(); // colour, first index, count
        for (i, id) in pix.ids.iter().enumerate() {
            if *id == OUT {
                let c = pix.rgb[i].unwrap();
                match seen.iter_mut().find(|e| e.0 == c) {
                    Some(e) => e.2 += 1,
                    None => seen.push((c, i, 1)),
                }
            }
        }
        for (c, i, n) in seen {
            let near = res.nearest(c).map_or(String::new(), |(s, k)| format!("; nearest set colour {} in slot {}", color::to_hex(k), res.slot_label(s)));
            out.push(Finding {
                rule: "palette_set",
                x: Some((i % g.w) as i32),
                y: Some((i / g.w) as i32),
                msg: format!("colour {} (key '{}', {} px) is not in palette set '{}' (slots {}){}", color::to_hex(c), g.cells[i] as char, n, res.set.name, allowed.join(","), near),
                fix: Some(format!("change the template colour of key '{}' to a set colour, or add the colour to {}", g.cells[i] as char, res.set.path.slash())),
            });
        }
    }
    let want_cells = enabled("cell_palette");
    let want_limit = enabled("hw_limit");
    if !want_cells && !want_limit && !enabled("stacked") {
        return out;
    }
    let pl = match place_frame(part, res, &pix, fi) {
        Ok(p) => p,
        Err(e) => {
            if want_cells {
                out.push(Finding { rule: "cell_palette", x: None, y: None, msg: format!("frame '{}': {}", f.name, e), fix: None });
            }
            return out;
        }
    };
    if want_cells {
        if let Some(&(x, y)) = pl.uncovered.first() {
            out.push(Finding {
                rule: "cell_palette",
                x: Some(x),
                y: Some(y),
                msg: format!("frame '{}': {} opaque pixel(s) are not covered by {}.cells.json (first at {},{}); every opaque pixel must sit in a cell", f.name, pl.uncovered.len(), f.name, x, y),
                fix: Some(format!("add the missing cells to {} or delete the file to use the automatic placement", sidecar_path(part, &f.name).slash())),
            });
        }
        if let Some(&(x, y)) = pl.unholdable.first() {
            let col = pix.rgb[y as usize * pix.w + x as usize].map(color::to_hex).unwrap_or_default();
            out.push(Finding {
                rule: "cell_palette",
                x: Some(x),
                y: Some(y),
                msg: format!("frame '{}': {} opaque pixel(s) cannot be held by any cell that covers them (first at {},{} colour {}); give a covering cell the slot of that colour", f.name, pl.unholdable.len(), x, y, col),
                fix: None,
            });
        }
        for c in pl.cells.iter().filter(|c| c.invalid) {
            let body = match &c.note {
                Some(n) => n.clone(),
                None => fold_hint(res, c),
            };
            out.push(Finding {
                rule: "cell_palette",
                x: Some(c.x),
                y: Some(c.y),
                msg: format!("frame '{}': cell {},{} ({}x{}) cannot be drawn with one palette slot: colours {}; {}", f.name, c.x, c.y, c.w, c.h, hexs(&c.colors), body),
                fix: None,
            });
        }
    }
    // stacked = two cells that sit mostly on top of each other (an overlap of at least half a cell); cells that merely overlap at the edge
    // share pixels by ownership and are what the client's verifier accepts
    let stacked_pair = |a: &Cell, b: &Cell| -> bool {
        let ox = (a.x + a.w as i32).min(b.x + b.w as i32) - a.x.max(b.x);
        let oy = (a.y + a.h as i32).min(b.y + b.h as i32) - a.y.max(b.y);
        ox > 0 && oy > 0 && (ox * oy) as usize * 2 >= a.w * a.h
    };
    let has_stack = pl.authored && pl.overlap_px > 0 && (0..pl.cells.len()).any(|i| (i + 1..pl.cells.len()).any(|j| stacked_pair(&pl.cells[i], &pl.cells[j])));
    if enabled("stacked") && has_stack && !prof.overlay_categories.iter().any(|c| *c == res.category) {
        // stacked cells outside the categories that may use them must survive a flatten: every covering cell holds the pixel's colour
        let mut lost: Vec<(i32, i32, Rgb, u8)> = Vec::new();
        for y in 0..pix.h {
            for x in 0..pix.w {
                let Some(c) = pix.rgb[y * pix.w + x] else { continue };
                let cov: Vec<&Cell> = pl.cells.iter().enumerate().filter(|(i, k)| (x as i32) >= k.x && (x as i32) < k.x + k.w as i32 && (y as i32) >= k.y && (y as i32) < k.y + k.h as i32 && (0..pl.cells.len()).any(|j| j != *i && stacked_pair(k, &pl.cells[j]))).map(|(_, k)| k).collect();
                if cov.len() < 2 {
                    continue;
                }
                for k in cov {
                    let holds = [Some(k.slot), k.slot2].into_iter().flatten().any(|s| res.slot_index(s).map_or(false, |i| res.slots[i].2.contains(&c)));
                    if !holds {
                        lost.push((x as i32, y as i32, c, k.slot));
                        break;
                    }
                }
            }
        }
        if let Some(&(x, y, c, s)) = lost.first() {
            out.push(Finding {
                rule: "stacked",
                x: Some(x),
                y: Some(y),
                msg: format!("frame '{}': stacked cells outside the categories that may overlap them ({}): {} pixel(s) lose their colour when the cells are flattened (first at {},{} colour {} is not in slot {} of a covering cell)", f.name, if prof.overlay_categories.is_empty() { "none".to_string() } else { prof.overlay_categories.join(", ") }, lost.len(), x, y, color::to_hex(c), s),
                fix: Some("avoid overlapping cells, or give every covering cell a slot that holds the colour".into()),
            });
        }
    }
    if want_cells {
        // multi-set parts: the shared slot index must keep a cell's colours apart in every other set
        for e in &res.extras {
            for c in pl.cells.iter().filter(|c| !c.invalid) {
                for sl in [Some(c.slot), c.slot2].into_iter().flatten() {
                    let Some(si) = res.slot_index(sl) else { continue };
                    let mut used: Vec<(Rgb, Rgb)> = Vec::new(); // (colour here, colour in the other set)
                    for (col, _) in &c.colors {
                        if let Some(pos) = res.slots[si].2.iter().position(|k| k == col) {
                            used.push((*col, e.cols[si][pos]));
                        }
                    }
                    for i in 0..used.len() {
                        for j in i + 1..used.len() {
                            if used[i].1 == used[j].1 {
                                out.push(Finding {
                                    rule: "cell_palette",
                                    x: Some(c.x),
                                    y: Some(c.y),
                                    msg: format!("frame '{}': cell {},{} slot {}: in palette set '{}' the colours {} and {} are both {}, so the cell loses detail there", f.name, c.x, c.y, sl, e.name, color::to_hex(used[i].0), color::to_hex(used[j].0), color::to_hex(used[i].1)),
                                    fix: None,
                                });
                            }
                        }
                    }
                }
            }
        }
    }
    if want_limit {
        let how = if pl.authored { "the authored cells".to_string() } else { format!("the best placement (band offset {})", pl.oy.unwrap_or(0)) };
        let split = format!("{} cells, {} with two slots", pl.cells.len(), pl.multi);
        if res.per_frame > 0 && pl.sprites > res.per_frame {
            out.push(Finding {
                rule: "hw_limit",
                x: None,
                y: None,
                msg: format!("frame '{}' needs {} sprites ({}) in {}, category '{}' allows {} per frame; hint: pack the figure into fewer cells, split it across objects or frames, or narrow it", f.name, pl.sprites, split, how, res.category, res.per_frame),
                fix: None,
            });
        }
        if res.per_line > 0 {
            let limit = |y: usize| if pl.line_multi[y] { res.overlay_line } else { res.per_line };
            let mut y = 0;
            while y < pl.line_load.len() {
                if pl.line_load[y] > limit(y) {
                    let (l, lim) = (pl.line_load[y], limit(y));
                    let y0 = y;
                    while y + 1 < pl.line_load.len() && pl.line_load[y + 1] == l && limit(y + 1) == lim {
                        y += 1;
                    }
                    out.push(Finding {
                        rule: "hw_limit",
                        x: None,
                        y: Some(y0 as i32),
                        msg: format!("frame '{}': scanlines y={}..{} carry {} sprites ({}) in {}, category '{}' allows {} per line; hint: make the figure narrower on those rows or move cells to another band", f.name, y0, y, l, split, how, res.category, lim),
                        fix: None,
                    });
                }
                y += 1;
            }
        }
    }
    out
}

/// Part level `palette_set` issue when the set cannot be resolved; else the resolved set.
pub fn resolve_for_lint(part: &Part, prof: &Profile) -> std::result::Result<Option<Resolved>, String> {
    if part.palette_set.is_none() {
        return Ok(None);
    }
    resolve(part, prof).map(Some).map_err(|e| e.msg)
}

// ------------------------------------------------------------------------------------------------ export

/// 32-entry palette of a resolved set: entry `slot*4` transparent, `slot*4 + 1..3` the slot colours in set order.
/// Slots the part does not use are black. Also the tRNS table.
pub fn palette32(res: &Resolved) -> (Vec<Rgb>, Vec<u8>) {
    let mut pal = vec![Rgb::new(0, 0, 0); 32];
    for (s, _, cols) in &res.slots {
        for (i, c) in cols.iter().enumerate() {
            pal[*s as usize * 4 + 1 + i] = *c;
        }
    }
    let mut alpha = vec![255u8; 29];
    for s in 0..8 {
        alpha[s * 4] = 0;
    }
    alpha.truncate(29);
    (pal, alpha)
}

pub struct FrameOut {
    pub base: Vec<u8>,
    /// Pixels of the second slot of two-slot cells (the player's overlay cells).
    pub detail: Option<Vec<u8>>,
    pub cells_json: String,
    pub placement: Placement,
}

fn slot_index_of(res: &Resolved, slot: u8, c: Rgb) -> Option<u8> {
    let i = res.slot_index(slot)?;
    res.slots[i].2.iter().position(|k| *k == c).map(|p| slot * 4 + 1 + p as u8)
}

/// Indexed pixels, cell list and placement of a frame for `export`. Errors name the frame, cell and colours.
pub fn export_frame(part: &Part, tpl: &Template, res: &Resolved, prof: &Profile, fi: usize) -> Result<FrameOut> {
    let f = &part.frames[fi];
    let loc = Loc::file(part.path.slash()).frame(f.name.clone());
    let pix = frame_pix(part, tpl, res, fi, prof.channel_step);
    let pl = place_frame(part, res, &pix, fi).map_err(|e| PxlError::validation(e).with_loc(loc.clone()))?;
    if let Some(i) = pix.ids.iter().position(|id| *id == OUT) {
        return Err(PxlError::validation(format!("colour {} at {},{} is not in palette set '{}'", color::to_hex(pix.rgb[i].unwrap()), i % pix.w, i / pix.w, res.set.name))
            .with_loc(loc)
            .with_hint("run `pxl lint` (rule palette_set) for the nearest set colour"));
    }
    if let Some(&(x, y)) = pl.uncovered.first() {
        return Err(PxlError::validation(format!("opaque pixel {},{} is not covered by {}.cells.json", x, y, f.name)).with_loc(loc));
    }
    if let Some(c) = pl.cells.iter().find(|c| c.invalid) {
        return Err(PxlError::validation(format!("cell {},{} cannot be drawn with one palette slot: colours {}; {}", c.x, c.y, hexs(&c.colors), fold_hint(res, c)))
            .with_loc(loc)
            .with_hint("run `pxl cells` for the layout, or `pxl lint` (rule cell_palette)"));
    }
    let (w, h) = (pix.w, pix.h);
    let mut base = vec![0u8; w * h];
    let mut detail = vec![0u8; w * h];
    let mut any_detail = false;
    for i in 0..w * h {
        let oi = pl.owner[i];
        if oi < 0 {
            continue;
        }
        let c = &pl.cells[oi as usize];
        let (x, y) = (i % w, i / w);
        let Some(col) = pix.rgb[i] else { continue };
        if let Some(ix) = slot_index_of(res, c.slot, col) {
            base[i] = ix;
        } else if let Some(ix) = c.slot2.and_then(|s2| slot_index_of(res, s2, col)) {
            detail[i] = ix;
            any_detail = true;
        } else {
            return Err(PxlError::validation(format!("colour {} at {},{} is in neither slot of its cell", color::to_hex(col), x, y)).with_loc(loc));
        }
    }
    let cells_json = cells_json_text(part, &pl);
    Ok(FrameOut { base, detail: any_detail.then_some(detail), cells_json, placement: pl })
}

/// The `<frame>.cells.json` text of a placement (x, y relative to the origin; searched layouts sorted by row then column,
/// authored ones keep their order because it decides overlapped pixels).
pub fn cells_json_text(part: &Part, pl: &Placement) -> String {
    let mut cj: Vec<&Cell> = pl.cells.iter().collect();
    if !pl.authored {
        cj.sort_by_key(|c| (c.y, c.x));
    }
    let lines: Vec<String> = cj
        .iter()
        .map(|c| match c.slot2 {
            Some(s2) => format!("  {{\"x\": {}, \"y\": {}, \"slot\": {}, \"slot2\": {}}}", c.x - part.origin.0, c.y - part.origin.1, c.slot, s2),
            None => format!("  {{\"x\": {}, \"y\": {}, \"slot\": {}}}", c.x - part.origin.0, c.y - part.origin.1, c.slot),
        })
        .collect();
    if lines.is_empty() { "[]\n".to_string() } else { format!("[\n{}\n]\n", lines.join(",\n")) }
}

/// File of the cell list that goes with a frame image.
pub fn cells_json_path(png: &Path) -> PathBuf {
    png.with_extension("cells.json")
}

/// Everything the export of a palette-set part declares, in frame order: (frame, kind, path) with kind 0 = image,
/// 1 = detail image, 2 = cells json. Needs the placements (a detail image exists only with two-slot cells).
pub fn expected_outputs(part: &Part, tpl: &Template, prof: &Profile, spec: &crate::manifest::OutSpec) -> Option<Vec<PathBuf>> {
    let res = resolve(part, prof).ok()?;
    let mut v = Vec::new();
    for (fi, f) in part.frames.iter().enumerate() {
        let png = spec.file_path(&f.name, 0, "base");
        v.push(png.clone());
        v.push(cells_json_path(&png));
        let pix = frame_pix(part, tpl, &res, fi, prof.channel_step);
        if let Ok(pl) = place_frame(part, &res, &pix, fi) {
            if pl.multi > 0 {
                v.push(spec.file_path(&f.name, 1, "detail"));
            }
        }
    }
    v.push(spec.ase.clone());
    Some(v)
}

/// Extra text for the export config hash: palette set files and authored layouts. Empty for other parts.
pub fn config_extra(part: &Part) -> String {
    let Some(name) = &part.palette_set else { return String::new() };
    let mut s = format!(";pset={}", name);
    if let Ok(set) = load_set(name, &part.dir()) {
        for f in &set.files {
            s.push_str(&format!(";{}", fsutil::content_hash(&std::fs::read(f).unwrap_or_default())));
        }
    }
    for f in &part.frames {
        if let Ok(b) = std::fs::read(sidecar_path(part, &f.name)) {
            s.push_str(&format!(";c{}={}", f.name, fsutil::content_hash(&b)));
        }
    }
    s
}

// ------------------------------------------------------------------------------------------------ pxl cells

const SLOT_RGB: [(u8, u8, u8); 8] = [(80, 160, 255), (255, 120, 80), (255, 210, 60), (150, 150, 170), (190, 90, 255), (60, 220, 120), (60, 220, 220), (255, 130, 220)];

fn run_lines(load: &[u32]) -> String {
    let mut parts = Vec::new();
    let mut y = 0;
    while y < load.len() {
        let l = load[y];
        let y0 = y;
        while y + 1 < load.len() && load[y + 1] == l {
            y += 1;
        }
        if l > 0 {
            parts.push(if y0 == y { format!("y{}:{}", y0, l) } else { format!("y{}-{}:{}", y0, y, l) });
        }
        y += 1;
    }
    parts.join(" ")
}

pub fn run_cells(a: &crate::cli::CellsShowArgs, out: &mut Out) -> Result<()> {
    let path = fsutil::resolve_part(&a.part)?;
    let part = crate::pxl::load(&path)?;
    if part.palette_set.is_none() {
        let msg = format!("{} has no `palette_set`: cells are only derived for palette-set parts (add palette_set = \"<name>\" to the header)", path.slash());
        out.line(msg.clone());
        out.set("palette_set", Value::Null);
        out.warn(msg);
        return Ok(());
    }
    let prof = profile::load_for(&part, None)?;
    let res = resolve(&part, &prof)?;
    let tpl = crate::template::load_for(&part)?;
    let frames: Vec<usize> = match &a.frame {
        Some(n) => vec![part.frame_index(n).ok_or_else(|| PxlError::usage(format!("{}: no frame '{}'", path.slash(), n)))?],
        None => (0..part.frames.len()).collect(),
    };
    let recol = match &a.set {
        Some(sn) if a.png.is_some() && part.palette_set.as_deref() != Some(sn.as_str()) => Some(recoloured_grids(&part, &tpl, &res, &prof, sn)?),
        Some(sn) if !res.all_sets().iter().any(|(n, _)| n == sn) => return Err(PxlError::usage(format!("'{}' is not one of the sets of {}: {}", sn, part.name, part.palette_sets().join(", ")))),
        _ => None,
    };
    let mut jf = Vec::new();
    let mut bad = 0usize;
    out.line(format!("{}: palette set '{}', category '{}' (limits {} per line, {} per frame{}), cells {}x{}", part.name, res.set.name, res.category, res.per_line, res.per_frame, if res.overlay_line > res.per_line { format!(", {} on lines with a two-slot cell", res.overlay_line) } else { String::new() }, res.cell.0, res.cell.1));
    for &fi in &frames {
        let f = &part.frames[fi];
        let pix = frame_pix(&part, &tpl, &res, fi, prof.channel_step);
        let pl = place_frame(&part, &res, &pix, fi).map_err(|e| PxlError::validation(e).with_loc(Loc::file(path.slash()).frame(f.name.clone())))?;
        bad += pl.invalid + pl.uncovered.len().min(1);
        let how = if pl.authored { "authored cells".to_string() } else { format!("band offset oy={}", pl.oy.unwrap_or(0)) };
        out.line(format!("frame {}: {} cells, {} sprites, max {} per line, {} invalid ({})", f.name, pl.cells.len(), pl.sprites, pl.max_line, pl.invalid, how));
        let mut cells: Vec<&Cell> = pl.cells.iter().collect();
        cells.sort_by_key(|c| (c.y, c.x));
        for c in &cells {
            let st = if c.invalid { format!("INVALID {}", c.note.clone().unwrap_or_else(|| fold_hint(&res, c))) } else if let Some(s2) = c.slot2 { format!("two slots: {} + {} (2 sprites)", c.slot, s2) } else { "ok".into() };
            out.line(format!("  cell {:>4},{:>4}  slot {}  {} colour(s) {}  {}", c.x - part.origin.0, c.y - part.origin.1, res.slot_label(c.slot), c.colors.len(), hexs(&c.colors), st));
        }
        for &(x, y) in pl.uncovered.iter().take(3) {
            out.line(format!("  uncovered opaque pixel {},{}", x, y));
        }
        if pl.overlap_px > 0 {
            out.line(format!("  {} px are covered by more than one cell (each takes the first covering cell that holds its colour); overlapping cells all cost sprites on their lines", pl.overlap_px));
        }
        out.line(format!("  line load: {}", run_lines(&pl.line_load)));
        jf.push(json!({
            "frame": f.name,
            "authored": pl.authored,
            "band_offset": pl.oy,
            "sprites": pl.sprites,
            "max_per_line": pl.max_line,
            "invalid": pl.invalid,
            "two_slot_cells": pl.multi,
            "overlap_px": pl.overlap_px,
            "uncovered": pl.uncovered.len(),
            "cells": cells.iter().map(|c| json!({
                "x": c.x - part.origin.0, "y": c.y - part.origin.1, "frame_x": c.x, "frame_y": c.y, "w": c.w, "h": c.h,
                "slot": c.slot, "slot2": c.slot2, "sprites": c.cost(), "valid": !c.invalid,
                "colors": c.colors.iter().map(|(k, n)| json!({"color": color::to_hex(*k), "pixels": n})).collect::<Vec<_>>(),
                "fix": if c.invalid { Some(c.note.clone().unwrap_or_else(|| fold_hint(&res, c))) } else { None },
            })).collect::<Vec<_>>(),
            "lines": pl.line_load,
        }));
        if let Some(p) = &a.png {
            let (mut x0, mut y0, mut x1, mut y1) = (0i32, 0i32, pix.w as i32, pix.h as i32);
            for c in &pl.cells {
                x0 = x0.min(c.x);
                y0 = y0.min(c.y);
                x1 = x1.max(c.x + c.w as i32);
                y1 = y1.max(c.y + c.h as i32);
            }
            let marks: Vec<((i32, i32, usize, usize), crate::img::Rgba)> = pl
                .cells
                .iter()
                .map(|c| {
                    let col = if c.invalid { crate::img::Rgba::opaque(255, 0, 0) } else { let k = SLOT_RGB[c.slot as usize % 8]; crate::img::Rgba::opaque(k.0, k.1, k.2) };
                    ((c.x, c.y, c.w, c.h), col)
                })
                .collect();
            let mut marks = marks;
            for (a, ca) in pl.cells.iter().enumerate() {
                for cb in pl.cells.iter().skip(a + 1) {
                    let (ix0, iy0) = (ca.x.max(cb.x), ca.y.max(cb.y));
                    let (ix1, iy1) = ((ca.x + ca.w as i32).min(cb.x + cb.w as i32), (ca.y + ca.h as i32).min(cb.y + cb.h as i32));
                    if ix1 > ix0 && iy1 > iy0 {
                        marks.push(((ix0, iy0, (ix1 - ix0) as usize, (iy1 - iy0) as usize), crate::img::Rgba::opaque(255, 255, 255)));
                    }
                }
            }
            let (im, _) = match &recol {
                Some((t2, gs)) => crate::show::render_panels_marked(&part, Some(t2), None, &[(format!("{} ({})", f.name, a.set.clone().unwrap_or_default()), &gs[fi])], (x0, y0, (x1 - x0) as usize, (y1 - y0) as usize), &marks, a.zoom),
                None => crate::show::render_panels_marked(&part, Some(&tpl), None, &[(f.name.clone(), &f.grid)], (x0, y0, (x1 - x0) as usize, (y1 - y0) as usize), &marks, a.zoom),
            };
            let target = if frames.len() > 1 {
                let pp = Path::new(p);
                let stem = pp.file_stem().and_then(|s| s.to_str()).unwrap_or("cells");
                pp.with_file_name(format!("{}-{}.png", stem, f.name))
            } else {
                PathBuf::from(p)
            };
            crate::show::write_png(&target, &im)?;
            out.line(format!("  wrote {}", target.slash()));
        }
    }
    out.set("schema", json!(1));
    out.set("part", json!(part.name));
    out.set("palette_set", json!(res.set.name));
    out.set("category", json!(res.category));
    out.set("limits", json!({"per_line": res.per_line, "overlay_line": res.overlay_line, "per_frame": res.per_frame}));
    out.set("frames", Value::Array(jf));
    if bad > 0 {
        out.warn(format!("{} problem(s) in the cell layout; `pxl lint` names each one", bad));
    }
    Ok(())
}

// ------------------------------------------------------------------------------------------------ other sets

/// Every pixel's colour in each set of the part (the authoring set first), from the slots of `pl`.
pub fn colours_in_sets(res: &Resolved, pix: &Pix, pl: &Placement) -> Vec<Vec<Option<Rgb>>> {
    let sets = res.all_sets();
    let mut out: Vec<Vec<Option<Rgb>>> = vec![vec![None; pix.w * pix.h]; sets.len()];
    for i in 0..pix.w * pix.h {
        let oi = pl.owner[i];
        if oi < 0 {
            continue;
        }
        let c = &pl.cells[oi as usize];
        let Some(col) = pix.rgb[i] else { continue };
        for sl in [Some(c.slot), c.slot2].into_iter().flatten() {
            let Some(si) = res.slot_index(sl) else { continue };
            if let Some(pos) = res.slots[si].2.iter().position(|k| *k == col) {
                for (k, (_, cols)) in sets.iter().enumerate() {
                    out[k][i] = Some(cols[si][pos]);
                }
                break;
            }
        }
    }
    out
}

/// The frames of a multi-set part drawn in the colours of another of its sets: a template of the colours that set gives
/// the cells' slot indexes, and one grid per frame (`--set` of `compare` and `cells --png`).
pub fn recoloured_grids(part: &Part, tpl: &Template, res: &Resolved, prof: &Profile, set: &str) -> Result<(Template, Vec<crate::model::Grid>)> {
    let Some(si_set) = res.all_sets().iter().position(|(n, _)| n == set) else {
        return Err(PxlError::usage(format!("'{}' is not one of the sets of {}: {}", set, part.name, part.palette_sets().join(", "))));
    };
    let cols_of: Vec<[Rgb; 3]> = res.all_sets()[si_set].1.clone();
    let mut t = Template::empty(&format!("{}@{}", tpl.name, set));
    let mut keys: Vec<(Rgb, u8)> = Vec::new();
    let mut layer = crate::template::Layer { name: "base".into(), keys: Vec::new() };
    let pool = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789#$%&*+-/:;<=>?@^_~";
    let mut key_of = |c: Rgb, layer: &mut crate::template::Layer| -> u8 {
        if let Some((_, k)) = keys.iter().find(|(x, _)| *x == c) {
            return *k;
        }
        let k = pool.as_bytes()[keys.len() % pool.len()];
        keys.push((c, k));
        layer.keys.push(crate::template::KeyDef { key: k, role: format!("c_{}", color::to_hex(c).trim_start_matches('#')), color: Some(c), under: None });
        k
    };
    let mut grids = Vec::new();
    for fi in 0..part.frames.len() {
        let fo = export_frame(part, tpl, res, prof, fi)?;
        let g0 = &part.frames[fi].grid;
        let mut g = crate::model::Grid::new(g0.w, g0.h, b'.');
        for i in 0..g.cells.len() {
            let v = if fo.base[i] != 0 { fo.base[i] } else { fo.detail.as_ref().map_or(0, |d| d[i]) };
            if v == 0 {
                continue;
            }
            let (slot, pos) = ((v - 1) / 4, ((v - 1) % 4) as usize);
            let Some(si) = res.slot_index(slot) else { continue };
            g.cells[i] = key_of(cols_of[si][pos], &mut layer);
        }
        grids.push(g);
    }
    t.layers.push(layer);
    Ok((t, grids))
}

/// `part` and `tpl` redrawn in the colours of `set` (the part unchanged when it is the authoring set).
pub fn recolour_part(part: Part, tpl: Template, set: &str) -> Result<(Part, Template)> {
    if part.palette_set.is_none() || part.palette_set.as_deref() == Some(set) {
        return Ok((part, tpl));
    }
    let prof = profile::load_for(&part, None)?;
    let res = resolve(&part, &prof)?;
    let (t, grids) = recoloured_grids(&part, &tpl, &res, &prof, set)?;
    let mut p = part;
    for (f, g) in p.frames.iter_mut().zip(grids) {
        f.grid = g;
    }
    p.colors.clear();
    Ok((p, t))
}

// ------------------------------------------------------------------------------------------------ editing feedback

/// Warnings after an edit of a palette-set part: cells that now need a second slot (or are now invalid).
pub fn edit_warnings(part: &Part, tpl: &Template, before: &crate::model::Grid, after: &crate::model::Grid, fi: usize) -> Vec<String> {
    let Ok(prof) = profile::load_for(part, None) else { return Vec::new() };
    let Ok(res) = resolve(part, &prof) else { return Vec::new() };
    let f = &part.frames[fi];
    let step = prof.channel_step;
    let pb = frame_pix_of(part, tpl, &res, before, step);
    let pa = frame_pix_of(part, tpl, &res, after, step);
    let (Ok(b), Ok(a)) = (place_frame(part, &res, &pb, fi), place_frame(part, &res, &pa, fi)) else { return Vec::new() };
    let mut out = Vec::new();
    if a.invalid > b.invalid {
        for c in a.cells.iter().filter(|c| c.invalid).take(3) {
            out.push(format!("warning: frame {} cell {},{} cannot be drawn with one palette slot now: colours {}; {}", f.name, c.x - part.origin.0, c.y - part.origin.1, hexs(&c.colors), c.note.clone().unwrap_or_else(|| fold_hint(&res, c))));
        }
    }
    if a.multi > b.multi {
        out.push(format!("warning: frame {}: {} cell(s) now need a second slot (each costs 2 sprites; {} allowed for category '{}')", f.name, a.multi, res.multi_max, res.category));
    }
    if a.max_line > b.max_line && a.max_line > res.per_line.max(res.overlay_line) {
        out.push(format!("warning: frame {}: {} sprites on one scanline now, category '{}' allows {}", f.name, a.max_line, res.category, res.per_line));
    }
    out
}

/// `pxl show --cells`: the cell layout of a frame and a map of the slot of every pixel.
pub fn show_cells(part: &Part, fi: usize) -> Result<(String, Value)> {
    let prof = profile::load_for(part, None)?;
    let res = resolve(part, &prof)?;
    let tpl = crate::template::load_for(part)?;
    let f = &part.frames[fi];
    let pix = frame_pix(part, &tpl, &res, fi, prof.channel_step);
    let pl = place_frame(part, &res, &pix, fi).map_err(PxlError::validation)?;
    let (w, h) = (pix.w, pix.h);
    let mut map = vec![b'.'; w * h];
    for i in 0..w * h {
        if pl.owner[i] >= 0 && pix.ids[i] != T {
            let c = &pl.cells[pl.owner[i] as usize];
            map[i] = if c.invalid { b'!' } else { b'0' + c.slot };
        }
    }
    let mut text = format!("cells of {}/{}: {} sprites, max {} per line, {} invalid ({}); pixel map = slot of the cell ('!' = invalid cell)", part.name, f.name, pl.sprites, pl.max_line, pl.invalid, if pl.authored { "authored".to_string() } else { format!("band offset {}", pl.oy.unwrap_or(0)) });
    let lw = (h.max(1) - 1).to_string().len();
    for y in 0..h {
        text.push_str(&format!("\n{:>lw$} {}", y, String::from_utf8_lossy(&map[y * w..(y + 1) * w]), lw = lw));
    }
    let mut cells: Vec<&Cell> = pl.cells.iter().collect();
    cells.sort_by_key(|c| (c.y, c.x));
    for c in &cells {
        let st = if c.invalid { "INVALID".to_string() } else if let Some(s2) = c.slot2 { format!("two slots {}+{}", c.slot, s2) } else { "ok".into() };
        text.push_str(&format!("\ncell {},{}  slot {}  {} colour(s)  {}", c.x - part.origin.0, c.y - part.origin.1, res.slot_label(c.slot), c.colors.len(), st));
    }
    let j = json!({"sprites": pl.sprites, "max_per_line": pl.max_line, "invalid": pl.invalid, "cells": cells.iter().map(|c| json!({"x": c.x - part.origin.0, "y": c.y - part.origin.1, "slot": c.slot, "slot2": c.slot2, "valid": !c.invalid})).collect::<Vec<_>>(), "slot_map": (0..h).map(|y| String::from_utf8_lossy(&map[y * w..(y + 1) * w]).into_owned()).collect::<Vec<_>>()});
    Ok((text, j))
}
