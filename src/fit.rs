//! `pxl palette fit`: remap a part into its palette set per cell (M5 phase T2), and the same code as the final pass of
//! `reduce` for palette-set parts.
//!
//! Per frame: the distinct source colours are clustered (drift closer than `CLUSTER` Oklab merges); the cell layout is
//! searched with a cost that counts *folds* (two clusters landing on one slot colour, or a colour moved further than
//! `FAR`) so the layout moves before colours fold; every cell then takes the slot with the fewest folds and the lowest
//! Oklab error, and each pixel the colour its cluster was assigned. Colours already in the set map to themselves, so a
//! second run changes nothing.
//!
//! Template keys: one RGB = one key. A target colour reuses a key of the part's template with that exact colour; the
//! rest are added to the template file as keys of a layer `set` (role `set_rrggbb`) and nothing else in the template
//! changes, so older edits stay valid and `undo` keeps working.

use crate::fsutil::PathSlash;
use crate::cells::{self, Resolved, OPQ, T};
use crate::cli::FitArgs;
use crate::color::{self, Rgb};
use crate::edit::{self, Op};
use crate::fsutil;
use crate::lease;
use crate::lint;
use crate::model::{Grid, Part};
use crate::output::{Loc, Out, PxlError, Result};
use crate::profile;
use crate::pxl;
use crate::status;
use crate::template::{self, KeyDef, Layer, Template};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;

/// Source colours closer than this (Oklab) are one colour (palette drift).
const CLUSTER: f64 = 0.03;
/// A colour moved further than this (Oklab) counts as lost.
const FAR: f64 = 0.14;
/// dE (Oklab x 100) above which a change counts as visible.
pub const VISIBLE: f64 = 10.0;

struct Opt {
    slot: u8,
    colors: Vec<Rgb>,
    /// The same indexes in the other sets of a multi-set part.
    alts: Vec<Vec<Rgb>>,
}

struct Core {
    opts: Vec<Opt>,
    reps: Vec<Rgb>,
    labs: Vec<color::Oklab>,
    counts: Vec<usize>,
    multi_ok: bool,
}

fn dist(a: color::Oklab, b: color::Oklab) -> f64 {
    color::oklab_dist2(a, b).sqrt()
}

/// Distance of a colour to index `t`, averaged over the sets of a multi-set part (the index is shared by all of them).
fn mean_dist(l: color::Oklab, cols: &[Rgb], alts: &[Vec<Rgb>], t: usize) -> f64 {
    let mut s = dist(l, color::srgb_to_oklab(cols[t]));
    for a in alts {
        s += dist(l, color::srgb_to_oklab(a[t]));
    }
    s / (1 + alts.len()) as f64
}

/// Best assignment of clusters `cl` onto colours `cols`: (score = folds + far, cost, target colour index per cluster).
fn assign(core: &Core, cl: &[usize], cols: &[Rgb], alts: &[Vec<Rgb>], weights: Option<&[usize]>) -> (usize, f64, Vec<usize>) {
    let n = cl.len();
    let k = cols.len();
    let w = |i: usize| weights.map_or(core.counts[cl[i]], |ws| ws[i]) as f64;
    let d: Vec<Vec<f64>> = cl.iter().map(|&c| (0..k).map(|t| mean_dist(core.labs[c], cols, alts, t)).collect()).collect();
    let score_of = |a: &[usize]| -> (usize, f64) {
        let mut used = vec![false; k];
        let mut far = 0;
        let mut cost = 0.0;
        for (i, &t) in a.iter().enumerate() {
            used[t] = true;
            if d[i][t] > FAR {
                far += 1;
            }
            cost += w(i) * d[i][t];
        }
        (n - used.iter().filter(|u| **u).count() + far, cost)
    };
    let nearest: Vec<usize> = (0..n).map(|i| (0..k).fold(0, |b, t| if d[i][t] < d[i][b] { t } else { b })).collect();
    if n > 6 || k == 0 || n == 0 {
        let (s, c) = score_of(&nearest);
        return (s, c, nearest);
    }
    let mut best = (usize::MAX, f64::MAX, nearest.clone());
    let total = k.pow(n as u32);
    let mut a = vec![0usize; n];
    for code in 0..total {
        let mut c = code;
        for x in a.iter_mut() {
            *x = c % k;
            c /= k;
        }
        let (s, cost) = score_of(&a);
        if s < best.0 || (s == best.0 && cost < best.1 - 1e-12) {
            best = (s, cost, a.clone());
        }
    }
    best
}

