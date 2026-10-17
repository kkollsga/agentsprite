//! Method scoring and `pxl reload --try`: run several methods on the selected frames, write nothing to the part,
//! print a per-frame x method table and a summary, optionally write side-by-side preview PNGs.
//!
//! Metrics per frame and method (all cell level, see `reduce::silhouette_vs_ref`):
//! `iou` silhouette IoU, `bb` largest bbox edge delta in cells, `ok` mean Oklab distance of cells opaque in
//! both (key colour vs mean block colour), `L` lost features (`lint::lost_features`), `O` orphan and `J` jaggy
//! warnings (`lint` rules), `g` cells that differ from `golden/<frame>.txt` (when it exists), `v1` cells that
//! differ from `attempts/*/<frame>.txt` (weak signal: unreviewed hand drawn grids).
//!
//! `auto` ranks candidates by [`Score::value`] = `100*(1-iou) + 50*(1-nIoU) + 8*max(0,bb-1) + 40*ok + 4*L + 40*E + 3*eyeErr + 1.5*shareDef + 2*(O+J)` (`E` = lost eyes); lower is better.

use crate::fsutil::PathSlash;
use super::{Prep, TRANSPARENT};
use crate::cli::{FrameSel, ReloadArgs};
use crate::color;
use crate::img::{Rgba, RgbaImage};
use crate::lint::{self, LintOpts};
use crate::load;
use crate::model::{Frame, Grid, Method, Part, RefFrame};
use crate::output::{Out, PxlError, Result};
use crate::template::Template;
use crate::{fsutil, png, profile, pxl, template};
use serde_json::json;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default)]
pub struct Score {
    pub iou: f64,
    /// Native resolution silhouette IoU (`silhouette_native`).
    pub niou: f64,
    pub bbox: f64,
    pub oklab: f64,
    pub lost: usize,
    pub eyes_found: usize,
    pub eyes_lost: usize,
    pub eye_err: usize,
    /// Cells the identity colours are short of 75 % of their reference share.
    pub share_def: f64,
    pub orphans: usize,
    pub jaggies: usize,
}

impl Score {
    pub fn value(&self) -> f64 {
        100.0 * (1.0 - self.iou) + 50.0 * (1.0 - self.niou) + 8.0 * (self.bbox - 1.0).max(0.0) + 40.0 * self.oklab + 4.0 * self.lost as f64 + crate::metrics::EYE_PENALTY * self.eyes_lost as f64 + crate::metrics::EYE_CELL * self.eye_err as f64 + crate::metrics::SHARE_CELL * self.share_def + 2.0 * (self.orphans + self.jaggies) as f64
    }
}

/// Mean Oklab distance between the key colours and the mean reference colour of the blocks, over cells opaque in both.
fn oklab_err(grid: &Grid, rf: &RefFrame, part: &Part, tpl: &Template) -> f64 {
    let f = part.reduce.factor.or(part.input.factor).unwrap_or(1).max(1) as i32;
    let ph = part.reduce.phase.unwrap_or((0, 0));
    let org = (rf.origin.0 - f * part.origin.0 + ph.0, rf.origin.1 - f * part.origin.1 + ph.1);
    let (mut sum, mut n) = (0.0, 0usize);
    for cy in 0..grid.h as i32 {
        for cx in 0..grid.w as i32 {
            let k = grid.get(cx, cy);
            if k == TRANSPARENT {
                continue;
            }
            let (mut c, mut sr, mut sg, mut sb) = (0u32, 0u32, 0u32, 0u32);
            for j in 0..f {
                for i in 0..f {
                    let p = rf.img.get(org.0 + f * cx + i, org.1 + f * cy + j);
                    if p.a >= 128 {
                        c += 1;
                        sr += p.r as u32;
                        sg += p.g as u32;
                        sb += p.b as u32;
                    }
                }
            }
            if c * 2 < (f * f) as u32 {
                continue;
            }
            let rc = color::Rgb { r: ((sr + c / 2) / c) as u8, g: ((sg + c / 2) / c) as u8, b: ((sb + c / 2) / c) as u8 };
            if let Some(oc) = lint::effective_color(part, tpl, k) {
                sum += color::oklab_dist2(color::srgb_to_oklab(oc), color::srgb_to_oklab(rc)).sqrt();
                n += 1;
            }
        }
    }
    if n == 0 { 0.0 } else { sum / n as f64 }
}

