//! Work queue and coordinator commands (DESIGN 6.2, 6.3). Owner: queue worker.
//!
//! Part discovery: explicit arguments (files, dirs, globs) win; otherwise the nearest `pxl.toml` (current folder or
//! above, via `manifest::Manifest::discover`; part paths are relative to the folder holding it) with a
//! `[parts]` table (`name = "path.pxl"` or `[parts.name]` with `path`, optional `priority` (int, default 0)
//! and `tags` (list)); otherwise every `**/*.pxl` below the current folder. Tag matching (`--tag`): a manifest
//! tag or any folder name of the part's path equals the tag.
//!
//! `next` order: priority descending, then worst frame score (`metrics.<frame>.score`, higher = worse; a part
//! without scores counts as worst), then path. The first candidate whose lease can be taken wins.

use crate::fsutil::PathSlash;
use crate::cli::*;
use crate::fsutil;
use crate::lease;
use crate::model::{Part, PartKind};
use crate::output::{Out, PxlError, Result};
use crate::pxl;
use crate::status::{self, StatusFile};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub(crate) struct Entry {
    pub(crate) path: PathBuf,
    priority: i64,
    tags: Vec<String>,
}

/// Entries of the nearest `pxl.toml` (same discovery as `export`: paths are relative to the folder holding it, so
/// queue and export agree from any working folder). A manifest that fails to parse is reported by `export`/`check`;
/// the queue falls back to scanning folders so status keeps working.
fn manifest_entries() -> Option<Vec<Entry>> {
    let m = crate::manifest::Manifest::discover(None).ok()??;
    let mut v: Vec<Entry> = m
        .parts
        .iter()
        .filter(|d| d.path.is_file())
        .map(|d| Entry { path: d.path.clone(), priority: d.priority, tags: d.tags.clone() })
        .collect();
    v.sort_by(|a, b| a.path.cmp(&b.path));
    Some(v)
}

pub(crate) fn discover(args: &[String]) -> Result<Vec<Entry>> {
    if args.is_empty() {
        if let Some(m) = manifest_entries().filter(|m| !m.is_empty()) {
            return Ok(m);
        }
        return Ok(fsutil::expand_parts(&[".".to_string()]).unwrap_or_default().into_iter().map(plain).collect());
    }
    let paths = fsutil::expand_parts(args)?;
    let man = manifest_entries().unwrap_or_default();
    Ok(paths
        .into_iter()
        .map(|p| match man.iter().find(|m| same(&m.path, &p)) {
            Some(m) => Entry { path: p, priority: m.priority, tags: m.tags.clone() },
            None => plain(p),
        })
        .collect())
}

fn plain(p: PathBuf) -> Entry {
    let p = p.strip_prefix("./").map(|x| x.to_path_buf()).unwrap_or(p);
    Entry { path: p, priority: 0, tags: Vec::new() }
}

fn same(a: &Path, b: &Path) -> bool {
    crate::manifest::canon(a) == crate::manifest::canon(b)
}

fn has_tag(e: &Entry, tag: &str) -> bool {
    e.tags.iter().any(|t| t == tag)
        || e.path.parent().map_or(false, |d| d.components().any(|c| c.as_os_str().to_string_lossy() == tag))
}

fn require_agent(out: &Out, cmd: &str) -> Result<String> {
    out.agent().map(String::from).ok_or_else(|| PxlError::usage(format!("`{}` needs --agent <name>", cmd)))
}

fn lease_json(i: &Option<lease::LeaseInfo>) -> Value {
    match i {
        Some(i) => json!({"agent": i.lease.agent, "since": i.lease.since, "idle_s": (i.idle * 10.0).round() / 10.0, "live": i.live}),
        None => Value::Null,
    }
}

fn metrics_text(s: &StatusFile, n: usize) -> String {
    let r = s.ranked();
    if r.is_empty() {
        return "no metrics yet".into();
    }
    r.iter().take(n).map(|(f, v)| format!("{}={:.3}", f, v)).collect::<Vec<_>>().join(" ")
}

