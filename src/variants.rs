//! Variants of an asset (M6 R4): `asset.pxl` is the variant `main`; further variants are `asset.<name>.pxl` beside it
//! (same input/ and template). `variants.toml` records which one is `delivered`. The delivered variant exports to the
//! asset's delivery path; the others export under `export/variants/<asset>/<name>/` (project) and are exempt from the
//! scene budgets. `variants.png` shows them side by side.

use crate::fsutil::PathSlash;
use crate::asset;
use crate::cli::VariantCmd;
use crate::compare;
use crate::img::{Rgba, RgbaImage};
use crate::manifest::{Manifest, OutSpec};
use crate::output::{Out, PxlError, Result};
use crate::{fsutil, pxl, versions};
use serde_json::json;
use std::path::{Path, PathBuf};

fn file_of(dir: &Path, v: &str) -> PathBuf {
    if v == "main" {
        dir.join(asset::ASSET_FILE)
    } else {
        dir.join(format!("asset.{}.pxl", v))
    }
}

/// Variant names of an asset folder, `main` first.
pub fn names(dir: &Path) -> Vec<String> {
    let mut v = vec!["main".to_string()];
    let mut others: Vec<String> = std::fs::read_dir(dir)
        .map(|r| r.flatten().filter_map(|e| asset::variant_of(&e.path()).filter(|_| e.path().is_file())).collect())
        .unwrap_or_default();
    others.sort();
    v.extend(others);
    v
}

pub fn delivered(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("variants.toml"))
        .ok()
        .and_then(|t| t.parse::<toml::Table>().ok())
        .and_then(|t| t.get("delivered").and_then(|d| d.as_str().map(String::from)))
        .filter(|d| file_of(dir, d).is_file())
        .unwrap_or_else(|| "main".into())
}

fn write_toml(dir: &Path, delivered: &str) -> Result<()> {
    let list = names(dir).iter().map(|n| asset::q(n)).collect::<Vec<_>>().join(", ");
    fsutil::atomic_write_plain(&dir.join("variants.toml"), format!("# Variants of this asset; exactly one is delivered. `pxl variant deliver ASSET NAME` switches.\ndelivered = {}\nvariants = [{}]\n", asset::q(delivered), list).as_bytes())
}

/// `variants.png`: every variant's latest art side by side, the delivered one starred. No file for an asset with one variant.
pub fn refresh_sheet(dir: &Path) {
    let ns = names(dir);
    if ns.len() < 2 {
        return;
    }
    let del = delivered(dir);
    let mut panels: Vec<(String, RgbaImage)> = Vec::new();
    for n in &ns {
        if let Ok(p) = pxl::load(&file_of(dir, n)) {
            panels.push((format!("{}{}", n, if *n == del { " *" } else { "" }), versions::render_sheet(&p)));
        }
    }
    let s = panels.iter().map(|(_, im)| (192 / im.h.max(1)).clamp(1, 8)).min().unwrap_or(1);
    let h = panels.iter().map(|(_, im)| im.h * s).max().unwrap_or(1);
    let w: usize = panels.iter().map(|(l, im)| (im.w * s).max(compare::text_w(l, 1) + 4) + 8).sum::<usize>() + 8;
    let mut cv = RgbaImage::filled(w, h + 18, Rgba::opaque(0x30, 0x30, 0x34));
    let mut x = 8;
    for (label, im) in &panels {
        let big = im.upscale(s);
        let mut chk = compare::checker(big.w, big.h, 4);
        for (i, p) in big.px.iter().enumerate() {
            if p.a > 0 {
                chk.px[i] = Rgba::opaque(p.r, p.g, p.b);
            }
        }
        compare::draw_text(&mut cv, x, 4, label, 1, Rgba::opaque(0xf0, 0xf0, 0xf0));
        compare::paste(&mut cv, &chk, x, 16);
        x += (big.w).max(compare::text_w(label, 1) + 4) + 8;
    }
    let _ = fsutil::atomic_write_plain(&dir.join("variants.png"), &compare::encode_sheet_png(&cv));
}

