//! `tiles fit`: make a tile set satisfy `tile_colors` and `tile_palettes`.
//!
//! 1. Every tile gets one key per RGB (canonical key) and, when the template has `[palettes]`, the cheapest
//!    template palette: keys outside the palette are remapped to the nearest palette colour (Oklab, pixel weighted).
//!    Without `[palettes]` the closest pair of colours of a tile with more than 4 is merged until 4 remain.
//! 2. The checker's first-fit packing is simulated in sheet order. The tile that opens palette 9 is recoloured the
//!    cheapest way to join an existing palette (it may add colours to it); repeat until at most 8 palettes remain.

use super::stats::{self, colors_of};
use super::{Loaded, Tile, T};
use crate::cli::TilesArgs;
use crate::output::{Out, PxlError, Result};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug)]
pub struct TileFix {
    pub tile: usize,
    /// original key -> new key
    pub maps: Vec<(u8, u8)>,
    /// Sum of pixel colour errors (Oklab x 100).
    pub cost: f64,
    pub max_de: f64,
    pub px: usize,
    pub why: &'static str,
}

#[derive(Clone, Debug, Default)]
pub struct PackInfo {
    pub greedy: usize,
    pub optimal: usize,
    pub exact: bool,
    pub over4: usize,
}

pub struct Plan {
    pub fixes: Vec<TileFix>,
    pub before: PackInfo,
    pub after: PackInfo,
    pub failures: Vec<String>,
    pub tiles: Vec<Tile>,
}

fn pack_info(tiles: &[Tile], l: &Loaded) -> PackInfo {
    let sets: Vec<Vec<u32>> = tiles.iter().map(|t| colors_of(&t.px(&l.cols))).collect();
    let g = stats::greedy_sets(&sets);
    let (optimal, exact) = stats::optimal_sets(&sets);
    PackInfo { greedy: g.bins.len(), optimal, exact, over4: sets.iter().filter(|s| s.len() > 4).count() }
}

fn counts(keys: &[u8; 64]) -> BTreeMap<u8, usize> {
    let mut m = BTreeMap::new();
    for &k in keys {
        *m.entry(k).or_insert(0) += 1;
    }
    m
}

fn apply(keys: &mut [u8; 64], map: &BTreeMap<u8, u8>) {
    for k in keys.iter_mut() {
        if let Some(&n) = map.get(k) {
            *k = n;
        }
    }
}

