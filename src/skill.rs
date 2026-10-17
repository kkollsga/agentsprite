//! `pxl skill list | show | install | uninstall | path`: the skills that ship inside the binary. The repo's `skills/`
//! folder is embedded at build time (`build.rs`), so a skill and its `references/*` files travel with the tool and the
//! installed text is exactly the text `pxl skill show` prints, plus one version stamp line after the front matter.
//!
//! Install layout: `<root>/.claude/skills/<name>/` and `<root>/.codex/skills/<name>/`, where root is the home folder
//! or, with `--project`, the current directory. Every installed skill folder carries a `.agentsprite-managed` marker
//! (the pxl version that wrote it); a folder without the marker is never replaced or removed.

use crate::fsutil::PathSlash;
use crate::cli::{SkillHost, SkillInstallArgs, SkillPathArgs, SkillShowArgs};
use crate::output::{Out, PxlError, Result};
use serde_json::json;
use std::path::{Path, PathBuf};

/// Marker file in every folder this command installed; its content is the pxl version that wrote it.
pub const MARKER: &str = ".agentsprite-managed";
/// The version of this binary (what `pxl --version` prints).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

mod gen {
    include!(concat!(env!("OUT_DIR"), "/skills_gen.rs"));
}

/// Every embedded file as `(relative path, text)`, sorted: `<skill>/SKILL.md`, `<skill>/references/<file>`, `README.md`.
pub fn files() -> &'static [(&'static str, &'static str)] {
    gen::FILES
}

/// Skill names: the folders that hold a `SKILL.md`, sorted.
pub fn names() -> Vec<&'static str> {
    let mut v: Vec<&str> = gen::FILES.iter().filter_map(|(p, _)| p.strip_suffix("/SKILL.md")).filter(|n| !n.contains('/')).collect();
    v.sort();
    v.dedup();
    v
}

/// Files of one skill as `(path inside the skill, text)`.
pub fn skill_files(name: &str) -> Vec<(&'static str, &'static str)> {
    let pre = format!("{}/", name);
    gen::FILES.iter().filter_map(|(p, t)| p.strip_prefix(pre.as_str()).map(|r| (r, *t))).collect()
}

/// `description:` of the SKILL.md front matter (first line only).
pub fn description(text: &str) -> String {
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return String::new();
    }
    for l in lines {
        if l.trim() == "---" {
            break;
        }
        if let Some(d) = l.strip_prefix("description:") {
            return d.trim().trim_matches('"').to_string();
        }
    }
    String::new()
}

fn unknown(name: &str) -> PxlError {
    let n = names();
    let hint = if n.is_empty() { "this build ships no skills".to_string() } else { format!("skills: {}", n.join(", ")) };
    PxlError::usage(format!("unknown skill '{}'", name)).with_hint(hint)
}

pub fn run_list(out: &mut Out) -> Result<()> {
    let mut items = Vec::new();
    for n in names() {
        let fs = skill_files(n);
        let desc = fs.iter().find(|(p, _)| *p == "SKILL.md").map(|(_, t)| description(t)).unwrap_or_default();
        let refs: Vec<&str> = fs.iter().filter_map(|(p, _)| p.strip_prefix("references/")).collect();
        out.line(format!("{}  {}", n, desc));
        for r in &refs {
            out.line(format!("    reference: {}", r));
        }
        items.push(json!({"name": n, "description": desc, "references": refs}));
    }
    if items.is_empty() {
        out.line("no skills are embedded in this build");
    } else {
        out.line("read one: pxl skill show NAME [--ref FILE]; install for Claude Code and Codex: pxl skill install");
    }
    out.set("skills", json!(items));
    Ok(())
}

pub fn run_show(a: &SkillShowArgs, out: &mut Out) -> Result<()> {
    if !names().contains(&a.name.as_str()) {
        return Err(unknown(&a.name));
    }
    let fs = skill_files(&a.name);
    let (path, text) = match &a.reference {
        None => fs.iter().find(|(p, _)| *p == "SKILL.md").copied().ok_or_else(|| unknown(&a.name))?,
        Some(r) => {
            let r = r.trim_start_matches("references/");
            let cands = [format!("references/{}", r), format!("references/{}.md", r)];
            match fs.iter().find(|(p, _)| cands.iter().any(|c| c == p)) {
                Some(f) => *f,
                None => {
                    let avail: Vec<&str> = fs.iter().filter_map(|(p, _)| p.strip_prefix("references/")).collect();
                    return Err(PxlError::usage(format!("skill '{}' has no reference '{}'", a.name, r)).with_hint(format!("references: {}", avail.join(", "))));
                }
            }
        }
    };
    out.emit(text, json!({"skill": a.name, "file": path, "text": text}));
    Ok(())
}

