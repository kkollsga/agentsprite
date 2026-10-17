//! Versions of an asset (M6 R4). Every write of `asset.pxl` (any command: edit, import, fit, reduce, undo, revert, ingest)
//! goes through `fsutil::atomic_write`, which calls `after_write` here. A store creates
//!
//! * `versions/<name>_vNNN.pxl`: the exact file text (a snapshot `revert` can restore),
//! * `versions/<name>_vNNN.png`: a sheet of all frames, native pixels, transparent,
//! * `compare.png`: input | latest (or latest only without an input), and `compare.html`,
//!
//! then, inside a project, prints the scene budget delta and rebuilds the asset's export. A write whose text equals the
//! latest snapshot stores nothing. `--no-version` (or `PXL_NO_VERSION=1`) switches the whole step off for bulk work.

use crate::fsutil::PathSlash;
use crate::asset;
use crate::cli::{RevertArgs, VersionsArgs};
use crate::compare;
use crate::img::{Rgba, RgbaImage};
use crate::manifest::Manifest;
use crate::model::Part;
use crate::output::{Out, PxlError, Result};
use crate::{fsutil, png, pxl, template};
use serde_json::json;
use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

static DISABLED: AtomicBool = AtomicBool::new(false);
static NOTES: Mutex<Vec<String>> = Mutex::new(Vec::new());

thread_local! {
    static SUPPRESS: Cell<u32> = const { Cell::new(0) };
}

pub fn disable() {
    DISABLED.store(true, Ordering::SeqCst);
}

fn off() -> bool {
    DISABLED.load(Ordering::SeqCst) || std::env::var("PXL_NO_VERSION").map_or(false, |v| !v.is_empty() && v != "0") || SUPPRESS.with(|s| s.get() > 0)
}

/// Run `f` without storing versions (creating an asset, internal bulk writes).
pub fn suppressed<T>(f: impl FnOnce() -> T) -> T {
    SUPPRESS.with(|s| s.set(s.get() + 1));
    let r = f();
    SUPPRESS.with(|s| s.set(s.get() - 1));
    r
}

fn note(s: String) {
    if let Ok(mut n) = NOTES.lock() {
        n.push(s);
    }
}

/// Lines the stores of this run printed (drained by `Out::render`).
pub fn take_notes() -> Vec<String> {
    NOTES.lock().map(|mut n| std::mem::take(&mut *n)).unwrap_or_default()
}

pub struct Version {
    pub n: u32,
    pub png: PathBuf,
    pub pxl: PathBuf,
    pub ts: u64,
}

fn vname(path: &Path) -> String {
    let base = asset::asset_name(&asset::dir_of(path));
    match asset::variant_of(path) {
        Some(v) => format!("{}.{}", base, v),
        None => base,
    }
}

fn vdir(path: &Path) -> PathBuf {
    asset::dir_of(path).join("versions")
}

/// Stored versions of the asset file `path`, oldest first.
pub fn list(path: &Path) -> Vec<Version> {
    let name = vname(path);
    let prefix = format!("{}_v", name);
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir(vdir(path)) {
        for e in rd.flatten() {
            let f = e.file_name().to_string_lossy().into_owned();
            if let Some(num) = f.strip_prefix(&prefix).and_then(|r| r.strip_suffix(".pxl")) {
                if let Ok(n) = num.parse::<u32>() {
                    let ts = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
                    v.push(Version { n, png: e.path().with_extension("png"), pxl: e.path(), ts });
                }
            }
        }
    }
    v.sort_by_key(|x| x.n);
    v
}

pub fn latest(path: &Path) -> Option<Version> {
    list(path).pop()
}

fn file_for(path: &Path, n: u32, ext: &str) -> PathBuf {
    vdir(path).join(format!("{}_v{:03}.{}", vname(path), n, ext))
}

