//! The design language of a project (M6 R7): `pxl style analyze | propose | approve | mark | show`, `style.md` and `style.toml`.
//!
//! * `style analyze` measures the input style per asset and over the project (outline, tones per material, light direction, highlight style,
//!   dithering, palette size, eye construction) into `.pxl/style_analysis.json`.
//! * `style propose` combines that with the console's `[style]` and its hardware limits into `style.md`: the rules (each with an id the lint rules
//!   and the review checklist quote), the identity list per asset, the shared material ramps and, per limit, what it forces on this input.
//! * `style approve` is a milestone; `style mark ASSET [FRAMES]` names the style frames, the reference assets every producer compares against.
//! * `style.toml` holds the machine state: approval (with a hash of style.md), the style frames, the shared ramps and the dry-run record.

use crate::fsutil::PathSlash;
use crate::asset;
use crate::cli::*;
use crate::color::Rgb;
use crate::img::{Rgba, RgbaImage};
use crate::insight::{self, hex};
use crate::model::{Part, PartKind};
use crate::output::{Out, PxlError, Result};
use crate::{console, fsutil, load, profile, pxl, status, tasks, template, versions};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------------------------- state files

pub fn md_path(home: &Path) -> PathBuf {
    home.join("style.md")
}

pub fn state_path(home: &Path) -> PathBuf {
    home.join("style.toml")
}

pub fn analysis_path(home: &Path) -> PathBuf {
    home.join(".pxl").join("style_analysis.json")
}

pub fn read_state(home: &Path) -> toml::Table {
    std::fs::read_to_string(state_path(home)).ok().and_then(|t| t.parse::<toml::Table>().ok()).unwrap_or_default()
}

pub fn write_state(home: &Path, t: &toml::Table) -> Result<()> {
    let body = toml::to_string(t).map_err(|e| PxlError::io(e.to_string()))?;
    fsutil::atomic_write_plain(&state_path(home), format!("# written by `pxl style`; edit with the style, milestone and deliver commands, not by hand.\n{}", body).as_bytes())
}

fn sub<'a>(t: &'a toml::Table, k: &str) -> Option<&'a toml::Table> {
    t.get(k).and_then(|v| v.as_table())
}

/// Style frames: `(asset name, frame names)`; an empty frame list means every frame.
pub fn style_frames(home: &Path) -> Vec<(String, Vec<String>)> {
    let st = read_state(home);
    sub(&st, "frames")
        .map(|f| f.iter().map(|(k, v)| (k.clone(), v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default())).collect())
        .unwrap_or_default()
}

pub fn is_style_frame(home: &Path, asset_name: &str) -> bool {
    style_frames(home).iter().any(|f| f.0 == asset_name)
}

/// The text of style.md without its status line, hashed: approval stays valid while the rules are unchanged.
fn body_hash(text: &str) -> String {
    let body: String = text.lines().filter(|l| !l.starts_with("Status:")).collect::<Vec<_>>().join("\n");
    fsutil::content_hash(body.as_bytes())
}

#[derive(Clone, Debug, Default)]
pub struct DocStatus {
    pub exists: bool,
    pub approved: bool,
    pub changed: bool,
    pub by: String,
    pub on: String,
}

pub fn doc_status(home: &Path) -> DocStatus {
    let Ok(text) = std::fs::read_to_string(md_path(home)) else { return DocStatus::default() };
    let st = read_state(home);
    let ap = sub(&st, "approval");
    let hash = ap.and_then(|a| a.get("hash")).and_then(|h| h.as_str()).unwrap_or("");
    let s = |k: &str| ap.and_then(|a| a.get(k)).and_then(|h| h.as_str()).unwrap_or("").to_string();
    let approved = !hash.is_empty();
    DocStatus { exists: true, approved: approved && hash == body_hash(&text), changed: approved && hash != body_hash(&text), by: s("by"), on: s("on") }
}

/// Rules of style.md: `- **id**: text` lines below `## Rules`.
pub fn rules(home: &Path) -> Vec<(String, String)> {
    let Ok(text) = std::fs::read_to_string(md_path(home)) else { return Vec::new() };
    let mut in_rules = false;
    let mut v = Vec::new();
    for l in text.lines() {
        if l.starts_with("## ") {
            in_rules = l.trim() == "## Rules";
            continue;
        }
        if in_rules {
            if let Some(rest) = l.strip_prefix("- **") {
                if let Some((id, t)) = rest.split_once("**:") {
                    v.push((id.trim().to_string(), t.trim().to_string()));
                }
            }
        }
    }
    v
}

// ---------------------------------------------------------------------------------------------- measuring

#[derive(Clone, Debug, Default)]
pub struct FamM {
    pub name: String,
    pub neutral: bool,
    pub share: f64,
    pub tones: usize,
    /// The material's tones as exact source colours, dark to light, distinct (one to three).
    pub ramp: Vec<Rgb>,
    pub chroma: f64,
}

#[derive(Clone, Debug, Default)]
pub struct EyeM {
    pub frames_with: usize,
    pub max_per_frame: usize,
    /// Median eye white in cells (w, h) and pupil pixels in cells.
    pub white_cells: (i32, i32),
    /// The smallest eye white of the asset in cells (width and height separately).
    pub white_min: (i32, i32),
    pub pupil_cells: usize,
}

#[derive(Clone, Debug, Default)]
pub struct AssetM {
    pub name: String,
    pub group: String,
    pub mode: String,
    pub frames: usize,
    pub with_input: bool,
    /// Frames whose source image was unavailable (`reference_missing`).
    pub ref_missing: Vec<String>,
    pub factor: usize,
    pub size: (usize, usize),
    pub outline_presence: f64,
    pub outline_colour: Option<Rgb>,
    /// In output pixels.
    pub outline_thickness: f64,
    pub families: Vec<FamM>,
    pub light: (String, f64),
    pub highlight: (String, f64),
    pub dither: f64,
    pub palette: (usize, usize),
    pub eyes: EyeM,
    pub identity: Vec<(Rgb, f64)>,
}

impl AssetM {
    pub fn to_json(&self) -> Value {
        json!({
            "asset": self.name, "group": self.group, "mode": self.mode, "frames": self.frames, "input": self.with_input, "reference_missing": self.ref_missing, "factor": self.factor,
            "outline": {"presence": r3(self.outline_presence), "colour": self.outline_colour.map(hex), "thickness": r3(self.outline_thickness)},
            "materials": self.families.iter().map(|f| json!({"name": f.name, "neutral": f.neutral, "share": r3(f.share), "tones": f.tones, "ramp": f.ramp.iter().map(|c| hex(*c)).collect::<Vec<_>>()})).collect::<Vec<_>>(),
            "light": {"direction": self.light.0, "strength": r3(self.light.1)},
            "highlight": {"style": self.highlight.0, "share": r3(self.highlight.1)},
            "dither": r3(self.dither),
            "palette": {"distinct": self.palette.0, "major": self.palette.1},
            "eyes": {"frames_with": self.eyes.frames_with, "max_per_frame": self.eyes.max_per_frame, "white_cells": [self.eyes.white_cells.0, self.eyes.white_cells.1], "pupil_cells": self.eyes.pupil_cells},
            "identity": self.identity.iter().map(|(c, s)| json!({"colour": hex(*c), "name": insight::colour_name(*c), "share": r3(*s)})).collect::<Vec<_>>(),
        })
    }
}

