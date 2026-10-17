//! `template suggest --derive`: derive the colour template from the native resolution inputs, BEFORE reduction.
//!
//! Pipeline (all distances are Euclidean Oklab, deterministic, no randomness):
//! 1. **Weighted histogram** of every opaque pixel of every reference frame at native resolution. A pixel weighs
//!    `1 + boost`; `boost` protects features that area alone would outvote. Per frame, 8-connected components of
//!    one source colour are found; a component whose Oklab distance to the dominant opaque colour around it
//!    is a *feature* when it is **small** (contrast >= `SMALL_CONTRAST`) (<= 3 f^2 px, f = reduction
//!    factor, and at most `LOCAL_SHARE_MAX` of the (4f+1)^2 window around it: eyes, pupils, dots; boost
//!    `SMALL_BOOST`) or **thin** (contrast >= `FEATURE_CONTRAST`, a long component, >= 6 f^2 px, with a run of at most f px in x or y:
//!    separators, lines; boost `THIN_BOOST`).
//! 2. **Colours per layer**: k-medoids (exhaustive when small, greedy plus swaps otherwise) over the snapped
//!    source colours minimising the weighted mean Oklab error, then each centre is refined on its members and
//!    snapped to `channel_step`: `real` mode picks the best snapped neighbour (floor/ceil per channel) of a
//!    member source colour, `optimised` mode hill-climbs the snapped grid from there (never naive rounding).
//! 3. **Overlay**: with `--layers 2` an overlay is built; by default it is added only when it pays: the
//!    weighted error must drop by at least `OVERLAY_MIN_GAIN_ABS` and `OVERLAY_MIN_GAIN_REL` (relative).
//!    The base is the best k colours for the whole image; the m overlay colours are then chosen to maximise the
//!    weighted error reduction of the colours that join the overlay: a colour joins only when an overlay colour
//!    is closer than its best base colour by more than `OVERLAY_BIAS` (each overlay pixel costs hardware sprite
//!    area), so the overlay holds the details the base cannot show. Base and overlay are then re-fitted
//!    alternately (the base is re-picked from the colours that stay in it).
//!    Overlay `under` = the base key with the lowest weighted distance to the overlay key's source colours.
//! 4. **Map**: an explicit rule for every source colour. A `near` rule is added for an overlay colour when
//!    >= `NEAR_MIN_SHARE` of its (weighted) pixels have another overlay colour within f+1 px (the spatial
//!    co-occurrence), at most `NEAR_MAX_SHARE` do (otherwise a plain rule is enough), and sending the other
//!    pixels to the fallback key (`under`, or a closer key of the same layer, see `Template::fallback_key`) costs <=
//!    `NEAR_MAX_MEAN_COST` mean error and moves the colour by at most `NEAR_MAX_COLOUR_STEP` (Oklab): a bright
//!    identity colour (shoe red) is never demoted to a duller key by a `near` test.
//! 5. **Cell refinement** (opt-in, `--refine-cells`): the output is measured against the mean colour of each f x f block, so key colours are
//!    then re-centred (same snapping) on the pixels and block means of the cells that use them and the map is rebuilt; kept
//!    only while the weighted cell error falls (centre = compromise: native pixel colours and block means of
//!    the cells carry equal total weight, `CELL_BLEND`). The cell error is what `pxl compare` reports as `oklab`. It is
//!    opt-in: a pure block-mean fit lowers that number by washing out saturated identity colours (outline,
//!    shoes, white), hence the blend.
//! 6. **Report**: per source colour -> key, mean Delta E, pixel share, feature share; total weighted error;
//!    per-layer usage; hardware validity. `evaluate` applies any template with the reducer's rule semantics.

use crate::color::{self, Oklab, Rgb};
use crate::img::RgbaImage;
use crate::template::{KeyDef, Layer, MapRule, Template};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

pub const FEATURE_CONTRAST: f64 = 0.25;
pub const SMALL_CONTRAST: f64 = 0.10;
pub const SMALL_BOOST: f64 = 8.0;
pub const THIN_BOOST: f64 = 1.0;
pub const CELL_BLEND: f64 = 1.0;
pub const LOCAL_SHARE_MAX: f64 = 0.15;
pub const OVERLAY_BIAS: f64 = 0.08;
pub const OVERLAY_MIN_GAIN_ABS: f64 = 0.005;
pub const OVERLAY_MIN_GAIN_REL: f64 = 0.20;
/// A `near` rule is only added when the colour's fallback key is at most this much further (Oklab) than its own key.
pub const NEAR_MAX_COLOUR_STEP: f64 = 0.08;
pub const NEAR_MIN_SHARE: f64 = 0.60;
pub const NEAR_MAX_SHARE: f64 = 0.97;
pub const NEAR_MAX_MEAN_COST: f64 = 0.004;
/// Identity-hue protection: a source colour with Oklab chroma >= this is saturated.
pub const SAT_CHROMA: f64 = 0.07;
/// A template colour with Oklab chroma >= this can carry a saturated hue.
pub const SAT_KEEP: f64 = 0.06;
/// A hue cluster must keep a saturated template colour within this hue angle (radians).
pub const HUE_TOL: f64 = 0.7;
/// Minimum weighted share of the opaque pixels for a hue cluster to be protected.
pub const HUE_MIN_SHARE: f64 = 0.02;
/// An identity hue is worth this many times the plain area-weighted error it removes: a colour is evicted for
/// it only while the extra error stays below `HUE_VALUE x share x (distance to the nearest kept colour)`.
pub const HUE_VALUE: f64 = 6.0;
/// A neutral template colour covering at least this share of the weight is never evicted by a hue.
/// Mean Oklab chroma for a hue cluster to count as vivid (blue, red) rather than a muted brown/olive.
pub const VIVID_CHROMA: f64 = 0.16;
/// A hue cluster with at least this share may evict a non-dominant colour even if that has a bigger share.
pub const STRONG_HUE_SHARE: f64 = 0.05;
pub const NEUTRAL_KEEP_SHARE: f64 = 0.20;
/// Colours of a flat (single layer) base.
pub const MAX_BASE: usize = 16;
const MAX_CAND: usize = 64;
const MAX_COMBOS: u64 = 400_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Real,
    Optimised,
}

