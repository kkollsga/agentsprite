//! `pxl intake` (M6 R7): the operator's intake, mostly inferred. It reads the request text and the project's inputs, writes `brief.toml`, records each
//! inferred item as an assumption (a decision `--by operator`, reversible with `pxl decide "..." --set key=value`), prints the assumptions, and opens
//! at most a few real questions (with options and a recommendation) when something is high-impact and cannot be inferred. `--interview` is the opt-in
//! questionnaire; `--answers FILE` applies answers from a TOML file.

use crate::fsutil::PathSlash;
use crate::cli::*;
use crate::insight::hex;
use crate::output::{Out, PxlError, Result};
use crate::{asset, console, decisions, fsutil, style, tasks};
use serde_json::json;
use std::path::{Path, PathBuf};

/// Operator defaults and questions are data: `skills/pxl-operator/references/defaults.toml` (embedded, shared with the skill).
fn data() -> &'static toml::Table {
    static D: std::sync::OnceLock<toml::Table> = std::sync::OnceLock::new();
    D.get_or_init(|| {
        crate::skill::skill_files("pxl-operator").into_iter().find(|f| f.0 == "references/defaults.toml").and_then(|f| f.1.parse::<toml::Table>().ok()).unwrap_or_default()
    })
}

fn default_list(key: &str, fallback: &[&str]) -> Vec<String> {
    data().get("defaults").and_then(|d| d.get(key)).and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).filter(|v: &Vec<String>| !v.is_empty()).unwrap_or_else(|| fallback.iter().map(|s| s.to_string()).collect())
}

/// Trade-off order, first given up first.
pub fn default_tradeoff() -> Vec<String> {
    default_list("tradeoff", &["cut animation steps", "fold colours", "narrow sizes", "convert to BG", "counter-propose to the client"])
}

pub fn fidelity_options() -> Vec<String> {
    default_list("fidelity_options", &["faithful", "reinterpreted"])
}

pub fn cadence_options() -> Vec<String> {
    default_list("cadence_options", &["as_work_lands", "per_batch", "daily", "weekly"])
}

/// A question of the data file by topic: `(recommend, why, options as (key, label))`, with `{name}` placeholders replaced from `vars`.
fn question(topic: &str, vars: &[(&str, &str)]) -> Option<(String, String, Vec<(String, String)>)> {
    let fill = |s: &str| vars.iter().fold(s.to_string(), |acc, (k, v)| acc.replace(&format!("{{{}}}", k), v));
    let q = data().get("question")?.as_array()?.iter().filter_map(|q| q.as_table()).find(|q| q.get("topic").and_then(|t| t.as_str()) == Some(topic))?;
    let rec = q.get("recommend")?.as_str()?.to_string();
    let why = fill(q.get("why")?.as_str()?);
    let opts = q.get("options")?.as_table()?.iter().filter_map(|(k, v)| v.as_str().map(|l| (k.clone(), fill(l)))).collect();
    Some((rec, why, opts))
}
/// Keys `pxl decide --set` and `--answers` may change.
pub const KEYS: [&str; 8] = ["console", "fidelity", "cadence", "tradeoff", "exceptions", "deadline", "reviewer_approves", "delivery"];

pub fn brief_path(home: &Path) -> PathBuf {
    home.join("brief.toml")
}

pub fn read_brief(home: &Path) -> toml::Table {
    std::fs::read_to_string(brief_path(home)).ok().and_then(|t| t.parse::<toml::Table>().ok()).unwrap_or_default()
}

fn write_brief(home: &Path, t: &toml::Table) -> Result<()> {
    let body = toml::to_string(t).map_err(|e| PxlError::io(e.to_string()))?;
    fsutil::atomic_write_plain(&brief_path(home), format!("# written by `pxl intake`; every item is an assumption until confirmed.\n# Change one with: pxl decide \"why\" --set key=value (console, fidelity, cadence, tradeoff, exceptions, deadline).\n{}", body).as_bytes())
}

pub fn value_of(b: &toml::Table, key: &str) -> String {
    b.get("brief").and_then(|t| t.get(key)).and_then(|v| v.as_str()).unwrap_or("").to_string()
}

