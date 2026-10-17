//! Milestones and gates (M6 R7): `plan accepted`, `style approved` (style.md plus the style frames approved), `dry run passed`
//! (`pxl deliver --dry-run` on the style frames). In a project with `[project] gates = true` (which `pxl new` sets) `next` and `claim` hand out
//! production work only after all three; until then they hand out the style frames only. Projects without the key behave as before.

use crate::cli::*;
use crate::manifest::Manifest;
use crate::output::{Out, PxlError, Result};
use crate::{asset, fsutil, status, style, versions};
use serde_json::{json, Value};
use std::path::Path;

pub fn gates_on(home: &Path) -> bool {
    crate::project::project_table(&home.join("pxl.toml")).get("gates").and_then(|v| v.as_bool()).unwrap_or(false)
}

#[derive(Clone, Debug, Default)]
pub struct Item {
    pub ok: bool,
    pub text: String,
}

#[derive(Clone, Debug, Default)]
pub struct Status {
    pub gates: bool,
    pub plan: Item,
    pub style_doc: Item,
    pub style_frames: Item,
    pub style: Item,
    pub dry_run: Item,
    pub frames: Vec<String>,
}

impl Status {
    pub fn all_ok(&self) -> bool {
        self.plan.ok && self.style.ok && self.dry_run.ok
    }

    pub fn text(&self) -> String {
        let mark = |i: &Item| if i.ok { "[x]" } else { "[ ]" };
        let mut s = String::new();
        if !self.gates {
            s.push_str("gates are off in this project ([project] gates = true turns them on; `pxl new` sets it)\n");
        }
        s.push_str(&format!("{} plan accepted: {}\n", mark(&self.plan), self.plan.text));
        s.push_str(&format!("{} style approved: {}\n", mark(&self.style), self.style.text));
        s.push_str(&format!("      style.md: {}\n      style frames: {}\n", self.style_doc.text, self.style_frames.text));
        s.push_str(&format!("{} dry run passed: {}\n", mark(&self.dry_run), self.dry_run.text));
        s.push_str(&format!(
            "production: {}",
            if self.all_ok() { "open".to_string() } else if self.gates { format!("closed; `next` hands out the style frames only ({})", if self.frames.is_empty() { "none marked yet: pxl style mark ASSET".to_string() } else { self.frames.join(", ") }) } else { "not gated".to_string() }
        ));
        s
    }

    pub fn to_json(&self) -> Value {
        json!({
            "gates": self.gates, "production_open": self.all_ok() || !self.gates,
            "plan_accepted": {"ok": self.plan.ok, "text": self.plan.text},
            "style_approved": {"ok": self.style.ok, "text": self.style.text, "style_md": {"ok": self.style_doc.ok, "text": self.style_doc.text}, "style_frames": {"ok": self.style_frames.ok, "text": self.style_frames.text}},
            "dry_run_passed": {"ok": self.dry_run.ok, "text": self.dry_run.text},
            "style_frames": self.frames,
        })
    }
}

/// Latest version numbers of the style frames: a changed signature means the dry run is stale.
pub fn frames_signature(home: &Path) -> String {
    let mut s = String::new();
    for (name, _) in style::style_frames(home) {
        if let Some(a) = asset::scan(home).into_iter().find(|a| a.name == name) {
            s.push_str(&format!("{}:{};", name, versions::latest(&a.file).map_or(0, |v| v.n)));
        }
    }
    fsutil::content_hash(s.as_bytes())
}

pub fn status(home: &Path) -> Status {
    let mut st = Status { gates: gates_on(home), ..Default::default() };
    // plan
    st.plan = match Manifest::load(&home.join("pxl.toml")) {
        Ok(m) => {
            let (ok, text) = crate::plan::plan_accepted(&m);
            Item { ok, text }
        }
        Err(e) => Item { ok: false, text: e.msg },
    };
    // style.md
    let d = style::doc_status(home);
    st.style_doc = if !d.exists {
        Item { ok: false, text: "none (pxl style propose)".into() }
    } else if d.approved {
        Item { ok: true, text: format!("approved by {} on {}", d.by, d.on) }
    } else if d.changed {
        Item { ok: false, text: "changed since it was approved (pxl style approve)".into() }
    } else {
        Item { ok: false, text: "proposed, not approved (pxl style approve)".into() }
    };
    // style frames approved
    let frames = style::style_frames(home);
    st.frames = frames.iter().map(|f| f.0.clone()).collect();
    let all = asset::scan(home);
    let mut parts = Vec::new();
    let mut ok = !frames.is_empty();
    for (name, _) in &frames {
        let state = all.iter().find(|a| a.name == *name).map(|a| status::read(&a.file).state).unwrap_or_else(|| "missing".into());
        if state != "approved" {
            ok = false;
        }
        parts.push(format!("{} ({})", name, state));
    }
    st.style_frames = Item { ok, text: if frames.is_empty() { "none marked (pxl style mark ASSET)".into() } else { parts.join(", ") } };
    st.style = Item { ok: st.style_doc.ok && st.style_frames.ok, text: if st.style_doc.ok && st.style_frames.ok { "style.md and the style frames are approved".into() } else { "style.md and every style frame must be approved".into() } };
    // dry run
    let state = style::read_state(home);
    let dr = state.get("dry_run").and_then(|v| v.as_table()).cloned();
    st.dry_run = match dr {
        None => Item { ok: false, text: "not run (pxl deliver --dry-run)".into() },
        Some(t) => {
            let sig = t.get("signature").and_then(|v| v.as_str()).unwrap_or("");
            let on = t.get("on").and_then(|v| v.as_str()).unwrap_or("");
            if sig == frames_signature(home) {
                Item { ok: true, text: format!("passed on {}", on) }
            } else {
                Item { ok: false, text: format!("passed on {} but the style frames changed since (pxl deliver --dry-run)", on) }
            }
        }
    };
    st
}

/// `next` and `claim`: may this asset be handed out? Outside gated projects always.
pub fn allows(home: &Path, asset_name: &str) -> Result<()> {
    if !gates_on(home) {
        return Ok(());
    }
    let st = status(home);
    if st.all_ok() || style::is_style_frame(home, asset_name) {
        return Ok(());
    }
    Err(PxlError::validation(format!(
        "production has not started: {} is not a style frame and the gates are not all passed",
        asset_name
    ))
    .with_hint(format!("{} (pxl milestone status)", st.text().lines().map(|l| l.trim()).collect::<Vec<_>>().join("; "))))
}

/// The gate of a project, computed once: `None` outside gated projects.
pub struct Gate {
    pub open: bool,
    pub frames: Vec<String>,
    pub status: Status,
}

impl Gate {
    pub fn allows(&self, asset_name: &str) -> bool {
        self.open || self.frames.iter().any(|f| f == asset_name)
    }
}

pub fn gate_for(home: &Path) -> Option<Gate> {
    if !gates_on(home) {
        return None;
    }
    let st = status(home);
    Some(Gate { open: st.all_ok(), frames: st.frames.clone(), status: st })
}

pub fn run_milestone(c: &MilestoneCmd, out: &mut Out) -> Result<()> {
    match c {
        MilestoneCmd::Status => {
            let cwd = std::env::current_dir().map_err(|e| crate::output::PxlError::io(e.to_string()))?;
            let m = Manifest::find_file(&cwd).ok_or_else(|| PxlError::usage("no project here (pxl.toml not found)"))?;
            let home = asset::dir_of(&m);
            let st = status(&home);
            out.emit(st.text(), st.to_json());
            Ok(())
        }
    }
}
