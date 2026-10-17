//! `exemplar`: learn from approved frames.
//!
//! The bank holds every *locked* frame of the part (grid + reference) and every extra grid listed in
//! `[reduce] exemplars = ["../golden/01_stand.txt", ...]` (paths relative to the part; the file stem names the
//! frame whose reference is the grid's input, so `golden/01_stand.txt` pairs with frame `01_stand`). The frame
//! being reduced is never its own exemplar.
//!
//! An exemplar grid may use another template's keys. `"golden/01_stand.txt@templates/old.toml"` names the
//! grid's source template: its keys are translated to the part's template by colour (then by role). Without a
//! source template a key must already exist in the part's template (key equality). An exemplar that cannot be
//! translated (or read, or paired with a reference) is skipped with a warning in `Bank::warnings`; building the
//! bank never fails because of one bad exemplar.
//!
//! For every target cell, in order:
//! 1. **Identical region copy:** if the reference pixels (raw RGBA) of the 3x3 cell region around the cell are
//!    identical to the same region of an exemplar frame (same cell position, window not empty), the exemplar's
//!    key is copied (majority over all identical exemplars, ties to the palette order). Unchanged parts of an
//!    animation stay identical to the approved frame (DESIGN 1.1 Consistent).
//! 2. **Exact neighbourhood match:** the signature of a cell is its classified source block (f x f template
//!    keys) plus a ring of one source pixel; an identical signature in the bank gives its key (majority).
//! 3. **Nearest neighbourhood:** otherwise the bank signature with the smallest distance wins when the
//!    distance is at most `MAX_DIST`; distance = 3 per differing block pixel + 1 per differing ring pixel,
//!    ties to the first signature in byte order, key by majority.
//!    Steps 2 and 3 only recolour cells that `vote` made opaque (the silhouette stays faithful to the reference).
//! 4. Otherwise the cell keeps the `vote` result (fallback; also used when the bank is empty or the part uses an
//!    explicit `target` size).
//!
//! The method gets stronger as more frames are locked: more neighbourhoods, more identical regions.

use crate::fsutil::PathSlash;
use super::*;
use crate::model::Frame;
use crate::template;
use crate::{fsutil, load, pxl};
use std::path::Path;

/// Largest signature distance accepted in step 3 (one differing block pixel, or three ring pixels).
pub const MAX_DIST: u32 = 3;

struct Ex {
    name: String,
    prep: Prep,
    img: RgbaImage,
    grid: Grid,
}

pub struct Bank {
    exs: Vec<Ex>,
    /// signature -> (key, exemplar index) entries, in byte order of the signature.
    index: Vec<(Vec<u8>, Vec<(u8, u32)>)>,
    lookup: BTreeMap<Vec<u8>, usize>,
    f: i32,
    /// Exemplars that were skipped or translated, with the reason (callers print them as warnings).
    pub warnings: Vec<String>,
}

/// Translate the keys of `grid` (written in `src`'s template, if known) to `tpl`: by colour, then by role; with
/// no source template a key must exist in `tpl`. Err names the first key that has no counterpart.
fn translate_keys(grid: &mut Grid, src: Option<&Template>, tpl: &Template) -> std::result::Result<usize, String> {
    let mut used: Vec<u8> = grid.cells.clone();
    used.sort_unstable();
    used.dedup();
    let mut map: Vec<(u8, u8)> = Vec::new();
    for k in used {
        if k == TRANSPARENT {
            continue;
        }
        let to = match src.and_then(|s| s.def(k)) {
            Some(d) => d
                .color
                .and_then(|c| tpl.keys().find(|t| t.color == Some(c)).map(|t| t.key))
                .or_else(|| tpl.key_for_role(&d.role))
                .or_else(|| tpl.def(k).map(|_| k)),
            None if src.is_some() => None,
            None => tpl.def(k).map(|_| k),
        };
        match to {
            Some(t) => map.push((k, t)),
            None => return Err(format!("key '{}' has no counterpart in template '{}'", k as char, tpl.name)),
        }
    }
    let changed = map.iter().filter(|(a, b)| a != b).count();
    for c in grid.cells.iter_mut() {
        if let Some(&(_, t)) = map.iter().find(|(a, _)| a == c) {
            *c = t;
        }
    }
    Ok(changed)
}

fn factor_of(part: &Part) -> Option<i32> {
    if part.input.target.is_some() && part.reduce.factor.or(part.input.factor).is_none() {
        None
    } else {
        Some(part.reduce.factor.or(part.input.factor).unwrap_or(1).max(1) as i32)
    }
}

