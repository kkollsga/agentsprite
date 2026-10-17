//! `template suggest --derive`: colour template derived from native resolution inputs (DESIGN 4.3, PLAN palette).

use agentsprite::cli::SuggestArgs;
use agentsprite::color::{self, Rgb};
use agentsprite::img::{Rgba, RgbaImage};
use agentsprite::model::{Frame, Grid, Part};
use agentsprite::output::Out;
use agentsprite::suggest::derive::{self, Data, Mode, Opts};
use agentsprite::{png, pxl, template};
use std::path::PathBuf;

const NAVY: (u8, u8, u8) = (0x18, 0x20, 0x78);
const BLUE: (u8, u8, u8) = (0x48, 0x50, 0xb8);
const SKIN: (u8, u8, u8) = (0xf0, 0xb0, 0x90);
const WHITE: (u8, u8, u8) = (0xf8, 0xf8, 0xf8);
const RED: (u8, u8, u8) = (0xf8, 0x10, 0x10);
const GREEN: (u8, u8, u8) = (0x20, 0xc0, 0x30);

fn px(c: (u8, u8, u8)) -> Rgba {
    Rgba::opaque(c.0, c.1, c.2)
}

/// 24x24 sprite: navy outline, blue body (x < 12), skin head part; optional details drawn on top.
fn sprite(details: &[((usize, usize), (u8, u8, u8))]) -> RgbaImage {
    let mut img = RgbaImage::new(24, 24);
    for y in 2..22 {
        for x in 2..22 {
            let c = if x == 2 || y == 2 || x == 21 || y == 21 {
                NAVY
            } else if x < 12 {
                BLUE
            } else {
                SKIN
            };
            img.set(x as i32, y as i32, px(c));
        }
    }
    for &((x, y), c) in details {
        img.set(x as i32, y as i32, px(c));
    }
    img
}

fn eye(dx: usize, dy: usize) -> Vec<((usize, usize), (u8, u8, u8))> {
    vec![((15 + dx, 8 + dy), WHITE), ((16 + dx, 8 + dy), WHITE)]
}

fn opts(layers: Option<u8>) -> Opts {
    Opts { k: 3, step: 8, layers, mode: Mode::Optimised, name: "t".into(), cells: false }
}

fn frames(details: &[Vec<((usize, usize), (u8, u8, u8))>]) -> Data {
    Data::build(details.iter().map(|d| sprite(d)).collect(), 2)
}

fn lab_dist(a: Rgb, b: (u8, u8, u8)) -> f64 {
    color::oklab_dist2(color::srgb_to_oklab(a), color::srgb_to_oklab(Rgb::new(b.0, b.1, b.2))).sqrt()
}

