//! `pxl report [--html]` (M6 R7): the progress digest for the user. Done, in review, new images since the last report, pending decisions with
//! their recommendations, budget status, the milestones and the assumptions still unconfirmed. The brief's `cadence` says when one is due
//! (`--if-due`): as work lands (new versions since the last report), per batch (three or more), daily, or weekly.

use crate::fsutil::PathSlash;
use crate::cli::*;
use crate::manifest::Manifest;
use crate::output::{Out, PxlError, Result};
use crate::{asset, decisions, fsutil, intake, milestone, status, style, tasks, versions};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

fn state_file(home: &Path) -> PathBuf {
    home.join("review").join(".report.json")
}

fn last_report(home: &Path) -> u64 {
    std::fs::read_to_string(state_file(home)).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|v| v["ts"].as_u64()).unwrap_or(0)
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

struct Item {
    name: String,
    state: String,
    version: String,
    compare: PathBuf,
    approved_by: Option<String>,
    new: bool,
    open_tasks: usize,
}

pub fn run_report(a: &ReportArgs, out: &mut Out) -> Result<()> {
    let cwd = std::env::current_dir().map_err(|e| PxlError::io(e.to_string()))?;
    let mp = Manifest::find_file(&cwd).ok_or_else(|| PxlError::usage("no project here (pxl.toml not found)").with_hint("pxl new DIR --console gbc"))?;
    let home = asset::dir_of(&mp);
    let last = last_report(&home);
    let now = tasks::now();
    let cadence = intake::cadence_of(&home);
    let all_tasks = tasks::load_all(&home);
    let items: Vec<Item> = asset::scan(&home)
        .into_iter()
        .map(|ai| {
            let st = status::read(&ai.file);
            let v = versions::latest(&ai.file);
            Item {
                new: v.as_ref().map_or(false, |v| v.ts > last),
                version: v.map_or("-".into(), |v| format!("v{:03}", v.n)),
                compare: ai.dir.join("compare.png"),
                approved_by: st.approved_by.clone(),
                state: st.state,
                open_tasks: all_tasks.iter().filter(|t| t.asset == ai.name && t.status == "open").count(),
                name: ai.name,
            }
        })
        .collect();
    let new_count = items.iter().filter(|i| i.new).count();
    if a.if_due {
        let due = match cadence.as_str() {
            "daily" => now.saturating_sub(last) >= 86_400 && new_count > 0,
            "weekly" => now.saturating_sub(last) >= 604_800 && new_count > 0,
            "per_batch" => new_count >= 3,
            _ => new_count > 0,
        };
        if !due {
            out.emit(format!("no report due (cadence {}; {} new image(s) since the last report)", cadence, new_count), json!({"due": false, "cadence": cadence, "new": new_count}));
            return Ok(());
        }
    }
    let approved: Vec<&Item> = items.iter().filter(|i| i.state == "approved").collect();
    let review: Vec<&Item> = items.iter().filter(|i| i.state == "done").collect();
    let working: Vec<&Item> = items.iter().filter(|i| matches!(i.state.as_str(), "in_progress" | "rework" | "todo" | "blocked")).collect();
    let decs: Vec<decisions::Decision> = decisions::load(&home).into_iter().filter(|d| d.open()).collect();
    let ms = milestone::status(&home);
    // budgets
    let mut budget: Vec<String> = Vec::new();
    let mut budget_js: Vec<Value> = Vec::new();
    if let Ok(m) = Manifest::load(&mp) {
        if m.scenes.is_empty() {
            budget.push("no scenes yet (pxl plan --apply proposes them)".into());
        } else if let Ok((sp, tp)) = crate::scene::hardware(&m) {
            let mut cache = std::collections::HashMap::new();
            for sc in &m.scenes {
                let rep = crate::scene::check_scene(&m, sc, &sp, &tp, &mut cache);
                let bad: Vec<String> = rep.rows.iter().filter(|r| !r.ok).map(|r| format!("{} {} / {}", r.item, r.used, r.limit)).collect();
                budget.push(format!("{}: {}", sc.name, if bad.is_empty() { "all rows within the limits".to_string() } else { format!("OVER on {}", bad.join("; ")) }));
                budget_js.push(json!({"scene": sc.name, "ok": bad.is_empty(), "over": bad}));
            }
        }
    }
    let assumptions: Vec<String> = std::fs::read_to_string(home.join("decisions.md")).unwrap_or_default().lines().filter(|l| l.contains("[operator]") && l.contains("(assumption ")).map(|l| l.trim_start_matches("- ").to_string()).collect();
    let brief = intake::read_brief(&home);
    let unconfirmed: Vec<String> = brief.get("assumptions").and_then(|t| t.as_table()).map(|t| t.iter().filter(|(_, v)| v.as_str() == Some("assumed")).map(|(k, _)| k.clone()).collect()).unwrap_or_default();

    let title = format!("Progress report {}", tasks::date_of(now));
    let mut text = format!("{} (cadence {}; last report {})\n", title, cadence, if last == 0 { "never".to_string() } else { tasks::date_of(last) });
    text.push_str(&format!("\nMILESTONES\n{}\n", ms.text()));
    text.push_str(&format!("\nAPPROVED ({})\n", approved.len()));
    for i in &approved {
        text.push_str(&format!("  {} {}{}\n", i.name, i.version, i.approved_by.as_ref().map_or(String::new(), |b| format!(" (approved by {})", b))));
    }
    text.push_str(&format!("\nIN REVIEW ({})\n", review.len()));
    for i in &review {
        text.push_str(&format!("  {} {}  {}\n", i.name, i.version, i.compare.slash()));
    }
    text.push_str(&format!("\nIN WORK ({})\n", working.len()));
    for i in &working {
        text.push_str(&format!("  {} [{}] {}{}\n", i.name, i.state, i.version, if i.open_tasks > 0 { format!(", {} open task(s)", i.open_tasks) } else { String::new() }));
    }
    text.push_str(&format!("\nNEW IMAGES SINCE THE LAST REPORT ({})\n", new_count));
    for i in items.iter().filter(|i| i.new) {
        text.push_str(&format!("  {} {}  {}\n", i.name, i.version, i.compare.slash()));
    }
    text.push_str(&format!("\nPENDING DECISIONS ({})\n", decs.len()));
    for d in &decs {
        text.push_str(&format!("  {} {}: options {}; recommended {} because {}\n    page {}\n", d.id, d.topic, d.options.iter().map(|o| o.key.as_str()).collect::<Vec<_>>().join("/"), d.recommend, d.why, decisions::page_of(&home, &d.id).slash()));
    }
    text.push_str("\nBUDGET\n");
    for b in &budget {
        text.push_str(&format!("  {}\n", b));
    }
    if !unconfirmed.is_empty() {
        text.push_str(&format!("\nASSUMPTIONS NOT CONFIRMED YET ({}): {} (change one: pxl decide \"why\" --set key=value)\n", unconfirmed.len(), unconfirmed.join(", ")));
    }
    // html
    let mut html_path = None;
    if let Some(h) = &a.html {
        let path = if h == "review/report.html" { home.join(h) } else { PathBuf::from(h) };
        let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."));
        std::fs::create_dir_all(&dir)?;
        let img = |p: &Path| if p.is_file() { format!("<img src=\"{}\" alt=\"compare\">", esc(&style::rel_path(&dir, p))) } else { String::new() };
        let mut html = format!("<!doctype html><meta charset=\"utf-8\"><title>{0}</title><style>body{{font:14px sans-serif;margin:16px;max-width:1000px}}img{{image-rendering:pixelated;max-width:100%}}pre{{background:#f4f4f4;padding:8px}}.card{{border:1px solid #bbb;padding:8px;margin:8px 0}}</style><h1>{0}</h1>", esc(&title));
        html.push_str(&format!("<h2>Milestones</h2><pre>{}</pre>", esc(&ms.text())));
        html.push_str(&format!("<h2>Pending decisions ({})</h2>", decs.len()));
        for d in &decs {
            let rel = style::rel_path(&dir, &decisions::page_of(&home, &d.id));
            html.push_str(&format!("<div class=\"card\"><b>{} {}</b><br>options {}; <b>recommended {}</b> because {}. <a href=\"{}\">open the options page</a></div>", esc(&d.id), esc(&d.topic), esc(&d.options.iter().map(|o| o.key.as_str()).collect::<Vec<_>>().join("/")), esc(&d.recommend), esc(&d.why), esc(&rel)));
        }
        html.push_str(&format!("<h2>New images ({})</h2>", new_count));
        for i in items.iter().filter(|i| i.new) {
            html.push_str(&format!("<div class=\"card\"><b>{}</b> {} [{}]<br>{}</div>", esc(&i.name), i.version, esc(&i.state), img(&i.compare)));
        }
        html.push_str(&format!("<h2>In review ({})</h2><ul>", review.len()));
        for i in &review {
            html.push_str(&format!("<li>{} {}</li>", esc(&i.name), i.version));
        }
        html.push_str(&format!("</ul><h2>Approved ({})</h2><ul>", approved.len()));
        for i in &approved {
            html.push_str(&format!("<li>{} {}{}</li>", esc(&i.name), i.version, i.approved_by.as_ref().map_or(String::new(), |b| format!(" (approved by {})", esc(b)))));
        }
        html.push_str(&format!("</ul><h2>In work ({})</h2><ul>", working.len()));
        for i in &working {
            html.push_str(&format!("<li>{} [{}] {}</li>", esc(&i.name), esc(&i.state), i.version));
        }
        html.push_str("</ul><h2>Budget</h2><ul>");
        for b in &budget {
            html.push_str(&format!("<li>{}</li>", esc(b)));
        }
        html.push_str("</ul>");
        if !assumptions.is_empty() {
            html.push_str("<h2>Assumptions</h2><ul>");
            for l in &assumptions {
                html.push_str(&format!("<li>{}</li>", esc(l)));
            }
            html.push_str("</ul>");
        }
        fsutil::atomic_write_plain(&path, html.as_bytes())?;
        text.push_str(&format!("\nwrote {}\n", path.slash()));
        html_path = Some(path);
    }
    if !a.peek {
        std::fs::create_dir_all(home.join("review"))?;
        fsutil::atomic_write_plain(&state_file(&home), json!({"ts": now}).to_string().as_bytes())?;
    }
    out.emit(
        text.trim_end(),
        json!({
            "due": true, "cadence": cadence, "since": last,
            "milestones": ms.to_json(),
            "approved": approved.iter().map(|i| json!({"asset": i.name, "version": i.version, "approved_by": i.approved_by})).collect::<Vec<_>>(),
            "in_review": review.iter().map(|i| json!({"asset": i.name, "version": i.version, "compare": i.compare.slash()})).collect::<Vec<_>>(),
            "in_work": working.iter().map(|i| json!({"asset": i.name, "state": i.state})).collect::<Vec<_>>(),
            "new_images": items.iter().filter(|i| i.new).map(|i| json!({"asset": i.name, "version": i.version, "compare": i.compare.slash()})).collect::<Vec<_>>(),
            "pending_decisions": decs.iter().map(|d| json!({"id": d.id, "topic": d.topic, "recommend": d.recommend, "why": d.why, "options": d.options.iter().map(|o| o.key.clone()).collect::<Vec<_>>(), "page": decisions::page_of(&home, &d.id).slash()})).collect::<Vec<_>>(),
            "budget": budget_js, "assumptions": assumptions, "unconfirmed": unconfirmed,
            "html": html_path.map(|p| p.slash()),
        }),
    );
    Ok(())
}
