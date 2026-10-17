//! `import <png|grid.txt|.aseprite> --part P [--frame F] [--overlay png] [--template T]` (DESIGN 5.4). Owner: export worker.
//!
//! Brings existing images, grids or artist edits back into a part. `source` may be one file, a glob or a folder
//! (its `.txt`, `.png` and `.aseprite` files, sorted); `--frame` and `--overlay` need exactly one source.
//!
//! * **grid `.txt`**: rows of keys, exactly the part's size; every key must be defined by the template (error names
//!   file, row, column). Frame name = `--frame`, else the file stem.
//! * **tile sets and screens**: a PNG (any colour type, no transparency) must use exactly template colours; the error names
//!   the tile, the pixel and the nearest key. An `.aseprite` imports its plain `image` layer (the tilemap layer is ignored).
//! * **`.png`**: indexed or RGBA. Transparent (alpha 0) becomes `.`; every other pixel must have exactly the colour of
//!   a template key (or that colour snapped to the profile's `channel_step`), else the import fails naming the
//!   first pixel and its colour. Semi-transparent pixels are errors. With `--overlay` the second PNG supplies the
//!   overlay keys (they win over the base). The base PNG may hold an overlay colour too.
//! * **`.aseprite`**: every ase frame becomes a `.pxl` frame. Layers map to template layers by name (case-insensitive);
//!   a layer with pixels that matches no template layer is an error (rename it), as is any pixel colour that is
//!   not a key colour of its template layer or a semi-transparent pixel. Frame name = the single-frame tag at that
//!   frame, else the part's frame at the same index when both have as many frames, else `<stem>_<NN>`; `--frame`
//!   imports only the ase frame of that name. Everything a `.pxl` cannot express is listed under `unsupported` (and
//!   as warnings) instead of being dropped silently: frame durations, multi-frame (animation) tags, tag direction
//!   and repeat, slices (an `origin` pivot that differs from the part origin), hidden layers, layer opacity and
//!   blend modes, user data, properties and unknown chunks, group layers, non-indexed colour modes.
//!
//! Imported frames are written into the part and **locked**. When the part file changes it is saved atomically,
//! one `import` entry per call is appended to the journal (`pxl undo` restores the grids and lock flags of frames
//! that existed before and removes the frames the import added) and the status file records an edit.
//! `lease::guard` runs first.

use crate::fsutil::PathSlash;
use crate::ase::{self, ColorMode, LayerType};
use crate::cli::ImportArgs;
use crate::color::{self, Rgb};
use crate::fsutil;
use crate::journal::{self, FrameDelta};
use crate::lease;
use crate::lint;
use crate::model::{Frame, Grid, Part};
use crate::output::{Loc, Out, PxlError, Result};
use crate::png;
use crate::profile;
use crate::pxl;
use crate::status::{self, StatusEvent};
use crate::template::{self, Template};
use serde_json::json;
use std::path::{Path, PathBuf};

struct Imp {
    name: String,
    grid: Grid,
}

struct Lookup {
    /// (colour, key, template layer index)
    e: Vec<(Rgb, u8, usize)>,
    hint: String,
}

impl Lookup {
    fn new(part: &Part, tpl: &Template) -> Lookup {
        let prof = profile::load_for(part, None).ok();
        let mut e = Vec::new();
        let mut names = Vec::new();
        for (li, l) in tpl.layers.iter().enumerate() {
            for k in &l.keys {
                if let Some(c) = lint::effective_color(part, tpl, k.key) {
                    e.push((c, k.key, li));
                    let s = prof.as_ref().map_or(c, |p| p.snap(c));
                    if s != c {
                        e.push((s, k.key, li));
                    }
                    names.push(format!("{}={}", k.key as char, color::to_hex(c)));
                }
            }
        }
        Lookup { e, hint: format!("template colours: {}", names.join(" ")) }
    }
    /// Key of a colour; layers in `prefer` order are searched first, then `only` (when None: all layers).
    fn find(&self, c: Rgb, layers: Option<&[usize]>) -> Option<u8> {
        self.e.iter().find(|(ec, _, li)| *ec == c && layers.map_or(true, |ls| ls.contains(li))).map(|x| x.1)
    }
}

