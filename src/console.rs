//! Consoles and image modes (DESIGN "Consoles and image modes").
//!
//! A console is a folder of data: `consoles/<id>/console.toml` (the hardware facts shared by every image mode)
//! plus `sprite.toml`, `tile.toml` and `screen.toml` (the image modes). The built-in consoles are compiled in.
//! A project may override any file from disk with `consoles/<id>/<file>.toml` (found by walking up from the part
//! or the manifest); its keys replace the built-in keys, tables merge. A console that exists only on disk works too.
//! A mode file inherits every key of `console.toml`; `[console]` and `[mode]` tables carry descriptions only.

use crate::fsutil::PathSlash;
use crate::cli::ConsoleCmd;
use crate::fsutil;
use crate::output::{Loc, Out, PxlError, Result};
use crate::profile::{self, Profile};
use serde_json::json;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

pub const DEFAULT_CONSOLE: &str = "gbc";
pub const MODES: [&str; 3] = ["sprite", "tile", "screen"];
const FILES: [&str; 4] = ["console", "sprite", "tile", "screen"];

/// Built-in consoles: id and the four embedded files (console, sprite, tile, screen).
const BUILTIN: [(&str, [&str; 4]); 9] = [
    (
        "gbc",
        [
            include_str!("../consoles/gbc/console.toml"),
            include_str!("../consoles/gbc/sprite.toml"),
            include_str!("../consoles/gbc/tile.toml"),
            include_str!("../consoles/gbc/screen.toml"),
        ],
    ),
    (
        "gb",
        [
            include_str!("../consoles/gb/console.toml"),
            include_str!("../consoles/gb/sprite.toml"),
            include_str!("../consoles/gb/tile.toml"),
            include_str!("../consoles/gb/screen.toml"),
        ],
    ),
    (
        "nes",
        [
            include_str!("../consoles/nes/console.toml"),
            include_str!("../consoles/nes/sprite.toml"),
            include_str!("../consoles/nes/tile.toml"),
            include_str!("../consoles/nes/screen.toml"),
        ],
    ),
    (
        "sms",
        [
            include_str!("../consoles/sms/console.toml"),
            include_str!("../consoles/sms/sprite.toml"),
            include_str!("../consoles/sms/tile.toml"),
            include_str!("../consoles/sms/screen.toml"),
        ],
    ),
    (
        "gg",
        [
            include_str!("../consoles/gg/console.toml"),
            include_str!("../consoles/gg/sprite.toml"),
            include_str!("../consoles/gg/tile.toml"),
            include_str!("../consoles/gg/screen.toml"),
        ],
    ),
    (
        "md",
        [
            include_str!("../consoles/md/console.toml"),
            include_str!("../consoles/md/sprite.toml"),
            include_str!("../consoles/md/tile.toml"),
            include_str!("../consoles/md/screen.toml"),
        ],
    ),
    (
        "snes",
        [
            include_str!("../consoles/snes/console.toml"),
            include_str!("../consoles/snes/sprite.toml"),
            include_str!("../consoles/snes/tile.toml"),
            include_str!("../consoles/snes/screen.toml"),
        ],
    ),
    (
        "pce",
        [
            include_str!("../consoles/pce/console.toml"),
            include_str!("../consoles/pce/sprite.toml"),
            include_str!("../consoles/pce/tile.toml"),
            include_str!("../consoles/pce/screen.toml"),
        ],
    ),
    (
        "gba",
        [
            include_str!("../consoles/gba/console.toml"),
            include_str!("../consoles/gba/sprite.toml"),
            include_str!("../consoles/gba/tile.toml"),
            include_str!("../consoles/gba/screen.toml"),
        ],
    ),
];

/// Old profile names and the console / mode they mean.
pub fn legacy_profile(name: &str) -> Option<(&'static str, &'static str)> {
    match name {
        "gbc-sprite" => Some(("gbc", "sprite")),
        "gbc-tile" => Some(("gbc", "tile")),
        "gbc-screen" => Some(("gbc", "screen")),
        _ => None,
    }
}

pub fn builtin_ids() -> Vec<&'static str> {
    BUILTIN.iter().map(|(i, _)| *i).collect()
}

