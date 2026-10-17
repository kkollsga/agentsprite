//! Delivery tools (M6 R6b): `pxl sheet`, `pxl package`, `pxl verify-package`, `pxl deliver`, `pxl changes`.
//!
//! * `sheet GROUP [--set S] [--on #rrggbb]`: a contact sheet of the latest frames of a group (or one asset) in a palette set, on a colour.
//! * `package`: assembles `package/` from the delivery manifest: the files of every part that is delivered (non-delivered variants and
//!   dropped frames stay out), the manifest, the configured spec listing and the scene palette files, plus `package.json` (every file
//!   with hash and size, the dropped frames with their reasons, the exceptions of the assets). Files an earlier package listed that the
//!   new one does not are removed, so no stale output survives.
//! * `verify-package [DIR]`: the checks of `scripts/verify_m5.py`, generalised to every console and driven by the manifest: files listed
//!   and present with the recorded hashes, no unlisted file, frame PNG size, console colours, every opaque pixel in a listed cell,
//!   sprites per scanline and per frame, palette tables (slots, colours, ids), spec listing. It counts what it checked.
//! * `deliver`: our scene check, the package, the verifier, then the client's checker and pipeline (output and images captured); it
//!   refuses on our failures, and snapshots to `deliveries/N/`.
//! * `changes --since N [--to M]`: what changed between two snapshots (or a snapshot and now): parts, frames, size, origin,
//!   footprint (collision), cells, palette variant and palette colours.

use crate::fsutil::PathSlash;
use crate::cli::{ChangesArgs, DeliverArgs, PackageArgs, SheetArgs, VerifyPackageArgs};
use crate::color::Rgb;
use crate::dmanifest;
use crate::fsutil;
use crate::img::{Rgba, RgbaImage};
use crate::manifest::Manifest;
use crate::output::{Out, PxlError, Result};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn manifest_of(explicit: &Option<String>) -> Result<Manifest> {
    Manifest::discover(explicit.as_deref())?.ok_or_else(|| PxlError::usage("this needs a project (pxl.toml); `pxl new` creates one"))
}

fn pkg_dir(m: &Manifest, arg: &Option<String>) -> PathBuf {
    let rel = arg.clone().or_else(|| dmanifest::delivery_cfg(m).package).unwrap_or_else(|| "package".into());
    let p = PathBuf::from(&rel);
    if p.is_absolute() {
        p
    } else {
        m.dir.join(p)
    }
}

fn hash(bytes: &[u8]) -> String {
    fsutil::content_hash(bytes)
}

// ------------------------------------------------------------------------------------------------ sheet

fn render_part(path: &Path, set: &Option<String>) -> Result<RgbaImage> {
    let part = crate::pxl::load(path)?;
    let tpl = crate::template::load_for(&part)?;
    let (part, tpl) = match set {
        Some(s) => crate::cells::recolour_part(part, tpl, s)?,
        None => (part, tpl),
    };
    let n = part.frames.len().max(1);
    let (w, h) = part.frames.iter().fold((1, 1), |a, f| (a.0.max(f.grid.w), a.1.max(f.grid.h)));
    let mut cols = (n as f64).sqrt().ceil() as usize;
    while cols > 1 && cols * (w + 2) > 512 {
        cols -= 1;
    }
    let rows = (n + cols - 1) / cols;
    let mut im = RgbaImage::new(cols * (w + 2), rows * (h + 2));
    for (i, f) in part.frames.iter().enumerate() {
        let (ox, oy) = ((i % cols) * (w + 2), (i / cols) * (h + 2));
        for y in 0..f.grid.h {
            for x in 0..f.grid.w {
                let k = f.grid.cells[y * f.grid.w + x];
                if k != b'.' {
                    let c = crate::compare::key_color(&part, Some(&tpl), k);
                    im.px[(oy + y) * im.w + ox + x] = Rgba::opaque(c.r, c.g, c.b);
                }
            }
        }
    }
    Ok(im)
}

