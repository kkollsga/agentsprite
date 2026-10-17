//! Reduction of a high resolution reference to a grid of template keys (DESIGN 5.1).
//!
//! One sampling phase per part. For every target cell the reducer looks at its *block* (the source pixels the
//! cell covers) and its *ring* (a band around the block):
//!
//! 1. alpha: majority of the block; a tie is decided by the ring coverage; a thin opaque line that runs through
//!    block and ring keeps the cell opaque (`thin`);
//! 2. colour: every source pixel is classified into one template key (`[map]` rules with `near` filters,
//!    otherwise the nearest base-layer key in Oklab); votes: block 1.0, ring 0.2. Two bonuses protect features:
//!    an isolated detail (key present in the block, absent from the ring) and a 1-2 px band that continues
//!    through the ring (separator, line);
//! 3. smoothing: iterated conditional modes; protected cells (unanimous blocks, bonus winners) never flip;
//! 4. outline for the configured roles, then orphan cleanup;
//! 5. overlay keys take part in the same vote (the grid stores the overlay key, the base under it is implied by
//!    the template's `under`).
//!
//! The steps above describe the default method `vote`. `[reduce] method` (or `load/reload --method`) selects
//! `nearest`, `contour`, `exemplar` or `auto` instead; see the submodules. `reload --try a,b,c` compares them.
//!
//! Everything is integer arithmetic (votes are fixed point), ties go to the lower palette index, no randomness.

use crate::fsutil::PathSlash;
use crate::color::{self, Oklab, Rgb};
use crate::img::RgbaImage;
use crate::model::{Grid, Method, Part, RefFrame};
use crate::output::{Loc, PxlError, Result};
use crate::template::{MapRule, Template};
use std::collections::BTreeMap;

pub mod auto;
pub mod contour;
pub mod exemplar;
pub mod nearest;
pub mod trial;

const W_BLOCK: i64 = 100;
const W_RING: i64 = 20;
/// Smoothing strength: cost per 4-neighbour with a different key.
const LAMBDA: i64 = 60;
/// A half-covered block is opaque when at least this percentage of its ring is opaque.
const TIE_RING_PCT: i64 = 16;
const ICM_PASSES: usize = 6;
/// Share of the reference's cell-equivalent area of a salient key that the output keeps at least.
const SHARE_TARGET: f64 = 0.9;
pub const TRANSPARENT: u8 = b'.';

struct Votes {
    /// (key, votes) sorted by palette rank.
    v: Vec<(u8, i64)>,
}

impl Votes {
    fn add(&mut self, key: u8, w: i64, rank: &[usize; 256]) {
        match self.v.iter().position(|e| e.0 == key) {
            Some(i) => self.v[i].1 += w,
            None => {
                let p = self.v.iter().position(|e| rank[e.0 as usize] > rank[key as usize]).unwrap_or(self.v.len());
                self.v.insert(p, (key, w));
            }
        }
    }
    /// Highest vote, ties to the lower palette rank (the vector is sorted by rank, so the first wins).
    fn best(&self) -> Option<u8> {
        let mut b: Option<(u8, i64)> = None;
        for &(k, w) in &self.v {
            if b.map_or(true, |x| w > x.1) {
                b = Some((k, w));
            }
        }
        b.map(|x| x.0)
    }
}

/// Classifier: source pixel -> template key.
pub struct Classifier<'a> {
    tpl: &'a Template,
    rules: BTreeMap<Rgb, &'a MapRule>,
    /// Base-layer keys with their Oklab colour, in palette order.
    base: Vec<(u8, Oklab)>,
}