struct CellPlan {
    slot: u8,
    slot2: Option<u8>,
    score: usize,
    cost: f64,
    /// cluster -> target colour
    target: Vec<(usize, Rgb)>,
}

fn clusters_of(mask: u32) -> Vec<usize> {
    (0..31).filter(|b| mask & (1 << b) != 0).collect()
}

fn plan_cell(core: &Core, mask: u32, weights: Option<&HashMap<usize, usize>>) -> CellPlan {
    let cl = clusters_of(mask & !OPQ);
    let ws: Option<Vec<usize>> = weights.map(|m| cl.iter().map(|c| m.get(c).copied().unwrap_or(0)).collect());
    let mut best: Option<CellPlan> = None;
    for o in &core.opts {
        let (s, c, a) = assign(core, &cl, &o.colors, &o.alts, ws.as_deref());
        if best.as_ref().map_or(true, |b| (s, c) < (b.score, b.cost)) {
            best = Some(CellPlan { slot: o.slot, slot2: None, score: s, cost: c, target: cl.iter().zip(&a).map(|(c, t)| (*c, o.colors[*t])).collect() });
        }
    }
    let mut best = best.unwrap_or(CellPlan { slot: 0, slot2: None, score: 0, cost: 0.0, target: Vec::new() });
    if best.score > 0 && core.multi_ok {
        for a in 0..core.opts.len() {
            for b in a + 1..core.opts.len() {
                let cols: Vec<Rgb> = core.opts[a].colors.iter().chain(core.opts[b].colors.iter()).copied().collect();
                let alts: Vec<Vec<Rgb>> = (0..core.opts[a].alts.len()).map(|i| core.opts[a].alts[i].iter().chain(core.opts[b].alts[i].iter()).copied().collect()).collect();
                let nearest: Vec<usize> = cl.iter().map(|&c| (0..cols.len()).fold(0, |bi, t| if mean_dist(core.labs[c], &cols, &alts, t) < mean_dist(core.labs[c], &cols, &alts, bi) { t } else { bi })).collect();
                let mut used = BTreeSet::new();
                let mut far = 0;
                let mut cost = 0.0;
                for (i, &c) in cl.iter().enumerate() {
                    let dd = mean_dist(core.labs[c], &cols, &alts, nearest[i]);
                    used.insert(cols[nearest[i]]);
                    far += usize::from(dd > FAR);
                    cost += dd * core.counts[c] as f64;
                }
                let score = cl.len() - used.len() + far;
                if score < best.score {
                    let in_a = nearest.iter().filter(|t| **t < core.opts[a].colors.len()).count();
                    let (p, s) = if in_a * 2 >= cl.len() { (a, b) } else { (b, a) };
                    best = CellPlan { slot: core.opts[p].slot, slot2: Some(core.opts[s].slot), score, cost, target: cl.iter().zip(&nearest).map(|(c, t)| (*c, cols[*t])).collect() };
                }
            }
        }
    }
    best
}

/// One cell of a fit.
#[derive(Clone, Debug)]
pub struct CellFit {
    pub x: i32,
    pub y: i32,
    pub slot: u8,
    pub slot2: Option<u8>,
    pub score: usize,
    /// Colours that were folded into another or moved far: (source, target, pixels).
    pub folds: Vec<(Rgb, Rgb, usize)>,
}

#[derive(Clone, Debug)]
pub struct FrameFit {
    pub rgb: Vec<Option<Rgb>>,
    pub cells: Vec<CellFit>,
    pub changed: usize,
    /// Pixels moved by more than `VISIBLE` dE (Oklab x 100).
    pub visible: usize,
    pub mean_de: f64,
    pub max_de: f64,
    pub oy: i32,
}

impl FrameFit {
    pub fn folded_cells(&self) -> usize {
        self.cells.iter().filter(|c| !c.folds.is_empty()).count()
    }
}

