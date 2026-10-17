//! Review packets and the rework record.
//!
//! `pxl review` writes, per part, a folder `OUT/<part>/` holding
//!
//! * `compare/`: original | output | diff pages for every frame (tile sets: the ranked tile pages),
//! * `zoom/`: the `--worst N` frames (by score) one per page, large,
//! * `lineup.png`: the part's frames next to its siblings' (each item at 1x and zoomed),
//! * `review.md`: the checklist (from `skills/pxl-review/references/checklist.md`), the part's facts, the raw
//!   (not hand-finished) frames, open rework notes and a per-frame table to fill in.
//!
//! Siblings of a part are the parts that share its `family`, its template base (the first `extends` ancestor of
//! the template, or the template itself) or that are listed in `[review] siblings` of the part or of the manifest.
//!
//! Verdicts live in `<part>.status.json` (`review` entries, see `status::Note`): `approve` and `rework` (optionally
//! per frame). A per-frame rework note is open for as long as the frame's grid still hashes as it did when the
//! note was written; editing the frame closes it. `status --rework` lists what is open; `status --metrics` counts
//! rework rounds and groups the notes by the first checklist keyword they mention.

use crate::fsutil::PathSlash;
use crate::cli::*;
use crate::model::{Part, PartKind};
use crate::output::{Out, Result};
use crate::status::{self, StatusFile};
use crate::{fsutil, lineup, pxl, queue, template};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const CHECKLIST_PATH: &str = "skills/pxl-review/references/checklist.md";
const EMBEDDED: &str = "pxl-review/references/checklist.md";

/// Words used to group rework notes when the checklist has no usable bullets.
pub const DEFAULT_KEYWORDS: [&str; 12] = ["eyes", "face", "outline", "colour", "shading", "noise", "limbs", "shoes", "texture", "consistency", "palette", "silhouette"];

/// The review checklist: a project copy (`skills/pxl-review/references/checklist.md` or the installed
/// `.claude/skills/...` one, searched upwards from `dir`), else the copy embedded in the tool.
pub fn checklist(dir: &Path) -> String {
    for rel in [CHECKLIST_PATH, ".claude/skills/pxl-review/references/checklist.md"] {
        if let Some(p) = fsutil::find_up(dir, rel) {
            if let Ok(t) = std::fs::read_to_string(p) {
                return t;
            }
        }
    }
    crate::skill::files().iter().find(|(p, _)| *p == EMBEDDED).map(|(_, t)| t.to_string()).unwrap_or_else(|| "CHECKLIST\n".into())
}

/// Reason keywords of a checklist: the list on a `keywords:` line if there is one, otherwise the first word of every
/// bullet (`-`, `*`, `1.`), lowercased, at least 3 letters. Falls back to [`DEFAULT_KEYWORDS`].
pub fn keywords(text: &str) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    let push = |w: String, v: &mut Vec<String>| {
        if w.len() >= 3 && !v.contains(&w) {
            v.push(w);
        }
    };
    for l in text.lines() {
        let t = l.trim();
        if let Some(rest) = t.strip_prefix("keywords:") {
            let ks: Vec<String> = rest.split(',').map(|k| k.trim().to_lowercase()).filter(|k| !k.is_empty()).collect();
            if !ks.is_empty() {
                return ks;
            }
        }
    }
    for l in text.lines() {
        let t = l.trim();
        let body = if let Some(r) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
            Some(r)
        } else {
            let d = t.trim_start_matches(|c: char| c.is_ascii_digit());
            (d.len() < t.len()).then(|| d.strip_prefix(". ").or_else(|| d.strip_prefix(") "))).flatten()
        };
        if let Some(b) = body {
            let b = b.trim_start_matches("[ ]").trim_start_matches("[x]").trim().trim_start_matches('*').trim_start_matches('_');
            let w: String = b.chars().take_while(|c| c.is_alphabetic()).collect::<String>().to_lowercase();
            push(w, &mut v);
        }
    }
    if v.is_empty() {
        DEFAULT_KEYWORDS.iter().map(|s| s.to_string()).collect()
    } else {
        v
    }
}