fn cerr(src: &str, msg: String) -> PxlError {
    PxlError::validation(msg).with_loc(Loc::file(src.to_string()))
}

fn file_index(file: &str) -> usize {
    FILES.iter().position(|f| *f == file).unwrap_or(0)
}

fn parse_table(text: &str, src: &str) -> Result<toml::Table> {
    text.parse().map_err(|e: toml::de::Error| cerr(src, format!("invalid console TOML: {}", e.message())))
}

/// `b` over `a`: scalar and array keys of `b` replace, tables merge key by key.
fn merge(a: &mut toml::Table, b: toml::Table) {
    for (k, v) in b {
        match (a.get_mut(&k), v) {
            (Some(toml::Value::Table(x)), toml::Value::Table(y)) => merge(x, y),
            (_, v) => {
                a.insert(k, v);
            }
        }
    }
}

fn disk_file(start: Option<&Path>, id: &str, file: &str) -> Option<PathBuf> {
    start.and_then(|d| fsutil::find_up(d, &format!("consoles/{}/{}.toml", id, file)))
}

/// Does a console of this id exist (built-in, or a `consoles/<id>/console.toml` above `start`)?
pub fn exists(id: &str, start: Option<&Path>) -> bool {
    BUILTIN.iter().any(|(i, _)| *i == id) || disk_file(start, id, "console").is_some()
}

fn unknown(id: &str, start: Option<&Path>) -> PxlError {
    let mut ids: Vec<String> = builtin_ids().into_iter().map(String::from).collect();
    for c in disk_ids(start) {
        if !ids.contains(&c) {
            ids.push(c);
        }
    }
    PxlError::usage(format!("unknown console '{}'", id)).with_hint(format!("consoles: {} (see `pxl console list`)", ids.join(", ")))
}

/// One file of a console: the built-in text with the project's file over it.
fn file_table(id: &str, file: &str, start: Option<&Path>) -> Result<toml::Table> {
    let mut t = match BUILTIN.iter().find(|(i, _)| *i == id) {
        Some((_, files)) => parse_table(files[file_index(file)], &format!("consoles/{}/{}.toml", id, file))?,
        None => toml::Table::new(),
    };
    if let Some(f) = disk_file(start, id, file) {
        let o = parse_table(&fsutil::read_string(&f)?, &f.slash())?;
        merge(&mut t, o);
    }
    Ok(t)
}

/// Facts of one console. `source` is `built-in` or `built-in + project` / `project`.
#[derive(Clone, Debug)]
pub struct ConsoleInfo {
    pub id: String,
    pub name: String,
    pub short: String,
    pub screen: (u32, u32),
    pub colour: String,
    pub palettes: String,
    pub oam: String,
    pub vram: String,
    pub references: Vec<String>,
    /// Typical background colour of the console's screen (`compare_1x.png`).
    pub preview_bg: String,
    pub source: String,
}

/// Design-language hints of a console (`[style]` in console.toml; the project's `[style]` in pxl.toml overrides keys). Skills, the automatic
/// first version and the quality harness read them. Keys:
/// * `white`: `active` (white is a main tone: classic Game Boy), `highlights` (white only for highlights and eyes), `balanced`;
/// * `outline`: `darkest` (the darkest tone is the outline), `ramp` (a dark tone of the material), `none`;
/// * `min_value_spread`: Oklab lightness range (0..1) the output should span;
/// * `value_map`: `stretch` (map by luminance with a contrast stretch per sprite) or `colour` (keep the hues);
/// * `reason`: why.
#[derive(Clone, Debug, PartialEq)]
pub struct Style {
    pub white: String,
    pub outline: String,
    pub min_value_spread: f64,
    pub value_map: String,
    pub reason: String,
}

impl Default for Style {
    fn default() -> Style {
        Style { white: "highlights".into(), outline: "darkest".into(), min_value_spread: 0.5, value_map: "colour".into(), reason: String::new() }
    }
}

