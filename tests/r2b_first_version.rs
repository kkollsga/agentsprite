//! M6 R2b: the first version of `image` / `ingest` is hardware-valid by construction (lint 0 errors), or says "needs hand work" and
//! leaves a task.
mod e2e_common;

use agentsprite::model::Frame;
use agentsprite::color::{parse_hex, Rgb};
use agentsprite::img::{Rgba, RgbaImage};
use agentsprite::{console, png};
use e2e_common::*;
use std::path::Path;

const CONSOLES: [&str; 9] = ["gbc", "gb", "nes", "sms", "gg", "md", "snes", "pce", "gba"];

fn ok(o: &std::process::Output) -> String {
    assert!(o.status.success(), "{}{}", stdout(o), stderr(o));
    stdout(o)
}

/// A 48x32 bug: red body, dark outline, white eye with a pupil, grey wheels, blue stripe.
fn bug(w: usize, h: usize) -> RgbaImage {
    let mut im = RgbaImage::new(w, h);
    let (cx, cy, rx, ry) = (w as f64 / 2.0, h as f64 / 2.0, w as f64 / 2.0 - 2.0, h as f64 / 2.0 - 3.0);
    for y in 0..h {
        for x in 0..w {
            let d = ((x as f64 - cx) / rx).powi(2) + ((y as f64 - cy) / ry).powi(2);
            let (fx, fy) = (x as f64, y as f64);
            let p = if (fx - w as f64 * 0.25).powi(2) + (fy - h as f64 * 0.8).powi(2) <= 16.0 || (fx - w as f64 * 0.75).powi(2) + (fy - h as f64 * 0.8).powi(2) <= 16.0 {
                Some((90, 90, 100))
            } else if d <= 1.0 {
                if d > 0.85 {
                    Some((40, 10, 10))
                } else if (fx - w as f64 * 0.7).powi(2) + (fy - h as f64 * 0.4).powi(2) <= 9.0 {
                    Some((255, 255, 255))
                } else if (fx - w as f64 * 0.72).powi(2) + (fy - h as f64 * 0.4).powi(2) <= 2.0 {
                    Some((0, 0, 0))
                } else if (h as f64 * 0.45..h as f64 * 0.53).contains(&fy) {
                    Some((30, 60, 200))
                } else {
                    Some((255, 0, 0))
                }
            } else {
                None
            };
            if let Some((r, g, b)) = p {
                im.px[y * w + x] = Rgba::opaque(r, g, b);
            }
        }
    }
    im
}

/// A blue face with light periwinkle highlights, white eyes with pupils: the light tone of the face is periwinkle, not white.
fn face(w: usize, h: usize) -> RgbaImage {
    let mut im = RgbaImage::new(w, h);
    let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f64, y as f64);
            let d = (((fx - cx) / (w as f64 / 2.0 - 2.0)).powi(2) + ((fy - cy) / (h as f64 / 2.0 - 2.0)).powi(2)).sqrt();
            if d > 1.0 {
                continue;
            }
            let eye = |ex: f64| (fx - ex).powi(2) + (fy - cy * 0.8).powi(2);
            let c = if d > 0.92 {
                (20, 20, 80)
            } else if eye(w as f64 * 0.35) <= 16.0 || eye(w as f64 * 0.65) <= 16.0 {
                if eye(w as f64 * 0.37) <= 3.0 || eye(w as f64 * 0.67) <= 3.0 { (0, 0, 0) } else { (255, 255, 255) }
            } else if fy < cy * 0.5 {
                (146, 146, 255)
            } else if fy < cy * 1.0 {
                (109, 109, 219)
            } else if fy < cy * 1.5 {
                (73, 73, 182)
            } else {
                (36, 36, 146)
            };
            im.px[y * w + x] = Rgba::opaque(c.0, c.1, c.2);
        }
    }
    im
}

fn item(n: usize) -> RgbaImage {
    let mut im = RgbaImage::new(n, n);
    let c = n as f64 / 2.0;
    for y in 0..n {
        for x in 0..n {
            let d = (((x as f64 - c).powi(2)) + ((y as f64 - c).powi(2))).sqrt();
            if d < c - 1.0 {
                im.px[y * n + x] = if d > c / 2.0 { Rgba::opaque(255, 200, 0) } else { Rgba::opaque(255, 255, 255) };
            }
        }
    }
    im
}