/// The first keyword (in checklist order) the note mentions, else `other`.
pub fn reason_of(text: &str, kws: &[String]) -> String {
    let t = text.to_lowercase();
    kws.iter().find(|k| t.contains(k.as_str())).cloned().unwrap_or_else(|| "other".into())
}

// ------------------------------------------------------------------ siblings

fn template_base(part: &Part) -> Option<PathBuf> {
    let p = template::locate(part).ok()?;
    let text = std::fs::read_to_string(&p).ok()?;
    let ext = text.parse::<toml::Table>().ok()?.get("extends").and_then(|v| v.as_str().map(String::from));
    let base = match ext {
        Some(rel) => fsutil::find_up(p.parent().unwrap_or_else(|| Path::new(".")), &rel).unwrap_or(p),
        None => p,
    };
    Some(crate::manifest::canon(&base))
}

fn matches_name(path: &Path, part: &Part, name: &str) -> bool {
    part.name == name || path.file_stem().map_or(false, |s| s.to_string_lossy() == name) || crate::manifest::canon(path) == crate::manifest::canon(Path::new(name)) || path.ends_with(name)
}

/// Why `other` is a sibling of `part`, if it is.
fn sibling_reason(path: &Path, part: &Part, groups: &[Vec<String>], opath: &Path, other: &Part) -> Option<&'static str> {
    if crate::manifest::canon(path) == crate::manifest::canon(opath) {
        return None;
    }
    if part.review_siblings.iter().any(|n| matches_name(opath, other, n)) || other.review_siblings.iter().any(|n| matches_name(path, part, n)) {
        return Some("listed");
    }
    if groups.iter().any(|g| g.iter().any(|n| matches_name(path, part, n)) && g.iter().any(|n| matches_name(opath, other, n))) {
        return Some("listed (manifest)");
    }
    if part.family.is_some() && part.family == other.family {
        return Some("family");
    }
    match (template_base(part), template_base(other)) {
        (Some(a), Some(b)) if a == b => Some("template base"),
        _ => None,
    }
}

// ------------------------------------------------------------------ packet

struct Frames {
    /// (frame, iou %, oklab, lint e/w, score)
    rows: Vec<(String, Option<f64>, Option<f64>, Option<(u32, u32)>, Option<f64>)>,
}

fn fmt_opt(v: Option<f64>, prec: usize, scale: f64) -> String {
    v.map_or("-".to_string(), |x| format!("{:.*}", prec, x * scale))
}

fn lineup_pages(base: &Path) -> Vec<PathBuf> {
    let stem = base.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let dir = base.parent().map(|d| d.to_path_buf()).unwrap_or_default();
    let mut v = Vec::new();
    for i in 1..20 {
        let f = if i == 1 { dir.join(format!("{}.png", stem)) } else { dir.join(format!("{}-{}.png", stem, i)) };
        if f.is_file() {
            v.push(f);
        } else {
            break;
        }
    }
    v
}

/// Frames of a part worth showing in a lineup: locked (approved) ones first, then the rest in order.
fn lineup_frames(part: &Part, max: usize) -> Vec<String> {
    let mut v: Vec<&crate::model::Frame> = part.frames.iter().filter(|f| f.locked).collect();
    v.extend(part.frames.iter().filter(|f| !f.locked));
    v.into_iter().take(max).map(|f| f.name.clone()).collect()
}

struct Packet {
    dir: PathBuf,
    sheets: Vec<String>,
    zooms: Vec<String>,
    lineup: Vec<String>,
    siblings: Vec<(String, &'static str)>,
    frames: Frames,
    warnings: Vec<String>,
    md: PathBuf,
}

const MANIFEST: &str = ".packet.json";

/// Content hash of the part: every frame's name and grid.
pub fn part_hash(part: &Part) -> String {
    let mut acc = String::new();
    for f in &part.frames {
        acc.push_str(&f.name);
        acc.push(':');
        acc.push_str(&fsutil::content_hash(&f.grid.cells));
        acc.push('\n');
    }
    fsutil::content_hash(acc.as_bytes())
}

fn utc_now() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    // civil-from-days
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", y, m, d, rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Relative paths (with `/`) of all files below `dir`.
fn list_files(dir: &Path) -> Vec<String> {
    fn walk(base: &Path, d: &Path, out: &mut Vec<String>) {
        if let Ok(rd) = std::fs::read_dir(d) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(base, &p, out);
                } else if let Ok(r) = p.strip_prefix(base) {
                    out.push(r.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/"));
                }
            }
        }
    }
    let mut v = Vec::new();
    walk(dir, dir, &mut v);
    v
}

