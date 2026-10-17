//! Colour templates (DESIGN 4.3): layers of keys, input colour map, `extends`, grid validation (AC-TP2).
//!
//! Optional `[features]` table: which keys draw an eye, used by the eye metric, lint and the stamping pass
//! (`features.rs`). Keys or role names; without it the lightest key is the white and the `outline` role (else the
//! darkest key) the pupil:
//!
//! ```toml
//! [features]
//! eye = { white = "W", pupil = "1" }
//! ```
//!
//! A `near` map rule whose filter fails falls back via [`Template::fallback_key`] (the `under` key unless a key of
//! the same layer or a base key is clearly closer in Oklab), so a bright red never becomes the blue outline.

use crate::fsutil::PathSlash;
use crate::color::{self, Rgb};
use crate::fsutil;
use crate::model::Part;
use crate::output::{Loc, Out, PxlError, Result};
use serde_json::json;
use std::path::{Path, PathBuf};

/// A fallback key must be this much closer (Oklab distance) than the `under` key to replace it.
pub const FALLBACK_MARGIN: f64 = 0.04;

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct KeyDef {
    pub key: u8,
    pub role: String,
    /// None only for the transparent key.
    pub color: Option<Rgb>,
    /// Overlay keys: the base key that shows through in the base layer export.
    pub under: Option<u8>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Layer {
    pub name: String,
    pub keys: Vec<KeyDef>,
}

/// `from` maps to `key`; when `near = (c, r)` the rule applies only within `r` source px of colour `c`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MapRule {
    pub from: Rgb,
    pub key: u8,
    pub near: Option<(Rgb, u32)>,
}

/// `[features]` table: which keys draw eyes (`eye = { white = "W", pupil = "1" }`); absent = derived.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Features {
    pub eye_white: Option<u8>,
    pub eye_pupil: Option<u8>,
}

/// One hardware tile palette of `[palettes]` (`name = "abcd"`): up to 4 keys.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TilePalette {
    pub name: String,
    pub keys: Vec<u8>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Template {
    pub name: String,
    pub path: PathBuf,
    /// In file order; parent layers first.
    pub layers: Vec<Layer>,
    pub map: Vec<MapRule>,
    /// `[features]` hints (eye keys).
    pub features: Features,
    /// `[palettes]` (tile templates): at most 8 palettes of at most 4 keys, in file order.
    pub palettes: Vec<TilePalette>,
    /// `[tiles] air`: keys that show the backdrop (a tile containing air needs one in its palette).
    pub tile_air: Vec<u8>,
    transparent: KeyDef,
}