impl<'a> Classifier<'a> {
    pub fn new(tpl: &'a Template) -> Result<Classifier<'a>> {
        let mut base = Vec::new();
        if let Some(l) = tpl.layers.first() {
            for k in &l.keys {
                if let Some(c) = k.color {
                    base.push((k.key, color::srgb_to_oklab(c)));
                }
            }
        }
        if base.is_empty() {
            return Err(PxlError::validation(format!("template '{}' has no base layer colours", tpl.name))
                .with_hint("the first [layers.<name>] must define at least one coloured key"));
        }
        let mut rules = BTreeMap::new();
        for r in &tpl.map {
            rules.insert(r.from, r);
        }
        Ok(Classifier { tpl, rules, base })
    }

    pub fn nearest_base(&self, c: Rgb) -> u8 {
        let o = color::srgb_to_oklab(c);
        let mut best = (self.base[0].0, f64::MAX);
        for &(k, ok) in &self.base {
            let d = color::oklab_dist2(o, ok);
            if d < best.1 {
                best = (k, d);
            }
        }
        best.0
    }

    fn near(img: &RgbaImage, x: i32, y: i32, c: Rgb, r: u32) -> bool {
        let r = r as i32;
        for j in -r..=r {
            for i in -r..=r {
                let p = img.get(x + i, y + j);
                if p.a >= 128 && p.rgb() == c {
                    return true;
                }
            }
        }
        false
    }

    fn fallback(&self, rule: &MapRule, c: Rgb) -> u8 {
        self.tpl.fallback_key(rule.key, c)
    }

    pub fn classify(&self, img: &RgbaImage, x: i32, y: i32) -> u8 {
        let p = img.get(x, y);
        if p.a < 128 {
            return TRANSPARENT;
        }
        let c = p.rgb();
        match self.rules.get(&c) {
            Some(rule) => {
                let applies = match rule.near {
                    Some((nc, r)) => Self::near(img, x, y, nc, r),
                    None => true,
                };
                if applies {
                    rule.key
                } else {
                    self.fallback(rule, c)
                }
            }
            None => self.nearest_base(c),
        }
    }
}

/// Source rectangle (x0, y0, x1, y1), exclusive end, covered by target cell (X, Y).
pub struct Geometry {
    pub f: (i32, i32),
    /// Source coordinate of the top left of cell (0, 0).
    pub org: (i32, i32),
    /// `Some((src w, src h, target w, target h))` for area reduction to an explicit target size.
    pub area: Option<(i32, i32, i32, i32)>,
}

impl Geometry {
    pub fn block(&self, cx: i32, cy: i32) -> (i32, i32, i32, i32) {
        match self.area {
            Some((sw, sh, tw, th)) => (cx * sw / tw, cy * sh / th, (cx + 1) * sw / tw, (cy + 1) * sh / th),
            None => {
                let x0 = self.org.0 + self.f.0 * cx;
                let y0 = self.org.1 + self.f.1 * cy;
                (x0, y0, x0 + self.f.0, y0 + self.f.1)
            }
        }
    }
}

/// Is there a 1-2 px thick band of keys satisfying `pred` that runs through the block and continues through
/// the ring on both ends (horizontally or vertically)?
pub fn band_through(cls: &dyn Fn(i32, i32) -> u8, pred: &dyn Fn(u8) -> bool, blk: (i32, i32, i32, i32), ring: i32) -> bool {
    let (x0, y0, x1, y1) = blk;
    // horizontal: rows that are `pred` along the whole block + ring span
    let row_ok = |y: i32| (x0 - ring..x1 + ring).all(|x| pred(cls(x, y)));
    let col_ok = |x: i32| (y0 - ring..y1 + ring).all(|y| pred(cls(x, y)));
    let thin = |ok: &dyn Fn(i32) -> bool, lo: i32, hi: i32| -> bool {
        // some line in [lo, hi) qualifies and the contiguous run of qualifying lines through it is <= 2
        for l in lo..hi {
            if ok(l) {
                let mut a = l;
                while ok(a - 1) && l - a < 3 {
                    a -= 1;
                }
                let mut b = l;
                while ok(b + 1) && b - l < 3 {
                    b += 1;
                }
                if b - a + 1 <= 2 {
                    return true;
                }
            }
        }
        false
    };
    thin(&row_ok, y0, y1) || thin(&col_ok, x0, x1)
}

/// Thin separator band (DESIGN 5.1 step 2): a run of 1-2 source rows (or columns) inside the block whose pixels
/// differ from the key that dominates the line just before the run and the line just after it, over the span of
/// block + margin. Returns the key that dominates the band. A band of 2 lines that straddles two cells belongs
/// to the cell holding its first line.
pub fn band_key(cls: &dyn Fn(i32, i32) -> u8, blk: (i32, i32, i32, i32), margin: i32, vertical: bool, rank: &[usize; 256]) -> Option<u8> {
    let (x0, y0, x1, y1) = blk;
    // (line range inside the block, span along the line)
    let (l0, l1, s0, s1) = if vertical { (x0, x1, y0 - margin, y1 + margin) } else { (y0, y1, x0 - margin, x1 + margin) };
    let px = |l: i32, s: i32| -> u8 {
        if vertical {
            cls(l, s)
        } else {
            cls(s, l)
        }
    };
    let span = (s1 - s0) as i64;
    let dominant = |l: i32| -> Option<u8> {
        let mut t: Vec<(u8, i64)> = Vec::new();
        for s in s0..s1 {
            let k = px(l, s);
            if k != TRANSPARENT {
                match t.iter_mut().find(|e| e.0 == k) {
                    Some(e) => e.1 += 1,
                    None => t.push((k, 1)),
                }
            }
        }
        let mut best: Option<(u8, i64)> = None;
        for &(k, c) in &t {
            if best.map_or(true, |b| c > b.1 || (c == b.1 && rank[k as usize] < rank[b.0 as usize])) {
                best = Some((k, c));
            }
        }
        best.filter(|b| b.1 * 2 >= span).map(|b| b.0)
    };
    for t in 1..=2i32 {
        for b in (l0 - t + 1)..l1 {
            // lines b..b+t; the run must touch the block and belong to this cell
            let inside: Vec<i32> = (b..b + t).filter(|l| *l >= l0 && *l < l1).collect();
            if inside.is_empty() {
                continue;
            }
            if t == 2 && inside.len() == 1 && b < l0 {
                continue; // first line is in the cell before: that cell owns the band
            }
            let (ka, kb) = match (dominant(b - 1), dominant(b + t)) {
                (Some(a), Some(c)) => (a, c),
                _ => continue,
            };
            let mut count: Vec<(u8, i64)> = Vec::new();
            let (mut other, mut total) = (0i64, 0i64);
            for l in b..b + t {
                for s in s0..s1 {
                    let k = px(l, s);
                    total += 1;
                    if k != TRANSPARENT && k != ka && k != kb {
                        other += 1;
                        match count.iter_mut().find(|e| e.0 == k) {
                            Some(e) => e.1 += 1,
                            None => count.push((k, 1)),
                        }
                    }
                }
            }
            if other * 4 < total * 3 {
                continue;
            }
            let mut best: Option<(u8, i64)> = None;
            for &(k, c) in &count {
                if best.map_or(true, |x| c > x.1 || (c == x.1 && rank[k as usize] < rank[x.0 as usize])) {
                    best = Some((k, c));
                }
            }
            if let Some(x) = best {
                return Some(x.0);
            }
        }
    }
    None
}

/// Shared preparation of a reduction: geometry, classified source image, tie-break ranks.
pub struct Prep {
    pub gw: usize,
    pub gh: usize,
    pub geo: Geometry,
    pub rank: [usize; 256],
    pub ring: i32,
    pub iw: i32,
    pub ih: i32,
    cls_img: Vec<u8>,
    /// Eyes found on the native reference (empty for area reduction).
    pub eyes: Vec<crate::features::Feature>,
}

impl Prep {
    pub fn new(img: &RgbaImage, origin: (i32, i32), part: &Part, tpl: &Template) -> Result<Prep> {
        let (gw, gh) = part.size;
        let cl = Classifier::new(tpl)?;
        let mut rank = [usize::MAX; 256];
        rank[TRANSPARENT as usize] = 0;
        for (i, k) in tpl.keys().enumerate() {
            rank[k.key as usize] = i + 1;
        }
        let factor = part.reduce.factor.or(part.input.factor);
        let geo = match (factor, part.input.target) {
            (None, Some((tw, th))) if tw > 0 && th > 0 => {
                Geometry { f: (1, 1), org: (0, 0), area: Some((img.w as i32, img.h as i32, tw as i32, th as i32)) }
            }
            _ => {
                let f = factor.unwrap_or(1).max(1) as i32;
                let ph = part.reduce.phase.unwrap_or((0, 0));
                Geometry { f: (f, f), org: (origin.0 - f * part.origin.0 + ph.0, origin.1 - f * part.origin.1 + ph.1), area: None }
            }
        };
        let (bw, bh) = {
            let b = geo.block(0, 0);
            ((b.2 - b.0).max(1), (b.3 - b.1).max(1))
        };
        let ring = ((bw.min(bh) + 1) / 2).max(1);
        let (iw, ih) = (img.w as i32, img.h as i32);
        let mut cls_img = vec![TRANSPARENT; img.w * img.h];
        for y in 0..ih {
            for x in 0..iw {
                cls_img[y as usize * img.w + x as usize] = cl.classify(img, x, y);
            }
        }
        let eyes = match geo.area {
            None => crate::features::detect(img, crate::features::Geo { org: geo.org, f: geo.f.0 }),
            Some(_) => Vec::new(),
        };
        Ok(Prep { gw, gh, geo, rank, ring, iw, ih, cls_img, eyes })
    }
    /// Template key of a source pixel (`.` outside the image or transparent).
    pub fn cls(&self, x: i32, y: i32) -> u8 {
        if x < 0 || y < 0 || x >= self.iw || y >= self.ih {
            TRANSPARENT
        } else {
            self.cls_img[y as usize * self.iw as usize + x as usize]
        }
    }
    pub fn block(&self, cx: i32, cy: i32) -> (i32, i32, i32, i32) {
        self.geo.block(cx, cy)
    }
    /// Native pixels per key (index = key byte), opaque pixels only.
    pub fn key_pixels(&self) -> [u64; 256] {
        let mut n = [0u64; 256];
        for &k in &self.cls_img {
            if k != TRANSPARENT {
                n[k as usize] += 1;
            }
        }
        n
    }

