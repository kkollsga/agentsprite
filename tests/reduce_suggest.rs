//! `template suggest`: scored candidates, never overwrites.

use agentsprite::cli::SuggestArgs;
use agentsprite::color::Rgb;
use agentsprite::img::{Rgba, RgbaImage};
use agentsprite::model::{Frame, Grid, Part};
use agentsprite::output::Out;
use agentsprite::{png, pxl, suggest, template};
use std::collections::BTreeMap;

fn setup(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("pxl_suggest_{}_{}", tag, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("refs")).unwrap();
    // 24x24 picture: dark outline, a blue body with a lighter shade, a skin patch; two near-duplicate colours
    let mut img = RgbaImage::new(24, 24);
    for y in 2..22 {
        for x in 2..22 {
            let c = if x == 2 || y == 2 || x == 21 || y == 21 {
                Rgba::opaque(0x10, 0x18, 0x70)
            } else if x < 12 {
                if (x + y) % 5 == 0 { Rgba::opaque(0x4c, 0x54, 0xb4) } else { Rgba::opaque(0x48, 0x50, 0xb8) }
            } else {
                Rgba::opaque(0xf0, 0xb0, 0x90)
            };
            img.set(x, y, c);
        }
    }
    png::write_rgba(&dir.join("refs/a.png"), &img).unwrap();
    let mut p = Part::new("demo", (12, 12));
    p.path = dir.join("demo.pxl");
    p.input.images = Some("refs/{frame}.png".into());
    p.input.factor = Some(2);
    p.reduce.factor = Some(2);
    p.frames.push(Frame { name: "a".into(), src: None, locked: false, grid: Grid::new(12, 12, b'.') });
    pxl::save(&p.path, &p).unwrap();
    p.path.clone()
}

#[test]
fn suggests_scored_candidates_and_writes_only_new_files() {
    let pp = setup("a");
    let dir = pp.parent().unwrap().to_path_buf();
    let target = dir.join("templates/new.toml");
    let args = SuggestArgs { part: pp.display().to_string(), candidates: 4, write: Some(target.display().to_string()), pick: 1, mode: "optimised".into(), ..Default::default() };
    let mut out = Out::new(false, None);
    agentsprite::suggest::run_suggest(&args, &mut out).unwrap();
    let t = template::load(&target).expect("the written template must load");
    let base = &t.layers[0];
    assert_eq!(base.keys.len(), 3, "three colours per layer (gbc-sprite default)");
    assert!(t.def(b'1').unwrap().role == "outline");
    // key 1 is the darkest colour
    let l = |k: u8| agentsprite::color::srgb_to_oklab(t.color_of(k).unwrap()).l;
    assert!(l(b'1') < l(b'2') && l(b'2') < l(b'3'));
    // never overwrites
    let before = std::fs::read(&target).unwrap();
    assert!(agentsprite::suggest::run_suggest(&args, &mut Out::new(false, None)).is_err());
    assert_eq!(before, std::fs::read(&target).unwrap());
}

#[test]
fn candidates_are_deterministic_and_sorted_by_error() {
    let mut hist: BTreeMap<Rgb, u64> = BTreeMap::new();
    for (i, (r, g, b)) in [(16u8, 24u8, 112u8), (72, 80, 184), (76, 84, 180), (240, 176, 144), (255, 255, 255), (0, 0, 0)].iter().enumerate() {
        hist.insert(Rgb::new(*r, *g, *b), 10 + 7 * i as u64);
    }
    let a = suggest::candidates(&hist, 3);
    let b = suggest::candidates(&hist, 3);
    assert!(!a.is_empty());
    assert_eq!(a.iter().map(|c| (c.name, c.colors.clone(), c.score)).collect::<Vec<_>>(), b.iter().map(|c| (c.name, c.colors.clone(), c.score)).collect::<Vec<_>>());
    assert!(a.windows(2).all(|w| w[0].score <= w[1].score));
    for c in &a {
        assert!(c.colors.len() <= 3);
        assert!(c.colors.iter().all(|x| agentsprite::color::is_step_aligned(*x, 8)), "RGB555 legal");
    }
}
