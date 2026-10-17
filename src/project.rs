//! Single-image mode and project mode (M6 R4): `pxl image`, `pxl new`, `pxl ingest`, `pxl group`, `pxl tree`,
//! `pxl project show`.

use crate::fsutil::PathSlash;
use crate::asset::{self, AssetInfo};
use crate::cli::*;
use crate::img::{Rgba, RgbaImage};
use crate::manifest::Manifest;
use crate::color::Rgb;
use crate::model::{Frame, Grid, Part, PartKind};
use crate::output::{Out, PxlError, Result};
use crate::{console, fsutil, journal, lease, load, png, pxl, status, suggest, versions};
use serde_json::json;
use std::path::{Path, PathBuf};

// ------------------------------------------------------------------------------------- defaults

/// `[project.defaults]` of pxl.toml.
#[derive(Clone, Debug, Default)]
pub struct Defaults {
    pub mode: Option<String>,
    pub size: Option<(usize, usize)>,
    pub factor: Option<u32>,
    pub category: Option<String>,
    pub palette_set: Option<String>,
    pub template: Option<String>,
    /// Group name -> hardware category: `pxl ingest --group player` makes player assets (two-slot cells allowed).
    /// `[project.defaults] group_categories = { player = "player", enemies = "enemy" }`; the built-in mapping is `player = "player"`.
    pub group_categories: std::collections::BTreeMap<String, String>,
}

impl Defaults {
    /// The category a new asset of `group` gets: the group mapping first (the group is the more specific word), then the project default.
    pub fn category_for(&self, group: &str) -> Option<String> {
        let parts: Vec<&str> = group.split('/').filter(|p| !p.is_empty()).collect();
        for p in parts.iter().rev() {
            if let Some(c) = self.group_categories.get(*p) {
                return Some(c.clone());
            }
        }
        self.category.clone()
    }
}

pub fn project_table(manifest: &Path) -> toml::Table {
    std::fs::read_to_string(manifest).ok().and_then(|t| t.parse::<toml::Table>().ok()).and_then(|t| t.get("project").and_then(|p| p.as_table().cloned())).unwrap_or_default()
}

pub fn defaults_of(manifest: &Path) -> Defaults {
    let t = project_table(manifest);
    let d = t.get("defaults").and_then(|d| d.as_table()).cloned().unwrap_or_default();
    let s = |k: &str| d.get(k).and_then(|v| v.as_str()).filter(|v| !v.is_empty()).map(String::from);
    Defaults {
        mode: s("mode"),
        size: d.get("size").and_then(|v| v.as_array()).filter(|a| a.len() == 2).and_then(|a| Some((a[0].as_integer()? as usize, a[1].as_integer()? as usize))),
        factor: d.get("factor").and_then(|v| v.as_integer()).map(|n| n.max(1) as u32),
        category: s("category"),
        palette_set: s("palette_set"),
        template: s("template"),
        group_categories: {
            let mut m: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
            m.insert("player".into(), "player".into());
            if let Some(t) = d.get("group_categories").and_then(|v| v.as_table()) {
                for (k, v) in t {
                    match v.as_str() {
                        Some("") | Some("none") => {
                            m.remove(k);
                        }
                        Some(c) => {
                            m.insert(k.clone(), c.to_string());
                        }
                        None => {}
                    }
                }
            }
            m
        },
    }
}

fn parse_mode(m: &str) -> Result<PartKind> {
    PartKind::from_text(m).ok_or_else(|| PxlError::usage(format!("unknown mode '{}'", m)).with_hint("modes: sprite, tile, screen"))
}

// ------------------------------------------------------------------------------------- sheets

/// Cut `im` into frames of `gw` x `gh`; empty cells are skipped.
pub fn cut_grid(im: &RgbaImage, gw: usize, gh: usize) -> Vec<RgbaImage> {
    let mut v = Vec::new();
    if gw == 0 || gh == 0 {
        return v;
    }
    for cy in 0..im.h / gh {
        for cx in 0..im.w / gw {
            let c = im.crop((cx * gw) as i32, (cy * gh) as i32, gw, gh);
            if c.px.iter().any(|p| p.a > 0) {
                v.push(c);
            }
        }
    }
    v
}

/// Connected regions of the picture (8-neighbourhood, regions within 2 px of each other are one sprite), as frames on a common canvas
/// aligned at the bottom centre. The background is transparency, else the colour of the top-left pixel.
pub fn cut_regions(im: &RgbaImage) -> Vec<RgbaImage> {
    let bg = if im.px.iter().any(|p| p.a == 0) { None } else { im.px.first().copied() };
    let solid = |p: &Rgba| p.a > 0 && Some(*p) != bg;
    let (w, h) = (im.w, im.h);
    let mut label = vec![usize::MAX; w * h];
    let mut boxes: Vec<(usize, usize, usize, usize)> = Vec::new(); // x0,y0,x1,y1 inclusive
    for s in 0..w * h {
        if label[s] != usize::MAX || !solid(&im.px[s]) {
            continue;
        }
        let id = boxes.len();
        let mut b = (s % w, s / w, s % w, s / w);
        let mut st = vec![s];
        label[s] = id;
        while let Some(i) = st.pop() {
            let (x, y) = ((i % w) as i32, (i / w) as i32);
            b = (b.0.min(x as usize), b.1.min(y as usize), b.2.max(x as usize), b.3.max(y as usize));
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h {
                        let j = ny as usize * w + nx as usize;
                        if label[j] == usize::MAX && solid(&im.px[j]) {
                            label[j] = id;
                            st.push(j);
                        }
                    }
                }
            }
        }
        boxes.push(b);
    }
    // merge boxes that touch or lie within 2 px
    let g = 2usize;
    loop {
        let mut merged = false;
        'o: for i in 0..boxes.len() {
            for j in i + 1..boxes.len() {
                let (a, b) = (boxes[i], boxes[j]);
                if a.0 <= b.2 + g && b.0 <= a.2 + g && a.1 <= b.3 + g && b.1 <= a.3 + g {
                    boxes[i] = (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3));
                    boxes.remove(j);
                    merged = true;
                    break 'o;
                }
            }
        }
        if !merged {
            break;
        }
    }
    // drop specks (fewer than 4 px of area)
    boxes.retain(|b| (b.2 - b.0 + 1) * (b.3 - b.1 + 1) >= 4);
    // reading order: rows of similar height first
    let band = boxes.iter().map(|b| b.3 - b.1 + 1).max().unwrap_or(1).max(1);
    boxes.sort_by_key(|b| ((b.1 + b.3) / 2 / band, b.0));
    let (cw, ch) = (boxes.iter().map(|b| b.2 - b.0 + 1).max().unwrap_or(1), boxes.iter().map(|b| b.3 - b.1 + 1).max().unwrap_or(1));
    boxes
        .iter()
        .map(|b| {
            let (bw, bh) = (b.2 - b.0 + 1, b.3 - b.1 + 1);
            let mut f = RgbaImage::new(cw, ch);
            let (ox, oy) = ((cw - bw) / 2, ch - bh);
            for y in 0..bh {
                for x in 0..bw {
                    let p = im.px[(b.1 + y) * w + b.0 + x];
                    if solid(&p) {
                        f.px[(oy + y) * cw + ox + x] = p;
                    }
                }
            }
            f
        })
        .collect()
}