pub fn plan(l: &Loaded, max_pal: usize) -> Plan {
    let tpl = &l.tpl;
    let cols = &l.cols;
    let air: Vec<u8> = tpl.tile_air.clone();
    let orig: Vec<Tile> = l.sheet.tiles.clone();
    let mut cur: Vec<[u8; 64]> = orig.iter().map(|t| t.keys).collect();
    let mut why: Vec<&'static str> = vec![""; cur.len()];
    let mut failures = Vec::new();
    let before = pack_info(&orig, l);

    // 0. canonical keys
    for (i, t) in cur.iter_mut().enumerate() {
        let before_keys = *t;
        for k in t.iter_mut() {
            *k = if cols.code[*k as usize] & super::TRANSP != 0 {
                air.first().copied().unwrap_or(*k)
            } else {
                cols.canon[*k as usize]
            };
        }
        if *t != before_keys {
            why[i] = "same colour, one key";
        }
    }

    // 1. palettes
    for i in 0..cur.len() {
        let cnt = counts(&cur[i]);
        let keys: Vec<u8> = cnt.keys().cloned().collect();
        if keys.len() <= 4 && tpl.palettes.iter().any(|p| keys.iter().all(|k| p.keys.contains(k))) {
            continue;
        }
        if !tpl.palettes.is_empty() {
            let has_air = keys.iter().any(|k| air.contains(k));
            let mut best: Option<(f64, usize, BTreeMap<u8, u8>)> = None;
            for (pi, p) in tpl.palettes.iter().enumerate() {
                let pair = p.keys.iter().cloned().find(|k| air.contains(k));
                if has_air && pair.is_none() {
                    continue;
                }
                let targets: Vec<u8> = p.keys.iter().cloned().filter(|k| !air.contains(k)).collect();
                let mut map = BTreeMap::new();
                let mut cost = 0.0;
                let mut ok = true;
                for (&k, &n) in &cnt {
                    if p.keys.contains(&k) {
                        continue;
                    }
                    if air.contains(&k) {
                        map.insert(k, pair.unwrap());
                        cost += n as f64 * cols.de(k, pair.unwrap());
                        continue;
                    }
                    match targets.iter().map(|&t| (cols.de(k, t), t)).min_by(|a, b| a.0.partial_cmp(&b.0).unwrap().then(a.1.cmp(&b.1))) {
                        Some((d, t)) => {
                            map.insert(k, t);
                            cost += n as f64 * d;
                        }
                        None => ok = false,
                    }
                }
                if ok && best.as_ref().map_or(true, |b| cost < b.0) {
                    best = Some((cost, pi, map));
                }
            }
            match best {
                Some((_, _, map)) => {
                    apply(&mut cur[i], &map);
                    why[i] = "palette";
                }
                None => failures.push(format!("tile #{} has air but no template palette contains an air key", i)),
            }
        } else {
            let mut cnt = cnt;
            while cnt.len() > 4 {
                let ks: Vec<u8> = cnt.keys().cloned().collect();
                let mut best: Option<(f64, u8, u8)> = None;
                for a in 0..ks.len() {
                    for b in a + 1..ks.len() {
                        // weighted: merge the rarer into the commoner
                        let (lo, hi) = if cnt[&ks[a]] <= cnt[&ks[b]] { (ks[a], ks[b]) } else { (ks[b], ks[a]) };
                        let d = cols.de(lo, hi) * cnt[&lo] as f64;
                        if best.map_or(true, |x| d < x.0) {
                            best = Some((d, lo, hi));
                        }
                    }
                }
                let (_, lo, hi) = best.unwrap();
                let mut m = BTreeMap::new();
                m.insert(lo, hi);
                apply(&mut cur[i], &m);
                let n = cnt.remove(&lo).unwrap();
                *cnt.get_mut(&hi).unwrap() += n;
                why[i] = "colours";
            }
        }
    }

    // 2. first-fit repair
    let mut guard = 0;
    loop {
        guard += 1;
        if guard > cur.len() + 16 {
            failures.push("palette repair did not converge".into());
            break;
        }
        let mut bins: Vec<BTreeSet<u8>> = Vec::new();
        let mut opener: Option<usize> = None;
        for (i, t) in cur.iter().enumerate() {
            let s: BTreeSet<u8> = t.iter().cloned().collect();
            if s.len() > 4 {
                continue;
            }
            if let Some(b) = bins.iter_mut().find(|b| b.union(&s).count() <= 4) {
                b.extend(s);
            } else {
                if bins.len() >= max_pal {
                    opener = Some(i);
                    break;
                }
                bins.push(s);
            }
        }
        let Some(t) = opener else { break };
        let cnt = counts(&cur[t]);
        let s: Vec<u8> = cnt.keys().cloned().collect();
        let mut best: Option<(f64, usize, usize, BTreeMap<u8, u8>)> = None;
        for (bi, b) in bins.iter().enumerate() {
            let rest: Vec<u8> = s.iter().cloned().filter(|k| !b.contains(k)).collect();
            for mask in 0..(1u32 << rest.len()) {
                let e: Vec<u8> = rest.iter().enumerate().filter(|(j, _)| mask >> j & 1 == 1).map(|(_, &k)| k).collect();
                if b.len() + e.len() > 4 {
                    continue;
                }
                let mut ns: BTreeSet<u8> = b.clone();
                ns.extend(e.iter().cloned());
                if s.iter().any(|k| air.contains(k) && !ns.contains(k)) {
                    continue;
                }
                let targets: Vec<u8> = ns.iter().cloned().filter(|k| !air.contains(k)).collect();
                let mut map = BTreeMap::new();
                let mut cost = 0.0;
                let mut ok = true;
                for &k in &s {
                    if ns.contains(&k) {
                        continue;
                    }
                    match targets.iter().map(|&x| (cols.de(k, x), x)).min_by(|a, b| a.0.partial_cmp(&b.0).unwrap().then(a.1.cmp(&b.1))) {
                        Some((d, x)) => {
                            map.insert(k, x);
                            cost += cnt[&k] as f64 * d;
                        }
                        None => ok = false,
                    }
                }
                if !ok {
                    continue;
                }
                let better = match &best {
                    None => true,
                    Some(b0) => (cost, e.len(), bi) < (b0.0, b0.1, b0.2),
                };
                if better {
                    best = Some((cost, e.len(), bi, map));
                }
            }
        }
        match best {
            Some((_, _, _, map)) => {
                apply(&mut cur[t], &map);
                why[t] = "first-fit";
            }
            None => {
                failures.push(format!("tile #{} cannot join any of the {} palettes", t, max_pal));
                break;
            }
        }
    }

    // fixes: original key -> final key per tile
    let mut fixes = Vec::new();
    for i in 0..cur.len() {
        if cur[i] == orig[i].keys {
            continue;
        }
        let mut map: BTreeMap<u8, u8> = BTreeMap::new();
        let (mut cost, mut max_de, mut px) = (0.0, 0.0f64, 0usize);
        for j in 0..64 {
            let (a, b) = (orig[i].keys[j], cur[i][j]);
            if a != b {
                map.insert(a, b);
                let d = cols.de(a, b);
                cost += d;
                if d > 0.0 {
                    px += 1;
                }
                max_de = max_de.max(d);
            }
        }
        fixes.push(TileFix { tile: i, maps: map.into_iter().collect(), cost, max_de, px, why: why[i] });
    }
    let tiles: Vec<Tile> = cur.iter().map(|k| Tile { keys: *k }).collect();
    let after = pack_info(&tiles, l);
    Plan { fixes, before, after, failures, tiles }
}

