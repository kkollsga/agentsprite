//! Pixel level adjustments (DESIGN 5.2): set, row, patch, line, rect, fill, replace, copy, mirror, flip,
//! pattern, write_grid, apply, clean.
//!
//! Every modifying command runs the same pipeline: `lease::guard` (skipped for `--dry-run`), load the part
//! and its template, apply the ops to an in-memory copy (every key is resolved against the template, every
//! coordinate is bounds checked, errors name frame, pixel and key), diff against the original, lock the
//! touched frames unless `--no-lock`, then save atomically (`pxl::save` semantics), append a journal entry
//! (see `journal`) and record the status event. Nothing is written when any step fails (AC-AD1), and an
//! edit that changes no pixel writes nothing at all.
//!
//! # `apply` JSON schema
//!
//! A file (or `-` for stdin) holding either `{"ops": [...]}` or a bare array of ops. All ops are applied in
//! order to one in-memory copy and committed as ONE journal entry (one `undo` step); any error aborts the
//! whole batch. Unknown fields are errors. Points are `[x,y]` or `"x,y"`, regions `[x,y,w,h]` or
//! `"x,y,w,h"`, keys are key characters or template role names, grids a string with `\n` or an array of rows.
//!
//! ```text
//! {"op":"set","frame":F,"pixels":["17,11=W", {"x":18,"y":11,"key":"white"}]}
//! {"op":"row","frame":F,"x":16,"y":13,"pixels":"1222221"}            (space = unchanged)
//! {"op":"patch","frame":F,"x":12,"y":6,"grid":["12 3","1221"]}        (space = unchanged)
//! {"op":"line","frame":F,"from":[9,12],"to":[14,12],"key":"1"}
//! {"op":"rect","frame":F,"region":[2,2,5,4],"key":"2","outline":false}
//! {"op":"fill","frame":F,"at":[15,17],"key":"3"}
//! {"op":"replace","frame":F,"maps":["2=1"] | {"2":"1"},"region":[9,6,6,8]}   (region optional)
//! {"op":"copy","from":F1,"to":F2,"region":[9,6,14,10],"to_at":[0,0],"skip_transparent":false}
//! {"op":"mirror","frame":F,"axis":"x","keep":"low","region":[..]}
//! {"op":"flip","frame":F,"axis":"y","region":[..]}
//! {"op":"write_grid","frame":F,"grid":"....\n...."}
//! {"op":"pick","frame":F,"method":"contour","region":[9,6,14,10]}      (region optional; see `pxl candidates`)
//! {"op":"pattern","frame":F,"unit":[0,0,4,4],"region":[0,0,16,16],"phase":[0,0],"mask":"2"}
//! ```

use crate::fsutil::PathSlash;
use crate::cli::*;
use crate::fsutil;
use crate::compare;
use crate::img::RgbaImage;
use crate::journal::{self, FrameDelta};
use crate::lint;
use crate::lease;
use crate::model::{Grid, Method, Part};
use crate::output::{Loc, Out, PxlError, Result};
use crate::pxl;
use crate::show;
use crate::status::{self, StatusEvent};
use crate::template::{self, Template};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;

// ------------------------------------------------------------------ ops

#[derive(Clone, Debug)]
pub enum Op {
    Set { frame: String, pixels: Vec<(i32, i32, String)> },
    Row { frame: String, x: i32, y: i32, text: String },
    /// `grid` is plain rows (then `x`,`y` are needed) or text printed by `pxl show` (rows place themselves).
    Patch { frame: String, x: Option<i32>, y: Option<i32>, grid: String },
    Line { frame: String, from: (i32, i32), to: (i32, i32), key: String },
    Rect { frame: String, region: Region, key: String, outline: bool },
    Fill { frame: String, at: (i32, i32), key: String },
    Replace { frame: String, maps: Vec<(String, String)>, region: Option<Region> },
    Copy { from: String, to: String, region: Option<Region>, to_at: Option<(i32, i32)>, skip_transparent: bool },
    Mirror { frame: String, axis: Axis, keep_high: bool, region: Option<Region> },
    Flip { frame: String, axis: Axis, region: Option<Region> },
    WriteGrid { frame: String, grid: String },
    /// Reduce the frame afresh with `method` (frame held out of the exemplar bank) and write the result.
    Pick { frame: String, method: Method, region: Option<Region> },
    /// Paste a saved snippet with its top left at `at`.
    Stamp { frame: String, name: String, at: (i32, i32), flip: Option<Axis> },
    /// Move the pixels of a region (whole frame for nudge); vacated cells get `fill` (default `.`).
    Move { frame: String, region: Option<Region>, by: (i32, i32), fill: Option<String> },
    /// Close the outline ring around the shape with `key` (default: the template's `outline` role).
    Outline { frame: String, key: Option<String> },
    /// Orphan / jaggy / eye fixes of one frame.
    Fix { frame: String, rules: Vec<String> },
    /// Repeat a unit (a saved snippet, or `unit` = a rectangle of this frame) over `region` with a fixed `phase`;
    /// with `mask` only over cells currently holding one of those keys.
    Pattern { frame: String, snippet: Option<String>, unit: Option<Region>, region: Option<Region>, phase: Option<(i32, i32)>, mask: Option<String> },
}

