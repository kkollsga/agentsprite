//! `pxl pattern suggest`: period detection on synthetic checker / stripe / brick textures (clean and noisy),
//! region phase, tile blocks, and the saved snippet applied by `pxl pattern`.

use agentsprite::img::{Rgba, RgbaImage};
use agentsprite::png;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const COL: [[u8; 3]; 3] = [[0x18, 0x20, 0x78], [0x48, 0x50, 0xb8], [0xf0, 0xb0, 0x90]];

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("pxl_suggest_{}_{}", tag, std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("refs")).unwrap();
    std::fs::create_dir_all(d.join("templates")).unwrap();
    std::fs::write(
        d.join("templates/syn.toml"),
        "name = \"syn\"\n[layers.base]\n\".\" = { role = \"transparent\" }\n\"1\" = { role = \"outline\", color = \"#182078\" }\n\"2\" = { role = \"body\", color = \"#4850b8\" }\n\"3\" = { role = \"skin\", color = \"#f0b090\" }\n",
    )
    .unwrap();
    d
}

fn pxl(d: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pxl")).current_dir(d).args(args).output().unwrap()
}

fn json(o: &Output) -> Value {
    serde_json::from_slice(&o.stdout).unwrap_or_else(|e| panic!("bad json {}: {}\n{}", e, String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)))
}

/// Target texture: key index (0..3) at target pixel (x, y).
type Tex = fn(usize, usize) -> usize;

fn checker(x: usize, y: usize) -> usize {
    (x / 2 + y / 2) % 2
}
fn stripes(x: usize, _y: usize) -> usize {
    (x / 3) % 2 * 2
}
fn brick(x: usize, y: usize) -> usize {
    let (row, ry) = (y / 4, y % 4);
    if ry == 3 {
        return 0;
    }
    let off = if row % 2 == 0 { 0 } else { 4 };
    if (x + off) % 8 == 0 {
        0
    } else {
        1
    }
}

fn rng(s: &mut u32) -> u32 {
    *s = s.wrapping_mul(1664525).wrapping_add(1013904223);
    *s >> 16
}

/// Reference image: the texture upscaled by `f`, optionally with colour jitter and 3% wrong-class pixels.
fn write_ref(path: &Path, w: usize, h: usize, f: usize, tex: Tex, noise: bool) {
    let mut im = RgbaImage::new(w * f, h * f);
    let mut s = 7u32;
    for y in 0..h * f {
        for x in 0..w * f {
            let mut k = tex(x / f, y / f);
            let mut c = COL[k];
            if noise {
                if rng(&mut s) % 100 < 3 {
                    k = (k + 1) % 3;
                    c = COL[k];
                }
                for ch in c.iter_mut() {
                    *ch = (*ch as i32 + (rng(&mut s) % 9) as i32 - 4).clamp(0, 255) as u8;
                }
            }
            im.px[y * w * f + x] = Rgba::opaque(c[0], c[1], c[2]);
        }
    }
    png::write_rgba(path, &im).unwrap();
}

fn setup_sprite(d: &Path, w: usize, h: usize, f: usize, tex: Tex, noise: bool) {
    write_ref(&d.join("refs/t.png"), w, h, f, tex, noise);
    let blank = vec!["." .repeat(w); h].join("\n");
    std::fs::write(
        d.join("p.pxl"),
        format!(
            "name = 'p'\nsize = [{w}, {h}]\norigin = [0, 0]\ntemplate = 'templates/syn.toml'\n[input]\nimages = 'refs/{{frame}}.png'\nfactor = {f}\n[reduce]\nfactor = {f}\n[[frames]]\nname = 't'\ngrid = '''\n{blank}\n'''\n"
        ),
    )
    .unwrap();
}

fn unit_of(v: &Value) -> Vec<String> {
    v["unit"].as_array().unwrap().iter().map(|r| r.as_str().unwrap().to_string()).collect()
}

fn expect_unit(tex: Tex, tw: usize, th: usize) -> Vec<String> {
    (0..th).map(|y| (0..tw).map(|x| (b'1' + tex(x, y) as u8) as char).collect()).collect()
}

#[test]
fn checker_stripe_and_brick_periods_clean_and_noisy() {
    for (name, tex, period) in [("checker", checker as Tex, (4, 4)), ("stripes", stripes as Tex, (6, 1)), ("brick", brick as Tex, (8, 8))] {
        for noise in [false, true] {
            let d = dir(&format!("{}{}", name, noise));
            setup_sprite(&d, 32, 24, 2, tex, noise);
            let o = pxl(&d, &["--json", "pattern", "suggest", "p", "t", "--region", "0,0,32,24"]);
            assert!(o.status.success(), "{} {}: {}", name, noise, String::from_utf8_lossy(&o.stderr));
            let v = json(&o);
            assert_eq!(v["period"], serde_json::json!([period.0, period.1]), "{} noise={}: {}", name, noise, v);
            assert_eq!(v["factor"], 2);
            assert_eq!(v["native_period"], serde_json::json!([period.0 * 2, period.1 * 2]));
            assert!(v["confidence"].as_f64().unwrap() > 0.7, "{} noise={} confidence {}", name, noise, v["confidence"]);
            assert_eq!(unit_of(&v), expect_unit(tex, period.0, period.1), "{} noise={}", name, noise);
            assert!(v["command"].as_str().unwrap().contains("pattern_NAME"));
        }
    }
}