impl Mode {
    pub fn parse(s: &str) -> Option<Mode> {
        match s {
            "real" => Some(Mode::Real),
            "optimised" | "optimized" => Some(Mode::Optimised),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Mode::Real => "real",
            Mode::Optimised => "optimised",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Opts {
    /// Colours per layer (profile `colors`).
    pub k: usize,
    pub step: u32,
    /// `Some(1)` base only, `Some(2)` force an overlay, `None` add one only when it pays.
    pub layers: Option<u8>,
    pub mode: Mode,
    pub name: String,
    /// Re-centre key colours on the block means of the cells that use them (see module docs, step 5).
    pub cells: bool,
}

// ------------------------------------------------------------------ data

pub struct Col {
    pub rgb: Rgb,
    pub lab: Oklab,
    /// Raw opaque pixel count.
    pub n: u64,
    /// Weighted pixel count (area x importance).
    pub w: f64,
    /// Pixels flagged as features.
    pub feat: u64,
}

pub struct FramePix {
    pub w: usize,
    pub h: usize,
    /// Colour index, `NONE` for transparent.
    pub idx: Vec<u32>,
    pub wt: Vec<f64>,
    pub feat: Vec<bool>,
}

pub const NONE: u32 = u32::MAX;

/// One reduction cell: the mean colour of the opaque pixels of its f x f source block (what `compare` measures
/// the output against) and its importance (mean pixel weight).
pub struct Cell {
    pub x0: i32,
    pub y0: i32,
    pub mean: Rgb,
    pub lab: Oklab,
    pub w: f64,
}

pub struct Data {
    pub cells: Vec<Vec<Cell>>,
    pub cols: Vec<Col>,
    pub frames: Vec<FramePix>,
    pub imgs: Vec<RgbaImage>,
    pub factor: u32,
    pub total_n: u64,
    pub total_w: f64,
}

fn dist(a: Oklab, b: Oklab) -> f64 {
    color::oklab_dist2(a, b).sqrt()
}

impl Data {
    pub fn build(imgs: Vec<RgbaImage>, factor: u32) -> Data {
        let n = imgs.len();
        Data::build_with(imgs, factor, vec![(0, 0); n])
    }

    /// `orgs[i]`: source position of the top left of target cell (0,0) in frame i (the cell grid phase).
    pub fn build_with(imgs: Vec<RgbaImage>, factor: u32, orgs: Vec<(i32, i32)>) -> Data {
        let f = factor.max(1) as usize;
        let mut set: BTreeSet<Rgb> = BTreeSet::new();
        for im in &imgs {
            for p in &im.px {
                if p.a >= 128 {
                    set.insert(p.rgb());
                }
            }
        }
        let rgbs: Vec<Rgb> = set.into_iter().collect();
        let mut cols: Vec<Col> = rgbs.iter().map(|&rgb| Col { rgb, lab: color::srgb_to_oklab(rgb), n: 0, w: 0.0, feat: 0 }).collect();
        let mut frames = Vec::new();
        for im in &imgs {
            let (w, h) = (im.w, im.h);
            let idx: Vec<u32> = im.px.iter().map(|p| if p.a >= 128 { rgbs.binary_search(&p.rgb()).map_or(NONE, |i| i as u32) } else { NONE }).collect();
            // contrast per pixel
            let mut con = vec![0.0f64; w * h];
            for y in 0..h {
                for x in 0..w {
                    let i = idx[y * w + x];
                    if i == NONE {
                        continue;
                    }
                    let mut m = 0.0f64;
                    for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
                        let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                        if nx < 0 || ny < 0 || nx as usize >= w || ny as usize >= h {
                            continue;
                        }
                        let j = idx[ny as usize * w + nx as usize];
                        if j != NONE && j != i {
                            m = m.max(dist(cols[i as usize].lab, cols[j as usize].lab));
                        }
                    }
                    con[y * w + x] = m;
                }
            }
            // runs
            let mut hrun = vec![0u32; w * h];
            let mut vrun = vec![0u32; w * h];
            for y in 0..h {
                let mut x = 0;
                while x < w {
                    let mut e = x;
                    while e < w && idx[y * w + e] == idx[y * w + x] {
                        e += 1;
                    }
                    for t in x..e {
                        hrun[y * w + t] = (e - x) as u32;
                    }
                    x = e;
                }
            }
            for x in 0..w {
                let mut y = 0;
                while y < h {
                    let mut e = y;
                    while e < h && idx[e * w + x] == idx[y * w + x] {
                        e += 1;
                    }
                    for t in y..e {
                        vrun[t * w + x] = (e - y) as u32;
                    }
                    y = e;
                }
            }
            // components
            let mut wt = vec![1.0f64; w * h];
            let mut feat = vec![false; w * h];
            let mut seen = vec![false; w * h];
            for s in 0..w * h {
                if idx[s] == NONE || seen[s] {
                    continue;
                }
                let mut comp = vec![s];
                seen[s] = true;
                let mut k = 0;
                while k < comp.len() {
                    let p = comp[k];
                    k += 1;
                    let (px, py) = ((p % w) as i32, (p / w) as i32);
                    for dy in -1..=1i32 {
                        for dx in -1..=1i32 {
                            let (nx, ny) = (px + dx, py + dy);
                            if nx < 0 || ny < 0 || nx as usize >= w || ny as usize >= h {
                                continue;
                            }
                            let q = ny as usize * w + nx as usize;
                            if !seen[q] && idx[q] == idx[s] {
                                seen[q] = true;
                                comp.push(q);
                            }
                        }
                    }
                }
                // contrast: Oklab distance to the dominant colour surrounding the component
                let mut around: BTreeMap<u32, u32> = BTreeMap::new();
                for &p in &comp {
                    let (px, py) = ((p % w) as i32, (p / w) as i32);
                    for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
                        let (nx, ny) = (px + dx, py + dy);
                        if nx < 0 || ny < 0 || nx as usize >= w || ny as usize >= h {
                            continue;
                        }
                        let j = idx[ny as usize * w + nx as usize];
                        if j != NONE && j != idx[s] {
                            *around.entry(j).or_insert(0) += 1;
                        }
                    }
                }
                let dom = around.iter().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0))).map(|(&j, _)| j);
                let cc = dom.map_or(0.0, |j| dist(cols[idx[s] as usize].lab, cols[j as usize].lab));
                if cc < SMALL_CONTRAST {
                    continue;
                }
                // local share: fraction of the (4f+1)^2 window around the component centre taken by this colour
                let (mut sx, mut sy) = (0usize, 0usize);
                for &p in &comp {
                    sx += p % w;
                    sy += p / w;
                }
                let (cx, cy) = ((sx / comp.len()) as i32, (sy / comp.len()) as i32);
                let rr = 2 * f as i32;
                let (mut same, mut opaque) = (0u32, 0u32);
                for dy in -rr..=rr {
                    for dx in -rr..=rr {
                        let (nx, ny) = (cx + dx, cy + dy);
                        if nx < 0 || ny < 0 || nx as usize >= w || ny as usize >= h {
                            continue;
                        }
                        let j = idx[ny as usize * w + nx as usize];
                        if j != NONE {
                            opaque += 1;
                            if j == idx[s] {
                                same += 1;
                            }
                        }
                    }
                }
                let local = same as f64 / opaque.max(1) as f64;
                let small = comp.len() <= 3 * f * f && local <= LOCAL_SHARE_MAX;
                let line_ok = cc >= FEATURE_CONTRAST;
                for &p in &comp {
                    let thin = line_ok && hrun[p].min(vrun[p]) as usize <= f && comp.len() >= 6 * f * f;
                    if small {
                        wt[p] = 1.0 + SMALL_BOOST;
                        feat[p] = true;
                    } else if thin {
                        wt[p] = 1.0 + THIN_BOOST;
                        feat[p] = true;
                    }
                }
            }
            for p in 0..w * h {
                if idx[p] != NONE {
                    let c = &mut cols[idx[p] as usize];
                    c.n += 1;
                    c.w += wt[p];
                    if feat[p] {
                        c.feat += 1;
                    }
                }
            }
            frames.push(FramePix { w, h, idx, wt, feat });
        }
        let total_n = cols.iter().map(|c| c.n).sum();
        let total_w = cols.iter().map(|c| c.w).sum();
        let fi = f as i32;
        let mut cells = Vec::new();
        for (k, fp) in frames.iter().enumerate() {
            let org = orgs.get(k).copied().unwrap_or((0, 0));
            let sx = org.0.rem_euclid(fi);
            let sy = org.1.rem_euclid(fi);
            let (bx0, by0) = (if sx > 0 { sx - fi } else { 0 }, if sy > 0 { sy - fi } else { 0 });
            let mut v = Vec::new();
            let mut y0 = by0;
            while y0 < fp.h as i32 {
                let mut x0 = bx0;
                while x0 < fp.w as i32 {
                    let (mut n, mut sr, mut sg, mut sb, mut sw) = (0u32, 0u32, 0u32, 0u32, 0.0f64);
                    for j in 0..fi {
                        for i in 0..fi {
                            let (x, y) = (x0 + i, y0 + j);
                            if x < 0 || y < 0 || x as usize >= fp.w || y as usize >= fp.h {
                                continue;
                            }
                            let q = y as usize * fp.w + x as usize;
                            if fp.idx[q] != NONE {
                                let c = cols[fp.idx[q] as usize].rgb;
                                n += 1;
                                sr += c.r as u32;
                                sg += c.g as u32;
                                sb += c.b as u32;
                                sw += fp.wt[q];
                            }
                        }
                    }
                    if n * 2 >= (f * f) as u32 && n > 0 {
                        let mean = Rgb { r: ((sr + n / 2) / n) as u8, g: ((sg + n / 2) / n) as u8, b: ((sb + n / 2) / n) as u8 };
                        v.push(Cell { x0, y0, mean, lab: color::srgb_to_oklab(mean), w: sw / n as f64 });
                    }
                    x0 += fi;
                }
                y0 += fi;
            }
            cells.push(v);
        }
        Data { cells, cols, frames, imgs, factor: factor.max(1), total_n, total_w }
    }
}

// ------------------------------------------------------------------ colour selection

fn top_of(step: u32) -> u32 {
    (255 / step.max(1)) * step.max(1)
}

/// Floor and ceil multiples of `step` for one channel.
fn floor_ceil(v: u8, step: u32) -> [u8; 2] {
    let s = step.max(1);
    let f = (v as u32 / s) * s;
    let c = if v as u32 % s == 0 { f } else { (f + s).min(top_of(s)) };
    [f as u8, c as u8]
}

fn centre_cost(members: &[&Col], c: Rgb) -> f64 {
    let l = color::srgb_to_oklab(c);
    members.iter().map(|m| m.w * dist(m.lab, l)).sum()
}

/// Best snapped centre for `members` (see module docs for the two modes).
fn refine_centre(members: &[&Col], mode: Mode, step: u32) -> Rgb {
    let mut order: Vec<&&Col> = members.iter().collect();
    order.sort_by(|a, b| b.w.partial_cmp(&a.w).unwrap_or(std::cmp::Ordering::Equal).then(a.rgb.cmp(&b.rgb)));
    let mut best: Option<(f64, Rgb)> = None;
    for m in order.iter().take(40) {
        for r in floor_ceil(m.rgb.r, step) {
            for g in floor_ceil(m.rgb.g, step) {
                for b in floor_ceil(m.rgb.b, step) {
                    let c = Rgb { r, g, b };
                    let e = centre_cost(members, c);
                    if best.map_or(true, |(be, bc)| e < be - 1e-12 || ((e - be).abs() <= 1e-12 && c < bc)) {
                        best = Some((e, c));
                    }
                }
            }
        }
    }
    let (mut e, mut c) = best.unwrap_or((0.0, Rgb::default()));
    if mode == Mode::Optimised {
        let s = step.max(1) as i32;
        let top = top_of(step) as i32;
        for _ in 0..200 {
            let mut imp: Option<(f64, Rgb)> = None;
            for dr in -1..=1i32 {
                for dg in -1..=1i32 {
                    for db in -1..=1i32 {
                        if dr == 0 && dg == 0 && db == 0 {
                            continue;
                        }
                        let n = [c.r as i32 + dr * s, c.g as i32 + dg * s, c.b as i32 + db * s];
                        if n.iter().any(|v| *v < 0 || *v > top) {
                            continue;
                        }
                        let cand = Rgb { r: n[0] as u8, g: n[1] as u8, b: n[2] as u8 };
                        let ce = centre_cost(members, cand);
                        if ce < e - 1e-12 && imp.map_or(true, |(ie, _)| ce < ie - 1e-12) {
                            imp = Some((ce, cand));
                        }
                    }
                }
            }
            match imp {
                Some((ie, ic)) => {
                    e = ie;
                    c = ic;
                }
                None => break,
            }
        }
    }
    c
}

fn nearest(cols: &[Col], centres: &[Rgb]) -> Vec<usize> {
    let labs: Vec<Oklab> = centres.iter().map(|&c| color::srgb_to_oklab(c)).collect();
    cols.iter()
        .map(|c| {
            let mut best = (0usize, f64::MAX);
            for (j, &l) in labs.iter().enumerate() {
                let d = dist(c.lab, l);
                if d < best.1 {
                    best = (j, d);
                }
            }
            best.0
        })
        .collect()
}

fn mean_err(cols: &[Col], centres: &[Rgb]) -> f64 {
    let tw: f64 = cols.iter().map(|c| c.w).sum();
    if tw <= 0.0 || centres.is_empty() {
        return 0.0;
    }
    let a = nearest(cols, centres);
    let labs: Vec<Oklab> = centres.iter().map(|&c| color::srgb_to_oklab(c)).collect();
    cols.iter().zip(a).map(|(c, j)| c.w * dist(c.lab, labs[j])).sum::<f64>() / tw
}

fn combos(n: usize, m: usize) -> u64 {
    let mut r: u128 = 1;
    for i in 0..m {
        r = r * (n - i) as u128 / (i + 1) as u128;
        if r > u64::MAX as u128 {
            return u64::MAX;
        }
    }
    r as u64
}

