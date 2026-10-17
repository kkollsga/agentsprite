//! `export`, `build` and `check` (DESIGN 4.4, 5.4, 6.4). Owner: export worker.
//!
//! # What `export` writes
//!
//! Per `.pxl` frame one indexed PNG per template layer (layer 0 = base, further layers = overlays):
//!
//! * The PLTE of a layer image is `[transparent, <the layer's keys in template order>]`; index 0 is transparent
//!   through tRNS (RGB 0,0,0), all other entries are opaque. Colours are the template colours (a part's local
//!   `[colors]` overrides win, exactly as lint sees them), snapped to the profile's `channel_step`.
//! * The base image holds the base keys; under every overlay pixel it holds the overlay key's `under` base key, so
//!   the base alone still reads correctly. An overlay image holds the overlay keys and is transparent elsewhere.
//! * File names come from the manifest patterns (`{frame}.png`, `{frame}_{layer}.png` by default).
//!
//! Plus one `.aseprite`: indexed, one palette `[transparent, layer 0 keys, layer 1 keys, ...]`, one layer per template
//! layer (same names, same order), one frame per `.pxl` frame (100 ms, or the duration of the MD animation script
//! that names the frame), one tag per frame name, then one tag per MD animation script (only when the part has an
//! `[input] animation`; frames are matched by name suffix or `src`). A script that plays an ascending run of
//! consecutive frames is tagged over those base frames; one that interleaves, repeats or reorders frames gets its
//! own contiguous sequence of frames appended after the base frames (linked cels to the originals, the script's
//! per-step duration) with a tag over that sequence. Steps naming frames absent from the part are skipped (one note), the grid of the profile (`hw_cell`), and the object origin as the pivot of the slice `origin`
//! (a full canvas slice). The bytes are checked with `ase::validate` and the export fails when it reports anything.
//!
//! All writes are atomic and skipped when the file already holds the same bytes. Tile sets and screens are exported by
//! `tileio` (indexed PNG without tRNS, `animated/`, `palette_cycle/`, `.aseprite` with a tilemap layer).
//!
//! # Incremental `build`
//!
//! A part is skipped when the part file, template (with `extends`), inputs and a config hash (profile channel step and
//! grid, output patterns and paths) equal the hashes recorded by the last export in `<part>.status.json`, and every
//! recorded output exists with the recorded hash. The config hash is `hashes.exported_profile`.
//! A part whose lease is held by another agent is reported under `leased` and left alone (not a failure).
//!
//! # `check`
//!
//! Per manifest part: lint (`lint_part_auto`, errors fail), every expected file present, and no unlisted image
//! (`png jpg jpeg gif bmp webp aseprite ase`) directly inside an output folder (dot files ignored; the expected set
//! is the union over the whole manifest, so parts may share a folder). Stale exports (hash mismatch) are warnings.
//! One JSON summary; exit 1 when anything fails.

use crate::fsutil::PathSlash;
use crate::ase::{self, AseBuilder, LoopDir, SliceSpec};
use crate::cli::*;
use crate::color::{self, Rgb};
use crate::fsutil;
use crate::lease;
use crate::lint::{self, LintOpts, Severity};
use crate::manifest::{self, Manifest, OutSpec, PartDecl};
use crate::model::{Part, PartKind};
use crate::output::{Loc, Out, PxlError, Result};
use crate::png;
use crate::profile::{self, Profile};
use crate::pxl;
use crate::status::{self, StatusEvent};
use crate::template::{self, Template};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const DEFAULT_MS: u16 = 100;
const IMAGE_EXTS: [&str; 8] = ["png", "jpg", "jpeg", "gif", "bmp", "webp", "aseprite", "ase"];

// ------------------------------------------------------------------------------------------------ plan

#[derive(Clone, Copy)]
struct KeyInfo {
    layer: usize,
    local: u8,
    global: u8,
}

struct LayerPlan {
    name: String,
    /// `[transparent, keys...]`
    palette: Vec<Rgb>,
}

struct Plan {
    layers: Vec<LayerPlan>,
    keys: Vec<Option<KeyInfo>>,
    /// base key of an overlay key (`under`), as (local, global)
    under: Vec<Option<(u8, u8)>>,
    global: Vec<Rgb>,
    warnings: Vec<String>,
}

fn plan(part: &Part, tpl: &Template, prof: &Profile) -> Result<Plan> {
    if tpl.layers.is_empty() {
        return Err(PxlError::validation(format!("template '{}' has no layers", tpl.name)).with_loc(Loc::file(tpl.path.slash())));
    }
    let mut p = Plan { layers: Vec::new(), keys: vec![None; 256], under: vec![None; 256], global: vec![Rgb::new(0, 0, 0)], warnings: Vec::new() };
    let mut snapped: BTreeSet<String> = BTreeSet::new();
    for (li, l) in tpl.layers.iter().enumerate() {
        if l.keys.len() > 254 {
            return Err(PxlError::validation(format!("template layer '{}' has {} keys, an indexed PNG holds at most 254", l.name, l.keys.len())));
        }
        let mut lp = LayerPlan { name: l.name.clone(), palette: vec![Rgb::new(0, 0, 0)] };
        for kd in &l.keys {
            let raw = lint::effective_color(part, tpl, kd.key)
                .ok_or_else(|| PxlError::validation(format!("template key '{}' ({}) has no colour", kd.key as char, kd.role)).with_loc(Loc::file(tpl.path.slash())))?;
            let c = prof.snap(raw);
            if c != raw {
                snapped.insert(format!("{} {} -> {}", kd.key as char, color::to_hex(raw), color::to_hex(c)));
            }
            lp.palette.push(c);
            if p.global.len() > 255 {
                return Err(PxlError::validation("template has more than 255 colours in total, an indexed .aseprite holds at most 255"));
            }
            p.keys[kd.key as usize] = Some(KeyInfo { layer: li, local: (lp.palette.len() - 1) as u8, global: p.global.len() as u8 });
            p.global.push(c);
        }
        p.layers.push(lp);
    }
    for kd in tpl.keys() {
        let Some(ki) = p.keys[kd.key as usize] else { continue };
        if ki.layer == 0 {
            continue;
        }
        if let Some(u) = kd.under {
            if let Some(ui) = p.keys[u as usize] {
                if ui.layer == 0 {
                    p.under[kd.key as usize] = Some((ui.local, ui.global));
                }
            }
        }
    }
    if !snapped.is_empty() {
        p.warnings.push(format!("colours snapped to {}: {}", if prof.fixed_colours.is_empty() { format!("channel_step {}", prof.channel_step) } else { "the console palette".to_string() }, snapped.into_iter().collect::<Vec<_>>().join(", ")));
    }
    Ok(p)
}

