//! `pxl pattern suggest PART FRAME`: find the repeating unit of a texture in the reference and propose it as a
//! `pxl pattern` snippet.
//!
//! * The reference (native resolution) of the area is colour-classified (greedy Oklab clusters, so noise and
//!   anti-aliasing fold into the main colours).
//! * For every candidate target period `tw x th` (native `tw*f x th*f`, `f` = the part's factor) the
//!   agreement of the area with itself shifted by one period (horizontally and vertically) is measured. The
//!   smallest period whose agreement is within a small margin of the best one wins, so multiples of a period
//!   never beat the period itself.
//! * The unit is then built by majority over every repeat, per target pixel, and mapped to template keys.
//! * Whole tiles (`--tile N`, `--tiles 37-40`) are read as blocks that wrap around (periods must divide the tile);
//!   a region (default: the whole frame) is read as a flat area.
//!
//! The saved snippet is rotated so that the command's default phase (the part origin for sprites, 0,0 for tile
//! sets and screens) reproduces the reference lattice; the original lattice phase is printed too.

use crate::fsutil::PathSlash;
use crate::cli::{PatternArgs, Region};
use crate::color::{self, Oklab, Rgb};
use crate::img::RgbaImage;
use crate::model::PartKind;
use crate::output::{Out, PxlError, Result};
use crate::template::Template;
use crate::{compare, fsutil, load, metrics, pxl, snippet, template};
use serde_json::json;

/// One block of reference pixels analysed on its own.
struct Patch {
    w: usize,
    h: usize,
    /// Colour class per pixel, `NONE` = transparent.
    cls: Vec<u8>,
    /// Block wraps around (a tile) instead of being a flat area.
    cyclic: bool,
}

const NONE: u8 = 255;
const MAX_CLASSES: usize = 12;
/// Oklab distance (x100) under which a colour joins an existing class.
const JOIN: f64 = 7.0;
/// A period counts as good when its agreement is within this of the best.
const MARGIN: f64 = 0.04;

/// Cluster colours (greedy in reading order, then one refinement pass). Returns classes as mean colours.
fn classify(pix: &[Vec<(usize, Rgb)>]) -> (Vec<Rgb>, Vec<Vec<u8>>) {
    let lab = |c: Rgb| color::srgb_to_oklab(c);
    let mut cent: Vec<(Oklab, [f64; 3], usize)> = Vec::new();
    let near = |cent: &[(Oklab, [f64; 3], usize)], o: Oklab| -> (usize, f64) {
        cent.iter().enumerate().map(|(i, c)| (i, color::oklab_dist2(c.0, o).sqrt() * 100.0)).fold((usize::MAX, f64::MAX), |b, x| if x.1 < b.1 { x } else { b })
    };
    for patch in pix {
        for &(_, c) in patch {
            let o = lab(c);
            let (i, d) = near(&cent, o);
            if i == usize::MAX || (d > JOIN && cent.len() < MAX_CLASSES) {
                cent.push((o, [c.r as f64, c.g as f64, c.b as f64], 1));
            } else {
                let e = &mut cent[i];
                e.2 += 1;
                for k in 0..3 {
                    e.1[k] += ([c.r, c.g, c.b][k] as f64 - e.1[k]) / e.2 as f64;
                }
                e.0 = lab(Rgb { r: e.1[0].round() as u8, g: e.1[1].round() as u8, b: e.1[2].round() as u8 });
            }
        }
    }
    let cols: Vec<Rgb> = cent.iter().map(|c| Rgb { r: c.1[0].round() as u8, g: c.1[1].round() as u8, b: c.1[2].round() as u8 }).collect();
    let cl: Vec<Oklab> = cols.iter().map(|&c| lab(c)).collect();
    let out = pix
        .iter()
        .map(|patch| {
            patch
                .iter()
                .map(|&(_, c)| {
                    let o = lab(c);
                    (0..cl.len()).min_by(|&a, &b| color::oklab_dist2(cl[a], o).partial_cmp(&color::oklab_dist2(cl[b], o)).unwrap()).unwrap_or(0) as u8
                })
                .collect()
        })
        .collect();
    (cols, out)
}