fn r3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// The reduced frame as an image (transparent where empty), using the template colours.
pub fn render_frame(part: &Part, tpl: Option<&template::Template>, fi: usize) -> RgbaImage {
    let g = &part.frames[fi].grid;
    let mut im = RgbaImage::new(g.w, g.h);
    for y in 0..g.h {
        for x in 0..g.w {
            let k = g.cells[y * g.w + x];
            if k == b'.' {
                continue;
            }
            if let Some(c) = tpl.and_then(|t| crate::lint::effective_color(part, t, k)) {
                im.px[y * g.w + x] = Rgba::opaque(c.r, c.g, c.b);
            }
        }
    }
    im
}

/// Measure one asset from its input images (else from its reduced frames).
pub fn measure(file: &Path, group: &str) -> Result<AssetM> {
    let part = pxl::load(file)?;
    let dir = asset::dir_of(file);
    let tpl = template::load_for(&part).ok();
    let factor = part.reduce.factor.or(part.input.factor).unwrap_or(1).max(1) as usize;
    let mut m = AssetM { name: asset::asset_name(&dir), group: group.to_string(), mode: part.kind.mode_name().to_string(), frames: part.frames.len(), factor, size: part.size, ..Default::default() };
    let mut ref_imgs: Vec<(RgbaImage, usize)> = Vec::new();
    let mut eye_w: Vec<(i32, i32)> = Vec::new();
    let mut pupil: Vec<usize> = Vec::new();
    for f in part.frames.iter() {
        let loaded = load::reference(&part, &dir, f);
        if loaded.is_err() {
            m.ref_missing.push(f.name.clone());
        }
        if let Ok(rf) = loaded {
            let n = match crate::features::Geo::of(&part, &rf) {
                Some(g) => {
                    let eyes = crate::features::detect(&rf.img, g);
                    for e in &eyes {
                        eye_w.push(e.white_cells(g.f));
                        pupil.push(e.pcells.len().max(1));
                    }
                    eyes.len()
                }
                None => insight::eyes_by_pixels(rf.img.w, rf.img.h, 0.85, &insight::image_px(&rf.img)),
            };
            if n > 0 {
                m.eyes.frames_with += 1;
            }
            m.eyes.max_per_frame = m.eyes.max_per_frame.max(n);
            ref_imgs.push((rf.img, factor));
        }
    }
    m.with_input = !ref_imgs.is_empty();
    let imgs: Vec<(RgbaImage, usize)> = if m.with_input { ref_imgs } else { (0..part.frames.len()).map(|i| (render_frame(&part, tpl.as_ref(), i), 1)).collect() };
    if imgs.is_empty() {
        return Ok(m);
    }
    let mut all: Vec<(i32, i32, Rgb)> = Vec::new();
    let (mut pres, mut thick, mut bsum) = (0.0, 0.0, 0.0);
    let mut dark_cols: Vec<Rgb> = Vec::new();
    let (mut hl_share, mut dith) = (0.0, 0.0);
    let mut hl_style = (0usize, 0usize, 0usize);
    for (im, f) in &imgs {
        let ol = insight::outline(im);
        pres += ol.presence * ol.boundary as f64;
        thick += (ol.thickness / *f as f64) * ol.boundary as f64;
        bsum += ol.boundary as f64;
        if let Some(c) = ol.colour {
            dark_cols.push(c);
        }
        let (hs, hsh) = insight::highlight(im);
        hl_share += hsh;
        match hs.as_str() {
            "none" => hl_style.0 += 1,
            "spots" => hl_style.1 += 1,
            _ => hl_style.2 += 1,
        }
        dith += insight::dither(im, *f);
        all.extend(insight::opaque(im));
    }
    let nf = imgs.len() as f64;
    m.outline_presence = if bsum > 0.0 { pres / bsum } else { 0.0 };
    m.outline_thickness = if bsum > 0.0 { thick / bsum } else { 0.0 };
    m.outline_colour = dark_cols.first().copied().map(|_| {
        let k = dark_cols.len() as f64;
        Rgb { r: (dark_cols.iter().map(|c| c.r as f64).sum::<f64>() / k).round() as u8, g: (dark_cols.iter().map(|c| c.g as f64).sum::<f64>() / k).round() as u8, b: (dark_cols.iter().map(|c| c.b as f64).sum::<f64>() / k).round() as u8 }
    });
    m.highlight = (
        if hl_style.2 > hl_style.1.max(hl_style.0) { "areas" } else if hl_style.1 >= hl_style.0 && hl_style.1 > 0 { "spots" } else { "none" }.to_string(),
        hl_share / nf,
    );
    m.dither = dith / nf;
    let colours: Vec<Rgb> = all.iter().map(|p| p.2).collect();
    m.palette = insight::palette_size(&colours);
    m.light = insight::light_direction(&all);
    m.identity = insight::identity_colours(&colours);
    // materials: the outline and the pure whites are not materials (they have their own rules), so a hero with no grey has no grey
    let mats = insight::material_pixels(&colours);
    m.families = insight::families(&mats)
        .into_iter()
        .filter(|f| f.share >= 0.02)
        .map(|f| FamM { name: f.name(), neutral: f.neutral, share: f.share, tones: f.tones, ramp: f.ramp_src.clone(), chroma: f.chroma })
        .collect();
    if !eye_w.is_empty() {
        eye_w.sort();
        m.eyes.white_min = (eye_w.iter().map(|e| e.0).min().unwrap_or(0), eye_w.iter().map(|e| e.1).min().unwrap_or(0));
        m.eyes.white_cells = eye_w[eye_w.len() / 2];
        pupil.sort();
        m.eyes.pupil_cells = pupil[pupil.len() / 2];
    }
    Ok(m)
}

fn project_home() -> Result<PathBuf> {
    let c = std::env::current_dir().map_err(|e| PxlError::io(e.to_string()))?;
    crate::manifest::Manifest::find_file(&c).map(|m| asset::dir_of(&m)).ok_or_else(|| PxlError::usage("no project here (pxl.toml not found)").with_hint("pxl new DIR --console gbc"))
}

/// Assets selected by name (default: all) as `(file, group)`.
fn select(home: &Path, names: &[String]) -> Result<Vec<(PathBuf, String)>> {
    let all = asset::scan(home);
    if names.is_empty() {
        return Ok(all.into_iter().map(|a| (a.file, a.group)).collect());
    }
    let mut v = Vec::new();
    for n in names {
        let f = fsutil::resolve_part(n)?;
        let g = all.iter().find(|a| a.file == f || crate::manifest::canon(&a.file) == crate::manifest::canon(&f)).map(|a| a.group.clone()).unwrap_or_default();
        v.push((f, g));
    }
    Ok(v)
}

pub fn measure_all(home: &Path, names: &[String]) -> Result<Vec<AssetM>> {
    let sel = select(home, names)?;
    let r: Vec<Option<AssetM>> = fsutil::par_map(&sel, |(f, g)| measure(f, g).ok());
    Ok(r.into_iter().flatten().collect())
}