pub fn list_of(b: &toml::Table, key: &str) -> Vec<String> {
    b.get("brief").and_then(|t| t.get(key)).and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
}

/// `(eyes, colours)` of an asset's identity list.
pub fn identity_of(b: &toml::Table, asset_name: &str) -> Option<(usize, Vec<String>)> {
    let t = b.get("identity")?.as_table()?.get(asset_name)?.as_table()?;
    let eyes = t.get("eyes").and_then(|v| v.as_integer()).unwrap_or(0).max(0) as usize;
    let cols = t.get("colours").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
    Some((eyes, cols))
}

pub fn cadence_of(home: &Path) -> String {
    let c = value_of(&read_brief(home), "cadence");
    if c.is_empty() {
        "as_work_lands".into()
    } else {
        c
    }
}

// ---------------------------------------------------------------------------------------------- inference from text

const ALIASES: [(&str, &str); 28] = [
    ("nintendo entertainment system", "nes"),
    ("game boy advance", "gba"),
    ("gameboy advance", "gba"),
    ("game boy color", "gbc"),
    ("gameboy color", "gbc"),
    ("game boy colour", "gbc"),
    ("super nintendo", "snes"),
    ("super famicom", "snes"),
    ("master system", "sms"),
    ("turbografx-16", "pce"),
    ("turbografx", "pce"),
    ("pc engine", "pce"),
    ("mega drive", "md"),
    ("megadrive", "md"),
    ("game gear", "gg"),
    ("super nes", "snes"),
    ("famicom", "nes"),
    ("genesis", "md"),
    ("gameboy", "gb"),
    ("game boy", "gb"),
    ("snes", "snes"),
    ("gbc", "gbc"),
    ("gba", "gba"),
    ("dmg", "gb"),
    ("nes", "nes"),
    ("sms", "sms"),
    ("pce", "pce"),
    ("gb", "gb"),
];

fn is_word(c: char) -> bool {
    c.is_alphanumeric()
}