fn pixel_err(src: &str, frame: &str, x: usize, y: usize, what: String, hint: &str) -> PxlError {
    PxlError::validation(what).with_loc(Loc::file(src.to_string()).frame(frame.to_string()).at(y, x)).with_hint(hint.to_string())
}

fn check_size(src: &str, w: usize, h: usize, part: &Part) -> Result<()> {
    if (w, h) != part.size {
        return Err(PxlError::validation(format!("{} is {}x{} px, the part canvas is {}x{}", src, w, h, part.size.0, part.size.1))
            .with_hint("import expects the exact canvas size; crop or pad the image first"));
    }
    Ok(())
}

fn import_txt(path: &Path, name: &str, part: &Part, tpl: &Template) -> Result<Imp> {
    let text = fsutil::read_string(path)?;
    let src = path.slash();
    let grid = pxl::parse_grid(&text, part.size.0, part.size.1, &Loc::file(src.clone()).frame(name.to_string()))?;
    for (i, &k) in grid.cells.iter().enumerate() {
        if tpl.def(k).is_none() {
            return Err(PxlError::validation(format!("key '{}' is not defined by template '{}'", k as char, tpl.name))
                .with_loc(Loc::file(src).frame(name.to_string()).at(i / grid.w, i % grid.w))
                .with_hint("add the key to the template, fix the grid, or rewrite keys in a part with `pxl template remap PART OLD=NEW`"));
        }
    }
    Ok(Imp { name: name.to_string(), grid })
}

fn rgb_of(c: crate::img::Rgba) -> Rgb {
    Rgb { r: c.r, g: c.g, b: c.b }
}

fn import_png(path: &Path, overlay: Option<&Path>, name: &str, part: &Part, lk: &Lookup, notes: &mut Vec<String>) -> Result<Imp> {
    let src = path.slash();
    let img = png::read_rgba(path)?;
    check_size(&src, img.w, img.h, part)?;
    let mut grid = Grid::new(img.w, img.h, b'.');
    for y in 0..img.h {
        for x in 0..img.w {
            let p = img.px[y * img.w + x];
            if p.a == 0 {
                continue;
            }
            if p.a != 255 {
                return Err(pixel_err(&src, name, x, y, format!("pixel ({},{}) is semi-transparent (alpha {})", x, y, p.a), "palette images need fully opaque or fully transparent pixels"));
            }
            let k = lk.find(rgb_of(p), Some(&[0])).or_else(|| lk.find(rgb_of(p), None)).ok_or_else(|| {
                pixel_err(&src, name, x, y, format!("pixel ({},{}) has colour {} that matches no template key", x, y, color::to_hex(rgb_of(p))), &lk.hint)
            })?;
            grid.cells[y * img.w + x] = k;
        }
    }
    if let Some(op) = overlay {
        let osrc = op.slash();
        let oi = png::read_rgba(op)?;
        check_size(&osrc, oi.w, oi.h, part)?;
        let over: Vec<usize> = (1..lk.e.iter().map(|e| e.2).max().unwrap_or(0) + 1).collect();
        let mut mismatch = 0;
        for y in 0..oi.h {
            for x in 0..oi.w {
                let p = oi.px[y * oi.w + x];
                if p.a == 0 {
                    continue;
                }
                if p.a != 255 {
                    return Err(pixel_err(&osrc, name, x, y, format!("pixel ({},{}) is semi-transparent (alpha {})", x, y, p.a), "palette images need fully opaque or fully transparent pixels"));
                }
                let k = lk.find(rgb_of(p), Some(&over)).ok_or_else(|| {
                    pixel_err(&osrc, name, x, y, format!("overlay pixel ({},{}) has colour {} that matches no overlay key of the template", x, y, color::to_hex(rgb_of(p))), &lk.hint)
                })?;
                let cur = &mut grid.cells[y * oi.w + x];
                if *cur == b'.' {
                    mismatch += 1;
                }
                *cur = k;
            }
        }
        if mismatch > 0 {
            notes.push(format!("{}: {} overlay pixel(s) lie over a transparent base pixel", name, mismatch));
        }
    }
    Ok(Imp { name: name.to_string(), grid })
}

