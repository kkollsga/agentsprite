//! Recomputed raw-output baseline: for parts loaded before the raw audit existed, re-run the automatic
//! methods (same hold-out rules as `candidates`) and mark every frame (tile) whose pixels equal a method's
//! output, or differ from the closest one by at most `tolerance` pixels, as raw.
//!
//! Only `status.json` is written (under the per-part lock); the part file is never touched. The
//! reduce -> fit -> dedupe chain of tile sets is not replayed (it needs the whole sheet and is not cheap);
//! plain tile reducer output is compared per tile.

use crate::model::{Grid, Method, Part};
use crate::output::Result;
use crate::reduce::Prep;
use crate::template::Template;
use crate::{candidates, fsutil, load, pxl, status, template};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recomputed {
    pub frame: String,
    /// Raw tiles (tile parts); empty for a sprite frame.
    pub tiles: Vec<usize>,
    /// Raw units (tiles, or 1 for a raw sprite frame).
    pub raw: usize,
    /// Units that could be raw at all.
    pub total: usize,
    /// Matching method of a sprite frame (or the most common one of a tile frame).
    pub method: String,
    /// Pixel distance to the matching method output (sprite) or the largest distance among raw tiles.
    pub distance: usize,
    /// Pixel distance of the closest method output even if too far to count (sprite frames).
    pub closest: usize,
}

fn dist(a: &[u8], b: &[u8]) -> usize {
    if a.len() != b.len() {
        return usize::MAX;
    }
    a.iter().zip(b).filter(|(x, y)| x != y).count()
}

fn tile_cells(g: &Grid, t: usize) -> [u8; 64] {
    let per = g.w / 8;
    let (tx, ty) = ((t % per) * 8, (t / per) * 8);
    let mut b = [0u8; 64];
    for y in 0..8 {
        for x in 0..8 {
            b[y * 8 + x] = g.cells[(ty + y) * g.w + tx + x];
        }
    }
    b
}

/// Default tolerance: 0.5% of the opaque pixels, rounded (tiny sprites must match exactly).
pub fn default_tolerance(opaque: usize) -> usize {
    ((opaque as f64) * 0.005).round() as usize
}

/// Is baseline entry `prior` already information? A differing entry means the unit was edited; an entry equal to
/// the current pixels is trusted unless a previous recompute wrote it (that one may have been wrong).
fn known(prior: Option<&str>, cur: &str, from_recompute: bool) -> Option<bool> {
    let p = prior?;
    if p == cur && from_recompute {
        None
    } else {
        Some(p == cur)
    }
}

pub fn region_hits_tile(r: &(i32, i32, usize, usize), per: usize, t: usize) -> bool {
    let (tx, ty) = (((t % per) * 8) as i32, ((t / per) * 8) as i32);
    r.0 < tx + 8 && r.0 + r.2 as i32 > tx && r.1 < ty + 8 && r.1 + r.3 as i32 > ty
}

/// Recompute the raw baseline of the part at `path` and store it. Returns the per-frame result.
///
/// Frames (tiles) with a standing edit in the part's journal since their last load are hand-finished, never
/// raw. Frames the status already knows (a baseline that differs from the pixels) are not touched. Only
/// source-driven methods count (vote, nearest, contour, and auto/exemplar with an empty bank): a frame equal to a
/// finished sibling is not raw.
pub fn recompute(path: &Path, tolerance: Option<usize>) -> Result<Vec<Recomputed>> {
    let _pl = fsutil::lock_part(path)?;
    let part = pxl::load(path)?;
    let tpl = template::load_for(&part).ok();
    let hand = crate::journal::hand_edited(path)?;
    let mut s = status::read(path);
    let (res, hashes, edited) = analyse(&part, tpl.as_ref(), tolerance, &hand, &s);
    let had = !s.raw.is_empty();
    let mut filled = false;
    for (name, h) in hashes {
        filled |= s.raw.insert(name, h.clone()).as_deref() != Some(h.as_str());
    }
    for (name, v) in edited {
        let e = s.edited.entry(name).or_default();
        for t in v {
            if !e.contains(&t) {
                e.push(t);
            }
        }
        e.sort_unstable();
    }
    s.edited.retain(|k, v| !v.is_empty() && part.frames.iter().any(|f| &f.name == k));
    if s.raw_source.is_none() {
        s.raw_source = Some(if had && filled { "mixed" } else { "recomputed" }.into());
    }
    status::write(path, &s)?;
    Ok(res)
}

