//! Plan before pixels (M6 R6a): `pxl plan`, `pxl plan accept`, `pxl palette plan`, and the what-if / reservation tools of `pxl budget`.
//!
//! * `pxl plan` reads every asset of the project (their first versions), proposes scenes (from the groups, or evaluates the `[scene.NAME]`
//!   tables that exist), measures sprite cells per group, sprite VRAM, the OAM worst case, BG tiles of a linked tile set and a shared
//!   palette plan, and says whether each scene fits *before anyone draws*. A scene that does not fit lists its options with their costs
//!   (cut, rebalance, BG conversion, per-act split). Objects that probably need a BG version are flagged as dual-use candidates.
//! * `pxl plan --apply` writes the proposed scenes into pxl.toml, builds the shared palette sets and reduces the assets into them
//!   (`palette plan --apply` per scene), and records `plan.toml` with state `proposed`. `pxl plan accept` signs it off; until then `next`
//!   and `claim` refuse in a project that has a plan (or whose `[project] plan = "required"`).
//! * `pxl palette plan SCENE` builds one palette set for all inputs of a scene with the learner of the first version, measures the loss
//!   per part and the identity colours lost, keeps reserved slots out of the plan, aligns materials to the slots of another set, and with
//!   `--what-if` (or without `--apply`) writes nothing.
//! * `pxl budget SCENE --what-if "remove PART" --what-if "add PART GROUP" ...` shows the budget rows before and after. Reservations
//!   (`[scene.X.reserve]`) hold slack back for the other producer; see `manifest::Reserve`.

use crate::fsutil::PathSlash;
use crate::autoset::{self, CellData};
use crate::cellmem::{self, PartMem};
use crate::cli::{BudgetArgs, FitArgs, FrameSel, PalettePlanArgs, PlanArgs};
use crate::color::{self, Rgb};
use crate::fsutil;
use crate::manifest::{Manifest, PartDecl, Scene, SceneGroup};
use crate::model::{Part, PartKind};
use crate::output::{Out, PxlError, Result};
use crate::profile::Profile;
use crate::scene;
use crate::{cells, pxl, template};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// dE (Oklab x 100) above which a colour counts as lost.
const LOST: f64 = 10.0;
type Cache = HashMap<String, std::result::Result<PartMem, String>>;

// ------------------------------------------------------------------------------------------------ what-if

fn parse_u32(s: &str, what: &str) -> Result<u32> {
    s.trim().parse::<u32>().map_err(|_| PxlError::usage(format!("what-if: {} must be a non-negative number, got '{}'", what, s)))
}

/// A copy of the scene with the what-if operations applied (nothing is written):
/// `remove PART`, `add PART GROUP`, `budget GROUP N`, `reserve GROUP|sprite_cells|bg_tiles N`, `resident N`.
pub fn apply_what_if(m: &Manifest, sc: &Scene, ops: &[String]) -> Result<Scene> {
    let mut s = sc.clone();
    for op in ops {
        let w: Vec<&str> = op.split_whitespace().collect();
        match w.as_slice() {
            ["remove", part] => {
                let gone: BTreeSet<String> = cellmem::resolve_parts(m, part).into_iter().map(|d| d.name.clone()).collect();
                if gone.is_empty() {
                    return Err(PxlError::usage(format!("what-if: '{}' matches no part", part)));
                }
                for g in &mut s.groups {
                    let names: Vec<String> = g.parts.iter().flat_map(|p| cellmem::resolve_parts(m, p).into_iter().map(|d| d.name.clone())).collect();
                    if names.iter().any(|n| gone.contains(n)) {
                        g.parts = names.into_iter().filter(|n| !gone.contains(n)).collect();
                    }
                }
            }
            ["add", part, group] => {
                if cellmem::resolve_parts(m, part).is_empty() {
                    return Err(PxlError::usage(format!("what-if: '{}' matches no part", part)));
                }
                let g = s.groups.iter_mut().find(|g| g.name == *group).ok_or_else(|| PxlError::usage(format!("what-if: scene {} has no group '{}'", s.name, group)))?;
                g.parts.push(part.to_string());
            }
            ["budget", group, n] => {
                let n = parse_u32(n, "budget")?;
                let g = s.groups.iter_mut().find(|g| g.name == *group).ok_or_else(|| PxlError::usage(format!("what-if: scene {} has no group '{}'", s.name, group)))?;
                if g.streamed {
                    g.per_frame = Some(n)
                } else {
                    g.budget = Some(n)
                }
            }
            ["reserve", what, n] => {
                let n = parse_u32(n, "reserve")?;
                match *what {
                    "sprite_cells" => s.reserve.sprite_cells = n,
                    "bg_tiles" => s.reserve.bg_tiles = n,
                    g if s.groups.iter().any(|x| x.name == g) => {
                        s.reserve.groups.insert(g.to_string(), n);
                    }
                    o => return Err(PxlError::usage(format!("what-if: reserve '{}' is neither sprite_cells, bg_tiles nor a group of scene {}", o, s.name))),
                }
            }
            ["resident", n] => s.resident_budget = Some(parse_u32(n, "resident")?),
            _ => {
                return Err(PxlError::usage(format!("what-if: cannot read '{}'", op)).with_hint("remove PART | add PART GROUP | budget GROUP N | reserve GROUP|sprite_cells|bg_tiles N | resident N"))
            }
        }
    }
    Ok(s)
}

/// The budget rows before and after the what-if operations. Returns whether the changed scene passes.
pub fn what_if_report(m: &Manifest, sc: &Scene, ops: &[String], cache: &mut Cache, out: &mut Out) -> Result<bool> {
    let (sp, tp) = scene::hardware(m)?;
    let alt = apply_what_if(m, sc, ops)?;
    let before = scene::check_scene(m, sc, &sp, &tp, cache);
    let after = scene::check_scene(m, &alt, &sp, &tp, cache);
    out.line(format!("what-if for scene {} ({}); nothing is written", sc.name, ops.join("; ")));
    let mut js = Vec::new();
    for r in &after.rows {
        let b = before.rows.iter().find(|x| x.item == r.item);
        let changed = b.map_or(true, |b| b.used != r.used || b.limit != r.limit || b.ok != r.ok);
        if changed || !r.ok {
            let was = b.map(|b| b.used.clone()).unwrap_or_else(|| "-".into());
            out.line(format!("  {:<22} {} -> {} / {}  {}", r.item, was, r.used, r.limit, if r.ok { "ok" } else { "OVER" }));
        }
        js.push(json!({"item": r.item, "before": b.map(|b| b.used.clone()), "after": r.used, "limit": r.limit, "ok": r.ok, "was_ok": b.map(|b| b.ok), "changed": changed}));
    }
    for r in &before.rows {
        if !after.rows.iter().any(|x| x.item == r.item) {
            out.line(format!("  {:<22} {} -> (gone)", r.item, r.used));
        }
    }
    out.line(format!("  result: {} (was {})", if after.ok() { "fits" } else { "OVER" }, if before.ok() { "fits" } else { "OVER" }));
    out.set("what_if", json!({"scene": sc.name, "ops": ops, "before_ok": before.ok(), "after_ok": after.ok(), "rows": js}));
    Ok(after.ok())
}

/// `pxl budget --what-if`: called from `run_budget`.
pub fn budget_what_if(a: &BudgetArgs, m: &Manifest, out: &mut Out) -> Result<()> {
    let sc = match (&a.scene, m.scenes.len()) {
        (Some(n), _) => m.scenes.iter().find(|s| &s.name == n),
        (None, 1) => m.scenes.first(),
        _ => None,
    }
    .ok_or_else(|| PxlError::usage("--what-if needs one scene").with_hint(format!("scenes: {}", m.scenes.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", "))))?;
    let mut cache = HashMap::new();
    if what_if_report(m, sc, &a.what_if, &mut cache, out)? {
        Ok(())
    } else {
        Err(PxlError::lint(format!("scene {} would exceed a shared budget", sc.name)))
    }
}

// ------------------------------------------------------------------------------------------------ palette plan

#[derive(Clone, Debug)]
pub struct SlotPlan {
    pub slot: u8,
    pub colors: [Rgb; 3],
    pub parts: Vec<String>,
    pub cells: usize,
    /// Slot of the aligned set this slot was matched to.
    pub aligned_to: Option<u8>,
    pub reserved: bool,
}