fn claim_status(path: &Path, agent: &str) {
    let _ = status::update(path, |s| {
        if s.state != "approved" {
            s.state = "in_progress".into();
        }
        s.owner = Some(agent.to_string());
    });
}

fn claim_report(path: &Path, agent: &str, out: &mut Out) {
    let s = status::read(path);
    if crate::asset::is_asset_file(path) {
        let home = crate::tasks::home_of(path);
        let name = crate::asset::asset_name(&crate::asset::dir_of(path));
        let open: Vec<_> = crate::tasks::load_all(&home).into_iter().filter(|t| t.asset == name && t.status == "open" && !crate::tasks::blocked(&home, t) && t.assign.as_deref().map_or(true, |a| a == agent)).collect();
        if !open.is_empty() {
            let lines: Vec<String> = open.iter().map(|t| format!("{} [{}] {}", t.id, t.kind, t.text)).collect();
            out.line(format!("open tasks: {}", lines.join(" | ")));
            out.set("tasks", json!(open.iter().map(|t| json!({"id": t.id, "kind": t.kind, "text": t.text})).collect::<Vec<_>>()));
        }
    }
    let task = if s.review.last().map_or(false, |n| n.verdict == "rework") { "rework" } else { "work" };
    let notes: Vec<String> = s
        .review
        .iter()
        .filter(|n| !n.text.is_empty())
        .map(|n| if n.frames.is_empty() { n.text.clone() } else { format!("[{}] {}", n.frames.join(","), n.text) })
        .collect();
    out.emit(
        format!(
            "claimed {} for {}\ntask: {}{}\nmetrics: {}",
            path.slash(),
            agent,
            task,
            if notes.is_empty() { String::new() } else { format!(" (review notes: {})", notes.join(" | ")) },
            metrics_text(&s, 5)
        ),
        json!({"part": path.slash(), "agent": agent, "task": task, "state": "in_progress",
               "metrics": s.metrics, "review": s.review}),
    );
}

pub fn run_claim(a: &PartArgs, out: &mut Out) -> Result<()> {
    let agent = require_agent(out, "claim")?;
    crate::plan::gate()?;
    let path = fsutil::resolve_part(&a.part)?;
    if let Ok(Some(m)) = crate::manifest::Manifest::discover(None) {
        let name = if crate::asset::is_asset_file(&path) { crate::asset::asset_name(&crate::asset::dir_of(&path)) } else { crate::asset::name_of(&path) };
        crate::milestone::allows(&m.dir, &name)?;
    }
    if status::read(&path).state == "approved" {
        return Err(PxlError::validation(format!("{} is approved", path.slash())).with_hint("send it back with `pxl rework <part> --note ...` first"));
    }
    lease::acquire(&path, &agent)?;
    claim_status(&path, &agent);
    claim_report(&path, &agent, out);
    Ok(())
}

pub fn run_renew(a: &PartArgs, out: &mut Out) -> Result<()> {
    let agent = require_agent(out, "renew")?;
    let path = fsutil::resolve_part(&a.part)?;
    lease::renew(&path, &agent)?;
    out.emit(format!("renewed {} for {}", path.slash(), agent), json!({"part": path.slash(), "agent": agent}));
    Ok(())
}

pub fn run_release(a: &ReleaseArgs, out: &mut Out) -> Result<()> {
    let agent = require_agent(out, "release")?;
    let path = fsutil::resolve_part(&a.part)?;
    if a.state == Some(ReleaseState::Blocked) && a.note.is_none() {
        return Err(PxlError::usage("`release --state blocked` needs --note \"why\"").with_hint("e.g. pxl release PART --state blocked --note \"wrong palette line\""));
    }
    lease::release(&path, &agent)?;
    let state = match a.state {
        Some(ReleaseState::Done) => "done",
        Some(ReleaseState::Blocked) => "blocked",
        None => "todo",
    };
    status::update(&path, |s| {
        // Without --state the part goes back to the queue, but never downgrades a verdict.
        if a.state.is_some() || s.state == "in_progress" {
            s.state = state.to_string();
        }
        s.owner = None;
        if let Some(n) = &a.note {
            s.notes.push(status::Note { by: agent.clone(), text: n.clone(), ..Default::default() });
        }
    })?;
    let st = status::read(&path).state;
    out.emit(format!("released {} ({})", path.slash(), st), json!({"part": path.slash(), "state": st}));
    Ok(())
}