/// `m` snapped centres minimising the weighted mean Oklab error of `cols`.
pub fn pick_centres(cols: &[Col], m: usize, mode: Mode, step: u32) -> Vec<Rgb> {
    let m = m.min(cols.len());
    if m == 0 {
        return Vec::new();
    }
    let mut order: Vec<usize> = (0..cols.len()).collect();
    order.sort_by(|&a, &b| cols[b].w.partial_cmp(&cols[a].w).unwrap_or(std::cmp::Ordering::Equal).then(a.cmp(&b)));
    let mut cand: Vec<Rgb> = Vec::new();
    for &i in &order {
        let s = color::snap_step(cols[i].rgb, step);
        if !cand.contains(&s) {
            cand.push(s);
        }
        if cand.len() >= MAX_CAND {
            break;
        }
    }
    let m = m.min(cand.len());
    let nc = cand.len();
    let clab: Vec<Oklab> = cand.iter().map(|&c| color::srgb_to_oklab(c)).collect();
    let d: Vec<Vec<f64>> = cols.iter().map(|c| clab.iter().map(|&l| dist(c.lab, l)).collect()).collect();
    let w: Vec<f64> = cols.iter().map(|c| c.w).collect();
    let cost_of = |sel: &[usize]| -> f64 { (0..cols.len()).map(|i| w[i] * sel.iter().map(|&j| d[i][j]).fold(f64::MAX, f64::min)).sum() };
    let mut chosen: Vec<usize>;
    if combos(nc, m) <= MAX_COMBOS {
        fn rec(start: usize, left: usize, nc: usize, cur: &mut Vec<usize>, mins: &[f64], d: &[Vec<f64>], w: &[f64], best: &mut (f64, Vec<usize>)) {
            if left == 0 {
                let c: f64 = mins.iter().zip(w).map(|(m, w)| m * w).sum();
                if c < best.0 - 1e-12 {
                    *best = (c, cur.clone());
                }
                return;
            }
            for j in start..=nc - left {
                let nm: Vec<f64> = mins.iter().enumerate().map(|(i, &m)| m.min(d[i][j])).collect();
                cur.push(j);
                rec(j + 1, left - 1, nc, cur, &nm, d, w, best);
                cur.pop();
            }
        }
        let mut best = (f64::MAX, Vec::new());
        rec(0, m, nc, &mut Vec::new(), &vec![f64::MAX; cols.len()], &d, &w, &mut best);
        chosen = best.1;
    } else {
        chosen = Vec::new();
        while chosen.len() < m {
            let mut b: Option<(f64, usize)> = None;
            for j in 0..nc {
                if chosen.contains(&j) {
                    continue;
                }
                let mut t = chosen.clone();
                t.push(j);
                let c = cost_of(&t);
                if b.map_or(true, |(bc, _)| c < bc - 1e-12) {
                    b = Some((c, j));
                }
            }
            chosen.push(b.map(|x| x.1).unwrap_or(0));
        }
        for _ in 0..20 {
            let mut improved = false;
            for p in 0..chosen.len() {
                let mut bc = cost_of(&chosen);
                for j in 0..nc {
                    if chosen.contains(&j) {
                        continue;
                    }
                    let mut t = chosen.clone();
                    t[p] = j;
                    let c = cost_of(&t);
                    if c < bc - 1e-12 {
                        bc = c;
                        chosen = t;
                        improved = true;
                    }
                }
            }
            if !improved {
                break;
            }
        }
    }
    let mut centres: Vec<Rgb> = chosen.iter().map(|&j| cand[j]).collect();
    // refinement on the members
    for _ in 0..8 {
        let a = nearest(cols, &centres);
        let mut next = Vec::new();
        for j in 0..centres.len() {
            let mem: Vec<&Col> = cols.iter().zip(&a).filter(|(_, &x)| x == j).map(|(c, _)| c).collect();
            if !mem.is_empty() {
                next.push(refine_centre(&mem, mode, step));
            }
        }
        next.sort();
        next.dedup();
        let mut cur = centres.clone();
        cur.sort();
        if next == cur {
            break;
        }
        centres = next;
    }
    centres.sort();
    centres.dedup();
    centres
}

// ------------------------------------------------------------------ keys, assignment, map

#[derive(Clone, Debug)]
pub struct KeyInfo {
    pub key: u8,
    pub layer: usize,
    pub rgb: Rgb,
    pub under: Option<u8>,
    pub role: String,
}

/// Index into `keys` for every source colour: nearest base key, or the nearest overlay key when it is closer by
/// more than `OVERLAY_BIAS`.
fn assign(cols: &[Col], keys: &[KeyInfo]) -> Vec<usize> {
    let labs: Vec<Oklab> = keys.iter().map(|k| color::srgb_to_oklab(k.rgb)).collect();
    cols.iter()
        .map(|c| {
            let (mut bb, mut bo) = ((usize::MAX, f64::MAX), (usize::MAX, f64::MAX));
            for (j, k) in keys.iter().enumerate() {
                let d = dist(c.lab, labs[j]);
                let slot = if k.layer == 0 { &mut bb } else { &mut bo };
                if d < slot.1 {
                    *slot = (j, d);
                }
            }
            if bo.0 != usize::MAX && (bb.0 == usize::MAX || bo.1 + OVERLAY_BIAS < bb.1) {
                bo.0
            } else {
                bb.0
            }
        })
        .collect()
}

fn assigned_error(cols: &[Col], keys: &[KeyInfo], a: &[usize]) -> f64 {
    let tw: f64 = cols.iter().map(|c| c.w).sum();
    if tw <= 0.0 {
        return 0.0;
    }
    cols.iter().zip(a).map(|(c, &j)| c.w * dist(c.lab, color::srgb_to_oklab(keys[j].rgb))).sum::<f64>() / tw
}

fn role_name(rgb: Rgb) -> String {
    let l = color::srgb_to_oklab(rgb);
    let c = (l.a * l.a + l.b * l.b).sqrt();
    if c < 0.05 {
        return if l.l > 0.92 {
            "white"
        } else if l.l > 0.65 {
            "light_gray"
        } else if l.l > 0.35 {
            "gray"
        } else {
            "black"
        }
        .into();
    }
    let refs: [(&str, Rgb); 9] = [
        ("red", Rgb::new(0xe0, 0x10, 0x10)),
        ("orange", Rgb::new(0xf0, 0x80, 0x10)),
        ("yellow", Rgb::new(0xf0, 0xe0, 0x20)),
        ("green", Rgb::new(0x20, 0xb0, 0x30)),
        ("cyan", Rgb::new(0x10, 0xc0, 0xc0)),
        ("blue", Rgb::new(0x30, 0x50, 0xe0)),
        ("purple", Rgb::new(0x90, 0x30, 0xc0)),
        ("pink", Rgb::new(0xf0, 0x80, 0xc0)),
        ("brown", Rgb::new(0x80, 0x48, 0x18)),
    ];
    let mut best = ("red", f64::MIN);
    for (n, r) in refs {
        let o = color::srgb_to_oklab(r);
        let oc = (o.a * o.a + o.b * o.b).sqrt();
        let dot = (l.a * o.a + l.b * o.b) / (c * oc);
        if dot > best.1 {
            best = (n, dot);
        }
    }
    let n = best.0;
    if n == "orange" && l.l > 0.70 && c < 0.13 {
        return "skin".into();
    }
    if (n == "orange" || n == "red") && l.l < 0.65 && c < 0.13 {
        return "brown".into();
    }
    if n == "orange" && l.l < 0.55 {
        return "brown".into();
    }
    if l.l < 0.45 && n != "brown" {
        return format!("dark_{}", n);
    }
    if l.l > 0.88 {
        return format!("light_{}", n);
    }
    n.into()
}

fn unique_role(used: &mut Vec<String>, base: String) -> String {
    let mut r = base.clone();
    let mut i = 2;
    while used.contains(&r) {
        r = format!("{}{}", base, i);
        i += 1;
    }
    used.push(r.clone());
    r
}

fn overlay_letter(role: &str, used: &mut Vec<u8>) -> u8 {
    let mut cands: Vec<u8> = Vec::new();
    let (lower_first, rest) = match role.strip_prefix("dark_") {
        Some(r) => (true, r),
        None => (false, role.strip_prefix("light_").unwrap_or(role)),
    };
    let first = rest.bytes().find(|b| b.is_ascii_alphabetic()).unwrap_or(b'a').to_ascii_lowercase();
    if lower_first {
        cands.push(first);
        cands.push(first.to_ascii_uppercase());
    } else {
        cands.push(first.to_ascii_uppercase());
        cands.push(first);
    }
    for b in role.bytes().filter(|b| b.is_ascii_alphabetic()) {
        cands.push(b.to_ascii_uppercase());
        cands.push(b.to_ascii_lowercase());
    }
    cands.extend(b'A'..=b'Z');
    cands.extend(b'a'..=b'z');
    let k = cands.into_iter().find(|c| !used.contains(c)).unwrap_or(b'?');
    used.push(k);
    k
}

/// Base key of `keys` that best represents overlay colours `members` when the overlay is absent.
fn choose_under(cols: &[Col], members: &[usize], keys: &[KeyInfo]) -> Option<u8> {
    let mut best: Option<(f64, u8)> = None;
    for k in keys.iter().filter(|k| k.layer == 0) {
        let l = color::srgb_to_oklab(k.rgb);
        let e: f64 = members.iter().map(|&i| cols[i].w * dist(cols[i].lab, l)).sum();
        if best.map_or(true, |(be, _)| e < be - 1e-12) {
            best = Some((e, k.key));
        }
    }
    best.map(|b| b.1)
}