/// A two-slot (stacked) cell the plan proposes: a cell of a player part that needs 4 to 6 colours, drawn with two palette slots (2 sprites).
#[derive(Clone, Debug)]
pub struct Stacked {
    pub frame: String,
    /// Cell column and row on the canvas grid of the part.
    pub cell: (usize, usize),
    pub slots: (u8, u8),
    pub colours: usize,
    /// Mean dE of the cell's source pixels with one slot and with the pair.
    pub before_de: f64,
    pub after_de: f64,
}

/// The loss of one frame measured against its SOURCE image (the reduced art when the frame has none).
#[derive(Clone, Debug, Default)]
pub struct FrameLoss {
    pub frame: String,
    pub pixels: usize,
    pub mean_de: f64,
    pub max_de: f64,
    pub from_source: bool,
    /// Identity colours of this frame (source, kept as, mean dE) whose nearest colour in the plan is visibly different.
    pub lost: Vec<(Rgb, Rgb, f64)>,
}

#[derive(Clone, Debug, Default)]
pub struct PartLoss {
    pub part: String,
    pub pixels: usize,
    pub mean_de: f64,
    pub max_de: f64,
    pub slots: BTreeSet<u8>,
    /// Identity colours (source, kept as, dE) whose nearest colour in the plan is visibly different (the worst of every frame).
    pub lost: Vec<(Rgb, Rgb, f64)>,
    /// Per frame, against the source.
    pub frames: Vec<FrameLoss>,
    /// Stacked (two-slot) cells proposed for player parts.
    pub stacked: Vec<Stacked>,
    /// The part's category lets it use stacked cells (`player`).
    pub stack_allowed: bool,
    /// The part sits in the player group (or has the category) so stacked cells apply to it.
    pub is_player: bool,
}

#[derive(Clone, Debug, Default)]
pub struct PalettePlan {
    pub slots: Vec<SlotPlan>,
    pub losses: Vec<PartLoss>,
    pub skipped: Vec<String>,
    pub mean_de: f64,
    pub console: String,
}

impl PalettePlan {
    pub fn set_text(&self, scene: &str) -> String {
        let mut s = format!("# generated by `pxl palette plan {}`: one palette set for every input of the scene\nconsole = \"{}\"\n", scene, self.console);
        for sl in &self.slots {
            s.push_str(&format!("\n[[slots]]\nslot = {}\nname = \"{}\"\ncolors = [{}]\n", sl.slot, if sl.reserved { format!("reserved_{}", sl.slot) } else { format!("plan_{}", sl.slot) }, sl.colors.iter().map(|x| format!("\"{}\"", color::to_hex(*x))).collect::<Vec<_>>().join(", ")));
        }
        s
    }
    pub fn lost_total(&self) -> usize {
        self.losses.iter().map(|l| l.lost.len()).sum()
    }
    pub fn to_json(&self) -> Value {
        json!({
            "mean_de": r2(self.mean_de),
            "identity_lost": self.lost_total(),
            "slots": self.slots.iter().map(|s| json!({"slot": s.slot, "colors": s.colors.iter().map(|c| color::to_hex(*c)).collect::<Vec<_>>(), "parts": s.parts, "cells": s.cells, "aligned_to": s.aligned_to, "reserved": s.reserved})).collect::<Vec<_>>(),
            "parts": self.losses.iter().map(|l| json!({
                "part": l.part, "pixels": l.pixels, "mean_de": r2(l.mean_de), "max_de": r2(l.max_de), "slots": l.slots,
                "identity_lost": l.lost.iter().map(|(a, b, d)| json!({"source": color::to_hex(*a), "kept_as": color::to_hex(*b), "de": r2(*d)})).collect::<Vec<_>>(),
                "frames": l.frames.iter().map(|f| json!({"frame": f.frame, "against": if f.from_source { "source" } else { "reduced art" }, "pixels": f.pixels, "mean_de": r2(f.mean_de), "max_de": r2(f.max_de),
                    "identity_lost": f.lost.iter().map(|(a, b, d)| json!({"source": color::to_hex(*a), "kept_as": color::to_hex(*b), "de": r2(*d)})).collect::<Vec<_>>()})).collect::<Vec<_>>(),
                "stacked_cells": l.stacked.iter().map(|c| json!({"frame": c.frame, "cell": [c.cell.0, c.cell.1], "slots": [c.slots.0, c.slots.1], "colours": c.colours, "before_de": r2(c.before_de), "after_de": r2(c.after_de)})).collect::<Vec<_>>(),
                "stacked_allowed": l.stack_allowed, "player": l.is_player,
            })).collect::<Vec<_>>(),
            "skipped": self.skipped,
        })
    }
    pub fn text(&self) -> String {
        let mut s = String::new();
        for sl in &self.slots {
            s.push_str(&format!(
                "  slot {}  {}{}  {} cell(s)  {}\n",
                sl.slot,
                sl.colors.iter().map(|c| color::to_hex(*c)).collect::<Vec<_>>().join(" "),
                if sl.reserved { " (reserved, kept)" } else { "" },
                sl.cells,
                if sl.parts.is_empty() { String::new() } else { format!("[{}]", sl.parts.join(", ")) }
            ));
            if let Some(a) = sl.aligned_to {
                s.push_str(&format!("          aligned with slot {} of the other set\n", a));
            }
        }
        s.push_str(&format!("  mean error dE {:.1}, identity colours lost: {}\n", self.mean_de, self.lost_total()));
        for l in &self.losses {
            let against = if l.frames.iter().all(|f| f.from_source) { "vs source" } else if l.frames.iter().any(|f| f.from_source) { "vs source (some frames: reduced art)" } else { "vs reduced art (no source image)" };
            s.push_str(&format!("  {:<18} dE {:>4.1} (max {:>4.1}) {} slots {:?}", l.part, l.mean_de, l.max_de, against, l.slots.iter().collect::<Vec<_>>()));
            if !l.lost.is_empty() {
                s.push_str(&format!("  LOST: {}", l.lost.iter().map(|(a, b, d)| lost_text(*a, *b, *d)).collect::<Vec<_>>().join(", ")));
            }
            s.push('\n');
            for f in l.frames.iter().filter(|f| !f.lost.is_empty()) {
                s.push_str(&format!("      {} {}: dE {:.1}; loses {}\n", l.part, f.frame, f.mean_de, f.lost.iter().map(|(a, b, d)| lost_text(*a, *b, *d)).collect::<Vec<_>>().join(", ")));
            }
            if !l.stacked.is_empty() {
                let frames: BTreeSet<&str> = l.stacked.iter().map(|c| c.frame.as_str()).collect();
                s.push_str(&format!(
                    "      {}: {} stacked (two-slot) cell(s) proposed in {} frame(s), 2 sprites each: {}{}\n",
                    l.part,
                    l.stacked.len(),
                    frames.len(),
                    l.stacked.iter().take(6).map(|c| format!("{} cell {},{} slots {}+{} ({} colours, dE {:.0} -> {:.0})", c.frame, c.cell.0, c.cell.1, c.slots.0, c.slots.1, c.colours, c.before_de, c.after_de)).collect::<Vec<_>>().join("; "),
                    if l.stack_allowed { String::new() } else { format!("; allowed once the category is player: `pxl asset set {} category player`", l.part) }
                ));
            }
        }
        for k in &self.skipped {
            s.push_str(&format!("  skipped {}\n", k));
        }
        s
    }
}

fn lost_text(src: Rgb, kept: Rgb, d: f64) -> String {
    if src == kept {
        format!("{} split over several colours (mean dE {:.0})", color::to_hex(src), d)
    } else {
        format!("{} -> {} (dE {:.0})", color::to_hex(src), color::to_hex(kept), d)
    }
}

fn r2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn de(a: Rgb, b: Rgb) -> f64 {
    color::oklab_dist2(color::srgb_to_oklab(a), color::srgb_to_oklab(b)).sqrt() * 100.0
}

fn chroma(c: Rgb) -> f64 {
    let o = color::srgb_to_oklab(c);
    (o.a * o.a + o.b * o.b).sqrt()
}

struct Loaded {
    name: String,
    part: Part,
    data: CellData,
    rgb: BTreeMap<u8, Rgb>,
    /// The source image of every frame (None: the frame has none, the reduced art is measured instead).
    sources: Vec<Option<autoset::SourceFrame>>,
    /// Identity colours of the asset (brief.toml list, else detected on the sources).
    identity: Vec<Rgb>,
    /// The part is a player part (category player, or it sits in the player group).
    is_player: bool,
}