pub fn run_approve(a: &ApproveArgs, out: &mut Out) -> Result<()> {
    crate::review::run_approve(a, out)
}

pub fn run_rework(a: &ReworkArgs, out: &mut Out) -> Result<()> {
    crate::review::run_rework(a, out)
}

// ------------------------------------------------------------------ next

fn eligible(s: &StatusFile, l: &Option<lease::LeaseInfo>) -> bool {
    let live = l.as_ref().map_or(false, |i| i.live);
    match s.state.as_str() {
        "todo" | "rework" => !live,
        "in_progress" => !live,
        _ => false,
    }
}

pub fn run_next(a: &NextArgs, out: &mut Out) -> Result<()> {
    let agent = require_agent(out, "next")?;
    crate::plan::gate()?;
    let args: Vec<String> = a.dir.iter().cloned().collect();
    let entries = discover(&args)?;
    let gate = crate::manifest::Manifest::discover(None).ok().flatten().and_then(|m| crate::milestone::gate_for(&m.dir));
    let mut gated_out = 0usize;
    struct Cand {
        e: Entry,
        score: f64,
        /// Open tasks for this agent (assigned to it, or to nobody): such assets come first.
        tasks: u8,
    }
    let mut cands: Vec<Cand> = Vec::new();
    for e in entries {
        if let Some(t) = &a.tag {
            if !has_tag(&e, t) {
                continue;
            }
        }
        let s = status::read(&e.path);
        if !eligible(&s, &lease::read(&e.path)) {
            continue;
        }
        if let Some(g) = &gate {
            let name = if crate::asset::is_asset_file(&e.path) { crate::asset::asset_name(&crate::asset::dir_of(&e.path)) } else { crate::asset::name_of(&e.path) };
            if !g.allows(&name) {
                gated_out += 1;
                continue;
            }
        }
        if let Some(k) = a.kind {
            let want = match k {
                KindArg::Sprite => PartKind::Sprite,
                KindArg::Tileset => PartKind::Tileset,
            };
            match pxl::load(&e.path) {
                Ok(p) if p.kind == want => {}
                _ => continue,
            }
        }
        if crate::asset::is_asset_file(&e.path) && crate::tasks::all_blocked(&crate::tasks::home_of(&e.path), &crate::asset::asset_name(&crate::asset::dir_of(&e.path))) {
            continue; // waits for a decision
        }
        let score = s.worst().map_or(f64::MAX, |w| w.1);
        let tasks = if crate::asset::is_asset_file(&e.path) { crate::tasks::rank_for(&crate::tasks::home_of(&e.path), &crate::asset::asset_name(&crate::asset::dir_of(&e.path)), &agent) } else { 0 };
        cands.push(Cand { e, score, tasks });
    }
    cands.sort_by(|x, y| {
        y.tasks
            .cmp(&x.tasks)
            .then(y.e.priority.cmp(&x.e.priority))
            .then(y.score.partial_cmp(&x.score).unwrap_or(std::cmp::Ordering::Equal))
            .then(x.e.path.cmp(&y.e.path))
    });
    for c in cands {
        match lease::acquire(&c.e.path, &agent) {
            Ok(_) => {
                // Re-check under the lease: another agent may have finished it between scan and claim.
                let s = status::read(&c.e.path);
                if !matches!(s.state.as_str(), "todo" | "rework" | "in_progress") {
                    let _ = lease::release(&c.e.path, &agent);
                    continue;
                }
                claim_status(&c.e.path, &agent);
                claim_report(&c.e.path, &agent, out);
                return Ok(());
            }
            Err(e) if e.exit_code() == 1 => continue,
            Err(e) => return Err(e),
        }
    }
    if let (Some(g), true) = (&gate, gated_out > 0) {
        return Err(PxlError::validation(format!("no work is handed out yet: {} asset(s) wait for the gates", gated_out)).with_hint(format!("{} (only the style frames are handed out before the gates pass: pxl style mark ASSET)", g.status.text().lines().map(|l| l.trim()).collect::<Vec<_>>().join("; "))));
    }
    out.emit("no work available", json!({"part": Value::Null}));
    Ok(())
}

