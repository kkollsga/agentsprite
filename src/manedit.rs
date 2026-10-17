//! Manifest commands (M6 R4): agents never hand-edit pxl.toml or the palette files.
//! `pxl scene set SCENE KEY VALUE`, `pxl scene add-part SCENE GROUP PART`, `pxl scene remove-part SCENE PART`,
//! `pxl palette slot set SET SLOT #rrggbb ...`. Each takes the lease of the file (another agent's live lease refuses) and
//! edits the text in place, so comments and layout survive; a result that does not parse is never written.

use crate::cli::{AssetCmd, SceneArgs, SlotCmd};
use crate::manifest::Manifest;
use crate::output::{Out, PxlError, Result};
use crate::{asset, cells, fsutil, lease};
use serde_json::json;
use std::path::Path;

fn header(line: &str) -> Option<&str> {
    let t = line.trim();
    t.strip_prefix('[').and_then(|r| r.strip_suffix(']')).filter(|r| !r.starts_with('['))
}

fn block(lines: &[String], name: &str) -> Option<(usize, usize)> {
    let s = lines.iter().position(|l| header(l) == Some(name))?;
    let e = lines.iter().enumerate().skip(s + 1).find(|(_, l)| header(l).is_some() || l.trim().starts_with("[[")).map_or(lines.len(), |(i, _)| i);
    Some((s, e))
}

/// Raw TOML text of a CLI value: integers, booleans and `[..]` lists stay, anything else is a string.
fn raw_value(v: &str) -> String {
    let t = v.trim();
    if t.parse::<i64>().is_ok() || t == "true" || t == "false" || (t.starts_with('[') && t.ends_with(']')) {
        t.to_string()
    } else {
        asset::q(t)
    }
}

/// Set `key = value` in the table `[name]`.
pub fn set_table_key(text: &str, name: &str, key: &str, value: &str) -> Option<String> {
    let mut l: Vec<String> = text.lines().map(String::from).collect();
    let (s, e) = block(&l, name)?;
    let line = format!("{} = {}", key, value);
    if let Some(i) = (s + 1..e).find(|&i| l[i].trim_start().starts_with(key) && l[i].trim_start()[key.len()..].trim_start().starts_with('=')) {
        l[i] = line;
    } else {
        let mut at = e;
        while at > s + 1 && (l[at - 1].trim().is_empty() || l[at - 1].trim_start().starts_with('#')) {
            at -= 1;
        }
        l.insert(at, line);
    }
    let mut r = l.join("\n");
    r.push('\n');
    Some(r)
}

/// Parts list of a group table: (line range of the `parts = [...]` entry, items).
fn parts_entry(l: &[String], s: usize, e: usize) -> Option<(usize, usize, Vec<String>)> {
    let i = (s + 1..e).find(|&i| l[i].trim_start().starts_with("parts") && l[i].trim_start()["parts".len()..].trim_start().starts_with('='))?;
    let mut j = i;
    let mut buf = l[i].clone();
    while buf.matches('[').count() > buf.matches(']').count() && j + 1 < e {
        j += 1;
        buf.push('\n');
        buf.push_str(&l[j]);
    }
    let t: toml::Table = buf.parse().ok()?;
    let items = t.get("parts")?.as_array()?.iter().filter_map(|x| x.as_str().map(String::from)).collect();
    Some((i, j, items))
}

