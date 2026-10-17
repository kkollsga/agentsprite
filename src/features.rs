//! Key features of a sprite (DESIGN 1.1): eyes (and their pupils), found generically on the native reference,
//! checked on a reduced grid and stamped back when a reduction lost them.
//!
//! **Detection** (`detect`): an eye is a compact light region (eye white: Oklab L >= 0.9, nearly neutral, an
//! area between one cell and about 40 cells, a roughly filled bounding box) that has a dark core (pupil: L <=
//! 0.28) either inside its bounding box (a bite or hole) or touching it on at most about half of its border
//! (a white glove with a dark outline all around is not an eye). Each feature records the native pixels, the
//! centres in target cell units and the *gaze*: pupil centre minus white centre, in cells.
//!
//! **Survival** (`eye_ok`): near the eye's cells (window of one cell around) the grid has a white key cell AND
//! a pupil key cell within one cell of a white cell whose offset from the white centre matches the gaze within
//! one cell on each axis. Keys: template `[features] eye = { white = "W", pupil = "1" }`, else the lightest key
//! (white) and the `outline` role or darkest key (pupil).
//!
//! **Stamping** (`stamp`): for every eye that did not survive, draw a minimal clean eye (white cells sized from
//! the reference eye at target scale, a pupil cell placed by gaze). Only opaque cells are recoloured, so the
//! silhouette never changes; the white avoids the outline ring. An exemplar frame with a similar eye (same
//! size, similar gaze) is copied first when the caller provides one. Deterministic.

use crate::color::{self, Rgb};
use crate::img::RgbaImage;
use crate::model::{Grid, Part, RefFrame};
use crate::template::Template;

pub const TRANSPARENT: u8 = b'.';
/// Oklab lightness of an eye white and of a pupil.
const WHITE_L: f64 = 0.9;
const WHITE_CHROMA: f64 = 0.06;
const DARK_L: f64 = 0.28;
/// Pupil offset tolerance (cells, per axis) against the reference gaze.
const TOL: f64 = 0.85;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geo {
    pub org: (i32, i32),
    pub f: i32,
}

impl Geo {
    /// Cell geometry of a frame (None for area reduction to an explicit target size).
    pub fn of(part: &Part, rf: &RefFrame) -> Option<Geo> {
        if part.input.target.is_some() && part.reduce.factor.or(part.input.factor).is_none() {
            return None;
        }
        let g = crate::metrics::geometry(part, rf);
        Some(Geo { org: g.org, f: g.f })
    }
}

#[derive(Clone, Debug)]
pub struct Feature {
    /// Native pixels of the eye white and of the pupil.
    pub white: Vec<(i32, i32)>,
    pub pupil: Vec<(i32, i32)>,
    /// Native bbox of the white, inclusive.
    pub wbox: (i32, i32, i32, i32),
    /// Centres in cell units (cell (0,0) spans 0..1).
    pub wc: (f64, f64),
    pub pc: (f64, f64),
    pub gaze: (f64, f64),
    /// Inclusive cell bbox of white and pupil.
    pub cells: (i32, i32, i32, i32),
    /// Native white / pupil pixels per cell: `(cx, cy, count)`, and pixels per cell (`f*f`).
    pub wcells: Vec<(i32, i32, u32)>,
    pub pcells: Vec<(i32, i32, u32)>,
    pub cell_px: u32,
}

impl Feature {
    /// Size of the reference eye white in cells (rounded, at least 1).
    pub fn white_cells(&self, f: i32) -> (i32, i32) {
        let w = self.wbox.2 - self.wbox.0 + 1;
        let h = self.wbox.3 - self.wbox.1 + 1;
        (((w as f64 / f as f64).round() as i32).clamp(1, 4), ((h as f64 / f as f64).round() as i32).clamp(1, 5))
    }
}

/// Output keys that draw an eye.
#[derive(Clone, Copy, Debug)]
pub struct Keys {
    pub white: u8,
    pub pupil: u8,
}

fn lab(c: Rgb) -> color::Oklab {
    color::srgb_to_oklab(c)
}

/// Eye keys of a template: the `[features]` hint, else lightest key / outline (or darkest) key.
pub fn keys_for(tpl: &Template) -> Option<Keys> {
    let white = tpl.features.eye_white.or_else(|| {
        let mut best: Option<(u8, f64)> = None;
        for k in tpl.keys() {
            if let Some(c) = k.color {
                let l = lab(c).l;
                if best.map_or(true, |b| l > b.1 + 1e-9) {
                    best = Some((k.key, l));
                }
            }
        }
        best.filter(|b| b.1 >= 0.75).map(|b| b.0)
    })?;
    let pupil = tpl.features.eye_pupil.or_else(|| {
        tpl.key_for_role("outline").or_else(|| {
            let mut best: Option<(u8, f64)> = None;
            for k in tpl.keys() {
                if let Some(c) = k.color {
                    let l = lab(c).l;
                    if best.map_or(true, |b| l < b.1 - 1e-9) {
                        best = Some((k.key, l));
                    }
                }
            }
            best.map(|b| b.0)
        })
    })?;
    if white == pupil {
        return None;
    }
    Some(Keys { white, pupil })
}