#[test]
fn region_phase_is_aligned_and_snippet_reproduces_the_texture() {
    let d = dir("phase");
    setup_sprite(&d, 32, 24, 2, brick, false);
    // a region that starts mid-brick: the saved unit must still be the grid-aligned one
    let o = pxl(&d, &["--json", "pattern", "suggest", "p", "t", "--region", "3,1,24,20", "--save", "brick"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let v = json(&o);
    assert_eq!(v["period"], serde_json::json!([8, 8]));
    assert_eq!(v["phase"], serde_json::json!([3, 1]));
    assert_eq!(unit_of(&v), expect_unit(brick, 8, 8));
    assert!(d.join("templates/snippets/pattern_brick.txt").is_file());
    // applying it to a blank frame reproduces the reference exactly
    let o = pxl(&d, &["pattern", "p", "t", "pattern_brick", "--region", "0,0,32,24", "--no-ref"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let o = pxl(&d, &["--json", "show", "p", "t"]);
    assert!(o.status.success());
    let part = agentsprite::pxl::load(&d.join("p.pxl")).unwrap();
    let g = &part.frames[0].grid;
    for y in 0..24 {
        for x in 0..32 {
            assert_eq!(g.get(x, y), b'1' + brick(x as usize, y as usize) as u8, "at {},{}", x, y);
        }
    }
}

#[test]
fn flat_and_too_small_areas_say_so() {
    let d = dir("flat");
    setup_sprite(&d, 16, 16, 2, |_, _| 1, false);
    let v = json(&pxl(&d, &["--json", "pattern", "suggest", "p", "t"]));
    assert!(v["period"].is_null());
    assert!(v["reason"].as_str().unwrap().contains("flat"));
    let d = dir("small");
    setup_sprite(&d, 16, 16, 2, checker, false);
    let v = json(&pxl(&d, &["--json", "pattern", "suggest", "p", "t", "--region", "0,0,3,3"]));
    assert!(v["period"].is_null() || v["confidence"].as_f64().unwrap() < 0.6);
}

#[test]
fn tile_sets_read_tiles_as_wrapping_blocks() {
    let d = dir("tiles");
    // 4 tiles in a 32x8 sheet; reference at factor 2 (64x16). tiles 1 and 2 carry a 2x2 checker, tile 0 a stripe
    let tex: Tex = |x, y| if x / 8 == 0 { (x / 2) % 2 * 2 } else { (x / 2 + y / 2) % 2 };
    write_ref(&d.join("refs/blocks.png"), 32, 8, 2, tex, true);
    let blank = vec!["a".repeat(32); 8].join("\n");
    std::fs::write(
        d.join("templates/tiles.toml"),
        "name = \"tiles\"\n[layers.base]\n\"a\" = { role = \"air\", color = \"#88c8f8\" }\n\"1\" = { role = \"outline\", color = \"#182078\" }\n\"2\" = { role = \"body\", color = \"#4850b8\" }\n\"3\" = { role = \"skin\", color = \"#f0b090\" }\n",
    )
    .unwrap();
    std::fs::write(
        d.join("ts.pxl"),
        format!("name = 'ts'\nkind = 'tileset'\nsize = [32, 8]\ntile = [8, 8]\ntemplate = 'templates/tiles.toml'\n[input]\nimages = 'refs/{{frame}}.png'\nfactor = 2\n[reduce]\nfactor = 2\n[[frames]]\nname = 'blocks'\ngrid = '''\n{blank}\n'''\n"),
    )
    .unwrap();
    let v = json(&pxl(&d, &["--json", "pattern", "suggest", "ts", "blocks", "--tiles", "1-2", "--save", "chk"]));
    assert_eq!(v["period"], serde_json::json!([4, 4]), "{}", v);
    assert_eq!(unit_of(&v), expect_unit(|x, y| (x / 2 + y / 2) % 2, 4, 4));
    assert!(v["command"].as_str().unwrap().contains("--tile 1"));
    let v = json(&pxl(&d, &["--json", "pattern", "suggest", "ts", "blocks", "--tile", "0"]));
    assert_eq!(v["period"], serde_json::json!([4, 1]), "{}", v);
}
