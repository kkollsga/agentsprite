//! `template suggest --derive` for tile sets: eight palettes of four colours (DESIGN 4.3, 5.6).
//!
//! The items are the unique source blocks (weighted by use). Palettes are found by alternating assignment (each
//! block takes the palette with the lowest squared Oklab error over its solid pixels) and Lloyd updates (each
//! palette colour becomes the mean of the colours assigned to it, snapped to a real source colour and to the
//! 8-step RGB555 grid). `n` of the palettes carry the air colour plus three colours (blocks with air need one);
//! the others hold four colours. Every `n` is tried and the lowest error wins. Deterministic.

use crate::color::{self, Oklab, Rgb};
use crate::output::{PxlError, Result};
use std::collections::BTreeMap;

pub const AIR: u32 = u32::MAX;

struct Item {
    hist: Vec<(Rgb, Oklab, f64)>,
    has_air: bool,
    w: f64,
    mean: Oklab,
}

pub struct Derived {
    pub text: String,
    /// Palettes as RGB lists (air colour first in air palettes).
    pub palettes: Vec<Vec<Rgb>>,
    pub air_palettes: usize,
    /// Mean Oklab x 100 error per solid pixel (weighted).
    pub error: f64,
    pub lines: Vec<String>,
}

fn d2(a: Oklab, b: Oklab) -> f64 {
    color::oklab_dist2(a, b)
}

fn cost(it: &Item, cols: &[Oklab]) -> f64 {
    it.hist.iter().map(|(_, l, n)| n * cols.iter().map(|c| d2(*l, *c)).fold(f64::MAX, f64::min)).sum()
}

/// Weighted Lloyd in Oklab over (colour, weight); farthest-first start from the heaviest colour.
fn kmeans(cols: &[(Rgb, Oklab, f64)], k: usize) -> Vec<Oklab> {
    if cols.is_empty() {
        return Vec::new();
    }
    let k = k.min(cols.len());
    let heavy = (0..cols.len()).max_by(|&a, &b| cols[a].2.partial_cmp(&cols[b].2).unwrap().then(b.cmp(&a))).unwrap();
    let mut cen = vec![cols[heavy].1];
    while cen.len() < k {
        let far = (0..cols.len())
            .max_by(|&a, &b| {
                let da = cen.iter().map(|c| d2(cols[a].1, *c)).fold(f64::MAX, f64::min) * cols[a].2.sqrt();
                let db = cen.iter().map(|c| d2(cols[b].1, *c)).fold(f64::MAX, f64::min) * cols[b].2.sqrt();
                da.partial_cmp(&db).unwrap().then(b.cmp(&a))
            })
            .unwrap();
        cen.push(cols[far].1);
    }
    for _ in 0..8 {
        let mut sum = vec![(0.0, 0.0, 0.0, 0.0); k];
        for (_, l, w) in cols {
            let bi = (0..k).min_by(|&a, &b| d2(*l, cen[a]).partial_cmp(&d2(*l, cen[b])).unwrap().then(a.cmp(&b))).unwrap();
            sum[bi].0 += l.l * w;
            sum[bi].1 += l.a * w;
            sum[bi].2 += l.b * w;
            sum[bi].3 += w;
        }
        for i in 0..k {
            if sum[i].3 > 0.0 {
                cen[i] = Oklab { l: sum[i].0 / sum[i].3, a: sum[i].1 / sum[i].3, b: sum[i].2 / sum[i].3 };
            }
        }
    }
    cen
}

fn merge_hist(items: &[&Item]) -> Vec<(Rgb, Oklab, f64)> {
    let mut m: BTreeMap<Rgb, (Oklab, f64)> = BTreeMap::new();
    for it in items {
        for (c, l, n) in &it.hist {
            m.entry(*c).or_insert((*l, 0.0)).1 += n * it.w;
        }
    }
    m.into_iter().map(|(c, (l, n))| (c, l, n)).collect()
}

struct Solution {
    /// Solid colours per palette.
    pals: Vec<Vec<Rgb>>,
    air_pal: Vec<bool>,
    err: f64,
    assign: Vec<usize>,
}

