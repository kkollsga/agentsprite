//! M4 T3: tile set and screen input/output (export, import, tile-level show/view/edit helpers, tile compare).
//!
//! # Export of `tileset` and `screen` parts
//!
//! * One indexed PNG per frame, **no tRNS** (tiles and screens are opaque). The PLTE is the template palette keys in
//!   order (`[palettes]` in file order, a key once; keys of the grid that no palette lists follow; without
//!   `[palettes]` every template key in template order), colours snapped to the profile's `channel_step`. A `.`
//!   cell is an error.
//! * File names: a frame `blocks` (any frame) is `<out>/<frame>.png`; a tileset frame named `<name>_<nn>` is an
//!   animated tile frame and goes to `<out>/animated/<frame>.png`. Screens (`splash`, `background`, `background_N`)
//!   and fonts/walls are flat. Each `[[cycle]]` writes `palette_cycle/<nn>.png` (4x1 swatches, `nn` = 01.. in
//!   order, one file per step; step k shows the cycle colours rotated by k: pixel i = colour `(i + k) % count`).
//! * The `.aseprite` holds a visible plain image layer `image` and a hidden tilemap layer `tiles` (unique tiles
//!   up to mirroring, flips set, tile 0 empty) of the same picture, one frame per part frame, tags by frame name,
//!   an 8x8 grid.
//!
//! # Tile editing helpers
//!
//! `show/view/set/row/patch --tile N` use tile-relative coordinates 0..7. [`status_of`] is the one-line
//! "tile N: palette P ok, K placements, unique" echo.
//!
//! # Compare
//!
//! [`compare_tileset`]: pages of `reference block | tile x2 | diff` per tile, ranked worst error x most used first.

use crate::fsutil::PathSlash;
use crate::cli::CompareArgs;
use crate::color::{self, Oklab, Rgb};
use crate::compare::{self, C_BAD, C_GOOD, C_WARN, DIM, GUT_L, GUT_T, HEADER_H, MARGIN, MAX_PX, PLATE, TEXT};
use crate::export::{self, ExportResult, Job};
use crate::fsutil;
use crate::img::{Rgba, RgbaImage};
use crate::lint;
use crate::load;
use crate::manifest::OutSpec;
use crate::model::{Frame, Grid, Part, PartKind};
use crate::output::{Loc, Out, PxlError, Result};
use crate::png;
use crate::profile::Profile;
use crate::status::{self, StatusEvent};
use crate::template::Template;
use crate::tiles::{self, stats, Colors, Sheet, T};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

pub const SUB_ANIMATED: &str = "animated";
pub const SUB_CYCLE: &str = "palette_cycle";

// ------------------------------------------------------------------------------------------------ file names

/// `<name>_<nn>`: a numbered frame of an animated tile set.
pub fn is_anim_frame(name: &str) -> bool {
    match name.rfind('_') {
        Some(i) if i > 0 => {
            let d = &name[i + 1..];
            !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit())
        }
        _ => false,
    }
}

/// Path of a frame's PNG.
pub fn frame_path(part: &Part, spec: &OutSpec, frame: &str) -> PathBuf {
    let name = spec.file_name(frame, 0, "base");
    if part.kind == PartKind::Tileset && is_anim_frame(frame) {
        spec.dir.join(SUB_ANIMATED).join(name)
    } else {
        spec.dir.join(name)
    }
}

/// Every declared output of a tile set or screen part: frame PNGs, palette-cycle swatches, the `.aseprite` last.
pub fn expected_files(part: &Part, spec: &OutSpec) -> Vec<(String, usize, PathBuf)> {
    let mut v: Vec<(String, usize, PathBuf)> = part.frames.iter().map(|f| (f.name.clone(), 0, frame_path(part, spec, &f.name))).collect();
    for (ci, c) in part.cycles.iter().enumerate() {
        v.push((format!("cycle_json_{:02}", ci + 1), 0, spec.dir.join(SUB_CYCLE).join(format!("{}.json", cycle_name(c, ci)))));
    }
    if part.priority.is_some() {
        v.push((String::new(), 0, spec.dir.join("priority.png")));
    }
    if !part.underwater.is_empty() {
        v.push((String::new(), 0, spec.dir.join("palettes_underwater.json")));
    }
    v.push((String::new(), 0, spec.ase.clone()));
    v
}

fn cycle_name(c: &crate::model::CycleCfg, i: usize) -> String {
    c.name.clone().or_else(|| c.palette.clone()).unwrap_or_else(|| format!("cycle_{:02}", i + 1))
}

// ------------------------------------------------------------------------------------------------ plan

pub struct TilePlan {
    /// PLTE colours (snapped).
    pub pal: Vec<Rgb>,
    /// Palette index per key.
    pub index: Vec<Option<u8>>,
    pub warnings: Vec<String>,
}