fn median(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    v[v.len() / 2]
}

/// The project-wide picture: shared medians and the shared material ramps.
#[derive(Clone, Debug, Default)]
pub struct Aggregate {
    pub assets: usize,
    pub with_input: usize,
    pub outline_presence: f64,
    pub outline_thickness: f64,
    pub outline_colour: Option<Rgb>,
    pub tones: usize,
    pub light: String,
    pub highlight: String,
    pub dither: f64,
    pub major_colours: usize,
    pub factor: usize,
    pub eyes_assets: usize,
    pub eye_white: (i32, i32),
    /// Minimum eye white (cells) the style asks for: the smallest source eye at the target scale, never below 2x3.
    pub eye_min: (i32, i32),
    pub eye_max: usize,
    /// Materials seen in more than one asset (or the main ones): `(name, ramp, assets)`.
    pub materials: Vec<(String, Vec<Rgb>, Vec<String>)>,
}

/// The smallest eye white a style may ask for, in output cells: 2 wide, 3 high (the `pxl-sprite` skill's eye rule).
pub const EYE_FLOOR: (i32, i32) = (2, 3);

/// The minimum eye white (width, height in cells) of the project's style: `[eyes] min_white` of style.toml, else the `white WxH cells` of the eyes rule
/// of style.md; `None` when the project has no style.md eyes rule (nothing to check against).
pub fn eye_min(home: &Path) -> Option<(usize, usize)> {
    let st = read_state(home);
    if let Some(a) = sub(&st, "eyes").and_then(|e| e.get("min_white")).and_then(|v| v.as_array()) {
        if let (Some(w), Some(h)) = (a.first().and_then(|x| x.as_integer()), a.get(1).and_then(|x| x.as_integer())) {
            return Some((w.max(1) as usize, h.max(1) as usize));
        }
    }
    for (id, t) in rules(home) {
        if id == "eyes" {
            if let Some(i) = t.find("white ") {
                let rest = &t[i + 6..];
                let w: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                let rest2 = rest[w.len()..].strip_prefix('x').unwrap_or("");
                let h: String = rest2.chars().take_while(|c| c.is_ascii_digit()).collect();
                if let (Ok(w), Ok(h)) = (w.parse::<usize>(), h.parse::<usize>()) {
                    return Some((w, h));
                }
            }
        }
    }
    None
}

pub fn aggregate(ms: &[AssetM]) -> Aggregate {
    let sprites: Vec<&AssetM> = ms.iter().filter(|m| m.with_input || m.frames > 0).collect();
    let mut a = Aggregate { assets: ms.len(), with_input: ms.iter().filter(|m| m.with_input).count(), factor: 1, ..Default::default() };
    if sprites.is_empty() {
        return a;
    }
    a.outline_presence = median(sprites.iter().map(|m| m.outline_presence).collect());
    a.outline_thickness = median(sprites.iter().filter(|m| m.outline_presence > 0.3).map(|m| m.outline_thickness).collect());
    a.outline_colour = sprites.iter().filter(|m| m.outline_presence > 0.3).find_map(|m| m.outline_colour);
    a.tones = median(sprites.iter().flat_map(|m| m.families.iter().filter(|f| f.share >= 0.05).map(|f| f.tones as f64)).collect()).round() as usize;
    let mut dirs: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for m in &sprites {
        if m.light.0 != "even" {
            *dirs.entry(m.light.0.clone()).or_insert(0) += 1;
        }
    }
    a.light = dirs.into_iter().max_by_key(|e| e.1).map(|e| e.0).unwrap_or_else(|| "even".into());
    let mut hs: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for m in &sprites {
        *hs.entry(m.highlight.0.clone()).or_insert(0) += 1;
    }
    a.highlight = hs.into_iter().max_by_key(|e| e.1).map(|e| e.0).unwrap_or_else(|| "none".into());
    a.dither = median(sprites.iter().map(|m| m.dither).collect());
    a.major_colours = median(sprites.iter().map(|m| m.palette.1 as f64).collect()).round() as usize;
    a.factor = median(sprites.iter().map(|m| m.factor as f64).collect()).round().max(1.0) as usize;
    a.eyes_assets = sprites.iter().filter(|m| m.eyes.frames_with > 0).count();
    a.eye_max = sprites.iter().map(|m| m.eyes.max_per_frame).max().unwrap_or(0);
    let mut ws: Vec<(i32, i32)> = sprites.iter().filter(|m| m.eyes.frames_with > 0).map(|m| m.eyes.white_cells).collect();
    ws.sort();
    a.eye_white = ws.get(ws.len() / 2).copied().unwrap_or((0, 0));
    // the minimum is feasible for the source: the smallest eye any asset shows at the target scale, with 2x3 cells as the floor (the pxl-sprite skill's rule)
    let small: Vec<(i32, i32)> = sprites.iter().filter(|m| m.eyes.frames_with > 0).map(|m| m.eyes.white_min).collect();
    a.eye_min = (small.iter().map(|e| e.0).min().unwrap_or(0).max(EYE_FLOOR.0), small.iter().map(|e| e.1).min().unwrap_or(0).max(EYE_FLOOR.1));
    // shared material ramps: families of one name across assets; the ramp is the source colours of the asset that shows the material most
    // (never an average of several assets: a blend is a colour no source has), with distinct dark / mid / light
    let mut by: Vec<(String, (f64, Vec<Rgb>), Vec<String>)> = Vec::new();
    let mut add = |name: &str, share: f64, ramp: &Vec<Rgb>, asset: &str| match by.iter_mut().find(|e| e.0 == name) {
        Some(e) => {
            if share > (e.1).0 {
                e.1 = (share, ramp.clone());
            }
            if !e.2.contains(&asset.to_string()) {
                e.2.push(asset.to_string());
            }
        }
        None => by.push((name.to_string(), (share, ramp.clone()), vec![asset.to_string()])),
    };
    for m in &sprites {
        for f in m.families.iter().filter(|f| f.share >= 0.05 && !f.neutral) {
            add(&f.name, f.share, &f.ramp, &m.name);
        }
    }
    for m in &sprites {
        for f in m.families.iter().filter(|f| f.neutral && f.share >= 0.08) {
            add("grey", f.share, &f.ramp, &m.name);
        }
    }
    for (name, (_, ramp), assets) in by {
        a.materials.push((name, ramp, assets));
    }
    a
}

// ---------------------------------------------------------------------------------------------- commands

pub fn run_style(c: &StyleCmd, out: &mut Out) -> Result<()> {
    match c {
        StyleCmd::Analyze { assets } => run_analyze(assets, out),
        StyleCmd::Propose { force } => run_propose(*force, out),
        StyleCmd::Approve { by } => run_approve(by.as_deref(), out),
        StyleCmd::Mark { asset, frames, remove } => run_mark(asset, frames, *remove, out),
        StyleCmd::Identity { asset, op, values } => run_identity(asset, op.as_deref(), values, out),
        StyleCmd::Show => run_show(out),
    }
}