/// The stamp line put right after the front matter of an installed SKILL.md, so an agent can see which pxl wrote it.
pub fn stamp_line() -> String {
    format!("<!-- installed by pxl {VERSION} (agentsprite-managed). If `pxl --version` prints another version, run `pxl skill install` to refresh. -->")
}

/// The text written for one bundled file: SKILL.md gets the stamp after its front matter, everything else is verbatim.
pub fn installed_text(rel: &str, text: &str) -> String {
    if rel != "SKILL.md" {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + 200);
    let mut fm = 0;
    let mut done = false;
    for l in text.split_inclusive('\n') {
        out.push_str(l);
        if !done && l.trim() == "---" {
            fm += 1;
            if fm == 2 {
                out.push_str(&stamp_line());
                out.push('\n');
                done = true;
            }
        }
    }
    if !done {
        out = format!("{}\n{}", stamp_line(), text);
    }
    out
}

fn hosts(sel: &[SkillHost]) -> Vec<SkillHost> {
    if sel.is_empty() {
        return vec![SkillHost::Claude, SkillHost::Codex];
    }
    let mut v: Vec<SkillHost> = Vec::new();
    for h in sel {
        if !v.contains(h) {
            v.push(*h);
        }
    }
    v
}

fn host_label(h: SkillHost) -> &'static str {
    match h {
        SkillHost::Claude => "claude",
        SkillHost::Codex => "codex",
    }
}

fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE").filter(|h| !h.is_empty()))
        .map(PathBuf::from)
        .ok_or_else(|| PxlError::usage("cannot find the home folder (HOME is not set)").with_hint("use --project to install under the current folder"))
}

/// `<root>/.claude/skills` or `<root>/.codex/skills`.
pub fn skills_root(host: SkillHost, project: bool) -> Result<PathBuf> {
    let root = if project { std::env::current_dir()? } else { home_dir()? };
    Ok(root.join(match host {
        SkillHost::Claude => ".claude",
        SkillHost::Codex => ".codex",
    })
    .join("skills"))
}

fn io_err(what: &str, p: &Path, e: std::io::Error) -> PxlError {
    PxlError::io(format!("{} {}: {}", what, p.slash(), e))
}

fn managed_version(dir: &Path) -> Option<String> {
    std::fs::read_to_string(dir.join(MARKER)).ok().map(|s| s.trim().to_string())
}

/// Write one skill folder next to its destination, then swap it in (the old managed folder is moved aside and removed after).
fn write_skill_atomically(name: &str, dest: &Path) -> Result<()> {
    let parent = dest.parent().ok_or_else(|| PxlError::io(format!("{} has no parent folder", dest.slash())))?;
    std::fs::create_dir_all(parent).map_err(|e| io_err("cannot create", parent, e))?;
    let pid = std::process::id();
    let tmp = parent.join(format!(".{name}.tmp-{pid}"));
    let backup = parent.join(format!(".{name}.old-{pid}"));
    for p in [&tmp, &backup] {
        if p.exists() {
            std::fs::remove_dir_all(p).map_err(|e| io_err("cannot clear", p, e))?;
        }
    }
    for (rel, text) in skill_files(name) {
        let f = tmp.join(rel);
        if let Some(d) = f.parent() {
            std::fs::create_dir_all(d).map_err(|e| io_err("cannot create", d, e))?;
        }
        std::fs::write(&f, installed_text(rel, text)).map_err(|e| io_err("cannot write", &f, e))?;
    }
    std::fs::write(tmp.join(MARKER), VERSION).map_err(|e| io_err("cannot write marker in", &tmp, e))?;
    if dest.exists() {
        std::fs::rename(dest, &backup).map_err(|e| io_err("cannot move aside", dest, e))?;
        if let Err(e) = std::fs::rename(&tmp, dest) {
            let _ = std::fs::rename(&backup, dest);
            let _ = std::fs::remove_dir_all(&tmp);
            return Err(io_err("cannot activate", dest, e));
        }
        let _ = std::fs::remove_dir_all(&backup);
    } else if let Err(e) = std::fs::rename(&tmp, dest) {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(io_err("cannot activate", dest, e));
    }
    Ok(())
}

fn refuse_unmanaged(dest: &Path, verb: &str) -> PxlError {
    PxlError::validation(format!("refusing to {} {}: it exists and was not installed by pxl (no {} marker)", verb, dest.slash(), MARKER))
        .with_hint("move or delete that folder yourself if you want pxl to manage it")
}