impl Template {
    pub fn empty(name: &str) -> Template {
        Template {
            name: name.to_string(),
            path: PathBuf::new(),
            layers: Vec::new(),
            map: Vec::new(),
            features: Features::default(),
            palettes: Vec::new(),
            tile_air: Vec::new(),
            transparent: KeyDef { key: b'.', role: "transparent".into(), color: None, under: None },
        }
    }
    /// Definition of a key. `.` is always defined (transparent).
    pub fn def(&self, key: u8) -> Option<&KeyDef> {
        if key == b'.' {
            return Some(&self.transparent);
        }
        self.layers.iter().flat_map(|l| l.keys.iter()).find(|k| k.key == key)
    }
    /// Key with this role name.
    pub fn key_for_role(&self, role: &str) -> Option<u8> {
        if role == "transparent" {
            return Some(b'.');
        }
        self.layers.iter().flat_map(|l| l.keys.iter()).find(|k| k.role == role).map(|k| k.key)
    }
    /// Index into `layers` of the layer that defines `key` (`.` belongs to no layer).
    pub fn layer_of(&self, key: u8) -> Option<usize> {
        self.layers.iter().position(|l| l.keys.iter().any(|k| k.key == key))
    }
    /// A key written as the key character itself or as its role name (`W` or `white`).
    pub fn resolve(&self, token: &str) -> Option<u8> {
        let b = token.as_bytes();
        if b.len() == 1 && self.def(b[0]).is_some() {
            return Some(b[0]);
        }
        self.key_for_role(token)
    }
    pub fn color_of(&self, key: u8) -> Option<Rgb> {
        self.def(key).and_then(|d| d.color)
    }
    /// Key for a colour whose `near` rule (to `rule_key`) does not apply: the key under `rule_key` (an overlay's
    /// base), unless the colour is clearly closer (Oklab, by `FALLBACK_MARGIN`) to another key of the rule's own
    /// layer or to the nearest base key: a bright red far from the dark red goes to the dark red, not to the
    /// blue outline under the shoe. Without `under`: the nearest base key.
    pub fn fallback_key(&self, rule_key: u8, c: Rgb) -> u8 {
        let o = color::srgb_to_oklab(c);
        let d = |k: u8| -> Option<f64> { self.color_of(k).map(|kc| color::oklab_dist2(o, color::srgb_to_oklab(kc)).sqrt()) };
        let mut nb: Option<(u8, f64)> = None;
        if let Some(l) = self.layers.first() {
            for k in &l.keys {
                if let Some(dd) = d(k.key) {
                    if nb.map_or(true, |b| dd < b.1) {
                        nb = Some((k.key, dd));
                    }
                }
            }
        }
        let Some(nb) = nb else { return rule_key };
        let mut best = nb;
        if let Some(li) = self.layer_of(rule_key) {
            for kd in &self.layers[li].keys {
                if kd.key != rule_key {
                    if let Some(dd) = d(kd.key) {
                        if dd < best.1 {
                            best = (kd.key, dd);
                        }
                    }
                }
            }
        }
        match self.def(rule_key).and_then(|k| k.under) {
            Some(u) => match d(u) {
                Some(du) if best.1 + FALLBACK_MARGIN < du => best.0,
                _ => u,
            },
            None => nb.0,
        }
    }

    /// All defined keys (excluding `.`), layer by layer in file order.
    pub fn keys(&self) -> impl Iterator<Item = &KeyDef> {
        self.layers.iter().flat_map(|l| l.keys.iter())
    }
}

fn terr(path: &Path, msg: impl Into<String>) -> PxlError {
    PxlError::validation(msg).with_loc(Loc::file(path.slash()))
}

fn one_key(path: &Path, what: &str, s: &str) -> Result<u8> {
    let b = s.as_bytes();
    if b.len() != 1 || !crate::pxl::valid_key(b[0]) {
        return Err(terr(path, format!("{} '{}' must be a single printable ASCII character (no space, quote or backslash)", what, s)));
    }
    Ok(b[0])
}

fn parse_color(path: &Path, ctx: &str, v: &toml::Value) -> Result<Rgb> {
    let s = v.as_str().ok_or_else(|| terr(path, format!("{}: colour must be a string like \"#4850b8\"", ctx)))?;
    color::parse_hex(s).map_err(|e| terr(path, format!("{}: {}", ctx, e.msg)))
}

fn check_keys(path: &Path, ctx: &str, t: &toml::Table, allowed: &[&str]) -> Result<()> {
    for k in t.keys() {
        if !allowed.contains(&k.as_str()) {
            return Err(terr(path, format!("{}: unknown key '{}'", ctx, k)).with_hint(format!("allowed: {}", allowed.join(", "))));
        }
    }
    Ok(())
}