fn run_analyze(names: &[String], out: &mut Out) -> Result<()> {
    let home = project_home()?;
    let ms = measure_all(&home, names)?;
    if ms.is_empty() {
        return Err(PxlError::usage("no assets to analyze").with_hint("pxl ingest first"));
    }
    let ag = aggregate(&ms);
    let mut text = String::new();
    for m in &ms {
        text.push_str(&format!(
            "{:<22} {:<6} outline {:>3.0}%{} {:.1}px  tones {}  light {}  highlight {}  dither {:.1}%  colours {}/{}  eyes {}{}\n",
            m.name,
            m.mode,
            m.outline_presence * 100.0,
            m.outline_colour.map_or(String::new(), |c| format!(" {}", hex(c))),
            m.outline_thickness,
            m.families.iter().filter(|f| f.share >= 0.05).map(|f| format!("{}:{}", f.name, f.tones)).collect::<Vec<_>>().join(","),
            m.light.0,
            m.highlight.0,
            m.dither * 100.0,
            m.palette.1,
            m.palette.0,
            m.eyes.max_per_frame,
            if m.with_input { "" } else { "  (no input image: measured on the reduced frames)" }
        ));
        if !m.ref_missing.is_empty() {
            let shown: Vec<&str> = m.ref_missing.iter().take(4).map(|s| s.as_str()).collect();
            text.push_str(&format!(
                "  reference_missing: asset {} frame(s) {}{}: the source image is unavailable, so {}\n",
                m.name,
                shown.join(", "),
                if m.ref_missing.len() > 4 { format!(" (+{} more)", m.ref_missing.len() - 4) } else { String::new() },
                if m.with_input { "these frames are left out of the measurement" } else { "the measurement uses the reduced frames; eye_parity, identity_colour and flat cannot compare with a source" }
            ));
        }
    }
    text.push_str(&format!(
        "project: outline {:.0}% {:.1}px, {} tone(s) per material, light {}, highlights {}, dither {:.1}%, {} major colours, factor {}, eyes in {} asset(s)",
        ag.outline_presence * 100.0,
        ag.outline_thickness,
        ag.tones,
        ag.light,
        ag.highlight,
        ag.dither * 100.0,
        ag.major_colours,
        ag.factor,
        ag.eyes_assets
    ));
    let js = json!({"assets": ms.iter().map(|m| m.to_json()).collect::<Vec<_>>(), "aggregate": aggregate_json(&ag)});
    std::fs::create_dir_all(home.join(".pxl"))?;
    fsutil::atomic_write_plain(&analysis_path(&home), (serde_json::to_string_pretty(&js).unwrap() + "\n").as_bytes())?;
    out.emit(text, json!({"analysis": js, "file": analysis_path(&home).slash(), "checked": {"ok": ms.len(), "of": ms.len()}}));
    Ok(())
}

fn aggregate_json(a: &Aggregate) -> Value {
    json!({
        "assets": a.assets, "with_input": a.with_input,
        "outline": {"presence": r3(a.outline_presence), "thickness": r3(a.outline_thickness), "colour": a.outline_colour.map(hex)},
        "tones_per_material": a.tones, "light": a.light, "highlight": a.highlight, "dither": r3(a.dither), "major_colours": a.major_colours, "factor": a.factor,
        "eyes": {"assets": a.eyes_assets, "max_per_frame": a.eye_max, "white_cells": [a.eye_white.0, a.eye_white.1], "min_white": [a.eye_min.0, a.eye_min.1]},
        "materials": a.materials.iter().map(|(n, r, assets)| json!({"name": n, "ramp": r.iter().map(|c| hex(*c)).collect::<Vec<_>>(), "assets": assets})).collect::<Vec<_>>(),
    })
}

// ---------------------------------------------------------------------------------------------- propose

struct Limits {
    console: String,
    console_name: String,
    colors: u32,
    obj_palettes: u32,
    hw_cell: (u32, u32),
    per_line: u32,
    pixels_per_line: u32,
    fixed: bool,
    greys: bool,
    channel_step: u32,
    colour_bits: u32,
    palette_set_model: bool,
    attr: (u32, u32),
    colors_per_tile: u32,
    sprite_size_mode: String,
}

fn limits(home: &Path) -> Limits {
    let pc = console::project_console(home);
    let sp = profile::for_project(home, Some(&pc.id), "sprite").ok();
    let tp = profile::for_project(home, Some(&pc.id), "tile").ok();
    let info = console::info(&pc.id, Some(home)).ok();
    let (Some(sp), Some(tp)) = (sp, tp) else {
        return Limits { console: pc.id.clone(), console_name: info.map_or(pc.id.clone(), |i| i.name), colors: 3, obj_palettes: 0, hw_cell: (8, 8), per_line: 0, pixels_per_line: 0, fixed: false, greys: false, channel_step: 8, colour_bits: 0, palette_set_model: false, attr: (0, 0), colors_per_tile: 0, sprite_size_mode: "fixed".into() };
    };
    let greys = !sp.fixed_colours.is_empty() && sp.fixed_colours.iter().all(|c| insight::chroma(*c) < 0.03);
    Limits {
        console: pc.id.clone(),
        console_name: info.map_or(pc.id.clone(), |i| i.name),
        colors: sp.colors,
        obj_palettes: sp.obj_palettes,
        hw_cell: sp.hw_cell,
        per_line: sp.hw_cells_per_row_max,
        pixels_per_line: sp.hw_pixels_per_line_max,
        fixed: !sp.fixed_colours.is_empty(),
        greys,
        channel_step: sp.channel_step,
        colour_bits: sp.colour_bits,
        palette_set_model: sp.colors == 3 && sp.obj_palettes >= 4 && sp.hw_cell == (8, 16),
        attr: tp.attr_block,
        colors_per_tile: tp.colors_per_tile,
        sprite_size_mode: sp.sprite_size_mode.clone(),
    }
}

fn pct(v: f64) -> String {
    format!("{:.0}%", v * 100.0)
}