fn character(n: usize) -> RgbaImage {
    let mut im = RgbaImage::new(n, n);
    for y in n / 16..n - n / 16 {
        for x in n * 5 / 16..n * 11 / 16 {
            let c = if y < n / 3 { (255, 200, 160) } else if y < n * 5 / 8 { (0, 80, 255) } else if y < n * 13 / 16 { (200, 0, 0) } else { (60, 40, 20) };
            im.px[y * n + x] = Rgba::opaque(c.0, c.1, c.2);
        }
    }
    im
}

fn sheet() -> RgbaImage {
    let mut im = RgbaImage::new(128, 32);
    for f in 0..4usize {
        im.blit(&item(24), (f * 32 + 4) as i32, 4);
        for y in 10..14 {
            for x in 0..24 {
                im.px[(y + 4) * 128 + f * 32 + 4 + x] = Rgba::opaque((40 * f) as u8 + 20, 120, 200);
            }
        }
    }
    im
}

fn tmp(tag: &str, id: &str) -> std::path::PathBuf {
    tmpdir(&format!("r2b_{}_{}", tag, id))
}

fn errors(d: &Path, target: &str) -> (bool, String) {
    let o = pxl_in(d, &["lint", target]);
    (o.status.success(), format!("{}{}", stdout(&o), stderr(&o)))
}

#[test]
fn the_first_version_of_a_single_image_lints_clean_on_every_console() {
    for id in CONSOLES {
        for (name, img, size) in [("moto", bug(48, 32), "24x16"), ("item", item(16), "8x8"), ("char", character(64), "32x32"), ("bug64", bug(64, 40), "32x20")] {
            let d = tmp(name, id);
            png::write_rgba(&d.join(format!("{}.png", name)), &img).unwrap();
            let t = ok(&pxl_in(&d, &["image", &format!("{}.png", name), "--console", id, "--size", size]));
            let target = format!("{}.pxl", name);
            let (clean, text) = errors(&d, &target);
            assert!(clean, "{} / {}: the first version has lint errors:\n{}\n{}", id, name, text, t);
            assert!(!t.contains("needs hand work"), "{} / {}: {}", id, name, t);
        }
    }
}

#[test]
fn a_sheet_split_into_frames_is_valid_too() {
    for id in CONSOLES {
        let p = tmp("sheet", id);
        ok(&pxl_in(&p, &["new", "p", "--console", id]));
        let p = p.join("p");
        png::write_rgba(&p.join("inbox/walk.png"), &sheet()).unwrap();
        let t = ok(&pxl_in(&p, &["ingest", "--grid", "32x32", "--size", "16x16"]));
        assert!(t.contains("created"), "{}: {}", id, t);
        let part = agentsprite::pxl::load(&p.join("assets/walk/asset.pxl")).unwrap();
        assert_eq!(part.frames.len(), 4);
        let (clean, text) = errors(&p, "walk");
        assert!(clean, "{}: {}", id, text);
        assert!(ok(&pxl_in(&p, &["check"])).contains("PASS"), "{}", id);
    }
}

#[test]
fn colours_go_through_the_console_colour_model() {
    // #ff0000 is not an RGB555 colour: the derive left it as it was
    let d = tmp("red", "gbc");
    png::write_rgba(&d.join("moto.png"), &bug(48, 32)).unwrap();
    ok(&pxl_in(&d, &["image", "moto.png", "--console", "gbc", "--size", "24x16"]));
    let prof = console::resolve("gbc", "sprite", None).unwrap();
    for f in ["moto.pxl/template.toml", "moto.pxl/palettes/moto.toml"] {
        let t = std::fs::read_to_string(d.join(f)).unwrap();
        for l in t.lines().filter(|l| !l.contains(" = \"") || l.contains("role =") || l.contains("colors =")) {
            for (i, _) in l.match_indices('#') {
                if let Ok(c) = parse_hex(&l[i..(i + 7).min(l.len())]) {
                    assert!(prof.legal(c), "{}: {}", f, l);
                }
            }
        }
    }
    for id in ["nes", "gb", "md", "sms", "snes", "pce", "gba"] {
        let d = tmp("legal", id);
        png::write_rgba(&d.join("moto.png"), &bug(48, 32)).unwrap();
        ok(&pxl_in(&d, &["image", "moto.png", "--console", id, "--size", "24x16"]));
        let prof = console::resolve(id, "sprite", None).unwrap();
        let mut texts = vec![std::fs::read_to_string(d.join("moto.pxl/template.toml")).unwrap()];
        if let Ok(rd) = std::fs::read_dir(d.join("moto.pxl/palettes")) {
            for e in rd.flatten() {
                texts.push(std::fs::read_to_string(e.path()).unwrap());
            }
        }
        for t in texts {
            for l in t.lines().filter(|l| l.contains("role =") || l.contains("colors =")) {
                for (i, _) in l.match_indices('#') {
                    if let Ok(c) = parse_hex(&l[i..(i + 7).min(l.len())]) {
                        assert!(prof.legal(c), "{}: {}", id, l);
                    }
                }
            }
        }
    }
}

