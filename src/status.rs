//! `<part>.status.json` (schema 1): the handback file of a part (DESIGN 6.3). Owner: queue worker.
//!
//! ```json
//! { "schema": 1, "state": "todo|in_progress|done|approved|rework|blocked", "owner": "A"|null,
//!   "notes":  [{"by": "A", "text": "..."}],            // agent/coordinator notes (release --note, ...)
//!   "review": [{"by": "coordinator", "verdict": "approved|rework", "text": "..."}],
//!   "metrics": { "<frame>": <opaque object; "score" (higher = worse) ranks frames> },
//!   "lint": {"errors":0,"warnings":0,"infos":0} | absent,
//!   "locked": ["frame", ...],
//!   "hashes": { "part": h, "template": h|null, "inputs": h,
//!               "loaded_template": h, "loaded_inputs": h,          // recorded at last `Loaded`
//!               "exported_part": h, "exported_template": h, "exported_inputs": h,  // at last `Exported`
//!               "exported_profile": h,                                   // export config (profile + output paths)
//!               "outputs": { "<path>": h } } }
//! ```
//! Staleness: the template or inputs hash now differs from the one recorded at the last load -> `stale`.
//! Export freshness: `none` (never exported), `fresh`, or `stale` (part/template/inputs changed since export).

use crate::fsutil::PathSlash;
use crate::fsutil;
use crate::model::Part;
use crate::output::{PxlError, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const STATES: [&str; 6] = ["todo", "in_progress", "done", "approved", "rework", "blocked"];

/// Events other modules report. Unit variants are the originals; the payload variants let compare, lint and
/// export hand their results in without owning the file format.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StatusEvent {
    Loaded,
    Edited,
    Compared,
    Linted,
    Exported,
    Released,
    /// Per-frame metrics (frame -> opaque object, `score` ranks worst-first). Replaces those frames' entries.
    Metrics(BTreeMap<String, Value>),
    /// Lint totals for the whole part.
    LintSummary { errors: u32, warnings: u32, infos: u32 },
    /// Output file -> content hash, recorded by export (replaces the previous map).
    Outputs(BTreeMap<String, String>),
    /// Hash of the export configuration (profile channel step and grid, output patterns and paths), recorded by
    /// export next to `Exported`.
    ExportConfig(String),
    /// These frames (indexes) were just written by an automatic method (load, reload, pick): remember their pixels.
    RawFrames(Vec<usize>),
    /// These tiles of one frame were just rewritten automatically (tiles fit, tiles dedupe).
    RawTiles { frame: usize, tiles: Vec<usize> },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    #[serde(default)]
    pub by: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub verdict: String,
    #[serde(default)]
    pub text: String,
    /// Review notes only: the frames the note is about (empty = the whole part).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub frames: Vec<String>,
    /// Frame -> content hash of its grid when the note was written. A frame whose grid still hashes the same has not
    /// been edited since, so the note is still open for it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub at: BTreeMap<String, String>,
}

/// Content hash of a frame's grid (used to close per-frame review notes when the frame is edited).
pub fn frame_hash(part: &Part, name: &str) -> Option<String> {
    part.frame(name).map(|f| fsutil::content_hash(&f.grid.cells))
}

impl Note {
    /// A rework note over `frames` with the frames' current hashes recorded.
    pub fn rework(by: &str, text: &str, frames: &[String], part: &Part) -> Note {
        let at = frames.iter().filter_map(|f| frame_hash(part, f).map(|h| (f.clone(), h))).collect();
        Note { by: by.to_string(), verdict: "rework".into(), text: text.to_string(), frames: frames.to_vec(), at }
    }
    /// Frames of this note that have not been edited since it was written.
    pub fn open_frames(&self, part: &Part) -> Vec<String> {
        self.frames.iter().filter(|f| match (self.at.get(*f), frame_hash(part, f)) {
            (Some(h), Some(c)) => *h == c,
            (_, None) => false, // the frame is gone
            (None, _) => true,
        }).cloned().collect()
    }
}