/// Delete the files the previous run's manifest lists (nothing else), then empty directories left behind.
fn remove_previous(dir: &Path) {
    let mf = dir.join(MANIFEST);
    let Some(v) = std::fs::read_to_string(&mf).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) else { return };
    for f in v["files"].as_array().into_iter().flatten().filter_map(|f| f.as_str()) {
        if f.is_empty() || f.starts_with('/') || f.split('/').any(|c| c == ".." || c.is_empty()) {
            continue;
        }
        let p = dir.join(f);
        if p.is_file() {
            let _ = std::fs::remove_file(&p);
            let mut up = p.parent().map(Path::to_path_buf);
            while let Some(u) = up {
                if u == dir || std::fs::remove_dir(&u).is_err() {
                    break;
                }
                up = u.parent().map(Path::to_path_buf);
            }
        }
    }
    let _ = std::fs::remove_file(&mf);
}

fn build_packet(path: &Path, part: &Part, s: &StatusFile, root: &Path, a: &ReviewArgs, all: &[(PathBuf, Part)], groups: &[Vec<String>]) -> Result<Packet> {
    let dir = root.join(&part.name);
    std::fs::create_dir_all(&dir)?;
    // drop what the previous run generated (and only that), then remember what exists so the new files can be told apart
    remove_previous(&dir);
    let before = list_files(&dir);
    let hash = part_hash(part);
    let stamp: String = hash.chars().take(8).collect();
    let mut warnings = Vec::new();
    let mut sheets: Vec<String> = Vec::new();
    let mut zooms: Vec<String> = Vec::new();
    let mut rows: Vec<(String, Option<f64>, Option<f64>, Option<(u32, u32)>, Option<f64>)> = Vec::new();
    // (a) compare pages for every frame
    if part.kind == PartKind::Sprite {
        let o = crate::compare::RunOpts { out_dir: Some(dir.join("compare")), kind: "review".into(), stamp: stamp.clone(), ..Default::default() };
        match crate::compare::compare_part(path, &o) {
            Ok(r) => {
                warnings.extend(r.warnings.clone());
                sheets = r.pages.iter().map(|p| p.slash()).collect();
                for (f, m) in &r.ranking {
                    rows.push((f.clone(), Some(m.iou), m.oklab, Some((m.errors, m.warnings)), Some(m.score)));
                }
                let mut worst: Vec<String> = r.ranking.iter().take(a.worst).map(|(f, _)| f.clone()).collect();
                // every frame with an open rework note gets a zoom page as well
                for (_, of) in s.open_rework(part) {
                    for f in of {
                        if !worst.contains(&f) && r.ranking.iter().any(|(n, _)| *n == f) {
                            worst.push(f);
                        }
                    }
                }
                if !worst.is_empty() {
                    let z = crate::compare::RunOpts { names: worst, zoom: Some(0), out_dir: Some(dir.join("zoom")), kind: "review".into(), stamp: stamp.clone(), ..Default::default() };
                    match crate::compare::compare_part(path, &z) {
                        Ok(zr) => zooms = zr.pages.iter().map(|p| p.slash()).collect(),
                        Err(e) => warnings.push(format!("zoom pages: {}", e.msg)),
                    }
                }
            }
            Err(e) => warnings.push(format!("no compare pages: {}", e.msg)),
        }
    } else {
        let ca = CompareArgs { parts: Vec::new(), sel: FrameSel::default(), on: None, set: None, zoom: None, out: Some(dir.join("compare").slash()) };
        let mut sink = Out::new(true, None);
        match crate::tileio::compare_tileset(path, part, &ca, &mut sink) {
            Ok(()) => {
                let jp = dir.join("compare").join(format!("{}-tiles.json", part.name));
                if let Some(v) = std::fs::read_to_string(&jp).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
                    sheets = v["pages"].as_array().map(|a| a.iter().filter_map(|p| p["path"].as_str().map(String::from)).collect()).unwrap_or_default();
                }
            }
            Err(e) => warnings.push(format!("no compare pages: {}", e.msg)),
        }
    }
    for f in &part.frames {
        if !rows.iter().any(|r| r.0 == f.name) {
            let m = s.metrics.get(&f.name);
            rows.push((f.name.clone(), m.and_then(|m| m["iou"].as_f64()), m.and_then(|m| m["oklab"].as_f64()), None, m.and_then(|m| m["score"].as_f64())));
        }
    }
    // keep part order in the table
    rows.sort_by_key(|r| part.frame_index(&r.0).unwrap_or(usize::MAX));
    // (b) siblings and lineup
    let sibs: Vec<(&PathBuf, &Part, &'static str)> = all.iter().filter_map(|(p, o)| sibling_reason(path, part, groups, p, o).map(|r| (p, o, r))).collect();
    let region = (part.kind != PartKind::Sprite).then(|| Region { x: 0, y: 0, w: 128, h: 64 });
    let mut specs = vec![format!("{}:{}", path.slash(), lineup_frames(part, 12).join(","))];
    for (p, o, _) in sibs.iter().take(8) {
        let fr = lineup_frames(o, 4);
        if !fr.is_empty() {
            specs.push(format!("{}:{}", p.slash(), fr.join(",")));
        }
    }
    let base = dir.join("lineup.png");
    let la = LineupArgs { parts: specs, on: None, zoom: None, region, out: Some(base.slash()) };
    let mut sink = Out::new(true, None);
    let lineup = match lineup::run(&la, &mut sink) {
        Ok(()) => lineup_pages(&base).iter().map(|p| p.slash()).collect(),
        Err(e) => {
            warnings.push(format!("no lineup: {}", e.msg));
            Vec::new()
        }
    };
    let siblings = sibs.iter().map(|(p, o, r)| (format!("{} ({})", o.name, p.slash()), *r)).collect();
    // (c, d) review.md
    let md = dir.join("review.md");
    let pk = Packet { dir: dir.clone(), sheets, zooms, lineup, siblings, frames: Frames { rows }, warnings, md: md.clone() };
    fsutil::atomic_write(&md, review_md(path, part, s, &pk, &hash).as_bytes())?;
    let mut owned: Vec<String> = list_files(&dir).into_iter().filter(|f| !before.contains(f)).collect();
    if !owned.iter().any(|f| f == "review.md") {
        owned.push("review.md".into());
    }
    let _ = fsutil::atomic_write(&dir.join(MANIFEST), serde_json::to_string_pretty(&json!({"hash": hash, "files": owned})).unwrap_or_default().as_bytes());
    Ok(pk)
}

