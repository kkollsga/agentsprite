//! `load` / `reload` and the high resolution references (DESIGN 5.1).

use crate::fsutil::PathSlash;
use crate::cli::{FrameSel, LoadArgs, ReloadArgs};
pub use crate::model::RefFrame;
use crate::model::{Frame, Grid, Method, Part, PartKind};
use crate::output::{Loc, Out, PxlError, Result};
use crate::status::StatusEvent;
use crate::{fsutil, lease, png, pxl, reduce, status, template};
use serde_json::json;
use std::path::{Path, PathBuf};

/// `pxl load`: references -> first reduction into every unlocked frame. `--new` creates the part first.
pub fn run_load(a: &LoadArgs, out: &mut Out) -> Result<()> {
    if let Some(name) = &a.new {
        let path = create_part(a, name)?;
        out.line(format!("created {}", path.slash()));
        out.set("created", json!(path.slash()));
        let part = pxl::load(&path)?;
        if part.template.is_some() && template::locate(&part).is_ok() {
            return reduce_parts(&[path], &a.sel, a.force, out, true, parse_method(a.method.as_deref())?, a.set_method);
        }
        out.line("no template yet: set `template` in the part, then run `pxl load`");
        return Ok(());
    }
    if a.parts.is_empty() {
        return Err(PxlError::usage("no part given").with_hint("pxl load parts/player.pxl   or   pxl load --new NAME --input IMG --size 32x32 --origin 16,16 --template T"));
    }
    let paths = fsutil::expand_parts(&a.parts)?;
    for p in &paths {
        status::refuse_if_approved(p, a.force_approved)?;
    }
    reduce_parts(&paths, &a.sel, a.force, out, true, parse_method(a.method.as_deref())?, a.set_method)
}

fn parse_method(m: Option<&str>) -> Result<Option<Method>> {
    match m {
        None => Ok(None),
        Some(s) => Method::parse(s).map(Some).ok_or_else(|| PxlError::usage(format!("unknown method '{}'", s)).with_hint("methods: vote, nearest, contour, exemplar, auto")),
    }
}

/// `pxl reload`: regenerate unlocked frames after input, template or reduce changes. `--force` also
/// regenerates locked frames (their hand edits are lost); `--frames a,b` limits the frames.
pub fn run_reload(a: &ReloadArgs, out: &mut Out) -> Result<()> {
    if a.parts.is_empty() {
        return Err(PxlError::usage("no part given").with_hint("pxl reload parts/player.pxl [--frames 01_stand] [--force]"));
    }
    let paths = fsutil::expand_parts(&a.parts)?;
    if !a.try_methods.is_empty() {
        for p in &paths {
            reduce::trial::run_try(p, a, &a.sel, out)?;
        }
        return Ok(());
    }
    let ov = parse_method(a.method.as_deref())?;
    for p in &paths {
        status::refuse_if_approved(p, a.force_approved)?;
    }
    reduce_parts(&paths, &a.sel, a.force, out, false, ov, a.set_method)
}