/// Per layer: (local indices, global indices) of one frame.
fn render_frame(plan: &Plan, part: &Part, fi: usize) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
    let f = &part.frames[fi];
    let n = f.grid.w * f.grid.h;
    let mut out: Vec<(Vec<u8>, Vec<u8>)> = (0..plan.layers.len()).map(|_| (vec![0u8; n], vec![0u8; n])).collect();
    for (i, &k) in f.grid.cells.iter().enumerate() {
        if k == b'.' {
            continue;
        }
        let Some(ki) = plan.keys[k as usize] else {
            let (x, y) = (i % f.grid.w, i / f.grid.w);
            return Err(PxlError::validation(format!("key '{}' is not defined by the template", k as char))
                .with_loc(Loc::file(part.path.slash()).frame(f.name.clone()).at(y, x)));
        };
        out[ki.layer].0[i] = ki.local;
        out[ki.layer].1[i] = ki.global;
        if ki.layer != 0 {
            if let Some((l, g)) = plan.under[k as usize] {
                out[0].0[i] = l;
                out[0].1[i] = g;
            }
        }
    }
    Ok(out)
}

fn safe_frame_name(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('.') && !name.contains(['/', '\\', ':', '\0'])
}

/// Per frame and layer: whether the frame has a pixel of a key of that layer (layer 0 is always written).
/// An overlay PNG is only written (and expected) when its layer is used.
pub fn layers_used(part: &Part, layer_keys: &[Vec<u8>]) -> Vec<Vec<bool>> {
    part.frames
        .iter()
        .map(|f| layer_keys.iter().enumerate().map(|(li, ks)| li == 0 || f.grid.cells.iter().any(|c| ks.contains(c))).collect())
        .collect()
}

/// Every declared output of a part, in order: (frame name or "", layer index, path); the `.aseprite` last.
pub fn expected_files(part: &Part, layer_names: &[String], spec: &OutSpec, used: &[Vec<bool>]) -> Vec<(String, usize, PathBuf)> {
    let mut v = Vec::new();
    for (fi, f) in part.frames.iter().enumerate() {
        for (li, ln) in layer_names.iter().enumerate() {
            if li > 0 && !used.get(fi).and_then(|u| u.get(li)).copied().unwrap_or(false) {
                continue;
            }
            v.push((f.name.clone(), li, spec.file_path(&f.name, li, ln)));
        }
    }
    v.push((String::new(), 0, spec.ase.clone()));
    v
}

// ------------------------------------------------------------------------------------------------ animation tags

struct Anim {
    /// Frame durations in ms (`None` = the default).
    durations: Vec<Option<u16>>,
}

fn anim_info(part: &Part) -> Anim {
    Anim { durations: vec![None; part.frames.len()] }
}

// ------------------------------------------------------------------------------------------------ export

pub struct Job {
    pub part_path: PathBuf,
    pub spec: OutSpec,
    /// Restrict PNGs to these frames (empty = all).
    pub frames: Vec<String>,
    pub agent: Option<String>,
    /// An `--out` export into a directory other than the part's recorded one: deletes nothing outside that
    /// directory and leaves the recorded outputs in the status file alone.
    pub scratch: bool,
}

#[derive(Clone, Debug, Default)]
pub struct ExportResult {
    pub name: String,
    pub part: String,
    pub frames: usize,
    pub files: usize,
    pub written: usize,
    pub unchanged: usize,
    pub outputs: BTreeMap<String, String>,
    pub warnings: Vec<String>,
    /// Files this export no longer produces (recorded earlier, or overlay files with no pixels). They are NOT deleted by
    /// `export_part`; the caller removes them with `cleanup_stale` once every part of the run has written, skipping any
    /// path another part declares.
    pub stale: Vec<PathBuf>,
}

fn norm_path(p: &Path) -> PathBuf {
    let name = p.file_name().map(|n| n.to_os_string());
    match (p.parent().filter(|d| !d.as_os_str().is_empty()), name) {
        (Some(d), Some(n)) => d.canonicalize().map(|c| c.join(n)).unwrap_or_else(|_| p.to_path_buf()),
        (None, Some(n)) => std::env::current_dir().map(|c| c.join(n)).unwrap_or_else(|_| p.to_path_buf()),
        _ => p.to_path_buf(),
    }
}

/// Deletes the stale files of finished exports, but never a path that any result of this run declares, nor one that a
/// manifest part outside this run (skipped, failed, unselected) has recorded as its output.
pub fn cleanup_stale(results: &[&ExportResult], specs: &[&OutSpec], man: Option<&Manifest>) {
    let mut keep: BTreeSet<PathBuf> = BTreeSet::new();
    for (r, spec) in results.iter().zip(specs) {
        for k in r.outputs.keys() {
            keep.insert(norm_path(&spec.rel_base.join(k)));
        }
    }
    if let Some(m) = man {
        for d in &m.parts {
            let name = d.path.slash();
            if results.iter().any(|r| r.part == name) {
                continue;
            }
            for k in status::read(&d.path).hashes.outputs.keys() {
                keep.insert(norm_path(&d.spec.rel_base.join(k)));
            }
        }
    }
    // only files inside an output directory written in this run can be stale
    let dirs: Vec<PathBuf> = specs.iter().map(|sp| norm_path(&sp.dir.join("x")).parent().map(|p| p.to_path_buf()).unwrap_or_default()).collect();
    for r in results {
        for p in &r.stale {
            let np = norm_path(p);
            if !keep.contains(&np) && dirs.iter().any(|d| np.starts_with(d)) {
                let _ = std::fs::remove_file(p);
            }
        }
    }
}

/// Recorded outputs this run no longer produces (none at all for a scratch `--out` export). `cleanup_stale` deletes
/// only those inside an output directory this run writes.
pub(crate) fn push_stale(job: &Job, res: &mut ExportResult) {
    if job.scratch {
        return;
    }
    for k in status::read(&job.part_path).hashes.outputs.keys() {
        if !res.outputs.contains_key(k) {
            res.stale.push(job.spec.rel_base.join(k));
        }
    }
}

fn load_checked(part_path: &Path) -> Result<(Part, Template)> {
    let part = pxl::load(part_path)?;
    let tpl = template::load_for(&part)?;
    template::validate_grid(&tpl, &part)?;
    Ok((part, tpl))
}