const N8: [(i32, i32); 8] = [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)];
const N4: [(i32, i32); 4] = [(0, -1), (-1, 0), (1, 0), (0, 1)];

/// Eyes of a native reference, in reading order.
pub fn detect(img: &RgbaImage, g: Geo) -> Vec<Feature> {
    let (w, h) = (img.w as i32, img.h as i32);
    let f = g.f.max(1);
    // 0 other, 1 light, 2 dark
    let mut cls = vec![0u8; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            let p = img.get(x, y);
            if p.a < 128 {
                continue;
            }
            let o = lab(p.rgb());
            let ch = (o.a * o.a + o.b * o.b).sqrt();
            cls[(y * w + x) as usize] = if o.l >= WHITE_L && ch <= WHITE_CHROMA {
                1
            } else if o.l <= DARK_L {
                2
            } else if o.l >= 0.88 && ch >= 0.15 && ch <= 0.25 {
                3 // a bright iris (yellow, pale blue) beside the white: part of the eye, never an eye on its own
            } else {
                0
            };
        }
    }
    let at = |x: i32, y: i32| -> u8 {
        if x < 0 || y < 0 || x >= w || y >= h {
            0
        } else {
            cls[(y * w + x) as usize]
        }
    };
    let mut seen = vec![false; (w * h) as usize];
    let mut out = Vec::new();
    let mut irises: Vec<bool> = Vec::new();
    for sy in 0..h {
        for sx in 0..w {
            if at(sx, sy) != 1 || seen[(sy * w + sx) as usize] {
                continue;
            }
            let mut comp = vec![(sx, sy)];
            seen[(sy * w + sx) as usize] = true;
            let mut k = 0;
            while k < comp.len() {
                let (px, py) = comp[k];
                k += 1;
                for (dx, dy) in N8 {
                    let (qx, qy) = (px + dx, py + dy);
                    if (at(qx, qy) == 1 || at(qx, qy) == 3) && !seen[(qy * w + qx) as usize] {
                        seen[(qy * w + qx) as usize] = true;
                        comp.push((qx, qy));
                    }
                }
            }
            let n = comp.len() as i32;
            // an eye white is blocky: it holds a full f x f block of light pixels (at most 2 x 2); diagonal glints and sparkles do not
            if f >= 2 {
                let m = f.min(2);
                let light = |x: i32, y: i32| matches!(at(x, y), 1 | 3) && comp.contains(&(x, y));
                if !comp.iter().any(|&(x, y)| (0..m).all(|dy| (0..m).all(|dx| light(x + dx, y + dy)))) {
                    continue;
                }
            }
            let bb = comp.iter().fold((i32::MAX, i32::MAX, i32::MIN, i32::MIN), |b, &(x, y)| (b.0.min(x), b.1.min(y), b.2.max(x), b.3.max(y)));
            let (bw, bh) = (bb.2 - bb.0 + 1, bb.3 - bb.1 + 1);
            if n < f * f || bw > 7 * f || bh > 7 * f || n > 40 * f * f || n * 100 < 30 * bw * bh || bw.max(bh) > 3 * bw.min(bh) + f {
                continue;
            }
            let mut sorted: Vec<(i32, i32)> = comp.iter().map(|&(x, y)| (y, x)).collect();
            sorted.sort_unstable();
            let is_white = |x: i32, y: i32| sorted.binary_search(&(y, x)).is_ok();
            // border of the white
            let (mut border, mut dark_border) = (0i32, 0i32);
            let mut bseen: Vec<(i32, i32)> = Vec::new();
            let mut clear_border = 0i32;
            for &(x, y) in &comp {
                for (dx, dy) in N4 {
                    let (qx, qy) = (x + dx, y + dy);
                    if !is_white(qx, qy) && !bseen.contains(&(qx, qy)) {
                        bseen.push((qx, qy));
                        border += 1;
                        if at(qx, qy) == 2 {
                            dark_border += 1;
                        }
                        if qx < 0 || qy < 0 || qx >= w || qy >= h || img.get(qx, qy).a < 128 {
                            clear_border += 1;
                        }
                    }
                }
            }
            // pupil: dark pixels touching the white, grown inside the window bbox +- f
            let (wx0, wy0, wx1, wy1) = (bb.0 - f, bb.1 - f, bb.2 + f, bb.3 + f);
            let mut pupil: Vec<(i32, i32)> = Vec::new();
            for &(x, y) in &comp {
                for (dx, dy) in N8 {
                    let (qx, qy) = (x + dx, y + dy);
                    if at(qx, qy) == 2 && !pupil.contains(&(qx, qy)) {
                        pupil.push((qx, qy));
                    }
                }
            }
            let mut k = 0;
            while k < pupil.len() {
                let (px, py) = pupil[k];
                k += 1;
                for (dx, dy) in N8 {
                    let (qx, qy) = (px + dx, py + dy);
                    if qx >= wx0 && qx <= wx1 && qy >= wy0 && qy <= wy1 && at(qx, qy) == 2 && !pupil.contains(&(qx, qy)) {
                        pupil.push((qx, qy));
                    }
                }
            }
            let min_p = (f * f / 2).max(1);
            let inside = pupil.iter().filter(|&&(x, y)| x >= bb.0 && x <= bb.2 && y >= bb.1 && y <= bb.3).count() as i32;
            let frac = if border == 0 { 1.0 } else { dark_border as f64 / border as f64 };
            let ok = clear_border * 10 <= border && pupil.len() as i32 >= min_p && pupil.len() as i32 <= 2 * n && (inside > 0 || frac <= 0.55);
            if !ok {
                continue;
            }
            // a real pupil is a core: dark pixels inside the white's bounding box (a bite or hole), a bar flush
            // against the white, or else the white borders the dark region on at least two sides, 
            let mut sides = [false; 4];
            let in_box = |x: i32, y: i32| x >= bb.0 && x <= bb.2 && y >= bb.1 && y <= bb.3;
            let core: Vec<(i32, i32)> = pupil.iter().copied().filter(|&(x, y)| in_box(x, y)).collect();
            let has_core = !core.is_empty();
            let core = if has_core { core } else { pupil.clone() };
            for &(x, y) in &core {
                for (i, (dx, dy)) in N4.iter().enumerate() {
                    if is_white(x + dx, y + dy) {
                        sides[i] = true;
                    }
                }
            }
            // (a bar flush against the white keeps within its extent on one axis; an outline wrapping a patch does not)
            let pb = pupil.iter().fold((i32::MAX, i32::MAX, i32::MIN, i32::MIN), |b, &(x, y)| (b.0.min(x), b.1.min(y), b.2.max(x), b.3.max(y)));
            let flush = (pb.0 >= bb.0 - 1 && pb.2 <= bb.2 + 1) || (pb.1 >= bb.1 - 1 && pb.3 <= bb.3 + 1);
            // (a white with a coloured iris beside it keeps the flush-pupil allowance; a lone white highlight beside an outline does not)
            // (or a white of at least two cells: a one-cell white beside an outline is a highlight)
            let iris = comp.iter().any(|&(x, y)| at(x, y) == 3) || n >= 2 * f * f;
            if !has_core && !(flush && iris) && sides.iter().filter(|s| **s).count() < 2 {
                continue;
            }
            // a lone highlight beside the outline is not an eye: its "pupil" is the outline itself (a dark region far larger than any pupil)
            // and no dark pixel lies inside the white's bounding box
            if !has_core && !(flush && iris) {
                let mut dseen = std::collections::HashSet::new();
                let mut st: Vec<(i32, i32)> = pupil.iter().copied().take(1).collect();
                for p in &st {
                    dseen.insert(*p);
                }
                let mut dn = 0;
                while let Some((cx, cy)) = st.pop() {
                    dn += 1;
                    if dn > 12 * f * f {
                        break;
                    }
                    for (dx, dy) in N8 {
                        let q = (cx + dx, cy + dy);
                        if at(q.0, q.1) == 2 && dseen.insert(q) {
                            st.push(q);
                        }
                    }
                }
                if dn > 12 * f * f {
                    continue;
                }
            }
            // it sits in the upper part of the blob (the opaque region) it belongs to
            if !in_upper_part(img, comp[0], bb) {
                    continue;
            }
            pupil.sort_by_key(|&(x, y)| (y, x));
            let mut white = comp.clone();
            white.sort_by_key(|&(x, y)| (y, x));
            let cen = |v: &[(i32, i32)]| -> (f64, f64) {
                let n = v.len() as f64;
                let (sx, sy) = v.iter().fold((0.0, 0.0), |a, &(x, y)| (a.0 + x as f64 + 0.5, a.1 + y as f64 + 0.5));
                (((sx / n) - g.org.0 as f64) / f as f64, ((sy / n) - g.org.1 as f64) / f as f64)
            };
            let (wc, pc) = (cen(&white), cen(&pupil));
            let cell = |x: i32, y: i32| ((x - g.org.0).div_euclid(f), (y - g.org.1).div_euclid(f));
            let mut cb = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
            for &(x, y) in white.iter().chain(pupil.iter()) {
                let (cx, cy) = cell(x, y);
                cb = (cb.0.min(cx), cb.1.min(cy), cb.2.max(cx), cb.3.max(cy));
            }
            let tally = |v: &[(i32, i32)]| -> Vec<(i32, i32, u32)> {
                let mut t: Vec<(i32, i32, u32)> = Vec::new();
                for &(x, y) in v {
                    let (cx, cy) = cell(x, y);
                    match t.iter_mut().find(|e| e.0 == cx && e.1 == cy) {
                        Some(e) => e.2 += 1,
                        None => t.push((cx, cy, 1)),
                    }
                }
                t.sort();
                t
            };
            let (wcells, pcells) = (tally(&white), tally(&pupil));
            irises.push(iris);
            out.push(Feature { white, pupil, wbox: bb, wc, pc, gaze: (pc.0 - wc.0, pc.1 - wc.1), cells: cb, wcells, pcells, cell_px: (f * f) as u32 });
        }
    }
    // eyes sit in the upper part of the sprite (gloves and shoe buckles do not): centre in the top 60% of the
    // opaque bbox; at most the two highest candidates
    if let Some((_, by0, _, by1)) = img.bbox() {
        let limit = by0 as f64 + 0.6 * (by1 + 1 - by0) as f64;
        // an eye with a coloured iris is surer evidence than a bare white: it may sit lower (a head under an antenna)
        let limit_iris = by0 as f64 + 0.75 * (by1 + 1 - by0) as f64;
        let mut k = 0;
        out.retain(|e: &Feature| {
            let c = (e.wbox.1 + e.wbox.3 + 1) as f64 / 2.0;
            let keep = c <= limit || (irises[k] && c <= limit_iris);
            k += 1;
            keep
        });
    }
    out.sort_by_key(|e: &Feature| (e.wbox.1, e.wbox.0));
    out.truncate(2);
    if out.len() == 2 {
        let (a, b) = (&out[0], &out[1]);
        let (ca, cb) = ((a.wbox.1 + a.wbox.3) / 2, (b.wbox.1 + b.wbox.3) / 2);
        let (na, nb) = (a.white.len(), b.white.len());
        if (ca - cb).abs() > 4 * f || nb * 4 < na || na * 4 < nb {
            let keep = if na >= nb { 0 } else { 1 };
            let k = out.swap_remove(keep);
            out = vec![k];
        }
    }
    out.sort_by_key(|e: &Feature| (e.wbox.1, e.wbox.0));
    out
}