/// Each limit with its impact on this input style, one line each.
fn impact_lines(l: &Limits, ag: &Aggregate, ms: &[AssetM]) -> Vec<String> {
    let mut v = Vec::new();
    let tones = ag.tones.max(1);
    let main_materials = ms.iter().flat_map(|m| m.families.iter().filter(|f| f.share >= 0.05)).count() as f64 / ms.len().max(1) as f64;
    if l.greys {
        v.push(format!(
            "**shades: {} fixed greys** -> white is a main tone, not a highlight: light materials take white, shadows take the darkest grey; the input's {} tone(s) per material collapse to the shade steps ({} major colours become {} shades).",
            l.colors + 1,
            tones,
            ag.major_colours,
            l.colors + 1
        ));
    } else if l.fixed {
        v.push(format!("**colours: a fixed master palette** -> every colour snaps to the nearest entry (chroma-weighted), so identity colours are chosen from the palette first and ramps are built around them; the input's {} major colours fold to at most {} per palette line.", ag.major_colours, l.colors));
    } else {
        let step = if l.colour_bits > 0 { format!("{} bits per channel", l.colour_bits) } else { format!("channel step {}", l.channel_step) };
        v.push(format!("**colour model: {}** -> small colour differences in the input collapse; two input tones closer than one step become one colour.", step));
    }
    if l.colors <= 3 {
        v.push(format!(
            "**{} colours per palette line (plus transparent)** -> a material gets an outline, a mid and a light tone; the highlight is the light tone, not a fourth colour. The input has {} tone(s) per material over {:.1} materials per asset, so {}.",
            l.colors,
            tones,
            main_materials,
            if tones > l.colors as usize { format!("{} tone(s) per material must fold, least visible first", tones - l.colors as usize) } else { "no tone folds".to_string() }
        ));
    } else if l.colors <= 7 {
        v.push(format!("**{} colours per palette line** -> one 3-tone ramp plus an outline and a highlight per palette line; materials beyond the first one or two share a line only when their ramps overlap.", l.colors));
    } else {
        v.push(format!("**{} colours per palette line** -> room for 2 to 3 material ramps per line; the input's {} major colours per asset {} the limit.", l.colors, ag.major_colours, if (ag.major_colours as u32) > l.colors { "exceed" } else { "fit" }));
    }
    if l.obj_palettes > 0 {
        v.push(format!("**{} sprite palette line(s)** -> at most {} colour group(s) on screen at once; related assets must share lines, so shared material ramps decide the plan (`pxl palette plan`).", l.obj_palettes, l.obj_palettes));
    }
    if l.palette_set_model {
        v.push(format!("**one palette per {}x{} cell** -> colour changes must fall on cell boundaries: put shoes, gloves or a face in their own cell band instead of mixing two material ramps inside one cell.", l.hw_cell.0, l.hw_cell.1));
    }
    if ag.factor > 1 {
        let (w, h) = ag.eye_min;
        v.push(format!("**factor {}** -> features under {} source pixels vanish unless redrawn: eyes need at least {}x{} white cells plus a 1x1 pupil, outlines are 1 cell, and a highlight is 1 cell at most.", ag.factor, ag.factor, w, h));
    } else {
        v.push("**factor 1 (the input is at target size)** -> every input pixel is kept; the work is folding colours, not shrinking shapes.".to_string());
    }
    if l.per_line > 0 {
        v.push(format!(
            "**{} sprite cell(s) per scanline{}** -> bodies wider than {} px of cells must be split or shortened; check the widest frame of an animation, not the first.",
            l.per_line,
            if l.pixels_per_line > 0 { format!(" and {} sprite pixels", l.pixels_per_line) } else { String::new() },
            l.per_line * l.hw_cell.0
        ));
    }
    if l.sprite_size_mode != "fixed" && !l.sprite_size_mode.is_empty() {
        v.push(format!("**sprite sizes: {}** -> a figure is covered by the allowed sizes; padding is free of colour but costs a scanline slot.", l.sprite_size_mode));
    }
    if l.attr.0 > 0 {
        v.push(format!("**background attributes per {}x{} px** -> a tile region keeps its colour sets per {}x{} block; level art is drawn on that grid.", l.attr.0, l.attr.1, l.attr.0, l.attr.1));
    } else if l.colors_per_tile > 0 {
        v.push(format!("**{} colours per background tile** -> tile art folds to {} colours in every 8x8 tile; gradients become two-step bands.", l.colors_per_tile, l.colors_per_tile));
    }
    v
}

const IDENTITY_INTRO: &str = "The colours and features that must survive, per asset, ranked by area, saturation and distinctness of their material (inferred from the inputs; the lists are in brief.toml and the gates read them). Correct a list with `pxl style identity ASSET add|remove|set #rrggbb ...` or `pxl decide \"why\" --set identity.ASSET=#rrggbb,#rrggbb`.\n";

fn identity_lines(entries: &[(String, usize, Vec<String>)]) -> String {
    let mut md = String::new();
    for (name, eyes, cols) in entries {
        let named: Vec<String> = cols.iter().map(|c| insight::parse_hex(c).map_or(c.clone(), |r| format!("{} ({})", c, insight::colour_name(r)))).collect();
        md.push_str(&format!("- `{}`: {}{}\n", name, if *eyes > 0 { format!("{} eye(s) with white and pupil", eyes) } else { "no eyes".to_string() }, if named.is_empty() { String::new() } else { format!("; colours {}", named.join(", ")) }));
    }
    md
}

/// Rewrite the `## Identity list` section of style.md from brief.toml after an identity edit (a style.md that is not there is left alone).
pub fn refresh_identity_section(home: &Path) {
    let Ok(text) = std::fs::read_to_string(md_path(home)) else { return };
    let Some(start) = text.find("## Identity list") else { return };
    let after = &text[start + 1..];
    let end = after.find("\n## ").map(|i| start + 1 + i + 1).unwrap_or(text.len());
    let brief = crate::intake::read_brief(home);
    let order: Vec<String> = asset::scan(home).into_iter().map(|a| a.name).collect();
    let entries: Vec<(String, usize, Vec<String>)> = order.iter().filter_map(|n| crate::intake::identity_of(&brief, n).map(|(e, c)| (n.clone(), e, c))).collect();
    let mut new = String::from("## Identity list\n\n");
    new.push_str(IDENTITY_INTRO);
    new.push('\n');
    new.push_str(&identity_lines(&entries));
    new.push('\n');
    let out = format!("{}{}{}", &text[..start], new, &text[end..]);
    let _ = fsutil::atomic_write_plain(&md_path(home), out.as_bytes());
}

fn run_identity(asset_name: &str, op: Option<&str>, values: &[String], out: &mut Out) -> Result<()> {
    let home = project_home()?;
    let op = op.unwrap_or("show");
    let by = out.agent().unwrap_or("agent").to_string();
    let cols = crate::intake::identity_edit(&home, asset_name, op, values, &by)?;
    let (eyes, _, confirmed) = crate::intake::identity_entry(&crate::intake::read_brief(&home), asset_name).unwrap_or((0, Vec::new(), false));
    let named: Vec<String> = cols.iter().map(|c| insight::parse_hex(c).map_or(c.clone(), |r| format!("{} ({})", c, insight::colour_name(r)))).collect();
    out.emit(
        format!("{}: {} eye(s); colours {}{}{}", asset_name, eyes, if named.is_empty() { "(none)".to_string() } else { named.join(", ") }, if confirmed { " [confirmed]" } else { " [inferred]" }, if op == "show" { String::new() } else { "; the identity_colour gate and style.md use this list".to_string() }),
        json!({"asset": asset_name, "eyes": eyes, "colours": cols, "confirmed": confirmed}),
    );
    Ok(())
}

fn tradeoff_text(home: &Path) -> String {
    let b = crate::intake::read_brief(home);
    let order = crate::intake::list_of(&b, "tradeoff");
    let ex = crate::intake::value_of(&b, "exceptions");
    let order = if order.is_empty() { crate::intake::default_tradeoff() } else { order };
    format!("when the hardware does not fit, apply in this order: {}. {}", order.join(", then "), if ex == "ask" { "Exceptions to the client may be asked for." } else { "Exceptions are never asked for: fix within the limits." })
}