fn style_from(t: &toml::Table, base: &mut Style) -> std::result::Result<(), String> {
    for (k, v) in t {
        let st = || v.as_str().map(String::from).ok_or_else(|| format!("[style] {} must be a string", k));
        match k.as_str() {
            "white" => {
                let x = st()?;
                if !["active", "highlights", "balanced"].contains(&x.as_str()) {
                    return Err("[style] white must be active, highlights or balanced".into());
                }
                base.white = x;
            }
            "outline" => {
                let x = st()?;
                if !["darkest", "ramp", "none"].contains(&x.as_str()) {
                    return Err("[style] outline must be darkest, ramp or none".into());
                }
                base.outline = x;
            }
            "value_map" => {
                let x = st()?;
                if !["stretch", "colour"].contains(&x.as_str()) {
                    return Err("[style] value_map must be stretch or colour".into());
                }
                base.value_map = x;
            }
            "reason" => base.reason = st()?,
            "min_value_spread" => base.min_value_spread = v.as_float().or_else(|| v.as_integer().map(|i| i as f64)).filter(|f| (0.0..=1.0).contains(f)).ok_or("[style] min_value_spread must be a number 0..1")?,
            o => return Err(format!("[style]: unknown key '{}' (allowed: white, outline, min_value_spread, value_map, reason)", o)),
        }
    }
    Ok(())
}

/// The style of a console: its console.toml `[style]`, then the `[style]` table of the project's pxl.toml found above `start`.
pub fn style(id: &str, start: Option<&Path>) -> Style {
    let mut s = Style::default();
    if let Ok(t) = file_table(id, "console", start) {
        if let Some(st) = t.get("style").and_then(|x| x.as_table()) {
            let _ = style_from(st, &mut s);
        }
    }
    if let Some(m) = start.and_then(|d| fsutil::find_up(d, "pxl.toml")) {
        if let Some(st) = std::fs::read_to_string(&m).ok().and_then(|t| t.parse::<toml::Table>().ok()).and_then(|t| t.get("style")?.as_table().cloned()) {
            let _ = style_from(&st, &mut s);
        }
    }
    s
}

/// Validate a project `[style]` table (used by the manifest parser).
pub fn check_style(t: &toml::Table) -> std::result::Result<(), String> {
    style_from(t, &mut Style::default())
}

fn pair(v: Option<&toml::Value>) -> Option<(u32, u32)> {
    let a = v?.as_array()?;
    if a.len() != 2 {
        return None;
    }
    Some((a[0].as_integer()? as u32, a[1].as_integer()? as u32))
}

pub fn info(id: &str, start: Option<&Path>) -> Result<ConsoleInfo> {
    if !exists(id, start) {
        return Err(unknown(id, start));
    }
    let t = file_table(id, "console", start)?;
    let c = t.get("console").and_then(|c| c.as_table()).cloned().unwrap_or_default();
    let s = |k: &str| c.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let builtin = BUILTIN.iter().any(|(i, _)| *i == id);
    let disk = disk_file(start, id, "console").is_some() || MODES.iter().any(|m| disk_file(start, id, m).is_some());
    Ok(ConsoleInfo {
        id: id.to_string(),
        name: if s("name").is_empty() { id.to_string() } else { s("name") },
        short: s("short"),
        screen: pair(c.get("screen")).unwrap_or((160, 144)),
        colour: s("colour"),
        palettes: s("palettes"),
        oam: s("oam"),
        vram: s("vram"),
        preview_bg: s("preview_bg"),
        references: c.get("references").and_then(|r| r.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default(),
        source: match (builtin, disk) {
            (true, false) => "built-in".into(),
            (true, true) => "built-in + project".into(),
            _ => "project".into(),
        },
    })
}

/// Console ids that exist only on disk (`consoles/<id>/console.toml` in the project above `start`).
fn disk_ids(start: Option<&Path>) -> Vec<String> {
    let mut ids = Vec::new();
    let Some(d) = start.and_then(|d| fsutil::find_up(d, "consoles")) else { return ids };
    if let Ok(rd) = std::fs::read_dir(&d) {
        for e in rd.flatten() {
            if e.path().join("console.toml").is_file() {
                if let Some(n) = e.file_name().to_str() {
                    ids.push(n.to_string());
                }
            }
        }
    }
    ids.sort();
    ids
}

/// Every console: the built-ins, then those only the project defines.
pub fn list(start: Option<&Path>) -> Vec<ConsoleInfo> {
    let mut ids: Vec<String> = builtin_ids().into_iter().map(String::from).collect();
    for c in disk_ids(start) {
        if !ids.contains(&c) {
            ids.push(c);
        }
    }
    ids.iter().filter_map(|i| info(i, start).ok()).collect()
}

/// The merged table of one image mode (console.toml + mode file, project overrides applied), without the
/// description tables.
pub fn mode_table(id: &str, mode: &str, start: Option<&Path>) -> Result<toml::Table> {
    if !exists(id, start) {
        return Err(unknown(id, start));
    }
    if !MODES.contains(&mode) {
        return Err(PxlError::usage(format!("unknown image mode '{}'", mode)).with_hint("modes: sprite, tile, screen"));
    }
    let mut t = file_table(id, "console", start)?;
    merge(&mut t, file_table(id, mode, start)?);
    t.remove("console");
    t.remove("mode");
    t.remove("style");
    Ok(t)
}

/// The profile of `<console>/<mode>`: console facts, then the mode file, then the project's overrides.
pub fn resolve(id: &str, mode: &str, start: Option<&Path>) -> Result<Profile> {
    let t = mode_table(id, mode, start)?;
    let mut p = profile::parse_table(&format!("{}/{}", id, mode), &t, &format!("consoles/{}/{}.toml", id, mode))?;
    if mode == "screen" && p.size.is_none() {
        p.size = Some(info(id, start)?.screen);
    }
    p.style = style(id, start);
    Ok(p)
}

// ------------------------------------------------------------------------------------- project console

/// The console a project targets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectConsole {
    pub id: String,
    /// `[project] console` is set in the manifest.
    pub explicit: bool,
    /// The manifest found above the start folder (None: no project; `gbc` is used silently).
    pub manifest: Option<PathBuf>,
}