/// Remap one frame's colours into the set. `avail` limits the target colours (a template that has no key for a set
/// colour cannot use it).
pub fn fit_frame(res: &Resolved, rgb: &[Option<Rgb>], w: usize, h: usize, avail: &dyn Fn(Rgb) -> bool) -> FrameFit {
    let mut freq: BTreeMap<Rgb, usize> = BTreeMap::new();
    for c in rgb.iter().flatten() {
        *freq.entry(*c).or_insert(0) += 1;
    }
    let mut order: Vec<(Rgb, usize)> = freq.into_iter().collect();
    order.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    // exact set colours are their own cluster; others join the nearest cluster within the threshold
    let mut thr = CLUSTER;
    let (reps, cluster_of) = loop {
        let mut reps: Vec<Rgb> = Vec::new();
        let mut of: HashMap<Rgb, usize> = HashMap::new();
        for (c, _) in &order {
            let l = color::srgb_to_oklab(*c);
            let near = reps.iter().position(|r| dist(l, color::srgb_to_oklab(*r)) < thr && (res.colour_bit(*c).is_none() || res.colour_bit(*r).is_none()));
            match near {
                Some(i) => {
                    of.insert(*c, i);
                }
                None => {
                    of.insert(*c, reps.len());
                    reps.push(*c);
                }
            }
        }
        if reps.len() <= 30 {
            break (reps, of);
        }
        thr *= 1.5;
    };
    let mut counts = vec![0usize; reps.len()];
    for (c, n) in &order {
        counts[cluster_of[c]] += n;
    }
    let opts: Vec<Opt> = res
        .slots
        .iter()
        .enumerate()
        .map(|(si, (s, _, cols))| {
            let keep: Vec<usize> = (0..3).filter(|i| avail(cols[*i])).collect();
            Opt { slot: *s, colors: keep.iter().map(|i| cols[*i]).collect(), alts: res.extras.iter().map(|e| keep.iter().map(|i| e.cols[si][*i]).collect()).collect() }
        })
        .filter(|o| !o.colors.is_empty())
        .collect();
    let core = Core { labs: reps.iter().map(|c| color::srgb_to_oklab(*c)).collect(), reps, counts, opts, multi_ok: res.multi_max > 0 };
    let ids: Vec<u8> = rgb.iter().map(|c| c.map_or(T, |c| cluster_of[&c] as u8)).collect();
    if core.opts.is_empty() || order.is_empty() {
        return FrameFit { rgb: rgb.to_vec(), cells: Vec::new(), changed: 0, visible: 0, mean_de: 0.0, max_de: 0.0, oy: 0 };
    }
    let memo: RefCell<HashMap<u32, (usize, u32, usize)>> = RefCell::new(HashMap::new());
    let eval = |m: u32| -> (usize, u32, usize) {
        if let Some(v) = memo.borrow().get(&m) {
            return *v;
        }
        let p = plan_cell(&core, m, None);
        let v = (p.score, if p.slot2.is_some() { 2 } else { 1 }, usize::from(p.slot2.is_some()));
        memo.borrow_mut().insert(m, v);
        v
    };
    let (oy, pos) = cells::search_by(res.cell, res.multi_max as usize, (res.per_line, res.overlay_line, res.per_frame), &ids, w, h, &eval);
    let (cw, ch) = (res.cell.0 as i32, res.cell.1 as i32);
    let mut out = rgb.to_vec();
    let mut cells_out = Vec::new();
    let mut owned = vec![false; w * h];
    let (mut changed, mut visible, mut sum_de, mut max_de) = (0usize, 0usize, 0.0f64, 0.0f64);
    for (x0, y0) in pos {
        let mut mask = OPQ;
        let mut wts: HashMap<usize, usize> = HashMap::new();
        let mut px: Vec<usize> = Vec::new();
        for y in y0.max(0)..(y0 + ch).min(h as i32) {
            for x in x0.max(0)..(x0 + cw).min(w as i32) {
                let i = y as usize * w + x as usize;
                if ids[i] == T || owned[i] {
                    continue;
                }
                owned[i] = true;
                mask |= 1 << ids[i];
                *wts.entry(ids[i] as usize).or_insert(0) += 1;
                px.push(i);
            }
        }
        let plan = plan_cell(&core, mask, Some(&wts));
        let tmap: HashMap<usize, Rgb> = plan.target.iter().copied().collect();
        // folds: clusters sharing a target (all but the biggest) and clusters moved far
        let mut by_target: BTreeMap<Rgb, Vec<usize>> = BTreeMap::new();
        for (c, t) in &plan.target {
            by_target.entry(*t).or_default().push(*c);
        }
        let mut folds = Vec::new();
        for (t, cs) in &by_target {
            let keep = cs.iter().copied().max_by_key(|c| (wts.get(c).copied().unwrap_or(0), std::cmp::Reverse(*c)));
            for &c in cs {
                if Some(c) != keep || dist(core.labs[c], color::srgb_to_oklab(*t)) > FAR {
                    folds.push((core.reps[c], *t, wts.get(&c).copied().unwrap_or(0)));
                }
            }
        }
        for i in px {
            let src = rgb[i].unwrap();
            let dst = tmap[&(ids[i] as usize)];
            if dst != src {
                let de = dist(color::srgb_to_oklab(src), color::srgb_to_oklab(dst)) * 100.0;
                changed += 1;
                visible += usize::from(de > VISIBLE);
                sum_de += de;
                max_de = max_de.max(de);
                out[i] = Some(dst);
            }
        }
        cells_out.push(CellFit { x: x0, y: y0, slot: plan.slot, slot2: plan.slot2, score: plan.score, folds });
    }
    FrameFit { rgb: out, cells: cells_out, changed, visible, mean_de: if changed > 0 { sum_de / changed as f64 } else { 0.0 }, max_de, oy }
}

