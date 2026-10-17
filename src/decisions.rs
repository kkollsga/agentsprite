//! Decisions with options (M6 R7): `pxl ask`, `pxl decide <id> <option>`, `pxl decisions`.
//!
//! An open decision lives in `<project>/decisions.toml` (`[[decision]]` with stable ids `d0001`, never reused): a topic, two or more options (a key, an
//! optional image and a label each), the operator's recommendation with the reason, and later the choice. Every decision has an image page
//! `review/decisions/<id>.html` (options side by side, the recommendation marked). A task added with `--after d0001` stays blocked until the
//! decision is decided. A closed decision is also written to `decisions.md` as one dated line, like every other decision.

use crate::fsutil::PathSlash;
use crate::asset;
use crate::cli::*;
use crate::output::{Out, PxlError, Result};
use crate::{fsutil, tasks};
use serde_json::json;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default)]
pub struct OptionDef {
    pub key: String,
    /// Path of the image relative to `review/decisions/`.
    pub image: String,
    pub label: String,
}

#[derive(Clone, Debug, Default)]
pub struct Decision {
    pub id: String,
    pub topic: String,
    pub asset: String,
    pub status: String,
    pub options: Vec<OptionDef>,
    pub recommend: String,
    pub why: String,
    pub created: u64,
    pub by: String,
    pub chosen: String,
    pub decided_by: String,
    pub decided_on: u64,
}

impl Decision {
    pub fn open(&self) -> bool {
        self.status != "decided"
    }
}

fn file(home: &Path) -> PathBuf {
    home.join("decisions.toml")
}

pub fn dir(home: &Path) -> PathBuf {
    home.join("review").join("decisions")
}

pub fn page_of(home: &Path, id: &str) -> PathBuf {
    dir(home).join(format!("{}.html", id))
}

pub fn load(home: &Path) -> Vec<Decision> {
    let Ok(text) = std::fs::read_to_string(file(home)) else { return Vec::new() };
    let Ok(t) = text.parse::<toml::Table>() else { return Vec::new() };
    let mut v = Vec::new();
    for x in t.get("decision").and_then(|x| x.as_array()).cloned().unwrap_or_default() {
        let Some(x) = x.as_table() else { continue };
        let s = |k: &str| x.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let i = |k: &str| x.get(k).and_then(|v| v.as_integer()).unwrap_or(0) as u64;
        let options = x
            .get("option")
            .and_then(|o| o.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|o| o.as_table())
                    .map(|o| {
                        let g = |k: &str| o.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
                        OptionDef { key: g("key"), image: g("image"), label: g("label") }
                    })
                    .collect()
            })
            .unwrap_or_default();
        v.push(Decision {
            id: s("id"),
            topic: s("topic"),
            asset: s("asset"),
            status: if s("status").is_empty() { "open".into() } else { s("status") },
            options,
            recommend: s("recommend"),
            why: s("why"),
            created: i("created"),
            by: s("by"),
            chosen: s("chosen"),
            decided_by: s("decided_by"),
            decided_on: i("decided_on"),
        });
    }
    v
}

fn save(home: &Path, all: &[Decision]) -> Result<()> {
    let mut s = String::from("# open and closed decisions with options. Edit with `pxl ask` and `pxl decide ID OPTION`, not by hand.\n");
    for d in all {
        s.push_str(&format!("\n[[decision]]\nid = {}\ntopic = {}\nstatus = {}\nrecommend = {}\nwhy = {}\ncreated = {}\nby = {}\n", asset::q(&d.id), asset::q(&d.topic), asset::q(&d.status), asset::q(&d.recommend), asset::q(&d.why), d.created, asset::q(&d.by)));
        if !d.asset.is_empty() {
            s.push_str(&format!("asset = {}\n", asset::q(&d.asset)));
        }
        if !d.chosen.is_empty() {
            s.push_str(&format!("chosen = {}\ndecided_by = {}\ndecided_on = {}\n", asset::q(&d.chosen), asset::q(&d.decided_by), d.decided_on));
        }
        for o in &d.options {
            s.push_str(&format!("\n[[decision.option]]\nkey = {}\nimage = {}\nlabel = {}\n", asset::q(&o.key), asset::q(&o.image), asset::q(&o.label)));
        }
    }
    fsutil::atomic_write_plain(&file(home), s.as_bytes())
}

pub fn get(home: &Path, id: &str) -> Option<Decision> {
    load(home).into_iter().find(|d| d.id == id)
}

pub fn is_id(s: &str) -> bool {
    s.len() >= 5 && s.starts_with('d') && s[1..].bytes().all(|b| b.is_ascii_digit())
}