/// Classified window of a cell: block pixels first (row major), then the ring pixels.
fn signature(prep: &Prep, f: i32, cx: i32, cy: i32) -> Vec<u8> {
    let (x0, y0, _, _) = prep.block(cx, cy);
    let mut s = Vec::with_capacity(((f + 2) * (f + 2)) as usize);
    for y in 0..f {
        for x in 0..f {
            s.push(prep.cls(x0 + x, y0 + y));
        }
    }
    for y in -1..f + 1 {
        for x in -1..f + 1 {
            if x < 0 || y < 0 || x >= f || y >= f {
                s.push(prep.cls(x0 + x, y0 + y));
            }
        }
    }
    s
}

fn dist(a: &[u8], b: &[u8], nblock: usize, limit: u32) -> Option<u32> {
    let mut d = 0u32;
    for i in 0..a.len() {
        if a[i] != b[i] {
            d += if i < nblock { 3 } else { 1 };
            if d > limit {
                return None;
            }
        }
    }
    Some(d)
}

impl Bank {
    /// Build the bank of a part. `skip` excludes a frame by name (hold-out); None keeps all and `reduce` skips
    /// the frame being reduced per call.
    pub fn build(part: &Part, tpl: &Template, skip: Option<&str>) -> Result<Bank> {
        let dir = part.dir();
        let mut exs: Vec<Ex> = Vec::new();
        let f = factor_of(part).unwrap_or(1);
        let mut warnings: Vec<String> = Vec::new();
        let add = |name: &str, grid: Grid, exs: &mut Vec<Ex>| -> Result<()> {
            let fr = part.frame(name).cloned().unwrap_or(Frame { name: name.to_string(), src: None, locked: false, grid: Grid::new(0, 0, b'.') });
            let rf = load::reference(part, &dir, &fr)?;
            let prep = Prep::new(&rf.img, rf.origin, part, tpl)?;
            exs.push(Ex { name: name.to_string(), prep, img: rf.img, grid });
            Ok(())
        };
        if factor_of(part).is_some() {
            for fr in &part.frames {
                if fr.locked && Some(fr.name.as_str()) != skip {
                    add(&fr.name, fr.grid.clone(), &mut exs)?;
                }
            }
            for entry in &part.reduce.exemplars {
                let (rel, srct) = match entry.split_once('@') {
                    Some((r, t)) => (r, Some(t)),
                    None => (entry.as_str(), None),
                };
                let stem = Path::new(rel).file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
                if Some(stem.as_str()) == skip || exs.iter().any(|e| e.name == stem) {
                    continue; // a locked frame of the same name already supplies it
                }
                let mut one = || -> std::result::Result<(), String> {
                    let p = fsutil::find_up(&dir, rel).unwrap_or_else(|| dir.join(rel));
                    let text = fsutil::read_string(&p).map_err(|e| e.msg)?;
                    let mut grid = pxl::parse_grid(&text, part.size.0, part.size.1, &Loc::file(p.slash())).map_err(|e| e.msg)?;
                    let src_tpl = match srct {
                        Some(t) => {
                            let tp = fsutil::find_up(&dir, t).ok_or_else(|| format!("source template '{}' not found", t))?;
                            Some(template::load(&tp).map_err(|e| e.msg)?)
                        }
                        None => None,
                    };
                    let n = translate_keys(&mut grid, src_tpl.as_ref(), tpl)?;
                    add(&stem, grid, &mut exs).map_err(|e| e.msg)?;
                    if n > 0 {
                        warnings.push(format!("exemplar {}: translated {} key(s) to template '{}'", rel, n, tpl.name));
                    }
                    Ok(())
                };
                if let Err(why) = one() {
                    warnings.push(format!(
                        "exemplar {} skipped: {} (list it as \"{}@<its template>\" to translate keys by colour)",
                        rel,
                        why,
                        rel
                    ));
                }
            }
        }
        let mut map: BTreeMap<Vec<u8>, Vec<(u8, u32)>> = BTreeMap::new();
        for (ei, e) in exs.iter().enumerate() {
            for cy in 0..e.grid.h as i32 {
                for cx in 0..e.grid.w as i32 {
                    let s = signature(&e.prep, f, cx, cy);
                    map.entry(s).or_default().push((e.grid.get(cx, cy), ei as u32));
                }
            }
        }
        let mut index = Vec::new();
        let mut lookup = BTreeMap::new();
        for (i, (s, v)) in map.into_iter().enumerate() {
            lookup.insert(s.clone(), i);
            index.push((s, v));
        }
        Ok(Bank { exs, index, lookup, f, warnings })
    }
    /// Grid of an exemplar whose reference is pixel-identical to `img` (skipping the held-out frame).
    pub fn identical(&self, img: &RgbaImage, skip: Option<&str>) -> Option<&Grid> {
        self.exs.iter().find(|e| Some(e.name.as_str()) != skip && e.img.w == img.w && e.img.h == img.h && e.img.px == img.px).map(|e| &e.grid)
    }
    pub fn is_empty(&self) -> bool {
        self.exs.is_empty()
    }
    pub fn len(&self) -> usize {
        self.exs.len()
    }
}