type Analysis = (Vec<Recomputed>, Vec<(String, String)>, Vec<(String, Vec<usize>)>);

fn analyse(part: &Part, tpl: Option<&Template>, tolerance: Option<usize>, hand: &BTreeMap<String, Vec<(i32, i32, usize, usize)>>, st: &status::StatusFile) -> Analysis {
    let mut res = Vec::new();
    let mut hashes = Vec::new();
    let mut edited = Vec::new();
    let from_rec = st.raw_source.as_deref() == Some("recomputed");
    // empty exemplar bank: no learning from the part's own (finished) frames
    let bank = tpl.and_then(|t| {
        let mut base = part.clone();
        base.frames.clear();
        candidates::bank_for(&base, t, &Method::ALL).ok().flatten()
    });
    for (fi, f) in part.frames.iter().enumerate() {
        let tiles = status::tile_count(part, fi);
        let prior = st.raw.get(&f.name).map(|x| x.as_str());
        let cur_all = status::raw_hash_of(part, fi);
        let regions: &[(i32, i32, usize, usize)] = hand.get(&f.name).map(|v| v.as_slice()).unwrap_or(&[]);
        let known_edited: &[usize] = st.edited.get(&f.name).map(|v| v.as_slice()).unwrap_or(&[]);
        match tiles {
            Some(n) => {
                let per = f.grid.w / 8;
                let mut raw_tiles = Vec::new();
                let mut total = 0;
                let mut maxd = 0;
                let mut cur = prior.filter(|p| p.len() == n * 8).map(|p| p.to_string()).unwrap_or_else(|| cur_all.clone());
                let mut method = String::new();
                let mut ed = Vec::new();
                let mut outs: Option<Vec<(String, Grid)>> = None;
                for t in 0..n {
                    let cells = tile_cells(&f.grid, t);
                    if cells.iter().all(|&c| c == cells[0]) {
                        continue;
                    }
                    total += 1;
                    let ch = status::tile_hash(&f.grid, t);
                    let (pt, ct) = (prior.filter(|p| p.len() == n * 8).map(|p| &p[t * 8..t * 8 + 8]), &cur_all[t * 8..t * 8 + 8]);
                    if known_edited.contains(&t) || regions.iter().any(|r| region_hits_tile(r, per, t)) {
                        ed.push(t);
                        if cur[t * 8..t * 8 + 8] == ch {
                            cur.replace_range(t * 8..t * 8 + 8, "00000000");
                        }
                        continue;
                    }
                    if let Some(is_raw) = known(pt, ct, from_rec) {
                        if is_raw {
                            raw_tiles.push(t);
                            method = "baseline".into();
                        }
                        continue;
                    }
                    let outs = outs.get_or_insert_with(|| tile_outputs(part, tpl, fi));
                    let tol = tolerance.unwrap_or_else(|| default_tolerance(cells.iter().filter(|&&c| c != b'.').count()));
                    let best = outs.iter().filter(|(_, g)| g.w == f.grid.w && g.h == f.grid.h).map(|(m, g)| (dist(&cells, &tile_cells(g, t)), m, g)).min_by_key(|x| x.0);
                    match best {
                        Some((d, m, _)) if d <= tol => {
                            raw_tiles.push(t);
                            maxd = maxd.max(d);
                            method = m.clone();
                            cur.replace_range(t * 8..t * 8 + 8, &ch);
                        }
                        Some((_, _, g)) => cur.replace_range(t * 8..t * 8 + 8, &status::tile_hash(g, t)),
                        None => cur.replace_range(t * 8..t * 8 + 8, "00000000"),
                    }
                }
                hashes.push((f.name.clone(), cur));
                edited.push((f.name.clone(), ed));
                res.push(Recomputed { frame: f.name.clone(), raw: raw_tiles.len(), tiles: raw_tiles, total, method, distance: maxd, closest: 0 });
            }
            None => {
                let has = f.grid.cells.iter().any(|&c| c != b'.');
                if !known_edited.is_empty() || !regions.is_empty() {
                    let h = match prior {
                        Some(p) if p != cur_all => p.to_string(),
                        _ => "0".to_string(),
                    };
                    hashes.push((f.name.clone(), h));
                    edited.push((f.name.clone(), vec![0]));
                    res.push(Recomputed { frame: f.name.clone(), tiles: Vec::new(), raw: 0, total: has as usize, method: String::new(), distance: 0, closest: 0 });
                    continue;
                }
                if let Some(is_raw) = known(prior, &cur_all, from_rec) {
                    let raw = (is_raw && has) as usize;
                    hashes.push((f.name.clone(), prior.unwrap_or("0").to_string()));
                    res.push(Recomputed { frame: f.name.clone(), tiles: Vec::new(), raw, total: has as usize, method: if raw > 0 { "baseline".into() } else { String::new() }, distance: 0, closest: 0 });
                    continue;
                }
                let outs = sprite_outputs(part, tpl, fi, bank.as_ref());
                let tol = tolerance.unwrap_or_else(|| default_tolerance(f.grid.cells.iter().filter(|&&c| c != b'.').count()));
                let best = outs.iter().map(|(m, g)| (dist(&f.grid.cells, &g.cells), m, g)).min_by_key(|x| x.0);
                let (raw, method, d, closest, h) = match best {
                    Some((d, m, _)) if has && d <= tol => (1, m.clone(), d, d, cur_all),
                    Some((d, _, g)) => (0, String::new(), 0, if d == usize::MAX { 0 } else { d }, fsutil::content_hash(&g.cells)),
                    None => (0, String::new(), 0, 0, "0".into()),
                };
                hashes.push((f.name.clone(), h));
                res.push(Recomputed { frame: f.name.clone(), tiles: Vec::new(), raw, total: has as usize, method, distance: d, closest });
            }
        }
    }
    (res, hashes, edited)
}