/// Complete map: an explicit rule for every source colour, `near` rules from spatial co-occurrence.
/// Returns the rules and a note per colour that got a `near` rule.
fn build_map(data: &Data, keys: &[KeyInfo], notes: &mut Vec<String>) -> Vec<MapRule> {
    let a = assign(&data.cols, keys);
    let r = (data.factor + 1).max(2) as i32;
    // co-occurrence of overlay colours
    let ncol = data.cols.len();
    let mut cooc: BTreeMap<usize, BTreeMap<usize, f64>> = BTreeMap::new();
    let mut tot: BTreeMap<usize, f64> = BTreeMap::new();
    for ci in 0..ncol {
        if keys[a[ci]].layer > 0 {
            cooc.insert(ci, BTreeMap::new());
            tot.insert(ci, 0.0);
        }
    }
    if !cooc.is_empty() {
        for fp in &data.frames {
            for y in 0..fp.h as i32 {
                for x in 0..fp.w as i32 {
                    let i = fp.idx[y as usize * fp.w + x as usize];
                    if i == NONE || !cooc.contains_key(&(i as usize)) {
                        continue;
                    }
                    let wgt = fp.wt[y as usize * fp.w + x as usize];
                    let mut seen: BTreeSet<u32> = BTreeSet::new();
                    for dy in -r..=r {
                        for dx in -r..=r {
                            let (nx, ny) = (x + dx, y + dy);
                            if nx < 0 || ny < 0 || nx as usize >= fp.w || ny as usize >= fp.h {
                                continue;
                            }
                            let j = fp.idx[ny as usize * fp.w + nx as usize];
                            if j != NONE && j != i {
                                seen.insert(j);
                            }
                        }
                    }
                    let m = cooc.get_mut(&(i as usize)).unwrap();
                    for j in seen {
                        *m.entry(j as usize).or_insert(0.0) += wgt;
                    }
                    *tot.get_mut(&(i as usize)).unwrap() += wgt;
                }
            }
        }
    }
    let mut rules = Vec::new();
    for ci in 0..ncol {
        let k = &keys[a[ci]];
        let mut near = None;
        if k.layer > 0 {
            let t = tot[&ci];
            let mut best: Option<(usize, f64)> = None;
            for (&d, &v) in &cooc[&ci] {
                if keys[a[d]].layer == k.layer && best.map_or(true, |(_, bv)| v > bv + 1e-12) {
                    best = Some((d, v));
                }
            }
            if let (Some((d, v)), Some(u)) = (best, k.under) {
                let share = if t > 0.0 { v / t } else { 0.0 };
                if share >= NEAR_MIN_SHARE && share <= NEAR_MAX_SHARE {
                    // the reducer's fallback for the pixels that are not near: `under`, unless a key of the same
                    // layer or the nearest base key is clearly closer (Template::fallback_key)
                    let cl = data.cols[ci].lab;
                    let du = keys.iter().find(|x| x.key == u).map(|x| dist(cl, color::srgb_to_oklab(x.rgb)));
                    let alt = keys
                        .iter()
                        .filter(|x| (x.layer == 0 || x.layer == k.layer) && x.key != k.key)
                        .map(|x| dist(cl, color::srgb_to_oklab(x.rgb)))
                        .fold(f64::MAX, f64::min);
                    if let Some(du) = du {
                        let fb = if alt + crate::template::FALLBACK_MARGIN < du { alt } else { du };
                        let inc = (t - v) * (fb - dist(cl, color::srgb_to_oklab(k.rgb)));
                        // identity colours (a bright red highlight) must not be demoted to a duller key where
                        // the near test fails: the per-colour step to the fallback must be small
                        if inc / data.total_w.max(1.0) <= NEAR_MAX_MEAN_COST && fb - dist(cl, color::srgb_to_oklab(k.rgb)) <= NEAR_MAX_COLOUR_STEP {
                            near = Some((data.cols[d].rgb, r as u32));
                            notes.push(format!(
                                "{} -> {} near {}: {:.0}% of its weighted pixels have {} within {} px; the rest fall back to '{}' (+{:.4} mean error)",
                                color::to_hex(data.cols[ci].rgb),
                                k.key as char,
                                color::to_hex(data.cols[d].rgb),
                                share * 100.0,
                                color::to_hex(data.cols[d].rgb),
                                r,
                                u as char,
                                inc / data.total_w.max(1.0)
                            ));
                        }
                    }
                }
            }
        }
        rules.push(MapRule { from: data.cols[ci].rgb, key: k.key, near });
    }
    rules
}

/// Key of the i-th base colour: 1-9, then a-g (flat templates only; overlay keys are upper case or non-hex letters of their role).
fn base_key(i: usize) -> u8 {
    b"123456789abcdefghijklmnop"[i.min(24)]
}

fn finalize(data: &Data, slots: &[(Rgb, usize)], opts: &Opts, overlay_name: &str, trace: &mut Vec<String>) -> Template {
    let cols = &data.cols;
    // base keys: darkest first
    let mut base: Vec<Rgb> = slots.iter().filter(|s| s.1 == 0).map(|s| s.0).collect();
    base.sort_by(|a, b| {
        color::srgb_to_oklab(*a).l.partial_cmp(&color::srgb_to_oklab(*b).l).unwrap_or(std::cmp::Ordering::Equal).then(a.cmp(b))
    });
    let mut over: Vec<Rgb> = slots.iter().filter(|s| s.1 > 0).map(|s| s.0).collect();
    over.sort_by(|a, b| {
        color::srgb_to_oklab(*b).l.partial_cmp(&color::srgb_to_oklab(*a).l).unwrap_or(std::cmp::Ordering::Equal).then(a.cmp(b))
    });
    let mut keys: Vec<KeyInfo> = Vec::new();
    for (i, &c) in base.iter().enumerate() {
        keys.push(KeyInfo { key: base_key(i), layer: 0, rgb: c, under: None, role: String::new() });
    }
    // roles for the base: outline, body (largest area), then by hue
    let a0 = assign(cols, &keys);
    let mut area = vec![0u64; keys.len()];
    for (i, c) in cols.iter().enumerate() {
        area[a0[i]] += c.n;
    }
    let body = (1..keys.len()).max_by(|&x, &y| area[x].cmp(&area[y]).then(y.cmp(&x)));
    let mut used = Vec::new();
    for i in 0..keys.len() {
        let r = if i == 0 {
            "outline".to_string()
        } else if Some(i) == body {
            "body".to_string()
        } else {
            role_name(keys[i].rgb)
        };
        keys[i].role = unique_role(&mut used, r);
    }
    let nb = keys.len();
    let mut letters: Vec<u8> = Vec::new();
    for &c in &over {
        let role = unique_role(&mut used, role_name(c));
        let key = overlay_letter(&role, &mut letters);
        keys.push(KeyInfo { key, layer: 1, rgb: c, under: None, role });
    }
    // under: base key with the lowest weighted distance to the members of each overlay key
    let a1 = assign(cols, &keys);
    for j in nb..keys.len() {
        let mem: Vec<usize> = (0..cols.len()).filter(|&i| a1[i] == j).collect();
        let u = choose_under(cols, &mem, &keys).or(Some(b'1'));
        keys[j].under = u;
    }
    let mut notes = Vec::new();
    let map = build_map(data, &keys, &mut notes);
    trace.extend(notes);
    let mut t = Template::empty(&opts.name);
    let kd = |k: &KeyInfo| KeyDef { key: k.key, role: k.role.clone(), color: Some(k.rgb), under: k.under };
    t.layers.push(Layer { name: "base".into(), keys: keys.iter().filter(|k| k.layer == 0).map(kd).collect() });
    if keys.iter().any(|k| k.layer > 0) {
        t.layers.push(Layer { name: overlay_name.into(), keys: keys.iter().filter(|k| k.layer > 0).map(kd).collect() });
    }
    t.map = map;
    t
}

fn chroma_hue(l: Oklab) -> (f64, f64) {
    ((l.a * l.a + l.b * l.b).sqrt(), l.b.atan2(l.a))
}

fn hue_diff(a: f64, b: f64) -> f64 {
    let d = (a - b).abs() % std::f64::consts::TAU;
    d.min(std::f64::consts::TAU - d)
}

/// Saturated hue clusters (indices into `cols`, weighted share, mean hue) with at least `HUE_MIN_SHARE` of the weight.
fn hue_clusters(cols: &[Col]) -> Vec<(Vec<usize>, f64, f64)> {
    let total: f64 = cols.iter().map(|c| c.w).sum();
    if total <= 0.0 {
        return Vec::new();
    }
    let mut sat: Vec<(usize, f64)> = cols.iter().enumerate().filter_map(|(i, c)| {
        let (ch, h) = chroma_hue(c.lab);
        (ch >= SAT_CHROMA).then_some((i, h))
    }).collect();
    sat.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(&b.0)));
    let mut groups: Vec<Vec<(usize, f64)>> = Vec::new();
    for e in sat {
        match groups.last_mut() {
            Some(g) if e.1 - g.last().unwrap().1 <= 0.35 => g.push(e),
            _ => groups.push(vec![e]),
        }
    }
    if groups.len() > 1 {
        let wrap = groups[0][0].1 + std::f64::consts::TAU - groups.last().unwrap().last().unwrap().1;
        if wrap <= 0.35 {
            let last = groups.pop().unwrap();
            groups[0].splice(0..0, last);
        }
    }
    let mut out = Vec::new();
    for g in groups {
        let w: f64 = g.iter().map(|e| cols[e.0].w).sum();
        if w / total < HUE_MIN_SHARE {
            continue;
        }
        let (sx, sy) = g.iter().fold((0.0, 0.0), |a, e| (a.0 + cols[e.0].w * e.1.cos(), a.1 + cols[e.0].w * e.1.sin()));
        out.push((g.iter().map(|e| e.0).collect(), w / total, sy.atan2(sx)));
    }
    out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// Merging by nearest colour can fold a saturated identity hue (a blue body) into a neutral colour when