#[test]
fn per_cell_palettes_for_gbc_and_nes_one_line_for_the_rest() {
    for id in ["gbc", "nes", "gb"] {
        let d = tmp("set", id);
        png::write_rgba(&d.join("moto.png"), &bug(48, 32)).unwrap();
        ok(&pxl_in(&d, &["image", "moto.png", "--console", id, "--size", "24x16"]));
        let a = std::fs::read_to_string(d.join("moto.pxl/asset.pxl")).unwrap();
        assert!(a.contains("palette_set = \"moto\""), "{}: {}", id, a);
        let set = std::fs::read_to_string(d.join("moto.pxl/palettes/moto.toml")).unwrap();
        assert!(set.contains(&format!("console = \"{}\"", id)) && set.contains("slot = 0"), "{}", set);
        let slots = set.matches("[[slots]]").count();
        assert!(slots >= 1 && slots <= console::resolve(id, "sprite", None).unwrap().obj_palettes as usize, "{} slots", slots);
        // the eye white is a colour of the set
        assert!(set.contains("#fcfcfc") || set.contains("#f8f8f8") || set.contains("#ffffff"), "{}: the white eye is kept in a palette:\n{}", id, set);
    }
    for id in ["sms", "gg", "md", "snes", "pce", "gba"] {
        let d = tmp("flat", id);
        png::write_rgba(&d.join("moto.png"), &bug(48, 32)).unwrap();
        ok(&pxl_in(&d, &["image", "moto.png", "--console", id, "--size", "24x16"]));
        let a = std::fs::read_to_string(d.join("moto.pxl/asset.pxl")).unwrap();
        assert!(!a.contains("palette_set"), "{}", id);
        let t = std::fs::read_to_string(d.join("moto.pxl/template.toml")).unwrap();
        assert_eq!(t.matches("[layers.").count(), 1, "{}: one layer, no overlay", id);
    }
}

#[test]
fn an_input_the_hardware_cannot_hold_says_needs_hand_work_and_leaves_a_task() {
    // 160 px wide on the NES: 20 sprite columns against 8 per scanline
    let d = tmp("wide", "nes");
    let mut im = RgbaImage::new(160, 32);
    for y in 4..28 {
        for x in 2..158 {
            im.px[y * 160 + x] = Rgba::opaque(200, (x * 3 % 255) as u8, (y * 9 % 255) as u8);
        }
    }
    png::write_rgba(&d.join("wide.png"), &im).unwrap();
    let o = pxl_in(&d, &["image", "wide.png", "--console", "nes", "--size", "80x16"]);
    let t = ok(&o);
    assert!(t.contains("needs hand work: hw_limit"), "{}", t);
    assert!(stderr(&o).contains("needs hand work"));
    let tasks = ok(&pxl_in(&d, &["tasks", "--asset", "wide.pxl"]));
    assert!(tasks.contains("[fix]") && tasks.contains("first version needs hand work: hw_limit"), "{}", tasks);
    assert!(!errors(&d, "wide.pxl").0, "the asset exists with its best version");
    assert!(d.join("wide.pxl/versions/wide_v001.png").is_file());
    let j = json_of(&pxl_in(&d, &["image", "wide.png", "--console", "nes", "--size", "80x16", "--json"]));
    let _ = j;
}