fn run_propose(force: bool, out: &mut Out) -> Result<()> {
    let home = project_home()?;
    let doc = doc_status(&home);
    if doc.approved && !force {
        return Err(PxlError::validation("style.md is approved; proposing again would replace the approved design language").with_hint("pass --force to write a new proposal (the approval is cleared)"));
    }
    let ms = measure_all(&home, &[])?;
    if ms.is_empty() {
        return Err(PxlError::usage("no assets to base a style on").with_hint("pxl ingest first, then pxl style propose"));
    }
    let ag = aggregate(&ms);
    let js = json!({"assets": ms.iter().map(|m| m.to_json()).collect::<Vec<_>>(), "aggregate": aggregate_json(&ag)});
    std::fs::create_dir_all(home.join(".pxl"))?;
    fsutil::atomic_write_plain(&analysis_path(&home), (serde_json::to_string_pretty(&js).unwrap() + "\n").as_bytes())?;
    let l = limits(&home);
    let cs = console::style(&l.console, Some(&home));
    let project = crate::project::project_table(&home.join("pxl.toml")).get("name").and_then(|n| n.as_str()).unwrap_or("project").to_string();
    let f = ag.factor;
    let mut md = String::new();
    md.push_str(&format!("# Design language: {}\n\nStatus: proposed\n\nConsole: {} ({}). {} asset(s) measured, {} with input images. Written by `pxl style propose`; approve with `pxl style approve`.\n\n", project, l.console, l.console_name, ms.len(), ag.with_input));
    md.push_str("## The input style (measured)\n\n");
    md.push_str(&format!(
        "- Outline: {} of the silhouette is dark{}, {:.1} px thick (output pixels).\n",
        pct(ag.outline_presence),
        ag.outline_colour.map_or(String::new(), |c| format!(" ({}, {})", hex(c), insight::colour_name(c))),
        ag.outline_thickness
    ));
    md.push_str(&format!("- Shading: {} tone(s) per material; materials: {}.\n", ag.tones, if ag.materials.is_empty() { "none distinct".to_string() } else { ag.materials.iter().map(|m| m.0.clone()).collect::<Vec<_>>().join(", ") }));
    md.push_str(&format!("- Light: from {}.\n- Highlights: {}.\n- Dithering: {} ({}).\n- Palette: {} major colours per asset.\n", ag.light, ag.highlight, if ag.dither >= 0.03 { "present" } else { "none" }, pct(ag.dither), ag.major_colours));
    if ag.eyes_assets > 0 {
        md.push_str(&format!("- Eyes: white with a pupil, up to {} per frame, in {} asset(s); the white is about {}x{} cells.\n", ag.eye_max, ag.eyes_assets, ag.eye_white.0, ag.eye_white.1));
    } else {
        md.push_str("- Eyes: none detected (no white eye with a dark pupil in any input).\n");
    }
    md.push_str("\n## The limits and what they do to this style\n\n");
    for line in impact_lines(&l, &ag, &ms) {
        md.push_str(&format!("- {}\n", line));
    }
    md.push_str("\n## Rules\n\nEvery producer and reviewer follows these. A rule id is quoted in review notes (`pxl review note ... --rule eyes`) and in lint messages.\n\n");
    // outline
    let outline_rule = if ag.outline_presence >= 0.6 {
        format!(
            "keep the outline: {}, {} px, closed around the silhouette. {}",
            ag.outline_colour.map_or("the darkest tone".to_string(), |c| format!("{} ({})", hex(c), insight::colour_name(c))),
            (ag.outline_thickness.round() as i64).max(1),
            match cs.outline.as_str() {
                "darkest" => "On this console the outline is the darkest tone of the palette.",
                "ramp" => "On this console the outline is a dark tone of the material itself.",
                _ => "The console default is no outline, but the input has one: it stays.",
            }
        )
    } else if ag.outline_presence < 0.3 {
        format!("the input has no outline (only {} of the silhouette is dark): do not add one; separate shapes by value contrast.{}", pct(ag.outline_presence), if cs.outline != "none" { format!(" The console default is `{}`; this project overrides it to `none`.", cs.outline) } else { String::new() })
    } else {
        format!("the input outlines only part of the silhouette ({}): draw an outline where the source has one, nowhere else.", pct(ag.outline_presence))
    };
    md.push_str(&format!("- **outline**: {}\n", outline_rule));
    // eyes
    let (ew, eh) = ag.eye_min;
    if ag.eyes_assets > 0 {
        md.push_str(&format!(
            "- **eyes**: every face keeps its eyes, white plus a pupil, as many per frame as the source shows (lint `eye_parity`). Minimum size at factor {}: white {}x{} cells and a 1x1 pupil inside or at the edge of the white; never merge an eye into the outline; never draw one eye where the source shows two.\n",
            f, ew, eh
        ));
    } else {
        md.push_str("- **eyes**: no eyes were detected in the inputs. If an asset has a face, state its construction here before drawing it, with a minimum size of a 2x2 white and a 1x1 pupil.\n");
    }
    md.push_str("- **identity**: every colour on an asset's identity list (below) is present in every frame where the source has it (lint `identity_colour`); a colour is never removed to make a palette fit, it is folded last.\n");
    let tones = (l.colors.max(2) as usize).min(ag.tones.max(2)).min(4);
    md.push_str(&format!("- **ramps**: each material has one ramp of {} tone(s), dark to light, shared by every asset that shows the material (lint `consistency`); the highlight is the light tone, not an extra colour. The ramps are in the table below.\n", tones));
    md.push_str(&format!("- **light**: light comes from {}; shadows fall on the opposite side, the same on every asset (lint `flat` checks the value structure).\n", if ag.light == "even" { "the top-left (the input shows no clear direction, so the usual one is assumed)".to_string() } else { ag.light.clone() }));
    // white and highlight
    let white = match cs.white.as_str() {
        "active" => format!("white is a main tone on this console ({} shades only): light materials and large light areas use white, not only highlights and eyes (lint `flat` and the first-version contrast stretch keep the spread at {} or more).", l.colors + 1, cs.min_value_spread),
        "balanced" => "white is used for highlights, eyes and the lightest material, in that order of priority.".to_string(),
        _ => "white is only for highlights and eye whites; light materials use their own lightest tone.".to_string(),
    };
    md.push_str(&format!("- **white**: {}\n", white));
    md.push_str(&format!(
        "- **highlight**: {}.\n",
        match ag.highlight.as_str() {
            "none" => "the input has almost no highlights: do not add shine; keep the light tone of the ramp".to_string(),
            "spots" => "single-cell sparkles on convex shapes, the light tone of the ramp (or white where the console style says white is active)".to_string(),
            _ => "light areas follow the light direction as a band of the light tone; at most one band per material".to_string(),
        }
    ));
    md.push_str(&format!(
        "- **dither**: {}.\n",
        if ag.dither >= 0.03 && l.colors > 3 { "the input dithers; keep it only as an ordered 2x2 pattern on large areas where the palette has no mid tone" } else { "no dithering: a checkerboard of two tones reads as noise at 1x and costs tiles; use a band instead" }
    ));
    md.push_str(&format!("- **fold**: {}\n", tradeoff_text(&home)));
    md.push_str("- **readability**: at 1x the silhouette and the eyes must read against the console's preview background, and a figure keeps a value spread of at least the console minimum (lint `readability`).\n");
    // identity list
    md.push_str("\n## Identity list\n\n");
    md.push_str(IDENTITY_INTRO);
    md.push('\n');
    let brief = crate::intake::read_brief(&home);
    let entries: Vec<(String, usize, Vec<String>)> = ms
        .iter()
        .map(|m| match crate::intake::identity_of(&brief, &m.name) {
            Some((e, c)) => (m.name.clone(), e, c),
            None => (m.name.clone(), m.eyes.max_per_frame, m.identity.iter().map(|(c, _)| hex(*c)).collect()),
        })
        .collect();
    md.push_str(&identity_lines(&entries));
    md.push_str("\n## Shared material ramps\n\nBuilt from the source colours of each material (a `-` is a tone the material does not have: a two-tone material has no mid).\n\n| material | dark | mid | light | assets |\n|---|---|---|---|---|\n");
    for (name, r, assets) in &ag.materials {
        let (d, m, l) = match r.len() {
            0 => ("-".to_string(), "-".to_string(), "-".to_string()),
            1 => ("-".to_string(), hex(r[0]), "-".to_string()),
            2 => (hex(r[0]), "-".to_string(), hex(r[1])),
            _ => (hex(r[0]), hex(r[1]), hex(r[2])),
        };
        md.push_str(&format!("| {} | {} | {} | {} | {} |\n", name, d, m, l, assets.join(", ")));
    }
    md.push_str(&format!("\nConsole style defaults (`[style]`): white {}, outline {}, value_map {}, min_value_spread {}{}.\n", cs.white, cs.outline, cs.value_map, cs.min_value_spread, if cs.reason.is_empty() { String::new() } else { format!(" ({})", cs.reason) }));
    fsutil::atomic_write_plain(&md_path(&home), md.as_bytes())?;
    // state: clear the approval, keep the frames, store the ramps
    let mut st = read_state(&home);
    st.remove("approval");
    let ramps: Vec<toml::Value> = ag
        .materials
        .iter()
        .map(|(n, r, assets)| {
            let mut t = toml::Table::new();
            t.insert("material".into(), n.clone().into());
            t.insert("colours".into(), toml::Value::Array(r.iter().map(|c| hex(*c).into()).collect()));
            t.insert("assets".into(), toml::Value::Array(assets.iter().map(|a| a.clone().into()).collect()));
            toml::Value::Table(t)
        })
        .collect();
    st.insert("ramp".into(), toml::Value::Array(ramps));
    if ag.eyes_assets > 0 {
        let mut e = toml::Table::new();
        e.insert("min_white".into(), toml::Value::Array(vec![(ag.eye_min.0 as i64).into(), (ag.eye_min.1 as i64).into()]));
        st.insert("eyes".into(), toml::Value::Table(e));
    } else {
        st.remove("eyes");
    }
    write_state(&home, &st)?;
    let rule_ids: Vec<String> = rules(&home).into_iter().map(|r| r.0).collect();
    let marked = style_frames(&home);
    out.emit(
        format!(
            "wrote {} ({} rules: {})\n  next: read it, mark the style frames (`pxl style mark ASSET`), draw them to final quality, then `pxl style approve`{}",
            md_path(&home).slash(),
            rule_ids.len(),
            rule_ids.join(", "),
            if marked.is_empty() { String::new() } else { format!("\n  style frames so far: {}", marked.iter().map(|f| f.0.clone()).collect::<Vec<_>>().join(", ")) }
        ),
        json!({"file": md_path(&home).slash(), "rules": rule_ids, "analysis": aggregate_json(&ag), "limits": impact_lines(&l, &ag, &ms)}),
    );
    Ok(())
}

