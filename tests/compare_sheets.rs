//! `pxl compare`: AC-CP1 (perfect reduction has no fringe), AC-CP2 (page limit, speed), AC-CP3 (exact
//! previews), determinism, diff and review.

use agentsprite::cli::{DiffArgs, FrameSel, ReviewArgs};
use agentsprite::compare::{self, RunOpts, MAX_PX};
use agentsprite::img::{Rgba, RgbaImage};
use agentsprite::model::{Frame, Grid, Part};
use agentsprite::output::Out;
use agentsprite::{png, pxl};
use serde_json::Value;
use std::path::{Path, PathBuf};

const KEYS: [(u8, [u8; 3]); 3] = [(b'1', [0x18, 0x20, 0x78]), (b'2', [0x48, 0x50, 0xb8]), (b'3', [0xf0, 0xb0, 0x90])];

fn key_rgb(k: u8) -> Option<[u8; 3]> {
    KEYS.iter().find(|(c, _)| *c == k).map(|(_, c)| *c)
}

/// Deterministic blobby grid.
fn make_grid(w: usize, h: usize, seed: u32) -> Grid {
    let mut g = Grid::new(w, h, b'.');
    let mut s = seed.wrapping_mul(2654435761).wrapping_add(12345);
    let mut next = || {
        s = s.wrapping_mul(1664525).wrapping_add(1013904223);
        (s >> 16) as usize
    };
    let (x0, y0) = (2 + next() % 3, 2 + next() % 3);
    let (x1, y1) = (w - 2 - next() % 3, h - 2 - next() % 3);
    for y in y0..y1 {
        for x in x0..x1 {
            let edge = x == x0 || y == y0 || x == x1 - 1 || y == y1 - 1;
            let k = if edge { b'1' } else if (x + y + seed as usize) % 4 == 0 { b'3' } else { b'2' };
            g.set(x as i32, y as i32, k);
        }
    }
    g
}

/// A part whose references are the exact xfactor nearest upscale of the grids (perfect reduction).
fn setup(tag: &str, n: usize, w: usize, h: usize, f: usize) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pxl_compare_{}_{}", tag, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("refs")).unwrap();
    std::fs::create_dir_all(dir.join("templates")).unwrap();
    std::fs::write(
        dir.join("templates/syn.toml"),
        "name = \"syn\"\n[layers.base]\n\".\" = { role = \"transparent\" }\n\"1\" = { role = \"outline\", color = \"#182078\" }\n\"2\" = { role = \"body\", color = \"#4850b8\" }\n\"3\" = { role = \"skin\", color = \"#f0b090\" }\n",
    )
    .unwrap();
    let mut p = Part::new("syn", (w, h));
    p.path = dir.join("syn.pxl");
    p.template = Some("templates/syn.toml".into());
    p.input.images = Some("refs/{frame}.png".into());
    p.input.factor = Some(f as u32);
    p.reduce.factor = Some(f as u32);
    p.origin = (0, 0);
    for i in 0..n {
        let name = format!("f{:03}", i);
        let g = make_grid(w, h, i as u32 + 1);
        let mut img = RgbaImage::new(w * f, h * f);
        for y in 0..h * f {
            for x in 0..w * f {
                if let Some(c) = key_rgb(g.cells[(y / f) * w + x / f]) {
                    img.set(x as i32, y as i32, Rgba::opaque(c[0], c[1], c[2]));
                }
            }
        }
        png::write_rgba(&dir.join(format!("refs/{}.png", name)), &img).unwrap();
        p.frames.push(Frame { name, src: None, locked: false, grid: g });
    }
    pxl::save(&p.path, &p).unwrap();
    p.path.clone()
}

fn run(path: &Path, out: &Path, o: RunOpts) -> compare::PartResult {
    compare::compare_part(path, &RunOpts { out_dir: Some(out.to_path_buf()), kind: "compare".into(), ..o }).unwrap()
}

fn is_red_or_green(p: Rgba) -> bool {
    (p.r == 0xe0 && p.g == 0x3c && p.b == 0x3c) || (p.r == 0x3c && p.g == 0xc8 && p.b == 0x64)
}

