//! Sprite tile memory (M5 phase T2): unique 8x16 cell patterns, `pxl budget`, the `tile_budget` check, and
//! `pxl cells dedupe`.
//!
//! A cell's *pattern* is its 2-bit pixel indexes inside the cell's slot (0 = transparent, 1..3 = the slot colour), so it
//! does not depend on which colours the slot holds. The hardware flips tiles for free, so a pattern and its X, Y and XY
//! mirrors are one tile (the canonical pattern is the smallest of the four). A two-slot (overlay) cell has two patterns,
//! one per sprite. Parts do not share tiles: a group's total is the sum of its parts' unique cells.

use crate::fsutil::PathSlash;
use crate::cells::{self, Placement, Resolved};
use crate::cli::{BudgetArgs, DedupeArgs};
use crate::color::{self, Rgb};
use crate::edit::{self, Op};
use crate::fit;
use crate::fsutil;
use crate::lease;
use crate::manifest::{Manifest, PartDecl, Scene};
use crate::model::Part;
use crate::output::{Out, PxlError, Result};
use crate::profile;
use crate::pxl;
use crate::status;
use crate::template::{self, Template};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

pub type Pattern = Vec<u8>;

/// Mirror a cell pattern (`fx`: left-right, `fy`: top-bottom).
pub fn flip(p: &[u8], cw: usize, ch: usize, fx: bool, fy: bool) -> Pattern {
    let mut o = vec![0u8; p.len()];
    for y in 0..ch {
        for x in 0..cw {
            let (sx, sy) = (if fx { cw - 1 - x } else { x }, if fy { ch - 1 - y } else { y });
            o[y * cw + x] = p[sy * cw + sx];
        }
    }
    o
}

const FLIPS: [(bool, bool); 4] = [(false, false), (true, false), (false, true), (true, true)];

/// Canonical pattern and the flip index (into FLIPS) that turns `p` into it.
pub fn canonical(p: &[u8], cw: usize, ch: usize) -> (Pattern, usize) {
    let mut best: Option<(Pattern, usize)> = None;
    for (i, (fx, fy)) in FLIPS.iter().enumerate() {
        let f = flip(p, cw, ch, *fx, *fy);
        if best.as_ref().map_or(true, |b| f < b.0) {
            best = Some((f, i));
        }
    }
    best.unwrap()
}

/// One sprite of a frame: its cell and pattern.
#[derive(Clone, Debug)]
pub struct Sprite {
    pub fi: usize,
    pub x: i32,
    pub y: i32,
    pub slot: u8,
    /// True for the second sprite of a two-slot cell.
    pub overlay: bool,
    pub pat: Pattern,
    /// A single-slot cell (can be rewritten by `dedupe`).
    pub movable: bool,
}

/// Patterns of every sprite of a part (all frames) from the best placement of each frame.
pub fn part_sprites(part: &Part, tpl: &Template, res: &Resolved, step: u32) -> std::result::Result<(Vec<Sprite>, Vec<Placement>), String> {
    part_sprites_with(part, tpl, res, step, true)
}