pub fn edit_parts(text: &str, scene: &str, group: Option<&str>, part: &str, add: bool) -> std::result::Result<String, String> {
    let mut l: Vec<String> = text.lines().map(String::from).collect();
    let groups: Vec<String> = l.iter().filter_map(|x| header(x).and_then(|h| h.strip_prefix(&format!("scene.{}.groups.", scene))).map(String::from)).collect();
    if block(&l, &format!("scene.{}", scene)).is_none() {
        return Err(format!("no scene '{}'", scene));
    }
    let targets: Vec<String> = match (group, add) {
        (Some(g), _) => vec![g.to_string()],
        (None, _) => groups.clone(),
    };
    let mut changed = 0;
    for g in targets {
        let name = format!("scene.{}.groups.{}", scene, g);
        let Some((s, e)) = block(&l, &name) else { return Err(format!("scene {} has no group '{}' (groups: {})", scene, g, groups.join(", "))) };
        let Some((i, j, mut items)) = parts_entry(&l, s, e) else {
            if add {
                l.insert(e, format!("parts = [{}]", asset::q(part)));
                changed += 1;
                continue;
            }
            continue;
        };
        let had = items.iter().any(|x| x == part);
        if add && !had {
            items.push(part.to_string());
        } else if !add && had {
            items.retain(|x| x != part);
        } else {
            continue;
        }
        let line = format!("parts = [{}]", items.iter().map(|x| asset::q(x)).collect::<Vec<_>>().join(", "));
        l.splice(i..=j, [line]);
        changed += 1;
    }
    if changed == 0 {
        return Ok(text.to_string());
    }
    let mut r = l.join("\n");
    r.push('\n');
    Ok(r)
}

fn manifest_path() -> Result<std::path::PathBuf> {
    let cwd = std::env::current_dir().map_err(|e| PxlError::io(e.to_string()))?;
    Manifest::find_file(&cwd).ok_or_else(|| PxlError::usage("this needs a pxl.toml"))
}

pub fn run_scene_edit(a: &SceneArgs, out: &mut Out) -> Result<()> {
    let mp = manifest_path()?;
    lease::guard(&mp, out.agent())?;
    let text = fsutil::read_string(&mp)?;
    let usage = |m: &str| PxlError::usage(m.to_string());
    let new = match a.name.as_str() {
        "set" if a.rest.len() == 3 && a.rest[1].starts_with("group.") => {
            // pxl scene set SCENE group.GROUP.budget N | group.GROUP.per_frame N | group.GROUP.streamed true|false
            let (scene, key, value) = (&a.rest[0], &a.rest[1], &a.rest[2]);
            let parts: Vec<&str> = key.splitn(3, '.').collect();
            let [_, group, field] = parts.as_slice() else { return Err(usage("pxl scene set SCENE group.GROUP.budget N")) };
            if !["budget", "per_frame", "streamed"].contains(field) {
                return Err(usage(&format!("group key '{}' cannot be set; keys: budget, per_frame, streamed", field)));
            }
            if matches!(*field, "budget" | "per_frame") && value.parse::<u32>().is_err() {
                return Err(usage(&format!("{} must be a whole number of sprite cells, got '{}'", field, value)));
            }
            if *field == "streamed" && !["true", "false"].contains(&value.as_str()) {
                return Err(usage("streamed is true or false"));
            }
            let table = format!("scene.{}.groups.{}", scene, group);
            let groups: Vec<String> = text.lines().filter_map(|x| header(x).and_then(|h| h.strip_prefix(&format!("scene.{}.groups.", scene))).map(String::from)).collect();
            if !text.lines().any(|x| header(x) == Some(table.as_str())) {
                return Err(usage(&format!("scene '{}' has no group '{}'", scene, group)).with_hint(if groups.is_empty() { format!("no scene '{}' or it has no groups; groups come from `pxl plan --apply`", scene) } else { format!("groups: {}", groups.join(", ")) }));
            }
            set_table_key(&text, &table, field, &raw_value(value)).ok_or_else(|| usage("cannot edit that group"))?
        }
        "set" => {
            let [scene, key, value] = a.rest.as_slice() else { return Err(usage("pxl scene set SCENE KEY VALUE (or SCENE group.GROUP.budget N)")) };
            const KEYS: [&str; 7] = ["palette_set", "tileset", "resident_budget", "bg_reserved", "palettes_out", "window", "layout"];
            if !KEYS.contains(&key.as_str()) {
                return Err(usage(&format!("scene key '{}' cannot be set; keys: {}", key, KEYS.join(", "))));
            }
            set_table_key(&text, &format!("scene.{}", scene), key, &raw_value(value)).ok_or_else(|| usage(&format!("no scene '{}'", scene)))?
        }
        "add-part" | "remove-part" => {
            let add = a.name == "add-part";
            let (scene, group, part) = match (add, a.rest.as_slice()) {
                (true, [s, g, p]) => (s, Some(g.as_str()), p),
                (false, [s, p]) => (s, None, p),
                (false, [s, g, p]) => (s, Some(g.as_str()), p),
                _ => return Err(usage(if add { "pxl scene add-part SCENE GROUP PART" } else { "pxl scene remove-part SCENE PART" })),
            };
            let m = Manifest::load(&mp)?;
            if add && !m.parts.iter().any(|d| &d.name == part) && !crate::cellmem::resolve_parts(&m, part).iter().any(|_| true) {
                return Err(usage(&format!("'{}' is not a part of the manifest", part)).with_hint("assets are registered by `pxl ingest`"));
            }
            edit_parts(&text, scene, group, part, add).map_err(|e| usage(&e))?
        }
        _ => return Err(usage("unknown scene edit")),
    };
    if new == text {
        out.emit("nothing to change", json!({"changed": false}));
        return Ok(());
    }
    Manifest::parse(&new, &mp)?;
    fsutil::atomic_write(&mp, new.as_bytes())?;
    let m = Manifest::load(&mp)?;
    out.emit(format!("pxl.toml updated ({} {})", a.name, a.rest.join(" ")), json!({"changed": true, "scenes": m.scenes.len()}));
    Ok(())
}