fn source_frames(part: &Part) -> Vec<Option<autoset::SourceFrame>> {
    let dir = std::fs::canonicalize(part.dir()).unwrap_or_else(|_| part.dir());
    part.frames
        .iter()
        .map(|f| {
            let rf = crate::load::reference(part, &dir, f).ok()?;
            let g = crate::features::Geo::of(part, &rf)?;
            Some(autoset::SourceFrame { img: rf.img, f: g.f, org: g.org })
        })
        .collect()
}

fn load_parts(m: &Manifest, parts: &[&PartDecl], prof: &Profile, skipped: &mut Vec<String>) -> Vec<Loaded> {
    let mut v = Vec::new();
    let defaults = crate::project::defaults_of(&m.path);
    for d in parts {
        let part = match pxl::load(&d.path) {
            Ok(p) => p,
            Err(e) => {
                skipped.push(format!("{}: {}", d.name, e.msg));
                continue;
            }
        };
        if part.kind != PartKind::Sprite {
            skipped.push(format!("{}: not a sprite part", d.name));
            continue;
        }
        let tpl = match template::load_for(&part) {
            Ok(t) => t,
            Err(e) => {
                skipped.push(format!("{}: {}", d.name, e.msg));
                continue;
            }
        };
        let rgb: BTreeMap<u8, Rgb> = (0..=255u8).filter_map(|k| crate::lint::effective_color(&part, &tpl, k).map(|c| (k, c))).collect();
        // measured on the source where there is one: the reduced art has already lost the colours a plan must keep
        let sources = source_frames(&part);
        let data = autoset::cell_data_source(&part, &|k| rgb.get(&k).copied(), prof.hw_cell, &|fi| sources[fi].as_ref().map(|s| autoset::SourceFrame { img: s.img.clone(), f: s.f, org: s.org }), &|c| prof.snap(c));
        if data.cells.is_empty() {
            skipped.push(format!("{}: no opaque pixels", d.name));
            continue;
        }
        let group = group_of(m, d);
        let is_player = part.category.as_deref() == Some("player") || (part.category.is_none() && defaults.category_for(&group).as_deref() == Some("player"));
        let identity = {
            let from_brief = crate::intake::identity_entry(&crate::intake::read_brief(&m.dir), &d.name).map(|e| e.1).unwrap_or_default();
            let v: Vec<Rgb> = from_brief.iter().filter_map(|c| crate::insight::parse_hex(c)).collect();
            if v.is_empty() {
                let all: Vec<Rgb> = sources.iter().flatten().flat_map(|sf| sf.img.px.iter().filter(|p| p.a >= 128).map(|p| Rgb { r: p.r, g: p.g, b: p.b })).collect();
                crate::insight::identity_colours(&all).into_iter().map(|c| c.0).collect()
            } else {
                v
            }
        };
        v.push(Loaded { name: d.name.clone(), part, data, rgb, sources, identity, is_player });
    }
    v
}

/// Slot distance of two colour triples (symmetric nearest-colour cost, the learner's metric).
fn triple_dist(a: &[Rgb; 3], b: &[Rgb; 3]) -> f64 {
    let one = |x: &[Rgb; 3], y: &[Rgb; 3]| x.iter().map(|c| y.iter().map(|d| autoset::metric(*c, *d)).fold(f64::MAX, f64::min)).sum::<f64>();
    one(a, b) + one(b, a)
}

pub struct PlanOpts {
    pub slots: Option<usize>,
    pub reserve: Vec<u8>,
    /// Existing set whose slot meanings the new plan lines up with, and the colours of reserved slots to keep.
    pub align: Option<cells::PaletteSet>,
    pub keep: Option<cells::PaletteSet>,
}

