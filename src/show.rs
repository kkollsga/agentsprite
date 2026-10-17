//! `pxl show`: a frame as text with rulers, optionally with the input next to it (`--ref`).
//!
//! Layout: row labels on the left, a two-line column ruler on top (tens digit where it changes, ones
//! digit on every column). With `--ref` the input is printed to the right at native resolution, one
//! letter per source colour (legend printed). Each target row `12` is followed by `factor` input rows
//! labelled `12a`, `12b`, ...; the input is aligned with the part's `factor` and `reduce.phase`:
//! target pixel X covers input x in `origin.x + factor*(X - part.origin.x) + phase.x + 0..factor`
//! (for factor 2 and phase [-1, 0]: input x in {2X-1, 2X}), likewise for y.

use crate::fsutil::PathSlash;
use crate::cli::{FrameSel, Region, ShowArgs};
use crate::fsutil;
use crate::img::Rgba;
use crate::load;
use crate::model::Grid;
use crate::output::{Out, PxlError, Result};
use crate::pxl;
use serde_json::json;
use std::collections::BTreeMap;

/// Clip a region to a `w`x`h` canvas. `None` when nothing is left.
pub fn clip_region(r: Region, w: usize, h: usize) -> Option<(i32, i32, usize, usize)> {
    let x0 = r.x.max(0) as i64;
    let y0 = r.y.max(0) as i64;
    let x1 = (r.x as i64 + r.w as i64).min(w as i64);
    let y1 = (r.y as i64 + r.h as i64).min(h as i64);
    if x1 <= x0 || y1 <= y0 {
        None
    } else {
        Some((x0 as i32, y0 as i32, (x1 - x0) as usize, (y1 - y0) as usize))
    }
}

fn digits(n: usize) -> usize {
    n.to_string().len()
}

/// The two ruler lines (tens, ones) for columns `x0..x0+w`, each column `rep` characters wide,
/// preceded by `pad` spaces. The tens line is omitted when every column is below 10.
pub fn ruler(x0: i32, w: usize, rep: usize, pad: usize) -> Vec<String> {
    let mut tens = " ".repeat(pad);
    let mut ones = " ".repeat(pad);
    let mut any_tens = false;
    for i in 0..w {
        let x = x0 + i as i32;
        let show_tens = x >= 10 && (x % 10 == 0 || i == 0);
        any_tens |= show_tens;
        let t = if show_tens { char::from(b'0' + ((x / 10) % 10) as u8) } else { ' ' };
        let o = char::from(b'0' + (x % 10) as u8);
        tens.push(t);
        ones.push(o);
        for _ in 1..rep {
            tens.push(' ');
            ones.push(o);
        }
    }
    let mut v = Vec::new();
    if any_tens {
        v.push(tens.trim_end().to_string());
    }
    v.push(ones);
    v
}

/// Render `region` of a grid as ruler lines plus `  y |cells` rows.
pub fn render_grid(g: &Grid, region: (i32, i32, usize, usize)) -> Vec<String> {
    let (x0, y0, w, h) = region;
    let lw = digits((y0 as usize + h).saturating_sub(1).max(1));
    let mut lines = ruler(x0, w, 1, lw + 2);
    for yy in 0..h {
        let y = y0 + yy as i32;
        let cells: String = (0..w).map(|i| g.get(x0 + i as i32, y) as char).collect();
        lines.push(format!("{:>lw$} |{}", y, cells, lw = lw));
    }
    lines
}

fn hex(c: Rgba) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
}

fn letter(i: usize) -> char {
    if i < 26 {
        char::from(b'a' + i as u8)
    } else if i < 52 {
        char::from(b'A' + (i - 26) as u8)
    } else {
        '?'
    }
}

/// Total line width used when laying several frames side by side.
const MULTI_WIDTH: usize = 120;

pub fn run_show(a: &ShowArgs, out: &mut Out) -> Result<()> {
    run_show_inner(a, out)?;
    if a.cells {
        let path = fsutil::resolve_part(&a.part)?;
        let part = pxl::load(&path)?;
        if part.palette_set.is_none() {
            out.line("(--cells: the part has no palette_set)");
            return Ok(());
        }
        let sel = FrameSel { frames: a.frame.split(',').map(|s| s.to_string()).collect(), all: false };
        let mut js = Vec::new();
        for f in sel.select(&part)? {
            let fi = part.frame_index(&f.name).unwrap();
            let (t, j) = crate::cells::show_cells(&part, fi)?;
            out.line(t);
            js.push(json!({"frame": f.name, "cells": j}));
        }
        out.set("cell_layout", serde_json::Value::Array(js));
    }
    Ok(())
}