pub fn run_sheet(a: &SheetArgs, out: &mut Out) -> Result<()> {
    let m = manifest_of(&a.manifest)?;
    let bg = crate::color::parse_hex(&a.on)?;
    let mut parts: Vec<&crate::manifest::PartDecl> = m.parts.iter().filter(|d| d.name == a.target || d.tags.iter().any(|t| *t == a.target)).collect();
    if parts.is_empty() {
        parts = m.parts.iter().filter(|d| d.path.slash().contains(&format!("/{}/", a.target))).collect();
    }
    if parts.is_empty() {
        return Err(PxlError::usage(format!("no group or asset '{}'", a.target)).with_hint("groups are the folders under assets/; `pxl group list`"));
    }
    parts.sort_by(|x, y| x.name.cmp(&y.name));
    let z = a.zoom.max(1);
    let mut panels: Vec<(String, RgbaImage)> = Vec::new();
    let mut skipped = Vec::new();
    for d in &parts {
        match render_part(&d.path, &a.set) {
            Ok(im) => panels.push((d.name.clone(), im)),
            Err(e) => skipped.push(format!("{}: {}", d.name, e.msg)),
        }
    }
    if panels.is_empty() {
        return Err(PxlError::validation(format!("nothing to draw: {}", skipped.join("; "))));
    }
    // rows of panels, wrapped at about 1200 px
    let label_h = 12;
    let (mut x, mut y, mut row_h, mut max_w) = (6usize, 6usize, 0usize, 0usize);
    let mut placed = Vec::new();
    for (name, im) in &panels {
        let (w, h) = ((im.w * z).max(name.len() * 6 + 4), im.h * z + label_h);
        if x + w > 1200 && x > 6 {
            x = 6;
            y += row_h + 8;
            row_h = 0;
        }
        placed.push((x, y, w, h));
        x += w + 8;
        row_h = row_h.max(h);
        max_w = max_w.max(x);
    }
    let mut sheet = RgbaImage::filled(max_w + 4, y + row_h + 10, Rgba::opaque(bg.r, bg.g, bg.b));
    let lum = crate::color::srgb_to_oklab(bg).l;
    let text = if lum > 0.6 { Rgba::opaque(20, 20, 30) } else { crate::compare::TEXT };
    for ((name, im), (px, py, _, _)) in panels.iter().zip(&placed) {
        crate::compare::draw_text(&mut sheet, *px, *py, name, 1, text);
        for yy in 0..im.h {
            for xx in 0..im.w {
                let p = im.px[yy * im.w + xx];
                if p.a > 0 {
                    crate::compare::fill(&mut sheet, px + xx * z, py + label_h + yy * z, z, z, p);
                }
            }
        }
    }
    let file = a.out.clone().unwrap_or_else(|| format!("{}_sheet.png", a.target));
    crate::show::write_png(Path::new(&file), &sheet)?;
    out.emit(format!("wrote {}: {} asset(s) on {}{}{}", file, panels.len(), a.on, a.set.as_ref().map(|s| format!(", set {}", s)).unwrap_or_default(), if skipped.is_empty() { String::new() } else { format!("; skipped {}", skipped.join("; ")) }), json!({"file": file, "assets": panels.iter().map(|p| p.0.clone()).collect::<Vec<_>>(), "skipped": skipped, "on": a.on}));
    Ok(())
}

// ------------------------------------------------------------------------------------------------ package

fn rel_to_base(m: &Manifest, key: &str) -> String {
    let base = dmanifest::out_base(m);
    let p = m.dir.join(key);
    p.strip_prefix(&base).map(|r| r.to_string_lossy().replace('\\', "/")).unwrap_or_else(|_| key.to_string())
}

/// The files a package holds: (package path, source file).
fn package_files(m: &Manifest, man: &Value) -> Vec<(String, PathBuf)> {
    let mut v: BTreeMap<String, PathBuf> = BTreeMap::new();
    for p in man["parts"].as_array().into_iter().flatten() {
        // a non-delivered variant is not delivered
        if p["tags"].as_array().map_or(false, |t| t.iter().any(|x| x == "variant")) {
            continue;
        }
        for f in p["frames"].as_array().into_iter().flatten().filter(|f| f["dropped"] != true) {
            for file in f["files"].as_array().into_iter().flatten().filter_map(|x| x.as_str()) {
                v.insert(rel_to_base(m, file), m.dir.join(file));
            }
        }
    }
    if let Ok(list) = crate::scene::palette_outputs(m) {
        for (_, path, _) in list {
            if let Ok(r) = path.strip_prefix(dmanifest::out_base(m)) {
                v.insert(r.to_string_lossy().replace('\\', "/"), path);
            }
        }
    }
    v.into_iter().collect()
}

fn exceptions_of(m: &Manifest) -> Vec<Value> {
    let mut out = Vec::new();
    for d in &m.parts {
        let dir = crate::asset::dir_of(&d.path);
        for l in std::fs::read_to_string(dir.join("exceptions.md")).unwrap_or_default().lines().filter(|l| l.starts_with("- ")) {
            out.push(json!({"asset": d.name, "exception": l.trim_start_matches("- ")}));
        }
    }
    out
}