type Cache = HashMap<PathBuf, (Option<SystemTime>, u64, Option<String>)>;
static CACHE: Mutex<Option<Cache>> = Mutex::new(None);

fn console_of_manifest(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    let key = (meta.modified().ok(), meta.len());
    if let Ok(g) = CACHE.lock() {
        if let Some((m, l, c)) = g.as_ref().and_then(|c| c.get(path)) {
            if (*m, *l) == key {
                return c.clone();
            }
        }
    }
    let c = std::fs::read_to_string(path).ok().and_then(|t| t.parse::<toml::Table>().ok()).and_then(|t| t.get("project")?.as_table()?.get("console")?.as_str().map(String::from));
    if let Ok(mut g) = CACHE.lock() {
        g.get_or_insert_with(HashMap::new).insert(path.to_path_buf(), (key.0, key.1, c.clone()));
    }
    c
}

/// The console of the project that holds `dir` (the nearest `pxl.toml` above it); `gbc` when there is none.
pub fn project_console(dir: &Path) -> ProjectConsole {
    match crate::manifest::Manifest::find_file(dir) {
        Some(m) => match console_of_manifest(&m) {
            Some(id) => ProjectConsole { id, explicit: true, manifest: Some(m) },
            None => ProjectConsole { id: DEFAULT_CONSOLE.into(), explicit: false, manifest: Some(m) },
        },
        None => ProjectConsole { id: DEFAULT_CONSOLE.into(), explicit: false, manifest: None },
    }
}

/// One-line note for `lint` and `check` when parts still name an old `profile = "gbc-*"`.
pub fn deprecated_profile_note(names: &[String]) -> Option<String> {
    let mut uniq: Vec<&String> = names.iter().collect();
    uniq.sort();
    uniq.dedup();
    if names.is_empty() {
        return None;
    }
    let list: Vec<String> = uniq.iter().map(|n| format!("{}", n)).collect();
    Some(format!("{} part(s) use the deprecated profile name(s) {}; the console/mode form replaces them (run `pxl migrate`)", names.len(), list.join(", ")))
}

/// One-line note when the project holding `dir` has a pxl.toml without `[project] console`.
pub fn no_console_note(dir: &Path) -> Option<String> {
    let pc = project_console(dir);
    (pc.manifest.is_some() && !pc.explicit).then(|| format!("the project sets no `[project] console`; using {} (`pxl console set {}` or `pxl migrate` writes it)", DEFAULT_CONSOLE, DEFAULT_CONSOLE))
}