/// Orphan and jaggy counts from the lint rules, on a one frame copy of the part.
fn lint_counts(base: &Part, tpl: &Template, name: &str, grid: &Grid) -> (usize, usize) {
    let mut p = base.clone();
    p.frames = vec![Frame { name: name.to_string(), src: None, locked: false, grid: grid.clone() }];
    let prof = match profile::load_for(&p, None) {
        Ok(x) => x,
        Err(_) => return (0, 0),
    };
    let opts = LintOpts { rules: vec!["orphan".into(), "jaggy".into()], frames: vec![], refs: false, advice: false };
    let issues = lint::lint_part(&p, tpl, &prof, &opts);
    (issues.iter().filter(|i| i.rule == "orphan").count(), issues.iter().filter(|i| i.rule == "jaggy").count())
}

/// Full score of a candidate grid. `base` is the part (frames are ignored).
pub fn score_grid(base: &Part, tpl: &Template, name: &str, grid: &Grid, rf: &RefFrame) -> Score {
    let (iou, bbox) = super::silhouette_vs_ref(grid, rf, base);
    let fr = Frame { name: name.to_string(), src: None, locked: false, grid: grid.clone() };
    let lost = lint::lost_features(base, tpl, &fr, rf).len();
    let (eyes_found, el) = lint::eye_losses(base, tpl, grid, rf);
    let (orphans, jaggies) = lint_counts(base, tpl, name, grid);
    let (niou, _) = super::silhouette_native(grid, rf, base);
    Score { iou, niou, bbox, oklab: oklab_err(grid, rf, base, tpl), lost, eyes_found, eyes_lost: el.len(), eye_err: lint::eye_error(base, tpl, grid, rf), share_def: crate::features::colour_shares(base, tpl, grid, rf).iter().map(|s| s.deficit()).sum(), orphans, jaggies }
}

fn read_txt_grid(p: &Path, w: usize, h: usize) -> Option<Grid> {
    let t = std::fs::read_to_string(p).ok()?;
    pxl::parse_grid(&t, w, h, &crate::output::Loc::file(p.slash())).ok()
}

pub fn grid_diff(a: &Grid, b: &Grid) -> usize {
    a.cells.iter().zip(&b.cells).filter(|(x, y)| x != y).count()
}

fn attempt_dirs(dir: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(a) = fsutil::find_up(dir, "attempts") {
        if let Ok(rd) = std::fs::read_dir(&a) {
            let mut d: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_dir()).collect();
            d.sort();
            v = d;
        }
    }
    v
}

struct Row {
    name: String,
    grids: Vec<Grid>,
    scores: Vec<Score>,
    gold: Vec<Option<usize>>,
    v1: Vec<Option<usize>>,
    golden: Option<Grid>,
}

fn render(part: &Part, tpl: &Template, g: &Grid, cell: usize, r: (usize, usize, usize, usize)) -> RgbaImage {
    let mut im = RgbaImage::filled(r.2 * cell, r.3 * cell, Rgba { r: 96, g: 96, b: 96, a: 255 });
    for y in r.1..r.1 + r.3 {
        for x in r.0..r.0 + r.2 {
            let (ox, oy) = (x - r.0, y - r.1);
            if x >= g.w || y >= g.h {
                continue;
            }
            let k = g.cells[y * g.w + x];
            if k == TRANSPARENT {
                continue;
            }
            let c = lint::effective_color(part, tpl, k).unwrap_or(color::Rgb { r: 255, g: 0, b: 255 });
            for j in 0..cell {
                for i in 0..cell {
                    im.set((ox * cell + i) as i32, (oy * cell + j) as i32, Rgba { r: c.r, g: c.g, b: c.b, a: 255 });
                }
            }
        }
    }
    im
}

fn render_ref(part: &Part, rf: &RefFrame, scale: usize, r: (usize, usize, usize, usize)) -> RgbaImage {
    let f = part.reduce.factor.or(part.input.factor).unwrap_or(1).max(1) as i32;
    let ph = part.reduce.phase.unwrap_or((0, 0));
    let org = (rf.origin.0 - f * part.origin.0 + ph.0, rf.origin.1 - f * part.origin.1 + ph.1);
    let fu = f as usize;
    let (w, h) = (r.2 * fu, r.3 * fu);
    let (px0, py0) = (r.0 as i32 * f, r.1 as i32 * f);
    let mut im = RgbaImage::filled(w * scale, h * scale, Rgba { r: 96, g: 96, b: 96, a: 255 });
    for y in 0..h {
        for x in 0..w {
            let p = rf.img.get(org.0 + px0 + x as i32, org.1 + py0 + y as i32);
            if p.a < 128 {
                continue;
            }
            for j in 0..scale {
                for i in 0..scale {
                    im.set((x * scale + i) as i32, (y * scale + j) as i32, Rgba { r: p.r, g: p.g, b: p.b, a: 255 });
                }
            }
        }
    }
    im
}

