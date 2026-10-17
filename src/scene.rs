//! Scenes and the shared hardware model (M5 phase T5): `pxl check` per scene, `pxl scene NAME [--worst]`.
//!
//! A scene (`[scene.NAME]` in pxl.toml) links everything that is on the GBC at the same time: the sprite palette set, the BG
//! tile set, the window / HUD tile parts and the sprite groups. The profiles `gbc-sprite` / `gbc-tile` declare the shared
//! resources (OBJ and BG palettes, OAM, sprites per line, VRAM map, DMA bytes per frame). A scene passes when
//!
//! * the palette set exists and defines at most `obj_palettes` slots;
//! * every sprite group fits its budget (unique 8x16 cells with flips free, streamed groups per frame) and the groups together
//!   fit the sprite VRAM (`sprite_cells`; a streamed group is double buffered, so it counts twice its largest frame);
//! * streamed groups fit the DMA budget (unique cells that change between consecutive frames x 32 bytes, summed over the
//!   streamed groups, at most `dma_bytes_per_frame`);
//! * the tile set (`tileset = "zone_1_blocks"`) fits the BG budget (level tiles + animated slots) and the BG palettes that are
//!   not reserved;
//! * the window parts (`window = ["hud", "cards"]`) fit `window_tiles` and live on a reserved BG palette.
//!
//! The check fails closed: a project with sprite or tile parts and no scenes, a sprite part without a `palette_set`, or one that
//! no scene lists is an error, unless the manifest says `legacy = true`.

use crate::fsutil::PathSlash;
use crate::cellmem::{self, GroupReport, PartMem};
use crate::cli::SceneArgs;
use crate::color::Rgb;
use crate::manifest::{Manifest, Scene};
use crate::model::PartKind;
use crate::output::{Out, PxlError, Result};
use crate::profile::{self, Profile};
use crate::tiles;
use crate::{cells, pxl, template};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Clone, Debug)]
pub struct Row {
    pub item: String,
    pub used: String,
    pub limit: String,
    pub ok: bool,
    pub note: String,
}

#[derive(Clone, Debug, Default)]
pub struct SceneReport {
    pub name: String,
    pub rows: Vec<Row>,
    pub warnings: Vec<String>,
    pub vram: Option<Vram>,
}

/// VRAM of a scene in 8x8 tiles (768 = 2 banks x 384): BG level + animated, window, sprites.
#[derive(Clone, Debug, Default)]
pub struct Vram {
    pub bg_used: usize,
    pub bg_budget: usize,
    pub window: usize,
    pub resident_cells: usize,
    pub streamed_cells: usize,
    /// BG tiles left over and the sprite cells they hold (`vram_share`).
    pub slack: usize,
    pub extra_cells: usize,
    pub effective_cells: usize,
    pub total_tiles: usize,
    pub total_limit: usize,
}

impl SceneReport {
    pub fn ok(&self) -> bool {
        self.rows.iter().all(|r| r.ok)
    }
    pub fn to_json(&self) -> Value {
        json!({"scene": self.name, "ok": self.ok(), "vram": self.vram.as_ref().map(|v| json!({"bg_used": v.bg_used, "bg_budget": v.bg_budget, "window": v.window, "sprite_resident_cells": v.resident_cells, "sprite_streamed_cells": v.streamed_cells, "bg_slack": v.slack, "extra_sprite_cells": v.extra_cells, "effective_sprite_cells": v.effective_cells, "total_tiles": v.total_tiles, "total_limit": v.total_limit})), "rows": self.rows.iter().map(|r| json!({"item": r.item, "used": r.used, "limit": r.limit, "ok": r.ok, "note": r.note})).collect::<Vec<_>>(), "warnings": self.warnings})
    }
    pub fn text(&self) -> String {
        let mut s = format!("scene {}: {}\n", self.name, if self.ok() { "ok" } else { "FAIL" });
        for r in &self.rows {
            s.push_str(&format!("  {} {:<22} {:>10} / {:<8}{}\n", if r.ok { "ok  " } else { "FAIL" }, r.item, r.used, r.limit, if r.note.is_empty() { String::new() } else { format!("  {}", r.note) }));
        }
        for w in &self.warnings {
            s.push_str(&format!("  note: {}\n", w));
        }
        s
    }
}

/// The shared-resource profiles of a project (the project's console; `consoles/<id>/<mode>.toml` next to the manifest overrides the built-in).
/// Does the project's console budget sprites per scene with shared palette sets (gb, gbc, nes, and any project without a console)? The flat
/// consoles (sms, gg, md, snes, pce, gba) give every sprite its own palette and are limited per asset and per line: no scene budgets or palette
/// sets are demanded of them. (By console id, so a project that names a legacy profile keeps the old behaviour exactly.)
pub fn shared_palette_model(m: &Manifest) -> bool {
    !matches!(m.console.as_deref(), Some("sms" | "gg" | "md" | "snes" | "pce" | "gba"))
}

