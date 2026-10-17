//! Snippets: small reusable shapes (a good eye, a shoe) cut from an approved frame and stamped into others.
//!
//! A snippet is `snippets/<name>.txt` in the folder of the part's template (so every part of that family
//! sees it): plain rows of key characters. A space means "leave the target pixel unchanged", `.` means
//! "make it transparent". `pxl snippet save` writes `.` as a space unless `--solid` is given, so a stamped
//! eye does not punch holes into the face around it. `pxl stamp` is an ordinary edit (see `edit`).

use crate::fsutil::PathSlash;
use crate::cli::{Axis, SnippetListArgs, SnippetSaveArgs};
use crate::fsutil;
use crate::model::Part;
use crate::output::{Loc, Out, PxlError, Result};
use crate::pxl;
use crate::template;
use serde_json::json;
use std::path::PathBuf;

/// Edit distance (insert, delete, substitute) between two short strings.
fn distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + usize::from(a[i - 1] != b[j - 1]));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// The candidate closest to `name` (case-insensitive; a prefix or substring counts), if any is near enough.
pub fn closest(name: &str, cands: &[String]) -> Option<String> {
    let n = name.to_lowercase();
    let mut best: Option<(usize, &String)> = None;
    for c in cands {
        let l = c.to_lowercase();
        let d = if l == n {
            0
        } else if !n.is_empty() && (l.contains(&n) || n.contains(&l)) {
            1
        } else {
            distance(&n, &l)
        };
        if best.map_or(true, |(bd, _)| d < bd) {
            best = Some((d, c));
        }
    }
    best.filter(|(d, _)| *d <= (n.chars().count() / 3).max(2)).map(|(_, c)| c.clone())
}

/// Folder holding the snippets of a part's family: `snippets/` next to its template (else next to the part).
pub fn dir_for(part: &Part) -> PathBuf {
    let base = template::locate(part).ok().and_then(|p| p.parent().map(|d| d.to_path_buf())).unwrap_or_else(|| part.dir());
    base.join("snippets")
}

fn valid_name(n: &str) -> bool {
    !n.is_empty() && n.len() <= 40 && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Names of the saved snippets, sorted.
pub fn names(part: &Part) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir_for(part))
        .map(|rd| rd.filter_map(|e| e.ok()).filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(".txt")).map(|s| s.to_string())).collect())
        .unwrap_or_default();
    v.sort();
    v
}

/// The rows of a snippet (not padded). Unknown names list the available ones and the closest.
pub fn load(part: &Part, name: &str) -> Result<Vec<String>> {
    let dir = dir_for(part);
    let p = dir.join(format!("{}.txt", name));
    if !valid_name(name) || !p.is_file() {
        let all = names(part);
        let mut hint = if all.is_empty() { "no snippets saved yet: pxl snippet save PART FRAME NAME --region x,y,w,h".to_string() } else { format!("snippets: {}", all.join(", ")) };
        if let Some(c) = closest(name, &all) {
            hint = format!("did you mean '{}'? {}", c, hint);
        }
        return Err(PxlError::usage(format!("no snippet '{}' for part '{}'", name, part.name)).with_loc(Loc::file(dir.slash())).with_hint(hint));
    }
    let text = fsutil::read_string(&p)?;
    let rows: Vec<String> = text.strip_suffix('\n').unwrap_or(&text).split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l).to_string()).collect();
    for (y, r) in rows.iter().enumerate() {
        if let Some((x, c)) = r.chars().enumerate().find(|(_, c)| *c != ' ' && !(pxl::valid_key(*c as u8) && c.is_ascii())) {
            return Err(PxlError::validation(format!("invalid snippet character {:?}", c)).with_loc(Loc::file(p.slash()).at(y, x)));
        }
    }
    Ok(rows)
}

/// Mirror rows: `x` reverses every row (padded to the widest), `y` reverses the row order.
pub fn flip(rows: &[String], axis: Axis) -> Vec<String> {
    match axis {
        Axis::Y => rows.iter().rev().cloned().collect(),
        Axis::X => {
            let w = rows.iter().map(|r| r.chars().count()).max().unwrap_or(0);
            rows.iter().map(|r| format!("{:<w$}", r, w = w).chars().rev().collect()).collect()
        }
    }
}