/// True while the decision exists and is not decided.
pub fn is_open(home: &Path, id: &str) -> bool {
    get(home, id).map_or(false, |d| d.open())
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The image page of a decision.
pub fn write_page(home: &Path, d: &Decision) {
    let mut h = format!(
        "<!doctype html><meta charset=\"utf-8\"><title>{id} {t}</title><style>body{{font:14px sans-serif;margin:16px;max-width:1100px}}.opts{{display:flex;gap:16px;flex-wrap:wrap}}.o{{border:2px solid #888;padding:8px;max-width:48%}}.o.rec{{border-color:#2a7}}.o.chosen{{background:#e8f6ee}}img{{image-rendering:pixelated;max-width:100%}}</style><h1>{id}: {t}</h1>",
        id = esc(&d.id),
        t = esc(&d.topic)
    );
    if d.open() {
        h.push_str(&format!("<p><b>Open.</b> Decide with <code>pxl decide {} &lt;option&gt;</code>. Recommended: <b>{}</b>, because {}.</p>", esc(&d.id), esc(&d.recommend), esc(&d.why)));
    } else {
        h.push_str(&format!("<p><b>Decided: {}</b> by {} on {}. (Recommended: {}, because {}.)</p>", esc(&d.chosen), esc(&d.decided_by), tasks::date_of(d.decided_on), esc(&d.recommend), esc(&d.why)));
    }
    h.push_str("<div class=\"opts\">");
    for o in &d.options {
        let class = format!("o{}{}", if o.key == d.recommend { " rec" } else { "" }, if o.key == d.chosen { " chosen" } else { "" });
        h.push_str(&format!("<div class=\"{}\"><h2>{}{}</h2>", class, esc(&o.key), if o.key == d.recommend { " (recommended)" } else { "" }));
        if !o.image.is_empty() {
            h.push_str(&format!("<p><img src=\"{}\" alt=\"option {}\"></p>", esc(&o.image), esc(&o.key)));
        }
        if !o.label.is_empty() {
            h.push_str(&format!("<p>{}</p>", esc(&o.label)));
        }
        h.push_str("</div>");
    }
    h.push_str("</div>");
    let _ = std::fs::create_dir_all(dir(home));
    let _ = fsutil::atomic_write_plain(&page_of(home, &d.id), h.as_bytes());
}

/// Create an open decision. `options` = `(key, image path or empty, label)`; image files are copied next to the page.
pub fn ask(home: &Path, topic: &str, asset_name: Option<&str>, options: &[(String, String, String)], recommend: &str, why: &str, by: &str) -> Result<Decision> {
    if options.len() < 2 {
        return Err(PxlError::usage("a decision needs at least two options: --option A=img --option B=img").with_hint("never ask an open question: give options, an image each and a recommendation"));
    }
    let mut seen: Vec<&str> = Vec::new();
    for o in options {
        if o.0.is_empty() || seen.contains(&o.0.as_str()) {
            return Err(PxlError::usage(format!("option key '{}' is empty or used twice", o.0)));
        }
        seen.push(&o.0);
    }
    if !options.iter().any(|o| o.0 == recommend) {
        return Err(PxlError::usage(format!("--recommend {} is not one of the options ({})", recommend, options.iter().map(|o| o.0.as_str()).collect::<Vec<_>>().join(", "))));
    }
    let mut all = load(home);
    let n = all.iter().filter_map(|d| d.id.strip_prefix('d').and_then(|n| n.parse::<u32>().ok())).max().unwrap_or(0) + 1;
    let id = format!("d{:04}", n);
    let mut defs = Vec::new();
    for (key, img, label) in options {
        let mut image = String::new();
        if !img.is_empty() {
            let src = PathBuf::from(img);
            if !src.is_file() {
                return Err(PxlError::usage(format!("option {}: image {} not found", key, img)));
            }
            let ext = src.extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_else(|| "png".into());
            let rel = format!("{}/{}.{}", id, key, ext);
            let dst = dir(home).join(&rel);
            std::fs::create_dir_all(dst.parent().unwrap())?;
            std::fs::copy(&src, &dst)?;
            image = rel;
        }
        defs.push(OptionDef { key: key.clone(), image, label: label.clone() });
    }
    let d = Decision { id, topic: topic.to_string(), asset: asset_name.unwrap_or("").to_string(), status: "open".into(), options: defs, recommend: recommend.to_string(), why: why.to_string(), created: tasks::now(), by: by.to_string(), ..Default::default() };
    all.push(d.clone());
    save(home, &all)?;
    write_page(home, &d);
    Ok(d)
}

/// Close a decision. Returns it.
pub fn decide(home: &Path, id: &str, option: &str, by: &str) -> Result<Decision> {
    let mut all = load(home);
    let d = all.iter_mut().find(|d| d.id == id).ok_or_else(|| PxlError::usage(format!("no decision {}", id)).with_hint("pxl decisions lists them"))?;
    if !d.open() {
        return Err(PxlError::usage(format!("{} is already decided: {}", id, d.chosen)));
    }
    if !d.options.iter().any(|o| o.key == option) {
        return Err(PxlError::usage(format!("{} has no option '{}'", id, option)).with_hint(format!("options: {}", d.options.iter().map(|o| o.key.as_str()).collect::<Vec<_>>().join(", "))));
    }
    d.status = "decided".into();
    d.chosen = option.to_string();
    d.decided_by = by.to_string();
    d.decided_on = tasks::now();
    let d = d.clone();
    save(home, &all)?;
    write_page(home, &d);
    Ok(d)
}

pub fn run_ask(a: &AskArgs, out: &mut Out) -> Result<()> {
    let home = tasks::home_here();
    let mut opts = Vec::new();
    for o in &a.option {
        let (k, rest) = o.split_once('=').unwrap_or((o.as_str(), ""));
        // KEY=image|label, KEY=image, or KEY=label (when the text is not a file)
        let (img, label) = match rest.split_once('|') {
            Some((i, l)) => (i.to_string(), l.to_string()),
            None if !rest.is_empty() && Path::new(rest).is_file() => (rest.to_string(), String::new()),
            None => (String::new(), rest.to_string()),
        };
        opts.push((k.trim().to_string(), img, label));
    }
    let (rec, why) = match a.recommend.as_slice() {
        [r, w] => (r.clone(), w.clone()),
        [r] => (r.clone(), String::new()),
        _ => return Err(PxlError::usage("--recommend OPTION \"why\" is required: every question comes with a recommendation")),
    };
    if why.trim().is_empty() {
        return Err(PxlError::usage("--recommend OPTION \"why\": give the reason in one sentence"));
    }
    let by = out.agent().unwrap_or("agent").to_string();
    let asset_name = match &a.asset {
        Some(x) => Some(asset::asset_name(&asset::dir_of(&fsutil::resolve_part(x)?))),
        None => None,
    };
    let d = ask(&home, &a.topic, asset_name.as_deref(), &opts, &rec, &why, &by)?;
    out.emit(
        format!("{} opened: {} (recommend {}: {})\n  page {}\n  decide with: pxl decide {} {}", d.id, d.topic, d.recommend, d.why, page_of(&home, &d.id).slash(), d.id, d.recommend),
        json!({"id": d.id, "topic": d.topic, "recommend": d.recommend, "page": page_of(&home, &d.id).slash(), "options": d.options.iter().map(|o| json!({"key": o.key, "image": o.image, "label": o.label})).collect::<Vec<_>>()}),
    );
    Ok(())
}

pub fn run_decisions(a: &DecisionsArgs, out: &mut Out) -> Result<()> {
    let home = tasks::home_here();
    let all = load(&home);
    let sel: Vec<&Decision> = all.iter().filter(|d| a.all || d.open()).collect();
    let mut text = String::new();
    for d in &sel {
        text.push_str(&format!(
            "{} {:<8} {}  options {}  recommend {}: {}{}\n",
            d.id,
            d.status,
            d.topic,
            d.options.iter().map(|o| o.key.as_str()).collect::<Vec<_>>().join("/"),
            d.recommend,
            d.why,
            if d.open() { String::new() } else { format!("  -> {} ({})", d.chosen, d.decided_by) }
        ));
    }
    text.push_str(&format!("{} decision(s){}", sel.len(), if a.all { "" } else { " open" }));
    out.emit(
        text,
        json!({"decisions": sel.iter().map(|d| json!({"id": d.id, "topic": d.topic, "status": d.status, "recommend": d.recommend, "why": d.why, "chosen": d.chosen, "options": d.options.iter().map(|o| o.key.clone()).collect::<Vec<_>>(), "page": page_of(&home, &d.id).slash()})).collect::<Vec<_>>()}),
    );
    Ok(())
}

/// `pxl decide ID OPTION`: close a decision (called from `tasks::run_decide`).
pub fn run_decide_option(id: &str, option: &str, by: &str, out: &mut Out) -> Result<()> {
    let home = tasks::home_here();
    let d = decide(&home, id, option, by)?;
    let label = d.options.iter().find(|o| o.key == d.chosen).map(|o| o.label.clone()).unwrap_or_default();
    tasks::append_decision_line(&home, &format!("- {} [{}]{} decided {} ({}): option {}{}", tasks::date_of(tasks::now()), by, d.asset.is_empty().then(String::new).unwrap_or_else(|| format!(" (asset {})", d.asset)), d.id, d.topic, d.chosen, if label.is_empty() { String::new() } else { format!(" - {}", label) }))?;
    let unblocked: Vec<String> = tasks::load_all(&home).into_iter().filter(|t| t.status == "open" && t.after.as_deref() == Some(id)).map(|t| t.id).collect();
    out.emit(
        format!("{} decided: {} ({}){}", d.id, d.chosen, d.topic, if unblocked.is_empty() { String::new() } else { format!("; unblocked task(s): {}", unblocked.join(", ")) }),
        json!({"id": d.id, "chosen": d.chosen, "by": by, "unblocked": unblocked}),
    );
    Ok(())
}
