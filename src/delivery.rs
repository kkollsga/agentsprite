//! `pxl delivery`: fills the client's DELIVERY_TEMPLATE.md (M5 phase T4).
//!
//! * header table: studio, delivery number, date, contact from the options (the date defaults to today);
//! * "Assets delivered": one row per checklist ID (read from CHECKLIST.md next to the template when it exists, by folder
//!   prefix), `complete` when every part of the folder is approved/done with 0 lint errors, else `partial`;
//! * "Checker output": the tail of `--checker FILE`, or of `check_assets.py` run in `--package DIR`;
//! * "Warnings and agreed exceptions": the rows of `--exceptions FILE` (3 columns Folder | Change | Reason are mapped to the
//!   template's ID | File | Exception | Agreed by / date);
//! * added before "Open questions": the per-scene budget table (the same checks as `pxl check`) and the parts list from
//!   `pxl status` (state, frames, lint errors, export freshness).

use crate::cli::DeliveryArgs;
use crate::lint::{self, LintOpts, Severity};
use crate::manifest::Manifest;
use crate::output::{Out, PxlError, Result};
use crate::{fsutil, profile, pxl, scene, status};
use serde_json::json;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

fn civil_today() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let z = secs / 86400 + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:04}-{:02}-{:02}", if m <= 2 { y + 1 } else { y }, m, d)
}

/// `| A-041 | \`zones/zone_2\` | ...` rows of a checklist: (id, folder).
fn checklist(dir: &Path) -> Vec<(String, String)> {
    let Ok(t) = std::fs::read_to_string(dir.join("CHECKLIST.md")) else { return Vec::new() };
    t.lines()
        .filter_map(|l| {
            let c: Vec<&str> = l.split('|').map(|x| x.trim()).collect();
            (c.len() > 2 && c[1].starts_with("A-")).then(|| (c[1].to_string(), c[2].trim_matches('`').to_string()))
        })
        .collect()
}

/// Rows of the first table of a markdown file (header and separator dropped).
fn table_rows(text: &str) -> Vec<Vec<String>> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    for l in text.lines().filter(|l| l.trim_start().starts_with('|')) {
        let c: Vec<String> = l.trim().trim_matches('|').split('|').map(|x| x.trim().to_string()).collect();
        if c.iter().all(|x| x.chars().all(|ch| ch == '-' || ch == ':' || ch == ' ')) {
            continue;
        }
        rows.push(c);
    }
    if !rows.is_empty() {
        rows.remove(0);
    }
    rows
}

fn rel_folder(m: &Manifest, d: &crate::manifest::PartDecl) -> String {
    let base = match &m.out_root {
        Some(r) => m.dir.join(r),
        None => m.dir.clone(),
    };
    let norm = |p: &Path| -> PathBuf {
        let mut o = PathBuf::new();
        for c in p.components() {
            match c {
                std::path::Component::ParentDir => {
                    o.pop();
                }
                std::path::Component::CurDir => {}
                c => o.push(c),
            }
        }
        o
    };
    norm(&d.spec.dir).strip_prefix(norm(&base)).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_else(|_| d.spec.dir.to_string_lossy().replace('\\', "/"))
}

fn replace_row(text: &mut String, after_heading: &str, placeholder: &str, rows: &[String]) {
    let Some(h) = text.find(after_heading) else { return };
    let Some(off) = text[h..].find(placeholder) else { return };
    let at = h + off;
    let end = at + placeholder.len();
    text.replace_range(at..end, &rows.join("\n"));
}

