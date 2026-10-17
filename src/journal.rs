//! Edit journal and `undo` (DESIGN 5.2).
//!
//! `<part>.journal` (the part path with extension `journal`, e.g. `player.journal`) is append-only JSON
//! lines, kept permanently. Two entry types:
//!
//! * `edit`: `{type,id,ts,agent,cmd,frames:[{name,locked_before,locked_after,region:[x,y,w,h],before:[rows]}],
//!   after_hash,raw_before?}`. `region` is the bounding box of the changed pixels of that frame and `before`
//!   the rows of that box before the edit, so the exact previous state is restorable. `raw_before` holds the
//!   complete previous file text only when that text was not in canonical `.pxl` form (so even a
//!   hand-formatted file is restored byte for byte). `after_hash` is the content hash of the file the edit
//!   wrote (undo warns when the file changed since). `added` (optional) lists frame names the edit appended to the
//!   part (`import` of new frames); undo removes them again.
//! * `undo`: `{type,id,ts,agent,undone:[ids]}`.
//!
//! Semantics: the *active stack* is the `edit` entries not named in any `undo` entry, in file order.
//! `pxl undo --steps N` reverts the last N active edits, newest first (each frame's region and `locked` flag
//! return to their `before` values), saves once atomically, then appends one `undo` entry naming them. An
//! undo is never itself undone (there is no redo); a later `undo` continues down the stack, so N edits
//! followed by N undo steps restores the part byte for byte (AC-AD3). Entries are never rewritten.

use crate::fsutil::PathSlash;
use crate::cli::UndoArgs;
use crate::fsutil;
use crate::lease;
use crate::model::Part;
use crate::output::{Loc, Out, PxlError, Result};
use crate::pxl;
use crate::status::{self, StatusEvent};
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};