pub fn run_slot(c: &SlotCmd, out: &mut Out) -> Result<()> {
    let (set, slot, colors) = match c {
        SlotCmd::Set { set, slot, colors } => (set, slot, colors),
        SlotCmd::Remove { set, slot } => return run_slot_remove(set, *slot, out),
    };
    if *slot > 7 {
        return Err(PxlError::usage("slot is 0..7"));
    }
    let cwd = std::env::current_dir().map_err(|e| PxlError::io(e.to_string()))?;
    let file = fsutil::find_up(&cwd, &format!("palettes/{}.toml", set)).ok_or_else(|| PxlError::usage(format!("palette set '{}' not found", set)))?;
    lease::guard(&file, out.agent())?;
    let text = fsutil::read_string(&file)?;
    let new = set_slot_text(&text, *slot, colors).map_err(PxlError::usage)?;
    // the result must still be a valid set
    cells::parse_set(set, &new, &file, 0)?;
    if new == text {
        out.emit("nothing to change", json!({"changed": false}));
        return Ok(());
    }
    fsutil::atomic_write(&file, new.as_bytes())?;
    out.emit(format!("{} slot {} = {}", set, slot, colors.join(" ")), json!({"set": set, "slot": slot, "colors": colors, "changed": true}));
    Ok(())
}

/// `pxl palette slot remove SET SLOT`: drop the `[[slots]]` block of that slot (an included slot of another file stays in that file).
fn run_slot_remove(set: &str, slot: u8, out: &mut Out) -> Result<()> {
    if slot > 7 {
        return Err(PxlError::usage("slot is 0..7"));
    }
    let cwd = std::env::current_dir().map_err(|e| PxlError::io(e.to_string()))?;
    let file = fsutil::find_up(&cwd, &format!("palettes/{}.toml", set)).ok_or_else(|| PxlError::usage(format!("palette set '{}' not found", set)))?;
    lease::guard(&file, out.agent())?;
    let text = fsutil::read_string(&file)?;
    let new = remove_slot_text(&text, slot).map_err(PxlError::usage)?;
    cells::parse_set(set, &new, &file, 0)?;
    fsutil::atomic_write(&file, new.as_bytes())?;
    out.emit(
        format!("{} slot {} removed; fit the parts that used it again (`pxl palette fit PART --set {} --apply`)", set, slot, set),
        json!({"set": set, "slot": slot, "removed": true, "changed": true}),
    );
    Ok(())
}

