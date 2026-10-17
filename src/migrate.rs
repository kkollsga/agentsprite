//! `pxl migrate [--dry-run]`: rewrites an old project to the console model (M6 R1).
//!
//! * pxl.toml gets `[project] console = "gbc"` when it has none.
//! * In every part of the manifest the header key `kind` becomes `mode` (`tileset` -> `tile`), and
//!   `profile = "gbc-sprite"` / `"gbc-tile"` / `"gbc-screen"` is dropped when it equals the console and mode the part
//!   resolves to anyway, else written as `console/mode`.
//!
//! Part files change through the edit journal (command `migrate`, no pixels touched, the whole previous text kept so
//! `pxl undo` restores it byte for byte). Status hashes of the part are carried over, so the export freshness and the
//! review state of a part do not change. The command is idempotent: a second run reports nothing to do.

use crate::fsutil::PathSlash;
use crate::cli::MigrateArgs;
use crate::console;
use crate::fsutil;
use crate::journal;
use crate::manifest::Manifest;
use crate::model::PartKind;
use crate::output::{Out, PxlError, Result};
use crate::{pxl, status};
use serde_json::json;
use std::path::Path;

fn quoted(line: &str, key: &str) -> Option<String> {
    let t = line.trim();
    let rest = t.strip_prefix(key)?.trim_start().strip_prefix('=')?.trim();
    let rest = rest.strip_prefix('"')?;
    Some(rest[..rest.find('"')?].to_string())
}

/// New text of a part file and the list of changes (empty when the part is already current). Only the top-level
/// header (before the first `[table]`) is touched.
pub fn migrate_part_text(text: &str, console_id: &str) -> (String, Vec<String>) {
    let lines: Vec<&str> = text.lines().collect();
    let header_end = lines.iter().position(|l| l.trim_start().starts_with('[')).unwrap_or(lines.len());
    let find = |key: &str| (0..header_end).find(|&i| quoted(lines[i], key).is_some());
    let (ki, mi, pi) = (find("kind"), find("mode"), find("profile"));
    let kind_text = ki.and_then(|i| quoted(lines[i], "kind"));
    let mode_text = mi.and_then(|i| quoted(lines[i], "mode"));
    let kind = PartKind::from_text(mode_text.as_deref().or(kind_text.as_deref()).unwrap_or("sprite")).unwrap_or(PartKind::Sprite);
    let mut changes = Vec::new();
    let mut out: Vec<String> = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if Some(i) == ki {
            let k = kind_text.clone().unwrap_or_default();
            if mi.is_some() {
                changes.push(format!("remove `kind = \"{}\"` (mode is set)", k));
            } else if kind == PartKind::Sprite {
                changes.push(format!("remove `kind = \"{}\"` (sprite is the default mode)", k));
            } else {
                changes.push(format!("kind = \"{}\" -> mode = \"{}\"", k, kind.mode_name()));
                out.push(format!("mode = \"{}\"", kind.mode_name()));
            }
        } else if Some(i) == pi {
            let p = quoted(l, "profile").unwrap_or_default();
            match console::legacy_profile(&p) {
                Some((c, m)) if c == console_id && m == kind.mode_name() => changes.push(format!("remove profile = \"{}\" (it is {}/{}, the default)", p, c, m)),
                Some((c, m)) => {
                    changes.push(format!("profile = \"{}\" -> \"{}/{}\"", p, c, m));
                    out.push(format!("profile = \"{}/{}\"", c, m));
                }
                None => out.push(l.to_string()),
            }
        } else {
            out.push(l.to_string());
        }
    }
    if changes.is_empty() {
        return (text.to_string(), changes);
    }
    let mut s = out.join("\n");
    if text.ends_with('\n') {
        s.push('\n');
    }
    (s, changes)
}

fn rel(base: &Path, p: &Path) -> String {
    p.strip_prefix(base).unwrap_or(p).slash()
}