fn run_show_inner(a: &ShowArgs, out: &mut Out) -> Result<()> {
    let path = fsutil::resolve_part(&a.part)?;
    let part = pxl::load(&path)?;
    let sel = FrameSel { frames: a.frame.split(',').map(|s| s.to_string()).collect(), all: false };
    let frames = sel.select(&part)?;
    if let Some(t) = a.tile {
        if frames.len() != 1 {
            return Err(PxlError::usage("--tile takes exactly one frame"));
        }
        if a.region.is_some() {
            return Err(PxlError::usage("--tile and --region cannot be combined (tile coordinates are 0..7)"));
        }
        let (text, j) = crate::tileio::show_tile(&part, frames[0], t, a.reference, a.raw)?;
        out.emit(text, j);
        return Ok(());
    }
    if frames.len() == 1 {
        let (text, j) = show_one(&part, frames[0], a)?;
        out.emit(text, j);
        return Ok(());
    }
    if a.reference {
        let mut text = String::new();
        let mut js = Vec::new();
        for f in &frames {
            let (t, j) = show_one(&part, f, a)?;
            if !text.is_empty() {
                text.push_str("\n\n");
            }
            text.push_str(&t);
            js.push(j);
        }
        out.emit(text, json!({"part": part.name, "frames": js}));
        return Ok(());
    }
    // side by side, sharing the row labels
    struct Blk<'a> {
        f: &'a crate::model::Frame,
        reg: (i32, i32, usize, usize),
    }
    let mut blks = Vec::new();
    for f in &frames {
        let g = &f.grid;
        let r = a.region.unwrap_or(Region { x: 0, y: 0, w: g.w, h: g.h });
        let reg = clip_region(r, g.w, g.h).ok_or_else(|| {
            PxlError::usage(format!("region {},{},{},{} lies outside the {}x{} canvas of frame {}", r.x, r.y, r.w, r.h, g.w, g.h, f.name))
        })?;
        blks.push(Blk { f, reg });
    }
    let y0 = blks.iter().map(|b| b.reg.1).min().unwrap_or(0);
    let y1 = blks.iter().map(|b| b.reg.1 + b.reg.3 as i32).max().unwrap_or(0);
    let lw = digits((y1 as usize).saturating_sub(1).max(1));
    let pad = lw + 2;
    let mut text = format!("{} {} frames", part.name, blks.len());
    let mut i = 0;
    while i < blks.len() {
        let mut j = i;
        let mut used = pad;
        while j < blks.len() {
            let w = blks[j].reg.2.max(blks[j].f.name.len().min(blks[j].reg.2.max(8)));
            let add = w + if j > i { 2 } else { 0 };
            if j > i && used + add > MULTI_WIDTH {
                break;
            }
            used += add;
            j += 1;
        }
        let grp = &blks[i..j];
        let cw: Vec<usize> = grp.iter().map(|b| b.reg.2.max(b.f.name.len().min(b.reg.2.max(8)))).collect();
        let rulers: Vec<Vec<String>> = grp.iter().map(|b| ruler(b.reg.0, b.reg.2, 1, 0)).collect();
        let nr = rulers.iter().map(|r| r.len()).max().unwrap_or(0);
        let mut head = " ".repeat(pad);
        for (k, b) in grp.iter().enumerate() {
            if k > 0 {
                head.push_str("  ");
            }
            let mut n = b.f.name.clone();
            if b.f.locked {
                n.push('*');
            }
            n.truncate(cw[k]);
            head.push_str(&format!("{:<w$}", n, w = cw[k]));
        }
        text.push('\n');
        text.push_str(head.trim_end());
        for li in 0..nr {
            let mut l = " ".repeat(pad);
            for (k, r) in rulers.iter().enumerate() {
                if k > 0 {
                    l.push_str("  ");
                }
                let line = (li + r.len()).checked_sub(nr).and_then(|x| r.get(x)).cloned().unwrap_or_default();
                l.push_str(&format!("{:<w$}", line, w = cw[k]));
            }
            text.push('\n');
            text.push_str(l.trim_end());
        }
        for y in y0..y1 {
            let mut l = format!("{:>lw$} |", y, lw = lw);
            for (k, b) in grp.iter().enumerate() {
                if k > 0 {
                    l.push_str("  ");
                }
                let (bx, by, bw, bh) = b.reg;
                let cells: String = if y >= by && y < by + bh as i32 { (0..bw).map(|x| b.f.grid.get(bx + x as i32, y) as char).collect() } else { String::new() };
                l.push_str(&format!("{:<w$}", cells, w = cw[k]));
            }
            text.push('\n');
            text.push_str(l.trim_end());
        }
        text.push('\n');
        i = j;
    }
    let text = text.trim_end().to_string();
    let js: Vec<serde_json::Value> = blks
        .iter()
        .map(|b| {
            let (x0, y0, w, h) = b.reg;
            let cells: Vec<String> = (0..h).map(|i| (0..w).map(|j| b.f.grid.get(x0 + j as i32, y0 + i as i32) as char).collect()).collect();
            json!({"frame": b.f.name, "locked": b.f.locked, "size": [b.f.grid.w, b.f.grid.h], "region": [x0, y0, w, h], "rows": cells})
        })
        .collect();
    out.emit(text, json!({"part": part.name, "frames": js}));
    Ok(())
}