fn review_md(path: &Path, part: &Part, s: &StatusFile, pk: &Packet, hash: &str) -> String {
    let rel = |p: &str| -> String { Path::new(p).strip_prefix(&pk.dir).map(|r| r.slash()).unwrap_or_else(|_| p.to_string()) };
    let mut m = format!("# Review: {}\n\n", part.name);
    m.push_str(&format!("generated: {} UTC  part hash: `{}` (the page images carry `{}` in their title strip; if it differs the images are stale)\n\n", utc_now(), hash, &hash[..8.min(hash.len())]));
    m.push_str(&format!("part: `{}`  state: {}  frames: {}  lint: {}\n\n", path.slash(), s.state, part.frames.len(), s.lint.map_or("not linted".to_string(), |l| format!("{} errors / {} warnings / {} infos", l.errors, l.warnings, l.infos))));
    m.push_str("## Checklist\n\n");
    if let Some(h) = crate::manifest::Manifest::find_file(&part.dir()).map(|m| crate::asset::dir_of(&m)) {
        let me = crate::asset::asset_name(&part.dir());
        if let Some((n, p)) = crate::style::nearest_style_frame(&h, &me) {
            m.push_str(&format!("Nearest style frame: `{}` ({}). Review against it and against `pxl review checklist` (rules of style.md).\n\n", n, p.slash()));
        } else if crate::style::is_style_frame(&h, &me) {
            m.push_str(&format!("This asset is a style frame itself (`{}`): it is the reference the other assets are compared against. Review it against the rules of style.md (`pxl review checklist`) and its source.\n\n", me));
        }
    }
    m.push_str(checklist(&part.dir()).trim_end());
    m.push_str("\n\nLook at the pictures before you fill in the table. Numbers rank where to look first; they do not decide.\n\n## Pictures\n\n");
    for p in &pk.sheets {
        m.push_str(&format!("- compare (original | output | diff): `{}`\n", rel(p)));
    }
    for p in &pk.zooms {
        m.push_str(&format!("- zoom (worst frames, one per page): `{}`\n", rel(p)));
    }
    for p in &pk.lineup {
        m.push_str(&format!("- lineup with siblings (each item at 1x left, zoomed right): `{}`\n", rel(p)));
    }
    m.push_str("\n## Siblings\n\n");
    if pk.siblings.is_empty() {
        m.push_str("none found (no shared `family`, template base or `[review] siblings`); the lineup shows this part only.\n");
    }
    for (n, r) in &pk.siblings {
        m.push_str(&format!("- {} [{}]\n", n, r));
    }
    let raw = status::raw_audit(s, part);
    m.push_str("\n## Raw frames (still equal to the automatic output)\n\n");
    let rawf: Vec<_> = raw.iter().filter(|r| r.count > 0).collect();
    if rawf.is_empty() {
        m.push_str("none\n");
    }
    for r in rawf {
        if r.tiles.is_empty() {
            m.push_str(&format!("- {}\n", r.frame));
        } else {
            m.push_str(&format!("- {}: {} of {} tiles: {}\n", r.frame, r.count, r.total, tile_list(&r.tiles, 40)));
        }
    }
    m.push_str("\n## Marked finished without edit\n\nThese frames were declared hand-finished with no pixel change (`pxl finish`). Reject any that are not: `pxl rework PART --frames F --note ...` reopens them.\n\n");
    let marks = crate::journal::finish_marks(path).unwrap_or_default();
    if marks.is_empty() {
        m.push_str("none\n");
    }
    for mk in &marks {
        let t = if mk.tiles.is_empty() { String::new() } else { format!(" tiles {}", tile_list(&mk.tiles, 40)) };
        m.push_str(&format!("- {}{}: {}{}\n", mk.frame, t, mk.reason, mk.agent.as_ref().map_or(String::new(), |a| format!(" ({})", a))));
    }
    let open = s.open_rework(part);
    m.push_str("\n## Open rework notes\n\n");
    if open.is_empty() {
        m.push_str("none\n");
    }
    for (i, of) in &open {
        let n = &s.review[*i];
        m.push_str(&format!("- ({}) {}{}\n", n.by, if of.is_empty() { String::new() } else { format!("[{}] ", of.join(",")) }, n.text));
    }
    m.push_str("\n## Frames\n\nFill in `verdict` (ok / rework) and `notes` (what is wrong, where, direction, reuse hint).\n\n");
    m.push_str("| frame | iou % | oklab | lint e/w | raw | open note | verdict | notes |\n|---|---|---|---|---|---|---|---|\n");
    for (f, iou, ok, lint, _) in &pk.frames.rows {
        let rawc = raw.iter().find(|r| r.frame == *f).map_or(0, |r| r.count);
        let on = open.iter().any(|(_, of)| of.contains(f));
        m.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} |  |  |\n",
            f,
            fmt_opt(*iou, 1, 100.0),
            fmt_opt(*ok, 3, 1.0),
            lint.map_or("-".to_string(), |(e, w)| format!("{}/{}", e, w)),
            if rawc > 0 { "raw" } else { "" },
            if on { "open" } else { "" }
        ));
    }
    for w in &pk.warnings {
        m.push_str(&format!("\nnote: {}\n", w));
    }
    m.push_str(&format!("\n## Verdict\n\n```\npxl approve {p} --note \"...\" --agent R\npxl rework {p} --frames 01_stand,02_wait --note \"frame -> what is wrong -> where -> direction\" --agent R\n```\n", p = path.slash()));
    m
}