/// Agreement of the patches with themselves shifted by `n` along one axis: (agreeing pairs, pairs).
fn shift_agree(patches: &[Patch], n: usize, horizontal: bool) -> (usize, usize) {
    let (mut ok, mut all) = (0usize, 0usize);
    for p in patches {
        let (len, other) = if horizontal { (p.w, p.h) } else { (p.h, p.w) };
        if p.cyclic && n % len == 0 {
            continue; // a shift by the whole tile compares it with itself: no evidence
        }
        let at = |a: usize, b: usize| if horizontal { p.cls[b * p.w + a] } else { p.cls[a * p.w + b] };
        for a in 0..len {
            let a2 = if p.cyclic {
                (a + n) % len
            } else if a + n < len {
                a + n
            } else {
                continue;
            };
            for b in 0..other {
                let (c1, c2) = (at(a, b), at(a2, b));
                if c1 != NONE && c2 != NONE {
                    all += 1;
                    ok += (c1 == c2) as usize;
                }
            }
        }
    }
    (ok, all)
}

#[derive(Clone, Debug)]
struct Cand {
    tw: usize,
    th: usize,
    agree: f64,
    repeats: f64,
}

fn candidates(patches: &[Patch], f: usize, max_p: usize) -> Vec<Cand> {
    let Some(first) = patches.first() else { return Vec::new() };
    let cyclic = first.cyclic;
    let ok_dim = |t: usize, len: usize| -> bool {
        if cyclic {
            len % (t * f) == 0
        } else {
            len >= 2 * t * f
        }
    };
    let wmin = patches.iter().map(|p| p.w).min().unwrap_or(0);
    let hmin = patches.iter().map(|p| p.h).min().unwrap_or(0);
    let xs: Vec<(usize, (usize, usize))> = (1..=max_p).filter(|&t| ok_dim(t, wmin)).map(|t| (t, shift_agree(patches, t * f, true))).collect();
    let ys: Vec<(usize, (usize, usize))> = (1..=max_p).filter(|&t| ok_dim(t, hmin)).map(|t| (t, shift_agree(patches, t * f, false))).collect();
    let mut v = Vec::new();
    for &(tw, (ox, ax)) in &xs {
        for &(th, (oy, ay)) in &ys {
            let all = ax + ay;
            if all == 0 {
                continue;
            }
            let agree = (ox + oy) as f64 / all as f64;
            let repeats: f64 = patches.iter().map(|p| if p.cyclic { (p.w / (tw * f) * (p.h / (th * f))) as f64 } else { (p.w / (tw * f)) as f64 * (p.h / (th * f)) as f64 }).sum();
            v.push(Cand { tw, th, agree, repeats });
        }
    }
    v
}

/// The chosen period and the next best ones (not multiples of it).
fn choose(c: &[Cand]) -> Option<(Cand, Vec<Cand>)> {
    let best = c.iter().map(|x| x.agree).fold(f64::MIN, f64::max);
    if best <= f64::MIN {
        return None;
    }
    let good = |x: &&Cand| x.agree >= best - MARGIN;
    let pick = c
        .iter()
        .filter(good)
        .min_by(|a, b| (a.tw * a.th).cmp(&(b.tw * b.th)).then(b.agree.partial_cmp(&a.agree).unwrap()).then(a.tw.cmp(&b.tw)))?
        .clone();
    let mut alt: Vec<Cand> = c
        .iter()
        .filter(|x| !(x.tw % pick.tw == 0 && x.th % pick.th == 0) && x.agree >= best - 0.08)
        .cloned()
        .collect();
    alt.sort_by(|a, b| (a.tw * a.th).cmp(&(b.tw * b.th)).then(b.agree.partial_cmp(&a.agree).unwrap()));
    // keep alternatives that are not multiples of each other
    let mut out: Vec<Cand> = Vec::new();
    for a in alt {
        if out.iter().all(|o| !(a.tw % o.tw == 0 && a.th % o.th == 0)) {
            out.push(a);
        }
        if out.len() >= 2 {
            break;
        }
    }
    Some((pick, out))
}