// ------------------------------------------------------------------------------------- creating an asset

struct Source {
    /// Asset name.
    name: String,
    /// Files copied into `input/` (frame name = file stem), sorted.
    files: Vec<PathBuf>,
    /// A single sheet to cut into frames.
    sheet: bool,
    /// Frame names when they are not the file stems (a numbered sequence).
    names: Vec<String>,
}

struct Plan {
    mode: PartKind,
    console: Option<String>,
    size: Option<(usize, usize)>,
    factor: Option<u32>,
    grid: Option<Size>,
    auto_frames: bool,
    defaults: Defaults,
}

fn stem(p: &Path) -> String {
    p.file_stem().and_then(|s| s.to_str()).unwrap_or("frame").to_string()
}

fn safe_name(s: &str) -> String {
    let n: String = s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect();
    if n.is_empty() {
        "asset".into()
    } else {
        n
    }
}

fn is_png(p: &Path) -> bool {
    p.extension().map_or(false, |e| e.eq_ignore_ascii_case("png"))
}

/// Copy the source images into `<asset>/input/` and cut sheets into `input/frames/`. Returns (frame name, path relative to the asset).
fn stage_inputs(dir: &Path, src: &Source, plan: &Plan) -> Result<Vec<(String, String)>> {
    let input = dir.join("input");
    std::fs::create_dir_all(&input)?;
    let mut frames = Vec::new();
    for f in &src.files {
        let fname = f.file_name().and_then(|n| n.to_str()).unwrap_or("input.png").to_string();
        let dst = input.join(&fname);
        if f != &dst {
            std::fs::copy(f, &dst).map_err(|e| PxlError::io(format!("cannot copy {}: {}", f.slash(), e)))?;
        }
        if src.sheet {
            let im = png::read_rgba(&dst)?;
            let cuts = if let Some(g) = plan.grid { cut_grid(&im, g.w, g.h) } else { cut_regions(&im) };
            if cuts.is_empty() {
                return Err(PxlError::validation(format!("{}: no frames found in the sheet", f.slash())).with_hint("check --grid WxH, or use --frames auto on a picture with a transparent or flat background"));
            }
            let fd = input.join("frames");
            std::fs::create_dir_all(&fd)?;
            for (i, c) in cuts.iter().enumerate() {
                let n = format!("{}_{:02}", safe_name(&src.name), i);
                png::write_rgba(&fd.join(format!("{}.png", n)), c)?;
                frames.push((n.clone(), format!("input/frames/{}.png", n)));
            }
        } else {
            let n = src.names.get(frames.len()).cloned().unwrap_or_else(|| safe_name(&stem(f)));
            frames.push((n, format!("input/{}", fname)));
        }
    }
    // the inbox contract: `source.json` beside the pictures (zone, art_zone, unused labels) travels with the asset for source QA
    if let Some(sj) = src.files.first().and_then(|f| f.parent()).map(|p| p.join("source.json")).filter(|p| p.is_file()) {
        let _ = std::fs::copy(&sj, input.join("source.json"));
    }
    Ok(frames)
}

fn round_up(n: usize, m: usize) -> usize {
    (n + m - 1) / m * m
}

/// Build the part (header, frames) of a new asset; the template and the first reduction follow.
fn new_part(dir: &Path, name: &str, frames: &[(String, String)], plan: &Plan) -> Result<Part> {
    let mut maxw = 1;
    let mut maxh = 1;
    for (_, rel) in frames {
        let im = png::read_rgba(&dir.join(rel))?;
        maxw = maxw.max(im.w);
        maxh = maxh.max(im.h);
    }
    let screen = console::info(plan.console.as_deref().unwrap_or(console::DEFAULT_CONSOLE), Some(dir)).map(|c| c.screen).unwrap_or((160, 144));
    let (mut size, factor): ((usize, usize), u32) = match plan.mode {
        PartKind::Screen => {
            let f = plan.factor.unwrap_or_else(|| ((maxw + screen.0 as usize / 2) / screen.0 as usize).max(1) as u32);
            (plan.size.unwrap_or((screen.0 as usize, screen.1 as usize)), f)
        }
        _ => match (plan.size, plan.factor) {
            (Some(s), Some(f)) => (s, f),
            (Some(s), None) => (s, ((maxw + s.0 / 2) / s.0.max(1)).max(1) as u32),
            (None, f) => {
                let f = f.unwrap_or(if plan.mode == PartKind::Tileset { 1 } else { ((maxw.max(maxh) + 63) / 64).max(1) as u32 });
                (((maxw + f as usize - 1) / f as usize, (maxh + f as usize - 1) / f as usize), f)
            }
        },
    };
    if plan.mode == PartKind::Tileset {
        size = (round_up(size.0, 8), round_up(size.1, 8));
    }
    let mut part = Part::new(name, size);
    part.kind = plan.mode;
    part.console = plan.console.clone();
    part.path = dir.join(asset::ASSET_FILE);
    part.origin = (size.0 as i32 / 2, size.1 as i32 / 2);
    if plan.mode == PartKind::Tileset {
        part.tile = Some((8, 8));
        part.origin = (0, 0);
    }
    part.input.factor = Some(factor);
    part.reduce.factor = Some(factor);
    part.reduce.thin = plan.mode == PartKind::Sprite;
    if plan.mode == PartKind::Sprite && factor >= 2 {
        part.reduce.method = crate::model::Method::Feature;
    }
    for (n, rel) in frames {
        part.frames.push(Frame { name: n.clone(), src: Some(rel.clone()), locked: false, grid: Grid::new(size.0, size.1, b'.') });
    }
    if plan.mode == PartKind::Screen {
        // a screen is one picture: reduce it as `background`
    }
    if let Some(c) = &plan.defaults.category {
        part.category = Some(c.clone());
    }
    if let Some(s) = &plan.defaults.palette_set {
        part.palette_set = Some(s.clone());
    }
    if let Some(t) = &plan.defaults.template {
        part.template = Some(t.clone());
    }
    Ok(part)
}