    /// Identity colour pass (shoes, glove highlights, mouths): salient keys keep a proportional share of the
    /// cells and at least one cell per contiguous source patch of a cell or more. `vote` lets the dominant
    /// shade win every block, so a bright highlight on a mostly dark shoe disappears; here the cells where the
    /// salient key has the largest share of the block flip to it until the output holds `SHARE_TARGET` of the
    /// reference's cell-equivalent area. Cells that keep the silhouette untouched: only opaque cells change, and
    /// an outline cell on the silhouette edge flips only when the key covers half of its block.
    pub fn protect_salient(&self, grid: &mut Grid, tpl: &Template) {
        let f2 = (self.geo.f.0 * self.geo.f.1).max(1) as i64;
        let pix = self.key_pixels();
        let total: u64 = pix.iter().sum();
        let keys = crate::features::salient_keys(tpl, &pix, total, f2 as u64);
        if keys.is_empty() {
            return;
        }
        let outline = tpl.key_for_role("outline");
        let (gw, gh) = (self.gw as i32, self.gh as i32);
        let edge = |g: &Grid, x: i32, y: i32| [(0, 1), (0, -1), (1, 0), (-1, 0)].iter().any(|d| !g.in_bounds(x + d.0, y + d.1) || g.get(x + d.0, y + d.1) == TRANSPARENT);
        for k in keys {
            // per cell: native pixels of key k and of the cell's current key
            let count = |cx: i32, cy: i32, key: u8| -> i64 {
                let (x0, y0, x1, y1) = self.block(cx, cy);
                let mut n = 0;
                for y in y0..y1 {
                    for x in x0..x1 {
                        if self.cls(x, y) == key {
                            n += 1;
                        }
                    }
                }
                n
            };
            let can_flip = |g: &Grid, cx: i32, cy: i32, ck: i64| -> bool {
                let cur = g.get(cx, cy);
                if cur == TRANSPARENT || cur == k {
                    return false;
                }
                let cc = count(cx, cy, cur);
                if ck * 2 < cc {
                    return false;
                }
                !(Some(cur) == outline && edge(g, cx, cy) && ck * 2 < f2)
            };
            // 1. every contiguous source patch of at least one cell keeps a cell
            let (iw, ih) = (self.iw, self.ih);
            let mut seen = vec![false; (iw * ih) as usize];
            for sy in 0..ih {
                for sx in 0..iw {
                    if self.cls(sx, sy) != k || seen[(sy * iw + sx) as usize] {
                        continue;
                    }
                    let mut comp = vec![(sx, sy)];
                    seen[(sy * iw + sx) as usize] = true;
                    let mut i = 0;
                    while i < comp.len() {
                        let (px, py) = comp[i];
                        i += 1;
                        for dy in -1..=1 {
                            for dx in -1..=1 {
                                let (qx, qy) = (px + dx, py + dy);
                                if qx >= 0 && qy >= 0 && qx < iw && qy < ih && self.cls(qx, qy) == k && !seen[(qy * iw + qx) as usize] {
                                    seen[(qy * iw + qx) as usize] = true;
                                    comp.push((qx, qy));
                                }
                            }
                        }
                    }
                    if (comp.len() as i64) < f2 {
                        continue;
                    }
                    // cells of the patch with the number of its pixels in them
                    let mut cells: Vec<((i32, i32), i64)> = Vec::new();
                    for &(x, y) in &comp {
                        let c = self.cell_of(x, y);
                        match cells.iter_mut().find(|e| e.0 == c) {
                            Some(e) => e.1 += 1,
                            None => cells.push((c, 1)),
                        }
                    }
                    if cells.iter().any(|(c, _)| grid.in_bounds(c.0, c.1) && grid.get(c.0, c.1) == k) {
                        continue;
                    }
                    cells.retain(|(c, _)| grid.in_bounds(c.0, c.1));
                    cells.sort_by_key(|&(c, n)| (std::cmp::Reverse(n), c.1, c.0));
                    for (c, n) in cells {
                        if can_flip(grid, c.0, c.1, n) {
                            grid.set(c.0, c.1, k);
                            break;
                        }
                    }
                }
            }
            // 2. proportional share
            let want = (SHARE_TARGET * pix[k as usize] as f64 / f2 as f64).round() as usize;
            let mut have = grid.cells.iter().filter(|&&c| c == k).count();
            if have >= want {
                continue;
            }
            let mut cand: Vec<(i64, i32, i32)> = Vec::new();
            for cy in 0..gh {
                for cx in 0..gw {
                    let ck = count(cx, cy, k);
                    if ck > 0 && ck * 4 >= f2 && can_flip(grid, cx, cy, ck) {
                        cand.push((ck, cy, cx));
                    }
                }
            }
            cand.sort_by_key(|&(ck, cy, cx)| (std::cmp::Reverse(ck), cy, cx));
            for (ck, cy, cx) in cand {
                if have >= want {
                    break;
                }
                if can_flip(grid, cx, cy, ck) {
                    grid.set(cx, cy, k);
                    have += 1;
                }
            }
        }
    }

