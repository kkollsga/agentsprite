//! Work lists, decisions, exceptions, board, brief and the reviewer workflow (M6 R4).
//!
//! * Tasks live in `<project>/tasks.toml` (a single-image asset keeps its own `tasks.toml`): `[[task]]` tables with a stable
//!   id `t0001`, `asset`, `text`, `kind` (draw | fix | review | question), optional `assign`, `priority`, `status`
//!   (open | done), timestamps and the closing note. Ids are never reused.
//! * Decisions go to `<project>/decisions.md`, one dated line each; exceptions to `<asset>/exceptions.md`.
//! * `pxl review next | note | approve | rework` is the reviewer's side: notes carry a region and a crop image.

use crate::fsutil::PathSlash;
use crate::asset;
use crate::cli::*;
use crate::img::{Rgba, RgbaImage};
use crate::manifest::Manifest;
use crate::output::{Out, PxlError, Result};
use crate::{compare, console, fsutil, lease, load, png, project, pxl, status, template, versions};
use serde_json::json;
use std::path::{Path, PathBuf};

pub const KINDS: [&str; 4] = ["draw", "fix", "review", "question"];

#[derive(Clone, Debug)]
pub struct Task {
    pub id: String,
    pub asset: String,
    pub text: String,
    pub kind: String,
    pub assign: Option<String>,
    pub priority: i64,
    pub status: String,
    pub created: u64,
    pub by: String,
    pub note: String,
    /// The task waits for this decision (`d0001`) to be decided (`pxl ask`, `pxl decide`).
    pub after: Option<String>,
    /// Where the task came from: `review-note` for the fix task of a `pxl review note` (a later rework of the same issue adds no second task).
    pub src: String,
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// `YYYY-MM-DD` of a unix time (UTC).
pub fn date_of(ts: u64) -> String {
    let z = (ts / 86400) as i64 + 719468;
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

/// Where tasks, decisions and notes of the asset at `file` live: the project folder, else the asset folder.
pub fn home_of(file: &Path) -> PathBuf {
    let d = asset::dir_of(file);
    Manifest::find_file(&d).map(|m| asset::dir_of(&m)).unwrap_or(d)
}

fn tasks_path(home: &Path) -> PathBuf {
    home.join("tasks.toml")
}

pub fn load_all(home: &Path) -> Vec<Task> {
    let Ok(text) = std::fs::read_to_string(tasks_path(home)) else { return Vec::new() };
    let Ok(t) = text.parse::<toml::Table>() else { return Vec::new() };
    let mut v = Vec::new();
    for x in t.get("task").and_then(|x| x.as_array()).cloned().unwrap_or_default() {
        let Some(x) = x.as_table() else { continue };
        let s = |k: &str| x.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        v.push(Task {
            id: s("id"),
            asset: s("asset"),
            text: s("text"),
            kind: if s("kind").is_empty() { "draw".into() } else { s("kind") },
            assign: Some(s("assign")).filter(|a| !a.is_empty()),
            priority: x.get("priority").and_then(|p| p.as_integer()).unwrap_or(0),
            status: if s("status").is_empty() { "open".into() } else { s("status") },
            created: x.get("created").and_then(|p| p.as_integer()).unwrap_or(0) as u64,
            by: s("by"),
            note: s("note"),
            after: Some(s("after")).filter(|a| !a.is_empty()),
            src: s("src"),
        });
    }
    v
}

fn save_all(home: &Path, tasks: &[Task]) -> Result<()> {
    let mut s = String::from("# pxl work list. Edit with `pxl task add | done`, not by hand.\n");
    for t in tasks {
        s.push_str(&format!("\n[[task]]\nid = {}\nasset = {}\ntext = {}\nkind = {}\n", asset::q(&t.id), asset::q(&t.asset), asset::q(&t.text), asset::q(&t.kind)));
        if let Some(a) = &t.assign {
            s.push_str(&format!("assign = {}\n", asset::q(a)));
        }
        s.push_str(&format!("priority = {}\nstatus = {}\ncreated = {}\n", t.priority, asset::q(&t.status), t.created));
        if !t.by.is_empty() {
            s.push_str(&format!("by = {}\n", asset::q(&t.by)));
        }
        if !t.note.is_empty() {
            s.push_str(&format!("note = {}\n", asset::q(&t.note)));
        }
        if let Some(a) = &t.after {
            s.push_str(&format!("after = {}\n", asset::q(a)));
        }
        if !t.src.is_empty() {
            s.push_str(&format!("src = {}\n", asset::q(&t.src)));
        }
    }
    fsutil::atomic_write_plain(&tasks_path(home), s.as_bytes())
}

/// Add a task (asset = the asset name). Returns its id.
pub fn add_task(home: &Path, asset_name: &str, text: &str, kind: &str, assign: Option<String>, priority: i64, by: &str) -> Result<String> {
    add_task_after(home, asset_name, text, kind, assign, priority, by, None)
}

/// Like [`add_task`]; with `after` the task stays blocked until that decision is decided.
#[allow(clippy::too_many_arguments)]
pub fn add_task_after(home: &Path, asset_name: &str, text: &str, kind: &str, assign: Option<String>, priority: i64, by: &str, after: Option<String>) -> Result<String> {
    add_task_src(home, asset_name, text, kind, assign, priority, by, after, "")
}

#[allow(clippy::too_many_arguments)]
pub fn add_task_src(home: &Path, asset_name: &str, text: &str, kind: &str, assign: Option<String>, priority: i64, by: &str, after: Option<String>, src: &str) -> Result<String> {
    if let Some(d) = &after {
        if crate::decisions::get(home, d).is_none() {
            return Err(PxlError::usage(format!("--after {}: no such decision", d)).with_hint("pxl ask opens one; pxl decisions lists them"));
        }
    }
    if !KINDS.contains(&kind) {
        return Err(PxlError::usage(format!("unknown task kind '{}'", kind)).with_hint(format!("kinds: {}", KINDS.join(", "))));
    }
    let mut all = load_all(home);
    let n = all.iter().filter_map(|t| t.id.strip_prefix('t').and_then(|n| n.parse::<u32>().ok())).max().unwrap_or(0) + 1;
    let id = format!("t{:04}", n);
    all.push(Task { id: id.clone(), asset: asset_name.to_string(), text: text.to_string(), kind: kind.to_string(), assign, priority, status: "open".into(), created: now(), by: by.to_string(), note: String::new(), after, src: src.to_string() });
    save_all(home, &all)?;
    Ok(id)
}

/// Open tasks of an asset assigned to `agent` or to nobody.
pub fn open_for(home: &Path, asset_name: &str, agent: &str) -> usize {
    load_all(home).iter().filter(|t| t.asset == asset_name && t.status == "open" && !blocked(home, t) && t.assign.as_deref().map_or(true, |a| a == agent)).count()
}

/// True while the task waits for an undecided decision.
pub fn blocked(home: &Path, t: &Task) -> bool {
    t.status == "open" && t.after.as_deref().map_or(false, |d| crate::decisions::is_open(home, d))
}

/// The asset has open tasks and every one of them waits for a decision: `next` leaves it alone.
pub fn all_blocked(home: &Path, asset_name: &str) -> bool {
    let all = load_all(home);
    let open: Vec<&Task> = all.iter().filter(|t| t.asset == asset_name && t.status == "open").collect();
    !open.is_empty() && open.iter().all(|t| blocked(home, t))
}

/// How much work waits for `agent` on an asset: 2 = a task assigned to it, 1 = only unassigned tasks, 0 = none.
pub fn rank_for(home: &Path, asset_name: &str, agent: &str) -> u8 {
    let all = load_all(home);
    let mine = all.iter().filter(|t| t.asset == asset_name && t.status == "open" && !blocked(home, t));
    let mut rank = 0;
    for t in mine {
        match t.assign.as_deref() {
            Some(a) if a == agent => return 2,
            None => rank = 1,
            _ => {}
        }
    }
    rank
}

pub fn run_task(c: &TaskCmd, out: &mut Out) -> Result<()> {
    match c {
        TaskCmd::Add { asset: a, text, kind, assign, priority, after } => {
            let file = fsutil::resolve_part(a)?;
            let home = home_of(&file);
            let name = asset::asset_name(&asset::dir_of(&file));
            let by = out.agent().unwrap_or("user").to_string();
            let id = add_task_after(&home, &name, text, kind, assign.clone(), *priority, &by, after.clone())?;
            write_page(&asset::dir_of(&file));
            out.emit(format!("{} added: {} [{}] {}{}", id, name, kind, text, after.as_ref().map_or(String::new(), |d| format!(" (blocked until {} is decided)", d))), json!({"id": id, "asset": name, "kind": kind, "assign": assign, "priority": priority, "after": after}));
        }
        TaskCmd::Done { id, note } => {
            let home = find_home_with_task(id)?;
            let mut all = load_all(&home);
            let t = all.iter_mut().find(|t| t.id == *id).ok_or_else(|| PxlError::usage(format!("no task {}", id)))?;
            if t.status == "done" {
                out.emit(format!("{} was already done", id), json!({"id": id, "done": true, "changed": false}));
                return Ok(());
            }
            t.status = "done".into();
            t.note = note.clone().unwrap_or_default();
            let name = t.asset.clone();
            save_all(&home, &all)?;
            if let Some(f) = asset::scan(&home).into_iter().find(|x| x.name == name) {
                write_page(&f.dir);
            }
            out.emit(format!("{} done", id), json!({"id": id, "done": true, "changed": true}));
        }
    }
    Ok(())
}

fn cwd() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// The folder whose tasks.toml holds `id`: the project around the current folder, else the current folder / asset folder.
fn find_home_with_task(id: &str) -> Result<PathBuf> {
    let c = cwd();
    let mut cands = vec![];
    if let Some(m) = Manifest::find_file(&c) {
        cands.push(asset::dir_of(&m));
    }
    cands.push(c.clone());
    for h in cands {
        if load_all(&h).iter().any(|t| t.id == id) {
            return Ok(h);
        }
    }
    Err(PxlError::usage(format!("no task {} here", id)).with_hint("run it in the project folder (or the single-image asset folder)"))
}

pub fn home_here() -> PathBuf {
    let c = cwd();
    Manifest::find_file(&c).map(|m| asset::dir_of(&m)).unwrap_or(c)
}

pub fn run_tasks(a: &TasksArgs, out: &mut Out) -> Result<()> {
    let (home, only) = match &a.asset {
        Some(x) => {
            let f = fsutil::resolve_part(x)?;
            (home_of(&f), Some(asset::asset_name(&asset::dir_of(&f))))
        }
        None => (home_here(), None),
    };
    let all = load_all(&home);
    let me = out.agent().map(String::from);
    if a.mine && me.is_none() {
        return Err(PxlError::usage("--mine needs --agent NAME"));
    }
    let groups: Vec<asset::AssetInfo> = asset::scan(&home);
    let mut sel: Vec<&Task> = all
        .iter()
        .filter(|t| a.all || t.status == "open")
        .filter(|t| only.as_deref().map_or(true, |n| t.asset == n))
        .filter(|t| !a.mine || t.assign.as_deref() == me.as_deref())
        .filter(|t| a.assign.as_deref().map_or(true, |x| t.assign.as_deref() == Some(x)))
        .filter(|t| a.group.as_deref().map_or(true, |g| groups.iter().any(|x| x.name == t.asset && (x.group == g || x.group.starts_with(&format!("{}/", g))))))
        .collect();
    sel.sort_by(|x, y| y.priority.cmp(&x.priority).then(x.id.cmp(&y.id)));
    let mut text = String::new();
    for t in &sel {
        text.push_str(&format!("{} {:<5} p{} {:<24} [{}] {}{}{}\n", t.id, t.status, t.priority, t.asset, t.kind, t.text, t.assign.as_ref().map_or(String::new(), |a| format!("  -> {}", a)), if blocked(&home, t) { format!("  (blocked until {} is decided)", t.after.as_deref().unwrap_or("")) } else { String::new() }));
    }
    text.push_str(&format!("{} task(s)", sel.len()));
    out.emit(
        text,
        json!({"tasks": sel.iter().map(|t| json!({"id": t.id, "asset": t.asset, "text": t.text, "kind": t.kind, "assign": t.assign, "priority": t.priority, "status": t.status, "note": t.note, "after": t.after, "blocked": blocked(&home, t)})).collect::<Vec<_>>()}),
    );
    Ok(())
}

// ---------------------------------------------------------------------------------- decisions, exceptions

fn decisions_path(home: &Path) -> PathBuf {
    home.join("decisions.md")
}

pub fn decisions(home: &Path) -> Vec<(String, Option<String>, String)> {
    // (line, asset, text)
    let Ok(t) = std::fs::read_to_string(decisions_path(home)) else { return Vec::new() };
    t.lines()
        .filter(|l| l.starts_with("- "))
        .map(|l| {
            let asset = l.find("(asset ").and_then(|i| l[i + 7..].find(')').map(|j| l[i + 7..i + 7 + j].to_string()));
            (l.to_string(), asset, l.to_string())
        })
        .collect()
}

/// Append one line to decisions.md (creating it).
pub fn append_decision_line(home: &Path, line: &str) -> Result<()> {
    let mut text = std::fs::read_to_string(decisions_path(home)).unwrap_or_else(|_| "# Decisions\n\nOne dated line each; `pxl decide` appends.\n\n".into());
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(line);
    text.push('\n');
    fsutil::atomic_write_plain(&decisions_path(home), text.as_bytes())
}

pub fn run_decide(a: &DecideArgs, out: &mut Out) -> Result<()> {
    if let Some(opt) = &a.option {
        if !crate::decisions::is_id(&a.text) {
            return Err(PxlError::usage("`pxl decide ID OPTION`: the first word must be a decision id (d0001); to record a plain decision give only the text"));
        }
        return crate::decisions::run_decide_option(&a.text, opt, &a.by, out);
    }
    if crate::decisions::is_id(&a.text) {
        let home = home_here();
        let opts = crate::decisions::get(&home, &a.text).map(|d| d.options.iter().map(|o| o.key.clone()).collect::<Vec<_>>().join(", ")).unwrap_or_default();
        return Err(PxlError::usage(format!("{} needs an option: pxl decide {} <{}>", a.text, a.text, if opts.is_empty() { "option".into() } else { opts })));
    }
    for kv in &a.set {
        crate::intake::apply_set(&home_here(), kv, &a.by)?;
    }
    let (home, asset_name, adir) = match &a.asset {
        Some(x) => {
            let f = fsutil::resolve_part(x)?;
            (home_of(&f), Some(asset::asset_name(&asset::dir_of(&f))), Some(asset::dir_of(&f)))
        }
        None => (home_here(), None, None),
    };
    let line = format!("- {} [{}]{} {}", date_of(now()), a.by, asset_name.as_ref().map_or(String::new(), |n| format!(" (asset {})", n)), a.text.trim());
    let mut text = std::fs::read_to_string(decisions_path(&home)).unwrap_or_else(|_| "# Decisions\n\nOne dated line each; `pxl decide` appends.\n\n".into());
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&line);
    text.push('\n');
    fsutil::atomic_write_plain(&decisions_path(&home), text.as_bytes())?;
    if let Some(d) = adir {
        write_page(&d);
    }
    out.emit(format!("recorded: {}", line.trim_start_matches("- ")), json!({"decision": line, "file": decisions_path(&home).slash()}));
    Ok(())
}

pub fn run_exception(c: &ExceptionCmd, out: &mut Out) -> Result<()> {
    match c {
        ExceptionCmd::Add { asset: a, change, reason } => {
            if reason.trim().is_empty() {
                return Err(PxlError::usage("an exception needs a --reason"));
            }
            let f = fsutil::resolve_part(a)?;
            let d = asset::dir_of(&f);
            let p = d.join("exceptions.md");
            let mut text = std::fs::read_to_string(&p).unwrap_or_else(|_| format!("# Exceptions of {}\n\n", asset::asset_name(&d)));
            let by = out.agent().unwrap_or("user");
            text.push_str(&format!("- {} [{}] {} (reason: {})\n", date_of(now()), by, change.trim(), reason.trim()));
            fsutil::atomic_write_plain(&p, text.as_bytes())?;
            write_page(&d);
            out.emit(format!("exception recorded for {}: {} ({})", asset::asset_name(&d), change, reason), json!({"asset": asset::asset_name(&d), "file": p.slash()}));
        }
        ExceptionCmd::List { asset: a } => {
            let home = home_here();
            let files: Vec<(String, PathBuf)> = match a {
                Some(x) => {
                    let f = fsutil::resolve_part(x)?;
                    vec![(asset::asset_name(&asset::dir_of(&f)), asset::dir_of(&f))]
                }
                None => asset::scan(&home).into_iter().map(|x| (x.name, x.dir)).collect(),
            };
            let mut text = String::new();
            let mut js = Vec::new();
            for (n, d) in &files {
                for l in std::fs::read_to_string(d.join("exceptions.md")).unwrap_or_default().lines().filter(|l| l.starts_with("- ")) {
                    text.push_str(&format!("{}: {}\n", n, l.trim_start_matches("- ")));
                    js.push(json!({"asset": n, "exception": l.trim_start_matches("- ")}));
                }
            }
            text.push_str(&format!("{} exception(s) over {} asset(s)", js.len(), files.len()));
            out.emit(text, json!({"exceptions": js, "checked": {"ok": files.len(), "of": files.len()}}));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------- compare page

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// `compare.html` of an asset: the compare image, the variants, decisions, open tasks, exceptions, versions.
pub fn write_page(dir: &Path) {
    let file = dir.join(asset::ASSET_FILE);
    if !file.is_file() {
        return;
    }
    let home = home_of(&file);
    let name = asset::asset_name(dir);
    let mut h = format!("<!doctype html><meta charset=\"utf-8\"><title>{0}</title><style>body{{font:14px sans-serif;margin:16px;max-width:900px}}img{{image-rendering:pixelated;max-width:100%}}</style><h1>{0}</h1>", esc(&name));
    if dir.join("compare.png").is_file() {
        h.push_str("<p><img src=\"compare.png\" alt=\"input | latest\"></p>");
    }
    if !crate::style::style_frames(&home).is_empty() {
        if let Some((n, rel)) = crate::style::nearest_style_link(&home, dir, &name) {
            h.push_str(&format!("<h2>Nearest style frame: {}</h2><p><img src=\"{}\" alt=\"style frame {}\"></p>", esc(&n), esc(&rel), esc(&n)));
        }
    }
    if dir.join("variants.png").is_file() {
        h.push_str("<h2>Variants</h2><p><img src=\"variants.png\" alt=\"variants\"></p>");
    }
    let dec: Vec<String> = decisions(&home).into_iter().filter(|d| d.1.as_deref().map_or(true, |a| a == name)).map(|d| d.0).collect();
    if !dec.is_empty() {
        h.push_str("<h2>Decisions</h2><ul>");
        for d in dec {
            h.push_str(&format!("<li>{}</li>", esc(d.trim_start_matches("- "))));
        }
        h.push_str("</ul>");
    }
    let open: Vec<Task> = load_all(&home).into_iter().filter(|t| t.asset == name && t.status == "open").collect();
    if !open.is_empty() {
        h.push_str("<h2>Open tasks</h2><ul>");
        for t in open {
            h.push_str(&format!("<li>{} [{}] {}{}{}</li>", t.id, t.kind, esc(&t.text), t.assign.clone().map_or(String::new(), |a| format!(" -&gt; {}", esc(&a))), if blocked(&home, &t) { format!(" (blocked until {} is decided)", esc(t.after.as_deref().unwrap_or(""))) } else { String::new() }));
        }
        h.push_str("</ul>");
    }
    if let Ok(t) = std::fs::read_to_string(dir.join("exceptions.md")) {
        h.push_str("<h2>Exceptions</h2><ul>");
        for l in t.lines().filter(|l| l.starts_with("- ")) {
            h.push_str(&format!("<li>{}</li>", esc(l.trim_start_matches("- "))));
        }
        h.push_str("</ul>");
    }
    let vs = versions::list(&file);
    if !vs.is_empty() {
        h.push_str("<h2>Versions</h2><ul>");
        for v in vs.iter().rev().take(20) {
            h.push_str(&format!("<li><a href=\"versions/{}\">v{:03}</a></li>", esc(&v.png.file_name().unwrap().to_string_lossy()), v.n));
        }
        h.push_str("</ul>");
    }
    let _ = fsutil::atomic_write_plain(&dir.join("compare.html"), h.as_bytes());
}

// ---------------------------------------------------------------------------------- board

struct Row {
    name: String,
    group: String,
    state: String,
    version: String,
    open: usize,
    approved_by: Option<String>,
}

fn rows(home: &Path) -> Vec<Row> {
    let tasks = load_all(home);
    asset::scan(home)
        .into_iter()
        .map(|a| {
            let st = status::read(&a.file);
            Row {
                version: versions::latest(&a.file).map_or("-".into(), |v| format!("v{:03}", v.n)),
                open: tasks.iter().filter(|t| t.asset == a.name && t.status == "open").count(),
                state: st.state,
                approved_by: st.approved_by,
                group: a.group,
                name: a.name,
            }
        })
        .collect()
}

pub fn run_board(a: &BoardArgs, out: &mut Out) -> Result<()> {
    let home = home_here();
    let rs = rows(&home);
    let tasks = load_all(&home);
    let dec = decisions(&home);
    let mut groups: Vec<String> = rs.iter().map(|r| r.group.clone()).collect();
    groups.sort();
    groups.dedup();
    let mut text = String::new();
    let mut js = Vec::new();
    let mut html = String::from("<!doctype html><meta charset=\"utf-8\"><title>pxl board</title><style>body{font:14px sans-serif;margin:16px;max-width:1000px}table{border-collapse:collapse}td,th{border:1px solid #888;padding:3px 8px;text-align:left}</style><h1>Board</h1>");
    for g in &groups {
        let members: Vec<&Row> = rs.iter().filter(|r| &r.group == g).collect();
        let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
        for r in &members {
            *counts.entry(r.state.as_str()).or_insert(0) += 1;
        }
        let open: usize = members.iter().map(|r| r.open).sum();
        let label = if g.is_empty() { "(top level)" } else { g.as_str() };
        text.push_str(&format!("{}: {} asset(s) [{}], {} open task(s)\n", label, members.len(), counts.iter().map(|(k, v)| format!("{} {}", v, k)).collect::<Vec<_>>().join(", "), open));
        html.push_str(&format!("<h2>{}</h2><table><tr><th>asset<th>state<th>version<th>open tasks</tr>", esc(label)));
        for r in &members {
            text.push_str(&format!("  {:<26} {:<11} {:<5}{}{}\n", r.name, r.state, r.version, if r.open > 0 { format!(" {} open", r.open) } else { String::new() }, r.approved_by.as_ref().map_or(String::new(), |b| format!(" (approved by {})", b))));
            html.push_str(&format!("<tr><td>{}<td>{}<td>{}<td>{}</tr>", esc(&r.name), esc(&r.state), r.version, r.open));
        }
        html.push_str("</table>");
        js.push(json!({"group": g, "assets": members.iter().map(|r| json!({"asset": r.name, "state": r.state, "version": r.version, "open_tasks": r.open, "approved_by": r.approved_by})).collect::<Vec<_>>(), "counts": counts}));
    }
    let open: Vec<&Task> = tasks.iter().filter(|t| t.status == "open").collect();
    text.push_str(&format!("open tasks: {}\n", open.len()));
    html.push_str("<h2>Open tasks</h2><ul>");
    for t in &open {
        text.push_str(&format!("  {} {} [{}] {}{}\n", t.id, t.asset, t.kind, t.text, t.assign.as_ref().map_or(String::new(), |a| format!(" -> {}", a))));
        html.push_str(&format!("<li>{} {} [{}] {}{}</li>", t.id, esc(&t.asset), t.kind, esc(&t.text), t.assign.as_ref().map_or(String::new(), |a| format!(" -&gt; {}", esc(a)))));
    }
    html.push_str("</ul><h2>Decisions</h2><ul>");
    if !dec.is_empty() {
        text.push_str("decisions:\n");
    }
    for d in dec.iter().rev().take(15) {
        text.push_str(&format!("  {}\n", d.0.trim_start_matches("- ")));
        html.push_str(&format!("<li>{}</li>", esc(d.0.trim_start_matches("- "))));
    }
    html.push_str("</ul>");
    let open_dec: Vec<crate::decisions::Decision> = crate::decisions::load(&home).into_iter().filter(|d| d.open()).collect();
    let brief = crate::intake::read_brief(&home);
    let unconfirmed: Vec<String> = brief.get("assumptions").and_then(|t| t.as_table()).map(|t| t.iter().filter(|(_, v)| v.as_str() == Some("assumed")).map(|(k, _)| k.clone()).collect()).unwrap_or_default();
    if !open_dec.is_empty() {
        text.push_str("open decisions (pxl decide ID OPTION):\n");
        html.push_str("<h2>Open decisions</h2><ul>");
        for d in &open_dec {
            text.push_str(&format!("  {} {}: {} (recommend {}: {})\n", d.id, d.topic, d.options.iter().map(|o| o.key.as_str()).collect::<Vec<_>>().join("/"), d.recommend, d.why));
            html.push_str(&format!("<li>{} {}: recommend {} ({})</li>", esc(&d.id), esc(&d.topic), esc(&d.recommend), esc(&d.why)));
        }
        html.push_str("</ul>");
    }
    if !unconfirmed.is_empty() {
        text.push_str(&format!("assumptions not confirmed: {} (pxl decide \"why\" --set key=value)\n", unconfirmed.join(", ")));
        html.push_str(&format!("<p>Assumptions not confirmed: {}</p>", esc(&unconfirmed.join(", "))));
    }
    text.push_str(&format!("{} asset(s) in {} group(s)", rs.len(), groups.len()));
    if let Some(f) = &a.html {
        fsutil::atomic_write_plain(Path::new(f), html.as_bytes())?;
        text.push_str(&format!("\nwrote {}", f));
    }
    out.emit(text, json!({"groups": js, "open_tasks": open.len(), "decisions": dec.iter().map(|d| d.0.clone()).collect::<Vec<_>>(), "open_decisions": open_dec.iter().map(|d| json!({"id": d.id, "topic": d.topic, "recommend": d.recommend})).collect::<Vec<_>>(), "unconfirmed_assumptions": unconfirmed, "html": a.html}));
    Ok(())
}

// ---------------------------------------------------------------------------------- brief

pub fn run_brief(a: &BriefArgs, out: &mut Out) -> Result<()> {
    let home = home_here();
    let mut text = String::new();
    let mut js = serde_json::Map::new();
    let files: Vec<(String, PathBuf)> = match fsutil::resolve_part(&a.target) {
        Ok(f) if asset::is_asset_file(&f) => vec![(asset::asset_name(&asset::dir_of(&f)), f)],
        _ => {
            let g = a.target.trim_matches('/');
            let v: Vec<(String, PathBuf)> = asset::scan(&home).into_iter().filter(|x| x.group == g || x.group.starts_with(&format!("{}/", g))).map(|x| (x.name, x.file)).collect();
            if v.is_empty() {
                return Err(PxlError::usage(format!("'{}' is neither an asset nor a group", a.target)).with_hint("pxl tree lists both"));
            }
            v
        }
    };
    let single = files.len() == 1 && asset::is_asset_file(&fsutil::resolve_part(&a.target).unwrap_or_default());
    let tasks = load_all(&home);
    let dec = decisions(&home);
    let m = Manifest::find_file(&home).and_then(|p| Manifest::load(&p).ok());
    text.push_str(&format!("BRIEF {}\n", a.target));
    let bf = crate::intake::read_brief(&home);
    if !bf.is_empty() {
        let g = |k: &str| crate::intake::value_of(&bf, k);
        text.push_str(&format!("project brief: console {}, fidelity {}, cadence {}, tradeoff {}, exceptions {}\n", g("console"), g("fidelity"), g("cadence"), crate::intake::list_of(&bf, "tradeoff").join(" > "), g("exceptions")));
        let unconfirmed: Vec<String> = bf.get("assumptions").and_then(|t| t.as_table()).map(|t| t.iter().filter(|(_, v)| v.as_str() == Some("assumed")).map(|(k, _)| k.clone()).collect()).unwrap_or_default();
        if !unconfirmed.is_empty() {
            text.push_str(&format!("assumptions (not confirmed): {}\n", unconfirmed.join(", ")));
        }
        js.insert("brief".into(), json!({"console": g("console"), "fidelity": g("fidelity"), "cadence": g("cadence"), "unconfirmed": unconfirmed}));
    }
    for d in crate::decisions::load(&home).into_iter().filter(|d| d.open()) {
        text.push_str(&format!("open decision {} {}: options {}; recommended {} ({})\n", d.id, d.topic, d.options.iter().map(|o| o.key.as_str()).collect::<Vec<_>>().join("/"), d.recommend, d.why));
    }
    if let Some((id, t)) = crate::style::rules(&home).first().map(|r| (r.0.clone(), r.1.clone())) {
        let _ = (id, t);
        text.push_str(&format!("design language: style.md ({} rules; `pxl review checklist ASSET`)\n", crate::style::rules(&home).len()));
    }
    let names: Vec<&str> = files.iter().map(|f| f.0.as_str()).collect();
    text.push_str("decisions:\n");
    let mut dl = Vec::new();
    for d in &dec {
        if d.1.as_deref().map_or(true, |x| names.contains(&x)) {
            text.push_str(&format!("  {}\n", d.0.trim_start_matches("- ")));
            dl.push(d.0.clone());
        }
    }
    if dl.is_empty() {
        text.push_str("  (none)\n");
    }
    js.insert("decisions".into(), json!(dl));
    let mut assets_js = Vec::new();
    for (name, file) in &files {
        let st = status::read(file);
        let dir = asset::dir_of(file);
        let open: Vec<&Task> = tasks.iter().filter(|t| t.asset == *name && t.status == "open").collect();
        text.push_str(&format!("\n{} state {} owner {} version {}{}\n", name, st.state, st.owner.as_deref().unwrap_or("-"), versions::latest(file).map_or("-".into(), |v| format!("v{:03}", v.n)), st.approved_by.as_ref().map_or(String::new(), |b| format!(" approved by {}", b))));
        text.push_str("  open tasks:\n");
        for t in &open {
            text.push_str(&format!("    {} [{}] {}{}\n", t.id, t.kind, t.text, t.assign.as_ref().map_or(String::new(), |a| format!(" -> {}", a))));
        }
        if open.is_empty() {
            text.push_str("    (none)\n");
        }
        text.push_str("  review history:\n");
        for n in &st.review {
            text.push_str(&format!("    {} {}: {}\n", n.by, if n.verdict.is_empty() { "note" } else { &n.verdict }, n.text));
        }
        if st.review.is_empty() {
            text.push_str("    (none)\n");
        }
        let mut budget_lines: Vec<String> = Vec::new();
        if let Some(m) = &m {
            if let Some(d) = m.find(file) {
                if let Ok((sp, tp)) = crate::scene::hardware(m) {
                    let mut cache = std::collections::HashMap::new();
                    for sc in versions::scenes_of(m, &d.name) {
                        let rep = crate::scene::check_scene(m, sc, &sp, &tp, &mut cache);
                        for r in rep.rows.iter().filter(|r| r.item.contains("VRAM") || r.item.starts_with("group") || r.item.starts_with("BG")) {
                            budget_lines.push(format!("{} {} {} / {} {}", sc.name, r.item, r.used, r.limit, if r.ok { "ok" } else { "FAIL" }));
                        }
                    }
                }
            }
        }
        text.push_str("  budgets:\n");
        for b in &budget_lines {
            text.push_str(&format!("    {}\n", b));
        }
        if budget_lines.is_empty() {
            text.push_str("    (the asset is in no scene)\n");
        }
        let ex = std::fs::read_to_string(dir.join("exceptions.md")).unwrap_or_default();
        let exl: Vec<&str> = ex.lines().filter(|l| l.starts_with("- ")).collect();
        if !exl.is_empty() {
            text.push_str("  exceptions:\n");
            for e in &exl {
                text.push_str(&format!("    {}\n", e.trim_start_matches("- ")));
            }
        }
        text.push_str(&format!("  paths: compare {}  latest {}  input {}  review {}\n", dir.join("compare.png").slash(), versions::latest(file).map_or("-".into(), |v| v.png.slash()), dir.join("input").slash(), dir.join("review").slash()));
        assets_js.push(json!({"asset": name, "state": st.state, "owner": st.owner, "approved_by": st.approved_by, "open_tasks": open.iter().map(|t| json!({"id": t.id, "kind": t.kind, "text": t.text})).collect::<Vec<_>>(),
            "review": st.review.iter().map(|n| json!({"by": n.by, "verdict": n.verdict, "text": n.text})).collect::<Vec<_>>(), "budgets": budget_lines,
            "paths": {"compare": dir.join("compare.png").slash(), "input": dir.join("input").slash(), "review": dir.join("review").slash()}}));
    }
    let _ = single;
    js.insert("assets".into(), json!(assets_js));
    js.insert("checked".into(), json!({"ok": files.len(), "of": files.len()}));
    out.emit(text.trim_end(), serde_json::Value::Object(js));
    Ok(())
}

// ---------------------------------------------------------------------------------- reviewer workflow

const DIRECTION_WORDS: [&str; 40] = [
    "red", "green", "blue", "yellow", "orange", "purple", "pink", "brown", "black", "white", "grey", "gray", "teal", "cyan", "magenta", "colour", "color", "darker", "lighter", "brighter", "left", "right", "up", "down", "top", "bottom", "above", "below", "source", "should", "too", "more", "less",
    "wider", "narrower", "taller", "shorter", "shifted", "mirrored", "flipped",
];

/// True when a note names a colour or a direction and so must carry a region.
pub fn needs_region(text: &str) -> bool {
    text.to_lowercase().split(|c: char| !c.is_alphanumeric()).any(|w| DIRECTION_WORDS.contains(&w))
}

pub fn review_sub(a: &ReviewArgs, out: &mut Out) -> Option<Result<()>> {
    match a.parts.first().map(|s| s.as_str()) {
        Some("next") => Some(review_next(out)),
        Some("note") => Some(review_note(a, out)),
        Some("approve") => Some(review_approve(a, out)),
        Some("rework") => Some(review_rework(a, out)),
        Some("checklist") => Some(review_checklist(a, out)),
        _ => None,
    }
}

fn reviewer(out: &Out, cmd: &str) -> Result<String> {
    out.agent().map(String::from).ok_or_else(|| PxlError::usage(format!("`review {}` needs --agent NAME (the reviewer identity is recorded)", cmd)))
}

fn review_checklist(a: &ReviewArgs, out: &mut Out) -> Result<()> {
    let asset_arg = a.parts.get(1).ok_or_else(|| PxlError::usage("pxl review checklist ASSET"))?;
    let file = fsutil::resolve_part(asset_arg)?;
    crate::style::checklist(&file, out)
}

fn review_next(out: &mut Out) -> Result<()> {
    let home = home_here();
    let mut ready: Vec<(String, PathBuf, usize)> = Vec::new();
    let tasks = load_all(&home);
    for a in asset::scan(&home) {
        let st = status::read(&a.file);
        let rt = tasks.iter().filter(|t| t.asset == a.name && t.status == "open" && t.kind == "review").count();
        if st.state == "done" || rt > 0 {
            ready.push((a.name, a.file, rt));
        }
    }
    let mut text = String::new();
    let mut js = Vec::new();
    for (n, f, rt) in &ready {
        let d = asset::dir_of(f);
        text.push_str(&format!("{:<26} {} review task(s)  compare {}\n", n, rt, d.join("compare.png").slash()));
        js.push(json!({"asset": n, "review_tasks": rt, "compare": d.join("compare.png").slash()}));
    }
    if ready.is_empty() {
        text.push_str("nothing to review\n");
    }
    text.push_str(&format!("{} asset(s) waiting", ready.len()));
    out.emit(text, json!({"queue": js}));
    Ok(())
}

fn review_note(a: &ReviewArgs, out: &mut Out) -> Result<()> {
    let by = reviewer(out, "note")?;
    // pxl review note ASSET FRAME ["text"] --region x,y,w,h
    let asset_arg = a.parts.get(1).ok_or_else(|| PxlError::usage("pxl review note ASSET FRAME \"text\" --region x,y,w,h"))?;
    let frame_arg = a.parts.get(2).ok_or_else(|| PxlError::usage("pxl review note ASSET FRAME \"text\" --region x,y,w,h"))?;
    let text = a.parts.get(3).cloned().or_else(|| a.note.clone()).ok_or_else(|| PxlError::usage("the note needs its text"))?;
    if needs_region(&text) && a.region.is_none() {
        return Err(PxlError::usage("a note that names a colour or a direction must carry --region x,y,w,h").with_hint("so anyone can check the claim from the crop image; the region is in frame pixels (pxl show prints rulers)"));
    }
    let file = fsutil::resolve_part(asset_arg)?;
    let part = pxl::load(&file)?;
    let frame = part.frame(frame_arg).ok_or_else(|| crate::cli::unknown_frame(&part, frame_arg))?;
    let dir = asset::dir_of(&file);
    if let Some(rule) = &a.rule {
        let rs = crate::style::rules(&home_of(&file));
        if rs.is_empty() {
            return Err(PxlError::usage("--rule needs a style.md with rules").with_hint("pxl style propose"));
        }
        if !rs.iter().any(|r| r.0 == *rule) {
            return Err(PxlError::usage(format!("style.md has no rule '{}'", rule)).with_hint(format!("rules: {}", rs.iter().map(|r| r.0.as_str()).collect::<Vec<_>>().join(", "))));
        }
    }
    let rdir = dir.join("review");
    std::fs::create_dir_all(&rdir)?;
    let n = std::fs::read_dir(&rdir).map(|r| r.flatten().filter(|e| e.file_name().to_string_lossy().starts_with("note_") && e.file_name().to_string_lossy().ends_with(".md")).count()).unwrap_or(0) + 1;
    let mut crop_name = None;
    if let Some(r) = a.region {
        if r.w == 0 || r.h == 0 {
            return Err(PxlError::usage("--region needs a width and a height"));
        }
        let img = crop_image(&part, &dir, frame, r, a.zoom)?;
        let cn = format!("note_{:03}.png", n);
        fsutil::atomic_write_plain(&rdir.join(&cn), &compare::encode_sheet_png(&img))?;
        crop_name = Some(cn);
    }
    let md = format!(
        "# Note {} on {} / {}\n\nby {} on {}\nregion {}\nrule {}\n\n{}\n{}\n",
        n,
        asset::asset_name(&dir),
        frame.name,
        by,
        date_of(now()),
        a.region.map_or("-".into(), |r| format!("{},{},{},{}", r.x, r.y, r.w, r.h)),
        a.rule.as_deref().unwrap_or("-"),
        text,
        crop_name.as_ref().map_or(String::new(), |c| format!("\n![crop](./{})\n", c))
    );
    fsutil::atomic_write_plain(&rdir.join(format!("note_{:03}.md", n)), md.as_bytes())?;
    let home = home_of(&file);
    let owner = status::read(&file).owner;
    let body = format!("{}{} / {}{}: {}", a.rule.as_ref().map_or(String::new(), |r| format!("[rule {}] ", r)), frame.name, a.region.map_or(String::new(), |r| format!("[{},{},{},{}] ", r.x, r.y, r.w, r.h)), by, text);
    let id = add_task_src(&home, &asset::asset_name(&dir), &body, "fix", owner, 1, &by, None, "review-note")?;
    if status::read(&file).state == "done" {
        let _ = status::update(&file, |s| s.state = "rework".into());
    }
    write_page(&dir);
    out.emit(
        format!("note {} recorded ({}), fix task {} created{}", n, rdir.join(format!("note_{:03}.md", n)).slash(), id, crop_name.map_or(String::new(), |c| format!(", crop {}", rdir.join(c).slash()))),
        json!({"note": n, "task": id, "asset": asset::asset_name(&dir), "frame": frame.name, "by": by}),
    );
    Ok(())
}

/// source | ours of a region, at zoom.
fn crop_image(part: &crate::model::Part, dir: &Path, frame: &crate::model::Frame, r: Region, zoom: Option<usize>) -> Result<RgbaImage> {
    let z = zoom.unwrap_or_else(|| (96 / r.w.max(r.h)).clamp(2, 24));
    let tpl = template::load_for(part).ok();
    let mut ours = compare::checker(r.w * z, r.h * z, 4);
    for y in 0..r.h {
        for x in 0..r.w {
            let (gx, gy) = (r.x + x as i32, r.y + y as i32);
            if gx >= 0 && gy >= 0 && (gx as usize) < frame.grid.w && (gy as usize) < frame.grid.h {
                let k = frame.grid.cells[gy as usize * frame.grid.w + gx as usize];
                if k != b'.' {
                    let c = compare::key_color(part, tpl.as_ref(), k);
                    compare::fill(&mut ours, x * z, y * z, z, z, Rgba::opaque(c.r, c.g, c.b));
                }
            }
        }
    }
    let mut src = compare::checker(r.w * z, r.h * z, 4);
    if let Ok(rf) = load::reference(part, dir, frame) {
        let f = part.input.factor.or(part.reduce.factor).unwrap_or(1).max(1) as i32;
        for y in 0..(r.h * z) {
            for x in 0..(r.w * z) {
                let (gx, gy) = (r.x + (x / z) as i32, r.y + (y / z) as i32);
                // reference pixel under the centre of the frame pixel's block
                let sub = ((x % z) as i32 * f) / z as i32;
                let subv = ((y % z) as i32 * f) / z as i32;
                let p = rf.img.get(rf.origin.0 + (gx - part.origin.0) * f + sub, rf.origin.1 + (gy - part.origin.1) * f + subv);
                if p.a > 0 {
                    src.px[y * src.w + x] = Rgba::opaque(p.r, p.g, p.b);
                }
            }
        }
    }
    let gap = 6;
    let mut cv = RgbaImage::filled(src.w + gap + ours.w, src.h, Rgba::opaque(0x30, 0x30, 0x34));
    compare::paste(&mut cv, &src, 0, 0);
    compare::paste(&mut cv, &ours, src.w + gap, 0);
    Ok(cv)
}

fn review_approve(a: &ReviewArgs, out: &mut Out) -> Result<()> {
    let by = reviewer(out, "approve")?;
    let asset_arg = a.parts.get(1).ok_or_else(|| PxlError::usage("pxl review approve ASSET --agent NAME"))?;
    let file = fsutil::resolve_part(asset_arg)?;
    let user = by == "user";
    let note = a.note.clone().or_else(|| a.parts.get(2).cloned()).unwrap_or_default();
    let note = format!("{}{}", note, crate::design::approval_gate(&file, a.justify.as_deref())?);
    status::update(&file, |s| {
        s.state = "approved".into();
        s.owner = None;
        if user {
            s.approved_by = Some("user".into());
        }
        s.review.push(status::Note { by: by.clone(), verdict: "approved".into(), text: note.clone(), ..Default::default() });
    })?;
    // approving closes the open review tasks
    let home = home_of(&file);
    let name = asset::asset_name(&asset::dir_of(&file));
    let mut all = load_all(&home);
    let mut closed = 0;
    for t in all.iter_mut().filter(|t| t.asset == name && t.status == "open" && (t.kind == "review" || t.src == "review-note")) {
        t.status = "done".into();
        t.note = if t.kind == "review" { format!("approved by {}", by) } else { format!("settled by the approval of {}", by) };
        closed += 1;
    }
    if closed > 0 {
        save_all(&home, &all)?;
    }
    write_page(&asset::dir_of(&file));
    out.emit(format!("{} approved by {}{}", name, by, if user { " (user: changes need --force-user-approved)" } else { "" }), json!({"asset": name, "state": "approved", "by": by, "approved_by": if user { Some("user") } else { None }}));
    Ok(())
}

fn review_rework(a: &ReviewArgs, out: &mut Out) -> Result<()> {
    let _by = reviewer(out, "rework")?;
    let asset_arg = a.parts.get(1).ok_or_else(|| PxlError::usage("pxl review rework ASSET --note \"...\" --agent NAME"))?;
    let note = a.note.clone().or_else(|| a.parts.get(2).cloned()).ok_or_else(|| PxlError::usage("rework needs --note"))?;
    if needs_region(&note) && a.region.is_none() {
        return Err(PxlError::usage("a rework note that names a colour or a direction must carry --region x,y,w,h").with_hint("use `pxl review note ASSET FRAME \"text\" --region x,y,w,h` first, then rework"));
    }
    crate::review::run_rework(&ReworkArgs { part: asset_arg.clone(), frames: Vec::new(), note }, out)
}

/// Hook of `pxl rework`: a fix task for the owner. One issue is one task: `review note` already opened a fix task per note, so a rework
/// of an asset that still has open fix tasks (or whose open task already says the same) adds none; it returns the ids that now carry the work.
pub fn rework_task(file: &Path, note: &str, by: &str) -> (Vec<String>, bool) {
    if !asset::is_asset_file(file) {
        return (Vec::new(), false);
    }
    let home = home_of(file);
    let name = asset::asset_name(&asset::dir_of(file));
    // the fix tasks of this round's review notes carry the work: the rework of the same issue adds no second task
    let open: Vec<String> = load_all(&home).iter().filter(|t| t.asset == name && t.status == "open" && t.kind == "fix" && t.src == "review-note").map(|t| t.id.clone()).collect();
    if !open.is_empty() {
        return (open, false);
    }
    let owner = None::<String>;
    let ids = add_task(&home, &name, note, "fix", owner, 1, by).map(|i| vec![i]).unwrap_or_default();
    write_page(&asset::dir_of(file));
    (ids, true)
}

/// A change to an asset the user approved personally: a review task.
pub fn user_approved_changed(file: &Path, version: u32) {
    let st = status::read(file);
    if st.approved_by.as_deref() != Some("user") {
        return;
    }
    let home = home_of(file);
    let name = asset::asset_name(&asset::dir_of(file));
    if load_all(&home).iter().any(|t| t.asset == name && t.status == "open" && t.kind == "review" && t.text.starts_with("changed after user approval")) {
        return;
    }
    let _ = add_task(&home, &name, &format!("changed after user approval (v{:03}): the user must look again", version), "review", None, 5, "pxl");
}

#[allow(dead_code)]
fn unused(_: &Out) -> Option<&'static str> {
    let _ = (console::DEFAULT_CONSOLE, project::state_of, lease::guard, png::read_rgba);
    None
}