pub fn tile_plan(part: &Part, tpl: &Template, prof: &Profile) -> Result<TilePlan> {
    let mut order: Vec<u8> = Vec::new();
    for p in &tpl.palettes {
        for &k in &p.keys {
            if !order.contains(&k) {
                order.push(k);
            }
        }
    }
    let mut used = [false; 256];
    for f in &part.frames {
        for &c in &f.grid.cells {
            used[c as usize] = true;
        }
    }
    for kd in tpl.keys() {
        if (tpl.palettes.is_empty() || used[kd.key as usize]) && !order.contains(&kd.key) {
            order.push(kd.key);
        }
    }
    if prof.pattern_unique && !order.is_empty() {
        // one global colour order that makes equal shapes in different palettes equal index patterns (tiles::pattern)
        let cols = Colors::new(part, tpl);
        let air = tiles::air_codes(tpl, &cols);
        let fi = part.frame_index("blocks").unwrap_or(0);
        if let Some(f) = part.frames.get(fi) {
            if f.grid.w % T == 0 && f.grid.h % T == 0 && f.grid.w > 0 && f.grid.h > 0 {
                let model = tiles::pattern::build(&Sheet::from_grid(&f.grid), &cols, &air, true);
                let pos: HashMap<u32, usize> = model.order.iter().enumerate().map(|(i, c)| (*c, i)).collect();
                order.sort_by_key(|k| pos.get(&cols.code[*k as usize]).copied().unwrap_or(usize::MAX));
            }
        }
    }
    if order.is_empty() {
        return Err(PxlError::validation(format!("template '{}' has no coloured keys", tpl.name)).with_loc(Loc::file(tpl.path.slash())));
    }
    if order.len() > 256 {
        return Err(PxlError::validation("more than 256 colours: an indexed PNG holds at most 256"));
    }
    let mut p = TilePlan { pal: Vec::new(), index: vec![None; 256], warnings: Vec::new() };
    let mut snapped: BTreeSet<String> = BTreeSet::new();
    for (i, &k) in order.iter().enumerate() {
        let raw = lint::effective_color(part, tpl, k).ok_or_else(|| PxlError::validation(format!("template key '{}' has no colour", k as char)).with_loc(Loc::file(tpl.path.slash())))?;
        let c = prof.snap(raw);
        if c != raw {
            snapped.insert(format!("{} {} -> {}", k as char, color::to_hex(raw), color::to_hex(c)));
        }
        p.pal.push(c);
        p.index[k as usize] = Some(i as u8);
    }
    for i in 0..p.pal.len() {
        for j in 0..i {
            if p.pal[i] == p.pal[j] {
                p.warnings.push(format!("keys '{}' and '{}' have the same colour {} in the PNG palette", order[j] as char, order[i] as char, color::to_hex(p.pal[i])));
            }
        }
    }
    if !snapped.is_empty() {
        p.warnings.push(format!("colours snapped to channel_step {}: {}", prof.channel_step, snapped.into_iter().collect::<Vec<_>>().join(", ")));
    }
    Ok(p)
}

/// Palette indices of one frame; `.` and undefined keys are errors.
fn frame_indices(plan: &TilePlan, part: &Part, fi: usize) -> Result<Vec<u8>> {
    let f = &part.frames[fi];
    let mut v = Vec::with_capacity(f.grid.cells.len());
    for (i, &k) in f.grid.cells.iter().enumerate() {
        match plan.index[k as usize] {
            Some(ix) => v.push(ix),
            None => {
                let (x, y) = (i % f.grid.w, i / f.grid.w);
                let why = if k == b'.' { "is transparent: level tiles and screens have no transparency".to_string() } else { format!("uses key '{}' that the template does not define", k as char) };
                return Err(PxlError::validation(format!("pixel ({},{}) of frame {} {}", x, y, f.name, why))
                    .with_loc(Loc::file(part.path.slash()).frame(f.name.clone()).at(y, x))
                    .with_hint("fill every cell with a template key"));
            }
        }
    }
    Ok(v)
}

// ------------------------------------------------------------------------------------------------ palette cycle

fn hex_list(cols: &[Rgb]) -> Vec<String> {
    cols.iter().map(|c| color::to_hex(*c)).collect()
}

/// (path below the output folder, bytes) of the palette-cycle files: `palette_cycle/<name>.json` per cycle
/// (`bg_palette`, `frames_per_step`, `steps`, DESIGN 5.6a), plus warnings.
fn cycle_files(part: &Part, _tpl: &Template, prof: &Profile) -> Result<(Vec<(String, Vec<u8>)>, Vec<String>)> {
    let mut files = Vec::new();
    let warnings = Vec::new();
    if part.cycles.is_empty() {
        return Ok((files, warnings));
    }
    for (ci, c) in part.cycles.iter().enumerate() {
        let loc = Loc::file(part.path.slash());
        let name = cycle_name(c, ci);
        let (palette_of, steps): (Vec<(u8, u8)>, Vec<Vec<Rgb>>);
        {
            // targets (indexes 0..3 of the BG palette) and the colours of every step
            if c.targets.is_empty() || c.steps.is_empty() || c.bg_palette.is_none() {
                return Err(PxlError::validation(format!("[[cycle]] {}: it needs bg_palette, targets and steps", name)).with_loc(loc));
            }
            if c.targets.len() > 4 || c.targets.iter().any(|t| *t > 3) {
                return Err(PxlError::validation(format!("[[cycle]] {}: targets are 1..4 colour indexes 0..3 of one BG palette", name)).with_loc(loc));
            }
            let mut st = Vec::new();
            for (j, s) in c.steps.iter().enumerate() {
                if s.len() != c.targets.len() {
                    return Err(PxlError::validation(format!("[[cycle]] {}: step {} has {} colours for {} targets", name, j, s.len(), c.targets.len())).with_loc(loc));
                }
                st.push(s.iter().map(|h| color::parse_hex(h).map(|x| prof.snap(x))).collect::<Result<Vec<Rgb>>>()?);
            }
            palette_of = c.targets.iter().map(|t| (c.bg_palette.unwrap(), *t)).collect();
            steps = st;
        }
        let bg = palette_of[0].0;
        let mut step_json = Vec::new();
        for s in &steps {
            let mut m = serde_json::Map::new();
            for (i, col) in s.iter().enumerate() {
                m.insert(palette_of[i].1.to_string(), json!(color::to_hex(*col)));
            }
            step_json.push(Value::Object(m));
        }
        let doc = json!({"bg_palette": bg, "frames_per_step": c.frames_per_step.unwrap_or(8), "steps": step_json});
        files.push((format!("{}.json", name), (serde_json::to_string_pretty(&doc).unwrap() + "\n").into_bytes()));
    }
    Ok((files, warnings))
}

/// `palettes_underwater.json`: the colours of every BG / OBJ palette below the water line, in swap order.
fn underwater_json(part: &Part, prof: &Profile) -> Result<Option<Vec<u8>>> {
    if part.underwater.is_empty() {
        return Ok(None);
    }
    let loc = Loc::file(part.path.slash());
    let mut items: Vec<(u32, usize, Value)> = Vec::new();
    for (i, u) in part.underwater.iter().enumerate() {
        let cols = u.colors.iter().map(|h| color::parse_hex(h).map(|x| prof.snap(x))).collect::<Result<Vec<Rgb>>>()?;
        if cols.is_empty() || cols.len() > 4 {
            return Err(PxlError::validation(format!("[[underwater]] {} palette {}: needs 1..4 colours (colors)", u.kind, u.palette)).with_loc(loc));
        }
        let ord = u.order.unwrap_or(i as u32 + 1);
        items.push((ord, i, json!({"kind": u.kind, "palette": u.palette, "order": ord, "colors": hex_list(&cols)})));
    }
    items.sort_by_key(|x| (x.0, x.1));
    let doc = json!({"palettes": items.into_iter().map(|x| x.2).collect::<Vec<_>>()});
    Ok(Some((serde_json::to_string_pretty(&doc).unwrap() + "\n").into_bytes()))
}