    /// Outline pass (factor 2 and up): a source outline one pixel thin is half a cell at factor 2 and loses the vote against the fill
    /// next to it. When the template's outline key draws most of the source silhouette's edge, an opaque cell whose block holds a full
    /// line of that key (`f` pixels) becomes outline, so the silhouette keeps a 1-px outline and thin inner lines (mouth, limb gaps) survive.
    pub fn protect_outline(&self, grid: &mut Grid, tpl: &Template) {
        let f = self.geo.f.0.max(1);
        if f < 2 || self.geo.area.is_some() {
            return;
        }
        let Some(o) = tpl.key_for_role("outline") else { return };
        // does the source outline its silhouette with this key?
        let (mut edge, mut edge_o) = (0i64, 0i64);
        for y in 0..self.ih {
            for x in 0..self.iw {
                let k = self.cls(x, y);
                if k != TRANSPARENT && [(0, 1), (0, -1), (1, 0), (-1, 0)].iter().any(|d| self.cls(x + d.0, y + d.1) == TRANSPARENT) {
                    edge += 1;
                    if k == o {
                        edge_o += 1;
                    }
                }
            }
        }
        if edge == 0 || edge_o * 10 < edge * 6 {
            return;
        }
        let mut flip = Vec::new();
        for cy in 0..self.gh as i32 {
            for cx in 0..self.gw as i32 {
                let cur = grid.get(cx, cy);
                if cur == TRANSPARENT || cur == o {
                    continue;
                }
                let (x0, y0, x1, y1) = self.block(cx, cy);
                let mut n = 0;
                for y in y0..y1 {
                    for x in x0..x1 {
                        if self.cls(x, y) == o {
                            n += 1;
                        }
                    }
                }
                if n >= f {
                    flip.push((cx, cy));
                }
            }
        }
        for (cx, cy) in flip {
            grid.set(cx, cy, o);
        }
    }