/// The deprecated legacy profile name a part declares, if any.
pub fn legacy_name_of(part: &crate::model::Part) -> Option<String> {
    part.profile.as_deref().filter(|p| legacy_profile(p).is_some()).map(String::from)
}

// ----------------------------------------------------------------------------------------------- CLI

fn start_dir() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

pub fn run(cmd: &ConsoleCmd, out: &mut Out) -> Result<()> {
    match cmd {
        ConsoleCmd::List => run_list(out),
        ConsoleCmd::Show { id } => run_show(id.as_deref(), out),
        ConsoleCmd::Set { id } => run_set(id, out),
    }
}

fn run_list(out: &mut Out) -> Result<()> {
    let start = start_dir();
    let pc = project_console(&start);
    let all = list(Some(&start));
    let mut text = String::new();
    let mut js = Vec::new();
    for c in &all {
        let mark = if c.id == pc.id { "*" } else { " " };
        text.push_str(&format!("{} {:<6} {:<18} {}x{}  {}; {}; {}  [{}]\n", mark, c.id, c.name, c.screen.0, c.screen.1, c.colour, c.palettes, c.oam, c.source));
        js.push(json!({"id": c.id, "name": c.name, "short": c.short, "screen": [c.screen.0, c.screen.1], "colour": c.colour, "palettes": c.palettes, "oam": c.oam, "vram": c.vram, "source": c.source, "current": c.id == pc.id}));
    }
    text.push_str(&format!("project console: {}{}", pc.id, if pc.explicit { "" } else if pc.manifest.is_some() { " (default; `pxl console set` writes it)" } else { " (default, no pxl.toml here)" }));
    out.emit(text, json!({"consoles": js, "project_console": pc.id}));
    Ok(())
}

fn run_show(id: Option<&str>, out: &mut Out) -> Result<()> {
    let start = start_dir();
    let id = id.map(String::from).unwrap_or_else(|| project_console(&start).id);
    let c = info(&id, Some(&start))?;
    let mut text = format!("{} ({}) [{}]\nscreen {}x{}\ncolour: {}\npalettes: {}\nsprites: {}\nvram: {}\n", c.name, c.id, c.source, c.screen.0, c.screen.1, c.colour, c.palettes, c.oam, c.vram);
    for r in &c.references {
        text.push_str(&format!("reference: {}\n", r));
    }
    let st = style(&id, Some(&start));
    text.push_str(&format!("style: white {}, outline {}, value_map {}, min_value_spread {}{}\n", st.white, st.outline, st.value_map, st.min_value_spread, if st.reason.is_empty() { String::new() } else { format!(" ({})", st.reason) }));
    let mut modes = serde_json::Map::new();
    for m in MODES {
        let t = mode_table(&id, m, Some(&start))?;
        let own = file_table(&id, m, Some(&start))?;
        let desc = own.get("mode").and_then(|x| x.as_table()).and_then(|x| x.get("description")).and_then(|x| x.as_str()).unwrap_or("").to_string();
        let p = resolve(&id, m, Some(&start))?;
        text.push_str(&format!("\n[{}] {}\n", m, desc));
        let mut keys: BTreeMap<String, String> = BTreeMap::new();
        for (k, v) in &t {
            keys.insert(k.clone(), v.to_string());
        }
        if m == "screen" {
            keys.insert("size".into(), format!("[{}, {}]", p.size.map_or(0, |s| s.0), p.size.map_or(0, |s| s.1)));
        }
        let inherited: Vec<&String> = keys.keys().filter(|k| !own.contains_key(*k)).collect();
        for (k, v) in &keys {
            if k == "budget" {
                continue;
            }
            text.push_str(&format!("  {} = {}{}\n", k, v, if inherited.contains(&k) { "   (console)" } else { "" }));
        }
        if let Some(b) = t.get("budget").and_then(|b| b.as_table()) {
            for (cat, bt) in b {
                text.push_str(&format!("  budget.{} = {}\n", cat, bt));
            }
        }
        // stacked sprites (two-slot cells): a plain-language line of the rule, from this console's numbers
        let multi = t.get("multi_slot_max").and_then(|v| v.as_integer()).unwrap_or(0);
        if m == "sprite" && multi > 0 {
            let player = t.get("budget").and_then(|b| b.get("player")).and_then(|b| b.as_table());
            let ov = player.and_then(|b| b.get("overlay_line")).and_then(|v| v.as_integer());
            let cell = p.hw_cell;
            text.push_str(&format!(
                "  stacked sprites: category `player` only. Up to {} {}x{} cells per frame (multi_slot_max) may be stacked: a cell that needs 4 to 6 colours is drawn twice on the same spot with two palette slots (`slot` and `slot2`), so it costs 2 sprites{}. Any other category must fit every cell in one slot. Set the category with `pxl asset set ASSET category player` (or `pxl ingest --group player`); `pxl palette plan SCENE` proposes the stacked cells; `pxl cells PART FRAME` shows them\n",
                multi,
                cell.0,
                cell.1,
                ov.map_or(String::new(), |o| format!(" and a scanline with one may carry {} sprites (budget.player.overlay_line)", o))
            ));
        }
        modes.insert(m.to_string(), json!({"description": desc, "facts": keys}));
    }
    out.emit(text.trim_end(), json!({"console": {"id": c.id, "name": c.name, "screen": [c.screen.0, c.screen.1], "colour": c.colour, "palettes": c.palettes, "oam": c.oam, "vram": c.vram, "references": c.references, "source": c.source, "style": {"white": st.white, "outline": st.outline, "value_map": st.value_map, "min_value_spread": st.min_value_spread, "reason": st.reason}}, "modes": modes}));
    Ok(())
}