/// Extra text for the export configuration hash: the cycle sources.
pub fn config_extra(part: &Part) -> String {
    if part.kind == PartKind::Sprite {
        return String::new();
    }
    let mut s = String::from(";tiles1");
    for c in &part.cycles {
        s.push_str(&format!(";cycle={}", c.palette.clone().unwrap_or_default()));
        s.push_str(&format!(";cyc2={:?},{:?},{:?},{:?},{:?}", c.name, c.bg_palette, c.frames_per_step, c.targets, c.steps));
    }
    for u in &part.underwater {
        s.push_str(&format!(";uw={:?}", u));
    }
    s
}

// ------------------------------------------------------------------------------------------------ .aseprite

fn canon_tile(t: &[u8; 64]) -> ([u8; 64], bool, bool) {
    let flip = |fx: bool, fy: bool| -> [u8; 64] {
        let mut o = [0u8; 64];
        for y in 0..T {
            for x in 0..T {
                o[y * T + x] = t[(if fy { T - 1 - y } else { y }) * T + if fx { T - 1 - x } else { x }];
            }
        }
        o
    };
    let mut best = (*t, false, false);
    for (fx, fy) in [(true, false), (false, true), (true, true)] {
        let f = flip(fx, fy);
        if f < best.0 {
            best = (f, fx, fy);
        }
    }
    best
}

fn build_ase_tiles(part: &Part, plan: &TilePlan, frames: &[Vec<u8>]) -> Result<Vec<u8>> {
    use crate::ase::{self, AseBuilder, LoopDir};
    let (w, h) = (part.size.0, part.size.1);
    if w == 0 || h == 0 || w > 65535 || h > 65535 {
        return Err(PxlError::validation(format!("canvas {}x{} cannot be stored in an .aseprite", w, h)));
    }
    for f in &part.frames {
        if (f.grid.w, f.grid.h) != (w, h) {
            return Err(PxlError::validation(format!("frame {} is {}x{}, the part canvas is {}x{}", f.name, f.grid.w, f.grid.h, w, h)).with_loc(Loc::file(part.path.slash()).frame(f.name.clone())));
        }
    }
    let mut pal = vec![Rgb::new(0, 0, 0)];
    pal.extend(plan.pal.iter().cloned());
    let mut b = AseBuilder::new_indexed(w as u16, h as u16, &pal);
    b.set_grid(0, 0, T as u16, T as u16);
    let tiled = w % T == 0 && h % T == 0;
    let img_layer = b.add_layer("image");
    let mut map_layer = None;
    // unique tiles over all frames (canonical orientation), ids from 1
    let mut ids: HashMap<[u8; 64], u32> = HashMap::new();
    let mut uniq: Vec<[u8; 64]> = Vec::new();
    let mut maps: Vec<Vec<u32>> = Vec::new();
    if tiled {
        let (tw, th) = (w / T, h / T);
        for fr in frames {
            let mut cells = Vec::with_capacity(tw * th);
            for ty in 0..th {
                for tx in 0..tw {
                    let mut t = [0u8; 64];
                    for y in 0..T {
                        for x in 0..T {
                            t[y * T + x] = fr[(ty * T + y) * w + tx * T + x] + 1;
                        }
                    }
                    let (c, fx, fy) = canon_tile(&t);
                    let id = *ids.entry(c).or_insert_with(|| {
                        uniq.push(c);
                        uniq.len() as u32
                    });
                    cells.push(id | if fx { 0x8000_0000 } else { 0 } | if fy { 0x4000_0000 } else { 0 });
                }
            }
            maps.push(cells);
        }
        let mut data = vec![0u8; T * T];
        for t in &uniq {
            data.extend_from_slice(t);
        }
        b.add_tileset(0, "tiles", T as u16, T as u16, uniq.len() as u32 + 1, data);
        let mut l = ase::Layer::normal("tiles");
        l.layer_type = ase::LayerType::Tilemap;
        l.tileset_index = 0;
        l.flags = ase::layer_flags::EDITABLE;
        map_layer = Some(b.add_layer_with(l));
    }
    for (fi, f) in part.frames.iter().enumerate() {
        let af = b.add_frame(100);
        let canvas: Vec<u8> = frames[fi].iter().map(|&i| i + 1).collect();
        b.set_cel_canvas(af, img_layer, &canvas);
        if let Some(ml) = map_layer {
            b.set_tilemap_cel(af, ml, 0, 0, (w / T) as u16, (h / T) as u16, maps[fi].clone());
        }
        b.add_tag(&f.name, fi, fi, LoopDir::Forward, 0);
    }
    let bytes = b.build()?;
    let problems = ase::validate(&bytes);
    if !problems.is_empty() {
        let first: Vec<String> = problems.iter().take(3).map(|p| format!("{} (offset {})", p.msg, p.offset)).collect();
        return Err(PxlError::validation(format!("generated .aseprite fails validation ({} problems): {}", problems.len(), first.join("; "))));
    }
    Ok(bytes)
}

// ------------------------------------------------------------------------------------------------ export