/// Consoles named in a text, in order of appearance, longest names first (so "game boy color" is not also "game boy").
pub fn consoles_in(text: &str) -> Vec<String> {
    let mut t: Vec<char> = text.to_lowercase().chars().collect();
    let mut found: Vec<(usize, String)> = Vec::new();
    let mut aliases = ALIASES.to_vec();
    aliases.sort_by_key(|a| std::cmp::Reverse(a.0.len()));
    for (alias, id) in aliases {
        let a: Vec<char> = alias.chars().collect();
        let mut i = 0;
        while i + a.len() <= t.len() {
            if t[i..i + a.len()] == a[..] && (i == 0 || !is_word(t[i - 1])) && (i + a.len() == t.len() || !is_word(t[i + a.len()])) {
                found.push((i, id.to_string()));
                for c in t.iter_mut().skip(i).take(a.len()) {
                    *c = ' ';
                }
                i += a.len();
            } else {
                i += 1;
            }
        }
    }
    found.sort_by_key(|f| f.0);
    let mut ids: Vec<String> = Vec::new();
    for (_, id) in found {
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids
}

fn has_any(text: &str, words: &[&str]) -> bool {
    let t = text.to_lowercase();
    words.iter().any(|w| t.contains(w))
}

const FAITHFUL: [&str; 11] = ["faithful", "exact", "pixel-accurate", "pixel accurate", "accurate", "1:1", "same as the", "like the original", "match the source", "as close", "true to"];
const REINTERPRET: [&str; 8] = ["reinterpret", "redraw", "own style", "inspired", "stylize", "stylise", "reimagine", "re-imagine"];

fn find_date(text: &str) -> Option<String> {
    let b: Vec<char> = text.chars().collect();
    for i in 0..b.len().saturating_sub(9) {
        let w: String = b[i..i + 10].iter().collect();
        let ok = w.chars().enumerate().all(|(k, c)| if k == 4 || k == 7 { c == '-' } else { c.is_ascii_digit() });
        if ok {
            return Some(w);
        }
    }
    None
}

// ---------------------------------------------------------------------------------------------- decisions.md assumptions

fn decisions_md(home: &Path) -> PathBuf {
    home.join("decisions.md")
}

/// Write an assumption / confirmation line for `key`, replacing the earlier line of that key.
fn record(home: &Path, by: &str, key: &str, text: &str) -> Result<()> {
    let marker = format!("(assumption {})", key);
    let mut lines: Vec<String> = std::fs::read_to_string(decisions_md(home)).map(|t| t.lines().map(String::from).collect()).unwrap_or_else(|_| vec!["# Decisions".into(), "".into(), "One dated line each; `pxl decide` appends.".into(), "".into()]);
    lines.retain(|l| !l.contains(&marker));
    lines.push(format!("- {} [{}] {} {}", tasks::date_of(tasks::now()), by, marker, text));
    let mut s = lines.join("\n");
    s.push('\n');
    fsutil::atomic_write_plain(&decisions_md(home), s.as_bytes())
}

fn set_item(b: &mut toml::Table, key: &str, v: toml::Value, status: &str) {
    let br = b.entry("brief").or_insert_with(|| toml::Value::Table(toml::Table::new()));
    if let Some(t) = br.as_table_mut() {
        t.insert(key.into(), v);
    }
    let a = b.entry("assumptions").or_insert_with(|| toml::Value::Table(toml::Table::new()));
    if let Some(t) = a.as_table_mut() {
        t.insert(key.into(), status.to_string().into());
    }
}

fn status_of(b: &toml::Table, key: &str) -> String {
    b.get("assumptions").and_then(|a| a.get(key)).and_then(|v| v.as_str()).unwrap_or("").to_string()
}

fn text_value(v: &str) -> toml::Value {
    toml::Value::String(v.to_string())
}

fn list_value(v: &[String]) -> toml::Value {
    toml::Value::Array(v.iter().map(|s| s.clone().into()).collect())
}

fn check_value(key: &str, v: &str, home: &Path) -> Result<()> {
    match key {
        "console" if !console::exists(v, Some(home)) => Err(PxlError::usage(format!("unknown console '{}'", v)).with_hint("pxl console list")),
        "fidelity" if !fidelity_options().iter().any(|o| o == v) => Err(PxlError::usage(format!("fidelity is one of {}", fidelity_options().join(", ")))),
        "cadence" if !cadence_options().iter().any(|o| o == v) => Err(PxlError::usage(format!("cadence is one of {}", cadence_options().join(", ")))),
        "exceptions" if !["never", "ask"].contains(&v) => Err(PxlError::usage("exceptions is never or ask")),
        "deadline" if !v.is_empty() && find_date(v).is_none() => Err(PxlError::usage("deadline is a date, YYYY-MM-DD")),
        _ if !KEYS.contains(&key) => Err(PxlError::usage(format!("unknown brief item '{}'", key)).with_hint(format!("items: {}", KEYS.join(", ")))),
        _ => Ok(()),
    }
}

/// `pxl decide ... --set key=value`: change a brief item and mark it confirmed.
pub fn apply_set(home: &Path, kv: &str, by: &str) -> Result<()> {
    let (k, v) = kv.split_once('=').ok_or_else(|| PxlError::usage("--set takes key=value"))?;
    let (k, v) = (k.trim(), v.trim());
    if let Some(rest) = k.strip_prefix("identity.") {
        // identity.ASSET=#rrggbb,#rrggbb (the colours that must survive) | identity.ASSET.eyes=N
        return match rest.split_once('.') {
            Some((asset, "eyes")) => identity_edit(home, asset, "eyes", &[v.to_string()], by).map(|_| ()),
            Some(_) => Err(PxlError::usage("identity.ASSET=#rrggbb,#rrggbb or identity.ASSET.eyes=N")),
            None => {
                let cols: Vec<String> = v.split(|c: char| c == ',' || c.is_whitespace()).filter(|s| !s.is_empty()).map(String::from).collect();
                identity_edit(home, rest, "set", &cols, by).map(|_| ())
            }
        };
    }
    check_value(k, v, home)?;
    let mut b = read_brief(home);
    if k == "tradeoff" {
        let items: Vec<String> = v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        set_item(&mut b, k, list_value(&items), "confirmed");
    } else {
        set_item(&mut b, k, text_value(v), "confirmed");
    }
    write_brief(home, &b)?;
    record(home, by, k, &format!("{} set to {} (confirmed)", k, v))?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------- identity list edits

fn norm_hex(c: &str) -> Result<String> {
    let t = c.trim().to_lowercase();
    let t = if t.starts_with('#') { t } else { format!("#{}", t) };
    if t.len() == 7 && t[1..].bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(t)
    } else {
        Err(PxlError::usage(format!("'{}' is not a colour like #209898", c)))
    }
}

/// The identity list of an asset from brief.toml: `(eyes, colours, confirmed)`.
pub fn identity_entry(b: &toml::Table, asset_name: &str) -> Option<(usize, Vec<String>, bool)> {
    let t = b.get("identity")?.as_table()?.get(asset_name)?.as_table()?;
    let eyes = t.get("eyes").and_then(|v| v.as_integer()).unwrap_or(0).max(0) as usize;
    let cols = t.get("colours").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
    Some((eyes, cols, t.get("confirmed").and_then(|v| v.as_bool()).unwrap_or(false)))
}

/// `pxl style identity ASSET show | add #hex.. | remove #hex.. | set #hex.. | eyes N`, and `pxl decide --set identity.ASSET=#hex,#hex`.
/// The list is the one the gates (`identity_colour`) and style.md use; an edited list is confirmed (a new intake keeps it). Returns the colours.
pub fn identity_edit(home: &Path, asset_name: &str, op: &str, args: &[String], by: &str) -> Result<Vec<String>> {
    let known: Vec<String> = asset::scan(home).into_iter().map(|a| a.name).collect();
    if !known.iter().any(|n| n == asset_name) {
        return Err(PxlError::usage(format!("no asset '{}'", asset_name)).with_hint(format!("assets: {}", known.join(", "))));
    }
    let mut b = read_brief(home);
    let (mut eyes, mut cols, _) = identity_entry(&b, asset_name).unwrap_or((0, Vec::new(), false));
    let mut materials: Vec<String> = b.get("identity").and_then(|t| t.get(asset_name)).and_then(|t| t.get("materials")).and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
    let hexes = |args: &[String]| -> Result<Vec<String>> { args.iter().flat_map(|a| a.split(',').map(String::from).collect::<Vec<_>>()).filter(|s| !s.trim().is_empty()).map(|s| norm_hex(&s)).collect() };
    let text = match op {
        "show" => return Ok(cols),
        "add" => {
            let add = hexes(args)?;
            if add.is_empty() {
                return Err(PxlError::usage("pxl style identity ASSET add #rrggbb ..."));
            }
            for c in &add {
                if !cols.contains(c) {
                    cols.push(c.clone());
                }
            }
            format!("identity of {}: added {}", asset_name, add.join(", "))
        }
        "remove" => {
            let rm = hexes(args)?;
            let before = cols.len();
            cols.retain(|c| !rm.contains(c));
            if cols.len() == before {
                return Err(PxlError::usage(format!("{} is not on the identity list of {} ({})", rm.join(", "), asset_name, cols.join(", "))));
            }
            format!("identity of {}: removed {}", asset_name, rm.join(", "))
        }
        "set" => {
            cols = hexes(args)?;
            let mut seen = Vec::new();
            cols.retain(|c| if seen.contains(c) { false } else { seen.push(c.clone()); true });
            format!("identity of {} set to {}", asset_name, if cols.is_empty() { "(nothing)".to_string() } else { cols.join(", ") })
        }
        "eyes" => {
            let n: usize = args.first().and_then(|a| a.trim().parse().ok()).ok_or_else(|| PxlError::usage("pxl style identity ASSET eyes N"))?;
            eyes = n;
            format!("identity of {}: {} eye(s)", asset_name, n)
        }
        _ => return Err(PxlError::usage("pxl style identity ASSET show | add #rrggbb.. | remove #rrggbb.. | set #rrggbb.. | eyes N")),
    };
    materials.retain(|m| !m.is_empty());
    let mut t = toml::Table::new();
    t.insert("eyes".into(), (eyes as i64).into());
    t.insert("colours".into(), list_value(&cols));
    t.insert("materials".into(), list_value(&materials));
    t.insert("confirmed".into(), true.into());
    let idt = b.entry("identity").or_insert_with(|| toml::Value::Table(toml::Table::new()));
    if let Some(it) = idt.as_table_mut() {
        it.insert(asset_name.to_string(), toml::Value::Table(t));
    }
    write_brief(home, &b)?;
    record(home, by, &format!("identity.{}", asset_name), &format!("{} (confirmed)", text))?;
    // style.md lists the identity colours too: keep its list in step (a changed rule text clears the approval, as any edit of style.md does)
    style::refresh_identity_section(home);
    Ok(cols)
}

// ---------------------------------------------------------------------------------------------- the command

/// Set an inferred item unless the user confirmed it before; records the assumption.
fn put(home: &Path, b: &mut toml::Table, assumptions: &mut Vec<(String, String, String)>, key: &str, val: toml::Value, shown: String, why: String) -> Result<()> {
    if status_of(b, key) == "confirmed" {
        let cur = if key == "tradeoff" { list_of(b, key).join(", ") } else { value_of(b, key) };
        assumptions.push((key.to_string(), cur, "confirmed earlier; unchanged".to_string()));
        return Ok(());
    }
    set_item(b, key, val, "assumed");
    record(home, "operator", key, &format!("{} = {} ({})", key, shown, why))?;
    assumptions.push((key.to_string(), shown, why));
    Ok(())
}


struct Inferred {
    console: String,
    console_why: String,
    mentioned: Vec<String>,
    fidelity: String,
    fidelity_why: String,
    contradictory: bool,
    cadence: String,
    tradeoff: Vec<String>,
    exceptions: String,
    deadline: String,
}

fn infer(home: &Path, request: &str, project_console: &str) -> Inferred {
    let mentioned = consoles_in(request);
    let mut spec_console: Option<String> = None;
    let spec_path = crate::project::project_table(&home.join("pxl.toml")).get("delivery").and_then(|d| d.get("client_spec")).and_then(|v| v.as_str()).map(|s| home.join(s));
    for p in spec_path.into_iter().chain([home.join("inbox/spec.json"), home.join("spec.json")]) {
        if let Some(c) = std::fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).and_then(|j| j["console"].as_str().map(String::from)) {
            if console::exists(&c, Some(home)) {
                spec_console = Some(c);
                break;
            }
        }
    }
    let (console, console_why) = if let Some(c) = mentioned.first() {
        (c.clone(), "named in the request".to_string())
    } else if let Some(c) = spec_console {
        (c, "named in the client spec".to_string())
    } else {
        (project_console.to_string(), "the project setting (pxl new --console)".to_string())
    };
    let f = has_any(request, &FAITHFUL);
    let r = has_any(request, &REINTERPRET);
    let (fidelity, fidelity_why) = match (f, r) {
        (true, false) => ("faithful", "the request asks for fidelity to the source"),
        (false, true) => ("reinterpreted", "the request asks for a reinterpretation"),
        (true, true) => ("faithful", "the request asks for both; faithful is assumed until you choose"),
        _ => ("faithful", "not stated; faithful to the source is the default (it is what users of earlier projects wanted)"),
    };
    let low = request.to_lowercase();
    let cadence = if low.contains("daily") || low.contains("every day") {
        "daily"
    } else if low.contains("weekly") || low.contains("every week") {
        "weekly"
    } else if low.contains("per batch") || low.contains("each batch") || low.contains("in batches") {
        "per_batch"
    } else {
        "as_work_lands"
    };
    let mut tradeoff: Vec<String> = default_tradeoff();
    if has_any(request, &["keep all animation", "don't cut animation", "do not cut animation", "never cut animation", "keep the animation"]) {
        tradeoff.retain(|t| t != "cut animation steps");
        tradeoff.push("cut animation steps".into());
    }
    let exceptions = if has_any(request, &["ask for exceptions", "ask the client for an exception", "exceptions are ok", "exceptions allowed", "may ask for exceptions"]) { "ask" } else { "never" };
    Inferred { console, console_why, mentioned, fidelity: fidelity.into(), fidelity_why: fidelity_why.into(), contradictory: f && r, cadence: cadence.into(), tradeoff, exceptions: exceptions.into(), deadline: find_date(request).unwrap_or_default() }
}