/// the neutral colours already cover most of the error. Every saturated hue cluster with a meaningful
/// share keeps a saturated template colour: the cheapest unprotected base colour is replaced by a centre
/// fitted to the cluster.
fn protect_hues(cols: &[Col], slots: Vec<(Rgb, usize)>, opts: &Opts, trace: &mut Vec<String>) -> Vec<(Rgb, usize)> {
    let mut slots = slots;
    let clusters = hue_clusters(cols);
    let covers = |c: Rgb, hue: f64| {
        let (ch, h) = chroma_hue(color::srgb_to_oklab(c));
        ch >= SAT_KEEP && hue_diff(h, hue) <= HUE_TOL
    };
    for (members, share, hue) in &clusters {
        if slots.iter().any(|s| covers(s.0, *hue)) {
            continue;
        }
        let mem: Vec<&Col> = members.iter().map(|&i| &cols[i]).collect();
        let fitted = refine_centre(&mem, opts.mode, opts.step);
        // the chosen colour is one of the cluster's own source colours (no blended hue)
        let fl = color::srgb_to_oklab(fitted);
        let centre = mem.iter().filter(|c| covers(c.rgb, *hue)).min_by(|a, b| {
            dist(fl, a.lab).partial_cmp(&dist(fl, b.lab)).unwrap_or(std::cmp::Ordering::Equal)
        }).map_or(fitted, |c| c.rgb);
        // the colour must be one the hardware has: every channel a multiple of the step (this left #ff0000 unsnapped on the Game Boy Color)
        let centre = if opts.step > 1 { color::snap_step(centre, opts.step) } else { centre };
        if !covers(centre, *hue) {
            continue;
        }
        // weighted mean chroma of the cluster: only a vivid hue (saturated blue, red) counts as strong
        let (cw, cs) = mem.iter().fold((0.0, 0.0), |a, c| (a.0 + c.w, a.1 + c.w * chroma_hue(c.lab).0));
        let vivid = cs / cw.max(1e-12) >= VIVID_CHROMA;
        let base_n = slots.iter().filter(|s| s.1 == 0).count();
        let all: Vec<Rgb> = slots.iter().map(|s| s.0).collect();
        if base_n >= opts.k {
            let cur = mean_err(cols, &all);
            // the darkest colour is the outline: never evicted
            let darkest = (0..slots.len()).filter(|&j| slots[j].1 == 0).min_by(|&a, &b| {
                color::srgb_to_oklab(slots[a].0).l.partial_cmp(&color::srgb_to_oklab(slots[b].0).l).unwrap_or(std::cmp::Ordering::Equal)
            });
            let mut victim: Option<(f64, usize)> = None;
            let total: f64 = cols.iter().map(|c| c.w).sum::<f64>().max(1e-12);
            let owner = nearest(cols, &all);
            let mut slot_share = vec![0.0f64; slots.len()];
            for (i, &o) in owner.iter().enumerate() {
                slot_share[o] += cols[i].w / total;
            }
            for j in 0..slots.len() {
                if slots[j].1 != 0 || Some(j) == darkest || clusters.iter().any(|c| covers(slots[j].0, c.2)) {
                    continue;
                }
                // a hue may only evict a colour with a smaller own share, never a dominant neutral
                let neutral = chroma_hue(color::srgb_to_oklab(slots[j].0)).0 < SAT_KEEP;
                // a strong hue (a real part of the object) may still take a secondary shade of a ramp,
                // but never the dominant slot
                let dominant = slot_share.iter().all(|&o| o <= slot_share[j]);
                let strong = *share >= STRONG_HUE_SHARE && vivid && !dominant;
                if !strong && (slot_share[j] >= *share || (neutral && slot_share[j] >= NEUTRAL_KEEP_SHARE)) {
                    continue;
                }
                let mut rest: Vec<Rgb> = slots.iter().enumerate().filter(|(i, _)| *i != j).map(|(_, s)| s.0).collect();
                rest.push(centre);
                let e = mean_err(cols, &rest);
                if victim.map_or(true, |v| e < v.0) {
                    victim = Some((e, j));
                }
            }
            let Some((e, j)) = victim else { continue };
            // distance of the cluster to what keeps it now
            let cl = color::srgb_to_oklab(centre);
            let near = all.iter().map(|&c| dist(cl, color::srgb_to_oklab(c))).fold(f64::MAX, f64::min);
            if e - cur > HUE_VALUE * share * near {
                trace.push(format!("identity hue {:.0} deg ({:.1}% of the weight) has no saturated colour but keeping one would cost {:.4} error: left merged", hue.to_degrees().rem_euclid(360.0), share * 100.0, e - cur));
                continue;
            }
            trace.push(format!(
                "identity hue {:.0} deg ({:.1}% of the weight) had no saturated colour: {} replaces {}",
                hue.to_degrees().rem_euclid(360.0),
                share * 100.0,
                color::to_hex(centre),
                color::to_hex(slots[j].0)
            ));
            slots[j] = (centre, 0);
        } else {
            trace.push(format!("identity hue {:.0} deg ({:.1}% of the weight) had no saturated colour: added {}", hue.to_degrees().rem_euclid(360.0), share * 100.0, color::to_hex(centre)));
            slots.push((centre, 0));
        }
    }
    slots
}

/// Lloyd refinement of slots (colour, layer) under the biased assignment. Empty slots are dropped.
fn refine_slots(cols: &[Col], slots: Vec<(Rgb, usize)>, opts: &Opts) -> Vec<(Rgb, usize)> {
    let mut slots = slots;
    for _ in 0..6 {
        let keys: Vec<KeyInfo> =
            slots.iter().enumerate().map(|(i, s)| KeyInfo { key: i as u8, layer: s.1, rgb: s.0, under: None, role: String::new() }).collect();
        let a = assign(cols, &keys);
        let mut next = Vec::new();
        for (j, s) in slots.iter().enumerate() {
            let mem: Vec<&Col> = cols.iter().zip(&a).filter(|(_, &x)| x == j).map(|(c, _)| c).collect();
            if !mem.is_empty() {
                next.push((refine_centre(&mem, opts.mode, opts.step), s.1));
            }
        }
        next.dedup();
        if next == slots {
            break;
        }
        slots = next;
    }
    slots
}

fn slot_keys(slots: &[(Rgb, usize)]) -> Vec<KeyInfo> {
    slots.iter().enumerate().map(|(i, s)| KeyInfo { key: i as u8, layer: s.1, rgb: s.0, under: None, role: String::new() }).collect()
}

/// Subset search: exhaustive when small, otherwise greedy plus swaps. Maximises `value`.
fn search_subset(nc: usize, m: usize, value: &dyn Fn(&[usize]) -> f64) -> Vec<usize> {
    let m = m.min(nc);
    if m == 0 {
        return Vec::new();
    }
    if combos(nc, m) <= 100_000 {
        let mut cur: Vec<usize> = (0..m).collect();
        let mut best = (value(&cur), cur.clone());
        loop {
            // next combination in lexicographic order
            let mut i = m;
            while i > 0 && cur[i - 1] == nc - m + i - 1 {
                i -= 1;
            }
            if i == 0 {
                break;
            }
            cur[i - 1] += 1;
            for j in i..m {
                cur[j] = cur[j - 1] + 1;
            }
            let v = value(&cur);
            if v > best.0 + 1e-12 {
                best = (v, cur.clone());
            }
        }
        return best.1;
    }
    let mut chosen: Vec<usize> = Vec::new();
    while chosen.len() < m {
        let mut b: Option<(f64, usize)> = None;
        for j in 0..nc {
            if chosen.contains(&j) {
                continue;
            }
            let mut t = chosen.clone();
            t.push(j);
            let v = value(&t);
            if b.map_or(true, |(bv, _)| v > bv + 1e-12) {
                b = Some((v, j));
            }
        }
        chosen.push(b.map(|x| x.1).unwrap_or(0));
    }
    for _ in 0..10 {
        let mut improved = false;
        for p in 0..chosen.len() {
            let mut bv = value(&chosen);
            for j in 0..nc {
                if chosen.contains(&j) {
                    continue;
                }
                let mut t = chosen.clone();
                t[p] = j;
                let v = value(&t);
                if v > bv + 1e-12 {
                    bv = v;
                    chosen = t;
                    improved = true;
                }
            }
        }
        if !improved {
            break;
        }
    }
    chosen
}

/// Overlay centres for fixed `base`: a colour joins the overlay when an overlay colour is closer than its best
/// base colour by more than `OVERLAY_BIAS`; the centres maximise the weighted error reduction beyond that bias.
fn select_overlay(cols: &[Col], base: &[Rgb], m: usize, opts: &Opts, bias: f64) -> Vec<Rgb> {
    let blab: Vec<Oklab> = base.iter().map(|&c| color::srgb_to_oklab(c)).collect();
    let dbase: Vec<f64> = cols.iter().map(|c| blab.iter().map(|&l| dist(c.lab, l)).fold(f64::MAX, f64::min)).collect();
    // candidates: snapped source colours, those with the largest weighted base error first
    let mut order: Vec<usize> = (0..cols.len()).collect();
    order.sort_by(|&a, &b| (cols[b].w * dbase[b]).partial_cmp(&(cols[a].w * dbase[a])).unwrap_or(std::cmp::Ordering::Equal).then(a.cmp(&b)));
    let mut cand: Vec<Rgb> = Vec::new();
    for &i in &order {
        let s = color::snap_step(cols[i].rgb, opts.step);
        if !cand.contains(&s) && !base.contains(&s) {
            cand.push(s);
        }
        if cand.len() >= MAX_CAND {
            break;
        }
    }
    if cand.is_empty() {
        return Vec::new();
    }
    let clab: Vec<Oklab> = cand.iter().map(|&c| color::srgb_to_oklab(c)).collect();
    let d: Vec<Vec<f64>> = cols.iter().map(|c| clab.iter().map(|&l| dist(c.lab, l)).collect()).collect();
    let value = |sel: &[usize]| -> f64 {
        (0..cols.len())
            .map(|i| {
                let dm = sel.iter().map(|&j| d[i][j]).fold(f64::MAX, f64::min);
                cols[i].w * (dbase[i] - dm - bias).max(0.0)
            })
            .sum()
    };
    let sel = search_subset(cand.len(), m, &value);
    let mut centres: Vec<Rgb> = sel.iter().map(|&j| cand[j]).collect();
    // refine each centre on the colours it wins
    for _ in 0..4 {
        let labs: Vec<Oklab> = centres.iter().map(|&c| color::srgb_to_oklab(c)).collect();
        let mut next = Vec::new();
        for (j, &c) in centres.iter().enumerate() {
            let mem: Vec<&Col> = (0..cols.len())
                .filter(|&i| {
                    let (bj, bd) = labs.iter().enumerate().map(|(q, &l)| (q, dist(cols[i].lab, l))).fold((0, f64::MAX), |a, x| if x.1 < a.1 { x } else { a });
                    bj == j && dbase[i] - bd > bias
                })
                .map(|i| &cols[i])
                .collect();
            next.push(if mem.is_empty() { c } else { refine_centre(&mem, opts.mode, opts.step) });
        }
        if next == centres {
            break;
        }
        centres = next;
    }
    centres.sort();
    centres.dedup();
    centres
}