pub fn hardware(m: &Manifest) -> Result<(Profile, Profile)> {
    Ok((profile::for_project(&m.dir, m.console.as_deref(), "sprite")?, profile::for_project(&m.dir, m.console.as_deref(), "tile")?))
}

fn row(item: &str, used: impl ToString, limit: impl ToString, ok: bool, note: impl Into<String>) -> Row {
    Row { item: item.into(), used: used.to_string(), limit: limit.to_string(), ok, note: note.into() }
}

fn part_by_key<'a>(m: &'a Manifest, key: &str) -> Option<&'a crate::manifest::PartDecl> {
    m.parts.iter().find(|d| d.name == key).or_else(|| cellmem::resolve_parts(m, key).into_iter().next())
}

/// Level tiles + animated slots and the BG palettes of a tile set part.
fn bg_rows(m: &Manifest, key: &str, tp: &Profile, reserved: &[u8]) -> (Vec<Row>, Option<(usize, usize)>) {
    let Some(d) = part_by_key(m, key) else { return (vec![row("BG tile set", "-", "-", false, format!("'{}' is not a part of the manifest", key))], None) };
    let load = || -> Result<(usize, usize, usize, usize, usize)> {
        let part = pxl::load(&d.path)?;
        let tpl = template::load_for(&part)?;
        let prof = profile::load_for(&part, None)?;
        let fi = tiles::pick_frame(&part, None)?;
        let l = tiles::load_frame(part, tpl, fi)?;
        let uniq = tiles::pattern::groups(&l.sheet, &l.cols, &l.air, true).len();
        let anim = tiles::slots::animated_slots(&l.part, &prof).total;
        let limit = tiles::unique_limit(&l.part, &prof).min(tp.unique_tiles_max as usize);
        let sets: Vec<Vec<u32>> = l.sheet.tiles.iter().map(|t| tiles::stats::colors_of(&t.px(&l.cols))).collect();
        let bins = tiles::stats::greedy_sets(&sets).bins.len();
        let maxp = if l.part.bg_palette.is_some() { 1 } else { (prof.palettes as usize).saturating_sub(reserved.len()).max(1) };
        Ok((uniq, anim, limit, bins, maxp))
    };
    match load() {
        Ok((u, a, limit, bins, maxp)) => (
            vec![
                row("BG tiles", u + a, limit, u + a <= limit && u + a > 0, if u + a == 0 { format!("0 tiles counted for {}: a silent zero", key) } else { format!("{} level tiles + {} animated slots ({})", u, a, key) }),
                row("BG palettes", bins, maxp, bins <= maxp, if reserved.is_empty() { "none reserved in this scene (no window parts)".to_string() } else { format!("{} reserved", reserved.iter().map(|b| b.to_string()).collect::<Vec<_>>().join(",")) }),
            ],
            Some((u + a, limit)),
        ),
        Err(e) => (vec![row("BG tile set", "-", "-", false, format!("{}: {}", key, e.msg))], None),
    }
}

fn window_row(m: &Manifest, sc: &Scene, tp: &Profile, reserved: &[u8]) -> (Option<Row>, usize) {
    if sc.window.is_empty() {
        return (None, 0);
    }
    let mut total = 0usize;
    let mut notes = Vec::new();
    let mut ok = true;
    for pat in &sc.window {
        let hit = cellmem::resolve_parts(m, pat);
        if hit.is_empty() {
            ok = false;
            notes.push(format!("'{}' matches no part", pat));
        }
        for d in hit {
            let r = (|| -> Result<usize> {
                let part = pxl::load(&d.path)?;
                let tpl = template::load_for(&part)?;
                if !part.bg_palette.map_or(false, |b| reserved.contains(&b)) {
                    ok = false;
                    notes.push(format!("{} is not on a reserved BG palette (bg_palette = {:?}, reserved {:?})", d.name, part.bg_palette, reserved));
                }
                let mut n = 0;
                for fi in 0..part.frames.len() {
                    let g = &part.frames[fi].grid;
                    if g.w % 8 != 0 || g.h % 8 != 0 {
                        continue;
                    }
                    let cols = tiles::Colors::new(&part, &tpl);
                    let air = tiles::air_codes(&tpl, &cols);
                    n += tiles::pattern::groups(&tiles::Sheet::from_grid(g), &cols, &air, true).len();
                }
                Ok(n)
            })();
            match r {
                Ok(n) => total += n,
                Err(e) => {
                    ok = false;
                    notes.push(format!("{}: {}", d.name, e.msg));
                }
            }
        }
    }
    let lim = tp.window_tiles as usize;
    (Some(row("window tiles", total, lim, ok && total <= lim, notes.join("; "))), total)
}