pub fn run(a: &MigrateArgs, out: &mut Out) -> Result<()> {
    if let Some(l) = &a.layout {
        if l != "assets" {
            return Err(PxlError::usage(format!("unknown layout '{}'", l)).with_hint("the only layout is `assets`: pxl migrate --layout assets [--dry-run]"));
        }
        return run_layout(a, out);
    }
    let m = Manifest::discover(a.manifest.as_deref())?.ok_or_else(|| PxlError::usage("`migrate` needs a pxl.toml").with_hint("run it from the project folder or pass --manifest"))?;
    let mut lines: Vec<String> = Vec::new();
    let mut files: Vec<serde_json::Value> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    let mut changed = 0usize;
    let console_id = m.console.clone().unwrap_or_else(|| console::DEFAULT_CONSOLE.to_string());
    if !console::exists(&console_id, Some(&m.dir)) {
        return Err(PxlError::usage(format!("[project] console = \"{}\" is not a known console", console_id)).with_hint("see `pxl console list`"));
    }

    // pxl.toml
    if m.console.is_none() {
        let text = fsutil::read_string(&m.path)?;
        let new = console::set_console_text(&text, &console_id);
        lines.push(format!("pxl.toml: add [project] console = \"{}\"", console_id));
        files.push(json!({"file": "pxl.toml", "changes": [format!("add [project] console = \"{}\"", console_id)]}));
        changed += 1;
        if !a.dry_run {
            fsutil::atomic_write(&m.path, new.as_bytes())?;
        }
    }

    // parts
    let mut seen = std::collections::BTreeSet::new();
    for d in &m.parts {
        if !seen.insert(d.path.clone()) {
            continue;
        }
        let Ok(raw) = fsutil::read_string(&d.path) else {
            skipped.push(format!("{}: cannot read", rel(&m.dir, &d.path)));
            continue;
        };
        let (new, ch) = migrate_part_text(&raw, &console_id);
        if ch.is_empty() {
            continue;
        }
        let name = rel(&m.dir, &d.path);
        if let Err(e) = crate::pxl::parse_str(&new, &d.path) {
            skipped.push(format!("{}: the migrated text does not parse ({}); left unchanged", name, e.msg));
            continue;
        }
        for c in &ch {
            lines.push(format!("{}: {}", name, c));
        }
        files.push(json!({"file": name, "changes": ch}));
        changed += 1;
        if a.dry_run {
            continue;
        }
        if let Err(e) = crate::lease::guard(&d.path, out.agent()) {
            skipped.push(format!("{}: {}", name, e.msg));
            continue;
        }
        let _pl = fsutil::lock_part(&d.path)?;
        let entry = journal::make_entry(out.agent(), "migrate", Vec::new(), &new, Some(raw.clone()));
        fsutil::atomic_write(&d.path, new.as_bytes())?;
        journal::append_edit(&d.path, &entry)?;
        // carry the recorded hashes over so export freshness and review state do not change
        if status::status_path(&d.path).is_file() {
            let (old_h, new_h) = (fsutil::content_hash(raw.as_bytes()), fsutil::content_hash(new.as_bytes()));
            let _ = status::update(&d.path, |s| {
                for h in [&mut s.hashes.part, &mut s.hashes.exported_part] {
                    if h.as_deref() == Some(old_h.as_str()) {
                        *h = Some(new_h.clone());
                    }
                }
            });
        }
    }

    // legacy profile files stay as they are
    let legacy_files: Vec<String> = ["gbc-sprite", "gbc-tile", "gbc-screen"].iter().map(|n| format!("profiles/{}.toml", n)).filter(|f| m.dir.join(f).is_file()).collect();
    for f in &legacy_files {
        lines.push(format!("note: {} is kept; it still replaces that profile as a whole. To override single keys use consoles/{}/<mode>.toml (see `pxl console show`)", f, console_id));
    }
    for s in &skipped {
        lines.push(format!("skipped {}", s));
    }
    let head = if changed == 0 && skipped.is_empty() {
        "nothing to migrate: the project already uses the console model".to_string()
    } else if a.dry_run {
        format!("dry run: {} file(s) would change", changed)
    } else {
        format!("migrated {} file(s)", changed)
    };
    lines.push(head);
    out.emit(lines.join("\n"), json!({"dry_run": a.dry_run, "console": console_id, "changed": changed, "files": files, "skipped": skipped, "kept_profile_files": legacy_files}));
    if !skipped.is_empty() {
        return Err(PxlError::validation(format!("{} file(s) skipped", skipped.len())).with_hint("fix the reason above and run `pxl migrate` again; finished files are not touched twice"));
    }
    Ok(())
}


// ------------------------------------------------------------------------------------ --layout assets

fn norm(p: &Path) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => {
                v.pop();
            }
            std::path::Component::CurDir => {}
            c => v.push(c.as_os_str().to_string_lossy().into_owned()),
        }
    }
    v
}