// ------------------------------------------------------------------ report

#[derive(Clone, Debug)]
pub struct ColRow {
    pub rgb: Rgb,
    pub n: u64,
    pub share: f64,
    pub feat_share: f64,
    /// (key, pixel count), most used first.
    pub keys: Vec<(u8, u64)>,
    /// Mean Delta E (Oklab) of the pixels of this colour.
    pub de: f64,
    /// Weighted error mass (sum of w x dE).
    pub emass: f64,
    pub implicit: bool,
}

#[derive(Clone, Debug)]
pub struct KeyUse {
    pub key: u8,
    pub role: String,
    pub color: Rgb,
    pub layer: String,
    pub px: u64,
    pub share: f64,
}

#[derive(Clone, Debug)]
pub struct Report {
    pub template: String,
    pub rows: Vec<ColRow>,
    /// Weighted mean Oklab error (area x feature importance).
    pub werr: f64,
    /// Plain mean Oklab error per pixel.
    pub merr: f64,
    /// Mean Delta E over feature pixels.
    pub ferr: f64,
    pub feature_px: u64,
    pub keys: Vec<KeyUse>,
    pub overlay_share: f64,
    pub issues: Vec<String>,
    pub total_px: u64,
    /// Reduced-resolution error: block mean colour vs the colour of the cell's majority key (what `compare`
    /// reports as `oklab`), plain and weighted by cell importance.
    pub cerr: f64,
    pub cwerr: f64,
}

impl Report {
    pub fn valid(&self) -> bool {
        self.issues.is_empty()
    }
}

fn rule_key(tpl: &Template, rules: &BTreeMap<Rgb, &MapRule>, img: &RgbaImage, x: i32, y: i32, c: Rgb) -> (u8, bool) {
    match rules.get(&c) {
        Some(rule) => {
            let applies = match rule.near {
                Some((nc, r)) => {
                    let r = r as i32;
                    let mut f = false;
                    'o: for j in -r..=r {
                        for i in -r..=r {
                            let p = img.get(x + i, y + j);
                            if p.a >= 128 && p.rgb() == nc {
                                f = true;
                                break 'o;
                            }
                        }
                    }
                    f
                }
                None => true,
            };
            if applies {
                (rule.key, false)
            } else {
                (tpl.fallback_key(rule.key, c), false)
            }
        }
        None => (nearest_base_key(tpl, c), true),
    }
}

fn nearest_base_key(tpl: &Template, c: Rgb) -> u8 {
    let o = color::srgb_to_oklab(c);
    let mut best = (b'1', f64::MAX);
    if let Some(l) = tpl.layers.first() {
        for k in &l.keys {
            if let Some(kc) = k.color {
                let d = color::oklab_dist2(o, color::srgb_to_oklab(kc));
                if d < best.1 {
                    best = (k.key, d);
                }
            }
        }
    }
    best.0
}

/// Key of every source pixel under `tpl` (0 = transparent) and whether the colour had no explicit rule.
fn pixel_keys(data: &Data, tpl: &Template) -> Vec<Vec<(u8, bool)>> {
    let rules: BTreeMap<Rgb, &MapRule> = tpl.map.iter().map(|r| (r.from, r)).collect();
    data.frames
        .iter()
        .zip(&data.imgs)
        .map(|(fp, img)| {
            (0..fp.w * fp.h)
                .map(|q| {
                    let i = fp.idx[q];
                    if i == NONE {
                        (0u8, false)
                    } else {
                        rule_key(tpl, &rules, img, (q % fp.w) as i32, (q / fp.w) as i32, data.cols[i as usize].rgb)
                    }
                })
                .collect()
        })
        .collect()
}

/// Majority key of the pixels of each cell (ties: the lower key).
fn cell_keys(data: &Data, pk: &[Vec<(u8, bool)>]) -> Vec<Vec<u8>> {
    let f = data.factor as i32;
    data.cells
        .iter()
        .zip(data.frames.iter().zip(pk))
        .map(|(cells, (fp, keys))| {
            cells
                .iter()
                .map(|c| {
                    let mut votes: BTreeMap<u8, u32> = BTreeMap::new();
                    for j in 0..f {
                        for i in 0..f {
                            let (x, y) = (c.x0 + i, c.y0 + j);
                            if x >= 0 && y >= 0 && (x as usize) < fp.w && (y as usize) < fp.h {
                                let k = keys[y as usize * fp.w + x as usize].0;
                                if k != 0 {
                                    *votes.entry(k).or_insert(0) += 1;
                                }
                            }
                        }
                    }
                    votes.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0))).map(|v| v.0).unwrap_or(b'1')
                })
                .collect()
        })
        .collect()
}

/// Cell-level error: Oklab distance between the block mean and the colour of the cell's majority key.
/// Returns (plain mean, weighted mean).
fn cell_error(data: &Data, tpl: &Template, ck: &[Vec<u8>]) -> (f64, f64) {
    let (mut e, mut n, mut ew, mut ww) = (0.0, 0.0, 0.0, 0.0);
    for (cells, keys) in data.cells.iter().zip(ck) {
        for (c, &k) in cells.iter().zip(keys) {
            let d = tpl.color_of(k).map_or(0.0, |kc| dist(c.lab, color::srgb_to_oklab(kc)));
            e += d;
            n += 1.0;
            ew += c.w * d;
            ww += c.w;
        }
    }
    (if n > 0.0 { e / n } else { 0.0 }, if ww > 0.0 { ew / ww } else { 0.0 })
}

/// Move every key colour to the best snapped centre of the block means of the cells that use it (the reduced
/// image is measured against block means, so edge cells pull outline and detail colours towards their
/// surroundings), then rebuild the map for the new colours. Repeats while the weighted cell error improves.
fn cell_refine(data: &Data, t0: &Template, opts: &Opts, trace: &mut Vec<String>) -> Template {
    let mut cur = t0.clone();
    let mut best = {
        let pk = pixel_keys(data, &cur);
        cell_error(data, &cur, &cell_keys(data, &pk)).1
    };
    let start = best;
    for _ in 0..4 {
        let pk = pixel_keys(data, &cur);
        let ck = cell_keys(data, &pk);
        // members per key: the native colours of its pixels and the block means of its cells, each side carrying
        // the same total weight (so the centre is a compromise between the identity colour and the block mean)
        let mut native: BTreeMap<u8, BTreeMap<Rgb, f64>> = BTreeMap::new();
        for ((fp, keys), _) in data.frames.iter().zip(&pk).zip(&ck) {
            for q in 0..fp.w * fp.h {
                let (k, _) = keys[q];
                if k != 0 && fp.idx[q] != NONE {
                    *native.entry(k).or_default().entry(data.cols[fp.idx[q] as usize].rgb).or_insert(0.0) += fp.wt[q];
                }
            }
        }
        let mut cellm: BTreeMap<u8, BTreeMap<Rgb, f64>> = BTreeMap::new();
        for (cells, keys) in data.cells.iter().zip(&ck) {
            for (c, &k) in cells.iter().zip(keys) {
                *cellm.entry(k).or_default().entry(c.mean).or_insert(0.0) += c.w;
            }
        }
        let mut members: BTreeMap<u8, BTreeMap<Rgb, (f64, u64)>> = BTreeMap::new();
        for (k, nm) in &native {
            let nw: f64 = nm.values().sum();
            let e = members.entry(*k).or_default();
            for (&c, &w) in nm {
                let x = e.entry(c).or_insert((0.0, 1));
                x.0 += w;
            }
            if let Some(cm) = cellm.get(k) {
                let cw: f64 = cm.values().sum();
                let scale = if cw > 0.0 { nw / cw * CELL_BLEND } else { 0.0 };
                for (&c, &w) in cm {
                    let x = e.entry(c).or_insert((0.0, 1));
                    x.0 += w * scale;
                }
            }
        }
        let mut next = cur.clone();
        for l in next.layers.iter_mut() {
            for kd in l.keys.iter_mut() {
                if let Some(m) = members.get(&kd.key) {
                    let cols: Vec<Col> = m.iter().map(|(&rgb, &(w, n))| Col { rgb, lab: color::srgb_to_oklab(rgb), n, w, feat: 0 }).collect();
                    let refs: Vec<&Col> = cols.iter().collect();
                    kd.color = Some(refine_centre(&refs, opts.mode, opts.step));
                }
            }
        }
        // key 1 stays the darkest base colour
        if let Some(l0) = next.layers.first() {
            let lum = |k: &KeyDef| k.color.map(|c| color::srgb_to_oklab(c).l).unwrap_or(f64::MAX);
            if let Some(first) = l0.keys.iter().find(|k| k.key == b'1') {
                if l0.keys.iter().any(|k| lum(k) < lum(first) - 1e-9) {
                    break;
                }
            }
        }
        let keys: Vec<KeyInfo> = next
            .layers
            .iter()
            .enumerate()
            .flat_map(|(li, l)| l.keys.iter().filter_map(move |k| k.color.map(|c| KeyInfo { key: k.key, layer: li, rgb: c, under: k.under, role: k.role.clone() })))
            .collect();
        let mut notes = Vec::new();
        next.map = build_map(data, &keys, &mut notes);
        let pk2 = pixel_keys(data, &next);
        let e = cell_error(data, &next, &cell_keys(data, &pk2)).1;
        if e + 1e-9 < best {
            best = e;
            cur = next;
        } else {
            break;
        }
    }
    trace.push(format!("cell refinement (colours re-centred on block means): weighted cell error {:.4} -> {:.4}", start, best));
    cur
}