/// Points of a pixel-art Bresenham line (8-connected, no doubled steps), both ends included.
pub fn line_points(x0: i32, y0: i32, x1: i32, y1: i32) -> Vec<(i32, i32)> {
    let (mut x, mut y) = (x0, y0);
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    let mut v = Vec::new();
    loop {
        v.push((x, y));
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
    v
}

/// Count and bounding box (minx, miny, maxx, maxy) of differing cells; `None` when equal.
pub fn diff_grids(a: &Grid, b: &Grid) -> Option<(usize, (usize, usize, usize, usize))> {
    if a.w != b.w || a.h != b.h {
        return Some((a.w.max(b.w) * a.h.max(b.h), (0, 0, a.w.max(b.w) - 1, a.h.max(b.h) - 1)));
    }
    let mut n = 0;
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for y in 0..a.h {
        let (ra, rb) = (a.row(y), b.row(y));
        if ra == rb {
            continue;
        }
        for x in 0..a.w {
            if ra[x] != rb[x] {
                n += 1;
                x0 = x0.min(x);
                x1 = x1.max(x);
                y0 = y0.min(y);
                y1 = y1.max(y);
            }
        }
    }
    (n > 0).then_some((n, (x0, y0, x1, y1)))
}

/// Bounding box widened by a 1 px margin (clipped to the canvas), and its rows.
pub fn region_rows(g: &Grid, bb: (usize, usize, usize, usize)) -> ((i32, i32, usize, usize), Vec<String>) {
    let x0 = bb.0.saturating_sub(1);
    let y0 = bb.1.saturating_sub(1);
    let x1 = (bb.2 + 1).min(g.w - 1);
    let y1 = (bb.3 + 1).min(g.h - 1);
    let reg = (x0 as i32, y0 as i32, x1 - x0 + 1, y1 - y0 + 1);
    (reg, rows_of(g, reg))
}

fn rows_of(g: &Grid, r: (i32, i32, usize, usize)) -> Vec<String> {
    (0..r.3).map(|i| (0..r.2).map(|j| g.get(r.0 + j as i32, r.1 + i as i32) as char).collect()).collect()
}

// ------------------------------------------------------------------ context

struct Ctx {
    path: PathBuf,
    raw: String,
    part: Part,
    tpl: Option<Template>,
    before: BTreeMap<usize, (Grid, bool)>,
    /// Lines to print before the changed-region echo (what `fix` repaired).
    notes: Vec<String>,
    /// `--tile N`: set / row / patch coordinates are relative to this tile; the frame touched last.
    tile: Option<usize>,
    tile_frame: Option<usize>,
    /// Frames rewritten whole by an automatic method (pick without region): their pixels count as raw output.
    auto_frames: Vec<usize>,
}

fn describe_keys(t: &Template) -> String {
    let mut v = vec![".(transparent)".to_string()];
    for k in t.keys().take(24) {
        v.push(format!("{}({})", k.key as char, k.role));
    }
    v.join(" ")
}

impl Ctx {
    fn open(part_arg: &str, agent: Option<&str>, guard: bool, force_approved: bool) -> Result<Ctx> {
        let path = fsutil::resolve_part(part_arg)?;
        if guard {
            status::refuse_if_approved(&path, force_approved)?;
            lease::guard(&path, agent)?;
        }
        let raw = fsutil::read_string(&path)?;
        let part = pxl::parse_str(&raw, &path)?;
        let tpl = if part.template.is_some() { Some(template::load_for(&part)?) } else { None };
        Ok(Ctx { path, raw, part, tpl, before: BTreeMap::new(), notes: Vec::new(), tile: None, tile_frame: None, auto_frames: Vec::new() })
    }

    fn fidx(&self, name: &str) -> Result<usize> {
        self.part.frame_index(name).ok_or_else(|| unknown_frame(&self.part, name))
    }

    fn loc(&self, fi: usize, at: Option<(i32, i32)>) -> Loc {
        let l = Loc::file(self.path.slash()).frame(self.part.frames[fi].name.clone());
        match at {
            Some((x, y)) if x >= 0 && y >= 0 => l.at(y as usize, x as usize),
            _ => l,
        }
    }

    /// Canvas point of an op coordinate: tile-relative with `--tile`, checked against the tile; else the canvas.
    fn map_pt(&mut self, fi: usize, x: i32, y: i32) -> Result<(i32, i32)> {
        let Some(t) = self.tile else {
            self.check_pt(fi, x, y)?;
            return Ok((x, y));
        };
        let tr = crate::tileio::tile_ref(&self.part.frames[fi], t)?;
        self.tile_frame = Some(fi);
        if x < 0 || y < 0 || x >= 8 || y >= 8 {
            return Err(PxlError::validation(format!("pixel {},{} is outside tile {} (8x8)", x, y, t))
                .with_loc(self.loc(fi, None))
                .with_hint("with --tile the coordinates are 0..7 inside the tile, as `pxl show P F --tile N` prints them"));
        }
        Ok((tr.ox + x, tr.oy + y))
    }

    fn touch(&mut self, fi: usize) {
        if !self.before.contains_key(&fi) {
            let f = &self.part.frames[fi];
            self.before.insert(fi, (f.grid.clone(), f.locked));
        }
    }

    fn check_pt(&self, fi: usize, x: i32, y: i32) -> Result<()> {
        let g = &self.part.frames[fi].grid;
        if g.in_bounds(x, y) {
            return Ok(());
        }
        let name = &self.part.frames[fi].name;
        Err(PxlError::validation(format!("pixel {},{} is outside the {}x{} canvas of frame {}", x, y, g.w, g.h, name))
            .with_loc(self.loc(fi, Some((x, y))))
            .with_hint(format!("x must be 0..{}, y 0..{}; the nearest valid pixel is {},{}", g.w - 1, g.h - 1, x.clamp(0, g.w as i32 - 1), y.clamp(0, g.h as i32 - 1))))
    }

    /// Region must be non-empty and inside the canvas of frame `fi`.
    fn check_region(&self, fi: usize, r: &Region, what: &str) -> Result<()> {
        let g = &self.part.frames[fi].grid;
        if r.w == 0 || r.h == 0 {
            return Err(PxlError::validation(format!("{} region {},{},{},{} is empty", what, r.x, r.y, r.w, r.h)).with_loc(self.loc(fi, None)).with_hint("width and height must be at least 1"));
        }
        let (ex, ey) = (r.x as i64 + r.w as i64 - 1, r.y as i64 + r.h as i64 - 1);
        if r.x < 0 || r.y < 0 || ex >= g.w as i64 || ey >= g.h as i64 {
            let (bx, by) = if r.x < 0 || ex >= g.w as i64 { (if r.x < 0 { r.x as i64 } else { ex }, r.y as i64) } else { (r.x as i64, if r.y < 0 { r.y as i64 } else { ey }) };
            return Err(PxlError::validation(format!(
                "{} region {},{},{},{} reaches pixel {},{}, outside the {}x{} canvas of frame {}",
                what, r.x, r.y, r.w, r.h, bx, by, g.w, g.h, self.part.frames[fi].name
            ))
            .with_loc(self.loc(fi, Some((bx as i32, by as i32))))
            .with_hint(format!("x must be 0..{}, y 0..{}; the region is x,y,w,h", g.w - 1, g.h - 1)));
        }
        Ok(())
    }

    /// Resolve a key token (key character or role name) to a key defined in the template.
    fn key(&self, tok: &str, fi: usize, at: Option<(i32, i32)>) -> Result<u8> {
        let b = tok.as_bytes();
        match &self.tpl {
            Some(t) => t.resolve(tok).ok_or_else(|| {
                let cands: Vec<String> = t.keys().flat_map(|k| [(k.key as char).to_string(), k.role.clone()]).collect();
                let did = crate::snippet::closest(tok, &cands).map(|c| format!("did you mean '{}'? ", c)).unwrap_or_default();
                PxlError::validation(format!("unknown key or role '{}'{} in template '{}'", tok, at.map(|(x, y)| format!(" for pixel {},{}", x, y)).unwrap_or_default(), t.name))
                    .with_loc(self.loc(fi, at))
                    .with_hint(format!("{}defined: {}; stored keys no longer in the template: pxl template remap {} OLD=NEW", did, describe_keys(t), self.part.name))
            }),
            None if b.len() == 1 && pxl::valid_key(b[0]) => Ok(b[0]),
            None => Err(PxlError::validation(format!("'{}' is not a valid key (the part has no template to resolve role names)", tok)).with_loc(self.loc(fi, at))),
        }
    }

    /// Like `key`, but a single valid character is accepted even when the template does not define it
    /// (so stray keys can be replaced away).
    fn key_lenient(&self, tok: &str, fi: usize) -> Result<u8> {
        let b = tok.as_bytes();
        if b.len() == 1 && pxl::valid_key(b[0]) {
            return Ok(b[0]);
        }
        self.key(tok, fi, None)
    }

    fn put(&mut self, fi: usize, x: i32, y: i32, k: u8) {
        self.touch(fi);
        self.part.frames[fi].grid.set(x, y, k);
    }

    /// Write `row` (space = unchanged) with its first character at x,y.
    fn put_row(&mut self, fi: usize, x: i32, y: i32, row: &str) -> Result<()> {
        for (i, c) in row.chars().enumerate() {
            if c == ' ' {
                continue;
            }
            let (px, py) = self.map_pt(fi, x + i as i32, y)?;
            let k = self.key(&c.to_string(), fi, Some((px, py)))?;
            self.put(fi, px, py, k);
        }
        Ok(())
    }

    fn exec(&mut self, op: &Op) -> Result<()> {
        match op {
            Op::Set { frame, pixels } => {
                let fi = self.fidx(frame)?;
                for (x, y, t) in pixels {
                    let (ax, ay) = self.map_pt(fi, *x, *y)?;
                    let k = self.key(t, fi, Some((ax, ay)))?;
                    self.put(fi, ax, ay, k);
                }
            }
            Op::Row { frame, x, y, text } => {
                let fi = self.fidx(frame)?;
                let (gw, gh) = if self.tile.is_some() { (8, 8) } else { (self.part.frames[fi].grid.w as i32, self.part.frames[fi].grid.h as i32) };
                let n = text.chars().count() as i32;
                if *x < 0 || *y < 0 || *y >= gh || *x + n > gw {
                    return Err(PxlError::validation(format!("row y={} from x={}: the run of {} cells (x {}..{}) leaves the {}x{} canvas", y, x, n, x, x + n - 1, gw, gh))
                        .with_loc(self.loc(fi, None))
                        .with_hint("x is the first column and y the row: `row PART FRAME X Y KEYS` (or x=.. y=..); are X and Y swapped?"));
                }
                for (i, c) in text.chars().enumerate() {
                    if c == ' ' {
                        continue;
                    }
                    let (px, py) = self.map_pt(fi, *x + i as i32, *y)?;
                    let k = self.key(&c.to_string(), fi, Some((px, py)))?;
                    self.put(fi, px, py, k);
                }
            }
            Op::Patch { frame, x, y, grid } => {
                let fi = self.fidx(frame)?;
                let gt = show::parse_grid_text(grid)?;
                if gt.labelled {
                    let first = gt.rows.iter().find_map(|(l, _)| *l).unwrap_or(0);
                    let bx = x.or(gt.x0).unwrap_or(0);
                    for (ly, row) in &gt.rows {
                        let py = ly.unwrap_or(first) + y.map_or(0, |oy| oy - first);
                        self.put_row(fi, bx, py, row)?;
                    }
                } else {
                    let (Some(bx), Some(by)) = (*x, *y) else {
                        return Err(PxlError::usage("patch needs X Y for plain rows").with_loc(self.loc(fi, None)).with_hint("give `patch PART FRAME X Y -`, or paste the text `pxl show` printed (its row labels place the rows)"));
                    };
                    for (j, (_, row)) in gt.rows.iter().enumerate() {
                        self.put_row(fi, bx, by + j as i32, row)?;
                    }
                }
            }
            Op::Line { frame, from, to, key } => {
                let fi = self.fidx(frame)?;
                self.check_pt(fi, from.0, from.1)?;
                self.check_pt(fi, to.0, to.1)?;
                let k = self.key(key, fi, Some(*from))?;
                for (px, py) in line_points(from.0, from.1, to.0, to.1) {
                    self.put(fi, px, py, k);
                }
            }
            Op::Rect { frame, region, key, outline } => {
                let fi = self.fidx(frame)?;
                self.check_region(fi, region, "rect")?;
                let k = self.key(key, fi, Some((region.x, region.y)))?;
                for yy in 0..region.h as i32 {
                    for xx in 0..region.w as i32 {
                        let edge = xx == 0 || yy == 0 || xx == region.w as i32 - 1 || yy == region.h as i32 - 1;
                        if !*outline || edge {
                            self.put(fi, region.x + xx, region.y + yy, k);
                        }
                    }
                }
            }
            Op::Fill { frame, at, key } => {
                let fi = self.fidx(frame)?;
                self.check_pt(fi, at.0, at.1)?;
                let k = self.key(key, fi, Some(*at))?;
                let target = self.part.frames[fi].grid.get(at.0, at.1);
                if target != k {
                    self.touch(fi);
                    let g = &mut self.part.frames[fi].grid;
                    let mut stack = vec![*at];
                    while let Some((x, y)) = stack.pop() {
                        if !g.in_bounds(x, y) || g.get(x, y) != target {
                            continue;
                        }
                        g.set(x, y, k);
                        stack.extend_from_slice(&[(x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)]);
                    }
                }
            }
            Op::Replace { frame, maps, region } => {
                let fi = self.fidx(frame)?;
                let g = &self.part.frames[fi].grid;
                let r = region.unwrap_or(Region { x: 0, y: 0, w: g.w, h: g.h });
                self.check_region(fi, &r, "replace")?;
                let mut pairs = Vec::new();
                for (a, b) in maps {
                    pairs.push((self.key_lenient(a, fi)?, self.key(b, fi, None)?));
                }
                for yy in 0..r.h as i32 {
                    for xx in 0..r.w as i32 {
                        let (x, y) = (r.x + xx, r.y + yy);
                        let c = self.part.frames[fi].grid.get(x, y);
                        if let Some((_, to)) = pairs.iter().find(|(from, _)| *from == c) {
                            if *to != c {
                                self.put(fi, x, y, *to);
                            }
                        }
                    }
                }
            }
            Op::Copy { from, to, region, to_at, skip_transparent } => {
                let fs = self.fidx(from)?;
                let ft = self.fidx(to)?;
                let sg = self.part.frames[fs].grid.clone();
                let r = region.unwrap_or(Region { x: 0, y: 0, w: sg.w, h: sg.h });
                self.check_region(fs, &r, "copy source")?;
                let (dx, dy) = to_at.unwrap_or((r.x, r.y));
                // validate the whole destination first
                self.check_pt(ft, dx, dy)?;
                self.check_pt(ft, dx + r.w as i32 - 1, dy + r.h as i32 - 1)?;
                for yy in 0..r.h as i32 {
                    for xx in 0..r.w as i32 {
                        let c = sg.get(r.x + xx, r.y + yy);
                        if *skip_transparent && c == b'.' {
                            continue;
                        }
                        self.put(ft, dx + xx, dy + yy, c);
                    }
                }
            }
            Op::Mirror { frame, axis, keep_high, region } => {
                let fi = self.fidx(frame)?;
                let g = self.part.frames[fi].grid.clone();
                let r = region.unwrap_or(Region { x: 0, y: 0, w: g.w, h: g.h });
                self.check_region(fi, &r, "mirror")?;
                let (len, other) = if *axis == Axis::X { (r.w as i32, r.h as i32) } else { (r.h as i32, r.w as i32) };
                for o in 0..other {
                    for i in 0..len / 2 {
                        let (src_i, dst_i) = if *keep_high { (len - 1 - i, i) } else { (i, len - 1 - i) };
                        let (sx, sy, dx, dy) = if *axis == Axis::X {
                            (r.x + src_i, r.y + o, r.x + dst_i, r.y + o)
                        } else {
                            (r.x + o, r.y + src_i, r.x + o, r.y + dst_i)
                        };
                        let c = g.get(sx, sy);
                        self.put(fi, dx, dy, c);
                    }
                }
            }
            Op::Flip { frame, axis, region } => {
                let fi = self.fidx(frame)?;
                let g = self.part.frames[fi].grid.clone();
                let r = region.unwrap_or(Region { x: 0, y: 0, w: g.w, h: g.h });
                self.check_region(fi, &r, "flip")?;
                for yy in 0..r.h as i32 {
                    for xx in 0..r.w as i32 {
                        let (sx, sy) = if *axis == Axis::X { (r.x + r.w as i32 - 1 - xx, r.y + yy) } else { (r.x + xx, r.y + r.h as i32 - 1 - yy) };
                        let c = g.get(sx, sy);
                        self.put(fi, r.x + xx, r.y + yy, c);
                    }
                }
            }
            Op::Pick { frame, method, region } => {
                let fi = self.fidx(frame)?;
                let Some(tpl) = self.tpl.as_ref() else {
                    return Err(PxlError::validation("pick needs a part with a template").with_loc(self.loc(fi, None)));
                };
                let (w, h) = (self.part.frames[fi].grid.w, self.part.frames[fi].grid.h);
                let r = region.unwrap_or(Region { x: 0, y: 0, w, h });
                self.check_region(fi, &r, "pick")?;
                if region.is_none() && !self.auto_frames.contains(&fi) {
                    self.auto_frames.push(fi);
                }
                let bank = crate::candidates::bank_for(&self.part, tpl, &[*method])?;
                let (g, _) = crate::candidates::reduce_frame(&self.part, tpl, fi, *method, bank.as_ref())?;
                for yy in 0..r.h as i32 {
                    for xx in 0..r.w as i32 {
                        let (x, y) = (r.x + xx, r.y + yy);
                        let k = g.get(x, y);
                        if self.part.frames[fi].grid.get(x, y) != k {
                            self.put(fi, x, y, k);
                        }
                    }
                }
            }
            Op::Stamp { frame, name, at, flip } => {
                let fi = self.fidx(frame)?;
                let mut rows = crate::snippet::load(&self.part, name)?;
                if let Some(a) = flip {
                    rows = crate::snippet::flip(&rows, *a);
                }
                let w = rows.iter().map(|r| r.chars().count()).max().unwrap_or(0);
                let g = &self.part.frames[fi].grid;
                if at.0 < 0 || at.1 < 0 || at.0 as i64 + w as i64 > g.w as i64 || at.1 as i64 + rows.len() as i64 > g.h as i64 {
                    return Err(PxlError::validation(format!(
                        "snippet '{}' ({}x{}) stamped at {},{} reaches x {}..{}, y {}..{}, outside the {}x{} canvas of frame {}",
                        name,
                        w,
                        rows.len(),
                        at.0,
                        at.1,
                        at.0,
                        at.0 + w as i32 - 1,
                        at.1,
                        at.1 + rows.len() as i32 - 1,
                        g.w,
                        g.h,
                        self.part.frames[fi].name
                    ))
                    .with_loc(self.loc(fi, Some(*at)))
                    .with_hint(format!(
                        "x,y is the top left; nearest valid is {},{}",
                        at.0.clamp(0, (g.w as i32 - w as i32).max(0)),
                        at.1.clamp(0, (g.h as i32 - rows.len() as i32).max(0))
                    )));
                }
                for (j, row) in rows.iter().enumerate() {
                    self.put_row(fi, at.0, at.1 + j as i32, row.trim_end_matches(' '))?;
                }
            }
            Op::Move { frame, region, by, fill } => {
                let fi = self.fidx(frame)?;
                let g = self.part.frames[fi].grid.clone();
                let r = region.unwrap_or(Region { x: 0, y: 0, w: g.w, h: g.h });
                self.check_region(fi, &r, "move")?;
                let fk = match fill {
                    Some(t) => self.key(t, fi, None)?,
                    None => b'.',
                };
                let mut moved = Vec::new();
                for yy in 0..r.h as i32 {
                    for xx in 0..r.w as i32 {
                        let (x, y) = (r.x + xx, r.y + yy);
                        let c = g.get(x, y);
                        if c == b'.' {
                            continue;
                        }
                        let (nx, ny) = (x + by.0, y + by.1);
                        if !g.in_bounds(nx, ny) {
                            return Err(PxlError::validation(format!("moving by {},{} pushes pixel {},{} ('{}') to {},{}, outside the {}x{} canvas of frame {}", by.0, by.1, x, y, c as char, nx, ny, g.w, g.h, self.part.frames[fi].name))
                                .with_loc(self.loc(fi, Some((nx, ny))))
                                .with_hint("move by less, or erase those pixels first"));
                        }
                        moved.push((nx, ny, c));
                    }
                }
                for yy in 0..r.h as i32 {
                    for xx in 0..r.w as i32 {
                        let (x, y) = (r.x + xx, r.y + yy);
                        if g.get(x, y) != fk {
                            self.put(fi, x, y, fk);
                        }
                    }
                }
                for (x, y, c) in moved {
                    self.put(fi, x, y, c);
                }
            }
            Op::Outline { frame, key } => {
                let fi = self.fidx(frame)?;
                let o = match (key, &self.tpl) {
                    (Some(t), _) => self.key(t, fi, None)?,
                    (None, Some(t)) => t.key_for_role("outline").ok_or_else(|| PxlError::validation("the template has no key with the role 'outline'").with_loc(self.loc(fi, None)).with_hint("pass --key KEY"))?,
                    (None, None) => return Err(PxlError::validation("outline needs --key (the part has no template)").with_loc(self.loc(fi, None))),
                };
                let g = self.part.frames[fi].grid.clone();
                let mut cand = Vec::new();
                let mut ring = g.clone();
                for y in 0..g.h as i32 {
                    for x in 0..g.w as i32 {
                        if g.get(x, y) != b'.' {
                            continue;
                        }
                        let body = [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)].iter().any(|&(nx, ny)| g.in_bounds(nx, ny) && g.get(nx, ny) != b'.' && g.get(nx, ny) != o);
                        if body {
                            cand.push((x, y));
                            ring.set(x, y, o);
                        }
                    }
                }
                // no doubled steps: drop the new pixels the jaggy rule would remove
                let mut thin = ring.clone();
                clean_grid(&mut thin, false, true);
                for (x, y) in cand {
                    if thin.get(x, y) == b'.' {
                        ring.set(x, y, b'.');
                    }
                }
                for y in 0..g.h as i32 {
                    for x in 0..g.w as i32 {
                        if ring.get(x, y) != g.get(x, y) {
                            self.put(fi, x, y, ring.get(x, y));
                        }
                    }
                }
            }
            Op::Fix { frame, rules } => {
                let fi = self.fidx(frame)?;
                let all = rules.is_empty();
                let has = |r: &str| all || rules.iter().any(|x| x == r);
                let mut g = self.part.frames[fi].grid.clone();
                let mut notes = Vec::new();
                for f in clean_grid(&mut g, has("orphan"), has("jaggy")) {
                    notes.push(match f.fix {
                        Some(k) => format!("fixed {} at {},{} '{}' -> '{}'", f.kind, f.x, f.y, f.key as char, k as char),
                        None => format!("left {} at {},{} '{}' (no automatic fix: patch it by hand)", f.kind, f.x, f.y, f.key as char),
                    });
                }
                if has("eye") {
                    if let (Some(tpl), Some(rf)) = (self.tpl.as_ref(), show::try_reference(&self.part, &self.part.frames[fi])) {
                        let (_, lost) = lint::eye_losses(&self.part, tpl, &g, &rf);
                        for (l, eye) in lost {
                            match lint::eye_fix_pixels(&self.part, tpl, &g, &rf, &eye) {
                                Some(px) => {
                                    for t in px.split(' ') {
                                        if let Ok(p) = t.parse::<Pixel>() {
                                            if let Some(k) = p.key.bytes().next() {
                                                g.set(p.x, p.y, k);
                                            }
                                        }
                                    }
                                    notes.push(format!("fixed eye at {},{}: {}", l.x, l.y, px));
                                }
                                None => notes.push(format!("eye at {},{} is lost but cannot be restored automatically: draw it with patch or stamp", l.x, l.y)),
                            }
                        }
                    }
                }
                let before = self.part.frames[fi].grid.clone();
                for y in 0..g.h as i32 {
                    for x in 0..g.w as i32 {
                        if g.get(x, y) != before.get(x, y) {
                            self.put(fi, x, y, g.get(x, y));
                        }
                    }
                }
                if notes.is_empty() {
                    notes.push(format!("nothing to fix in {}", self.part.frames[fi].name));
                }
                self.notes.extend(notes);
            }
            Op::Pattern { frame, snippet, unit, region, phase, mask } => self.exec_pattern(frame, snippet.as_deref(), *unit, *region, *phase, mask.as_deref())?,
            Op::WriteGrid { frame, grid } => {
                let fi = self.fidx(frame)?;
                let (w, h) = (self.part.frames[fi].grid.w, self.part.frames[fi].grid.h);
                let gt = show::parse_grid_text(grid)?;
                let grid_text = if gt.labelled {
                    let ok = gt.x0.unwrap_or(0) == 0 && gt.rows.len() == h && gt.rows.iter().enumerate().all(|(i, (l, r))| *l == Some(i as i32) && r.chars().count() == w);
                    if !ok {
                        return Err(PxlError::validation(format!("write_grid replaces the whole {}x{} frame, but the pasted text covers rows {}..{} from x={}", w, h, gt.rows.first().and_then(|r| r.0).unwrap_or(0), gt.rows.last().and_then(|r| r.0).unwrap_or(0), gt.x0.unwrap_or(0)))
                            .with_loc(self.loc(fi, None))
                            .with_hint("use `pxl patch PART FRAME -` for a region: it takes the same text"));
                    }
                    gt.rows.iter().map(|(_, r)| r.as_str()).collect::<Vec<_>>().join("\n")
                } else {
                    grid.clone()
                };
                let ng = pxl::parse_grid(&grid_text, w, h, &self.loc(fi, None))?;
                if let Some(t) = &self.tpl {
                    for y in 0..h {
                        for x in 0..w {
                            let c = ng.cells[y * w + x];
                            if t.def(c).is_none() {
                                return Err(PxlError::validation(format!("key '{}' is not defined in template '{}'", c as char, t.name))
                                    .with_loc(self.loc(fi, Some((x as i32, y as i32))))
                                    .with_hint(format!("defined: {}; to rename stored keys across the part: pxl template remap {} OLD=NEW", describe_keys(t), self.part.name)));
                            }
                        }
                    }
                }
                self.touch(fi);
                self.part.frames[fi].grid = ng;
            }
        }
        Ok(())
    }

    /// `pattern`: the cell at (x, y) takes `unit[(y - py) mod h][(x - px) mod w]`, so a fixed phase makes every
    /// region, frame and neighbouring part that uses the same unit and phase tile identically.
    fn exec_pattern(&mut self, frame: &str, snippet: Option<&str>, unit: Option<Region>, region: Option<Region>, phase: Option<(i32, i32)>, mask: Option<&str>) -> Result<()> {
        let fi = self.fidx(frame)?;
        let (ox, oy, lw, lh) = match self.tile {
            Some(t) => {
                let tr = crate::tileio::tile_ref(&self.part.frames[fi], t)?;
                self.tile_frame = Some(fi);
                (tr.ox, tr.oy, 8usize, 8usize)
            }
            None => (0, 0, self.part.frames[fi].grid.w, self.part.frames[fi].grid.h),
        };
        let area = if self.tile.is_some() { "tile" } else { "canvas" };
        let inside = |r: &Region, what: &str, this: &Ctx| -> Result<()> {
            let (ex, ey) = (r.x as i64 + r.w as i64 - 1, r.y as i64 + r.h as i64 - 1);
            if r.w == 0 || r.h == 0 || r.x < 0 || r.y < 0 || ex >= lw as i64 || ey >= lh as i64 {
                return Err(PxlError::validation(format!("pattern {} {},{},{},{} is empty or outside the {}x{} {} of frame {}", what, r.x, r.y, r.w, r.h, lw, lh, area, this.part.frames[fi].name))
                    .with_loc(this.loc(fi, None))
                    .with_hint(if this.tile.is_some() { "with --tile the coordinates are 0..7 inside the tile" } else { "regions are x,y,w,h inside the canvas" }));
            }
            Ok(())
        };
        let region = region.unwrap_or(Region { x: 0, y: 0, w: lw, h: lh });
        inside(&region, "region", self)?;
        // unit cells: None = leave the pixel as it is
        let cells: Vec<Vec<Option<u8>>> = match (snippet, unit) {
            (Some(name), None) => {
                let rows = crate::snippet::load(&self.part, name)?;
                let w = rows.iter().map(|r| r.chars().count()).max().unwrap_or(0);
                if w == 0 || rows.is_empty() {
                    return Err(PxlError::validation(format!("snippet '{}' is empty", name)).with_loc(self.loc(fi, None)));
                }
                let mut v = Vec::new();
                for r in &rows {
                    let mut line = Vec::new();
                    for c in format!("{:<w$}", r, w = w).chars() {
                        line.push(if c == ' ' { None } else { Some(self.key(&c.to_string(), fi, None)?) });
                    }
                    v.push(line);
                }
                v
            }
            (None, Some(u)) => {
                inside(&u, "unit", self)?;
                (0..u.h as i32).map(|j| (0..u.w as i32).map(|i| Some(self.part.frames[fi].grid.get(ox + u.x + i, oy + u.y + j))).collect()).collect()
            }
            _ => return Err(PxlError::usage("pattern needs exactly one unit: a snippet name or --unit x,y,w,h").with_loc(self.loc(fi, None)).with_hint("pxl pattern PART FRAME SNIPPET --region x,y,w,h   or   pxl pattern PART FRAME --unit x,y,w,h --region x,y,w,h")),
        };
        let (uw, uh) = (cells[0].len() as i32, cells.len() as i32);
        let (px, py) = phase.unwrap_or(if self.tile.is_none() && self.part.kind == crate::model::PartKind::Sprite { self.part.origin } else { (0, 0) });
        let mut mkeys = Vec::new();
        if let Some(m) = mask {
            for t in m.split(',').filter(|t| !t.is_empty()) {
                mkeys.push(self.key_lenient(t, fi)?);
            }
        }
        for yy in 0..region.h as i32 {
            for xx in 0..region.w as i32 {
                let (x, y) = (region.x + xx, region.y + yy);
                let Some(k) = cells[(y - py).rem_euclid(uh) as usize][(x - px).rem_euclid(uw) as usize] else { continue };
                let cur = self.part.frames[fi].grid.get(ox + x, oy + y);
                if cur != k && (mkeys.is_empty() || mkeys.contains(&cur)) {
                    self.put(fi, ox + x, oy + y, k);
                }
            }
        }
        Ok(())
    }

    /// Validate, lock, save, journal, report. `extra` adds a named array to the JSON/text output.
    fn finish(mut self, cmd: &str, flags: &EditFlags, out: &mut Out, extra: Option<(&str, Vec<String>)>) -> Result<()> {
        struct Ch {
            fi: usize,
            n: usize,
            bb: (usize, usize, usize, usize),
        }
        let mut changes = Vec::new();
        for (fi, (bg, _)) in &self.before {
            if let Some((n, bb)) = diff_grids(bg, &self.part.frames[*fi].grid) {
                changes.push(Ch { fi: *fi, n, bb });
            }
        }
        // final template validation of every changed pixel
        if let Some(t) = &self.tpl {
            for c in &changes {
                let g = &self.part.frames[c.fi].grid;
                let bg = &self.before[&c.fi].0;
                for y in c.bb.1..=c.bb.3 {
                    for x in c.bb.0..=c.bb.2 {
                        let k = g.cells[y * g.w + x];
                        if k != bg.cells[y * g.w + x] && t.def(k).is_none() {
                            return Err(PxlError::validation(format!("key '{}' is not defined in template '{}'", k as char, t.name))
                                .with_loc(self.loc(c.fi, Some((x as i32, y as i32))))
                                .with_hint(format!("defined: {}; to rename stored keys across the part: pxl template remap {} OLD=NEW", describe_keys(t), self.part.name)));
                        }
                    }
                }
            }
        }
        let total: usize = changes.iter().map(|c| c.n).sum();
        if total > 0 && self.part.palette_set.is_some() {
            if let Some(tpl) = &self.tpl {
                for c in &changes {
                    let w = crate::cells::edit_warnings(&self.part, tpl, &self.before[&c.fi].0, &self.part.frames[c.fi].grid, c.fi);
                    self.notes.extend(w);
                }
            }
        }
        let dry = flags.dry_run;
        let mut deltas = Vec::new();
        for c in &changes {
            if !flags.no_lock {
                self.part.frames[c.fi].locked = true;
            }
            let (bg, lb) = &self.before[&c.fi];
            let region = (c.bb.0 as i32, c.bb.1 as i32, c.bb.2 - c.bb.0 + 1, c.bb.3 - c.bb.1 + 1);
            deltas.push(FrameDelta {
                name: self.part.frames[c.fi].name.clone(),
                locked_before: *lb,
                locked_after: self.part.frames[c.fi].locked,
                region,
                before: rows_of(bg, region),
            });
        }
        if total > 0 && !dry {
            let text = pxl::to_string(&self.part);
            let canonical = pxl::parse_str(&self.raw, &self.path).map(|p| pxl::to_string(&p) == self.raw).unwrap_or(true);
            fsutil::atomic_write(&self.path, text.as_bytes())?;
            let mut auto: Vec<usize> = self.auto_frames.iter().copied().filter(|fi| changes.iter().any(|c| c.fi == *fi)).collect();
            // a palette fit is an automatic method too: a frame that was still raw before it is raw after it (never a hand-finished one)
            if cmd == "palette fit" {
                let st = status::read(&self.path);
                for c in &changes {
                    let name = &self.part.frames[c.fi].name;
                    let was_raw = st.raw.get(name).map_or(false, |r| *r == fsutil::content_hash(&self.before[&c.fi].0.cells)) && st.edited.get(name).map_or(true, |e| e.is_empty());
                    if was_raw && !auto.contains(&c.fi) {
                        auto.push(c.fi);
                    }
                }
            }
            let mut entry = journal::make_entry(out.agent(), cmd, deltas, &text, if canonical { None } else { Some(self.raw.clone()) });
            entry.auto = auto.iter().map(|&fi| self.part.frames[fi].name.clone()).collect();
            journal::append_edit(&self.path, &entry)?;
            status::record(&self.path, &self.part, StatusEvent::Edited);
            // automatic output stays "raw": whole-frame picks, and the tiles that fit / dedupe rewrote
            if !auto.is_empty() {
                status::record(&self.path, &self.part, StatusEvent::RawFrames(auto));
            }
            if cmd == "tiles fit" || cmd == "tiles dedupe" {
                for c in &changes {
                    if let Some(n) = status::tile_count(&self.part, c.fi) {
                        let (g, bg) = (&self.part.frames[c.fi].grid, &self.before[&c.fi].0);
                        let tiles: Vec<usize> = (0..n).filter(|&t| status::tile_hash(g, t) != status::tile_hash(bg, t)).collect();
                        status::record(&self.path, &self.part, StatusEvent::RawTiles { frame: c.fi, tiles });
                    }
                }
            }
        }
        // ---- report
        let mut text = String::new();
        let mut frames_json = Vec::new();
        let pname = self.part.name.clone();
        let tag = if dry { " (dry run, nothing written)" } else { "" };
        let tile_mode = self.tile;
        if total == 0 && tile_mode.is_none() {
            text.push_str(&format!("{}: no pixel changed{}", cmd, tag));
        }
        if let Some(ti) = tile_mode {
            let fi = changes.first().map(|c| c.fi).or(self.tile_frame).unwrap_or_else(|| crate::tiles::pick_frame(&self.part, None).unwrap_or(0));
            let (t, fj) = crate::tileio::edit_echo(&self.part, self.tpl.as_ref(), fi, ti, total, cmd, flags.reference && !flags.no_ref, tag)?;
            text = t;
            frames_json.push(fj);
        }
        let loop_changes: &[Ch] = if tile_mode.is_some() { &[] } else { &changes };
        for (i, c) in loop_changes.iter().enumerate() {
            let g = &self.part.frames[c.fi].grid;
            let (reg, rows) = region_rows(g, c.bb);
            if i > 0 {
                text.push('\n');
            }
            text.push_str(&format!("{} {}/{}: {} px changed, region {},{},{},{}{}", cmd, pname, self.part.frames[c.fi].name, c.n, reg.0, reg.1, reg.2, reg.3, tag));
            let mut echoed = false;
            if reg.3 <= 48 && !flags.no_ref && (flags.reference || changes.len() == 1) {
                if let Ok(rb) = show::ref_block(&self.part, &self.part.frames[c.fi], reg) {
                    // by default only when the whole output stays under AC-AD4's 15 lines
                    if flags.reference || rb.lines.len() + 2 + extra.as_ref().map_or(0, |e| e.1.len()) + self.notes.len() <= 12 {
                        for l in &rb.lines {
                            text.push('\n');
                            text.push_str(l.trim_end());
                        }
                        if !rb.legend.is_empty() {
                            text.push_str(&format!("\nlegend: {}  (right: reference letters; >K = its template key, ~K = nearest key)", rb.legend));
                        }
                        echoed = true;
                    }
                }
            }
            if !echoed {
                if reg.3 <= 48 {
                    for l in show::render_grid(g, reg) {
                        text.push('\n');
                        text.push_str(&l);
                    }
                } else {
                    text.push_str("\n(region too large to echo; use `pxl show` with --region)");
                }
            }
            frames_json.push(json!({"frame": self.part.frames[c.fi].name, "changed": c.n, "region": [reg.0, reg.1, reg.2, reg.3], "rows": rows}));
        }
        if changes.len() > 1 {
            text.push_str(&format!("\ntotal {} px changed in {} frames", total, changes.len()));
        }
        let mut j = json!({"changed": total, "dry_run": dry, "frames": frames_json});
        if frames_json_len(&j) == 1 {
            let f = j["frames"][0].clone();
            j["frame"] = f["frame"].clone();
            j["region"] = f["region"].clone();
            j["rows"] = f["rows"].clone();
        }
        if let Some(arg) = flags.preview.as_deref().filter(|_| total > 0) {
            let tpl = self.tpl.as_ref();
            let mut stack: Vec<RgbaImage> = Vec::new();
            let mut height = 0;
            for c in changes.iter().take(3) {
                let fr = &self.part.frames[c.fi];
                let win = show::window_around(c.bb, 3, 12, 10, fr.grid.w, fr.grid.h);
                let rf = show::try_reference(&self.part, fr);
                let mark = (c.bb.0 as i32, c.bb.1 as i32, c.bb.2 - c.bb.0 + 1, c.bb.3 - c.bb.1 + 1);
                let (im, _) = show::render_panels(&self.part, tpl, rf.as_ref(), &[("before".to_string(), &self.before[&c.fi].0), (format!("after: {}", fr.name), &fr.grid)], win, Some(mark), None);
                if !stack.is_empty() && height + im.h > compare::MAX_PX {
                    break;
                }
                height += im.h;
                stack.push(im);
            }
            if let Some(first) = stack.first() {
                let w = stack.iter().map(|i| i.w).max().unwrap_or(first.w);
                let mut sheet = RgbaImage::filled(w, height, compare::BG);
                let mut y = 0;
                for im in &stack {
                    compare::paste(&mut sheet, im, 0, y);
                    y += im.h;
                }
                let first_name = self.part.frames[changes[0].fi].name.clone();
                let p = if arg == "auto" { show::default_png(&self.part, &first_name, "") } else { PathBuf::from(arg) };
                show::write_png(&p, &sheet)?;
                text.push_str(&format!("\npreview: {} ({}x{} px: reference | before | after, changed region ringed)", p.slash(), sheet.w, sheet.h));
                j["preview"] = json!(p.slash());
            }
        }
        let mut items = self.notes.clone();
        if let Some((name, mut more)) = extra {
            items.append(&mut more);
            if !items.is_empty() {
                text = format!("{}\n{}", items.join("\n"), text);
            }
            j[name] = json!(items);
        } else if !items.is_empty() {
            text = format!("{}\n{}", items.join("\n"), text);
            j["notes"] = json!(items);
        }
        out.emit(text, j);
        Ok(())
    }
}