/// `strict` false: invalid cells are indexed as far as their slot goes (pixels outside it count as empty).
pub fn part_sprites_with(part: &Part, tpl: &Template, res: &Resolved, step: u32, strict: bool) -> std::result::Result<(Vec<Sprite>, Vec<Placement>), String> {
    let (cw, ch) = res.cell;
    let mut out = Vec::new();
    let mut pls = Vec::new();
    for fi in 0..part.frames.len() {
        let f = &part.frames[fi];
        let pix = cells::frame_pix(part, tpl, res, fi, step);
        let pl = cells::place_frame(part, res, &pix, fi)?;
        if strict && (pl.invalid > 0 || !pl.uncovered.is_empty()) {
            return Err(format!("frame '{}' has {} invalid cell(s); run `pxl lint` / `pxl cells`", f.name, pl.invalid.max(1)));
        }
        for (ci, c) in pl.cells.iter().enumerate() {
            let si = res.slot_index(c.slot).unwrap();
            let s2 = c.slot2.and_then(|s| res.slot_index(s));
            let mut p1 = vec![0u8; cw * ch];
            let mut p2 = vec![0u8; cw * ch];
            for dy in 0..ch {
                for dx in 0..cw {
                    let (x, y) = (c.x + dx as i32, c.y + dy as i32);
                    if x < 0 || y < 0 || x >= pix.w as i32 || y >= pix.h as i32 {
                        continue;
                    }
                    let pi = y as usize * pix.w + x as usize;
                    if pl.owner[pi] != ci as i32 {
                        continue;
                    }
                    let Some(col) = pix.rgb[pi] else { continue };
                    if let Some(k) = res.slots[si].2.iter().position(|k| *k == col) {
                        p1[dy * cw + dx] = k as u8 + 1;
                    } else if let Some(k) = s2.and_then(|j| res.slots[j].2.iter().position(|k| *k == col)) {
                        p2[dy * cw + dx] = k as u8 + 1;
                    }
                }
            }
            // overlapping authored cells share pixels: merging by rewriting a cell would damage its neighbour
            let movable = c.slot2.is_none() && pl.overlap_px == 0;
            if p1.iter().any(|v| *v != 0) || s2.is_none() {
                out.push(Sprite { fi, x: c.x, y: c.y, slot: c.slot, overlay: false, pat: p1, movable });
            }
            if let Some(j) = s2 {
                if p2.iter().any(|v| *v != 0) {
                    out.push(Sprite { fi, x: c.x, y: c.y, slot: res.slots[j].0, overlay: true, pat: p2, movable: false });
                }
            }
        }
        pls.push(pl);
    }
    Ok((out, pls))
}

/// Memory use of one part.
#[derive(Clone, Debug, Default)]
pub struct PartMem {
    pub name: String,
    pub path: String,
    pub sprites: usize,
    pub unique: usize,
    /// OBJ slots the cells of the part use (both slots of a two-slot cell).
    pub used_slots: BTreeSet<u8>,
    /// Cells no single slot can hold (their patterns are approximate).
    pub invalid: usize,
    /// Largest number of sprites in one frame / of unique patterns in one frame, with that frame's name.
    pub max_frame_sprites: usize,
    pub max_frame_unique: usize,
    pub max_frame_name: String,
    /// Most unique cells that change between two consecutive frames (what a streamed part uploads per frame).
    pub dma_cells: usize,
}

pub fn part_mem(path: &Path, name: &str) -> std::result::Result<PartMem, String> {
    let part = pxl::load(path).map_err(|e| e.msg)?;
    if part.palette_set.is_none() {
        return Err("no palette_set (run `pxl palette fit PART --set NAME --apply`)".into());
    }
    let prof = profile::load_for(&part, None).map_err(|e| e.msg)?;
    let res = cells::resolve(&part, &prof).map_err(|e| e.msg)?;
    let tpl = template::load_for(&part).map_err(|e| e.msg)?;
    let (sp, pls) = part_sprites_with(&part, &tpl, &res, prof.channel_step, false)?;
    let (cw, ch) = res.cell;
    let mut all: BTreeSet<Pattern> = BTreeSet::new();
    let mut per_frame: BTreeMap<usize, (usize, BTreeSet<Pattern>)> = BTreeMap::new();
    for s in &sp {
        let (c, _) = canonical(&s.pat, cw, ch);
        all.insert(c.clone());
        let e = per_frame.entry(s.fi).or_default();
        e.0 += 1;
        e.1.insert(c);
    }
    let mut m = PartMem { name: name.to_string(), path: path.slash(), sprites: sp.len(), unique: all.len(), invalid: pls.iter().map(|p| p.invalid).sum(), used_slots: pls.iter().flat_map(|p| p.cells.iter().flat_map(|c| [Some(c.slot), c.slot2].into_iter().flatten())).collect(), ..Default::default() };
    let mut prev: Option<&BTreeSet<Pattern>> = None;
    for (_, (_, u)) in &per_frame {
        if let Some(p) = prev {
            m.dma_cells = m.dma_cells.max(u.difference(p).count());
        }
        prev = Some(u);
    }
    for (fi, (n, u)) in &per_frame {
        if u.len() > m.max_frame_unique || m.max_frame_name.is_empty() {
            m.max_frame_unique = u.len();
            m.max_frame_name = part.frames[*fi].name.clone();
        }
        m.max_frame_sprites = m.max_frame_sprites.max(*n);
    }
    Ok(m)
}

