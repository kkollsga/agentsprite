//! Automatic first version (M6 R2b/R2c): `image` and `ingest` derive a template that is hardware-valid and keeps the design.
//!
//! * `auto_template = "flat"` (gb, sms, gg, md): one layer of at most `colors` colours, every colour through the console colour
//!   model. A sprite is one palette line on these consoles; overlays are never added automatically.
//! * `auto_template = "set"` (gbc, nes): a flat template of up to 24 colours is reduced first; the colours of every hardware cell are
//!   counted and **learned into palette slots** (an asset-local set `<asset>/palettes/<name>.toml`, one slot per OBJ palette):
//!   1. *Materials*: colours group into hue families (red shell, blue face, ...) and one neutral family (greys, black, white); each
//!      becomes a dark / mid / light ramp, the seed of a slot. When slots are short the cheapest merge (Ward cost on Oklab with the
//!      weight raised for saturated colours) goes first, so a distinct hue is never folded into a grey while a slot is free.
//!   2. *Learning*: every cell goes to the slot that holds its colours best, every slot takes the three colours that minimise the
//!      weighted error of its cells (Oklab, chroma counts three times as much as lightness, so a hue family is kept). Slots still free
//!      are given to the worst-fitting cell while that lowers the total error.
//!   3. *Eyes*: a near-white pixel with a dark pixel within two pixels is an eye; the white and the dark get a large weight in the cell
//!      of the white, so eye white and pupil end up in the same slot.
//!   The ordinary reduce then picks the slot of every cell (the M5 model), so `cell_palette` cannot fail.
//! * A single image (or a project without `[project.defaults] category`) is limited by the hardware only, not by a category.

use crate::color::{self, Rgb};
use crate::model::Part;
use std::collections::BTreeMap;

/// Distance of two colours: Oklab with the chroma axes counted three times, so lightness may move but a hue family stays.
pub fn metric(a: Rgb, b: Rgb) -> f64 {
    let (x, y) = (color::srgb_to_oklab(a), color::srgb_to_oklab(b));
    (x.l - y.l).powi(2) + 3.0 * ((x.a - y.a).powi(2) + (x.b - y.b).powi(2))
}

fn chroma(c: Rgb) -> f64 {
    let o = color::srgb_to_oklab(c);
    (o.a * o.a + o.b * o.b).sqrt()
}

fn lightness(c: Rgb) -> f64 {
    color::srgb_to_oklab(c).l
}

/// Importance of one colour in a cell: area, raised for saturated colours and for near-white ones.
pub fn importance(c: Rgb, area: f64) -> f64 {
    let sat = (chroma(c) / 0.25).min(1.0);
    let white = if lightness(c) > 0.9 && chroma(c) < 0.06 { 6.0 } else { 0.0 };
    area * (1.0 + 4.0 * sat) * (1.0 + white)
}

/// The colours of the cells of a part: the palette (every colour of the frames) and per cell (palette index, weight).
pub struct CellData {
    pub palette: Vec<Rgb>,
    pub cells: Vec<Vec<(usize, f64)>>,
    /// Frame index and canvas-aligned cell (column, row) of every entry of `cells`.
    pub pos: Vec<(usize, usize, usize)>,
    /// Eyes found (white with a dark pixel next to it), for reporting.
    pub eyes: usize,
}

/// The cells of several parts as one data set (a shared palette plan): the palettes are united, every cell keeps its weights.
pub fn merge_data(all: &[&CellData]) -> CellData {
    let mut palette: Vec<Rgb> = Vec::new();
    let (mut cells, mut pos, mut eyes) = (Vec::new(), Vec::new(), 0);
    for d in all {
        let map: Vec<usize> = d
            .palette
            .iter()
            .map(|c| {
                palette.iter().position(|x| x == c).unwrap_or_else(|| {
                    palette.push(*c);
                    palette.len() - 1
                })
            })
            .collect();
        for (n, c) in d.cells.iter().enumerate() {
            cells.push(c.iter().map(|&(i, w)| (map[i], w)).collect());
            pos.push(d.pos[n]);
        }
        eyes += d.eyes;
    }
    CellData { palette, cells, pos, eyes }
}

