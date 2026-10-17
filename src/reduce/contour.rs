//! `contour`: structure first.
//!
//! 1. **Silhouette** at target resolution (alpha rule of `vote`: block majority, ring decides ties, thin
//!    opaque lines survive), cleaned: 1-cell holes are filled, cells without any 8-neighbour are dropped.
//! 2. **Outline ring:** the silhouette's 4-boundary (opaque cells with a transparent 4-neighbour) is one closed
//!    1 px contour. Ring cells whose dominant colour is a key of an `outline` role become the outline key;
//!    other ring cells (a white glove at the edge, a shoe) keep their own key. Doubled steps (L corners of the
//!    ring, the `jaggy` lint) are removed on the silhouette and the ring is re-derived.
//! 3. **Interior fill:** area majority per cell over the source pixels that are not part of a feature
//!    (ties: ring votes, then palette order), then orphan cells join their neighbours.
//! 4. **Features:** connected components (8-connected, same key) of the classified source with an area of at
//!    least one cell (or half a cell for detail-layer keys) and at most `FEATURE_CELLS` cells are stamped as
//!    clean minimal shapes: `round(area / f^2)` (min 1) 8-connected cells chosen by source overlap, grown from
//!    the best cell. Larger components are stamped first so that pupils land on top of eye whites. Eye white
//!    with a pupil, a separator line, a spike tip: each becomes one coherent shape, not speckle.
//!
//! Everything is integer arithmetic, ties go to the lower palette rank, no randomness.

use crate::fsutil::PathSlash;
use super::*;

const FEATURE_CELLS: i64 = 12;

fn alpha_mask(prep: &Prep, thin: bool) -> Vec<bool> {
    let (gw, gh, ring) = (prep.gw, prep.gh, prep.ring);
    let cls = |x: i32, y: i32| prep.cls(x, y);
    let mut out = vec![false; gw * gh];
    for cy in 0..gh as i32 {
        for cx in 0..gw as i32 {
            let blk = prep.block(cx, cy);
            let (x0, y0, x1, y1) = blk;
            let area = ((x1 - x0) * (y1 - y0)) as i64;
            let mut n_op = 0i64;
            for y in y0..y1 {
                for x in x0..x1 {
                    if cls(x, y) != TRANSPARENT {
                        n_op += 1;
                    }
                }
            }
            let (mut r_op, mut r_tot) = (0i64, 0i64);
            for y in y0 - ring..y1 + ring {
                for x in x0 - ring..x1 + ring {
                    if x >= x0 && x < x1 && y >= y0 && y < y1 {
                        continue;
                    }
                    r_tot += 1;
                    if cls(x, y) != TRANSPARENT {
                        r_op += 1;
                    }
                }
            }
            out[cy as usize * gw + cx as usize] = if n_op * 2 > area {
                true
            } else if n_op * 2 == area {
                r_op * 100 >= 16 * r_tot
            } else {
                n_op > 0 && thin && band_through(&cls, &|k| k != TRANSPARENT, blk, ring)
            };
        }
    }
    out
}

fn op(m: &[bool], gw: usize, gh: usize, x: i32, y: i32) -> bool {
    x >= 0 && y >= 0 && (x as usize) < gw && (y as usize) < gh && m[y as usize * gw + x as usize]
}

const N4: [(i32, i32); 4] = [(0, -1), (1, 0), (0, 1), (-1, 0)];
const N8: [(i32, i32); 8] = [(0, -1), (1, -1), (1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1)];

fn is_ring(m: &[bool], gw: usize, gh: usize, x: i32, y: i32) -> bool {
    op(m, gw, gh, x, y) && N4.iter().any(|d| !op(m, gw, gh, x + d.0, y + d.1))
}