// ------------------------------------------------------------------------------------------------ scenes

fn glob(p: &[u8], s: &[u8]) -> bool {
    match p.split_first() {
        None => s.is_empty(),
        Some((b'*', rest)) => (0..=s.len()).any(|i| glob(rest, &s[i..])),
        Some((c, rest)) => s.first() == Some(c) && glob(rest, &s[1..]),
    }
}

/// Manifest parts a `parts` entry names: part key, path, folder prefix or glob (`*` crosses folders).
pub(crate) fn resolve_parts<'a>(m: &'a Manifest, pat: &str) -> Vec<&'a PartDecl> {
    let pat = pat.trim_end_matches('/');
    m.parts
        .iter()
        .filter(|d| {
            let rel = d.path.strip_prefix(&m.dir).unwrap_or(&d.path).to_string_lossy().replace('\\', "/");
            d.name == pat || rel == pat || rel.starts_with(&format!("{}/", pat)) || (pat.contains('*') && glob(pat.as_bytes(), rel.as_bytes()))
        })
        .collect()
}

#[derive(Clone, Debug)]
pub struct GroupReport {
    pub name: String,
    pub budget: Option<u32>,
    pub streamed: bool,
    pub per_frame: Option<u32>,
    /// Resident groups: sum of the parts' unique cells. Streamed groups: the largest per-frame unique count.
    pub total: usize,
    pub parts: Vec<PartMem>,
    pub problems: Vec<String>,
    pub max_part: String,
}

impl GroupReport {
    pub fn limit(&self) -> u32 {
        if self.streamed { self.per_frame.unwrap_or(0) } else { self.budget.unwrap_or(0) }
    }
    pub fn over(&self) -> bool {
        self.total as u32 > self.limit() || !self.problems.is_empty()
    }
}

/// Why a part's palette set does not fit a scene's set (`None` = fits): every slot the part can use must hold the same
/// colours in the scene's set. A part on `global` fits any set that includes it.
pub fn set_mismatch(part_path: &Path, scene_set: &str, used: Option<&BTreeSet<u8>>) -> Option<String> {
    let part = pxl::load(part_path).ok()?;
    let sets = part.palette_sets();
    if sets.is_empty() {
        return None;
    }
    // a multi-set part fits a scene when one of its sets fits (it is recoloured by the scene's set at runtime)
    let mut first = None;
    for name in sets {
        match one_set_mismatch(&part, name, scene_set, used) {
            None => return None,
            Some(m) => first = first.or(Some(m)),
        }
    }
    first
}

fn one_set_mismatch(part: &Part, name: &str, scene_set: &str, used: Option<&BTreeSet<u8>>) -> Option<String> {
    if name == scene_set {
        return None;
    }
    let dir = part.dir();
    let (ps, ss) = match (cells::load_set(name, &dir), cells::load_set(scene_set, &dir)) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => return Some(e.msg),
    };
    for slot in 0..8usize {
        if !part.slots.is_empty() && !part.slots.contains(&(slot as u8)) {
            continue;
        }
        // only the slots the part's cells use have to agree with the scene
        if used.map_or(false, |u| !u.contains(&(slot as u8))) {
            continue;
        }
        let Some(a) = &ps.slots[slot] else { continue };
        match &ss.slots[slot] {
            Some(b) if a.colors == b.colors && a.variants == b.variants => {}
            Some(_) => return Some(format!("palette_set '{}' slot {} differs from the scene's set '{}'", name, slot, scene_set)),
            None => return Some(format!("palette_set '{}' uses slot {} which the scene's set '{}' does not define", name, slot, scene_set)),
        }
    }
    None
}