pub(crate) fn config_hash(part: &Part, prof: &Profile, spec: &OutSpec) -> String {
    fsutil::content_hash(
        format!("v2;step={};cell={:?};png={};overlay={};dir={};ase={}{}{}{}", prof.channel_step, prof.hw_cell, spec.png, spec.overlay, spec.key(&spec.dir), spec.key(&spec.ase), crate::tileio::config_extra(part), crate::cells::config_extra(part), if prof.export_palette.is_empty() { String::new() } else { format!(";hw={}:{}:{:?}", prof.export_palette, prof.colors, prof.sprite_sizes) }).as_bytes(),
    )
}

pub(crate) fn write_if_changed(path: &Path, bytes: &[u8]) -> Result<bool> {
    if let Ok(old) = std::fs::read(path) {
        if old == bytes {
            return Ok(false);
        }
    }
    if let Some(d) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(d).map_err(|e| PxlError::io(format!("cannot create {}: {}", d.slash(), e)))?;
    }
    fsutil::atomic_write(path, bytes)?;
    Ok(true)
}

/// Builds the `.aseprite` bytes of a part (validated).
fn build_ase(part: &Part, plan: &Plan, prof: &Profile, anim: &Anim) -> Result<Vec<u8>> {
    let names: Vec<String> = plan.layers.iter().map(|l| l.name.clone()).collect();
    build_ase_with(part, &plan.global, &names, prof, anim, &|fi| Ok(render_frame(plan, part, fi)?.into_iter().map(|(_, g)| g).collect()))
}

/// Palette-set parts: one layer `sprite`, the 32-entry palette, cel pixels in the per-cell slot indexes.
fn build_ase_pset(part: &Part, pal: &[Rgb], composed: &[Vec<u8>], prof: &Profile, anim: &Anim) -> Result<Vec<u8>> {
    build_ase_with(part, pal, &["sprite".to_string()], prof, anim, &|fi| Ok(vec![composed[fi].clone()]))
}

fn build_ase_with(part: &Part, global: &[Rgb], layers: &[String], prof: &Profile, anim: &Anim, cv_of: &dyn Fn(usize) -> Result<Vec<Vec<u8>>>) -> Result<Vec<u8>> {
    let (w, h) = (part.size.0, part.size.1);
    if w == 0 || h == 0 || w > 65535 || h > 65535 {
        return Err(PxlError::validation(format!("canvas {}x{} cannot be stored in an .aseprite", w, h)));
    }
    let mut b = AseBuilder::new_indexed(w as u16, h as u16, global);
    b.set_grid(0, 0, prof.hw_cell.0.clamp(1, 65535) as u16, prof.hw_cell.1.clamp(1, 65535) as u16);
    for l in layers {
        b.add_layer(l);
    }
    let mut has_cel: Vec<Vec<bool>> = Vec::new();
    for (fi, f) in part.frames.iter().enumerate() {
        let dur = anim.durations[fi].unwrap_or(DEFAULT_MS);
        let af = b.add_frame(dur);
        let cv = cv_of(fi)?;
        has_cel.push(cv.iter().enumerate().map(|(li, g)| b.set_cel_canvas(af, li, g)).collect());
        b.add_tag(&f.name, fi, fi, LoopDir::Forward, 0);
    }
    b.add_slice(SliceSpec { name: "origin".into(), x: 0, y: 0, w: w as u32, h: h as u32, center: None, pivot: Some((part.origin.0, part.origin.1)) });
    let bytes = b.build()?;
    let problems = ase::validate(&bytes);
    if !problems.is_empty() {
        let first: Vec<String> = problems.iter().take(3).map(|p| format!("{} (offset {})", p.msg, p.offset)).collect();
        return Err(PxlError::validation(format!("generated .aseprite fails validation ({} problems): {}", problems.len(), first.join("; "))));
    }
    Ok(bytes)
}

/// Exports one part. Holds the lease check, writes atomically, records the status.
pub fn export_part(job: &Job) -> Result<ExportResult> {
    lease::guard(&job.part_path, job.agent.as_deref())?;
    let (part, tpl) = load_checked(&job.part_path)?;
    for fname in &job.frames {
        if part.frame(fname).is_none() {
            return Err(PxlError::usage(format!("{}: no frame '{}'", job.part_path.slash(), fname)));
        }
    }
    for f in &part.frames {
        if !safe_frame_name(&f.name) {
            return Err(PxlError::validation(format!("frame name '{}' cannot be used as a file name", f.name)).with_loc(Loc::file(job.part_path.slash()).frame(f.name.clone())));
        }
    }
    let prof = profile::load_for(&part, None)?;
    if part.kind != PartKind::Sprite {
        return crate::tileio::export_tiles(job, &part, &tpl, &prof);
    }
    let pl = plan(&part, &tpl, &prof)?;
    let spec = &job.spec;
    let lnames: Vec<String> = pl.layers.iter().map(|l| l.name.clone()).collect();
    let lkeys: Vec<Vec<u8>> = tpl.layers.iter().map(|l| l.keys.iter().map(|k| k.key).collect()).collect();
    let used = layers_used(&part, &lkeys);
    let files = expected_files(&part, &lnames, spec, &used);
    let pset = if part.palette_set.is_some() { Some(crate::cells::resolve(&part, &prof)?) } else { None };
    let mut seen = BTreeSet::new();
    for (_, _, p) in files.iter().filter(|_| pset.is_none()) {
        if !seen.insert(p.clone()) {
            return Err(PxlError::validation(format!("two outputs share the file {}", p.slash()))
                .with_hint("the manifest `overlay` pattern needs {layer} when the template has several overlay layers"));
        }
    }
    let (w, h) = part.size;
    let mut res = ExportResult { name: part.name.clone(), part: job.part_path.slash(), frames: part.frames.len(), warnings: pl.warnings.clone(), ..Default::default() };
    let write = |path: &Path, bytes: &[u8], res: &mut ExportResult| -> Result<()> {
        if write_if_changed(path, bytes)? {
            res.written += 1;
        } else {
            res.unchanged += 1;
        }
        res.files += 1;
        res.outputs.insert(spec.key(path), fsutil::content_hash(bytes));
        Ok(())
    };
    let partial = !job.frames.is_empty();
    let mut composed: Vec<Vec<u8>> = Vec::new();
    if let Some(pres) = &pset {
        for fi in 0..part.frames.len() {
            let fo = crate::cells::export_frame(&part, &tpl, pres, &prof, fi)?;
            let mut c = fo.base;
            if let Some(d) = &fo.detail {
                for (i, v) in c.iter_mut().enumerate() {
                    if *v == 0 {
                        *v = d[i];
                    }
                }
            }
            composed.push(c);
        }
    }
    for (fi, f) in part.frames.iter().enumerate() {
        if partial && !job.frames.contains(&f.name) {
            continue;
        }
        if let Some(pres) = &pset {
            // shared palette set: one 32-entry image, the cell list, and a detail image only for two-slot cells
            let fo = crate::cells::export_frame(&part, &tpl, pres, &prof, fi)?;
            let (pal, alpha) = crate::cells::palette32(pres);
            let png_path = spec.file_path(&f.name, 0, "base");
            write(&png_path, &png::encode_indexed_alpha(w, h, &fo.base, &pal, Some(&alpha))?, &mut res)?;
            write(&crate::cells::cells_json_path(&png_path), fo.cells_json.as_bytes(), &mut res)?;
            let dpath = spec.file_path(&f.name, 1, "detail");
            match &fo.detail {
                Some(d) => write(&dpath, &png::encode_indexed_alpha(w, h, d, &pal, Some(&alpha))?, &mut res)?,
                None => res.stale.push(dpath),
            }
            continue;
        }
        let cv = render_frame(&pl, &part, fi)?;
        for (li, (local, _)) in cv.iter().enumerate() {
            if !used[fi][li] {
                // no overlay pixel: no file, and none left over from an earlier export (removed later by `cleanup_stale`)
                res.stale.push(spec.file_path(&f.name, li, &pl.layers[li].name));
                continue;
            }
            let bytes = png::encode_indexed(w, h, local, &pl.layers[li].palette, true)?;
            write(&spec.file_path(&f.name, li, &pl.layers[li].name), &bytes, &mut res)?;
        }
    }
    if !prof.export_palette.is_empty() {
        // palette lines: the colour layers, or the slots of the part's palette set
        let layers: Vec<(String, Vec<Rgb>)> = match &pset {
            Some(r) => r.slots.iter().map(|(s, n, cols)| (format!("slot {} ({})", s, n), vec![Rgb::new(0, 0, 0), cols[0], cols[1], cols[2]])).collect(),
            None => pl.layers.iter().map(|l| (l.name.clone(), l.palette.clone())).collect(),
        };
        write(&spec.dir.join(format!("{}.palette.json", part.name)), crate::hwexport::palette_json(&prof, &part, &layers).as_bytes(), &mut res)?;
        if prof.sprite_size_mode == "variable" {
            write(&spec.dir.join(format!("{}.sprites.json", part.name)), crate::hwexport::sprites_json(&prof, &part).as_bytes(), &mut res)?;
        }
    }
    let anim = anim_info(&part);
    let ase_bytes = match &pset {
        Some(pres) => build_ase_pset(&part, &crate::cells::palette32(pres).0, &composed, &prof, &anim)?,
        None => build_ase(&part, &pl, &prof, &anim)?,
    };
    write(&spec.ase, &ase_bytes, &mut res)?;
    if !partial {
        push_stale(job, &mut res);
        if !job.scratch {
            status::record(&job.part_path, &part, StatusEvent::Exported);
            status::record(&job.part_path, &part, StatusEvent::ExportConfig(config_hash(&part, &prof, spec)));
            status::record(&job.part_path, &part, StatusEvent::Outputs(res.outputs.clone()));
        }
    }
    Ok(res)
}