/// Apply `tpl` to the native pixels with the reducer's rule semantics (explicit rule, `near` filter, `under`
/// fallback, otherwise nearest base key) and measure the Oklab error against the source colours.
pub fn evaluate(data: &Data, tpl: &Template, k_limit: usize, step: u32) -> Report {
    let rules: BTreeMap<Rgb, &MapRule> = tpl.map.iter().map(|r| (r.from, r)).collect();
    struct Acc {
        n: u64,
        feat: u64,
        keys: BTreeMap<u8, u64>,
        esum: f64,
        emass: f64,
        implicit: bool,
    }
    let mut acc: Vec<Acc> = data.cols.iter().map(|_| Acc { n: 0, feat: 0, keys: BTreeMap::new(), esum: 0.0, emass: 0.0, implicit: false }).collect();
    let mut key_px: BTreeMap<u8, u64> = BTreeMap::new();
    let (mut ew, mut ww, mut en, mut fe, mut fn_) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0u64);
    let mut total = 0u64;
    for (fp, img) in data.frames.iter().zip(&data.imgs) {
        for y in 0..fp.h {
            for x in 0..fp.w {
                let i = fp.idx[y * fp.w + x];
                if i == NONE {
                    continue;
                }
                let col = &data.cols[i as usize];
                let (key, implicit) = rule_key(tpl, &rules, img, x as i32, y as i32, col.rgb);
                let eff = tpl.color_of(key).unwrap_or(col.rgb);
                let de = dist(col.lab, color::srgb_to_oklab(eff));
                let w = fp.wt[y * fp.w + x];
                let a = &mut acc[i as usize];
                a.n += 1;
                *a.keys.entry(key).or_insert(0) += 1;
                a.esum += de;
                a.emass += w * de;
                a.implicit |= implicit;
                if fp.feat[y * fp.w + x] {
                    a.feat += 1;
                    fe += de;
                    fn_ += 1;
                }
                *key_px.entry(key).or_insert(0) += 1;
                ew += w * de;
                ww += w;
                en += de;
                total += 1;
            }
        }
    }
    let rows: Vec<ColRow> = data
        .cols
        .iter()
        .zip(acc)
        .map(|(c, a)| {
            let mut keys: Vec<(u8, u64)> = a.keys.into_iter().collect();
            keys.sort_by(|x, y| y.1.cmp(&x.1).then(x.0.cmp(&y.0)));
            ColRow {
                rgb: c.rgb,
                n: a.n,
                share: a.n as f64 / total.max(1) as f64,
                feat_share: if a.n > 0 { a.feat as f64 / a.n as f64 } else { 0.0 },
                keys,
                de: if a.n > 0 { a.esum / a.n as f64 } else { 0.0 },
                emass: a.emass,
                implicit: a.implicit,
            }
        })
        .collect();
    let mut keys = Vec::new();
    let mut issues = Vec::new();
    let mut overlay_px = 0u64;
    for (li, l) in tpl.layers.iter().enumerate() {
        let colours = l.keys.iter().filter(|k| k.color.is_some()).count();
        if colours > k_limit {
            issues.push(format!("layer '{}' has {} colours, the profile allows {}", l.name, colours, k_limit));
        }
        for k in &l.keys {
            if let Some(c) = k.color {
                if !color::is_step_aligned(c, step) {
                    issues.push(format!("key '{}' colour {} is not a multiple of channel_step {}", k.key as char, color::to_hex(c), step));
                }
                let px = key_px.get(&k.key).copied().unwrap_or(0);
                if li > 0 {
                    overlay_px += px;
                }
                keys.push(KeyUse { key: k.key, role: k.role.clone(), color: c, layer: l.name.clone(), px, share: px as f64 / total.max(1) as f64 });
            }
            if let Some(u) = k.under {
                if tpl.layer_of(u) != Some(0) {
                    issues.push(format!("key '{}' has under '{}' which is not a base key", k.key as char, u as char));
                }
            }
        }
    }
    if let Some(l0) = tpl.layers.first() {
        let lum = |k: &KeyDef| k.color.map(|c| color::srgb_to_oklab(c).l).unwrap_or(f64::MAX);
        if let Some(first) = l0.keys.iter().find(|k| k.key == b'1') {
            if l0.keys.iter().any(|k| lum(k) < lum(first) - 1e-9) {
                issues.push("key '1' is not the darkest base colour".into());
            }
        }
    }
    let unmapped = rows.iter().filter(|r| r.implicit).count();
    if unmapped > 0 {
        issues.push(format!("{} source colour(s) have no explicit [map] rule (the reducer falls back to the nearest base key)", unmapped));
    }
    let (cerr, cwerr) = {
        let pk = pixel_keys(data, tpl);
        cell_error(data, tpl, &cell_keys(data, &pk))
    };
    Report {
        cerr,
        cwerr,
        template: tpl.name.clone(),
        rows,
        werr: if ww > 0.0 { ew / ww } else { 0.0 },
        merr: if total > 0 { en / total as f64 } else { 0.0 },
        ferr: if fn_ > 0 { fe / fn_ as f64 } else { 0.0 },
        feature_px: fn_,
        keys,
        overlay_share: overlay_px as f64 / total.max(1) as f64,
        issues,
        total_px: total,
    }
}

impl Report {
    pub fn lines(&self, tpl: &Template) -> Vec<String> {
        let mut v = vec![format!(
            "template {}: weighted error {:.4}  mean error {:.4}  cell error {:.4} (weighted {:.4})  feature error {:.4} ({} feature px)  overlay share {:.1}%  hardware {}",
            self.template,
            self.werr,
            self.merr,
            self.cerr,
            self.cwerr,
            self.ferr,
            self.feature_px,
            self.overlay_share * 100.0,
            if self.valid() { "valid".to_string() } else { format!("INVALID ({})", self.issues.join("; ")) }
        )];
        for k in &self.keys {
            v.push(format!("  layer {:<8} key {} {:<10} {}  {:>5.1}% of pixels", k.layer, k.key as char, k.role, color::to_hex(k.color), k.share * 100.0));
        }
        v.push("  source colour   share  feat   dE     -> key (template colour)".into());
        let mut rows: Vec<&ColRow> = self.rows.iter().collect();
        rows.sort_by(|a, b| b.n.cmp(&a.n).then(a.rgb.cmp(&b.rgb)));
        for r in rows {
            let ks: Vec<String> = r
                .keys
                .iter()
                .map(|(k, n)| {
                    let hex = tpl.color_of(*k).map(color::to_hex).unwrap_or_default();
                    if r.keys.len() > 1 {
                        format!("{} {} ({:.0}%)", *k as char, hex, *n as f64 * 100.0 / r.n.max(1) as f64)
                    } else {
                        format!("{} {}", *k as char, hex)
                    }
                })
                .collect();
            v.push(format!(
                "  {}  {:>6.2}% {:>4.0}%  {:.3}  -> {}{}",
                color::to_hex(r.rgb),
                r.share * 100.0,
                r.feat_share * 100.0,
                r.de,
                ks.join(", "),
                if r.implicit { "  (implicit)" } else { "" }
            ));
        }
        v
    }

    pub fn to_json(&self) -> Value {
        json!({
            "template": self.template,
            "weighted_error": round4(self.werr),
            "mean_error": round4(self.merr),
            "cell_error": round4(self.cerr),
            "weighted_cell_error": round4(self.cwerr),
            "feature_error": round4(self.ferr),
            "feature_px": self.feature_px,
            "overlay_share": round4(self.overlay_share),
            "valid": self.valid(),
            "issues": self.issues,
            "keys": self.keys.iter().map(|k| json!({"key": (k.key as char).to_string(), "role": k.role, "color": color::to_hex(k.color), "layer": k.layer, "px": k.px, "share": round4(k.share)})).collect::<Vec<_>>(),
            "colours": self.rows.iter().map(|r| json!({
                "source": color::to_hex(r.rgb), "px": r.n, "share": round4(r.share), "feature_share": round4(r.feat_share),
                "delta_e": round4(r.de), "keys": r.keys.iter().map(|(k, n)| json!({"key": (*k as char).to_string(), "px": n})).collect::<Vec<_>>(),
                "implicit": r.implicit })).collect::<Vec<_>>(),
        })
    }
}

fn round4(v: f64) -> f64 {
    (v * 10000.0).round() / 10000.0
}

// ------------------------------------------------------------------ derive

pub struct Derived {
    pub template: Template,
    pub text: String,
    pub report: Report,
    /// Decisions: layer error table, overlay verdict, near rules.
    pub trace: Vec<String>,
}

/// Derive a template from native resolution frames. `optimised` mode also runs the `real` search and keeps
/// whichever template has the lower weighted error, so it is never worse than `real`.
pub fn derive_from_data(data: &Data, opts: &Opts) -> Derived {
    let a = derive_inner(data, opts);
    if opts.mode == Mode::Optimised {
        let b = derive_inner(data, &Opts { mode: Mode::Real, ..opts.clone() });
        if b.report.cwerr + 1e-9 < a.report.cwerr {
            let mut b = b;
            b.trace.insert(0, format!("the real-colour search (cell error {:.4}) beat the hill-climbed search (cell error {:.4}); kept it", b.report.cwerr, a.report.cwerr));
            return b;
        }
    }
    a
}