/// Template for a new asset, then the first reduction. Failures are warnings: the asset stays usable. The first version is made to
/// be hardware-valid (see `autoset`); what cannot be is reported as "needs hand work" and becomes a task.
fn derive_and_reduce(path: &Path, part: &mut Part, out: &mut Out) -> Result<()> {
    let prof = crate::profile::load_for(part, None).ok();
    let mut derived_set = false;
    if part.template.is_none() {
        let tpl_path = asset::dir_of(path).join("template.toml");
        let sprite = part.kind == PartKind::Sprite;
        let set_model = sprite && prof.as_ref().map_or(false, |p| p.auto_template == "set");
        let res: Result<()> = if set_model {
            derived_set = true;
            auto_set(path, part, prof.as_ref().unwrap(), &tpl_path, out)
        } else {
            let mut scratch = Out::new(true, None);
            // sprites: one flat layer (a second layer is a second palette line and doubles the cells)
            let a = SuggestArgs { part: path.slash(), derive: true, write: Some(tpl_path.slash()), mode: "optimised".into(), candidates: 4, pick: 1, layers: if sprite { Some(1) } else { None }, ..Default::default() };
            suggest::run_suggest(&a, &mut scratch).map(|_| {
                part.template = Some("template.toml".into());
            })
        };
        match res {
            Ok(()) => {
                versions::suppressed(|| pxl::save(path, part))?;
            }
            Err(e) => {
                out.warn(format!("{}: no template derived ({}); set `template` in asset.pxl, then `pxl load`", asset::name_of(path), e.msg));
                return Ok(());
            }
        }
    }
    let _ = crate::fit::sync_set_keys(part);
    // an automatic palette set already folded the cells (auto_set); everything else is reduced now
    if !part.template.is_some() || !(part.palette_set.is_some() && derived_set) {
        match versions::suppressed(|| load::reduce_parts(&[path.to_path_buf()], &FrameSel::default(), false, out, true, None, false)) {
            Ok(()) => {}
            Err(e) => out.warn(format!("{}: first reduction failed: {}", asset::name_of(path), e.msg)),
        }
    }
    // the automatic palette-set fold is an automatic output too: until someone edits it, `status --raw` and `check --hand-finished` count it as raw
    if derived_set {
        if let Ok(done) = pxl::load(path) {
            let all: Vec<usize> = (0..done.frames.len()).collect();
            crate::status::record(path, &done, crate::status::StatusEvent::RawFrames(all));
        }
    }
    if part.kind == PartKind::Sprite {
        report_hand_work(path, out);
    }
    Ok(())
}

