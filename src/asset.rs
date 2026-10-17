//! The asset layout (M6 R4): `asset.pxl` in a folder of its own, with `input/`, `versions/`, `compare.png`.
//!
//! * A **single-image asset** is a folder `<file>.pxl/` beside the picture, with the console in its own header.
//! * In a **project** the assets live below `assets/<group>/<name>/`; groups are the folders between. `pxl.toml` keeps
//!   one `[parts.<name>]` entry per asset (path, out, tags), so `build`, `check`, scenes and the queue work unchanged.
//! * Any command accepts the folder, `asset.pxl`, or (inside a project) the asset name.
//! * Variants of an asset are further files `asset.<variant>.pxl` in the same folder (see `variants`).

use crate::manifest::Manifest;
use std::path::{Path, PathBuf};

pub const ASSET_FILE: &str = "asset.pxl";

/// `asset.pxl` or `asset.<variant>.pxl`.
pub fn is_asset_file(p: &Path) -> bool {
    p.file_name().and_then(|n| n.to_str()).map_or(false, |n| n == ASSET_FILE || (n.starts_with("asset.") && n.ends_with(".pxl") && n.len() > "asset..pxl".len()))
}

/// The variant a file holds: `None` for `asset.pxl` (the main one).
pub fn variant_of(p: &Path) -> Option<String> {
    let n = p.file_name()?.to_str()?;
    let mid = n.strip_prefix("asset.")?.strip_suffix(".pxl")?;
    (!mid.is_empty()).then(|| mid.to_string())
}