/// `pxl reload --try m1,m2,...`: compare methods, write nothing to the part.
pub fn run_try(path: &Path, a: &ReloadArgs, sel: &FrameSel, out: &mut Out) -> Result<()> {
    let mut methods = Vec::new();
    for m in &a.try_methods {
        methods.push(Method::parse(m).ok_or_else(|| PxlError::usage(format!("unknown method '{}'", m)).with_hint("methods: vote, nearest, contour, exemplar, auto"))?);
    }
    let part = pxl::load(path)?;
    let tpl = template::load_for(&part)?;
    let dir = part.dir();
    let frames: Vec<Frame> = sel.select(&part)?.into_iter().cloned().collect();
    let bank = if methods.iter().any(|m| matches!(m, Method::Exemplar | Method::Auto)) { Some(super::exemplar::Bank::build(&part, &tpl, None)?) } else { None };
    if let Some(b) = &bank {
        for w in &b.warnings {
            out.warn(w.clone());
        }
    }
    let att = attempt_dirs(&dir);
    let mut base = part.clone();
    base.frames.clear();
    let rows: Vec<Result<(Row, RefFrame)>> = fsutil::par_map(&frames, |f| -> Result<(Row, RefFrame)> {
        let rf = load::reference(&part, &dir, f)?;
        let prep = Prep::new(&rf.img, rf.origin, &part, &tpl)?;
        let mut row = Row { name: f.name.clone(), grids: vec![], scores: vec![], gold: vec![], v1: vec![], golden: None };
        let gold = fsutil::find_up(&dir, &format!("golden/{}.txt", f.name)).and_then(|p| read_txt_grid(&p, part.size.0, part.size.1));
        let v1s: Vec<Grid> = att.iter().filter_map(|d| read_txt_grid(&d.join(format!("{}.txt", f.name)), part.size.0, part.size.1)).collect();
        row.golden = gold.clone();
        for &m in &methods {
            let g = super::reduce_with(&prep, &rf.img, rf.origin, &part, &tpl, if a.no_holdout { None } else { Some(f.name.as_str()) }, m, bank.as_ref())?;
            row.scores.push(score_grid(&base, &tpl, &f.name, &g, &rf));
            row.gold.push(gold.as_ref().map(|x| grid_diff(&g, x)));
            row.v1.push(if v1s.is_empty() { None } else { Some(v1s.iter().map(|x| grid_diff(&g, x)).min().unwrap()) });
            row.grids.push(g);
        }
        Ok((row, rf))
    });
    let mut rows_ok = Vec::new();
    for r in rows {
        rows_ok.push(r?);
    }
    // previews
    if let Some(d) = &a.preview_dir {
        std::fs::create_dir_all(d)?;
        for (row, rf) in &rows_ok {
            let cell = part.reduce.factor.or(part.input.factor).unwrap_or(1).max(1) as usize * 6;
            let cov = super::ref_coverage(rf, &part);
            let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
            let mut mark = |x: usize, y: usize| {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            };
            for y in 0..part.size.1 {
                for x in 0..part.size.0 {
                    if cov[y * part.size.0 + x] > 0 {
                        mark(x, y);
                    }
                }
            }
            for g in row.grids.iter().chain(row.golden.iter()) {
                for y in 0..g.h {
                    for x in 0..g.w {
                        if g.cells[y * g.w + x] != TRANSPARENT {
                            mark(x, y);
                        }
                    }
                }
            }
            let r = if x0 == usize::MAX { (0, 0, part.size.0, part.size.1) } else { (x0.saturating_sub(1), y0.saturating_sub(1), (x1 + 1).min(part.size.0) - x0.saturating_sub(1), (y1 + 1).min(part.size.1) - y0.saturating_sub(1)) };
            let mut imgs = vec![render_ref(&part, rf, 6, r)];
            if let Some(g) = &row.golden {
                imgs.push(render(&part, &tpl, g, cell, r));
            }
            for g in &row.grids {
                imgs.push(render(&part, &tpl, g, cell, r));
            }
            let gap = 4;
            let w: usize = imgs.iter().map(|i| i.w + gap).sum::<usize>() - gap;
            let h = imgs.iter().map(|i| i.h).max().unwrap_or(1);
            let mut sheet = RgbaImage::filled(w, h, Rgba { r: 20, g: 20, b: 20, a: 255 });
            let mut x = 0;
            for i in &imgs {
                sheet.blit(i, x as i32, 0);
                x += i.w + gap;
            }
            png::write_rgba(&Path::new(d).join(format!("{}.png", row.name)), &sheet)?;
        }
        out.line(format!("previews: {} (reference | [golden] | {})", d, methods.iter().map(|m| m.name()).collect::<Vec<_>>().join(" | ")));
    }
    // table
    let mn: Vec<&str> = methods.iter().map(|m| m.name()).collect();
    let mut head = format!("{:<16}", "frame");
    for n in &mn {
        head.push_str(&format!("| {:<8} iou  nIoU bb  ok   L  O  J    g   v1 ", n));
    }
    out.line(head);
    let mut jrows = Vec::new();
    for (row, _) in &rows_ok {
        let mut l = format!("{:<16}", row.name);
        let mut jm = serde_json::Map::new();
        for (i, n) in mn.iter().enumerate() {
            let s = &row.scores[i];
            let opt = |o: Option<usize>| o.map_or("   -".to_string(), |v| format!("{:4}", v));
            l.push_str(&format!("| {:<8} {:.3} {:.3} {:3.0} {:.2} {:2} {:2} {:2} {} {} ", "", s.iou, s.niou, s.bbox, s.oklab, s.lost, s.orphans, s.jaggies, opt(row.gold[i]), opt(row.v1[i])));
            jm.insert(n.to_string(), json!({"iou": s.iou, "niou": s.niou, "bbox": s.bbox, "oklab": s.oklab, "lost": s.lost, "orphans": s.orphans, "jaggies": s.jaggies, "golden_diff": row.gold[i], "v1_diff": row.v1[i], "score": s.value()}));
        }
        out.line(l);
        jrows.push(json!({"frame": row.name, "methods": jm}));
    }
    out.line("");
    out.line(format!("{:<10} {:>7} {:>7} {:>7} {:>6} {:>6} {:>6} {:>6} {:>6} {:>7} {:>8} {:>8}", "method", "iouMean", "iouMin", "nIoUm", "bbMean", "bbMax", "okMean", "lost", "orph", "jaggy", "gold(01)", "v1Mean"));
    let mut jsum = serde_json::Map::new();
    let n = rows_ok.len().max(1) as f64;
    for (i, name) in mn.iter().enumerate() {
        let sc: Vec<&Score> = rows_ok.iter().map(|r| &r.0.scores[i]).collect();
        let iou_mean = sc.iter().map(|s| s.iou).sum::<f64>() / n;
        let iou_min = sc.iter().map(|s| s.iou).fold(1.0, f64::min);
        let niou_mean = sc.iter().map(|s| s.niou).sum::<f64>() / n;
        let bb_mean = sc.iter().map(|s| s.bbox).sum::<f64>() / n;
        let bb_max = sc.iter().map(|s| s.bbox).fold(0.0, f64::max);
        let ok_mean = sc.iter().map(|s| s.oklab).sum::<f64>() / n;
        let lost: usize = sc.iter().map(|s| s.lost).sum();
        let orph: usize = sc.iter().map(|s| s.orphans).sum();
        let jag: usize = sc.iter().map(|s| s.jaggies).sum();
        let golds: Vec<usize> = rows_ok.iter().filter_map(|r| r.0.gold[i]).collect();
        let v1s: Vec<usize> = rows_ok.iter().filter_map(|r| r.0.v1[i]).collect();
        let gold_s = if golds.is_empty() { "-".to_string() } else { format!("{}", golds.iter().sum::<usize>()) };
        let v1_mean = if v1s.is_empty() { None } else { Some(v1s.iter().sum::<usize>() as f64 / v1s.len() as f64) };
        out.line(format!("{:<10} {:>7.3} {:>7.3} {:>7.3} {:>6.2} {:>6.0} {:>6.3} {:>6} {:>6} {:>7} {:>8} {:>8}", name, iou_mean, iou_min, niou_mean, bb_mean, bb_max, ok_mean, lost, orph, jag, gold_s, v1_mean.map_or("-".into(), |v| format!("{:.1}", v))));
        jsum.insert(name.to_string(), json!({"iou_mean": iou_mean, "iou_min": iou_min, "niou_mean": niou_mean, "bbox_mean": bb_mean, "bbox_max": bb_max, "oklab_mean": ok_mean, "lost": lost, "orphans": orph, "jaggies": jag, "golden_diff_total": golds.iter().sum::<usize>(), "v1_diff_mean": v1_mean}));
    }
    out.line("columns: iou/bb/ok/L/O/J per frame; g = cells differing from golden/<frame>.txt, v1 = from attempts/*/<frame>.txt. Nothing was written.");
    out.set("try", json!({"part": path.slash(), "frames": jrows, "summary": jsum}));
    Ok(())
}