/// The palette-set model: reduce flat with up to `obj_palettes x 3` colours, pack the colours of the cells into an asset-local set, pick
/// the category, and leave the final reduction (slot per cell) to the caller.
fn auto_set(path: &Path, part: &mut Part, prof: &crate::profile::Profile, tpl_path: &Path, out: &mut Out) -> Result<()> {
    use suggest::derive;
    let dir = asset::dir_of(path);
    let slots = (prof.obj_palettes as usize).max(1);
    let fixed = !prof.fixed_colours.is_empty();
    let data = suggest::load_data(part)?;
    // style value_map = stretch (the classic Game Boy): value separates the materials, white is a main tone
    let stretch = prof.style.value_map == "stretch" && prof.fixed_colours.len() == 4;
    let text = if stretch {
        let cols: Vec<(Rgb, u64)> = data.cols.iter().map(|c| (c.rgb, c.n)).collect();
        crate::autoset::stretch_template(&format!("{}_derived", part.name), &cols, prof)
    } else {
        let opts = derive::Opts { k: (slots * 3).min(24), step: if fixed { 1 } else { prof.channel_step.max(1) }, layers: Some(1), mode: derive::Mode::Optimised, name: format!("{}_derived", part.name), cells: false };
        let d = derive::derive_from_data(&data, &opts);
        suggest::snap_template_text(&d.text, prof).0
    };
    suggest::write_new(&tpl_path.slash(), &text)?;
    part.template = Some("template.toml".into());
    versions::suppressed(|| pxl::save(path, part))?;
    let mut scratch = Out::new(true, None);
    versions::suppressed(|| load::reduce_parts(&[path.to_path_buf()], &FrameSel::default(), false, &mut scratch, true, None, false))?;
    // pack the colours of the hardware cells into the slots
    let flat = pxl::load(path)?;
    for (i, f) in flat.frames.iter().enumerate() {
        part.frames[i].grid = f.grid.clone();
    }
    let tpl = crate::template::load_for(&flat)?;
    let data = crate::autoset::cell_data(&flat, &|k| crate::lint::effective_color(&flat, &tpl, k), prof.hw_cell);
    let packed = if stretch { crate::autoset::stretch_slots(prof) } else { crate::autoset::learn_slots(&data, slots, &|c, used| suggest::nearest_free(prof, c, used)) };
    if packed.is_empty() {
        return Err(PxlError::validation("the inputs have no opaque pixels"));
    }
    let cid = part.console.clone().unwrap_or_else(|| console::project_console(&dir).id);
    let name = safe_name(&asset::asset_name(&dir));
    std::fs::create_dir_all(dir.join("palettes"))?;
    fsutil::atomic_write_plain(&dir.join("palettes").join(format!("{}.toml", name)), crate::autoset::set_text(&cid, &packed).as_bytes())?;
    part.palette_set = Some(name);
    versions::suppressed(|| pxl::save(path, part))?;
    let _ = crate::fit::sync_set_keys(part);
    // fold every cell into its slot (identity colours kept), on the grids of the flat reduction
    let mut fitted = pxl::load(path)?;
    let tpl2 = crate::template::load_for(&fitted)?;
    let table: Vec<(u8, Rgb)> = tpl2.keys().filter_map(|k| crate::lint::effective_color(&fitted, &tpl2, k.key).map(|c| (k.key, c))).collect();
    let key_of = |c: Rgb| table.iter().find(|x| x.1 == c).map(|x| x.0);
    let rgb_of = |k: u8| table.iter().find(|x| x.0 == k).map(|x| x.1);
    if stretch {
        crate::autoset::fold_cells_levels(&mut fitted, &data, &packed, &crate::autoset::stretch_shades(prof), prof.hw_cell, &key_of, &rgb_of);
    } else {
        crate::autoset::fold_cells(&mut fitted, &data, &packed, prof.hw_cell, &key_of, &rgb_of);
    }
    for (i, f) in fitted.frames.iter().enumerate() {
        part.frames[i].grid = f.grid.clone();
    }
    versions::suppressed(|| pxl::save(path, part))?;
    // no category unless the project's defaults name one: the hardware limits apply, not the operator's budgets
    if part.category.is_none() {
        part.budget.per_line = Some(prof.hw_cells_per_row_max);
        part.budget.per_frame = Some(prof.hw_cells_max);
        versions::suppressed(|| pxl::save(path, part))?;
    }
    out.line(format!("{}: palette set of {} slot(s) ({} colours){}", asset::name_of(path), packed.len(), packed.len() * 3, if data.eyes > 0 { format!(", {} eye pixel(s) protected", data.eyes) } else { String::new() }));
    Ok(())
}

/// Lint errors of the first version: print "needs hand work" and make a task per rule.
fn report_hand_work(path: &Path, out: &mut Out) {
    let Ok(part) = pxl::load(path) else { return };
    let Ok(prof) = crate::profile::load_for(&part, None) else { return };
    let issues: Vec<_> = crate::lint::lint_part_auto(&part, &prof, &crate::lint::LintOpts::default()).into_iter().filter(|i| i.severity == crate::lint::Severity::Error).collect();
    if issues.is_empty() {
        return;
    }
    let name = asset::asset_name(&asset::dir_of(path));
    let mut rules: Vec<String> = Vec::new();
    for i in &issues {
        if !rules.contains(&i.rule) {
            rules.push(i.rule.clone());
        }
    }
    let home = crate::tasks::home_of(path);
    let mut list = Vec::new();
    for r in rules.iter().take(5) {
        let first = issues.iter().find(|i| &i.rule == r).unwrap();
        let n = issues.iter().filter(|i| &i.rule == r).count();
        let text = format!("first version needs hand work: {} ({}x): {}", r, n, first.message);
        let _ = crate::tasks::add_task(&home, &name, &text, "fix", None, 2, "pxl");
        list.push(format!("{} ({}x): {}", r, n, first.message));
    }
    out.line(format!("{}: needs hand work: {}", name, list.join(" | ")));
    out.warn(format!("{}: the first version has {} lint error(s) (needs hand work); see `pxl tasks`", name, issues.len()));
    out.set("needs_hand_work", json!(list));
}

fn create_asset(dir: &Path, src: &Source, plan: &Plan, out: &mut Out) -> Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let frames = stage_inputs(dir, src, plan)?;
    let name = asset::asset_name(dir);
    let mut part = new_part(dir, &name, &frames, plan)?;
    let path = dir.join(asset::ASSET_FILE);
    versions::suppressed(|| pxl::save(&path, &part))?;
    derive_and_reduce(&path, &mut part, out)?;
    // a version exists even when the reduction was not possible (blank frames)
    versions::store_noted(&path);
    crate::sourceqa::on_ingest(&path, out);
    Ok(path)
}