fn load_inner(path: &Path, depth: usize) -> Result<Template> {
    if depth > 8 {
        return Err(terr(path, "`extends` chain too deep (cycle?)"));
    }
    let text = fsutil::read_string(path)?;
    let root: toml::Table = text.parse().map_err(|e: toml::de::Error| {
        let (l, c) = e.span().map_or((0, 0), |s| {
            let b = &text[..s.start.min(text.len())];
            (b.matches('\n').count() + 1, b.rsplit('\n').next().map_or(0, |x| x.chars().count()) + 1)
        });
        terr(path, format!("{} (line {}, column {})", e.message(), l, c))
    })?;
    check_keys(path, "template", &root, &["name", "extends", "layers", "map", "features", "palettes", "tiles"])?;
    let fallback_name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("template").to_string();
    let mut t = match root.get("extends") {
        Some(v) => {
            let rel = v.as_str().ok_or_else(|| terr(path, "`extends` must be a path string"))?;
            let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
            let base = fsutil::find_up(dir, rel).ok_or_else(|| {
                terr(path, format!("extends '{}' not found", rel)).with_hint("the path is tried relative to this file and each parent folder")
            })?;
            load_inner(&base, depth + 1)?
        }
        None => Template::empty(&fallback_name),
    };
    t.path = path.to_path_buf();
    t.name = match root.get("name") {
        Some(v) => v.as_str().ok_or_else(|| terr(path, "`name` must be a string"))?.to_string(),
        None => fallback_name,
    };
    if let Some(lv) = root.get("layers") {
        let lt = lv.as_table().ok_or_else(|| terr(path, "`layers` must be a table of [layers.<name>]"))?;
        for (lname, kv) in lt {
            let kt = kv.as_table().ok_or_else(|| terr(path, format!("[layers.{}] must be a table", lname)))?;
            let li = match t.layers.iter().position(|l| &l.name == lname) {
                Some(i) => i,
                None => {
                    t.layers.push(Layer { name: lname.clone(), keys: Vec::new() });
                    t.layers.len() - 1
                }
            };
            for (ks, def) in kt {
                let ctx = format!("[layers.{}] \"{}\"", lname, ks);
                let key = one_key(path, "key", ks)?;
                let d = def.as_table().ok_or_else(|| terr(path, format!("{}: expected {{ role = ..., color = ... }}", ctx)))?;
                check_keys(path, &ctx, d, &["role", "color", "under"])?;
                let role = d
                    .get("role")
                    .and_then(|r| r.as_str())
                    .ok_or_else(|| terr(path, format!("{}: missing `role`", ctx)))?
                    .to_string();
                let color = match d.get("color") {
                    Some(c) => Some(parse_color(path, &ctx, c)?),
                    None => None,
                };
                if color.is_none() && role != "transparent" {
                    return Err(terr(path, format!("{}: missing `color`", ctx)));
                }
                let under = match d.get("under") {
                    Some(u) => Some(one_key(path, "`under`", u.as_str().ok_or_else(|| terr(path, format!("{}: `under` must be a key string", ctx)))?)?),
                    None => None,
                };
                if key == b'.' {
                    if role != "transparent" || color.is_some() {
                        return Err(terr(path, format!("{}: '.' is always transparent", ctx)));
                    }
                    continue;
                }
                for (i, l) in t.layers.iter().enumerate() {
                    if i != li && l.keys.iter().any(|k| k.key == key) {
                        return Err(terr(path, format!("{}: key already defined in layer '{}'", ctx, l.name)));
                    }
                }
                let kd = KeyDef { key, role, color, under };
                match t.layers[li].keys.iter().position(|k| k.key == key) {
                    Some(p) => t.layers[li].keys[p] = kd,
                    None => t.layers[li].keys.push(kd),
                }
            }
        }
    }
    // unique roles, `under` targets exist
    let all: Vec<&KeyDef> = t.keys().collect();
    for (i, a) in all.iter().enumerate() {
        if all[..i].iter().any(|b| b.role == a.role) {
            return Err(terr(path, format!("role '{}' is used by more than one key", a.role)));
        }
        if let Some(u) = a.under {
            if t.def(u).is_none() {
                return Err(terr(path, format!("key '{}' has under = '{}', which is not defined", a.key as char, u as char)));
            }
        }
    }
    if let Some(mv) = root.get("map") {
        let mt = mv.as_table().ok_or_else(|| terr(path, "`map` must be a table"))?;
        for (fs, v) in mt {
            let ctx = format!("[map] \"{}\"", fs);
            let from = color::parse_hex(fs).map_err(|e| terr(path, format!("{}: {}", ctx, e.msg)))?;
            let (key, near) = match v {
                toml::Value::String(s) => (one_key(path, "map key", s)?, None),
                toml::Value::Table(d) => {
                    check_keys(path, &ctx, d, &["key", "near"])?;
                    let k = d.get("key").and_then(|x| x.as_str()).ok_or_else(|| terr(path, format!("{}: missing `key`", ctx)))?;
                    let near = match d.get("near") {
                        Some(toml::Value::Array(a)) if a.len() == 2 => {
                            let c = parse_color(path, &ctx, &a[0])?;
                            let r = a[1].as_integer().filter(|r| *r >= 0).ok_or_else(|| terr(path, format!("{}: near radius must be a non-negative integer", ctx)))?;
                            Some((c, r as u32))
                        }
                        Some(_) => return Err(terr(path, format!("{}: `near` must be [\"#rrggbb\", radius]", ctx))),
                        None => None,
                    };
                    (one_key(path, "map key", k)?, near)
                }
                _ => return Err(terr(path, format!("{}: expected a key or {{ key = ..., near = [...] }}", ctx))),
            };
            if t.def(key).is_none() {
                return Err(terr(path, format!("{}: maps to key '{}', which is not defined in any layer", ctx, key as char)));
            }
            let rule = MapRule { from, key, near };
            match t.map.iter().position(|m| m.from == from) {
                Some(p) => t.map[p] = rule,
                None => t.map.push(rule),
            }
        }
    }
    if let Some(pv) = root.get("palettes") {
        let pt = pv.as_table().ok_or_else(|| terr(path, "`palettes` must be a table: [palettes] grass = \"abcd\""))?;
        for (name, v) in pt {
            let s = v.as_str().ok_or_else(|| terr(path, format!("[palettes] {}: expected a string of up to 4 keys like \"abcd\"", name)))?;
            let mut keys = Vec::new();
            for ch in s.chars() {
                let k = one_key(path, &format!("[palettes] {} key", name), &ch.to_string())?;
                if t.def(k).is_none() || k == b'.' {
                    return Err(terr(path, format!("[palettes] {}: key '{}' is not a coloured key of this template", name, ch)));
                }
                if keys.contains(&k) {
                    return Err(terr(path, format!("[palettes] {}: key '{}' appears twice", name, ch)));
                }
                keys.push(k);
            }
            if keys.is_empty() || keys.len() > 4 {
                return Err(terr(path, format!("[palettes] {}: a palette has 1 to 4 keys, got {}", name, keys.len())));
            }
            let pal = TilePalette { name: name.clone(), keys };
            match t.palettes.iter().position(|p| &p.name == name) {
                Some(i) => t.palettes[i] = pal,
                None => t.palettes.push(pal),
            }
        }
    }
    if let Some(tv) = root.get("tiles") {
        let tt = tv.as_table().ok_or_else(|| terr(path, "`tiles` must be a table: [tiles] air = [\"a\"]"))?;
        check_keys(path, "[tiles]", tt, &["air"])?;
        if let Some(av) = tt.get("air") {
            let items: Vec<&str> = match av {
                toml::Value::String(s) => vec![s.as_str()],
                toml::Value::Array(a) => a.iter().map(|x| x.as_str().unwrap_or("")).collect(),
                _ => return Err(terr(path, "[tiles] air must be a key or a list of keys")),
            };
            t.tile_air.clear();
            for s in items {
                let k = one_key(path, "[tiles] air key", s)?;
                if t.def(k).is_none() || k == b'.' {
                    return Err(terr(path, format!("[tiles] air: key '{}' is not a coloured key of this template", s)));
                }
                t.tile_air.push(k);
            }
        }
    }
    if t.palettes.len() > 8 {
        return Err(terr(path, format!("[palettes] has {} palettes, the hardware has 8", t.palettes.len())));
    }
    if !t.palettes.is_empty() {
        let mut seen: Vec<(Rgb, u8)> = Vec::new();
        for p in &t.palettes {
            for &k in &p.keys {
                let c = t.color_of(k).unwrap_or_default();
                match seen.iter().find(|(sc, sk)| *sc == c && *sk != k) {
                    Some((_, sk)) => {
                        return Err(terr(path, format!("keys '{}' and '{}' have the same colour {}; with [palettes] one RGB is one key", *sk as char, k as char, color::to_hex(c)))
                            .with_hint("give the colour a single key and use it in every palette that needs it"))
                    }
                    None => {
                        if !seen.iter().any(|(_, sk)| *sk == k) {
                            seen.push((c, k));
                        }
                    }
                }
            }
        }
        if let Some(&a) = t.tile_air.first() {
            if !t.palettes.iter().any(|p| p.keys.contains(&a)) {
                return Err(terr(path, format!("[tiles] air key '{}' is in no palette", a as char)));
            }
        }
    }
    if let Some(fv) = root.get("features") {
        let ft = fv.as_table().ok_or_else(|| terr(path, "`features` must be a table"))?;
        check_keys(path, "[features]", ft, &["eye"])?;
        if let Some(ev) = ft.get("eye") {
            let et = ev.as_table().ok_or_else(|| terr(path, "[features] eye = { white = \"W\", pupil = \"1\" }"))?;
            check_keys(path, "[features] eye", et, &["white", "pupil"])?;
            for (name, slot) in [("white", &mut t.features.eye_white), ("pupil", &mut t.features.eye_pupil)] {
                if let Some(v) = et.get(name) {
                    let s = v.as_str().ok_or_else(|| terr(path, format!("[features] eye.{} must be a key or role string", name)))?;
                    let key = match s.as_bytes() {
                        [b] if t.layers.iter().any(|l| l.keys.iter().any(|k| k.key == *b)) => *b,
                        _ => t.layers.iter().flat_map(|l| l.keys.iter()).find(|k| k.role == s).map(|k| k.key).ok_or_else(|| terr(path, format!("[features] eye.{} '{}' is not a key or role of this template", name, s)))?,
                    };
                    *slot = Some(key);
                }
            }
        }
    }
    Ok(t)
}