const EYE_BOOST: f64 = 25.0;

/// The pixels of one frame as colour indexes at some resolution: the reduced art (`f` = 1, `org` = 0) or the source image (`f` source pixels per
/// output pixel, `org` = the source position of output pixel (0, 0)).
struct Layer {
    w: usize,
    h: usize,
    px: Vec<Option<usize>>,
    f: usize,
    org: (i32, i32),
}

pub fn cell_data(part: &Part, key_rgb: &dyn Fn(u8) -> Option<Rgb>, cell: (u32, u32)) -> CellData {
    cell_data_source(part, key_rgb, cell, &|_| None, &|c| c)
}

/// A source frame for [`cell_data_source`]: the image, the source pixels per output pixel and the source position of output pixel (0, 0).
pub struct SourceFrame {
    pub img: crate::img::RgbaImage,
    pub f: i32,
    pub org: (i32, i32),
}

/// The colours of the hardware cells measured on the SOURCE where a frame has one (`src(frame)`; every source pixel counts 1/f^2, colours go through
/// `snap`, the console's colour model), else on the reduced art. This is what a palette plan must be judged against: the reduced art has already
/// lost the colours the plan is meant to keep.
pub fn cell_data_source(part: &Part, key_rgb: &dyn Fn(u8) -> Option<Rgb>, cell: (u32, u32), src: &dyn Fn(usize) -> Option<SourceFrame>, snap: &dyn Fn(Rgb) -> Rgb) -> CellData {
    let (cw, ch) = (cell.0 as usize, cell.1 as usize);
    let mut palette: Vec<Rgb> = Vec::new();
    let idx = |c: Rgb, palette: &mut Vec<Rgb>| -> usize {
        palette.iter().position(|x| *x == c).unwrap_or_else(|| {
            palette.push(c);
            palette.len() - 1
        })
    };
    let mut cells: Vec<Vec<(usize, f64)>> = Vec::new();
    let mut pos: Vec<(usize, usize, usize)> = Vec::new();
    let mut eyes = 0;
    for (fi, fr) in part.frames.iter().enumerate() {
        let g = &fr.grid;
        let layer = match src(fi) {
            Some(sf) => {
                let mut px: Vec<Option<usize>> = vec![None; sf.img.w * sf.img.h];
                for (i, p) in sf.img.px.iter().enumerate() {
                    if p.a >= 128 {
                        px[i] = Some(idx(snap(Rgb { r: p.r, g: p.g, b: p.b }), &mut palette));
                    }
                }
                Layer { w: sf.img.w, h: sf.img.h, px, f: sf.f.max(1) as usize, org: sf.org }
            }
            None => {
                let mut px: Vec<Option<usize>> = vec![None; g.w * g.h];
                for (i, &k) in g.cells.iter().enumerate() {
                    if k != b'.' {
                        if let Some(c) = key_rgb(k) {
                            px[i] = Some(idx(c, &mut palette));
                        }
                    }
                }
                Layer { w: g.w, h: g.h, px, f: 1, org: (0, 0) }
            }
        };
        let white: Vec<bool> = palette.iter().map(|c| lightness(*c) > 0.85 && chroma(*c) < 0.06).collect();
        let dark: Vec<bool> = palette.iter().map(|c| lightness(*c) < 0.4).collect();
        let ncell_x = (g.w + cw - 1) / cw;
        let ncell_y = (g.h + ch - 1) / ch;
        let f = layer.f as i32;
        let wgt = 1.0 / (layer.f * layer.f) as f64;
        // the output cell of a layer pixel (None outside the canvas)
        let cell_of = |x: usize, y: usize| -> Option<usize> {
            let (ox, oy) = ((x as i32 - layer.org.0).div_euclid(f), (y as i32 - layer.org.1).div_euclid(f));
            (ox >= 0 && oy >= 0 && (ox as usize) < g.w && (oy as usize) < g.h).then(|| (oy as usize / ch) * ncell_x + ox as usize / cw)
        };
        let mut data: Vec<BTreeMap<usize, f64>> = vec![BTreeMap::new(); ncell_x * ncell_y];
        for y in 0..layer.h {
            for x in 0..layer.w {
                if let (Some(c), Some(ci)) = (layer.px[y * layer.w + x], cell_of(x, y)) {
                    *data[ci].entry(c).or_insert(0.0) += importance(palette[c], wgt);
                }
            }
        }
        // eyes: a white area of at least two output pixels (a one-pixel glint is not an eye) with a dark pixel within two output pixels keeps
        // both in the cell of the white
        let mut seen = vec![false; layer.w * layer.h];
        let reach = 2 * f;
        for y0 in 0..layer.h {
            for x0 in 0..layer.w {
                let i0 = y0 * layer.w + x0;
                if seen[i0] || !layer.px[i0].map_or(false, |c| white[c]) {
                    continue;
                }
                let mut comp = vec![(x0, y0)];
                seen[i0] = true;
                let mut k = 0;
                while k < comp.len() {
                    let (cx, cy) = comp[k];
                    k += 1;
                    for dy in -1i32..=1 {
                        for dx in -1i32..=1 {
                            let (nx, ny) = (cx as i32 + dx, cy as i32 + dy);
                            if nx >= 0 && ny >= 0 && (nx as usize) < layer.w && (ny as usize) < layer.h {
                                let j = ny as usize * layer.w + nx as usize;
                                if !seen[j] && layer.px[j].map_or(false, |c| white[c]) {
                                    seen[j] = true;
                                    comp.push((nx as usize, ny as usize));
                                }
                            }
                        }
                    }
                }
                if comp.len() < 2 * layer.f * layer.f {
                    continue;
                }
                for &(x, y) in &comp {
                    let Some(c) = layer.px[y * layer.w + x] else { continue };
                    let mut pupil: Option<usize> = None;
                    'o: for dy in -reach..=reach {
                        for dx in -reach..=reach {
                            let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                            if nx >= 0 && ny >= 0 && (nx as usize) < layer.w && (ny as usize) < layer.h {
                                if let Some(d) = layer.px[ny as usize * layer.w + nx as usize] {
                                    if dark[d] {
                                        pupil = Some(d);
                                        break 'o;
                                    }
                                }
                            }
                        }
                    }
                    if let (Some(d), Some(ci)) = (pupil, cell_of(x, y)) {
                        *data[ci].entry(c).or_insert(0.0) += EYE_BOOST * wgt;
                        *data[ci].entry(d).or_insert(0.0) += EYE_BOOST * wgt;
                        eyes += 1;
                    }
                }
            }
        }
        for (ci, m) in data.into_iter().enumerate() {
            if !m.is_empty() {
                cells.push(m.into_iter().collect());
                pos.push((fi, ci % ncell_x, ci / ncell_x));
            }
        }
    }
    CellData { palette, cells, pos, eyes }
}

