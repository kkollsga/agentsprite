//! `template suggest --derive` keeps saturated identity hues (a blue body must not become grey).

use agentsprite::color::{self, Rgb};
use agentsprite::img::{Rgba, RgbaImage};
use agentsprite::suggest::derive::{self, Data, Mode, Opts};

const BLACK: (u8, u8, u8) = (0x10, 0x10, 0x10);
const GREY_L: (u8, u8, u8) = (0xb8, 0xb8, 0xb8);
const GREY_D: (u8, u8, u8) = (0x78, 0x78, 0x78);
const BLUE: (u8, u8, u8) = (0x30, 0x48, 0xd8);
const BLUE_D: (u8, u8, u8) = (0x20, 0x30, 0x98);
const RED: (u8, u8, u8) = (0xe0, 0x28, 0x28);

fn rgb(c: (u8, u8, u8)) -> Rgb {
    Rgb::new(c.0, c.1, c.2)
}

/// 32x32 robot: black outline, big grey body (two shades), a blue chest block (two shades) and a small red lamp.
fn robot() -> RgbaImage {
    let mut img = RgbaImage::new(32, 32);
    for y in 2..30 {
        for x in 2..30 {
            let c = if x == 2 || y == 2 || x == 29 || y == 29 {
                BLACK
            } else if (6..14).contains(&x) && (6..14).contains(&y) {
                if y < 10 {
                    BLUE
                } else {
                    BLUE_D
                }
            } else if (20..26).contains(&x) && (6..10).contains(&y) {
                RED
            } else if y < 18 {
                GREY_L
            } else {
                GREY_D
            };
            img.set(x as i32, y as i32, Rgba::opaque(c.0, c.1, c.2));
        }
    }
    img
}

fn chroma(c: Rgb) -> f64 {
    let l = color::srgb_to_oklab(c);
    (l.a * l.a + l.b * l.b).sqrt()
}

#[test]
fn derive_keeps_blue_and_red_identity_colours() {
    let d = Data::build(vec![robot()], 2);
    for mode in [Mode::Real, Mode::Optimised] {
        for layers in [Some(1u8), None] {
            let o = Opts { k: 3, step: 8, layers, mode, name: "t".into(), cells: false };
            let r = derive::derive_from_data(&d, &o);
            for (name, src) in [("blue", BLUE), ("blue dark", BLUE_D)] {
                let k = r.template.map.iter().find(|m| m.from == rgb(src)).unwrap_or_else(|| panic!("no rule for {}", name)).key;
                let c = r.template.color_of(k).unwrap();
                assert!(chroma(c) > 0.08, "{} maps to desaturated {} ({:?} {:?}) {:#?}", name, color::to_hex(c), mode, layers, r.trace);
            }
            assert!(r.template.layers[0].keys.iter().filter(|k| k.color.is_some()).count() <= 3);
        }
    }
}

#[test]
fn derive_keeps_red_too_when_the_budget_allows() {
    let d = Data::build(vec![robot()], 2);
    let o = Opts { k: 5, step: 8, layers: Some(1), mode: Mode::Real, name: "t".into(), cells: false };
    let r = derive::derive_from_data(&d, &o);
    let key_of = |c| r.template.map.iter().find(|m| m.from == rgb(c)).unwrap().key;
    for src in [BLUE, RED] {
        let c = r.template.color_of(key_of(src)).unwrap();
        assert!(chroma(c) > 0.08, "{:?} maps to {}", src, color::to_hex(c));
    }
    assert_ne!(key_of(BLUE), key_of(RED));
}

/// Mostly grey casing (two greys) with a small saturated brown/olive part.
fn casing(hue: (u8, u8, u8)) -> RgbaImage {
    let mut img = RgbaImage::new(32, 32);
    for y in 2..30 {
        for x in 2..30 {
            let c = if x == 2 || y == 2 || x == 29 || y == 29 {
                BLACK
            } else if (20..26).contains(&x) && (6..10).contains(&y) {
                hue
            } else if y < 18 {
                GREY_L
            } else {
                GREY_D
            };
            img.set(x as i32, y as i32, Rgba::opaque(c.0, c.1, c.2));
        }
    }
    img
}

#[test]
fn derive_keeps_dominant_grey_grey_with_small_saturated_part() {
    for hue in [(0xb8u8, 0x68u8, 0x48u8), (0x98, 0x98, 0x00)] {
        let d = Data::build(vec![casing(hue)], 2);
        for mode in [Mode::Real, Mode::Optimised] {
            for k in [3usize, 4] {
                let o = Opts { k, step: 8, layers: Some(1), mode, name: "t".into(), cells: false };
                let r = derive::derive_from_data(&d, &o);
                for src in [GREY_L, GREY_D] {
                    let key = r.template.map.iter().find(|m| m.from == rgb(src)).unwrap().key;
                    let c = r.template.color_of(key).unwrap();
                    assert!(chroma(c) < 0.04, "grey {:?} became {} ({:?} k={})", src, color::to_hex(c), mode, k);
                }
                // any saturated template colour is an exact source colour
                for key in r.template.layers[0].keys.iter().filter_map(|k| k.color) {
                    if chroma(key) > 0.04 {
                        assert_eq!(key, rgb(hue), "blended hue {}", color::to_hex(key));
                    }
                }
            }
        }
    }
}

/// Mostly grey smoke with a small orange flame: the grey must stay grey.
#[test]
fn derive_grey_smoke_with_orange_flame_stays_grey() {
    let flame = (0xf0u8, 0x80u8, 0x10u8);
    let mut img = RgbaImage::new(32, 32);
    for y in 2..30 {
        for x in 2..30 {
            let c = if x == 2 || y == 2 || x == 29 || y == 29 {
                BLACK
            } else if (12..17).contains(&x) && (22..28).contains(&y) {
                flame
            } else if (x + y) % 5 < 2 {
                GREY_D
            } else {
                GREY_L
            };
            img.set(x as i32, y as i32, Rgba::opaque(c.0, c.1, c.2));
        }
    }
    let d = Data::build(vec![img], 2);
    for mode in [Mode::Real, Mode::Optimised] {
        for k in [3usize, 4] {
            let o = Opts { k, step: 8, layers: Some(1), mode, name: "t".into(), cells: false };
            let r = derive::derive_from_data(&d, &o);
            for src in [GREY_L, GREY_D] {
                let key = r.template.map.iter().find(|m| m.from == rgb(src)).unwrap().key;
                let c = r.template.color_of(key).unwrap();
                assert!(chroma(c) < 0.04, "smoke grey became {} ({:?} k={})", color::to_hex(c), mode, k);
            }
        }
    }
}
