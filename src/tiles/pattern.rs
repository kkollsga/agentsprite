//! Palette-independent tile identity (M5 phase T3).
//!
//! On the hardware a tile is a 2-bit index pattern plus a palette attribute, so two tiles with the same index pattern
//! (mirrors included) are one tile even when their palettes differ. The pattern depends on the order of the colours
//! inside each palette. The order is the order of the colours in the exported PLTE, so the exporter picks one global
//! order that makes as many patterns equal as possible:
//!
//! * every tile belongs to the palette the checker's first-fit packing gives it (`stats::greedy_sets`);
//! * inside a palette the colours sit in global order; the index of a pixel is the rank of its colour in its palette;
//! * the global order starts with the air colours (they must be colour 0 of their palette, see `priority_color0`), then
//!   the colours by lightness (the same role gets the same index in different hues), then a deterministic local search
//!   moves single colours to the position that lowers the number of distinct patterns (two passes).
//!
//! A checker that reads blocks.png can verify the result: pack the colour sets first-fit in sheet order, index every
//! colour by its PLTE position inside its palette, canonicalise over the four mirrors, count.

use super::stats::{self, canon};
use super::{Colors, Sheet, TRANSP};
use crate::color;
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Clone, Debug, Default)]
pub struct Model {
    /// Colour codes in PLTE order (air first).
    pub order: Vec<u32>,
    /// Palette colours in index order, per palette (first-fit bins).
    pub bins: Vec<Vec<u32>>,
    pub bin_of: Vec<Option<usize>>,
    /// Index pattern per tile (`None` for tiles over 4 colours).
    pub pats: Vec<Option<[u8; 64]>>,
    /// Groups of tiles that are the same tile, in order of first appearance.
    pub groups: Vec<Vec<usize>>,
}

pub fn canon_pat(p: &[u8; 64]) -> [u8; 64] {
    let flip = |fx: bool, fy: bool| -> [u8; 64] {
        let mut o = [0u8; 64];
        for y in 0..8 {
            for x in 0..8 {
                o[y * 8 + x] = p[(if fy { 7 - y } else { y }) * 8 + if fx { 7 - x } else { x }];
            }
        }
        o
    };
    let mut best = *p;
    for (fx, fy) in [(true, false), (false, true), (true, true)] {
        let f = flip(fx, fy);
        if f < best {
            best = f;
        }
    }
    best
}

fn pattern_of(codes: &[u32; 64], bin: &[u32]) -> [u8; 64] {
    let mut o = [0u8; 64];
    for i in 0..64 {
        o[i] = bin.iter().position(|c| *c == codes[i]).unwrap_or(0) as u8;
    }
    o
}

/// Palette colours of bin `b` in the order of `pos`.
fn bin_in_order(set: &BTreeSet<u32>, pos: &HashMap<u32, usize>) -> Vec<u32> {
    let mut v: Vec<u32> = set.iter().copied().collect();
    v.sort_by_key(|c| pos[c]);
    v
}