/// One palette set for all the parts: learned from the cells of every part together (a shared material gets one slot), reserved
/// slots left out (and kept from `keep`), slot numbers lined up with `align`; then the loss per part.
pub fn palette_plan(m: &Manifest, parts: &[&PartDecl], prof: &Profile, opts: &PlanOpts) -> Result<PalettePlan> {
    let mut plan = PalettePlan { console: m.console.clone().unwrap_or_else(|| "gbc".into()), ..Default::default() };
    let loaded = load_parts(m, parts, prof, &mut plan.skipped);
    if loaded.is_empty() {
        return Err(PxlError::validation("no sprite part with pixels to plan a palette for").with_hint("assets come from `pxl ingest`"));
    }
    let total_slots = prof.obj_palettes as usize;
    let numbers: Vec<u8> = (0..total_slots as u8).filter(|s| !opts.reserve.contains(s)).collect();
    let want = opts.slots.unwrap_or(numbers.len()).min(numbers.len()).max(1);
    let merged = autoset::merge_data(&loaded.iter().map(|l| &l.data).collect::<Vec<_>>());
    let learned = autoset::learn_slots(&merged, want, &|c, used| crate::suggest::nearest_free(prof, c, used));
    if learned.is_empty() {
        return Err(PxlError::validation("the inputs have no opaque pixels"));
    }
    // slot numbers: lined up with the other set where a learned slot matches one of its slots, the rest ascending
    let mut free: Vec<u8> = numbers.clone();
    let mut number: Vec<Option<(u8, Option<u8>)>> = vec![None; learned.len()];
    if let Some(al) = &opts.align {
        let mut pairs: Vec<(f64, usize, u8)> = Vec::new();
        for (i, l) in learned.iter().enumerate() {
            for (n, d) in al.slots.iter().enumerate().filter_map(|(n, d)| d.as_ref().filter(|d| d.colors.len() == 3).map(|d| (n as u8, d))) {
                if free.contains(&n) {
                    pairs.push((triple_dist(l, &[d.colors[0], d.colors[1], d.colors[2]]), i, n));
                }
            }
        }
        pairs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        for (d, i, n) in pairs {
            if d <= 0.12 && number[i].is_none() && free.contains(&n) {
                number[i] = Some((n, Some(n)));
                free.retain(|x| *x != n);
            }
        }
    }
    for n in number.iter_mut() {
        if n.is_none() {
            if free.is_empty() {
                return Err(PxlError::validation("more learned slots than free palette slots"));
            }
            *n = Some((free.remove(0), None));
        }
    }
    let mut slots: Vec<SlotPlan> = learned.iter().zip(&number).map(|(c, n)| SlotPlan { slot: n.unwrap().0, colors: *c, parts: Vec::new(), cells: 0, aligned_to: n.unwrap().1, reserved: false }).collect();
    // reserved slots keep the colours they hold in the current set
    if let Some(k) = &opts.keep {
        for &r in &opts.reserve {
            if let Some(d) = k.slots.get(r as usize).and_then(|d| d.as_ref()).filter(|d| d.colors.len() == 3) {
                slots.push(SlotPlan { slot: r, colors: [d.colors[0], d.colors[1], d.colors[2]], parts: Vec::new(), cells: 0, aligned_to: None, reserved: true });
            }
        }
    }
    slots.sort_by_key(|s| s.slot);
    // loss per part, per frame, against the SOURCE (the reduced art where a frame has no source image): every canvas cell takes the slot that
    // holds its colours best; cells of player parts that need 4 to 6 colours may take two slots (a stacked cell, 2 sprites)
    let mut total_sum = 0.0;
    let mut total_px = 0usize;
    let usable: Vec<usize> = (0..slots.len()).filter(|i| !slots[*i].reserved).collect();
    let multi_max = if prof.multi_slot_max > 0 { prof.multi_slot_max as usize } else { 0 };
    for l in &loaded {
        let (cw, ch) = (prof.hw_cell.0 as usize, prof.hw_cell.1 as usize);
        let mut loss = PartLoss { part: l.name.clone(), is_player: l.is_player, stack_allowed: l.part.category.as_deref() == Some("player") && multi_max > 0, ..Default::default() };
        // the slot(s) of every cell: single best first
        let cost_of = |n: usize, cols: &[Rgb]| -> f64 { l.data.cells[n].iter().map(|&(c, w)| w * cols.iter().map(|x| autoset::metric(l.data.palette[c], *x)).fold(f64::MAX, f64::min)).sum() };
        let mut assign: BTreeMap<(usize, usize, usize), Vec<usize>> = BTreeMap::new();
        let mut stacked_cand: Vec<(f64, usize, usize, usize, usize, usize, f64, f64)> = Vec::new(); // gain, frame, cx, cy, slot a, slot b, single, pair
        for (n, &(fi, cx, cy)) in l.data.pos.iter().enumerate() {
            let Some(&best) = usable.iter().min_by(|a, b| cost_of(n, &slots[**a].colors).partial_cmp(&cost_of(n, &slots[**b].colors)).unwrap_or(std::cmp::Ordering::Equal)) else { continue };
            assign.insert((fi, cx, cy), vec![best]);
            slots[best].cells += 1;
            if !slots[best].parts.contains(&l.name) {
                slots[best].parts.push(l.name.clone());
            }
            loss.slots.insert(slots[best].slot);
            if l.is_player && multi_max > 0 && usable.len() >= 2 {
                let total_w: f64 = l.data.cells[n].iter().map(|c| c.1).sum::<f64>().max(1e-9);
                let single = cost_of(n, &slots[best].colors) / total_w;
                // 4 to 6 colours: colours that hold at least 4 % of the cell
                let ncol = l.data.cells[n].iter().filter(|c| c.1 / total_w >= 0.04).count();
                if single > 0.004 && (4..=6).contains(&ncol) {
                    let mut bp: Option<(f64, usize)> = None;
                    for &o in usable.iter().filter(|o| **o != best) {
                        let cols: Vec<Rgb> = slots[best].colors.iter().chain(slots[o].colors.iter()).copied().collect();
                        let c = cost_of(n, &cols) / total_w;
                        if bp.map_or(true, |x| c < x.0 - 1e-12) {
                            bp = Some((c, o));
                        }
                    }
                    if let Some((pair, o)) = bp {
                        if pair <= 0.8 * single {
                            stacked_cand.push(((single - pair) * total_w, fi, cx, cy, best, o, single, pair));
                        }
                    }
                }
            }
        }
        // the most worthwhile stacked cells of every frame, at most `multi_slot_max`
        stacked_cand.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut per_frame_n: BTreeMap<usize, usize> = BTreeMap::new();
        for c in stacked_cand {
            let k = per_frame_n.entry(c.1).or_insert(0);
            if *k >= multi_max {
                continue;
            }
            *k += 1;
            let ncol = l.data.cells.get(l.data.pos.iter().position(|p| *p == (c.1, c.2, c.3)).unwrap_or(0)).map_or(0, |cs| {
                let tw: f64 = cs.iter().map(|x| x.1).sum::<f64>().max(1e-9);
                cs.iter().filter(|x| x.1 / tw >= 0.04).count()
            });
            assign.insert((c.1, c.2, c.3), vec![c.4, c.5]);
            loss.stacked.push(Stacked { frame: l.part.frames[c.1].name.clone(), cell: (c.2, c.3), slots: (slots[c.4].slot, slots[c.5].slot), colours: ncol, before_de: c.6.sqrt() * 100.0, after_de: c.7.sqrt() * 100.0 });
        }
        loss.stacked.sort_by(|a, b| a.frame.cmp(&b.frame).then(a.cell.cmp(&b.cell)));
        let mut sum = 0.0;
        let mut by_part_lost: BTreeMap<Rgb, (Rgb, f64)> = BTreeMap::new();
        for (fi, fr) in l.part.frames.iter().enumerate() {
            let mut fl = FrameLoss { frame: fr.name.clone(), from_source: l.sources[fi].is_some(), ..Default::default() };
            let mut by_src: BTreeMap<Rgb, (usize, BTreeMap<Rgb, usize>, f64)> = BTreeMap::new();
            let mut fsum = 0.0;
            // one measured pixel: its colour, the cell it falls in
            let mut measure = |c: Rgb, cx: usize, cy: usize| {
                let Some(si) = assign.get(&(fi, cx, cy)) else { return };
                let cols: Vec<Rgb> = si.iter().flat_map(|i| slots[*i].colors.iter().copied()).collect();
                let t = *cols.iter().min_by(|a, b| autoset::metric(c, **a).partial_cmp(&autoset::metric(c, **b)).unwrap_or(std::cmp::Ordering::Equal)).unwrap();
                let d = de(c, t);
                fsum += d;
                fl.pixels += 1;
                fl.max_de = fl.max_de.max(d);
                let e = by_src.entry(c).or_insert_with(|| (0, BTreeMap::new(), 0.0));
                e.0 += 1;
                *e.1.entry(t).or_insert(0) += 1;
                e.2 += d;
            };
            match &l.sources[fi] {
                Some(sf) => {
                    let f = sf.f.max(1);
                    for (i, p) in sf.img.px.iter().enumerate() {
                        if p.a < 128 {
                            continue;
                        }
                        let (x, y) = ((i % sf.img.w) as i32, (i / sf.img.w) as i32);
                        let (ox, oy) = ((x - sf.org.0).div_euclid(f), (y - sf.org.1).div_euclid(f));
                        if ox < 0 || oy < 0 || ox as usize >= fr.grid.w || oy as usize >= fr.grid.h {
                            continue;
                        }
                        measure(prof.snap(Rgb { r: p.r, g: p.g, b: p.b }), ox as usize / cw, oy as usize / ch);
                    }
                }
                None => {
                    let g = &fr.grid;
                    for y in 0..g.h {
                        for x in 0..g.w {
                            let k = g.cells[y * g.w + x];
                            if let Some(&c) = (k != b'.').then(|| l.rgb.get(&k)).flatten() {
                                measure(c, x / cw, y / ch);
                            }
                        }
                    }
                }
            }
            fl.mean_de = fsum / fl.pixels.max(1) as f64;
            sum += fsum;
            // identity colours lost in this frame: large or saturated colours of the frame, and every colour of the asset's identity list it holds
            let total_px_f = fl.pixels.max(1) as f64;
            for (src, (n, kept, dsum)) in &by_src {
                let share = *n as f64 / total_px_f;
                let identity = share > 0.03 || (chroma(*src) > 0.12 && share > 0.005);
                let mean = dsum / *n as f64;
                if identity && mean > LOST {
                    let k = kept.iter().max_by_key(|e| *e.1).map(|e| *e.0).unwrap_or(*src);
                    fl.lost.push((*src, k, mean));
                }
            }
            for ic in &l.identity {
                let near: Vec<(&Rgb, &(usize, BTreeMap<Rgb, usize>, f64))> = by_src.iter().filter(|(c, _)| de(**c, *ic) <= 8.0).collect();
                let n: usize = near.iter().map(|e| e.1 .0).sum();
                if n == 0 {
                    continue;
                }
                let mean = near.iter().map(|e| e.1 .2).sum::<f64>() / n as f64;
                if mean > LOST && !fl.lost.iter().any(|x| de(x.0, *ic) <= 8.0) {
                    let mut kept: BTreeMap<Rgb, usize> = BTreeMap::new();
                    for e in &near {
                        for (k, c) in &e.1 .1 {
                            *kept.entry(*k).or_insert(0) += c;
                        }
                    }
                    let k = kept.into_iter().max_by_key(|e| e.1).map(|e| e.0).unwrap_or(*ic);
                    fl.lost.push((*ic, k, mean));
                }
            }
            fl.lost.sort_by(|a, b| a.0.cmp(&b.0));
            for (src, kept, d) in &fl.lost {
                let e = by_part_lost.entry(*src).or_insert((*kept, *d));
                if *d > e.1 {
                    *e = (*kept, *d);
                }
            }
            loss.pixels += fl.pixels;
            loss.max_de = loss.max_de.max(fl.max_de);
            loss.frames.push(fl);
        }
        loss.mean_de = sum / loss.pixels.max(1) as f64;
        loss.lost = by_part_lost.into_iter().map(|(s, (k, d))| (s, k, d)).collect();
        total_sum += sum;
        total_px += loss.pixels;
        plan.losses.push(loss);
    }
    plan.mean_de = total_sum / total_px.max(1) as f64;
    plan.slots = slots;
    Ok(plan)
}

fn find_parts<'a>(m: &'a Manifest, sc: &Scene, extra: &[String], without: &[String]) -> Vec<&'a PartDecl> {
    let mut seen = BTreeSet::new();
    let mut v = Vec::new();
    let pats: Vec<&String> = sc.groups.iter().flat_map(|g| g.parts.iter()).chain(extra.iter()).collect();
    for p in pats {
        for d in cellmem::resolve_parts(m, p) {
            if seen.insert(d.name.clone()) {
                v.push(d);
            }
        }
    }
    let gone: BTreeSet<String> = without.iter().flat_map(|w| cellmem::resolve_parts(m, w).into_iter().map(|d| d.name.clone())).collect();
    v.retain(|d| !gone.contains(&d.name));
    v
}