pub fn run_review(a: &ReviewArgs, out: &mut Out) -> Result<()> {
    if let Some(r) = crate::tasks::review_sub(a, out) {
        return r;
    }
    let mut entries = queue::collect_review(a)?;
    if let Some(fam) = &a.family {
        entries.retain(|e| pxl::load(&e.path).map_or(false, |p| p.family.as_deref() == Some(fam.as_str())));
    }
    let manifest = crate::manifest::Manifest::discover(None).ok().flatten();
    let groups = manifest.as_ref().map(|m| m.review_groups.clone()).unwrap_or_default();
    // every part of the project is a sibling candidate (loaded once)
    let all: Vec<(PathBuf, Part)> = if entries.is_empty() {
        Vec::new()
    } else {
        queue::discover(&[]).unwrap_or_default().into_iter().filter_map(|e| pxl::load(&e.path).ok().map(|p| (e.path, p))).collect()
    };
    let mut jparts = Vec::new();
    for e in &entries {
        let part = pxl::load(&e.path)?;
        let s = status::read(&e.path);
        out.line(format!("{} [{}]", e.path.slash(), e.state));
        if let Some(l) = e.lint {
            out.line(format!("  lint: {} errors, {} warnings, {} infos", l.errors, l.warnings, l.infos));
        }
        for n in &e.notes {
            out.line(format!("  note ({}): {}", n.by, n.text));
        }
        let root = match &a.out {
            Some(o) => PathBuf::from(o),
            None => part.dir().join("review"),
        };
        let pk = build_packet(&e.path, &part, &s, &root, a, &all, &groups)?;
        for w in &pk.warnings {
            out.warn(format!("{}: {}", part.name, w));
        }
        out.line(format!("  packet: {}", pk.dir.slash()));
        out.line(format!("  review.md: {}", pk.md.slash()));
        out.line(format!("  {} compare page(s), {} zoom page(s), lineup {} page(s), {} sibling(s)", pk.sheets.len(), pk.zooms.len(), pk.lineup.len(), pk.siblings.len()));
        for (n, r) in &pk.siblings {
            out.line(format!("  sibling: {} [{}]", n, r));
        }
        let ranked: Vec<&(String, Option<f64>, Option<f64>, Option<(u32, u32)>, Option<f64>)> = {
            let mut v: Vec<_> = pk.frames.rows.iter().filter(|r| r.4.is_some()).collect();
            v.sort_by(|x, y| y.4.partial_cmp(&x.4).unwrap_or(std::cmp::Ordering::Equal));
            v.into_iter().take(a.worst).collect()
        };
        for r in &ranked {
            out.line(format!("  worst {}: iou {} oklab {} lint {} score {}", r.0, fmt_opt(r.1, 1, 100.0) + "%", fmt_opt(r.2, 3, 1.0), r.3.map_or("-".into(), |(e, w)| format!("{}/{}", e, w)), fmt_opt(r.4, 1, 1.0)));
        }
        let raw = status::raw_audit(&s, &part);
        let raw_n: usize = raw.iter().map(|r| r.count).sum();
        if raw_n > 0 {
            out.line(format!("  raw: {} (not hand-finished): {}", raw_n, raw.iter().filter(|r| r.count > 0).map(|r| r.frame.clone()).collect::<Vec<_>>().join(" ")));
        }
        jparts.push(json!({"path": e.path.slash(), "name": e.name, "state": e.state, "notes": e.notes, "lint": e.lint,
            "packet": pk.dir.slash(), "review_md": pk.md.slash(),
            "sheets": pk.sheets, "zoom": pk.zooms, "lineup": pk.lineup,
            "siblings": pk.siblings.iter().map(|(n, r)| json!({"part": n, "why": r})).collect::<Vec<_>>(),
            "raw_frames": raw.iter().filter(|r| r.count > 0).map(|r| json!({"frame": r.frame, "raw": r.count, "of": r.total, "tiles": r.tiles})).collect::<Vec<_>>(),
            "frames": pk.frames.rows.iter().map(|r| json!({"frame": r.0, "iou": r.1, "oklab": r.2, "score": r.4, "errors": r.3.map(|x| x.0), "warnings": r.3.map(|x| x.1)})).collect::<Vec<_>>(),
            "worst": e.worst.iter().map(|(f, m)| json!({"frame": f, "metrics": m})).collect::<Vec<_>>()}));
    }
    if entries.is_empty() {
        out.line(format!("no parts in state '{}'", a.state.as_deref().unwrap_or("done")));
    }
    out.set("parts", Value::Array(jparts));
    Ok(())
}