pub fn check_scene(m: &Manifest, sc: &Scene, sp: &Profile, tp: &Profile, cache: &mut HashMap<String, std::result::Result<PartMem, String>>) -> SceneReport {
    let mut rep = SceneReport { name: sc.name.clone(), ..Default::default() };
    if !shared_palette_model(m) {
        rep.warnings.push(format!("{}: sprites use one palette each and VRAM is not budgeted per scene; per-asset hardware limits apply (`pxl check`)", sp.name));
        return rep;
    }
    match &sc.palette_set {
        None => rep.rows.push(row("palette plan", "-", sp.obj_palettes, false, "the scene has no palette_set")),
        Some(n) => match cells::load_set(n, &m.dir) {
            Ok(set) => {
                let k = set.slots.iter().flatten().count();
                rep.rows.push(row("OBJ palettes", k, sp.obj_palettes, k <= sp.obj_palettes as usize, format!("set '{}'", n)));
            }
            Err(e) => rep.rows.push(row("palette plan", "-", sp.obj_palettes, false, e.msg)),
        },
    }
    if sc.groups.is_empty() {
        rep.rows.push(row("sprite groups", 0, "-", false, "the scene allocates no sprite groups (no budget)"));
    }
    let reports: Vec<GroupReport> = cellmem::scene_report(m, sc, cache);
    for g in &reports {
        let listed: usize = sc.groups.iter().find(|x| x.name == g.name).map_or(0, |x| x.parts.iter().map(|p| cellmem::resolve_parts(m, p).len()).sum());
        if g.total == 0 && listed > 0 {
            rep.rows.push(row(&format!("count {}", g.name), 0, "> 0", false, format!("group lists {} part(s) but counts 0 cells: a silent zero (frames unreadable, or no palette set)", listed)));
        }
    }
    let (mut resident, mut streamed, mut dma_cells) = (0usize, 0usize, 0usize);
    for g in &reports {
        let note = g.problems.iter().take(2).cloned().collect::<Vec<_>>().join("; ");
        let held = if g.streamed { 0 } else { sc.reserve.groups.get(&g.name).copied().unwrap_or(0) as usize };
        let note = if held > 0 { format!("{}{}{} cells reserved for growth", note, if note.is_empty() { "" } else { "; " }, held) } else { note };
        // a FAIL on a row means the value is over its limit; a group whose count is under the limit but that has other problems (a palette set
        // that does not fit, unreadable frames) gets its own row naming them, so `player 12 / 14 FAIL` never appears
        let count_ok = if g.streamed { g.total as u32 <= g.limit() } else { g.total + held <= g.limit() as usize };
        let note_row = if g.problems.is_empty() { note.clone() } else { String::new() };
        rep.rows.push(row(&format!("group {}", g.name), if held > 0 { format!("{}+{}", g.total, held) } else { g.total.to_string() }, g.limit(), count_ok, if g.streamed { format!("streamed, cells in one frame{}{}", if note_row.is_empty() { "" } else { "; " }, note_row) } else { note_row }));
        if !g.problems.is_empty() {
            rep.rows.push(row(&format!("group {} problems", g.name), format!("{} problem(s)", g.problems.len()), "0", false, g.problems.iter().take(3).cloned().collect::<Vec<_>>().join("; ")));
        }
        if g.streamed {
            streamed += g.total * 2;
            dma_cells += g.parts.iter().map(|p| p.dma_cells).max().unwrap_or(0);
        } else {
            resident += g.total + held;
        }
    }
    // BG palettes are only reserved where there is a window / HUD to put on them (or the scene says so)
    let reserved: Vec<u8> = sc.bg_reserved.clone().unwrap_or_else(|| if sc.window.is_empty() { Vec::new() } else { tp.bg_reserved.clone() });
    let (bg_rows_v, bg_used) = match &sc.tileset {
        Some(k) => bg_rows(m, k, tp, &reserved),
        None => {
            rep.warnings.push("no tileset linked (tileset = \"zone_N_blocks\"): the BG budget of this scene is not verified, no BG slack is lent to sprites".into());
            (Vec::new(), None)
        }
    };
    let (win_row, win_used) = window_row(m, sc, tp, &reserved);
    if !sc.window.is_empty() && win_used == 0 {
        rep.rows.push(row("count window", 0, "> 0", false, "the scene lists window parts but they count 0 tiles: a silent zero"));
    }
    // VRAM: 768 tiles. Sprites own sprite_cells; BG tiles a scene does not use can hold more cells (the 0x8800-0x8FFF block is
    // addressable by OBJ and BG), up to the addressable OBJ limit.
    let default_cells = sp.sprite_cells as usize;
    let (bg_used_n, bg_budget) = bg_used.unwrap_or((tp.unique_tiles_max as usize, tp.unique_tiles_max as usize));
    let slack = bg_budget.saturating_sub(bg_used_n);
    let extra = if sp.vram_share && bg_used.is_some() { slack.saturating_sub(sc.reserve.bg_tiles as usize) / 2 } else { 0 };
    let cap_cells = (sp.obj_tiles_max / 2) as usize;
    let effective = (default_cells + extra).min(cap_cells).max(default_cells.min(cap_cells));
    let extra = effective.saturating_sub(default_cells);
    if !reports.is_empty() {
        let used = resident + streamed + sc.reserve.sprite_cells as usize;
        let note = if extra > 0 { format!("{} resident + {} double-buffered streamed cells; {} default + {} from {} unused BG tiles", resident, streamed, default_cells, extra, slack) } else { format!("{} resident + {} double-buffered streamed cells", resident, streamed) };
        let note = if sc.reserve.sprite_cells > 0 || sc.reserve.bg_tiles > 0 { format!("{}; reserved: {} sprite cells, {} BG tiles", note, sc.reserve.sprite_cells, sc.reserve.bg_tiles) } else { note };
        rep.rows.push(row("sprite VRAM", used, effective, used <= effective, note));
        if let Some(rb) = sc.resident_budget {
            let need = rb as usize + streamed_cap(&reports);
            rep.rows.push(row("resident budget", rb, effective.saturating_sub(streamed_cap(&reports)), need <= effective && resident <= rb as usize, format!("{} resident cells used; the budget plus the streamed buffers ({}) must fit the effective sprite VRAM", resident, streamed_cap(&reports))));
        }
        let bytes = dma_cells * 32;
        rep.rows.push(row("DMA bytes/frame", bytes, sp.dma_bytes_per_frame, sp.dma_bytes_per_frame == 0 || bytes <= sp.dma_bytes_per_frame as usize, format!("{} changed cells x 32 bytes, worst part of each streamed group", dma_cells)));
    }
    if extra > 0 {
        rep.warnings.push(format!("VRAM split differs from the operator's default: {} extra sprite cells ({} sprite cells instead of {}) from {} BG tiles this scene does not use (BG {} of {}); list it in DELIVERY.md", extra, effective, default_cells, slack, bg_used_n, bg_budget));
    }
    let total_tiles = bg_used_n + win_used + 2 * (resident + streamed + sc.reserve.sprite_cells as usize);
    if bg_used.is_some() {
        let bg_all = bg_used_n + win_used;
        rep.rows.push(row("BG addressable", bg_all, sp.bg_tiles_max, bg_all <= sp.bg_tiles_max as usize, "level + animated + window tiles (8800 mode)"));
        rep.rows.push(row("VRAM tiles", total_tiles, 2 * sp.vram_tiles_per_bank, total_tiles <= (2 * sp.vram_tiles_per_bank) as usize, "BG used + window + sprite resident + streamed (8x8 tiles)"));
    }
    rep.vram = Some(Vram { bg_used: bg_used_n, bg_budget, window: win_used, resident_cells: resident, streamed_cells: streamed, slack, extra_cells: extra, effective_cells: effective, total_tiles, total_limit: (2 * sp.vram_tiles_per_bank) as usize });
    rep.rows.extend(bg_rows_v);
    if let Some(r) = win_row {
        rep.rows.push(r);
    }
    rep
}