fn import_ase(path: &Path, only: Option<&str>, part: &Part, tpl: &Template, lk: &Lookup, unsupported: &mut Vec<String>) -> Result<Vec<Imp>> {
    let src = path.slash();
    let bytes = fsutil::read_file(path)?;
    let doc = ase::read(&bytes).map_err(|e| PxlError { msg: format!("{}: {}", src, e.msg), ..e })?;
    check_size(&src, doc.width as usize, doc.height as usize, part)?;
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("frame").to_string();
    if doc.mode != ColorMode::Indexed {
        unsupported.push(format!("colour mode {:?}: colours are matched to template keys, the mode itself is not kept", doc.mode));
    }
    let layers = doc.layers();
    // layer index -> template layer index
    let mut map: Vec<Option<usize>> = vec![None; layers.len()];
    for (i, l) in layers.iter().enumerate() {
        let has_cels = doc.frames.iter().any(|f| f.chunks.iter().any(|c| matches!(c, ase::Chunk::Cel(c) if c.layer as usize == i)));
        match l.layer_type {
            LayerType::Group => {
                unsupported.push(format!("group layer '{}': flattened (its children are mapped by their own names)", l.name));
                continue;
            }
            LayerType::Tilemap if part.kind != crate::model::PartKind::Sprite => {
                unsupported.push(format!("tilemap layer '{}' ignored: the plain `image` layer is imported", l.name));
                continue;
            }
            LayerType::Tilemap => {
                if has_cels {
                    return Err(PxlError::validation(format!("{}: tilemap layer '{}' cannot be expressed in a sprite part", src, l.name)));
                }
                unsupported.push(format!("tilemap layer '{}' (no cels) ignored", l.name));
                continue;
            }
            LayerType::Normal => {}
        }
        let by_name = tpl.layers.iter().position(|t| t.name.eq_ignore_ascii_case(&l.name)).or_else(|| (part.kind != crate::model::PartKind::Sprite && l.name.eq_ignore_ascii_case("image")).then_some(0));
        match by_name {
            Some(t) => {
                map[i] = Some(t);
                if !l.visible() {
                    unsupported.push(format!("layer '{}' is hidden in the file; its pixels were imported anyway", l.name));
                }
                if l.opacity != 255 {
                    unsupported.push(format!("layer '{}' opacity {} ignored", l.name, l.opacity));
                }
                if l.blend_mode != 0 {
                    unsupported.push(format!("layer '{}' blend mode {} ignored (treated as normal)", l.name, l.blend_mode));
                }
            }
            None if has_cels => {
                return Err(PxlError::validation(format!(
                    "{}: layer '{}' has pixels but no template layer of that name (template layers: {})",
                    src,
                    l.name,
                    tpl.layers.iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join(", ")
                ))
                .with_hint("rename the layer in Aseprite to a template layer name; nothing is imported so no pixels are lost"));
            }
            None => unsupported.push(format!("empty layer '{}' ignored", l.name)),
        }
    }
    // names
    let tags = doc.tags();
    let n = doc.frames.len();
    let mut names: Vec<String> = Vec::new();
    for i in 0..n {
        let single = tags.iter().find(|t| t.from as usize == i && t.to as usize == i).map(|t| t.name.clone());
        let nm = single.unwrap_or_else(|| {
            if n == part.frames.len() {
                part.frames[i].name.clone()
            } else {
                format!("{}_{:02}", stem, i + 1)
            }
        });
        names.push(nm);
    }
    for t in &tags {
        if t.from != t.to {
            unsupported.push(format!("tag '{}' spans frames {}..{}: animation tags are not stored in a part", t.name, t.from, t.to));
        }
        if t.direction != ase::LoopDir::Forward || t.repeat != 0 {
            unsupported.push(format!("tag '{}' direction/repeat ({:?}, {}) not stored", t.name, t.direction, t.repeat));
        }
    }
    let durs: Vec<u16> = doc.frames.iter().map(|f| f.duration).collect();
    if durs.iter().any(|&d| d != 100) {
        unsupported.push(format!("frame durations (ms) {} are not stored in a part", fmt_runs(&durs)));
    }
    for s in doc.slices() {
        match (s.name.as_str(), s.keys.first().and_then(|k| k.pivot)) {
            ("origin", Some(p)) if p == (part.origin.0, part.origin.1) => {}
            ("origin", Some(p)) => unsupported.push(format!("slice 'origin' pivot {:?} differs from the part origin {:?}; part origin unchanged", p, part.origin)),
            (nm, _) => unsupported.push(format!("slice '{}' not stored", nm)),
        }
    }
    let mut ud = 0;
    let mut unk: Vec<u16> = Vec::new();
    for f in &doc.frames {
        for c in &f.chunks {
            match c {
                ase::Chunk::UserData(_) => ud += 1,
                ase::Chunk::Unknown { chunk_type, .. } if !unk.contains(chunk_type) => unk.push(*chunk_type),
                _ => {}
            }
        }
    }
    if ud > 0 {
        unsupported.push(format!("{} user data / properties chunk(s) not stored", ud));
    }
    for t in unk {
        unsupported.push(format!("unknown chunk type 0x{:04x} not stored", t));
    }
    // pixels
    let mut out = Vec::new();
    for fi in 0..n {
        if only.map_or(false, |o| o != names[fi]) {
            continue;
        }
        let mut grid = Grid::new(part.size.0, part.size.1, b'.');
        for (li, tl) in map.iter().enumerate() {
            let Some(tl) = tl else { continue };
            let px = ase::render_layer_rgba(&doc, fi, li)?;
            for (i, p) in px.iter().enumerate() {
                if p.a == 0 {
                    continue;
                }
                let (x, y) = (i % grid.w, i / grid.w);
                if p.a != 255 {
                    return Err(pixel_err(&src, &names[fi], x, y, format!("layer '{}' pixel ({},{}) is semi-transparent (alpha {})", layers[li].name, x, y, p.a), "palette images need fully opaque pixels"));
                }
                let k = lk.find(rgb_of(*p), Some(&[*tl])).ok_or_else(|| {
                    pixel_err(&src, &names[fi], x, y, format!("layer '{}' pixel ({},{}) has colour {} that is no key colour of template layer '{}'", layers[li].name, x, y, color::to_hex(rgb_of(*p)), tpl.layers[*tl].name), &lk.hint)
                })?;
                // overlay layers win over the base, higher template layers over lower ones
                let cur = grid.cells[i];
                if cur == b'.' || tpl.layer_of(cur).map_or(true, |c| c <= *tl) {
                    grid.cells[i] = k;
                }
            }
        }
        out.push(Imp { name: names[fi].clone(), grid });
    }
    if out.is_empty() {
        return Err(PxlError::usage(match only {
            Some(o) => format!("{} has no frame named '{}' (frames: {})", src, o, names.join(", ")),
            None => format!("{} has no frames", src),
        }));
    }
    Ok(out)
}