// ------------------------------------------------------------------ verdicts

pub fn run_approve(a: &ApproveArgs, out: &mut Out) -> Result<()> {
    let path = fsutil::resolve_part(&a.part)?;
    let by = out.agent().unwrap_or("coordinator").to_string();
    let user = by == "user";
    let suffix = crate::design::approval_gate(&path, a.justify.as_deref())?;
    status::update(&path, |s| {
        s.state = "approved".into();
        s.owner = None;
        if user {
            s.approved_by = Some("user".into());
        }
        s.review.push(status::Note { by, verdict: "approved".into(), text: format!("{}{}", a.note.clone().unwrap_or_default(), suffix), ..Default::default() });
    })?;
    out.emit(format!("{} approved", path.slash()), json!({"part": path.slash(), "state": "approved"}));
    Ok(())
}

pub fn run_rework(a: &ReworkArgs, out: &mut Out) -> Result<()> {
    let path = fsutil::resolve_part(&a.part)?;
    let by = out.agent().unwrap_or("coordinator").to_string();
    let mut frames: Vec<String> = Vec::new();
    let part = pxl::load(&path);
    if !a.frames.is_empty() {
        let part = part.as_ref().map_err(|e| e.clone())?;
        let sel = FrameSel { frames: a.frames.clone(), all: false };
        frames = sel.select(part)?.iter().map(|f| f.name.clone()).collect();
    }
    let note = match &part {
        Ok(p) => status::Note::rework(&by, &a.note, &frames, p),
        Err(_) => status::Note { by, verdict: "rework".into(), text: a.note.clone(), ..Default::default() },
    };
    // frames marked finished without edit are reopened by a rework note
    if let Ok(marks) = crate::journal::finish_marks(&path) {
        let mut names: Vec<String> = marks.iter().filter(|m| frames.is_empty() || frames.contains(&m.frame)).map(|m| m.frame.clone()).collect();
        names.sort();
        names.dedup();
        if !names.is_empty() {
            let tiles: Vec<usize> = marks.iter().filter(|m| names.contains(&m.frame)).flat_map(|m| m.tiles.clone()).collect();
            let whole = marks.iter().any(|m| names.contains(&m.frame) && m.tiles.is_empty());
            crate::journal::append_unfinish(&path, &names);
            crate::edit::reopen_finished(&path, &names, if whole { &[] } else { &tiles });
        }
    }
    status::update(&path, |s| {
        s.state = "rework".into();
        s.owner = None;
        s.review.push(note);
    })?;
    let (task_ids, task_created) = crate::tasks::rework_task(&path, &a.note, out.agent().unwrap_or("coordinator"));
    // The owner's lease is void: the part goes back into the queue.
    let _ = std::fs::remove_file(crate::lease::lease_path(&path));
    let rounds = status::read(&path).rework_rounds();
    out.emit(
        format!(
            "{} rework{} (round {}); {}",
            path.slash(),
            if frames.is_empty() { String::new() } else { format!(" [{}]", frames.join(",")) },
            rounds,
            if task_created { format!("fix task {} created", task_ids.join(", ")) } else if task_ids.is_empty() { "no fix task".to_string() } else { format!("the open fix task(s) {} from the review notes carry the work; no duplicate created", task_ids.join(", ")) }
        ),
        json!({"part": path.slash(), "state": "rework", "frames": frames, "round": rounds, "tasks": task_ids, "task_created": task_created}),
    );
    Ok(())
}