#[test]
fn learning_slots_keeps_hue_families_and_eyes() {
    use agentsprite::autoset::{cell_data, learn_slots, metric};
    // a part drawn with keys: R red, r dark red, B blue, G grey, g dark grey, W white, K black
    let d = tmpdir("r2c_learn");
    let mut part = agentsprite::model::Part::new("p", (32, 16));
    part.path = d.join("p.pxl");
    let mut g = agentsprite::model::Grid::new(32, 16, b'.');
    for y in 0..16 {
        for x in 0..8 {
            g.set(x, y, if (x + y) % 5 == 0 { b'r' } else { b'R' });
        }
        for x in 8..16 {
            g.set(x, y, if (x + y) % 4 == 0 { b'K' } else { b'B' });
        }
        for x in 16..24 {
            g.set(x, y, if (x + y) % 3 == 0 { b'g' } else { b'G' });
        }
    }
    // an eye in the red cell: white with a black pupil
    for (x, y) in [(2, 5), (3, 5), (4, 5), (2, 6), (4, 6), (2, 7), (3, 7), (4, 7)] {
        g.set(x, y, b'W');
    }
    g.set(3, 6, b'K');
    part.frames.push(Frame { name: "f".into(), src: None, locked: false, grid: g });
    let rgb = |k: u8| -> Option<Rgb> {
        Some(match k {
            b'R' => Rgb { r: 248, g: 16, b: 8 },
            b'r' => Rgb { r: 144, g: 0, b: 0 },
            b'B' => Rgb { r: 72, g: 72, b: 184 },
            b'G' => Rgb { r: 144, g: 144, b: 144 },
            b'g' => Rgb { r: 72, g: 72, b: 72 },
            b'W' => Rgb { r: 248, g: 248, b: 248 },
            b'K' => Rgb { r: 0, g: 0, b: 0 },
            _ => return None,
        })
    };
    let free = |base: Rgb, used: &[Rgb]| (1..9u8).map(|i| Rgb { r: base.r.wrapping_add(8 * i), ..base }).find(|x| !used.contains(x));
    let data = cell_data(&part, &rgb, (8, 16));
    assert!(data.eyes >= 1, "the eye is found");
    for slots in [3usize, 4, 8] {
        let packed = learn_slots(&data, slots, &free);
        assert!(!packed.is_empty() && packed.len() <= slots);
        for s in &packed {
            assert!(s[0] != s[1] && s[1] != s[2] && s[0] != s[2]);
        }
        // the eye cell holds white and a dark colour in one slot
        assert!(packed.iter().any(|s| s.contains(&rgb(b'W').unwrap()) && s.iter().any(|c| metric(*c, rgb(b'K').unwrap()) < 0.02)), "{} slots: {:?}", slots, packed);
        // the blue is never folded into a grey: some slot holds a blue
        assert!(packed.iter().any(|s| s.iter().any(|c| metric(*c, rgb(b'B').unwrap()) < 0.01)), "{} slots: {:?}", slots, packed);
        // and a red
        assert!(packed.iter().any(|s| s.iter().any(|c| metric(*c, rgb(b'R').unwrap()) < 0.01)), "{} slots: {:?}", slots, packed);
    }
    assert!(learn_slots(&data, 8, &free).len() >= 3, "red, blue and grey each get a palette when slots are free");
}

// ---------------------------------------------------------------------------------------- quality, not only lint

fn oklab_de(a: Rgb, b: Rgb) -> f64 {
    let (x, y) = (agentsprite::color::srgb_to_oklab(a), agentsprite::color::srgb_to_oklab(b));
    ((x.l - y.l).powi(2) + (x.a - y.a).powi(2) + (x.b - y.b).powi(2)).sqrt() * 100.0
}

fn chroma(c: Rgb) -> f64 {
    let o = agentsprite::color::srgb_to_oklab(c);
    (o.a * o.a + o.b * o.b).sqrt()
}