fn read_request(a: &IntakeArgs, home: &Path) -> String {
    match &a.request {
        Some(r) if Path::new(r).is_file() => std::fs::read_to_string(r).unwrap_or_default(),
        Some(r) => r.clone(),
        None => ["request.md", "REQUEST.md", "brief.md", "request.txt"].iter().find_map(|f| std::fs::read_to_string(home.join(f)).ok()).unwrap_or_else(|| value_of(&read_brief(home), "request")),
    }
}

pub fn run_intake(a: &IntakeArgs, out: &mut Out) -> Result<()> {
    let cwd = std::env::current_dir().map_err(|e| PxlError::io(e.to_string()))?;
    let mp = crate::manifest::Manifest::find_file(&cwd).ok_or_else(|| PxlError::usage("no project here (pxl.toml not found)").with_hint("pxl new DIR --console gbc"))?;
    let home = asset::dir_of(&mp);
    let pc = console::project_console(&home);
    let request = read_request(a, &home);
    let inf = infer(&home, &request, &pc.id);
    let mut b = read_brief(&home);
    let kept = |b: &toml::Table, k: &str| status_of(b, k) == "confirmed";
    let mut assumptions: Vec<(String, String, String)> = Vec::new(); // (key, value, why)
    {
        let br = b.entry("brief").or_insert_with(|| toml::Value::Table(toml::Table::new()));
        if let Some(t) = br.as_table_mut() {
            t.insert("request".into(), text_value(request.trim()));
        }
    }
    put(&home, &mut b, &mut assumptions, "console", text_value(&inf.console), inf.console.clone(), inf.console_why.clone())?;
    // image modes of the inputs
    let assets = asset::scan(&home);
    let mut modes = (0usize, 0usize, 0usize);
    for ai in &assets {
        match crate::pxl::load(&ai.file).map(|p| p.kind) {
            Ok(crate::model::PartKind::Sprite) => modes.0 += 1,
            Ok(crate::model::PartKind::Tileset) => modes.1 += 1,
            Ok(crate::model::PartKind::Screen) => modes.2 += 1,
            Err(_) => {}
        }
    }
    {
        let mut mt = toml::Table::new();
        mt.insert("sprite".into(), (modes.0 as i64).into());
        mt.insert("tile".into(), (modes.1 as i64).into());
        mt.insert("screen".into(), (modes.2 as i64).into());
        b.insert("modes".into(), toml::Value::Table(mt));
    }
    let modes_text = format!("{} sprite, {} tile, {} screen asset(s)", modes.0, modes.1, modes.2);
    record(&home, "operator", "modes", &format!("image modes = {} (from the ingested inputs)", modes_text))?;
    assumptions.push(("modes".into(), modes_text, "counted from the ingested inputs".into()));
    put(&home, &mut b, &mut assumptions, "fidelity", text_value(&inf.fidelity), inf.fidelity.clone(), inf.fidelity_why.clone())?;
    put(&home, &mut b, &mut assumptions, "cadence", text_value(&inf.cadence), inf.cadence.clone(), if inf.cadence == "as_work_lands" { "default: images are sent as work lands, without waiting for feedback".into() } else { "from the request".into() })?;
    put(
        &home,
        &mut b,
        &mut assumptions,
        "tradeoff",
        list_value(&inf.tradeoff),
        inf.tradeoff.join(" > "),
        if inf.tradeoff[0] == default_tradeoff()[0] { "default: strict hardware limits, the least visible cuts first".into() } else { "from the request".into() },
    )?;
    put(&home, &mut b, &mut assumptions, "exceptions", text_value(&inf.exceptions), inf.exceptions.clone(), if inf.exceptions == "never" { "default: no client exceptions, fix within the limits".into() } else { "the request allows asking".into() })?;
    if !inf.deadline.is_empty() {
        put(&home, &mut b, &mut assumptions, "deadline", text_value(&inf.deadline), inf.deadline.clone(), "found in the request".into())?;
    }
    put(&home, &mut b, &mut assumptions, "reviewer_approves", text_value("assets"), "assets (the user approves the plan, the design language and decisions)".into(), "default split of approvals".into())?;
    put(&home, &mut b, &mut assumptions, "delivery", text_value("package/ with manifest, DELIVERY.md"), "package/ with manifest, DELIVERY.md".into(), "default: `pxl deliver` snapshots to deliveries/N/".into())?;
    // identity list per asset: weighted by area, saturation and distinctness per material (insight::identity_list); a list somebody corrected
    // (`pxl style identity` / `pxl decide --set identity.ASSET=...`) is confirmed and never overwritten by a new intake
    let ms = style::measure_all(&home, &[])?;
    let old_identity = b.get("identity").and_then(|t| t.as_table()).cloned().unwrap_or_default();
    let mut idt = toml::Table::new();
    let mut id_lines = Vec::new();
    for m in &ms {
        if let Some(t) = old_identity.get(&m.name).and_then(|v| v.as_table()).filter(|t| t.get("confirmed").and_then(|v| v.as_bool()) == Some(true)) {
            let cols: Vec<String> = t.get("colours").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
            let eyes = t.get("eyes").and_then(|v| v.as_integer()).unwrap_or(0);
            id_lines.push(format!("{}: {} eye(s); colours {} (confirmed, kept)", m.name, eyes, cols.join(", ")));
            idt.insert(m.name.clone(), toml::Value::Table(t.clone()));
            continue;
        }
        let mut t = toml::Table::new();
        t.insert("eyes".into(), (m.eyes.max_per_frame as i64).into());
        let cols: Vec<String> = m.identity.iter().take(8).map(|(c, _)| hex(*c)).collect();
        t.insert("colours".into(), list_value(&cols));
        let mats: Vec<String> = m.families.iter().filter(|f| f.share >= 0.1).map(|f| format!("{} x{}", f.name, f.tones)).collect();
        t.insert("materials".into(), list_value(&mats));
        id_lines.push(format!("{}: {}{}{}", m.name, if m.eyes.max_per_frame > 0 { format!("{} eye(s) with white and pupil", m.eyes.max_per_frame) } else { "no eyes".to_string() }, if cols.is_empty() { String::new() } else { format!("; colours {}", m.identity.iter().take(8).map(|(c, _)| format!("{} ({})", hex(*c), crate::insight::colour_name(*c))).collect::<Vec<_>>().join(", ")) }, if mats.is_empty() { String::new() } else { format!("; materials {}", mats.join(", ")) }));
        idt.insert(m.name.clone(), toml::Value::Table(t));
        record(&home, "operator", &format!("identity.{}", m.name), &format!("identity list of {}: {}", m.name, id_lines.last().unwrap().split_once(": ").map(|x| x.1).unwrap_or("")))?;
    }
    b.insert("identity".into(), toml::Value::Table(idt));
    // real questions
    let mut questions: Vec<serde_json::Value> = Vec::new();
    let first_image = assets.first().map(|a| a.dir.join("compare.png")).filter(|p| p.is_file());
    let existing = decisions::load(&home);
    let by = out.agent().unwrap_or("operator").to_string();
    let mut ask_once = |topic: &str, opts: Vec<(String, String, String)>, rec: &str, why: &str, b: &mut toml::Table| -> Result<()> {
        let _ = b;
        if existing.iter().any(|d| d.topic == topic) {
            return Ok(());
        }
        let d = decisions::ask(&home, topic, None, &opts, rec, why, &by)?;
        questions.push(json!({"id": d.id, "topic": topic, "options": d.options.iter().map(|o| json!({"key": o.key, "label": o.label})).collect::<Vec<_>>(), "recommend": rec, "why": why, "page": decisions::page_of(&home, &d.id).slash()}));
        Ok(())
    };
    if let Some(other) = inf.mentioned.iter().find(|c| **c != pc.id) {
        if !kept(&b, "console") || value_of(&b, "console") != pc.id {
            let (rec, why, opts) = question("which console is the target", &[("other", other), ("project", &pc.id)]).unwrap_or_else(|| ("B".into(), String::new(), vec![("A".into(), format!("keep {}", pc.id)), ("B".into(), format!("switch to {}", other))]));
            ask_once("which console is the target", opts.into_iter().map(|(k, l)| (k, String::new(), l)).collect(), &rec, &why, &mut b)?;
        }
    }
    if inf.contradictory || (has_any(&request, &["style", "look", "mood", "vibe"]) && !has_any(&request, &FAITHFUL) && !has_any(&request, &REINTERPRET) && !ms.is_empty()) {
        let img = first_image.as_ref().map(|p| p.slash()).unwrap_or_default();
        let (rec, why, opts) = question("style direction", &[]).unwrap_or_else(|| ("A".into(), String::new(), vec![("A".into(), "faithful".into()), ("B".into(), "reinterpreted".into())]));
        ask_once("style direction", opts.into_iter().map(|(k, l)| (k, img.clone(), l)).collect(), &rec, &why, &mut b)?;
    }
    write_brief(&home, &b)?;
    // interview and answers
    if a.interview {
        interview(&home, &mut Vec::new())?;
    }
    if let Some(f) = &a.answers {
        let t: toml::Table = std::fs::read_to_string(f).map_err(|e| PxlError::io(format!("{}: {}", f, e)))?.parse().map_err(|e| PxlError::usage(format!("{}: {}", f, e)))?;
        for (k, v) in t {
            let s = match &v {
                toml::Value::String(s) => s.clone(),
                toml::Value::Array(a) => a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(","),
                o => o.to_string(),
            };
            apply_set(&home, &format!("{}={}", k, s), "user")?;
        }
    }
    let mut text = format!("brief written to {}\nassumptions (change one with `pxl decide \"why\" --set key=value`):\n", brief_path(&home).slash());
    for (k, v, why) in &assumptions {
        text.push_str(&format!("  {} = {}  ({})\n", k, v, why));
    }
    for l in &id_lines {
        text.push_str(&format!("  identity {}\n", l));
    }
    if questions.is_empty() {
        text.push_str("questions: none; everything was inferred or assumed. Show the style frames and let the user correct what they dislike.");
    } else {
        text.push_str(&format!("questions ({}), each with options and a recommendation:\n", questions.len()));
        for q in &questions {
            text.push_str(&format!("  {} {}: options {}; recommend {} because {}\n    page {}\n", q["id"].as_str().unwrap_or(""), q["topic"].as_str().unwrap_or(""), q["options"].as_array().map(|o| o.iter().map(|x| format!("{} = {}", x["key"].as_str().unwrap_or(""), x["label"].as_str().unwrap_or(""))).collect::<Vec<_>>().join(" | ")).unwrap_or_default(), q["recommend"].as_str().unwrap_or(""), q["why"].as_str().unwrap_or(""), q["page"].as_str().unwrap_or("")));
        }
        text.pop();
    }
    out.emit(
        text,
        json!({
            "brief": brief_path(&home).slash(),
            "assumptions": assumptions.iter().map(|(k, v, w)| json!({"key": k, "value": v, "why": w})).collect::<Vec<_>>(),
            "identity": id_lines,
            "questions": questions,
            "checked": {"ok": ms.len(), "of": assets.len()},
        }),
    );
    Ok(())
}

/// Opt-in questionnaire on stdin: one prompt per item with the current value as the default.
fn interview(home: &Path, _log: &mut Vec<String>) -> Result<()> {
    use std::io::{BufRead, Write};
    let b = read_brief(home);
    let stdin = std::io::stdin();
    for k in ["console", "fidelity", "cadence", "tradeoff", "exceptions", "deadline"] {
        let cur = if k == "tradeoff" { list_of(&b, k).join(",") } else { value_of(&b, k) };
        eprint!("{} [{}]: ", k, cur);
        let _ = std::io::stderr().flush();
        let mut line = String::new();
        if stdin.lock().read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let v = line.trim();
        if !v.is_empty() && v != cur {
            apply_set(home, &format!("{}={}", k, v), "user")?;
        }
    }
    Ok(())
}