/// Is the white (bbox `bb`) centred in the upper 65% of the opaque 8-connected blob containing `start`?
fn in_upper_part(img: &RgbaImage, start: (i32, i32), bb: (i32, i32, i32, i32)) -> bool {
    let (w, h) = (img.w as i32, img.h as i32);
    let mut seen = vec![false; (w * h) as usize];
    // the white's pixel itself is opaque: flood the opaque region from it
    let mut stack = vec![start];
    seen[(start.1 * w + start.0) as usize] = true;
    let (mut y0, mut y1) = (start.1, start.1);
    while let Some((x, y)) = stack.pop() {
        y0 = y0.min(y);
        y1 = y1.max(y);
        for (dx, dy) in N8 {
            let (qx, qy) = (x + dx, y + dy);
            if qx >= 0 && qy >= 0 && qx < w && qy < h && !seen[(qy * w + qx) as usize] && img.get(qx, qy).a >= 128 {
                seen[(qy * w + qx) as usize] = true;
                stack.push((qx, qy));
            }
        }
    }
    let centre = (bb.1 + bb.3 + 1) as f64 / 2.0;
    centre <= y0 as f64 + 0.65 * (y1 + 1 - y0) as f64
}

fn key_at(g: &Grid, x: i32, y: i32) -> u8 {
    if g.in_bounds(x, y) {
        g.get(x, y)
    } else {
        TRANSPARENT
    }
}