/// Hue clusters of an image: (share of the opaque pixels, mean colour, mean chroma), chromatic colours grouped by hue.
fn hue_clusters(im: &RgbaImage) -> Vec<(f64, Rgb, f64)> {
    let mut by: Vec<(f64, f64, f64, f64, f64)> = Vec::new(); // hue, n, r, g, b sums
    let mut total: f64 = 0.0;
    for p in im.px.iter().filter(|p| p.a > 0) {
        total += 1.0;
        let c = Rgb { r: p.r, g: p.g, b: p.b };
        if chroma(c) < 0.05 {
            continue;
        }
        let o = agentsprite::color::srgb_to_oklab(c);
        let h = o.b.atan2(o.a);
        match by.iter_mut().find(|e| {
            let d = (e.0 - h).abs();
            d.min(std::f64::consts::TAU - d) < 0.45
        }) {
            Some(e) => {
                e.1 += 1.0;
                e.2 += p.r as f64;
                e.3 += p.g as f64;
                e.4 += p.b as f64;
            }
            None => by.push((h, 1.0, p.r as f64, p.g as f64, p.b as f64)),
        }
    }
    by.into_iter()
        .map(|e| {
            let m = Rgb { r: (e.2 / e.1) as u8, g: (e.3 / e.1) as u8, b: (e.4 / e.1) as u8 };
            (e.1 / total.max(1.0), m, chroma(m))
        })
        .collect()
}

/// The colours the first version draws with (template keys used in a frame, through the template).
fn output_colours(asset: &Path) -> Vec<Rgb> {
    let part = agentsprite::pxl::load(&asset.join("asset.pxl")).unwrap();
    let tpl = agentsprite::template::load_for(&part).unwrap();
    let mut seen = [false; 256];
    for f in &part.frames {
        for &k in &f.grid.cells {
            seen[k as usize] = true;
        }
    }
    (0..=255u8).filter(|k| seen[*k as usize] && *k != b'.').filter_map(|k| agentsprite::lint::effective_color(&part, &tpl, k)).collect()
}

/// Eyes of a colour grid: connected near-white groups with a dark pixel within two pixels. (count, any group lacks a pupil)
fn eyes_of(w: usize, h: usize, white_l: f64, px: &dyn Fn(usize, usize) -> Option<Rgb>) -> usize {
    let white = |c: Rgb| agentsprite::color::srgb_to_oklab(c).l > white_l && chroma(c) < 0.06;
    let dark = |c: Rgb| agentsprite::color::srgb_to_oklab(c).l < 0.4;
    let mut seen = vec![false; w * h];
    let mut eyes = 0;
    for y in 0..h {
        for x in 0..w {
            if seen[y * w + x] || !px(x, y).map_or(false, white) {
                continue;
            }
            let (mut st, mut pupil) = (vec![(x, y)], false);
            seen[y * w + x] = true;
            while let Some((cx, cy)) = st.pop() {
                for dy in -2i32..=2 {
                    for dx in -2i32..=2 {
                        let (nx, ny) = (cx as i32 + dx, cy as i32 + dy);
                        if nx < 0 || ny < 0 || nx as usize >= w || ny as usize >= h {
                            continue;
                        }
                        let (nx, ny) = (nx as usize, ny as usize);
                        if let Some(c) = px(nx, ny) {
                            if dark(c) {
                                pupil = true;
                            }
                            if dx.abs() <= 1 && dy.abs() <= 1 && white(c) && !seen[ny * w + nx] {
                                seen[ny * w + nx] = true;
                                st.push((nx, ny));
                            }
                        }
                    }
                }
            }
            if pupil {
                eyes += 1;
            }
        }
    }
    eyes
}