pub fn path_for(part_path: &Path) -> PathBuf {
    part_path.with_extension("journal")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameDelta {
    pub name: String,
    pub locked_before: bool,
    pub locked_after: bool,
    /// x, y, w, h of the changed bounding box.
    pub region: (i32, i32, usize, usize),
    pub before: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditEntry {
    pub id: String,
    pub ts: u64,
    pub agent: Option<String>,
    pub cmd: String,
    pub frames: Vec<FrameDelta>,
    pub after_hash: String,
    pub raw_before: Option<String>,
    /// Frames appended to the part by this edit (removed again by undo).
    pub added: Vec<String>,
    /// Frames of this edit that were rewritten wholesale by an automatic method (whole-frame `pick`): still raw.
    pub auto: Vec<String>,
    /// `finish` entries: why the frames count as hand-finished without a pixel change.
    pub reason: Option<String>,
    /// `finish` entries: the tile range marked (tile parts); empty = the whole frame.
    pub tiles: Vec<usize>,
}

fn now() -> (u64, u32) {
    let d = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    (d.as_secs(), d.subsec_nanos())
}

/// A unique entry id: time plus a hash of the content.
pub fn new_id(salt: &str) -> String {
    let (s, n) = now();
    let h = fsutil::content_hash(format!("{}{}{}", s, n, salt).as_bytes());
    format!("{:x}{:09}-{}", s, n, &h[..h.len().min(6)])
}

pub fn make_entry(agent: Option<&str>, cmd: &str, frames: Vec<FrameDelta>, after_text: &str, raw_before: Option<String>) -> EditEntry {
    let id = new_id(&format!("{}{}", cmd, after_text.len()));
    EditEntry {
        id,
        ts: now().0,
        agent: agent.map(|s| s.to_string()),
        cmd: cmd.to_string(),
        frames,
        after_hash: fsutil::content_hash(after_text.as_bytes()),
        raw_before,
        added: Vec::new(),
        auto: Vec::new(),
        reason: None,
        tiles: Vec::new(),
    }
}

fn entry_json(e: &EditEntry) -> Value {
    let frames: Vec<Value> = e
        .frames
        .iter()
        .map(|f| {
            json!({"name": f.name, "locked_before": f.locked_before, "locked_after": f.locked_after,
                   "region": [f.region.0, f.region.1, f.region.2, f.region.3], "before": f.before})
        })
        .collect();
    let mut v = json!({"type": "edit", "id": e.id, "ts": e.ts, "agent": e.agent, "cmd": e.cmd, "frames": frames, "after_hash": e.after_hash});
    if let Some(r) = &e.raw_before {
        v["raw_before"] = json!(r);
    }
    if !e.added.is_empty() {
        v["added"] = json!(e.added);
    }
    if !e.auto.is_empty() {
        v["auto"] = json!(e.auto);
    }
    if let Some(r) = &e.reason {
        v["reason"] = json!(r);
    }
    if !e.tiles.is_empty() {
        v["tiles"] = json!(e.tiles);
    }
    v
}

fn append_line(part_path: &Path, v: &Value) -> Result<()> {
    let p = path_for(part_path);
    let mut line = v.to_string();
    line.push('\n');
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&p).map_err(|e| PxlError::io(format!("cannot open journal {}: {}", p.slash(), e)))?;
    f.write_all(line.as_bytes()).map_err(|e| PxlError::io(format!("cannot write journal {}: {}", p.slash(), e)))
}

pub fn append_edit(part_path: &Path, e: &EditEntry) -> Result<()> {
    append_line(part_path, &entry_json(e))
}

fn jerr(path: &Path, line: usize, msg: &str) -> PxlError {
    PxlError::validation(format!("journal line {}: {}", line, msg)).with_loc(Loc::file(path.slash())).with_hint("the journal is append-only JSON lines; do not edit it by hand")
}

fn parse_edit(path: &Path, line: usize, v: &Value) -> Result<EditEntry> {
    let bad = |m: &str| jerr(path, line, m);
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(|x| x.to_string());
    let mut frames = Vec::new();
    for f in v.get("frames").and_then(|x| x.as_array()).ok_or_else(|| bad("edit without frames"))? {
        let reg = f.get("region").and_then(|x| x.as_array()).ok_or_else(|| bad("frame without region"))?;
        let n = |i: usize| reg.get(i).and_then(|x| x.as_i64()).ok_or_else(|| bad("bad region"));
        let before: Vec<String> = f.get("before").and_then(|x| x.as_array()).ok_or_else(|| bad("frame without before"))?.iter().map(|r| r.as_str().unwrap_or("").to_string()).collect();
        frames.push(FrameDelta {
            name: f.get("name").and_then(|x| x.as_str()).ok_or_else(|| bad("frame without name"))?.to_string(),
            locked_before: f.get("locked_before").and_then(|x| x.as_bool()).unwrap_or(false),
            locked_after: f.get("locked_after").and_then(|x| x.as_bool()).unwrap_or(false),
            region: (n(0)? as i32, n(1)? as i32, n(2)? as usize, n(3)? as usize),
            before,
        });
    }
    Ok(EditEntry {
        id: s("id").ok_or_else(|| bad("edit without id"))?,
        ts: v.get("ts").and_then(|x| x.as_u64()).unwrap_or(0),
        agent: s("agent"),
        cmd: s("cmd").unwrap_or_default(),
        frames,
        after_hash: s("after_hash").unwrap_or_default(),
        raw_before: s("raw_before"),
        added: v.get("added").and_then(|x| x.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default(),
        auto: v.get("auto").and_then(|x| x.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default(),
        reason: s("reason"),
        tiles: v.get("tiles").and_then(|x| x.as_array()).map(|a| a.iter().filter_map(|x| x.as_u64().map(|n| n as usize)).collect()).unwrap_or_default(),
    })
}

/// The active edit stack (oldest first): edits not undone by a later `undo` entry.
pub fn active_edits(part_path: &Path) -> Result<Vec<EditEntry>> {
    let p = path_for(part_path);
    if !p.is_file() {
        return Ok(Vec::new());
    }
    let text = fsutil::read_string(&p)?;
    let mut stack: Vec<EditEntry> = Vec::new();
    for (i, l) in text.lines().enumerate() {
        if l.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(l).map_err(|e| jerr(&p, i + 1, &e.to_string()))?;
        match v.get("type").and_then(|x| x.as_str()) {
            Some("edit") => stack.push(parse_edit(&p, i + 1, &v)?),
            Some("load") | Some("unfinish") => {}
            Some("undo") => {
                let ids: Vec<&str> = v.get("undone").and_then(|x| x.as_array()).map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
                stack.retain(|e| !ids.contains(&e.id.as_str()));
            }
            _ => return Err(jerr(&p, i + 1, "unknown entry type")),
        }
    }
    Ok(stack)
}

/// Mark frames as freshly (re)generated by `pxl load`/`reload`. Written only when the part already has a
/// journal (nothing to separate otherwise); earlier edits of these frames no longer count as hand work.
pub fn append_load(part_path: &Path, frames: &[String]) {
    if frames.is_empty() || !path_for(part_path).is_file() {
        return;
    }
    let _ = append_line(part_path, &json!({"type": "load", "ts": now().0, "frames": frames}));
}

/// Changed bounding boxes (x, y, w, h) per frame of every edit that still stands (not undone) since the
/// frame was last loaded, reloaded or rewritten by a whole-frame pick. Lock/unlock and the tile reducers
/// (`tiles fit`, `tiles dedupe`) are not hand work.
pub fn hand_edited(part_path: &Path) -> Result<std::collections::BTreeMap<String, Vec<(i32, i32, usize, usize)>>> {
    use std::collections::{BTreeMap, BTreeSet};
    let p = path_for(part_path);
    let mut map: BTreeMap<String, Vec<(i32, i32, usize, usize)>> = BTreeMap::new();
    if !p.is_file() {
        return Ok(map);
    }
    let text = fsutil::read_string(&p)?;
    let mut vals = Vec::new();
    let mut undone: BTreeSet<String> = BTreeSet::new();
    for (i, l) in text.lines().enumerate() {
        if l.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(l).map_err(|e| jerr(&p, i + 1, &e.to_string()))?;
        if v.get("type").and_then(|x| x.as_str()) == Some("undo") {
            for id in v.get("undone").and_then(|x| x.as_array()).into_iter().flatten().filter_map(|x| x.as_str()) {
                undone.insert(id.to_string());
            }
        }
        vals.push((i + 1, v));
    }
    let mut fin: BTreeMap<String, Vec<(i32, i32, usize, usize)>> = BTreeMap::new();
    for (line, v) in &vals {
        match v.get("type").and_then(|x| x.as_str()) {
            Some("load") => {
                for n in v.get("frames").and_then(|x| x.as_array()).into_iter().flatten().filter_map(|x| x.as_str()) {
                    map.remove(n);
                    fin.remove(n);
                }
            }
            Some("unfinish") => {
                for n in v.get("frames").and_then(|x| x.as_array()).into_iter().flatten().filter_map(|x| x.as_str()) {
                    if let (Some(rs), Some(m)) = (fin.remove(n), map.get_mut(n)) {
                        for r in rs {
                            if let Some(i) = m.iter().position(|x| *x == r) {
                                m.remove(i);
                            }
                        }
                        if m.is_empty() {
                            map.remove(n);
                        }
                    }
                }
            }
            Some("edit") => {
                let e = parse_edit(&p, *line, v)?;
                if undone.contains(&e.id) || matches!(e.cmd.as_str(), "lock" | "unlock" | "tiles fit" | "tiles dedupe") {
                    continue;
                }
                for d in &e.frames {
                    if e.auto.contains(&d.name) {
                        map.remove(&d.name);
                    } else if d.region.2 > 0 && d.region.3 > 0 {
                        map.entry(d.name.clone()).or_default().push(d.region);
                        if e.cmd == "finish" {
                            fin.entry(d.name.clone()).or_default().push(d.region);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok(map)
}

/// A standing `finish` mark: frames (or tiles) declared hand-finished without a pixel change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FinishMark {
    pub frame: String,
    /// Tiles marked; empty = the whole frame.
    pub tiles: Vec<usize>,
    pub reason: String,
    pub agent: Option<String>,
}

/// Standing `finish` marks: not undone, not cleared by a later `load` or `unfinish` of the frame.
pub fn finish_marks(part_path: &Path) -> Result<Vec<FinishMark>> {
    let p = path_for(part_path);
    if !p.is_file() {
        return Ok(Vec::new());
    }
    let text = fsutil::read_string(&p)?;
    let mut vals = Vec::new();
    let mut undone: std::collections::BTreeSet<String> = Default::default();
    for (i, l) in text.lines().enumerate() {
        if l.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(l).map_err(|e| jerr(&p, i + 1, &e.to_string()))?;
        if v.get("type").and_then(|x| x.as_str()) == Some("undo") {
            for id in v.get("undone").and_then(|x| x.as_array()).into_iter().flatten().filter_map(|x| x.as_str()) {
                undone.insert(id.to_string());
            }
        }
        vals.push((i + 1, v));
    }
    let mut marks: Vec<FinishMark> = Vec::new();
    for (line, v) in &vals {
        match v.get("type").and_then(|x| x.as_str()) {
            Some("load") | Some("unfinish") => {
                for n in v.get("frames").and_then(|x| x.as_array()).into_iter().flatten().filter_map(|x| x.as_str()) {
                    marks.retain(|m| m.frame != n);
                }
            }
            Some("edit") if v.get("cmd").and_then(|x| x.as_str()) == Some("finish") => {
                let e = parse_edit(&p, *line, v)?;
                if undone.contains(&e.id) {
                    continue;
                }
                let mut seen: Vec<&str> = Vec::new();
                for d in &e.frames {
                    if !seen.contains(&d.name.as_str()) {
                        seen.push(&d.name);
                        marks.push(FinishMark { frame: d.name.clone(), tiles: e.tiles.clone(), reason: e.reason.clone().unwrap_or_default(), agent: e.agent.clone() });
                    }
                }
            }
            _ => {}
        }
    }
    Ok(marks)
}

/// Record that `pxl rework` reopened these frames: their `finish` marks no longer count.
pub fn append_unfinish(part_path: &Path, frames: &[String]) {
    if !frames.is_empty() {
        let _ = append_line(part_path, &json!({"type": "unfinish", "ts": now().0, "frames": frames}));
    }
}

/// Revert one entry's deltas inside `part` (in memory).
fn revert(part: &mut Part, e: &EditEntry, path: &Path) -> Result<()> {
    for d in &e.frames {
        let f = part.frame_mut(&d.name).ok_or_else(|| {
            PxlError::validation(format!("cannot undo edit {}: frame '{}' no longer exists", e.id, d.name)).with_loc(Loc::file(path.slash()).frame(d.name.clone()))
        })?;
        let (x0, y0, w, h) = d.region;
        for (i, row) in d.before.iter().enumerate().take(h) {
            for (j, &c) in row.as_bytes().iter().enumerate().take(w) {
                f.grid.set(x0 + j as i32, y0 + i as i32, c);
            }
        }
        f.locked = d.locked_before;
    }
    part.frames.retain(|f| !e.added.contains(&f.name));
    Ok(())
}

pub fn run_undo(a: &UndoArgs, out: &mut Out) -> Result<()> {
    if a.steps == 0 {
        return Err(PxlError::usage("--steps must be at least 1"));
    }
    let path = fsutil::resolve_part(&a.part)?;
    lease::guard(&path, out.agent())?;
    crate::status::refuse_if_approved(&path, a.force_approved)?;
    let stack = active_edits(&path)?;
    if stack.is_empty() {
        return Err(PxlError::validation(format!("nothing to undo for {}", path.slash())).with_loc(Loc::file(path_for(&path).slash())).with_hint("the journal has no active edits"));
    }
    if a.steps > stack.len() {
        return Err(PxlError::validation(format!("cannot undo {} steps: only {} edits in the journal", a.steps, stack.len())).with_hint(format!("use --steps {} or fewer", stack.len())));
    }
    let raw = fsutil::read_string(&path)?;
    let orig = pxl::parse_str(&raw, &path)?;
    let mut part = orig.clone();
    let undone: Vec<&EditEntry> = stack.iter().rev().take(a.steps).collect();
    if let Some(top) = undone.first() {
        if !top.after_hash.is_empty() && top.after_hash != fsutil::content_hash(raw.as_bytes()) {
            out.warn("the part changed outside the journal since its last edit; undo restores only the journaled regions");
        }
    }
    for e in &undone {
        revert(&mut part, e, &path)?;
    }
    let oldest = undone.last().unwrap();
    let text = match &oldest.raw_before {
        Some(r) => r.clone(),
        None => pxl::to_string(&part),
    };
    fsutil::atomic_write(&path, text.as_bytes())?;
    let ids: Vec<&str> = undone.iter().map(|e| e.id.as_str()).collect();
    append_line(&path, &json!({"type": "undo", "id": new_id("undo"), "ts": now().0, "agent": out.agent(), "undone": ids}))?;
    if let Ok(p) = pxl::parse_str(&text, &path) {
        status::record(&path, &p, StatusEvent::Edited);
    }
    for e in undone.iter().filter(|e| e.cmd == "finish") {
        let mut names: Vec<String> = e.frames.iter().map(|d| d.name.clone()).collect();
        names.dedup();
        crate::edit::reopen_finished(&path, &names, &e.tiles);
    }
    // report: what changed relative to the file before the undo
    let after = pxl::parse_str(&text, &path).unwrap_or(part);
    let mut frames_json = Vec::new();
    let mut total = 0usize;
    let mut text_out = format!("undid {} edit{}", undone.len(), if undone.len() == 1 { "" } else { "s" });
    for (fo, fa) in orig.frames.iter().zip(after.frames.iter()) {
        if let Some((n, bb)) = crate::edit::diff_grids(&fo.grid, &fa.grid) {
            total += n;
            let (reg, rows) = crate::edit::region_rows(&fa.grid, bb);
            text_out.push_str(&format!("\n{}/{}: {} px restored, region {},{},{},{}", after.name, fa.name, n, reg.0, reg.1, reg.2, reg.3));
            for l in crate::show::render_grid(&fa.grid, reg) {
                text_out.push('\n');
                text_out.push_str(&l);
            }
            frames_json.push(json!({"frame": fa.name, "changed": n, "region": [reg.0, reg.1, reg.2, reg.3], "rows": rows}));
        }
    }
    text_out.push_str(&format!("\n{} px changed", total));
    out.emit(text_out, json!({"undone": ids, "changed": total, "frames": frames_json}));
    Ok(())
}