pub fn export_tiles(job: &Job, part: &Part, tpl: &Template, prof: &Profile) -> Result<ExportResult> {
    let spec = &job.spec;
    let plan = tile_plan(part, tpl, prof)?;
    let files = expected_files(part, spec);
    let mut seen = BTreeSet::new();
    for (_, _, p) in &files {
        if !seen.insert(p.clone()) {
            return Err(PxlError::validation(format!("two outputs share the file {}", p.slash())));
        }
    }
    let mut res = ExportResult { name: part.name.clone(), part: job.part_path.slash(), frames: part.frames.len(), warnings: plan.warnings.clone(), ..Default::default() };
    let write = |path: &Path, bytes: &[u8], res: &mut ExportResult| -> Result<()> {
        if export::write_if_changed(path, bytes)? {
            res.written += 1;
        } else {
            res.unchanged += 1;
        }
        res.files += 1;
        res.outputs.insert(spec.key(path), fsutil::content_hash(bytes));
        Ok(())
    };
    let partial = !job.frames.is_empty();
    let mut all_idx: Vec<Vec<u8>> = Vec::new();
    for (fi, f) in part.frames.iter().enumerate() {
        let idx = frame_indices(&plan, part, fi)?;
        if !partial || job.frames.contains(&f.name) {
            let bytes = png::encode_indexed(f.grid.w, f.grid.h, &idx, &plan.pal, false)?;
            write(&frame_path(part, spec, &f.name), &bytes, &mut res)?;
        }
        all_idx.push(idx);
    }
    let (sw, cw) = cycle_files(part, tpl, prof)?;
    res.warnings.extend(cw);
    for (name, bytes) in &sw {
        write(&spec.dir.join(SUB_CYCLE).join(name), bytes, &mut res)?;
    }
    if let Some(ids) = &part.priority {
        let bi = part.frame_index("blocks").unwrap_or(0);
        if let Some(f) = part.frames.get(bi) {
            let tw = (f.grid.w / T).max(1);
            let mut m = vec![0u8; f.grid.w * f.grid.h];
            for &t in ids {
                let (tx, ty) = ((t % tw) * T, (t / tw) * T);
                for y in ty..(ty + T).min(f.grid.h) {
                    for x in tx..(tx + T).min(f.grid.w) {
                        m[y * f.grid.w + x] = 1;
                    }
                }
            }
            let bytes = png::encode_indexed(f.grid.w, f.grid.h, &m, &[Rgb::new(0, 0, 0), Rgb::new(255, 255, 255)], false)?;
            write(&spec.dir.join("priority.png"), &bytes, &mut res)?;
        }
    }
    if let Some(bytes) = underwater_json(part, prof)? {
        write(&spec.dir.join("palettes_underwater.json"), &bytes, &mut res)?;
    }
    if part.kind == PartKind::Screen {
        if let Some(p) = prof.size {
            for f in &part.frames {
                if (f.grid.w, f.grid.h) != (p.0 as usize, p.1 as usize) {
                    res.warnings.push(format!("screen frame {} is {}x{}, the checker wants {}x{}", f.name, f.grid.w, f.grid.h, p.0, p.1));
                }
            }
        }
    }
    let ase_bytes = build_ase_tiles(part, &plan, &all_idx)?;
    write(&spec.ase, &ase_bytes, &mut res)?;
    if !partial {
        export::push_stale(job, &mut res);
        if !job.scratch {
            status::record(&job.part_path, part, StatusEvent::Exported);
            status::record(&job.part_path, part, StatusEvent::ExportConfig(export::config_hash(part, prof, spec)));
            status::record(&job.part_path, part, StatusEvent::Outputs(res.outputs.clone()));
        }
    }
    Ok(res)
}

// ------------------------------------------------------------------------------------------------ import

fn nearest_key(part: &Part, tpl: &Template, c: Rgb) -> Option<(u8, Rgb)> {
    let o = color::srgb_to_oklab(c);
    tpl.keys()
        .filter_map(|k| lint::effective_color(part, tpl, k.key).map(|kc| (k.key, kc)))
        .min_by(|a, b| color::oklab_dist2(o, color::srgb_to_oklab(a.1)).partial_cmp(&color::oklab_dist2(o, color::srgb_to_oklab(b.1))).unwrap_or(std::cmp::Ordering::Equal))
}

/// A tile set or screen PNG (any colour type; RGB or indexed) as a grid of template keys. Every pixel must have
/// exactly a template colour (or that colour snapped to the profile's step), else the error names the tile and
/// the pixel and the nearest key.
pub fn import_tile_png(path: &Path, name: &str, part: &Part, tpl: &Template) -> Result<Grid> {
    let src = path.slash();
    let img = png::read_rgba(path)?;
    if (img.w, img.h) != part.size {
        return Err(PxlError::validation(format!("{} is {}x{} px, the part canvas is {}x{}", src, img.w, img.h, part.size.0, part.size.1))
            .with_hint("import expects the exact canvas size; a sheet is 128 px wide, a screen 160x144"));
    }
    let prof_snap = crate::profile::load_for(part, None).ok();
    // colour -> key; palette keys first so a shared colour resolves to the palette key
    let mut map: HashMap<(u8, u8, u8), u8> = HashMap::new();
    let mut order: Vec<u8> = tpl.palettes.iter().flat_map(|p| p.keys.iter().cloned()).collect();
    order.extend(tpl.keys().map(|k| k.key));
    for k in order {
        if let Some(c) = lint::effective_color(part, tpl, k) {
            map.entry((c.r, c.g, c.b)).or_insert(k);
            let s = prof_snap.as_ref().map_or(c, |p| p.snap(c));
            map.entry((s.r, s.g, s.b)).or_insert(k);
        }
    }
    let per_row = (img.w / T).max(1);
    let mut grid = Grid::new(img.w, img.h, b'.');
    for y in 0..img.h {
        for x in 0..img.w {
            let p = img.px[y * img.w + x];
            let tile = (y / T) * per_row + x / T;
            if p.a != 255 {
                return Err(PxlError::validation(format!("tile #{} pixel ({},{}) is {} (alpha {}): level tiles and screens have no transparency", tile, x, y, if p.a == 0 { "transparent" } else { "semi-transparent" }, p.a))
                    .with_loc(Loc::file(src).frame(name.to_string()).at(y, x)));
            }
            match map.get(&(p.r, p.g, p.b)) {
                Some(&k) => grid.cells[y * img.w + x] = k,
                None => {
                    let c = Rgb { r: p.r, g: p.g, b: p.b };
                    let hint = match nearest_key(part, tpl, c) {
                        Some((k, kc)) => format!("nearest template key is '{}' ({}); only exact template colours import, recolour the PNG or extend the template", k as char, color::to_hex(kc)),
                        None => "the template has no coloured keys".to_string(),
                    };
                    return Err(PxlError::validation(format!(
                        "tile #{} (tile x {}, y {}) pixel ({},{}) has colour {} that matches no template key",
                        tile,
                        (x / T) * T,
                        (y / T) * T,
                        x,
                        y,
                        color::to_hex(c)
                    ))
                    .with_loc(Loc::file(src).frame(name.to_string()).at(y, x))
                    .with_hint(hint));
                }
            }
        }
    }
    Ok(grid)
}