impl StatusFile {
    /// Rework notes (index into `review`) that are still open: per-frame notes while any of their frames is
    /// unedited; whole-part notes while the part is not back in `done`/`approved` and no later verdict exists.
    pub fn open_rework(&self, part: &Part) -> Vec<(usize, Vec<String>)> {
        let mut v = Vec::new();
        for (i, n) in self.review.iter().enumerate() {
            if n.verdict != "rework" {
                continue;
            }
            if n.frames.is_empty() {
                let later = self.review[i + 1..].iter().any(|m| m.verdict == "approved");
                if !later && !matches!(self.state.as_str(), "done" | "approved") {
                    v.push((i, Vec::new()));
                }
            } else {
                let of = n.open_frames(part);
                if !of.is_empty() && !self.review[i + 1..].iter().any(|m| m.verdict == "approved") {
                    v.push((i, of));
                }
            }
        }
        v
    }
    /// Number of rework verdicts so far.
    pub fn rework_rounds(&self) -> usize {
        self.review.iter().filter(|n| n.verdict == "rework").count()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LintCounts {
    pub errors: u32,
    pub warnings: u32,
    pub infos: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hashes {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inputs: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loaded_template: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loaded_inputs: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exported_part: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exported_template: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exported_inputs: Option<String>,
    /// Export configuration hash at the last export (profile + output patterns/paths).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exported_profile: Option<String>,
    #[serde(default)]
    pub outputs: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusFile {
    pub schema: u32,
    pub state: String,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub notes: Vec<Note>,
    #[serde(default)]
    pub review: Vec<Note>,
    #[serde(default)]
    pub metrics: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lint: Option<LintCounts>,
    #[serde(default)]
    pub locked: Vec<String>,
    #[serde(default)]
    pub hashes: Hashes,
    /// Content hash of the last automatic method output per frame (tile parts: 8 hex digits per tile, in sheet
    /// order). A frame or tile whose current pixels still hash the same has not been hand-finished.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub raw: BTreeMap<String, String>,
    /// Where `raw` came from when it was not recorded live: `recomputed` (`status --raw --recompute`) or `mixed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_source: Option<String>,
    /// Units known to be hand-finished from the edit journal (`status --raw --recompute`): per frame the tile
    /// indexes, or `[0]` for a sprite frame. Never raw, whatever their similarity to a method output.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub edited: BTreeMap<String, Vec<usize>>,
    /// `user` when the user personally approved the asset: changes then need `--force-user-approved` and create a review task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_by: Option<String>,
}

impl Default for StatusFile {
    fn default() -> Self {
        StatusFile {
            schema: 1,
            state: "todo".into(),
            owner: None,
            notes: Vec::new(),
            review: Vec::new(),
            metrics: BTreeMap::new(),
            lint: None,
            locked: Vec::new(),
            hashes: Hashes::default(),
            raw: BTreeMap::new(),
            raw_source: None,
            edited: BTreeMap::new(),
            approved_by: None,
        }
    }
}

impl StatusFile {
    /// Worst frame by `metrics[frame].score` (higher = worse), ties by frame name. Frames without a numeric
    /// `score` are ignored.
    pub fn worst(&self) -> Option<(String, f64)> {
        self.ranked().into_iter().next()
    }
    /// All scored frames, worst first.
    pub fn ranked(&self) -> Vec<(String, f64)> {
        let mut v: Vec<(String, f64)> =
            self.metrics.iter().filter_map(|(k, m)| m.get("score").and_then(|s| s.as_f64()).map(|s| (k.clone(), s))).collect();
        v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(&b.0)));
        v
    }
}

pub fn status_path(part: &Path) -> PathBuf {
    part.with_extension("status.json")
}

/// Read the status file; a missing or unreadable file reads as the default (`todo`).
pub fn read(part: &Path) -> StatusFile {
    std::fs::read(status_path(part)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn write(part: &Path, s: &StatusFile) -> Result<()> {
    let mut text = serde_json::to_string_pretty(s).map_err(|e| PxlError::io(e.to_string()))?;
    text.push('\n');
    fsutil::atomic_write(&status_path(part), text.as_bytes())
}

/// Read-modify-write (atomic replace).
pub fn update(part: &Path, f: impl FnOnce(&mut StatusFile)) -> Result<()> {
    let _pl = fsutil::lock_part(part)?;
    let mut s = read(part);
    f(&mut s);
    s.schema = 1;
    write(part, &s)
}

fn locate_relative(base: &Path, rel: &str) -> Option<PathBuf> {
    fsutil::find_up(base, rel)
}

fn hash_template_file(path: &Path, depth: usize, acc: &mut String) {
    let Ok(bytes) = std::fs::read(path) else { return };
    acc.push_str(&fsutil::content_hash(&bytes));
    acc.push('\n');
    if depth > 8 {
        return;
    }
    if let Ok(text) = String::from_utf8(bytes) {
        if let Ok(v) = text.parse::<toml::Table>() {
            if let Some(rel) = v.get("extends").and_then(|x| x.as_str()) {
                let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."));
                if let Some(p) = locate_relative(&dir, rel) {
                    hash_template_file(&p, depth + 1, acc);
                }
            }
        }
    }
}

/// Content hash of the part's template including its `extends` chain; `None` when the part has no template or
/// the file is missing.
pub fn template_hash(part: &Part) -> Option<String> {
    let rel = part.template.as_deref()?;
    let p = locate_relative(&part.dir(), rel)?;
    let mut acc = String::new();
    hash_template_file(&p, 0, &mut acc);
    Some(fsutil::content_hash(acc.as_bytes()))
}

/// Content hash of the input spec and every input file it names (missing files hash as "missing").
pub fn inputs_hash(part: &Part) -> String {
    inputs_hash_with(part, true)
}

/// The pre-0.x hash, built from `Debug` output (changes whenever a tool build adds a field). Only used to
/// recognise hashes recorded by older builds so that upgrading the tool does not make every part STALE.
fn inputs_hash_legacy(part: &Part) -> String {
    inputs_hash_with(part, false)
}

fn inputs_hash_with(part: &Part, canonical: bool) -> String {
    let dir = part.dir();
    let mut files: Vec<String> = Vec::new();
    let i = &part.input;
    if let Some(pat) = &i.images {
        if pat.contains("{frame}") {
            for f in &part.frames {
                files.push(pat.replace("{frame}", f.src.as_deref().unwrap_or(&f.name)));
            }
        } else {
            files.push(pat.clone());
        }
    }
    files.sort();
    files.dedup();
    // `at`/`skip` and per-frame overrides only count when used, so existing parts keep their hash
    let mut acc = if canonical {
        // the serialized `.pxl` text of just the input/reduce/per-frame-input settings: stable across tool builds,
        // independent of where the part file lives and of everything else in it
        let mut c = Part::new("", (0, 0));
        c.input = part.input.clone();
        c.reduce = part.reduce.clone();
        c.frame_input = part.frame_input.clone();
        let mut t = format!("v2\n{}\n", crate::pxl::to_string(&c));
        // a frame's own reduction phase (only when used, so existing hashes stay)
        for (n, o) in &part.frame_input {
            if let Some(p) = o.phase {
                t.push_str(&format!("phase {}={},{}\n", n, p.0, p.1));
            }
        }
        t
    } else {
        let mut a = format!("{:?}|{:?}\n", part.input, part.reduce);
        if !part.frame_input.is_empty() {
            a.push_str(&format!("{:?}\n", part.frame_input).replace(", phase: None }", " }"));
        }
        a
    };
    for f in files {
        let h = locate_relative(&dir, &f).and_then(|p| std::fs::read(p).ok()).map_or("missing".to_string(), |b| fsutil::content_hash(&b));
        acc.push_str(&format!("{}:{}\n", f, h));
    }
    fsutil::content_hash(acc.as_bytes())
}

fn inputs_match(recorded: Option<&str>, part: &Part) -> bool {
    match recorded {
        None => false,
        Some(r) => r == inputs_hash(part) || r == inputs_hash_legacy(part),
    }
}

/// Input hashes of a part as they are now (take it before the part or its inputs move).
pub fn inputs_snapshot(part: &Part) -> (String, String) {
    (inputs_hash(part), inputs_hash_legacy(part))
}

/// After a part moved (its relative input paths changed): recorded input hashes that matched the old part now match the new one,
/// so the move does not make the part STALE or its export outdated.
pub fn rebase_inputs(s: &mut StatusFile, old: &(String, String), new: &Part) {
    let new_h = inputs_hash(new);
    for h in [&mut s.hashes.inputs, &mut s.hashes.loaded_inputs, &mut s.hashes.exported_inputs] {
        if h.as_deref() == Some(old.0.as_str()) || h.as_deref() == Some(old.1.as_str()) {
            *h = Some(new_h.clone());
        }
    }
}

/// Template or inputs changed since the last load. A part that was never loaded is not stale.
pub fn is_stale(s: &StatusFile, part: &Part) -> bool {
    if s.hashes.loaded_inputs.is_none() && s.hashes.loaded_template.is_none() {
        return false;
    }
    s.hashes.loaded_template != template_hash(part) || !inputs_match(s.hashes.loaded_inputs.as_deref(), part)
}

/// "none" | "fresh" | "stale".
pub fn export_freshness(s: &StatusFile, part: &Part, part_hash: &str) -> &'static str {
    let h = &s.hashes;
    let Some(ep) = &h.exported_part else { return "none" };
    if ep == part_hash && h.exported_template == template_hash(part) && inputs_match(h.exported_inputs.as_deref(), part) {
        "fresh"
    } else {
        "stale"
    }
}

pub fn part_file_hash(part: &Part) -> String {
    match std::fs::read(&part.path) {
        Ok(b) => fsutil::content_hash(&b),
        Err(_) => fsutil::content_hash(crate::pxl::to_string(part).as_bytes()),
    }
}

static USER_OK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `--force-user-approved`: allow changes to assets the user approved personally.
pub fn allow_user_approved() {
    USER_OK.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// Edits to an approved part must be reopened first: `pxl rework PART --note ...` (or `--force-approved`).
pub fn refuse_if_approved(part_path: &Path, force: bool) -> Result<()> {
    if read(part_path).approved_by.as_deref() == Some("user") && !USER_OK.load(std::sync::atomic::Ordering::SeqCst) {
        return Err(PxlError::validation(format!("{} was approved by the user personally", part_path.slash()))
            .with_hint("changing it needs --force-user-approved (a review task is created for the user)"));
    }
    if !force && read(part_path).state == "approved" {
        return Err(PxlError::validation(format!("part is approved; reopen with `pxl rework {} --note ...` first", part_path.slash()))
            .with_hint("`--force-approved` overrides this check"));
    }
    Ok(())
}

/// Update `<part>.status.json` for an event. Failures never fail the calling command.
pub fn record(part_path: &Path, part: &Part, event: StatusEvent) {
    let _ = record_inner(part_path, part, event);
}

fn record_inner(part_path: &Path, part: &Part, event: StatusEvent) -> Result<()> {
    let _pl = fsutil::lock_part(part_path)?;
    let mut s = read(part_path);
    s.schema = 1;
    s.locked = part.frames.iter().filter(|f| f.locked).map(|f| f.name.clone()).collect();
    let ph = part_file_hash(part);
    s.hashes.part = Some(ph.clone());
    let heavy = matches!(event, StatusEvent::Loaded | StatusEvent::Exported);
    if heavy {
        let (t, i) = (template_hash(part), inputs_hash(part));
        s.hashes.template = t.clone();
        s.hashes.inputs = Some(i.clone());
        if matches!(event, StatusEvent::Loaded) {
            s.hashes.loaded_template = t;
            s.hashes.loaded_inputs = Some(i);
        } else {
            s.hashes.exported_part = Some(ph);
            s.hashes.exported_template = t;
            s.hashes.exported_inputs = Some(i);
        }
    }
    match event {
        StatusEvent::Edited if s.state == "done" || s.state == "approved" => s.state = "in_progress".into(),
        StatusEvent::Metrics(m) => s.metrics.extend(m),
        StatusEvent::LintSummary { errors, warnings, infos } => s.lint = Some(LintCounts { errors, warnings, infos }),
        StatusEvent::Outputs(o) => s.hashes.outputs = o,
        StatusEvent::ExportConfig(c) => s.hashes.exported_profile = Some(c),
        StatusEvent::RawFrames(fs) => {
            for i in fs {
                if let Some(f) = part.frames.get(i) {
                    s.edited.remove(&f.name);
                    s.raw.insert(f.name.clone(), raw_hash_all(part, i));
                }
            }
        }
        StatusEvent::RawTiles { frame, tiles } => {
            if let Some(f) = part.frames.get(frame) {
                let n = tile_count(part, frame).unwrap_or(0);
                let mut cur = s.raw.get(&f.name).cloned().filter(|c| c.len() == n * 8).unwrap_or_else(|| "0".repeat(n * 8));
                for t in tiles.into_iter().filter(|&t| t < n) {
                    if let Some(e) = s.edited.get_mut(&f.name) {
                        e.retain(|&x| x != t);
                    }
                    cur.replace_range(t * 8..t * 8 + 8, &tile_hash(&f.grid, t));
                }
                s.raw.insert(f.name.clone(), cur);
            }
        }
        _ => {}
    }
    write(part_path, &s)
}

// ------------------------------------------------------------------ raw-output audit

/// Number of 8x8 tiles of a tile-mode frame (tile set or screen whose grid is a multiple of 8), else `None`.
pub fn tile_count(part: &Part, fi: usize) -> Option<usize> {
    let g = &part.frames.get(fi)?.grid;
    (part.kind != crate::model::PartKind::Sprite && g.w >= 8 && g.h >= 8 && g.w % 8 == 0 && g.h % 8 == 0).then(|| (g.w / 8) * (g.h / 8))
}

/// 8 hex digits over the 64 cells of tile `t` (row-major tile order).
pub fn tile_hash(g: &crate::model::Grid, t: usize) -> String {
    let per = g.w / 8;
    let (tx, ty) = ((t % per) * 8, (t / per) * 8);
    let mut b = [0u8; 64];
    for y in 0..8 {
        for x in 0..8 {
            b[y * 8 + x] = g.cells[(ty + y) * g.w + tx + x];
        }
    }
    fsutil::content_hash(&b)[..8].to_string()
}

fn tile_flat(g: &crate::model::Grid, t: usize) -> bool {
    let per = g.w / 8;
    let (tx, ty) = ((t % per) * 8, (t / per) * 8);
    let first = g.cells[ty * g.w + tx];
    (0..8).all(|y| (0..8).all(|x| g.cells[(ty + y) * g.w + tx + x] == first))
}

/// Current baseline-format hash of frame `fi`.
pub fn raw_hash_of(part: &Part, fi: usize) -> String {
    raw_hash_all(part, fi)
}

fn raw_hash_all(part: &Part, fi: usize) -> String {
    let g = &part.frames[fi].grid;
    match tile_count(part, fi) {
        Some(n) => (0..n).map(|t| tile_hash(g, t)).collect(),
        None => fsutil::content_hash(&g.cells),
    }
}

/// Frames (or tiles) that still equal the last automatic output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawFrame {
    pub frame: String,
    /// Raw tile indexes (tile parts); empty for a sprite frame.
    pub tiles: Vec<usize>,
    /// Number of raw units: tiles, or 1 for a raw sprite frame.
    pub count: usize,
    /// Units that could be raw at all (tiles with more than one key; 1 for a sprite frame with pixels).
    pub total: usize,
}

/// Raw-output audit of a part. Flat tiles (one key) and empty frames cannot be hand-finished and never count.
pub fn raw_audit(s: &StatusFile, part: &Part) -> Vec<RawFrame> {
    let mut v = Vec::new();
    for (fi, f) in part.frames.iter().enumerate() {
        let rec = s.raw.get(&f.name);
        let ed: &[usize] = s.edited.get(&f.name).map(|v| v.as_slice()).unwrap_or(&[]);
        match tile_count(part, fi) {
            Some(n) => {
                let tiles: Vec<usize> = (0..n)
                    .filter(|&t| !tile_flat(&f.grid, t) && !ed.contains(&t) && rec.map_or(false, |r| r.len() == n * 8 && r[t * 8..t * 8 + 8] == tile_hash(&f.grid, t)))
                    .collect();
                let total = (0..n).filter(|&t| !tile_flat(&f.grid, t)).count();
                v.push(RawFrame { frame: f.name.clone(), count: tiles.len(), tiles, total });
            }
            None => {
                let has = f.grid.cells.iter().any(|&c| c != b'.');
                let raw = has && ed.is_empty() && rec.map_or(false, |r| *r == fsutil::content_hash(&f.grid.cells));
                v.push(RawFrame { frame: f.name.clone(), tiles: Vec::new(), count: raw as usize, total: has as usize });
            }
        }
    }
    v
}

/// Total raw units of a part.
pub fn raw_count(s: &StatusFile, part: &Part) -> usize {
    raw_audit(s, part).iter().map(|r| r.count).sum()
}