/// Does the grid draw this eye (white cell and a gaze-consistent pupil cell)?
pub fn eye_ok(grid: &Grid, e: &Feature, k: &Keys) -> bool {
    let (x0, y0, x1, y1) = e.cells;
    let mut whites: Vec<(i32, i32)> = Vec::new();
    for y in y0 - 1..=y1 + 1 {
        for x in x0 - 1..=x1 + 1 {
            if key_at(grid, x, y) == k.white {
                whites.push((x, y));
            }
        }
    }
    if whites.is_empty() {
        return false;
    }
    let n = whites.len() as f64;
    let wcx = whites.iter().map(|w| w.0 as f64 + 0.5).sum::<f64>() / n;
    let wcy = whites.iter().map(|w| w.1 as f64 + 0.5).sum::<f64>() / n;
    // a centred pupil (ring eye) has no side to be on: allow it half a cell more slack
    let tol = |g: f64| if g.abs() <= 0.5 { TOL + 0.5 } else { TOL };
    let (tol_x, tol_y) = (tol(e.gaze.0), tol(e.gaze.1));
    for y in y0 - 2..=y1 + 2 {
        for x in x0 - 2..=x1 + 2 {
            if key_at(grid, x, y) != k.pupil {
                continue;
            }
            if !whites.iter().any(|w| (w.0 - x).abs() <= 1 && (w.1 - y).abs() <= 1) {
                continue;
            }
            let (dx, dy) = (x as f64 + 0.5 - wcx, y as f64 + 0.5 - wcy);
            if (dx - e.gaze.0).abs() <= tol_x && (dy - e.gaze.1).abs() <= tol_y {
                return true;
            }
        }
    }
    false
}

/// Cells where the grid's eye differs from the reference eye at target scale: expected white cells that are not
/// the white key, white key cells that are not expected, expected pupil cells that are not the pupil key.
/// 0 for a drawing that matches the reference eye cell by cell.
pub fn eye_mismatch(grid: &Grid, e: &Feature, k: &Keys) -> usize {
    let expect = |v: &[(i32, i32, u32)]| -> Vec<(i32, i32)> {
        let mut r: Vec<(i32, i32)> = v.iter().filter(|c| c.2 * 5 >= 2 * e.cell_px).map(|c| (c.0, c.1)).collect();
        if r.is_empty() {
            if let Some(m) = v.iter().max_by_key(|c| (c.2, std::cmp::Reverse((c.1, c.0)))) {
                r.push((m.0, m.1));
            }
        }
        r
    };
    let (ew, ep) = (expect(&e.wcells), expect(&e.pcells));
    let (x0, y0, x1, y1) = e.cells;
    let mut miss = 0;
    for y in y0 - 1..=y1 + 1 {
        for x in x0 - 1..=x1 + 1 {
            let key = key_at(grid, x, y);
            let (xw, xp) = (ew.contains(&(x, y)), ep.contains(&(x, y)));
            if xw != (key == k.white) {
                miss += 1;
            }
            if xp && key != k.pupil {
                miss += 1;
            }
        }
    }
    miss
}

