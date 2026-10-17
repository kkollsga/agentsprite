//! Quality harness for the automatic first version (`pxl image`): per console and input it measures
//!   * `mde`        mean Oklab error x100 between the first version and the input (per opaque pixel),
//!   * `hue_worst`  the worst dE x100 over the input's identity hue clusters (share > 3%, or chroma > 0.12 and share > 0.5%),
//!   * `neutral`    the share of neutral input clusters (share > 3%) that keep a neutral colour of similar lightness,
//!   * `white_over` the share of near-white pixels in the output minus the input's,
//!   * `eyes`       eyes (white with a pupil) in the output divided by eyes in the input,
//!   * `spread`     the lightness range (Oklab L, 2nd to 98th percentile of the output pixels) the output uses; the console style asks for
//!                  `min_value_spread`, and a regression is a lower spread than the baseline,
//!   * `white_deficit` for consoles whose style says `white = "active"` (gb): the input's light-material share (pixels with L > 0.92) minus the
//!                  output's white share, at least 0 (the output must carry as much white as the input has light material),
//!   * `flat_block` the largest single-colour region of the output whose hue is absent from the input under it (a chromatic colour over
//!                  input without that hue, or a grey over a chromatic input), as a share of the opaque pixels (a brown block over red),
//! and fails when a metric gets worse than `tests/fixtures/quality/baseline.json` beyond a tolerance. Improvements pass.
//! The synthetic inputs are generated here. A real approved sprite (never committed) is used when `$PXL_QUALITY_REAL` (a PNG) is set; its baseline entries are skipped otherwise.
//! `PXL_QUALITY_WRITE=1` rewrites the baseline (review the diff before committing it; do not commit entries of a real input).
mod e2e_common;

use agentsprite::color::{srgb_to_oklab, Rgb};
use agentsprite::img::{Rgba, RgbaImage};
use agentsprite::png;
use e2e_common::*;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const CONSOLES: [&str; 9] = ["gbc", "gb", "nes", "sms", "gg", "md", "snes", "pce", "gba"];

fn chroma(c: Rgb) -> f64 {
    let o = srgb_to_oklab(c);
    (o.a * o.a + o.b * o.b).sqrt()
}

fn de(a: Rgb, b: Rgb) -> f64 {
    let (x, y) = (srgb_to_oklab(a), srgb_to_oklab(b));
    ((x.l - y.l).powi(2) + (x.a - y.a).powi(2) + (x.b - y.b).powi(2)).sqrt() * 100.0
}

// ------------------------------------------------------------------------------------------------ inputs

fn bug(w: usize, h: usize) -> RgbaImage {
    let mut im = RgbaImage::new(w, h);
    let (cx, cy, rx, ry) = (w as f64 / 2.0, h as f64 / 2.0, w as f64 / 2.0 - 2.0, h as f64 / 2.0 - 3.0);
    for y in 0..h {
        for x in 0..w {
            let d = ((x as f64 - cx) / rx).powi(2) + ((y as f64 - cy) / ry).powi(2);
            let (fx, fy) = (x as f64, y as f64);
            let wheel = |wx: f64| (fx - w as f64 * wx).powi(2) + (fy - h as f64 * 0.8).powi(2) <= 16.0;
            let p = if wheel(0.25) || wheel(0.75) {
                Some((90, 90, 100))
            } else if d <= 1.0 {
                if d > 0.85 {
                    Some((40, 10, 10))
                } else if (fx - w as f64 * 0.7).powi(2) + (fy - h as f64 * 0.4).powi(2) <= 9.0 {
                    Some((255, 255, 255))
                } else if (fx - w as f64 * 0.72).powi(2) + (fy - h as f64 * 0.4).powi(2) <= 2.0 {
                    Some((0, 0, 0))
                } else if (h as f64 * 0.45..h as f64 * 0.53).contains(&fy) {
                    Some((30, 60, 200))
                } else {
                    Some((255, 0, 0))
                }
            } else {
                None
            };
            if let Some((r, g, b)) = p {
                im.px[y * w + x] = Rgba::opaque(r, g, b);
            }
        }
    }
    im
}