pub fn run_palette_plan(a: &PalettePlanArgs, out: &mut Out) -> Result<()> {
    let m = Manifest::discover(a.manifest.as_deref())?.ok_or_else(|| PxlError::usage("`palette plan` needs a pxl.toml with [scene.NAME]"))?;
    if a.apply && a.what_if {
        return Err(PxlError::usage("--what-if writes nothing; drop --apply to try an option, or drop --what-if to apply it"));
    }
    let sc = m.scenes.iter().find(|s| s.name == a.scene).ok_or_else(|| PxlError::usage(format!("no scene '{}'", a.scene)).with_hint(format!("scenes: {}", m.scenes.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", "))))?;
    let (sp, _) = scene::hardware(&m)?;
    let parts = find_parts(&m, sc, &a.parts, &a.without);
    if parts.is_empty() {
        return Err(PxlError::usage(format!("scene {} has no parts", sc.name)).with_hint("list parts in [scene.X.groups] or pass --parts PATTERN"));
    }
    let mut reserve = sc.reserve.palette_slots.clone();
    reserve.extend(a.reserve.iter().copied());
    reserve.sort();
    reserve.dedup();
    let keep = sc.palette_set.as_ref().and_then(|n| cells::load_set(n, &m.dir).ok());
    let align = match &a.align {
        Some(n) => Some(cells::load_set(n, &m.dir)?),
        None => None,
    };
    let opts = PlanOpts { slots: a.slots, reserve: reserve.clone(), align, keep };
    let plan = palette_plan(&m, &parts, &sp, &opts)?;
    let name = a.name.clone().unwrap_or_else(|| sc.name.clone());
    out.line(format!("palette plan for scene {}: {} part(s), {} slot(s) of {}{}", sc.name, parts.len(), plan.slots.iter().filter(|s| !s.reserved).count(), sp.obj_palettes, if reserve.is_empty() { String::new() } else { format!(", slot(s) {:?} reserved", reserve) }));
    out.line(plan.text().trim_end());
    let mut js = plan.to_json();
    js["scene"] = json!(sc.name);
    js["set"] = json!(name);
    js["reserved_slots"] = json!(reserve);
    js["applied"] = json!(false);
    if !a.apply {
        out.line(if a.what_if { "what-if: nothing written" } else { "dry run: add --apply to write palettes/<set>.toml, set the scene's palette_set and reduce the parts into it" });
        out.set("plan", js);
        return Ok(());
    }
    // write the set, link it to the scene, reduce every part into it
    crate::lease::guard(&m.path, out.agent())?;
    let dir = m.dir.join("palettes");
    std::fs::create_dir_all(&dir)?;
    let file = dir.join(format!("{}.toml", name));
    fsutil::atomic_write_plain(&file, plan.set_text(&sc.name).as_bytes())?;
    cells::load_set(&name, &m.dir)?; // the written set must parse (legal colours, three per slot)
    let text = fsutil::read_string(&m.path)?;
    if sc.palette_set.as_deref() != Some(name.as_str()) {
        let new = crate::manedit::set_table_key(&text, &format!("scene.{}", sc.name), "palette_set", &crate::asset::q(&name)).ok_or_else(|| PxlError::usage(format!("pxl.toml has no [scene.{}] table", sc.name)))?;
        Manifest::parse(&new, &m.path)?;
        fsutil::atomic_write(&m.path, new.as_bytes())?;
    }
    let mut fitted = 0;
    // parts of the player group (`[project.defaults] group_categories`, built in: player) get the category, so stacked (two-slot) cells are allowed
    let defaults = crate::project::defaults_of(&m.path);
    for d in &parts {
        if let Ok(p) = pxl::load(&d.path) {
            if p.kind == PartKind::Sprite && p.category.is_none() {
                if let Some(c) = defaults.group_categories.get(&group_of(&m, d)) {
                    let mut scratch = Out::new(true, None);
                    match crate::manedit::set_category(&d.path.slash(), c, &mut scratch) {
                        Ok(()) => out.line(format!("{}: category {} (its group '{}'; two-slot cells allowed for player parts)", d.name, c, group_of(&m, d))),
                        Err(e) => out.warn(format!("{}: category {}: {}", d.name, c, e.msg)),
                    }
                }
            }
        }
    }
    for d in &parts {
        let mut scratch = Out::new(true, None);
        let fa = FitArgs { part: d.path.slash(), set: Some(name.clone()), apply: true, sel: FrameSel::default(), png: None, zoom: None, include_locked: false, no_all_keys: false, force_approved: false };
        match crate::fit::run_fit(&fa, &mut scratch) {
            Ok(()) => fitted += 1,
            Err(e) => out.warn(format!("{}: could not reduce into {}: {}", d.name, name, e.msg)),
        }
    }
    out.line(format!("wrote {} and linked scene {}; reduced {} of {} part(s) into it", file.slash(), sc.name, fitted, parts.len()));
    js["applied"] = json!(true);
    js["file"] = json!(file.slash());
    js["reduced"] = json!(fitted);
    out.set("plan", js);
    Ok(())
}

// ------------------------------------------------------------------------------------------------ plan

fn group_of(m: &Manifest, d: &PartDecl) -> String {
    if let Some(t) = d.tags.first() {
        return t.clone();
    }
    let rel = d.path.strip_prefix(&m.dir).unwrap_or(&d.path);
    let c: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect();
    if c.len() >= 3 && c[0] == "assets" {
        c[1].clone()
    } else {
        "main".into()
    }
}

#[derive(Clone, Debug)]
struct Proposal {
    scene: Scene,
    proposed: bool,
    palette: Option<PalettePlan>,
    palette_err: Option<String>,
    report: scene::SceneReport,
    options: Vec<Value>,
    dual_use: Vec<Value>,
}

fn is_set_console(sp: &Profile) -> bool {
    sp.auto_template == "set"
}

fn propose_scene(m: &Manifest, name: &str, tile_parts: &[&PartDecl], sprite_parts: &[&PartDecl], cache: &mut Cache, sp: &Profile) -> Scene {
    let defaults = crate::project::defaults_of(&m.path);
    let mut groups: BTreeMap<String, Vec<&PartDecl>> = BTreeMap::new();
    for d in sprite_parts {
        groups.entry(group_of(m, d)).or_default().push(d);
    }
    let mut sc = Scene { name: name.into(), palette_set: None, tileset: None, window: Vec::new(), resident_budget: None, palettes_out: None, bg_reserved: None, reserve: Default::default(), layout: None, groups: Vec::new() };
    for (g, ds) in groups {
        let total: usize = ds.iter().map(|d| cache.entry(d.path.slash()).or_insert_with(|| cellmem::part_mem(&d.path, &d.name)).as_ref().map_or(0, |p| p.unique)).sum();
        // player parts may use stacked (two-slot) cells: up to `multi_slot_max` extra cells per frame, so the budget has room for them
        let stacked: usize = ds
            .iter()
            .filter_map(|d| pxl::load(&d.path).ok())
            .filter(|p| p.kind == PartKind::Sprite && (p.category.as_deref() == Some("player") || (p.category.is_none() && defaults.category_for(&g).as_deref() == Some("player"))))
            .map(|p| p.frames.len() * sp.multi_slot_max as usize)
            .sum();
        let budget = total + (total / 8).max(2) + stacked;
        sc.groups.push(SceneGroup { name: g, budget: Some(budget as u32), streamed: false, per_frame: None, parts: ds.iter().map(|d| d.name.clone()).collect() });
    }
    let named: Vec<&&PartDecl> = tile_parts.iter().filter(|d| d.name.contains(name)).collect();
    sc.tileset = named.first().map(|d| d.name.clone()).or_else(|| if tile_parts.len() == 1 { Some(tile_parts[0].name.clone()) } else { None });
    sc.window = tile_parts.iter().filter(|d| d.name.starts_with("hud") || d.name.contains("window")).map(|d| d.name.clone()).collect();
    sc
}