pub fn scene_report(m: &Manifest, sc: &Scene, cache: &mut HashMap<String, std::result::Result<PartMem, String>>) -> Vec<GroupReport> {
    let mut out = Vec::new();
    for g in &sc.groups {
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut r = GroupReport { name: g.name.clone(), budget: g.budget, streamed: g.streamed, per_frame: g.per_frame, total: 0, parts: Vec::new(), problems: Vec::new(), max_part: String::new() };
        for pat in &g.parts {
            let hit = resolve_parts(m, pat);
            if hit.is_empty() {
                r.problems.push(format!("'{}' matches no part of the manifest", pat));
            }
            for d in hit {
                if !seen.insert(d.name.clone()) {
                    continue;
                }
                let key = d.path.slash();
                let res = cache.entry(key).or_insert_with(|| part_mem(&d.path, &d.name)).clone();
                match res {
                    Ok(pm) => {
                        if let Some(sset) = sc.palette_set.as_ref() {
                            if let Some(why) = set_mismatch(&d.path, sset, Some(&pm.used_slots)) {
                                r.problems.push(format!("{}: {}", d.name, why));
                            }
                        }
                        if pm.invalid > 0 {
                            r.problems.push(format!("{}: {} cell(s) need more than one slot (counts are approximate; see `pxl cells`)", d.name, pm.invalid));
                        }
                        r.parts.push(pm)
                    }
                    Err(e) => r.problems.push(format!("{}: {}", d.name, e)),
                }
            }
        }
        if g.streamed {
            let best = r.parts.iter().max_by_key(|p| p.max_frame_unique);
            r.total = best.map_or(0, |p| p.max_frame_unique);
            r.max_part = best.map(|p| format!("{} frame {}", p.name, p.max_frame_name)).unwrap_or_default();
        } else {
            r.total = r.parts.iter().map(|p| p.unique).sum();
        }
        out.push(r);
    }
    out
}

/// `tile_budget` findings of every scene: (scene, group, message). Used by `check`.
pub fn scene_issues(m: &Manifest) -> Vec<(String, String, String)> {
    let mut v = Vec::new();
    if !crate::scene::shared_palette_model(m) {
        return v;
    }
    let mut cache = HashMap::new();
    for sc in &m.scenes {
        for g in scene_report(m, sc, &mut cache) {
            if g.total as u32 > g.limit() {
                let what = if g.streamed { format!("{} unique cells in one frame ({})", g.total, g.max_part) } else { format!("{} unique cells", g.total) };
                v.push((sc.name.clone(), g.name.clone(), format!("scene {} group {}: {} against {} {}", sc.name, g.name, what, g.limit(), if g.streamed { "per frame" } else { "budget" })));
            }
            for p in &g.problems {
                v.push((sc.name.clone(), g.name.clone(), format!("scene {} group {}: {}", sc.name, g.name, p)));
            }
        }
    }
    v
}