fn is_edge(g: &Grid, x: i32, y: i32) -> bool {
    N4.iter().any(|d| key_at(g, x + d.0, y + d.1) == TRANSPARENT)
}

/// An approved eye to copy from: a frame's eyes (detected on its reference) and its grid.
pub struct ExEye<'a> {
    pub eyes: &'a [Feature],
    pub grid: &'a Grid,
}

fn apply(g: &mut Grid, mods: &[(i32, i32, u8)]) {
    for &(x, y, k) in mods {
        if g.in_bounds(x, y) && g.get(x, y) != TRANSPARENT {
            g.set(x, y, k);
        }
    }
}

/// An eye white is never drawn smaller than 2 x 3 cells (3 x 2 for a wide eye): the rule of the pxl-sprite skill and the floor of style.md.
pub fn eye_size_floor(cw: i32, ch: i32) -> (i32, i32) {
    if ch >= cw {
        (cw.max(2), ch.max(3))
    } else {
        (cw.max(3), ch.max(2))
    }
}

/// Draw the eyes that did not survive. Returns the number of eyes stamped.
pub fn stamp(grid: &mut Grid, eyes: &[Feature], tpl: &Template, f: i32, exemplars: &[ExEye]) -> usize {
    let Some(k) = keys_for(tpl) else { return 0 };
    let mut done = 0;
    for e in eyes {
        if eye_ok(grid, e, &k) {
            continue;
        }
        let (cw, ch) = e.white_cells(f);
        let (cw, ch) = eye_size_floor(cw, ch);
        let mut variants: Vec<Vec<(i32, i32, u8)>> = Vec::new();
        // exemplar eye of the same size and a similar gaze
        for ex in exemplars {
            for xe in ex.eyes {
                if xe.white_cells(f) == e.white_cells(f) && (xe.gaze.0 - e.gaze.0).abs() <= 1.0 && (xe.gaze.1 - e.gaze.1).abs() <= 1.0 && eye_ok(ex.grid, xe, &k) {
                    let (dx, dy) = ((e.wc.0 - xe.wc.0).round() as i32, (e.wc.1 - xe.wc.1).round() as i32);
                    let mut m = Vec::new();
                    for y in xe.cells.1 - 1..=xe.cells.3 + 1 {
                        for x in xe.cells.0 - 1..=xe.cells.2 + 1 {
                            let c = key_at(ex.grid, x, y);
                            if c == k.white || c == k.pupil {
                                m.push((x + dx, y + dy, c));
                            }
                        }
                    }
                    variants.push(m);
                }
            }
        }
        let x0 = (e.wc.0 - cw as f64 / 2.0).round() as i32;
        let y0 = (e.wc.1 - ch as f64 / 2.0).round() as i32;
        let rect_ok = |x: i32, y: i32| -> bool { key_at(grid, x, y) == k.white || (key_at(grid, x, y) != TRANSPARENT && !is_edge(grid, x, y)) };
        let mut whites: Vec<(i32, i32, u8)> = Vec::new();
        for y in y0..y0 + ch {
            for x in x0..x0 + cw {
                if rect_ok(x, y) {
                    whites.push((x, y, k.white));
                }
            }
        }
        let sgn = |v: f64| if v >= 0.5 { 1 } else if v <= -0.5 { -1 } else { 0 };
        let (sx, sy) = (sgn(e.gaze.0), sgn(e.gaze.1));
        let pcell = (e.pc.0.floor() as i32, e.pc.1.floor() as i32);
        // A: white rect + pupil where the reference pupil is
        let mut a = whites.clone();
        a.push((pcell.0, pcell.1, k.pupil));
        variants.push(a);
        // B: pupil just outside the rect on the gaze side
        let bx = if sx > 0 { x0 + cw } else if sx < 0 { x0 - 1 } else { pcell.0.clamp(x0, x0 + cw - 1) };
        let by = if sy > 0 { y0 + ch } else if sy < 0 { y0 - 1 } else { pcell.1.clamp(y0, y0 + ch - 1) };
        let by = if sx != 0 && sy == 0 { pcell.1.clamp(y0, y0 + ch - 1) } else { by };
        let bx = if sy != 0 && sx == 0 { pcell.0.clamp(x0, x0 + cw - 1) } else { bx };
        let mut b = whites.clone();
        b.push((bx, by, k.pupil));
        variants.push(b);
        // C: pupil inside the rect on the gaze side
        let cx = if sx > 0 { x0 + cw - 1 } else if sx < 0 { x0 } else { pcell.0.clamp(x0, x0 + cw - 1) };
        let cy = if sy > 0 { y0 + ch - 1 } else if sy < 0 { y0 } else { pcell.1.clamp(y0, y0 + ch - 1) };
        let mut c = whites.clone();
        c.push((cx, cy, k.pupil));
        variants.push(c);
        // D: a wider white (one more column) with the pupil inside
        let mut d = whites.clone();
        for y in y0..y0 + ch {
            let x = if sx >= 0 { x0 - 1 } else { x0 + cw };
            if rect_ok(x, y) {
                d.push((x, y, k.white));
            }
        }
        d.push((cx, cy, k.pupil));
        variants.push(d);
        for v in variants {
            let mut t = grid.clone();
            apply(&mut t, &v);
            if eye_ok(&t, e, &k) {
                *grid = t;
                done += 1;
                break;
            }
        }
    }
    done
}