/// Parse `37-40,52` into tile numbers.
pub fn parse_ranges(s: &str) -> Result<Vec<usize>> {
    let mut v = Vec::new();
    for part in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let bad = || PxlError::usage(format!("tile range '{}' is not N or N-M", part));
        match part.split_once('-') {
            Some((a, b)) => {
                let (a, b): (usize, usize) = (a.trim().parse().map_err(|_| bad())?, b.trim().parse().map_err(|_| bad())?);
                if a > b {
                    return Err(bad());
                }
                v.extend(a..=b);
            }
            None => v.push(part.parse().map_err(|_| bad())?),
        }
    }
    if v.is_empty() {
        return Err(PxlError::usage("--tiles needs at least one tile"));
    }
    Ok(v)
}

fn nearest_key(tpl: &Template, c: Rgb) -> Option<u8> {
    let o = color::srgb_to_oklab(c);
    tpl.keys().filter_map(|d| d.color.map(|col| (d.key, color::oklab_dist2(color::srgb_to_oklab(col), o)))).min_by(|a, b| a.1.partial_cmp(&b.1).unwrap()).map(|x| x.0)
}

pub fn run_suggest(part_arg: &str, frame_arg: &str, a: &PatternArgs, out: &mut Out) -> Result<()> {
    let path = fsutil::resolve_part(part_arg)?;
    let part = pxl::load(&path)?;
    let fi = part.frame_index(frame_arg).ok_or_else(|| crate::cli::unknown_frame(&part, frame_arg))?;
    let frame = &part.frames[fi];
    let tpl = template::load_for(&part).map_err(|e| PxlError::usage(format!("pattern suggest needs the part's template to name the keys: {}", e.msg)))?;
    let max_p = a.max_period.unwrap_or(16).clamp(1, 64);
    let g = &frame.grid;
    // the reference, or (no reference) the part's own pixels at factor 1
    let rf = load::reference(&part, &std::fs::canonicalize(part.dir()).unwrap_or_else(|_| part.dir()), frame);
    let (img, org, f, own): (RgbaImage, (i32, i32), usize, bool) = match rf {
        Ok(rf) => {
            let geo = metrics::geometry(&part, &rf);
            (rf.img, geo.org, geo.f.max(1) as usize, false)
        }
        Err(e) => {
            if a.flags.reference {
                return Err(e);
            }
            out.warn(format!("no reference ({}); analysing the part's own pixels", e.msg));
            let mut im = RgbaImage::new(g.w, g.h);
            for y in 0..g.h {
                for x in 0..g.w {
                    let k = g.cells[y * g.w + x];
                    if k != b'.' {
                        let c = compare::key_color(&part, Some(&tpl), k);
                        im.px[y * g.w + x] = crate::img::Rgba::opaque(c.r, c.g, c.b);
                    }
                }
            }
            (im, (0, 0), 1, true)
        }
    };
    // areas: tiles (cyclic blocks) or one region
    let tile_mode = a.tiles.is_some() || (a.flags.tile.is_some() && a.region.is_none());
    if (a.tiles.is_some() || a.flags.tile.is_some()) && part.kind == PartKind::Sprite {
        return Err(PxlError::usage("--tile/--tiles need a tile set or screen part"));
    }
    let mut areas: Vec<(i32, i32, usize, usize, bool)> = Vec::new(); // target x,y,w,h, cyclic
    let mut tiles: Vec<usize> = Vec::new();
    let reg: Region;
    if tile_mode {
        tiles = match &a.tiles {
            Some(s) => parse_ranges(s)?,
            None => vec![a.flags.tile.unwrap()],
        };
        for &t in &tiles {
            let tr = crate::tileio::tile_ref(frame, t)?;
            areas.push((tr.ox, tr.oy, 8, 8, true));
        }
        reg = Region { x: 0, y: 0, w: 8, h: 8 };
    } else {
        let (ox, oy) = match a.flags.tile {
            Some(t) => {
                let tr = crate::tileio::tile_ref(frame, t)?;
                (tr.ox, tr.oy)
            }
            None => (0, 0),
        };
        let (lw, lh) = if a.flags.tile.is_some() { (8, 8) } else { (g.w, g.h) };
        reg = a.region.unwrap_or(Region { x: 0, y: 0, w: lw, h: lh });
        if reg.w == 0 || reg.h == 0 || reg.x < 0 || reg.y < 0 || reg.x as usize + reg.w > lw || reg.y as usize + reg.h > lh {
            return Err(PxlError::validation(format!("region {},{},{},{} is outside the {}x{} area of frame {}", reg.x, reg.y, reg.w, reg.h, lw, lh, frame.name)).with_hint("--region is x,y,w,h in target pixels (tile coordinates with --tile)"));
        }
        areas.push((ox + reg.x, oy + reg.y, reg.w, reg.h, false));
    }
    // native pixels per area
    let mut raw: Vec<Vec<(usize, Rgb)>> = Vec::new();
    for &(x, y, w, h, _) in &areas {
        let (nx, ny) = (org.0 + f as i32 * x, org.1 + f as i32 * y);
        let mut v = Vec::new();
        for j in 0..h * f {
            for i in 0..w * f {
                let p = img.get(nx + i as i32, ny + j as i32);
                if p.a >= 128 {
                    v.push((j * w * f + i, p.rgb()));
                }
            }
        }
        raw.push(v);
    }
    let (cols, classes) = classify(&raw);
    let mut patches: Vec<Patch> = Vec::new();
    for (k, &(_, _, w, h, cyc)) in areas.iter().enumerate() {
        let mut cls = vec![NONE; w * f * h * f];
        for (&(pos, _), &c) in raw[k].iter().zip(&classes[k]) {
            cls[pos] = c;
        }
        patches.push(Patch { w: w * f, h: h * f, cls, cyclic: cyc });
    }
    let used: Vec<usize> = (0..cols.len()).map(|c| patches.iter().map(|p| p.cls.iter().filter(|&&x| x as usize == c).count()).sum()).collect();
    let total: usize = used.iter().sum();
    if total == 0 {
        return Err(PxlError::validation("the area has no opaque reference pixels").with_hint("pick an --region over the textured area"));
    }
    let keys: Vec<u8> = cols.iter().map(|&c| nearest_key(&tpl, c).unwrap_or(b'.')).collect();
    let classes_json: Vec<serde_json::Value> = (0..cols.len()).map(|c| json!({"color": color::to_hex(cols[c]), "key": (keys[c] as char).to_string(), "share": (used[c] as f64 / total as f64 * 1000.0).round() / 1000.0})).collect();
    let flat = used.iter().filter(|&&n| n > 0).count() == 1;
    let cands = candidates(&patches, f, max_p);
    let Some((best, alts)) = choose(&cands).filter(|_| !flat) else {
        let why = if flat { "the area is one flat colour: no texture to repeat" } else { "no repeat found: the area is too small for --max-period, or (tiles) nothing repeats inside the tile" };
        out.emit(format!("pattern suggest {}/{}: {}", part.name, frame.name, why), json!({"period": null, "reason": why, "classes": classes_json}));
        return Ok(());
    };
    // unit by majority over every repeat
    let (tw, th) = (best.tw, best.th);
    let mut unit = vec![vec![b' '; tw]; th];
    for v in 0..th {
        for u in 0..tw {
            let mut votes = vec![0usize; cols.len()];
            for p in &patches {
                let (nxr, nyr) = (p.w / (tw * f), p.h / (th * f));
                for ky in 0..nyr.max(1) {
                    for kx in 0..nxr.max(1) {
                        for j in 0..f {
                            for i in 0..f {
                                let (x, y) = (kx * tw * f + u * f + i, ky * th * f + v * f + j);
                                if x < p.w && y < p.h && p.cls[y * p.w + x] != NONE {
                                    votes[p.cls[y * p.w + x] as usize] += 1;
                                }
                            }
                        }
                    }
                }
            }
            if let Some((c, n)) = votes.iter().enumerate().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(&a.0))) {
                if *n > 0 {
                    unit[v][u] = keys[c];
                }
            }
        }
    }
    // lattice phase of this unit (absolute target coordinates; tile-relative with --tile) and the unit rotated to the
    // default phase of `pxl pattern`
    let (px, py) = if tile_mode { (0, 0) } else { (reg.x.rem_euclid(tw as i32), reg.y.rem_euclid(th as i32)) };
    let (dx, dy) = if a.flags.tile.is_none() && !tile_mode && part.kind == PartKind::Sprite { part.origin } else { (0, 0) };
    let aligned: Vec<String> = (0..th as i32)
        .map(|j| (0..tw as i32).map(|i| unit[(dy + j - py).rem_euclid(th as i32) as usize][(dx + i - px).rem_euclid(tw as i32) as usize] as char).collect())
        .collect();
    let conf = best.agree * ((best.repeats - 1.0) / 3.0).clamp(0.0, 1.0);
    let name = a.save.as_deref().map(|n| format!("pattern_{}", n));
    let cmd_name = name.clone().unwrap_or_else(|| "pattern_NAME".into());
    let target = if tile_mode {
        let t = tiles.first().copied().unwrap_or(0);
        format!("--tile {}", t)
    } else if let Some(t) = a.flags.tile {
        format!("--tile {} --region {},{},{},{}", t, reg.x, reg.y, reg.w, reg.h)
    } else {
        format!("--region {},{},{},{}", reg.x, reg.y, reg.w, reg.h)
    };
    let cmd = format!("pxl pattern {} {} {} {}", part_arg, frame_arg, cmd_name, target);
    let saved = match &name {
        Some(n) => Some(snippet::save_rows(&part, n, &aligned)?),
        None => None,
    };
    let mut text = format!(
        "pattern suggest {}/{}: period {}x{} (native {}x{}, factor {}), agreement {:.0}%, {:.0} repeat(s), confidence {:.2}{}",
        part.name, frame.name, tw, th, tw * f, th * f, f, best.agree * 100.0, best.repeats, conf, if own { ", from the part's own pixels (no reference)" } else { "" }
    );
    if conf < 0.6 {
        text.push_str("\n  low confidence: the area is small or not periodic; widen --region or check the picture with `pxl show --ref`");
    }
    if tile_mode && tiles.len() > 1 {
        text.push_str(&format!("\n  {} tiles read as wrapping blocks: {}", tiles.len(), tiles.iter().map(|t| t.to_string()).collect::<Vec<_>>().join(" ")));
    }
    if (tile_mode || a.flags.tile.is_some()) && (8 % tw != 0 || 8 % th != 0) {
        text.push_str("\n  note: the period does not divide the 8 px tile; neighbouring tiles will not line up");
    }
    text.push_str(&format!("\n  lattice phase {},{} (unit read from the area's top left){}", px, py, if (dx, dy) != (0, 0) { format!("; unit below is rotated to the part origin {},{}", dx, dy) } else { String::new() }));
    text.push_str("\n  unit (template keys; pasteable as a snippet):");
    for (j, r) in aligned.iter().enumerate() {
        text.push_str(&format!("\n  {:>2} |{}", j, r));
    }
    for al in &alts {
        text.push_str(&format!("\n  alternative {}x{}: agreement {:.0}%", al.tw, al.th, al.agree * 100.0));
    }
    match &saved {
        Some(p) => text.push_str(&format!("\n  saved snippet {} -> {}", cmd_name, p.slash())),
        None => text.push_str("\n  add --save NAME to write the unit as the snippet pattern_NAME"),
    }
    text.push_str(&format!("\n  apply:  {}  --agent A", cmd));
    if tile_mode && tiles.len() > 1 {
        text.push_str("   (once per tile)");
    }
    out.emit(
        text,
        json!({"part": part.name, "frame": frame.name, "period": [tw, th], "native_period": [tw * f, th * f], "factor": f, "agreement": (best.agree * 1000.0).round() / 1000.0,
               "repeats": best.repeats, "confidence": (conf * 1000.0).round() / 1000.0, "phase": [px, py], "default_phase": [dx, dy], "unit": aligned, "unit_raw": unit.iter().map(|r| String::from_utf8_lossy(r).into_owned()).collect::<Vec<_>>(),
               "snippet": name, "saved": saved.map(|p| p.slash()), "command": cmd, "tiles": tiles, "from_reference": !own,
               "alternatives": alts.iter().map(|c| json!({"period": [c.tw, c.th], "agreement": (c.agree * 1000.0).round() / 1000.0})).collect::<Vec<_>>(), "classes": classes_json}),
    );
    Ok(())
}