#[test]
fn the_first_version_keeps_the_identity_colours_and_the_eye() {
    for id in ["gbc", "nes", "sms", "gg", "md", "snes", "pce", "gba"] {
        for (name, img, size) in [("moto", bug(48, 32), "24x16"), ("bug64", bug(64, 40), "32x20"), ("char", character(64), "32x32"), ("face", face(48, 48), "24x24")] {
            let d = tmp(&format!("q_{}", name), id);
            png::write_rgba(&d.join(format!("{}.png", name)), &img).unwrap();
            ok(&pxl_in(&d, &["image", &format!("{}.png", name), "--console", id, "--size", size]));
            let out = output_colours(&d.join(format!("{}.pxl", name)));
            let tol = match id {
                "sms" => 25.0,
                "nes" => 22.0,
                _ => 15.0,
            };
            for (share, mean, ch) in hue_clusters(&img) {
                if share > 0.03 || (ch > 0.12 && share > 0.005) {
                    let best = out.iter().map(|c| oklab_de(*c, mean)).fold(f64::MAX, f64::min);
                    assert!(best < tol, "{} / {}: the hue family {:?} ({:.1}% of the art) has no colour within dE {} (nearest {:.1})\n{:?}", id, name, mean, share * 100.0, tol, best, out);
                }
            }
            // white is for highlights: the first version has no more near-white than the input (plus a small margin)
            {
                let near_white = |c: Rgb| agentsprite::color::srgb_to_oklab(c).l > if id == "sms" { 0.75 } else { 0.9 } && chroma(c) < 0.06;
                let (mut op, mut wh) = (0usize, 0usize);
                for p in img.px.iter().filter(|p| p.a > 0) {
                    op += 1;
                    wh += usize::from(near_white(Rgb { r: p.r, g: p.g, b: p.b }));
                }
                let in_share = wh as f64 / op.max(1) as f64;
                let part = agentsprite::pxl::load(&d.join(format!("{}.pxl/asset.pxl", name))).unwrap();
                let tpl = agentsprite::template::load_for(&part).unwrap();
                let (mut oop, mut owh) = (0usize, 0usize);
                for f in &part.frames {
                    for &k in &f.grid.cells {
                        if k != b'.' {
                            oop += 1;
                            owh += usize::from(agentsprite::lint::effective_color(&part, &tpl, k).map_or(false, near_white));
                        }
                    }
                }
                let out_share = owh as f64 / oop.max(1) as f64;
                // (the Master System's brightest value is #c0c0c0, which has no clean "white" to count)
                assert!(id == "sms" || out_share <= in_share + 0.05, "{} / {}: {:.1}% of the first version is white, the input has {:.1}%", id, name, out_share * 100.0, in_share * 100.0);
            }
            // the eye: white with a pupil, in the input and in the first version
            let in_eyes = eyes_of(img.w, img.h, 0.85, &|x, y| Some(img.px[y * img.w + x]).filter(|p| p.a > 0).map(|p| Rgb { r: p.r, g: p.g, b: p.b }));
            if in_eyes > 0 {
                let part = agentsprite::pxl::load(&d.join(format!("{}.pxl/asset.pxl", name))).unwrap();
                let tpl = agentsprite::template::load_for(&part).unwrap();
                let g = &part.frames[0].grid;
                let cache: Vec<Option<Rgb>> = (0..=255u8).map(|k| agentsprite::lint::effective_color(&part, &tpl, k)).collect();
                let white_l = if id == "sms" { 0.75 } else { 0.85 }; // the Master System's brightest white is #c0c0c0
                let out_eyes = eyes_of(g.w, g.h, white_l, &|x, y| {
                    let k = g.cells[y * g.w + x];
                    if k == b'.' { None } else { cache[k as usize] }
                });
                assert!(out_eyes >= 1, "{} / {}: the eye (white with a pupil) is lost: {} in the input, {} in the first version", id, name, in_eyes, out_eyes);
                assert_eq!(out_eyes, in_eyes, "{} / {}: eye count", id, name);
            }
        }
    }
}

#[test]
fn a_neutral_grey_stays_a_neutral_grey_on_the_nes_and_gb() {
    for id in ["nes", "gb"] {
        let d = tmp("grey", id);
        // a body of grey metal (a slight blue cast, like pixel-art greys) with a red band and a dark outline
        let mut im = RgbaImage::new(48, 32);
        for y in 2..30 {
            for x in 2..46 {
                let edge = x < 4 || x > 43 || y < 4 || y > 27;
                im.px[y * 48 + x] = if edge { Rgba::opaque(30, 30, 34) } else if (12..18).contains(&y) { Rgba::opaque(220, 20, 20) } else if y < 12 { Rgba::opaque(150, 150, 158) } else { Rgba::opaque(100, 100, 108) };
            }
        }
        png::write_rgba(&d.join("moto.png"), &im).unwrap();
        ok(&pxl_in(&d, &["image", "moto.png", "--console", id, "--size", "24x16"]));
        let out = output_colours(&d.join("moto.pxl"));
        // the grey metal keeps a neutral grey of the console (never a tinted colour of similar lightness)
        let grey = out.iter().any(|c| chroma(*c) < 0.035 && (0.3..0.85).contains(&agentsprite::color::srgb_to_oklab(*c).l));
        assert!(grey, "{}: no neutral mid grey among {:?}", id, out);
    }
    // the console palette: every grey of the input maps to a grey, never to a tinted colour
    let nes = console::resolve("nes", "sprite", None).unwrap();
    for v in (16..=240).step_by(8) {
        let c = nes.snap(Rgb { r: v, g: v, b: v });
        assert!(chroma(c) < 0.035, "grey {} became {:?}", v, c);
    }
}