struct Ctx<'a> {
    d: Vec<Vec<f64>>,
    cells: &'a [Vec<(usize, f64)>],
}

impl Ctx<'_> {
    fn cell_cost(&self, cell: usize, slot: &[usize; 3]) -> f64 {
        self.cells[cell].iter().map(|&(c, w)| w * slot.iter().map(|&t| self.d[c][t]).fold(f64::MAX, f64::min)).sum()
    }
    fn assign(&self, slots: &[[usize; 3]]) -> (Vec<usize>, f64) {
        let mut owner = Vec::with_capacity(self.cells.len());
        let mut total = 0.0;
        for i in 0..self.cells.len() {
            let (mut best, mut bc) = (0, f64::MAX);
            for (s, t) in slots.iter().enumerate() {
                let c = self.cell_cost(i, t);
                if c < bc - 1e-12 {
                    best = s;
                    bc = c;
                }
            }
            owner.push(best);
            total += bc;
        }
        (owner, total)
    }
    /// The three colours that hold the colours of `cells` best.
    fn best_triple(&self, cells: &[usize], n: usize) -> [usize; 3] {
        let mut weight = vec![0.0f64; n];
        for &ci in cells {
            for &(c, w) in &self.cells[ci] {
                weight[c] += w;
            }
        }
        let mut cand: Vec<usize> = (0..n).filter(|c| weight[*c] > 0.0).collect();
        cand.sort_by(|a, b| weight[*b].partial_cmp(&weight[*a]).unwrap_or(std::cmp::Ordering::Equal));
        cand.truncate(14);
        if cand.len() <= 3 {
            let mut v = cand.clone();
            for c in 0..n {
                if v.len() >= 3 {
                    break;
                }
                if !v.contains(&c) {
                    v.push(c);
                }
            }
            while v.len() < 3 {
                v.push(v.first().copied().unwrap_or(0));
            }
            return [v[0], v[1], v[2]];
        }
        let mut best = ([cand[0], cand[1], cand[2]], f64::MAX);
        for a in 0..cand.len() {
            for b in a + 1..cand.len() {
                for c in b + 1..cand.len() {
                    let t = [cand[a], cand[b], cand[c]];
                    let cost: f64 = cells.iter().map(|&ci| self.cell_cost(ci, &t)).sum();
                    if cost < best.1 - 1e-12 {
                        best = (t, cost);
                    }
                }
            }
        }
        best.0
    }
}

