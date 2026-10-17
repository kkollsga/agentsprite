//! `pxl candidates`: alternative starting points for a frame, side by side (read-only), and the shared
//! "reduce this frame with method M" path used by `pxl pick` and the `pick` op of `apply`.
//!
//! Hold-out rule (same as `reload --try`): the exemplar bank is built from the whole part (locked frames and
//! `exemplars`), and the frame being reduced is held out of it. Because `candidates` and `pick` call the same
//! [`reduce_frame`], the panel you looked at and the pixels you pick are identical.
//!
//! Image per frame: `reference | current | one panel per method`; every panel shows the same cell window at
//! the same scale (nearest neighbour, like `compare`), labelled with method, score (lower is better), and
//! IoU / lost / orphans / jaggies. A panel identical to an earlier one says `= name`.

use crate::fsutil::PathSlash;
use crate::cli::{CandidatesArgs, FrameSel};
use crate::compare::{self, BG, CHECK_A, DIM, FONT_SCALE, HEADER_H, MARGIN, MAX_PX, PLATE, TEXT, C_GOOD};
use crate::img::{Rgba, RgbaImage};
use crate::model::{Frame, Grid, Method, Part, PartKind, RefFrame};
use crate::output::{Out, PxlError, Result};
use crate::reduce::exemplar::Bank;
use crate::reduce::trial::{score_grid, Score};
use crate::reduce::{self, Prep, TRANSPARENT};
use crate::template::{self, Template};
use crate::{fsutil, load, metrics, pxl};
use serde_json::{json, Value};
use std::path::PathBuf;

pub const DEFAULT_METHODS: &str = "vote,contour,exemplar,auto,nearest";
/// Narrowest panel (px): room for a 14 character label.
const MIN_PANEL: usize = 136;
const LINE_H: usize = 14;
const TITLE_H: usize = 16;
const GAP: usize = 4;
const MAX_SCALE: usize = 12;

/// Exemplar bank for `method` (None when the method does not use one). Never holds anything out: the
/// frame is held out by name in [`reduce_frame`].
pub fn bank_for(part: &Part, tpl: &Template, methods: &[Method]) -> Result<Option<Bank>> {
    if methods.iter().any(|m| matches!(m, Method::Exemplar | Method::Auto)) {
        Ok(Some(Bank::build(part, tpl, None)?))
    } else {
        Ok(None)
    }
}

/// Fresh reduction of frame `fi` with `method`, plus its reference. The one definition of "what method M
/// gives for this frame".
pub fn reduce_frame(part: &Part, tpl: &Template, fi: usize, method: Method, bank: Option<&Bank>) -> Result<(Grid, RefFrame)> {
    let f = &part.frames[fi];
    let rf = load::reference(part, &part.dir(), f)?;
    let prep = Prep::new(&rf.img, rf.origin, part, tpl)?;
    let g = reduce::reduce_with(&prep, &rf.img, rf.origin, part, tpl, Some(f.name.as_str()), method, bank)?;
    Ok((g, rf))
}

struct Panel {
    /// `None` = the reference.
    grid: Option<Grid>,
    l1: String,
    l2: String,
    best: bool,
}

struct Block {
    name: String,
    locked: bool,
    rf: RefFrame,
    crop: (usize, usize, usize, usize),
    panels: Vec<Panel>,
    current: Score,
    scores: Vec<(Method, Score, Option<String>)>,
    best: Option<usize>,
    eyes: Vec<crate::features::Feature>,
}

fn detail(s: &Score) -> String {
    format!("iou{} L{} E{}/{} O{} J{}", (s.iou * 100.0).round() as i64, s.lost, s.eyes_found - s.eyes_lost, s.eyes_found, s.orphans, s.jaggies)
}

fn crop_of(part: &Part, rf: &RefFrame, grids: &[&Grid]) -> (usize, usize, usize, usize) {
    let (gw, gh) = part.size;
    let cov = reduce::ref_coverage(rf, part);
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
    let mut mark = |x: usize, y: usize| {
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x + 1);
        y1 = y1.max(y + 1);
    };
    for y in 0..gh {
        for x in 0..gw {
            if cov[y * gw + x] > 0 || grids.iter().any(|g| g.cells[y * g.w + x] != TRANSPARENT) {
                mark(x, y);
            }
        }
    }
    if x1 == 0 {
        return (0, 0, gw, gh);
    }
    let (x0, y0) = (x0.saturating_sub(1), y0.saturating_sub(1));
    ((x0), (y0), (x1 + 1).min(gw) - x0, (y1 + 1).min(gh) - y0)
}