/// Double-buffered cells of the streamed groups (their per-frame caps x 2).
fn streamed_cap(reports: &[GroupReport]) -> usize {
    reports.iter().filter(|g| g.streamed).map(|g| g.limit() as usize * 2).sum()
}

/// Parts with `budget_exempt`: (part, reason, scenes whose groups list it). A part that is exempt and listed is contradictory.
pub fn exempt_parts(m: &Manifest) -> Vec<(String, String, Vec<String>)> {
    let mut v = Vec::new();
    for d in &m.parts {
        let Ok(p) = pxl::load(&d.path) else { continue };
        let Some(reason) = p.budget_exempt.clone() else { continue };
        let mut scenes: Vec<String> = Vec::new();
        for sc in &m.scenes {
            if sc.groups.iter().any(|g| g.parts.iter().any(|pat| cellmem::resolve_parts(m, pat).iter().any(|x| x.name == d.name))) {
                scenes.push(sc.name.clone());
            }
        }
        v.push((d.name.clone(), reason, scenes));
    }
    v
}

/// References between parts of a manifest, computed once per process: parts named as a scene tileset / window, and parts
/// listed in a tile set's `animated`.
fn referenced_set(mp: &std::path::Path) -> Option<std::sync::Arc<(BTreeSet<std::path::PathBuf>, bool, bool)>> {
    use std::sync::{Arc, Mutex};
    static CACHE: Mutex<Vec<(std::path::PathBuf, Option<std::time::SystemTime>, Arc<(BTreeSet<std::path::PathBuf>, bool, bool)>)>> = Mutex::new(Vec::new());
    let mt = std::fs::metadata(mp).ok()?.modified().ok();
    let mut g = CACHE.lock().ok()?;
    if let Some(e) = g.iter().find(|e| e.0 == mp && e.1 == mt) {
        return Some(e.2.clone());
    }
    let m = Manifest::load(mp).ok()?;
    let mut set: BTreeSet<std::path::PathBuf> = BTreeSet::new();
    for sc in &m.scenes {
        if let Some(t) = &sc.tileset {
            for d in cellmem::resolve_parts(&m, t) {
                set.insert(crate::manifest::canon(&d.path));
            }
        }
        for w in &sc.window {
            for d in cellmem::resolve_parts(&m, w) {
                set.insert(crate::manifest::canon(&d.path));
            }
        }
    }
    for d in &m.parts {
        if let Ok(p) = pxl::load(&d.path) {
            for rel in &p.animated {
                set.insert(crate::manifest::canon(&p.dir().join(rel)));
            }
        }
    }
    let strict = crate::project::project_table(mp).get("strict").and_then(|v| v.as_bool()).unwrap_or(false);
    let v = Arc::new((set, !m.legacy && !m.scenes.is_empty(), strict));
    g.push((mp.to_path_buf(), mt, v.clone()));
    Some(v)
}