/// True when the recorded export still matches part, template, inputs, config and the files on disk.
fn is_fresh(part_path: &Path, spec: &OutSpec) -> bool {
    let Ok(part) = pxl::load(part_path) else { return false };
    let Ok(prof) = profile::load_for(&part, None) else { return false };
    let s = status::read(part_path);
    let h = &s.hashes;
    let Some(ep) = &h.exported_part else { return false };
    if *ep != status::part_file_hash(&part) || h.exported_template != status::template_hash(&part) || h.exported_inputs.as_deref() != Some(status::inputs_hash(&part).as_str()) {
        return false;
    }
    if h.exported_profile.as_deref() != Some(config_hash(&part, &prof, spec).as_str()) {
        return false;
    }
    let mut n = 0;
    for (k, v) in &h.outputs {
        n += 1;
        match std::fs::read(spec.rel_base.join(k)) {
            Ok(b) if fsutil::content_hash(&b) == *v => {}
            _ => return false,
        }
    }
    n > 0 && h.outputs.contains_key(&spec.key(&spec.ase))
}

// ------------------------------------------------------------------------------------------------ commands

fn summarize_failures(total: usize, errs: &[(String, PxlError)]) -> PxlError {
    let first = &errs[0].1;
    let names: Vec<&str> = errs.iter().map(|e| e.0.as_str()).collect();
    PxlError { msg: format!("{} of {} part(s) failed ({}): {}", errs.len(), total, names.join(", "), first.msg), ..first.clone() }
}

fn err_json(part: &str, e: &PxlError) -> Value {
    json!({"part": part, "error": e.to_json()})
}

pub fn run_export(a: &ExportArgs, out: &mut Out) -> Result<()> {
    let man = Manifest::discover(a.manifest.as_deref())?;
    let agent = out.agent().map(String::from);
    let mut jobs: Vec<Job> = Vec::new();
    if a.all {
        let m = man.as_ref().ok_or_else(|| PxlError::usage("`export --all` needs a pxl.toml").with_hint("run it from the project folder or pass --manifest"))?;
        if m.parts.is_empty() {
            return Err(PxlError::usage("pxl.toml declares no parts"));
        }
        for d in &m.parts {
            jobs.push(Job { part_path: d.path.clone(), spec: d.spec.clone(), frames: a.frames.clone(), agent: agent.clone(), scratch: false });
        }
    } else {
        if a.parts.is_empty() {
            return Err(PxlError::usage("give a part (or --all)"));
        }
        let paths = fsutil::expand_parts(&a.parts)?;
        for p in paths {
            let mut spec = match man.as_ref().and_then(|m| m.find(&p)) {
                Some(d) => d.spec.clone(),
                None => OutSpec::default_for(&p),
            };
            let mut scratch = false;
            if let Some(o) = &a.out {
                scratch = norm_path(&spec.dir.join("x")) != norm_path(&Path::new(o).join("x"));
                let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("part").to_string();
                spec = spec.with_dir(Path::new(o), &stem);
            }
            jobs.push(Job { part_path: p, spec, frames: a.frames.clone(), agent: agent.clone(), scratch });
        }
        if a.out.is_some() && jobs.len() > 1 {
            return Err(PxlError::usage("--out takes one part; use pxl.toml for several"));
        }
    }
    let total = jobs.len();
    let results = fsutil::par_map(&jobs, export_part);
    let mut ok = Vec::new();
    let mut errs = Vec::new();
    for (j, r) in jobs.iter().zip(results) {
        match r {
            Ok(r) => ok.push(r),
            Err(e) => errs.push((j.part_path.slash(), e)),
        }
    }
    let ok_specs: Vec<&OutSpec> = ok.iter().filter_map(|r| jobs.iter().find(|j| j.part_path.slash() == r.part).map(|j| &j.spec)).collect();
    if ok_specs.len() == ok.len() {
        cleanup_stale(&ok.iter().collect::<Vec<_>>(), &ok_specs, man.as_ref());
    }
    let mut text = String::new();
    for r in &ok {
        text.push_str(&format!("exported {}: {} frames, {} files ({} written, {} unchanged)\n", r.part, r.frames, r.files, r.written, r.unchanged));
        for w in &r.warnings {
            out.warn(format!("{}: {}", r.part, w));
        }
    }
    for (p, e) in &errs {
        text.push_str(&format!("failed {}: {}\n", p, e.msg));
    }
    out.emit(
        text.trim_end(),
        json!({
            "exported": ok.iter().map(|r| json!({"part": r.part, "name": r.name, "frames": r.frames, "files": r.files, "written": r.written, "unchanged": r.unchanged,
                "outputs": r.outputs.iter().map(|(k, v)| json!({"path": k, "hash": v})).collect::<Vec<_>>()})).collect::<Vec<_>>(),
            "failed": errs.iter().map(|(p, e)| err_json(p, e)).collect::<Vec<_>>(),
        }),
    );
    if errs.is_empty() {
        Ok(())
    } else {
        Err(summarize_failures(total, &errs))
    }
}