pub(crate) fn reduce_parts(paths: &[PathBuf], sel: &FrameSel, force: bool, out: &mut Out, is_load: bool, ov: Option<Method>, set_method: bool) -> Result<()> {
    let mut results = Vec::new();
    let mut first_err: Option<PxlError> = None;
    for path in paths {
        match reduce_one(path, sel, force, out, ov, set_method) {
            Ok((done, locked_skipped)) => {
                let verb = if is_load { "loaded" } else { "reloaded" };
                let skip = if locked_skipped.is_empty() {
                    String::new()
                } else {
                    format!(", {} locked frame(s) kept: {} (--force regenerates them and loses their hand edits; `pxl unlock` first to keep the part unlocked)", locked_skipped.len(), locked_skipped.join(" "))
                };
                out.line(format!("{}: {} {} frame(s){}", path.slash(), verb, done.len(), skip));
                results.push(json!({"part": path.slash(), "frames": done, "locked_kept": locked_skipped}));
            }
            Err(e) => {
                out.line(format!("{}: error: {}", path.slash(), e.msg));
                results.push(json!({"part": path.slash(), "error": e.to_json()}));
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }
    }
    out.set("parts", json!(results));
    match first_err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// Reduce the selected frames of one part and save it. Returns (reduced frame names, locked frames kept).
fn reduce_one(path: &Path, sel: &FrameSel, force: bool, out: &mut Out, ov: Option<Method>, set_method: bool) -> Result<(Vec<String>, Vec<String>)> {
    lease::guard(path, out.agent())?;
    let mut part = pxl::load(path)?;
    let stored_method = part.reduce.method;
    if let Some(m) = ov {
        part.reduce.method = m;
    }
    let tpl = template::load_for(&part)?;
    let dir = part.dir();
    let method = part.reduce.method;
    let bank = if matches!(method, Method::Exemplar | Method::Auto) { Some(reduce::exemplar::Bank::build(&part, &tpl, None)?) } else { None };
    if let Some(b) = &bank {
        for w in &b.warnings {
            out.warn(w.clone());
        }
    }
    let explicit = !(sel.all || sel.frames.is_empty());
    let selected: Vec<String> = sel.select(&part)?.iter().map(|f| f.name.clone()).collect();
    let mut todo: Vec<usize> = Vec::new();
    let mut kept = Vec::new();
    for n in &selected {
        let i = part.frame_index(n).unwrap();
        if part.frames[i].locked && !force {
            kept.push(n.clone());
        } else {
            todo.push(i);
        }
    }
    let _ = explicit;
    // --force regenerates the todo frames, so their stored keys do not matter; every other frame must be valid
    template::validate_grid_skipping(&tpl, &part, if force { &todo } else { &[] })?;
    let tile_mode = matches!(part.kind, PartKind::Tileset | PartKind::Screen) && !tpl.palettes.is_empty();
    let results = fsutil::par_map(&todo, |&i| -> Result<Grid> {
        if tile_mode {
            return crate::tiles::reduce::reduce_frame(&part, &tpl, i);
        }
        let f = &part.frames[i];
        let rf = reference(&part, &dir, f)?;
        reduce::reduce_named(&rf.img, rf.origin, &part, &tpl, if f.locked { Some(f.name.as_str()) } else { None }, method, bank.as_ref()).map_err(|e| {
            if e.loc.is_none() {
                e.with_loc(Loc::file(part.path.slash()).frame(f.name.clone()))
            } else {
                e
            }
        })
    });
    let mut grids = Vec::new();
    for r in results {
        grids.push(r?);
    }
    let mut done = Vec::new();
    if tile_mode {
        if let Some(g0) = grids.first() {
            let size = (g0.w, g0.h);
            if let Some(f) = part.frames.iter().enumerate().find(|(i, f)| !todo.contains(i) && (f.grid.w, f.grid.h) != size) {
                return Err(PxlError::validation(format!("locked frame {} is {}x{} but the blocks need {}x{}", f.1.name, f.1.grid.w, f.1.grid.h, size.0, size.1))
                    .with_loc(Loc::file(part.path.slash()))
                    .with_hint("reload with --force to regenerate locked frames"));
            }
            part.size = size;
        }
    }
    for (&i, g) in todo.iter().zip(grids) {
        part.frames[i].grid = g;
        if force {
            part.frames[i].locked = false;
        }
        done.push(part.frames[i].name.clone());
    }
    template::validate_grid(&tpl, &part)?;
    if !set_method {
        part.reduce.method = stored_method;
    }
    pxl::save(path, &part)?;
    status::record(path, &part, StatusEvent::Loaded);
    status::record(path, &part, StatusEvent::RawFrames(todo.clone()));
    crate::journal::append_load(path, &done);
    Ok((done, kept))
}

fn rel_to(base_dir: &Path, p: &Path) -> String {
    let abs = |q: &Path| std::fs::canonicalize(q).unwrap_or_else(|_| q.to_path_buf());
    let (b, q) = (abs(base_dir), abs(p));
    match q.strip_prefix(&b) {
        Ok(r) => r.slash(),
        Err(_) => {
            // try one parent up ("../refs/x.png")
            if let Some(par) = b.parent() {
                if let Ok(r) = q.strip_prefix(par) {
                    return format!("../{}", r.slash());
                }
            }
            q.slash()
        }
    }
}

/// `load --new`: write a new part file; frames come from the input (an image, a folder of PNGs, or a
/// `{frame}` pattern is used as is with no frames). Never overwrites an existing part.
fn create_part(a: &LoadArgs, name: &str) -> Result<PathBuf> {
    let out_path = PathBuf::from(a.out.clone().unwrap_or_else(|| format!("{}.pxl", name)));
    if out_path.exists() {
        return Err(PxlError::usage(format!("{} already exists", out_path.slash())).with_hint("choose another --out, or delete the file first"));
    }
    let part_dir = match out_path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let mut part = Part::new(name, (0, 0));
    part.path = out_path.clone();
    part.profile = a.profile.clone();
    part.template = a.template.clone();
    let input = a.input.clone().ok_or_else(|| PxlError::usage("--new needs --input (an image, a folder of PNGs or a pattern with {frame})"))?;
    let mut imgs: Vec<(String, PathBuf)> = Vec::new();
    let ip = Path::new(&input);
    if input.contains("{frame}") {
        part.input.images = Some(fsutil::norm_slash(&input));
    } else if ip.is_dir() {
        let mut v: Vec<PathBuf> = std::fs::read_dir(ip)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().map_or(false, |e| e.eq_ignore_ascii_case("png")))
            .collect();
        v.sort();
        if v.is_empty() {
            return Err(PxlError::usage(format!("no PNG files in {}", ip.slash())));
        }
        let first_dir = rel_to(&part_dir, ip);
        part.input.images = Some(format!("{}/{{frame}}.png", first_dir.trim_end_matches('/')));
        for p in v {
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("frame").to_string();
            imgs.push((stem, p));
        }
    } else if ip.is_file() {
        let stem = ip.file_stem().and_then(|s| s.to_str()).unwrap_or("frame").to_string();
        imgs.push((stem, ip.to_path_buf()));
    } else {
        return Err(PxlError::usage(format!("input '{}' not found", input)).with_hint("give an image, a folder of PNGs, or a pattern like refs/{frame}.png"));
    }
    // factor and size
    let first_img = match imgs.first() {
        Some((_, p)) => Some(png::read_rgba(p)?),
        None => None,
    };
    let factor = a.factor;
    let size = match (a.size, &first_img, factor) {
        (Some(s), _, _) => (s.w, s.h),
        (None, Some(im), Some(f)) => ((im.w + f as usize - 1) / f as usize, (im.h + f as usize - 1) / f as usize),
        _ => return Err(PxlError::usage("--new needs --size WxH (or --factor with an image input)")),
    };
    part.size = size;
    let f = match (factor, &first_img) {
        (Some(f), _) => f.max(1),
        (None, Some(im)) => (((im.w + size.0 / 2) / size.0.max(1)).max(1)) as u32,
        (None, None) => 1,
    };
    part.input.factor = Some(f);
    part.reduce.factor = Some(f);
    part.reduce.thin = true;
    part.origin = a.origin.map_or((size.0 as i32 / 2, size.1 as i32 / 2), |o| (o.x, o.y));
    for (n, p) in imgs {
        let src = if part.input.images.is_some() { None } else { Some(rel_to(&part_dir, &p)) };
        part.frames.push(Frame { name: n, src, locked: false, grid: Grid::new(size.0, size.1, b'.') });
    }
    if let Some(d) = out_path.parent() {
        if !d.as_os_str().is_empty() {
            std::fs::create_dir_all(d)?;
        }
    }
    pxl::save(&out_path, &part)?;
    Ok(out_path)
}

/// High resolution reference for one frame.
///
/// * `input.images` pattern (`refs/{frame}.png`, `{frame}` = the frame's `src` or else its name), or a
///   `src` that itself ends in `.png`: read the PNG. Origin = `input.ref_origin`, else `part.origin * factor`
///   (factor = `input.factor`, else `reduce.factor`, else 1).
/// * otherwise: an error naming the missing input.
pub fn reference(part: &Part, part_dir: &Path, frame: &Frame) -> Result<RefFrame> {
    let mut rf = reference_base(part, part_dir, frame)?;
    // A frame's own `phase` is folded into the reference origin, so every consumer (reduce, metrics, show,
    // compare) that combines `origin` with `[reduce] phase` sees the frame's phase without knowing about it.
    let own = part.phase_for(&frame.name);
    let global = part.reduce.phase.unwrap_or((0, 0));
    rf.origin = (rf.origin.0 + own.0 - global.0, rf.origin.1 + own.1 - global.1);
    Ok(rf)
}

/// The explicit `reference_missing` warning text (no silent skips): names the part and the frame, says why, and lists what could not run.
pub fn reference_missing_text(part: &str, frame: &str, why: &str, skipped: &[&str]) -> String {
    let what = if skipped.is_empty() { String::new() } else { format!("; not checked without it: {}", skipped.join(", ")) };
    format!("reference_missing: part {} frame {}: the source image is unavailable ({}){}", part, frame, why, what)
}

fn reference_base(part: &Part, part_dir: &Path, frame: &Frame) -> Result<RefFrame> {
    let label = frame.src.as_deref().unwrap_or(&frame.name);
    let img_path = if label.ends_with(".png") {
        Some(label.to_string())
    } else {
        part.input.images.as_ref().map(|pat| pat.replace("{frame}", label))
    };
    if let Some(rel) = img_path {
        let p = fsutil::find_up(part_dir, &rel).unwrap_or_else(|| part_dir.join(&rel));
        if !p.is_file() {
            return Err(PxlError::io(format!("reference image {} not found", p.slash()))
                .with_loc(Loc::file(part.path.slash()).frame(frame.name.clone()))
                .with_hint("check input.images and the frame's src"));
        }
        let img = png::read_rgba(&p)?;
        let f = part.input.factor.or(part.reduce.factor).unwrap_or(1) as i32;
        let origin = part.input.ref_origin.unwrap_or((part.origin.0 * f, part.origin.1 * f));
        return Ok(RefFrame { img, origin });
    }
    Err(PxlError::validation("part has no reference image (set [input] images, or give the frame a `src` that ends in .png)")
        .with_loc(Loc::file(part.path.slash()).frame(frame.name.clone()))
        .with_hint("[input]  images = \"refs/{frame}.png\""))
}