fn pt(v: &Value) -> (usize, usize) {
    (v[0].as_u64().unwrap() as usize, v[1].as_u64().unwrap() as usize)
}

#[test]
fn ac_cp1_perfect_reduction_has_no_fringe_and_no_lost_features() {
    let path = setup("cp1", 6, 16, 14, 3);
    let out = path.parent().unwrap().join("out");
    let r = run(&path, &out, RunOpts::default());
    let frames = r.json["frames"].as_array().unwrap();
    assert_eq!(frames.len(), 6);
    let layout = &r.json["layout"];
    let (s, f) = (layout["scale"].as_u64().unwrap() as usize, layout["factor"].as_u64().unwrap() as usize);
    let crop: Vec<usize> = layout["crop"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap() as usize).collect();
    let pages: Vec<RgbaImage> = r.pages.iter().map(|p| png::read_rgba(p).unwrap()).collect();
    for fr in frames {
        assert_eq!(fr["iou"].as_f64().unwrap(), 1.0);
        assert_eq!(fr["bbox_max"].as_u64().unwrap(), 0);
        assert!(fr["lost"].as_array().unwrap().is_empty(), "lost features: {}", fr["lost"]);
        assert_eq!(fr["oklab"].as_f64().unwrap(), 0.0);
        let page = &pages[fr["page"].as_u64().unwrap() as usize - 1];
        let (dx, dy) = pt(&fr["panels"]["diff"]);
        for y in 0..crop[3] * f * s {
            for x in 0..crop[2] * f * s {
                assert!(!is_red_or_green(page.get((dx + x) as i32, (dy + y) as i32)), "fringe at {},{}", x, y);
            }
        }
    }
}

#[test]
fn ac_cp3_panels_are_exact_nearest_neighbour_upscales() {
    let (w, h, f) = (12, 10, 2);
    let path = setup("cp3", 4, w, h, f);
    let out = path.parent().unwrap().join("out");
    let r = run(&path, &out, RunOpts::default());
    let part = pxl::load(&path).unwrap();
    let layout = &r.json["layout"];
    let s = layout["scale"].as_u64().unwrap() as usize;
    assert!(s >= 2, "a tiny part should be shown enlarged, got {}x", s);
    let crop: Vec<usize> = layout["crop"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap() as usize).collect();
    let page_imgs: Vec<RgbaImage> = r.pages.iter().map(|p| png::read_rgba(p).unwrap()).collect();
    for fr in r.json["frames"].as_array().unwrap() {
        let name = fr["frame"].as_str().unwrap();
        let frame = part.frame(name).unwrap();
        let page = &page_imgs[fr["page"].as_u64().unwrap() as usize - 1];
        let (ox, oy) = pt(&fr["panels"]["output"]);
        let (ix, iy) = pt(&fr["panels"]["input"]);
        let reference = png::read_rgba(&path.parent().unwrap().join(format!("refs/{}.png", name))).unwrap();
        for cy in 0..crop[3] {
            for cx in 0..crop[2] {
                let k = frame.grid.get((cx + crop[0]) as i32, (cy + crop[1]) as i32);
                let Some(c) = key_rgb(k) else { continue };
                // centre of the target cell in the output panel
                let (px, py) = (ox + cx * f * s + f * s / 2, oy + cy * f * s + f * s / 2);
                let p = page.get(px as i32, py as i32);
                assert_eq!([p.r, p.g, p.b], c, "output cell {},{} of {}", cx, cy, name);
                // every pixel of the input panel block is the reference pixel
                for j in 0..f * s {
                    for i in 0..f * s {
                        let q = page.get((ix + cx * f * s + i) as i32, (iy + cy * f * s + j) as i32);
                        assert_eq!([q.r, q.g, q.b], c);
                    }
                }
                let rp = reference.get(((cx + crop[0]) * f) as i32, ((cy + crop[1]) * f) as i32);
                assert_eq!([rp.r, rp.g, rp.b], c);
            }
        }
    }
}