pub fn run_budget(a: &BudgetArgs, out: &mut Out) -> Result<()> {
    let m = Manifest::discover(a.manifest.as_deref())?.ok_or_else(|| PxlError::usage("`budget` needs a pxl.toml with [scene.NAME] tables").with_hint("run it from the project folder or pass --manifest"))?;
    if m.scenes.is_empty() {
        return Err(PxlError::usage(format!("{} declares no [scene.NAME]", m.path.slash())).with_hint("[scene.zone_2]\npalette_set = \"zone_2\"\ngroups = { common = { budget = 56, parts = [\"parts/items\"] } }"));
    }
    let scenes: Vec<&Scene> = match (&a.scene, a.all) {
        (Some(n), _) => vec![m.scenes.iter().find(|s| &s.name == n).ok_or_else(|| PxlError::usage(format!("no scene '{}'", n)).with_hint(format!("scenes: {}", m.scenes.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", "))))?],
        (None, true) => m.scenes.iter().collect(),
        (None, false) if m.scenes.len() == 1 => vec![&m.scenes[0]],
        _ => return Err(PxlError::usage("give a scene or --all").with_hint(format!("scenes: {}", m.scenes.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", ")))),
    };
    if !a.what_if.is_empty() {
        return crate::plan::budget_what_if(a, &m, out);
    }
    let mut cache = HashMap::new();
    let mut js = Vec::new();
    let mut over = 0;
    for sc in scenes {
        out.line(format!("scene {}{}", sc.name, sc.palette_set.as_ref().map(|s| format!(" (palette_set {})", s)).unwrap_or_default()));
        let mut jg = Vec::new();
        let (mut resident, mut resident_budget) = (0usize, 0u32);
        for g in scene_report(&m, sc, &mut cache) {
            let st = if g.total as u32 > g.limit() || !g.problems.is_empty() { "OVER" } else { "ok" };
            if st == "OVER" {
                over += 1;
            }
            if g.streamed {
                out.line(format!("  {:<10} streamed: at most {} unique cells in one frame, cap {} [{}]{}", g.name, g.total, g.limit(), st, if g.max_part.is_empty() { String::new() } else { format!(" ({})", g.max_part) }));
            } else {
                resident += g.total;
                resident_budget += g.limit();
                out.line(format!("  {:<10} {} / {} unique cells [{}]{}", g.name, g.total, g.limit(), st, if g.total as u32 > g.limit() { format!(" over by {}", g.total as u32 - g.limit()) } else { String::new() }));
            }
            let mut ps = g.parts.clone();
            ps.sort_by(|x, y| if g.streamed { y.max_frame_unique.cmp(&x.max_frame_unique) } else { y.unique.cmp(&x.unique) }.then(x.name.cmp(&y.name)));
            let top: Vec<String> = ps.iter().take(a.top).map(|p| format!("{} {}", p.name, if g.streamed { p.max_frame_unique } else { p.unique })).collect();
            if !top.is_empty() {
                out.line(format!("    biggest: {}", top.join(", ")));
            }
            for p in &g.problems {
                out.line(format!("    problem: {}", p));
            }
            jg.push(json!({"group": g.name, "streamed": g.streamed, "budget": g.budget, "per_frame": g.per_frame, "total": g.total, "limit": g.limit(), "over": g.total as u32 > g.limit(), "problems": g.problems,
                "parts": ps.iter().map(|p| json!({"part": p.name, "unique": p.unique, "sprites": p.sprites, "max_frame_unique": p.max_frame_unique, "max_frame_sprites": p.max_frame_sprites, "max_frame": p.max_frame_name})).collect::<Vec<_>>()}));
        }
        out.line(format!("  resident total {} / {}", resident, resident_budget));
        js.push(json!({"scene": sc.name, "palette_set": sc.palette_set, "resident_total": resident, "resident_budget": resident_budget, "groups": jg}));
    }
    out.set("schema", json!(1));
    out.set("scenes", Value::Array(js));
    if over > 0 {
        return Err(PxlError::lint(format!("{} group(s) over the sprite tile budget", over)));
    }
    Ok(())
}

// ------------------------------------------------------------------------------------------------ dedupe

struct Group {
    canon: Pattern,
    members: Vec<usize>, // indexes into sprites, with their flip to canon in `to_canon`
    movable: bool,
    /// Slot colours used to weigh differences (slot of the first member).
    cols: [Rgb; 3],
    alive: bool,
    /// merged into (group, flip index of the target pattern that matches this canon)
    into: Option<(usize, usize)>,
}

fn px_cost(a: &[u8], b: &[u8], cols: &[Rgb; 3]) -> (usize, f64) {
    let lab: Vec<color::Oklab> = cols.iter().map(|c| color::srgb_to_oklab(*c)).collect();
    let (mut n, mut c) = (0usize, 0.0f64);
    for (x, y) in a.iter().zip(b) {
        if x != y {
            n += 1;
            c += if *x == 0 || *y == 0 { 100.0 } else { color::oklab_dist2(lab[*x as usize - 1], lab[*y as usize - 1]).sqrt() * 100.0 };
        }
    }
    (n, c)
}

