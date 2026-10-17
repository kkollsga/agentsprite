//! Eyes on a synthetic sprite (no private data): detection, survival metric, stamping in every method,
//! gaze side, silhouette, determinism, template hints and the `lost_eye` lint fix.

use agentsprite::features::{self, Geo};
use agentsprite::img::{Rgba, RgbaImage};
use agentsprite::model::{Frame, Grid, Method, Part, RefFrame};
use agentsprite::reduce::{self, Prep};
use agentsprite::{lint, template};
use std::path::PathBuf;

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
"#000000" = "1"
"##;

const CELLS: usize = 16;

fn tmpdir() -> PathBuf {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let d = std::env::temp_dir().join(format!("pxl_eyes_{}_{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn setup(f: u32, hint: &str) -> (Part, template::Template) {
    let dir = tmpdir();
    let tp = dir.join("t.toml");
    std::fs::write(&tp, format!("{}{}", TEMPLATE, hint)).unwrap();
    let tpl = template::load(&tp).unwrap();
    let mut p = Part::new("t", (CELLS, CELLS));
    p.path = dir.join("t.pxl");
    p.reduce.factor = Some(f);
    p.input.factor = Some(f);
    p.reduce.outline = vec!["body".into()];
    (p, tpl)
}

const WHITE: Rgba = Rgba::opaque(0xf0, 0xf0, 0xf0);
const BLACK: Rgba = Rgba::opaque(0, 0, 0);

/// Head (outline ring, skin inside) with an n x n cell eye at (6,6): white, pupil column on the right
/// (`gaze_right`) or the left. `shrink`: the eye is redrawn half a cell off the grid and one pixel smaller, so
/// no block is mostly eye and plain voting loses it.
fn scene(f: usize, n: usize, gaze_right: bool, shrink: bool) -> (RgbaImage, Grid) {
    let mut img = RgbaImage::new(CELLS * f, CELLS * f);
    let mut exact = Grid::new(CELLS, CELLS, b'.');
    let col = |k: u8| match k {
        b'1' => Rgba::opaque(0x18, 0x20, 0x78),
        b'3' => Rgba::opaque(0xf0, 0xb0, 0x90),
        _ => Rgba::TRANSPARENT,
    };
    for cy in 2..14 {
        for cx in 2..14 {
            let ring = cx == 2 || cx == 13 || cy == 2 || cy == 13;
            let k = if ring { b'1' } else { b'3' };
            exact.set(cx as i32, cy as i32, k);
            for j in 0..f {
                for i in 0..f {
                    img.set((cx * f + i) as i32, (cy * f + j) as i32, col(k));
                }
            }
        }
    }
    let pupil_col = if gaze_right { 6 + n - 1 } else { 6 };
    let mut fill = |x0: usize, y0: usize, w: usize, h: usize, c: Rgba| {
        for y in y0..y0 + h {
            for x in x0..x0 + w {
                img.set(x as i32, y as i32, c);
            }
        }
    };
    if !shrink {
        fill(6 * f, 6 * f, n * f, n * f, WHITE);
        fill(pupil_col * f, 7 * f, f, (n - 1) * f, BLACK);
        for cy in 6..6 + n {
            for cx in 6..6 + n {
                let k = if cx == pupil_col && cy >= 7 { b'1' } else { b'W' };
                exact.set(cx as i32, cy as i32, k);
            }
        }
    } else {
        let (a, b) = (6 * f + f / 2, 6 * f + f / 2);
        let wpx = n * f - f + 1; // white width in px, pupil takes about one cell
        fill(a, b, wpx, n * f - 1, WHITE);
        let px = if gaze_right { a + wpx } else { a - f };
        fill(px, b + 1, f, n * f - 2, BLACK);
    }
    (img, exact)
}

fn rf(img: RgbaImage) -> RefFrame {
    RefFrame { img, origin: (0, 0) }
}

fn mask(g: &Grid) -> Vec<bool> {
    g.cells.iter().map(|&c| c != b'.').collect()
}

fn pupil_right_of_white(g: &Grid, k: &features::Keys) -> bool {
    let (mut wx, mut wn, mut px, mut pn) = (0.0, 0, 0.0, 0);
    for y in 5..11 {
        for x in 4..12 {
            if g.get(x, y) == k.white {
                wx += x as f64;
                wn += 1;
            }
        }
    }
    for y in 5..11 {
        for x in 4..12 {
            // pupil key cells that touch a white cell
            if g.get(x, y) == k.pupil && (-1..=1).any(|dy| (-1..=1).any(|dx| g.get(x + dx, y + dy) == k.white)) {
                px += x as f64;
                pn += 1;
            }
        }
    }
    wn > 0 && pn > 0 && px / pn as f64 > wx / wn as f64
}

#[test]
fn detects_eyes_of_two_three_and_four_cells_at_factor_two_and_three() {
    for f in [2usize, 3] {
        for n in [2usize, 3, 4] {
            let (img, _) = scene(f, n, true, false);
            let (part, _tpl) = setup(f as u32, "");
            let r = rf(img);
            let eyes = features::eyes_of(&part, &r);
            assert_eq!(eyes.len(), 1, "f{} n{}: one eye", f, n);
            let e = &eyes[0];
            assert!(e.gaze.0 > 0.4, "f{} n{}: pupil on the right, gaze {:?}", f, n, e.gaze);
            assert!(e.cells.0 >= 5 && e.cells.2 <= 6 + n as i32, "f{} n{}: cells {:?}", f, n, e.cells);
            // a left-looking eye has a negative gaze
            let (img, _) = scene(f, n, false, false);
            let eyes = features::eyes_of(&part, &rf(img));
            assert_eq!(eyes.len(), 1);
            assert!(eyes[0].gaze.0 < -0.4, "left gaze {:?}", eyes[0].gaze);
        }
    }
}

#[test]
fn a_plain_head_and_a_white_glove_are_not_eyes() {
    let (mut img, _) = scene(2, 2, true, false);
    // erase the eye: skin again
    for y in 12..16 {
        for x in 12..18 {
            img.set(x, y, Rgba::opaque(0xf0, 0xb0, 0x90));
        }
    }
    let (part, _) = setup(2, "");
    assert!(features::eyes_of(&part, &rf(img.clone())).is_empty(), "no eye left");
    // white blob without a dark core is no eye
    for y in 12..16 {
        for x in 12..16 {
            img.set(x, y, WHITE);
        }
    }
    assert!(features::eyes_of(&part, &rf(img)).is_empty(), "white without a pupil");
}

#[test]
fn survival_metric_needs_white_and_a_pupil_on_the_gaze_side() {
    for f in [2usize, 3] {
        for n in [2usize, 3, 4] {
            let (img, exact) = scene(f, n, true, false);
            let (part, tpl) = setup(f as u32, "");
            let eyes = features::eyes_of(&part, &rf(img));
            let k = features::keys_for(&tpl).unwrap();
            assert_eq!((k.white, k.pupil), (b'W', b'1'));
            assert!(features::eye_ok(&exact, &eyes[0], &k), "f{} n{} exact eye survives", f, n);
            assert_eq!(features::eye_mismatch(&exact, &eyes[0], &k), 0, "exact eye matches the reference cell by cell");
            // white blob without pupil
            let mut blob = exact.clone();
            for y in 0..CELLS as i32 {
                for x in 0..CELLS as i32 {
                    if blob.get(x, y) == b'1' && (6..6 + n as i32).contains(&x) && (6..6 + n as i32).contains(&y) {
                        blob.set(x, y, b'W');
                    }
                }
            }
            assert!(!features::eye_ok(&blob, &eyes[0], &k), "f{} n{}: blob is lost", f, n);
            // pupil on the wrong side
            let mut wrong = exact.clone();
            for y in 6..6 + n as i32 {
                wrong.set(6 + n as i32 - 1, y, b'W');
                wrong.set(6, y, b'1');
            }
            wrong.set(6 + n as i32 - 1, 6, b'W');
            if n >= 3 {
                assert!(!features::eye_ok(&wrong, &eyes[0], &k), "f{} n{}: pupil on the wrong side is lost", f, n);
            }
            // no white at all
            let mut none = exact.clone();
            for y in 6..6 + n as i32 {
                for x in 6..6 + n as i32 {
                    none.set(x, y, b'3');
                }
            }
            assert!(!features::eye_ok(&none, &eyes[0], &k));
        }
    }
}

#[test]
fn every_method_keeps_the_eye_and_the_silhouette_and_the_gaze_side() {
    let mut raw_lost = 0;
    for f in [2usize, 3] {
        for n in [2usize, 3, 4] {
            for right in [true, false] {
                let (img, exact) = scene(f, n, right, true);
                let (part, tpl) = setup(f as u32, "");
                let k = features::keys_for(&tpl).unwrap();
                let r = rf(img.clone());
                let eyes = features::eyes_of(&part, &r);
                assert_eq!(eyes.len(), 1, "f{} n{} right{}: shrunk eye still detected", f, n, right);
                let prep = Prep::new(&img, (0, 0), &part, &tpl).unwrap();
                for m in [Method::Vote, Method::Contour, Method::Exemplar, Method::Auto] {
                    let g = reduce::reduce_named(&img, (0, 0), &part, &tpl, None, m, None).unwrap();
                    assert!(features::eye_ok(&g, &eyes[0], &k), "f{} n{} right{} {}: eye lost\n{}", f, n, right, m.name(), g.cells.chunks(CELLS).map(|r| String::from_utf8_lossy(r).to_string()).collect::<Vec<_>>().join("\n"));
                    assert_eq!(mask(&g), mask(&exact), "f{} n{} {}: silhouette changed", f, n, m.name());
                    let pr = pupil_right_of_white(&g, &k);
                    assert_eq!(pr, right, "f{} n{} {}: gaze side", f, n, m.name());
                }
                // the stamping pass is what saves it: plain voting alone loses some of these eyes
                let raw = reduce::vote_raw(&prep, &part, &tpl).unwrap();
                if !features::eye_ok(&raw, &eyes[0], &k) {
                    raw_lost += 1;
                }
            }
        }
    }
    assert!(raw_lost > 0, "the shrunk eyes must be hard enough that plain voting loses some");
}

#[test]
fn stamping_is_deterministic_and_never_changes_the_silhouette() {
    let (img, exact) = scene(2, 3, true, false);
    let (part, tpl) = setup(2, "");
    let eyes = features::eyes_of(&part, &rf(img));
    let mut wiped = exact.clone();
    for y in 6..9 {
        for x in 6..9 {
            wiped.set(x, y, b'3');
        }
    }
    let mut a = wiped.clone();
    let mut b = wiped.clone();
    assert!(features::stamp(&mut a, &eyes, &tpl, 2, &[]) >= 1);
    features::stamp(&mut b, &eyes, &tpl, 2, &[]);
    assert_eq!(a, b);
    assert_eq!(mask(&a), mask(&wiped));
    let k = features::keys_for(&tpl).unwrap();
    assert!(features::eye_ok(&a, &eyes[0], &k));
    // an eye that is already fine is untouched
    let mut c = exact.clone();
    assert_eq!(features::stamp(&mut c, &eyes, &tpl, 2, &[]), 0);
    assert_eq!(c, exact);
    // exemplar eye is preferred: a different, valid drawing of a same-size eye is copied
    let mut ex = exact.clone();
    ex.set(6, 6, b'W');
    let exeyes = features::eyes_of(&part, &rf(scene(2, 3, true, false).0));
    let mut d = wiped.clone();
    features::stamp(&mut d, &eyes, &tpl, 2, &[features::ExEye { eyes: &exeyes, grid: &ex }]);
    assert_eq!(d.get(7, 7), ex.get(7, 7));
    assert!(features::eye_ok(&d, &eyes[0], &k));
}

#[test]
fn reduction_is_deterministic() {
    let (img, _) = scene(3, 3, true, true);
    let (part, tpl) = setup(3, "");
    for m in [Method::Vote, Method::Contour, Method::Auto] {
        let a = reduce::reduce_named(&img, (0, 0), &part, &tpl, None, m, None).unwrap();
        let b = reduce::reduce_named(&img, (0, 0), &part, &tpl, None, m, None).unwrap();
        assert_eq!(a, b, "{}", m.name());
    }
}

#[test]
fn template_hint_names_the_eye_keys() {
    let (_, tpl) = setup(2, "");
    let k = features::keys_for(&tpl).unwrap();
    assert_eq!((k.white, k.pupil), (b'W', b'1'));
    let (_, tpl) = setup(2, "\n[features]\neye = { white = \"3\", pupil = \"body\" }\n");
    let k = features::keys_for(&tpl).unwrap();
    assert_eq!((k.white, k.pupil), (b'3', b'2'));
    let dir = tmpdir();
    let tp = dir.join("bad.toml");
    std::fs::write(&tp, format!("{}\n[features]\neye = {{ white = \"Q\" }}\n", TEMPLATE)).unwrap();
    assert!(template::load(&tp).is_err());
}

#[test]
fn lost_eye_lint_has_a_ready_fix() {
    let (img, exact) = scene(2, 3, true, false);
    let (part, tpl) = setup(2, "");
    let mut wiped = exact.clone();
    for y in 6..9 {
        for x in 6..9 {
            wiped.set(x, y, b'3');
        }
    }
    let r = rf(img);
    let (found, lost) = lint::eye_losses(&part, &tpl, &wiped, &r);
    assert_eq!((found, lost.len()), (1, 1));
    assert_eq!(lost[0].0.kind, "eye");
    let fix = lint::eye_fix_pixels(&part, &tpl, &wiped, &r, &lost[0].1).expect("a fix");
    // applying the fix pixels restores a surviving eye
    let mut fixed = wiped.clone();
    for p in fix.split(' ') {
        let (xy, k) = p.split_once('=').unwrap();
        let (x, y) = xy.split_once(',').unwrap();
        fixed.set(x.parse().unwrap(), y.parse().unwrap(), k.as_bytes()[0]);
    }
    assert_eq!(lint::eye_losses(&part, &tpl, &fixed, &r).1.len(), 0);
    // whole-part lint reports it with a `pxl set` command
    let mut p2 = part.clone();
    p2.frames = vec![Frame { name: "a".into(), src: None, locked: false, grid: wiped }];
    let m = agentsprite::metrics::compute(&p2, Some(&tpl), &p2.frames[0], &r, 0, 0);
    assert_eq!((m.eyes_found, m.eyes_kept), (1, 0));
    assert!(m.lost.iter().any(|l| l.kind == "eye"));
    let good = agentsprite::metrics::compute(&{ let mut p = part.clone(); p.frames = vec![Frame { name: "a".into(), src: None, locked: false, grid: exact }]; p }, Some(&tpl), &Frame { name: "a".into(), src: None, locked: false, grid: scene(2, 3, true, false).1 }, &r, 0, 0);
    assert_eq!((good.eyes_found, good.eyes_kept), (1, 1));
    assert!(m.score > good.score + 30.0, "a lost eye costs more than any shape difference: {} vs {}", m.score, good.score);
    let _ = Geo { org: (0, 0), f: 2 };
}

/// Head scene with a ring eye (white, dark pupil of `p` cells centred in the white of `n` cells), exact grid too.
fn ring_scene(f: usize, n: usize, p: usize) -> (RgbaImage, Grid) {
    let (mut img, mut exact) = scene(f, n, true, false);
    let skin = Rgba::opaque(0xf0, 0xb0, 0x90);
    for y in 6 * f..(6 + n) * f {
        for x in 6 * f..(6 + n) * f {
            img.set(x as i32, y as i32, skin);
        }
    }
    for cy in 6..6 + n {
        for cx in 6..6 + n {
            exact.set(cx as i32, cy as i32, b'W');
        }
    }
    for y in 6 * f..(6 + n) * f {
        for x in 6 * f..(6 + n) * f {
            img.set(x as i32, y as i32, WHITE);
        }
    }
    let o = 6 + (n - p) / 2;
    for cy in o..o + p {
        for cx in o..o + p {
            exact.set(cx as i32, cy as i32, b'1');
            for j in 0..f {
                for i in 0..f {
                    img.set((cx * f + i) as i32, (cy * f + j) as i32, BLACK);
                }
            }
        }
    }
    (img, exact)
}

#[test]
fn ring_eyes_with_a_centred_pupil_are_found_and_kept() {
    for f in [2usize, 3] {
        for (n, p) in [(3usize, 1usize), (4, 2), (5, 1)] {
            let (img, exact) = ring_scene(f, n, p);
            let (part, tpl) = setup(f as u32, "");
            let eyes = features::eyes_of(&part, &rf(img));
            assert_eq!(eyes.len(), 1, "f{} n{} p{}: one ring eye", f, n, p);
            let k = features::keys_for(&tpl).unwrap();
            assert!(features::eye_ok(&exact, &eyes[0], &k), "f{} n{} p{}: centred pupil is kept (gaze {:?})", f, n, p, eyes[0].gaze);
        }
    }
}

#[test]
fn a_highlight_on_the_body_edge_is_not_an_eye() {
    // a light patch on the head touching the outline ring on one side only: no dark core, not an eye
    let (mut img, _) = scene(2, 2, true, false);
    let skin = Rgba::opaque(0xf0, 0xb0, 0x90);
    for y in 12..16 {
        for x in 12..18 {
            img.set(x, y, skin);
        }
    }
    for y in 3 * 2..5 * 2 {
        for x in 3 * 2..5 * 2 {
            img.set(x, y, WHITE);
        }
    }
    let (part, _) = setup(2, "");
    assert!(features::eyes_of(&part, &rf(img)).is_empty());
}