#[test]
fn derivation_is_deterministic_and_text_round_trips() {
    let d = frames(&[eye(0, 0), eye(1, 0), vec![((10, 10), RED), ((11, 10), RED)]]);
    let a = derive::derive_from_data(&d, &opts(None));
    let b = derive::derive_from_data(&d, &opts(None));
    assert_eq!(a.text, b.text);
    // the written text loads as the same template (layers and colours, every map rule)
    let dir = std::env::temp_dir().join(format!("pxl_palette_rt_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("t.toml");
    std::fs::write(&f, &a.text).unwrap();
    let t = template::load(&f).expect("derived template must parse");
    assert_eq!(t.layers, a.template.layers);
    assert_eq!(t.map.len(), a.template.map.len());
    let mut ra = a.template.map.clone();
    let mut rb = t.map.clone();
    ra.sort_by_key(|r| r.from);
    rb.sort_by_key(|r| r.from);
    assert_eq!(ra, rb);
}

#[test]
fn every_source_colour_has_an_explicit_rule_and_limits_hold() {
    let d = frames(&[eye(0, 0), vec![((10, 10), RED), ((11, 10), RED), ((5, 5), GREEN)]]);
    let r = derive::derive_from_data(&d, &opts(None));
    for c in &d.cols {
        assert!(r.template.map.iter().any(|m| m.from == c.rgb), "no map rule for {}", color::to_hex(c.rgb));
    }
    assert!(r.report.rows.iter().all(|x| !x.implicit));
    for l in &r.template.layers {
        assert!(l.keys.len() <= 3, "layer {} has {} colours", l.name, l.keys.len());
        for k in &l.keys {
            assert!(color::is_step_aligned(k.color.unwrap(), 8), "{} not step aligned", color::to_hex(k.color.unwrap()));
        }
    }
    assert!(r.report.valid(), "{:?}", r.report.issues);
    // `1` is the darkest base colour and `.` is not defined as a colour
    let base = &r.template.layers[0];
    let l = |c: Rgb| color::srgb_to_oklab(c).l;
    let k1 = base.keys.iter().find(|k| k.key == b'1').unwrap();
    assert_eq!(k1.role, "outline");
    assert!(base.keys.iter().all(|k| l(k.color.unwrap()) >= l(k1.color.unwrap()) - 1e-9));
    // overlay keys are letters with an `under` base key
    for ov in r.template.layers.iter().skip(1) {
        for k in &ov.keys {
            assert!(k.key.is_ascii_alphabetic());
            assert_eq!(r.template.layer_of(k.under.unwrap()), Some(0));
        }
    }
}

#[test]
fn a_two_pixel_eye_keeps_a_colour_of_its_own() {
    // base: navy, blue, skin (big areas); the white eye is 2 px per frame
    let d = frames(&[eye(0, 0), eye(0, 1), eye(1, 0)]);
    for layers in [None, Some(2u8)] {
        let r = derive::derive_from_data(&d, &opts(layers));
        let white = Rgb::new(WHITE.0, WHITE.1, WHITE.2);
        let key = r.template.map.iter().find(|m| m.from == white).expect("white has a rule").key;
        let c = r.template.color_of(key).unwrap();
        assert!(lab_dist(c, WHITE) < 0.06, "eye white maps to {} (layers {:?})", color::to_hex(c), layers);
        // and the big areas stay exact
        for area in [NAVY, BLUE, SKIN] {
            let k = r.template.map.iter().find(|m| m.from == Rgb::new(area.0, area.1, area.2)).unwrap().key;
            assert!(lab_dist(r.template.color_of(k).unwrap(), area) < 0.03);
        }
    }
}

#[test]
fn feature_weighting_marks_small_contrasting_details_only() {
    let d = frames(&[eye(0, 0)]);
    let w = d.cols.iter().find(|c| c.rgb == Rgb::new(WHITE.0, WHITE.1, WHITE.2)).unwrap();
    assert_eq!(w.feat, w.n, "the eye is a feature");
    assert!(w.w > w.n as f64 * 5.0, "and weighs much more than its area");
    let b = d.cols.iter().find(|c| c.rgb == Rgb::new(BLUE.0, BLUE.1, BLUE.2)).unwrap();
    assert_eq!(b.w, b.n as f64, "the body is plain area");
}

#[test]
fn an_overlay_is_added_only_when_it_pays() {
    // exactly three colours: nothing to gain
    let plain = frames(&[vec![]]);
    let r = derive::derive_from_data(&plain, &opts(None));
    assert_eq!(r.template.layers.len(), 1, "no overlay for a 3 colour sprite");
    assert!(r.report.merr < 0.01);
    // forced base only, even with details
    let rich = frames(&[eye(0, 0), vec![((10, 10), RED), ((11, 10), RED), ((5, 5), GREEN), ((6, 5), GREEN)]]);
    let r1 = derive::derive_from_data(&rich, &opts(Some(1)));
    assert_eq!(r1.template.layers.len(), 1);
    // with several distinct details the default adds one, and the error drops
    let r2 = derive::derive_from_data(&rich, &opts(None));
    assert_eq!(r2.template.layers.len(), 2, "trace: {:?}", r2.trace);
    assert!(r2.report.werr < r1.report.werr);
    assert!(r2.report.overlay_share < 0.1, "the overlay holds only the details");
}

#[test]
fn near_rule_comes_from_spatial_co_occurrence() {
    // dark red appears next to red details; a stray dark red pixel far from any red is not overlaid
    let mut dets = vec![];
    for x in 14..18 {
        dets.push(((x, 10), RED));
        dets.push(((x, 11), (0x90, 0x00, 0x00)));
    }
    dets.push(((4, 18), (0x90, 0x00, 0x00)));
    let d = frames(&[dets.clone(), dets]);
    let r = derive::derive_from_data(&d, &opts(Some(2)));
    let dark = Rgb::new(0x90, 0, 0);
    let red = Rgb::new(RED.0, RED.1, RED.2);
    let rd = r.template.map.iter().find(|m| m.from == dark).unwrap();
    let rr = r.template.map.iter().find(|m| m.from == red).unwrap();
    // whichever the layer split, rules must be consistent: a near rule always names a colour of the inputs
    for rule in [rd, rr] {
        if let Some((c, rad)) = rule.near {
            assert!(d.cols.iter().any(|x| x.rgb == c));
            assert!(rad >= 2);
        }
    }
}

// ------------------------------------------------------------------ CLI

fn setup(tag: &str, frames: &[RgbaImage]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pxl_palette_{}_{}", tag, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("refs")).unwrap();
    let mut p = Part::new("demo", (12, 12));
    p.path = dir.join("demo.pxl");
    p.input.images = Some("refs/{frame}.png".into());
    p.input.factor = Some(2);
    p.reduce.factor = Some(2);
    for (i, img) in frames.iter().enumerate() {
        let name = format!("f{}", i);
        png::write_rgba(&dir.join(format!("refs/{}.png", name)), img).unwrap();
        p.frames.push(Frame { name, src: None, locked: false, grid: Grid::new(12, 12, b'.') });
    }
    pxl::save(&p.path, &p).unwrap();
    p.path.clone()
}

fn args(part: &PathBuf) -> SuggestArgs {
    SuggestArgs { part: part.display().to_string(), candidates: 4, pick: 1, mode: "optimised".into(), derive: true, ..Default::default() }
}

#[test]
fn derive_write_never_overwrites_and_compare_lists_both() {
    let pp = setup("cli", &[sprite(&eye(0, 0)), sprite(&eye(1, 1))]);
    let dir = pp.parent().unwrap().to_path_buf();
    let target = dir.join("templates/derived.toml");
    let mut a = args(&pp);
    a.write = Some(target.display().to_string());
    agentsprite::suggest::run_suggest(&a, &mut Out::new(false, None)).unwrap();
    let t = template::load(&target).expect("the written template loads");
    assert!(t.layers[0].keys.len() <= 3);
    let before = std::fs::read(&target).unwrap();
    assert!(agentsprite::suggest::run_suggest(&a, &mut Out::new(false, None)).is_err());
    assert_eq!(before, std::fs::read(&target).unwrap());
    // compare against a second template: one-layer derivation
    let other = dir.join("templates/base_only.toml");
    let mut b = args(&pp);
    b.layers = Some(1);
    b.write = Some(other.display().to_string());
    agentsprite::suggest::run_suggest(&b, &mut Out::new(false, None)).unwrap();
    let mut c = SuggestArgs { part: pp.display().to_string(), mode: "optimised".into(), pick: 1, ..Default::default() };
    c.compare = vec![target.display().to_string(), other.display().to_string()];
    let mut out = Out::new(true, None);
    agentsprite::suggest::run_suggest(&c, &mut out).unwrap();
    let (stdout, _, code) = out.render(&Ok(()));
    assert_eq!(code, 0);
    assert!(stdout.contains("weighted_error"), "{}", stdout);
    // keep-colours: map derived for the colours of an existing template
    let m = dir.join("templates/map.toml");
    let mut k = args(&pp);
    k.keep_colors = Some(other.display().to_string());
    k.write = Some(m.display().to_string());
    agentsprite::suggest::run_suggest(&k, &mut Out::new(false, None)).unwrap();
    let tm = template::load(&m).unwrap();
    let to = template::load(&other).unwrap();
    assert_eq!(tm.layers.iter().map(|l| l.keys.iter().map(|k| (k.key, k.color)).collect::<Vec<_>>()).collect::<Vec<_>>(), to.layers.iter().map(|l| l.keys.iter().map(|k| (k.key, k.color)).collect::<Vec<_>>()).collect::<Vec<_>>());
    assert!(tm.map.len() >= 4);
}

#[test]
fn invalid_options_are_usage_errors() {
    let pp = setup("bad", &[sprite(&[])]);
    let mut a = args(&pp);
    a.mode = "fancy".into();
    assert!(agentsprite::suggest::run_suggest(&a, &mut Out::new(false, None)).is_err());
    let mut a = args(&pp);
    a.layers = Some(3);
    assert!(agentsprite::suggest::run_suggest(&a, &mut Out::new(false, None)).is_err());
}