/// Load a template (following `extends`).
pub fn load(path: &Path) -> Result<Template> {
    load_inner(path, 0)
}

/// Locate the template named by a part (`template = "templates/player.toml"`): relative to the part's
/// folder, then each parent folder.
pub fn locate(part: &Part) -> Result<PathBuf> {
    let rel = part.template.as_deref().ok_or_else(|| {
        PxlError::validation("part has no `template`").with_loc(Loc::file(part.path.slash())).with_hint("add template = \"templates/<name>.toml\"")
    })?;
    fsutil::find_up(&part.dir(), rel).ok_or_else(|| {
        PxlError::validation(format!("template '{}' not found", rel))
            .with_loc(Loc::file(part.path.slash()))
            .with_hint("the path is tried relative to the part and each parent folder")
    })
}

/// Load the template of a part.
pub fn load_for(part: &Part) -> Result<Template> {
    load(&locate(part)?)
}

/// AC-TP2: every key used in any frame must be defined in the template. The error names the file, frame,
/// row and column of the first offending pixel and counts the rest.
pub fn validate_grid(t: &Template, part: &Part) -> Result<()> {
    validate_grid_skipping(t, part, &[])
}

/// Like `validate_grid`, but frames whose index is in `skip` (about to be regenerated) are not checked.
/// The error lists frame, key and pixel count of every unknown key and names `pxl template remap`.
pub fn validate_grid_skipping(t: &Template, part: &Part, skip: &[usize]) -> Result<()> {
    let mut first: Option<(usize, usize, usize, u8)> = None;
    let mut counts: Vec<(usize, u8, usize)> = Vec::new();
    for (fi, f) in part.frames.iter().enumerate() {
        if skip.contains(&fi) {
            continue;
        }
        for y in 0..f.grid.h {
            for x in 0..f.grid.w {
                let k = f.grid.cells[y * f.grid.w + x];
                if t.def(k).is_none() {
                    if first.is_none() {
                        first = Some((fi, y, x, k));
                    }
                    match counts.iter_mut().find(|c| c.0 == fi && c.1 == k) {
                        Some(c) => c.2 += 1,
                        None => counts.push((fi, k, 1)),
                    }
                }
            }
        }
    }
    if let Some((fi, y, x, k)) = first {
        let f = &part.frames[fi];
        let total: usize = counts.iter().map(|c| c.2).sum();
        let mut list: Vec<String> = counts.iter().take(8).map(|c| format!("{} '{}' x{}", part.frames[c.0].name, c.1 as char, c.2)).collect();
        if counts.len() > 8 {
            list.push(format!("... {} more", counts.len() - 8));
        }
        let mut olds: Vec<u8> = counts.iter().map(|c| c.1).collect();
        olds.sort_unstable();
        olds.dedup();
        let maps: Vec<String> = olds.iter().map(|&c| format!("{}=<new key>", c as char)).collect();
        return Err(PxlError::validation(format!(
            "key '{}' is not defined in template '{}'{} [{}]",
            k as char,
            t.name,
            if total > 1 { format!(" ({} more pixels use keys missing from the template)", total - 1) } else { String::new() },
            list.join(", ")
        ))
        .with_loc(Loc::file(part.path.slash()).frame(f.name.clone()).at(y, x))
        .with_hint(format!(
            "template changed? rewrite the stored keys: pxl template remap {} {}   # or add \"{}\" to {} or replace the pixel: pxl set {} {} {},{}=<key>",
            part.name,
            maps.join(" "),
            k as char,
            t.path.slash(),
            part.name,
            f.name,
            x,
            y
        )));
    }
    Ok(())
}