/// Sheet of all frames of a part: native pixels, transparent background, 2 px gaps.
pub fn render_sheet(part: &Part) -> RgbaImage {
    let tpl = template::load_for(part).ok();
    let n = part.frames.len().max(1);
    let (w, h) = part.frames.first().map_or((1, 1), |f| (f.grid.w.max(1), f.grid.h.max(1)));
    let mut cols = (n as f64).sqrt().ceil() as usize;
    while cols > 1 && cols * (w + 2) > 1024 {
        cols -= 1;
    }
    let rows = (n + cols - 1) / cols;
    let mut im = RgbaImage::new(cols * (w + 2) - 2, rows * (h + 2) - 2);
    for (i, f) in part.frames.iter().enumerate() {
        let (ox, oy) = ((i % cols) * (w + 2), (i / cols) * (h + 2));
        for y in 0..f.grid.h.min(h) {
            for x in 0..f.grid.w.min(w) {
                let k = f.grid.cells[y * f.grid.w + x];
                if k != b'.' {
                    let c = compare::key_color(part, tpl.as_ref(), k);
                    im.px[(oy + y) * im.w + ox + x] = Rgba::opaque(c.r, c.g, c.b);
                }
            }
        }
    }
    im
}

/// The images in `input/` (PNG, files directly in the folder), sorted. The first is the comparison reference.
pub fn input_images(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir.join("input"))
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_file() && p.extension().map_or(false, |e| e.eq_ignore_ascii_case("png"))).collect())
        .unwrap_or_default();
    v.sort();
    v
}

fn frame_image(part: &Part, tpl: Option<&crate::template::Template>, fi: usize) -> RgbaImage {
    let f = &part.frames[fi];
    let mut im = RgbaImage::new(f.grid.w.max(1), f.grid.h.max(1));
    for y in 0..f.grid.h {
        for x in 0..f.grid.w {
            let k = f.grid.cells[y * f.grid.w + x];
            if k != b'.' {
                let c = compare::key_color(part, tpl, k);
                im.px[y * im.w + x] = Rgba::opaque(c.r, c.g, c.b);
            }
        }
    }
    im
}

/// The input of a frame aligned with the part canvas at the asset's factor: pixel (x, y) of the part is the `f` x `f` block at
/// (x * f, y * f) of the result, around the origin.
fn input_panel(part: &Part, dir: &Path, fi: usize, f: usize) -> Option<RgbaImage> {
    let frame = &part.frames[fi];
    let rf = crate::load::reference(part, dir, frame).ok()?;
    let (w, h) = (frame.grid.w * f, frame.grid.h * f);
    let (ox, oy) = (rf.origin.0 - part.origin.0 * f as i32, rf.origin.1 - part.origin.1 * f as i32);
    let mut im = RgbaImage::new(w.max(1), h.max(1));
    for y in 0..h {
        for x in 0..w {
            im.px[y * w + x] = rf.img.get(ox + x as i32, oy + y as i32);
        }
    }
    Some(im)
}

const WHITE: Rgba = Rgba::opaque(0xf0, 0xf0, 0xf0);
const BACK: Rgba = Rgba::opaque(0x30, 0x30, 0x34);
const MAX_W: usize = 2048;

/// Typical background of the asset's console (`preview_bg` of console.toml, or `[project.settings] preview_bg`).
fn preview_bg(dir: &Path, part: &Part) -> Rgba {
    let mut hex = None;
    if let Some(mp) = Manifest::find_file(dir) {
        hex = crate::project::project_table(&mp).get("settings").and_then(|s| s.as_table()).and_then(|s| s.get("preview_bg")).and_then(|v| v.as_str()).map(String::from);
    }
    let cid = part.console.clone().unwrap_or_else(|| crate::console::project_console(dir).id);
    let hex = hex.or_else(|| crate::console::info(&cid, Some(dir)).ok().map(|c| c.preview_bg).filter(|b| !b.is_empty()));
    hex.and_then(|h| crate::color::parse_hex(&h).ok()).map_or(Rgba::opaque(0xd0, 0xd0, 0xd0), |c| Rgba::opaque(c.r, c.g, c.b))
}