// ------------------------------------------------------------------ status

struct Row {
    path: PathBuf,
    json: Value,
    stale: bool,
    state: String,
    line: String,
    raw: Vec<status::RawFrame>,
}

fn build_row(path: &PathBuf) -> Row {
    let s = status::read(path);
    let l = lease::read(path);
    let name = crate::asset::display_name(path);
    let part: Option<Part> = pxl::load(path).ok();
    let (total, remaining, stale, fresh) = match &part {
        Some(p) => {
            let ph = status::part_file_hash(p);
            (
                p.frames.len(),
                p.frames.iter().filter(|f| !f.locked).count(),
                status::is_stale(&s, p),
                status::export_freshness(&s, p, &ph),
            )
        }
        None => (0, 0, false, "none"),
    };
    let worst = s.worst();
    let lint = s.lint.unwrap_or_default();
    let raw = part.as_ref().map(|p| status::raw_audit(&s, p)).unwrap_or_default();
    let raw_n: usize = raw.iter().map(|r| r.count).sum();
    let j = json!({
        "path": path.slash(), "name": name, "state": s.state, "owner": s.owner,
        "lease": lease_json(&l), "frames_total": total, "frames_remaining": remaining,
        "worst": worst.as_ref().map(|w| json!({"frame": w.0, "score": w.1})),
        "lint": s.lint, "stale": stale, "export": fresh, "raw": raw_n,
        "raw_frames": raw.iter().filter(|r| r.count > 0).map(|r| json!({"frame": r.frame, "raw": r.count, "of": r.total, "tiles": r.tiles})).collect::<Vec<_>>(),
        "notes": s.notes, "rework_rounds": s.rework_rounds(),
        "open_rework": part.as_ref().map(|p| crate::review::open_notes_json(&s, p)).unwrap_or_default(),
        "error": if part.is_none() { json!("cannot load part") } else { Value::Null },
    });
    let lease_txt = match &l {
        Some(i) if i.live => format!("{}({:.0}s)", i.lease.agent, i.idle),
        Some(i) => format!("{}(expired)", i.lease.agent),
        None => "-".into(),
    };
    let line = format!(
        "{:<28} {:<11} {:<16} {:>3}/{:<4} {:<22} lint {}/{}/{} {} raw {}{}",
        path.slash(),
        s.state,
        lease_txt,
        remaining,
        total,
        worst.map_or("-".to_string(), |w| format!("{}={:.3}", w.0, w.1)),
        lint.errors,
        lint.warnings,
        lint.infos,
        fresh,
        raw_n,
        if stale { " STALE" } else { "" }
    );
    Row { path: path.clone(), json: j, stale, state: s.state.clone(), line, raw }
}