// ------------------------------------------------------------------ status --rework / --metrics

/// Open rework notes of one part as JSON.
pub fn open_notes_json(s: &StatusFile, part: &Part) -> Vec<Value> {
    s.open_rework(part)
        .into_iter()
        .map(|(i, of)| {
            let n = &s.review[i];
            json!({"index": i, "by": n.by, "text": n.text, "frames": n.frames, "open_frames": of})
        })
        .collect()
}

pub fn run_rework_list(paths: &[PathBuf], out: &mut Out) {
    let mut js = Vec::new();
    for p in paths {
        let Ok(part) = pxl::load(p) else { continue };
        let s = status::read(p);
        let open = s.open_rework(&part);
        if open.is_empty() {
            continue;
        }
        out.line(format!("{} [{}] {} open note(s), rework round {}", p.slash(), s.state, open.len(), s.rework_rounds()));
        for (i, of) in &open {
            let n = &s.review[*i];
            let scope = if n.frames.is_empty() { "whole part".to_string() } else { format!("frames open {} of {}", of.join(","), n.frames.len()) };
            out.line(format!("  ({}) {}: {}", n.by, scope, n.text));
        }
        js.push(json!({"path": p.slash(), "state": s.state, "round": s.rework_rounds(), "open": open_notes_json(&s, &part)}));
    }
    if js.is_empty() {
        out.line("no open rework notes");
    }
    out.set("rework", Value::Array(js));
}