fn fit_width(im: RgbaImage, max: usize) -> RgbaImage {
    if im.w <= max {
        return im;
    }
    let h = (im.h * max / im.w).max(1);
    let mut o = RgbaImage::new(max, h);
    for y in 0..h {
        for x in 0..max {
            o.px[y * max + x] = im.px[(y * im.h / h).min(im.h - 1) * im.w + (x * im.w / max).min(im.w - 1)];
        }
    }
    o
}

/// `compare.png`: for every frame the input | the latest side by side at the same scale (the latest is enlarged by the asset's
/// factor with nearest neighbour so the pixels line up, plus an integer zoom for small sprites: at least 128 px tall), a label
/// per frame and a header with name, version, console / mode. Frames without an input are shown alone. Cells wrap at 2048 px.
/// `compare_1x.png`: the latest frames at 1x on the console's typical background.
pub fn write_compare(dir: &Path, part: &Part) -> Result<()> {
    let tpl = template::load_for(part).ok();
    let f = part.input.factor.or(part.reduce.factor).unwrap_or(1).max(1) as usize;
    let file = dir.join(asset::ASSET_FILE);
    let ver = latest(&file).map_or(0, |v| v.n);
    let cid = part.console.clone().unwrap_or_else(|| crate::console::project_console(dir).id);
    let inputs: Vec<Option<RgbaImage>> = (0..part.frames.len()).map(|i| input_panel(part, dir, i, f)).collect();
    let any_input = inputs.iter().any(|i| i.is_some());
    let mut cells: Vec<RgbaImage> = Vec::new();
    for (fi, fr) in part.frames.iter().enumerate() {
        let latest_im = frame_image(part, tpl.as_ref(), fi);
        let ph = latest_im.h * f;
        let z = if ph >= 128 { 1 } else { ((128 + ph - 1) / ph).min(8) };
        let right = over_checker_sized(&latest_im.upscale(f * z), 8);
        let left = inputs[fi].as_ref().map(|i| over_checker_sized(&i.upscale(z), 8));
        let gap = if left.is_some() { 4 } else { 0 };
        let pw = left.as_ref().map_or(0, |l| l.w + gap) + right.w;
        let label = format!("{} {}x{}", fr.name, latest_im.w, latest_im.h);
        let cw = pw.max(compare::text_w(&label, 2) + 4);
        let mut c = RgbaImage::filled(cw, right.h + 14, BACK);
        let mut x = 0;
        if let Some(l) = &left {
            compare::paste(&mut c, l, 0, 14);
            x = l.w + gap;
        }
        compare::paste(&mut c, &right, x, 14);
        compare::draw_text(&mut c, 1, 2, &label, 2, WHITE);
        cells.push(fit_width(c, MAX_W - 16));
    }
    let header = format!("{} v{:03}  {}/{}  x{}  {}", asset::asset_name(dir), ver, cid, part.kind.mode_name(), f, if any_input { "input | latest" } else { "latest" });
    let mut rows: Vec<(usize, Vec<&RgbaImage>)> = Vec::new();
    let (mut x, mut cur): (usize, Vec<&RgbaImage>) = (8, Vec::new());
    for c in &cells {
        if x + c.w + 8 > MAX_W && !cur.is_empty() {
            rows.push((cur.iter().map(|i| i.h).max().unwrap_or(0), std::mem::take(&mut cur)));
            x = 8;
        }
        x += c.w + 8;
        cur.push(c);
    }
    if !cur.is_empty() {
        rows.push((cur.iter().map(|i| i.h).max().unwrap_or(0), cur));
    }
    let width = rows.iter().map(|(_, r)| r.iter().map(|c| c.w + 8).sum::<usize>() + 8).max().unwrap_or(0).max(compare::text_w(&header, 2) + 16);
    let height = 20 + rows.iter().map(|(h, _)| h + 8).sum::<usize>() + 4;
    let mut cv = RgbaImage::filled(width, height, Rgba::opaque(0x22, 0x22, 0x26));
    compare::draw_text(&mut cv, 8, 5, &header, 2, WHITE);
    let mut y = 20;
    for (h, r) in &rows {
        let mut x = 8;
        for c in r {
            compare::paste(&mut cv, c, x, y);
            x += c.w + 8;
        }
        y += h + 8;
    }
    fsutil::atomic_write_plain(&dir.join("compare.png"), &compare::encode_sheet_png(&cv))?;
    // the real size
    let bg = preview_bg(dir, part);
    let n = part.frames.len().max(1);
    let (fw, fh) = part.frames.first().map_or((1, 1), |f| (f.grid.w.max(1), f.grid.h.max(1)));
    let pad = 4;
    let cols = (512 / (fw + pad)).max(1).min(n);
    let rws = (n + cols - 1) / cols;
    let mut real = RgbaImage::filled(cols * (fw + pad) + pad, rws * (fh + pad) + pad, bg);
    for fi in 0..part.frames.len() {
        let im = frame_image(part, tpl.as_ref(), fi);
        let (ox, oy) = (pad + (fi % cols) * (fw + pad), pad + (fi / cols) * (fh + pad));
        for y in 0..im.h.min(fh) {
            for x in 0..im.w.min(fw) {
                let p = im.px[y * im.w + x];
                if p.a > 0 {
                    real.px[(oy + y) * real.w + ox + x] = p;
                }
            }
        }
    }
    fsutil::atomic_write_plain(&dir.join("compare_1x.png"), &compare::encode_sheet_png(&real))?;
    Ok(())
}