fn frames_json_len(j: &Value) -> usize {
    j["frames"].as_array().map_or(0, |a| a.len())
}

/// Run ops against a part with the full pipeline.
pub fn execute(part_arg: &str, ops: &[Op], flags: &EditFlags, cmd: &str, out: &mut Out) -> Result<()> {
    let mut c = Ctx::open(part_arg, out.agent(), !flags.dry_run, flags.force_approved)?;
    if let Some(t) = flags.tile {
        if let Some(bad) = ops.iter().find(|o| !matches!(o, Op::Set { .. } | Op::Row { .. } | Op::Patch { .. } | Op::Pattern { .. })) {
            let _ = bad;
            return Err(PxlError::usage(format!("--tile works with set, row, patch and pattern, not {}", cmd)).with_hint("use tile-relative coordinates 0..7 with `set`, `row` or `patch`, or drop --tile"));
        }
        c.tile = Some(t);
    }
    for op in ops {
        c.exec(op)?;
    }
    c.finish(cmd, flags, out, None)
}

// ------------------------------------------------------------------ CLI entry points

pub fn run_set(a: &SetArgs, out: &mut Out) -> Result<()> {
    let op = Op::Set { frame: a.frame.clone(), pixels: a.pixels.iter().map(|p| (p.x, p.y, p.key.clone())).collect() };
    execute(&a.part, &[op], &a.flags, "set", out)
}
pub fn run_row(a: &RowArgs, out: &mut Out) -> Result<()> {
    let (x, y, text) = a.parse()?;
    let n = text.chars().count() as i32;
    let op = Op::Row { frame: a.frame.clone(), x, y, text };
    execute(&a.part, &[op], &a.flags, "row", out)?;
    out.line(format!("row y={} from x={} ({} cells: x {}..{})", y, x, n, x, x + n - 1));
    out.set("row", json!({"y": y, "x": x, "len": n}));
    Ok(())
}
pub fn run_patch(a: &PatchArgs, out: &mut Out) -> Result<()> {
    let (x, y, grid) = match a.rest.as_slice() {
        [g] => (None, None, g),
        [x, y, g] => {
            let n = |s: &String, w: &str| s.parse::<i32>().map_err(|_| PxlError::usage(format!("patch: {} must be a whole number, not '{}'", w, s)).with_hint("patch PART FRAME [X Y] GRID: X Y are only needed for plain rows"));
            (Some(n(x, "X")?), Some(n(y, "Y")?), g)
        }
        _ => return Err(PxlError::usage("patch takes GRID, or X Y GRID").with_hint("paste the text `pxl show PART FRAME --region ...` printed: pxl show P F --region 12,6,10,8 ; pxl patch P F -")),
    };
    let op = Op::Patch { frame: a.frame.clone(), x, y, grid: read_text_arg(grid)? };
    execute(&a.part, &[op], &a.flags, "patch", out)
}
pub fn run_line(a: &LineArgs, out: &mut Out) -> Result<()> {
    let op = Op::Line { frame: a.frame.clone(), from: (a.from.x, a.from.y), to: (a.to.x, a.to.y), key: a.key.clone() };
    execute(&a.part, &[op], &a.flags, "line", out)
}
pub fn run_rect(a: &RectArgs, out: &mut Out) -> Result<()> {
    let op = Op::Rect { frame: a.frame.clone(), region: a.region, key: a.key.clone(), outline: a.outline };
    execute(&a.part, &[op], &a.flags, "rect", out)
}
pub fn run_fill(a: &FillArgs, out: &mut Out) -> Result<()> {
    let op = Op::Fill { frame: a.frame.clone(), at: (a.at.x, a.at.y), key: a.key.clone() };
    execute(&a.part, &[op], &a.flags, "fill", out)
}
pub fn run_replace(a: &ReplaceArgs, out: &mut Out) -> Result<()> {
    let op = Op::Replace { frame: a.frame.clone(), maps: a.maps.iter().map(|m| (m.from.clone(), m.to.clone())).collect(), region: a.region };
    execute(&a.part, &[op], &a.flags, "replace", out)
}
/// `pxl template remap PART OLD=NEW ...`: rewrite keys in the stored grids. OLD need not exist in the
/// template (that is the point); NEW must. Keeps the lock flags; journaled like any edit (undo works).
pub fn run_remap(a: &crate::cli::RemapArgs, out: &mut Out) -> Result<()> {
    let path = fsutil::resolve_part(&a.part)?;
    let part = pxl::parse_str(&fsutil::read_string(&path)?, &path)?;
    let names: Vec<String> = a.sel.select(&part)?.iter().map(|f| f.name.clone()).collect();
    let maps: Vec<(String, String)> = a.maps.iter().map(|m| (m.from.clone(), m.to.clone())).collect();
    let ops: Vec<Op> = names.iter().map(|n| Op::Replace { frame: n.clone(), maps: maps.clone(), region: None }).collect();
    let mut flags = a.flags.clone();
    flags.no_lock = true;
    execute(&a.part, &ops, &flags, "remap", out)
}
pub fn run_copy(a: &CopyArgs, out: &mut Out) -> Result<()> {
    let op = Op::Copy { from: a.from.clone(), to: a.to.clone(), region: a.region, to_at: a.to_at.map(|p| (p.x, p.y)), skip_transparent: a.skip_transparent };
    execute(&a.part, &[op], &a.flags, "copy", out)
}
pub fn run_mirror(a: &MirrorArgs, out: &mut Out) -> Result<()> {
    let keep_high = match a.keep.as_str() {
        "low" => false,
        "high" => true,
        o => return Err(PxlError::usage(format!("--keep must be 'low' or 'high', not '{}'", o))),
    };
    let op = Op::Mirror { frame: a.frame.clone(), axis: a.axis, keep_high, region: a.region };
    execute(&a.part, &[op], &a.flags, "mirror", out)
}
pub fn run_flip(a: &FlipArgs, out: &mut Out) -> Result<()> {
    let op = Op::Flip { frame: a.frame.clone(), axis: a.axis, region: a.region };
    execute(&a.part, &[op], &a.flags, "flip", out)
}
pub fn run_pick(a: &PickArgs, out: &mut Out) -> Result<()> {
    let method = Method::parse(&a.method).ok_or_else(|| PxlError::usage(format!("unknown method '{}'", a.method)).with_hint("methods: vote, contour, exemplar, auto, nearest"))?;
    let op = Op::Pick { frame: a.frame.clone(), method, region: a.region };
    execute(&a.part, &[op], &a.flags, "pick", out)
}
/// `pxl lock` / `pxl unlock`: set the lock flag of the selected frames (no pixel changes). Journaled as one undo step.
pub fn run_lock(a: &crate::cli::LockArgs, lock: bool, out: &mut Out) -> Result<()> {
    let cmd = if lock { "lock" } else { "unlock" };
    if a.frames.is_none() && !a.all {
        return Err(PxlError::usage(format!("{} needs frames or --all", cmd)).with_hint(format!("pxl {} PART 01_stand,02_walk*   or   pxl {} PART --all", cmd, cmd)));
    }
    let path = fsutil::resolve_part(&a.part)?;
    lease::guard(&path, out.agent())?;
    if !a.dry_run {
        status::refuse_if_approved(&path, a.force_approved)?;
    }
    let raw = fsutil::read_string(&path)?;
    let mut part = pxl::parse_str(&raw, &path)?;
    let sel = FrameSel { frames: a.frames.as_deref().map(|s| s.split(',').map(|x| x.to_string()).collect()).unwrap_or_default(), all: a.all };
    let names: Vec<String> = sel.select(&part)?.iter().map(|f| f.name.clone()).collect();
    let mut deltas = Vec::new();
    let mut changed = Vec::new();
    for n in &names {
        let f = part.frame_mut(n).unwrap();
        if f.locked != lock {
            deltas.push(FrameDelta { name: n.clone(), locked_before: f.locked, locked_after: lock, region: (0, 0, 0, 0), before: Vec::new() });
            f.locked = lock;
            changed.push(n.clone());
        }
    }
    if !changed.is_empty() && !a.dry_run {
        let text = pxl::to_string(&part);
        let canonical = pxl::parse_str(&raw, &path).map(|p| pxl::to_string(&p) == raw).unwrap_or(true);
        fsutil::atomic_write(&path, text.as_bytes())?;
        let entry = journal::make_entry(out.agent(), cmd, deltas, &text, if canonical { None } else { Some(raw.clone()) });
        journal::append_edit(&path, &entry)?;
        status::record(&path, &part, StatusEvent::Edited);
    }
    let kept = names.len() - changed.len();
    let tag = if a.dry_run { " (dry run, nothing written)" } else { "" };
    out.emit(
        format!("{}ed {}: {} frame(s) changed{}{}{}", cmd, part.name, changed.len(), if kept > 0 { format!(", {} already {}ed", kept, cmd) } else { String::new() }, if changed.is_empty() { String::new() } else { format!(": {}", changed.join(" ")) }, tag),
        json!({"locked": lock, "changed": changed, "unchanged": kept, "dry_run": a.dry_run}),
    );
    Ok(())
}