fn selected<'a>(m: &'a Manifest, args: &[String]) -> Result<Vec<&'a PartDecl>> {
    if args.is_empty() {
        return Ok(m.parts.iter().collect());
    }
    let paths = fsutil::expand_parts(args)?;
    let mut v = Vec::new();
    for p in paths {
        match m.find(&p) {
            Some(d) => {
                if !v.iter().any(|x: &&PartDecl| x.name == d.name) {
                    v.push(d)
                }
            }
            None => return Err(PxlError::usage(format!("{} is not declared in {}", p.slash(), m.path.slash()))),
        }
    }
    Ok(v)
}

fn need_manifest(man: Option<Manifest>, cmd: &str) -> Result<Manifest> {
    let m = man.ok_or_else(|| PxlError::usage(format!("`{}` needs a pxl.toml", cmd)).with_hint("run it from the project folder or pass --manifest"))?;
    if m.parts.is_empty() {
        return Err(PxlError::usage(format!("{} declares no parts", m.path.slash())));
    }
    Ok(m)
}

pub fn run_build(a: &BuildArgs, out: &mut Out) -> Result<()> {
    let m = need_manifest(Manifest::discover(a.manifest.as_deref())?, "build")?;
    let decls = selected(&m, &a.parts)?;
    let agent = out.agent().map(String::from);
    let force = a.force;
    enum R {
        Built(ExportResult),
        Skipped,
        Leased(String),
        Failed(PxlError),
    }
    let results = fsutil::par_map(&decls, |d| {
        if !force && is_fresh(&d.path, &d.spec) {
            return R::Skipped;
        }
        let job = Job { part_path: d.path.clone(), spec: d.spec.clone(), frames: Vec::new(), agent: agent.clone(), scratch: false };
        match export_part(&job) {
            Ok(r) => R::Built(r),
            Err(e) if e.msg.contains("held by") => R::Leased(e.msg),
            Err(e) => R::Failed(e),
        }
    });
    let (mut built, mut skipped, mut leased, mut errs) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let results_kind: Vec<bool> = results.iter().map(|r| matches!(r, R::Built(_))).collect();
    for (d, r) in decls.iter().zip(results) {
        match r {
            R::Built(r) => built.push((d.name.clone(), r)),
            R::Skipped => skipped.push(d.name.clone()),
            R::Leased(m) => leased.push((d.name.clone(), m)),
            R::Failed(e) => errs.push((d.name.clone(), e)),
        }
    }
    {
        let rs: Vec<&ExportResult> = built.iter().map(|b| &b.1).collect();
        let specs: Vec<&OutSpec> = decls.iter().zip(&results_kind).filter(|(_, k)| **k).map(|(d, _)| &d.spec).collect();
        cleanup_stale(&rs, &specs, Some(&m));
    }
    let mut palette_files: Vec<std::path::PathBuf> = Vec::new();
    if a.parts.is_empty() && errs.is_empty() {
        for (p, _) in crate::scene::write_palettes(&m)? {
            palette_files.push(p);
        }
    }
    let mut text = String::new();
    // the delivery manifest (and the configured spec listing) is regenerated by every full build; a problem fails the build
    let mut manifest_files: Vec<String> = Vec::new();
    let mut manifest_problems: Vec<String> = Vec::new();
    if a.parts.is_empty() && errs.is_empty() && crate::dmanifest::enabled(&m) {
        match crate::dmanifest::write(&m) {
            Ok((files, problems)) => {
                manifest_files = files.iter().map(|f| f.slash()).collect();
                manifest_problems = problems;
            }
            Err(e) => manifest_problems.push(e.msg),
        }
    }
    if a.parts.is_empty() && errs.is_empty() {
        for mv in export_manifest_sync(&m, &palette_files) {
            text.push_str(&format!("removed {}\n", mv));
        }
    }
    for p in &palette_files {
        text.push_str(&format!("palettes {}\n", p.slash()));
    }
    for f in &manifest_files {
        text.push_str(&format!("manifest {}\n", f));
    }
    for p in &manifest_problems {
        text.push_str(&format!("manifest problem: {}\n", p));
    }
    for (n, r) in &built {
        text.push_str(&format!("built {}: {} frames, {} files ({} written, {} unchanged)\n", n, r.frames, r.files, r.written, r.unchanged));
        for w in &r.warnings {
            out.warn(format!("{}: {}", n, w));
        }
    }
    for n in &skipped {
        text.push_str(&format!("up to date {}\n", n));
    }
    for (n, m) in &leased {
        text.push_str(&format!("leased {}: {}\n", n, m));
    }
    for (n, e) in &errs {
        text.push_str(&format!("failed {}: {}\n", n, e.msg));
    }
    let written: usize = built.iter().map(|b| b.1.written).sum();
    out.emit(
        text.trim_end(),
        json!({
            "built": built.iter().map(|(n, r)| json!({"part": n, "frames": r.frames, "files": r.files, "written": r.written, "unchanged": r.unchanged})).collect::<Vec<_>>(),
            "skipped": skipped,
            "leased": leased.iter().map(|(n, m)| json!({"part": n, "message": m})).collect::<Vec<_>>(),
            "failed": errs.iter().map(|(n, e)| err_json(n, e)).collect::<Vec<_>>(),
            "files_written": written,
            "manifest": {"files": manifest_files, "problems": manifest_problems},
        }),
    );
    if !manifest_problems.is_empty() {
        return Err(PxlError::validation(format!("the delivery manifest cannot be written: {} problem(s), first: {}", manifest_problems.len(), manifest_problems[0])));
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(summarize_failures(decls.len(), &errs))
    }
}