/// Write a snippet from ready rows (a space leaves the target pixel unchanged). Returns the file.
pub fn save_rows(part: &Part, name: &str, rows: &[String]) -> Result<PathBuf> {
    if !valid_name(name) {
        return Err(PxlError::usage(format!("snippet name '{}' is not valid", name)).with_hint("use letters, digits, - and _"));
    }
    let dir = dir_for(part);
    std::fs::create_dir_all(&dir)?;
    let file = dir.join(format!("{}.txt", name));
    fsutil::atomic_write(&file, (rows.join("\n") + "\n").as_bytes())?;
    Ok(file)
}

pub fn run_save(a: &SnippetSaveArgs, out: &mut Out) -> Result<()> {
    if !valid_name(&a.name) {
        return Err(PxlError::usage(format!("snippet name '{}' is not valid", a.name)).with_hint("use letters, digits, - and _ (e.g. eye, shoe_l)"));
    }
    let path = fsutil::resolve_part(&a.part)?;
    let part = pxl::load(&path)?;
    let frame = part.frames.iter().find(|f| f.name == a.frame).ok_or_else(|| crate::cli::unknown_frame(&part, &a.frame))?;
    let g = &frame.grid;
    let r = a.region;
    let (ex, ey) = (r.x as i64 + r.w as i64 - 1, r.y as i64 + r.h as i64 - 1);
    if r.w == 0 || r.h == 0 || r.x < 0 || r.y < 0 || ex >= g.w as i64 || ey >= g.h as i64 {
        return Err(PxlError::validation(format!("region {},{},{},{} is outside the {}x{} canvas of frame {}", r.x, r.y, r.w, r.h, g.w, g.h, frame.name))
            .with_loc(Loc::file(path.slash()).frame(frame.name.clone()))
            .with_hint(format!("x must be 0..{}, y 0..{}; the region is x,y,w,h", g.w - 1, g.h - 1)));
    }
    let rows: Vec<String> = (0..r.h as i32)
        .map(|j| {
            (0..r.w as i32)
                .map(|i| {
                    let c = g.get(r.x + i, r.y + j) as char;
                    if c == '.' && !a.solid {
                        ' '
                    } else {
                        c
                    }
                })
                .collect::<String>()
        })
        .collect();
    let dir = dir_for(&part);
    let file = dir.join(format!("{}.txt", a.name));
    let existed = file.is_file();
    std::fs::create_dir_all(&dir)?;
    fsutil::atomic_write(&file, (rows.join("\n") + "\n").as_bytes())?;
    let mut text = format!("snippet {} {}: {}x{} from {}/{} at {},{}{}  -> {}", if existed { "replaced" } else { "saved" }, a.name, r.w, r.h, part.name, frame.name, r.x, r.y, if a.solid { " (solid)" } else { "" }, file.slash());
    for (j, row) in rows.iter().enumerate() {
        text.push_str(&format!("\n{:>2} |{}", j, row));
    }
    out.emit(text, json!({"name": a.name, "file": file.slash(), "replaced": existed, "size": [r.w, r.h], "rows": rows}));
    Ok(())
}

pub fn run_list(a: &SnippetListArgs, out: &mut Out) -> Result<()> {
    let path = fsutil::resolve_part(&a.part)?;
    let part = pxl::load(&path)?;
    let names = names(&part);
    let dir = dir_for(&part);
    let mut text = format!("snippets in {} ({})", dir.slash(), names.len());
    let mut js = Vec::new();
    for n in &names {
        let rows = load(&part, n)?;
        let w = rows.iter().map(|r| r.chars().count()).max().unwrap_or(0);
        text.push_str(&format!("\n{} {}x{}", n, w, rows.len()));
        for r in &rows {
            text.push_str(&format!("\n  |{}", r));
        }
        js.push(json!({"name": n, "size": [w, rows.len()], "rows": rows}));
    }
    if names.is_empty() {
        text.push_str("\nnone yet: pxl snippet save PART FRAME NAME --region x,y,w,h");
    }
    out.emit(text, json!({"dir": dir.slash(), "snippets": js}));
    Ok(())
}