fn parse_tile_range(s: &str) -> Result<Vec<usize>> {
    let bad = || PxlError::usage(format!("bad --tiles '{}'", s)).with_hint("--tiles 37 or --tiles 37-40");
    let (a, b) = match s.split_once('-') {
        Some((a, b)) => (a.trim().parse::<usize>().map_err(|_| bad())?, b.trim().parse::<usize>().map_err(|_| bad())?),
        None => {
            let n = s.trim().parse::<usize>().map_err(|_| bad())?;
            (n, n)
        }
    };
    if b < a {
        return Err(bad());
    }
    Ok((a..=b).collect())
}

/// After a `finish` mark is dropped (rework, undo): the frames' pixels are the automatic output again, so they
/// count as raw unless a real hand edit still stands.
pub fn reopen_finished(path: &std::path::Path, names: &[String], tiles: &[usize]) {
    let Ok(part) = pxl::load(path) else { return };
    let hand = journal::hand_edited(path).unwrap_or_default();
    for name in names {
        let Some(fi) = part.frames.iter().position(|f| &f.name == name) else { continue };
        let regs: &[(i32, i32, usize, usize)] = hand.get(name).map(|v| v.as_slice()).unwrap_or(&[]);
        match status::tile_count(&part, fi) {
            Some(n) => {
                let per = part.frames[fi].grid.w / 8;
                let ts: Vec<usize> = (0..n).filter(|t| (tiles.is_empty() || tiles.contains(t)) && !regs.iter().any(|r| crate::rawaudit::region_hits_tile(r, per, *t))).collect();
                status::record(path, &part, StatusEvent::RawTiles { frame: fi, tiles: ts });
            }
            None if regs.is_empty() => status::record(path, &part, StatusEvent::RawFrames(vec![fi])),
            None => {}
        }
    }
}