/// The text of a palette set without the `[[slots]]` block of `slot`.
pub fn remove_slot_text(text: &str, slot: u8) -> std::result::Result<String, String> {
    let l: Vec<String> = text.lines().map(String::from).collect();
    let starts: Vec<usize> = l.iter().enumerate().filter(|(_, x)| x.trim() == "[[slots]]").map(|(i, _)| i).collect();
    for (k, &s) in starts.iter().enumerate() {
        let e = starts.get(k + 1).copied().unwrap_or(l.len());
        let is_slot = (s + 1..e).any(|i| {
            let t = l[i].replace(' ', "");
            t == format!("slot={}", slot) || t.starts_with(&format!("slot={}#", slot))
        });
        if !is_slot {
            continue;
        }
        let mut keep: Vec<String> = l[..s].to_vec();
        // trailing comments / blank lines before the block belong to it only when blank
        while keep.last().map_or(false, |x| x.trim().is_empty()) {
            keep.pop();
        }
        keep.extend(l[e..].iter().cloned());
        let mut r = keep.join("\n");
        r.push('\n');
        return Ok(r);
    }
    Err(format!("the set has no own slot {} (a slot that comes from `include` is removed in the included file)", slot))
}

/// `pxl project set PATH VALUE`: a setting of pxl.toml by command (the file is never edited by hand): `defaults.*`, `settings.*`, `style.*`, `delivery.*`
/// or a `[project]` key. A result that does not parse is never written.
pub fn run_project_set(path: &str, value: &str, out: &mut Out) -> Result<()> {
    let mp = manifest_path()?;
    lease::guard(&mp, out.agent())?;
    let usage = |m: &str| PxlError::usage(m.to_string());
    let segs: Vec<&str> = path.split('.').collect();
    let (table, key): (String, String) = match segs.as_slice() {
        ["defaults", "group_categories", g] => ("project.defaults.group_categories".into(), g.to_string()),
        ["defaults", k @ ("mode" | "size" | "factor" | "category" | "palette_set" | "template")] => ("project.defaults".into(), k.to_string()),
        ["settings", k] => ("project.settings".into(), k.to_string()),
        ["style", k] => ("style".into(), k.to_string()),
        ["delivery", k] => ("delivery".into(), k.to_string()),
        [k @ ("name" | "strict" | "gates" | "manifest" | "plan")] => ("project".into(), k.to_string()),
        _ => {
            return Err(usage(&format!("cannot set '{}'", path)).with_hint("defaults.KEY (mode size factor category palette_set template) | defaults.group_categories.GROUP | settings.KEY | style.KEY | delivery.KEY | name | strict | gates | manifest | plan"))
        }
    };
    let raw = match (table.as_str(), key.as_str()) {
        ("project.defaults", "size") => {
            let (w, h) = value.split_once('x').and_then(|(w, h)| Some((w.trim().parse::<u32>().ok()?, h.trim().parse::<u32>().ok()?))).ok_or_else(|| usage("defaults.size is WxH, e.g. 32x32"))?;
            format!("[{}, {}]", w, h)
        }
        ("project.defaults", "category") | ("project.defaults.group_categories", _) => {
            if !crate::profile::CATEGORIES.contains(&value) {
                return Err(usage(&format!("unknown category '{}'", value)).with_hint(format!("categories: {}", crate::profile::CATEGORIES.join(", "))));
            }
            asset::q(value)
        }
        _ => raw_value(value),
    };
    let text = fsutil::read_string(&mp)?;
    let mut t = text.clone();
    if !t.lines().any(|l| header(l) == Some(table.as_str())) {
        if !t.ends_with('\n') {
            t.push('\n');
        }
        t.push_str(&format!("\n[{}]\n", table));
    }
    let new = set_table_key(&t, &table, &key, &raw).ok_or_else(|| usage("cannot edit that table"))?;
    Manifest::parse(&new, &mp)?;
    if new == text {
        out.emit("nothing to change", json!({"changed": false}));
        return Ok(());
    }
    fsutil::atomic_write(&mp, new.as_bytes())?;
    out.emit(format!("pxl.toml: {} = {}", path, raw), json!({"changed": true, "path": path, "value": raw}));
    Ok(())
}

/// `pxl asset set ASSET KEY VALUE` (also `pxl meta ASSET set category X`).
pub fn run_asset(c: &AssetCmd, out: &mut Out) -> Result<()> {
    let AssetCmd::Set { asset, key, value } = c;
    set_asset_key(asset, key, value, out)
}

pub fn set_asset_key(asset: &str, key: &str, value: &str, out: &mut Out) -> Result<()> {
    match key {
        "category" => set_category(asset, value, out),
        _ => Err(PxlError::usage(format!("asset key '{}' cannot be set here; keys: category", key))),
    }
}