/// New input for an existing asset: the changed file becomes the new input (the old one moves to `input/history/`),
/// then the frames that were not hand-edited are reduced again.
fn refresh_asset(dir: &Path, src: &Source, plan: &Plan, out: &mut Out) -> Result<(bool, Vec<String>)> {
    let path = dir.join(asset::ASSET_FILE);
    let input = dir.join("input");
    let mut changed = false;
    for f in &src.files {
        let dst = input.join(f.file_name().unwrap());
        let new = std::fs::read(f)?;
        if let Ok(old) = std::fs::read(&dst) {
            if old == new {
                continue;
            }
            let hist = input.join("history");
            std::fs::create_dir_all(&hist)?;
            let n = std::fs::read_dir(&hist).map(|r| r.flatten().filter(|e| e.file_name().to_string_lossy().starts_with(&format!("{}_i", stem(&dst)))).count()).unwrap_or(0) + 1;
            std::fs::rename(&dst, hist.join(format!("{}_i{:03}.{}", stem(&dst), n, dst.extension().and_then(|e| e.to_str()).unwrap_or("png"))))?;
        }
        std::fs::write(&dst, new)?;
        changed = true;
    }
    if !changed {
        return Ok((false, Vec::new()));
    }
    let frames = stage_inputs(dir, src, plan)?;
    let mut part = pxl::load(&path)?;
    // frames that do not exist yet are appended; existing ones keep their art unless they were not hand-edited
    let (w, h) = part.size;
    for (n, rel) in &frames {
        if !part.frames.iter().any(|f| &f.name == n) {
            part.frames.push(Frame { name: n.clone(), src: Some(rel.clone()), locked: false, grid: Grid::new(w, h, b'.') });
        }
    }
    versions::suppressed(|| pxl::save(&path, &part))?;
    let hand = journal::hand_edited(&path).unwrap_or_default();
    let redo: Vec<String> = part.frames.iter().filter(|f| !f.locked && !hand.contains_key(&f.name)).map(|f| f.name.clone()).collect();
    let kept: Vec<String> = part.frames.iter().filter(|f| hand.contains_key(&f.name) || f.locked).map(|f| f.name.clone()).collect();
    lease::guard(&path, out.agent())?;
    if !redo.is_empty() && part.template.is_some() {
        let sel = FrameSel { frames: redo, all: false };
        load::reduce_parts(&[path.clone()], &sel, false, out, false, None, false)?;
    } else {
        let bytes = std::fs::read(&path)?;
        let _ = versions::store(&path, &bytes);
        // compare.png shows the new input even when the art did not change
        if let Ok(p) = pxl::load(&path) {
            let _ = versions::write_compare(dir, &p);
        }
    }
    if let Ok(p) = pxl::load(&path) {
        let _ = versions::write_compare(dir, &p);
    }
    Ok((true, kept))
}

// ------------------------------------------------------------------------------------- pxl image

pub fn run_image(a: &ImageArgs, out: &mut Out) -> Result<()> {
    let file = PathBuf::from(&a.file);
    if !file.is_file() || !is_png(&file) {
        return Err(PxlError::usage(format!("'{}' is not a PNG file", a.file)));
    }
    let mode = parse_mode(&a.mode)?;
    let cid = a.console.clone().unwrap_or_else(|| console::DEFAULT_CONSOLE.to_string());
    if !console::exists(&cid, Some(&asset::dir_of(&file))) {
        return Err(PxlError::usage(format!("unknown console '{}'", cid)).with_hint("pxl console list"));
    }
    let parent = asset::dir_of(&file);
    let dir = parent.join(format!("{}.pxl", stem(&file)));
    let plan = Plan { mode, console: Some(cid.clone()), size: a.size.map(|s| (s.w, s.h)), factor: a.factor, grid: None, auto_frames: false, defaults: Defaults::default() };
    let src = Source { name: stem(&file), files: vec![file.clone()], sheet: false, names: Vec::new() };
    let existed = dir.join(asset::ASSET_FILE).is_file();
    if existed {
        let (changed, _) = refresh_asset(&dir, &src, &plan, out)?;
        out.emit(format!("{}: {}", dir.slash(), if changed { "input changed, new version" } else { "unchanged" }), json!({"asset": dir.slash(), "created": false, "changed": changed}));
        return Ok(());
    }
    let path = create_asset(&dir, &src, &plan, out)?;
    out.emit(
        format!("created {} ({} / {}); versions/, compare.png and `pxl export {}` work from here", dir.slash(), cid, mode.mode_name(), dir.slash()),
        json!({"asset": dir.slash(), "file": path.slash(), "console": cid, "mode": mode.mode_name(), "created": true}),
    );
    let _ = plan.auto_frames;
    Ok(())
}

// ------------------------------------------------------------------------------------- pxl new

pub fn run_new(a: &NewArgs, out: &mut Out) -> Result<()> {
    let dir = PathBuf::from(&a.dir);
    if dir.join("pxl.toml").exists() {
        return Err(PxlError::usage(format!("{} already holds a project", dir.slash())));
    }
    if !console::exists(&a.console, Some(&dir)) {
        return Err(PxlError::usage(format!("unknown console '{}'", a.console)).with_hint("pxl console list"));
    }
    let name = a.name.clone().unwrap_or_else(|| std::fs::canonicalize(&dir).ok().or_else(|| Some(dir.clone())).and_then(|d| d.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_else(|| "project".into()));
    for d in ["inbox", "assets", "palettes", "consoles", "templates", "export", "review"] {
        std::fs::create_dir_all(dir.join(d))?;
    }
    let text = format!(
        "# pxl project. Assets live in assets/<group>/<name>/ (pxl ingest, pxl group); pxl.toml keeps one [parts.NAME] per asset.\n\
         # `legacy = true` skips the shared scene budgets until you define [scene.NAME] tables; remove it then.\n\
         legacy = true\nout_root = \"export\"\n\n\
         [project]\nname = {}\nconsole = {}\nstrict = true   # silent-zero findings (an unreferenced tile set) are errors, not warnings\n{}\n\n\
         # New assets take these (all optional): mode, size = [w, h], factor, category, palette_set, template\n\
         [project.defaults]\nmode = \"sprite\"\n\n\
         # Free-form settings for skills and agents (`pxl project show` prints them)\n\
         [project.settings]\n",
        asset::q(&name),
        asset::q(&a.console),
        if a.no_gates { "" } else { "gates = true    # production starts after: plan accepted, style approved, dry run passed (pxl milestone status); the design-language lint rules run on every lint" }
    );
    fsutil::atomic_write(&dir.join("pxl.toml"), text.as_bytes())?;
    out.emit(
        format!("created project {} ({}) in {}\n  drop images into inbox/, then: pxl ingest, pxl intake --request \"...\", pxl style propose", name, a.console, dir.slash()),
        json!({"project": name, "console": a.console, "dir": dir.slash()}),
    );
    Ok(())
}

// ------------------------------------------------------------------------------------- pxl ingest

fn need_project() -> Result<(PathBuf, PathBuf)> {
    let cwd = std::env::current_dir().map_err(|e| PxlError::io(e.to_string()))?;
    let mp = Manifest::find_file(&cwd).ok_or_else(|| PxlError::usage("this needs a project (pxl.toml)").with_hint("pxl new DIR --console gbc"))?;
    Ok((asset::dir_of(&mp), mp))
}

fn png_files(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir).map(|r| r.flatten().map(|e| e.path()).filter(|p| p.is_file() && is_png(p)).collect()).unwrap_or_default();
    v.sort();
    v
}

