//! Source QA at ingest and load (M6 R6b): wrong source art was drawn from (a door that rendered noise, frames of
//! another zone's mapping, unused frames). Three checks run on an asset's source images and its `input/source.json`:
//!
//! * **noise**: a garbage reference (graphics decoded from the wrong offset, compressed data shown as tiles) is nearly random: most
//!   pixels differ from most of their neighbours, over many colours. The score is the share of 4-neighbour pairs of opaque pixels
//!   with different colours; art stays well under 0.6, random data is near 0.75. It flags above 0.65 with more than 8 colours.
//! * **borrowed art**: `source.json` `zone` is the zone the asset is for, `art_zone` the zone its mapping really draws from; they differ.
//! * **unused**: `source.json` has `"unused": true` (the asset) or `"labels": [{"name": .., "unused": true}]` (frames).
//!
//! `source.json` is part of the inbox contract of the `pxl-extract` skill (one per object, copied to `input/` by `ingest`).
//! A finding becomes a `question` task for the asset (once), and the asset is set to `blocked`, so `pxl next` does not hand it out
//! as a draw job until someone answers.

use crate::cli::QaArgs;
use crate::img::RgbaImage;
use crate::output::{Out, PxlError, Result};
use crate::{asset, pxl, status, tasks};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Finding {
    pub asset: String,
    pub kind: &'static str,
    pub message: String,
}

/// (noise score, distinct colours) of an image.
pub fn noise_score(im: &RgbaImage) -> (f64, usize) {
    let (w, h) = (im.w, im.h);
    let px = |x: usize, y: usize| im.px[y * w + x];
    let (mut diff, mut pairs) = (0usize, 0usize);
    let mut colours = BTreeSet::new();
    for y in 0..h {
        for x in 0..w {
            let p = px(x, y);
            if p.a == 0 {
                continue;
            }
            colours.insert((p.r, p.g, p.b));
            for (nx, ny) in [(x + 1, y), (x, y + 1)] {
                if nx < w && ny < h {
                    let q = px(nx, ny);
                    if q.a > 0 {
                        pairs += 1;
                        diff += usize::from((p.r, p.g, p.b) != (q.r, q.g, q.b));
                    }
                }
            }
        }
    }
    (if pairs == 0 { 0.0 } else { diff as f64 / pairs as f64 }, colours.len())
}

pub fn source_json(dir: &Path) -> Option<Value> {
    for p in [dir.join("input/source.json"), dir.join("source.json")] {
        if let Some(v) = std::fs::read_to_string(p).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
            return Some(v);
        }
    }
    None
}

pub fn check_asset(path: &Path) -> Result<Vec<Finding>> {
    let dir = asset::dir_of(path);
    let name = asset::asset_name(&dir);
    let part = pxl::load(path)?;
    let mut v = Vec::new();
    let mut noisy: Vec<String> = Vec::new();
    for f in &part.frames {
        let Some(src) = &f.src else { continue };
        let p = dir.join(src);
        if let Ok(im) = crate::png::read_rgba(&p) {
            let (n, c) = noise_score(&im);
            if n > 0.65 && c > 8 && im.w * im.h >= 64 {
                noisy.push(format!("{} ({:.2}, {} colours)", f.name, n, c));
            }
        }
    }
    if !noisy.is_empty() {
        v.push(Finding { asset: name.clone(), kind: "noise", message: format!("the source of {} looks like garbage, not art (nearly every pixel differs from its neighbours): {}; is the extraction offset or palette right? Ask for a new reference before drawing", name, noisy.join(", ")) });
    }
    if let Some(sj) = source_json(&dir) {
        let (z, az) = (sj["zone"].as_str(), sj["art_zone"].as_str());
        if let (Some(z), Some(az)) = (z, az) {
            if z != az {
                v.push(Finding { asset: name.clone(), kind: "borrowed_art", message: format!("the mapping of {} for zone {} uses the art of zone {}: its slots mean other colours there. Confirm which zone's art to draw from", name, z, az) });
            }
        }
        let mut unused: Vec<String> = Vec::new();
        if sj["unused"].as_bool() == Some(true) {
            unused.push("the whole object".into());
        }
        for l in sj["labels"].as_array().into_iter().flatten() {
            if l["unused"].as_bool() == Some(true) {
                unused.push(l["name"].as_str().unwrap_or("?").to_string());
            }
        }
        if !unused.is_empty() {
            v.push(Finding { asset: name.clone(), kind: "unused", message: format!("the source marks {} of {} as unused: drawing it may be wasted work. Confirm it is needed", unused.join(", "), name) });
        }
    }
    Ok(v)
}

/// Create the `question` tasks (once per finding) and block the asset.
pub fn apply(path: &Path, findings: &[Finding]) -> Result<usize> {
    if findings.is_empty() {
        return Ok(0);
    }
    let home = tasks::home_of(path);
    let existing = tasks::load_all(&home);
    let mut made = 0;
    for f in findings {
        let text = format!("source QA ({}): {}", f.kind, f.message);
        if existing.iter().any(|t| t.asset == f.asset && t.kind == "question" && t.text == text && t.status == "open") {
            continue;
        }
        tasks::add_task(&home, &f.asset, &text, "question", None, 3, "pxl")?;
        made += 1;
    }
    status::update(path, |s| {
        if s.state == "todo" {
            s.state = "blocked".into();
        }
    })?;
    Ok(made)
}

/// Called by `ingest` for a new asset: findings are reported and recorded.
pub fn on_ingest(path: &Path, out: &mut Out) {
    match check_asset(path) {
        Ok(f) if !f.is_empty() => {
            let _ = apply(path, &f);
            for x in &f {
                out.warn(format!("source QA {}: {}", x.kind, x.message));
            }
            out.set("source_qa", json!(f.iter().map(|x| json!({"asset": x.asset, "kind": x.kind, "message": x.message})).collect::<Vec<_>>()));
        }
        _ => {}
    }
}

pub fn run_qa(a: &QaArgs, out: &mut Out) -> Result<()> {
    let paths: Vec<PathBuf> = match &a.asset {
        Some(x) => vec![crate::fsutil::resolve_part(x)?],
        None => {
            let m = crate::manifest::Manifest::discover(a.manifest.as_deref())?.ok_or_else(|| PxlError::usage("`qa` without an asset needs a project (pxl.toml)"))?;
            m.parts.iter().map(|d| d.path.clone()).collect()
        }
    };
    let (mut all, mut made) = (Vec::new(), 0);
    for p in &paths {
        let f = check_asset(p)?;
        if !a.dry_run {
            made += apply(p, &f)?;
        }
        all.extend(f);
    }
    for f in &all {
        out.line(format!("{} [{}]: {}", f.asset, f.kind, f.message));
    }
    let tail = if a.dry_run { String::new() } else { format!(", {} new question task(s)", made) };
    out.line(format!("source QA: {} asset(s) checked, {} finding(s){}", paths.len(), all.len(), tail));
    out.set("qa", json!({"checked": paths.len(), "findings": all.iter().map(|x| json!({"asset": x.asset, "kind": x.kind, "message": x.message})).collect::<Vec<_>>(), "tasks_created": made}));
    Ok(())
}
