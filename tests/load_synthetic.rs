//! AC-LD3 and the load / reload commands on synthetic images (no private data needed).

use agentsprite::cli::{FrameSel, LoadArgs, Point, ReloadArgs, Size};
use agentsprite::color::Rgb;
use agentsprite::img::{Rgba, RgbaImage};
use agentsprite::model::{Frame, Grid, Part};
use agentsprite::output::Out;
use agentsprite::{load, png, pxl, reduce, template};
use std::path::{Path, PathBuf};

const TEMPLATE: &str = r##"
name = "t"
[layers.base]
"1" = { role = "outline", color = "#182078" }
"2" = { role = "body",    color = "#4850b8" }
"3" = { role = "skin",    color = "#f0b090" }
[layers.detail]
"W" = { role = "white", color = "#f0f0f0", under = "3" }
[map]
"#182078" = "1"
"#4850b8" = "2"
"#f0b090" = "3"
"#f0f0f0" = "W"
"##;

fn tmpdir(tag: &str) -> PathBuf {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let d = std::env::temp_dir().join(format!("pxl_load_{}_{}_{}", tag, std::process::id(), n));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn color_of(k: u8) -> Rgba {
    match k {
        b'1' => Rgba::opaque(0x18, 0x20, 0x78),
        b'2' => Rgba::opaque(0x48, 0x50, 0xb8),
        b'3' => Rgba::opaque(0xf0, 0xb0, 0x90),
        b'W' => Rgba::opaque(0xf0, 0xf0, 0xf0),
        _ => Rgba::TRANSPARENT,
    }
}

/// Deterministic pseudo random cell grid with a margin of `m` empty cells; includes isolated single cells.
fn pattern(w: usize, h: usize, m: usize, seed: u64) -> Grid {
    let mut g = Grid::new(w, h, b'.');
    let mut s = seed;
    for y in m..h - m {
        for x in m..w - m {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let r = (s >> 33) % 7;
            g.cells[y * w + x] = match r {
                0 | 1 => b'.',
                2 | 3 => b'1',
                4 => b'2',
                5 => b'3',
                _ => b'W',
            };
        }
    }
    g
}

fn upscale(g: &Grid, f: usize, shift: (usize, usize), size: (usize, usize)) -> RgbaImage {
    let mut img = RgbaImage::new(size.0, size.1);
    for y in 0..g.h * f {
        for x in 0..g.w * f {
            img.set((x + shift.0) as i32, (y + shift.1) as i32, color_of(g.cells[(y / f) * g.w + x / f]));
        }
    }
    img
}

fn setup(w: usize, h: usize, f: u32) -> (Part, template::Template, PathBuf) {
    let dir = tmpdir("tpl");
    let tp = dir.join("t.toml");
    std::fs::write(&tp, TEMPLATE).unwrap();
    let tpl = template::load(&tp).unwrap();
    let mut p = Part::new("t", (w, h));
    p.path = dir.join("t.pxl");
    p.reduce.factor = Some(f);
    p.input.factor = Some(f);
    (p, tpl, dir)
}

#[test]
fn ac_ld3_exact_upscale_reloads_exactly() {
    for f in [2usize, 3, 4] {
        let (part, tpl, _d) = setup(16, 16, f as u32);
        let g = pattern(16, 16, 0, 7 + f as u64);
        let img = upscale(&g, f, (0, 0), (16 * f, 16 * f));
        let out = reduce::reduce_frame(&img, (0, 0), &part, &tpl).unwrap();
        assert_eq!(out, g, "factor {}", f);
    }
}

#[test]
fn ac_ld3_shift_by_factor_shifts_one_pixel() {
    let f = 2usize;
    let (part, tpl, _d) = setup(20, 20, f as u32);
    let g = pattern(20, 20, 3, 11);
    let base = reduce::reduce_frame(&upscale(&g, f, (0, 0), (40, 40)), (0, 0), &part, &tpl).unwrap();
    assert_eq!(base, g);
    for (dx, dy) in [(1usize, 0usize), (0, 1), (1, 1)] {
        let shifted = upscale(&g, f, (dx * f, dy * f), (40, 40));
        let out = reduce::reduce_frame(&shifted, (0, 0), &part, &tpl).unwrap();
        for y in 0..20i32 {
            for x in 0..20i32 {
                assert_eq!(out.get(x + dx as i32, y + dy as i32), g.get(x, y), "shift {:?} at {},{}", (dx, dy), x, y);
            }
        }
    }
}

#[test]
fn phase_and_origin_are_honoured() {
    // content drawn with its block grid offset by the phase reloads exactly
    let f = 2usize;
    let (mut part, tpl, _d) = setup(12, 12, f as u32);
    part.reduce.phase = Some((-1, 0));
    part.origin = (6, 6);
    let g = pattern(12, 12, 1, 5);
    // ref origin (12, 12) = origin * factor; block of cell X starts at 12 + 2 (X - 6) - 1 = 2X - 1
    let mut img = RgbaImage::new(26, 26);
    for y in 0..24usize {
        for x in 0..24usize {
            img.set(x as i32 - 1 + 1, y as i32, color_of(g.cells[(y / f) * 12 + x / f]));
        }
    }
    // image pixel (x + 0) holds cell column x / 2 -> block of cell X covers x in {2X, 2X+1}: shift by one px
    let out = reduce::reduce_frame(&img, (12 + 1, 12), &part, &tpl).unwrap();
    // (ref origin moved by +1 as well, the phase of -1 cancels it: block of X = 13 + 2(X-6) - 1 = 2X)
    assert_eq!(out, g);
}

fn write_part(dir: &Path, grids_locked: &[(&str, bool)], g: &Grid, f: usize) -> PathBuf {
    std::fs::create_dir_all(dir.join("refs")).unwrap();
    std::fs::write(dir.join("t.toml"), TEMPLATE).unwrap();
    let mut p = Part::new("t", (g.w, g.h));
    p.path = dir.join("t.pxl");
    p.template = Some("t.toml".into());
    p.input.images = Some("refs/{frame}.png".into());
    p.input.factor = Some(f as u32);
    p.reduce.factor = Some(f as u32);
    for (n, locked) in grids_locked {
        png::write_rgba(&dir.join(format!("refs/{}.png", n)), &upscale(g, f, (0, 0), (g.w * f, g.h * f))).unwrap();
        p.frames.push(Frame { name: n.to_string(), src: None, locked: *locked, grid: Grid::new(g.w, g.h, b'3') });
    }
    pxl::save(&p.path, &p).unwrap();
    p.path.clone()
}

#[test]
fn load_reload_respect_locks() {
    let dir = tmpdir("cmd");
    let g = pattern(10, 10, 0, 3);
    let pp = write_part(&dir, &[("a", false), ("b", true)], &g, 2);
    let mut out = Out::new(false, None);
    load::run_load(&LoadArgs { parts: vec![pp.display().to_string()], ..Default::default() }, &mut out).unwrap();
    let p = pxl::load(&pp).unwrap();
    assert_eq!(p.frames[0].grid, g, "unlocked frame is reduced");
    assert_eq!(p.frames[1].grid, Grid::new(10, 10, b'3'), "locked frame is never touched");
    assert!(p.frames[1].locked);

    // reload with --frames limits the work; --force also takes locked frames (and unlocks them)
    let mut out = Out::new(false, None);
    load::run_reload(&ReloadArgs { parts: vec![pp.display().to_string()], sel: FrameSel { frames: vec!["b".into()], all: false }, force: false, ..Default::default() }, &mut out).unwrap();
    assert_eq!(pxl::load(&pp).unwrap().frames[1].grid, Grid::new(10, 10, b'3'));
    load::run_reload(&ReloadArgs { parts: vec![pp.display().to_string()], sel: FrameSel::default(), force: true, ..Default::default() }, &mut out).unwrap();
    let p = pxl::load(&pp).unwrap();
    assert_eq!(p.frames[1].grid, g);
    assert!(!p.frames[1].locked);
}

#[test]
fn load_is_deterministic_and_byte_stable() {
    let dir = tmpdir("det");
    let g = pattern(12, 12, 1, 9);
    let pp = write_part(&dir, &[("a", false), ("b", false)], &g, 2);
    let mut out = Out::new(false, None);
    let args = LoadArgs { parts: vec![pp.display().to_string()], ..Default::default() };
    load::run_load(&args, &mut out).unwrap();
    let first = std::fs::read(&pp).unwrap();
    load::run_load(&args, &mut out).unwrap();
    assert_eq!(first, std::fs::read(&pp).unwrap());
}

#[test]
fn load_new_creates_part_and_never_overwrites() {
    let dir = tmpdir("new");
    let g = pattern(8, 8, 0, 1);
    std::fs::write(dir.join("t.toml"), TEMPLATE).unwrap();
    let img = upscale(&g, 2, (0, 0), (16, 16));
    png::write_rgba(&dir.join("hero.png"), &img).unwrap();
    let out_path = dir.join("hero.pxl");
    let args = LoadArgs {
        new: Some("hero".into()),
        input: Some(dir.join("hero.png").display().to_string()),
        size: Some(Size { w: 8, h: 8 }),
        origin: Some(Point { x: 0, y: 0 }),
        template: Some("t.toml".into()),
        out: Some(out_path.display().to_string()),
        factor: Some(2),
        ..Default::default()
    };
    let mut out = Out::new(false, None);
    load::run_load(&args, &mut out).unwrap();
    let p = pxl::load(&out_path).unwrap();
    assert_eq!(p.frames.len(), 1);
    assert_eq!(p.frames[0].name, "hero");
    assert_eq!(p.frames[0].grid, g);
    let before = std::fs::read(&out_path).unwrap();
    assert!(load::run_load(&args, &mut out).is_err(), "must not overwrite");
    assert_eq!(before, std::fs::read(&out_path).unwrap());
}

#[test]
fn unknown_color_goes_to_nearest_base_key_and_near_rules_fall_back_to_under() {
    let (part, tpl, _d) = setup(2, 1, 1);
    let mut img = RgbaImage::new(2, 1);
    img.set(0, 0, Rgba::opaque(0x40, 0x50, 0xc0)); // not in the map: nearest base key is 2
    img.set(1, 0, Rgba::opaque(0xf0, 0xf0, 0xf0));
    let out = reduce::reduce_frame(&img, (0, 0), &part, &tpl).unwrap();
    assert_eq!(out.get(0, 0), b'2');
    assert_eq!(out.get(1, 0), b'W');
    let _ = Rgb::new(0, 0, 0);
}