/// `pxl finish PART FRAMES --reason "..." [--tiles A-B]`: declare frames (tiles) hand-finished without changing a
/// pixel (the automatic output is already right). Journaled as one undo step (`cmd` finish); the reviewer sees
/// the reason under "Marked finished without edit".
pub fn run_finish(a: &crate::cli::FinishArgs, out: &mut Out) -> Result<()> {
    if a.reason.trim().is_empty() {
        return Err(PxlError::usage("finish needs a --reason").with_hint("pxl finish PART 01_stand --reason \"already equals the source within the palette\""));
    }
    let path = fsutil::resolve_part(&a.part)?;
    status::refuse_if_approved(&path, a.force_approved)?;
    lease::guard(&path, out.agent())?;
    let _pl = fsutil::lock_part(&path)?;
    let raw = fsutil::read_string(&path)?;
    let part = pxl::parse_str(&raw, &path)?;
    let sel = FrameSel { frames: a.frames.split(',').map(|x| x.trim().to_string()).collect(), all: false };
    let idx: Vec<usize> = sel.select(&part)?.iter().filter_map(|f| part.frames.iter().position(|g| g.name == f.name)).collect();
    let tiles = match &a.tiles {
        Some(t) => parse_tile_range(t)?,
        None => Vec::new(),
    };
    let mut deltas = Vec::new();
    let mut edited: Vec<(String, Vec<usize>)> = Vec::new();
    for &fi in &idx {
        let f = &part.frames[fi];
        if tiles.is_empty() {
            let n = status::tile_count(&part, fi);
            deltas.push(FrameDelta { name: f.name.clone(), locked_before: f.locked, locked_after: f.locked, region: (0, 0, f.grid.w, f.grid.h), before: Vec::new() });
            edited.push((f.name.clone(), n.map_or(vec![0], |n| (0..n).collect())));
        } else {
            let n = status::tile_count(&part, fi).ok_or_else(|| PxlError::usage(format!("--tiles needs a tile part; frame '{}' is not made of 8x8 tiles", f.name)))?;
            if let Some(t) = tiles.iter().find(|&&t| t >= n) {
                return Err(PxlError::usage(format!("tile {} is out of range: frame '{}' has {} tiles (0-{})", t, f.name, n, n - 1)));
            }
            let per = f.grid.w / 8;
            for &t in &tiles {
                deltas.push(FrameDelta { name: f.name.clone(), locked_before: f.locked, locked_after: f.locked, region: (((t % per) * 8) as i32, ((t / per) * 8) as i32, 8, 8), before: Vec::new() });
            }
            edited.push((f.name.clone(), tiles.clone()));
        }
    }
    let mut entry = journal::make_entry(out.agent(), "finish", deltas, &raw, None);
    entry.reason = Some(a.reason.trim().to_string());
    entry.tiles = tiles.clone();
    journal::append_edit(&path, &entry)?;
    status::update(&path, |s| {
        for (name, ts) in &edited {
            let e = s.edited.entry(name.clone()).or_default();
            for t in ts {
                if !e.contains(t) {
                    e.push(*t);
                }
            }
            e.sort_unstable();
        }
    })?;
    status::record(&path, &part, StatusEvent::Edited);
    let names: Vec<String> = edited.iter().map(|(n, _)| n.clone()).collect();
    out.emit(
        format!("finished {}: {} frame(s) marked hand-finished{}: {} ({})", part.name, names.len(), if tiles.is_empty() { String::new() } else { format!(", tiles {}", a.tiles.as_deref().unwrap_or("")) }, names.join(" "), a.reason.trim()),
        json!({"finished": names, "tiles": tiles, "reason": a.reason.trim()}),
    );
    Ok(())
}

