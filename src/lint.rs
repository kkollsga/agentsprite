//! Lint (DESIGN 5.5). `lint_part` is pure and reusable (`compare`, `check`, `status`); `run_lint` is the CLI.
//!
//! Conventions (documented decisions):
//! * **Layers.** The first template layer is the *base* layer (exported palette 1); every other layer is an
//!   overlay. Colour limits apply per layer (`profile.colors` plus transparent). An overlay pixel also counts
//!   in the base layer through its `under` key, because the delivered base PNG holds that key there.
//!   Palette slot of a key = its 1-based position in its layer (template order), exactly as `export` writes it.
//! * **Family** (AC-TP3) is explicit: `family = "name"` in the part header (or `family` in its `[parts.X]` manifest
//!   entry). Only parts declaring the same family are compared: the template named by most of them (ties: the
//!   smallest path string) is the family template; parts naming another one are `template` errors. A part
//!   without a family is never compared with other parts (real packages have one palette per object/group).
//! * **Palette sets.** A part with `palette_set` uses the flat per-cell model of `cells`: rules `palette_set` (colour not
//!   in the set), `cell_palette` (a cell needs more than one slot) and `hw_limit` by category replace `palette_size`,
//!   `hw_cells` and the old grid `hw_limit`, which keep applying to every other part.
//! * **Tags** (`jitter`, `shimmer`) come from frame names: a leading `NN_` order prefix and the trailing
//!   digits are dropped, so `06_walk11` and `07_walk12` are both tag `walk`. Single-frame tags are skipped.
//! * Tile sets and screens run `tile_colors`, `tile_palettes` and `unique_tiles` (hard limits, counted as
//!   `check_assets.py` counts them, see `tiles::stats`) plus `alpha`, `size` and `template`.

use crate::fsutil::PathSlash;
use crate::cli::LintArgs;
use crate::color::{self, Rgb};
use crate::fsutil;
use crate::model::{Frame, Grid, Part, PartKind, RefFrame};
use crate::output::{Out, PxlError, Result};
use crate::profile::{self, Profile};
use crate::template::{self, Template};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const SPRITE_RULES: [&str; 21] = [
    "size",
    "palette_size",
    "rgb_step",
    "alpha",
    "shared_palette",
    "empty_frame",
    "template",
    "hw_cells",
    "hw_limit",
    "overlay",
    "palette_set",
    "cell_palette",
    "orphan",
    "jaggy",
    "noise",
    "jitter",
    "shimmer",
    "ref_scale",
    "lost_feature",
    "lost_eye",
    "colour_share",
];
/// Rules that need the high resolution reference.
pub const REF_RULES: [&str; 5] = ["shimmer", "ref_scale", "lost_feature", "lost_eye", "colour_share"];
/// Rules that describe looks, not limits: severity `advice`, off unless `--advice` / `--rules` asks for them.
pub const ADVICE_RULES: [&str; 9] = ["orphan", "jaggy", "noise", "jitter", "shimmer", "ref_scale", "lost_feature", "lost_eye", "colour_share"];
/// Tile rules (`tile_rules`).
pub const TILE_RULES: [&str; 7] = ["tile_colors", "tile_palettes", "unique_tiles", "priority_color0", "animated_set", "tile_unreferenced", "attr_block"];

/// Rules of the generalised console model (sprites): palette lines used by a part.
pub const CONSOLE_RULES: [&str; 3] = ["palette_lines", "near_duplicate", "stacked"];

/// Rules that run by default only in projects that deliver with the manifest (`[project] manifest = true`), so older projects keep their
/// results; `--rules` runs them anywhere.
pub const DELIVERY_RULES: [&str; 2] = ["near_duplicate", "stacked"];