pub fn assemble(m: &Manifest, dir: &Path) -> Result<(Value, Vec<String>)> {
    if !dmanifest::enabled(m) {
        return Err(PxlError::validation("this project does not write a delivery manifest").with_hint("[project] manifest = true in pxl.toml, then `pxl build`"));
    }
    let stale = dmanifest::stale(m);
    if !stale.is_empty() {
        return Err(PxlError::validation(format!("the delivery manifest is not current: {}", stale[0])).with_hint("run `pxl build` first"));
    }
    let man: Value = serde_json::from_slice(&std::fs::read(dmanifest::manifest_file(m))?).map_err(|e| PxlError::validation(format!("manifest.json: {}", e)))?;
    let mut files = package_files(m, &man);
    // the manifest and the spec listing go with the package
    files.push(("manifest.json".into(), dmanifest::manifest_file(m)));
    if let Some(s) = dmanifest::delivery_cfg(m).spec {
        files.push((Path::new(&s).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "spec.json".into()), m.dir.join(s)));
    }
    let mut missing = Vec::new();
    let mut listed: Vec<Value> = Vec::new();
    std::fs::create_dir_all(dir)?;
    let old: BTreeSet<String> = std::fs::read_to_string(dir.join("package.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).map(|v| v["files"].as_array().into_iter().flatten().filter_map(|f| f["path"].as_str().map(crate::fsutil::norm_slash)).collect()).unwrap_or_default();
    let mut now: BTreeSet<String> = BTreeSet::new();
    for (rel, src) in &files {
        let Ok(bytes) = std::fs::read(src) else {
            missing.push(format!("{} (from {})", rel, src.slash()));
            continue;
        };
        let dst = dir.join(rel);
        if let Some(p) = dst.parent() {
            std::fs::create_dir_all(p)?;
        }
        if std::fs::read(&dst).ok().as_deref() != Some(&bytes) {
            std::fs::write(&dst, &bytes)?;
        }
        now.insert(rel.clone());
        listed.push(json!({"path": rel, "hash": hash(&bytes), "size": bytes.len()}));
    }
    // no stale outputs: files an earlier package listed that this one does not, and any other file in the folder (it is ours)
    let mut removed = Vec::new();
    let mut on_disk: Vec<String> = Vec::new();
    fn walk(d: &Path, base: &Path, out: &mut Vec<String>) {
        if let Ok(rd) = std::fs::read_dir(d) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, base, out);
                } else if let Ok(r) = p.strip_prefix(base) {
                    out.push(r.to_string_lossy().replace('\\', "/"));
                }
            }
        }
    }
    walk(dir, dir, &mut on_disk);
    let extra: Vec<String> = on_disk.into_iter().filter(|f| f != "package.json" && !now.contains(f) && !old.contains(f)).collect();
    for o in old.difference(&now).chain(extra.iter()) {
        let p = dir.join(o);
        if p.is_file() && std::fs::remove_file(&p).is_ok() {
            removed.push(o.clone());
            let mut d = p.parent().map(|x| x.to_path_buf());
            while let Some(dd) = d {
                if dd == dir || std::fs::remove_dir(&dd).is_err() {
                    break;
                }
                d = dd.parent().map(|x| x.to_path_buf());
            }
        }
    }
    let dropped: Vec<Value> = man["parts"].as_array().into_iter().flatten().flat_map(|p| p["frames"].as_array().into_iter().flatten().filter(|f| f["dropped"] == true).map(|f| json!({"part": p["name"], "frame": f["name"], "reason": f["reason"]})).collect::<Vec<_>>()).collect();
    let info = json!({"schema": 1, "console": man["console"], "files": listed, "dropped_frames": dropped, "exceptions": exceptions_of(m)});
    fsutil::atomic_write_plain(&dir.join("package.json"), (serde_json::to_string_pretty(&info).unwrap() + "\n").as_bytes())?;
    let mut note = removed;
    note.sort();
    Ok((json!({"dir": dir.slash(), "files": listed.len(), "dropped_frames": dropped.len(), "removed": note, "missing": missing}), missing))
}

pub fn run_package(a: &PackageArgs, out: &mut Out) -> Result<()> {
    let m = manifest_of(&a.manifest)?;
    let dir = pkg_dir(&m, &a.out);
    let (j, missing) = assemble(&m, &dir)?;
    out.line(format!("package {}: {} file(s), {} dropped frame(s) left out, {} stale file(s) removed", dir.slash(), j["files"], j["dropped_frames"], j["removed"].as_array().map_or(0, |r| r.len())));
    for x in &missing {
        out.line(format!("missing {}", x));
    }
    out.set("package", j);
    if missing.is_empty() {
        Ok(())
    } else {
        Err(PxlError::validation(format!("{} listed file(s) are not exported (run `pxl build`)", missing.len())))
    }
}

// ------------------------------------------------------------------------------------------------ verify-package

pub struct Verdict {
    pub problems: Vec<String>,
    pub files: usize,
    pub frames: usize,
    pub parts: usize,
    pub of_parts: usize,
    pub palettes: usize,
}