fn run_set(id: &str, out: &mut Out) -> Result<()> {
    let start = start_dir();
    if !exists(id, Some(&start)) {
        return Err(unknown(id, Some(&start)));
    }
    let m = crate::manifest::Manifest::find_file(&start).ok_or_else(|| PxlError::usage("`console set` needs a pxl.toml").with_hint("run it inside the project folder"))?;
    let text = fsutil::read_string(&m)?;
    let new = set_console_text(&text, id);
    if new != text {
        fsutil::atomic_write(&m, new.as_bytes())?;
    }
    out.emit(format!("{}: [project] console = \"{}\"", m.slash(), id), json!({"manifest": m.slash(), "console": id, "changed": new != text}));
    Ok(())
}

/// pxl.toml text with `[project] console = "<id>"`: replaces the line, adds it to an existing `[project]`, or
/// inserts a new `[project]` table before the first table of the file.
pub fn set_console_text(text: &str, id: &str) -> String {
    let line = format!("console = \"{}\"", id);
    let lines: Vec<&str> = text.lines().collect();
    let is_header = |l: &str| l.trim_start().starts_with('[');
    let mut out: Vec<String> = Vec::new();
    if let Some(pi) = lines.iter().position(|l| l.trim() == "[project]") {
        let end = lines.iter().enumerate().skip(pi + 1).find(|(_, l)| is_header(l)).map_or(lines.len(), |(i, _)| i);
        let ci = (pi + 1..end).find(|&i| lines[i].trim_start().starts_with("console") && lines[i].contains('='));
        for (i, l) in lines.iter().enumerate() {
            if Some(i) == ci {
                out.push(line.clone());
            } else {
                out.push(l.to_string());
            }
            if i == pi && ci.is_none() {
                out.push(line.clone());
            }
        }
    } else if let Some(fi) = lines.iter().position(|l| is_header(l)) {
        // keep a comment block directly above the first table with that table
        let mut at = fi;
        while at > 0 && lines[at - 1].trim_start().starts_with('#') {
            at -= 1;
        }
        for (i, l) in lines.iter().enumerate() {
            if i == at {
                out.push("[project]".into());
                out.push(line.clone());
                out.push(String::new());
            }
            out.push(l.to_string());
        }
    } else {
        out = lines.iter().map(|l| l.to_string()).collect();
        if !out.is_empty() && !out.last().map_or(true, |l| l.is_empty()) {
            out.push(String::new());
        }
        out.push("[project]".into());
        out.push(line);
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}