#[test]
fn pages_respect_the_limit_and_are_deterministic() {
    let path = setup("pages", 130, 24, 24, 2);
    let out = path.parent().unwrap().join("out");
    let a = run(&path, &out, RunOpts::default());
    assert!(!a.pages.is_empty());
    for p in &a.pages {
        let im = png::read_rgba(p).unwrap();
        assert!(im.w.max(im.h) <= MAX_PX, "{} is {}x{}", p.display(), im.w, im.h);
    }
    let bytes: Vec<Vec<u8>> = a.pages.iter().map(|p| std::fs::read(p).unwrap()).collect();
    let b = run(&path, &out, RunOpts { record: true, ..Default::default() });
    let c = run(&path, &out, RunOpts { record: true, ..Default::default() });
    for (p, old) in c.pages.iter().zip(&bytes) {
        assert_eq!(&std::fs::read(p).unwrap(), old, "sheet bytes must not change between runs");
    }
    assert_eq!(std::fs::read(b.json_path.unwrap()).unwrap(), std::fs::read(c.json_path.unwrap()).unwrap());
}

#[test]
fn zoom_and_on_modes_write_pages_within_the_limit() {
    let path = setup("zoom", 3, 20, 20, 2);
    let out = path.parent().unwrap().join("out");
    let z = run(&path, &out, RunOpts { names: vec!["f001".into()], zoom: Some(0), ..Default::default() });
    assert_eq!(z.pages.len(), 1);
    let im = png::read_rgba(&z.pages[0]).unwrap();
    assert!(im.w.max(im.h) <= MAX_PX && im.w.max(im.h) > MAX_PX - 200, "zoom should fill the page: {}x{}", im.w, im.h);
    let on = run(&path, &out, RunOpts { on: Some("#3a7bd5".into()), ..Default::default() });
    for p in &on.pages {
        let im = png::read_rgba(p).unwrap();
        assert!(im.w.max(im.h) <= MAX_PX);
    }
}

#[test]
fn compare_records_scores_for_next() {
    let path = setup("status", 3, 16, 16, 2);
    let out = path.parent().unwrap().join("out");
    run(&path, &out, RunOpts { record: true, ..Default::default() });
    let s = agentsprite::status::read(&path);
    assert_eq!(s.metrics.len(), 3);
    assert!(s.worst().is_some());
}

#[test]
fn diff_reports_changed_cells() {
    let path = setup("diff", 2, 12, 12, 2);
    let mut part = pxl::load(&path).unwrap();
    let k = part.frames[0].grid.get(5, 5);
    part.frames[0].grid.set(5, 5, if k == b'3' { b'2' } else { b'3' });
    let b = path.parent().unwrap().join("syn_b.pxl");
    part.path = b.clone();
    pxl::save(&b, &part).unwrap();
    let mut out = Out::new(true, None);
    compare::run_diff(&DiffArgs { a: path.display().to_string(), b: b.display().to_string(), sel: FrameSel::default() }, &mut out).unwrap();
    let (txt, _, code) = out.render(&Ok(()));
    assert_eq!(code, 0);
    let v: Value = serde_json::from_str(&txt).unwrap();
    assert_eq!(v["total_changed"], 1);
    assert_eq!(v["frames"][0]["changes"][0]["x"], 5);
}

#[test]
fn review_writes_sheets_for_done_parts() {
    let path = setup("review", 8, 16, 16, 2);
    agentsprite::status::update(&path, |s| s.state = "done".into()).unwrap();
    let out_dir = path.parent().unwrap().join("rev");
    let mut out = Out::new(true, None);
    let args = ReviewArgs { parts: vec![path.display().to_string()], state: Some("done".into()), out: Some(out_dir.display().to_string()), worst: 3, ..Default::default() };
    agentsprite::queue::run_review(&args, &mut out).unwrap();
    let (txt, _, _) = out.render(&Ok(()));
    let v: Value = serde_json::from_str(&txt).unwrap();
    let sheets = v["parts"][0]["sheets"].as_array().unwrap();
    assert!(!sheets.is_empty());
    assert_eq!(v["parts"][0]["frames"].as_array().unwrap().len(), 8, "the packet covers every frame");
    assert!(Path::new(sheets[0].as_str().unwrap()).is_file());
}