fn derive_inner(data: &Data, opts: &Opts) -> Derived {
    // a flat (one layer) template may use more than nine colours: keys 1-9, then a-g
    let k = opts.k.clamp(1, if opts.layers == Some(1) { MAX_BASE } else { 9 });
    let cols = &data.cols;
    let mut trace = Vec::new();
    let base_only: Vec<(Rgb, usize)> = {
        let p = pick_centres(cols, k, opts.mode, opts.step);
        refine_slots(cols, p.into_iter().map(|c| (c, 0)).collect(), opts)
    };
    let e1 = mean_err(cols, &base_only.iter().map(|s| s.0).collect::<Vec<_>>());
    trace.push(format!("base only ({} colours): weighted error {:.4}", base_only.len(), e1));
    let mut chosen = base_only.clone();
    if opts.layers != Some(1) && cols.len() > k {
        let forced = opts.layers == Some(2);
        let mut options: Vec<(usize, f64, Vec<(Rgb, usize)>)> = Vec::new();
        for m in 1..=k {
            // alternate: overlay for the current base, then re-fit the base to what stays in it
            let mut base: Vec<Rgb> = base_only.iter().map(|s| s.0).collect();
            let mut ov = select_overlay(cols, &base, m, opts, OVERLAY_BIAS);
            if ov.is_empty() && forced {
                ov = select_overlay(cols, &base, m, opts, 0.0);
            }
            if ov.is_empty() {
                continue;
            }
            for _ in 0..3 {
                let keys: Vec<KeyInfo> = base
                    .iter()
                    .map(|&c| (c, 0usize))
                    .chain(ov.iter().map(|&c| (c, 1usize)))
                    .enumerate()
                    .map(|(i, s)| KeyInfo { key: i as u8, layer: s.1, rgb: s.0, under: None, role: String::new() })
                    .collect();
                let a = assign(cols, &keys);
                let rest: Vec<Col> = cols.iter().zip(&a).filter(|(_, &j)| keys[j].layer == 0).map(|(c, _)| Col { rgb: c.rgb, lab: c.lab, n: c.n, w: c.w, feat: c.feat }).collect();
                if rest.is_empty() {
                    break;
                }
                let nb = pick_centres(&rest, k, opts.mode, opts.step);
                let nov = select_overlay(cols, &nb, m, opts, OVERLAY_BIAS);
                if nb == base && nov == ov {
                    break;
                }
                if nov.is_empty() {
                    break;
                }
                base = nb;
                ov = nov;
            }
            let slots = refine_slots(cols, base.iter().map(|&c| (c, 0usize)).chain(ov.iter().map(|&c| (c, 1usize))).collect(), opts);
            if !slots.iter().any(|s| s.1 == 1) {
                continue;
            }
            let keys = slot_keys(&slots);
            let a = assign(cols, &keys);
            let e = assigned_error(cols, &keys, &a);
            let ovpx: u64 = cols.iter().zip(&a).filter(|(_, &j)| keys[j].layer > 0).map(|(c, _)| c.n).sum();
            trace.push(format!(
                "base {} + overlay {} colour(s): weighted error {:.4}, overlay pixels {:.1}%",
                slots.iter().filter(|s| s.1 == 0).count(),
                slots.iter().filter(|s| s.1 == 1).count(),
                e,
                ovpx as f64 * 100.0 / data.total_n.max(1) as f64
            ));
            options.push((m, e, slots));
        }
        if let Some(best) = options.iter().map(|o| o.1).fold(None, |a: Option<f64>, e| Some(a.map_or(e, |x| x.min(e)))) {
            let tol = (best * 0.10).max(0.003);
            let pick = options.iter().find(|o| o.1 <= best + tol).unwrap();
            let gain = e1 - pick.1;
            let need = OVERLAY_MIN_GAIN_ABS.max(OVERLAY_MIN_GAIN_REL * e1);
            if forced {
                trace.push(format!("overlay forced by --layers 2 (gain {:.4})", gain));
                chosen = pick.2.clone();
            } else if gain >= need {
                trace.push(format!("overlay added with {} colour(s): gain {:.4} >= required {:.4} (max of {} abs, {:.0}% of base error)", pick.0, gain, need, OVERLAY_MIN_GAIN_ABS, OVERLAY_MIN_GAIN_REL * 100.0));
                chosen = pick.2.clone();
            } else {
                trace.push(format!("no overlay: gain {:.4} < required {:.4}", gain, need));
            }
        } else {
            trace.push("no overlay: no colour is better represented by an overlay colour by more than the bias".into());
        }
    } else if opts.layers == Some(1) {
        trace.push("overlay disabled by --layers 1".into());
    }
    chosen = protect_hues(cols, chosen, opts, &mut trace);
    let template = finalize(data, &chosen, opts, "detail", &mut trace);
    let template = if opts.cells { cell_refine(data, &template, opts, &mut trace) } else { template };
    let report = evaluate(data, &template, opts.k, opts.step);
    let text = template_text(&template, &report, data, &opts.name, &format!("{} mode, {} colour(s) per layer", opts.mode.name(), opts.k), &trace);
    Derived { template, text, report, trace }
}

/// Keep the colours of `tpl`; derive only the `[map]` (and fill missing `under`) from the native pixels.
pub fn derive_map(data: &Data, tpl: &Template, opts: &Opts) -> Derived {
    let mut keys: Vec<KeyInfo> = Vec::new();
    for (li, l) in tpl.layers.iter().enumerate() {
        for k in &l.keys {
            if let Some(c) = k.color {
                keys.push(KeyInfo { key: k.key, layer: li, rgb: c, under: k.under, role: k.role.clone() });
            }
        }
    }
    let a = assign(&data.cols, &keys);
    for j in 0..keys.len() {
        if keys[j].layer > 0 && keys[j].under.is_none() {
            let mem: Vec<usize> = (0..data.cols.len()).filter(|&i| a[i] == j).collect();
            keys[j].under = choose_under(&data.cols, &mem, &keys).or(Some(b'1'));
        }
    }
    let mut trace = vec![format!("colours kept from template '{}'; overlay bias {} (an overlay colour must beat the best base colour by more than this)", tpl.name, OVERLAY_BIAS)];
    let map = build_map(data, &keys, &mut trace);
    let mut t = tpl.clone();
    t.name = opts.name.clone();
    t.path = std::path::PathBuf::new();
    t.map = map;
    for l in t.layers.iter_mut() {
        for kd in l.keys.iter_mut() {
            if let Some(k) = keys.iter().find(|k| k.key == kd.key) {
                kd.under = k.under;
            }
        }
    }
    let report = evaluate(data, &t, opts.k, opts.step);
    let text = template_text(&t, &report, data, &opts.name, "map derived for fixed colours", &trace);
    Derived { template: t, text, report, trace }
}

fn name_ok(s: &str) -> String {
    s.chars().map(|c| if c == '"' || c == '\\' || c.is_control() { '_' } else { c }).collect()
}

/// TOML text of a template, with per-colour comments in `[map]`.
pub fn template_text(t: &Template, rep: &Report, data: &Data, name: &str, how: &str, trace: &[String]) -> String {
    let mut s = String::new();
    s.push_str(&format!("# derived from the native resolution inputs by `pxl template suggest --derive` ({})\n", how));
    s.push_str(&format!(
        "# weighted Oklab error {:.4}, mean {:.4}, cell {:.4}, feature {:.4}; {} source colours, {} frames\n",
        rep.werr,
        rep.merr,
        rep.cerr,
        rep.ferr,
        data.cols.len(),
        data.frames.len()
    ));
    for l in trace {
        s.push_str(&format!("# {}\n", l));
    }
    s.push_str(&format!("name = \"{}\"\n", name_ok(name)));
    for (li, l) in t.layers.iter().enumerate() {
        s.push_str(&format!("\n[layers.{}]\n", l.name));
        if li == 0 {
            s.push_str("\".\" = { role = \"transparent\" }\n");
        }
        for k in &l.keys {
            if let Some(c) = k.color {
                s.push_str(&format!("\"{}\" = {{ role = \"{}\", color = \"{}\"", k.key as char, k.role, color::to_hex(c)));
                if let Some(u) = k.under {
                    s.push_str(&format!(", under = \"{}\"", u as char));
                }
                s.push_str(" }\n");
            }
        }
    }
    s.push_str("\n[map]\n");
    let mut rules: Vec<&MapRule> = t.map.iter().collect();
    rules.sort_by_key(|r| r.from);
    for r in rules {
        let note = rep.rows.iter().find(|x| x.rgb == r.from).map(|x| format!("  # {:.2}% of pixels, dE {:.3}", x.share * 100.0, x.de)).unwrap_or_default();
        match r.near {
            Some((c, d)) => s.push_str(&format!("\"{}\" = {{ key = \"{}\", near = [\"{}\", {}] }}{}\n", color::to_hex(r.from), r.key as char, color::to_hex(c), d, note)),
            None => s.push_str(&format!("\"{}\" = \"{}\"{}\n", color::to_hex(r.from), r.key as char, note)),
        }
    }
    s
}

/// Side by side comparison lines of several reports of the same data.
pub fn compare_lines(reps: &[(&Report, &Template)]) -> Vec<String> {
    let mut v = Vec::new();
    let head: Vec<String> = reps.iter().map(|(r, _)| format!("{:>14}", r.template.chars().take(14).collect::<String>())).collect();
    v.push(format!("{:<26}{}", "", head.join(" ")));
    let row = |label: &str, f: &dyn Fn(&Report) -> String| format!("{:<26}{}", label, reps.iter().map(|(r, _)| format!("{:>14}", f(r))).collect::<Vec<_>>().join(" "));
    v.push(row("weighted error", &|r| format!("{:.4}", r.werr)));
    v.push(row("mean error", &|r| format!("{:.4}", r.merr)));
    v.push(row("cell error (compare)", &|r| format!("{:.4}", r.cerr)));
    v.push(row("weighted cell error", &|r| format!("{:.4}", r.cwerr)));
    v.push(row("feature error", &|r| format!("{:.4}", r.ferr)));
    v.push(row("overlay pixel share", &|r| format!("{:.1}%", r.overlay_share * 100.0)));
    v.push(row("colours (base/overlay)", &|r| {
        let b = r.keys.iter().filter(|k| k.layer == "base").count();
        format!("{}/{}", b, r.keys.len() - b)
    }));
    v.push(row("hardware valid", &|r| if r.valid() { "yes".into() } else { format!("no ({})", r.issues.len()) }));
    v.push(String::new());
    v.push(format!("{:<26}{}", "source colour (share)", reps.iter().map(|_| format!("{:>14}", "key dE")).collect::<Vec<_>>().join(" ")));
    let first = reps[0].0;
    let mut order: Vec<usize> = (0..first.rows.len()).collect();
    order.sort_by(|&a, &b| first.rows[b].n.cmp(&first.rows[a].n).then(first.rows[a].rgb.cmp(&first.rows[b].rgb)));
    for i in order {
        let r0 = &first.rows[i];
        let cells: Vec<String> = reps
            .iter()
            .map(|(r, t)| {
                let row = &r.rows[i];
                let key = row.keys.first().map(|k| k.0 as char).unwrap_or('?');
                let hex = row.keys.first().and_then(|k| t.color_of(k.0)).map(color::to_hex).unwrap_or_default();
                format!("{:>14}", format!("{} {} {:.3}", key, &hex.get(1..).unwrap_or(""), row.de))
            })
            .collect();
        v.push(format!("{}  {:>6.2}%        {}", color::to_hex(r0.rgb), r0.share * 100.0, cells.join(" ")));
    }
    v
}