fn over_checker_sized(im: &RgbaImage, size: usize) -> RgbaImage {
    let mut o = compare::checker(im.w, im.h, size);
    for (i, p) in im.px.iter().enumerate() {
        if p.a > 0 {
            o.px[i] = Rgba::opaque(p.r, p.g, p.b);
        }
    }
    o
}

pub struct Stored {
    pub n: u32,
}

/// Store `bytes` (the new text of `path`) as the next version. `None` when it equals the latest snapshot.
pub fn store(path: &Path, bytes: &[u8]) -> Result<Option<Stored>> {
    if let Some(l) = latest(path) {
        if std::fs::read(&l.pxl).map_or(false, |b| b == bytes) {
            return Ok(None);
        }
    }
    let n = latest(path).map_or(1, |l| l.n + 1);
    std::fs::create_dir_all(vdir(path))?;
    fsutil::atomic_write_plain(&file_for(path, n, "pxl"), bytes)?;
    let text = String::from_utf8_lossy(bytes).into_owned();
    if let Ok(part) = pxl::parse_str(&text, path) {
        // palette-set assets: the template always has a key for every colour of the set
        let _ = crate::fit::sync_set_keys(&part);
        fsutil::atomic_write_plain(&file_for(path, n, "png"), &png::encode_rgba(&render_sheet(&part)))?;
        if asset::variant_of(path).is_none() {
            write_compare(&asset::dir_of(path), &part)?;
        }
        crate::variants::refresh_sheet(&asset::dir_of(path));
        crate::tasks::user_approved_changed(path, n);
        crate::tasks::write_page(&asset::dir_of(path));
    }
    Ok(Some(Stored { n }))
}

/// Store the file's current text as a version and say so in the command output (the first version of a new asset).
pub fn store_noted(path: &Path) {
    if DISABLED.load(Ordering::SeqCst) {
        return;
    }
    let Ok(bytes) = std::fs::read(path) else { return };
    if let Ok(Some(s)) = store(path, &bytes) {
        note(format!("stored {} v{:03}", asset::name_of(path), s.n));
    }
}