pub fn verify(dir: &Path) -> Result<Verdict> {
    let mut v = Verdict { problems: Vec::new(), files: 0, frames: 0, parts: 0, of_parts: 0, palettes: 0 };
    let info: Value = match std::fs::read_to_string(dir.join("package.json")).ok().and_then(|t| serde_json::from_str(&t).ok()) {
        Some(x) => x,
        None => {
            v.problems.push("package.json is missing: not a package made by `pxl package`".into());
            return Ok(v);
        }
    };
    let man: Value = match std::fs::read_to_string(dir.join("manifest.json")).ok().and_then(|t| serde_json::from_str(&t).ok()) {
        Some(x) => x,
        None => {
            v.problems.push("manifest.json is missing from the package".into());
            return Ok(v);
        }
    };
    // 1. the listed files are present and unchanged, nothing else is there
    let mut listed: BTreeSet<String> = BTreeSet::new();
    for f in info["files"].as_array().into_iter().flatten() {
        let p = f["path"].as_str().unwrap_or("");
        listed.insert(p.to_string());
        v.files += 1;
        match std::fs::read(dir.join(p)) {
            Ok(b) if hash(&b) == f["hash"].as_str().unwrap_or("") => {}
            Ok(_) => v.problems.push(format!("files: {} differs from the recorded hash (edited after packaging)", p)),
            Err(_) => v.problems.push(format!("files: {} is listed but missing", p)),
        }
    }
    fn walk(d: &Path, base: &Path, out: &mut Vec<String>) {
        if let Ok(rd) = std::fs::read_dir(d) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, base, out);
                } else if let Ok(r) = p.strip_prefix(base) {
                    out.push(r.to_string_lossy().replace('\\', "/"));
                }
            }
        }
    }
    let mut on_disk = Vec::new();
    walk(dir, dir, &mut on_disk);
    for f in on_disk {
        if f != "package.json" && !f.starts_with('.') && !listed.contains(&f) {
            v.problems.push(format!("unlisted: {} is in the package but not in package.json", f));
        }
    }
    // 2. per console rules
    let console = man["console"].as_str().unwrap_or("gbc").to_string();
    let prof = crate::console::resolve(&console, "sprite", Some(dir))?;
    let (cw, ch) = (prof.hw_cell.0 as i32, prof.hw_cell.1 as i32);
    let base_prefix = |file: &str, m_dir: &str| file.strip_prefix(m_dir).map(String::from);
    let _ = base_prefix;
    let parts = man["parts"].as_array().cloned().unwrap_or_default();
    v.of_parts = parts.len();
    let spec_listed: BTreeSet<String> = listed.clone();
    for p in &parts {
        let name = p["name"].as_str().unwrap_or("?");
        if p["tags"].as_array().map_or(false, |t| t.iter().any(|x| x == "variant")) || p["kind"] != "sprite" {
            continue;
        }
        v.parts += 1;
        let (ox, oy) = (p["origin"][0].as_i64().unwrap_or(0) as i32, p["origin"][1].as_i64().unwrap_or(0) as i32);
        for f in p["frames"].as_array().into_iter().flatten() {
            if f["dropped"] == true {
                continue;
            }
            v.frames += 1;
            let fname = f["name"].as_str().unwrap_or("?");
            let (w, h) = (f["size"][0].as_u64().unwrap_or(0) as usize, f["size"][1].as_u64().unwrap_or(0) as usize);
            let files: Vec<String> = f["files"].as_array().into_iter().flatten().filter_map(|x| x.as_str()).map(String::from).collect();
            if files.is_empty() {
                v.problems.push(format!("frames: {}/{} has no exported file", name, fname));
                continue;
            }
            let mut png_px: Option<(usize, usize, Vec<Option<Rgb>>)> = None;
            for file in &files {
                let rel = file.rsplit_once("/").map(|_| ()).map(|_| file.clone()).unwrap_or(file.clone());
                // the package path drops the output root: find the listed file that ends with this key
                let hit = spec_listed.iter().find(|l| rel.ends_with(l.as_str())).cloned();
                let Some(hit) = hit else {
                    v.problems.push(format!("frames: {}/{}: {} is not in the package", name, fname, file));
                    continue;
                };
                if hit.ends_with(".png") && !hit.ends_with("_detail.png") && png_px.is_none() {
                    match crate::png::read_rgba(&dir.join(&hit)) {
                        Ok(im) => {
                            if (im.w, im.h) != (w, h) {
                                v.problems.push(format!("frames: {}/{}: {} is {}x{}, the manifest says {}x{}", name, fname, hit, im.w, im.h, w, h));
                            }
                            png_px = Some((im.w, im.h, im.px.iter().map(|p| (p.a > 0).then(|| Rgb { r: p.r, g: p.g, b: p.b })).collect()));
                        }
                        Err(e) => v.problems.push(format!("frames: {}/{}: {}", name, fname, e.msg)),
                    }
                }
            }
            if let Some((iw, ih, px)) = &png_px {
                // console colours
                let mut bad: BTreeSet<Rgb> = BTreeSet::new();
                let mut colours: BTreeSet<Rgb> = BTreeSet::new();
                for c in px.iter().flatten() {
                    colours.insert(*c);
                    if !prof.legal(*c) {
                        bad.insert(*c);
                    }
                }
                if !bad.is_empty() {
                    v.problems.push(format!("colours: {}/{} uses {} colour(s) the {} cannot show (e.g. {})", name, fname, bad.len(), console, crate::color::to_hex(*bad.iter().next().unwrap())));
                }
                let limit = if prof.colour_bits > 0 && prof.obj_palettes > 0 { prof.obj_palettes as usize * prof.colors as usize } else { prof.colors as usize * prof.obj_palettes.max(1) as usize };
                if colours.len() > limit {
                    v.problems.push(format!("colours: {}/{} has {} colours, the console's sprite palettes hold {}", name, fname, colours.len(), limit));
                }
                // cells: every opaque pixel lies in a listed cell
                if let Some(cells) = f["cells"].as_array() {
                    let mut covered = vec![false; iw * ih];
                    let mut line = vec![0u32; *ih + 2 * ch as usize + 64];
                    for c in cells {
                        let (cx, cy) = (c["x"].as_i64().unwrap_or(0) as i32 + ox, c["y"].as_i64().unwrap_or(0) as i32 + oy);
                        let cost = if c.get("slot2").is_some() { 2 } else { 1 };
                        for yy in cy..cy + ch {
                            if yy >= 0 && (yy as usize) < line.len() {
                                line[yy as usize] += cost;
                            }
                            for xx in cx..cx + cw {
                                if xx >= 0 && yy >= 0 && (xx as usize) < *iw && (yy as usize) < *ih {
                                    covered[yy as usize * iw + xx as usize] = true;
                                }
                            }
                        }
                    }
                    let uncovered = px.iter().enumerate().filter(|(i, c)| c.is_some() && !covered[*i]).count();
                    if uncovered > 0 {
                        v.problems.push(format!("cells: {}/{} has {} opaque pixel(s) outside every listed cell", name, fname, uncovered));
                    }
                    let per_frame: u32 = cells.iter().map(|c| if c.get("slot2").is_some() { 2 } else { 1 }).sum();
                    let max_line = line.iter().copied().max().unwrap_or(0);
                    if prof.hw_cells_per_row_max > 0 && max_line > prof.hw_cells_per_row_max {
                        v.problems.push(format!("hw: {}/{} puts {} sprites on one scanline (limit {})", name, fname, max_line, prof.hw_cells_per_row_max));
                    }
                    if prof.hw_cells_max > 0 && per_frame > prof.hw_cells_max {
                        v.problems.push(format!("hw: {}/{} needs {} sprites (limit {})", name, fname, per_frame, prof.hw_cells_max));
                    }
                }
            }
        }
    }
    // 3. palette tables
    for sc in man["scenes"].as_array().into_iter().flatten() {
        let sname = sc["name"].as_str().unwrap_or("?");
        let pals = sc["obj_palettes"].as_array().cloned().unwrap_or_default();
        if pals.len() > prof.obj_palettes as usize {
            v.problems.push(format!("palettes: scene {} lists {} OBJ palettes, the console has {}", sname, pals.len(), prof.obj_palettes));
        }
        let mut ids = BTreeSet::new();
        for p in &pals {
            v.palettes += 1;
            let id = p["id"].as_str().unwrap_or("");
            if id.is_empty() || !ids.insert(id.to_string()) {
                v.problems.push(format!("palettes: scene {} has a missing or repeated palette id '{}'", sname, id));
            }
            for c in p["colors"].as_array().into_iter().flatten().filter_map(|x| x.as_str()) {
                match crate::color::parse_hex(c) {
                    Ok(rgb) if prof.legal(rgb) => {}
                    _ => v.problems.push(format!("palettes: scene {} palette {} colour {} is not a {} colour", sname, id, c, console)),
                }
            }
            if p["colors"].as_array().map_or(0, |c| c.len()) != 0 && p["colors"].as_array().map_or(0, |c| c.len()) != prof.colors as usize {
                v.problems.push(format!("palettes: scene {} palette {} has {} colours, the console uses {} per palette", sname, id, p["colors"].as_array().unwrap().len(), prof.colors));
            }
        }
    }
    Ok(v)
}