/// Relative path from folder `from` to `to` (both absolute).
fn rel_path(from: &Path, to: &Path) -> String {
    let (f, t) = (norm(from), norm(to));
    let common = f.iter().zip(&t).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<String> = vec!["..".into(); f.len() - common];
    parts.extend(t[common..].iter().cloned());
    parts.join("/")
}

/// One planned move.
#[derive(Debug)]
pub struct Move {
    pub name: String,
    pub from: std::path::PathBuf,
    pub to: std::path::PathBuf,
    pub group: String,
    /// Reasons the part cannot be moved automatically.
    pub blockers: Vec<String>,
    /// Number of path values that are rewritten for the new place.
    pub rewrites: usize,
}

fn project_of(m: &Manifest) -> std::path::PathBuf {
    let d = if m.dir.as_os_str().is_empty() { Path::new(".") } else { m.dir.as_path() };
    std::fs::canonicalize(d).unwrap_or_else(|_| d.to_path_buf())
}

fn group_of(project: &Path, from: &Path, name: &str) -> String {
    let rel = from.parent().and_then(|d| d.strip_prefix(project).ok()).map(|d| d.to_path_buf()).unwrap_or_default();
    let mut comps: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    if comps.first().map_or(false, |c| c == "parts") {
        comps.remove(0);
    }
    let stem = from.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    if comps.last().map_or(false, |c| c == name || c == stem) {
        comps.pop();
    }
    comps.join("/")
}

/// Folders that move as a whole (old folder -> new folder) and the stem of the part file inside.
#[derive(Clone, Default, Debug)]
struct Moved {
    dirs: Vec<(std::path::PathBuf, std::path::PathBuf, String)>,
}

impl Moved {
    /// Where a file of the old tree is afterwards.
    fn map(&self, abs: &Path) -> std::path::PathBuf {
        let best = self.dirs.iter().filter(|(od, _, _)| abs.starts_with(od)).max_by_key(|(od, _, _)| od.components().count());
        match best {
            Some((od, nd, stem)) => {
                let rel = abs.strip_prefix(od).unwrap_or(abs);
                // the part file and its journal / status / lease take the generic names
                let n = rel.to_string_lossy();
                for (from, to) in [(format!("{}.pxl", stem), "asset.pxl".to_string()), (format!("{}.journal", stem), "asset.journal".into()), (format!("{}.status.json", stem), "asset.status.json".into()), (format!("{}.lease", stem), "asset.lease".into())] {
                    if n == from {
                        return nd.join(to);
                    }
                }
                nd.join(rel)
            }
            None => abs.to_path_buf(),
        }
    }
}

/// Rewrite every quoted string of the part text that names an existing file (or the folder of a `{frame}` pattern) relative to
/// the old folder so that it names the same file from the new folder. Frame grids are left alone. Returns the text and the count.
fn rewrite_paths(text: &str, od: &Path, nd: &Path, moved: &Moved) -> (String, usize) {
    let mut out: Vec<String> = Vec::new();
    let mut n = 0;
    let mut in_grid = false;
    for l in text.lines() {
        if in_grid {
            if l.contains("\"\"\"") {
                in_grid = false;
            }
            out.push(l.to_string());
            continue;
        }
        if l.contains("\"\"\"") && l.matches("\"\"\"").count() == 1 {
            in_grid = true;
            out.push(l.to_string());
            continue;
        }
        let mut line = String::new();
        let mut rest = l;
        while let Some(i) = rest.find('"') {
            line.push_str(&rest[..i]);
            let after = &rest[i + 1..];
            let Some(j) = after.find('"') else {
                line.push_str(&rest[i..]);
                rest = "";
                break;
            };
            let tok = &after[..j];
            let new = new_token(tok, od, nd, moved);
            line.push('"');
            match new {
                Some(t) if t != tok => {
                    n += 1;
                    line.push_str(&t);
                }
                _ => line.push_str(tok),
            }
            line.push('"');
            rest = &after[j + 1..];
        }
        line.push_str(rest);
        out.push(line);
    }
    let mut s = out.join("\n");
    if text.ends_with('\n') {
        s.push('\n');
    }
    (s, n)
}

fn abs_of(od: &Path, tok: &str) -> Option<std::path::PathBuf> {
    let direct = od.join(tok);
    if direct.is_file() {
        return std::fs::canonicalize(&direct).ok();
    }
    // a template is also found in a parent folder
    if (tok.starts_with("templates/") || tok.ends_with(".toml")) && !tok.starts_with("../") {
        return crate::fsutil::find_up(od, tok).and_then(|f| std::fs::canonicalize(f).ok());
    }
    None
}