/// Called by `fsutil::atomic_write` after an `asset.pxl` was written.
pub fn after_write(path: &Path, bytes: &[u8]) {
    if off() {
        return;
    }
    match store(path, bytes) {
        Ok(Some(s)) => {
            let name = asset::name_of(path);
            let mut lines = vec![format!("stored {} v{:03}", name, s.n)];
            lines.extend(project_hooks(path));
            note(lines.join("\n"));
        }
        Ok(None) => {}
        Err(e) => note(format!("warning: version of {} not stored: {}", path.slash(), e.msg)),
    }
}

// ----------------------------------------------------------------------------------- project hooks

fn project_hooks(path: &Path) -> Vec<String> {
    let mut v = Vec::new();
    let Some(mp) = Manifest::find_file(&asset::dir_of(path)) else { return v };
    let Ok(m) = Manifest::load(&mp) else { return v };
    let Some(d) = m.find(path) else { return v };
    if std::env::var("PXL_NO_DELTA").is_err() {
        v.extend(scene_delta(&m, &d.name));
    }
    if let Some(l) = export_asset(path) {
        v.push(l);
    }
    v
}

/// Rebuild the export of the asset at `path` (its manifest entry, else `<asset>/export/`). One status line.
pub fn export_asset(path: &Path) -> Option<String> {
    if std::env::var("PXL_NO_AUTOEXPORT").is_ok() {
        return None;
    }
    let m = Manifest::find_file(&asset::dir_of(path)).and_then(|mp| Manifest::load(&mp).ok());
    let (name, spec) = match m.as_ref().and_then(|m| m.find(path)) {
        Some(d) => (d.name.clone(), d.spec.clone()),
        None if m.is_some() => return None,
        None => (asset::name_of(path), crate::manifest::OutSpec::default_for(path)),
    };
    let job = crate::export::Job { part_path: path.to_path_buf(), spec: spec.clone(), frames: Vec::new(), agent: None, scratch: false };
    Some(match crate::export::export_part(&job) {
        Ok(r) => {
            crate::export::cleanup_stale(&[&r], &[&spec], m.as_ref());
            format!("export {}: {} file(s), {} written", name, r.files, r.written)
        }
        Err(e) => format!("export {} skipped: {}", name, e.msg.lines().next().unwrap_or("")),
    })
}

fn state_path(m: &Manifest) -> PathBuf {
    m.dir.join(".pxl").join("scene_state.json")
}

/// Scenes that contain the part: in a sprite group, as the tile set, or as a window part.
pub fn scenes_of<'a>(m: &'a Manifest, key: &str) -> Vec<&'a crate::manifest::Scene> {
    m.scenes
        .iter()
        .filter(|sc| {
            sc.tileset.as_deref() == Some(key)
                || sc.window.iter().any(|w| crate::cellmem::resolve_parts(m, w).iter().any(|d| d.name == key))
                || sc.groups.iter().any(|g| g.parts.iter().any(|p| crate::cellmem::resolve_parts(m, p).iter().any(|d| d.name == key)))
        })
        .collect()
}