pub fn run_dedupe(a: &DedupeArgs, out: &mut Out) -> Result<()> {
    let path = fsutil::resolve_part(&a.part)?;
    let part = pxl::load(&path)?;
    if part.palette_set.is_none() {
        return Err(PxlError::usage(format!("{} has no palette_set", path.slash())));
    }
    let prof = profile::load_for(&part, None)?;
    let res = cells::resolve(&part, &prof)?;
    let tpl = template::load_for(&part)?;
    let step = prof.channel_step;
    let (cw, ch) = res.cell;
    let (sprites, pls) = part_sprites(&part, &tpl, &res, step).map_err(PxlError::validation)?;
    // groups by canonical pattern
    let mut idx: BTreeMap<Pattern, usize> = BTreeMap::new();
    let mut groups: Vec<Group> = Vec::new();
    let mut to_canon: Vec<usize> = Vec::new();
    for (i, s) in sprites.iter().enumerate() {
        let (c, f) = canonical(&s.pat, cw, ch);
        to_canon.push(f);
        let gi = *idx.entry(c.clone()).or_insert_with(|| {
            let si = res.slot_index(s.slot).unwrap();
            groups.push(Group { canon: c, members: Vec::new(), movable: true, cols: res.slots[si].2, alive: true, into: None });
            groups.len() - 1
        });
        groups[gi].members.push(i);
        groups[gi].movable &= s.movable;
    }
    let before = groups.len();
    out.line(format!("{}: {} sprites, {} unique cells (flips merged), target {}", part.name, sprites.len(), before, a.max));
    // pairwise cost of merging group i into j: (px, dE, flip of j matching i)
    let n = groups.len();
    let mut pair: Vec<Vec<Option<(usize, f64, usize)>>> = vec![vec![None; n]; n];
    if before > a.max {
        for i in 0..n {
            if !groups[i].movable {
                continue;
            }
            for j in 0..n {
                if i == j {
                    continue;
                }
                let mut best: Option<(usize, f64, usize)> = None;
                for (fi, (fx, fy)) in FLIPS.iter().enumerate() {
                    let t = flip(&groups[j].canon, cw, ch, *fx, *fy);
                    let (px, de) = px_cost(&groups[i].canon, &t, &groups[i].cols);
                    if best.map_or(true, |b| (de, px) < (b.1, b.0)) {
                        best = Some((px, de, fi));
                    }
                }
                pair[i][j] = best;
            }
        }
    }
    let mut merges: Vec<(usize, usize, usize, f64, usize, usize)> = Vec::new(); // from, to, px, de, flip, uses
    let mut alive = n;
    let usage: Vec<usize> = groups.iter().map(|g| g.members.len()).collect();
    let mut use_of = usage.clone();
    let canvas_ok = |g: &Group, t: &Pattern, tflip: usize| -> bool {
        // every member must keep its out-of-canvas pixels transparent
        let (fx, fy) = FLIPS[tflip];
        let tt = flip(t, cw, ch, fx, fy);
        g.members.iter().all(|&mi| {
            let s = &sprites[mi];
            let (gx, gy) = FLIPS[to_canon[mi]];
            let np = flip(&tt, cw, ch, gx, gy);
            let (w, h) = (part.frames[s.fi].grid.w as i32, part.frames[s.fi].grid.h as i32);
            (0..ch).all(|dy| (0..cw).all(|dx| {
                let (x, y) = (s.x + dx as i32, s.y + dy as i32);
                (x >= 0 && y >= 0 && x < w && y < h) || np[dy * cw + dx] == 0
            }))
        })
    };
    while alive > a.max {
        let mut pick: Option<(f64, usize, usize)> = None; // cost, i, j
        for i in 0..n {
            if !groups[i].alive || !groups[i].movable {
                continue;
            }
            for j in 0..n {
                if j == i || !groups[j].alive {
                    continue;
                }
                let Some((px, de, _)) = pair[i][j] else { continue };
                let cost = de * use_of[i] as f64 + px as f64 * 1e-6;
                // prefer the target already used most on equal cost
                if pick.map_or(true, |p| cost < p.0 - 1e-9 || ((cost - p.0).abs() <= 1e-9 && use_of[j] > use_of[p.2])) {
                    pick = Some((cost, i, j));
                }
            }
        }
        let Some((_, i, j)) = pick else { break };
        let (px, de, fl) = pair[i][j].unwrap();
        if !canvas_ok(&groups[i], &groups[j].canon, fl) {
            pair[i][j] = None;
            continue;
        }
        groups[i].alive = false;
        groups[i].into = Some((j, fl));
        use_of[j] += use_of[i];
        merges.push((i, j, px, de, fl, use_of[i]));
        alive -= 1;
    }
    // resolve chains to the final target and the total flip
    let root = |mut g: usize| -> (usize, usize) {
        let mut f = 0usize;
        while let Some((t, fl)) = groups[g].into {
            f ^= fl;
            g = t;
        }
        (g, f)
    };
    let loc_of = |gi: usize| -> String {
        let s = &sprites[groups[gi].members[0]];
        format!("{}:{},{}", part.frames[s.fi].name, s.x - part.origin.0, s.y - part.origin.1)
    };
    let mut jm = Vec::new();
    for (k, (i, _j, px, de, _fl, uses)) in merges.iter().enumerate() {
        let (r, _) = root(*i);
        let line = format!("  merge {}: {} ({} cell(s)) -> {} : {} px differ, summed dE {:.0}", k + 1, loc_of(*i), uses, loc_of(r), px, de);
        if k < 25 {
            out.line(line);
        }
        jm.push(json!({"from": loc_of(*i), "into": loc_of(r), "cells": uses, "px_diff": px, "de": de}));
    }
    if merges.len() > 25 {
        out.line(format!("  ... {} more merges (see --json)", merges.len() - 25));
    }
    out.line(format!("unique cells: {} -> {}{}", before, alive, if alive > a.max { format!(" (cannot reach {}: the rest are overlay or edge cells)", a.max) } else { String::new() }));
    out.set("schema", json!(1));
    out.set("dedupe", json!({"before": before, "after": alive, "max": a.max, "applied": a.apply, "merges": jm}));
    if !a.apply || merges.is_empty() {
        if !a.apply && !merges.is_empty() {
            out.line("dry run: nothing written, add --apply");
        }
        return Ok(());
    }
    // new pixels per member cell
    let mut needed: BTreeSet<Rgb> = BTreeSet::new();
    let mut edits: Vec<(usize, usize, Pattern)> = Vec::new(); // sprite, group root, new pattern
    for (gi, g) in groups.iter().enumerate() {
        if g.alive {
            continue;
        }
        let (r, tf) = root(gi);
        for &mi in &g.members {
            let (gx, gy) = FLIPS[to_canon[mi]];
            let (tx, ty) = FLIPS[tf];
            let np = flip(&flip(&groups[r].canon, cw, ch, tx, ty), cw, ch, gx, gy);
            let si = res.slot_index(sprites[mi].slot).unwrap();
            for v in &np {
                if *v != 0 {
                    needed.insert(res.slots[si].2[*v as usize - 1]);
                }
            }
            edits.push((mi, r, np));
        }
    }
    status::refuse_if_approved(&path, a.force_approved)?;
    lease::guard(&path, out.agent())?;
    let (ext, kmap, added) = fit::ensure_keys(&part, &tpl, &needed, step)?;
    let _ = &ext;
    let mut grids: BTreeMap<usize, crate::model::Grid> = BTreeMap::new();
    for (mi, _r, np) in &edits {
        let s = &sprites[*mi];
        let g = grids.entry(s.fi).or_insert_with(|| part.frames[s.fi].grid.clone());
        let si = res.slot_index(s.slot).unwrap();
        for dy in 0..ch {
            for dx in 0..cw {
                let (x, y) = (s.x + dx as i32, s.y + dy as i32);
                if !g.in_bounds(x, y) {
                    continue;
                }
                let v = np[dy * cw + dx];
                g.cells[y as usize * g.w + x as usize] = if v == 0 { b'.' } else { kmap[&res.slots[si].2[v as usize - 1]] };
            }
        }
    }
    fit::write_template_keys(&tpl.path, &added)?;
    // freeze the layout of the changed frames
    for fi in grids.keys() {
        let sp = cells::sidecar_path(&part, &part.frames[*fi].name);
        fsutil::atomic_write(&sp, cells::cells_json_text(&part, &pls[*fi]).as_bytes())?;
    }
    let ops: Vec<Op> = grids.iter().map(|(fi, g)| Op::WriteGrid { frame: part.frames[*fi].name.clone(), grid: g.to_lines().join("\n") }).collect();
    let flags = crate::cli::EditFlags { no_lock: true, force_approved: a.force_approved, no_ref: true, ..Default::default() };
    edit::execute(&a.part, &ops, &flags, "cells dedupe", out)?;
    // verify
    let after = pxl::load(&path)?;
    let tpl2 = template::load_for(&after)?;
    match part_sprites(&after, &tpl2, &res, step) {
        Ok((sp2, _)) => {
            let set: BTreeSet<Pattern> = sp2.iter().map(|s| canonical(&s.pat, cw, ch).0).collect();
            out.line(format!("after: {} unique cells ({} authored cells.json files written to freeze the layout)", set.len(), grids.len()));
        }
        Err(e) => out.warn(format!("after the merge: {}", e)),
    }
    Ok(())
}