/// Frame names a selector (name, `walk*`, `6-11`, `tag:walk`, comma separated) stands for.
fn select_names(part_arg: &str, sel: &str) -> Result<Vec<String>> {
    let path = fsutil::resolve_part(part_arg)?;
    let part = pxl::load(&path)?;
    let fs = FrameSel { frames: sel.split(',').map(|s| s.to_string()).collect(), all: false };
    Ok(fs.select(&part)?.iter().map(|f| f.name.clone()).collect())
}

pub fn run_stamp(a: &StampArgs, out: &mut Out) -> Result<()> {
    let op = Op::Stamp { frame: a.frame.clone(), name: a.name.clone(), at: (a.at.x, a.at.y), flip: a.flip };
    execute(&a.part, &[op], &a.flags, "stamp", out)
}
pub fn run_pattern(a: &PatternArgs, out: &mut Out) -> Result<()> {
    if a.part == "suggest" {
        // `pxl pattern suggest PART FRAME`: the positionals arrive shifted by one
        return match &a.snippet {
            Some(frame) => crate::texture::run_suggest(&a.frame, frame, a, out),
            None => Err(PxlError::usage("usage: pxl pattern suggest PART FRAME [--region x,y,w,h | --tile N | --tiles 37-40] [--max-period N] [--save NAME]")),
        };
    }
    let op = Op::Pattern { frame: a.frame.clone(), snippet: a.snippet.clone(), unit: a.unit, region: a.region, phase: a.phase.as_ref().map(|p| (p.x, p.y)), mask: a.mask.clone() };
    execute(&a.part, &[op], &a.flags, "pattern", out)
}
pub fn run_move(a: &MoveArgs, out: &mut Out) -> Result<()> {
    let op = Op::Move { frame: a.frame.clone(), region: Some(a.region), by: (a.by.x, a.by.y), fill: a.fill.clone() };
    execute(&a.part, &[op], &a.flags, "move", out)
}
pub fn run_nudge(a: &NudgeArgs, out: &mut Out) -> Result<()> {
    let ops: Vec<Op> = select_names(&a.part, &a.frame)?.into_iter().map(|frame| Op::Move { frame, region: None, by: (a.by.x, a.by.y), fill: None }).collect();
    execute(&a.part, &ops, &a.flags, "nudge", out)
}
pub fn run_outline(a: &OutlineArgs, out: &mut Out) -> Result<()> {
    let ops: Vec<Op> = select_names(&a.part, &a.frame)?.into_iter().map(|frame| Op::Outline { frame, key: a.key.clone() }).collect();
    execute(&a.part, &ops, &a.flags, "outline", out)
}
pub fn run_fix(a: &FixArgs, out: &mut Out) -> Result<()> {
    for r in &a.rules {
        if !["orphan", "jaggy", "eye"].contains(&r.as_str()) {
            return Err(PxlError::usage(format!("unknown fix rule '{}'", r)).with_hint("rules: orphan, jaggy, eye"));
        }
    }
    let ops: Vec<Op> = select_names(&a.part, &a.frame)?.into_iter().map(|frame| Op::Fix { frame, rules: a.rules.clone() }).collect();
    execute(&a.part, &ops, &a.flags, "fix", out)
}

