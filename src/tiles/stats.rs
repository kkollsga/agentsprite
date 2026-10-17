//! Tile statistics with the acceptance checker's own rules (`check_assets.py`, `check_tile_image`).

use super::{Colors, Sheet, TRANSP};
use std::collections::{BTreeMap, BTreeSet};

/// Distinct colour codes of a tile, sorted.
pub fn colors_of(px: &[u32; 64]) -> Vec<u32> {
    let mut v: Vec<u32> = px.to_vec();
    v.sort_unstable();
    v.dedup();
    v
}

pub fn flip(px: &[u32; 64], fx: bool, fy: bool) -> [u32; 64] {
    let mut o = *px;
    for y in 0..8 {
        for x in 0..8 {
            o[y * 8 + x] = px[(if fy { 7 - y } else { y }) * 8 + if fx { 7 - x } else { x }];
        }
    }
    o
}

/// Smallest of the four mirror variants: equal for tiles the checker counts as the same.
pub fn canon(px: &[u32; 64]) -> [u32; 64] {
    let mut best = *px;
    for (fx, fy) in [(true, false), (false, true), (true, true)] {
        let f = flip(px, fx, fy);
        if f < best {
            best = f;
        }
    }
    best
}

/// Groups of tiles that are the same tile up to mirroring, in order of first appearance.
pub fn unique_groups(sheet: &Sheet, cols: &Colors) -> Vec<Vec<usize>> {
    let mut idx: BTreeMap<[u32; 64], usize> = BTreeMap::new();
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (i, t) in sheet.tiles.iter().enumerate() {
        let c = canon(&t.px(cols));
        match idx.get(&c) {
            Some(&g) => groups[g].push(i),
            None => {
                idx.insert(c, groups.len());
                groups.push(vec![i]);
            }
        }
    }
    groups
}

/// The checker's palette packing: tiles in sheet order, each colour set joins the first palette whose union stays
/// within 4 colours, else opens a new one. Tiles with more than 4 colours are skipped (reported elsewhere).
#[derive(Clone, Debug, Default)]
pub struct Pack {
    pub bins: Vec<BTreeSet<u32>>,
    /// Tile that opened each bin.
    pub opener: Vec<usize>,
    /// Bin each tile went to (`None` for tiles over 4 colours).
    pub bin_of: Vec<Option<usize>>,
}

pub fn greedy_sets(sets: &[Vec<u32>]) -> Pack {
    greedy_sets_cap(sets, 4)
}

/// First-fit packing into palettes of `cap` colours.
pub fn greedy_sets_cap(sets: &[Vec<u32>], cap: usize) -> Pack {
    let mut p = Pack::default();
    for (i, s) in sets.iter().enumerate() {
        if s.len() > cap {
            p.bin_of.push(None);
            continue;
        }
        let mut placed = None;
        for (bi, b) in p.bins.iter_mut().enumerate() {
            if b.len() + s.iter().filter(|c| !b.contains(c)).count() <= cap {
                b.extend(s.iter().cloned());
                placed = Some(bi);
                break;
            }
        }
        match placed {
            Some(b) => p.bin_of.push(Some(b)),
            None => {
                p.bins.push(s.iter().cloned().collect());
                p.opener.push(i);
                p.bin_of.push(Some(p.bins.len() - 1));
            }
        }
    }
    p
}

pub fn greedy(sheet: &Sheet, cols: &Colors) -> Pack {
    let sets: Vec<Vec<u32>> = sheet.tiles.iter().map(|t| colors_of(&t.px(cols))).collect();
    greedy_sets(&sets)
}

/// Fewest palettes of 4 colours that hold every colour set. Exact search with a node budget; the flag says whether
/// the number is proven.
pub fn optimal_sets(sets: &[Vec<u32>]) -> (usize, bool) {
    optimal_sets_cap(sets, 4)
}

pub fn optimal_sets_cap(sets: &[Vec<u32>], cap: usize) -> (usize, bool) {
    let mut uniq: Vec<Vec<u32>> = sets.iter().filter(|s| s.len() <= cap).cloned().collect();
    uniq.sort();
    uniq.dedup();
    // drop sets contained in another set
    let all = uniq.clone();
    uniq.retain(|s| !all.iter().any(|o| o.len() > s.len() && s.iter().all(|c| o.contains(c))));
    if uniq.is_empty() {
        return (0, true);
    }
    uniq.sort_by(|a, b| b.len().cmp(&a.len()).then(a.cmp(b)));
    let upper = greedy_sets_cap(&uniq, cap).bins.len();
    let distinct: BTreeSet<u32> = uniq.iter().flatten().cloned().collect();
    let lb = distinct.len().div_ceil(cap).max(1);
    let mut budget: u64 = 4_000_000;
    let mut exact = true;
    for k in lb..upper {
        let mut bins: Vec<Vec<u32>> = Vec::new();
        match dfs(&uniq, 0, &mut bins, k, &mut budget, cap) {
            Some(true) => return (k, exact),
            Some(false) => {}
            None => exact = false,
        }
    }
    (upper, exact)
}

/// `Some(true)` found, `Some(false)` refuted, `None` budget exhausted.
fn dfs(sets: &[Vec<u32>], i: usize, bins: &mut Vec<Vec<u32>>, k: usize, budget: &mut u64, cap: usize) -> Option<bool> {
    if i == sets.len() {
        return Some(true);
    }
    if *budget == 0 {
        return None;
    }
    *budget -= 1;
    let s = &sets[i];
    let mut cut = false;
    for b in 0..bins.len() {
        let add: Vec<u32> = s.iter().filter(|c| !bins[b].contains(c)).cloned().collect();
        if bins[b].len() + add.len() <= cap {
            let old = bins[b].len();
            bins[b].extend(add);
            let r = dfs(sets, i + 1, bins, k, budget, cap);
            bins[b].truncate(old);
            match r {
                Some(true) => return Some(true),
                None => cut = true,
                _ => {}
            }
        }
    }
    if bins.len() < k {
        bins.push(s.clone());
        let r = dfs(sets, i + 1, bins, k, budget, cap);
        bins.pop();
        match r {
            Some(true) => return Some(true),
            None => cut = true,
            _ => {}
        }
    }
    if cut {
        None
    } else {
        Some(false)
    }
}

pub struct Stats {
    /// Tiles per colour count (index = colours).
    pub color_hist: Vec<usize>,
    pub over4: Vec<usize>,
    pub unique: usize,
    pub groups: Vec<Vec<usize>>,
    pub greedy: Pack,
    pub optimal: usize,
    pub optimal_exact: bool,
    /// Tiles with transparent pixels (not allowed in level tiles and screens).
    pub transparent: Vec<usize>,
}

pub fn stats(sheet: &Sheet, cols: &Colors) -> Stats {
    let sets: Vec<Vec<u32>> = sheet.tiles.iter().map(|t| colors_of(&t.px(cols))).collect();
    let mut hist = vec![0usize; 5];
    let mut over4 = Vec::new();
    let mut transparent = Vec::new();
    for (i, s) in sets.iter().enumerate() {
        if s.len() >= hist.len() {
            hist.resize(s.len() + 1, 0);
        }
        hist[s.len()] += 1;
        if s.len() > 4 {
            over4.push(i);
        }
        if s.iter().any(|c| c & TRANSP != 0) {
            transparent.push(i);
        }
    }
    let groups = unique_groups(sheet, cols);
    let greedy = greedy_sets(&sets);
    let (optimal, optimal_exact) = optimal_sets(&sets);
    Stats { color_hist: hist, over4, unique: groups.len(), groups, greedy, optimal, optimal_exact, transparent }
}