/// Replace ops, one per changed tile, as the `apply` JSON document.
pub fn ops_json(l: &Loaded, plan: &Plan) -> Value {
    let frame = l.part.frames[l.fi].name.clone();
    let ops: Vec<Value> = plan
        .fixes
        .iter()
        .map(|f| {
            let (x, y) = l.sheet.pos(f.tile);
            let maps: serde_json::Map<String, Value> = f.maps.iter().map(|(a, b)| ((*a as char).to_string(), json!((*b as char).to_string()))).collect();
            json!({"op": "replace", "frame": frame, "region": [x, y, T, T], "maps": maps})
        })
        .collect();
    json!({ "ops": ops })
}

fn pack_text(p: &PackInfo) -> String {
    format!("{} palettes by the checker's first-fit (optimal {}{}), {} tiles over 4 colours", p.greedy, if p.exact { "" } else { "<=" }, p.optimal, p.over4)
}

pub fn report(l: &Loaded, plan: &Plan, a: &TilesArgs, out: &mut Out) -> Result<()> {
    let visible = 3.0;
    let total: f64 = plan.fixes.iter().fold(0.0, |a, f| a + f.cost);
    let worst = plan.fixes.iter().map(|f| f.max_de).fold(0.0, f64::max);
    out.line(format!("before: {}", pack_text(&plan.before)));
    out.line(format!("after:  {}", pack_text(&plan.after)));
    out.line(format!("{} tile(s) recoloured, total colour error {:.1}, worst pixel {:.1} (Oklab x100; above {:.0} is clearly visible)", plan.fixes.len(), total, worst, visible));
    let mut shown = plan.fixes.iter().filter(|f| f.max_de > visible).collect::<Vec<_>>();
    shown.sort_by(|x, y| y.cost.partial_cmp(&x.cost).unwrap());
    for f in shown.iter().take(a.top.unwrap_or(15)) {
        let (x, y) = l.sheet.pos(f.tile);
        let maps: Vec<String> = f.maps.iter().map(|(p, q)| format!("{}->{}", *p as char, *q as char)).collect();
        out.line(format!("  tile #{} ({},{}) {}: {} px changed, worst {:.1}, error {:.1} [{}]", f.tile, x, y, maps.join(" "), f.px, f.max_de, f.cost, f.why));
    }
    if shown.len() > a.top.unwrap_or(15) {
        out.line(format!("  ... {} more visible recolours (--top N shows more)", shown.len() - a.top.unwrap_or(15)));
    }
    let doc = ops_json(l, plan);
    out.set("before", json!({"greedy": plan.before.greedy, "optimal": plan.before.optimal, "optimal_exact": plan.before.exact, "over_4_colors": plan.before.over4}));
    out.set("after", json!({"greedy": plan.after.greedy, "optimal": plan.after.optimal, "optimal_exact": plan.after.exact, "over_4_colors": plan.after.over4}));
    out.set("tiles_changed", json!(plan.fixes.len()));
    out.set("total_error", json!(total));
    out.set("worst_pixel_error", json!(worst));
    out.set(
        "fixes",
        json!(plan.fixes.iter().map(|f| json!({"tile": f.tile, "maps": f.maps.iter().map(|(p, q)| format!("{}={}", *p as char, *q as char)).collect::<Vec<_>>(), "error": f.cost, "worst": f.max_de, "pixels": f.px, "why": f.why})).collect::<Vec<_>>()),
    );
    if !plan.failures.is_empty() {
        return Err(PxlError::validation(format!("tiles fit cannot finish: {}", plan.failures.join("; "))).with_hint("add an air key to a palette, or free a palette; nothing was written"));
    }
    let max_pal = super::max_palettes(&l.part, &crate::profile::load_for(&l.part, None)?);
    if plan.after.greedy > max_pal || plan.after.over4 > 0 {
        return Err(PxlError::validation(format!("tiles fit ended with {} palettes and {} tiles over 4 colours", plan.after.greedy, plan.after.over4)));
    }
    if let Some(path) = &a.write {
        crate::fsutil::atomic_write(std::path::Path::new(path), serde_json::to_string_pretty(&doc).unwrap().as_bytes())?;
        out.line(format!("wrote {} ops to {} (apply with: pxl apply {} {})", plan.fixes.len(), path, a.tileset, path));
        out.set("ops_file", json!(path));
    }
    if a.apply {
        if plan.fixes.is_empty() {
            out.line("nothing to apply: the tile set already fits");
            return Ok(());
        }
        let ops = crate::edit::parse_ops(&doc.to_string())?;
        let flags = crate::cli::EditFlags { force_approved: a.force_approved, ..Default::default() };
        crate::edit::execute(&a.tileset, &ops, &flags, "tiles fit", out)?;
    } else if a.write.is_none() {
        out.line(format!("dry run: add --apply to edit the part, or --write ops.json  (pxl tiles fit {} --apply)", a.tileset));
    }
    Ok(())
}