/// Hue families of the palette: chromatic colours grouped by hue (a gap of more than ~28 degrees starts a new family), one family
/// for the neutrals (greys, black, white).
fn materials(palette: &[Rgb]) -> Vec<Vec<usize>> {
    let lab: Vec<color::Oklab> = palette.iter().map(|c| color::srgb_to_oklab(*c)).collect();
    let mut neutral = Vec::new();
    let mut chromatic: Vec<(f64, usize)> = Vec::new();
    for (i, o) in lab.iter().enumerate() {
        if (o.a * o.a + o.b * o.b).sqrt() < 0.035 {
            neutral.push(i);
        } else {
            chromatic.push((o.b.atan2(o.a), i));
        }
    }
    chromatic.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let gap = 0.5;
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (k, &(h, i)) in chromatic.iter().enumerate() {
        if k == 0 || h - chromatic[k - 1].0 > gap {
            groups.push(vec![i]);
        } else {
            groups.last_mut().unwrap().push(i);
        }
    }
    // the hue circle wraps
    if groups.len() > 1 {
        let (first, last) = (chromatic[0].0, chromatic[chromatic.len() - 1].0);
        if first + std::f64::consts::TAU - last <= gap {
            let l = groups.pop().unwrap();
            groups[0].extend(l);
        }
    }
    if !neutral.is_empty() {
        groups.push(neutral);
    }
    groups
}