pub fn run_install(a: &SkillInstallArgs, out: &mut Out) -> Result<()> {
    let names = names();
    if names.is_empty() {
        return Err(PxlError::usage("this build ships no skills"));
    }
    let mut plan: Vec<(SkillHost, PathBuf, &str, bool)> = Vec::new();
    for h in hosts(&a.host) {
        let root = skills_root(h, a.project)?;
        for n in &names {
            let dest = root.join(n);
            // refuse before writing anything, so a refusal leaves no half install
            if dest.exists() && !dest.join(MARKER).is_file() {
                return Err(refuse_unmanaged(&dest, "replace"));
            }
            plan.push((h, dest.clone(), n, dest.exists()));
        }
    }
    let mut items = Vec::new();
    for (h, dest, n, existed) in &plan {
        let verb = match (a.dry_run, existed) {
            (true, false) => "would install",
            (true, true) => "would update",
            (false, false) => "installed",
            (false, true) => "updated",
        };
        if !a.dry_run {
            write_skill_atomically(n, dest)?;
        }
        out.line(format!("{:<14} {:<7} {}  ({})", verb, host_label(*h), n, dest.slash()));
        items.push(json!({"host": host_label(*h), "skill": n, "path": dest.slash(), "action": verb}));
    }
    if a.dry_run {
        out.line(format!("dry run: nothing written (pxl {VERSION})"));
    } else {
        out.line(format!("{} skill folders written by pxl {}; each SKILL.md carries the version, `pxl --version` must match it", plan.len(), VERSION));
        out.line("load pxl-operator first; the agent host may need a restart to see new skills");
    }
    out.set("version", json!(VERSION));
    out.set("dry_run", json!(a.dry_run));
    out.set("skills", json!(items));
    Ok(())
}

/// Folders under `root` that carry the marker: the bundled skill names plus any `pxl-*` folder a previous version installed.
fn managed_dirs(root: &Path) -> Vec<(String, PathBuf)> {
    let mut v = Vec::new();
    for n in names() {
        v.push((n.to_string(), root.join(n)));
    }
    if let Ok(rd) = std::fs::read_dir(root) {
        let mut extra: Vec<String> = rd
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("pxl-") && !names().contains(&n.as_str()) && root.join(n).join(MARKER).is_file())
            .collect();
        extra.sort();
        for n in extra {
            let p = root.join(&n);
            v.push((n, p));
        }
    }
    v
}

pub fn run_uninstall(a: &SkillInstallArgs, out: &mut Out) -> Result<()> {
    let mut todo: Vec<(SkillHost, String, PathBuf)> = Vec::new();
    for h in hosts(&a.host) {
        let root = skills_root(h, a.project)?;
        for (n, p) in managed_dirs(&root) {
            if !p.exists() {
                continue;
            }
            if !p.join(MARKER).is_file() {
                return Err(refuse_unmanaged(&p, "remove"));
            }
            todo.push((h, n, p));
        }
    }
    let mut items = Vec::new();
    for (h, n, p) in &todo {
        if !a.dry_run {
            std::fs::remove_dir_all(p).map_err(|e| io_err("cannot remove", p, e))?;
        }
        let verb = if a.dry_run { "would remove" } else { "removed" };
        out.line(format!("{:<14} {:<7} {}  ({})", verb, host_label(*h), n, p.slash()));
        items.push(json!({"host": host_label(*h), "skill": n, "path": p.slash(), "action": verb}));
    }
    if todo.is_empty() {
        out.line("no pxl-managed skills found there; nothing to remove");
    }
    out.set("dry_run", json!(a.dry_run));
    out.set("skills", json!(items));
    Ok(())
}

/// Where the skills go (or are), per host, and whether they are installed and from which pxl version.
pub fn run_path(a: &SkillPathArgs, out: &mut Out) -> Result<()> {
    let total = names().len();
    let mut items = Vec::new();
    for h in hosts(&a.host) {
        let root = skills_root(h, a.project)?;
        let mut vers: Vec<String> = Vec::new();
        let mut n_ok = 0;
        for n in names() {
            if let Some(v) = managed_version(&root.join(n)) {
                n_ok += 1;
                if !vers.contains(&v) {
                    vers.push(v);
                }
            }
        }
        let current = n_ok == total && vers.len() == 1 && vers[0] == VERSION;
        let state = if n_ok == 0 {
            "not installed".to_string()
        } else if current {
            format!("installed {}/{} from pxl {} (current)", n_ok, total, VERSION)
        } else {
            format!("installed {}/{} from pxl {} (binary is {}: run `pxl skill install`)", n_ok, total, vers.join(", "), VERSION)
        };
        out.line(format!("{:<7} {}  {}", host_label(h), root.slash(), state));
        items.push(json!({"host": host_label(h), "path": root.slash(), "installed": n_ok, "bundled": total, "versions": vers, "current": current}));
    }
    out.set("version", json!(VERSION));
    out.set("hosts", json!(items));
    Ok(())
}