fn show_one(part: &crate::model::Part, frame: &crate::model::Frame, a: &ShowArgs) -> Result<(String, serde_json::Value)> {
    let g = &frame.grid;
    let whole = Region { x: 0, y: 0, w: g.w, h: g.h };
    let r = a.region.unwrap_or(whole);
    let (x0, y0, w, h) = clip_region(r, g.w, g.h).ok_or_else(|| {
        PxlError::usage(format!("region {},{},{},{} lies outside the {}x{} canvas of frame {}", r.x, r.y, r.w, r.h, g.w, g.h, frame.name))
    })?;
    let header = format!(
        "{}/{} {}x{} region {},{},{},{}{}",
        part.name,
        frame.name,
        g.w,
        g.h,
        x0,
        y0,
        w,
        h,
        if frame.locked { " locked" } else { "" }
    );
    if !a.reference {
        let mut rows = render_grid(g, (x0, y0, w, h));
        if a.raw {
            let n = rows.len() - h;
            rows.drain(..n);
        }
        let mut text = header.clone();
        for l in &rows {
            text.push('\n');
            text.push_str(l);
        }
        let cells: Vec<String> = (0..h).map(|i| (0..w).map(|j| g.get(x0 + j as i32, y0 + i as i32) as char).collect()).collect();
        return Ok((
            text,
            json!({"part": part.name, "frame": frame.name, "locked": frame.locked, "size": [g.w, g.h], "region": [x0, y0, w, h], "rows": cells}),
        ));
    }

    // ---- reference
    let rb = ref_block(part, frame, (x0, y0, w, h))?;
    let mut text = header;
    text.push_str(&format!("\nreference x{} phase {},{}  (left: target, right: input; '.' = transparent)", rb.factor, rb.phase.0, rb.phase.1));
    for l in &rb.lines {
        text.push('\n');
        text.push_str(l.trim_end());
    }
    text.push_str(&format!("\nlegend: {}", rb.legend));
    let cells: Vec<String> = (0..h).map(|i| (0..w).map(|j| g.get(x0 + j as i32, y0 + i as i32) as char).collect()).collect();
    Ok((
        text,
        json!({"part": part.name, "frame": frame.name, "locked": frame.locked, "size": [g.w, g.h], "region": [x0, y0, w, h], "rows": cells,
               "ref": {"factor": rb.factor, "phase": [rb.phase.0, rb.phase.1], "legend": rb.legend_json, "rows": rb.ref_rows}}),
    ))
}

/// The target region and the input side by side (rulers, rows `12`, sub rows `12a`/`12b`), plus the legend.
pub struct RefBlock {
    pub lines: Vec<String>,
    pub legend: String,
    pub legend_json: serde_json::Map<String, serde_json::Value>,
    pub ref_rows: Vec<serde_json::Value>,
    pub factor: i32,
    pub phase: (i32, i32),
}

pub fn ref_block(part: &crate::model::Part, frame: &crate::model::Frame, reg: (i32, i32, usize, usize)) -> Result<RefBlock> {
    ref_block_off(part, frame, reg, (0, 0))
}