// ------------------------------------------------------------------------------------------------ keys

/// Colour of every pixel of a grid (snapped to the profile step).
pub fn grid_rgb(part: &Part, tpl: &Template, g: &Grid, step: u32) -> Vec<Option<Rgb>> {
    g.cells.iter().map(|&k| if k == b'.' { None } else { Some(lint::effective_color(part, tpl, k).map_or(Rgb::new(0, 0, 0), |c| color::snap_step(c, step.max(1)))) }).collect()
}

/// Key of a colour in the template (first key in file order), if any.
fn key_for(part: &Part, tpl: &Template, c: Rgb, step: u32) -> Option<u8> {
    tpl.keys().find(|k| lint::effective_color(part, tpl, k.key).map(|x| color::snap_step(x, step.max(1))) == Some(c)).map(|k| k.key)
}

const POOL: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789#$%&*+-/:;<=>?@^_~";

/// A template extended with the keys `colors` need: (template, colour -> key, new keys (key, colour)).
pub fn ensure_keys(part: &Part, tpl: &Template, colors: &BTreeSet<Rgb>, step: u32) -> Result<(Template, BTreeMap<Rgb, u8>, Vec<(u8, Rgb)>)> {
    let mut t = tpl.clone();
    let mut map = BTreeMap::new();
    let mut added: Vec<(u8, Rgb)> = Vec::new();
    for &c in colors {
        if let Some(k) = key_for(part, &t, c, step) {
            map.insert(c, k);
            continue;
        }
        let k = POOL.bytes().find(|b| t.def(*b).is_none() && pxl::valid_key(*b)).ok_or_else(|| PxlError::validation("no free template key left for the set colours"))?;
        let kd = KeyDef { key: k, role: format!("set_{}", color::to_hex(c).trim_start_matches('#')), color: Some(c), under: None };
        match t.layers.iter_mut().find(|l| l.name == "set") {
            Some(l) => l.keys.push(kd),
            None => t.layers.push(Layer { name: "set".into(), keys: vec![kd] }),
        }
        map.insert(c, k);
        added.push((k, c));
    }
    Ok((t, map, added))
}