fn face(w: usize, h: usize) -> RgbaImage {
    let mut im = RgbaImage::new(w, h);
    let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f64, y as f64);
            let d = (((fx - cx) / (w as f64 / 2.0 - 2.0)).powi(2) + ((fy - cy) / (h as f64 / 2.0 - 2.0)).powi(2)).sqrt();
            if d > 1.0 {
                continue;
            }
            let eye = |ex: f64| (fx - ex).powi(2) + (fy - cy * 0.8).powi(2);
            let c = if d > 0.92 {
                (20, 20, 80)
            } else if eye(w as f64 * 0.35) <= 16.0 || eye(w as f64 * 0.65) <= 16.0 {
                if eye(w as f64 * 0.37) <= 3.0 || eye(w as f64 * 0.67) <= 3.0 { (0, 0, 0) } else { (255, 255, 255) }
            } else if fy < cy * 0.5 {
                (146, 146, 255)
            } else if fy < cy * 1.0 {
                (109, 109, 219)
            } else if fy < cy * 1.5 {
                (73, 73, 182)
            } else {
                (36, 36, 146)
            };
            im.px[y * w + x] = Rgba::opaque(c.0, c.1, c.2);
        }
    }
    im
}

fn item(n: usize) -> RgbaImage {
    let mut im = RgbaImage::new(n, n);
    let c = n as f64 / 2.0;
    for y in 0..n {
        for x in 0..n {
            let d = ((x as f64 - c).powi(2) + (y as f64 - c).powi(2)).sqrt();
            if d < c - 1.0 {
                im.px[y * n + x] = if d > c / 2.0 { Rgba::opaque(255, 200, 0) } else { Rgba::opaque(255, 255, 255) };
            }
        }
    }
    im
}

fn character(n: usize) -> RgbaImage {
    let mut im = RgbaImage::new(n, n);
    for y in n / 16..n - n / 16 {
        for x in n * 5 / 16..n * 11 / 16 {
            let c = if y < n / 3 { (255, 200, 160) } else if y < n * 5 / 8 { (0, 80, 255) } else if y < n * 13 / 16 { (200, 0, 0) } else { (60, 40, 20) };
            im.px[y * n + x] = Rgba::opaque(c.0, c.1, c.2);
        }
    }
    im
}

struct Input {
    name: &'static str,
    path: PathBuf,
    args: Vec<String>,
}

fn real_input() -> Option<PathBuf> {
    let env = std::env::var("PXL_QUALITY_REAL").ok().map(PathBuf::from);
    env.into_iter().find(|p| p.is_file())
}

fn inputs(dir: &Path) -> Vec<Input> {
    let mut v = Vec::new();
    for (name, img, size) in [("bug48", bug(48, 32), "24x16"), ("bug64", bug(64, 40), "32x20"), ("char64", character(64), "32x32"), ("face48", face(48, 48), "24x24"), ("item16", item(16), "8x8")] {
        let p = dir.join(format!("{}.png", name));
        png::write_rgba(&p, &img).unwrap();
        v.push(Input { name, path: p, args: vec!["--size".into(), size.into()] });
    }
    // our own CC0 example art, drawn at 2x the target size (the factor 2 path of the first version)
    let ex = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/inputs");
    for (name, rel) in [("hero2x", "hero/hero_02.png"), ("enemy2x", "enemy/crawler_01.png"), ("item2x", "item/gem.png")] {
        let p = dir.join(format!("{}.png", name));
        std::fs::copy(ex.join(rel), &p).unwrap();
        v.push(Input { name, path: p, args: vec!["--mode".into(), "sprite".into(), "--factor".into(), "2".into()] });
    }
    if let Some(r) = real_input() {
        let p = dir.join("real.png");
        std::fs::copy(r, &p).unwrap();
        v.push(Input { name: "real", path: p, args: vec!["--factor".into(), "1".into()] });
    }
    v
}

// ------------------------------------------------------------------------------------------------ metrics