/// "zone_1 sprite VRAM 165 -> 168 / 167 FAIL" lines for the scenes holding the part, against the last recorded state.
pub fn scene_delta(m: &Manifest, key: &str) -> Vec<String> {
    let scenes = scenes_of(m, key);
    if scenes.is_empty() {
        return Vec::new();
    }
    let Ok((sp, tp)) = crate::scene::hardware(m) else { return Vec::new() };
    let sp_path = state_path(m);
    let mut state: BTreeMap<String, BTreeMap<String, (String, String)>> = std::fs::read_to_string(&sp_path)
        .ok()
        .and_then(|t| serde_json::from_str::<BTreeMap<String, BTreeMap<String, [String; 2]>>>(&t).ok())
        .map(|s| s.into_iter().map(|(k, v)| (k, v.into_iter().map(|(a, [u, l])| (a, (u, l))).collect())).collect())
        .unwrap_or_default();
    let mut cache = std::collections::HashMap::new();
    let mut lines = Vec::new();
    for sc in scenes {
        let rep = crate::scene::check_scene(m, sc, &sp, &tp, &mut cache);
        let old = state.get(&sc.name).cloned();
        let mut cur = BTreeMap::new();
        for r in &rep.rows {
            cur.insert(r.item.clone(), (r.used.clone(), r.limit.clone()));
            let mark = if r.ok { "ok" } else { "FAIL" };
            match old.as_ref().and_then(|o| o.get(&r.item)) {
                Some((u, _)) if *u != r.used => lines.push(format!("{} {} {} -> {} / {} {}", sc.name, r.item, u, r.used, r.limit, mark)),
                None if !r.ok => lines.push(format!("{} {} {} / {} FAIL", sc.name, r.item, r.used, r.limit)),
                Some(_) | None => {
                    if !r.ok && old.is_some() {
                        lines.push(format!("{} {} {} / {} FAIL", sc.name, r.item, r.used, r.limit));
                    }
                }
            }
        }
        state.insert(sc.name.clone(), cur);
    }
    let ser: BTreeMap<&String, BTreeMap<&String, [&String; 2]>> = state.iter().map(|(k, v)| (k, v.iter().map(|(a, (u, l))| (a, [u, l])).collect())).collect();
    if let Ok(t) = serde_json::to_string_pretty(&ser) {
        let _ = std::fs::create_dir_all(sp_path.parent().unwrap());
        let _ = fsutil::atomic_write_plain(&sp_path, t.as_bytes());
    }
    lines
}

// ------------------------------------------------------------------------------------------ commands

pub fn run_versions(a: &VersionsArgs, out: &mut Out) -> Result<()> {
    let path = fsutil::resolve_part(&a.asset)?;
    let vs = list(&path);
    let mut text = format!("{}: {} version(s)\n", asset::name_of(&path), vs.len());
    let mut js = Vec::new();
    for v in &vs {
        text.push_str(&format!("  v{:03}  {}  {}\n", v.n, v.pxl.slash(), v.png.slash()));
        js.push(json!({"version": v.n, "pxl": v.pxl.slash(), "png": v.png.slash(), "ts": v.ts}));
    }
    out.emit(text.trim_end(), json!({"asset": asset::name_of(&path), "versions": js}));
    Ok(())
}

/// `pxl revert ASSET vNNN`: the text of that version becomes the file again, as a new version.
pub fn run_revert(a: &RevertArgs, out: &mut Out) -> Result<()> {
    let path = fsutil::resolve_part(&a.asset)?;
    let num: u32 = a.ver.trim_start_matches('v').trim_start_matches('V').parse().map_err(|_| PxlError::usage(format!("'{}' is not a version like v003", a.ver)))?;
    let vs = list(&path);
    let v = vs.iter().find(|v| v.n == num).ok_or_else(|| PxlError::usage(format!("{} has no version v{:03}", asset::name_of(&path), num)).with_hint("pxl versions ASSET lists them"))?;
    crate::status::refuse_if_approved(&path, a.force_approved)?;
    crate::lease::guard(&path, out.agent())?;
    let _pl = fsutil::lock_part(&path)?;
    let bytes = std::fs::read(&v.pxl)?;
    if std::fs::read(&path).map_or(false, |b| b == bytes) {
        out.emit(format!("{} already equals v{:03}", asset::name_of(&path), num), json!({"asset": asset::name_of(&path), "reverted_to": num, "new_version": null}));
        return Ok(());
    }
    fsutil::atomic_write(&path, &bytes)?;
    let now = latest(&path).map_or(0, |l| l.n);
    out.emit(format!("{} restored from v{:03} as v{:03}", asset::name_of(&path), num, now), json!({"asset": asset::name_of(&path), "reverted_to": num, "new_version": now}));
    Ok(())
}