fn panel_image(part: &Part, tpl: &Template, b: &Block, grid: Option<&Grid>, scale: usize) -> RgbaImage {
    let g = metrics::geometry(part, &b.rf);
    let f = g.f as usize;
    let (cx0, cy0, cw, ch) = b.crop;
    let (pw, ph) = (cw * f, ch * f);
    let mut im = compare::checker(pw, ph, 4);
    match grid {
        None => {
            let (ox, oy) = (g.org.0 + (cx0 * f) as i32, g.org.1 + (cy0 * f) as i32);
            for y in 0..ph {
                for x in 0..pw {
                    let p = b.rf.img.get(ox + x as i32, oy + y as i32);
                    if p.a > 0 {
                        let d = &mut im.px[y * pw + x];
                        *d = compare::blend(*d, p);
                    }
                }
            }
        }
        Some(gr) => {
            for cy in 0..ch {
                for cx in 0..cw {
                    let k = gr.cells[(cy + cy0) * gr.w + cx + cx0];
                    if k != TRANSPARENT {
                        let c = compare::key_color(part, Some(tpl), k);
                        compare::fill(&mut im, cx * f, cy * f, f, f, Rgba::opaque(c.r, c.g, c.b));
                    }
                }
            }
        }
    }
    let mut im = im.upscale(scale);
    compare::grid_lines(&mut im, f * scale);
        if let Some(gr) = grid {
        if let Some(k) = crate::features::keys_for(tpl) {
            for e in b.eyes.iter().filter(|e| !crate::features::eye_ok(gr, e, &k)) {
                let (x, y, w, h) = crate::features::marker_rect(e);
                let cell = (f * scale) as i32;
                compare::ring(&mut im, (x - cx0 as i32) * cell, (y - cy0 as i32) * cell, (w * cell) as usize, (h * cell) as usize, ((cell / 4).clamp(1, 3)) as usize, compare::C_LOST);
            }
        }
    }
    compare::coord_gutter(&im, cx0, cy0, f * scale)
}

fn parse_methods(spec: &[String]) -> Result<Vec<Method>> {
    let mut v: Vec<Method> = Vec::new();
    for s in spec {
        let m = Method::parse(s).ok_or_else(|| PxlError::usage(format!("unknown method '{}'", s)).with_hint("methods: vote, contour, exemplar, auto, nearest"))?;
        if !v.contains(&m) {
            v.push(m);
        }
    }
    if v.is_empty() {
        return Err(PxlError::usage("--methods is empty"));
    }
    Ok(v)
}

/// Rows of the sheet at `scale`: (width, height) of each block.
fn block_dims(part: &Part, blocks: &[Block], scale: usize) -> Vec<(usize, usize)> {
    blocks
        .iter()
        .map(|b| {
            let f = part.reduce.factor.or(part.input.factor).unwrap_or(1).max(1) as usize;
            let pw = (b.crop.2 * f * scale + compare::GUT_L).max(MIN_PANEL);
            let ph = b.crop.3 * f * scale + compare::GUT_T;
            (b.panels.len() * pw + (b.panels.len() - 1) * GAP, TITLE_H + 2 * LINE_H + ph)
        })
        .collect()
}

/// Greedy vertical packing into pages of at most MAX_PX height; Vec of block index ranges.
fn paginate(dims: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let avail = MAX_PX - HEADER_H - 2 * MARGIN;
    let mut pages = Vec::new();
    let (mut start, mut used) = (0, 0);
    for (i, d) in dims.iter().enumerate() {
        let need = d.1 + if i > start { MARGIN } else { 0 };
        if i > start && used + need > avail {
            pages.push((start, i));
            start = i;
            used = d.1;
        } else {
            used += need;
        }
    }
    pages.push((start, dims.len()));
    pages
}

/// Largest scale whose rows fit the page width: first aiming at six frames per page (never below scale 2),
/// then at three.
fn choose_scale(part: &Part, blocks: &[Block]) -> usize {
    let aw = MAX_PX - 2 * MARGIN;
    let n = blocks.len();
    for (per, min_scale) in [(6, 2), (3, 1)] {
        let want_pages = ((n + per - 1) / per).max(1);
        for s in (min_scale..=MAX_SCALE).rev() {
            let d = block_dims(part, blocks, s);
            if d.iter().all(|x| x.0 <= aw) && paginate(&d).len() <= want_pages {
                return s;
            }
        }
    }
    1
}