/// Eyes: a group of at least three near-white pixels with a dark pupil next to it that the white surrounds (three or more white
/// neighbours). A white speck beside a dark edge is a glint, not an eye.
fn eyes_of(w: usize, h: usize, white_l: f64, px: &dyn Fn(usize, usize) -> Option<Rgb>) -> usize {
    let white = |c: Rgb| srgb_to_oklab(c).l > white_l && chroma(c) < 0.06;
    let dark = |c: Rgb| srgb_to_oklab(c).l < 0.4;
    let mut seen = vec![false; w * h];
    let mut eyes = 0;
    for y in 0..h {
        for x in 0..w {
            if seen[y * w + x] || !px(x, y).map_or(false, white) {
                continue;
            }
            let (mut st, mut comp) = (vec![(x, y)], Vec::new());
            seen[y * w + x] = true;
            while let Some((cx, cy)) = st.pop() {
                comp.push((cx, cy));
                for dy in -1i32..=1 {
                    for dx in -1i32..=1 {
                        let (nx, ny) = (cx as i32 + dx, cy as i32 + dy);
                        if nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h && !seen[ny as usize * w + nx as usize] && px(nx as usize, ny as usize).map_or(false, white) {
                            seen[ny as usize * w + nx as usize] = true;
                            st.push((nx as usize, ny as usize));
                        }
                    }
                }
            }
            if comp.len() < 3 {
                continue;
            }
            let set: std::collections::HashSet<(usize, usize)> = comp.iter().copied().collect();
            let pupil = comp.iter().any(|&(cx, cy)| {
                (-1i32..=1).any(|dy| {
                    (-1i32..=1).any(|dx| {
                        let (nx, ny) = (cx as i32 + dx, cy as i32 + dy);
                        if nx < 0 || ny < 0 || nx as usize >= w || ny as usize >= h || !px(nx as usize, ny as usize).map_or(false, dark) {
                            return false;
                        }
                        let around = (-1i32..=1).map(|ey| (-1i32..=1).filter(|ex| nx + ex >= 0 && ny + ey >= 0 && set.contains(&((nx + ex) as usize, (ny + ey) as usize))).count()).sum::<usize>();
                        around >= 3
                    })
                })
            });
            if pupil {
                eyes += 1;
            }
        }
    }
    eyes
}

/// (share, mean colour, mean chroma) of the chromatic hue families of an image.
fn hue_clusters(im: &RgbaImage) -> Vec<(f64, Rgb, f64)> {
    let mut by: Vec<(f64, f64, f64, f64, f64)> = Vec::new();
    let mut total: f64 = 0.0;
    for p in im.px.iter().filter(|p| p.a > 0) {
        total += 1.0;
        let c = Rgb { r: p.r, g: p.g, b: p.b };
        if chroma(c) < 0.05 {
            continue;
        }
        let o = srgb_to_oklab(c);
        let h = o.b.atan2(o.a);
        match by.iter_mut().find(|e| {
            let d = (e.0 - h).abs();
            d.min(std::f64::consts::TAU - d) < 0.45
        }) {
            Some(e) => {
                e.1 += 1.0;
                e.2 += p.r as f64;
                e.3 += p.g as f64;
                e.4 += p.b as f64;
            }
            None => by.push((h, 1.0, p.r as f64, p.g as f64, p.b as f64)),
        }
    }
    by.into_iter()
        .map(|e| {
            let m = Rgb { r: (e.2 / e.1) as u8, g: (e.3 / e.1) as u8, b: (e.4 / e.1) as u8 };
            (e.1 / total.max(1.0), m, chroma(m))
        })
        .collect()
}

/// Neutral clusters: (share, mean colour) of the input's grey ramp bands (chroma < 0.05), 4 lightness bands.
fn neutral_bands(im: &RgbaImage) -> Vec<(f64, Rgb)> {
    let mut b: Vec<(f64, f64, f64, f64)> = vec![(0.0, 0.0, 0.0, 0.0); 4];
    let mut total: f64 = 0.0;
    for p in im.px.iter().filter(|p| p.a > 0) {
        total += 1.0;
        let c = Rgb { r: p.r, g: p.g, b: p.b };
        if chroma(c) >= 0.05 {
            continue;
        }
        let band = ((srgb_to_oklab(c).l * 4.0) as usize).min(3);
        b[band].0 += 1.0;
        b[band].1 += p.r as f64;
        b[band].2 += p.g as f64;
        b[band].3 += p.b as f64;
    }
    b.into_iter().filter(|e| e.0 > 0.0).map(|e| (e.0 / total.max(1.0), Rgb { r: (e.1 / e.0) as u8, g: (e.2 / e.0) as u8, b: (e.3 / e.0) as u8 })).collect()
}