// ------------------------------------------------------------------------------------------------ tile references

/// Position of tile `idx` in a frame.
#[derive(Clone, Copy, Debug)]
pub struct TileRef {
    pub idx: usize,
    pub ox: i32,
    pub oy: i32,
    pub per_row: usize,
    pub count: usize,
}

pub fn tile_ref(frame: &Frame, idx: usize) -> Result<TileRef> {
    let g = &frame.grid;
    if g.w % T != 0 || g.h % T != 0 || g.w == 0 {
        return Err(PxlError::usage(format!("frame {} is {}x{}: --tile needs a frame made of 8x8 tiles", frame.name, g.w, g.h)));
    }
    let (per_row, count) = (g.w / T, g.w / T * (g.h / T));
    if idx >= count {
        return Err(PxlError::usage(format!("tile {} does not exist: frame {} has {} tiles (0..{})", idx, frame.name, count, count - 1)).with_hint("tiles are numbered row by row, 16 per row in a 128 px sheet"));
    }
    Ok(TileRef { idx, ox: ((idx % per_row) * T) as i32, oy: ((idx / per_row) * T) as i32, per_row, count })
}

pub struct TileStatus {
    pub line: String,
    pub json: Value,
}

/// "tile 37: palette 3 (grass) ok, 2 placements, unique".
pub fn status_of(part: &Part, tpl: Option<&Template>, fi: usize, idx: usize) -> TileStatus {
    let frame = &part.frames[fi];
    let Some(tpl) = tpl else {
        return TileStatus { line: format!("tile {}: no template, palette fit not checked", idx), json: json!({"tile": idx}) };
    };
    let sheet = Sheet::from_grid(&frame.grid);
    let cols = Colors::new(part, tpl);
    let t = &sheet.tiles[idx];
    let keys = t.key_set();
    let ncol = stats::colors_of(&t.px(&cols)).len();
    let fit: String;
    let mut fit_ok = true;
    let mut pal_json = Value::Null;
    if ncol > 4 {
        fit = format!("{} colours, at most 4 per tile", ncol);
        fit_ok = false;
    } else if !tpl.palettes.is_empty() {
        match tiles::reduce::palette_of(tpl, &keys) {
            Some(p) => {
                fit = format!("palette {} ({}) ok", p + 1, tpl.palettes[p].name);
                pal_json = json!({"index": p + 1, "name": tpl.palettes[p].name});
            }
            None => {
                let ks: String = keys.iter().map(|&k| k as char).collect();
                let best = tpl.palettes.iter().enumerate().max_by_key(|(_, p)| keys.iter().filter(|k| p.keys.contains(k)).count());
                let near = best
                    .map(|(i, p)| {
                        let miss: String = keys.iter().filter(|k| !p.keys.contains(k)).map(|&k| k as char).collect();
                        format!("; closest is palette {} ({}) which lacks '{}'", i + 1, p.name, miss)
                    })
                    .unwrap_or_default();
                fit = format!("keys '{}' fit no palette{}", ks, near);
                fit_ok = false;
            }
        }
    } else {
        fit = format!("{} colours ok", ncol);
    }
    let groups = stats::unique_groups(&sheet, &cols);
    let g = groups.iter().find(|g| g.contains(&idx)).cloned().unwrap_or_else(|| vec![idx]);
    let first = *g.iter().min().unwrap_or(&idx);
    let (uniq, uniq_txt) = if g.len() == 1 {
        (true, "unique".to_string())
    } else if first == idx {
        let others: Vec<String> = g.iter().filter(|&&t| t != idx).take(6).map(|t| t.to_string()).collect();
        (true, format!("unique, copies: tile {}", others.join(", ")))
    } else {
        (false, format!("duplicate of tile {}", first))
    };
    let pl: Option<Vec<u32>> = None;
    let pl_n = pl.as_ref().and_then(|u| u.get(idx).copied());
    let pl_txt = pl_n.map(|n| format!(", {} placement{}", n, if n == 1 { "" } else { "s" })).unwrap_or_default();
    TileStatus {
        line: format!("tile {}: {}{}, {}", idx, fit, pl_txt, uniq_txt),
        json: json!({"tile": idx, "colors": ncol, "keys": keys.iter().map(|&k| (k as char).to_string()).collect::<Vec<_>>(), "palette": pal_json, "fits": fit_ok,
            "placements": pl_n, "unique": uniq, "duplicate_of": if uniq { Value::Null } else { json!(first) }, "same_as": g.iter().filter(|&&t| t != idx).collect::<Vec<_>>(),
            "unique_total": groups.len(), "tiles": sheet.tiles.len()}),
    }
}

/// The tile as rows with a tile-relative ruler, optionally with the 16x16 reference rows (`3a`/`3b` labels).
pub fn tile_lines(part: &Part, frame: &Frame, tr: &TileRef, with_ref: bool) -> Result<(Vec<String>, Option<String>, Vec<String>)> {
    let g = &frame.grid;
    let rows: Vec<String> = (0..T).map(|y| (0..T).map(|x| g.get(tr.ox + x as i32, tr.oy + y as i32) as char).collect()).collect();
    if with_ref {
        let rb = crate::show::ref_block_off(part, frame, (tr.ox, tr.oy, T, T), (tr.ox, tr.oy))?;
        let legend = format!("legend: {}  (right: reference letters; >K = its template key, ~K = nearest key)", rb.legend);
        return Ok((rb.lines.iter().map(|l| l.trim_end().to_string()).collect(), Some(legend), rows));
    }
    let mut lines = crate::show::ruler(0, T, 1, 3);
    for (y, r) in rows.iter().enumerate() {
        lines.push(format!("{} |{}", y, r));
    }
    Ok((lines, None, rows))
}