fn gather(a: &IngestArgs, project: &Path) -> Result<Vec<Source>> {
    let args: Vec<String> = if a.inputs.is_empty() { vec![project.join("inbox").slash()] } else { a.inputs.clone() };
    let mut out: Vec<Source> = Vec::new();
    let sheet = a.grid.is_some() || a.frames.as_deref() == Some("auto");
    for arg in &args {
        let mut paths: Vec<PathBuf> = if arg.contains('*') || arg.contains('?') { fsutil::glob(arg) } else { vec![PathBuf::from(arg)] };
        paths.sort();
        for p in paths {
            if p.is_dir() {
                let pngs = png_files(&p);
                if a.name_from == "folder" && !pngs.is_empty() {
                    out.push(Source { name: safe_name(&stem(&p)), files: pngs, sheet: false, names: Vec::new() });
                } else {
                    for f in pngs {
                        out.push(Source { name: safe_name(&stem(&f)), files: vec![f], sheet, names: Vec::new() });
                    }
                }
                // sub-folders are folders of frames
                let mut subs: Vec<PathBuf> = std::fs::read_dir(&p).map(|r| r.flatten().map(|e| e.path()).filter(|q| q.is_dir() && !q.file_name().map_or(false, |n| n.to_string_lossy().starts_with('.'))).collect()).unwrap_or_default();
                subs.sort();
                for s in subs {
                    let f = png_files(&s);
                    if !f.is_empty() {
                        out.push(Source { name: safe_name(&stem(&s)), files: f, sheet: false, names: Vec::new() });
                    }
                }
            } else if p.is_file() && is_png(&p) {
                out.push(Source { name: safe_name(&stem(&p)), files: vec![p], sheet, names: Vec::new() });
            } else if !(arg.contains('*') || arg.contains('?')) {
                return Err(PxlError::usage(format!("'{}' is not a PNG file or folder", p.slash())));
            }
        }
    }
    if !a.separate && !sheet {
        out = group_sequences(out);
    }
    Ok(out)
}

/// `walker_01.png`, `walk1.png`, `walk 2.png`, `walk-03.png`: the stem without its trailing number and the number.
pub fn sequence_of(stem: &str) -> Option<(String, u32, usize)> {
    let digits = stem.chars().rev().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 || digits > 6 {
        return None;
    }
    let cut = stem.len() - digits;
    let prefix = stem[..cut].trim_end_matches(|c: char| c.is_whitespace() || c == '_' || c == '-' || c == '.').to_string();
    if !prefix.chars().any(|c| c.is_alphanumeric()) {
        return None;
    }
    Some((prefix, stem[cut..].parse().ok()?, digits))
}

/// Single-picture sources with a common stem and distinct trailing numbers become one asset: frames in numeric order, named
/// `<stem>_NN` from the number. A lone numbered file stays an asset of its own.
fn group_sequences(src: Vec<Source>) -> Vec<Source> {
    let key = |s: &Source| -> Option<(String, u32, usize)> {
        if s.files.len() != 1 || s.sheet || !s.names.is_empty() {
            return None;
        }
        sequence_of(&stem(&s.files[0])).map(|(p, n, d)| (p.to_lowercase(), n, d))
    };
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    for (i, s) in src.iter().enumerate() {
        if let Some((k, _, _)) = key(s) {
            match groups.iter_mut().find(|g| g.0 == k) {
                Some(g) => g.1.push(i),
                None => groups.push((k, vec![i])),
            }
        }
    }
    let mut merged: std::collections::HashMap<usize, Source> = std::collections::HashMap::new();
    let mut drop: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for (k, idx) in &groups {
        if idx.len() < 2 {
            continue;
        }
        let mut items: Vec<(u32, usize, usize)> = idx.iter().map(|&i| { let (_, n, d) = key(&src[i]).unwrap(); (n, d, i) }).collect();
        items.sort();
        if items.windows(2).any(|w| w[0].0 == w[1].0) {
            continue; // two files with the same number: no sequence
        }
        let width = items.iter().map(|x| x.1).max().unwrap_or(2).max(2);
        let prefix = sequence_of(&stem(&src[items[0].2].files[0])).map(|x| x.0).unwrap_or_else(|| k.clone());
        let name = safe_name(&prefix);
        merged.insert(
            items[0].2.min(idx[0]),
            Source { name: name.clone(), files: items.iter().map(|x| src[x.2].files[0].clone()).collect(), sheet: false, names: items.iter().map(|x| format!("{}_{:0w$}", name, x.0, w = width)).collect() },
        );
        for x in &items {
            drop.insert(x.2);
        }
    }
    let mut out = Vec::new();
    let first_of: std::collections::HashMap<usize, usize> = merged.keys().map(|&k| (k, k)).collect();
    let mut merged = merged;
    for (i, s) in src.into_iter().enumerate() {
        if let Some(m) = first_of.get(&i).and_then(|k| merged.remove(k)) {
            out.push(m);
        } else if !drop.contains(&i) {
            out.push(s);
        }
    }
    out
}