/// Largest single-colour output region whose hue does not occur in the input under it, as a share of the opaque output pixels.
fn flat_block(img: &RgbaImage, g: &agentsprite::model::Grid, f: usize, out_px: &dyn Fn(usize, usize) -> Option<Rgb>) -> f64 {
    let hue = |c: Rgb| {
        let o = srgb_to_oklab(c);
        o.b.atan2(o.a)
    };
    let (mut seen, mut opaque, mut worst) = (vec![false; g.w * g.h], 0usize, 0usize);
    for y in 0..g.h {
        for x in 0..g.w {
            opaque += usize::from(out_px(x, y).is_some());
        }
    }
    for y in 0..g.h {
        for x in 0..g.w {
            let Some(c) = out_px(x, y) else { continue };
            if seen[y * g.w + x] {
                continue;
            }
            let (mut st, mut reg) = (vec![(x, y)], Vec::new());
            seen[y * g.w + x] = true;
            while let Some((cx, cy)) = st.pop() {
                reg.push((cx, cy));
                for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
                    let (nx, ny) = (cx as i32 + dx, cy as i32 + dy);
                    if nx >= 0 && ny >= 0 && (nx as usize) < g.w && (ny as usize) < g.h && !seen[ny as usize * g.w + nx as usize] && out_px(nx as usize, ny as usize) == Some(c) {
                        seen[ny as usize * g.w + nx as usize] = true;
                        st.push((nx as usize, ny as usize));
                    }
                }
            }
            let (mut total, mut chromatic, mut matching) = (0usize, 0usize, 0usize);
            for &(rx, ry) in &reg {
                for dy in 0..f {
                    for dx in 0..f {
                        let (ix, iy) = (rx * f + dx, ry * f + dy);
                        if ix < img.w && iy < img.h && img.px[iy * img.w + ix].a > 0 {
                            let ic = Rgb { r: img.px[iy * img.w + ix].r, g: img.px[iy * img.w + ix].g, b: img.px[iy * img.w + ix].b };
                            total += 1;
                            if chroma(ic) > 0.05 {
                                chromatic += 1;
                                if chroma(c) > 0.05 {
                                    let d = (hue(ic) - hue(c)).abs();
                                    matching += usize::from(d.min(std::f64::consts::TAU - d) < 0.7);
                                }
                            }
                        }
                    }
                }
            }
            if total == 0 {
                continue;
            }
            let absent = if chroma(c) > 0.05 { chromatic > 0 && matching * 10 < chromatic.max(total / 2) } else { chromatic * 2 > total };
            if absent && chroma(c) > 0.05 && chromatic * 4 < total {
                continue; // mostly neutral input under a chromatic colour: a tint, not a wrong hue
            }
            if absent {
                worst = worst.max(reg.len());
            }
        }
    }
    worst as f64 / opaque.max(1) as f64
}