    /// Cell of a source pixel.
    pub fn cell_of(&self, x: i32, y: i32) -> (i32, i32) {
        (((x - self.geo.org.0).div_euclid(self.geo.f.0.max(1))), ((y - self.geo.org.1).div_euclid(self.geo.f.1.max(1))))
    }

    /// Feature pass shared by every method but `nearest`: redraw eyes the reduction lost.
    pub fn stamp_features(&self, grid: &mut Grid, tpl: &Template, ex: &[crate::features::ExEye]) {
        if !self.eyes.is_empty() {
            crate::features::stamp(grid, &self.eyes, tpl, self.geo.f.0, ex);
        }
    }
}

/// Reduce one high resolution reference with the part's configured method (DESIGN 5.1). `origin` is the
/// object origin inside `img`. Exemplar-based methods build their bank from the part on every call; batch
/// callers use [`reduce_with`] with a shared [`exemplar::Bank`].
pub fn reduce_frame(img: &RgbaImage, origin: (i32, i32), part: &Part, tpl: &Template) -> Result<Grid> {
    reduce_named(img, origin, part, tpl, None, part.reduce.method, None)
}

/// Full entry: `frame` names the frame being reduced (excluded from the exemplar bank), `bank` is a prebuilt bank.
pub fn reduce_named(img: &RgbaImage, origin: (i32, i32), part: &Part, tpl: &Template, frame: Option<&str>, method: Method, bank: Option<&exemplar::Bank>) -> Result<Grid> {
    let prep = Prep::new(img, origin, part, tpl)?;
    reduce_with(&prep, img, origin, part, tpl, frame, method, bank)
}

pub fn reduce_with(prep: &Prep, img: &RgbaImage, origin: (i32, i32), part: &Part, tpl: &Template, frame: Option<&str>, method: Method, bank: Option<&exemplar::Bank>) -> Result<Grid> {
    let mut g = reduce_method(prep, img, origin, part, tpl, frame, method, bank)?;
    // palette-set parts: the slot of every cell is chosen from the set, not a free palette (see `fit`)
    crate::fit::fit_to_plan(part, tpl, &mut g)?;
    Ok(g)
}

fn reduce_method(prep: &Prep, img: &RgbaImage, origin: (i32, i32), part: &Part, tpl: &Template, frame: Option<&str>, method: Method, bank: Option<&exemplar::Bank>) -> Result<Grid> {
    match method {
        Method::Vote => vote_grid(prep, part, tpl),
        Method::Nearest => nearest::reduce(prep, img, part),
        Method::Contour => contour::reduce(prep, part, tpl),
        Method::Exemplar => {
            let owned;
            let b = match bank {
                Some(b) => b,
                None => {
                    owned = exemplar::Bank::build(part, tpl, frame)?;
                    &owned
                }
            };
            exemplar::reduce(prep, img, origin, part, tpl, frame, b)
        }
        Method::Auto => auto::reduce(prep, img, origin, part, tpl, frame, bank),
        Method::Feature => feature_grid(prep, part, tpl),
    }
}

/// The `vote` method (with the eye stamping pass).
pub fn vote_grid(prep: &Prep, part: &Part, tpl: &Template) -> Result<Grid> {
    let mut g = vote_raw(prep, part, tpl)?;
    prep.protect_salient(&mut g, tpl);
    prep.stamp_features(&mut g, tpl, &[]);
    Ok(g)
}

/// The `feature` method: `vote` fills, then the outline survives the downscale, identity colours and eyes are re-stamped at their scaled positions.
pub fn feature_grid(prep: &Prep, part: &Part, tpl: &Template) -> Result<Grid> {
    let mut g = vote_raw(prep, part, tpl)?;
    prep.protect_outline(&mut g, tpl);
    prep.protect_salient(&mut g, tpl);
    if !prep.eyes.is_empty() {
        crate::features::stamp_forced(&mut g, &prep.eyes, tpl, prep.geo.f.0);
    }
    Ok(g)
}

/// `vote` without the feature pass.
pub fn vote_raw(prep: &Prep, part: &Part, tpl: &Template) -> Result<Grid> {
    let (gw, gh) = (prep.gw, prep.gh);
    let ploc = || Loc::file(part.path.slash());
    let rank = prep.rank;
    let geo = &prep.geo;
    let ring = prep.ring;
    let thin = part.reduce.thin;
    let cls = |x: i32, y: i32| -> u8 { prep.cls(x, y) };
    let n_cells = gw * gh;
    let mut votes: Vec<Option<Votes>> = (0..n_cells).map(|_| None).collect();
    let mut protected = vec![false; n_cells];
    let mut key = vec![TRANSPARENT; n_cells];

    for cy in 0..gh as i32 {
        for cx in 0..gw as i32 {
            let blk = geo.block(cx, cy);
            let (x0, y0, x1, y1) = blk;
            let area = ((x1 - x0) * (y1 - y0)) as i64;
            let mut bcount: Vec<(u8, i64)> = Vec::new();
            let mut n_op = 0i64;
            for y in y0..y1 {
                for x in x0..x1 {
                    let k = cls(x, y);
                    if k != TRANSPARENT {
                        n_op += 1;
                        match bcount.iter_mut().find(|e| e.0 == k) {
                            Some(e) => e.1 += 1,
                            None => bcount.push((k, 1)),
                        }
                    }
                }
            }
            let mut rcount: Vec<(u8, i64)> = Vec::new();
            let (mut r_op, mut r_tot) = (0i64, 0i64);
            for y in y0 - ring..y1 + ring {
                for x in x0 - ring..x1 + ring {
                    if x >= x0 && x < x1 && y >= y0 && y < y1 {
                        continue;
                    }
                    r_tot += 1;
                    let k = cls(x, y);
                    if k != TRANSPARENT {
                        r_op += 1;
                        match rcount.iter_mut().find(|e| e.0 == k) {
                            Some(e) => e.1 += 1,
                            None => rcount.push((k, 1)),
                        }
                    }
                }
            }
            // 1. alpha
            let opaque = if n_op * 2 > area {
                true
            } else if n_op * 2 == area {
                r_op * 100 >= TIE_RING_PCT * r_tot
            } else {
                n_op > 0 && thin && band_through(&cls, &|k| k != TRANSPARENT, blk, ring)
            };
            let ci = cy as usize * gw + cx as usize;
            if !opaque {
                continue;
            }
            // 2. colour votes
            let mut v = Votes { v: Vec::new() };
            for &(k, c) in &bcount {
                v.add(k, c * W_BLOCK, &rank);
            }
            for &(k, c) in &rcount {
                v.add(k, c * W_RING, &rank);
            }
            let mut bonus_keys: Vec<u8> = Vec::new();
            let iso_min = ((area + 1) / 2).max(2).min(area);
            for &(k, c) in &bcount {
                let in_ring = rcount.iter().any(|e| e.0 == k);
                let isolated = !in_ring && c >= iso_min && c < n_op;
                // detail keys (overlay layer) covering at least half of the block win against the base colour
                let detail = c * 2 >= area && tpl.layer_of(k).map_or(false, |l| l > 0);
                if isolated || detail {
                    v.add(k, area * W_BLOCK, &rank);
                    bonus_keys.push(k);
                }
            }
            if thin {
                for vertical in [false, true] {
                    if let Some(k) = band_key(&cls, blk, ring + 1, vertical, &rank) {
                        v.add(k, area * W_BLOCK, &rank);
                        bonus_keys.push(k);
                    }
                }
            }
            let best = v.best().unwrap_or(TRANSPARENT);
            key[ci] = best;
            let unanimous = n_op == area && bcount.len() == 1;
            protected[ci] = unanimous || bonus_keys.contains(&best);
            votes[ci] = Some(v);
        }
    }

    // 3. smoothing (ICM): protected cells never flip
    let at = |key: &Vec<u8>, x: i32, y: i32| -> u8 {
        if x < 0 || y < 0 || x >= gw as i32 || y >= gh as i32 {
            TRANSPARENT
        } else {
            key[y as usize * gw + x as usize]
        }
    };
    for _ in 0..ICM_PASSES {
        let mut changed = false;
        for cy in 0..gh as i32 {
            for cx in 0..gw as i32 {
                let ci = cy as usize * gw + cx as usize;
                if protected[ci] || key[ci] == TRANSPARENT {
                    continue;
                }
                let v = match &votes[ci] {
                    Some(v) => v,
                    None => continue,
                };
                let nb = [at(&key, cx - 1, cy), at(&key, cx + 1, cy), at(&key, cx, cy - 1), at(&key, cx, cy + 1)];
                let mut best: Option<(u8, i64)> = None;
                for &(k, w) in &v.v {
                    let mism = nb.iter().filter(|&&n| n != TRANSPARENT && n != k).count() as i64;
                    let e = -w + LAMBDA * mism;
                    if best.map_or(true, |b| e < b.1) {
                        best = Some((k, e));
                    }
                }
                if let Some((k, _)) = best {
                    if k != key[ci] {
                        key[ci] = k;
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }

    // 4. outline for the given roles
    if !part.reduce.outline.is_empty() {
        let mut roles: Vec<u8> = Vec::new();
        for r in &part.reduce.outline {
            match tpl.resolve(r) {
                Some(k) if k != TRANSPARENT => roles.push(k),
                _ => {
                    return Err(PxlError::validation(format!("reduce.outline: '{}' is not a role or key of template '{}'", r, tpl.name))
                        .with_loc(ploc())
                        .with_hint("list template roles such as outline, body, skin"))
                }
            }
        }
        let ok = tpl
            .key_for_role("outline")
            .or_else(|| tpl.layers.first().and_then(|l| l.keys.first()).map(|k| k.key))
            .unwrap_or(b'1');
        let mut todo = Vec::new();
        for cy in 0..gh as i32 {
            for cx in 0..gw as i32 {
                let k = at(&key, cx, cy);
                if k != ok && roles.contains(&k) {
                    let edge = [(-1, 0), (1, 0), (0, -1), (0, 1)].iter().any(|d| at(&key, cx + d.0, cy + d.1) == TRANSPARENT);
                    if edge {
                        todo.push(cy as usize * gw + cx as usize);
                    }
                }
            }
        }
        for ci in todo {
            key[ci] = ok;
            protected[ci] = true;
        }
    }

    // orphan cleanup: a cell with no 4-neighbour of its own key joins its most common neighbour
    for _ in 0..2 {
        let mut changed = false;
        for cy in 0..gh as i32 {
            for cx in 0..gw as i32 {
                let ci = cy as usize * gw + cx as usize;
                let k = key[ci];
                if k == TRANSPARENT || protected[ci] {
                    continue;
                }
                let nb = [at(&key, cx - 1, cy), at(&key, cx + 1, cy), at(&key, cx, cy - 1), at(&key, cx, cy + 1)];
                if nb.iter().any(|&n| n == k) {
                    continue;
                }
                let mut tally: Vec<(u8, usize)> = Vec::new();
                for &n in &nb {
                    if n != TRANSPARENT {
                        match tally.iter_mut().find(|e| e.0 == n) {
                            Some(e) => e.1 += 1,
                            None => tally.push((n, 1)),
                        }
                    }
                }
                let mut best: Option<(u8, usize)> = None;
                for &(n, c) in &tally {
                    if best.map_or(true, |b| c > b.1 || (c == b.1 && rank[n as usize] < rank[b.0 as usize])) {
                        best = Some((n, c));
                    }
                }
                if let Some((n, _)) = best {
                    key[ci] = n;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }

    let mut g = Grid::new(gw, gh, TRANSPARENT);
    g.cells = key;
    Ok(g)
}

/// Native-resolution silhouette check (stricter than AC-LD4; the approved golden frame scores 0.86 here): the
/// grid is blown up to source resolution along the part's phase and compared pixel by pixel with the
/// reference alpha. Returns `(iou, max bbox delta per side in target pixels)`.
pub fn silhouette_native(grid: &Grid, rf: &RefFrame, part: &Part) -> (f64, f64) {
    let f = part.reduce.factor.or(part.input.factor).unwrap_or(1).max(1) as i32;
    let ph = part.reduce.phase.unwrap_or((0, 0));
    let org = (rf.origin.0 - f * part.origin.0 + ph.0, rf.origin.1 - f * part.origin.1 + ph.1);
    let (mut inter, mut uni) = (0usize, 0usize);
    let mut gb = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    let mut rb = gb;
    let (x_lo, y_lo) = (org.0.min(0), org.1.min(0));
    let x_hi = (org.0 + f * grid.w as i32).max(rf.img.w as i32);
    let y_hi = (org.1 + f * grid.h as i32).max(rf.img.h as i32);
    for y in y_lo..y_hi {
        for x in x_lo..x_hi {
            let r = rf.img.get(x, y).a >= 128;
            let (cx, cy) = ((x - org.0).div_euclid(f), (y - org.1).div_euclid(f));
            let g = grid.in_bounds(cx, cy) && grid.get(cx, cy) != TRANSPARENT;
            if g && r {
                inter += 1;
            }
            if g || r {
                uni += 1;
            }
            if g {
                gb = (gb.0.min(x), gb.1.min(y), gb.2.max(x + 1), gb.3.max(y + 1));
            }
            if r {
                rb = (rb.0.min(x), rb.1.min(y), rb.2.max(x + 1), rb.3.max(y + 1));
            }
        }
    }
    let iou = if uni == 0 { 1.0 } else { inter as f64 / uni as f64 };
    let delta = if gb.0 == i32::MAX || rb.0 == i32::MAX {
        if gb.0 == rb.0 { 0.0 } else { 99.0 }
    } else {
        [gb.0 - rb.0, gb.1 - rb.1, gb.2 - rb.2, gb.3 - rb.3].iter().map(|d| d.abs()).max().unwrap_or(0) as f64 / f as f64
    };
    (iou, delta)
}

/// Reference silhouette at target scale, per cell: block coverage in percent of opaque source pixels (same
/// block geometry as `reduce_frame`).
pub fn ref_coverage(rf: &RefFrame, part: &Part) -> Vec<u8> {
    let f = part.reduce.factor.or(part.input.factor).unwrap_or(1).max(1) as i32;
    let ph = part.reduce.phase.unwrap_or((0, 0));
    let org = (rf.origin.0 - f * part.origin.0 + ph.0, rf.origin.1 - f * part.origin.1 + ph.1);
    let (gw, gh) = part.size;
    let mut out = vec![0u8; gw * gh];
    for cy in 0..gh as i32 {
        for cx in 0..gw as i32 {
            let mut n = 0;
            for j in 0..f {
                for i in 0..f {
                    if rf.img.get(org.0 + f * cx + i, org.1 + f * cy + j).a >= 128 {
                        n += 1;
                    }
                }
            }
            out[cy as usize * gw + cx as usize] = (n * 100 / (f * f)) as u8;
        }
    }
    out
}

/// AC-LD4 silhouette check at target scale: IoU of the grid's opaque cells against the reference silhouette
/// (cells with at least half of their block opaque), and the largest bbox delta per side in cells.
pub fn silhouette_vs_ref(grid: &Grid, rf: &RefFrame, part: &Part) -> (f64, f64) {
    let cov = ref_coverage(rf, part);
    let (mut inter, mut uni) = (0usize, 0usize);
    let mut gb = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    let mut rb = gb;
    for y in 0..grid.h {
        for x in 0..grid.w {
            let g = grid.cells[y * grid.w + x] != TRANSPARENT;
            let r = cov[y * grid.w + x] >= 50;
            if g && r {
                inter += 1;
            }
            if g || r {
                uni += 1;
            }
            let (xi, yi) = (x as i32, y as i32);
            if g {
                gb = (gb.0.min(xi), gb.1.min(yi), gb.2.max(xi), gb.3.max(yi));
            }
            if r {
                rb = (rb.0.min(xi), rb.1.min(yi), rb.2.max(xi), rb.3.max(yi));
            }
        }
    }
    let iou = if uni == 0 { 1.0 } else { inter as f64 / uni as f64 };
    let delta = if gb.0 == i32::MAX || rb.0 == i32::MAX {
        if gb.0 == rb.0 { 0.0 } else { 99.0 }
    } else {
        [gb.0 - rb.0, gb.1 - rb.1, gb.2 - rb.2, gb.3 - rb.3].iter().map(|d| d.abs()).max().unwrap_or(0) as f64
    };
    (iou, delta)
}