/// `key = value` (raw TOML text) in the top-level header of a `.pxl` text, or removed with `None`.
fn set_header(text: &str, key: &str, value: Option<&str>) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let end = lines.iter().position(|l| l.trim_start().starts_with('[')).unwrap_or(lines.len());
    let idx = (0..end).find(|&i| lines[i].trim_start().starts_with(key) && lines[i].trim_start()[key.len()..].trim_start().starts_with('='));
    let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    match (idx, value) {
        (Some(i), Some(v)) => out[i] = format!("{} = {}", key, v),
        (Some(i), None) => {
            out.remove(i);
        }
        (None, Some(v)) => {
            // after the `size`/`origin` lines, i.e. at the end of the header
            let mut at = end;
            while at > 0 && lines[at - 1].trim().is_empty() {
                at -= 1;
            }
            out.insert(at, format!("{} = {}", key, v));
        }
        (None, None) => {}
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

fn set_exempt(file: &Path, reason: Option<&str>) -> Result<()> {
    let t = fsutil::read_string(file)?;
    let new = set_header(&t, "budget_exempt", reason.map(asset::q).as_deref());
    if new != t {
        versions::suppressed(|| fsutil::atomic_write(file, new.as_bytes()))?;
    }
    Ok(())
}

pub fn run(c: &VariantCmd, out: &mut Out) -> Result<()> {
    match c {
        VariantCmd::Add { asset: a, name } => {
            let main = fsutil::resolve_part(a)?;
            let dir = asset::dir_of(&main);
            if !asset::is_asset_file(&main) {
                return Err(PxlError::usage("variants belong to an asset folder"));
            }
            if name == "main" || name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                return Err(PxlError::usage("a variant name is letters, digits, _ or - (and not 'main')"));
            }
            let f = file_of(&dir, name);
            if f.exists() {
                return Err(PxlError::usage(format!("variant '{}' exists", name)));
            }
            let t = fsutil::read_string(&dir.join(asset::ASSET_FILE))?;
            fsutil::atomic_write(&f, t.as_bytes())?;
            let del = delivered(&dir);
            write_toml(&dir, &del)?;
            refresh_sheet(&dir);
            out.emit(format!("variant {} of {} created from main ({} is delivered)", name, asset::asset_name(&dir), del), json!({"asset": asset::asset_name(&dir), "variant": name, "delivered": del}));
        }
        VariantCmd::List { asset: a } => {
            let dir = asset::dir_of(&fsutil::resolve_part(a)?);
            let del = delivered(&dir);
            let ns = names(&dir);
            let mut text = String::new();
            for n in &ns {
                text.push_str(&format!("{}{}\n", n, if *n == del { "  (delivered)" } else { "" }));
            }
            text.push_str(&format!("{} variant(s)", ns.len()));
            out.emit(text, json!({"asset": asset::asset_name(&dir), "variants": ns, "delivered": del}));
        }
        VariantCmd::Deliver { asset: a, name } => {
            let dir = asset::dir_of(&fsutil::resolve_part(a)?);
            let ns = names(&dir);
            if !ns.contains(name) {
                return Err(PxlError::usage(format!("{} has no variant '{}'", asset::asset_name(&dir), name)).with_hint(format!("variants: {}", ns.join(", "))));
            }
            let aname = asset::asset_name(&dir);
            write_toml(&dir, name)?;
            let mut notes = Vec::new();
            let mp = Manifest::find_file(&dir);
            let mut exports: Vec<(String, PathBuf, OutSpec)> = Vec::new();
            if let Some(mp) = &mp {
                let project = asset::dir_of(mp);
                crate::project::manifest_lease(mp, out)?;
                let mut text = fsutil::read_string(mp)?;
                let rel = |f: &Path| f.strip_prefix(&project).map(|r| r.to_string_lossy().replace('\\', "/")).unwrap_or_else(|_| f.slash());
                let group = dir.parent().and_then(|p| p.strip_prefix(project.join("assets")).ok()).map(|g| g.to_string_lossy().replace('\\', "/")).unwrap_or_default();
                let outp = if group.is_empty() { aname.clone() } else { format!("{}/{}", group, aname) };
                // the delivered variant is the part `aname`
                text = if asset::has_part_entry(&text, &aname) { asset::set_part_key(&text, &aname, "path", &asset::q(&rel(&file_of(&dir, name)))) } else { asset::add_part_entry(&text, &aname, &rel(&file_of(&dir, name)), &outp, &[]) };
                for n in ns.iter().filter(|n| *n != name) {
                    let key = format!("{}__{}", aname, n);
                    let vout = format!("variants/{}/{}", aname, n);
                    text = if asset::has_part_entry(&text, &key) { asset::set_part_key(&text, &key, "path", &asset::q(&rel(&file_of(&dir, n)))) } else { asset::add_part_entry(&text, &key, &rel(&file_of(&dir, n)), &vout, &["variant".to_string()]) };
                }
                fsutil::atomic_write(mp, text.as_bytes())?;
                set_exempt(&file_of(&dir, name), None)?;
                for n in ns.iter().filter(|n| *n != name) {
                    set_exempt(&file_of(&dir, n), Some(&format!("non-delivered variant of {}", aname)))?;
                }
                notes.push(format!("pxl.toml: part {} -> {}", aname, rel(&file_of(&dir, name))));
                if let Ok(m) = Manifest::load(mp) {
                    for d in &m.parts {
                        if d.name == aname || d.name.starts_with(&format!("{}__", aname)) {
                            exports.push((d.name.clone(), d.path.clone(), d.spec.clone()));
                        }
                    }
                }
            } else {
                let base = OutSpec::default_for(&dir.join(asset::ASSET_FILE));
                for n in &ns {
                    let spec = if n == name { base.clone() } else { base.with_dir(&dir.join("export").join("variants").join(n), &format!("{}.{}", aname, n)) };
                    exports.push((n.clone(), file_of(&dir, n), spec));
                }
            }
            for (n, path, spec) in &exports {
                let job = crate::export::Job { part_path: path.clone(), spec: spec.clone(), frames: Vec::new(), agent: out.agent().map(String::from), scratch: false };
                match crate::export::export_part(&job) {
                    Ok(r) => notes.push(format!("exported {}: {} file(s)", n, r.files)),
                    Err(e) => notes.push(format!("export {} skipped: {}", n, e.msg.lines().next().unwrap_or(""))),
                }
            }
            refresh_sheet(&dir);
            crate::tasks::write_page(&dir);
            out.emit(format!("{}: {} is delivered\n{}", aname, name, notes.join("\n")).trim_end(), json!({"asset": aname, "delivered": name, "notes": notes}));
        }
    }
    Ok(())
}