pub fn run_write_grid(a: &WriteGridArgs, out: &mut Out) -> Result<()> {
    let op = Op::WriteGrid { frame: a.frame.clone(), grid: read_text_arg(&a.grid)? };
    execute(&a.part, &[op], &a.flags, "write_grid", out)
}

// ------------------------------------------------------------------ apply (JSON)

/// Shown by `pxl apply --help`: every op with its fields.
pub const APPLY_HELP: &str = r#"OPS FILE: a JSON array of ops, or {"ops": [...]}. "-" reads stdin. All ops run in order on one copy,
ONE undo step, any error aborts the whole batch; unknown fields are errors. Every op has "op" and (except copy) "frame".
Points are [x,y] or "x,y"; regions [x,y,w,h] or "x,y,w,h" (x,y = top-left, w,h = size); keys are template key
characters or role names ("W", "white"); grids are a string with 
 or an array of row strings (space = unchanged).

  set         pixels:["17,11=W", {"x":18,"y":11,"key":"white"}]
  row         x, y (first column, row), pixels:"1222221"        (space = unchanged; the run must fit the canvas)
  patch       x?, y?, grid                                       (x,y needed for plain rows; pasted `pxl show` text places itself)
  line        from:[x,y], to:[x,y], key
  rect        region:[x,y,w,h], key, outline?:false              (outline:true = border only)
  fill        at:[x,y], key                                      (4-connected flood fill)
  replace     maps:["2=1"] or {"2":"1"}, region?:[x,y,w,h]       (region default: whole frame)
  copy        from:FRAME, to:FRAME, region?:[x,y,w,h], to_at?:[x,y], skip_transparent?:false
  mirror      axis:"x"|"y", keep?:"low"|"high", region?
  flip        axis:"x"|"y", region?
  write_grid  grid                                               (replaces the whole frame)
  stamp       name:SNIPPET, at:[x,y], flip?:"x"|"y"              (paste a saved snippet)
  move        by:[dx,dy], region?:[x,y,w,h], fill?:KEY           (vacated cells become '.' or fill)
  nudge       by:[dx,dy]                                         (shift the whole frame)
  outline     key?:KEY                                           (re-close the outline ring)
  fix         rules?:["orphan","jaggy","eye"]                    (apply the lint fixes for the frame)
  pattern     snippet:NAME | unit:[x,y,w,h], region?:[x,y,w,h], phase?:[px,py], mask?:"1,2"
              (repeat the unit over the region; cell (x,y) = unit[(y-py) mod h][(x-px) mod w]; phase default = the
              part origin for sprites, 0,0 for tile sets and screens; mask = only over cells currently holding these keys)
  pick        method:"vote"|"contour"|"exemplar"|"auto"|"nearest", region?:[x,y,w,h]   (re-reduce from the reference)

EXAMPLE: [{"op":"set","frame":"01_stand","pixels":["17,11=W","18,11=white"]},
          {"op":"row","frame":"01_stand","x":16,"y":13,"pixels":"1222221"},
          {"op":"copy","from":"01_stand","to":"02_stand2","region":[9,6,14,10]}]"#;

const OP_FIELDS: &[(&str, &[&str])] = &[
    ("set", &["op", "frame", "pixels"]),
    ("row", &["op", "frame", "x", "y", "pixels"]),
    ("patch", &["op", "frame", "x", "y", "grid"]),
    ("stamp", &["op", "frame", "name", "at", "flip"]),
    ("move", &["op", "frame", "region", "by", "fill"]),
    ("nudge", &["op", "frame", "by"]),
    ("outline", &["op", "frame", "key"]),
    ("fix", &["op", "frame", "rules"]),
    ("pattern", &["op", "frame", "snippet", "unit", "region", "phase", "mask"]),
    ("line", &["op", "frame", "from", "to", "key"]),
    ("rect", &["op", "frame", "region", "key", "outline"]),
    ("fill", &["op", "frame", "at", "key"]),
    ("replace", &["op", "frame", "maps", "region"]),
    ("copy", &["op", "from", "to", "region", "to_at", "skip_transparent"]),
    ("mirror", &["op", "frame", "axis", "keep", "region"]),
    ("flip", &["op", "frame", "axis", "region"]),
    ("write_grid", &["op", "frame", "grid"]),
    ("pick", &["op", "frame", "method", "region"]),
];

struct P<'a> {
    i: usize,
    o: &'a serde_json::Map<String, Value>,
    kind: &'a str,
}

impl<'a> P<'a> {
    fn err(&self, m: impl AsRef<str>) -> PxlError {
        PxlError::validation(format!("ops[{}] ({}): {}", self.i, self.kind, m.as_ref())).with_hint("see the apply schema in `pxl guide` / src/edit.rs docs")
    }
    fn req(&self, k: &str) -> Result<&'a Value> {
        self.o.get(k).ok_or_else(|| self.err(format!("missing field '{}'", k)))
    }
    fn string(&self, k: &str) -> Result<String> {
        self.req(k)?.as_str().map(|s| s.to_string()).ok_or_else(|| self.err(format!("'{}' must be a string", k)))
    }
    fn int(&self, k: &str) -> Result<i32> {
        self.req(k)?.as_i64().map(|n| n as i32).ok_or_else(|| self.err(format!("'{}' must be an integer", k)))
    }
    fn ints(&self, v: &Value, n: usize, k: &str) -> Result<Vec<i32>> {
        let r: Option<Vec<i32>> = match v {
            Value::Array(a) => a.iter().map(|x| x.as_i64().map(|n| n as i32)).collect(),
            Value::String(s) => s.split(',').map(|p| p.trim().parse::<i32>().ok()).collect(),
            _ => None,
        };
        match r {
            Some(v) if v.len() == n => Ok(v),
            _ => Err(self.err(format!("'{}' must be {} integers ([..] or \"a,b,..\")", k, n))),
        }
    }
    fn point(&self, k: &str) -> Result<(i32, i32)> {
        let v = self.ints(self.req(k)?, 2, k)?;
        Ok((v[0], v[1]))
    }
    fn region_of(&self, v: &Value, k: &str) -> Result<Region> {
        let v = self.ints(v, 4, k)?;
        if v[2] < 0 || v[3] < 0 {
            return Err(self.err(format!("'{}' has a negative size", k)));
        }
        Ok(Region { x: v[0], y: v[1], w: v[2] as usize, h: v[3] as usize })
    }
    fn region(&self, k: &str) -> Result<Region> {
        self.region_of(self.req(k)?, k)
    }
    fn opt_region(&self, k: &str) -> Result<Option<Region>> {
        self.o.get(k).filter(|v| !v.is_null()).map(|v| self.region_of(v, k)).transpose()
    }
    fn text(&self, k: &str) -> Result<String> {
        match self.req(k)? {
            Value::String(s) => Ok(s.clone()),
            Value::Array(a) => a.iter().map(|r| r.as_str().map(|s| s.to_string()).ok_or_else(|| self.err(format!("'{}' rows must be strings", k)))).collect::<Result<Vec<_>>>().map(|v| v.join("\n")),
            _ => Err(self.err(format!("'{}' must be a string or an array of row strings", k))),
        }
    }
    fn opt_int(&self, k: &str) -> Result<Option<i32>> {
        match self.o.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(_) => self.int(k).map(Some),
        }
    }
    fn axis(&self) -> Result<Axis> {
        self.axis_of("axis")
    }
    fn axis_of(&self, k: &str) -> Result<Axis> {
        match self.string(k)?.to_lowercase().as_str() {
            "x" => Ok(Axis::X),
            "y" => Ok(Axis::Y),
            o => Err(self.err(format!("{} must be \"x\" or \"y\", not \"{}\"", k, o))),
        }
    }
    fn flag(&self, k: &str) -> Result<bool> {
        match self.o.get(k) {
            None | Some(Value::Null) => Ok(false),
            Some(v) => v.as_bool().ok_or_else(|| self.err(format!("'{}' must be true or false", k))),
        }
    }
}