/// `<out_root>/.export_manifest.json` lists every file the last full build produced. Files it names that no part
/// produces any more move to `<out_root>/.removed/` (never deleted, so a mistake can be undone). Returns the moves.
fn export_manifest_sync(m: &Manifest, palette_files: &[PathBuf]) -> Vec<String> {
    let Some(root) = m.out_root.as_ref().map(|r| m.dir.join(r)) else { return Vec::new() };
    let rootc = norm(&root.join("x")).parent().map(|p| p.to_path_buf()).unwrap_or(root.clone());
    let mut current: BTreeSet<String> = BTreeSet::new();
    let rel = |p: &Path| norm(p).strip_prefix(&rootc).ok().map(|r| r.to_string_lossy().replace('\\', "/"));
    for d in &m.parts {
        for k in status::read(&d.path).hashes.outputs.keys() {
            if let Some(r) = rel(&d.spec.rel_base.join(k)) {
                current.insert(r);
            }
        }
    }
    for p in palette_files {
        if let Some(r) = rel(p) {
            current.insert(r);
        }
    }
    let mpath = rootc.join(".export_manifest.json");
    let prev: Vec<String> = std::fs::read_to_string(&mpath).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|v| v["files"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(crate::fsutil::norm_slash)).collect())).unwrap_or_default();
    let mut moved = Vec::new();
    for f in prev.iter().filter(|f| !current.contains(*f)) {
        let src = rootc.join(f);
        if src.is_file() {
            let dst = rootc.join(".removed").join(f);
            if let Some(d) = dst.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            if std::fs::rename(&src, &dst).is_ok() {
                moved.push(format!("{} -> .removed/{}", f, f));
            }
        }
    }
    let body = json!({"files": current.iter().collect::<Vec<_>>(), "count": current.len()});
    let _ = fsutil::atomic_write(&mpath, serde_json::to_string_pretty(&body).unwrap_or_default().as_bytes());
    moved
}

struct Checked {
    name: String,
    path: String,
    frames: usize,
    errors: u32,
    warnings: u32,
    issues: Vec<Value>,
    missing: Vec<String>,
    problems: Vec<String>,
    freshness: &'static str,
    expected: Vec<PathBuf>,
    legacy_profile: Option<String>,
    /// Gated projects only: sprite frames checked with a source image, of all frames, and the `reference_missing` messages.
    refs: Option<(usize, usize, Vec<String>)>,
}

fn check_part(d: &PartDecl, selected: bool, hand_finished: bool) -> Checked {
    let mut c = Checked {
        name: d.name.clone(),
        path: d.path.slash(),
        frames: 0,
        errors: 0,
        warnings: 0,
        issues: Vec::new(),
        missing: Vec::new(),
        problems: Vec::new(),
        freshness: "none",
        expected: Vec::new(),
        legacy_profile: None,
        refs: None,
    };
    let part = match pxl::load(&d.path) {
        Ok(p) => p,
        Err(e) => {
            c.problems.push(format!("cannot load part: {}", e.msg));
            c.errors += 1;
            return c;
        }
    };
    c.frames = part.frames.len();
    c.legacy_profile = crate::console::legacy_name_of(&part);
    let tpl = template::load_for(&part).ok();
    let lnames: Vec<String> = match &tpl {
        Some(t) if !t.layers.is_empty() => t.layers.iter().map(|l| l.name.clone()).collect(),
        _ => vec!["base".to_string()],
    };
    let lkeys: Vec<Vec<u8>> = match &tpl {
        Some(t) if !t.layers.is_empty() => t.layers.iter().map(|l| l.keys.iter().map(|k| k.key).collect()).collect(),
        _ => vec![Vec::new()],
    };
    let pexp = if part.kind == PartKind::Sprite && part.palette_set.is_some() {
        match (&tpl, profile::load_for(&part, None)) {
            (Some(t), Ok(pf)) => crate::cells::expected_outputs(&part, t, &pf, &d.spec),
            _ => None,
        }
    } else {
        None
    };
    c.expected = if let Some(v) = pexp {
        v
    } else if part.kind == PartKind::Sprite {
        expected_files(&part, &lnames, &d.spec, &layers_used(&part, &lkeys)).into_iter().map(|e| e.2).collect()
    } else {
        crate::tileio::expected_files(&part, &d.spec).into_iter().map(|e| e.2).collect()
    };
    if !selected {
        return c;
    }
    match profile::load_for(&part, None) {
        Ok(prof) => {
            // gated projects lint with the sources too, exactly as `pxl lint` does, so `status` and `lint` count the same warnings
            let issues = lint::lint_part_auto(&part, &prof, &LintOpts { refs: part.kind == PartKind::Sprite && crate::design::design_project(&part), ..LintOpts::default() });
            let (mut e, mut w) = (0u32, 0u32);
            for i in &issues {
                match i.severity {
                    Severity::Error => e += 1,
                    Severity::Warning => w += 1,
                    Severity::Advice => {}
                }
            }
            if part.kind == PartKind::Sprite && crate::design::design_project(&part) {
                let miss: Vec<String> = issues.iter().filter(|i| i.rule == crate::design::REFERENCE_MISSING).map(|i| format!("{} {}", i.frame.clone().unwrap_or_default(), i.message)).collect();
                c.refs = Some((part.frames.len().saturating_sub(miss.len()), part.frames.len(), miss));
            }
            c.errors += e;
            c.warnings += w;
            c.issues = issues.iter().filter(|i| i.severity == Severity::Error).take(20).map(|i| i.to_json()).collect();
            status::record(&d.path, &part, StatusEvent::LintSummary { errors: e, warnings: w, infos: issues.iter().filter(|i| i.severity == Severity::Advice).count() as u32 });
        }
        Err(e) => {
            c.problems.push(format!("profile: {}", e.msg));
            c.errors += 1;
        }
    }
    for p in &c.expected {
        if !p.is_file() {
            c.missing.push(d.spec.key(p));
        }
    }
    let s = status::read(&d.path);
    c.freshness = status::export_freshness(&s, &part, &status::part_file_hash(&part));
    if hand_finished {
        let raw: Vec<status::RawFrame> = status::raw_audit(&s, &part).into_iter().filter(|r| r.count > 0).collect();
        let n: usize = raw.iter().map(|r| r.count).sum();
        if n > 0 {
            let which: Vec<String> = raw.iter().take(4).map(|r| if r.tiles.is_empty() { r.frame.clone() } else { format!("{} ({} tiles)", r.frame, r.count) }).collect();
            c.problems.push(format!("{} frame(s)/tile(s) are still the automatic output, not hand-finished: {} (pxl status --raw lists them)", n, which.join(", ")));
        }
    }
    c
}