#[test]
fn a_single_image_has_no_category_only_the_hardware_limits() {
    let d = tmp("nocat", "gbc");
    png::write_rgba(&d.join("moto.png"), &bug(64, 40)).unwrap();
    ok(&pxl_in(&d, &["image", "moto.png", "--console", "gbc", "--size", "32x20"]));
    let a = std::fs::read_to_string(d.join("moto.pxl/asset.pxl")).unwrap();
    assert!(!a.contains("category"), "{}", a);
    assert!(a.contains("[budget]") && a.contains("per_line = 10") && a.contains("per_frame = 40"), "the hardware limits: {}", a);
}

#[test]
fn the_game_boy_uses_white_actively_with_both_object_palettes() {
    // a red body with a blue stripe, a white eye with a black pupil: the DMG output is stretched to black, greys and white
    let d = tmp("gbstretch", "gb");
    png::write_rgba(&d.join("moto.png"), &bug(48, 32)).unwrap();
    ok(&pxl_in(&d, &["image", "moto.png", "--console", "gb", "--size", "24x16"]));
    let set = std::fs::read_to_string(d.join("moto.pxl/palettes/moto.toml")).unwrap();
    assert!(set.contains("[\"#000000\", \"#555555\", \"#ffffff\"]") && set.contains("[\"#000000\", \"#aaaaaa\", \"#ffffff\"]"), "OBP0 and OBP1: {}", set);
    let part = agentsprite::pxl::load(&d.join("moto.pxl/asset.pxl")).unwrap();
    let tpl = agentsprite::template::load_for(&part).unwrap();
    let g = &part.frames[0].grid;
    let mut shades = std::collections::BTreeMap::new();
    for &k in &g.cells {
        if k != b'.' {
            *shades.entry(agentsprite::lint::effective_color(&part, &tpl, k).unwrap()).or_insert(0usize) += 1;
        }
    }
    let has = |c: Rgb| shades.contains_key(&c);
    assert!(has(parse_hex("#000000").unwrap()) && has(parse_hex("#ffffff").unwrap()), "black outline and white are both used: {:?}", shades);
    // the white eye has a black pupil next to it
    let (w, h) = (g.w, g.h);
    let col = |x: usize, y: usize| agentsprite::lint::effective_color(&part, &tpl, g.cells[y * w + x]);
    let white = parse_hex("#ffffff").unwrap();
    let black = parse_hex("#000000").unwrap();
    let eye = (0..h).any(|y| (0..w).any(|x| col(x, y) == Some(white) && [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)].iter().any(|(dx, dy)| { let (nx, ny) = (x as i32 + dx, y as i32 + dy); nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h && col(nx as usize, ny as usize) == Some(black) })));
    assert!(eye, "white next to a black pupil");
    let l = pxl_in(&d, &["lint", "moto.pxl"]);
    assert!(l.status.success(), "{}{}", stdout(&l), stderr(&l));
}

#[test]
fn console_styles_exist_for_every_console_and_a_project_style_overrides() {
    for id in CONSOLES {
        let s = console::style(id, None);
        assert!(!s.reason.is_empty(), "{} has a reason", id);
        assert!(s.min_value_spread > 0.0);
    }
    assert_eq!(console::style("gb", None).white, "active");
    assert_eq!(console::style("gbc", None).white, "highlights");
    // the project's [style] overrides a key
    let d = tmp("style", "gbc");
    ok(&pxl_in(&d, &["new", "p", "--console", "gb"]));
    let p = d.join("p");
    let t = std::fs::read_to_string(p.join("pxl.toml")).unwrap();
    std::fs::write(p.join("pxl.toml"), format!("{}\n[style]\nwhite = \"highlights\"\n", t)).unwrap();
    assert_eq!(console::style("gb", Some(&p)).white, "highlights");
    assert_eq!(console::style("gb", Some(&p)).value_map, "stretch", "other keys keep the console default");
    std::fs::write(p.join("pxl.toml"), format!("{}\n[style]\nwhite = \"loud\"\n", t)).unwrap();
    let o = pxl_in(&p, &["check"]);
    assert!(stderr(&o).contains("white must be"), "{}{}", stdout(&o), stderr(&o));
    let show = pxl_in(&p, &["console", "show", "gb", "--json"]);
    let _ = show;
}