fn evaluate(m: &Manifest, sc: Scene, proposed: bool, sp: &Profile, tp: &Profile, cache: &mut Cache) -> Proposal {
    let mut report = scene::SceneReport::default();
    let mut options = Vec::new();
    let mut dual = Vec::new();
    let mut palette = None;
    let mut palette_err = None;
    let parts = find_parts(m, &sc, &[], &[]);
    if is_set_console(sp) {
        report = scene::check_scene(m, &sc, sp, tp, cache);
        if sc.palette_set.is_none() {
            report.rows.retain(|r| r.item != "palette plan");
        }
        // the palette plan from all inputs of the scene
        let opts = PlanOpts { slots: None, reserve: sc.reserve.palette_slots.clone(), align: None, keep: sc.palette_set.as_ref().and_then(|n| cells::load_set(n, &m.dir).ok()) };
        match palette_plan(m, &parts, sp, &opts) {
            Ok(p) => {
                let used = p.slots.iter().filter(|s| !s.reserved).count();
                report.rows.push(scene::Row { item: "palette plan".into(), used: used.to_string(), limit: sp.obj_palettes.to_string(), ok: used <= sp.obj_palettes as usize, note: format!("one shared set; mean dE {:.1}, {} identity colour(s) lost", p.mean_de, p.lost_total()) });
                palette = Some(p);
            }
            Err(e) => palette_err = Some(e.msg),
        }
        let (mut over_vram, mut over_oam) = (0usize, 0usize);
        if let Some(v) = &report.vram {
            over_vram = (v.resident_cells + v.streamed_cells + sc.reserve.sprite_cells as usize).saturating_sub(v.effective_cells);
        }
        let oam: usize = scene_groups_max_sprites(m, &sc, cache);
        if sp.hw_cells_max > 0 {
            // a pessimistic bound (every group shows its widest frame at once): a warning, not a failure
            report.rows.push(scene::Row { item: "OAM (widest frames)".into(), used: oam.to_string(), limit: sp.hw_cells_max.to_string(), ok: true, note: "the widest frame of every group together: a pessimistic bound".into() });
            over_oam = oam.saturating_sub(sp.hw_cells_max as usize);
            if over_oam > 0 {
                report.warnings.push(format!("OAM: {} sprites if every group shows its widest frame at once, {} over the limit of {}; real screens show fewer (`pxl scene {} --worst`), but merge sprites in the widest frames if they can meet", oam, over_oam, sp.hw_cells_max, sc.name));
            }
        }
        if !report.ok() {
            options = infeasible_options(m, &sc, &report, over_vram, over_oam, sp, cache);
        }
        dual = dual_use(m, &sc, &report, sp, cache);
    } else {
        // flat consoles: one palette per sprite; the shared VRAM model is not built for them (their budgets are per asset)
        let mut by_group: Vec<String> = Vec::new();
        for g in &sc.groups {
            let n: usize = g.parts.iter().map(|p| cellmem::resolve_parts(m, p).len()).sum();
            by_group.push(format!("{} ({} part(s))", g.name, n));
        }
        report.name = sc.name.clone();
        report.warnings.push(format!("{}: sprites use one palette each and VRAM is not budgeted per scene; per-asset hardware limits apply (`pxl check`). Groups: {}", sp.name, by_group.join(", ")));
    }
    report.name = sc.name.clone();
    Proposal { scene: sc, proposed, palette, palette_err, report, options, dual_use: dual }
}

fn scene_groups_max_sprites(m: &Manifest, sc: &Scene, cache: &mut Cache) -> usize {
    cellmem::scene_report(m, sc, cache).iter().map(|g| g.parts.iter().map(|p| p.max_frame_sprites).max().unwrap_or(0)).sum()
}

fn infeasible_options(m: &Manifest, sc: &Scene, rep: &scene::SceneReport, over_vram: usize, over_oam: usize, sp: &Profile, cache: &mut Cache) -> Vec<Value> {
    let mut opts = Vec::new();
    let reports = cellmem::scene_report(m, sc, cache);
    let mut parts: Vec<(&PartMem, &str)> = reports.iter().flat_map(|g| g.parts.iter().map(move |p| (p, g.name.as_str()))).collect();
    parts.sort_by(|a, b| b.0.unique.cmp(&a.0.unique).then(a.0.name.cmp(&b.0.name)));
    if over_vram > 0 {
        // cut
        let cut: Vec<String> = parts.iter().take(3).map(|(p, _)| format!("{} ({} cells)", p.name, p.unique)).collect();
        let per_frame: usize = parts.first().map_or(0, |(p, _)| p.unique / p.max_frame_unique.max(1).min(p.unique.max(1)));
        opts.push(json!({"kind": "cut", "saves": over_vram.min(parts.iter().map(|(p, _)| p.unique).sum::<usize>()), "cost": "frames or animation steps of the biggest assets", "text": format!("cut animation steps of the biggest assets: {} (each frame cut of {} saves about {} cells)", cut.join(", "), parts.first().map_or("-", |(p, _)| p.name.as_str()), per_frame.max(1))}));
        // rebalance
        let slack = rep.vram.as_ref().map_or(0, |v| v.slack);
        if sp.vram_share && sc.tileset.is_some() {
            let lend = slack.saturating_sub(sc.reserve.bg_tiles as usize) / 2;
            opts.push(json!({"kind": "rebalance", "saves": lend, "cost": "BG tiles the level does not use", "text": format!("lend BG slack to sprites: {} unused BG tiles can hold {} more cells (already counted when the tile set is linked)", slack, lend)}));
        } else if sp.vram_share {
            opts.push(json!({"kind": "rebalance", "saves": 0, "cost": "link the BG tile set first", "text": "no BG tile set is linked, so no BG slack can be lent to sprites; link it (tileset = ...) and plan again"}));
        }
        // BG conversion
        let obj: Vec<&(&PartMem, &str)> = parts.iter().filter(|(_, g)| matches!(*g, "objects" | "platforms" | "scenery" | "bridges")).collect();
        let pick = obj.first().map(|x| x.0).or_else(|| parts.first().map(|x| x.0));
        if let Some(p) = pick {
            opts.push(json!({"kind": "bg_conversion", "saves": p.unique, "cost": format!("{} BG tiles, no sprite flips or priority", p.unique * 2), "text": format!("convert {} to a BG version: frees {} sprite cells, costs {} BG tiles", p.name, p.unique, p.unique * 2)}));
        }
        // per-act split
        let mut totals: Vec<(String, usize)> = reports.iter().filter(|g| !g.streamed).map(|g| (g.name.clone(), g.total)).collect();
        totals.sort_by(|a, b| b.1.cmp(&a.1));
        let (mut left, mut right) = (Vec::new(), Vec::new());
        let (mut ls, mut rs) = (0usize, 0usize);
        for (n, t) in totals {
            if ls <= rs {
                ls += t;
                left.push(n);
            } else {
                rs += t;
                right.push(n);
            }
        }
        let eff = rep.vram.as_ref().map_or(0, |v| v.effective_cells);
        if !right.is_empty() {
            opts.push(json!({"kind": "act_split", "saves": over_vram, "cost": "two scene loads instead of one", "fits": ls.max(rs) <= eff, "text": format!("split into two scenes by act: {} ({} cells) and {} ({} cells) against {} cells each; the shared groups load twice", left.join("+"), ls, right.join("+"), rs, eff)}));
        }
    }
    if over_oam > 0 {
        opts.push(json!({"kind": "cut", "saves": over_oam, "cost": "frames or sprites per frame", "text": format!("{} sprites over the OAM limit if every group shows its widest frame at once: merge sprites in the widest frames, or keep those groups apart in time", over_oam)}));
    }
    // failing rows that no option above covers still get listed
    let named: BTreeSet<&str> = rep.rows.iter().filter(|r| !r.ok).map(|r| r.item.as_str()).collect();
    if !named.is_empty() {
        opts.push(json!({"kind": "failing", "saves": 0, "cost": "-", "text": format!("failing rows: {}", named.into_iter().collect::<Vec<_>>().join(", "))}));
    }
    opts
}

/// Objects that probably need a BG version: scenery, platform and object parts whose cells or widest frame weigh on the lines or on VRAM.
fn dual_use(m: &Manifest, sc: &Scene, rep: &scene::SceneReport, sp: &Profile, cache: &mut Cache) -> Vec<Value> {
    let mut v = Vec::new();
    for g in cellmem::scene_report(m, sc, cache) {
        let object_group = matches!(g.name.as_str(), "objects" | "platforms" | "scenery" | "bridges");
        for p in &g.parts {
            let category = pxl::load(Path::new(&p.path)).ok().and_then(|x| x.category);
            let object = object_group || matches!(category.as_deref(), Some("platform") | Some("scenery"));
            if !object {
                continue;
            }
            let wide = sp.hw_cells_per_row_max > 0 && p.max_frame_sprites * 2 >= sp.hw_cells_per_row_max as usize;
            let heavy = rep.vram.as_ref().map_or(false, |vr| vr.resident_cells + vr.streamed_cells + sc.reserve.sprite_cells as usize > vr.effective_cells) && p.unique >= 8;
            if wide || heavy {
                let bg = crate::dmanifest::load_meta(Path::new(&p.path)).ok().and_then(|m| m.bg_variant);
                v.push(json!({"part": p.name, "bg_variant": bg, "covered": bg.is_some(), "group": g.name, "sprites_in_widest_frame": p.max_frame_sprites, "unique_cells": p.unique, "reason": if wide { format!("{} sprites in one frame against {} per scanline: it may not stay a sprite on a real layout", p.max_frame_sprites, sp.hw_cells_per_row_max) } else { "the scene is over its sprite VRAM and this object is a large share".to_string() }, "suggest": if bg.is_some() { "a BG form exists (listed in the delivery manifest)" } else { "make a BG-palette version (8x8 cells on the scene BG palettes, colour-0 policy) as its own asset and link it: pxl meta PART set bg_variant BGPART" }}));
            }
        }
    }
    v
}