/// Learn at most `slots` palettes of three colours from the cells. `free(base, used)` supplies a console colour next to `base` when a
/// slot needs padding.
pub fn learn_slots(data: &CellData, slots: usize, free: &dyn Fn(Rgb, &[Rgb]) -> Option<Rgb>) -> Vec<[Rgb; 3]> {
    let n = data.palette.len();
    if n == 0 || data.cells.is_empty() {
        return Vec::new();
    }
    let d: Vec<Vec<f64>> = (0..n).map(|i| (0..n).map(|j| metric(data.palette[i], data.palette[j])).collect()).collect();
    let ctx = Ctx { d, cells: &data.cells };
    let mut total_w = vec![0.0f64; n];
    for c in &data.cells {
        for &(i, w) in c {
            total_w[i] += w;
        }
    }
    // seeds: one ramp per material, the cheapest merges first when there are too many
    let mut groups = materials(&data.palette);
    groups.retain(|g| g.iter().any(|i| total_w[*i] > 0.0));
    let gw = |g: &Vec<usize>| g.iter().map(|i| total_w[*i]).sum::<f64>();
    let centre = |g: &Vec<usize>| -> color::Oklab {
        let w = gw(g).max(1e-12);
        g.iter().fold(color::Oklab::default(), |a, &i| {
            let o = color::srgb_to_oklab(data.palette[i]);
            let k = total_w[i] / w;
            color::Oklab { l: a.l + o.l * k, a: a.a + o.a * k, b: a.b + o.b * k }
        })
    };
    while groups.len() > slots.max(1) {
        let mut best = (f64::MAX, 0, 1);
        for a in 0..groups.len() {
            for b in a + 1..groups.len() {
                let (ca, cb) = (centre(&groups[a]), centre(&groups[b]));
                let dd = (ca.l - cb.l).powi(2) + 3.0 * ((ca.a - cb.a).powi(2) + (ca.b - cb.b).powi(2));
                let (wa, wb) = (gw(&groups[a]), gw(&groups[b]));
                let cost = dd * wa * wb / (wa + wb).max(1e-12);
                if cost < best.0 {
                    best = (cost, a, b);
                }
            }
        }
        let g = groups.remove(best.2);
        groups[best.1].extend(g);
    }
    let seed = |g: &Vec<usize>| -> [usize; 3] {
        let mut v = g.clone();
        v.sort_by(|a, b| lightness(data.palette[*a]).partial_cmp(&lightness(data.palette[*b])).unwrap_or(std::cmp::Ordering::Equal));
        if v.len() >= 3 {
            // darkest, the heaviest of the rest, lightest
            let mid = v[1..v.len() - 1].iter().copied().max_by(|a, b| total_w[*a].partial_cmp(&total_w[*b]).unwrap_or(std::cmp::Ordering::Equal)).unwrap();
            [v[0], mid, v[v.len() - 1]]
        } else {
            let mut t = v.clone();
            for c in 0..n {
                if t.len() >= 3 {
                    break;
                }
                if !t.contains(&c) {
                    t.push(c);
                }
            }
            while t.len() < 3 {
                t.push(t.first().copied().unwrap_or(0));
            }
            [t[0], t[1], t[2]]
        }
    };
    let mut slot_sets: Vec<[usize; 3]> = groups.iter().map(|g| seed(g)).collect();
    let refine = |slot_sets: &mut Vec<[usize; 3]>, rounds: usize| -> f64 {
        let mut total = f64::MAX;
        for _ in 0..rounds {
            let (owner, t) = ctx.assign(slot_sets);
            total = t;
            let mut changed = false;
            for s in 0..slot_sets.len() {
                let cells: Vec<usize> = (0..owner.len()).filter(|i| owner[*i] == s).collect();
                if cells.is_empty() {
                    continue;
                }
                let nt = ctx.best_triple(&cells, n);
                let cost = |t: &[usize; 3]| cells.iter().map(|&c| ctx.cell_cost(c, t)).sum::<f64>();
                if cost(&nt) < cost(&slot_sets[s]) - 1e-9 {
                    slot_sets[s] = nt;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        ctx.assign(slot_sets).1.min(total)
    };
    let prune = |slot_sets: &mut Vec<[usize; 3]>| {
        let (owner, _) = ctx.assign(slot_sets);
        let mut s = 0;
        slot_sets.retain(|_| {
            s += 1;
            owner.iter().any(|o| *o == s - 1)
        });
    };
    let mut cost = refine(&mut slot_sets, 8);
    prune(&mut slot_sets);
    cost = ctx.assign(&slot_sets).1.min(cost.max(0.0)).min(ctx.assign(&slot_sets).1);
    // free slots go to the worst-fitting cells while that lowers the total error
    while slot_sets.len() < slots {
        let (owner, _) = ctx.assign(&slot_sets);
        // the cell that gains most from a palette of its own
        let mut worst: Option<(f64, usize, [usize; 3])> = None;
        for i in 0..owner.len() {
            let cur = ctx.cell_cost(i, &slot_sets[owner[i]]);
            if cur < 1e-9 {
                continue;
            }
            let t = ctx.best_triple(&[i], n);
            let gain = cur - ctx.cell_cost(i, &t);
            if gain > 1e-9 && worst.as_ref().map_or(true, |w| gain > w.0) {
                worst = Some((gain, i, t));
            }
        }
        let Some((_, _, own)) = worst else { break };
        let mut trial = slot_sets.clone();
        trial.push(own);
        let mut c = refine(&mut trial, 6);
        prune(&mut trial);
        c = ctx.assign(&trial).1.min(c);
        if c < cost - 1e-9 {
            slot_sets = trial;
            cost = c;
        } else {
            break;
        }
    }
    // drop slots no cell uses, heaviest first
    let (owner, _) = ctx.assign(&slot_sets);
    let mut used: Vec<(f64, [usize; 3])> = Vec::new();
    for (s, t) in slot_sets.iter().enumerate() {
        let w: f64 = (0..owner.len()).filter(|i| owner[*i] == s).map(|i| ctx.cells[i].iter().map(|x| x.1).sum::<f64>()).sum();
        if w > 0.0 {
            used.push((w, *t));
        }
    }
    used.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    used.into_iter()
        .map(|(_, t)| {
            let mut cols: Vec<Rgb> = Vec::new();
            for i in t {
                let c = data.palette[i];
                if !cols.contains(&c) {
                    cols.push(c);
                }
            }
            while cols.len() < 3 {
                let base = cols[0];
                match free(base, &cols) {
                    Some(c) => cols.push(c),
                    None => cols.push(Rgb { r: base.r ^ 8, g: base.g, b: base.b }),
                }
            }
            [cols[0], cols[1], cols[2]]
        })
        .collect()
}

/// The text of the asset-local palette set file.
pub fn set_text(console: &str, slots: &[[Rgb; 3]]) -> String {
    let mut s = format!("# generated by `pxl image` / `pxl ingest`: one slot per OBJ palette, learned from the colours of every cell\nconsole = \"{}\"\n", console);
    for (i, c) in slots.iter().enumerate() {
        s.push_str(&format!("\n[[slots]]\nslot = {}\nname = \"auto_{}\"\ncolors = [{}]\n", i, i, c.iter().map(|x| format!("\"{}\"", color::to_hex(*x))).collect::<Vec<_>>().join(", ")));
    }
    s
}

/// Fold every canvas-aligned cell of the frames into the slot that holds its colours best (same weights as the learning), so every
/// cell uses one slot and identity colours keep their palette. `key_of` gives the template key of a colour. Returns pixels changed.
pub fn fold_cells(part: &mut Part, data: &CellData, slots: &[[Rgb; 3]], cell: (u32, u32), key_of: &dyn Fn(Rgb) -> Option<u8>, rgb_of: &dyn Fn(u8) -> Option<Rgb>) -> usize {
    let (cw, ch) = (cell.0 as usize, cell.1 as usize);
    let mut changed = 0;
    for (n, &(fi, cx, cy)) in data.pos.iter().enumerate() {
        let cost = |t: &[Rgb; 3]| -> f64 { data.cells[n].iter().map(|&(c, w)| w * t.iter().map(|x| metric(data.palette[c], *x)).fold(f64::MAX, f64::min)).sum() };
        let Some(best) = slots.iter().min_by(|a, b| cost(a).partial_cmp(&cost(b)).unwrap_or(std::cmp::Ordering::Equal)) else { continue };
        let g = &mut part.frames[fi].grid;
        for y in cy * ch..((cy + 1) * ch).min(g.h) {
            for x in cx * cw..((cx + 1) * cw).min(g.w) {
                let k = g.cells[y * g.w + x];
                if k == b'.' {
                    continue;
                }
                let Some(c) = rgb_of(k) else { continue };
                // a pixel that is not white does not become white (light skin turned into a second eye highlight)
                let is_white = |c: Rgb| lightness(c) > 0.85 && chroma(c) < 0.06;
                let pen = |t: Rgb| if is_white(t) && !is_white(c) { 0.06 } else { 0.0 };
                let t = *best.iter().min_by(|a, b| (metric(c, **a) + pen(**a)).partial_cmp(&(metric(c, **b) + pen(**b))).unwrap_or(std::cmp::Ordering::Equal)).unwrap();
                if t != c {
                    if let Some(nk) = key_of(t) {
                        g.cells[y * g.w + x] = nk;
                        changed += 1;
                    }
                }
            }
        }
    }
    changed
}

// ------------------------------------------------------------------------------------------------ luminance stretch (value_map = "stretch")

/// The shades of a four-grey console sorted dark to light.
fn shades(prof: &crate::profile::Profile) -> Vec<Rgb> {
    let mut v = prof.fixed_colours.clone();
    v.sort_by(|a, b| lightness(*a).partial_cmp(&lightness(*b)).unwrap_or(std::cmp::Ordering::Equal));
    v
}

/// The level (0 = darkest shade .. 3 = lightest) of every source colour: Oklab lightness, stretched so that the darkest and the lightest
/// significant material (more than 1 per cent of the pixels) reach the ends, then cut at 0.2 / 0.5 / 0.8.
pub fn stretch_levels(cols: &[(Rgb, u64)]) -> Vec<(Rgb, usize)> {
    let total: u64 = cols.iter().map(|c| c.1).sum::<u64>().max(1);
    let sig: Vec<&(Rgb, u64)> = cols.iter().filter(|c| c.1 as f64 / total as f64 >= 0.01).collect();
    let pool: Vec<&(Rgb, u64)> = if sig.is_empty() { cols.iter().collect() } else { sig };
    let l = |c: Rgb| lightness(c);
    let lo = pool.iter().map(|c| l(c.0)).fold(f64::MAX, f64::min);
    // a sprite whose darkest material is light (a yellow coin) must not turn black: the bottom of the range rises only half as fast
    let lo = if lo < 0.35 { lo } else { 0.35 + (lo - 0.35) * 0.5 };
    let hi = pool.iter().map(|c| l(c.0)).fold(f64::MIN, f64::max);
    cols.iter()
        .map(|&(c, _)| {
            let t = if hi - lo < 0.05 { l(c) } else { ((l(c) - lo) / (hi - lo)).clamp(0.0, 1.0) };
            let lv = if t < 0.2 {
                0
            } else if t < 0.5 {
                1
            } else if t < 0.8 {
                2
            } else {
                3
            };
            (c, lv)
        })
        .collect()
}

/// The flat template of a four-grey console: the four shades as keys, every source colour mapped by `stretch_levels`.
pub fn stretch_template(name: &str, cols: &[(Rgb, u64)], prof: &crate::profile::Profile) -> String {
    let sh = shades(prof);
    let roles = ["outline", "dark", "light", "white"];
    let mut s = format!("# derived by `pxl image`: luminance stretch per sprite (console style value_map = stretch): darkest material black, lightest white\nname = \"{}\"\n\n[layers.base]\n\".\" = {{ role = \"transparent\" }}\n", name);
    for (i, c) in sh.iter().enumerate() {
        s.push_str(&format!("\"{}\" = {{ role = \"{}\", color = \"{}\" }}\n", i + 1, roles[i.min(3)], color::to_hex(*c)));
    }
    s.push_str("\n[map]\n");
    let mut lv = stretch_levels(cols);
    lv.sort_by_key(|x| x.0);
    for (c, l) in lv {
        s.push_str(&format!("\"{}\" = \"{}\"\n", color::to_hex(c), l + 1));
    }
    s
}

/// OBP0 = darkest / dark / white and OBP1 = darkest / light / white of a four-grey console.
pub fn stretch_slots(prof: &crate::profile::Profile) -> Vec<[Rgb; 3]> {
    let sh = shades(prof);
    vec![[sh[0], sh[1], sh[3]], [sh[0], sh[2], sh[3]]]
}

fn level_of(sh: &[Rgb], c: Rgb) -> usize {
    (0..sh.len()).min_by(|a, b| metric(c, sh[*a]).partial_cmp(&metric(c, sh[*b])).unwrap_or(std::cmp::Ordering::Equal)).unwrap_or(0)
}

/// The cell fold of the luminance stretch: every canvas cell takes the slot (OBP0 / OBP1) and a monotone map of its grey levels onto the
/// slot's three shades that keeps the materials apart: two levels share a shade only when the cell has more levels than the slot has
/// shades, and then the lightest-weighted pair merges. Returns the pixels changed.
pub fn fold_cells_levels(part: &mut Part, data: &CellData, slots: &[[Rgb; 3]], shades: &[Rgb], cell: (u32, u32), key_of: &dyn Fn(Rgb) -> Option<u8>, rgb_of: &dyn Fn(u8) -> Option<Rgb>) -> usize {
    let (cw, ch) = (cell.0 as usize, cell.1 as usize);
    let targets: Vec<Vec<usize>> = slots.iter().map(|s| { let mut v: Vec<usize> = s.iter().map(|c| level_of(shades, *c)).collect(); v.sort(); v.dedup(); v }).collect();
    let mut changed = 0;
    for (n, &(fi, cx, cy)) in data.pos.iter().enumerate() {
        let mut w: BTreeMap<usize, f64> = BTreeMap::new();
        for &(pi, wt) in &data.cells[n] {
            *w.entry(level_of(shades, data.palette[pi])).or_insert(0.0) += wt;
        }
        let present: Vec<(usize, f64)> = w.into_iter().collect();
        let k = present.len();
        let mut best: Option<(f64, usize, Vec<usize>)> = None;
        for (si, t) in targets.iter().enumerate() {
            // every non-decreasing map of the present levels onto the slot's levels
            let m = t.len();
            let mut idx = vec![0usize; k];
            loop {
                if idx.windows(2).all(|p| p[0] <= p[1]) {
                    let mut cost = 0.0;
                    for (i, &(l, wt)) in present.iter().enumerate() {
                        cost += wt * (l as f64 - t[idx[i]] as f64).abs();
                        // white is for the lightest material: a darker level does not take it just because the cell holds three levels
                        if t[idx[i]] == 3 && l < 3 {
                            cost += 1.5 * wt;
                        }
                        if i > 0 && idx[i] == idx[i - 1] {
                            cost += 1.0 * wt.min(present[i - 1].1);
                        }
                    }
                    if best.as_ref().map_or(true, |b| cost < b.0 - 1e-9) {
                        best = Some((cost, si, idx.iter().map(|&j| t[j]).collect()));
                    }
                }
                let mut p = 0;
                while p < k {
                    idx[p] += 1;
                    if idx[p] < m {
                        break;
                    }
                    idx[p] = 0;
                    p += 1;
                }
                if p == k {
                    break;
                }
            }
        }
        let Some((_, _, map)) = best else { continue };
        let g = &mut part.frames[fi].grid;
        for y in cy * ch..((cy + 1) * ch).min(g.h) {
            for x in cx * cw..((cx + 1) * cw).min(g.w) {
                let key = g.cells[y * g.w + x];
                if key == b'.' {
                    continue;
                }
                let Some(c) = rgb_of(key) else { continue };
                let l = level_of(shades, c);
                let Some(pos) = present.iter().position(|p| p.0 == l) else { continue };
                let t = shades[map[pos]];
                if t != c {
                    if let Some(nk) = key_of(t) {
                        g.cells[y * g.w + x] = nk;
                        changed += 1;
                    }
                }
            }
        }
    }
    changed
}

/// The shades of a four-grey console, dark to light (for `fold_cells_levels`).
pub fn stretch_shades(prof: &crate::profile::Profile) -> Vec<Rgb> {
    shades(prof)
}