pub fn run_verify(a: &VerifyPackageArgs, out: &mut Out) -> Result<()> {
    let m = Manifest::discover(a.manifest.as_deref()).ok().flatten();
    let dir = match (&a.package, &m) {
        (Some(p), _) => PathBuf::from(p),
        (None, Some(m)) => pkg_dir(m, &None),
        _ => PathBuf::from("package"),
    };
    let v = verify(&dir)?;
    for p in &v.problems {
        out.line(format!("FAIL {}", p));
    }
    out.line(format!("{}: verified {} of {} part(s), {} frame(s), {} file(s), {} palette table(s); {} problem(s)", if v.problems.is_empty() { "PASS" } else { "FAIL" }, v.parts, v.of_parts, v.frames, v.files, v.palettes, v.problems.len()));
    out.set("verify", json!({"pass": v.problems.is_empty(), "problems": v.problems, "checked": {"parts": v.parts, "of_parts": v.of_parts, "frames": v.frames, "files": v.files, "palettes": v.palettes}}));
    if v.problems.is_empty() {
        Ok(())
    } else {
        Err(PxlError::lint(format!("{} package problem(s)", v.problems.len())))
    }
}

// ------------------------------------------------------------------------------------------------ deliver

fn run_cmd(cmd: &str, dir: &Path, envs: &[(&str, String)]) -> Value {
    // hook commands run through the platform shell
    #[cfg(windows)]
    let mut c = {
        // cmd.exe does not parse the MSVC argument quoting Rust applies, so the command line goes to it verbatim
        use std::os::windows::process::CommandExt;
        let mut c = std::process::Command::new("cmd");
        c.arg("/C").raw_arg(cmd);
        c
    };
    #[cfg(not(windows))]
    let mut c = {
        let mut c = std::process::Command::new("sh");
        c.arg("-c").arg(cmd);
        c
    };
    c.current_dir(dir);
    for (k, v) in envs {
        c.env(k, v);
    }
    match c.output() {
        Ok(o) => json!({"command": cmd, "exit": o.status.code(), "ok": o.status.success(), "stdout": String::from_utf8_lossy(&o.stdout), "stderr": String::from_utf8_lossy(&o.stderr)}),
        Err(e) => json!({"command": cmd, "exit": null, "ok": false, "stdout": "", "stderr": format!("cannot run: {}", e)}),
    }
}

pub fn deliveries_dir(m: &Manifest) -> PathBuf {
    m.dir.join("deliveries")
}

pub fn next_number(m: &Manifest) -> u32 {
    std::fs::read_dir(deliveries_dir(m)).map(|r| r.flatten().filter_map(|e| e.file_name().to_string_lossy().parse::<u32>().ok()).max().unwrap_or(0)).unwrap_or(0) + 1
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)?.flatten() {
        let (p, q) = (e.path(), to.join(e.file_name()));
        if p.is_dir() {
            copy_tree(&p, &q)?;
        } else {
            std::fs::copy(&p, &q)?;
        }
    }
    Ok(())
}