/// Draw every eye at its scaled position at a minimum readable size, whether or not the vote kept something eye-like: a white
/// rectangle (the reference white at target scale, at least 2 cells) with a pupil block (the reference pupil at target scale, at
/// least 1 cell, always leaving one white column or row beside it) on the gaze side. Only opaque cells change. Returns eyes drawn.
pub fn stamp_forced(grid: &mut Grid, eyes: &[Feature], tpl: &Template, f: i32) -> usize {
    let Some(k) = keys_for(tpl) else { return 0 };
    let mut done = 0;
    for e in eyes {
        let (cw, ch) = e.white_cells(f);
        let (cw, ch) = eye_size_floor(cw, ch);
        let pb = e.pupil.iter().fold((i32::MAX, i32::MAX, i32::MIN, i32::MIN), |b, &(x, y)| (b.0.min(x), b.1.min(y), b.2.max(x), b.3.max(y)));
        let (ppw, pph) = ((pb.2 - pb.0 + 1) as f64 / f as f64, (pb.3 - pb.1 + 1) as f64 / f as f64);
        let mut pw = (ppw.round() as i32).max(1);
        let mut ph = (pph.round() as i32).max(1);
        if pw >= cw && cw >= 2 {
            pw = cw - 1;
        }
        if ph > ch {
            ph = ch;
        }
        let x0 = (e.wc.0 - cw as f64 / 2.0).round() as i32;
        let y0 = (e.wc.1 - ch as f64 / 2.0).round() as i32;
        let rect_ok = |g: &Grid, x: i32, y: i32| -> bool { let c = key_at(g, x, y); c != TRANSPARENT && (c == k.white || !is_edge(g, x, y)) };
        let mut mods: Vec<(i32, i32, u8)> = Vec::new();
        for y in y0..y0 + ch {
            for x in x0..x0 + cw {
                // a round eye: the corners of a rectangle of 3x3 cells or more stay fill
                let corner = cw >= 3 && ch >= 3 && (x == x0 || x == x0 + cw - 1) && (y == y0 || y == y0 + ch - 1);
                if !corner && rect_ok(grid, x, y) {
                    mods.push((x, y, k.white));
                }
            }
        }
        // pupil block: on the gaze side horizontally, at the reference pupil's height
        let px0 = if e.gaze.0 > 0.2 { x0 + cw - pw } else if e.gaze.0 < -0.2 { x0 } else { x0 + (cw - pw) / 2 };
        let py0 = ((e.pc.1 - ph as f64 / 2.0).round() as i32).clamp(y0, y0 + ch - ph);
        for y in py0..py0 + ph {
            for x in px0..px0 + pw {
                mods.push((x, y, k.pupil));
            }
        }
        let mut t = grid.clone();
        // stray pupil-coloured cells of the vote around the eye (not on the silhouette) take the fill beside them
        let pset: Vec<(i32, i32)> = mods.iter().filter(|m| m.2 == k.pupil).map(|m| (m.0, m.1)).collect();
        for y in e.cells.1..=e.cells.3 {
            for x in e.cells.0..=e.cells.2 {
                if t.in_bounds(x, y) && t.get(x, y) == k.pupil && !pset.contains(&(x, y)) && !is_edge(&t, x, y) {
                    let mut tally: Vec<(u8, usize)> = Vec::new();
                    for d in N4 {
                        let c = key_at(&t, x + d.0, y + d.1);
                        if c != TRANSPARENT && c != k.pupil && c != k.white {
                            match tally.iter_mut().find(|e| e.0 == c) {
                                Some(e) => e.1 += 1,
                                None => tally.push((c, 1)),
                            }
                        }
                    }
                    if let Some(b) = tally.iter().max_by_key(|e| e.1) {
                        t.set(x, y, b.0);
                    }
                }
            }
        }
        apply(&mut t, &mods);
        *grid = t;
        done += 1;
    }
    done
}

/// Cell rectangle (x, y, w, h) to circle for an eye on sheets.
pub fn marker_rect(e: &Feature) -> (i32, i32, i32, i32) {
    (e.cells.0 - 1, e.cells.1 - 1, e.cells.2 - e.cells.0 + 3, e.cells.3 - e.cells.1 + 3)
}

/// Eyes of a reference frame (empty without a geometry).
pub fn eyes_of(part: &Part, rf: &RefFrame) -> Vec<Feature> {
    match Geo::of(part, rf) {
        Some(g) => detect(&rf.img, g),
        None => Vec::new(),
    }
}