pub fn run_metrics(paths: &[PathBuf], out: &mut Out) {
    let kws = keywords(&checklist(&paths.first().and_then(|p| p.parent()).map(|d| d.to_path_buf()).unwrap_or_else(|| PathBuf::from("."))));
    let (mut first, mut once, mut twice, mut none) = (0usize, 0usize, 0usize, 0usize);
    let mut reasons: BTreeMap<String, usize> = BTreeMap::new();
    let mut per = Vec::new();
    for p in paths {
        let s = status::read(p);
        let rounds = s.rework_rounds();
        let approved = s.review.iter().any(|n| n.verdict == "approved");
        match (rounds, approved) {
            (0, true) => first += 1,
            (0, false) => none += 1,
            (1, _) => once += 1,
            _ => twice += 1,
        }
        let mut frames: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut preasons = Vec::new();
        for n in s.review.iter().filter(|n| n.verdict == "rework") {
            let r = reason_of(&n.text, &kws);
            *reasons.entry(r.clone()).or_default() += 1;
            preasons.push(r.clone());
            for f in &n.frames {
                frames.entry(f.clone()).or_default().push(r.clone());
            }
        }
        let name = p.file_stem().map_or(String::new(), |n| n.to_string_lossy().into_owned());
        if rounds > 0 {
            out.line(format!("{}: {} rework round(s): {}", name, rounds, preasons.join(", ")));
            for (f, rs) in &frames {
                out.line(format!("  {} x{} ({})", f, rs.len(), rs.join(", ")));
            }
        }
        per.push(json!({"path": p.slash(), "name": name, "rounds": rounds, "approved": approved,
            "reasons": preasons, "frames": frames.iter().map(|(f, r)| (f.clone(), json!({"rounds": r.len(), "reasons": r}))).collect::<serde_json::Map<_, _>>()}));
    }
    let mut top: Vec<(String, usize)> = reasons.into_iter().collect();
    top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    out.line(format!("{} parts: {} approved first time, {} reworked once, {} reworked twice or more, {} not reviewed yet", paths.len(), first, once, twice, none));
    if !top.is_empty() {
        out.line(format!("top rework reasons: {}", top.iter().take(8).map(|(r, n)| format!("{} {}", r, n)).collect::<Vec<_>>().join(", ")));
    }
    out.set(
        "metrics",
        json!({"parts": paths.len(), "approved_first_time": first, "reworked_once": once, "reworked_twice_plus": twice, "not_reviewed": none,
               "top_reasons": top.iter().map(|(r, n)| json!({"reason": r, "count": n})).collect::<Vec<_>>(), "keywords": kws, "per_part": per}),
    );
}

fn tile_list(t: &[usize], max: usize) -> String {
    let s: Vec<String> = t.iter().take(max).map(|x| x.to_string()).collect();
    format!("{}{}", s.join(" "), if t.len() > max { " ..." } else { "" })
}