/// `Some(message)` when `part` is a tile set that no scene (as tileset or window part) and no tile set (as `animated`) references.
/// Only projects with scenes and without `legacy = true` are checked.
pub fn unreferenced_tile_set(part: &crate::model::Part) -> Option<(String, bool)> {
    let mp = Manifest::find_file(&part.dir())?;
    let r = referenced_set(&mp)?;
    if !r.1 {
        return None;
    }
    let me = crate::manifest::canon(&part.path);
    let m = Manifest::load(&mp).ok()?;
    if m.find(&part.path).is_none() || r.0.contains(&me) {
        return None;
    }
    Some((format!("tile set {} is referenced by no scene (tileset / window) and by no tile set (animated), so none of its tiles is counted in any budget; add it with `pxl scene set SCENE tileset {}` or list it in a tile set's `animated`", part.name, part.name), r.2))
}

/// Fail-closed findings about the project as a whole (empty with `legacy = true`).
pub fn project_issues(m: &Manifest) -> Vec<String> {
    let mut v = Vec::new();
    if m.legacy || !shared_palette_model(m) {
        return v;
    }
    let mut sprite = 0;
    let mut tile = 0;
    let mut listed: BTreeSet<String> = BTreeSet::new();
    for sc in &m.scenes {
        for g in &sc.groups {
            for pat in &g.parts {
                listed.extend(cellmem::resolve_parts(m, pat).iter().map(|d| d.name.clone()));
            }
        }
    }
    for d in &m.parts {
        let Ok(p) = pxl::load(&d.path) else { continue };
        match p.kind {
            PartKind::Sprite => {
                sprite += 1;
                if p.palette_set.is_none() {
                    v.push(format!("sprite part {} has no palette_set (add one with `pxl palette fit {} --set NAME --apply`)", d.name, d.name));
                } else if !m.scenes.is_empty() && !listed.contains(&d.name) && p.budget_exempt.is_none() {
                    v.push(format!("sprite part {} is in no scene group, so its share of the shared budgets is not checked", d.name));
                }
            }
            PartKind::Tileset => tile += 1,
            PartKind::Screen => {}
        }
    }
    if (sprite > 0 || tile > 0) && m.scenes.is_empty() {
        v.push("the project has sprite or tile parts but declares no [scene.NAME]; budgets are shared by everything on screen (`legacy = true` in pxl.toml opts out)".into());
    }
    v
}

// ------------------------------------------------------------------------------------------------ pxl scene

pub(crate) struct Comp {
    pub(crate) label: String,
    pub(crate) part: String,
    pub(crate) frame: String,
    pub(crate) width: usize,
    pub(crate) sprites: u32,
    pub(crate) slots: BTreeSet<u8>,
    /// Sprites on each line, keyed by the distance above the lowest opaque row of the frame (negative = below it).
    pub(crate) loads: BTreeMap<i32, u32>,
    pub(crate) bottom: i32,
    pub(crate) w: usize,
    pub(crate) h: usize,
    pub(crate) rgb: Vec<Option<Rgb>>,
}

pub(crate) fn comps_of(m: &Manifest, key: &str, label: &str) -> Result<Vec<Comp>> {
    let d = part_by_key(m, key).ok_or_else(|| PxlError::usage(format!("'{}' is not a part of the manifest", key)))?;
    let part = pxl::load(&d.path)?;
    let tpl = template::load_for(&part)?;
    let prof = profile::load_for(&part, None)?;
    let res = cells::resolve(&part, &prof).map_err(|e| PxlError::validation(format!("{}: {}", d.name, e.msg)))?;
    let mut out = Vec::new();
    for fi in 0..part.frames.len() {
        let pix = cells::frame_pix(&part, &tpl, &res, fi, prof.channel_step);
        let pl = cells::place_frame(&part, &res, &pix, fi).map_err(PxlError::validation)?;
        let (mut x0, mut x1, mut y1) = (usize::MAX, 0usize, -1i32);
        for (i, id) in pix.ids.iter().enumerate() {
            if *id != 255 {
                x0 = x0.min(i % pix.w);
                x1 = x1.max(i % pix.w + 1);
                y1 = y1.max((i / pix.w) as i32);
            }
        }
        if y1 < 0 {
            continue;
        }
        let mut loads = BTreeMap::new();
        for (y, l) in pl.line_load.iter().enumerate() {
            if *l > 0 {
                loads.insert(y1 - y as i32, *l);
            }
        }
        let slots: BTreeSet<u8> = pl.cells.iter().flat_map(|c| [Some(c.slot), c.slot2].into_iter().flatten()).collect();
        out.push(Comp { label: label.into(), part: d.name.clone(), frame: part.frames[fi].name.clone(), width: x1 - x0, sprites: pl.sprites, slots, loads, bottom: y1, w: pix.w, h: pix.h, rgb: pix.rgb });
    }
    Ok(out)
}