/// `pxl show P F --tile N [--ref]`
pub fn show_tile(part: &Part, frame: &Frame, idx: usize, with_ref: bool, raw: bool) -> Result<(String, Value)> {
    let tr = tile_ref(frame, idx)?;
    let tpl = crate::template::load_for(part).ok();
    let fi = part.frame_index(&frame.name).unwrap_or(0);
    let st = status_of(part, tpl.as_ref(), fi, idx);
    let (mut lines, legend, rows) = tile_lines(part, frame, &tr, with_ref)?;
    if raw && !with_ref {
        lines.drain(..lines.len() - T);
    }
    let mut text = format!(
        "{}/{} tile {} at {},{} (frame coordinates) 8x8 region 0,0,8,8{}\n{}",
        part.name,
        frame.name,
        idx,
        tr.ox,
        tr.oy,
        if frame.locked { " locked" } else { "" },
        st.line
    );
    if with_ref {
        text.push_str("\nreference (left: tile, right: input at native resolution, rows 3a/3b; '.' = transparent)");
    }
    for l in &lines {
        text.push('\n');
        text.push_str(l);
    }
    if let Some(l) = &legend {
        text.push('\n');
        text.push_str(l);
    }
    let mut j = json!({"part": part.name, "frame": frame.name, "locked": frame.locked, "tile": idx, "at": [tr.ox, tr.oy], "size": [8, 8], "region": [0, 0, 8, 8], "rows": rows, "status": st.json});
    j["status_line"] = json!(st.line);
    Ok((text, j))
}

/// The echo of a tile edit: changed count, the tile (tile-relative), and the status line last.
pub fn edit_echo(part: &Part, tpl: Option<&Template>, fi: usize, idx: usize, changed: usize, cmd: &str, with_ref: bool, tag: &str) -> Result<(String, Value)> {
    let frame = &part.frames[fi];
    let tr = tile_ref(frame, idx)?;
    let st = status_of(part, tpl, fi, idx);
    let mut text = format!("{} {}/{}: {} px changed in tile {} (tile coordinates 0..7){}", cmd, part.name, frame.name, changed, idx, tag);
    let (lines, legend, rows) = tile_lines(part, frame, &tr, with_ref).or_else(|_| tile_lines(part, frame, &tr, false))?;
    for l in &lines {
        text.push('\n');
        text.push_str(l);
    }
    if let Some(l) = legend {
        text.push('\n');
        text.push_str(&l);
    }
    text.push('\n');
    text.push_str(&st.line);
    Ok((text, json!({"frame": frame.name, "tile": idx, "changed": changed, "region": [0, 0, 8, 8], "rows": rows, "status": st.json, "status_line": st.line})))
}

/// `pxl view P F --tile N`: reference | tile, in tile coordinates.
pub fn view_tile(a: &crate::cli::ViewArgs, out: &mut Out) -> Result<()> {
    let idx = a.tile.unwrap_or(0);
    let path = fsutil::resolve_part(&a.part)?;
    let part = crate::pxl::load(&path)?;
    let tpl = crate::template::load_for(&part).ok();
    let fi = part.frame_index(&a.frame).ok_or_else(|| crate::cli::unknown_frame(&part, &a.frame))?;
    let frame = &part.frames[fi];
    let tr = tile_ref(frame, idx)?;
    let rf0 = if a.no_ref { None } else { crate::show::try_reference(&part, frame) };
    let f = part.input.factor.or(part.reduce.factor).unwrap_or(1).max(1) as i32;
    let ph = part.reduce.phase.unwrap_or((0, 0));
    // a self-contained 8x8 part so the panels label tile coordinates
    let mut sub = part.clone();
    sub.origin = (0, 0);
    sub.reduce.phase = None;
    let mut g = Grid::new(T, T, b'.');
    for y in 0..T {
        for x in 0..T {
            g.cells[y * T + x] = frame.grid.get(tr.ox + x as i32, tr.oy + y as i32);
        }
    }
    let rf = rf0.map(|r| {
        let rx = r.origin.0 + f * (tr.ox - part.origin.0) + ph.0;
        let ry = r.origin.1 + f * (tr.oy - part.origin.1) + ph.1;
        crate::model::RefFrame { img: r.img.crop(rx, ry, T * f as usize, T * f as usize), origin: (0, 0) }
    });
    let (im, s) = crate::show::render_panels(&sub, tpl.as_ref(), rf.as_ref(), &[(format!("{} tile {}", frame.name, idx), &g)], (0, 0, T, T), None, a.zoom);
    let p = a.out.as_ref().map(PathBuf::from).unwrap_or_else(|| crate::show::default_png(&part, &frame.name, &format!("-tile{}", idx)));
    crate::show::write_png(&p, &im)?;
    let st = status_of(&part, tpl.as_ref(), fi, idx);
    out.emit(
        format!("view {}/{} tile {}: {}  ({}x{} px, cells at {} px, tile coordinates 0..7; {})\n{}", part.name, frame.name, idx, p.slash(), im.w, im.h, f as usize * s, if rf.is_some() { "reference | tile" } else { "tile only" }, st.line),
        json!({"part": part.name, "frame": frame.name, "tile": idx, "path": p.slash(), "size": [im.w, im.h], "zoom": s, "reference": rf.is_some(), "status": st.json}),
    );
    Ok(())
}

// ------------------------------------------------------------------------------------------------ compare

struct TileM {
    err: f64,
    max: f64,
    bad_px: usize,
    usage: u32,
    score: f64,
    colors: usize,
}

const PAD: usize = 6;
const GAP: usize = 2;

fn lerp(a: Rgba, b: Rgba, t: f64) -> Rgba {
    let m = |x: u8, y: u8| (x as f64 + (y as f64 - x as f64) * t).round() as u8;
    Rgba::opaque(m(a.r, b.r), m(a.g, b.g), m(a.b, b.b))
}