/// Every colour of the part's palette sets as a template key (`template add-set`, done automatically for assets): returns how many
/// keys were added. A part without palette sets, or without a template, changes nothing.
pub fn sync_set_keys(part: &Part) -> Result<usize> {
    let sets: Vec<String> = part.palette_set.iter().cloned().chain(part.palette_more.iter().cloned()).collect();
    if sets.is_empty() || part.template.is_none() {
        return Ok(0);
    }
    let path = template::locate(part)?;
    // only a template that belongs to the asset itself (its own folder) is changed; a shared project template is left alone
    let (pc, dc) = (std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone()), std::fs::canonicalize(part.dir()).unwrap_or_else(|_| part.dir()));
    if !pc.starts_with(&dc) {
        return Ok(0);
    }
    let tpl = template::load(&path)?;
    let dir = part.dir();
    let mut want: std::collections::BTreeSet<Rgb> = std::collections::BTreeSet::new();
    for n in &sets {
        let set = cells::load_set(n, &dir)?;
        want.extend(cells::all_colours(&set));
    }
    let dummy = Part::new("x", (1, 1));
    let (_, _, added) = ensure_keys(&dummy, &tpl, &want, 1)?;
    write_template_keys(&path, &added)?;
    Ok(added.len())
}

/// Append keys to the template file as keys of `[layers.set]` (in place when the table exists; every other byte is kept).
pub fn write_template_keys(path: &std::path::Path, added: &[(u8, Rgb)]) -> Result<()> {
    if added.is_empty() {
        return Ok(());
    }
    let text = fsutil::read_string(path)?;
    fsutil::atomic_write(path, template::insert_set_keys(&text, added).as_bytes())
}

// ------------------------------------------------------------------------------------------------ reduce pass

/// Final pass of `reduce` for palette-set parts: choose the slot per cell among the colours the template has keys for.
pub fn fit_to_plan(part: &Part, tpl: &Template, g: &mut Grid) -> Result<()> {
    if part.palette_set.is_none() {
        return Ok(());
    }
    let Ok(prof) = profile::load_for(part, None) else { return Ok(()) };
    let Ok(res) = cells::resolve(part, &prof) else { return Ok(()) };
    let step = prof.channel_step;
    let rgb = grid_rgb(part, tpl, g, step);
    let have = |c: Rgb| key_for(part, tpl, c, step).is_some();
    let ff = fit_frame(&res, &rgb, g.w, g.h, &have);
    for i in 0..g.cells.len() {
        if ff.rgb[i] != rgb[i] {
            if let Some(k) = ff.rgb[i].and_then(|c| key_for(part, tpl, c, step)) {
                g.cells[i] = k;
            }
        }
    }
    Ok(())
}

// ------------------------------------------------------------------------------------------------ command

fn hexf(c: Rgb) -> String {
    color::to_hex(c)
}