/// Like [`ref_block`], with the printed coordinates (rulers, row labels) shifted by `-off` (a tile at `off` prints 0..7).
pub fn ref_block_off(part: &crate::model::Part, frame: &crate::model::Frame, reg: (i32, i32, usize, usize), off: (i32, i32)) -> Result<RefBlock> {
    let g = &frame.grid;
    let (x0, y0, w, h) = reg;
    let rf = load::reference(part, &std::fs::canonicalize(part.dir()).unwrap_or_else(|_| part.dir()), frame)?;
    let f = part.input.factor.or(part.reduce.factor).unwrap_or(1).max(1) as i32;
    let ph = part.reduce.phase.unwrap_or((0, 0));
    let ref_at = |tx: i32, sub_x: i32, ty: i32, sub_y: i32| -> Rgba {
        let rx = rf.origin.0 + f * (tx - part.origin.0) + ph.0 + sub_x;
        let ry = rf.origin.1 + f * (ty - part.origin.1) + ph.1 + sub_y;
        rf.img.get(rx, ry)
    };
    // letters by frequency over the whole input image
    // one colour keeps one letter in every frame of the asset: the order is the frequency over the sources of all frames
    let mut counts: BTreeMap<(u8, u8, u8), usize> = BTreeMap::new();
    let pdir = std::fs::canonicalize(part.dir()).unwrap_or_else(|_| part.dir());
    let mut imgs: Vec<crate::img::RgbaImage> = vec![rf.img.clone()];
    for fr in part.frames.iter().filter(|x| x.name != frame.name).take(64) {
        if let Ok(o) = load::reference(part, &pdir, fr) {
            imgs.push(o.img);
        }
    }
    for im in &imgs {
        for p in &im.px {
            if p.a >= 128 {
                *counts.entry((p.r, p.g, p.b)).or_insert(0) += 1;
            }
        }
    }
    let mut order: Vec<((u8, u8, u8), usize)> = counts.into_iter().collect();
    order.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let letters: BTreeMap<(u8, u8, u8), char> = order.iter().enumerate().map(|(i, (c, _))| (*c, letter(i))).collect();
    let mut used: BTreeMap<char, String> = BTreeMap::new();

    let lw = digits(((y0 - off.1).max(0) as usize + h).saturating_sub(1).max(1));
    let rl = lw + 1;
    let left_pad = lw + 2;
    let ftu = f as usize;
    let lr = ruler(x0 - off.0, w, 1, left_pad);
    let rr = ruler(x0 - off.0, w, ftu, rl + 2);
    // header rulers side by side
    let mut lines: Vec<String> = Vec::new();
    let nr = lr.len().max(rr.len());
    for i in 0..nr {
        // align by bottom: the ones line is last in both
        let li = (i + lr.len()).checked_sub(nr).and_then(|k| lr.get(k)).cloned().unwrap_or_default();
        let ri = (i + rr.len()).checked_sub(nr).and_then(|k| rr.get(k)).cloned().unwrap_or_default();
        lines.push(format!("{:<width$}   {}", li, ri, width = left_pad + w).trim_end().to_string());
    }
    let mut ref_rows = Vec::new();
    for yy in 0..h {
        let ty = y0 + yy as i32;
        for sy in 0..f {
            let mut letters_row = String::new();
            for xx in 0..w {
                let tx = x0 + xx as i32;
                for sx in 0..f {
                    let p = ref_at(tx, sx, ty, sy);
                    if p.a < 128 {
                        letters_row.push('.');
                    } else {
                        let l = letters.get(&(p.r, p.g, p.b)).copied().unwrap_or('?');
                        used.entry(l).or_insert_with(|| hex(p));
                        letters_row.push(l);
                    }
                }
            }
            let sub = if f == 1 { String::new() } else { letter(sy as usize).to_string() };
            let label = format!("{}{}", ty - off.1, sub);
            let left = if sy == 0 {
                let cells: String = (0..w).map(|i| g.get(x0 + i as i32, ty) as char).collect();
                format!("{:>lw$} |{}", ty - off.1, cells, lw = lw)
            } else {
                " ".repeat(left_pad + w)
            };
            lines.push(format!("{}   {:>rl$} |{}", left, label, letters_row, rl = rl));
            ref_rows.push(json!({"label": label, "row": letters_row}));
        }
    }
    // which template key each input colour becomes: `>K` from the template's [map], `~K` nearest colour
    let tpl = crate::template::load_for(part).ok();
    let key_of = |hexs: &str| -> Option<String> {
        let t = tpl.as_ref()?;
        let c = crate::color::parse_hex(hexs).ok()?;
        if let Some(r) = t.map.iter().find(|r| r.from == c) {
            return Some(format!(">{}", r.key as char));
        }
        let o = crate::color::srgb_to_oklab(c);
        t.keys().filter_map(|k| t.color_of(k.key).map(|kc| (k.key, crate::color::oklab_dist2(o, crate::color::srgb_to_oklab(kc))))).min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal)).map(|(k, _)| format!("~{}", k as char))
    };
    let legend: Vec<String> = used.iter().map(|(l, c)| format!("{}={}{}", l, c, key_of(c).unwrap_or_default())).collect();
    let legend_json: serde_json::Map<String, serde_json::Value> = used.iter().map(|(l, c)| (l.to_string(), json!(c))).collect();
    Ok(RefBlock { lines, legend: legend.join(" "), legend_json, ref_rows, factor: f, phase: ph })
}