fn fmt_runs(d: &[u16]) -> String {
    let mut runs: Vec<(u16, usize)> = Vec::new();
    for &x in d {
        match runs.last_mut() {
            Some(l) if l.0 == x => l.1 += 1,
            _ => runs.push((x, 1)),
        }
    }
    runs.iter().map(|(v, n)| if *n > 1 { format!("{}x{}", v, n) } else { v.to_string() }).collect::<Vec<_>>().join(",")
}

fn expand_sources(s: &str) -> Result<Vec<PathBuf>> {
    let known = |p: &Path| p.extension().map_or(false, |e| matches!(e.to_string_lossy().to_ascii_lowercase().as_str(), "txt" | "png" | "aseprite" | "ase"));
    let mut v: Vec<PathBuf> = if s.contains('*') || s.contains('?') {
        fsutil::glob(s).into_iter().filter(|p| p.is_file()).collect()
    } else if Path::new(s).is_dir() {
        let mut v: Vec<PathBuf> = std::fs::read_dir(s).map_err(|e| PxlError::io(format!("cannot read {}: {}", s, e)))?.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_file() && known(p)).collect();
        v.sort();
        v
    } else if Path::new(s).is_file() {
        vec![PathBuf::from(s)]
    } else {
        return Err(PxlError::io(format!("import source '{}' not found", s)).with_hint("give a .png, grid .txt or .aseprite file, a glob or a folder"));
    };
    v.sort();
    if v.is_empty() {
        return Err(PxlError::usage(format!("no importable files match '{}'", s)));
    }
    for p in &v {
        if !known(p) {
            return Err(PxlError::usage(format!("{}: import takes .txt grids, .png or .aseprite files", p.slash())));
        }
    }
    Ok(v)
}