fn signature(m: &Manifest) -> String {
    let mut s = String::new();
    for sc in &m.scenes {
        s.push_str(&format!("{}|{:?}|{:?}|{:?}|{:?}|", sc.name, sc.palette_set, sc.tileset, sc.window, sc.reserve));
        for g in &sc.groups {
            s.push_str(&format!("{}:{:?}:{}:{:?};", g.name, g.budget, g.streamed, g.per_frame));
        }
        s.push('\n');
    }
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", h)
}

pub fn plan_path(m: &Manifest) -> PathBuf {
    m.dir.join("plan.toml")
}

#[derive(Clone, Debug, Default)]
pub struct PlanFile {
    pub state: String,
    pub signature: String,
    pub accepted_by: String,
    pub accepted_on: String,
}

pub fn read_plan(m: &Manifest) -> Option<PlanFile> {
    let t: toml::Table = std::fs::read_to_string(plan_path(m)).ok()?.parse().ok()?;
    let p = t.get("plan")?.as_table()?;
    let s = |k: &str| p.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    Some(PlanFile { state: s("state"), signature: s("signature"), accepted_by: s("accepted_by"), accepted_on: s("accepted_on") })
}

fn write_plan(m: &Manifest, p: &PlanFile) -> Result<()> {
    let mut s = String::from("# written by `pxl plan`; `pxl plan accept` signs it off. next and claim refuse until the plan is accepted.\n[plan]\n");
    s.push_str(&format!("state = \"{}\"\nsignature = \"{}\"\n", p.state, p.signature));
    if !p.accepted_by.is_empty() {
        s.push_str(&format!("accepted_by = \"{}\"\naccepted_on = \"{}\"\n", p.accepted_by, p.accepted_on));
    }
    fsutil::atomic_write_plain(&plan_path(m), s.as_bytes())?;
    Ok(())
}

/// `(accepted and current, what is missing)`: the milestone `plan accepted`.
pub fn plan_accepted(m: &Manifest) -> (bool, String) {
    match read_plan(m) {
        None => (false, "there is no plan (pxl plan --apply)".into()),
        Some(p) if p.state == "accepted" && p.signature == signature(m) => (true, format!("accepted by {} on {}", p.accepted_by, p.accepted_on)),
        Some(p) if p.state == "accepted" => (false, "the scenes changed since the plan was accepted (pxl plan accept)".into()),
        Some(_) => (false, "the plan is not accepted yet (pxl plan accept)".into()),
    }
}

/// `next` and `claim` call this: art work needs an accepted plan in a project that has one (or that asks for one; `[project] gates = true` asks).
pub fn gate() -> Result<()> {
    let Ok(Some(m)) = Manifest::discover(None) else { return Ok(()) };
    let required = crate::project::project_table(&m.path).get("plan").and_then(|v| v.as_str()) == Some("required") || crate::milestone::gates_on(&m.dir);
    match read_plan(&m) {
        None if required => Err(PxlError::validation("this project requires a plan before art work, and there is none").with_hint("pxl plan --apply, then pxl plan accept")),
        None => Ok(()),
        Some(p) if p.state == "accepted" && p.signature == signature(&m) => Ok(()),
        Some(p) if p.state == "accepted" => Err(PxlError::validation("the scenes changed since the plan was accepted").with_hint("check them with `pxl plan`, then `pxl plan accept` again")),
        Some(_) => Err(PxlError::validation("the plan is not accepted yet, so no art work is handed out").with_hint("review `pxl plan`, decide the options once, then `pxl plan accept`")),
    }
}

fn scene_toml(sc: &Scene) -> String {
    let q = |s: &str| format!("\"{}\"", s);
    let mut t = format!("\n[scene.{}]\n", sc.name);
    if let Some(p) = &sc.palette_set {
        t.push_str(&format!("palette_set = {}\n", q(p)));
    }
    if let Some(p) = &sc.tileset {
        t.push_str(&format!("tileset = {}\n", q(p)));
    }
    if !sc.window.is_empty() {
        t.push_str(&format!("window = [{}]\n", sc.window.iter().map(|w| q(w)).collect::<Vec<_>>().join(", ")));
    }
    for g in &sc.groups {
        t.push_str(&format!("\n[scene.{}.groups.{}]\n", sc.name, g.name));
        if g.streamed {
            t.push_str(&format!("streamed = true\nper_frame = {}\n", g.per_frame.unwrap_or(0)));
        } else {
            t.push_str(&format!("budget = {}\n", g.budget.unwrap_or(0)));
        }
        t.push_str(&format!("parts = [{}]\n", g.parts.iter().map(|p| q(p)).collect::<Vec<_>>().join(", ")));
    }
    t
}

fn proposal_json(p: &Proposal) -> Value {
    json!({
        "scene": p.scene.name, "proposed": p.proposed, "ok": p.report.ok() && p.palette_err.is_none(),
        "groups": p.scene.groups.iter().map(|g| json!({"group": g.name, "budget": g.budget, "streamed": g.streamed, "per_frame": g.per_frame, "parts": g.parts})).collect::<Vec<_>>(),
        "tileset": p.scene.tileset, "window": p.scene.window,
        "fit": p.report.to_json(),
        "palette": p.palette.as_ref().map(|x| x.to_json()), "palette_error": p.palette_err,
        "options": p.options, "dual_use": p.dual_use,
    })
}

