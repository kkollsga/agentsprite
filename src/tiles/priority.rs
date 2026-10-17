//! `pxl tiles priority`: the foreground priority mask of a tile set (M5 phase T3).
//!
//! The part lists the tiles drawn in front of sprites in `[priority] tiles = [..]`; `export` writes them as
//! `priority.png` (same size as blocks.png, white = over sprites) and lint rule `priority_color0` checks that the
//! see-through part (air) of such a tile is colour 0 of its palette.

use crate::cli::TilesArgs;
use crate::output::{Out, PxlError, Result};
use crate::{fsutil, lease, pxl, status};
use serde_json::json;

/// `3,7,10-12` -> sorted ids.
pub fn parse_ids(s: &str) -> Result<Vec<usize>> {
    let mut v = Vec::new();
    for tok in s.split(',').map(str::trim).filter(|t| !t.is_empty()) {
        let bad = || PxlError::usage(format!("'{}' is not a tile id or range", tok)).with_hint("ids look like 3,7,10-12");
        match tok.split_once('-') {
            Some((a, b)) => {
                let (a, b): (usize, usize) = (a.trim().parse().map_err(|_| bad())?, b.trim().parse().map_err(|_| bad())?);
                if b < a {
                    return Err(bad());
                }
                v.extend(a..=b);
            }
            None => v.push(tok.parse().map_err(|_| bad())?),
        }
    }
    v.sort_unstable();
    v.dedup();
    Ok(v)
}

fn fmt_ids(v: &[usize]) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut i = 0;
    while i < v.len() {
        let mut j = i;
        while j + 1 < v.len() && v[j + 1] == v[j] + 1 {
            j += 1;
        }
        parts.push(if j > i { format!("{}-{}", v[i], v[j]) } else { v[i].to_string() });
        i = j + 1;
    }
    parts.join(",")
}

pub fn run(a: &TilesArgs, out: &mut Out) -> Result<()> {
    let path = fsutil::resolve_part(&a.tileset)?;
    let mut part = pxl::load(&path)?;
    let mut cur: Vec<usize> = part.priority.clone().unwrap_or_default();
    let before = cur.clone();
    let mut write = false;
    if let Some(s) = &a.set {
        cur.extend(parse_ids(s)?);
        write = true;
    }
    if let Some(s) = &a.clear {
        let del = parse_ids(s)?;
        cur.retain(|t| !del.contains(t));
        write = true;
    }
    cur.sort_unstable();
    cur.dedup();
    let n_tiles = part.frames.get(crate::tiles::pick_frame(&part, a.frame.as_deref())?).map(|f| (f.grid.w / 8) * (f.grid.h / 8)).unwrap_or(0);
    if let Some(bad) = cur.iter().find(|t| **t >= n_tiles) {
        return Err(PxlError::validation(format!("tile #{} is outside the sheet ({} tiles)", bad, n_tiles)));
    }
    if write && (cur != before || part.priority.is_none()) {
        crate::status::refuse_if_approved(&path, a.force_approved)?;
        lease::guard(&path, out.agent())?;
        part.priority = Some(cur.clone());
        pxl::save(&path, &part)?;
        status::record(&path, &part, status::StatusEvent::Edited);
        out.line(format!("[priority] now {} tile(s): {}", cur.len(), fmt_ids(&cur)));
    } else {
        out.line(format!("[priority] {} tile(s): {}", cur.len(), fmt_ids(&cur)));
    }
    out.set("priority", json!(cur));
    Ok(())
}
