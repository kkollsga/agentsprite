//! `tiles dedupe`: merge near-duplicate tiles (mirrors included) until at most N unique tiles remain.
//!
//! Tiles that are the same up to mirroring already count once. A merge shows the target tile (possibly mirrored) in
//! place of every tile of the absorbed group. Order: merges whose differences nobody can see first (all moved tiles
//! differ by at most [`INVISIBLE`] in every pixel, or are unused), then the smallest colour error times use.
//! Every merge is reported with its
//! error.

use super::{Loaded, Tile, T};
use crate::cli::TilesArgs;
use crate::color::{self, Oklab};
use crate::output::{Out, PxlError, Result};
use serde_json::{json, Value};

/// Largest pixel difference (Oklab x 100) nobody notices.
pub const INVISIBLE: f64 = 2.5;

#[derive(Clone, Copy, Default)]
struct Cell {
    sum: f32,
    max: f32,
    px: u8,
    g: u8,
    /// 0 not computed, 1 valid, 2 not allowed
    state: u8,
}

#[derive(Clone, Debug)]
pub struct Merge {
    pub tile: usize,
    pub into: usize,
    pub flip: (bool, bool),
    pub usage: u32,
    pub mean: f64,
    pub max: f64,
    pub px: usize,
    pub visible: bool,
}

pub struct Outcome {
    pub before: usize,
    pub after: usize,
    pub merges: Vec<Merge>,
    pub tiles: Vec<Tile>,
    pub blocked: bool,
}

fn flip_idx(g: usize) -> (bool, bool) {
    (g & 1 == 1, g & 2 == 2)
}

/// Merge plan for `max` unique tiles. `usage[i]` = how often tile i is placed.
pub fn plan(l: &Loaded, max: usize, usage: &[u32]) -> Outcome {
    let n = l.sheet.tiles.len();
    let groups = super::pattern::groups(&l.sheet, &l.cols, &l.air, l.pattern);
    let before = groups.len();
    let px: Vec<[u32; 64]> = l.sheet.tiles.iter().map(|t| t.px(&l.cols)).collect();
    let lab = |c: u32| -> Oklab { if c & super::TRANSP != 0 { Oklab::default() } else { color::srgb_to_oklab(super::rgb_of(c)) } };
    let labs: Vec<[Oklab; 64]> = px.iter().map(|p| { let mut o = [Oklab::default(); 64]; for i in 0..64 { o[i] = lab(p[i]); } o }).collect();
    // flipped labs per tile (only reps are used as targets, but the table is cheap)
    let flab = |t: usize, g: usize| -> [Oklab; 64] {
        let (fx, fy) = flip_idx(g);
        let mut o = [Oklab::default(); 64];
        for y in 0..T {
            for x in 0..T {
                o[y * T + x] = labs[t][(if fy { T - 1 - y } else { y }) * T + if fx { T - 1 - x } else { x }];
            }
        }
        o
    };
    let reps: Vec<usize> = groups.iter().map(|g| g[0]).collect();
    let mut rep_flab: Vec<Option<[[Oklab; 64]; 4]>> = vec![None; n];
    for &r in &reps {
        rep_flab[r] = Some([flab(r, 0), flab(r, 1), flab(r, 2), flab(r, 3)]);
    }
    let mut table = vec![Cell::default(); n * n];
    let eval = |m: usize, r: usize, table: &mut Vec<Cell>| -> Cell {
        let c = table[m * n + r];
        if c.state != 0 {
            return c;
        }
        let rf = rep_flab[r].as_ref().unwrap();
        let mut best = Cell { state: 2, ..Cell::default() };
        let mut best_sum = f32::MAX;
        for g in 0..4 {
            let (mut sum, mut mx, mut cnt) = (0f32, 0f32, 0u8);
            for i in 0..64 {
                let d = (color::oklab_dist2(labs[m][i], rf[g][i]).sqrt() * 100.0) as f32;
                if d > 0.0 {
                    cnt += 1;
                }
                sum += d;
                mx = mx.max(d);
            }
            if sum < best_sum {
                best_sum = sum;
                best = Cell { sum, max: mx, px: cnt, g: g as u8, state: 1 };
            }
        }
        table[m * n + r] = best;
        best
    };
    struct Class {
        rep: usize,
        members: Vec<usize>,
    }
    let mut classes: Vec<Class> = groups.iter().map(|g| Class { rep: g[0], members: g.clone() }).collect();
    // current error of a tile (0 while it shows its own picture), and what it was moved to
    let mut cur: Vec<Option<(usize, Cell)>> = vec![None; n];
    let mut blocked = false;
    while classes.len() > max {
        let mut best: Option<((u8, f64, usize, usize), usize, usize)> = None;
        for a in 0..classes.len() {
            for b in 0..classes.len() {
                if a == b {
                    continue;
                }
                let (mut delta, mut vis, mut ok) = (0.0f64, false, true);
                for &m in &classes[a].members {
                    let c = eval(m, classes[b].rep, &mut table);
                    if c.state != 1 {
                        ok = false;
                        break;
                    }
                    let old = cur[m].map_or(0.0, |x| x.1.sum as f64);
                    let u = usage[m] as f64;
                    delta += u * (c.sum as f64 - old);
                    if usage[m] > 0 && c.max as f64 > INVISIBLE {
                        vis = true;
                    }
                }
                if !ok {
                    continue;
                }
                let key = (vis as u8, delta, classes[a].rep, classes[b].rep);
                if best.as_ref().map_or(true, |x| key < x.0) {
                    best = Some((key, a, b));
                }
            }
        }
        let Some((_, a, b)) = best else {
            blocked = true;
            break;
        };
        let rb = classes[b].rep;
        let moved = std::mem::take(&mut classes[a].members);
        for &m in &moved {
            let c = eval(m, rb, &mut table);
            cur[m] = Some((rb, c));
        }
        classes[b].members.extend(moved);
        classes.remove(a);
    }
    // final tiles and merge list
    let mut tiles = l.sheet.tiles.clone();
    let mut merges = Vec::new();
    for m in 0..n {
        if let Some((r, c)) = cur[m] {
            let (fx, fy) = flip_idx(c.g as usize);
            tiles[m] = l.sheet.tiles[r].flipped(fx, fy);
            merges.push(Merge { tile: m, into: r, flip: (fx, fy), usage: usage[m], mean: c.sum as f64 / 64.0, max: c.max as f64, px: c.px as usize, visible: usage[m] > 0 && c.max as f64 > INVISIBLE });
        }
    }
    let after = super::pattern::groups(&crate::tiles::Sheet { w: l.sheet.w, h: l.sheet.h, tiles: tiles.clone() }, &l.cols, &l.air, l.pattern).len();
    Outcome { before, after, merges, tiles, blocked }
}