/// Set the hardware budget category of an asset. `player` allows two-slot (stacked) cells (`multi_slot_max` per frame, 2 sprites each).
pub fn set_category(asset: &str, category: &str, out: &mut Out) -> Result<()> {
    let path = fsutil::resolve_part(asset)?;
    if !crate::profile::CATEGORIES.contains(&category) {
        return Err(PxlError::usage(format!("unknown category '{}'", category)).with_hint(format!("categories: {}", crate::profile::CATEGORIES.join(", "))));
    }
    lease::guard(&path, out.agent())?;
    let mut part = crate::pxl::load(&path)?;
    let mut probe = part.clone();
    probe.category = Some(category.to_string());
    probe.budget = Default::default();
    let prof = crate::profile::load_for(&probe, None)?;
    let b = prof.budgets.get(category).copied().ok_or_else(|| PxlError::validation(format!("the profile '{}' has no [budget.{}]", prof.name, category)))?;
    let was = part.category.clone();
    // the automatic hardware-wide limits set at ingest (no category) give way to the category's own
    let automatic = part.budget.per_line == Some(prof.hw_cells_per_row_max) && part.budget.per_frame == Some(prof.hw_cells_max);
    if automatic {
        part.budget = Default::default();
    }
    part.category = Some(category.to_string());
    if was.as_deref() == Some(category) && !automatic {
        out.emit(format!("{}: category is already {}", asset, category), json!({"asset": asset, "category": category, "changed": false}));
        return Ok(());
    }
    crate::versions::suppressed(|| crate::pxl::save(&path, &part))?;
    let multi = if category == "player" { prof.multi_slot_max } else { 0 };
    out.emit(
        format!(
            "{}: category {} -> {} ({} cells per line, {} per frame{}); `pxl palette fit {} --apply` folds the cells again with the new limits",
            asset,
            was.as_deref().unwrap_or("(none)"),
            category,
            b.per_line,
            b.per_frame,
            if multi > 0 { format!(", up to {} two-slot (stacked) cell(s) per frame, 2 sprites each", multi) } else { String::new() },
            asset
        ),
        json!({"asset": asset, "category": category, "was": was, "changed": true, "per_line": b.per_line, "per_frame": b.per_frame, "multi_slot_max": multi}),
    );
    Ok(())
}

fn hex_ok(c: &str) -> bool {
    c.len() == 7 && c.starts_with('#') && c[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn set_slot_text(text: &str, slot: u8, colors: &[String]) -> std::result::Result<String, String> {
    if colors.len() != 3 || !colors.iter().all(|c| hex_ok(c)) {
        return Err("give exactly three colours as #rrggbb".into());
    }
    let line = format!("colors = [{}]", colors.iter().map(|c| format!("\"{}\"", c.to_lowercase())).collect::<Vec<_>>().join(", "));
    let mut l: Vec<String> = text.lines().map(String::from).collect();
    let starts: Vec<usize> = l.iter().enumerate().filter(|(_, x)| x.trim() == "[[slots]]").map(|(i, _)| i).collect();
    for (k, &s) in starts.iter().enumerate() {
        let e = starts.get(k + 1).copied().unwrap_or(l.len());
        let is_slot = (s + 1..e).any(|i| {
            let t = l[i].replace(' ', "");
            t == format!("slot={}", slot) || t.starts_with(&format!("slot={}#", slot))
        });
        if !is_slot {
            continue;
        }
        if let Some(i) = (s + 1..e).find(|&i| l[i].trim_start().starts_with("colors")) {
            l[i] = line;
        } else {
            l.insert(e, line);
        }
        let mut r = l.join("\n");
        r.push('\n');
        return Ok(r);
    }
    let mut r = l.join("\n");
    if !r.ends_with('\n') {
        r.push('\n');
    }
    r.push_str(&format!("\n[[slots]]\nslot = {}\nname = \"slot_{}\"\n{}\n", slot, slot, line));
    Ok(r)
}

#[allow(dead_code)]
fn _unused(_: &Path) {}