/// `pxl deliver --dry-run`: the early delivery check on the style frames (M6 R7). Builds, checks the style frames, assembles a package in
/// `.pxl/dryrun/package` and verifies it with the same verifier as a delivery, runs the client's commands when given, keeps the style frames'
/// compare images as the report, and snapshots nothing. A pass is the milestone `dry run passed` (until a style frame changes).
fn run_dry(a: &DeliverArgs, out: &mut Out) -> Result<()> {
    let m = manifest_of(&a.manifest)?;
    let cfg = dmanifest::delivery_cfg(&m);
    let frames: Vec<String> = crate::style::style_frames(&m.dir).into_iter().map(|f| f.0).collect();
    // the dry run checks the style frames AND every part the scenes use (groups, tile set, window parts); what it does not cover is printed
    let mut names: Vec<String> = frames.clone();
    for sc in &m.scenes {
        let pats: Vec<&String> = sc.groups.iter().flat_map(|g| g.parts.iter()).chain(sc.tileset.iter()).chain(sc.window.iter()).collect();
        for p in pats {
            for d in crate::cellmem::resolve_parts(&m, p) {
                if !names.contains(&d.name) {
                    names.push(d.name.clone());
                }
            }
        }
    }
    if names.is_empty() {
        names = m.parts.iter().map(|d| d.name.clone()).collect();
    }
    let skipped: Vec<String> = m.parts.iter().map(|d| d.name.clone()).filter(|n| !names.contains(n)).collect();
    let work = m.dir.join(".pxl").join("dryrun");
    let dir = work.join("package");
    let images = work.join("report_images");
    std::fs::create_dir_all(&images)?;
    let mut problems: Vec<String> = Vec::new();
    let mut steps: Vec<Value> = Vec::new();
    let mut scratch = Out::new(true, None);
    // 1. the build keeps the manifest and the exports current
    let ba = crate::cli::BuildArgs { parts: Vec::new(), manifest: a.manifest.clone(), force: false };
    let b = crate::export::run_build(&ba, &mut scratch);
    out.line(format!("build: {}", if b.is_ok() { "PASS" } else { "FAIL" }));
    if let Err(e) = &b {
        problems.push(format!("build: {}", e.msg));
    }
    steps.push(json!({"step": "build", "pass": b.is_ok()}));
    // 2. our check on the style frames
    let ca = crate::cli::CheckArgs { parts: names.clone(), manifest: a.manifest.clone(), force: false, hand_finished: false, recompute: false, tolerance: None };
    let c = crate::export::run_check(&ca, &mut scratch);
    out.line(format!("check ({} part(s): {} style frame(s) + the parts of the scenes): {}", names.len(), frames.len(), if c.is_ok() { "PASS" } else { "FAIL" }));
    if !skipped.is_empty() {
        out.line(format!("not covered by this dry run ({} part(s), in no scene and not style frames): {}", skipped.len(), skipped.join(", ")));
    }
    if let Err(e) = &c {
        problems.push(format!("check: {}", e.msg));
    }
    steps.push(json!({"step": "check", "pass": c.is_ok(), "parts": names, "skipped": skipped}));
    // 3. package and verify
    match assemble(&m, &dir) {
        Ok((j, missing)) => {
            out.line(format!("package: {} file(s) in {}", j["files"], dir.slash()));
            if !missing.is_empty() {
                problems.push(format!("{} package file(s) are not exported, first: {}", missing.len(), missing[0]));
            }
            let v = verify(&dir)?;
            out.line(format!("verify-package: {} ({} frame(s), {} file(s))", if v.problems.is_empty() { "PASS" } else { "FAIL" }, v.frames, v.files));
            for p in v.problems.iter().take(10) {
                out.line(format!("  {}", p));
            }
            if !v.problems.is_empty() {
                problems.push(format!("verify-package: {} problem(s), first: {}", v.problems.len(), v.problems[0]));
            }
            steps.push(json!({"step": "verify-package", "pass": v.problems.is_empty(), "frames": v.frames, "files": v.files, "problems": v.problems}));
        }
        Err(e) => {
            problems.push(format!("package: {}", e.msg));
            steps.push(json!({"step": "package", "pass": false, "error": e.msg}));
        }
    }
    // 4. the client's commands, only when given
    let abs = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()).slash();
    let envs = [("PXL_PACKAGE", abs(&dir)), ("PXL_MANIFEST", abs(&dir.join("manifest.json"))), ("PXL_REPORT_DIR", abs(&images))];
    for (what, cmd) in [("client_checker", a.client_checker.clone().or(cfg.client_checker.clone())), ("client_pipeline", a.client_pipeline.clone().or(cfg.client_pipeline.clone()))] {
        if let (Some(c), true) = (cmd, dir.is_dir()) {
            let r = run_cmd(&c, &dir, &envs);
            out.line(format!("{}: exit {}", what, r["exit"]));
            if r["ok"] != true {
                problems.push(format!("{} failed", what));
            }
            steps.push(json!({"step": what, "pass": r["ok"] == true}));
        }
    }
    // 5. the style frames' compare images are the report
    let mut kept = Vec::new();
    for n in &names {
        if let Some(ai) = crate::asset::scan(&m.dir).into_iter().find(|x| x.name == *n) {
            let src = ai.dir.join("compare.png");
            if src.is_file() {
                let dst = images.join(format!("{}.png", n));
                if std::fs::copy(&src, &dst).is_ok() {
                    kept.push(dst.slash());
                }
            }
        }
    }
    let pass = problems.is_empty();
    let report = json!({"schema": 1, "dry_run": true, "pass": pass, "style_frames": frames, "parts": names, "skipped": skipped, "problems": problems, "steps": steps, "images": kept});
    fsutil::atomic_write_plain(&work.join("report.json"), (serde_json::to_string_pretty(&report).unwrap() + "\n").as_bytes())?;
    if pass {
        let mut st = crate::style::read_state(&m.dir);
        let mut t = toml::Table::new();
        t.insert("on".into(), crate::tasks::date_of(crate::tasks::now()).into());
        t.insert("by".into(), out.agent().unwrap_or("user").to_string().into());
        t.insert("signature".into(), crate::milestone::frames_signature(&m.dir).into());
        st.insert("dry_run".into(), toml::Value::Table(t));
        crate::style::write_state(&m.dir, &st)?;
        out.line(format!("dry run PASSED on {} part(s); milestone `dry run passed` recorded; nothing was snapshotted", names.len()));
        out.set("deliver", report);
        Ok(())
    } else {
        out.set("deliver", report);
        Err(PxlError::validation(format!("dry run failed: {}", problems.join("; "))).with_hint("fix these on the style frames now: this is the early warning for what would fail at delivery"))
    }
}