// ------------------------------------------------------------------ text round trip

/// Text that `show` (or an edit echo) printed, or a plain grid, taken apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridText {
    /// Rows carried `y |` labels (so they place themselves).
    pub labelled: bool,
    /// Column of the first cell, from the `region x,y,w,h` header or the ruler.
    pub x0: Option<i32>,
    /// (y if labelled, cells). A space is "unchanged".
    pub rows: Vec<(Option<i32>, String)>,
}

/// `  12 |cells` -> (12, "cells")
fn label_row(line: &str) -> Option<(i32, &str)> {
    let t = line.trim_start_matches(' ');
    let d = t.bytes().take_while(|b| b.is_ascii_digit()).count();
    if d == 0 || d > 6 {
        return None;
    }
    let rest = &t[d..];
    let cells = rest.strip_prefix(" |")?;
    Some((t[..d].parse().ok()?, cells))
}

/// `12a |letters` (the input rows `--ref` prints under a target row)
fn is_sub_row(line: &str) -> bool {
    let t = line.trim_start_matches(' ');
    let d = t.bytes().take_while(|b| b.is_ascii_digit()).count();
    d > 0 && t[d..].len() > 3 && t.as_bytes()[d].is_ascii_alphabetic() && t[d + 1..].starts_with(" |")
}

/// Cut the `--ref` side columns (`   12a |letters`) off the end of a row's cells.
fn strip_ref_cols(cells: &str) -> &str {
    let b = cells.as_bytes();
    let mut i = 0;
    while i + 3 <= b.len() {
        if &b[i..i + 3] == b"   " {
            let mut j = i;
            while j < b.len() && b[j] == b' ' {
                j += 1;
            }
            let d = b[j..].iter().take_while(|c| c.is_ascii_digit()).count();
            if d > 0 {
                let mut k = j + d;
                if k < b.len() && b[k].is_ascii_alphabetic() {
                    k += 1;
                }
                if cells[k..].starts_with(" |") {
                    return &cells[..i];
                }
            }
            i = j;
        } else {
            i += 1;
        }
    }
    cells
}

fn header_region(line: &str) -> Option<(i32, i32, usize, usize)> {
    let at = line.find("region ")? + "region ".len();
    let s: String = line[at..].chars().take_while(|c| c.is_ascii_digit() || *c == ',').collect();
    let v: Vec<i64> = s.split(',').filter_map(|p| p.parse().ok()).collect();
    (v.len() == 4).then(|| (v[0] as i32, v[1] as i32, v[2] as usize, v[3] as usize))
}