pub fn dir_of(p: &Path) -> PathBuf {
    match p.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// Name of the asset a part file belongs to; for a plain part its file stem.
pub fn name_of(p: &Path) -> String {
    if is_asset_file(p) {
        let d = std::fs::canonicalize(dir_of(p)).unwrap_or_else(|_| dir_of(p));
        let n = d.file_name().and_then(|n| n.to_str()).unwrap_or("asset");
        let n = n.strip_suffix(".pxl").unwrap_or(n);
        match variant_of(p) {
            Some(v) => format!("{}.{}", n, v),
            None => n.to_string(),
        }
    } else {
        p.file_stem().and_then(|s| s.to_str()).unwrap_or("part").to_string()
    }
}

/// Folder name of the asset (variants share it).
pub fn asset_name(dir: &Path) -> String {
    let d = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let n = d.file_name().and_then(|n| n.to_str()).unwrap_or("asset");
    n.strip_suffix(".pxl").unwrap_or(n).to_string()
}

/// The directory of the project that holds `start` (nearest `pxl.toml`).
pub fn project_dir(start: &Path) -> Option<PathBuf> {
    Manifest::find_file(start).map(|m| dir_of(&m))
}

#[derive(Clone, Debug)]
pub struct AssetInfo {
    pub name: String,
    /// Path below `assets/` of the folder that holds the asset ("" = top level).
    pub group: String,
    pub dir: PathBuf,
    pub file: PathBuf,
}

/// Every asset below `<project>/assets/`, sorted by group and name.
pub fn scan(project: &Path) -> Vec<AssetInfo> {
    let root = project.join("assets");
    let mut out = Vec::new();
    walk(&root, &root, &mut out);
    out.sort_by(|a, b| (a.group.as_str(), a.name.as_str()).cmp(&(b.group.as_str(), b.name.as_str())));
    out
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<AssetInfo>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut es: Vec<_> = rd.flatten().collect();
    es.sort_by_key(|e| e.file_name());
    for e in es {
        let p = e.path();
        let n = e.file_name().to_string_lossy().into_owned();
        if !p.is_dir() || n.starts_with('.') {
            continue;
        }
        if p.join(ASSET_FILE).is_file() {
            let group = dir.strip_prefix(root).map(|g| g.to_string_lossy().replace('\\', "/")).unwrap_or_default();
            out.push(AssetInfo { name: n, group, file: p.join(ASSET_FILE), dir: p });
        } else {
            walk(root, &p, out);
        }
    }
}

/// Group folders (including empty ones) below `<project>/assets/`.
pub fn groups(project: &Path) -> Vec<String> {
    fn rec(root: &Path, dir: &Path, out: &mut Vec<String>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let mut es: Vec<_> = rd.flatten().collect();
        es.sort_by_key(|e| e.file_name());
        for e in es {
            let p = e.path();
            if p.is_dir() && !p.join(ASSET_FILE).is_file() && !e.file_name().to_string_lossy().starts_with('.') {
                out.push(p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"));
                rec(root, &p, out);
            }
        }
    }
    let root = project.join("assets");
    let mut v = Vec::new();
    rec(&root, &root, &mut v);
    v
}

/// `arg` as an asset: a folder with `asset.pxl`, or an asset name of the project around the current folder.
pub fn find(arg: &str) -> Option<PathBuf> {
    let p = Path::new(arg);
    if p.is_dir() && p.join(ASSET_FILE).is_file() {
        return Some(p.join(ASSET_FILE));
    }
    if p.extension().is_none() {
        let q = PathBuf::from(format!("{}.pxl", arg));
        if q.is_dir() && q.join(ASSET_FILE).is_file() {
            return Some(q.join(ASSET_FILE));
        }
    }
    if !arg.contains('/') {
        let cwd = std::env::current_dir().ok()?;
        let proj = project_dir(&cwd)?;
        return scan(&proj).into_iter().find(|a| a.name == arg).map(|a| a.file);
    }
    None
}

/// Display name for status and board lines: the asset name, else the part's file stem.
pub fn display_name(p: &Path) -> String {
    name_of(p)
}

// -------------------------------------------------------------------------- pxl.toml text edits

fn header_of(line: &str) -> Option<&str> {
    let t = line.trim();
    t.strip_prefix('[').and_then(|r| r.strip_suffix(']')).filter(|r| !r.starts_with('['))
}

/// Line range of the table `[parts.<name>]` (header line to just before the next header of another table).
fn part_block(lines: &[String], name: &str) -> Option<(usize, usize)> {
    let key = format!("parts.{}", name);
    let s = lines.iter().position(|l| header_of(l) == Some(key.as_str()))?;
    let e = lines.iter().enumerate().skip(s + 1).find(|(_, l)| header_of(l).is_some() || l.trim().starts_with("[[")).map_or(lines.len(), |(i, _)| i);
    Some((s, e))
}

fn to_lines(text: &str) -> Vec<String> {
    text.lines().map(String::from).collect()
}

fn from_lines(l: Vec<String>) -> String {
    let mut s = l.join("\n");
    s.push('\n');
    s
}

fn qs(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Add `[parts.<name>]` (no-op when present). Placed before the first `[scene` table so scenes stay at the end.
pub fn add_part_entry(text: &str, name: &str, path: &str, out: &str, tags: &[String]) -> String {
    let mut l = to_lines(text);
    if part_block(&l, name).is_some() {
        return text.to_string();
    }
    let mut block = vec![format!("[parts.{}]", name), format!("path = {}", qs(path)), format!("out = {}", qs(out))];
    if !tags.is_empty() {
        block.push(format!("tags = [{}]", tags.iter().map(|t| qs(t)).collect::<Vec<_>>().join(", ")));
    }
    block.push(String::new());
    if !l.is_empty() && !l.last().unwrap().is_empty() && !l.iter().any(|x| header_of(x).map_or(false, |h| h == "scene" || h.starts_with("scene."))) {
        l.push(String::new());
    }
    let at = l.iter().position(|x| header_of(x).map_or(false, |h| h == "scene" || h.starts_with("scene.") || h == "review")).unwrap_or(l.len());
    for (i, b) in block.into_iter().enumerate() {
        l.insert(at + i, b);
    }
    from_lines(l)
}

/// Set (or add) `key = value` (raw TOML value text) inside `[parts.<name>]`.
pub fn set_part_key(text: &str, name: &str, key: &str, value: &str) -> String {
    let mut l = to_lines(text);
    let Some((s, e)) = part_block(&l, name) else { return text.to_string() };
    let line = format!("{} = {}", key, value);
    if let Some(i) = (s + 1..e).find(|&i| l[i].trim_start().starts_with(key) && l[i].trim_start()[key.len()..].trim_start().starts_with('=')) {
        l[i] = line;
    } else {
        let mut at = e;
        while at > s + 1 && l[at - 1].trim().is_empty() {
            at -= 1;
        }
        l.insert(at, line);
    }
    from_lines(l)
}

pub fn remove_part_entry(text: &str, name: &str) -> String {
    let mut l = to_lines(text);
    let Some((s, e)) = part_block(&l, name) else { return text.to_string() };
    l.drain(s..e);
    from_lines(l)
}

pub fn has_part_entry(text: &str, name: &str) -> bool {
    part_block(&to_lines(text), name).is_some()
}

pub fn q(s: &str) -> String {
    qs(s)
}