pub fn run(a: &DeliveryArgs, out: &mut Out) -> Result<()> {
    let tpath = Path::new(&a.template);
    let mut text = fsutil::read_string(tpath)?;
    let m = Manifest::discover(a.manifest.as_deref())?.ok_or_else(|| PxlError::usage("`delivery` needs a pxl.toml").with_hint("run it from the project folder or pass --manifest"))?;
    let tdir = tpath.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let ids = checklist(&tdir);
    // ---- header
    let fill = |text: &mut String, key: &str, v: &str| {
        let pat = format!("| {} | |", key);
        if let Some(i) = text.find(&pat) {
            text.replace_range(i..i + pat.len(), &format!("| {} | {} |", key, v));
        }
    };
    fill(&mut text, "Studio", a.studio.as_deref().unwrap_or(""));
    fill(&mut text, "Delivery number", a.number.as_deref().unwrap_or(""));
    let date = a.date.clone().unwrap_or_else(civil_today);
    fill(&mut text, "Date", &date);
    fill(&mut text, "Contact", a.contact.as_deref().unwrap_or(""));
    // ---- per part facts
    struct PartInfo {
        name: String,
        folder: String,
        state: String,
        frames: usize,
        lint_errors: usize,
        export: &'static str,
    }
    let mut infos: Vec<PartInfo> = Vec::new();
    for d in &m.parts {
        let Ok(p) = pxl::load(&d.path) else {
            infos.push(PartInfo { name: d.name.clone(), folder: rel_folder(&m, d), state: "unreadable".into(), frames: 0, lint_errors: 1, export: "none" });
            continue;
        };
        let errors = profile::load_for(&p, None).map(|prof| lint::lint_part_auto(&p, &prof, &LintOpts::default()).iter().filter(|i| i.severity == Severity::Error).count()).unwrap_or(1);
        let s = status::read(&d.path);
        let fresh = status::export_freshness(&s, &p, &status::part_file_hash(&p));
        let state = if s.state.is_empty() { "pending".to_string() } else { s.state.clone() };
        infos.push(PartInfo { name: d.name.clone(), folder: rel_folder(&m, d), state, frames: p.frames.len(), lint_errors: errors, export: fresh });
    }
    // ---- assets
    let id_of = |folder: &str| -> String {
        ids.iter().filter(|(_, f)| folder == f || folder.starts_with(&format!("{}/", f))).max_by_key(|(_, f)| f.len()).map(|(i, _)| i.clone()).unwrap_or_else(|| "-".into())
    };
    let mut by_id: BTreeMap<(String, String), Vec<&PartInfo>> = BTreeMap::new();
    for i in &infos {
        let id = id_of(&i.folder);
        let key = if id == "-" { i.folder.clone() } else { ids.iter().find(|(x, _)| *x == id).map(|(_, f)| f.clone()).unwrap() };
        by_id.entry((id, key)).or_default().push(i);
    }
    let mut asset_rows = Vec::new();
    for ((id, folder), v) in &by_id {
        let complete = v.iter().all(|i| (i.state == "approved" || i.state == "done") && i.lint_errors == 0);
        let frames: usize = v.iter().map(|i| i.frames).sum();
        let mut states: BTreeMap<&str, usize> = BTreeMap::new();
        for i in v {
            *states.entry(i.state.as_str()).or_insert(0) += 1;
        }
        let st = states.iter().map(|(k, n)| format!("{} {}", n, k)).collect::<Vec<_>>().join(", ");
        asset_rows.push(format!("| {} | `{}` | {} | {} parts, {} frames ({}) |", id, folder, if complete { "complete" } else { "partial" }, v.len(), frames, st));
    }
    replace_row(&mut text, "## Assets delivered", "| A-0xx | | | |", &asset_rows);
    // ---- checker output
    let mut checker = String::new();
    if let Some(f) = &a.checker {
        checker = fsutil::read_string(Path::new(f))?;
    } else if let Some(pk) = &a.package {
        let script = Path::new(pk).join("check_assets.py");
        if script.is_file() {
            let o = std::process::Command::new("python3").arg(&script).arg(pk).output().map_err(|e| PxlError::io(format!("cannot run python3: {}", e)))?;
            checker = String::from_utf8_lossy(&o.stdout).into_owned() + &String::from_utf8_lossy(&o.stderr);
        }
    }
    let tail: Vec<&str> = {
        let lines: Vec<&str> = checker.lines().filter(|l| !l.trim().is_empty()).collect();
        match lines.iter().position(|l| l.starts_with("Checked")) {
            Some(i) => lines[i..].to_vec(),
            None => lines.iter().rev().take(4).rev().copied().collect(),
        }
    };
    let block = if tail.is_empty() { "(not run: pass --checker FILE or --package DIR)".to_string() } else { tail.join("\n") };
    if let Some(h) = text.find("## Checker output") {
        if let Some(s) = text[h..].find("```") {
            let s0 = h + s + 3;
            if let Some(e) = text[s0..].find("```") {
                text.replace_range(s0..s0 + e, &format!("\n{}\n", block));
            }
        }
    }
    // ---- exceptions
    let mut exc_rows: Vec<String> = Vec::new();
    if let Some(f) = &a.exceptions {
        let rows = table_rows(&fsutil::read_string(Path::new(f))?);
        let mapped: Vec<String> = rows
            .iter()
            .map(|c| {
                if c.len() >= 4 {
                    format!("| {} | {} | {} | {} |", c[0], c[1], c[2], c[3])
                } else {
                    let folder = c.first().cloned().unwrap_or_default();
                    let folder_clean = folder.trim_matches('`').split_whitespace().next().unwrap_or("").to_string();
                    let exc = match (c.get(1), c.get(2)) {
                        (Some(x), Some(y)) => format!("{} Reason: {}", x, y),
                        (Some(x), None) => x.clone(),
                        _ => String::new(),
                    };
                    format!("| {} | {} | {} | |", id_of(&folder_clean), folder, exc)
                }
            })
            .collect();
        exc_rows = mapped;
    }
    for (n, reason, _) in scene::exempt_parts(&m) {
        let folder = infos.iter().find(|i| i.name == n).map(|i| i.folder.clone()).unwrap_or_default();
        exc_rows.push(format!("| {} | {} | Budget exempt: {} (not in any scene) | |", id_of(&folder), if folder.is_empty() { n.clone() } else { folder }, reason.replace('|', "/")));
    }
    if !exc_rows.is_empty() {
        replace_row(&mut text, "## Warnings and agreed exceptions", "| | | | |", &exc_rows);
    }
    // ---- scene budgets and parts
    let mut extra = String::new();
    let mut scene_json = Vec::new();
    let mut vram_rows: Vec<String> = Vec::new();
    let mut issues = scene::project_issues(&m);
    if !m.scenes.is_empty() {
        let (sp, tp) = scene::hardware(&m)?;
        let mut cache = HashMap::new();
        extra.push_str("## Hardware budgets per scene\n\nShared resources on the Game Boy Color, checked per scene by `pxl check` (palettes, tiles with flips free, sprite VRAM, DMA).\n\n| Scene | Check | Used | Limit | Result | Note |\n|---|---|---|---|---|---|\n");
        for sc in &m.scenes {
            let r = scene::check_scene(&m, sc, &sp, &tp, &mut cache);
            for row in &r.rows {
                extra.push_str(&format!("| {} | {} | {} | {} | {} | {} |\n", sc.name, row.item, row.used, row.limit, if row.ok { "ok" } else { "OVER" }, row.note.replace('|', "/")));
            }
            for w in &r.warnings {
                issues.push(format!("scene {}: {}", sc.name, w));
            }
            scene_json.push(r.to_json());
            if let Some(v) = &r.vram {
                vram_rows.push(format!("| {} | {} / {} | {} | {} | {} | {} (+{}) | {} / {} |", sc.name, v.bg_used, v.bg_budget, v.window, v.resident_cells, v.streamed_cells, v.effective_cells, v.extra_cells, v.total_tiles, v.total_limit));
            }
        }
        extra.push('\n');
        extra.push_str("### VRAM per scene (8x8 tiles; sprite columns in 8x16 cells)\n\nBG tiles a scene does not use may hold sprite cells (the 0x8800-0x8FFF block is addressable by sprites and background); a scene with extra sprite cells has a VRAM split that differs from the default 348 / 64 / 178.\n\n| Scene | BG level + animated | Window | Sprite resident | Sprite streamed (x2) | Effective sprite cells (+extra) | Total tiles / 768 |\n|---|---|---|---|---|---|---|\n");
        extra.push_str(&vram_rows.join("\n"));
        extra.push_str("\n\n");
    }
    if !issues.is_empty() {
        extra.push_str("Open findings:\n\n");
        for i in &issues {
            extra.push_str(&format!("- {}\n", i));
        }
        extra.push('\n');
    }
    extra.push_str("## Parts\n\n| Part | Folder | State | Frames | Lint errors | Export |\n|---|---|---|---|---|---|\n");
    for i in &infos {
        extra.push_str(&format!("| {} | `{}` | {} | {} | {} | {} |\n", i.name, i.folder, i.state, i.frames, i.lint_errors, i.export));
    }
    extra.push('\n');
    match text.find("## Open questions") {
        Some(i) => text.insert_str(i, &extra),
        None => text.push_str(&format!("\n{}", extra)),
    }
    let outp = a.out.clone().unwrap_or_else(|| "DELIVERY.md".into());
    fsutil::atomic_write(Path::new(&outp), text.as_bytes())?;
    out.line(format!("wrote {}: {} asset rows, {} parts, {} scene(s){}", outp, asset_rows.len(), infos.len(), m.scenes.len(), if issues.is_empty() { String::new() } else { format!(", {} open finding(s)", issues.len()) }));
    out.set("schema", json!(1));
    out.set("delivery", json!({"file": outp, "assets": asset_rows.len(), "parts": infos.len(), "scenes": scene_json, "findings": issues}));
    Ok(())
}