fn tile_outputs(part: &Part, tpl: Option<&Template>, fi: usize) -> Vec<(String, Grid)> {
    let mut outs = Vec::new();
    if let Some(tpl) = tpl {
        if !tpl.palettes.is_empty() {
            if let Ok(g) = crate::tiles::reduce::reduce_frame(part, tpl, fi) {
                outs.push(("tiles".into(), g));
            }
        }
    }
    outs
}

fn sprite_outputs(part: &Part, tpl: Option<&Template>, fi: usize, bank: Option<&crate::reduce::exemplar::Bank>) -> Vec<(String, Grid)> {
    let mut outs = Vec::new();
    let f = &part.frames[fi];
    if let Some(tpl) = tpl {
        if let Ok(rf) = load::reference(part, &part.dir(), f) {
            if let Ok(prep) = Prep::new(&rf.img, rf.origin, part, tpl) {
                for m in Method::ALL {
                    if let Ok(g) = crate::reduce::reduce_with(&prep, &rf.img, rf.origin, part, tpl, Some(f.name.as_str()), m, bank) {
                        outs.push((format!("{:?}", m).to_lowercase(), g));
                    }
                }
            }
        }
    }
    outs
}

/// Human line(s) for one part's result.
pub fn describe(r: &[Recomputed]) -> Vec<String> {
    let mut v = Vec::new();
    for x in r.iter().filter(|x| x.raw > 0) {
        if x.tiles.is_empty() {
            v.push(format!("  frame {}: matches {} (distance {} px)", x.frame, x.method, x.distance));
        } else {
            v.push(format!("  frame {}: {} of {} tiles match the tile reducer (largest distance {} px)", x.frame, x.raw, x.total, x.distance));
        }
    }
    v
}