pub fn run_import(a: &ImportArgs, out: &mut Out) -> Result<()> {
    // `pxl import ASSET FILE` or `pxl import FILE --part PART`
    let (part_arg, source_arg) = match &a.file {
        Some(f) => (Some(a.source.as_str()), f.as_str()),
        None => (a.part.as_deref(), a.source.as_str()),
    };
    let part_arg = part_arg.ok_or_else(|| PxlError::usage("import needs --part <part.pxl> (or: pxl import ASSET FILE)"))?;
    let part_path = fsutil::resolve_part(part_arg)?;
    let mut a = a.clone();
    if a.frame.is_none() && crate::asset::is_asset_file(&part_path) {
        // an asset with a single frame: that frame
        if let Ok(p) = pxl::load(&part_path) {
            if p.frames.len() == 1 {
                a.frame = Some(p.frames[0].name.clone());
            }
        }
    }
    let a = &a;
    let sources = expand_sources(source_arg)?;
    if sources.len() > 1 && (a.frame.is_some() || a.overlay.is_some()) {
        return Err(PxlError::usage("--frame and --overlay need exactly one source file"));
    }
    if a.overlay.is_some() && sources[0].extension().map_or(true, |e| !e.eq_ignore_ascii_case("png")) {
        return Err(PxlError::usage("--overlay goes with a .png source"));
    }
    lease::guard(&part_path, out.agent())?;
    crate::status::refuse_if_approved(&part_path, a.force_approved)?;
    let raw = fsutil::read_string(&part_path)?;
    let mut part = pxl::parse_str(&raw, &part_path)?;
    let tpl = match &a.template {
        Some(t) => {
            let p = if Path::new(t).is_file() { PathBuf::from(t) } else { fsutil::find_up(&part.dir(), t).ok_or_else(|| PxlError::usage(format!("template '{}' not found", t)))? };
            template::load(&p)?
        }
        None => template::load_for(&part)?,
    };
    let lk = Lookup::new(&part, &tpl);
    let mut notes: Vec<String> = Vec::new();
    let mut unsupported: Vec<String> = Vec::new();
    let mut imps: Vec<Imp> = Vec::new();
    for s in &sources {
        let ext = s.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        let stem = s.file_stem().and_then(|x| x.to_str()).unwrap_or("frame").to_string();
        match ext.as_str() {
            "txt" => imps.push(import_txt(s, a.frame.as_deref().unwrap_or(&stem), &part, &tpl)?),
            "png" if part.kind != crate::model::PartKind::Sprite => {
                if a.overlay.is_some() {
                    return Err(PxlError::usage("--overlay applies to sprites; tile sets and screens have one image"));
                }
                let name = a.frame.as_deref().unwrap_or(&stem);
                imps.push(Imp { name: name.to_string(), grid: crate::tileio::import_tile_png(s, name, &part, &tpl)? })
            }
            "png" => {
                let ov = a.overlay.as_deref().map(Path::new);
                if let Some(o) = ov {
                    if !o.is_file() {
                        return Err(PxlError::io(format!("overlay '{}' not found", o.slash())));
                    }
                }
                imps.push(import_png(s, ov, a.frame.as_deref().unwrap_or(&stem), &part, &lk, &mut notes)?)
            }
            _ => imps.extend(import_ase(s, a.frame.as_deref(), &part, &tpl, &lk, &mut unsupported)?),
        }
    }
    unsupported.dedup();
    // apply
    let mut deltas: Vec<FrameDelta> = Vec::new();
    let mut report = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut added: Vec<String> = Vec::new();
    for imp in imps {
        if seen.contains(&imp.name) {
            return Err(PxlError::usage(format!("frame '{}' is imported twice in one call", imp.name)));
        }
        seen.push(imp.name.clone());
        match part.frame_mut(&imp.name) {
            Some(f) => {
                let changed = f.grid.cells.iter().zip(&imp.grid.cells).filter(|(x, y)| x != y).count();
                deltas.push(FrameDelta { name: f.name.clone(), locked_before: f.locked, locked_after: true, region: (0, 0, f.grid.w, f.grid.h), before: f.grid.to_lines() });
                f.grid = imp.grid;
                f.locked = true;
                report.push((imp.name, false, changed));
            }
            None => {
                let n = imp.grid.cells.iter().filter(|&&c| c != b'.').count();
                added.push(imp.name.clone());
                part.frames.push(Frame { name: imp.name.clone(), src: None, locked: true, grid: imp.grid });
                report.push((imp.name, true, n));
            }
        }
    }
    let text = pxl::to_string(&part);
    let changed_file = text != raw;
    if changed_file {
        let canonical = pxl::parse_str(&raw, &part_path).map(|p| pxl::to_string(&p) == raw).unwrap_or(true);
        fsutil::atomic_write(&part_path, text.as_bytes())?;
        if !deltas.is_empty() || !added.is_empty() {
            let mut entry = journal::make_entry(out.agent(), "import", deltas, &text, if canonical { None } else { Some(raw.clone()) });
            entry.added = added;
            journal::append_edit(&part_path, &entry)?;
        }
        status::record(&part_path, &part, StatusEvent::Edited);
    }
    for u in &unsupported {
        out.warn(format!("not expressible in .pxl: {}", u));
    }
    for n in &notes {
        out.warn(n.clone());
    }
    let mut t = String::new();
    for (n, new, px) in &report {
        t.push_str(&format!("imported {}/{}: {} ({} {}), locked\n", part.name, n, if *new { "new frame" } else { "replaced" }, px, if *new { "opaque px" } else { "px changed" }));
    }
    if !changed_file {
        t.push_str("part file unchanged\n");
    }
    for u in &unsupported {
        t.push_str(&format!("unsupported: {}\n", u));
    }
    out.emit(
        t.trim_end(),
        json!({
            "part": part_path.slash(),
            "imported": report.iter().map(|(n, new, px)| json!({"frame": n, "new": new, "pixels": px})).collect::<Vec<_>>(),
            "unsupported": unsupported,
            "notes": notes,
            "changed": changed_file,
        }),
    );
    Ok(())
}