/// Parse a grid given to `patch` / `write_grid`: either plain rows, or what `pxl show` printed (header,
/// rulers, `y |` row labels, `--ref` columns) which is stripped and used to place the rows.
pub fn parse_grid_text(text: &str) -> Result<GridText> {
    let mut lines: Vec<&str> = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
    while lines.last().map_or(false, |l| l.trim().is_empty()) {
        lines.pop();
    }
    let labelled = lines.iter().any(|l| label_row(l).is_some());
    if !labelled {
        return Ok(GridText { labelled: false, x0: None, rows: lines.iter().map(|l| (None, l.to_string())).collect() });
    }
    let mut x0 = None;
    let mut ruler_lines: Vec<&str> = Vec::new();
    let mut rows = Vec::new();
    for (n, l) in lines.iter().enumerate() {
        if let Some((y, cells)) = label_row(l) {
            rows.push((Some(y), strip_ref_cols(cells).to_string()));
            continue;
        }
        let t = l.trim();
        if t.is_empty() || is_sub_row(l) || t.starts_with("legend:") || t.starts_with("tile ") || t.starts_with("reference ") || t.starts_with("preview:") || t.starts_with("total ") {
            continue;
        }
        if let Some(r) = header_region(l) {
            if x0.is_none() {
                x0 = Some(r.0);
            }
            continue;
        }
        if rows.is_empty() && t.bytes().all(|b| b.is_ascii_digit() || b == b' ') {
            ruler_lines.push(l);
            continue;
        }
        if rows.is_empty() && !t.contains('|') {
            continue; // a title line before the rows
        }
        return Err(PxlError::usage(format!("line {} of the pasted grid is neither a row (`12 |cells`), a ruler nor a header: {:?}", n + 1, t))
            .with_hint("paste the `pxl show` output as printed, or plain rows with X Y"));
    }
    if x0.is_none() {
        if let Some(ones) = ruler_lines.last() {
            let c = ones.bytes().position(|b| b.is_ascii_digit());
            if let Some(c) = c {
                let o = (ones.as_bytes()[c] - b'0') as i32;
                let tens = if ruler_lines.len() >= 2 { ruler_lines[ruler_lines.len() - 2].bytes().position(|b| b.is_ascii_digit()).filter(|p| *p == c).map(|p| (ruler_lines[ruler_lines.len() - 2].as_bytes()[p] - b'0') as i32) } else { None };
                x0 = Some(tens.unwrap_or(0) * 10 + o);
            }
        }
    }
    Ok(GridText { labelled: true, x0, rows })
}

// ------------------------------------------------------------------ PNG rendering (preview, view)

use crate::compare::{self, C_WARN, DIM, GUTTER, MARGIN, MAX_PX, TEXT};
use crate::img::RgbaImage;
use crate::model::{Part, RefFrame};
use crate::template::Template;

/// A window of cells around `bb` (min x, min y, max x, max y inclusive) with `margin` cells of context, at
/// least `min_w` x `min_h`, inside a `gw` x `gh` canvas.
pub fn window_around(bb: (usize, usize, usize, usize), margin: usize, min_w: usize, min_h: usize, gw: usize, gh: usize) -> (i32, i32, usize, usize) {
    let fit = |lo: usize, hi: usize, min: usize, max: usize| -> (usize, usize) {
        let mut a = lo.saturating_sub(margin);
        let mut b = (hi + 1 + margin).min(max);
        let want = min.min(max);
        while b - a < want {
            if a > 0 {
                a -= 1;
            }
            if b - a < want && b < max {
                b += 1;
            }
            if a == 0 && b == max {
                break;
            }
        }
        (a, b - a)
    };
    let (x, w) = fit(bb.0, bb.2, min_w, gw);
    let (y, h) = fit(bb.1, bb.3, min_h, gh);
    (x as i32, y as i32, w, h)
}

fn lighten(p: &mut Rgba) {
    p.r = p.r.saturating_add((255 - p.r) / 4);
    p.g = p.g.saturating_add((255 - p.g) / 4);
    p.b = p.b.saturating_add((255 - p.b) / 4);
}

/// Reference | grids side by side over the cell window `win`, one cell = `factor * zoom` display pixels, a
/// dim cell grid with brighter lines on every 4th coordinate, x labels on top and y labels on the left.
/// `mark` (cells) is ringed. `zoom` None picks the largest 1..=8 that keeps the image within 1568 px.
pub fn render_panels(part: &Part, tpl: Option<&Template>, rf: Option<&RefFrame>, grids: &[(String, &Grid)], win: (i32, i32, usize, usize), mark: Option<(i32, i32, usize, usize)>, zoom: Option<usize>) -> (RgbaImage, usize) {
    let marks: Vec<((i32, i32, usize, usize), Rgba)> = mark.into_iter().map(|m| (m, C_WARN)).collect();
    render_panels_marked(part, tpl, rf, grids, win, &marks, zoom)
}

