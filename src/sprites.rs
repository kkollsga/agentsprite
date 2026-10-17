//! Variable sprite sizes (M6 R2): a frame is covered with hardware sprites of the sizes a console allows (Mega Drive: 1-4 x 1-4
//! tiles). The search tries the 64 offsets of the 8x8 tile grid against the artwork and covers the occupied tiles greedily with
//! the rectangle that takes in most of them; it keeps the cover with the fewest sprites, then the least area. The result feeds the
//! `hw_limit` lint (sprites on screen, per scanline, pixels per scanline) and the `<name>.sprites.json` export.

use crate::model::Grid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placed {
    pub x: i32,
    pub y: i32,
    pub w: usize,
    pub h: usize,
}

fn tiles_of(sizes: &[(u32, u32)]) -> Vec<(usize, usize)> {
    let mut v: Vec<(usize, usize)> = sizes.iter().filter(|s| s.0 >= 8 && s.1 >= 8).map(|s| ((s.0 / 8) as usize, (s.1 / 8) as usize)).collect();
    v.sort();
    v.dedup();
    if v.is_empty() {
        v.push((1, 1));
    }
    v
}

fn cover(occ: &[Vec<bool>], tiles: &[(usize, usize)]) -> Vec<(usize, usize, usize, usize)> {
    let (rows, cols) = (occ.len(), occ.first().map_or(0, |r| r.len()));
    let mut done = vec![vec![false; cols]; rows];
    let mut out = Vec::new();
    for j in 0..rows {
        for i in 0..cols {
            if !occ[j][i] || done[j][i] {
                continue;
            }
            let mut best: Option<((usize, usize), (usize, usize))> = None; // (-gain, area), size
            for &(tw, th) in tiles {
                let mut gain = 0;
                for dj in 0..th {
                    for di in 0..tw {
                        let (y, x) = (j + dj, i + di);
                        if y < rows && x < cols && occ[y][x] && !done[y][x] {
                            gain += 1;
                        }
                    }
                }
                let key = (usize::MAX - gain, tw * th);
                if best.map_or(true, |b| key < b.0) {
                    best = Some((key, (tw, th)));
                }
            }
            let (tw, th) = best.unwrap().1;
            for dj in 0..th {
                for di in 0..tw {
                    if j + dj < rows && i + di < cols {
                        done[j + dj][i + di] = true;
                    }
                }
            }
            out.push((i, j, tw, th));
        }
    }
    out
}

/// The sprites that cover the opaque pixels of `g`, in pixel coordinates of the frame (may start left of / above 0).
pub fn place(g: &Grid, sizes: &[(u32, u32)]) -> Vec<Placed> {
    let tiles = tiles_of(sizes);
    let mut best: Option<((usize, usize), Vec<Placed>)> = None;
    for oy in 0..8i32 {
        for ox in 0..8i32 {
            let (x0, y0) = (ox - 8, oy - 8);
            let cols = (g.w as i32 - x0 + 7) as usize / 8 + 1;
            let rows = (g.h as i32 - y0 + 7) as usize / 8 + 1;
            let mut occ = vec![vec![false; cols]; rows];
            for y in 0..g.h {
                for x in 0..g.w {
                    if g.cells[y * g.w + x] != b'.' {
                        let (i, j) = (((x as i32 - x0) / 8) as usize, ((y as i32 - y0) / 8) as usize);
                        occ[j][i] = true;
                    }
                }
            }
            let rects = cover(&occ, &tiles);
            let area: usize = rects.iter().map(|r| r.2 * r.3).sum();
            let key = (rects.len(), area);
            if best.as_ref().map_or(true, |b| key < b.0) {
                let placed = rects.iter().map(|&(i, j, tw, th)| Placed { x: x0 + 8 * i as i32, y: y0 + 8 * j as i32, w: tw * 8, h: th * 8 }).collect();
                best = Some((key, placed));
            }
        }
    }
    best.map(|b| b.1).unwrap_or_default()
}

/// Per scanline y of the frame: (sprites, sprite pixels) of the cover.
pub fn line_loads(p: &[Placed], h: usize) -> Vec<(usize, usize)> {
    let mut v = vec![(0usize, 0usize); h];
    for s in p {
        for y in s.y.max(0)..(s.y + s.h as i32).min(h as i32) {
            v[y as usize].0 += 1;
            v[y as usize].1 += s.w;
        }
    }
    v
}
