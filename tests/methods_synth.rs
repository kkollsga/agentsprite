//! Downscale methods on synthetic data (no private test data): determinism, `nearest`, `contour`, `exemplar`,
//! `--try`, `.pxl` round trip of `[reduce] method`.

use agentsprite::cli::{FrameSel, LoadArgs, ReloadArgs};
use agentsprite::img::{Rgba, RgbaImage};
use agentsprite::lint::{self, LintOpts};
use agentsprite::model::{Frame, Grid, Method, Part};
use agentsprite::output::Out;
use agentsprite::{load, png, profile, pxl, reduce, template};
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
    let d = std::env::temp_dir().join(format!("pxl_methods_{}_{}_{}", tag, std::process::id(), n));
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

fn pattern(w: usize, h: usize, m: usize, seed: u64) -> Grid {
    let mut g = Grid::new(w, h, b'.');
    let mut s = seed;
    for y in m..h - m {
        for x in m..w - m {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            g.cells[y * w + x] = match (s >> 33) % 7 {
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

fn upscale(g: &Grid, f: usize) -> RgbaImage {
    let mut img = RgbaImage::new(g.w * f, g.h * f);
    for y in 0..g.h * f {
        for x in 0..g.w * f {
            img.set(x as i32, y as i32, color_of(g.cells[(y / f) * g.w + x / f]));
        }
    }
    img
}

fn setup(w: usize, h: usize, f: u32) -> (Part, template::Template, PathBuf) {
    let dir = tmpdir("s");
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
fn every_method_is_deterministic() {
    let (mut part, tpl, _d) = setup(16, 16, 3);
    part.reduce.outline = vec!["body".into()];
    part.reduce.thin = true;
    let mut noisy = upscale(&pattern(16, 16, 1, 21), 3);
    // break the block structure so that the methods really have to decide
    for i in (0..noisy.px.len()).step_by(7) {
        noisy.px[i] = color_of(b'W');
    }
    for m in Method::ALL {
        let a = reduce::reduce_named(&noisy, (0, 0), &part, &tpl, None, m, None).unwrap();
        let b = reduce::reduce_named(&noisy, (0, 0), &part, &tpl, None, m, None).unwrap();
        assert_eq!(a, b, "{}", m.name());
        assert_eq!((a.w, a.h), (16, 16));
    }
}

#[test]
fn nearest_is_exact_on_nearest_upscales() {
    for f in [2usize, 3] {
        let (part, tpl, _d) = setup(16, 16, f as u32);
        let g = pattern(16, 16, 0, 40 + f as u64);
        let out = reduce::reduce_named(&upscale(&g, f), (0, 0), &part, &tpl, None, Method::Nearest, None).unwrap();
        assert_eq!(out, g, "factor {}", f);
    }
}

/// Filled shape: `inside(x, y)` in cell coordinates, drawn at factor 2 in key 2.
fn shape_image(w: usize, h: usize, inside: &dyn Fn(f64, f64) -> bool) -> RgbaImage {
    let f = 2;
    let mut img = RgbaImage::new(w * f, h * f);
    for y in 0..h * f {
        for x in 0..w * f {
            if inside((x as f64 + 0.5) / f as f64, (y as f64 + 0.5) / f as f64) {
                img.set(x as i32, y as i32, color_of(b'2'));
            }
        }
    }
    img
}

fn l_orphans(part: &Part, tpl: &template::Template, g: &Grid) -> Vec<String> {
    let mut p = part.clone();
    p.frames = vec![Frame { name: "s".into(), src: None, locked: false, grid: g.clone() }];
    let prof = profile::load_for(&p, None).unwrap();
    let opts = LintOpts { rules: vec!["orphan".into(), "jaggy".into()], ..Default::default() };
    lint::lint_part(&p, tpl, &prof, &opts).iter().map(|i| format!("{} at {:?},{:?}", i.rule, i.x, i.y)).collect()
}

#[test]
fn contour_closed_outline_no_orphans_no_jaggies() {
    let shapes: Vec<(&str, Box<dyn Fn(f64, f64) -> bool>)> = vec![
        ("disc", Box::new(|x, y| (x - 12.0).powi(2) + (y - 12.0).powi(2) <= 8.3f64.powi(2))),
        ("ellipse", Box::new(|x, y| ((x - 12.0) / 10.0).powi(2) + ((y - 12.0) / 6.0).powi(2) <= 1.0)),
        ("box_notch", Box::new(|x, y| (4.0..20.0).contains(&x) && (5.0..19.0).contains(&y) && !((11.0..14.0).contains(&x) && y < 9.0))),
        ("diamond", Box::new(|x, y| (x - 12.0).abs() + (y - 12.0).abs() <= 9.0)),
    ];
    for (name, inside) in shapes {
        let (mut part, tpl, _d) = setup(24, 24, 2);
        part.reduce.outline = vec!["body".into()];
        part.reduce.thin = true;
        let img = shape_image(24, 24, &*inside);
        let g = reduce::reduce_named(&img, (0, 0), &part, &tpl, None, Method::Contour, None).unwrap();
        assert!(g.cells.iter().any(|&k| k != b'.'), "{}: empty", name);
        // closed outline: every opaque cell with a transparent 4-neighbour is the outline key
        for y in 0..24i32 {
            for x in 0..24i32 {
                if g.get(x, y) != b'.' && [(0, 1), (1, 0), (0, -1), (-1, 0)].iter().any(|d| !g.in_bounds(x + d.0, y + d.1) || g.get(x + d.0, y + d.1) == b'.') {
                    assert_eq!(g.get(x, y), b'1', "{}: ring cell {},{} is not outline\n{}", name, x, y, g.cells.chunks(24).map(|r| String::from_utf8_lossy(r).to_string()).collect::<Vec<_>>().join("\n"));
                }
            }
        }
        let issues = l_orphans(&part, &tpl, &g);
        assert!(issues.is_empty(), "{}: {:?}\n{}", name, issues, g.cells.chunks(24).map(|r| String::from_utf8_lossy(r).to_string()).collect::<Vec<_>>().join("\n"));
    }
}

#[test]
fn contour_keeps_eye_with_pupil_as_one_shape() {
    // body disc with a 4x4 white eye holding a 2x2 pupil (source pixels at factor 2: 2x2 cells, pupil 1 cell)
    let (mut part, tpl, _d) = setup(24, 24, 2);
    part.reduce.outline = vec!["body".into()];
    let mut img = shape_image(24, 24, &|x, y| (x - 12.0).powi(2) + (y - 12.0).powi(2) <= 8.3f64.powi(2));
    for y in 10..14 {
        for x in 10..14 {
            img.set(x * 2, y * 2, color_of(b'W'));
            img.set(x * 2 + 1, y * 2, color_of(b'W'));
            img.set(x * 2, y * 2 + 1, color_of(b'W'));
            img.set(x * 2 + 1, y * 2 + 1, color_of(b'W'));
        }
    }
    for y in 12..14 {
        for x in 12..14 {
            for j in 0..2 {
                for i in 0..2 {
                    img.set(x * 2 + i, y * 2 + j, color_of(b'1'));
                }
            }
        }
    }
    let g = reduce::reduce_named(&img, (0, 0), &part, &tpl, None, Method::Contour, None).unwrap();
    let count = |k: u8| g.cells.iter().filter(|&&c| c == k).count();
    assert_eq!(g.get(10, 10), b'W');
    assert_eq!(g.get(12, 12), b'1');
    assert!(count(b'W') >= 10 && count(b'W') <= 14, "white cells {}", count(b'W'));
}

fn write_two_frames(dir: &Path, a: &Grid, b_img: &RgbaImage, a_grid: &Grid, f: usize) -> PathBuf {
    std::fs::create_dir_all(dir.join("refs")).unwrap();
    std::fs::write(dir.join("t.toml"), TEMPLATE).unwrap();
    let mut p = Part::new("t", (a.w, a.h));
    p.path = dir.join("t.pxl");
    p.template = Some("t.toml".into());
    p.input.images = Some("refs/{frame}.png".into());
    p.input.factor = Some(f as u32);
    p.reduce.factor = Some(f as u32);
    p.reduce.method = Method::Exemplar;
    png::write_rgba(&dir.join("refs/a.png"), &upscale(a, f)).unwrap();
    png::write_rgba(&dir.join("refs/b.png"), b_img).unwrap();
    p.frames.push(Frame { name: "a".into(), src: None, locked: true, grid: a_grid.clone() });
    p.frames.push(Frame { name: "b".into(), src: None, locked: false, grid: Grid::new(a.w, a.h, b'.') });
    pxl::save(&p.path, &p).unwrap();
    p.path.clone()
}

#[test]
fn exemplar_reproduces_its_own_frame_and_copies_identical_regions() {
    let dir = tmpdir("ex");
    let g = pattern(16, 16, 1, 5);
    // the approved grid differs from what the reference reduces to (hand edits)
    let mut approved = g.clone();
    for (x, y, k) in [(4, 4, b'W'), (5, 4, b'W'), (9, 10, b'3'), (10, 10, b'3'), (3, 12, b'1')] {
        approved.set(x, y, k);
    }
    // frame b: the same picture with the bottom right corner changed
    let mut b_img = upscale(&g, 2);
    for y in 26..32 {
        for x in 26..32 {
            b_img.set(x, y, color_of(b'W'));
        }
    }
    let pp = write_two_frames(&dir, &g, &b_img, &approved, 2);
    let part = pxl::load(&pp).unwrap();
    let tpl = template::load_for(&part).unwrap();
    // own reference, not held out: exact
    let rf = load::reference(&part, &part.dir(), &part.frames[0]).unwrap();
    let own = reduce::reduce_named(&rf.img, rf.origin, &part, &tpl, None, Method::Exemplar, None).unwrap();
    assert_eq!(own, approved);
    // frame b: unchanged regions keep the approved (hand edited) cells
    let rb = load::reference(&part, &part.dir(), &part.frames[1]).unwrap();
    let out = reduce::reduce_named(&rb.img, rb.origin, &part, &tpl, None, Method::Exemplar, None).unwrap();
    for y in 0..16i32 {
        for x in 0..16i32 {
            if x < 12 && y < 12 {
                assert_eq!(out.get(x, y), approved.get(x, y), "cell {},{}", x, y);
            }
        }
    }
    // vote alone would not have reproduced the hand edit
    let v = reduce::reduce_named(&rb.img, rb.origin, &part, &tpl, None, Method::Vote, None).unwrap();
    assert_eq!(v.get(9, 10), g.get(9, 10));
    assert_ne!(approved.get(9, 10), g.get(9, 10));
}

#[test]
fn try_writes_nothing_and_override_does_not_persist() {
    let dir = tmpdir("try");
    let g = pattern(12, 12, 1, 9);
    let pp = write_two_frames(&dir, &g, &upscale(&g, 2), &g, 2);
    let before = std::fs::read(&pp).unwrap();
    let listing = |d: &Path| -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(d).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).collect();
        v.sort();
        v
    };
    let files = listing(&dir);
    let mut out = Out::new(false, None);
    let a = ReloadArgs { parts: vec![pp.display().to_string()], try_methods: vec!["vote".into(), "nearest".into(), "contour".into(), "exemplar".into(), "auto".into()], ..Default::default() };
    load::run_reload(&a, &mut out).unwrap();
    assert_eq!(std::fs::read(&pp).unwrap(), before);
    assert_eq!(listing(&dir), files);
    // previews go only where asked
    let pd = dir.join("prev");
    let a = ReloadArgs { preview_dir: Some(pd.display().to_string()), ..a };
    load::run_reload(&a, &mut out).unwrap();
    assert_eq!(std::fs::read(&pp).unwrap(), before);
    assert!(pd.join("a.png").is_file() && pd.join("b.png").is_file());
    // --method is for this run only; --set-method stores it
    let a = ReloadArgs { parts: vec![pp.display().to_string()], method: Some("nearest".into()), sel: FrameSel::default(), ..Default::default() };
    load::run_reload(&a, &mut out).unwrap();
    assert_eq!(pxl::load(&pp).unwrap().reduce.method, Method::Exemplar);
    let a = ReloadArgs { set_method: true, ..a };
    load::run_reload(&a, &mut out).unwrap();
    assert_eq!(pxl::load(&pp).unwrap().reduce.method, Method::Nearest);
    // unknown method names are usage errors
    let bad = LoadArgs { parts: vec![pp.display().to_string()], method: Some("blur".into()), ..Default::default() };
    assert!(load::run_load(&bad, &mut out).is_err());
}

#[test]
fn pxl_round_trip_with_method() {
    let src = "name = \"p\"\nsize = [4, 4]\n\n[reduce]\nmethod = \"contour\"\nexemplars = [\"../golden/a.txt\"]\nfactor = 2\n\n[[frames]]\nname = \"a\"\ngrid = \"\"\"\n....\n....\n....\n....\n\"\"\"\n";
    let p = pxl::parse_str(src, Path::new("p.pxl")).unwrap();
    assert_eq!(p.reduce.method, Method::Contour);
    assert_eq!(p.reduce.exemplars, vec!["../golden/a.txt".to_string()]);
    let s = pxl::to_string(&p);
    assert!(s.contains("method = \"contour\""));
    assert_eq!(pxl::to_string(&pxl::parse_str(&s, Path::new("p.pxl")).unwrap()), s);
    // the default method is not serialized, an explicit "vote" normalises away
    let q = pxl::parse_str(&src.replace("\"contour\"", "\"vote\""), Path::new("p.pxl")).unwrap();
    assert!(!pxl::to_string(&q).contains("method"));
    assert!(pxl::parse_str(&src.replace("\"contour\"", "\"blur\""), Path::new("p.pxl")).is_err());
}