pub(crate) fn widest(v: Vec<Comp>) -> Option<Comp> {
    v.into_iter().max_by_key(|c| (c.width, c.sprites))
}

pub fn run_scene(a: &SceneArgs, out: &mut Out) -> Result<()> {
    let m = Manifest::discover(a.manifest.as_deref())?.ok_or_else(|| PxlError::usage("`scene` needs a pxl.toml with [scene.NAME]"))?;
    let sc = m.scenes.iter().find(|s| s.name == a.name).ok_or_else(|| PxlError::usage(format!("no scene '{}'", a.name)).with_hint(format!("scenes: {}", m.scenes.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", "))))?;
    let (sp, tp) = hardware(&m)?;
    let mut cache = HashMap::new();
    if a.layout {
        return crate::layout::run_layout(a, &m, sc, &sp, out);
    }
    let rep = check_scene(&m, sc, &sp, &tp, &mut cache);
    if !a.worst {
        out.line(rep.text().trim_end());
        out.set("scene", rep.to_json());
        return if rep.ok() { Ok(()) } else { Err(PxlError::lint(format!("scene {} exceeds a shared budget", sc.name))) };
    }
    // ---- worst case: the player's widest frame, the two widest object frames, an enemy and a ring, all on the same lines
    let in_group = |names: &[&str]| -> Vec<String> {
        sc.groups.iter().filter(|g| names.contains(&g.name.as_str())).flat_map(|g| g.parts.iter().flat_map(|p| cellmem::resolve_parts(&m, p).into_iter().map(|d| d.name.clone()))).collect::<BTreeSet<_>>().into_iter().collect()
    };
    let all_parts: Vec<String> = sc.groups.iter().flat_map(|g| g.parts.iter().flat_map(|p| cellmem::resolve_parts(&m, p).into_iter().map(|d| d.name.clone()))).collect::<BTreeSet<_>>().into_iter().collect();
    let category_of = |k: &str| part_by_key(&m, k).and_then(|d| pxl::load(&d.path).ok()).and_then(|p| p.category);
    let mut comps: Vec<Comp> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    let try_comps = |k: &str, label: &str, skipped: &mut Vec<String>| -> Vec<Comp> {
        match comps_of(&m, k, label) {
            Ok(v) => v,
            Err(e) => {
                skipped.push(format!("{}: {}", k, e.msg));
                Vec::new()
            }
        }
    };
    // player
    let player = a.player.clone().or_else(|| in_group(&["player"]).into_iter().next()).or_else(|| all_parts.iter().find(|k| category_of(k).as_deref() == Some("player")).cloned()).ok_or_else(|| PxlError::usage("the scene has no player part").with_hint("--player PART"))?;
    comps.push(widest(comps_of(&m, &player, "player")?).ok_or_else(|| PxlError::validation("the player part has no pixels"))?);
    // objects: the two widest frames
    let obj_parts: Vec<String> = match &a.objects {
        Some(v) => v.clone(),
        None => {
            let g = in_group(&["objects"]);
            if g.is_empty() { all_parts.iter().filter(|k| matches!(category_of(k).as_deref(), Some("platform") | Some("scenery"))).cloned().collect() } else { g }
        }
    };
    let mut frames: Vec<Comp> = Vec::new();
    for k in &obj_parts {
        frames.extend(try_comps(k, "object", &mut skipped));
    }
    frames.sort_by(|x, y| (y.width, y.sprites).cmp(&(x.width, x.sprites)).then(x.part.cmp(&y.part)).then(x.frame.cmp(&y.frame)));
    if a.objects.is_some() {
        // explicit parts: the widest frame of each
        let mut seen = BTreeSet::new();
        frames.retain(|c| seen.insert(c.part.clone()));
    }
    // two objects share the lines; a scene with a single object frame gets two instances of it
    let mut picked: Vec<Comp> = frames.into_iter().take(2).collect();
    if picked.len() == 1 {
        let c = &picked[0];
        picked.push(Comp { label: c.label.clone(), part: c.part.clone(), frame: c.frame.clone(), width: c.width, sprites: c.sprites, slots: c.slots.clone(), loads: c.loads.clone(), bottom: c.bottom, w: c.w, h: c.h, rgb: c.rgb.clone() });
    }
    comps.extend(picked);
    // enemy
    let enemy = a.enemy.clone().or_else(|| in_group(&["enemies"]).into_iter().next());
    if let Some(k) = enemy {
        let pool = if a.enemy.is_some() { vec![k] } else { in_group(&["enemies"]) };
        let mut fr = Vec::new();
        for k in pool {
            fr.extend(try_comps(&k, "enemy", &mut skipped));
        }
        if let Some(c) = widest(fr) {
            comps.push(c);
        }
    }
    // ring
    let ring = a.ring.clone().or_else(|| all_parts.iter().find(|k| k.contains("collectible_c") || k.contains("ring")).cloned()).or_else(|| in_group(&["common"]).into_iter().next());
    if let Some(k) = ring {
        if let Some(mut c) = try_comps(&k, "ring", &mut skipped).into_iter().next() {
            c.label = "ring".into();
            comps.push(c);
        }
    }
    let mut line: BTreeMap<i32, u32> = BTreeMap::new();
    for c in &comps {
        for (d, l) in &c.loads {
            *line.entry(*d).or_insert(0) += l;
        }
    }
    let max_line = line.values().copied().max().unwrap_or(0);
    let total: u32 = comps.iter().map(|c| c.sprites).sum();
    let slots: BTreeSet<u8> = comps.iter().flat_map(|c| c.slots.iter().copied()).collect();
    let (lim_line, lim_oam, lim_pal) = (sp.hw_cells_per_row_max, sp.hw_cells_max, sp.obj_palettes);
    out.line(format!("scene {} worst case:", sc.name));
    for sk in &skipped {
        out.line(format!("  skipped {}", sk));
    }
    for c in &comps {
        out.line(format!("  {:<7} {} / {}: {} px wide, {} sprites, slots {:?}", c.label, c.part, c.frame, c.width, c.sprites, c.slots));
    }
    let worst_at = line.iter().max_by_key(|(d, l)| (**l, std::cmp::Reverse(**d))).map(|(d, _)| *d).unwrap_or(0);
    let ok_line = max_line <= lim_line;
    let ok_oam = total <= lim_oam;
    let ok_pal = slots.len() <= lim_pal as usize;
    out.line(format!("  sprites per scanline: {} (limit {}) {}, worst {} rows above the ground line", max_line, lim_line, if ok_line { "ok" } else { "OVER" }, worst_at));
    out.line(format!("  sprites in OAM:       {} (limit {}) {}", total, lim_oam, if ok_oam { "ok" } else { "OVER" }));
    out.line(format!("  OBJ palettes used:    {} {:?} (limit {}) {}", slots.len(), slots, lim_pal, if ok_pal { "ok" } else { "OVER" }));
    out.set("schema", json!(1));
    out.set(
        "worst",
        json!({"scene": sc.name, "skipped": skipped, "per_line": max_line, "per_line_limit": lim_line, "sprites": total, "sprites_limit": lim_oam, "palettes": slots, "palettes_limit": lim_pal,
            "parts": comps.iter().map(|c| json!({"role": c.label, "part": c.part, "frame": c.frame, "width": c.width, "sprites": c.sprites, "slots": c.slots})).collect::<Vec<_>>(),
            "lines": line.iter().map(|(d, l)| json!({"above_ground": d, "sprites": l})).collect::<Vec<_>>()}),
    );
    if let Some(p) = &a.png {
        let s = a.zoom.unwrap_or(4).max(1);
        let (dmin, dmax) = (*line.keys().next().unwrap_or(&0), *line.keys().last().unwrap_or(&0));
        let rows = (dmax - dmin + 1) as usize;
        let comp_w: usize = comps.iter().map(|c| c.w + 4).sum();
        let (pw, bar_w) = (comp_w * s, (lim_line as usize + 6) * 6 * s / 2);
        let head = 24;
        let mut im = crate::img::RgbaImage::filled(pw + bar_w + 24, rows * s + head + 8, crate::compare::BG);
        crate::compare::draw_text(&mut im, 6, 6, &format!("{}: {} per line (limit {}), {} sprites (limit {}), {} palettes (limit {})", sc.name, max_line, lim_line, total, lim_oam, slots.len(), lim_pal), 1, crate::compare::TEXT);
        let mut x = 0usize;
        for c in &comps {
            for (i, px) in c.rgb.iter().enumerate() {
                let Some(col) = px else { continue };
                let (cx, cy) = (i % c.w, (i / c.w) as i32);
                let row = (dmax - (c.bottom - cy)) as isize;
                if row < 0 || row as usize >= rows {
                    continue;
                }
                crate::compare::fill(&mut im, 6 + (x + cx) * s, head + row as usize * s, s, s, crate::img::Rgba::opaque(col.r, col.g, col.b));
            }
            x += c.w + 4;
            let _ = c.h;
        }
        for r in 0..rows {
            let d = dmax - r as i32;
            let l = line.get(&d).copied().unwrap_or(0);
            let col = if l > lim_line { crate::compare::C_BAD } else { crate::img::Rgba::opaque(0x60, 0xb0, 0x70) };
            crate::compare::fill(&mut im, pw + 14, head + r * s, (l as usize * 3 * s / 2).max(1), s, col);
        }
        crate::compare::fill(&mut im, pw + 14 + lim_line as usize * 3 * s / 2, head, 1, rows * s, crate::compare::DIM);
        crate::show::write_png(std::path::Path::new(p), &im)?;
        out.line(format!("  wrote {} (components bottom-aligned on the ground line; bars = sprites on each line, limit marked)", p));
    }
    if ok_line && ok_oam && ok_pal {
        Ok(())
    } else {
        Err(PxlError::lint(format!("scene {} worst case exceeds a shared budget", sc.name)))
    }
}