fn new_token(tok: &str, od: &Path, nd: &Path, moved: &Moved) -> Option<String> {
    if tok.is_empty() || tok.starts_with('#') || !(tok.contains('/') || tok.contains('.')) {
        return None;
    }
    if let Some(i) = tok.find("{frame}") {
        // `ref/{frame}.png`: the folder before the pattern
        let (dir, tail) = tok[..i].rsplit_once('/').map_or(("", &tok[..i]), |(d, t)| (d, t));
        let base = if dir.is_empty() { od.to_path_buf() } else { od.join(dir) };
        if !base.is_dir() {
            return None;
        }
        let base = std::fs::canonicalize(&base).ok()?;
        let r = rel_path(nd, &moved.map(&base));
        let rest = format!("{}{}", tail, &tok[i..]);
        return Some(if r.is_empty() { rest } else { format!("{}/{}", r, rest) });
    }
    let abs = abs_of(od, tok)?;
    Some(rel_path(nd, &moved.map(&abs)))
}

fn pxl_files_below(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let n = e.file_name().to_string_lossy().into_owned();
        if p.is_dir() {
            if n != "versions" && !n.starts_with('.') {
                pxl_files_below(&p, out);
            }
        } else if n.ends_with(".pxl") {
            out.push(p);
        }
    }
}

/// The moves of `pxl migrate --layout assets`. A part moves together with its folder (reference images, per-frame cell files,
/// compare and preview folders), so the folder may hold no other part; its `.pxl`, journal and status take the generic names.
pub fn plan_layout(m: &Manifest) -> Vec<Move> {
    let project = project_of(m);
    let path_patterns: Vec<(String, String)> = m.scenes.iter().flat_map(|s| s.groups.iter().flat_map(move |g| g.parts.iter().filter(|p| p.contains('/')).map(move |p| (s.name.clone(), p.clone())))).collect();
    struct Cand {
        name: String,
        from: std::path::PathBuf,
        to: std::path::PathBuf,
        group: String,
        stem: String,
        blockers: Vec<String>,
    }
    let mut cands: Vec<Cand> = Vec::new();
    for d in &m.parts {
        let from = std::fs::canonicalize(&d.path).unwrap_or_else(|_| d.path.clone());
        if crate::asset::is_asset_file(&from) {
            continue;
        }
        let group = group_of(&project, &from, &d.name);
        let nd = if group.is_empty() { project.join("assets").join(&d.name) } else { project.join("assets").join(&group).join(&d.name) };
        let od = from.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let mut blockers = Vec::new();
        let mut inside = Vec::new();
        pxl_files_below(&od, &mut inside);
        let others: Vec<String> = inside.iter().filter(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()) != from).map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        if !others.is_empty() {
            blockers.push(format!("its folder also holds {} (a part moves with its folder)", others.iter().take(3).cloned().collect::<Vec<_>>().join(", ")));
        }
        if od == project || nd.starts_with(&od) || od.starts_with(&nd) {
            blockers.push("its folder is the project folder or overlaps the target".into());
        }
        let rel = from.strip_prefix(&project).map(|r| r.to_string_lossy().replace('\\', "/")).unwrap_or_default();
        for (sc, pat) in &path_patterns {
            if rel == *pat || rel.starts_with(&format!("{}/", pat.trim_end_matches('/'))) {
                blockers.push(format!("scene {} lists it by path '{}' (use part keys first)", sc, pat));
            }
        }
        let stem = from.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
        cands.push(Cand { name: d.name.clone(), from, to: nd.join(crate::asset::ASSET_FILE), group, stem, blockers });
    }
    // settle which parts move: a part whose template cannot be found stays, which changes where others' references point
    let mut rewrites: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for _ in 0..3 {
        let moved = Moved { dirs: cands.iter().filter(|c| c.blockers.is_empty()).map(|c| (c.from.parent().unwrap().to_path_buf(), c.to.parent().unwrap().to_path_buf(), c.stem.clone())).collect() };
        let mut changed = false;
        for c in cands.iter_mut() {
            let od = c.from.parent().unwrap().to_path_buf();
            let nd = c.to.parent().unwrap().to_path_buf();
            match (fsutil::read_string(&c.from), pxl::load(&c.from)) {
                (Ok(text), Ok(p)) => {
                    if let Some(t) = &p.template {
                        if abs_of(&od, t).is_none() && !c.blockers.iter().any(|b| b.starts_with("template")) {
                            c.blockers.push(format!("template '{}' not found", t));
                            changed = true;
                        }
                    }
                    let own = Moved { dirs: moved.dirs.iter().cloned().collect() };
                    let (_, n) = if c.blockers.is_empty() { rewrite_paths(&text, &od, &nd, &own) } else { (String::new(), 0) };
                    rewrites.insert(c.name.clone(), n);
                }
                (_, Err(e)) => {
                    if !c.blockers.iter().any(|b| b.starts_with("does not load")) {
                        c.blockers.push(format!("does not load: {}", e.msg));
                        changed = true;
                    }
                }
                (Err(e), _) => c.blockers.push(format!("cannot read: {}", e.msg)),
            }
        }
        if !changed {
            break;
        }
    }
    cands.into_iter().map(|c| Move { rewrites: rewrites.get(&c.name).copied().unwrap_or(0), name: c.name, from: c.from, to: c.to, group: c.group, blockers: c.blockers }).collect()
}