fn solve(items: &[Item], n_air: usize, np: usize) -> Solution {
    // initial groups: farthest-first on block means
    let heavy = (0..items.len()).max_by(|&a, &b| items[a].w.partial_cmp(&items[b].w).unwrap().then(b.cmp(&a))).unwrap_or(0);
    let mut seeds = vec![heavy];
    while seeds.len() < np.min(items.len()) {
        let far = (0..items.len())
            .max_by(|&a, &b| {
                let da = seeds.iter().map(|&s| d2(items[a].mean, items[s].mean)).fold(f64::MAX, f64::min) * items[a].w.sqrt();
                let db = seeds.iter().map(|&s| d2(items[b].mean, items[s].mean)).fold(f64::MAX, f64::min) * items[b].w.sqrt();
                da.partial_cmp(&db).unwrap().then(b.cmp(&a))
            })
            .unwrap();
        seeds.push(far);
    }
    let np = seeds.len();
    let mut assign: Vec<usize> = items.iter().map(|it| (0..np).min_by(|&a, &b| d2(it.mean, items[seeds[a]].mean).partial_cmp(&d2(it.mean, items[seeds[b]].mean)).unwrap().then(a.cmp(&b))).unwrap()).collect();
    // air type: the groups with the largest share of blocks with air
    let mut share: Vec<(f64, usize)> = (0..np)
        .map(|p| {
            let (a, t) = items.iter().zip(&assign).filter(|(_, &g)| g == p).fold((0.0, 0.0), |s, (it, _)| (s.0 + if it.has_air { it.w } else { 0.0 }, s.1 + it.w));
            (if t > 0.0 { a / t } else { 0.0 }, p)
        })
        .collect();
    share.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap().then(a.1.cmp(&b.1)));
    let mut air_pal = vec![false; np];
    for s in share.iter().take(n_air.min(np)) {
        air_pal[s.1] = true;
    }
    let slots = |p: usize| if air_pal[p] { 3 } else { 4 };
    let mut cents: Vec<Vec<Oklab>> = vec![Vec::new(); np];
    for _round in 0..12 {
        for p in 0..np {
            let members: Vec<&Item> = items.iter().zip(&assign).filter(|(_, &g)| g == p).map(|(i, _)| i).collect();
            if !members.is_empty() {
                cents[p] = kmeans(&merge_hist(&members), slots(p));
            }
        }
        let mut changed = false;
        for (i, it) in items.iter().enumerate() {
            let mut best = (f64::MAX, assign[i]);
            for p in 0..np {
                if it.has_air && !air_pal[p] || cents[p].is_empty() {
                    continue;
                }
                let c = cost(it, &cents[p]);
                if c < best.0 {
                    best = (c, p);
                }
            }
            if best.1 != assign[i] {
                assign[i] = best.1;
                changed = true;
            }
        }
        // an empty palette is re-seeded with the worst fitting block (that can use it)
        for p in 0..np {
            if assign.iter().any(|&g| g == p) {
                continue;
            }
            let worst = (0..items.len())
                .filter(|&i| !items[i].has_air || air_pal[p])
                .filter(|&i| assign.iter().filter(|&&g| g == assign[i]).count() > 1)
                .max_by(|&a, &b| {
                    let ca = cost(&items[a], &cents[assign[a]]) * items[a].w;
                    let cb = cost(&items[b], &cents[assign[b]]) * items[b].w;
                    ca.partial_cmp(&cb).unwrap().then(b.cmp(&a))
                });
            if let Some(w) = worst {
                assign[w] = p;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    // snap: real source colour nearest to each centre, then the 8-step grid
    let mut pals: Vec<Vec<Rgb>> = Vec::new();
    for p in 0..np {
        let members: Vec<&Item> = items.iter().zip(&assign).filter(|(_, &g)| g == p).map(|(i, _)| i).collect();
        let hist = merge_hist(&members);
        let mut cols: Vec<Rgb> = Vec::new();
        for c in &cents[p] {
            if let Some((rgb, _, _)) = hist.iter().min_by(|a, b| d2(a.1, *c).partial_cmp(&d2(b.1, *c)).unwrap().then(a.0.cmp(&b.0))) {
                let s = color::snap_step(*rgb, 8);
                if !cols.contains(&s) {
                    cols.push(s);
                }
            }
        }
        pals.push(cols);
    }
    // final assignment error with the snapped palettes
    let labs: Vec<Vec<Oklab>> = pals.iter().map(|p| p.iter().map(|c| color::srgb_to_oklab(*c)).collect()).collect();
    let mut err = 0.0;
    for (i, it) in items.iter().enumerate() {
        let mut best = (f64::MAX, assign[i]);
        for p in 0..np {
            if it.has_air && !air_pal[p] || labs[p].is_empty() {
                continue;
            }
            let c = cost(it, &labs[p]);
            if c < best.0 {
                best = (c, p);
            }
        }
        assign[i] = best.1;
        err += best.0 * it.w;
    }
    Solution { pals, air_pal, err, assign }
}

/// Derive the palettes from block sources (`AIR` or colour codes, 16x16 each) with a weight per block.
pub fn derive(blocks: &[[u32; 256]], weights: &[f64], air: Rgb, name: &str) -> Result<Derived> {
    derive_air(blocks, weights, Some(air), name)
}

/// Like [`derive`]; without an air colour (screens, fonts) the template has no air key and no `[tiles]` section.
pub fn derive_air(blocks: &[[u32; 256]], weights: &[f64], air: Option<Rgb>, name: &str) -> Result<Derived> {
    let mut items = Vec::new();
    let mut seen: BTreeMap<Vec<u32>, usize> = BTreeMap::new();
    for (b, w) in blocks.iter().zip(weights) {
        if let Some(&i) = seen.get(&b.to_vec()) {
            let it: &mut Item = &mut items[i];
            it.w += 1.0 + w.sqrt();
            continue;
        }
        seen.insert(b.to_vec(), items.len());
        let mut h: BTreeMap<u32, f64> = BTreeMap::new();
        let mut has_air = false;
        for &c in b.iter() {
            if c == AIR {
                has_air = true;
            } else {
                *h.entry(c).or_insert(0.0) += 1.0;
            }
        }
        let hist: Vec<(Rgb, Oklab, f64)> = h.into_iter().map(|(c, n)| (super::rgb_of(c), color::srgb_to_oklab(super::rgb_of(c)), n)).collect();
        let tot: f64 = hist.iter().map(|x| x.2).sum();
        let mean = if tot > 0.0 { Oklab { l: hist.iter().map(|x| x.1.l * x.2).sum::<f64>() / tot, a: hist.iter().map(|x| x.1.a * x.2).sum::<f64>() / tot, b: hist.iter().map(|x| x.1.b * x.2).sum::<f64>() / tot } } else { Oklab::default() };
        items.push(Item { hist, has_air, w: 1.0 + w.sqrt(), mean });
    }
    if items.iter().all(|i| i.hist.is_empty()) {
        return Err(PxlError::validation("the blocks have no solid pixels"));
    }
    let any_air = items.iter().any(|i| i.has_air);
    // transparent source pixels need an air key; black is the stand-in when the caller gave none
    let air = air.or(if any_air { Some(Rgb::new(0, 0, 0)) } else { None });
    let mut best: Option<(usize, Solution)> = None;
    let range: Vec<usize> = if any_air { (1..=7).collect() } else { vec![0] };
    for n in range {
        let s = solve(&items, n, 8);
        if best.as_ref().map_or(true, |b| s.err < b.1.err) {
            best = Some((n, s));
        }
    }
    let (n_air, sol) = best.unwrap();
    let total_px: f64 = items.iter().map(|i| i.w * i.hist.iter().map(|h| h.2).sum::<f64>()).sum();
    let error = (sol.err / total_px.max(1.0)).sqrt() * 100.0;
    // keys: air is `a`; the rest in order of first appearance, dark to light inside a palette
    let keys_all: String = format!("{}bcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ", if air.is_some() { "" } else { "a" });
    let mut key_of: BTreeMap<Rgb, char> = BTreeMap::new();
    let mut order: Vec<(Rgb, char)> = Vec::new();
    let mut pal_keys: Vec<(String, String)> = Vec::new();
    let mut palettes_out: Vec<Vec<Rgb>> = Vec::new();
    let mut seen_sets: Vec<Vec<Rgb>> = Vec::new();
    let used: Vec<usize> = (0..sol.pals.len()).filter(|&p| sol.assign.contains(&p)).collect();
    let mut kit = keys_all.chars();
    for &p in &used {
        let mut cols = sol.pals[p].clone();
        cols.sort_by(|a, b| color::srgb_to_oklab(*a).l.partial_cmp(&color::srgb_to_oklab(*b).l).unwrap().then(a.cmp(b)));
        if let Some(a) = air {
            cols.retain(|c| *c != a);
        }
        let mut set = cols.clone();
        if let (true, Some(a)) = (sol.air_pal[p], air) {
            set.insert(0, a);
        }
        if seen_sets.contains(&set) {
            continue;
        }
        seen_sets.push(set.clone());
        let mut keys = String::new();
        if sol.air_pal[p] {
            keys.push('a');
        }
        for c in &cols {
            let k = match key_of.get(c) {
                Some(k) => *k,
                None => {
                    let k = kit.next().ok_or_else(|| PxlError::validation("more colours than keys"))?;
                    key_of.insert(*c, k);
                    order.push((*c, k));
                    k
                }
            };
            keys.push(k);
        }
        pal_keys.push((format!("p{}", pal_keys.len()), keys));
        palettes_out.push(set);
    }
    let mut text = format!("name = \"{}\"\n\n[layers.base]\n", name);
    if let Some(a) = air {
        text.push_str(&format!("\"a\" = {{ role = \"air\", color = \"{}\" }}\n", color::to_hex(a)));
    }
    for (i, (c, k)) in order.iter().enumerate() {
        text.push_str(&format!("\"{}\" = {{ role = \"c{:02}\", color = \"{}\" }}\n", k, i + 1, color::to_hex(*c)));
    }
    if air.is_some() {
        text.push_str("\n[tiles]\nair = [\"a\"]\n");
    }
    text.push_str("\n[palettes]\n");
    for (n, k) in &pal_keys {
        text.push_str(&format!("{} = \"{}\"\n", n, k));
    }
    let mut lines = vec![format!("{} unique blocks, {} palettes ({} with the air colour), {} colours; mean error {:.2} (Oklab x100) per solid pixel", items.len(), pal_keys.len(), n_air, order.len() + air.is_some() as usize, error)];
    for ((n, k), set) in pal_keys.iter().zip(&palettes_out) {
        lines.push(format!("  {} = \"{}\"  {}", n, k, set.iter().map(|c| color::to_hex(*c)).collect::<Vec<_>>().join(" ")));
    }
    Ok(Derived { text, palettes: palettes_out, air_palettes: n_air, error, lines })
}

/// `pxl template suggest PART --derive` for a tile set.
pub fn run(a: &crate::cli::SuggestArgs, part: &crate::model::Part, out: &mut crate::output::Out) -> Result<()> {
    use serde_json::json;
    let fi = super::pick_frame(part, None)?;
    let (blocks, _) = super::reduce::block_sources(part, &[], fi)?;
    let usage = vec![1u32; blocks.len()];
    let air = None;
    let w: Vec<f64> = usage.iter().map(|&u| u as f64).collect();
    let d = derive_air(&blocks, &w, air, &format!("{}_tiles", part.name))?;
    for l in &d.lines {
        out.line(l.clone());
    }
    out.set("palettes", json!(d.palettes.iter().map(|p| p.iter().map(|c| color::to_hex(*c)).collect::<Vec<_>>()).collect::<Vec<_>>()));
    out.set("air_palettes", json!(d.air_palettes));
    out.set("error", json!(d.error));
    match &a.write {
        Some(f) => {
            crate::suggest::write_new(f, &d.text)?;
            out.line(format!("wrote tile template to {}", f));
            out.set("written", json!(f));
        }
        None => out.line("(save with --write FILE)"),
    }
    Ok(())
}