/// Ring L corner with a neighbouring L corner (the `jaggy` lint definition applied to the ring).
fn l_corner(m: &[bool], gw: usize, gh: usize, x: i32, y: i32) -> Option<((i32, i32), (i32, i32))> {
    if !is_ring(m, gw, gh, x, y) {
        return None;
    }
    let n: Vec<(i32, i32)> = N4.iter().cloned().filter(|d| is_ring(m, gw, gh, x + d.0, y + d.1)).collect();
    if n.len() != 2 || (n[0].0 + n[1].0 == 0 && n[0].1 + n[1].1 == 0) {
        return None;
    }
    let d = (n[0].0 + n[1].0, n[0].1 + n[1].1);
    if is_ring(m, gw, gh, x + d.0, y + d.1) {
        return None;
    }
    Some((n[0], n[1]))
}

fn clean_silhouette(m: &mut Vec<bool>, gw: usize, gh: usize) {
    // fill 1-cell holes, drop cells with no 8-neighbour
    for _ in 0..2 {
        let snap = m.clone();
        for y in 0..gh as i32 {
            for x in 0..gw as i32 {
                let i = y as usize * gw + x as usize;
                if !snap[i] && N4.iter().all(|d| op(&snap, gw, gh, x + d.0, y + d.1)) {
                    m[i] = true;
                }
                if snap[i] && !N8.iter().any(|d| op(&snap, gw, gh, x + d.0, y + d.1)) {
                    m[i] = false;
                }
            }
        }
    }
    // doubled steps: remove the L corner when it is an outer cell, add the missing corner otherwise
    for _ in 0..6 {
        let mut changed = false;
        let mut flagged: Vec<(i32, i32)> = Vec::new();
        let snap = m.clone();
        for y in 0..gh as i32 {
            for x in 0..gw as i32 {
                let Some((a, c)) = l_corner(&snap, gw, gh, x, y) else { continue };
                if flagged.iter().any(|(fx, fy)| (fx - x).abs() + (fy - y).abs() == 1) {
                    continue;
                }
                let nb_l = l_corner(&snap, gw, gh, x + a.0, y + a.1).is_some() || l_corner(&snap, gw, gh, x + c.0, y + c.1).is_some();
                if !nb_l {
                    continue;
                }
                flagged.push((x, y));
                // x has ring neighbours a and c; its other two neighbours (-a, -c): if both opaque, x is an
                // inner cell of the step and the silhouette keeps it as interior; otherwise drop x.
                let (ia, ic) = (op(&snap, gw, gh, x - a.0, y - a.1), op(&snap, gw, gh, x - c.0, y - c.1));
                if !(ia && ic) {
                    m[y as usize * gw + x as usize] = false;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
}

struct Feature {
    key: u8,
    area: i64,
    px: Vec<(i32, i32)>,
}

fn components(prep: &Prep, tpl: &Template) -> Vec<Feature> {
    let (iw, ih) = (prep.iw, prep.ih);
    let mut seen = vec![false; (iw * ih) as usize];
    let mut out = Vec::new();
    let (bw, bh) = {
        let b = prep.block(0, 0);
        ((b.2 - b.0).max(1) as i64, (b.3 - b.1).max(1) as i64)
    };
    let cell_area = bw * bh;
    for y in 0..ih {
        for x in 0..iw {
            let k = prep.cls(x, y);
            if k == TRANSPARENT || seen[(y * iw + x) as usize] {
                continue;
            }
            let mut comp = vec![(x, y)];
            seen[(y * iw + x) as usize] = true;
            let mut i = 0;
            while i < comp.len() {
                let (px, py) = comp[i];
                i += 1;
                for d in N8 {
                    let (qx, qy) = (px + d.0, py + d.1);
                    if qx >= 0 && qy >= 0 && qx < iw && qy < ih && !seen[(qy * iw + qx) as usize] && prep.cls(qx, qy) == k {
                        seen[(qy * iw + qx) as usize] = true;
                        comp.push((qx, qy));
                    }
                }
            }
            let a = comp.len() as i64;
            let detail = tpl.layer_of(k).map_or(false, |l| l > 0);
            let min = if detail { (cell_area + 1) / 2 } else { cell_area };
            if a >= min.max(2) && a <= FEATURE_CELLS * cell_area {
                out.push(Feature { key: k, area: a, px: comp });
            }
        }
    }
    out
}

pub fn reduce(prep: &Prep, part: &Part, tpl: &Template) -> Result<Grid> {
    let (gw, gh) = (prep.gw, prep.gh);
    let rank = prep.rank;
    let (bw, bh) = {
        let b = prep.block(0, 0);
        ((b.2 - b.0).max(1) as i64, (b.3 - b.1).max(1) as i64)
    };
    let cell_area = bw * bh;
    // 1. silhouette
    let mut mask = alpha_mask(prep, part.reduce.thin);
    clean_silhouette(&mut mask, gw, gh);
    // pixel -> cell map
    let mut cell_of = vec![-1i32; (prep.iw * prep.ih) as usize];
    for cy in 0..gh as i32 {
        for cx in 0..gw as i32 {
            let (x0, y0, x1, y1) = prep.block(cx, cy);
            for y in y0..y1 {
                for x in x0..x1 {
                    if x >= 0 && y >= 0 && x < prep.iw && y < prep.ih {
                        cell_of[(y * prep.iw + x) as usize] = cy * gw as i32 + cx;
                    }
                }
            }
        }
    }
    // features
    let mut feats = components(prep, tpl);
    feats.sort_by(|a, b| b.area.cmp(&a.area).then(rank[a.key as usize].cmp(&rank[b.key as usize])).then(a.px[0].1.cmp(&b.px[0].1)).then(a.px[0].0.cmp(&b.px[0].0)));
    let mut is_feat = vec![false; (prep.iw * prep.ih) as usize];
    for f in &feats {
        for &(x, y) in &f.px {
            is_feat[(y * prep.iw + x) as usize] = true;
        }
    }
    // 3. fill: area majority per cell excluding feature pixels; ties by ring votes then rank
    let mut key = vec![TRANSPARENT; gw * gh];
    let mut protected = vec![false; gw * gh];
    for cy in 0..gh as i32 {
        for cx in 0..gw as i32 {
            let ci = cy as usize * gw + cx as usize;
            if !mask[ci] {
                continue;
            }
            let (x0, y0, x1, y1) = prep.block(cx, cy);
            let mut v: Vec<(u8, i64, i64)> = Vec::new(); // key, block, ring
            let mut add = |k: u8, b: i64, r: i64| match v.iter_mut().find(|e| e.0 == k) {
                Some(e) => {
                    e.1 += b;
                    e.2 += r;
                }
                None => v.push((k, b, r)),
            };
            for y in y0 - prep.ring..y1 + prep.ring {
                for x in x0 - prep.ring..x1 + prep.ring {
                    let k = prep.cls(x, y);
                    if k == TRANSPARENT || (x >= 0 && y >= 0 && x < prep.iw && y < prep.ih && is_feat[(y * prep.iw + x) as usize]) {
                        continue;
                    }
                    if x >= x0 && x < x1 && y >= y0 && y < y1 {
                        add(k, 1, 0);
                    } else {
                        add(k, 0, 1);
                    }
                }
            }
            let best = v.iter().max_by(|a, b| (a.1, a.2, std::cmp::Reverse(rank[a.0 as usize])).cmp(&(b.1, b.2, std::cmp::Reverse(rank[b.0 as usize]))));
            key[ci] = match best {
                Some(b) => b.0,
                None => {
                    // block entirely feature: its dominant feature key
                    let mut t: Vec<(u8, i64)> = Vec::new();
                    for y in y0..y1 {
                        for x in x0..x1 {
                            let k = prep.cls(x, y);
                            if k != TRANSPARENT {
                                match t.iter_mut().find(|e| e.0 == k) {
                                    Some(e) => e.1 += 1,
                                    None => t.push((k, 1)),
                                }
                            }
                        }
                    }
                    t.iter().max_by(|a, b| (a.1, std::cmp::Reverse(rank[a.0 as usize])).cmp(&(b.1, std::cmp::Reverse(rank[b.0 as usize])))).map(|e| e.0).unwrap_or(b'1')
                }
            };
        }
    }
    // 2. outline ring
    let ok = tpl.key_for_role("outline").or_else(|| tpl.layers.first().and_then(|l| l.keys.first()).map(|k| k.key)).unwrap_or(b'1');
    let mut roles: Vec<u8> = Vec::new();
    for r in &part.reduce.outline {
        match tpl.resolve(r) {
            Some(k) if k != TRANSPARENT => roles.push(k),
            _ => {
                return Err(PxlError::validation(format!("reduce.outline: '{}' is not a role or key of template '{}'", r, tpl.name))
                    .with_loc(Loc::file(part.path.slash()))
                    .with_hint("list template roles such as outline, body, skin"))
            }
        }
    }
    for y in 0..gh as i32 {
        for x in 0..gw as i32 {
            let ci = y as usize * gw + x as usize;
            if is_ring(&mask, gw, gh, x, y) && roles.contains(&key[ci]) {
                key[ci] = ok;
                protected[ci] = true;
            }
        }
    }
    // 4. features, big first
    for f in &feats {
        let n = ((f.area + cell_area / 2) / cell_area).max(1) as usize;
        let mut ov: Vec<(i32, i64)> = Vec::new();
        for &(x, y) in &f.px {
            let c = cell_of[(y * prep.iw + x) as usize];
            if c >= 0 && mask[c as usize] {
                match ov.iter_mut().find(|e| e.0 == c) {
                    Some(e) => e.1 += 1,
                    None => ov.push((c, 1)),
                }
            }
        }
        if ov.is_empty() {
            continue;
        }
        // greedy connected growth from the best cell; prefer cells not yet claimed by a feature
        let score = |e: &(i32, i64)| (e.1, std::cmp::Reverse(e.0));
        let first = *ov.iter().max_by_key(|e| score(e)).unwrap();
        let mut chosen = vec![first.0];
        while chosen.len() < n {
            let mut best: Option<(i32, i64)> = None;
            for e in &ov {
                if chosen.contains(&e.0) {
                    continue;
                }
                let (ex, ey) = (e.0 % gw as i32, e.0 / gw as i32);
                let adj = chosen.iter().any(|&c| {
                    let (cx, cy) = (c % gw as i32, c / gw as i32);
                    (cx - ex).abs() <= 1 && (cy - ey).abs() <= 1
                });
                if adj && best.map_or(true, |b| score(e) > score(&b)) {
                    best = Some(*e);
                }
            }
            match best {
                Some(b) => chosen.push(b.0),
                None => break,
            }
        }
        for c in chosen {
            key[c as usize] = f.key;
            protected[c as usize] = true;
        }
    }
    // orphan cleanup on the fill: a cell with no 4-neighbour of its own key joins its most common neighbour
    for _ in 0..2 {
        let snap = key.clone();
        let mut changed = false;
        for cy in 0..gh as i32 {
            for cx in 0..gw as i32 {
                let ci = cy as usize * gw + cx as usize;
                let k = snap[ci];
                if k == TRANSPARENT || protected[ci] {
                    continue;
                }
                let nb: Vec<u8> = N4.iter().map(|d| {
                    let (x, y) = (cx + d.0, cy + d.1);
                    if x < 0 || y < 0 || x >= gw as i32 || y >= gh as i32 { TRANSPARENT } else { snap[y as usize * gw + x as usize] }
                }).collect();
                if nb.iter().any(|&n| n == k) {
                    continue;
                }
                let mut tally: Vec<(u8, usize)> = Vec::new();
                for &n in &nb {
                    if n != TRANSPARENT {
                        match tally.iter_mut().find(|e| e.0 == n) {
                            Some(e) => e.1 += 1,
                            None => tally.push((n, 1)),
                        }
                    }
                }
                if let Some(b) = tally.iter().max_by(|a, b| (a.1, std::cmp::Reverse(rank[a.0 as usize])).cmp(&(b.1, std::cmp::Reverse(rank[b.0 as usize])))) {
                    key[ci] = b.0;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    let mut g = Grid::new(gw, gh, TRANSPARENT);
    g.cells = key;
    prep.protect_salient(&mut g, tpl);
    prep.stamp_features(&mut g, tpl, &[]);
    Ok(g)
}