fn norm(p: &Path) -> PathBuf {
    match (p.parent(), p.file_name()) {
        (Some(d), Some(n)) => manifest::canon(if d.as_os_str().is_empty() { Path::new(".") } else { d }).join(n),
        _ => p.to_path_buf(),
    }
}

pub fn run_check(a: &CheckArgs, out: &mut Out) -> Result<()> {
    let m = need_manifest(Manifest::discover(a.manifest.as_deref())?, "check")?;
    let sel = selected(&m, &a.parts)?;
    let sel_names: BTreeSet<&str> = sel.iter().map(|d| d.name.as_str()).collect();
    let all: Vec<&PartDecl> = m.parts.iter().collect();
    if a.hand_finished && a.recompute {
        for d in &sel {
            if let Err(e) = crate::rawaudit::recompute(&d.path, a.tolerance) {
                out.warn(format!("{}: recompute failed: {}", d.path.slash(), e.msg));
            }
        }
    }
    let checked = fsutil::par_map(&all, |d| check_part(d, sel_names.contains(d.name.as_str()), a.hand_finished));
    let mut expected: BTreeSet<PathBuf> = BTreeSet::new();
    for c in &checked {
        for p in &c.expected {
            expected.insert(norm(p));
        }
    }
    let mut dirs: BTreeSet<PathBuf> = BTreeSet::new();
    for d in &sel {
        dirs.insert(d.spec.dir.clone());
        if let Some(p) = d.spec.ase.parent() {
            dirs.insert(p.to_path_buf());
        }
    }
    for c in checked.iter().filter(|c| sel_names.contains(c.name.as_str())) {
        for p in &c.expected {
            if let Some(d) = p.parent() {
                dirs.insert(d.to_path_buf());
            }
        }
    }
    let mut unlisted: Vec<String> = Vec::new();
    for d in &dirs {
        let Ok(rd) = std::fs::read_dir(d) else { continue };
        let mut names: Vec<_> = rd.filter_map(|e| e.ok()).collect();
        names.sort_by_key(|e| e.file_name());
        for e in names {
            let p = e.path();
            let fname = e.file_name().to_string_lossy().into_owned();
            if fname.starts_with('.') || !p.is_file() {
                continue;
            }
            let ext = p.extension().map(|x| x.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            if IMAGE_EXTS.contains(&ext.as_str()) && !expected.contains(&norm(&p)) {
                let s = p.slash();
                if !unlisted.contains(&s) {
                    unlisted.push(s);
                }
            }
        }
    }
    // a `spec.json` of type "tiles" next to the outputs lists the files the package expects
    let mut spec_missing: Vec<String> = Vec::new();
    let mut spec_dirs: BTreeSet<PathBuf> = BTreeSet::new();
    for d in &sel {
        spec_dirs.insert(d.spec.dir.clone());
    }
    for dir in &spec_dirs {
        let Ok(txt) = std::fs::read_to_string(dir.join("spec.json")) else { continue };
        let Ok(v) = serde_json::from_str::<Value>(&txt) else { continue };
        if v["type"].as_str() != Some("tiles") {
            continue;
        }
        if let Some(files) = v["files"].as_object() {
            for f in files.keys() {
                if !expected.contains(&norm(&dir.join(crate::fsutil::norm_slash(f)))) {
                    spec_missing.push(format!("{}: spec.json expects {} but no part in the manifest exports it", dir.slash(), f));
                }
            }
        }
    }
    let legacy: Vec<String> = checked.iter().filter(|c| sel_names.contains(c.name.as_str())).filter_map(|c| c.legacy_profile.clone()).collect();
    if let Some(n) = crate::console::deprecated_profile_note(&legacy) {
        out.warn(n);
    }
    if m.console.is_none() {
        if let Some(n) = crate::console::no_console_note(&m.dir) {
            out.warn(n);
        }
    }
    let mut parts_json = Vec::new();
    let mut text = String::new();
    let (mut errors, mut warnings, mut missing_n) = (0u32, 0u32, 0usize);
    let mut ok_all = true;
    let (mut ref_have, mut ref_of) = (0usize, 0usize);
    for c in checked.iter().filter(|c| sel_names.contains(c.name.as_str())) {
        let ok = c.errors == 0 && c.missing.is_empty() && c.problems.is_empty();
        ok_all &= ok;
        errors += c.errors;
        warnings += c.warnings;
        missing_n += c.missing.len();
        text.push_str(&format!(
            "{} {}: {} frames, lint {} error(s) {} warning(s), {} missing file(s), export {}\n",
            if ok { "ok  " } else { "FAIL" },
            c.name,
            c.frames,
            c.errors,
            c.warnings,
            c.missing.len(),
            c.freshness
        ));
        for p in &c.problems {
            text.push_str(&format!("  {}\n", p));
        }
        for i in c.issues.iter().take(5) {
            text.push_str(&format!("  lint {}: {}\n", i["rule"].as_str().unwrap_or(""), i["message"].as_str().unwrap_or("")));
        }
        for mi in c.missing.iter().take(5) {
            text.push_str(&format!("  missing {}\n", mi));
        }
        if let Some((have, of, miss)) = &c.refs {
            ref_have += have;
            ref_of += of;
            for r in miss.iter().take(5) {
                text.push_str(&format!("  reference_missing: part {} frame {}\n", c.name, r));
            }
            if miss.len() > 5 {
                text.push_str(&format!("  reference_missing: ... {} more frame(s) of {}\n", miss.len() - 5, c.name));
            }
        }
        let mut pj = json!({
            "part": c.name, "path": c.path, "ok": ok, "frames": c.frames,
            "lint": {"errors": c.errors, "warnings": c.warnings, "issues": c.issues},
            "missing": c.missing, "problems": c.problems, "export": c.freshness,
        });
        if let Some((h, o, m)) = &c.refs {
            pj["references"] = json!({"checked": h, "of": o, "reference_missing": m.len()});
        }
        parts_json.push(pj);
    }
    for u in &unlisted {
        text.push_str(&format!("unlisted image {}\n", u));
    }
    for m in &spec_missing {
        text.push_str(&format!("spec {}\n", m));
    }
    let budget_issues: Vec<Value> = if a.parts.is_empty() {
        crate::cellmem::scene_issues(&m).into_iter().map(|(sc, g, msg)| json!({"rule": "tile_budget", "scene": sc, "group": g, "message": msg})).collect()
    } else {
        Vec::new()
    };
    for b in &budget_issues {
        text.push_str(&format!("tile_budget: {}\n", b["message"].as_str().unwrap_or("")));
    }
    // shared hardware budgets per scene, and the fail-closed project findings (M5 T5)
    let (mut scene_json, mut hw_issues): (Vec<Value>, Vec<String>) = (Vec::new(), Vec::new());
    if a.parts.is_empty() {
        hw_issues = crate::scene::project_issues(&m);
        if !m.scenes.is_empty() {
            match crate::scene::hardware(&m) {
                Ok((sp, tp)) => {
                    let mut cache = std::collections::HashMap::new();
                    for sc in &m.scenes {
                        let r = crate::scene::check_scene(&m, sc, &sp, &tp, &mut cache);
                        text.push_str(&r.text().lines().filter(|l| l.contains("FAIL") || l.contains("note:") || l.starts_with("scene")).map(|l| format!("{}\n", l)).collect::<String>());
                        if !r.ok() {
                            hw_issues.push(format!("scene {} exceeds a shared budget", sc.name));
                        }
                        scene_json.push(r.to_json());
                    }
                }
                Err(e) => hw_issues.push(format!("hardware profile: {}", e.msg)),
            }
        }
    }
    if a.parts.is_empty() {
        if crate::dmanifest::enabled(&m) {
            hw_issues.extend(crate::dmanifest::stale(&m));
        }
        hw_issues.extend(crate::layout::issues(&m));
    }
    for h in &hw_issues {
        text.push_str(&format!("hardware: {}\n", h));
    }
    if a.parts.is_empty() {
        match crate::scene::palette_outputs(&m) {
            Ok(list) => {
                for (sc, path, bytes) in list {
                    match std::fs::read(&path) {
                        Ok(have) if have == bytes => {}
                        Ok(_) => {
                            let h = format!("scene {}: {} is out of date (run `pxl build` or `pxl export-palettes`)", sc, path.slash());
                            text.push_str(&format!("hardware: {}\n", h));
                            hw_issues.push(h);
                        }
                        Err(_) => {
                            let h = format!("scene {}: {} is missing (run `pxl build` or `pxl export-palettes`)", sc, path.slash());
                            text.push_str(&format!("hardware: {}\n", h));
                            hw_issues.push(h);
                        }
                    }
                }
            }
            Err(e) => {
                text.push_str(&format!("hardware: sprite palettes: {}\n", e.msg));
                hw_issues.push(format!("sprite palettes: {}", e.msg));
            }
        }
    }
    let mut exempt_json: Vec<Value> = Vec::new();
    let mut exempt_warn: Vec<String> = Vec::new();
    if a.parts.is_empty() {
        for (n, reason, scenes) in crate::scene::exempt_parts(&m) {
            text.push_str(&format!("exempt: {}: {}\n", n, reason));
            if !scenes.is_empty() {
                let w = format!("part {} is budget_exempt but listed in scene group(s) of {}; exempt parts are not in any scene", n, scenes.join(", "));
                text.push_str(&format!("warning: {}\n", w));
                exempt_warn.push(w);
            }
            exempt_json.push(json!({"part": n, "reason": reason, "scenes": scenes}));
        }
    }
    ok_all &= unlisted.is_empty() && spec_missing.is_empty() && budget_issues.is_empty() && hw_issues.is_empty();
    text.push_str(&format!(
        "{}: {} part(s), {} lint error(s), {} warning(s), {} missing, {} unlisted; checked {} of {} part(s), {} of {} scene(s)",
        if ok_all { "PASS" } else { "FAIL" },
        parts_json.len(),
        errors,
        warnings,
        missing_n,
        unlisted.len(),
        parts_json.len(),
        all.len(),
        scene_json.len(),
        m.scenes.len()
    ));
    if ref_of > 0 {
        out.set("references", json!({"checked": ref_have, "of": ref_of, "reference_missing": ref_of - ref_have}));
        text.push_str(&format!("; references: checked {} of {} frame(s) with references{}", ref_have, ref_of, if ref_have < ref_of { format!(" ({} reference_missing)", ref_of - ref_have) } else { String::new() }));
    }
    out.emit(
        text,
        json!({
            "pass": ok_all,
            "parts": parts_json,
            "unlisted": unlisted,
            "spec_missing": spec_missing,
            "tile_budget": budget_issues,
            "scenes": scene_json,
            "hardware": hw_issues,
            "exempt": exempt_json,
            "exempt_warnings": exempt_warn,
            "checked": {"parts": parts_json.len(), "of_parts": all.len(), "scenes": scene_json.len(), "of_scenes": m.scenes.len()},
            "totals": {"parts": parts_json.len(), "lint_errors": errors, "lint_warnings": warnings, "missing": missing_n, "unlisted": unlisted.len()},
        }),
    );
    if ok_all {
        Ok(())
    } else {
        // say what failed, not only the counts: every reason that made the verdict FAIL, with the first examples
        let mut why: Vec<String> = Vec::new();
        let bad: Vec<String> = checked.iter().filter(|c| sel_names.contains(c.name.as_str()) && !(c.errors == 0 && c.missing.is_empty() && c.problems.is_empty())).map(|c| c.name.clone()).collect();
        if errors > 0 {
            why.push(format!("{} lint error(s)", errors));
        }
        if missing_n > 0 {
            why.push(format!("{} missing export file(s)", missing_n));
        }
        let problems: Vec<String> = checked.iter().filter(|c| sel_names.contains(c.name.as_str())).flat_map(|c| c.problems.iter().map(move |p| format!("{}: {}", c.name, p))).collect();
        if !problems.is_empty() {
            why.push(format!("{} part problem(s) ({})", problems.len(), problems.iter().take(2).cloned().collect::<Vec<_>>().join("; ")));
        }
        if !unlisted.is_empty() {
            why.push(format!("{} unlisted image(s)", unlisted.len()));
        }
        if !spec_missing.is_empty() {
            why.push(format!("{} file(s) missing from the delivery spec", spec_missing.len()));
        }
        if !budget_issues.is_empty() {
            why.push(format!("{} tile budget issue(s)", budget_issues.len()));
        }
        if !hw_issues.is_empty() {
            why.push(format!("{} hardware/scene issue(s) ({})", hw_issues.len(), hw_issues.iter().take(2).cloned().collect::<Vec<_>>().join("; ")));
        }
        Err(PxlError::lint(format!("check failed: {}{}", why.join(", "), if bad.is_empty() { String::new() } else { format!(" [parts: {}]", bad.join(", ")) })))
    }
}