pub fn run_deliver(a: &DeliverArgs, out: &mut Out) -> Result<()> {
    if a.dry_run {
        return run_dry(a, out);
    }
    let m = manifest_of(&a.manifest)?;
    let cfg = dmanifest::delivery_cfg(&m);
    let dir = pkg_dir(&m, &a.package);
    let mut report = json!({"schema": 1});
    let mut ours: Vec<String> = Vec::new();
    // 1. our scene check (the whole project)
    let mut scratch = Out::new(true, None);
    let ca = crate::cli::CheckArgs { parts: Vec::new(), manifest: a.manifest.clone(), force: false, hand_finished: false, recompute: false, tolerance: None };
    let check = crate::export::run_check(&ca, &mut scratch);
    let check_ok = check.is_ok();
    out.line(format!("our check: {}", if check_ok { "PASS" } else { "FAIL" }));
    if !check_ok {
        ours.push(format!("`pxl check` fails: {}", check.err().map(|e| e.msg).unwrap_or_default()));
    }
    report["check"] = json!({"pass": check_ok});
    // 2. package and verify
    let pk = assemble(&m, &dir);
    match &pk {
        Ok((j, missing)) => {
            out.line(format!("package: {} file(s)", j["files"]));
            if !missing.is_empty() {
                ours.push(format!("{} package file(s) are not exported", missing.len()));
            }
            report["package"] = j.clone();
        }
        Err(e) => ours.push(format!("package: {}", e.msg)),
    }
    if pk.is_ok() {
        let v = verify(&dir)?;
        out.line(format!("verify-package: {} ({} frame(s), {} file(s))", if v.problems.is_empty() { "PASS" } else { "FAIL" }, v.frames, v.files));
        for p in v.problems.iter().take(10) {
            out.line(format!("  {}", p));
        }
        if !v.problems.is_empty() {
            ours.push(format!("verify-package: {} problem(s), first: {}", v.problems.len(), v.problems[0]));
        }
        report["verify"] = json!({"pass": v.problems.is_empty(), "problems": v.problems, "frames": v.frames, "files": v.files});
    }
    if !ours.is_empty() {
        out.set("deliver", json!({"delivered": false, "refused": ours, "report": report}));
        return Err(PxlError::validation(format!("not delivered: {}", ours.join("; "))).with_hint("fix our own failures first; nothing was snapshotted"));
    }
    // 3. the client's checker and pipeline
    let n = next_number(&m);
    let snap = deliveries_dir(&m).join(n.to_string());
    let images = snap.join("report_images");
    std::fs::create_dir_all(&images)?;
    let abs = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()).slash();
    let envs = [("PXL_PACKAGE", abs(&dir)), ("PXL_MANIFEST", abs(&dir.join("manifest.json"))), ("PXL_REPORT_DIR", abs(&images))];
    let mut client_fail = false;
    for (what, cmd) in [("client_checker", a.client_checker.clone().or(cfg.client_checker.clone())), ("client_pipeline", a.client_pipeline.clone().or(cfg.client_pipeline.clone()))] {
        if let Some(c) = cmd {
            let r = run_cmd(&c, &dir, &envs);
            out.line(format!("{}: exit {}", what, r["exit"]));
            for l in r["stdout"].as_str().unwrap_or("").lines().rev().take(5).collect::<Vec<_>>().into_iter().rev() {
                out.line(format!("  {}", l));
            }
            client_fail |= r["ok"] != true;
            report[what] = r;
        }
    }
    let imgs: Vec<String> = std::fs::read_dir(&images).map(|r| r.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect()).unwrap_or_default();
    report["report_images"] = json!(imgs);
    if client_fail && !a.allow_client_fail {
        let _ = std::fs::remove_dir_all(&snap);
        out.set("deliver", json!({"delivered": false, "client_failed": true, "report": report}));
        return Err(PxlError::validation("the client's checker or pipeline failed; nothing was snapshotted").with_hint("read its output above, or pass --allow-client-fail to record it anyway"));
    }
    // 4. the snapshot
    std::fs::copy(dir.join("manifest.json"), snap.join("manifest.json"))?;
    std::fs::copy(dir.join("package.json"), snap.join("package.json"))?;
    copy_tree(&dir, &snap.join("package"))?;
    fsutil::atomic_write_plain(&snap.join("report.json"), (serde_json::to_string_pretty(&report).unwrap() + "\n").as_bytes())?;
    out.line(format!("delivery {} snapshotted to {}", n, snap.slash()));
    out.set("deliver", json!({"delivered": true, "number": n, "dir": snap.slash(), "client_failed": client_fail, "report": report}));
    Ok(())
}