/// Parse an `apply` document into ops.
pub fn parse_ops(text: &str) -> Result<Vec<Op>> {
    let v: Value = serde_json::from_str(text)?;
    let arr = match &v {
        Value::Array(a) => a,
        Value::Object(o) => {
            for k in o.keys() {
                if k != "ops" {
                    return Err(PxlError::validation(format!("unknown top-level field '{}' (only \"ops\" is allowed)", k)));
                }
            }
            o.get("ops").and_then(|x| x.as_array()).ok_or_else(|| PxlError::validation("\"ops\" must be an array"))?
        }
        _ => return Err(PxlError::validation("apply expects {\"ops\": [...]} or an array of ops")),
    };
    let mut ops = Vec::new();
    for (i, item) in arr.iter().enumerate() {
        let o = item.as_object().ok_or_else(|| PxlError::validation(format!("ops[{}] must be an object", i)))?;
        let kind = o.get("op").and_then(|x| x.as_str()).ok_or_else(|| PxlError::validation(format!("ops[{}]: missing string field 'op'", i)))?;
        let allowed = OP_FIELDS
            .iter()
            .find(|(n, _)| *n == kind)
            .ok_or_else(|| PxlError::validation(format!("ops[{}]: unknown op '{}'", i, kind)).with_hint("ops: set row patch line rect fill replace copy mirror flip write_grid pick stamp move nudge outline fix pattern"))?
            .1;
        let p = P { i, o, kind };
        for k in o.keys() {
            if !allowed.contains(&k.as_str()) {
                return Err(p.err(format!("unknown field '{}' (allowed: {})", k, allowed.join(", "))));
            }
        }
        let op = match kind {
            "set" => {
                let mut pixels = Vec::new();
                for it in p.req("pixels")?.as_array().ok_or_else(|| p.err("'pixels' must be an array"))? {
                    match it {
                        Value::String(s) => {
                            let px: Pixel = s.parse().map_err(|e: String| p.err(format!("pixel '{}': {}", s, e)))?;
                            pixels.push((px.x, px.y, px.key));
                        }
                        Value::Object(m) => {
                            let g = |k: &str| m.get(k).and_then(|x| x.as_i64()).map(|n| n as i32).ok_or_else(|| p.err(format!("pixel object needs integer '{}'", k)));
                            let key = m.get("key").and_then(|x| x.as_str()).ok_or_else(|| p.err("pixel object needs string 'key'"))?;
                            pixels.push((g("x")?, g("y")?, key.to_string()));
                        }
                        _ => return Err(p.err("pixels are \"x,y=KEY\" strings or {x,y,key} objects")),
                    }
                }
                Op::Set { frame: p.string("frame")?, pixels }
            }
            "row" => Op::Row { frame: p.string("frame")?, x: p.int("x")?, y: p.int("y")?, text: p.string("pixels")? },
            "patch" => Op::Patch { frame: p.string("frame")?, x: p.opt_int("x")?, y: p.opt_int("y")?, grid: p.text("grid")? },
            "stamp" => Op::Stamp {
                frame: p.string("frame")?,
                name: p.string("name")?,
                at: p.point("at")?,
                flip: match o.get("flip") {
                    None | Some(Value::Null) => None,
                    Some(_) => Some(p.axis_of("flip")?),
                },
            },
            "move" => Op::Move {
                frame: p.string("frame")?,
                region: p.opt_region("region")?,
                by: p.point("by")?,
                fill: match o.get("fill") {
                    None | Some(Value::Null) => None,
                    Some(_) => Some(p.string("fill")?),
                },
            },
            "pattern" => Op::Pattern {
                frame: p.string("frame")?,
                snippet: match o.get("snippet") {
                    None | Some(Value::Null) => None,
                    Some(_) => Some(p.string("snippet")?),
                },
                unit: p.opt_region("unit")?,
                region: p.opt_region("region")?,
                phase: match o.get("phase") {
                    None | Some(Value::Null) => None,
                    Some(_) => Some(p.point("phase")?),
                },
                mask: match o.get("mask") {
                    None | Some(Value::Null) => None,
                    Some(_) => Some(p.string("mask")?),
                },
            },
            "nudge" => Op::Move { frame: p.string("frame")?, region: None, by: p.point("by")?, fill: None },
            "outline" => Op::Outline {
                frame: p.string("frame")?,
                key: match o.get("key") {
                    None | Some(Value::Null) => None,
                    Some(_) => Some(p.string("key")?),
                },
            },
            "fix" => Op::Fix {
                frame: p.string("frame")?,
                rules: match o.get("rules") {
                    None | Some(Value::Null) => Vec::new(),
                    Some(Value::Array(a)) => a.iter().map(|r| r.as_str().map(|s| s.to_string()).ok_or_else(|| p.err("'rules' entries must be strings"))).collect::<Result<Vec<_>>>()?,
                    Some(Value::String(s)) => s.split(',').map(|x| x.trim().to_string()).collect(),
                    _ => return Err(p.err("'rules' must be [\"orphan\",\"jaggy\",\"eye\"]")),
                },
            },
            "line" => Op::Line { frame: p.string("frame")?, from: p.point("from")?, to: p.point("to")?, key: p.string("key")? },
            "rect" => Op::Rect { frame: p.string("frame")?, region: p.region("region")?, key: p.string("key")?, outline: p.flag("outline")? },
            "fill" => Op::Fill { frame: p.string("frame")?, at: p.point("at")?, key: p.string("key")? },
            "replace" => {
                let mut maps = Vec::new();
                match p.req("maps")? {
                    Value::Array(a) => {
                        for m in a {
                            let s = m.as_str().ok_or_else(|| p.err("maps entries are \"A=B\" strings"))?;
                            let km: KeyMap = s.parse().map_err(|e: String| p.err(format!("map '{}': {}", s, e)))?;
                            maps.push((km.from, km.to));
                        }
                    }
                    Value::Object(m) => {
                        for (k, v) in m {
                            maps.push((k.clone(), v.as_str().ok_or_else(|| p.err("map values must be strings"))?.to_string()));
                        }
                    }
                    _ => return Err(p.err("'maps' must be [\"A=B\"] or {\"A\":\"B\"}")),
                }
                Op::Replace { frame: p.string("frame")?, maps, region: p.opt_region("region")? }
            }
            "copy" => Op::Copy {
                from: p.string("from")?,
                to: p.string("to")?,
                region: p.opt_region("region")?,
                to_at: if o.get("to_at").map_or(false, |v| !v.is_null()) { Some(p.point("to_at")?) } else { None },
                skip_transparent: p.flag("skip_transparent")?,
            },
            "mirror" => {
                let keep = match o.get("keep") {
                    None | Some(Value::Null) => "low".to_string(),
                    Some(_) => p.string("keep")?,
                };
                let keep_high = match keep.as_str() {
                    "low" => false,
                    "high" => true,
                    other => return Err(p.err(format!("keep must be \"low\" or \"high\", not \"{}\"", other))),
                };
                Op::Mirror { frame: p.string("frame")?, axis: p.axis()?, keep_high, region: p.opt_region("region")? }
            }
            "flip" => Op::Flip { frame: p.string("frame")?, axis: p.axis()?, region: p.opt_region("region")? },
            "write_grid" => Op::WriteGrid { frame: p.string("frame")?, grid: p.text("grid")? },
            "pick" => {
                let m = p.string("method")?;
                let method = Method::parse(&m).ok_or_else(|| p.err(format!("unknown method '{}' (vote, contour, exemplar, auto, nearest)", m)))?;
                Op::Pick { frame: p.string("frame")?, method, region: p.opt_region("region")? }
            }
            _ => unreachable!(),
        };
        ops.push(op);
    }
    Ok(ops)
}

pub fn run_apply(a: &ApplyArgs, out: &mut Out) -> Result<()> {
    let text = if a.ops == "-" {
        read_text_arg("-")?
    } else {
        fsutil::read_string(std::path::Path::new(&a.ops))?
    };
    let ops = parse_ops(&text)?;
    if ops.is_empty() {
        return Err(PxlError::usage("apply: the ops list is empty"));
    }
    execute(&a.part, &ops, &a.flags, "apply", out)
}

// ------------------------------------------------------------------ clean

#[derive(Clone, Debug)]
pub struct Finding {
    pub kind: &'static str,
    pub x: i32,
    pub y: i32,
    pub key: u8,
    pub fix: Option<u8>,
}

fn same4(g: &Grid, x: i32, y: i32, k: u8) -> (bool, bool, bool, bool) {
    (g.get(x - 1, y) == k && x > 0, g.get(x + 1, y) == k && g.in_bounds(x + 1, y), g.get(x, y - 1) == k && y > 0, g.get(x, y + 1) == k && g.in_bounds(x, y + 1))
}

/// Elbow pixel: exactly two same-key 4-neighbours, one horizontal and one vertical.
/// Returns (dx, dy) pointing from the pixel to those neighbours.
fn elbow(g: &Grid, x: i32, y: i32) -> Option<(i32, i32)> {
    let k = g.get(x, y);
    if k == b'.' {
        return None;
    }
    let (l, r, u, d) = same4(g, x, y, k);
    if (l as u8 + r as u8 + u as u8 + d as u8) != 2 || !(l || r) || !(u || d) {
        return None;
    }
    Some((if l { -1 } else { 1 }, if u { -1 } else { 1 }))
}

/// Fix stray single pixels (no non-transparent 8-neighbour) and doubled-diagonal elbows in place.
pub fn clean_grid(g: &mut Grid, orphans: bool, jaggies: bool) -> Vec<Finding> {
    let mut found = Vec::new();
    if orphans {
        for y in 0..g.h as i32 {
            for x in 0..g.w as i32 {
                let k = g.get(x, y);
                if k == b'.' {
                    continue;
                }
                let mut lone = true;
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        if (dx != 0 || dy != 0) && g.get(x + dx, y + dy) != b'.' {
                            lone = false;
                        }
                    }
                }
                if lone {
                    g.set(x, y, b'.');
                    found.push(Finding { kind: "orphan", x, y, key: k, fix: Some(b'.') });
                }
            }
        }
    }
    if jaggies {
        let snap = g.clone();
        let mut removed = vec![false; g.w * g.h];
        for y in 0..g.h as i32 {
            for x in 0..g.w as i32 {
                let Some((dx, dy)) = elbow(&snap, x, y) else { continue };
                let k = snap.get(x, y);
                let (ax, ay, cx, cy) = (x + dx, y, x, y + dy);
                // a doubled diagonal: a neighbouring pixel of the chain is an elbow too; a lone corner stays
                if elbow(&snap, ax, ay).is_none() && elbow(&snap, cx, cy).is_none() {
                    continue;
                }
                if snap.get(x + dx, y + dy) == k {
                    continue; // 2x2 block, not a line
                }
                if removed[ay as usize * g.w + ax as usize] || removed[cy as usize * g.w + cx as usize] {
                    continue;
                }
                let (o1, o2) = (snap.get(x - dx, y), snap.get(x, y - dy));
                let fix = (o1 == o2).then_some(o1);
                found.push(Finding { kind: "jaggy", x, y, key: k, fix });
                if let Some(f) = fix {
                    g.set(x, y, f);
                    removed[y as usize * g.w + x as usize] = true;
                }
            }
        }
    }
    found
}

pub fn run_clean(a: &CleanArgs, out: &mut Out) -> Result<()> {
    if !a.orphans && !a.jaggies {
        return Err(PxlError::usage("clean needs --orphans and/or --jaggies").with_hint("e.g. pxl clean player --orphans --jaggies --dry-run"));
    }
    let mut c = Ctx::open(&a.part, out.agent(), !a.flags.dry_run, a.flags.force_approved)?;
    let names: Vec<String> = a.sel.select(&c.part)?.iter().map(|f| f.name.clone()).collect();
    let mut lines = Vec::new();
    for n in names {
        let fi = c.fidx(&n)?;
        let mut g = c.part.frames[fi].grid.clone();
        let found = clean_grid(&mut g, a.orphans, a.jaggies);
        if found.is_empty() {
            continue;
        }
        for f in &found {
            let fix = match f.fix {
                Some(k) => format!("-> '{}'", k as char),
                None => "(no automatic fix)".to_string(),
            };
            lines.push(format!("{} {}/{}: {} at {},{} '{}' {}", if a.flags.dry_run { "found" } else { "cleaned" }, c.part.name, n, f.kind, f.x, f.y, f.key as char, fix));
        }
        c.touch(fi);
        c.part.frames[fi].grid = g;
    }
    if lines.len() > 40 {
        let n = lines.len();
        lines.truncate(40);
        lines.push(format!("... {} findings in total", n));
    }
    c.finish("clean", &a.flags, out, Some(("findings", lines)))
}