fn run_layout(a: &MigrateArgs, out: &mut Out) -> Result<()> {
    let m = Manifest::discover(a.manifest.as_deref())?.ok_or_else(|| PxlError::usage("`migrate` needs a pxl.toml"))?;
    let plan = plan_layout(&m);
    let mut lines: Vec<String> = Vec::new();
    let mut js = Vec::new();
    let (mut movable, mut blocked) = (0usize, 0usize);
    let project = project_of(&m);
    let show = |p: &Path| p.strip_prefix(&project).unwrap_or(p).slash();
    for mv in &plan {
        if mv.blockers.is_empty() {
            movable += 1;
            lines.push(format!("{} -> {}{}", show(&mv.from), show(&mv.to), if mv.rewrites == 0 { String::new() } else { format!("  ({} path(s) rewritten)", mv.rewrites) }));
        } else {
            blocked += 1;
            lines.push(format!("{} stays: {}", show(&mv.from), mv.blockers.join("; ")));
        }
        js.push(json!({"part": mv.name, "from": show(&mv.from), "to": show(&mv.to), "group": mv.group, "movable": mv.blockers.is_empty(), "blockers": mv.blockers, "rewrites": mv.rewrites}));
    }
    let mut moved_n = 0usize;
    let mut failed: Vec<String> = Vec::new();
    if !a.dry_run {
        let mp = m.path.clone();
        crate::lease::guard(&mp, out.agent())?;
        let moved = Moved { dirs: plan.iter().filter(|m| m.blockers.is_empty()).map(|m| (m.from.parent().unwrap().to_path_buf(), m.to.parent().unwrap().to_path_buf(), m.from.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string())).collect() };
        // every new text is computed before anything moves (the tokens resolve against the old tree)
        let mut texts: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        for mv in plan.iter().filter(|m| m.blockers.is_empty()) {
            if let Ok(raw) = fsutil::read_string(&mv.from) {
                texts.insert(mv.name.clone(), rewrite_paths(&raw, mv.from.parent().unwrap(), mv.to.parent().unwrap(), &moved).0);
            }
        }
        // parts that stay but point into a moved folder (`animated`, shared images) are re-pointed
        let mut repoint: Vec<(std::path::PathBuf, String, String, (String, String))> = Vec::new();
        for mv in plan.iter().filter(|m| !m.blockers.is_empty()) {
            if let Ok(raw) = fsutil::read_string(&mv.from) {
                let od = mv.from.parent().unwrap();
                let (t, n) = rewrite_paths(&raw, od, od, &moved);
                if n > 0 {
                    if let Ok(p) = pxl::parse_str(&raw, &mv.from) {
                        repoint.push((mv.from.clone(), raw, t, status::inputs_snapshot(&p)));
                    }
                }
            }
        }
        for mv in plan.iter().filter(|m| m.blockers.is_empty()) {
            let Some(t) = texts.get(&mv.name) else {
                failed.push(format!("{}: cannot read", mv.name));
                continue;
            };
            match apply_move(&m, mv, t, out) {
                Ok(()) => moved_n += 1,
                Err(e) => failed.push(format!("{}: {}", mv.name, e.msg)),
            }
        }
        for (path, raw, t, snap) in &repoint {
            let entry = journal::make_entry(out.agent(), "migrate", Vec::new(), t, Some(raw.clone()));
            match crate::versions::suppressed(|| fsutil::atomic_write(path, t.as_bytes())) {
                Ok(()) => {
                    let _ = journal::append_edit(path, &entry);
                    if let Ok(np) = pxl::parse_str(t, path) {
                        let (old_h, new_h) = (fsutil::content_hash(raw.as_bytes()), fsutil::content_hash(t.as_bytes()));
                        let _ = status::update(path, |s| {
                            for h in [&mut s.hashes.part, &mut s.hashes.exported_part] {
                                if h.as_deref() == Some(old_h.as_str()) {
                                    *h = Some(new_h.clone());
                                }
                            }
                            status::rebase_inputs(s, snap, &np);
                        });
                    }
                    lines.push(format!("{} stays; its references into moved folders were updated", show(path)));
                }
                Err(e) => failed.push(format!("{}: {}", show(path), e.msg)),
            }
        }
    }
    lines.push(format!(
        "{}: {} part(s) of {} {}, {} stay where they are (see above); `out` paths, export files and scenes are not touched",
        if a.dry_run { "dry run" } else { "migrated" },
        if a.dry_run { movable } else { moved_n },
        plan.len(),
        if a.dry_run { "would move" } else { "moved" },
        blocked + failed.len()
    ));
    for f in &failed {
        lines.push(format!("failed {}", f));
    }
    out.emit(lines.join("\n"), json!({"dry_run": a.dry_run, "layout": "assets", "parts": js, "movable": movable, "blocked": blocked, "moved": moved_n, "failed": failed, "checked": {"ok": plan.len(), "of": m.parts.len()}}));
    if !failed.is_empty() {
        return Err(PxlError::validation(format!("{} move(s) failed", failed.len())));
    }
    Ok(())
}