/// Like [`render_panels`] with any number of coloured cell rectangles ringed in every panel.
pub fn render_panels_marked(part: &Part, tpl: Option<&Template>, rf: Option<&RefFrame>, grids: &[(String, &Grid)], win: (i32, i32, usize, usize), marks: &[((i32, i32, usize, usize), Rgba)], zoom: Option<usize>) -> (RgbaImage, usize) {
    let f = part.input.factor.or(part.reduce.factor).unwrap_or(1).max(1) as usize;
    let ph = part.reduce.phase.unwrap_or((0, 0));
    let (wx, wy, ww, wh) = win;
    let npan = grids.len() + usize::from(rf.is_some());
    let ylab = compare::text_w("000", 2) + 6;
    let top = 12 + 12 + 4;
    let dims = |s: usize| -> (usize, usize) {
        let cell = f * s;
        (2 * MARGIN + ylab + npan * ww * cell + npan.saturating_sub(1) * GUTTER, MARGIN + top + wh * cell + MARGIN)
    };
    let s = match zoom {
        Some(z) => z.max(1),
        None => (1..=8).rev().find(|s| {
            let (w, h) = dims(*s);
            w <= MAX_PX && h <= MAX_PX
        })
        .unwrap_or(1),
    };
    let cell = f * s;
    let (iw, ih) = dims(s);
    let mut im = RgbaImage::filled(iw, ih, compare::BG);
    let (pw, phh) = (ww * cell, wh * cell);
    let mut step = 4;
    while cell * step < compare::text_w("00", 2) + 4 {
        step *= 2;
    }
    let mut px0 = MARGIN + ylab;
    let mut panels: Vec<(String, Option<&Grid>)> = Vec::new();
    if rf.is_some() {
        panels.push(("reference".to_string(), None));
    }
    for (n, g) in grids {
        panels.push((n.clone(), Some(*g)));
    }
    for (title, g) in &panels {
        let mut pan = compare::checker(pw, phh, 4);
        match g {
            None => {
                if let Some(rf) = rf {
                    for dy in 0..phh {
                        for dx in 0..pw {
                            let (tx, ty) = (wx + (dx / cell) as i32, wy + (dy / cell) as i32);
                            let rx = rf.origin.0 + f as i32 * (tx - part.origin.0) + ph.0 + ((dx % cell) / s) as i32;
                            let ry = rf.origin.1 + f as i32 * (ty - part.origin.1) + ph.1 + ((dy % cell) / s) as i32;
                            let p = rf.img.get(rx, ry);
                            if p.a > 0 {
                                let d = &mut pan.px[dy * pw + dx];
                                *d = compare::blend(*d, p);
                            }
                        }
                    }
                }
            }
            Some(g) => {
                for cy in 0..wh {
                    for cx in 0..ww {
                        let k = g.get(wx + cx as i32, wy + cy as i32);
                        if k != b'.' && g.in_bounds(wx + cx as i32, wy + cy as i32) {
                            let c = compare::key_color(part, tpl, k);
                            compare::fill(&mut pan, cx * cell, cy * cell, cell, cell, Rgba::opaque(c.r, c.g, c.b));
                        }
                    }
                }
            }
        }
        compare::grid_lines(&mut pan, cell);
        if cell >= 3 {
            for dy in 0..phh {
                for dx in 0..pw {
                    let (cx, cy) = (wx + (dx / cell) as i32, wy + (dy / cell) as i32);
                    if (dx % cell == 0 && cx % 4 == 0) || (dy % cell == 0 && cy % 4 == 0) {
                        lighten(&mut pan.px[dy * pw + dx]);
                    }
                }
            }
        }
        for ((mx, my, mw, mh), col) in marks {
            compare::ring(&mut pan, (mx - wx) * cell as i32 - 1, (my - wy) * cell as i32 - 1, mw * cell + 2, mh * cell + 2, 2, *col);
        }
        compare::paste(&mut im, &pan, px0, MARGIN + top);
        compare::draw_text(&mut im, px0, MARGIN, title, 2, TEXT);
        for cx in 0..ww {
            let x = wx + cx as i32;
            if x % step as i32 == 0 {
                compare::draw_text(&mut im, px0 + cx * cell + 1, MARGIN + 14, &x.to_string(), 2, DIM);
            }
        }
        px0 += pw + GUTTER;
    }
    for cy in 0..wh {
        let y = wy + cy as i32;
        if y % step as i32 == 0 {
            compare::draw_text(&mut im, MARGIN, MARGIN + top + cy * cell + (cell.saturating_sub(10)) / 2, &y.to_string(), 2, DIM);
        }
    }
    (im, s)
}

pub fn write_png(path: &std::path::Path, im: &RgbaImage) -> Result<()> {
    if let Some(d) = path.parent() {
        if !d.as_os_str().is_empty() {
            std::fs::create_dir_all(d)?;
        }
    }
    fsutil::atomic_write(path, &compare::encode_sheet_png(im))
}

/// `preview/<part>-<frame>.png` next to the part.
pub fn default_png(part: &Part, frame: &str, suffix: &str) -> std::path::PathBuf {
    part.dir().join("preview").join(format!("{}-{}{}.png", part.name, frame, suffix))
}