// ------------------------------------------------------------------------------------------------ changes

fn load_manifest_of(m: &Manifest, n: u32) -> Result<Value> {
    let p = deliveries_dir(m).join(n.to_string()).join("manifest.json");
    serde_json::from_slice(&std::fs::read(&p).map_err(|_| PxlError::usage(format!("delivery {} has no manifest ({})", n, p.slash())).with_hint("`pxl deliver` snapshots to deliveries/N/"))?).map_err(|e| PxlError::validation(format!("{}: {}", p.slash(), e)))
}

/// Changes of the second manifest against the first, as (kind, text) pairs.
pub fn diff_manifests(a: &Value, b: &Value) -> Vec<(String, String)> {
    let mut ch: Vec<(String, String)> = Vec::new();
    let index = |v: &Value| -> BTreeMap<String, Value> { v["parts"].as_array().into_iter().flatten().map(|p| (p["name"].as_str().unwrap_or("").to_string(), p.clone())).collect() };
    let (pa, pb) = (index(a), index(b));
    for n in pb.keys().filter(|k| !pa.contains_key(*k)) {
        ch.push(("part_added".into(), format!("{}: new part", n)));
    }
    for n in pa.keys().filter(|k| !pb.contains_key(*k)) {
        ch.push(("part_removed".into(), format!("{}: removed", n)));
    }
    for (n, new) in &pb {
        let Some(old) = pa.get(n) else { continue };
        let fr = |p: &Value| -> BTreeMap<String, Value> { p["frames"].as_array().into_iter().flatten().filter(|f| f["dropped"] != true).map(|f| (f["name"].as_str().unwrap_or("").to_string(), f.clone())).collect() };
        let (fa, fb) = (fr(old), fr(new));
        for f in fb.keys().filter(|k| !fa.contains_key(*k)) {
            ch.push(("frame_added".into(), format!("{}/{}: frame added", n, f)));
        }
        for f in fa.keys().filter(|k| !fb.contains_key(*k)) {
            ch.push(("frame_removed".into(), format!("{}/{}: frame dropped or removed", n, f)));
        }
        for (f, nf) in &fb {
            let Some(of) = fa.get(f) else { continue };
            if of["size"] != nf["size"] {
                ch.push(("size".into(), format!("{}/{}: size {} -> {}", n, f, of["size"], nf["size"])));
            }
            if of["origin"] != nf["origin"] {
                ch.push(("origin".into(), format!("{}/{}: origin {} -> {}", n, f, of["origin"], nf["origin"])));
            }
            if of["cells"] != nf["cells"] && (of["cells"].is_array() || nf["cells"].is_array()) {
                ch.push(("cells".into(), format!("{}/{}: cells {} -> {}", n, f, of["cells"].as_array().map_or(0, |c| c.len()), nf["cells"].as_array().map_or(0, |c| c.len()))));
            }
            if of["semantics"] != nf["semantics"] {
                ch.push(("semantics".into(), format!("{}/{}: semantics {} -> {}", n, f, of["semantics"], nf["semantics"])));
            }
        }
        if old["size"] != new["size"] {
            ch.push(("size".into(), format!("{}: canvas {} -> {}", n, old["size"], new["size"])));
        }
        if old["origin"] != new["origin"] {
            ch.push(("origin".into(), format!("{}: origin {} -> {}", n, old["origin"], new["origin"])));
        }
        if old["collision"] != new["collision"] || old["visual"] != new["visual"] {
            ch.push(("footprint".into(), format!("{}: footprint {} / visual {} -> {} / {}", n, old["collision"], old["visual"], new["collision"], new["visual"])));
        }
        for k in ["palette_variant", "residency", "palette_set", "repeat", "compose"] {
            if old[k] != new[k] {
                ch.push((k.into(), format!("{}: {} {} -> {}", n, k, old[k], new[k])));
            }
        }
    }
    let (sa, sb) = (&a["palettes"], &b["palettes"]);
    for (id, nv) in sb.as_object().into_iter().flatten() {
        match sa.get(id) {
            None => ch.push(("palette_added".into(), format!("palette {} added", id))),
            Some(ov) if ov["colors"] != nv["colors"] => ch.push(("palette".into(), format!("palette {}: {} -> {}", id, ov["colors"], nv["colors"]))),
            _ => {}
        }
    }
    for id in sa.as_object().into_iter().flatten().map(|x| x.0).filter(|k| sb.get(k.as_str()).is_none()) {
        ch.push(("palette_removed".into(), format!("palette {} removed", id)));
    }
    ch
}

pub fn run_changes(a: &ChangesArgs, out: &mut Out) -> Result<()> {
    let m = manifest_of(&a.manifest)?;
    let old = load_manifest_of(&m, a.since)?;
    let (new, label) = match a.to {
        Some(t) => (load_manifest_of(&m, t)?, format!("delivery {}", t)),
        None => (dmanifest::build(&m)?.0, "the current project".to_string()),
    };
    let ch = diff_manifests(&old, &new);
    for (k, t) in &ch {
        out.line(format!("[{}] {}", k, t));
    }
    out.line(format!("{} change(s) between delivery {} and {}", ch.len(), a.since, label));
    out.set("changes", json!({"since": a.since, "to": a.to, "count": ch.len(), "changes": ch.iter().map(|(k, t)| json!({"kind": k, "text": t})).collect::<Vec<_>>()}));
    Ok(())
}