pub fn run_ingest(a: &IngestArgs, out: &mut Out) -> Result<()> {
    let (project, mp) = need_project()?;
    let defaults = defaults_of(&mp);
    let mode = parse_mode(a.mode.as_deref().or(defaults.mode.as_deref()).unwrap_or("sprite"))?;
    let cid = console::project_console(&project).id;
    let sources = gather(a, &project)?;
    if sources.is_empty() {
        out.emit("nothing to ingest (inbox/ is empty)", json!({"ingested": [], "checked": 0}));
        return Ok(());
    }
    let existing = asset::scan(&project);
    let mut results = Vec::new();
    let mut ok = 0;
    for src in &sources {
        let plan = Plan {
            mode,
            console: None,
            size: a.size.map(|s| (s.w, s.h)).or(defaults.size),
            factor: a.factor.or(defaults.factor),
            grid: a.grid,
            auto_frames: a.frames.as_deref() == Some("auto"),
            defaults: defaults.clone(),
        };
        let res = ingest_one(&project, &mp, src, &plan, a.group.as_deref(), &existing, out);
        match res {
            Ok((dir, what)) => {
                ok += 1;
                out.line(format!("{} {}", what, dir.slash()));
                results.push(json!({"asset": src.name, "dir": dir.slash(), "result": what}));
                // inbox files move into the asset
                for f in &src.files {
                    if f.starts_with(project.join("inbox")) && !f.starts_with(&dir) {
                        let _ = std::fs::remove_file(f);
                    }
                }
            }
            Err(e) => {
                out.line(format!("{}: error: {}", src.name, e.msg));
                results.push(json!({"asset": src.name, "error": e.msg}));
            }
        }
    }
    let _ = cid;
    let grouped: Vec<&Source> = sources.iter().filter(|s| !s.names.is_empty()).collect();
    for g in &grouped {
        out.line(format!("grouped {} files into one asset {} ({} frames: {})", g.files.len(), g.name, g.names.len(), g.names.join(" ")));
    }
    out.set("grouped", json!(grouped.iter().map(|g| json!({"asset": g.name, "files": g.files.iter().map(|f| f.slash()).collect::<Vec<_>>(), "frames": g.names})).collect::<Vec<_>>()));
    out.line(format!("ingested {} of {} source(s)", ok, sources.len()));
    out.set("ingested", json!(results));
    out.set("checked", json!({"ok": ok, "of": sources.len()}));
    if ok < sources.len() {
        return Err(PxlError::validation(format!("{} of {} source(s) failed", sources.len() - ok, sources.len())));
    }
    Ok(())
}

fn ingest_one(project: &Path, mp: &Path, src: &Source, plan: &Plan, group: Option<&str>, existing: &[AssetInfo], out: &mut Out) -> Result<(PathBuf, &'static str)> {
    if let Some(e) = existing.iter().find(|e| e.name == src.name) {
        let (changed, kept) = refresh_asset(&e.dir, src, plan, out)?;
        if !kept.is_empty() && changed {
            out.warn(format!("{}: {} frame(s) were hand-edited or locked and keep their art: {}", src.name, kept.len(), kept.join(" ")));
        }
        return Ok((e.dir.clone(), if changed { "updated" } else { "unchanged" }));
    }
    let g = group.unwrap_or("").trim_matches('/');
    let dir = if g.is_empty() { project.join("assets").join(&src.name) } else { project.join("assets").join(g).join(&src.name) };
    if dir.exists() {
        return Err(PxlError::usage(format!("{} exists but holds no asset", dir.slash())));
    }
    let mut plan2 = Plan { mode: plan.mode, console: None, size: plan.size, factor: plan.factor, grid: plan.grid, auto_frames: plan.auto_frames, defaults: plan.defaults.clone() };
    plan2.console = None;
    // `--group player` implies category player (two-slot cells, the player budgets); `[project.defaults] group_categories` maps other groups
    let palette_set_console = crate::profile::for_project(project, Some(&console::project_console(project).id), "sprite").map_or(false, |p| p.auto_template == "set");
    if plan2.mode == PartKind::Sprite && palette_set_console {
        if let Some(c) = plan.defaults.category_for(g) {
            if plan2.defaults.category.as_deref() != Some(c.as_str()) {
                out.line(format!("{}: category {} (group '{}'; change it with `pxl meta {} set category X`)", src.name, c, g, src.name));
            }
            plan2.defaults.category = Some(c);
        }
    }
    let path = create_asset(&dir, src, &plan2, out)?;
    // register in pxl.toml
    let rel = path.strip_prefix(project).unwrap_or(&path).to_string_lossy().replace('\\', "/");
    let outp = if g.is_empty() { src.name.clone() } else { format!("{}/{}", g, src.name) };
    let tags: Vec<String> = if g.is_empty() { Vec::new() } else { vec![g.split('/').next().unwrap().to_string()] };
    let text = fsutil::read_string(mp)?;
    let new = asset::add_part_entry(&text, &src.name, &rel, &outp, &tags);
    if new != text {
        fsutil::atomic_write(mp, new.as_bytes())?;
    }
    if let Some(l) = versions::export_asset(&path) {
        out.line(l);
    }
    Ok((dir, "created"))
}

// ------------------------------------------------------------------------------------- groups, tree