/// Reference of a frame, if the part has one.
pub fn try_reference(part: &Part, frame: &crate::model::Frame) -> Option<RefFrame> {
    load::reference(part, &std::fs::canonicalize(part.dir()).unwrap_or_else(|_| part.dir()), frame).ok()
}

pub fn run_view(a: &crate::cli::ViewArgs, out: &mut Out) -> Result<()> {
    if a.tile.is_some() {
        return crate::tileio::view_tile(a, out);
    }
    let path = fsutil::resolve_part(&a.part)?;
    let part = pxl::load(&path)?;
    let tpl = crate::template::load_for(&part).ok();
    let frame = part.frames.iter().find(|f| f.name == a.frame).ok_or_else(|| crate::cli::unknown_frame(&part, &a.frame))?;
    let g = &frame.grid;
    let rf = if a.no_ref {
        None
    } else if a.reference {
        Some(load::reference(&part, &std::fs::canonicalize(part.dir()).unwrap_or_else(|_| part.dir()), frame)?)
    } else {
        try_reference(&part, frame)
    };
    let win = match a.region {
        Some(r) => clip_region(r, g.w, g.h).ok_or_else(|| PxlError::usage(format!("region {},{},{},{} lies outside the {}x{} canvas of frame {}", r.x, r.y, r.w, r.h, g.w, g.h, frame.name)))?,
        None => {
            let mut bb: Option<(usize, usize, usize, usize)> = None;
            for y in 0..g.h {
                for x in 0..g.w {
                    if g.cells[y * g.w + x] != b'.' {
                        bb = Some(bb.map_or((x, y, x, y), |b| (b.0.min(x), b.1.min(y), b.2.max(x), b.3.max(y))));
                    }
                }
            }
            bb.map_or((0, 0, g.w, g.h), |b| window_around(b, 1, 8, 8, g.w, g.h))
        }
    };
    // optional hint layer (information only): where the detector sees eyes in the reference
    let mut hints: Vec<String> = Vec::new();
    let mut marks: Vec<((i32, i32, usize, usize), Rgba)> = Vec::new();
    if a.features {
        match rf.as_ref() {
            Some(r) => {
                for e in crate::features::eyes_of(&part, r) {
                    let (x, y, w, h) = crate::features::marker_rect(&e);
                    marks.push(((x, y, w.max(1) as usize, h.max(1) as usize), compare::C_LOST));
                    hints.push(format!("eye-like feature near {},{} (white {} cells, gaze {:+.1},{:+.1})", e.cells.0, e.cells.1, e.wcells.len(), e.gaze.0, e.gaze.1));
                }
            }
            None => hints.push("no reference for this frame: nothing to detect".to_string()),
        }
    }
    let (im, s) = render_panels_marked(&part, tpl.as_ref(), rf.as_ref(), &[(frame.name.clone(), g)], win, &marks, a.zoom);
    if im.w > MAX_PX || im.h > MAX_PX {
        return Err(PxlError::usage(format!("the image would be {}x{} px, over the {} px limit", im.w, im.h, MAX_PX)).with_hint("use a smaller --zoom or a --region"));
    }
    let p = a.out.as_ref().map(std::path::PathBuf::from).unwrap_or_else(|| default_png(&part, &frame.name, "-view"));
    write_png(&p, &im)?;
    let text = format!(
        "view {}/{}: {}  ({}x{} px, {}x{} cells at {} px, region {},{},{},{}; {}; coordinates labelled every 4 cells, bright lines mark them)",
        part.name,
        frame.name,
        p.slash(),
        im.w,
        im.h,
        win.2,
        win.3,
        part.input.factor.or(part.reduce.factor).unwrap_or(1).max(1) as usize * s,
        win.0,
        win.1,
        win.2,
        win.3,
        if rf.is_some() { "reference | frame" } else { "frame only" }
    );
    let mut text = text;
    if a.features {
        text.push_str("\nfeatures (hint layer, information only; purple rings):");
        if hints.is_empty() {
            text.push_str("\n  none detected");
        }
        for h in &hints {
            text.push_str(&format!("\n  {}", h));
        }
    }
    out.emit(text, json!({"features": hints, "part": part.name, "frame": frame.name, "path": p.slash(), "size": [im.w, im.h], "region": [win.0, win.1, win.2, win.3], "zoom": s, "reference": rf.is_some()}));
    Ok(())
}