fn delivery_project(part: &Part) -> bool {
    crate::manifest::Manifest::find_file(&part.dir()).map_or(false, |mp| crate::project::project_table(&mp).get("manifest").and_then(|v| v.as_bool()).unwrap_or(false))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
pub enum Severity {
    /// Objective limit broken (hardware, palette, format, template): fails `lint`, `check`, `release --state done`.
    Error,
    /// Objective but soft limit (hardware sprite budget as configured): reported, never fails.
    Warning,
    /// Observation about looks (stray pixels, jaggies, drift against the reference). Information only: never
    /// fails anything and never counts as a warning. Computed only with `--advice` or an explicit `--rules`.
    Advice,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Advice => "advice",
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct Issue {
    pub rule: String,
    pub severity: Severity,
    pub file: String,
    pub frame: Option<String>,
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub message: String,
    /// Ready-to-run `pxl ...` command, where one exists.
    pub fix: Option<String>,
}

impl Issue {
    pub fn to_json(&self) -> Value {
        json!({
            "rule": self.rule,
            "severity": self.severity.as_str(),
            "file": self.file,
            "frame": self.frame,
            "x": self.x,
            "y": self.y,
            "message": self.message,
            "fix": self.fix,
        })
    }
    pub fn to_text(&self) -> String {
        let mut s = format!("{}", self.file);
        if let Some(f) = &self.frame {
            s.push_str(&format!(":{}", f));
        }
        if let (Some(x), Some(y)) = (self.x, self.y) {
            s.push_str(&format!(":{},{}", x, y));
        }
        s.push_str(&format!(": {} [{}] {}", self.severity.as_str(), self.rule, self.message));
        if let Some(fx) = &self.fix {
            s.push_str(&format!("\n    fix: {}", fx));
        }
        s
    }
}

#[derive(Clone, Debug, Default)]
pub struct LintOpts {
    /// Only these rules; empty = all.
    pub rules: Vec<String>,
    /// Only report these frames; empty = all.
    pub frames: Vec<String>,
    /// Load references for `shimmer`, `ref_scale`, `lost_feature` (skipped silently when unavailable).
    pub refs: bool,
    /// Also run the advice rules when `rules` is empty (an explicit `rules` list always wins).
    pub advice: bool,
}

impl LintOpts {
    pub fn enabled(&self, rule: &str) -> bool {
        if self.rules.is_empty() {
            self.advice || !ADVICE_RULES.contains(&rule)
        } else {
            self.rules.iter().any(|r| r == rule)
        }
    }
    fn wants_refs(&self) -> bool {
        self.refs && REF_RULES.iter().any(|r| self.enabled(r))
    }
}

// ---------------------------------------------------------------------------------------------- helpers

fn quote(s: &str) -> String {
    if s.chars().any(|c| c.is_whitespace() || c == '\'') {
        format!("'{}'", s.replace('\'', "'\\''"))
    } else {
        s.to_string()
    }
}

fn part_ref(part: &Part) -> String {
    if part.path.as_os_str().is_empty() {
        quote(&part.name)
    } else {
        quote(&part.path.slash())
    }
}

fn file_of(part: &Part) -> String {
    if part.path.as_os_str().is_empty() {
        part.name.clone()
    } else {
        part.path.slash()
    }
}

fn kch(k: u8) -> char {
    k as char
}

/// Colour of a key after the part's local overrides (`Part.colors`, by key or role).
pub fn effective_color(part: &Part, tpl: &Template, key: u8) -> Option<Rgb> {
    if let Some(d) = tpl.def(key) {
        for (k, v) in &part.colors {
            if (k.len() == 1 && k.as_bytes()[0] == key) || *k == d.role {
                if let Ok(c) = color::parse_hex(v) {
                    return Some(c);
                }
            }
        }
    }
    tpl.color_of(key)
}

/// 1-based palette slot of `key` inside its layer.
fn slot_of(tpl: &Template, key: u8) -> Option<(usize, usize)> {
    let li = tpl.layer_of(key)?;
    let p = tpl.layers[li].keys.iter().position(|k| k.key == key)?;
    Some((li, p + 1))
}

/// Keys of the grid per layer in reading order of first appearance, with the position of the first pixel.
/// Overlay keys also register their `under` key in the base layer (layer 0).
fn layer_keys(tpl: &Template, g: &Grid) -> Vec<Vec<(u8, i32, i32)>> {
    let mut out: Vec<Vec<(u8, i32, i32)>> = vec![Vec::new(); tpl.layers.len().max(1)];
    for y in 0..g.h {
        for x in 0..g.w {
            let k = g.cells[y * g.w + x];
            if k == b'.' {
                continue;
            }
            let Some(li) = tpl.layer_of(k) else { continue };
            let mut add = |layer: usize, key: u8| {
                if !out[layer].iter().any(|e| e.0 == key) {
                    out[layer].push((key, x as i32, y as i32));
                }
            };
            add(li, k);
            if li != 0 {
                if let Some(u) = tpl.def(k).and_then(|d| d.under) {
                    if u != b'.' && tpl.layer_of(u) == Some(0) {
                        add(0, u);
                    }
                }
            }
        }
    }
    out
}

fn is_opaque(g: &Grid, x: i32, y: i32) -> bool {
    g.get(x, y) != b'.'
}

const N8: [(i32, i32); 8] = [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)];

fn bbox(g: &Grid) -> Option<(i32, i32, i32, i32)> {
    let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, -1, -1);
    for y in 0..g.h as i32 {
        for x in 0..g.w as i32 {
            if is_opaque(g, x, y) {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    (x1 >= 0).then_some((x0, y0, x1, y1))
}

/// Tag of a frame name: drop a leading `NN_` order prefix and the trailing digits (`06_walk11` -> `walk`).
pub fn tag_of(name: &str) -> String {
    let rest = match name.find('_') {
        Some(i) if i > 0 && name[..i].bytes().all(|b| b.is_ascii_digit()) => &name[i + 1..],
        _ => name,
    };
    let t = rest.trim_end_matches(|c: char| c.is_ascii_digit());
    if t.is_empty() { rest.to_string() } else { t.to_string() }
}

struct Ctx<'a> {
    part: &'a Part,
    tpl: Option<&'a Template>,
    prof: &'a Profile,
    opts: &'a LintOpts,
    /// Palette-set parts: the resolved set (flat per-cell palette model).
    pal: Option<crate::cells::Resolved>,
    file: String,
    out: Vec<(usize, Issue)>, // (frame index or usize::MAX for part level, issue)
}

impl<'a> Ctx<'a> {
    #[allow(clippy::too_many_arguments)]
    fn push(&mut self, rule: &str, sev: Severity, fi: Option<usize>, x: Option<i32>, y: Option<i32>, message: String, fix: Option<String>) {
        let frame = fi.map(|i| self.part.frames[i].name.clone());
        if let Some(f) = &frame {
            if !self.opts.frames.is_empty() && !self.opts.frames.iter().any(|n| n == f) {
                return;
            }
        }
        let issue = Issue { rule: rule.to_string(), severity: sev, file: self.file.clone(), frame, x, y, message, fix };
        self.out.push((fi.unwrap_or(usize::MAX), issue));
    }
}

// ---------------------------------------------------------------------------------------------- API

/// Lint one part against a template and a profile. Pure apart from reading reference images when
/// `opts.refs` is set. Issues are ordered by frame, y, x, rule.
pub fn lint_part(part: &Part, tpl: &Template, prof: &Profile, opts: &LintOpts) -> Vec<Issue> {
    lint_inner(part, Some(tpl), None, prof, opts, None)
}

/// Like [`lint_part`] with references the caller already loaded (one entry per part frame, `None` where
/// unavailable), so `compare` does not render them twice.
pub fn lint_part_refs(part: &Part, tpl: &Template, prof: &Profile, opts: &LintOpts, refs: &[Option<RefFrame>]) -> Vec<Issue> {
    lint_inner(part, Some(tpl), None, prof, opts, Some(refs))
}

/// Lint with the template resolved from the part. A missing template becomes a `template` error and the
/// colour rules are skipped.
pub fn lint_part_auto(part: &Part, prof: &Profile, opts: &LintOpts) -> Vec<Issue> {
    match template::load_for(part) {
        Ok(t) => lint_inner(part, Some(&t), None, prof, opts, None),
        Err(e) => lint_inner(part, None, Some(e.msg), prof, opts, None),
    }
}

fn lint_inner(part: &Part, tpl: Option<&Template>, tpl_err: Option<String>, prof: &Profile, opts: &LintOpts, refs_in: Option<&[Option<RefFrame>]>) -> Vec<Issue> {
    // the design-language gates (M6 R7) run by default only in projects with `[project] gates = true`; `--rules` runs them anywhere
    let design_on = if part.kind == PartKind::Sprite { crate::design::enabled(part, &opts.rules) } else { Vec::new() };
    let owned_opts;
    let opts = if opts.rules.is_empty() && !delivery_project(part) {
        let rules: Vec<String> = SPRITE_RULES.iter().chain(TILE_RULES.iter()).chain(CONSOLE_RULES.iter()).filter(|r| !DELIVERY_RULES.contains(r) && (opts.advice || !ADVICE_RULES.contains(r))).map(|r| r.to_string()).collect();
        owned_opts = LintOpts { rules, frames: opts.frames.clone(), refs: opts.refs, advice: opts.advice };
        &owned_opts
    } else {
        opts
    };
    let mut cx = Ctx { part, tpl, prof, opts, pal: None, file: file_of(part), out: Vec::new() };
    if part.kind == PartKind::Sprite && part.palette_set.is_some() {
        match crate::cells::resolve_for_lint(part, prof) {
            Ok(r) => cx.pal = r,
            Err(m) => {
                if opts.enabled("palette_set") {
                    cx.push("palette_set", Severity::Error, None, None, None, m, None);
                }
            }
        }
    }
    if part.kind != PartKind::Sprite {
        part_rules(&mut cx, tpl_err);
        set_link_rules(&mut cx);
        tile_rules(&mut cx);
    } else {
        palette_lines_rule(&mut cx);
        near_duplicate_rule(&mut cx);
        for fi in 0..part.frames.len() {
            frame_rules(&mut cx, fi);
        }
        part_rules(&mut cx, tpl_err);
        animation_rules(&mut cx);
        if let (false, Some(t)) = (design_on.is_empty(), tpl) {
            for f in crate::design::run(part, t, prof, &design_on) {
                cx.push(f.rule, if f.info { Severity::Advice } else { Severity::Warning }, f.frame_idx, f.x, f.y, f.message, f.fix);
            }
        }
        if opts.wants_refs() {
            let loaded;
            let refs: &[Option<RefFrame>] = match refs_in {
                Some(r) => r,
                None => {
                    loaded = load_refs(part, opts);
                    &loaded
                }
            };
            ref_rules(&mut cx, refs);
        }
    }
    let mut v = cx.out;
    v.sort_by(|a, b| {
        let ka = |p: &(usize, Issue)| (p.0, p.1.y.unwrap_or(-1), p.1.x.unwrap_or(-1));
        ka(a).cmp(&ka(b)).then_with(|| a.1.rule.cmp(&b.1.rule))
    });
    v.into_iter().map(|p| p.1).collect()
}

fn load_refs(part: &Part, opts: &LintOpts) -> Vec<Option<RefFrame>> {
    let dir = part.dir();
    let all = opts.enabled("shimmer");
    let want = |f: &Frame, i: usize| -> bool {
        opts.frames.is_empty() || all || opts.frames.iter().any(|n| *n == f.name) || (i > 0 && opts.frames.iter().any(|n| *n == part.frames[i - 1].name))
    };
    part.frames.iter().enumerate().map(|(i, f)| if want(f, i) { crate::load::reference(part, &dir, f).ok() } else { None }).collect()
}

// ---------------------------------------------------------------------------------------------- tile rules

/// No silent zeros (M6 R4): `animated_set` (a part listed in `animated` whose frames are not an `_NN` set counts no slots) and
/// `tile_unreferenced` (a tile set of a scene-based project that no scene's tileset / window and no tile set's `animated` names).
fn set_link_rules(cx: &mut Ctx) {
    let part = cx.part;
    if part.kind == PartKind::Tileset && cx.opts.enabled("animated_set") {
        for rel in &part.animated {
            let p = part.dir().join(rel);
            match crate::pxl::load(&p) {
                Err(e) => cx.push("animated_set", Severity::Error, None, None, None, format!("animated part {} cannot be loaded: {}", rel, e.msg), None),
                Ok(other) => {
                    let bare: Vec<&str> = other.frames.iter().filter(|f| !crate::tiles::slots::is_set_frame(&f.name)).map(|f| f.name.as_str()).collect();
                    if other.frames.is_empty() || !bare.is_empty() {
                        let why = if !bare.is_empty() { format!("frame(s) {} have no _NN suffix", bare.iter().take(4).cloned().collect::<Vec<_>>().join(", ")) } else { "it has no frames".to_string() };
                        cx.push("animated_set", Severity::Error, None, None, None, format!("animated part {} does not form an animation set ({}), so it would count 0 slots; name the frames <set>_00, <set>_01 ...", rel, why), None);
                    }
                }
            }
        }
    }
    if part.kind == PartKind::Tileset && cx.opts.enabled("tile_unreferenced") && part.budget_exempt.is_none() {
        if let Some((msg, strict)) = crate::scene::unreferenced_tile_set(part) {
            // an error in a project that sets `[project] strict = true` (`pxl new` does); a warning in one that predates it
            cx.push("tile_unreferenced", if strict { Severity::Error } else { Severity::Warning }, None, None, None, msg, None);
        }
    }
}

/// `tile_colors`, `tile_palettes`, `unique_tiles` (hard limits of the Game Boy Color tile hardware, as
/// `check_assets.py` counts them) and `alpha` / `size` for tile sets and screens.
fn tile_rules(cx: &mut Ctx) {
    use crate::tiles::{self, stats, Colors, Sheet, T, TRANSP};
    let part = cx.part;
    let Some(tpl) = cx.tpl else { return };
    let cols = Colors::new(part, tpl);
    let pr = part_ref(part);
    let en = |r: &str| cx.opts.enabled(r);
    let (e_colors, e_pal, e_uniq, e_alpha, e_size) = (en("tile_colors"), en("tile_palettes"), en("unique_tiles"), en("alpha"), en("size"));
    let prof = cx.prof;
    for fi in 0..part.frames.len() {
        let f = &part.frames[fi];
        let g = &f.grid;
        if e_size && (g.w, g.h) != part.size {
            cx.push("size", Severity::Error, Some(fi), None, None, format!("frame grid is {}x{} but the part canvas is {}x{}", g.w, g.h, part.size.0, part.size.1), None);
        }
        if g.w % T != 0 || g.h % T != 0 {
            cx.push("size", Severity::Error, Some(fi), None, None, format!("width and height must be multiples of 8, got {}x{}", g.w, g.h), None);
            continue;
        }
        if g.w == 0 || g.h == 0 {
            continue;
        }
        let sheet = Sheet::from_grid(g);
        let sets: Vec<Vec<u32>> = sheet.tiles.iter().map(|t| stats::colors_of(&t.px(&cols))).collect();
        let fr = if part.frames.len() > 1 { format!(" --frame {}", f.name) } else { String::new() };
        if e_alpha {
            let bad: Vec<usize> = (0..sets.len()).filter(|&i| sets[i].iter().any(|c| c & TRANSP != 0)).collect();
            if let Some(&first) = bad.first() {
                let (x, y) = sheet.pos(first);
                cx.push(
                    "alpha",
                    Severity::Error,
                    Some(fi),
                    Some(x as i32),
                    Some(y as i32),
                    format!("level tiles and screens must not contain transparency: {} tile(s) use the transparent key `.` (first: tile #{} at {},{})", bad.len(), first, x, y),
                    Some(format!("pxl tiles fit {}{} --apply   # replaces `.` with the air key", pr, fr)),
                );
            }
        }
        let max_colors = prof.colors_per_tile as usize;
        if e_colors {
            for (i, s) in sets.iter().enumerate() {
                if s.len() > max_colors {
                    let (x, y) = sheet.pos(i);
                    let list: Vec<String> = s.iter().map(|&c| tiles::hex_of(c)).collect();
                    cx.push(
                        "tile_colors",
                        Severity::Error,
                        Some(fi),
                        Some(x as i32),
                        Some(y as i32),
                        format!("tile #{} at x={}, y={} uses {} colours ({}), max {}", i, x, y, s.len(), list.join(" "), max_colors),
                        Some(format!("pxl tiles fit {}{} --apply", pr, fr)),
                    );
                }
            }
        }
        if e_pal {
            let pack = stats::greedy_sets_cap(&sets, max_colors);
            let (maxp, why) = match part.bg_palette {
                Some(b) => (1usize, format!(" (the part lives on BG palette {})", b)),
                None if !tiles::reserved_of(part, prof).is_empty() => (
                    (prof.palettes as usize).saturating_sub(tiles::reserved_of(part, prof).len()),
                    format!(" (BG palette {} reserved)", tiles::reserved_of(part, prof).iter().map(|b| b.to_string()).collect::<Vec<_>>().join(",")),
                ),
                None => (prof.palettes as usize, String::new()),
            };
            if pack.bins.len() > maxp {
                let opener = pack.opener[maxp];
                let (x, y) = sheet.pos(opener);
                let (opt, exact) = stats::optimal_sets_cap(&sets, max_colors);
                cx.push(
                    "tile_palettes",
                    Severity::Error,
                    Some(fi),
                    Some(x as i32),
                    Some(y as i32),
                    format!(
                        "colours need {} different {}-colour palettes (first-fit in sheet order, as check_assets.py counts), max {}{}; the first tile that does not fit is #{} at x={}, y={}; an optimal packing needs {}{}",
                        pack.bins.len(),
                        max_colors,
                        maxp,
                        why,
                        opener,
                        x,
                        y,
                        if exact { "" } else { "at most " },
                        opt
                    ),
                    Some(format!("pxl tiles fit {}{} --apply", pr, fr)),
                );
            }
        }
        if e_uniq && (f.name == "blocks" || part.max_tiles.is_some()) {
            let air = tiles::air_codes(tpl, &cols);
            let groups = if prof.pattern_unique {
                tiles::pattern::groups(&sheet, &cols, &air, true)
            } else if prof.dedupe_flips {
                stats::unique_groups(&sheet, &cols)
            } else {
                let mut seen: std::collections::BTreeMap<[u32; 64], usize> = std::collections::BTreeMap::new();
                let mut gs: Vec<Vec<usize>> = Vec::new();
                for (i, t) in sheet.tiles.iter().enumerate() {
                    let p = t.px(&cols);
                    match seen.get(&p) {
                        Some(&k) => gs[k].push(i),
                        None => {
                            seen.insert(p, gs.len());
                            gs.push(vec![i]);
                        }
                    }
                }
                gs
            };
            let anim = if prof.count_animated && f.name == "blocks" { tiles::slots::animated_slots(part, prof) } else { Default::default() };
            let maxu = crate::tiles::unique_limit(part, &prof);
            let total = groups.len() + anim.total;
            if total > maxu {
                let t = groups[maxu.saturating_sub(anim.total).min(groups.len() - 1)][0];
                let (x, y) = sheet.pos(t);
                let rule = if prof.pattern_unique { "same index pattern up to mirroring" } else if prof.dedupe_flips { "flips counted as duplicates" } else { "exact copies only" };
                let msg = if anim.total == 0 {
                    format!("{} unique tiles ({}), max {}; tile #{} at x={}, y={} is the first beyond the limit", groups.len(), rule, maxu, t, x, y)
                } else {
                    format!("{} unique tiles ({}) + {} animated slots ({}) = {}, max {}; tile #{} at x={}, y={} is the first beyond the limit", groups.len(), rule, anim.total, anim.describe(), total, maxu, t, x, y)
                };
                cx.push("unique_tiles", Severity::Error, Some(fi), Some(x as i32), Some(y as i32), msg, Some(format!("pxl tiles dedupe {}{} --max {} --apply", pr, fr, maxu.saturating_sub(anim.total))));
            }
        }
        if en("attr_block") && prof.attr_block.0 > 0 && prof.attr_block.1 > 0 {
            // one palette per attribute block (NES 16x16): the colours of all its tiles together must fit one palette
            let mut blocks: std::collections::BTreeMap<(usize, usize), (std::collections::BTreeSet<u32>, usize)> = std::collections::BTreeMap::new();
            for (i, s) in sets.iter().enumerate() {
                let (x, y) = sheet.pos(i);
                let e = blocks.entry((x / prof.attr_block.0 as usize, y / prof.attr_block.1 as usize)).or_insert_with(|| (std::collections::BTreeSet::new(), i));
                e.0.extend(s.iter().cloned());
            }
            for ((bx, by), (cols, first)) in &blocks {
                if cols.len() > max_colors {
                    let (x, y) = sheet.pos(*first);
                    cx.push("attr_block", Severity::Error, Some(fi), Some(x as i32), Some(y as i32), format!("the {}x{} attribute block at x={}, y={} uses {} colours, but one palette of the console holds {} (a block shares one palette)", prof.attr_block.0, prof.attr_block.1, bx * prof.attr_block.0 as usize, by * prof.attr_block.1 as usize, cols.len(), max_colors), None);
                }
            }
        }
        if en("priority_color0") && f.name == "blocks" {
            if let Some(ids) = &part.priority {
                let air = tiles::air_codes(tpl, &cols);
                let model = tiles::pattern::build(&sheet, &cols, &air, true);
                for &t in ids {
                    let (x, y) = if t < sheet.tiles.len() { sheet.pos(t) } else { (0, 0) };
                    if t >= sheet.tiles.len() {
                        cx.push("priority_color0", Severity::Error, Some(fi), None, None, format!("[priority] lists tile #{} but the sheet has {} tiles", t, sheet.tiles.len()), None);
                        continue;
                    }
                    if air.is_empty() {
                        cx.push("priority_color0", Severity::Error, Some(fi), Some(x as i32), Some(y as i32), format!("priority tile #{}: the template has no `[tiles] air` key, so the see-through part is not defined", t), None);
                        continue;
                    }
                    let here: Vec<u32> = sets[t].iter().copied().filter(|c| air.contains(c)).collect();
                    let bad = match (here.as_slice(), model.bin_of[t]) {
                        ([], _) => None,
                        ([a], Some(b)) => (model.bins[b].first() != Some(a)).then(|| format!("air colour {} is index {} of its palette", tiles::hex_of(*a), model.bins[b].iter().position(|c| c == a).unwrap_or(0))),
                        (many, _) => Some(format!("it uses {} different air colours ({})", many.len(), many.iter().map(|c| tiles::hex_of(*c)).collect::<Vec<_>>().join(" "))),
                    };
                    if let Some(why) = bad {
                        cx.push(
                            "priority_color0",
                            Severity::Error,
                            Some(fi),
                            Some(x as i32),
                            Some(y as i32),
                            format!("priority tile #{} at x={}, y={}: the see-through part must be colour 0 of its palette, but {}", t, x, y, why),
                            Some(format!("pxl tiles priority {}{} --clear {}   # or redraw the tile with one air colour", pr, fr, t)),
                        );
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------- part level

fn part_rules(cx: &mut Ctx, tpl_err: Option<String>) {
    let part = cx.part;
    if cx.opts.enabled("size") {
        if let Some((w, h)) = cx.prof.size {
            if part.size != (w as usize, h as usize) {
                cx.push("size", Severity::Error, None, None, None, format!("canvas must be exactly {}x{} px for profile {}, part size is {}x{}", w, h, cx.prof.name, part.size.0, part.size.1), None);
            }
        }
    }
    if !cx.opts.enabled("template") {
        return;
    }
    if part.template.is_none() {
        cx.push("template", Severity::Error, None, None, None, "part has no `template`".into(), Some("add template = \"templates/<name>.toml\" to the part header".into()));
    } else if let Some(e) = tpl_err {
        cx.push("template", Severity::Error, None, None, None, format!("template cannot be loaded: {}", e), None);
    }
    let Some(tpl) = cx.tpl else { return };
    for (k, v) in &part.colors {
        let known = if k.len() == 1 { tpl.def(k.as_bytes()[0]).is_some() } else { tpl.key_for_role(k).is_some() };
        if !known {
            cx.push("template", Severity::Error, None, None, None, format!("local colour override for '{}' ({}), which is not a key or role of template '{}'", k, v, tpl.name), None);
        } else {
            cx.push(
                "template",
                Severity::Error,
                None,
                None,
                None,
                format!("local colour override {} = {} departs from template '{}'", k, v, tpl.name),
                Some(format!("remove `{}` from [colors] in {}, or change the template colour: pxl template set {} {}={}", k, part_ref(part), quote(&tpl.path.slash()), k, v)),
            );
        }
    }
}

/// AC-TP3: parts of one explicit family (`family = "name"`) must use one template and no local colour
/// overrides. Returns issues for the parts that deviate from the family's most used template. Local overrides
/// are reported by [`lint_part`] itself.
pub fn family_issues(parts: &[&Part]) -> Vec<Issue> {
    let mut groups: BTreeMap<String, Vec<&Part>> = BTreeMap::new();
    for p in parts {
        if let Some(f) = &p.family {
            groups.entry(f.clone()).or_default().push(p);
        }
    }
    let mut out = Vec::new();
    for (family, members) in groups {
        if members.len() < 2 {
            continue;
        }
        let mut count: BTreeMap<String, usize> = BTreeMap::new();
        for p in &members {
            *count.entry(p.template.clone().unwrap_or_default()).or_default() += 1;
        }
        let mut best: (&String, usize) = (count.iter().next().unwrap().0, 0);
        for (t, c) in &count {
            if *c > best.1 {
                best = (t, *c);
            }
        }
        let fam = best.0.clone();
        for p in &members {
            let t = p.template.clone().unwrap_or_default();
            if t != fam && !t.is_empty() {
                out.push(Issue {
                    rule: "template".into(),
                    severity: Severity::Error,
                    file: file_of(p),
                    frame: None,
                    x: None,
                    y: None,
                    message: format!("part uses template '{}' but the other parts of family '{}' ({} of {}) use '{}'", t, family, best.1, members.len(), fam),
                    fix: Some(format!("set template = \"{}\" in {} and re-map the colours with its keys", fam, part_ref(p))),
                });
            }
        }
    }
    out.sort_by(|a, b| a.file.cmp(&b.file));
    out
}

// ---------------------------------------------------------------------------------------------- frame rules

fn frame_rules(cx: &mut Ctx, fi: usize) {
    let part = cx.part;
    let f = &part.frames[fi];
    let g = &f.grid;
    let en = |r: &str| cx.opts.enabled(r);
    let (e_size, e_pal, e_step, e_alpha, e_shared, e_empty, e_tpl, e_hw, e_orph, e_jag, e_noise) = (
        en("size"),
        en("palette_size"),
        en("rgb_step"),
        en("alpha"),
        en("shared_palette"),
        en("empty_frame"),
        en("template"),
        en("hw_cells"),
        en("orphan"),
        en("jaggy"),
        en("noise"),
    );
    if e_size && (g.w, g.h) != part.size {
        cx.push("size", Severity::Error, Some(fi), None, None, format!("frame grid is {}x{} but the part canvas is {}x{}", g.w, g.h, part.size.0, part.size.1), None);
    }
    if e_empty && bbox(g).is_none() {
        cx.push("empty_frame", Severity::Error, Some(fi), None, None, "frame is empty (every pixel is transparent)".into(), None);
    }
    if let Some(tpl) = cx.tpl {
        // keys missing from the template
        if e_tpl {
            let mut seen: Vec<(u8, i32, i32, usize)> = Vec::new();
            for y in 0..g.h {
                for x in 0..g.w {
                    let k = g.cells[y * g.w + x];
                    if tpl.def(k).is_none() {
                        match seen.iter_mut().find(|e| e.0 == k) {
                            Some(e) => e.3 += 1,
                            None => seen.push((k, x as i32, y as i32, 1)),
                        }
                    }
                }
            }
            for (k, x, y, n) in seen {
                let more = if n > 1 { format!(" ({} pixels)", n) } else { String::new() };
                cx.push(
                    "template",
                    Severity::Error,
                    Some(fi),
                    Some(x),
                    Some(y),
                    format!("key '{}' is not defined in template '{}'{}", kch(k), tpl.name, more),
                    Some(format!("pxl template remap {} {}=<new key>   # whole part; or pxl set {} {} {},{}=.   # or add \"{}\" to {}", part_ref(part), kch(k), part_ref(part), f.name, x, y, kch(k), tpl.path.slash())),
                );
            }
        }
        let lk = layer_keys(tpl, g);
        let limit = cx.prof.colors as usize;
        if e_pal && part.palette_set.is_none() {
            for (li, keys) in lk.iter().enumerate() {
                let lname = tpl.layers.get(li).map_or("base", |l| l.name.as_str());
                if keys.len() > limit {
                    let list: Vec<String> = keys.iter().map(|k| kch(k.0).to_string()).collect();
                    let (_, x, y) = keys[limit];
                    cx.push("palette_size", Severity::Error, Some(fi), Some(x), Some(y), format!("layer '{}' uses {} colours ({}), limit is {} plus transparent", lname, keys.len(), list.join(","), limit), None);
                } else {
                    for &(k, x, y) in keys {
                        let (_, slot) = slot_of(tpl, k).unwrap_or((li, 0));
                        if slot > limit {
                            cx.push("palette_size", Severity::Error, Some(fi), Some(x), Some(y), format!("key '{}' is palette slot {} of layer '{}' (template order); only slots 1..{} can be exported", kch(k), slot, lname, limit), None);
                        }
                    }
                }
            }
        }
        for (li, keys) in lk.iter().enumerate() {
            let lname = tpl.layers.get(li).map_or("base", |l| l.name.as_str());
            for (i, &(k, x, y)) in keys.iter().enumerate() {
                let c = effective_color(part, tpl, k);
                if e_alpha && c.is_none() {
                    cx.push("alpha", Severity::Error, Some(fi), Some(x), Some(y), format!("key '{}' has no opaque colour (palette entries other than index 0 must be fully opaque)", kch(k)), None);
                }
                let Some(c) = c else { continue };
                if e_step && !cx.prof.legal(c) {
                    let s = cx.prof.snap(c);
                    cx.push(
                        "rgb_step",
                        Severity::Error,
                        Some(fi),
                        Some(x),
                        Some(y),
                        if cx.prof.fixed_colours.is_empty() { format!("key '{}' colour {} has channels that are not multiples of {}; nearest legal colour is {}", kch(k), color::to_hex(c), cx.prof.channel_step, color::to_hex(s)) } else { format!("key '{}' colour {} is not in the {} palette of the console ({} colours); nearest legal colour is {}", kch(k), color::to_hex(c), cx.prof.name.split('/').next().unwrap_or(""), cx.prof.fixed_colours.len(), color::to_hex(s)) },
                        Some(format!("pxl template set {} {}={}", quote(&tpl.path.slash()), kch(k), color::to_hex(s))),
                    );
                }
                if e_shared && cx.prof.shared_palette {
                    if let Some(&(k0, _, _)) = keys[..i].iter().find(|o| effective_color(part, tpl, o.0) == Some(c)) {
                        cx.push("shared_palette", Severity::Error, Some(fi), Some(x), Some(y), format!("keys '{}' and '{}' of layer '{}' have the same colour {}; two palette entries must differ", kch(k0), kch(k), lname, color::to_hex(c)), None);
                    }
                }
            }
        }
    }
    if let (Some(res), Some(tpl)) = (cx.pal.clone(), cx.tpl) {
        let found = crate::cells::frame_findings(part, tpl, &res, cx.prof, fi, &|r| cx.opts.enabled(r));
        for fd in found {
            cx.push(fd.rule, Severity::Error, Some(fi), fd.x, fd.y, fd.msg, fd.fix);
        }
    } else {
        if cx.opts.enabled("overlay") && cx.prof.overlays != "allow" && part.palette_set.is_none() {
            overlay_rule(cx, fi);
        }
        if cx.prof.sprite_size_mode == "variable" {
            if cx.opts.enabled("hw_limit") && part.palette_set.is_none() {
                var_hw_limit(cx, fi);
            }
        } else {
            if e_hw && part.palette_set.is_none() {
                hw_cells(cx, fi);
            }
            if cx.opts.enabled("hw_limit") && part.palette_set.is_none() {
                hw_limit(cx, fi);
            }
        }
    }
    let outline = cx.tpl.and_then(|t| t.key_for_role("outline")).unwrap_or(b'1');
    if e_orph {
        orphans(cx, fi);
    }
    if e_jag {
        jaggies(cx, fi, outline);
    }
    if e_noise {
        noise(cx, fi);
    }
}

/// Overlay (detail) layers are off in the gbc profiles: an error when a frame uses a key of a layer after the first, unless
/// the part's category is listed in `overlay_categories` (profile `overlays = "category"`), then at most `overlay_cap`
/// overlay cells per frame.
fn overlay_rule(cx: &mut Ctx, fi: usize) {
    let (Some(tpl), part) = (cx.tpl, cx.part) else { return };
    let g = &part.frames[fi].grid;
    let used: Vec<u8> = {
        let mut v: Vec<u8> = Vec::new();
        for &k in &g.cells {
            if k != b'.' && tpl.layer_of(k).map_or(false, |l| l > 0) && !v.contains(&k) {
                v.push(k);
            }
        }
        v
    };
    if used.is_empty() {
        return;
    }
    let keys: String = used.iter().map(|k| (*k as char).to_string()).collect::<Vec<_>>().join(",");
    let cat = part.category.clone().unwrap_or_default();
    let allowed = cx.prof.overlays == "category" && cx.prof.overlay_categories.iter().any(|c| *c == cat);
    if !allowed {
        let why = if cx.prof.overlays == "category" { format!("only the categories {} may use them (this part's category: '{}')", cx.prof.overlay_categories.join(", "), if cat.is_empty() { "none" } else { &cat }) } else { "profile overlays = \"off\"".to_string() };
        cx.push("overlay", Severity::Error, Some(fi), None, None, format!("overlay layer keys ({}) are not allowed: {}; every overlay doubles the sprites of its cells", keys, why), Some("draw with one flat set of colours from the palette set (pxl palette fit), or give the part category = \"player\"".into()));
        return;
    }
    let hc = hw_counts(cx, fi);
    let cells: usize = hc.over.iter().sum();
    let cap = cx.prof.overlay_cap as usize;
    if cells > cap {
        cx.push("overlay", Severity::Error, Some(fi), None, None, format!("{} overlay cells in this frame ({}), the category '{}' may have at most {}", cells, keys, cat, cap), None);
    }
}

/// Hardware sprites per band (counted from the canvas top-left) as (base, overlay) cell counts, mirroring the exported PNGs:
/// the base PNG holds base keys and the `under` key of overlay pixels (so any painted pixel makes a base cell non-empty);
/// each overlay layer's PNG is its own palette, so its non-empty cells are extra sprites.
struct HwCounts {
    base: Vec<usize>,
    over: Vec<usize>,
}

impl HwCounts {
    fn band(&self, i: usize) -> usize {
        self.base[i] + self.over[i]
    }
    fn total(&self) -> usize {
        self.base.iter().chain(self.over.iter()).sum()
    }
    fn split(&self, i: usize) -> String {
        format!("{} base + {} overlay", self.base[i], self.over[i])
    }
    fn split_total(&self) -> String {
        format!("{} base + {} overlay", self.base.iter().sum::<usize>(), self.over.iter().sum::<usize>())
    }
}

fn hw_counts(cx: &Ctx, fi: usize) -> HwCounts {
    let g = &cx.part.frames[fi].grid;
    let (cw, ch) = (cx.prof.hw_cell.0 as usize, cx.prof.hw_cell.1 as usize);
    let overlay_layers: Vec<Vec<u8>> = cx.tpl.map(|t| t.layers.iter().skip(1).map(|l| l.keys.iter().map(|k| k.key).collect()).collect()).unwrap_or_default();
    let mut hc = HwCounts { base: Vec::new(), over: Vec::new() };
    let mut cy = 0;
    while cy < g.h {
        let (mut nb, mut no) = (0, 0);
        let mut cxx = 0;
        while cxx < g.w {
            let px = || (cy..(cy + ch).min(g.h)).flat_map(move |y| (cxx..(cxx + cw).min(g.w)).map(move |x| g.cells[y * g.w + x]));
            if px().any(|c| c != b'.') {
                nb += 1;
            }
            no += overlay_layers.iter().filter(|ks| px().any(|c| ks.contains(&c))).count();
            cxx += cw;
        }
        hc.base.push(nb);
        hc.over.push(no);
        cy += ch;
    }
    hc
}

fn hw_cells(cx: &mut Ctx, fi: usize) {
    let (cw, ch) = (cx.prof.hw_cell.0 as usize, cx.prof.hw_cell.1 as usize);
    let hc = hw_counts(cx, fi);
    let warn = cx.prof.hw_cells_warn as usize;
    let per = cx.prof.hw_cells_per_row_warn as usize;
    if hc.total() > warn {
        cx.push("hw_cells", Severity::Warning, Some(fi), None, None, format!("uses {} hardware sprites ({}x{} cells; {}), recommended <= {}", hc.total(), cw, ch, hc.split_total(), warn), None);
    }
    for i in 0..hc.base.len() {
        if hc.band(i) > per {
            cx.push("hw_cells", Severity::Warning, Some(fi), None, Some((i * ch) as i32), format!("uses {} hardware sprites side by side in the {} px band at y={} ({}), recommended <= {}", hc.band(i), ch, i * ch, hc.split(i), per), None);
        }
    }
}

/// Two colours of one palette (a template layer, or a slot of the part's palette sets) that are almost the same: the HUD with two
/// near-identical yellows. dE (Oklab x 100) below 4 is a warning, the palette keeps one of them.
fn near_duplicate_rule(cx: &mut Ctx) {
    if !cx.opts.enabled("near_duplicate") {
        return;
    }
    let de = |a: Rgb, b: Rgb| color::oklab_dist2(color::srgb_to_oklab(a), color::srgb_to_oklab(b)).sqrt() * 100.0;
    let mut found: Vec<String> = Vec::new();
    if let Some(tpl) = cx.tpl {
        for l in &tpl.layers {
            let cols: Vec<(u8, Rgb)> = l.keys.iter().filter_map(|k| k.color.map(|c| (k.key, c))).collect();
            for i in 0..cols.len() {
                for j in i + 1..cols.len() {
                    if cols[i].1 != cols[j].1 && de(cols[i].1, cols[j].1) < 4.0 {
                        found.push(format!("layer '{}': keys '{}' ({}) and '{}' ({}) are dE {:.1} apart", l.name, cols[i].0 as char, color::to_hex(cols[i].1), cols[j].0 as char, color::to_hex(cols[j].1), de(cols[i].1, cols[j].1)));
                    }
                }
            }
        }
    }
    if let Some(res) = cx.pal.clone() {
        for (s, name, cols) in &res.slots {
            for i in 0..3 {
                for j in i + 1..3 {
                    if cols[i] != cols[j] && de(cols[i], cols[j]) < 4.0 {
                        found.push(format!("palette set '{}' slot {} ({}): {} and {} are dE {:.1} apart", res.set.name, s, name, color::to_hex(cols[i]), color::to_hex(cols[j]), de(cols[i], cols[j])));
                    }
                }
            }
        }
    }
    for f in found.into_iter().take(6) {
        cx.push("near_duplicate", Severity::Warning, None, None, None, format!("near-duplicate colours in one palette: {}", f), Some("keep one of them; two almost identical tones waste a palette entry and read as one colour".into()));
    }
}

/// A part's colour layers are palette lines: base plus overlays may not use more lines than the console has for sprites.
fn palette_lines_rule(cx: &mut Ctx) {
    let (part, Some(tpl)) = (cx.part, cx.tpl) else { return };
    if !cx.opts.enabled("palette_lines") || part.palette_set.is_some() {
        return;
    }
    let max = cx.prof.obj_palettes as usize;
    let mut used = vec![false; tpl.layers.len()];
    for f in &part.frames {
        for &k in &f.grid.cells {
            if k != b'.' {
                if let Some((li, _)) = slot_of(tpl, k) {
                    if li < used.len() {
                        used[li] = true;
                    }
                }
            }
        }
    }
    let n = used.iter().filter(|u| **u).count();
    if max > 0 && n > max {
        cx.push("palette_lines", Severity::Error, None, None, None, format!("the part uses {} colour layers = {} palette lines, the console has {} for sprites", n, n, max), Some("draw with fewer layers (one palette line per layer)".into()));
    }
}

/// Consoles with per-sprite sizes (Mega Drive): cover the frame with the allowed sizes (`sprites::place`) and check the sprites on
/// screen, per scanline and the sprite pixels per scanline.
fn var_hw_limit(cx: &mut Ctx, fi: usize) {
    let g = &cx.part.frames[fi].grid;
    let fname = cx.part.frames[fi].name.clone();
    let placed = crate::sprites::place(g, &cx.prof.sprite_sizes);
    let (per, max, px) = (cx.prof.hw_cells_per_row_max as usize, cx.prof.hw_cells_max as usize, cx.prof.hw_pixels_per_line_max as usize);
    let sizes: std::collections::BTreeMap<(usize, usize), usize> = placed.iter().fold(std::collections::BTreeMap::new(), |mut m, p| {
        *m.entry((p.w, p.h)).or_insert(0) += 1;
        m
    });
    let list = sizes.iter().map(|((w, h), n)| format!("{}x {}x{}", n, w, h)).collect::<Vec<_>>().join(", ");
    if max > 0 && placed.len() > max {
        cx.push("hw_limit", Severity::Error, Some(fi), None, None, format!("frame '{}' needs {} sprites ({}), the hardware shows at most {} on screen; hint: use larger sprites, split the art across objects or frames", fname, placed.len(), list, max), None);
    }
    let loads = crate::sprites::line_loads(&placed, g.h);
    let mut reported = (false, false);
    for (y, (n, p)) in loads.iter().enumerate() {
        if per > 0 && *n > per && !reported.0 {
            reported.0 = true;
            cx.push("hw_limit", Severity::Error, Some(fi), None, Some(y as i32), format!("frame '{}': {} sprites on the scanline at y={} ({}), the hardware draws at most {} per scanline", fname, n, y, list, per), None);
        }
        if px > 0 && *p > px && !reported.1 {
            reported.1 = true;
            cx.push("hw_limit", Severity::Error, Some(fi), None, Some(y as i32), format!("frame '{}': {} sprite pixels on the scanline at y={}, the hardware draws at most {} per scanline", fname, p, y, px), None);
        }
    }
}

/// Hard hardware limits (GBC: 10 sprites per scanline, 40 on screen); a limit of 0 switches the check off.
fn hw_limit(cx: &mut Ctx, fi: usize) {
    let ch = cx.prof.hw_cell.1 as usize;
    let hc = hw_counts(cx, fi);
    let (per, max) = (cx.prof.hw_cells_per_row_max as usize, cx.prof.hw_cells_max as usize);
    let fname = cx.part.frames[fi].name.clone();
    if max > 0 && hc.total() > max {
        cx.push("hw_limit", Severity::Error, Some(fi), None, None, format!("frame '{}' needs {} sprite cells ({}), the hardware shows at most {} on screen; hint: split the sprite across objects or frames, or narrow it", fname, hc.total(), hc.split_total(), max), None);
    }
    if per > 0 {
        for i in 0..hc.base.len() {
            if hc.band(i) > per {
                cx.push("hw_limit", Severity::Error, Some(fi), None, Some((i * ch) as i32), format!("frame '{}': {} sprite cells in the {} px band at y={} ({}), the hardware draws at most {} per scanline; hint: split the sprite across objects or narrow this band", fname, hc.band(i), ch, i * ch, hc.split(i), per), None);
            }
        }
    }
}

fn orphans(cx: &mut Ctx, fi: usize) {
    let part = cx.part;
    let f = &part.frames[fi];
    let g = &f.grid;
    for y in 0..g.h as i32 {
        for x in 0..g.w as i32 {
            if is_opaque(g, x, y) && !N8.iter().any(|(dx, dy)| is_opaque(g, x + dx, y + dy)) {
                cx.push(
                    "orphan",
                    Severity::Advice,
                    Some(fi),
                    Some(x),
                    Some(y),
                    format!("possible stray pixel '{}': no opaque neighbour", kch(g.get(x, y))),
                    Some(format!("pxl set {} {} {},{}=.", part_ref(part), f.name, x, y)),
                );
            }
        }
    }
}

/// Outline pixel with exactly two outline 4-neighbours at a right angle (an L) whose opposite diagonal is not
/// outline, and where at least one of those neighbours is an L corner itself: a doubled staircase step that a
/// clean diagonal would not have. Rectangle corners (the neighbours continue straight) and 2:1 lines pass.
fn is_l_corner(g: &Grid, o: u8, x: i32, y: i32) -> Option<(i32, i32, i32, i32)> {
    if g.get(x, y) != o {
        return None;
    }
    let n: Vec<(i32, i32)> = [(0, -1), (1, 0), (0, 1), (-1, 0)].iter().cloned().filter(|(dx, dy)| g.get(x + dx, y + dy) == o).collect();
    if n.len() != 2 || (n[0].0 + n[1].0 == 0 && n[0].1 + n[1].1 == 0) {
        return None;
    }
    let (a, c) = (n[0], n[1]);
    // the 4th corner of the 2x2 square
    let d = (a.0 + c.0, a.1 + c.1);
    if g.get(x + d.0, y + d.1) == o {
        return None;
    }
    Some((a.0, a.1, c.0, c.1))
}

fn jaggies(cx: &mut Ctx, fi: usize, o: u8) {
    let part = cx.part;
    let f = &part.frames[fi];
    let g = &f.grid;
    // Removing one pixel of a staircase fixes its neighbours too: report a pixel only when no 4-neighbour
    // was reported before, so the listed fixes can all be applied together.
    let mut flagged: Vec<(i32, i32)> = Vec::new();
    for y in 0..g.h as i32 {
        for x in 0..g.w as i32 {
            let Some((ax, ay, cx_, cy_)) = is_l_corner(g, o, x, y) else { continue };
            if flagged.iter().any(|(fx, fy)| (fx - x).abs() + (fy - y).abs() == 1) {
                continue;
            }
            let nb_l = is_l_corner(g, o, x + ax, y + ay).is_some() || is_l_corner(g, o, x + cx_, y + cy_).is_some();
            if !nb_l {
                continue;
            }
            flagged.push((x, y));
            // replace by the key of the non-outline side neighbour that is opaque, else transparent
            let cands = [(-ax, -ay), (-cx_, -cy_)];
            let rep = cands.iter().map(|(dx, dy)| g.get(x + dx, y + dy)).find(|k| *k != b'.' && *k != o).unwrap_or(b'.');
            cx.push(
                "jaggy",
                Severity::Advice,
                Some(fi),
                Some(x),
                Some(y),
                "outline pixel forms an L corner inside a doubled diagonal step".to_string(),
                Some(format!("pxl set {} {} {},{}={}", part_ref(part), f.name, x, y, kch(rep))),
            );
        }
    }
}

/// Checkerboard patch: 2x2 windows `a b / b a` of two different opaque keys; a patch needs two windows that
/// overlap (a 3x2 or 2x3 alternating run). One issue per connected patch.
fn noise(cx: &mut Ctx, fi: usize) {
    let g = &cx.part.frames[fi].grid;
    let (w, h) = (g.w as i32, g.h as i32);
    let win = |x: i32, y: i32| -> bool {
        let (a, b, c, d) = (g.get(x, y), g.get(x + 1, y), g.get(x, y + 1), g.get(x + 1, y + 1));
        a != b'.' && b != b'.' && a != b && a == d && b == c && x + 1 < w && y + 1 < h
    };
    let mut flagged = vec![false; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            if win(x, y) && (win(x + 1, y) || win(x, y + 1) || (x > 0 && win(x - 1, y)) || (y > 0 && win(x, y - 1))) {
                flagged[(y * w + x) as usize] = true;
            }
        }
    }
    let mut seen = vec![false; flagged.len()];
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) as usize;
            if !flagged[i] || seen[i] {
                continue;
            }
            let mut stack = vec![(x, y)];
            seen[i] = true;
            let mut n = 0;
            while let Some((px, py)) = stack.pop() {
                n += 1;
                for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                    let (qx, qy) = (px + dx, py + dy);
                    if qx >= 0 && qy >= 0 && qx < w && qy < h {
                        let j = (qy * w + qx) as usize;
                        if flagged[j] && !seen[j] {
                            seen[j] = true;
                            stack.push((qx, qy));
                        }
                    }
                }
            }
            cx.push("noise", Severity::Advice, Some(fi), Some(x), Some(y), format!("checkerboard-like patch ({} alternating 2x2 windows)", n), None);
        }
    }
}

// ---------------------------------------------------------------------------------------------- animation

fn animation_rules(cx: &mut Ctx) {
    if !cx.opts.enabled("jitter") {
        return;
    }
    let part = cx.part;
    let mut tags: Vec<(String, Vec<usize>)> = Vec::new();
    for (i, f) in part.frames.iter().enumerate() {
        let t = tag_of(&f.name);
        match tags.iter_mut().find(|e| e.0 == t) {
            Some(e) => e.1.push(i),
            None => tags.push((t, vec![i])),
        }
    }
    for (tag, idx) in tags {
        let boxes: Vec<(usize, (i32, i32, i32, i32))> = idx.iter().filter_map(|&i| bbox(&part.frames[i].grid).map(|b| (i, b))).collect();
        if boxes.len() < 2 {
            continue;
        }
        // ground line: bottom row of the bbox; the most common value, ties to the earliest frame
        let mut best = (boxes[0].1 .3, 0usize);
        for (_, b) in &boxes {
            let n = boxes.iter().filter(|(_, c)| c.3 == b.3).count();
            if n > best.1 {
                best = (b.3, n);
            }
        }
        let ground = best.0;
        let mut cs: Vec<i32> = boxes.iter().map(|(_, b)| b.0 + b.2).collect(); // 2 * centre x
        cs.sort();
        let med2 = cs[(cs.len() - 1) / 2];
        for (i, b) in &boxes {
            if b.3 != ground {
                cx.push("jitter", Severity::Advice, Some(*i), Some(b.0), Some(b.3 - 1), format!("ground line (lowest row) is y={} while tag '{}' mostly rests on y={}", b.3 - 1, tag, ground - 1), None);
            }
            if ((b.0 + b.2) - med2).abs() > 4 {
                cx.push("jitter", Severity::Advice, Some(*i), Some(b.0), Some(b.1), format!("horizontal centre of the sprite moved {:.1} px against tag '{}'", ((b.0 + b.2) - med2) as f64 / 2.0, tag), None);
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------- references

/// Scale of reference pixels per target pixel: `input.factor`, else `reduce.factor`, else `input.target`
/// relative to the image, else 1.
pub fn ref_factor(part: &Part, rf: &RefFrame) -> (f64, f64) {
    if let Some(f) = part.input.factor.or(part.reduce.factor) {
        return (f as f64, f as f64);
    }
    if let Some((tw, th)) = part.input.target {
        if tw > 0 && th > 0 {
            return (rf.img.w as f64 / tw as f64, rf.img.h as f64 / th as f64);
        }
    }
    (1.0, 1.0)
}

/// Reference rectangle (x0, y0, x1, y1 exclusive, in reference pixels) of target pixel (x, y).
fn ref_rect(part: &Part, rf: &RefFrame, x: i32, y: i32) -> (i64, i64, i64, i64) {
    let (fx, fy) = ref_factor(part, rf);
    let ox = rf.origin.0 as f64 + (x - part.origin.0) as f64 * fx;
    let oy = rf.origin.1 as f64 + (y - part.origin.1) as f64 * fy;
    (ox.round() as i64, oy.round() as i64, (ox + fx).round() as i64, (oy + fy).round() as i64)
}

fn ref_rules(cx: &mut Ctx, refs: &[Option<RefFrame>]) {
    let part = cx.part;
    // shimmer
    if cx.opts.enabled("shimmer") {
        for i in 1..part.frames.len() {
            if tag_of(&part.frames[i].name) != tag_of(&part.frames[i - 1].name) {
                continue;
            }
            let (Some(Some(ra)), Some(Some(rb))) = (refs.get(i - 1), refs.get(i)) else { continue };
            let (ga, gb) = (&part.frames[i - 1].grid, &part.frames[i].grid);
            let mut first: Option<(i32, i32)> = None;
            let mut n = 0;
            for y in 0..gb.h.min(ga.h) as i32 {
                for x in 0..gb.w.min(ga.w) as i32 {
                    if ga.get(x, y) == gb.get(x, y) {
                        continue;
                    }
                    // only when the reference is unchanged here and around (a changing neighbourhood means the
                    // art is supposed to move: compare reference deltas, not just output deltas)
                    let calm = (-1..=1).all(|dy| (-1..=1).all(|dx| ref_blocks_equal(part, ra, rb, x + dx, y + dy)));
                    if calm {
                        n += 1;
                        first.get_or_insert((x, y));
                    }
                }
            }
            if let Some((x, y)) = first {
                cx.push("shimmer", Severity::Advice, Some(i), Some(x), Some(y), format!("{} pixel(s) differ from frame '{}' where the reference is unchanged", n, part.frames[i - 1].name), None);
            }
        }
    }
    for (i, f) in part.frames.iter().enumerate() {
        let Some(Some(rf)) = refs.get(i) else { continue };
        if !cx.opts.frames.is_empty() && !cx.opts.frames.iter().any(|n| *n == f.name) {
            continue;
        }
        if cx.opts.enabled("ref_scale") {
            if let Some(msg) = ref_scale_issue(part, f, rf) {
                cx.push("ref_scale", Severity::Advice, Some(i), None, None, msg, None);
            }
        }
        if cx.opts.enabled("lost_eye") {
            if let Some(tpl) = cx.tpl {
                let (_, lost) = eye_losses(part, tpl, &f.grid, rf);
                for (lf, e) in lost {
                    let fix = eye_fix_pixels(part, tpl, &f.grid, rf, &e).map(|px| format!("pxl set {} {} {}", part_ref(part), quote(&f.name), px));
                    cx.push(
                        "lost_eye",
                        Severity::Advice,
                        Some(i),
                        Some(lf.x),
                        Some(lf.y),
                        format!("the reference shows an eye here (white {} px, gaze {:+.1},{:+.1} cells); the art has no white cell with a pupil on that side", lf.size, e.gaze.0, e.gaze.1),
                        fix,
                    );
                }
            }
        }
        if cx.opts.enabled("colour_share") {
            if let Some(tpl) = cx.tpl {
                for sh in crate::features::colour_shares(part, tpl, &f.grid, rf) {
                    let name = tpl.def(sh.key).map_or(String::new(), |d| d.role.clone());
                    if sh.deficit() >= 1.0 && (sh.out_cells as f64) < 0.6 * sh.ref_cells {
                        cx.push(
                            "colour_share",
                            Severity::Advice,
                            Some(i),
                            None,
                            None,
                            format!("key '{}' ({}) covers {} cells but the reference holds about {:.0} cells of it at target scale", sh.key as char, name, sh.out_cells, sh.ref_cells),
                            None,
                        );
                    } else if sh.surplus() {
                        cx.push(
                            "colour_share",
                            Severity::Advice,
                            Some(i),
                            None,
                            None,
                            format!("key '{}' ({}) covers {} cells, far more than the reference's {:.0} cells at target scale", sh.key as char, name, sh.out_cells, sh.ref_cells),
                            None,
                        );
                    }
                }
            }
        }
        if cx.opts.enabled("lost_feature") {
            if let Some(tpl) = cx.tpl {
                for lf in lost_features(part, tpl, f, rf) {
                    cx.push(
                        "lost_feature",
                        Severity::Advice,
                        Some(i),
                        Some(lf.x),
                        Some(lf.y),
                        format!("the reference has a {} detail here ({} px, contrast {:.2}); the art shows none", if lf.dark { "dark" } else { "light" }, lf.size, lf.contrast),
                        None,
                    );
                }
            }
        }
    }
}

fn ref_blocks_equal(part: &Part, a: &RefFrame, b: &RefFrame, x: i32, y: i32) -> bool {
    let (ax0, ay0, ax1, ay1) = ref_rect(part, a, x, y);
    let (bx0, by0, bx1, by1) = ref_rect(part, b, x, y);
    if ax1 - ax0 != bx1 - bx0 || ay1 - ay0 != by1 - by0 {
        return false;
    }
    for dy in 0..(ay1 - ay0) {
        for dx in 0..(ax1 - ax0) {
            let pa = a.img.get((ax0 + dx) as i32, (ay0 + dy) as i32);
            let pb = b.img.get((bx0 + dx) as i32, (by0 + dy) as i32);
            if !(pa == pb || (pa.a == 0 && pb.a == 0)) {
                return false;
            }
        }
    }
    true
}

/// `ref_scale`: art bbox against the reference bbox at target scale; more than 1 px off on any side.
pub fn ref_scale_issue(part: &Part, f: &Frame, rf: &RefFrame) -> Option<String> {
    let (rx0, ry0, rx1, ry1) = rf.img.bbox()?;
    let (fx, fy) = ref_factor(part, rf);
    let t = (
        part.origin.0 as f64 + (rx0 as f64 - rf.origin.0 as f64) / fx,
        part.origin.1 as f64 + (ry0 as f64 - rf.origin.1 as f64) / fy,
        part.origin.0 as f64 + (rx1 as f64 - rf.origin.0 as f64) / fx,
        part.origin.1 as f64 + (ry1 as f64 - rf.origin.1 as f64) / fy,
    );
    let Some(a) = bbox(&f.grid) else {
        return Some(format!("art is empty but the reference spans x {:.1}..{:.1}, y {:.1}..{:.1}", t.0, t.2, t.1, t.3));
    };
    let d = [("left", a.0 as f64 - t.0), ("top", a.1 as f64 - t.1), ("right", a.2 as f64 - t.2), ("bottom", a.3 as f64 - t.3)];
    let bad: Vec<String> = d.iter().filter(|(_, v)| v.abs() > 1.0 + 1e-9).map(|(n, v)| format!("{} {:+.1}", n, v)).collect();
    if bad.is_empty() {
        None
    } else {
        Some(format!("art bbox x {}..{}, y {}..{} vs reference at target scale x {:.1}..{:.1}, y {:.1}..{:.1}: off by more than 1 px ({})", a.0, a.2, a.1, a.3, t.0, t.2, t.1, t.3, bad.join(", ")))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LostFeature {
    /// Top-left pixel (reading order) of the detail, in target coordinates.
    pub x: i32,
    pub y: i32,
    pub size: usize,
    pub dark: bool,
    pub contrast: f64,
    /// `detail` (small light/dark detail) or `eye`.
    pub kind: &'static str,
    /// Footprint in cells (1x1 for details).
    pub w: i32,
    pub h: i32,
}

fn median(v: &mut Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[(v.len() - 1) / 2]
}

/// Lightness (Oklab L) deviation of each opaque pixel from the median of its opaque 8-neighbours.
/// `None` for transparent pixels or pixels with fewer than `min_nb` opaque neighbours.
fn deviation_map(l: &[Option<f64>], w: usize, h: usize, min_nb: usize) -> Vec<Option<f64>> {
    let mut out = vec![None; w * h];
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let Some(c) = l[y as usize * w + x as usize] else { continue };
            let mut nb: Vec<f64> = Vec::new();
            for (dx, dy) in N8 {
                let (qx, qy) = (x + dx, y + dy);
                if qx >= 0 && qy >= 0 && (qx as usize) < w && (qy as usize) < h {
                    if let Some(v) = l[qy as usize * w + qx as usize] {
                        nb.push(v);
                    }
                }
            }
            if nb.len() >= min_nb {
                out[y as usize * w + x as usize] = Some(c - median(&mut nb));
            }
        }
    }
    out
}

/// Simple lost-feature detector (kept standalone so `compare` can reuse or replace it): the reference is
/// reduced to target scale (mean colour of blocks that are at least half covered); pixels whose lightness
/// differs from the median of their neighbours by >= 0.25 and that form groups of at most 6 pixels (isolated
/// details, thin bands) are features. A feature is lost when no pixel in the 3x3 around any of its pixels in
/// the art deviates the same way by >= 0.10.
pub fn lost_features(part: &Part, tpl: &Template, f: &Frame, rf: &RefFrame) -> Vec<LostFeature> {
    let (w, h) = (f.grid.w, f.grid.h);
    let mut rl: Vec<Option<f64>> = vec![None; w * h];
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let (x0, y0, x1, y1) = ref_rect(part, rf, x, y);
            let (mut n, mut tot, mut sr, mut sg, mut sb) = (0u32, 0u32, 0u32, 0u32, 0u32);
            for ry in y0..y1.max(y0 + 1) {
                for rx in x0..x1.max(x0 + 1) {
                    tot += 1;
                    let p = rf.img.get(rx as i32, ry as i32);
                    if p.a > 0 {
                        n += 1;
                        sr += p.r as u32;
                        sg += p.g as u32;
                        sb += p.b as u32;
                    }
                }
            }
            if n > 0 && n * 2 >= tot {
                let c = Rgb { r: (sr / n) as u8, g: (sg / n) as u8, b: (sb / n) as u8 };
                rl[y as usize * w + x as usize] = Some(color::srgb_to_oklab(c).l);
            }
        }
    }
    let al: Vec<Option<f64>> = f
        .grid
        .cells
        .iter()
        .map(|&k| if k == b'.' { None } else { effective_color(part, tpl, k).map(|c| color::srgb_to_oklab(c).l) })
        .collect();
    let rd = deviation_map(&rl, w, h, 3);
    let ad = deviation_map(&al, w, h, 2);
    let cand: Vec<Option<bool>> = rd.iter().map(|d| d.filter(|v| v.abs() >= 0.25).map(|v| v < 0.0)).collect();
    let mut seen = vec![false; w * h];
    let mut out = Vec::new();
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let i = y as usize * w + x as usize;
            let Some(dark) = cand[i] else { continue };
            if seen[i] {
                continue;
            }
            let mut comp = vec![(x, y)];
            seen[i] = true;
            let mut k = 0;
            while k < comp.len() {
                let (px, py) = comp[k];
                k += 1;
                for (dx, dy) in N8 {
                    let (qx, qy) = (px + dx, py + dy);
                    if qx >= 0 && qy >= 0 && (qx as usize) < w && (qy as usize) < h {
                        let j = qy as usize * w + qx as usize;
                        if !seen[j] && cand[j] == Some(dark) {
                            seen[j] = true;
                            comp.push((qx, qy));
                        }
                    }
                }
            }
            if comp.len() > 6 {
                continue;
            }
            let sign = if dark { -1.0 } else { 1.0 };
            let found = comp.iter().any(|&(px, py)| {
                (-1..=1).any(|dy| {
                    (-1..=1).any(|dx| {
                        let (qx, qy) = (px + dx, py + dy);
                        qx >= 0 && qy >= 0 && (qx as usize) < w && (qy as usize) < h && ad[qy as usize * w + qx as usize].map_or(false, |v| sign * v >= 0.10)
                    })
                })
            });
            if !found {
                let contrast = comp.iter().map(|&(px, py)| rd[py as usize * w + px as usize].unwrap().abs()).fold(0.0, f64::max);
                let (mx, my) = comp.iter().fold((x, y), |m, &(px, py)| if (py, px) < (m.1, m.0) { (px, py) } else { m });
                out.push(LostFeature { x: mx, y: my, size: comp.len(), dark, contrast, kind: "detail", w: 1, h: 1 });
            }
        }
    }
    out.sort_by_key(|l| (l.y, l.x));
    // details inside a lost eye are reported by the eye
    let (_, lost_eyes) = eye_losses(part, tpl, &f.grid, rf);
    out.retain(|d| !lost_eyes.iter().any(|(_, e)| d.x >= e.cells.0 - 1 && d.x <= e.cells.2 + 1 && d.y >= e.cells.1 - 1 && d.y <= e.cells.3 + 1));
    out
}

/// Eyes of the reference that the grid does not draw: `(eyes found, [(lost entry, eye)])`. The one eye check (`features::eye_check`) shared with the `eye_parity` gate.
pub fn eye_losses(part: &Part, tpl: &Template, grid: &Grid, rf: &RefFrame) -> (usize, Vec<(LostFeature, crate::features::Feature)>) {
    let chk = crate::features::eye_check(part, tpl, grid, rf);
    let v = chk
        .lost()
        .into_iter()
        .map(|i| {
            let e = chk.eyes[i].clone();
            (LostFeature { x: e.cells.0, y: e.cells.1, size: e.white.len(), dark: false, contrast: 1.0, kind: "eye", w: e.cells.2 - e.cells.0 + 1, h: e.cells.3 - e.cells.1 + 1 }, e)
        })
        .collect();
    (chk.found(), v)
}

/// Total `eye_mismatch` over the eyes of a reference (0 without eyes or keys).
pub fn eye_error(part: &Part, tpl: &Template, grid: &Grid, rf: &RefFrame) -> usize {
    let Some(k) = crate::features::keys_for(tpl) else { return 0 };
    crate::features::eyes_of(part, rf).iter().map(|e| crate::features::eye_mismatch(grid, e, &k)).sum()
}

/// `pxl set` arguments (`x,y=K ...`) that draw the minimal eye for a lost eye, if the stamping pass can.
pub fn eye_fix_pixels(part: &Part, tpl: &Template, grid: &Grid, rf: &RefFrame, eye: &crate::features::Feature) -> Option<String> {
    let geo = crate::features::Geo::of(part, rf)?;
    let mut g = grid.clone();
    crate::features::stamp(&mut g, std::slice::from_ref(eye), tpl, geo.f, &[]);
    let mut px = Vec::new();
    for y in 0..g.h as i32 {
        for x in 0..g.w as i32 {
            if g.get(x, y) != grid.get(x, y) {
                px.push(format!("{},{}={}", x, y, g.get(x, y) as char));
            }
        }
    }
    if px.is_empty() {
        None
    } else {
        Some(px.join(" "))
    }
}

// ---------------------------------------------------------------------------------------------- CLI

struct PartReport {
    part: Part,
    profile: String,
    issues: Vec<Issue>,
    skipped: Option<String>,
}

fn lint_path(path: &Path, a: &LintArgs, opts: &LintOpts) -> Result<PartReport> {
    let part = crate::pxl::load(path)?;
    let prof = profile::load_for(&part, a.profile.as_deref())?;
    let mut o = opts.clone();
    if !a.sel.all && !a.sel.frames.is_empty() {
        o.frames = a.sel.select(&part)?.iter().map(|f| f.name.clone()).collect();
    }
    let issues = lint_part_auto(&part, &prof, &o);
    Ok(PartReport { part, profile: prof.name, issues, skipped: None })
}

pub fn run_lint(a: &LintArgs, out: &mut Out) -> Result<()> {
    for r in &a.rules {
        if !SPRITE_RULES.contains(&r.as_str()) && !TILE_RULES.contains(&r.as_str()) && !CONSOLE_RULES.contains(&r.as_str()) && !crate::design::DESIGN_RULES.contains(&r.as_str()) {
            let all: Vec<&str> = SPRITE_RULES.iter().chain(TILE_RULES.iter()).chain(CONSOLE_RULES.iter()).chain(crate::design::DESIGN_RULES.iter()).cloned().collect();
            return Err(PxlError::usage(format!("unknown lint rule '{}'", r)).with_hint(format!("rules: {}", all.join(", "))));
        }
    }
    let opts = LintOpts { rules: a.rules.clone(), frames: Vec::new(), refs: true, advice: a.advice };
    let paths = fsutil::expand_parts(&a.parts)?;
    let results = fsutil::par_map(&paths, |p| lint_path(p, a, &opts));
    let mut reports = Vec::new();
    for r in results {
        reports.push(r?);
    }
    if opts.enabled("template") {
        // family check: only parts declaring the same `family` (in the part, else in the manifest) are compared.
        // Candidates: the selected parts, their siblings in the same directories, and the manifest's parts.
        let manifest = crate::manifest::Manifest::discover(None).ok().flatten();
        let fam_of = |pt: &mut Part| {
            if pt.family.is_none() {
                if let Some(d) = manifest.as_ref().and_then(|m| m.find(&pt.path)) {
                    pt.family = d.family.clone();
                }
            }
        };
        let mut all: Vec<Part> = Vec::new();
        let mut seen: Vec<PathBuf> = Vec::new();
        let mut cand: Vec<PathBuf> = Vec::new();
        for r in &reports {
            let mut pt = r.part.clone();
            fam_of(&mut pt);
            if pt.family.is_none() {
                continue;
            }
            seen.push(crate::manifest::canon(&pt.path));
            all.push(pt);
            if let Ok(rd) = std::fs::read_dir(r.part.dir()) {
                for e in rd.flatten() {
                    let p = e.path();
                    if p.extension().map_or(false, |x| x == "pxl") {
                        cand.push(p);
                    }
                }
            }
        }
        if let Some(m) = &manifest {
            cand.extend(m.parts.iter().map(|d| d.path.clone()));
        }
        cand.sort();
        cand.dedup();
        if !all.is_empty() {
            for p in cand {
                let cp = crate::manifest::canon(&p);
                if seen.contains(&cp) {
                    continue;
                }
                if let Ok(mut pt) = crate::pxl::load(&p) {
                    fam_of(&mut pt);
                    if all.iter().any(|a| a.family == pt.family) {
                        seen.push(cp);
                        all.push(pt);
                    }
                }
            }
        }
        let refs: Vec<&Part> = all.iter().collect();
        for iss in family_issues(&refs) {
            if let Some(r) = reports.iter_mut().find(|r| file_of(&r.part) == iss.file) {
                r.issues.push(iss);
            }
        }
        for r in reports.iter_mut() {
            let part = &r.part;
            r.issues.sort_by(|x, y| {
                let k = |i: &Issue| (i.frame.as_ref().map_or(0, |f| part.frame_index(f).map_or(0, |n| n + 1)), i.y.unwrap_or(-1), i.x.unwrap_or(-1));
                k(x).cmp(&k(y)).then_with(|| x.rule.cmp(&y.rule))
            });
        }
    }
    let legacy: Vec<String> = reports.iter().filter_map(|r| crate::console::legacy_name_of(&r.part)).collect();
    if let Some(n) = crate::console::deprecated_profile_note(&legacy) {
        out.warn(n);
    }
    if let Some(r) = reports.first() {
        if let Some(n) = crate::console::no_console_note(&r.part.dir()) {
            out.warn(n);
        }
    }
    let (mut ne, mut nw, mut nn) = (0usize, 0usize, 0usize);
    let mut parts_json = Vec::new();
    for r in &reports {
        let e = r.issues.iter().filter(|i| i.severity == Severity::Error).count();
        let w = r.issues.iter().filter(|i| i.severity == Severity::Warning).count();
        let n = r.issues.len() - e - w;
        nn += n;
        ne += e;
        nw += w;
        if let Some(s) = &r.skipped {
            out.warn(s.clone());
        }
        // `pxl status` shows what the last full lint said (warnings included), so the two never disagree
        if a.rules.is_empty() && a.sel.frames.is_empty() && r.skipped.is_none() && crate::manifest::Manifest::find_file(&r.part.dir()).is_some() {
            crate::status::record(&r.part.path, &r.part, crate::status::StatusEvent::LintSummary { errors: e as u32, warnings: w as u32, infos: n as u32 });
        }
        out.line(format!("{} ({}): {} error(s), {} warning(s){}", file_of(&r.part), r.profile, e, w, if n > 0 { format!(", {} note(s)", n) } else { String::new() }));
        for i in &r.issues {
            out.line(format!("  {}", i.to_text().replace("\n", "\n  ")));
        }
        parts_json.push(json!({
            "part": r.part.name,
            "file": file_of(&r.part),
            "profile": r.profile,
            "frames": r.part.frames.len(),
            "errors": e,
            "warnings": w,
            "notes": n,
            "issues": r.issues.iter().map(|i| i.to_json()).collect::<Vec<_>>(),
        }));
    }
    out.line(format!("{} part(s): {} error(s), {} warning(s){}", reports.len(), ne, nw, if nn > 0 { format!(", {} note(s) (information, not failures)", nn) } else { String::new() }));
    out.set("schema", json!(1));
    out.set("parts", Value::Array(parts_json));
    out.set("summary", json!({ "parts": reports.len(), "errors": ne, "warnings": nw, "notes": nn, "issues": ne + nw + nn }));
    if ne > 0 {
        return Err(PxlError::lint(format!("{} lint error(s) in {} part(s)", ne, reports.iter().filter(|r| r.issues.iter().any(|i| i.severity == Severity::Error)).count())));
    }
    Ok(())
}