fn render_pages(part: &Part, tpl: &Template, blocks: &[Block], title: &str) -> Vec<RgbaImage> {
    let scale = choose_scale(part, blocks);
    let dims = block_dims(part, blocks, scale);
    let f = part.reduce.factor.or(part.input.factor).unwrap_or(1).max(1) as usize;
    let pages = paginate(&dims);
    let n_pages = pages.len();
    let idx: Vec<usize> = (0..n_pages).collect();
    fsutil::par_map(&idx, |&p| {
        let (a, z) = pages[p];
        let w = dims[a..z].iter().map(|d| d.0).max().unwrap_or(0) + 2 * MARGIN;
        let h = HEADER_H + MARGIN + dims[a..z].iter().map(|d| d.1 + MARGIN).sum::<usize>();
        let mut cv = RgbaImage::filled(w.max(360), h, BG);
        let t = format!("{}  page {}/{}", title, p + 1, n_pages);
        compare::draw_text(&mut cv, MARGIN, (HEADER_H - 5 * FONT_SCALE) / 2, &t, FONT_SCALE, TEXT);
        let mut y = HEADER_H + MARGIN;
        for bi in a..z {
            let b = &blocks[bi];
            let ty = y + (TITLE_H - 5 * FONT_SCALE) / 2;
            let end = compare::draw_text(&mut cv, MARGIN, ty, &b.name, FONT_SCALE, TEXT);
            if b.locked {
                compare::draw_text(&mut cv, end + 12, ty, "locked", FONT_SCALE, DIM);
            }
            let pw = (b.crop.2 * f * scale + compare::GUT_L).max(MIN_PANEL);
            let mut x = MARGIN;
            for pn in &b.panels {
                let col = if pn.best { C_GOOD } else { TEXT };
                compare::fill(&mut cv, x, y + TITLE_H, pw, 2 * LINE_H, PLATE);
                let chars = (pw + FONT_SCALE) / (4 * FONT_SCALE);
                let ly = y + TITLE_H + (LINE_H - 5 * FONT_SCALE) / 2;
                compare::draw_text(&mut cv, x + 3, ly, &compare::truncate_chars(&pn.l1, chars.saturating_sub(1).max(1)), FONT_SCALE, col);
                compare::draw_text(&mut cv, x + 3, ly + LINE_H, &compare::truncate_chars(&pn.l2, chars.saturating_sub(1).max(1)), FONT_SCALE, DIM);
                let im = panel_image(part, tpl, b, pn.grid.as_ref(), scale);
                compare::fill(&mut cv, x, y + TITLE_H + 2 * LINE_H, pw, im.h, CHECK_A);
                compare::paste(&mut cv, &im, x, y + TITLE_H + 2 * LINE_H);
                x += pw + GAP;
            }
            y += dims[bi].1 + MARGIN;
        }
        cv
    })
}