pub fn run_group(c: &GroupCmd, out: &mut Out) -> Result<()> {
    let (project, mp) = need_project()?;
    match c {
        GroupCmd::List => {
            let assets = asset::scan(&project);
            let mut text = String::new();
            let mut js = Vec::new();
            let mut names = asset::groups(&project);
            if assets.iter().any(|a| a.group.is_empty()) {
                names.insert(0, String::new());
            }
            for g in names {
                let members: Vec<&AssetInfo> = assets.iter().filter(|a| a.group == g).collect();
                text.push_str(&format!("{:<24} {} asset(s){}\n", if g.is_empty() { "(top level)" } else { &g }, members.len(), if members.is_empty() { String::new() } else { format!(": {}", members.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(" ")) }));
                js.push(json!({"group": g, "assets": members.iter().map(|a| a.name.clone()).collect::<Vec<_>>()}));
            }
            out.emit(text.trim_end(), json!({"groups": js}));
        }
        GroupCmd::Add { name } => {
            let g = name.trim_matches('/');
            if g.is_empty() || g.contains("..") {
                return Err(PxlError::usage("give a group name such as enemies or zone_1/enemies"));
            }
            std::fs::create_dir_all(project.join("assets").join(g))?;
            out.emit(format!("group {} ready", g), json!({"group": g}));
        }
        GroupCmd::Move { asset: a, group } => {
            let _ml = manifest_lease(&mp, out)?;
            let found = asset::find(a).ok_or_else(|| PxlError::usage(format!("asset '{}' not found", a)))?;
            let adir = asset::dir_of(&found);
            let name = asset::asset_name(&adir);
            let g = group.trim_matches('/');
            let target = if g.is_empty() { project.join("assets").join(&name) } else { project.join("assets").join(g).join(&name) };
            if target == std::fs::canonicalize(&adir).unwrap_or(adir.clone()) || target == adir {
                out.emit(format!("{} is already in {}", name, g), json!({"asset": name, "group": g, "moved": false}));
                return Ok(());
            }
            if target.exists() {
                return Err(PxlError::usage(format!("{} already exists", target.slash())));
            }
            std::fs::create_dir_all(target.parent().unwrap())?;
            fsutil::release_locks_in(&adir);
            std::fs::rename(&adir, &target)?;
            let rel = target.join(asset::ASSET_FILE).strip_prefix(&project).unwrap().to_string_lossy().replace('\\', "/");
            let outp = if g.is_empty() { name.clone() } else { format!("{}/{}", g, name) };
            let mut text = fsutil::read_string(&mp)?;
            if asset::has_part_entry(&text, &name) {
                text = asset::set_part_key(&text, &name, "path", &asset::q(&rel));
                text = asset::set_part_key(&text, &name, "out", &asset::q(&outp));
                if !g.is_empty() {
                    text = asset::set_part_key(&text, &name, "tags", &format!("[{}]", asset::q(g.split('/').next().unwrap())));
                }
            } else {
                text = asset::add_part_entry(&text, &name, &rel, &outp, &[]);
            }
            fsutil::atomic_write(&mp, text.as_bytes())?;
            // the old outputs are stale: remove them with the next `pxl build`; rebuild the export at its new place
            let _ = versions::export_asset(&target.join(asset::ASSET_FILE));
            out.emit(format!("moved {} to {}", name, if g.is_empty() { "(top level)" } else { g }), json!({"asset": name, "group": g, "moved": true, "path": rel}));
        }
    }
    Ok(())
}

/// A lease on pxl.toml for manifest commands: refused while another agent holds it.
pub fn manifest_lease(mp: &Path, out: &Out) -> Result<()> {
    lease::guard(mp, out.agent())
}

pub fn state_of(file: &Path) -> String {
    status::read(file).state
}

pub fn run_tree(out: &mut Out) -> Result<()> {
    let (project, mp) = need_project()?;
    let assets = asset::scan(&project);
    let tasks = crate::tasks::load_all(&project);
    let mut text = format!("{} ({})\n", project_table(&mp).get("name").and_then(|n| n.as_str()).unwrap_or("project"), console::project_console(&project).id);
    let mut js = Vec::new();
    let mut last_group = String::from("\u{0}");
    for a in &assets {
        if a.group != last_group {
            text.push_str(&format!("{}/\n", if a.group.is_empty() { "." } else { &a.group }));
            last_group = a.group.clone();
        }
        let st = status::read(&a.file);
        let v = versions::latest(&a.file).map_or("-".to_string(), |v| format!("v{:03}", v.n));
        let open = tasks.iter().filter(|t| t.asset == a.name && t.status == "open").count();
        text.push_str(&format!("  {:<28} {:<11} {:<5}{}{}\n", a.name, st.state, v, if open > 0 { format!(" {} open task(s)", open) } else { String::new() }, st.approved_by.as_ref().map_or(String::new(), |b| format!(" approved by {}", b))));
        js.push(json!({"asset": a.name, "group": a.group, "state": st.state, "version": v, "open_tasks": open, "approved_by": st.approved_by}));
    }
    text.push_str(&format!("{} asset(s)", assets.len()));
    out.emit(text, json!({"assets": js, "checked": {"ok": assets.len(), "of": assets.len()}}));
    Ok(())
}

pub fn run_project(c: &ProjectCmd, out: &mut Out) -> Result<()> {
    if let ProjectCmd::Set { path, value } = c {
        return crate::manedit::run_project_set(path, value, out);
    }
    let (project, mp) = need_project()?;
    let t = project_table(&mp);
    let d = defaults_of(&mp);
    let cid = console::project_console(&project).id;
    let settings = t.get("settings").and_then(|s| s.as_table()).cloned().unwrap_or_default();
    let merged_mode = d.mode.clone().unwrap_or_else(|| "sprite".into());
    let mut text = format!(
        "project {} in {}\nconsole {}\ndefaults: mode {}, size {}, factor {}, category {}, palette_set {}, template {}, group categories {}\nsettings:",
        t.get("name").and_then(|n| n.as_str()).unwrap_or("-"),
        project.slash(),
        cid,
        merged_mode,
        d.size.map_or("auto".to_string(), |s| format!("{}x{}", s.0, s.1)),
        d.factor.map_or("auto".to_string(), |f| f.to_string()),
        d.category.as_deref().unwrap_or("-"),
        d.palette_set.as_deref().unwrap_or("-"),
        d.template.as_deref().unwrap_or("(derived per asset)"),
        d.group_categories.iter().map(|(g, c)| format!("{}={}", g, c)).collect::<Vec<_>>().join(", ")
    );
    if settings.is_empty() {
        text.push_str(" (none)");
    }
    for (k, v) in &settings {
        text.push_str(&format!("\n  {} = {}", k, v));
    }
    let settings_json: serde_json::Value = serde_json::to_value(&settings).unwrap_or_default();
    out.emit(
        text,
        json!({"name": t.get("name").and_then(|n| n.as_str()), "console": cid, "dir": project.slash(),
               "defaults": {"mode": merged_mode, "size": d.size, "factor": d.factor, "category": d.category, "palette_set": d.palette_set, "template": d.template, "group_categories": d.group_categories},
               "settings": settings_json}),
    );
    Ok(())
}