/// `(found, lost eyes)` for a grid.
pub fn eye_status(grid: &Grid, eyes: &[Feature], tpl: &Template) -> (usize, Vec<usize>) {
    let Some(k) = keys_for(tpl) else { return (eyes.len(), Vec::new()) };
    let lost = eyes.iter().enumerate().filter(|(_, e)| !eye_ok(grid, e, &k)).map(|(i, _)| i).collect();
    (eyes.len(), lost)
}

/// Eye count of a frame against its reference (rule `eye_parity`): the eyes the source shows, the eyes the grid draws (white cell with a pupil on the
/// gaze side, see `eye_ok`) and the indexes of the lost ones.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EyeParity {
    pub source: usize,
    pub drawn: usize,
    pub lost: Vec<usize>,
}

/// Is the reference eye drawn in a rendered frame? Colour based, so it works with any template or palette-set keys: the art holds near-white cells
/// (Oklab L > 0.85, nearly neutral) around the cells of the reference white (at least half of them, one to two cells) AND a dark cell (L < 0.4) on a
/// cell of the reference pupil, or within one cell of it when at least three near-white cells surround that dark cell. `px(x, y)` is the colour of an
/// opaque cell. This is the eye rule of `eye_parity`: an eye whose white or pupil was painted over is not drawn, whatever key colours it uses.
/// `white_l` is the lightness a near-white cell needs: 0.85, lower on a console whose brightest white is dim (SMS: #c0c0c0).
pub fn eye_drawn(px: &dyn Fn(i32, i32) -> Option<Rgb>, e: &Feature, white_l: f64) -> bool {
    eye_drawn_size(px, e, white_l).is_some()
}

/// [`eye_drawn`] with the size in cells (width, height of the bounding box) of the drawn white area; `None` when the eye is not drawn.
pub fn eye_drawn_size(px: &dyn Fn(i32, i32) -> Option<Rgb>, e: &Feature, white_l: f64) -> Option<(i32, i32)> {
    let white = |x: i32, y: i32| px(x, y).map_or(false, |c| lab(c).l > white_l && (lab(c).a.powi(2) + lab(c).b.powi(2)).sqrt() < 0.09);
    let dark = |x: i32, y: i32| px(x, y).map_or(false, |c| lab(c).l < 0.4);
    let (x0, y0, x1, y1) = (e.cells.0 - 1, e.cells.1 - 1, e.cells.2 + 1, e.cells.3 + 1);
    // the reference white in full cells; the art needs one connected near-white area of 40 % of it (at least one cell)
    let ref_cells = e.wcells.iter().map(|c| c.2 as f64).sum::<f64>() / e.cell_px.max(1) as f64;
    let need = ((0.4 * ref_cells).ceil() as usize).max(1);
    let mut seen = std::collections::HashSet::new();
    let mut best = 0usize;
    let mut best_box = (0i32, 0i32);
    for y in y0..=y1 {
        for x in x0..=x1 {
            if !white(x, y) || seen.contains(&(x, y)) {
                continue;
            }
            let (mut st, mut n) = (vec![(x, y)], 0usize);
            let mut bb = (x, y, x, y);
            seen.insert((x, y));
            while let Some((cx, cy)) = st.pop() {
                n += 1;
                bb = (bb.0.min(cx), bb.1.min(cy), bb.2.max(cx), bb.3.max(cy));
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        let (nx, ny) = (cx + dx, cy + dy);
                        if nx >= x0 && nx <= x1 && ny >= y0 && ny <= y1 && white(nx, ny) && seen.insert((nx, ny)) {
                            st.push((nx, ny));
                        }
                    }
                }
            }
            if n > best {
                best = n;
                best_box = (bb.2 - bb.0 + 1, bb.3 - bb.1 + 1);
            }
        }
    }
    if best < need {
        return None;
    }
    let around = |x: i32, y: i32| (-1..=1).map(|dy| (-1..=1).filter(|dx| (*dx, dy) != (0, 0) && white(x + dx, y + dy)).count()).sum::<usize>();
    let drawn = e.pcells.iter().any(|&(cx, cy, _)| dark(cx, cy) && around(cx, cy) >= 1) || e.pcells.iter().any(|&(cx, cy, _)| (-1..=1).any(|dy| (-1..=1).any(|dx| dark(cx + dx, cy + dy) && around(cx + dx, cy + dy) >= 3)));
    drawn.then_some(best_box)
}

/// Lightness a near-white eye cell needs on this part's console: 0.85 at most, lower where the brightest white is dim (SMS: #c0c0c0).
pub fn white_level(part: &Part) -> f64 {
    let Ok(prof) = crate::profile::load_for(part, None) else { return 0.85 };
    let step = prof.channel_step.max(1);
    let top = ((255 / step) * step).min(255) as u8;
    (lab(Rgb { r: top, g: top, b: top }).l - 0.03).min(0.85)
}