fn apply_move(m: &Manifest, mv: &Move, new_text: &str, out: &Out) -> Result<()> {
    let raw = fsutil::read_string(&mv.from)?;
    let old_part = pxl::parse_str(&raw, &mv.from)?;
    let old_inputs = status::inputs_snapshot(&old_part);
    let od = mv.from.parent().unwrap().to_path_buf();
    let nd = mv.to.parent().unwrap().to_path_buf();
    if nd.exists() {
        return Err(PxlError::validation(format!("{} exists", nd.slash())));
    }
    crate::lease::guard(&mv.from, out.agent())?;
    // other parts' tokens were computed against the final places, so move the whole folder
    std::fs::create_dir_all(nd.parent().unwrap())?;
    fsutil::release_locks_in(&od);
    std::fs::rename(&od, &nd).map_err(|e| PxlError::io(format!("cannot move {}: {}", od.slash(), e)))?;
    let stem = mv.from.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
    for (a, b) in [(format!("{}.pxl", stem), "asset.pxl"), (format!("{}.journal", stem), "asset.journal"), (format!("{}.status.json", stem), "asset.status.json"), (format!("{}.lease", stem), "asset.lease")] {
        let (a, b) = (nd.join(a), nd.join(b));
        if a.is_file() {
            std::fs::rename(&a, &b).map_err(|e| PxlError::io(format!("cannot rename {}: {}", a.slash(), e)))?;
        }
    }
    let entry = journal::make_entry(out.agent(), "migrate", Vec::new(), new_text, Some(raw.clone()));
    crate::versions::suppressed(|| fsutil::atomic_write(&mv.to, new_text.as_bytes()))?;
    journal::append_edit(&mv.to, &entry)?;
    if status::status_path(&mv.to).is_file() {
        if let Ok(new_part) = pxl::parse_str(new_text, &mv.to) {
            let (old_h, new_h) = (fsutil::content_hash(raw.as_bytes()), fsutil::content_hash(new_text.as_bytes()));
            let _ = status::update(&mv.to, |s| {
                for h in [&mut s.hashes.part, &mut s.hashes.exported_part] {
                    if h.as_deref() == Some(old_h.as_str()) {
                        *h = Some(new_h.clone());
                    }
                }
                status::rebase_inputs(s, &old_inputs, &new_part);
            });
        }
    }
    // the baseline version and compare.png (no project hooks: nothing is rebuilt)
    let _ = crate::versions::store(&mv.to, new_text.as_bytes());
    let text = fsutil::read_string(&m.path)?;
    let rel = mv.to.strip_prefix(project_of(m)).unwrap_or(&mv.to).to_string_lossy().replace('\\', "/");
    let new = crate::asset::set_part_key(&text, &mv.name, "path", &crate::asset::q(&rel));
    fsutil::atomic_write(&m.path, new.as_bytes())?;
    Ok(())
}