fn resolve_template_arg(arg: &str) -> Result<PathBuf> {
    let p = Path::new(arg);
    if p.is_file() {
        return Ok(p.to_path_buf());
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    for cand in [arg.to_string(), format!("templates/{}.toml", arg), format!("{}.toml", arg)] {
        if let Some(f) = fsutil::find_up(&cwd, &cand) {
            if f.is_file() {
                return Ok(f);
            }
        }
    }
    Err(PxlError::usage(format!("template '{}' not found", arg)).with_hint("give a path such as templates/player.toml"))
}

/// `pxl template set <template> <key|role>=<#rrggbb>...`: change colours in the template file itself
/// (coordinator operation). Only the `color` text changes; comments and layout are kept.
pub fn run_set(a: &crate::cli::TemplateSetArgs, out: &mut Out) -> Result<()> {
    let path = resolve_template_arg(&a.template)?;
    let tpl = load(&path)?;
    let mut text = fsutil::read_string(&path)?;
    let mut changes = Vec::new();
    for asg in &a.assigns {
        let (k, c) = asg.split_once('=').ok_or_else(|| PxlError::usage(format!("'{}' is not KEY=#rrggbb", asg)))?;
        let rgb = color::parse_hex(c).map_err(|e| PxlError::usage(e.msg).with_hint("use #rrggbb, e.g. #4850b8"))?;
        let key = tpl.resolve(k).filter(|b| *b != b'.').ok_or_else(|| {
            PxlError::usage(format!("template '{}' has no key or role '{}'", tpl.name, k))
        })?;
        let mut done = false;
        let mut in_layers = false;
        let mut lines: Vec<String> = text.split('\n').map(|s| s.to_string()).collect();
        for line in lines.iter_mut() {
            let t = line.trim_start();
            if t.starts_with('[') {
                in_layers = t.starts_with("[layers.");
                continue;
            }
            if !in_layers || done {
                continue;
            }
            let kq = format!("\"{}\"", key as char);
            let kb = format!("'{}'", key as char);
            if !(t.starts_with(&kq) || t.starts_with(&kb)) || !t[kq.len()..].trim_start().starts_with('=') {
                continue;
            }
            if let Some(ci) = line.find("color") {
                if let Some(q1) = line[ci..].find('"') {
                    let s = ci + q1 + 1;
                    if let Some(q2) = line[s..].find('"') {
                        line.replace_range(s..s + q2, &color::to_hex(rgb));
                        done = true;
                    }
                }
            }
        }
        if !done {
            return Err(PxlError::validation(format!("key '{}' is not defined in {} itself (inherited via extends?)", key as char, path.slash()))
                .with_hint("run `template set` on the template that defines it"));
        }
        text = lines.join("\n");
        changes.push((key as char, rgb));
    }
    load_check(&text, &path)?;
    fsutil::atomic_write(&path, text.as_bytes())?;
    let list: Vec<_> = changes.iter().map(|(k, c)| json!({"key": k.to_string(), "color": color::to_hex(*c)})).collect();
    for (k, c) in &changes {
        out.line(format!("{}: {} -> {}", path.slash(), k, color::to_hex(*c)));
    }
    out.set("template", json!(path.slash()));
    out.set("changed", json!(list));
    Ok(())
}

fn load_check(text: &str, path: &Path) -> Result<()> {
    // write to a sibling temp file so `extends` resolves the same way, then verify it loads
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let tmp = dir.join(format!(".{}.check.{}", path.file_name().and_then(|n| n.to_str()).unwrap_or("t"), std::process::id()));
    std::fs::write(&tmp, text)?;
    let r = load(&tmp);
    let _ = std::fs::remove_file(&tmp);
    r.map(|_| ())
}

/// Insert `"k" = { role = ".." , color = ".." }` lines as keys of `[layers.set]`: into the existing table where it is in the
/// file (right after its header, wherever that is), else as a new table at the end. Every other byte is kept.
pub fn insert_set_keys(text: &str, entries: &[(u8, Rgb)]) -> String {
    if entries.is_empty() {
        return text.to_string();
    }
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let body: String = entries.iter().map(|(k, c)| format!("\"{}\" = {{ role = \"set_{}\", color = \"{}\" }}{}", *k as char, color::to_hex(*c).trim_start_matches('#'), color::to_hex(*c), eol)).collect();
    let mut off = 0;
    for line in text.split_inclusive('\n') {
        if line.trim() == "[layers.set]" {
            let at = off + line.len();
            let mut t = text.to_string();
            if !line.ends_with('\n') {
                t.insert_str(at, eol);
                t.insert_str(at + eol.len(), &body);
            } else {
                t.insert_str(at, &body);
            }
            return t;
        }
        off += line.len();
    }
    let mut t = text.to_string();
    if !t.ends_with('\n') {
        t.push_str(eol);
    }
    t.push_str(&format!("{}# keys added from palette sets (one key per set colour)", eol));
    t.push_str(eol);
    t.push_str("[layers.set]");
    t.push_str(eol);
    t.push_str(&body);
    t
}

/// `pxl template add-set TEMPLATE --set NAME [--set NAME2]`: every colour of the named palette sets (following `include`, all slot-7
/// variants) as keys of `[layers.set]`, skipping colours the template has, with key characters that do not collide.
pub fn run_add_set(a: &crate::cli::AddSetArgs, out: &mut Out) -> Result<()> {
    let path = resolve_template_arg(&a.template)?;
    let tpl = load(&path)?;
    let text = fsutil::read_string(&path)?;
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).map(|d| d.to_path_buf()).unwrap_or_else(|| PathBuf::from("."));
    let mut cols: Vec<Rgb> = Vec::new();
    for n in &a.sets {
        let set = crate::cells::load_set(n, &dir)?;
        for c in crate::cells::all_colours(&set) {
            if !cols.contains(&c) {
                cols.push(c);
            }
        }
    }
    let dummy = Part::new("x", (1, 1));
    let want: std::collections::BTreeSet<Rgb> = cols.iter().copied().collect();
    let (_, _, added) = crate::fit::ensure_keys(&dummy, &tpl, &want, 1)?;
    let new_text = insert_set_keys(&text, &added);
    if !added.is_empty() {
        load_check(&new_text, &path)?;
        fsutil::atomic_write(&path, new_text.as_bytes())?;
    }
    out.line(format!("{}: {} set colour(s), {} already keys, {} added to [layers.set]{}", path.slash(), want.len(), want.len() - added.len(), added.len(), if added.is_empty() { String::new() } else { format!(": {}", added.iter().map(|(k, c)| format!("{}={}", *k as char, color::to_hex(*c))).collect::<Vec<_>>().join(" ")) }));
    out.set("template", json!(path.slash()));
    out.set("added", json!(added.iter().map(|(k, c)| json!({"key": (*k as char).to_string(), "color": color::to_hex(*c)})).collect::<Vec<_>>()));
    out.set("colors", json!(want.len()));
    Ok(())
}
