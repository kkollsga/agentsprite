//! Block -> tile reduction for tile sets (DESIGN 5.6).
//!
//! Each 16x16 block becomes one 8x8 tile on its own (no neighbour ring), so identical blocks give identical tiles
//! and a mirrored block gives the mirrored tile (the block is reduced in its canonical orientation).
//!
//! 1. **Mask**: every 2x2 source cell is air or solid by weighted vote; the source pixels on the tile border weigh
//!    more (edge priority), so the air/solid line joins the neighbours; a tie is solid. A block that has solid
//!    pixels never becomes all air.
//! 2. **Palette**: the template palette (containing an air key when the tile has air) with the smallest squared
//!    Oklab error over the solid pixels.
//! 3. **Colours**: each solid cell takes the palette key most of its pixels are nearest to (never the air key in a
//!    tile with air); ties go to the key nearest to the cell's mean colour. Air cells take the air key.

use crate::fsutil::PathSlash;
use super::{code_of, T};
use crate::color::{self, Oklab};

use crate::model::{Grid, Part, PartKind};
use crate::output::{Loc, PxlError, Result};
use crate::template::Template;
use std::collections::HashMap;

const AIR: u32 = u32::MAX;

struct Pal {
    name: String,
    keys: Vec<(u8, Oklab)>,
    air: Option<u8>,
}

pub struct Reducer<'a> {
    tpl: &'a Template,
    /// `[reduce] thin = true`: a 2-2 tie between two colours goes to the one that is rarer in the block, so 1 px lines
    /// (inner channels, strokes) survive the halving.
    pub thin: bool,
    pals: Vec<Pal>,
    labs: HashMap<u32, Oklab>,
}

fn flip<X: Copy>(a: &[X], n: usize, fx: bool, fy: bool) -> Vec<X> {
    let mut o = a.to_vec();
    for y in 0..n {
        for x in 0..n {
            o[y * n + x] = a[(if fy { n - 1 - y } else { y }) * n + if fx { n - 1 - x } else { x }];
        }
    }
    o
}

fn weight(x: usize, y: usize) -> i64 {
    2 + (x == 0) as i64 + (x == 15) as i64 + (y == 0) as i64 + (y == 15) as i64
}