// ---------------------------------------------------------------------------------------------- approve, mark, show

fn run_approve(by: Option<&str>, out: &mut Out) -> Result<()> {
    let home = project_home()?;
    let text = std::fs::read_to_string(md_path(&home)).map_err(|_| PxlError::usage("there is no style.md yet").with_hint("pxl style propose"))?;
    // an agent never signs as the user: `--by user` is the user's own word
    let who = by.map(String::from).unwrap_or_else(|| "agent".into());
    let on = tasks::date_of(tasks::now());
    let hash = body_hash(&text);
    let new_text: String = text.lines().map(|l| if l.starts_with("Status:") { format!("Status: approved by {} on {}", who, on) } else { l.to_string() }).collect::<Vec<_>>().join("\n") + "\n";
    fsutil::atomic_write_plain(&md_path(&home), new_text.as_bytes())?;
    let mut st = read_state(&home);
    let mut ap = toml::Table::new();
    ap.insert("by".into(), who.clone().into());
    ap.insert("on".into(), on.clone().into());
    ap.insert("hash".into(), hash.into());
    st.insert("approval".into(), toml::Value::Table(ap));
    write_state(&home, &st)?;
    // project [style] overrides that differ from the console default (a flat input without outline: outline = "none")
    let mut notes = Vec::new();
    if let Ok(a) = std::fs::read_to_string(analysis_path(&home)).map(|t| serde_json::from_str::<Value>(&t).unwrap_or(Value::Null)) {
        let pres = a["aggregate"]["outline"]["presence"].as_f64().unwrap_or(1.0);
        let pc = console::project_console(&home);
        let cs = console::style(&pc.id, Some(&home));
        if pres < 0.3 && cs.outline != "none" {
            let mp = home.join("pxl.toml");
            crate::project::manifest_lease(&mp, out)?;
            if let Ok(t) = std::fs::read_to_string(&mp) {
                let t2 = if t.lines().any(|l| l.trim() == "[style]") { t.clone() } else { format!("{}\n[style]\n", t.trim_end()) };
                if let Some(t3) = crate::manedit::set_table_key(&t2, "style", "outline", "\"none\"") {
                    fsutil::atomic_write(&mp, t3.as_bytes())?;
                    notes.push("[style] outline = \"none\" written to pxl.toml (the input has no outline)".to_string());
                }
            }
        }
    }
    tasks::append_decision_line(&home, &format!("- {} [{}] design language approved (style.md {})", on, who, rules(&home).iter().map(|r| r.0.clone()).collect::<Vec<_>>().join(", ")))?;
    let ms = crate::milestone::status(&home);
    out.emit(
        format!("style.md approved by {} on {}{}\n{}", who, on, notes.iter().map(|n| format!("\n  {}", n)).collect::<String>(), ms.text()),
        json!({"approved_by": who, "on": on, "notes": notes, "milestones": ms.to_json()}),
    );
    Ok(())
}