pub fn run_plan(a: &PlanArgs, out: &mut Out) -> Result<()> {
    let m = Manifest::discover(a.manifest.as_deref())?.ok_or_else(|| PxlError::usage("`plan` needs a project (pxl.toml); `pxl new` creates one"))?;
    if a.action.as_deref() == Some("accept") {
        return run_accept(&m, a, out);
    }
    if let Some(x) = a.action.as_deref().filter(|x| *x != "show") {
        return Err(PxlError::usage(format!("unknown plan action '{}'", x)).with_hint("pxl plan | pxl plan --apply | pxl plan accept | pxl plan show"));
    }
    let (sp, tp) = scene::hardware(&m)?;
    let mut cache: Cache = HashMap::new();
    // sort the assets: sprites and tiles
    let mut sprites: Vec<&PartDecl> = Vec::new();
    let mut tiles: Vec<&PartDecl> = Vec::new();
    for d in &m.parts {
        match pxl::load(&d.path).map(|p| p.kind) {
            Ok(PartKind::Sprite) => sprites.push(d),
            Ok(PartKind::Tileset) => tiles.push(d),
            _ => {}
        }
    }
    if sprites.is_empty() && tiles.is_empty() {
        return Err(PxlError::validation("the project has no assets to plan from").with_hint("put source art in inbox/ and run `pxl ingest`"));
    }
    let mut props: Vec<Proposal> = Vec::new();
    if m.scenes.is_empty() {
        let names: Vec<String> = if a.scenes.is_empty() { vec!["main".into()] } else { a.scenes.clone() };
        for n in names {
            let sc = propose_scene(&m, &n, &tiles, &sprites, &mut cache, &sp);
            props.push(evaluate(&m, sc, true, &sp, &tp, &mut cache));
        }
    } else {
        for sc in &m.scenes {
            props.push(evaluate(&m, sc.clone(), false, &sp, &tp, &mut cache));
        }
    }
    out.line(format!("plan for {} ({}): {} sprite asset(s), {} tile asset(s)", std::fs::canonicalize(if m.dir.as_os_str().is_empty() { std::path::Path::new(".") } else { &m.dir }).ok().and_then(|p| p.file_name().map(|s| s.to_string_lossy().to_string())).unwrap_or_default(), m.console.as_deref().unwrap_or("gbc"), sprites.len(), tiles.len()));
    let state = read_plan(&m);
    out.line(format!("  plan state: {}", state.as_ref().map_or("none (this is a proposal)".to_string(), |p| if p.state == "accepted" && p.signature != signature(&m) { "accepted, but the scenes changed since".into() } else { p.state.clone() })));
    let mut infeasible = 0;
    for p in &props {
        out.line(format!("scene {}{}", p.scene.name, if p.proposed { " (proposed from the groups)" } else { "" }));
        for g in &p.scene.groups {
            out.line(format!("  group {:<10} budget {:>4}  parts: {}", g.name, g.budget.map_or("-".into(), |b| b.to_string()), g.parts.join(", ")));
        }
        out.line(p.report.text().trim_end());
        if let Some(pp) = &p.palette {
            out.line(format!("  palette plan: {} slot(s), mean dE {:.1}, identity colours lost {}", pp.slots.iter().filter(|s| !s.reserved).count(), pp.mean_de, pp.lost_total()));
            // measured on the source images, per frame; stacked (two-slot) cells proposed for the player
            for l in &pp.losses {
                for f in l.frames.iter().filter(|f| !f.lost.is_empty()) {
                    out.line(format!("    {} {} loses {}{}", l.part, f.frame, f.lost.iter().map(|(a, b, d)| lost_text(*a, *b, *d)).collect::<Vec<_>>().join(", "), if f.from_source { "" } else { " (reduced art: no source image)" }));
                }
                if !l.stacked.is_empty() {
                    let frames: BTreeSet<&str> = l.stacked.iter().map(|c| c.frame.as_str()).collect();
                    out.line(format!(
                        "    {}: {} stacked (two-slot) cell(s) proposed in {} frame(s), 2 sprites each, e.g. {} cell {},{} slots {}+{} ({} colours, dE {:.0} -> {:.0}){}",
                        l.part,
                        l.stacked.len(),
                        frames.len(),
                        l.stacked[0].frame,
                        l.stacked[0].cell.0,
                        l.stacked[0].cell.1,
                        l.stacked[0].slots.0,
                        l.stacked[0].slots.1,
                        l.stacked[0].colours,
                        l.stacked[0].before_de,
                        l.stacked[0].after_de,
                        if l.stack_allowed { String::new() } else { format!("; `pxl asset set {} category player` allows them", l.part) }
                    ));
                }
            }
        }
        if let Some(e) = &p.palette_err {
            out.line(format!("  palette plan: {}", e));
        }
        let ok = p.report.ok() && p.palette_err.is_none();
        if !ok {
            infeasible += 1;
            out.line(format!("  NOT FEASIBLE as planned. Options (decide once, up front):"));
            for (i, o) in p.options.iter().enumerate() {
                out.line(format!("    {}. [{}] {}  (cost: {})", i + 1, o["kind"].as_str().unwrap_or(""), o["text"].as_str().unwrap_or(""), o["cost"].as_str().unwrap_or("")));
            }
        }
        for d in &p.dual_use {
            out.line(format!("  dual-use candidate: {} ({}): {}{}", d["part"].as_str().unwrap_or(""), d["group"].as_str().unwrap_or(""), d["reason"].as_str().unwrap_or(""), if d["covered"] == true { format!(" [BG form: {}]", d["bg_variant"].as_str().unwrap_or("")) } else { " [no BG form yet]".to_string() }));
        }
    }
    let mut js = json!({"console": m.console, "state": state.as_ref().map(|p| p.state.clone()), "scenes": props.iter().map(proposal_json).collect::<Vec<_>>(), "infeasible": infeasible, "saved": false});
    if a.apply {
        if a.action.as_deref() == Some("show") {
            return Err(PxlError::usage("`plan show` does not write"));
        }
        crate::lease::guard(&m.path, out.agent())?;
        let mut text = fsutil::read_string(&m.path)?;
        let mut wrote = Vec::new();
        for p in props.iter().filter(|p| p.proposed) {
            text.push_str(&scene_toml(&p.scene));
            wrote.push(p.scene.name.clone());
        }
        if !wrote.is_empty() {
            text = drop_legacy(&text);
            // the delivery manifest is regenerated by every build from now on
            if let Some(t2) = crate::manedit::set_table_key(&text, "project", "manifest", "true") {
                text = t2;
            }
            Manifest::parse(&text, &m.path)?;
            fsutil::atomic_write(&m.path, text.as_bytes())?;
            out.line(format!("pxl.toml: added scene(s) {} (the shared scene budgets now apply)", wrote.join(", ")));
        }
        let m2 = Manifest::load(&m.path)?;
        // the shared palette sets: one per scene, the parts reduced into them
        if is_set_console(&sp) {
            for sc in &m2.scenes {
                if props.iter().any(|p| p.proposed && p.scene.name == sc.name) || sc.palette_set.is_none() {
                    let pa = PalettePlanArgs { scene: sc.name.clone(), slots: None, reserve: Vec::new(), without: Vec::new(), parts: Vec::new(), align: None, name: None, what_if: false, apply: true, manifest: Some(m.path.slash()) };
                    let mut scratch = Out::new(true, None);
                    match run_palette_plan(&pa, &mut scratch) {
                        Ok(()) => out.line(format!("palette set '{}' written and linked to scene {}", sc.name, sc.name)),
                        Err(e) => out.warn(format!("palette plan for scene {}: {}", sc.name, e.msg)),
                    }
                }
            }
        }
        let m3 = Manifest::load(&m.path)?;
        if crate::dmanifest::enabled(&m3) {
            match crate::dmanifest::write(&m3) {
                Ok((files, problems)) => {
                    for f in files {
                        out.line(format!("delivery manifest {}", f.slash()));
                    }
                    for p in problems {
                        out.warn(format!("delivery manifest: {}", p));
                    }
                }
                Err(e) => out.warn(format!("delivery manifest: {}", e.msg)),
            }
        }
        write_plan(&m3, &PlanFile { state: "proposed".into(), signature: signature(&m3), ..Default::default() })?;
        out.line(format!("plan.toml written (state proposed); review it, then `pxl plan accept`{}", if infeasible > 0 { " (decide the options for the infeasible scenes first)" } else { "" }));
        js["saved"] = json!(true);
    }
    out.set("plan", js);
    Ok(())
}

/// `legacy = true` (and the comment that explains it) leave pxl.toml once scenes exist: the shared budgets apply, and no stale comment stays behind.
pub fn drop_legacy(text: &str) -> String {
    let mut lines: Vec<&str> = Vec::new();
    for l in text.lines() {
        let t = l.trim();
        if t == "legacy = true" || t == "legacy = false" || (t.starts_with('#') && t.contains("`legacy = ")) {
            continue;
        }
        lines.push(l);
    }
    let mut r = lines.join("\n");
    r.push('\n');
    r
}

fn run_accept(m: &Manifest, a: &PlanArgs, out: &mut Out) -> Result<()> {
    let cur = read_plan(m).ok_or_else(|| PxlError::usage("there is no plan to accept").with_hint("pxl plan --apply writes the scenes and plan.toml"))?;
    if m.scenes.is_empty() {
        return Err(PxlError::validation("the project has no scenes").with_hint("pxl plan --apply"));
    }
    let (sp, tp) = scene::hardware(m)?;
    let mut cache: Cache = HashMap::new();
    let mut bad = Vec::new();
    for sc in &m.scenes {
        let p = evaluate(m, sc.clone(), false, &sp, &tp, &mut cache);
        if !(p.report.ok() && p.palette_err.is_none()) {
            bad.push(sc.name.clone());
        }
    }
    if !bad.is_empty() && a.decision.is_none() {
        return Err(PxlError::validation(format!("scene(s) {} do not fit as planned", bad.join(", "))).with_hint("choose an option (cut, rebalance, BG conversion, act split), apply it, or accept anyway: pxl plan accept --decision \"<what was decided and why>\""));
    }
    let _ = &out;
    // an agent never signs as the user: `--by user` is the user's own word
    let by = a.by.clone().unwrap_or_else(|| "agent".into());
    let on = crate::tasks::date_of(crate::tasks::now());
    if let Some(d) = &a.decision {
        let f = m.dir.join("decisions.md");
        let mut t = std::fs::read_to_string(&f).unwrap_or_else(|_| "# Decisions\n\nOne dated line each; `pxl decide` appends.\n\n".into());
        if !t.ends_with('\n') {
            t.push('\n');
        }
        t.push_str(&format!("- {} [{}] plan accepted with {} scene(s) not fitting as planned ({}): {}\n", on, by, bad.len(), bad.join(", "), d.trim()));
        fsutil::atomic_write_plain(&f, t.as_bytes())?;
    }
    write_plan(m, &PlanFile { state: "accepted".into(), signature: signature(m), accepted_by: by.clone(), accepted_on: on.clone() })?;
    let _ = cur;
    out.emit(format!("plan accepted by {} on {}; next and claim hand out art work again", by, on), json!({"state": "accepted", "by": by, "on": on, "not_fitting": bad}));
    Ok(())
}