impl<'a> Reducer<'a> {
    pub fn new(tpl: &'a Template) -> Result<Reducer<'a>> {
        if tpl.palettes.is_empty() {
            return Err(PxlError::validation(format!("template '{}' has no [palettes]", tpl.name))
                .with_loc(Loc::file(tpl.path.slash()))
                .with_hint("tile sets need up to 8 palettes of 4 keys: [palettes]  grass = \"abcd\"  and  [tiles] air = [\"a\"]"));
        }
        let pals = tpl
            .palettes
            .iter()
            .map(|p| Pal {
                name: p.name.clone(),
                keys: p.keys.iter().map(|&k| (k, tpl.color_of(k).map(color::srgb_to_oklab).unwrap_or_default())).collect(),
                air: p.keys.iter().cloned().find(|k| tpl.tile_air.contains(k)),
            })
            .collect();
        Ok(Reducer { tpl, thin: false, pals, labs: HashMap::new() })
    }

    fn lab(&mut self, c: u32) -> Oklab {
        *self.labs.entry(c).or_insert_with(|| color::srgb_to_oklab(super::rgb_of(c)))
    }

    /// Reduce one block (`AIR` or a colour code per pixel, row major 16x16) to 8x8 template keys.
    pub fn reduce_block(&mut self, src: &[u32; 256]) -> Result<[u8; 64]> {
        let mut best = (0usize, src.to_vec());
        for (i, (fx, fy)) in [(false, false), (true, false), (false, true), (true, true)].iter().enumerate() {
            if i == 0 {
                continue;
            }
            let v = flip(src, 16, *fx, *fy);
            if v < best.1 {
                best = (i, v);
            }
        }
        let mut canon = [0u32; 256];
        canon.copy_from_slice(&best.1);
        let (fx, fy) = [(false, false), (true, false), (false, true), (true, true)][best.0];
        let t = self.reduce_canon(&canon)?;
        let f = flip(&t, T, fx, fy);
        let mut o = [0u8; 64];
        o.copy_from_slice(&f);
        Ok(o)
    }

    fn reduce_canon(&mut self, src: &[u32; 256]) -> Result<[u8; 64]> {
        // 1. mask
        let mut air_cell = [false; 64];
        let mut solid_w = [0i64; 64];
        let mut any_solid_px = false;
        for cy in 0..8 {
            for cx in 0..8 {
                let (mut a, mut s) = (0i64, 0i64);
                for j in 0..2 {
                    for i in 0..2 {
                        let (x, y) = (cx * 2 + i, cy * 2 + j);
                        if src[y * 16 + x] == AIR {
                            a += weight(x, y);
                        } else {
                            s += weight(x, y);
                            any_solid_px = true;
                        }
                    }
                }
                air_cell[cy * 8 + cx] = a > s;
                solid_w[cy * 8 + cx] = s;
            }
        }
        if any_solid_px && air_cell.iter().all(|&a| a) {
            let m = *solid_w.iter().max().unwrap();
            for i in 0..64 {
                if solid_w[i] == m {
                    air_cell[i] = false;
                }
            }
        }
        let has_air = air_cell.iter().any(|&a| a);
        if has_air && self.tpl.tile_air.is_empty() {
            return Err(PxlError::validation("a block has air but the template has no [tiles] air key")
                .with_loc(Loc::file(self.tpl.path.slash()))
                .with_hint("add  [tiles]  air = [\"a\"]  with the key that shows the backdrop, and put it in the palettes"));
        }
        let any_solid_cell = air_cell.iter().any(|&a| !a);
        // 2. palette
        let cands: Vec<usize> = (0..self.pals.len()).filter(|&i| !has_air || self.pals[i].air.is_some()).collect();
        if cands.is_empty() {
            return Err(PxlError::validation("no palette contains the air key, but a block has air")
                .with_loc(Loc::file(self.tpl.path.slash()))
                .with_hint("put the [tiles] air key into at least one palette"));
        }
        let allowed = |p: &Pal| -> Vec<(u8, Oklab)> { p.keys.iter().filter(|(k, _)| !has_air || !self.tpl.tile_air.contains(k)).cloned().collect() };
        let mut chosen = cands[0];
        if any_solid_cell {
            let mut bestc: Option<(f64, usize)> = None;
            for &pi in &cands {
                let al = allowed(&self.pals[pi]);
                if al.is_empty() {
                    continue;
                }
                let mut cost = 0.0;
                for cy in 0..8 {
                    for cx in 0..8 {
                        if air_cell[cy * 8 + cx] {
                            continue;
                        }
                        for j in 0..2 {
                            for i in 0..2 {
                                let (x, y) = (cx * 2 + i, cy * 2 + j);
                                let c = src[y * 16 + x];
                                if c == AIR {
                                    continue;
                                }
                                let l = self.lab(c);
                                let d = al.iter().map(|(_, kl)| color::oklab_dist2(l, *kl)).fold(f64::MAX, f64::min);
                                cost += d * weight(x, y) as f64;
                            }
                        }
                    }
                }
                if bestc.map_or(true, |b| cost < b.0) {
                    bestc = Some((cost, pi));
                }
            }
            chosen = bestc.map(|b| b.1).ok_or_else(|| PxlError::validation(format!("palette '{}' has only the air key", self.pals[cands[0]].name)))?;
        }
        // 3. colours
        let al = allowed(&self.pals[chosen]);
        let air_key = self.pals[chosen].air.unwrap_or(self.tpl.tile_air.first().copied().unwrap_or(b'.'));
        let mut out = [air_key; 64];
        let mut empty: Vec<usize> = Vec::new();
        // pixels of the whole block nearest to each key (the `thin` tie break)
        let mut blk = vec![0i64; al.len()];
        if self.thin {
            for &c in src.iter() {
                if c == AIR {
                    continue;
                }
                let l = self.lab(c);
                if let Some(bi) = (0..al.len()).min_by(|&a, &b| color::oklab_dist2(l, al[a].1).partial_cmp(&color::oklab_dist2(l, al[b].1)).unwrap()) {
                    blk[bi] += 1;
                }
            }
        }
        for cy in 0..8 {
            for cx in 0..8 {
                if air_cell[cy * 8 + cx] {
                    continue;
                }
                let mut votes = vec![0i64; al.len()];
                let (mut ml, mut ma, mut mb, mut n) = (0.0, 0.0, 0.0, 0.0);
                for j in 0..2 {
                    for i in 0..2 {
                        let (x, y) = (cx * 2 + i, cy * 2 + j);
                        let c = src[y * 16 + x];
                        if c == AIR {
                            continue;
                        }
                        let l = self.lab(c);
                        let mut bi = 0;
                        let mut bd = f64::MAX;
                        for (ki, (_, kl)) in al.iter().enumerate() {
                            let d = color::oklab_dist2(l, *kl);
                            if d < bd {
                                bd = d;
                                bi = ki;
                            }
                        }
                        votes[bi] += weight(x, y);
                        ml += l.l;
                        ma += l.a;
                        mb += l.b;
                        n += 1.0;
                    }
                }
                if n == 0.0 {
                    // a cell with no solid source pixel: the colour of the nearest drawn cell
                    empty.push(cy * 8 + cx);
                    continue;
                }
                let mx = *votes.iter().max().unwrap();
                let mean = Oklab { l: ml / n, a: ma / n, b: mb / n };
                let mut pick = 0;
                let mut pd = f64::MAX;
                let rare = (0..al.len()).filter(|&k| votes[k] == mx).map(|k| blk[k]).min().unwrap_or(0);
                for (ki, (_, kl)) in al.iter().enumerate() {
                    if votes[ki] == mx && (!self.thin || blk[ki] == rare) {
                        let d = color::oklab_dist2(mean, *kl);
                        if d < pd {
                            pd = d;
                            pick = ki;
                        }
                    }
                }
                out[cy * 8 + cx] = al[pick].0;
            }
        }
        for &k in &empty {
            let (kx, ky) = ((k % 8) as i32, (k / 8) as i32);
            let near = (0..64)
                .filter(|&j| !air_cell[j] && !empty.contains(&j))
                .min_by_key(|&j| ((j % 8) as i32 - kx).abs() + ((j / 8) as i32 - ky).abs());
            out[k] = near.map(|j| out[j]).unwrap_or_else(|| al.first().map(|a| a.0).unwrap_or(air_key));
        }
        Ok(out)
    }
}

/// Source pixels of every block of a frame: `AIR` or a colour code.
pub fn block_sources(part: &Part, air_codes: &[u32], fi: usize) -> Result<(Vec<[u32; 256]>, usize)> {
    let frame = &part.frames[fi];
    let rf = crate::load::reference(part, &part.dir(), frame)?;
    let img = &rf.img;
    // factor 2: the reference is the native picture (16x16 per tile); factor 1: it is already tile sized (fonts, walls)
    // and every pixel is doubled so the same reducer gives it back exactly.
    let f = part.reduce.factor.unwrap_or(2) as usize;
    if f != 1 && f != 2 {
        return Err(PxlError::validation(format!("[reduce] factor = {} but tile sets and screens reduce 16x16 to 8x8 (factor 2) or take 8x8 as is (factor 1)", f)).with_loc(Loc::file(part.path.slash())));
    }
    let edge = 8 * f;
    if img.w % edge != 0 || img.h % edge != 0 {
        return Err(PxlError::validation(format!("tile reference is {}x{}, not a multiple of {}", img.w, img.h, edge)).with_loc(Loc::file(part.path.slash()).frame(frame.name.clone())));
    }
    let per_row = img.w / edge;
    let mut blocks = Vec::new();
    for by in 0..img.h / edge {
        for bx in 0..per_row {
            let mut s = [AIR; 256];
            for y in 0..16 {
                for x in 0..16 {
                    let p = img.get((bx * edge + x / (3 - f)) as i32, (by * edge + y / (3 - f)) as i32);
                    let c = code_of(p.rgb());
                    if p.a >= 128 && !air_codes.contains(&c) {
                        s[y * 16 + x] = c;
                    }
                }
            }
            blocks.push(s);
        }
    }
    Ok((blocks, per_row))
}

/// Reduce frame `fi` of a tile set: tile i = block i, `per_row` tiles per row .
pub fn reduce_frame(part: &Part, tpl: &Template, fi: usize) -> Result<Grid> {
    debug_assert!(part.kind != PartKind::Sprite);
    let (blocks, per_row) = block_sources(part, &tpl.tile_air.iter().filter_map(|&k| tpl.color_of(k)).map(code_of).collect::<Vec<u32>>(), fi)?;
    let mut r = Reducer::new(tpl)?;
    r.thin = part.reduce.thin;
    let air = tpl.tile_air.first().copied().unwrap_or(b'.');
    let rows = blocks.len().div_ceil(per_row).max(1);
    let mut g = Grid::new(per_row * T, rows * T, air);
    let mut cache: HashMap<[u32; 256], [u8; 64]> = HashMap::new();
    for (i, b) in blocks.iter().enumerate() {
        let t = match cache.get(b) {
            Some(t) => *t,
            None => {
                let t = r.reduce_block(b).map_err(|e| {
                    let e = if e.loc.is_none() { e.with_loc(Loc::file(part.path.slash()).frame(part.frames[fi].name.clone())) } else { e };
                    PxlError { msg: format!("block {}: {}", i, e.msg), ..e }
                })?;
                cache.insert(*b, t);
                t
            }
        };
        let (px, py) = ((i % per_row) * T, (i / per_row) * T);
        for y in 0..T {
            for x in 0..T {
                g.cells[(py + y) * g.w + px + x] = t[y * T + x];
            }
        }
    }
    Ok(g)
}

/// Palette index (in the template) a tile of keys fits, if any: the first palette holding all its keys.
pub fn palette_of(tpl: &Template, keys: &[u8]) -> Option<usize> {
    tpl.palettes.iter().position(|p| keys.iter().all(|k| p.keys.contains(k)))
}