pub fn build(sheet: &Sheet, cols: &Colors, air: &[u32], optimise: bool) -> Model {
    let n = sheet.tiles.len();
    let codes: Vec<[u32; 64]> = sheet.tiles.iter().map(|t| t.px(cols)).collect();
    let sets: Vec<Vec<u32>> = codes.iter().map(|c| stats::colors_of(c)).collect();
    let pack = stats::greedy_sets(&sets);
    let mut distinct: BTreeSet<u32> = BTreeSet::new();
    for (i, b) in pack.bin_of.iter().enumerate() {
        if b.is_some() {
            distinct.extend(sets[i].iter().copied());
        }
    }
    let lightness = |c: u32| -> f64 {
        if c & TRANSP != 0 {
            -1.0
        } else {
            color::srgb_to_oklab(super::rgb_of(c)).l
        }
    };
    let mut order: Vec<u32> = air.iter().copied().filter(|c| distinct.contains(c)).collect();
    order.dedup();
    let pinned = order.len();
    let mut rest: Vec<u32> = distinct.iter().copied().filter(|c| !order.contains(c)).collect();
    rest.sort_by(|a, b| lightness(*a).partial_cmp(&lightness(*b)).unwrap().then(a.cmp(b)));
    order.extend(rest);
    let pos_of = |order: &[u32]| -> HashMap<u32, usize> { order.iter().enumerate().map(|(i, c)| (*c, i)).collect() };
    // tiles per bin and per colour
    let tiles_of_bin: Vec<Vec<usize>> = (0..pack.bins.len()).map(|b| (0..n).filter(|&i| pack.bin_of[i] == Some(b)).collect()).collect();
    let bins_with: BTreeMap<u32, Vec<usize>> = {
        let mut m: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
        for (b, set) in pack.bins.iter().enumerate() {
            for c in set {
                m.entry(*c).or_default().push(b);
            }
        }
        m
    };
    let patterns = |order: &[u32]| -> Vec<Option<[u8; 64]>> {
        let pos = pos_of(order);
        let bins: Vec<Vec<u32>> = pack.bins.iter().map(|s| bin_in_order(s, &pos)).collect();
        (0..n).map(|i| pack.bin_of[i].map(|b| pattern_of(&codes[i], &bins[b]))).collect()
    };
    if optimise && order.len() > pinned + 1 {
        let mut pats = patterns(&order);
        let mut counts: HashMap<[u8; 64], u32> = HashMap::new();
        for p in pats.iter().flatten() {
            *counts.entry(canon_pat(p)).or_insert(0) += 1;
        }
        for _pass in 0..2 {
            let mut improved = false;
            let colours: Vec<u32> = order[pinned..].to_vec();
            for c in colours {
                let cur = order.iter().position(|x| *x == c).unwrap();
                let affected: Vec<usize> = bins_with[&c].iter().flat_map(|b| tiles_of_bin[*b].iter().copied()).collect();
                let mut best: Option<(usize, Vec<Option<[u8; 64]>>, HashMap<[u8; 64], u32>)> = None;
                let mut best_u = counts.len();
                for p in pinned..order.len() {
                    if p == cur {
                        continue;
                    }
                    let mut cand = order.clone();
                    cand.remove(cur);
                    cand.insert(p, c);
                    let pos = pos_of(&cand);
                    let mut bins_cache: HashMap<usize, Vec<u32>> = HashMap::new();
                    let mut cnt = counts.clone();
                    let mut newp = pats.clone();
                    for &t in &affected {
                        let b = pack.bin_of[t].unwrap();
                        let bin = bins_cache.entry(b).or_insert_with(|| bin_in_order(&pack.bins[b], &pos));
                        let np = pattern_of(&codes[t], bin);
                        if Some(np) != pats[t] {
                            let oc = canon_pat(&pats[t].unwrap());
                            if let Some(v) = cnt.get_mut(&oc) {
                                *v -= 1;
                                if *v == 0 {
                                    cnt.remove(&oc);
                                }
                            }
                            *cnt.entry(canon_pat(&np)).or_insert(0) += 1;
                            newp[t] = Some(np);
                        }
                    }
                    if cnt.len() < best_u {
                        best_u = cnt.len();
                        best = Some((p, newp, cnt));
                    }
                }
                if let Some((p, newp, cnt)) = best {
                    order.remove(cur);
                    order.insert(p, c);
                    pats = newp;
                    counts = cnt;
                    improved = true;
                }
            }
            if !improved {
                break;
            }
        }
    }
    let pos = pos_of(&order);
    let bins: Vec<Vec<u32>> = pack.bins.iter().map(|s| bin_in_order(s, &pos)).collect();
    let pats = patterns(&order);
    // groups
    let mut idx_p: HashMap<[u8; 64], usize> = HashMap::new();
    let mut idx_c: HashMap<[u32; 64], usize> = HashMap::new();
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for i in 0..n {
        let g = match &pats[i] {
            Some(p) => *idx_p.entry(canon_pat(p)).or_insert_with(|| {
                groups.push(Vec::new());
                groups.len() - 1
            }),
            None => *idx_c.entry(canon(&codes[i])).or_insert_with(|| {
                groups.push(Vec::new());
                groups.len() - 1
            }),
        };
        groups[g].push(i);
    }
    Model { order, bins, bin_of: pack.bin_of, pats, groups }
}

/// Groups of equal tiles by the profile's rule: index patterns (`pattern`) or pixel colours (the old rule).
pub fn groups(sheet: &Sheet, cols: &Colors, air: &[u32], pattern: bool) -> Vec<Vec<usize>> {
    if pattern {
        build(sheet, cols, air, true).groups
    } else {
        stats::unique_groups(sheet, cols)
    }
}