fn measure(id: &str, input: &Input, asset: &Path) -> Value {
    let img = png::read_rgba(&input.path).unwrap();
    let part = agentsprite::pxl::load(&asset.join("asset.pxl")).unwrap();
    let tpl = agentsprite::template::load_for(&part).unwrap();
    let cache: Vec<Option<Rgb>> = (0..=255u8).map(|k| agentsprite::lint::effective_color(&part, &tpl, k)).collect();
    let g = &part.frames[0].grid;
    let out_px = |x: usize, y: usize| -> Option<Rgb> {
        let k = g.cells[y * g.w + x];
        if k == b'.' { None } else { cache[k as usize] }
    };
    let f = part.input.factor.or(part.reduce.factor).unwrap_or(1).max(1) as usize;
    // mean error against the mean input colour of each block
    let (mut sum, mut n) = (0.0, 0usize);
    let mut out_colours: Vec<Rgb> = Vec::new();
    let mut out_colours_all: Vec<Rgb> = Vec::new();
    let (mut owh, mut oop) = (0usize, 0usize);
    let near_white = |c: Rgb, wl: f64| srgb_to_oklab(c).l > wl && chroma(c) < 0.06;
    let wl = if id == "sms" { 0.75 } else { 0.9 };
    for y in 0..g.h {
        for x in 0..g.w {
            let Some(c) = out_px(x, y) else { continue };
            oop += 1;
            out_colours_all.push(c);
            owh += usize::from(near_white(c, wl));
            if !out_colours.contains(&c) {
                out_colours.push(c);
            }
            let (mut l, mut a, mut b, mut k) = (0.0, 0.0, 0.0, 0.0);
            for dy in 0..f {
                for dx in 0..f {
                    let (ix, iy) = (x * f + dx, y * f + dy);
                    if ix < img.w && iy < img.h && img.px[iy * img.w + ix].a > 0 {
                        let p = img.px[iy * img.w + ix];
                        let o = srgb_to_oklab(Rgb { r: p.r, g: p.g, b: p.b });
                        l += o.l;
                        a += o.a;
                        b += o.b;
                        k += 1.0;
                    }
                }
            }
            if k > 0.0 {
                let oc = srgb_to_oklab(c);
                sum += (((oc.l - l / k).powi(2) + (oc.a - a / k).powi(2) + (oc.b - b / k).powi(2)).sqrt()) * 100.0;
                n += 1;
            }
        }
    }
    let mde = sum / n.max(1) as f64;
    let mut hue_worst: f64 = 0.0;
    for (share, mean, ch) in hue_clusters(&img) {
        if share > 0.03 || (ch > 0.12 && share > 0.005) {
            hue_worst = hue_worst.max(out_colours.iter().map(|c| de(*c, mean)).fold(f64::MAX, f64::min));
        }
    }
    let bands = neutral_bands(&img);
    let big: Vec<&(f64, Rgb)> = bands.iter().filter(|b| b.0 > 0.03).collect();
    let kept = big.iter().filter(|(_, m)| out_colours.iter().any(|c| chroma(*c) < 0.035 && (srgb_to_oklab(*c).l - srgb_to_oklab(*m).l).abs() < 0.2)).count();
    let neutral = if big.is_empty() { 1.0 } else { kept as f64 / big.len() as f64 };
    let (mut iop, mut iwh) = (0usize, 0usize);
    for p in img.px.iter().filter(|p| p.a > 0) {
        iop += 1;
        iwh += usize::from(near_white(Rgb { r: p.r, g: p.g, b: p.b }, 0.9));
    }
    let white_over = if id == "sms" { 0.0 } else { owh as f64 / oop.max(1) as f64 - iwh as f64 / iop.max(1) as f64 };
    let in_eyes = eyes_of(img.w, img.h, 0.85, &|x, y| Some(img.px[y * img.w + x]).filter(|p| p.a > 0).map(|p| Rgb { r: p.r, g: p.g, b: p.b }));
    let out_eyes = eyes_of(g.w, g.h, if id == "sms" { 0.75 } else { 0.85 }, &out_px);
    let eyes = if in_eyes == 0 { 1.0 } else { (out_eyes as f64 / in_eyes as f64).min(1.0) };
    // value spread and white deficit
    let mut ls: Vec<f64> = out_colours_all.iter().map(|c| srgb_to_oklab(*c).l).collect();
    ls.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let q = |p: f64| if ls.is_empty() { 0.0 } else { ls[((ls.len() - 1) as f64 * p).round() as usize] };
    let spread = q(0.98) - q(0.02);
    let style = agentsprite::console::style(id, None);
    let light_in = img.px.iter().filter(|p| p.a > 0 && srgb_to_oklab(Rgb { r: p.r, g: p.g, b: p.b }).l > 0.92).count() as f64 / iop.max(1) as f64;
    let white_out = owh as f64 / oop.max(1) as f64;
    let white_deficit = if style.white == "active" { (light_in - white_out).max(0.0) } else { 0.0 };
    let r = |v: f64| (v * 1000.0).round() / 1000.0;
    json!({"spread": r(spread), "white_deficit": r(white_deficit), "mde": r(mde), "hue_worst": r(hue_worst), "neutral": r(neutral), "white_over": r(white_over), "eyes": r(eyes), "flat_block": r(flat_block(&img, g, f, &out_px))})
}

fn baseline_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/quality/baseline.json")
}