pub fn run_candidates(a: &CandidatesArgs, out: &mut Out) -> Result<()> {
    let path = fsutil::resolve_part(&a.part)?;
    let part = pxl::load(&path)?;
    if part.kind != PartKind::Sprite {
        return Err(PxlError::not_implemented(&format!("candidates for {} parts (M4)", part.kind.name())));
    }
    let methods = parse_methods(&a.methods)?;
    let tpl = template::load_for(&part)?;
    let sel = FrameSel { frames: a.frames.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(), all: false };
    let idx: Vec<usize> = {
        let names: Vec<&str> = sel.select(&part)?.iter().map(|f| f.name.as_str()).collect();
        part.frames.iter().enumerate().filter(|(_, f)| names.contains(&f.name.as_str())).map(|(i, _)| i).collect()
    };
    let bank = bank_for(&part, &tpl, &methods)?;
    if let Some(b) = &bank {
        for w in &b.warnings {
            out.warn(w.clone());
        }
    }
    let mut base = part.clone();
    base.frames.clear();
    let res: Vec<Result<Block>> = fsutil::par_map(&idx, |&fi| -> Result<Block> {
        let fr: &Frame = &part.frames[fi];
        let rf = load::reference(&part, &part.dir(), fr)?;
        let prep = Prep::new(&rf.img, rf.origin, &part, &tpl)?;
        let mut cands: Vec<(Method, Grid, Score)> = Vec::new();
        for &m in &methods {
            let g = reduce::reduce_with(&prep, &rf.img, rf.origin, &part, &tpl, Some(fr.name.as_str()), m, bank.as_ref())?;
            let s = score_grid(&base, &tpl, &fr.name, &g, &rf);
            cands.push((m, g, s));
        }
        let current = score_grid(&base, &tpl, &fr.name, &fr.grid, &rf);
        let mut best: Option<usize> = None;
        for (i, c) in cands.iter().enumerate() {
            if best.map_or(true, |b| c.2.value() < cands[b].2.value() - 1e-9) {
                best = Some(i);
            }
        }
        let mut grids: Vec<&Grid> = cands.iter().map(|c| &c.1).collect();
        grids.push(&fr.grid);
        let crop = crop_of(&part, &rf, &grids);
        let mut panels = vec![
            Panel { grid: None, l1: "reference".into(), l2: format!("{}x native", metrics::geometry(&part, &rf).f), best: false },
            Panel { grid: Some(fr.grid.clone()), l1: format!("current {:.1}", current.value()), l2: detail(&current), best: false },
        ];
        let mut scores = Vec::new();
        for (i, (m, g, s)) in cands.iter().enumerate() {
            let same = if *g == fr.grid {
                Some("current".to_string())
            } else {
                cands[..i].iter().find(|c| c.1 == *g).map(|c| c.0.name().to_string())
            };
            panels.push(Panel {
                grid: Some(g.clone()),
                l1: format!("{} {:.1}", m.name(), s.value()),
                l2: same.as_ref().map_or_else(|| detail(s), |n| format!("= {}", n)),
                best: best == Some(i),
            });
            scores.push((*m, s.clone(), same));
        }
        let eyes = prep.eyes.clone();
        Ok(Block { name: fr.name.clone(), locked: fr.locked, rf, crop, panels, current, scores, best, eyes })
    });
    let mut blocks = Vec::new();
    for r in res {
        blocks.push(r?);
    }
    // images
    let dir: PathBuf = match &a.out {
        Some(d) => PathBuf::from(d),
        None => path.parent().map(|p| p.to_path_buf()).unwrap_or_default().join("compare"),
    };
    let title = format!("{} candidates", part.name);
    let pages = render_pages(&part, &tpl, &blocks, &title);
    std::fs::create_dir_all(&dir)?;
    let mut files = Vec::new();
    for (i, im) in pages.iter().enumerate() {
        let stem = if blocks.len() == 1 { format!("{}_cand_{}", part.name, blocks[0].name) } else if pages.len() == 1 { format!("{}_cand", part.name) } else { format!("{}_cand_p{}", part.name, i + 1) };
        let p = dir.join(format!("{}.png", stem));
        fsutil::atomic_write(&p, &compare::encode_sheet_png(im))?;
        files.push(p);
    }
    // page of each block: recompute with the same plan
    let scale_pages = paginate(&block_dims(&part, &blocks, choose_scale(&part, &blocks)));
    for f in &files {
        out.line(f.slash());
    }
    let names: Vec<&str> = methods.iter().map(|m| m.name()).collect();
    let mut head = format!("{:<16} {:>8}", "frame", "current");
    for n in &names {
        head.push_str(&format!(" {:>9}", n));
    }
    out.line(format!("{}   (similarity to the reference, lower = closer, a pointer for where to look, not a verdict on the art; * = closest; = x means same pixels as x)", head));
    let mut jf = Vec::new();
    for (bi, b) in blocks.iter().enumerate() {
        let mut l = format!("{:<16} {:>8.1}", b.name, b.current.value());
        let mut jm = serde_json::Map::new();
        for (i, (m, s, same)) in b.scores.iter().enumerate() {
            let mark = if b.best == Some(i) { "*" } else if same.is_some() { "=" } else { " " };
            l.push_str(&format!(" {:>8.1}{}", s.value(), mark));
            jm.insert(
                m.name().to_string(),
                json!({"score": round1(s.value()), "iou": round3(s.iou), "bbox": s.bbox, "lost": s.lost, "eyes_found": s.eyes_found, "eyes_kept": s.eyes_found - s.eyes_lost, "orphans": s.orphans, "jaggies": s.jaggies, "same_as": same}),
            );
        }
        out.line(l);
        let page = scale_pages.iter().position(|r| bi >= r.0 && bi < r.1).unwrap_or(0);
        jf.push(json!({
            "frame": b.name,
            "locked": b.locked,
            "page": files.get(page).map(|p| p.slash()),
            "best": b.best.map(|i| b.scores[i].0.name()),
            "window": [b.crop.0, b.crop.1, b.crop.2, b.crop.3],
            "current": {"score": round1(b.current.value()), "iou": round3(b.current.iou), "lost": b.current.lost, "eyes_found": b.current.eyes_found, "eyes_kept": b.current.eyes_found - b.current.eyes_lost, "orphans": b.current.orphans, "jaggies": b.current.jaggies},
            "methods": Value::Object(jm),
        }));
    }
    if let Some(b0) = blocks.first() {
        out.line(format!("window origin x={} y={} (cells); panels are labelled with cell coordinates every 4 cells", b0.crop.0, b0.crop.1));
    }
    out.line(format!("panels: reference | current | {}; pick one with `pxl pick {} FRAME METHOD [--region x,y,w,h]`", names.join(" | "), part.name));
    out.set("part", json!(path.slash()));
    out.set("images", json!(files.iter().map(|p| p.slash()).collect::<Vec<_>>()));
    out.set("methods", json!(names));
    out.set("score_meaning", json!("similarity to the reference (lower = closer); where to look, not a quality verdict"));
    out.set("frames", Value::Array(jf));
    Ok(())
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}
fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}