pub fn run_status(a: &StatusArgs, out: &mut Out) -> Result<()> {
    let entries = discover(&a.parts)?;
    let paths: Vec<PathBuf> = entries
        .into_iter()
        .filter(|e| a.tag.as_deref().map_or(true, |t| has_tag(e, t)))
        .filter(|e| {
            a.group.as_deref().map_or(true, |g| {
                let g = g.trim_matches('/');
                let comps: Vec<String> = e.path.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
                comps.iter().position(|c| c == "assets").map_or(false, |i| comps[i + 1..].join("/").starts_with(&format!("{}/", g)))
            })
        })
        .map(|e| e.path)
        .collect();
    let mut recomputed = Vec::new();
    if a.recompute {
        // sequential and one part at a time: this is the heavy path
        for p in &paths {
            match crate::rawaudit::recompute(p, a.tolerance) {
                Ok(r) => recomputed.push(json!({"path": p.slash(), "raw": r.iter().map(|x| x.raw).sum::<usize>(), "raw_source": "recomputed",
                    "frames": r.iter().filter(|x| x.raw > 0).map(|x| json!({"frame": x.frame, "raw": x.raw, "of": x.total, "method": x.method, "distance": x.distance, "tiles": x.tiles})).collect::<Vec<_>>()})),
                Err(e) => out.warn(format!("{}: recompute failed: {}", p.slash(), e.msg)),
            }
        }
    }
    let mut rows = fsutil::par_map(&paths, build_row);
    rows.retain(|r| {
        (!a.stale || r.stale) && (!a.blocked || r.state == "blocked") && a.state.as_deref().map_or(true, |s| r.state == s)
    });
    rows.sort_by(|x, y| x.path.cmp(&y.path));
    if a.rework || a.metrics {
        let ps: Vec<PathBuf> = rows.iter().map(|r| r.path.clone()).collect();
        if a.rework {
            crate::review::run_rework_list(&ps, out);
        }
        if a.metrics {
            crate::review::run_metrics(&ps, out);
        }
        return Ok(());
    }
    let mut counts = std::collections::BTreeMap::new();
    for r in &rows {
        *counts.entry(r.state.clone()).or_insert(0u32) += 1;
    }
    if !rows.is_empty() {
        out.line(format!(
            "{:<28} {:<11} {:<16} {:<8} {:<22} lint err/warn/info export raw",
            "PART", "STATE", "LEASE", "TODO/ALL", "WORST FRAME=SCORE"
        ));
    }
    for r in &rows {
        out.line(&r.line);
    }
    if !rows.is_empty() {
        out.line("TODO/ALL = frames not locked yet / frames in the part; WORST = frame with the highest score (higher is worse); raw = frames (tile sets: tiles) still equal to the last automatic output, i.e. not hand-finished (`pxl status --raw` lists them)");
    }
    if a.recompute {
        out.set("recomputed", Value::Array(recomputed));
    }
    if a.raw || a.recompute {
        for r in &rows {
            let n: usize = r.raw.iter().map(|x| x.count).sum();
            out.line(format!("{}: {} raw{}", r.path.slash(), n, if status::read(&r.path).raw_source.is_some() { " (baseline recomputed)" } else { "" }));
            for f in r.raw.iter().filter(|f| f.count > 0) {
                if f.tiles.is_empty() {
                    out.line(format!("  frame {} is still the automatic output", f.frame));
                } else {
                    let shown: Vec<String> = f.tiles.iter().take(60).map(|t| t.to_string()).collect();
                    out.line(format!("  frame {}: {} of {} tiles raw: {}{}", f.frame, f.count, f.total, shown.join(" "), if f.tiles.len() > 60 { " ..." } else { "" }));
                }
            }
        }
    }
    out.line(format!("{} parts: {}", rows.len(), counts.iter().map(|(k, v)| format!("{} {}", v, k)).collect::<Vec<_>>().join(", ")));
    out.set("parts", Value::Array(rows.iter().map(|r| r.json.clone()).collect()));
    out.set("counts", json!(counts));
    Ok(())
}

// ------------------------------------------------------------------ review

/// One part's review material. The compare worker (P3) extends `run_review` by calling `collect_review` and
/// rendering the `worst` frames' comparison sheets itself.
pub struct ReviewEntry {
    pub path: PathBuf,
    pub name: String,
    pub state: String,
    pub notes: Vec<status::Note>,
    pub lint: Option<status::LintCounts>,
    /// Worst frames first: (frame, metrics object).
    pub worst: Vec<(String, Value)>,
}

pub fn collect_review(a: &ReviewArgs) -> Result<Vec<ReviewEntry>> {
    let want = a.state.clone().unwrap_or_else(|| "done".into());
    let mut v = Vec::new();
    for e in discover(&a.parts)? {
        let s = status::read(&e.path);
        if s.state != want {
            continue;
        }
        let worst = s.ranked().into_iter().take(a.worst).map(|(f, _)| (f.clone(), s.metrics[&f].clone())).collect();
        v.push(ReviewEntry {
            name: e.path.file_stem().map_or(String::new(), |n| n.to_string_lossy().into_owned()),
            path: e.path,
            state: s.state,
            notes: s.notes,
            lint: s.lint,
            worst,
        });
    }
    v.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(v)
}

pub fn run_review(a: &ReviewArgs, out: &mut Out) -> Result<()> {
    crate::review::run_review(a, out)
}