#[test]
fn first_version_quality_does_not_regress() {
    let work = tmpdir("quality");
    let ins = inputs(&work);
    let mut now: BTreeMap<String, Value> = BTreeMap::new();
    for id in CONSOLES {
        for input in &ins {
            let d = tmpdir(&format!("q_{}_{}", id, input.name));
            std::fs::copy(&input.path, d.join("in.png")).unwrap();
            let mut args: Vec<&str> = vec!["image", "in.png", "--console", id];
            args.extend(input.args.iter().map(|s| s.as_str()));
            let o = pxl_in(&d, &args);
            assert!(o.status.success(), "{} / {}: {}{}", id, input.name, stdout(&o), stderr(&o));
            now.insert(format!("{}/{}", id, input.name), measure(id, input, &d.join("in.pxl")));
        }
    }
    if std::env::var("PXL_QUALITY_WRITE").is_ok() {
        let mut all: BTreeMap<String, Value> = std::fs::read_to_string(baseline_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        all.extend(now.clone());
        std::fs::write(baseline_path(), serde_json::to_string_pretty(&all).unwrap() + "\n").unwrap();
        eprintln!("baseline written: {} entries", all.len());
        return;
    }
    let base: BTreeMap<String, Value> = serde_json::from_str(&std::fs::read_to_string(baseline_path()).expect("tests/fixtures/quality/baseline.json")).unwrap();
    let mut table = String::new();
    let mut problems = Vec::new();
    let mut checked = 0;
    for (k, v) in &now {
        let Some(b) = base.get(k) else {
            problems.push(format!("{}: no baseline entry (run with PXL_QUALITY_WRITE=1)", k));
            continue;
        };
        checked += 1;
        let g = |x: &Value, f: &str| x[f].as_f64().unwrap();
        table.push_str(&format!("{:<18} mde {:>6.2} ({:>6.2})  hue {:>6.2} ({:>6.2})  neutral {:.2} ({:.2})  white {:>6.3} ({:>6.3})  eyes {:.2} ({:.2})  block {:.3} ({:.3})  spread {:.2} ({:.2})  wdef {:.3} ({:.3})\n", k, g(v, "mde"), g(b, "mde"), g(v, "hue_worst"), g(b, "hue_worst"), g(v, "neutral"), g(b, "neutral"), g(v, "white_over"), g(b, "white_over"), g(v, "eyes"), g(b, "eyes"), v["flat_block"].as_f64().unwrap_or(0.0), b["flat_block"].as_f64().unwrap_or(0.0), v["spread"].as_f64().unwrap_or(0.0), b["spread"].as_f64().unwrap_or(f64::NAN), v["white_deficit"].as_f64().unwrap_or(0.0), b["white_deficit"].as_f64().unwrap_or(f64::NAN)));
        if g(v, "mde") > g(b, "mde") + 1.0 {
            problems.push(format!("{}: mean error {:.2} is worse than the baseline {:.2}", k, g(v, "mde"), g(b, "mde")));
        }
        if g(v, "hue_worst") > g(b, "hue_worst") + 3.0 {
            problems.push(format!("{}: the worst identity hue is dE {:.1}, baseline {:.1}", k, g(v, "hue_worst"), g(b, "hue_worst")));
        }
        if g(v, "neutral") < g(b, "neutral") - 0.01 {
            problems.push(format!("{}: neutral survival {:.2}, baseline {:.2}", k, g(v, "neutral"), g(b, "neutral")));
        }
        if g(v, "white_over") > g(b, "white_over") + 0.02 {
            problems.push(format!("{}: white share is {:.3} above the input, baseline {:.3}", k, g(v, "white_over"), g(b, "white_over")));
        }
        if v["flat_block"].as_f64().unwrap_or(0.0) > b["flat_block"].as_f64().unwrap_or(0.0) + 0.02 {
            problems.push(format!("{}: a flat block of a hue the input lacks covers {:.3} of the sprite, baseline {:.3}", k, v["flat_block"].as_f64().unwrap_or(0.0), b["flat_block"].as_f64().unwrap_or(0.0)));
        }
        let opt = |x: &Value, f: &str| x[f].as_f64();
        if let (Some(a), Some(c)) = (opt(v, "spread"), opt(b, "spread")) {
            if a < c - 0.03 {
                problems.push(format!("{}: value spread {:.2} is below the baseline {:.2}", k, a, c));
            }
        }
        if let (Some(a), Some(c)) = (opt(v, "white_deficit"), opt(b, "white_deficit")) {
            if a > c + 0.02 {
                problems.push(format!("{}: white deficit {:.3} (input light share minus output white share), baseline {:.3}", k, a, c));
            }
        }
        if g(v, "eyes") < g(b, "eyes") - 0.01 {
            problems.push(format!("{}: eye parity {:.2}, baseline {:.2}", k, g(v, "eyes"), g(b, "eyes")));
        }
    }
    eprintln!("{}", table);
    assert!(problems.is_empty(), "{} of {} entries checked\n{}\n{}", problems.len(), checked, problems.join("\n"), table);
    assert!(checked >= CONSOLES.len() * 8, "checked {} entries", checked);
}

/// The example hero at factor 2 keeps its features on the colour consoles: eye_parity reports nothing and no identity hue is lost,
/// the silhouette keeps a dark outline, the eyes keep white and a dark pupil beside it, and the red shoes keep their hue.
#[test]
fn example_hero_first_version_keeps_outline_eyes_and_shoes() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/inputs/hero/hero_02.png");
    for id in ["gbc", "nes", "md", "sms", "snes"] {
        let d = tmpdir(&format!("hero_feat_{}", id));
        std::fs::copy(&src, d.join("hero.png")).unwrap();
        let o = pxl_in(&d, &["image", "hero.png", "--console", id, "--mode", "sprite", "--factor", "2"]);
        assert!(o.status.success(), "{}: {}{}", id, stdout(&o), stderr(&o));
        // the eyes survive (eye_parity, with the style minimum of 2x3), and no identity hue is lost outright; the tone-structure warnings of the stricter
        // gates (a material drawn with one tone where the source has two, an identity tone missing) are what hand work on the first version is for
        let o = pxl_in(&d, &["lint", "hero.pxl", "--rules", "eye_parity"]);
        assert!(stdout(&o).contains("0 error(s), 0 warning(s)"), "{}: eye_parity reports on the first version:\n{}", id, stdout(&o));
        let o = pxl_in(&d, &["lint", "hero.pxl", "--rules", "identity_colour,flat"]);
        assert!(!stdout(&o).contains("no colour of that hue family"), "{}: an identity hue is lost in the first version:\n{}", id, stdout(&o));
        let part = agentsprite::pxl::load(&d.join("hero.pxl/asset.pxl")).unwrap();
        let tpl = agentsprite::template::load_for(&part).unwrap();
        let g = &part.frames[0].grid;
        let col = |x: i32, y: i32| -> Option<Rgb> {
            if x < 0 || y < 0 || x as usize >= g.w || y as usize >= g.h {
                return None;
            }
            let k = g.cells[y as usize * g.w + x as usize];
            if k == b'.' { None } else { agentsprite::lint::effective_color(&part, &tpl, k) }
        };
        // outline: most silhouette-edge cells are dark
        let (mut edge, mut dark) = (0, 0);
        for y in 0..g.h as i32 {
            for x in 0..g.w as i32 {
                if let Some(c) = col(x, y) {
                    if [(0, 1), (0, -1), (1, 0), (-1, 0)].iter().any(|d| col(x + d.0, y + d.1).is_none()) {
                        edge += 1;
                        if srgb_to_oklab(c).l < 0.4 {
                            dark += 1;
                        }
                    }
                }
            }
        }
        assert!(dark * 10 >= edge * 7, "{}: only {} of {} silhouette edge cells are outline", id, dark, edge);
        // eyes: at least two white cells with a dark cell beside each, in the head
        let white = |c: Rgb| srgb_to_oklab(c).l > 0.75 && chroma(c) < 0.06;
        let mut eyes = 0;
        for y in 3..11 {
            for x in 0..g.w as i32 {
                if col(x, y).map_or(false, white) && [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|d| col(x + d.0, y + d.1).map_or(false, |c| srgb_to_oklab(c).l < 0.4)) {
                    eyes += 1;
                }
            }
        }
        assert!(eyes >= 2, "{}: {} eye cells (white beside a dark pupil)", id, eyes);
        // shoes: at least two cells of a red hue in the lower third
        let mut red = 0;
        for y in 16..g.h as i32 {
            for x in 0..g.w as i32 {
                if let Some(c) = col(x, y) {
                    if c.r > 150 && (c.g as i32) < c.r as i32 - 70 && (c.b as i32) < c.r as i32 - 70 {
                        red += 1;
                    }
                }
            }
        }
        assert!(red >= 3, "{}: {} red shoe cells", id, red);
    }
}