/// Pages of `reference block | tile x2 | diff`, ranked by Oklab error x placements, worst first.
pub fn compare_tileset(path: &Path, part: &Part, a: &CompareArgs, out: &mut Out) -> Result<()> {
    let tpl = crate::template::load_for(part)?;
    let want = a.sel.frames.first().map(|s| s.as_str());
    let fi = tiles::pick_frame(part, want)?;
    let frame = &part.frames[fi];
    let rf = load::reference(part, &std::fs::canonicalize(part.dir()).unwrap_or_else(|_| part.dir()), frame)?;
    let sheet = Sheet::from_grid(&frame.grid);
    let cols = Colors::new(part, &tpl);
    let f = part.input.factor.or(part.reduce.factor).unwrap_or(1).max(1) as usize;
    let ph = part.reduce.phase.unwrap_or((0, 0));
    let n = sheet.tiles.len();
    let usage: Vec<u32> = vec![1; n];
    let kc = |k: u8| -> Rgba {
        let c = compare::key_color(part, Some(&tpl), k);
        Rgba::opaque(c.r, c.g, c.b)
    };
    let lab = |c: Rgba| -> Oklab { color::srgb_to_oklab(Rgb { r: c.r, g: c.g, b: c.b }) };
    let bs = T * f;
    let mut ms: Vec<TileM> = Vec::new();
    // per tile reference crop (opaque pixels only)
    let mut crops: Vec<Vec<Rgba>> = Vec::with_capacity(n);
    let mut diffs: Vec<Vec<f64>> = Vec::with_capacity(n);
    for (i, t) in sheet.tiles.iter().enumerate() {
        let (px, py) = sheet.pos(i);
        let rx = rf.origin.0 + f as i32 * (px as i32 - part.origin.0) + ph.0;
        let ry = rf.origin.1 + f as i32 * (py as i32 - part.origin.1) + ph.1;
        let mut crop = Vec::with_capacity(bs * bs);
        let mut d = Vec::with_capacity(bs * bs);
        let (mut sum, mut mx, mut cnt, mut bad) = (0.0f64, 0.0f64, 0usize, 0usize);
        for y in 0..bs {
            for x in 0..bs {
                let r = rf.img.get(rx + x as i32, ry + y as i32);
                crop.push(r);
                if r.a < 128 {
                    d.push(0.0);
                    continue;
                }
                let tk = t.keys[(y / f) * T + x / f];
                let e = color::oklab_dist2(lab(kc(tk)), lab(r)).sqrt() * 100.0;
                sum += e;
                mx = mx.max(e);
                cnt += 1;
                if e > 5.0 {
                    bad += 1;
                }
                d.push(e);
            }
        }
        let err = if cnt > 0 { sum / cnt as f64 } else { 0.0 };
        let u = usage.get(i).copied().unwrap_or(0);
        ms.push(TileM { err, max: mx, bad_px: bad, usage: u, score: err * u as f64, colors: stats::colors_of(&t.px(&cols)).len() });
        crops.push(crop);
        diffs.push(d);
    }
    // all-air tiles (backdrop only) dominate by placements and say nothing about the art: not ranked, kept in the json
    let air_codes: Vec<u32> = tpl.tile_air.iter().map(|&k| cols.code[k as usize]).collect();
    let backdrop_only: Vec<bool> = sheet.tiles.iter().map(|t| !air_codes.is_empty() && t.keys.iter().all(|&k| air_codes.contains(&cols.code[k as usize]))).collect();
    let mut rank: Vec<usize> = (0..n).filter(|&i| !backdrop_only[i]).collect();
    rank.sort_by(|&x, &y| ms[y].score.partial_cmp(&ms[x].score).unwrap_or(std::cmp::Ordering::Equal).then(ms[y].err.partial_cmp(&ms[x].err).unwrap_or(std::cmp::Ordering::Equal)).then(x.cmp(&y)));

    // layout: cells of three panels, ranked; scale = display px per reference px
    let layout = |s: usize| -> (usize, usize, usize, usize, usize) {
        let pw = 16 * s.max(1);
        let _ = pw;
        let panel = bs * s;
        let cell_w = 3 * panel + 2 * GAP + PAD;
        let cell_h = panel + compare::LABEL_H + PAD;
        let cols_n = ((MAX_PX - 2 * MARGIN - GUT_L) / cell_w).max(1);
        let rows_n = ((MAX_PX - 2 * MARGIN - HEADER_H - GUT_T) / cell_h).max(1);
        (panel, cell_w, cell_h, cols_n, rows_n)
    };
    let max_pages = 3usize;
    let s = match a.zoom {
        Some(z) if z > 0 => z,
        _ => (1..=(8 / f.max(1)).max(1)).rev().find(|s| {
            let l = layout(*s);
            l.0 * 3 + 2 * GAP + PAD + 2 * MARGIN + GUT_L <= MAX_PX && rank.len().div_ceil(l.3 * l.4) <= max_pages
        })
        .unwrap_or(1),
    };
    let (panel, cell_w, cell_h, cols_n, rows_n) = layout(s);
    if 3 * panel + 2 * GAP + 2 * MARGIN + GUT_L > MAX_PX {
        return Err(PxlError::usage(format!("--zoom {} makes a tile cell {} px wide, over the {} px limit", s, 3 * panel, MAX_PX)).with_hint("use a smaller --zoom"));
    }
    let per_page = cols_n * rows_n;
    let pages_n = rank.len().div_ceil(per_page).min(max_pages).max(1);
    let shown = (pages_n * per_page).min(rank.len());
    let dir = a.out.as_ref().map(PathBuf::from).unwrap_or_else(|| path.parent().map(|p| p.to_path_buf()).unwrap_or_default().join("compare"));
    let mut page_paths: Vec<PathBuf> = Vec::new();
    let mut page_json: Vec<Value> = Vec::new();
    for pg in 0..pages_n {
        let lo = pg * per_page;
        let hi = ((pg + 1) * per_page).min(shown);
        let rows_used = (hi - lo).div_ceil(cols_n).max(1);
        let (iw, ih) = (2 * MARGIN + GUT_L + cols_n.min(hi - lo) * cell_w - PAD, 2 * MARGIN + HEADER_H + GUT_T + rows_used * cell_h - PAD);
        let mut im = RgbaImage::filled(iw.max(8), ih.max(8), compare::BG);
        compare::draw_text(&mut im, MARGIN, MARGIN, &format!("{}/{}: ranked worst error x most used first, ranks {}-{} of {}  (reference | tile x2 | diff)", part.name, frame.name, lo + 1, hi, rank.len()), 2, TEXT);
        let (x0, y0) = (MARGIN + GUT_L, MARGIN + HEADER_H + GUT_T);
        for c in 0..cols_n.min(hi - lo) {
            compare::draw_text(&mut im, x0 + c * cell_w + 1, MARGIN + HEADER_H, &format!("c{}", c + 1), 2, DIM);
        }
        for r in 0..rows_used {
            compare::draw_text(&mut im, MARGIN, y0 + r * cell_h + compare::LABEL_H + (panel.saturating_sub(10)) / 2, &format!("{}", lo + r * cols_n + 1), 2, DIM);
        }
        let mut cells_json = Vec::new();
        for (slot, &ti) in rank[lo..hi].iter().enumerate() {
            let (c, r) = (slot % cols_n, slot / cols_n);
            let (cx, cy) = (x0 + c * cell_w, y0 + r * cell_h);
            let m = &ms[ti];
            let col = if m.err < 2.0 { C_GOOD } else if m.err < 4.0 { C_WARN } else { C_BAD };
            compare::fill(&mut im, cx, cy, 3 * panel + 2 * GAP, compare::LABEL_H - 2, PLATE);
            let lab_txt = format!("#{} e{:.1} u{}", ti, m.err, m.usage);
            let lab_txt = if compare::text_w(&lab_txt, 2) > 3 * panel { format!("#{}", ti) } else { lab_txt };
            compare::draw_text(&mut im, cx + 2, cy + 1, &lab_txt, 2, col);
            let py = cy + compare::LABEL_H;
            let mut a_ref = compare::checker(panel, panel, 4);
            let mut b_til = RgbaImage::filled(panel, panel, compare::BG);
            let mut c_dif = RgbaImage::filled(panel, panel, compare::BG);
            for y in 0..bs {
                for x in 0..bs {
                    let r0 = crops[ti][y * bs + x];
                    let t = lerp(Rgba::opaque(0x2a, 0x2b, 0x31), Rgba::opaque(0xf0, 0x40, 0x40), (diffs[ti][y * bs + x] / 10.0).min(1.0));
                    let tk = sheet.tiles[ti].keys[(y / f) * T + x / f];
                    let tc = kc(tk);
                    for dy in 0..s {
                        for dx in 0..s {
                            let o = (y * s + dy) * panel + x * s + dx;
                            if r0.a > 0 {
                                a_ref.px[o] = compare::blend(a_ref.px[o], r0);
                            }
                            b_til.px[o] = tc;
                            c_dif.px[o] = t;
                        }
                    }
                }
            }
            compare::paste(&mut im, &a_ref, cx, py);
            compare::paste(&mut im, &b_til, cx + panel + GAP, py);
            compare::paste(&mut im, &c_dif, cx + 2 * (panel + GAP), py);
            cells_json.push(json!({"tile": ti, "rank": lo + slot + 1, "col": c + 1, "row": r + 1, "origin": [cx, py]}));
        }
        if im.w > MAX_PX || im.h > MAX_PX {
            return Err(PxlError::usage(format!("compare page would be {}x{} px, over the {} px limit", im.w, im.h, MAX_PX)));
        }
        let p = dir.join(format!("{}-tiles-{:02}.png", part.name, pg + 1));
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        fsutil::atomic_write(&p, &compare::encode_sheet_png(&im))?;
        page_json.push(json!({"path": p.slash(), "size": [im.w, im.h], "ranks": [lo + 1, hi], "columns": cols_n.min(hi - lo), "rows": rows_used, "cells": cells_json}));
        page_paths.push(p);
    }
    let mean_err = if n > 0 { ms.iter().map(|m| m.err).sum::<f64>() / n as f64 } else { 0.0 };
    let entry = |r: Option<usize>, i: usize| {
        let m = &ms[i];
        let (x, y) = sheet.pos(i);
        json!({"rank": r, "tile": i, "x": x, "y": y, "error": round2(m.err), "max_error": round2(m.max), "px_over_5": m.bad_px, "usage": m.usage, "score": round2(m.score), "colors": m.colors, "backdrop_only": backdrop_only[i]})
    };
    let mut ranking: Vec<Value> = rank.iter().enumerate().map(|(r, &i)| entry(Some(r + 1), i)).collect();
    ranking.extend((0..n).filter(|&i| backdrop_only[i]).map(|i| entry(None, i)));
    let body = json!({
        "part": part.name, "frame": frame.name, "kind": "tileset", "tiles": n, "ranked": rank.len(), "backdrop_only_tiles": backdrop_only.iter().filter(|b| **b).count(), "scale": s, "per_page": per_page, "shown": shown,
        "pages": page_json, "ranking": ranking,
        "summary": {"mean_error": round2(mean_err), "worst_error": round2(ms.iter().map(|m| m.err).fold(0.0, f64::max)), "tiles_error_over_4": ms.iter().filter(|m| m.err >= 4.0).count()},
        "legend": "error = mean Oklab distance x100 between the tile (shown x2) and the reference block per reference pixel; score = error",
    });
    let jp = dir.join(format!("{}-tiles.json", part.name));
    fsutil::atomic_write(&jp, serde_json::to_string_pretty(&body).unwrap_or_default().as_bytes())?;
    out.line(format!("{}/{}: {} tiles, mean error {:.2}{}", part.name, frame.name, n, mean_err, ""));
    for p in &page_paths {
        out.line(format!("  {}", p.slash()));
    }
    out.line(format!("  {}", jp.slash()));
    out.line("  where to look first (error x placements; not a quality verdict):");
    for (r, &i) in rank.iter().take(15).enumerate() {
        let m = &ms[i];
        let (x, y) = sheet.pos(i);
        out.line(format!("  {:>3}. tile {:>3} ({:>3},{:>3})  error {:>5.2}  max {:>5.1}  used {:>4}x  score {:>7.1}", r + 1, i, x, y, m.err, m.max, m.usage, m.score));
    }
    if let Value::Object(mm) = body {
        for (k, v) in mm {
            out.set(&k, v);
        }
    }
    Ok(())
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}