// ------------------------------------------------------------------------------------------------ sprite_palettes.json

fn upper(c: Rgb) -> String {
    crate::color::to_hex(c).to_uppercase()
}

/// `sprite_palettes.json` of a palette set (hardware feedback section 7): the slots in order, following `include`; the dynamic
/// slot lists its variants.
pub fn sprite_palettes_json(set: &cells::PaletteSet) -> Value {
    let slots: Vec<Value> = set
        .slots
        .iter()
        .flatten()
        .map(|d| {
            let mut o = serde_json::Map::new();
            o.insert("slot".into(), json!(d.slot));
            o.insert("name".into(), json!(d.name));
            if !d.colors.is_empty() {
                o.insert("colors".into(), json!(d.colors.iter().map(|c| upper(*c)).collect::<Vec<_>>()));
            }
            if !d.variants.is_empty() {
                let mut v = serde_json::Map::new();
                for (n, cs) in &d.variants {
                    v.insert(n.clone(), json!(cs.iter().map(|c| upper(*c)).collect::<Vec<_>>()));
                }
                o.insert("variants".into(), Value::Object(v));
            }
            Value::Object(o)
        })
        .collect();
    json!({"slots": slots})
}

/// The sprite palette files a project exports: (scene, path, bytes). A scene writes one when it sets `palettes_out`, or is called
/// `zone_N` (default `zones/<scene>/sprite_palettes.json`); paths are below `out_root`. Scenes with the same path must agree.
pub fn palette_outputs(m: &Manifest) -> Result<Vec<(String, std::path::PathBuf, Vec<u8>)>> {
    let base = match &m.out_root {
        Some(r) => m.dir.join(r),
        None => m.dir.clone(),
    };
    let mut out: Vec<(String, std::path::PathBuf, Vec<u8>)> = Vec::new();
    for sc in &m.scenes {
        let rel = match (&sc.palettes_out, sc.name.strip_prefix("zone_")) {
            (Some(p), _) => p.clone(),
            (None, Some(n)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => format!("zones/{}/sprite_palettes.json", sc.name),
            _ => continue,
        };
        let Some(set_name) = &sc.palette_set else { continue };
        let set = cells::load_set(set_name, &m.dir)?;
        let bytes = (serde_json::to_string_pretty(&sprite_palettes_json(&set)).unwrap() + "\n").into_bytes();
        let path = base.join(rel);
        if let Some(prev) = out.iter().find(|o| o.1 == path) {
            if prev.2 != bytes {
                return Err(PxlError::validation(format!("scenes {} and {} write {} with different palette sets", prev.0, sc.name, path.slash())));
            }
            continue;
        }
        out.push((sc.name.clone(), path, bytes));
    }
    Ok(out)
}

/// Write the sprite palette files (only when they differ); returns the paths written or already current.
pub fn write_palettes(m: &Manifest) -> Result<Vec<(std::path::PathBuf, bool)>> {
    let mut v = Vec::new();
    for (_, path, bytes) in palette_outputs(m)? {
        let changed = crate::export::write_if_changed(&path, &bytes)?;
        v.push((path, changed));
    }
    Ok(v)
}

/// `pxl export-palettes`
pub fn run_export_palettes(a: &crate::cli::ExportPalettesArgs, out: &mut Out) -> Result<()> {
    let m = Manifest::discover(a.manifest.as_deref())?.ok_or_else(|| PxlError::usage("`export-palettes` needs a pxl.toml with [scene.NAME]"))?;
    let w = write_palettes(&m)?;
    for (p, changed) in &w {
        out.line(format!("{} {}", if *changed { "wrote" } else { "up to date" }, p.slash()));
    }
    if w.is_empty() {
        out.line("no scene exports palettes (scenes called zone_N, or with palettes_out)");
    }
    out.set("schema", json!(1));
    out.set("palettes", json!(w.iter().map(|(p, c)| json!({"path": p.slash(), "written": c})).collect::<Vec<_>>()));
    Ok(())
}