fn run_mark(asset_arg: &str, frames: &[String], remove: bool, out: &mut Out) -> Result<()> {
    let home = project_home()?;
    let f = fsutil::resolve_part(asset_arg)?;
    let name = asset::asset_name(&asset::dir_of(&f));
    let part = pxl::load(&f)?;
    for fr in frames {
        if part.frame(fr).is_none() {
            return Err(crate::cli::unknown_frame(&part, fr));
        }
    }
    let mut st = read_state(&home);
    let mut fm = sub(&st, "frames").cloned().unwrap_or_default();
    if remove {
        fm.remove(&name);
    } else {
        fm.insert(name.clone(), toml::Value::Array(frames.iter().map(|s| s.clone().into()).collect()));
    }
    st.insert("frames".into(), toml::Value::Table(fm));
    write_state(&home, &st)?;
    tasks::write_page(&asset::dir_of(&f));
    let all = style_frames(&home);
    out.emit(
        format!("{} {} style frame{}; style frames: {}", name, if remove { "is no longer a" } else { "marked as a" }, if frames.is_empty() { String::new() } else { format!(" ({})", frames.join(",")) }, all.iter().map(|x| x.0.clone()).collect::<Vec<_>>().join(", ")),
        json!({"asset": name, "marked": !remove, "frames": frames, "style_frames": all.iter().map(|x| json!({"asset": x.0, "frames": x.1})).collect::<Vec<_>>()}),
    );
    Ok(())
}

fn run_show(out: &mut Out) -> Result<()> {
    let home = project_home()?;
    let d = doc_status(&home);
    let fr = style_frames(&home);
    let rs = rules(&home);
    let mut text = format!("style.md: {}\n", if !d.exists { "none (pxl style propose)".to_string() } else if d.approved { format!("approved by {} on {}", d.by, d.on) } else if d.changed { "changed since it was approved".to_string() } else { "proposed".to_string() });
    text.push_str(&format!("style frames: {}\n", if fr.is_empty() { "none (pxl style mark ASSET)".to_string() } else { fr.iter().map(|f| f.0.clone()).collect::<Vec<_>>().join(", ") }));
    for (id, t) in &rs {
        text.push_str(&format!("  {}: {}\n", id, t));
    }
    out.emit(text.trim_end(), json!({"exists": d.exists, "approved": d.approved, "changed": d.changed, "by": d.by, "style_frames": fr.iter().map(|x| json!({"asset": x.0, "frames": x.1})).collect::<Vec<_>>(), "rules": rs.iter().map(|r| json!({"id": r.0, "text": r.1})).collect::<Vec<_>>()}));
    Ok(())
}

// ---------------------------------------------------------------------------------------------- style frames next to assets

pub fn rel_path(from_dir: &Path, to: &Path) -> String {
    let (fa, fb) = (std::fs::canonicalize(from_dir).unwrap_or_else(|_| from_dir.to_path_buf()), std::fs::canonicalize(to).unwrap_or_else(|_| to.to_path_buf()));
    let a: Vec<_> = fa.components().collect();
    let b: Vec<_> = fb.components().collect();
    let mut i = 0;
    while i < a.len() && i < b.len() && a[i] == b[i] {
        i += 1;
    }
    let mut parts: Vec<String> = std::iter::repeat("..".to_string()).take(a.len() - i).collect();
    parts.extend(b[i..].iter().map(|c| c.as_os_str().to_string_lossy().to_string()));
    parts.join("/")
}

/// The style frame nearest to an asset: same group first, then same mode, then the first one. `(asset name, compare.png path)`.
pub fn nearest_style_frame(home: &Path, asset_name: &str) -> Option<(String, PathBuf)> {
    let frames = style_frames(home);
    // a style frame is the reference itself: it is never pointed at another asset
    if frames.iter().any(|f| f.0 == asset_name) {
        return None;
    }
    let all = asset::scan(home);
    let me = all.iter().find(|a| a.name == asset_name)?;
    let my_mode = pxl::load(&me.file).map(|p| p.kind).unwrap_or(PartKind::Sprite);
    let cands: Vec<&asset::AssetInfo> = frames.iter().filter(|f| f.0 != asset_name).filter_map(|f| all.iter().find(|a| a.name == f.0)).collect();
    let pick = cands
        .iter()
        .find(|a| a.group == me.group)
        .or_else(|| cands.iter().find(|a| pxl::load(&a.file).map(|p| p.kind == my_mode).unwrap_or(false)))
        .or_else(|| cands.first())?;
    Some((pick.name.clone(), pick.dir.join("compare.png")))
}

/// `(name, relative link)` for a page in `from_dir`.
pub fn nearest_style_link(home: &Path, from_dir: &Path, asset_name: &str) -> Option<(String, String)> {
    nearest_style_frame(home, asset_name).filter(|n| n.1.is_file()).map(|(n, p)| (n, rel_path(from_dir, &p)))
}

// ---------------------------------------------------------------------------------------------- checklist

/// The reviewer checklist of an asset, generated from style.md.
pub fn checklist(file: &Path, out: &mut Out) -> Result<()> {
    let home = tasks::home_of(file);
    let dir = asset::dir_of(file);
    let name = asset::asset_name(&dir);
    let rs = rules(&home);
    if rs.is_empty() {
        return Err(PxlError::usage("there is no style.md with rules yet").with_hint("pxl style propose, then pxl style approve"));
    }
    let d = doc_status(&home);
    let brief = crate::intake::read_brief(&home);
    let mut text = format!("REVIEW CHECKLIST {} (style.md {})\n", name, if d.approved { format!("approved by {}", d.by) } else { "NOT approved yet".to_string() });
    let mut items = Vec::new();
    if let Some((eyes, cols)) = crate::intake::identity_of(&brief, &name) {
        text.push_str(&format!("identity list: {}{}\n", if eyes > 0 { format!("{} eye(s) with white and pupil", eyes) } else { "no eyes".to_string() }, if cols.is_empty() { String::new() } else { format!("; colours {}", cols.join(", ")) }));
    }
    for (id, t) in &rs {
        text.push_str(&format!("[ ] {}: {}\n", id, t));
        items.push(json!({"rule": id, "text": t}));
    }
    // the current design gate findings
    let all_findings = crate::design::gate_findings(file);
    let findings: Vec<_> = all_findings.iter().filter(|f| f.rule != crate::design::REFERENCE_MISSING).collect();
    if all_findings.is_empty() {
        text.push_str("design gates: no warnings\n");
    } else {
        text.push_str(&format!("design gates: {} warning(s) block `review approve` unless --justify\n", findings.len()));
        for f in &all_findings {
            text.push_str(&format!("  {} {}{}\n", f.rule, f.frame.as_ref().map_or(String::new(), |fr| format!("[{}] ", fr)), f.message));
        }
    }
    if let Some((n, p)) = nearest_style_frame(&home, &name) {
        text.push_str(&format!("nearest style frame: {} ({})\n", n, p.slash()));
    }
    text.push_str(&format!("compare: {}\ncite a rule in a note: pxl review note {} FRAME \"text\" --region x,y,w,h --rule ID --agent NAME", dir.join("compare.png").slash(), name));
    out.emit(
        text,
        json!({"asset": name, "approved": d.approved, "rules": items, "findings": findings.iter().map(|f| json!({"rule": f.rule, "frame": f.frame, "message": f.message})).collect::<Vec<_>>(),
               "nearest_style_frame": nearest_style_frame(&home, &name).map(|n| n.0)}),
    );
    Ok(())
}

#[allow(dead_code)]
fn unused() {
    let _ = (versions::latest, status::read);
}