/// A grid as an image through the template colours (transparent where empty).
pub fn render_grid(part: &Part, tpl: &Template, grid: &Grid) -> RgbaImage {
    let mut im = RgbaImage::new(grid.w, grid.h);
    for y in 0..grid.h {
        for x in 0..grid.w {
            let k = grid.cells[y * grid.w + x];
            if k == TRANSPARENT {
                continue;
            }
            if let Some(c) = crate::lint::effective_color(part, tpl, k) {
                im.px[y * grid.w + x] = crate::img::Rgba::opaque(c.r, c.g, c.b);
            }
        }
    }
    im
}

/// THE eye check: the eyes the source frame shows, which of them the art draws (by colour: a near-white area and a dark pupil cell, whatever
/// the template keys are) and the size of the drawn white. `compare` ("eyes found N kept M"), `lint` and the `eye_parity` gate all call this.
pub struct EyeCheck {
    pub eyes: Vec<Feature>,
    /// Per eye: the size in cells of the drawn white, `None` when the eye is lost.
    pub drawn: Vec<Option<(i32, i32)>>,
}

impl EyeCheck {
    pub fn found(&self) -> usize {
        self.eyes.len()
    }
    pub fn kept(&self) -> usize {
        self.drawn.iter().filter(|d| d.is_some()).count()
    }
    pub fn lost(&self) -> Vec<usize> {
        self.drawn.iter().enumerate().filter(|(_, d)| d.is_none()).map(|(i, _)| i).collect()
    }
}

pub fn eye_check(part: &Part, tpl: &Template, grid: &Grid, rf: &RefFrame) -> EyeCheck {
    let eyes = eyes_of(part, rf);
    let art = render_grid(part, tpl, grid);
    let white_l = white_level(part);
    let px = |x: i32, y: i32| -> Option<Rgb> {
        if x < 0 || y < 0 || x as usize >= art.w || y as usize >= art.h {
            return None;
        }
        let p = art.px[y as usize * art.w + x as usize];
        (p.a >= 128).then_some(Rgb { r: p.r, g: p.g, b: p.b })
    };
    let drawn = eyes.iter().map(|e| eye_drawn_size(&px, e, white_l)).collect();
    EyeCheck { eyes, drawn }
}

/// Compare the reference eyes of a frame with the grid. Without eye keys in the template every source eye counts as drawn (nothing can be checked).
pub fn parity(grid: &Grid, eyes: &[Feature], tpl: &Template) -> EyeParity {
    let (source, lost) = eye_status(grid, eyes, tpl);
    EyeParity { source, drawn: source - lost.len(), lost }
}

/// Keys whose colour is an identity colour of the sprite: every overlay key, and base keys that are saturated
/// (Oklab chroma >= 0.15) yet rare (at most 12 % of the opaque pixels), e.g. shoes, glove highlights, mouths.
/// `pix` = native pixels per key, `total` their sum. Keys that never occur are left out.
pub fn salient_keys(tpl: &Template, pix: &[u64; 256], total: u64, _cell_px: u64) -> Vec<u8> {
    let mut v = Vec::new();
    for (li, l) in tpl.layers.iter().enumerate() {
        for k in &l.keys {
            let Some(c) = k.color else { continue };
            if pix[k.key as usize] == 0 {
                continue;
            }
            let o = lab(c);
            let chroma = (o.a * o.a + o.b * o.b).sqrt();
            let rare = pix[k.key as usize] * 100 <= 12 * total.max(1);
            if li > 0 || (chroma >= 0.15 && rare) {
                v.push(k.key);
            }
        }
    }
    v
}

#[derive(Clone, Debug, PartialEq)]
pub struct KeyShare {
    pub key: u8,
    /// Reference area of the key in cells (native pixels classified to the key / f^2).
    pub ref_cells: f64,
    pub out_cells: usize,
}

impl KeyShare {
    /// Cells missing against 75 % of the reference share (0 for keys below two reference cells).
    pub fn deficit(&self) -> f64 {
        if self.ref_cells < 2.0 {
            0.0
        } else {
            (0.75 * self.ref_cells - self.out_cells as f64).max(0.0)
        }
    }
    /// Far more cells than the reference holds (more than twice plus two).
    pub fn surplus(&self) -> bool {
        self.out_cells as f64 > 2.0 * self.ref_cells + 2.0
    }
}

/// Output vs reference area of every salient key (`salient_keys`). Empty for area reduction.
pub fn colour_shares(part: &Part, tpl: &Template, grid: &Grid, rf: &RefFrame) -> Vec<KeyShare> {
    let Some(g) = Geo::of(part, rf) else { return Vec::new() };
    let Ok(cl) = crate::reduce::Classifier::new(tpl) else { return Vec::new() };
    let mut pix = [0u64; 256];
    for y in 0..rf.img.h as i32 {
        for x in 0..rf.img.w as i32 {
            let k = cl.classify(&rf.img, x, y);
            if k != TRANSPARENT {
                pix[k as usize] += 1;
            }
        }
    }
    let total: u64 = pix.iter().sum();
    salient_keys(tpl, &pix, total, 0)
        .into_iter()
        .map(|k| KeyShare { key: k, ref_cells: pix[k as usize] as f64 / (g.f * g.f) as f64, out_cells: grid.cells.iter().filter(|&&c| c == k).count() })
        .collect()
}