fn majority(keys: &[u8], rank: &[usize; 256]) -> Option<u8> {
    let mut t: Vec<(u8, usize)> = Vec::new();
    for &k in keys {
        match t.iter_mut().find(|e| e.0 == k) {
            Some(e) => e.1 += 1,
            None => t.push((k, 1)),
        }
    }
    t.iter().max_by(|a, b| (a.1, std::cmp::Reverse(rank[a.0 as usize])).cmp(&(b.1, std::cmp::Reverse(rank[b.0 as usize])))).map(|e| e.0)
}

/// Steps 2 and 3 only choose the colour of a cell the silhouette (vote) already made opaque: an exemplar never
/// adds or removes cells, so the shape stays faithful to the reference; the exact copy of step 1 does.
fn recolour(fallback: &Grid, cx: i32, cy: i32, k: u8) -> bool {
    fallback.get(cx, cy) != TRANSPARENT && k != TRANSPARENT
}

pub fn reduce(prep: &Prep, img: &RgbaImage, _origin: (i32, i32), part: &Part, tpl: &Template, frame: Option<&str>, bank: &Bank) -> Result<Grid> {
    if let Some(g) = bank.identical(img, frame) {
        return Ok(g.clone());
    }
    let fallback = vote_raw(prep, part, tpl)?;
    if bank.is_empty() || factor_of(part).is_none() {
        let mut fb = fallback;
        prep.protect_salient(&mut fb, tpl);
        prep.stamp_features(&mut fb, tpl, &[]);
        return Ok(fb);
    }
    let f = bank.f;
    let mut g = fallback.clone();
    let skip = |ei: u32| Some(bank.exs[ei as usize].name.as_str()) == frame;
    let nblock = (f * f) as usize;
    let max_dist: u32 = std::env::var("PXL_MAXD").ok().and_then(|v| v.parse().ok()).unwrap_or(MAX_DIST);
    for cy in 0..prep.gh as i32 {
        for cx in 0..prep.gw as i32 {
            let sig = signature(prep, f, cx, cy);
            let empty = sig.iter().all(|&k| k == TRANSPARENT);
            if empty {
                g.set(cx, cy, TRANSPARENT);
                continue;
            }
            let (bx, by, _, _) = prep.block(cx, cy);
            // 1. identical region
            let mut copies: Vec<u8> = Vec::new();
            for (ei, e) in bank.exs.iter().enumerate() {
                if skip(ei as u32) {
                    continue;
                }
                let (ex0, ey0, _, _) = e.prep.block(cx, cy);
                let mut same = true;
                let mut any = false;
                'w: for j in -f..2 * f {
                    for i in -f..2 * f {
                        let a = img.get(bx + i, by + j);
                        let b = e.img.get(ex0 + i, ey0 + j);
                        if a != b {
                            same = false;
                            break 'w;
                        }
                        if a.a >= 128 {
                            any = true;
                        }
                    }
                }
                if same && any {
                    copies.push(e.grid.get(cx, cy));
                }
            }
            if let Some(k) = majority(&copies, &prep.rank) {
                g.set(cx, cy, k);
                continue;
            }
            // 2. exact signature
            if let Some(&i) = bank.lookup.get(&sig) {
                let keys: Vec<u8> = bank.index[i].1.iter().filter(|e| !skip(e.1)).map(|e| e.0).collect();
                if let Some(k) = majority(&keys, &prep.rank) {
                    if recolour(&fallback, cx, cy, k) {
                        g.set(cx, cy, k);
                    }
                    continue;
                }
            }
            // 3. nearest signature
            let mut best: Option<(u32, usize)> = None;
            for (i, (s, ents)) in bank.index.iter().enumerate() {
                let limit = best.map_or(max_dist, |b| b.0.saturating_sub(1));
                if let Some(d) = dist(&sig, s, nblock, limit) {
                    if ents.iter().any(|e| !skip(e.1)) {
                        best = Some((d, i));
                    }
                }
            }
            if let Some((_, i)) = best {
                let keys: Vec<u8> = bank.index[i].1.iter().filter(|e| !skip(e.1)).map(|e| e.0).collect();
                if let Some(k) = majority(&keys, &prep.rank) {
                    if recolour(&fallback, cx, cy, k) {
                        g.set(cx, cy, k);
                    }
                }
            }
        }
    }
    prep.protect_salient(&mut g, tpl);
    let exs: Vec<crate::features::ExEye> = bank.exs.iter().filter(|e| Some(e.name.as_str()) != frame).map(|e| crate::features::ExEye { eyes: &e.prep.eyes, grid: &e.grid }).collect();
    prep.stamp_features(&mut g, tpl, &exs);
    Ok(g)
}