pub fn run_fit(a: &FitArgs, out: &mut Out) -> Result<()> {
    let path = fsutil::resolve_part(&a.part)?;
    let mut part = pxl::load(&path)?;
    let set_changed = match &a.set {
        Some(s) => {
            let names: Vec<String> = s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect();
            if names.is_empty() {
                return Err(PxlError::usage("--set needs a set name (or a comma separated list for a part used in several zones)"));
            }
            let cur: Vec<String> = part.palette_sets().iter().map(|x| x.to_string()).collect();
            if cur != names {
                part.palette_set = Some(names[0].clone());
                part.palette_more = names[1..].to_vec();
                true
            } else {
                false
            }
        }
        None => false,
    };
    if part.palette_set.is_none() {
        return Err(PxlError::usage(format!("{} has no palette_set", path.slash())).with_hint("give the set: pxl palette fit PART --set zone_2"));
    }
    let prof = profile::load_for(&part, None)?;
    let res = cells::resolve(&part, &prof)?;
    let tpl = template::load_for(&part)?;
    let step = prof.channel_step;
    let sel: Vec<usize> = a.sel.select(&part)?.iter().filter_map(|f| part.frame_index(&f.name)).collect();
    let mut skipped_locked = Vec::new();
    let mut fits: Vec<(usize, FrameFit, Vec<Option<Rgb>>)> = Vec::new();
    for fi in sel {
        let f = &part.frames[fi];
        if f.locked && !a.include_locked {
            skipped_locked.push(f.name.clone());
            continue;
        }
        let rgb = grid_rgb(&part, &tpl, &f.grid, step);
        let ff = fit_frame(&res, &rgb, f.grid.w, f.grid.h, &|_| true);
        fits.push((fi, ff, rgb));
    }
    let mut needed: BTreeSet<Rgb> = BTreeSet::new();
    for (_, ff, rgb) in &fits {
        for i in 0..rgb.len() {
            if ff.rgb[i] != rgb[i] {
                needed.insert(ff.rgb[i].unwrap());
            }
        }
    }
    if !a.no_all_keys {
        for n in part.palette_sets() {
            needed.extend(cells::all_colours(&cells::load_set(n, &part.dir())?));
        }
    }
    let (ext, kmap, added) = ensure_keys(&part, &tpl, &needed, step)?;
    // new grids
    let mut new_grids: Vec<(usize, Grid)> = Vec::new();
    for (fi, ff, rgb) in &fits {
        let mut g = part.frames[*fi].grid.clone();
        for i in 0..g.cells.len() {
            if ff.rgb[i] != rgb[i] {
                g.cells[i] = kmap[&ff.rgb[i].unwrap()];
            }
        }
        if g != part.frames[*fi].grid {
            new_grids.push((*fi, g));
        }
    }
    // report
    let (mut tot_changed, mut tot_folded) = (0usize, 0usize);
    let mut jf = Vec::new();
    out.line(format!("{}: fit into palette set '{}' ({} slots), category '{}'{}", part.name, part.palette_sets().join(" + "), res.slots.len(), res.category, if a.apply { "" } else { " (dry run: nothing written, add --apply)" }));
    for (fi, ff, rgb) in &fits {
        let f = &part.frames[*fi];
        let pix = cells::pix_from_rgb(&res, f.grid.w, f.grid.h, ff.rgb.clone());
        let after = cells::place_frame(&part, &res, &pix, *fi).map_err(|e| PxlError::validation(e).with_loc(Loc::file(path.slash()).frame(f.name.clone())))?;
        tot_changed += ff.changed;
        tot_folded += ff.folded_cells();
        let opaque = rgb.iter().flatten().count();
        out.line(format!(
            "frame {}: {} of {} px changed ({} visible, dE > 10), mean dE {:.1}, max dE {:.1}; {} cell(s) with folds; layout {} sprites, max {} per line, {} invalid",
            f.name, ff.changed, opaque, ff.visible, ff.mean_de, ff.max_de, ff.folded_cells(), after.sprites, after.max_line, after.invalid
        ));
        let mut per_set = Vec::new();
        if !res.extras.is_empty() {
            let cs = cells::colours_in_sets(&res, &pix, &after);
            for (k, (sname, _)) in res.all_sets().iter().enumerate() {
                let (mut n, mut vis, mut sum, mut mx) = (0usize, 0usize, 0.0f64, 0.0f64);
                for (i, src) in rgb.iter().enumerate() {
                    if let (Some(a), Some(b)) = (src, cs[k][i]) {
                        if *a != b {
                            let de = dist(color::srgb_to_oklab(*a), color::srgb_to_oklab(b)) * 100.0;
                            n += 1;
                            vis += usize::from(de > VISIBLE);
                            sum += de;
                            mx = mx.max(de);
                        }
                    }
                }
                out.line(format!("  set {}: {} px differ from the source ({} visible), mean dE {:.1}, max dE {:.1}", sname, n, vis, if n > 0 { sum / n as f64 } else { 0.0 }, mx));
                per_set.push(json!({"set": sname, "differs": n, "visible": vis, "mean_de": if n > 0 { sum / n as f64 } else { 0.0 }, "max_de": mx}));
            }
        }
        let mut fj = Vec::new();
        for c in ff.cells.iter().filter(|c| !c.folds.is_empty()) {
            let fl: Vec<String> = c.folds.iter().map(|(s, t, n)| format!("{} -> {} ({} px)", hexf(*s), hexf(*t), n)).collect();
            out.line(format!("  cell {},{} slot {}: {}", c.x - part.origin.0, c.y - part.origin.1, c.slot, fl.join(", ")));
            fj.push(json!({"x": c.x - part.origin.0, "y": c.y - part.origin.1, "slot": c.slot, "folds": c.folds.iter().map(|(s, t, n)| json!({"from": hexf(*s), "to": hexf(*t), "pixels": n})).collect::<Vec<_>>()}));
        }
        jf.push(json!({"frame": f.name, "changed": ff.changed, "visible": ff.visible, "opaque": opaque, "mean_de": ff.mean_de, "max_de": ff.max_de, "folded_cells": ff.folded_cells(), "folds": fj, "sets": per_set,
            "layout": {"sprites": after.sprites, "max_per_line": after.max_line, "invalid": after.invalid, "two_slot_cells": after.multi}}));
    }
    if !skipped_locked.is_empty() {
        out.line(format!("skipped {} locked frame(s) (--include-locked fits them too): {}", skipped_locked.len(), skipped_locked.join(", ")));
    }
    if !added.is_empty() {
        out.line(format!("template '{}' gets {} new key(s) in layer 'set': {}", tpl.name, added.len(), added.iter().map(|(k, c)| format!("{}={}", *k as char, hexf(*c))).collect::<Vec<_>>().join(" ")));
    }
    out.line(format!("total: {} px changed in {} frame(s), {} cell(s) with folds", tot_changed, fits.len(), tot_folded));
    out.set("schema", json!(1));
    out.set("fit", json!({"palette_set": res.set.name, "applied": a.apply, "frames": jf, "changed": tot_changed, "folded_cells": tot_folded, "new_keys": added.iter().map(|(k, c)| json!({"key": (*k as char).to_string(), "color": hexf(*c)})).collect::<Vec<_>>(), "skipped_locked": skipped_locked}));
    // dry-run compare page: before | after | changed pixels only
    if let Some(p) = &a.png {
        let mut stack: Vec<crate::img::RgbaImage> = Vec::new();
        let mut height = 0;
        for (fi, g) in new_grids.iter() {
            let f = &part.frames[*fi];
            let mut diff = Grid::new(g.w, g.h, b'.');
            for i in 0..g.cells.len() {
                if g.cells[i] != f.grid.cells[i] {
                    diff.cells[i] = g.cells[i];
                }
            }
            let ffit = fits.iter().find(|x| x.0 == *fi).map(|x| &x.1).unwrap();
            let marks: Vec<((i32, i32, usize, usize), crate::img::Rgba)> = ffit.cells.iter().filter(|c| !c.folds.is_empty()).map(|c| ((c.x, c.y, res.cell.0, res.cell.1), crate::img::Rgba::opaque(255, 0, 0))).collect();
            let win = (0, 0, g.w, g.h);
            let (im, _) = crate::show::render_panels_marked(&part, Some(&ext), None, &[("before".to_string(), &f.grid), (format!("after: {}", f.name), g), ("changed".to_string(), &diff)], win, &marks, a.zoom);
            if !stack.is_empty() && height + im.h > crate::compare::MAX_PX {
                break;
            }
            height += im.h;
            stack.push(im);
        }
        if let Some(first) = stack.first() {
            let w = stack.iter().map(|i| i.w).max().unwrap_or(first.w);
            let mut sheet = crate::img::RgbaImage::filled(w, height, crate::compare::BG);
            let mut y = 0;
            for im in &stack {
                crate::compare::paste(&mut sheet, im, 0, y);
                y += im.h;
            }
            let target = PathBuf::from(p);
            crate::show::write_png(&target, &sheet)?;
            out.line(format!("compare page: {} ({}x{} px: before | after | changed pixels; folded cells ringed red)", target.slash(), sheet.w, sheet.h));
            out.set("png", json!(target.slash()));
        } else {
            out.line("nothing changes, no compare page written");
        }
    }
    if !a.apply {
        return Ok(());
    }
    status::refuse_if_approved(&path, a.force_approved)?;
    lease::guard(&path, out.agent())?;
    write_template_keys(&tpl.path, &added)?;
    if set_changed {
        pxl::save(&path, &part)?;
        out.line(format!("palette_set = {} written to the part header", part.palette_sets().join(", ")));
    }
    if new_grids.is_empty() {
        return Ok(());
    }
    let ops: Vec<Op> = new_grids.iter().map(|(fi, g)| Op::WriteGrid { frame: part.frames[*fi].name.clone(), grid: g.to_lines().join("\n") }).collect();
    let flags = crate::cli::EditFlags { no_lock: true, force_approved: a.force_approved, no_ref: true, ..Default::default() };
    edit::execute(&a.part, &ops, &flags, "palette fit", out)?;
    // execute's own JSON fields would hide ours; keep ours under "fit"
    let _: Option<Value> = None;
    Ok(())
}