pub fn run(l: &Loaded, a: &TilesArgs, out: &mut Out) -> Result<()> {
    let prof = crate::profile::load_for(&l.part, None)?;
    let max = a.max.unwrap_or_else(|| super::unique_limit(&l.part, &prof));
    let usage = vec![1u32; l.sheet.tiles.len()];
    let o = plan(l, max, &usage);
    let vis: Vec<&Merge> = o.merges.iter().filter(|m| m.visible).collect();
    let worst = o.merges.iter().filter(|m| m.usage > 0).map(|m| m.max).fold(0.0, f64::max);
    out.line(format!("unique tiles: {} before, {} after (target {}); {} tiles merged, {} with a visible difference, worst pixel {:.1} (Oklab x100)", o.before, o.after, max, o.merges.len(), vis.len(), worst));
    let mut vv: Vec<&Merge> = vis.clone();
    vv.sort_by(|x, y| (y.mean * y.usage as f64).partial_cmp(&(x.mean * x.usage as f64)).unwrap());
    for m in &vv {
        let (x, y) = l.sheet.pos(m.tile);
        out.line(format!(
            "  visible: tile #{} ({},{}) -> #{}{}{}, used {}x, worst pixel {:.1}, mean {:.2}, {} px differ",
            m.tile,
            x,
            y,
            m.into,
            if m.flip.0 { " mirrored x" } else { "" },
            if m.flip.1 { " mirrored y" } else { "" },
            m.usage,
            m.max,
            m.mean,
            m.px
        ));
    }
    let free = o.merges.len() - vis.len();
    if free > 0 {
        out.line(format!("  {} merge(s) invisible (unused tiles or differences of at most {:.1} per pixel)", free, INVISIBLE));
    }
    let merges_json: Vec<Value> = o
        .merges
        .iter()
        .map(|m| json!({"tile": m.tile, "into": m.into, "mirror_x": m.flip.0, "mirror_y": m.flip.1, "usage": m.usage, "worst": m.max, "mean": m.mean, "pixels": m.px, "visible": m.visible}))
        .collect();
    out.set("unique_before", json!(o.before));
    out.set("unique_after", json!(o.after));
    out.set("max", json!(max));
    out.set("merges", Value::Array(merges_json));
    out.set("visible_merges", json!(vis.len()));
    out.set("worst_error", json!(worst));
    if o.after > max {
        return Err(PxlError::validation(format!(
            "cannot reach {} unique tiles: {} remain because no further merge is possible",
            max, o.after
        ))
        .with_hint("nothing was written; raise --max, or redraw the tiles so more of them are alike"));
    }
    if a.apply {
        if o.merges.is_empty() {
            out.line("nothing to merge");
            return Ok(());
        }
        let frame = l.part.frames[l.fi].name.clone();
        let ops: Vec<crate::edit::Op> = o
            .merges
            .iter()
            .map(|m| {
                let (x, y) = l.sheet.pos(m.tile);
                crate::edit::Op::Patch { frame: frame.clone(), x: Some(x as i32), y: Some(y as i32), grid: o.tiles[m.tile].rows().join("\n") }
            })
            .collect();
        crate::edit::execute(&a.tileset, &ops, &crate::cli::EditFlags { force_approved: a.force_approved, ..Default::default() }, "tiles dedupe", out)?;
    } else {
        out.line(format!("dry run: pxl tiles dedupe {} --max {} --apply", a.tileset, max));
    }
    Ok(())
}
