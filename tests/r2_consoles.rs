//! M6 R2: the generalised hardware model and the gb, nes, sms, gg and md consoles. Per console: single-image mode (image, lint,
//! export with the console-native palette file) and project mode (new, ingest, check), `console show`, and the facts that make
//! the console different (colour model, palette lines, attribute blocks, variable sprite sizes, per-line pixels).
mod e2e_common;

use agentsprite::color::{parse_hex, to_hex};
use agentsprite::hwexport;
use agentsprite::lint::{self, LintOpts};
use agentsprite::model::{Frame, Grid, Part};
use agentsprite::{console, png, profile, sprites, template};
use e2e_common::*;
use std::path::Path;

const CONSOLES: [&str; 8] = ["gb", "nes", "sms", "gg", "md", "snes", "pce", "gba"];

fn pic(path: &Path, seed: u64) {
    png::write_rgba(path, &upscale(&sprite_grid(16, 16, seed), 4)).unwrap();
}

fn ok(o: &std::process::Output) -> String {
    assert!(o.status.success(), "{}{}", stdout(o), stderr(o));
    stdout(o)
}

fn prof(id: &str, mode: &str) -> profile::Profile {
    console::resolve(id, mode, None).unwrap()
}

#[test]
fn every_console_has_facts_and_references() {
    for id in CONSOLES.iter().chain(["gbc"].iter()) {
        let c = console::info(id, None).unwrap();
        assert!(!c.name.is_empty() && !c.colour.is_empty() && !c.palettes.is_empty() && !c.oam.is_empty() && !c.vram.is_empty() && !c.preview_bg.is_empty(), "{}", id);
        assert!(c.references.len() >= 2, "{} cites its references", id);
        for m in console::MODES {
            console::resolve(id, m, None).unwrap_or_else(|e| panic!("{}/{}: {}", id, m, e.msg));
        }
    }
    let d = tmpdir("r2_show");
    for id in CONSOLES {
        let t = ok(&pxl_in(&d, &["console", "show", id]));
        assert!(t.contains("reference:") && t.contains("[sprite]") && t.contains("[tile]") && t.contains("[screen]") && t.contains("colour_bits"), "{}: {}", id, t);
    }
    let l = ok(&pxl_in(&d, &["console", "list"]));
    for id in CONSOLES {
        assert!(l.contains(&format!("  {} ", id)), "{}", l);
    }
    // the screen mode takes the console's screen
    assert_eq!(prof("nes", "screen").size, Some((256, 240)));
    assert_eq!(prof("md", "screen").size, Some((320, 224)));
    assert_eq!(prof("sms", "screen").size, Some((256, 192)));
    assert_eq!(prof("gg", "screen").size, Some((160, 144)));
    assert_eq!(prof("snes", "screen").size, Some((256, 224)));
    assert_eq!(prof("pce", "screen").size, Some((256, 224)));
    assert_eq!(prof("gba", "screen").size, Some((240, 160)));
}

#[test]
fn the_facts_table() {
    let p = |i: &str| prof(i, "sprite");
    // (console, bits per channel, colours per palette, OBJ palettes, sprites on screen, per line, cell)
    for (id, bits, colors, objp, max, per, cell) in [("gb", 0, 3, 2, 40, 10, (8, 16)), ("nes", 0, 3, 4, 64, 8, (8, 16)), ("sms", 2, 15, 1, 64, 8, (8, 16)), ("gg", 4, 15, 1, 64, 8, (8, 16)), ("md", 3, 15, 4, 80, 20, (8, 8))] {
        let x = p(id);
        assert_eq!((x.colour_bits, x.colors, x.obj_palettes, x.hw_cells_max, x.hw_cells_per_row_max, x.hw_cell), (bits, colors, objp, max, per, cell), "{}", id);
    }
    // the R3 consoles: (colour bits, colours per palette, OBJ palettes, sprites, per line, pixels per line)
    for (id, bits, colors, objp, max, per, px) in [("snes", 5, 15, 8, 128, 32, 272), ("pce", 3, 15, 16, 64, 16, 256), ("gba", 5, 15, 16, 128, 128, 1210)] {
        let x = p(id);
        assert_eq!((x.colour_bits, x.colors, x.obj_palettes, x.hw_cells_max, x.hw_cells_per_row_max, x.hw_pixels_per_line_max), (bits, colors, objp, max, per, px), "{}", id);
        assert_eq!(x.sprite_size_mode, "variable", "{}", id);
    }
    assert_eq!(p("snes").sprite_sizes, vec![(16, 16), (32, 32)], "one OBSEL pair");
    assert_eq!(p("pce").sprite_sizes.len(), 6, "16/32 wide x 16/32/64 high");
    assert!(p("pce").sprite_sizes.contains(&(32, 64)) && !p("pce").sprite_sizes.contains(&(8, 8)));
    assert_eq!(p("gba").sprite_sizes.len(), 12, "the 12 OBJ shapes");
    for sz in [(8, 8), (64, 64), (16, 8), (8, 32), (32, 64), (64, 32)] {
        assert!(p("gba").sprite_sizes.contains(&sz), "{:?}", sz);
    }
    assert_eq!((p("snes").channel_step, p("gba").channel_step, p("pce").channel_step), (8, 8, 32));
    assert!(!prof("pce", "tile").dedupe_flips && prof("snes", "tile").dedupe_flips && prof("gba", "tile").dedupe_flips);
    assert_eq!(p("md").hw_pixels_per_line_max, 320);
    assert_eq!(p("md").sprite_size_mode, "variable");
    assert_eq!(p("md").sprite_sizes.len(), 16, "1-4 x 1-4 tiles");
    assert!(p("md").sprite_sizes.contains(&(32, 32)) && p("md").sprite_sizes.contains(&(8, 32)));
    assert_eq!((p("md").channel_step, p("sms").channel_step, p("gg").channel_step), (32, 64, 16));
    assert_eq!(p("gb").fixed_colours.len(), 4);
    assert_eq!(p("nes").fixed_colours.len(), 55);
    assert_eq!(prof("nes", "tile").attr_block, (16, 16));
    assert_eq!(prof("nes", "tile").palettes, 4);
    assert!(!prof("nes", "tile").dedupe_flips && prof("md", "tile").dedupe_flips && prof("sms", "tile").dedupe_flips);
    assert_eq!((prof("md", "tile").colors_per_tile, prof("sms", "tile").colors_per_tile, prof("nes", "tile").colors_per_tile), (16, 16, 4));
    // GBC keeps the old numbers
    let g = p("gbc");
    assert_eq!((g.colour_bits, g.channel_step, g.sprite_size_mode.as_str(), g.hw_pixels_per_line_max, g.export_palette.as_str(), g.attr_block), (5, 8, "fixed", 0, "", (0, 0)));
}

#[test]
fn colour_models_snap_and_judge() {
    // bits per channel
    let md = prof("md", "sprite");
    assert!(md.legal(parse_hex("#e0a020").unwrap()) && !md.legal(parse_hex("#f0b090").unwrap()));
    assert_eq!(to_hex(md.snap(parse_hex("#f0b090").unwrap())), "#e0c0a0");
    let sms = prof("sms", "sprite");
    assert!(sms.legal(parse_hex("#c08000").unwrap()) && !sms.legal(parse_hex("#c08010").unwrap()));
    assert!(prof("gg", "sprite").legal(parse_hex("#f0a010").unwrap()));
    // fixed lists
    let gb = prof("gb", "sprite");
    assert_eq!(to_hex(gb.snap(parse_hex("#181818").unwrap())), "#000000");
    assert_eq!(to_hex(gb.snap(parse_hex("#b0b0b0").unwrap())), "#aaaaaa");
    assert!(!gb.legal(parse_hex("#123456").unwrap()));
    let nes = prof("nes", "sprite");
    assert!(nes.legal(parse_hex("#fcfcfc").unwrap()) && !nes.legal(parse_hex("#fdfdfd").unwrap()));
    assert!(nes.legal(nes.snap(parse_hex("#123456").unwrap())));
    // GBC is the old RGB555 step
    assert_eq!(to_hex(prof("gbc", "sprite").snap(parse_hex("#f0b093").unwrap())), "#f0b090");
}

#[test]
fn native_palette_encodings() {
    let md = prof("md", "sprite");
    assert_eq!(hwexport::native("md", parse_hex("#e0e0e0").unwrap(), &md), Some(0x0EEE));
    assert_eq!(hwexport::native("md", parse_hex("#e00000").unwrap(), &md), Some(0x000E));
    assert_eq!(hwexport::native("md", parse_hex("#0000e0").unwrap(), &md), Some(0x0E00));
    assert_eq!(hwexport::native("sms", parse_hex("#c0c000").unwrap(), &prof("sms", "sprite")), Some(0x0F));
    assert_eq!(hwexport::native("sms", parse_hex("#0000c0").unwrap(), &prof("sms", "sprite")), Some(0x30));
    assert_eq!(hwexport::native("gg", parse_hex("#f0a010").unwrap(), &prof("gg", "sprite")), Some(0x01AF));
    let nes = prof("nes", "sprite");
    assert_eq!(hwexport::native("nes", parse_hex("#fcfcfc").unwrap(), &nes), Some(0x30));
    assert_eq!(hwexport::native("nes", parse_hex("#000000").unwrap(), &nes), Some(0x0F));
    assert_eq!(hwexport::native("nes", parse_hex("#7c7c7c").unwrap(), &nes), Some(0x00));
    assert_eq!(hwexport::native("gb", parse_hex("#555555").unwrap(), &prof("gb", "sprite")), Some(2));
    // BGR555 on the SNES and the GBA, the GRB word of the PC Engine
    assert_eq!(hwexport::native("snes", parse_hex("#f80000").unwrap(), &prof("snes", "sprite")), Some(0x001F));
    assert_eq!(hwexport::native("snes", parse_hex("#0000f8").unwrap(), &prof("snes", "sprite")), Some(0x7C00));
    assert_eq!(hwexport::native("gba", parse_hex("#00f800").unwrap(), &prof("gba", "sprite")), Some(0x03E0));
    assert_eq!(hwexport::native("pce", parse_hex("#e00000").unwrap(), &prof("pce", "sprite")), Some(0x38));
    assert_eq!(hwexport::native("pce", parse_hex("#00e000").unwrap(), &prof("pce", "sprite")), Some(0x1C0));
    assert_eq!(hwexport::native("pce", parse_hex("#0000e0").unwrap(), &prof("pce", "sprite")), Some(0x07));
    assert_eq!(hwexport::nes_unique().len(), 55);
}

#[test]
fn single_image_mode_per_console() {
    for id in CONSOLES {
        let d = tmpdir(&format!("r2_img_{}", id));
        pic(&d.join("blob.png"), 3);
        let t = ok(&pxl_in(&d, &["image", "blob.png", "--console", id, "--size", "16x16"]));
        assert!(t.contains(&format!("({} / sprite)", id)), "{}: {}", id, t);
        let l = pxl_in(&d, &["lint", "blob.pxl"]);
        assert!(l.status.success(), "{} lint: {}{}", id, stdout(&l), stderr(&l));
        assert!(stdout(&l).contains(&format!("{}/sprite", id)));
        ok(&pxl_in(&d, &["export", "blob.pxl"]));
        let e = d.join("blob.pxl/export");
        for f in ["blob.png", "blob.aseprite", "blob.palette.json"] {
            assert!(e.join(f).is_file(), "{}: {}", id, f);
        }
        let prof = prof(id, "sprite");
        let j: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(e.join("blob.palette.json")).unwrap()).unwrap();
        assert_eq!(j["console"], id);
        assert_eq!(j["entries_per_line"].as_u64().unwrap() as u32, prof.colors + 1, "palette entries = palettes x (colours + 1)");
        assert_eq!(j["entries"].as_u64().unwrap() as u32, j["lines"].as_array().unwrap().len() as u32 * (prof.colors + 1));
        let line = &j["lines"][0]["entries"];
        assert_eq!(line.as_array().unwrap().len() as u32, prof.colors + 1);
        assert_eq!(line[0]["transparent"], true, "index 0 is transparent");
        for e in line.as_array().unwrap().iter().skip(1) {
            if let Some(h) = e["rgb"].as_str() {
                assert!(prof.legal(parse_hex(h).unwrap()), "{}: {} is not a colour of the console", id, h);
                assert!(e["native"].is_u64(), "{}: native value", id);
            }
        }
        // the indexed PNG holds the same colours: transparent index 0, then the layer colours
        let (_, _, _, pal, _) = png::read_indexed(&e.join("blob.png")).unwrap();
        if prof.auto_template != "set" {
            assert!(pal.len() >= 2 && pal.len() <= (prof.colors + 1) as usize, "{}: {} palette entries", id, pal.len());
        } else {
            assert_eq!(pal.len(), 32, "{}: a palette-set export is the 8 slots x 4 entries image", id);
        }
        assert_eq!(prof.sprite_size_mode == "variable", e.join("blob.sprites.json").is_file(), "only variable-size consoles write a sprite cover");
        // idempotent: a second export writes nothing
        assert!(ok(&pxl_in(&d, &["export", "blob.pxl"])).contains("0 written"), "{}", id);
    }
}

#[test]
fn project_mode_per_console() {
    for id in CONSOLES {
        let d = tmpdir(&format!("r2_prj_{}", id));
        ok(&pxl_in(&d, &["new", "p", "--console", id]));
        let p = d.join("p");
        pic(&p.join("inbox/blob.png"), 3);
        pic(&p.join("inbox/orb.png"), 5);
        let t = ok(&pxl_in(&p, &["ingest", "--size", "16x16"]));
        assert!(t.contains("ingested 2 of 2"), "{}: {}", id, t);
        let o = pxl_in(&p, &["lint", "blob", "orb"]);
        assert!(o.status.success(), "{} lint: {}{}", id, stdout(&o), stderr(&o));
        ok(&pxl_in(&p, &["build"]));
        assert!(p.join("export/blob/blob.png").is_file() && p.join("export/blob/blob.palette.json").is_file(), "{}", id);
        let c = ok(&pxl_in(&p, &["check"]));
        assert!(c.contains("PASS") && c.contains("checked 2 of 2 part(s)"), "{}: {}", id, c);
        assert!(ok(&pxl_in(&p, &["console", "show"])).contains(&format!("({})", id)));
        // a store rebuilds the export and the palette file
        ok(&pxl_in(&p, &["set", "blob", "blob", "3,3=1"]));
        assert!(text_of(&p.join("export/blob/blob.palette.json")).contains("\"entries_per_line\""));
    }
}

fn text_of(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap()
}

#[test]
fn the_unlegal_colour_is_named_with_the_console_palette() {
    let d = tmpdir("r2_illegal");
    std::fs::create_dir_all(d.join("templates")).unwrap();
    std::fs::write(d.join("pxl.toml"), "[project]\nconsole = \"nes\"\n").unwrap();
    std::fs::write(d.join("templates/t.toml"), TEMPLATE).unwrap();
    let mut part = Part::new("p", (8, 8));
    part.template = Some("templates/t.toml".into());
    part.path = d.join("p.pxl");
    part.frames.push(Frame { name: "f".into(), src: None, locked: false, grid: sprite_grid(8, 8, 1) });
    std::fs::write(d.join("p.pxl"), agentsprite::pxl::to_string(&part)).unwrap();
    let o = pxl_in(&d, &["lint", "p.pxl", "--rules", "rgb_step"]);
    assert!(!o.status.success() && stdout(&o).contains("not in the nes palette of the console (55 colours)"), "{}", stdout(&o));
    std::fs::write(d.join("pxl.toml"), "[project]\nconsole = \"md\"\n").unwrap();
    let o = pxl_in(&d, &["lint", "p.pxl", "--rules", "rgb_step"]);
    assert!(stdout(&o).contains("not multiples of 32") || stdout(&o).contains("multiples of 32"), "{}", stdout(&o));
}

fn tpl_layers(n: usize) -> String {
    let mut s = String::from("name = \"l\"\n");
    for i in 0..n {
        s.push_str(&format!("[layers.l{}]\n{}\"{}\" = {{ role = \"c{}\", color = \"#{:02x}{:02x}{:02x}\" }}\n", i, if i == 0 { "\".\" = { role = \"transparent\" }\n" } else { "" }, i + 1, i, (i * 32) % 256, 64, 128));
    }
    s
}

#[test]
fn palette_lines_limit_the_colour_layers() {
    let d = tmpdir("r2_lines");
    std::fs::create_dir_all(d.join("templates")).unwrap();
    std::fs::write(d.join("templates/l.toml"), tpl_layers(5)).unwrap();
    let mut part = Part::new("p", (8, 8));
    part.template = Some("templates/l.toml".into());
    part.path = d.join("p.pxl");
    let mut g = Grid::new(8, 8, b'.');
    for (i, k) in b"12345".iter().enumerate() {
        g.set(i as i32, 0, *k);
    }
    part.frames.push(Frame { name: "f".into(), src: None, locked: false, grid: g });
    let tpl = template::load_for(&part).unwrap();
    let run = |id: &str| lint::lint_part(&part, &tpl, &prof(id, "sprite"), &LintOpts { rules: vec!["palette_lines".into()], ..Default::default() });
    assert!(run("gbc").is_empty(), "8 OBJ palettes hold 5 layers");
    let v = run("md");
    assert_eq!(v.len(), 1, "{:?}", v);
    assert!(v[0].message.contains("5 colour layers") && v[0].message.contains("console has 4"), "{}", v[0].message);
    assert_eq!(run("nes").len(), 1);
    assert_eq!(run("sms").len(), 1, "one sprite palette");
}

#[test]
fn variable_sprite_sizes_are_covered_by_a_placement_search() {
    let sizes = prof("md", "sprite").sprite_sizes.clone();
    let solid = |w: usize, h: usize| {
        let mut g = Grid::new(w, h, b'.');
        for y in 0..h {
            for x in 0..w {
                g.set(x as i32, y as i32, b'1');
            }
        }
        g
    };
    let covers = |g: &Grid, p: &[sprites::Placed]| {
        (0..g.h as i32).all(|y| (0..g.w as i32).all(|x| g.get(x, y) == b'.' || p.iter().any(|s| x >= s.x && x < s.x + s.w as i32 && y >= s.y && y < s.y + s.h as i32)))
    };
    for (w, h, max) in [(8, 8, 1), (32, 32, 1), (16, 32, 1), (64, 32, 2), (40, 40, 4), (24, 8, 1)] {
        let g = solid(w, h);
        let p = sprites::place(&g, &sizes);
        assert!(covers(&g, &p), "{}x{} is covered", w, h);
        assert!(p.len() <= max, "{}x{} needs at most {} sprites, got {}: {:?}", w, h, max, p.len(), p);
        assert!(p.iter().all(|s| sizes.contains(&(s.w as u32, s.h as u32))), "only allowed sizes");
    }
    // an art grid that is not on the 8 px grid still needs no more sprites: the cover may start anywhere
    let mut g = Grid::new(40, 24, b'.');
    for y in 3..19 {
        for x in 5..37 {
            g.set(x, y, b'1');
        }
    }
    assert_eq!(sprites::place(&g, &sizes).len(), 1, "a 32x16 block at (5,3) is one sprite");
    // line loads
    let p = sprites::place(&solid(64, 32), &sizes);
    let l = sprites::line_loads(&p, 32);
    assert!(l.iter().all(|x| *x == (2, 64)), "{:?}", l);
}

fn md_part(w: usize, h: usize, put: impl Fn(&mut Grid)) -> (Part, template::Template) {
    let d = tmpdir("r2_md");
    std::fs::create_dir_all(d.join("templates")).unwrap();
    std::fs::write(d.join("templates/t.toml"), "name = \"t\"\n[layers.base]\n\".\" = { role = \"transparent\" }\n\"1\" = { role = \"c\", color = \"#e0a020\" }\n").unwrap();
    let mut part = Part::new("p", (w, h));
    part.template = Some("templates/t.toml".into());
    part.path = d.join("p.pxl");
    let mut g = Grid::new(w, h, b'.');
    put(&mut g);
    part.frames.push(Frame { name: "f".into(), src: None, locked: false, grid: g });
    let tpl = template::load_for(&part).unwrap();
    (part, tpl)
}

#[test]
fn md_limits_sprites_per_line_and_pixels_per_line() {
    let hw = |part: &Part, tpl: &template::Template| lint::lint_part(part, tpl, &prof("md", "sprite"), &LintOpts { rules: vec!["hw_limit".into()], ..Default::default() });
    // 20 sprites on a line: allowed; 21: an error
    let dots = |n: usize| {
        md_part(n * 40, 8, |g| {
            for i in 0..n {
                for y in 0..8 {
                    for x in 0..8 {
                        g.set((i * 40 + x) as i32, y, b'1');
                    }
                }
            }
        })
    };
    let (p, t) = dots(20);
    assert!(hw(&p, &t).is_empty(), "{:?}", hw(&p, &t));
    let (p, t) = dots(21);
    let v = hw(&p, &t);
    assert_eq!(v.len(), 1, "{:?}", v);
    assert!(v[0].message.contains("21 sprites on the scanline") && v[0].message.contains("at most 20"), "{}", v[0].message);
    // 10 wide sprites are only 10 sprites, but 10 x 32 = 320 pixels fit and 11 x 32 = 352 do not
    let wide = |n: usize| {
        md_part(n * 40, 32, |g| {
            for i in 0..n {
                for y in 0..32 {
                    for x in 0..32 {
                        g.set((i * 40 + x) as i32, y, b'1');
                    }
                }
            }
        })
    };
    let (p, t) = wide(10);
    assert!(hw(&p, &t).is_empty(), "{:?}", hw(&p, &t));
    let (p, t) = wide(11);
    let v = hw(&p, &t);
    assert_eq!(v.len(), 1, "{:?}", v);
    assert!(v[0].message.contains("352 sprite pixels") && v[0].message.contains("at most 320"), "{}", v[0].message);
    // more than 80 sprites in a frame
    let (p, t) = md_part(40 * 9, 40 * 10, |g| {
        for j in 0..10 {
            for i in 0..9 {
                for y in 0..8 {
                    for x in 0..8 {
                        g.set((i * 40 + x) as i32, (j * 40 + y) as i32, b'1');
                    }
                }
            }
        }
    });
    let v = hw(&p, &t);
    assert!(v.iter().any(|i| i.message.contains("needs 90 sprites") && i.message.contains("at most 80 on screen")), "{:?}", v);
}

const NES_TPL: &str = "name = \"t\"\n[layers.base]\n\"a\" = { role = \"air\", color = \"#3cbcfc\" }\n\"b\" = { role = \"c_b\", color = \"#fcfcfc\" }\n\"c\" = { role = \"c_c\", color = \"#000000\" }\n\"d\" = { role = \"c_d\", color = \"#58d854\" }\n\"e\" = { role = \"c_e\", color = \"#f83800\" }\n\"f\" = { role = \"c_f\", color = \"#f8b800\" }\n[tiles]\nair = [\"a\"]\n";

fn tile_frame(rows: [&str; 2]) -> String {
    // a 16x16 frame of 2x2 tiles; each tile row string is 16 wide for rows 0-7 and 8-15
    let mut s = String::new();
    for r in rows {
        for _ in 0..8 {
            s.push_str(r);
            s.push('\n');
        }
    }
    s
}

#[test]
fn nes_attribute_blocks_share_one_palette() {
    let d = tmpdir("r2_attr");
    std::fs::create_dir_all(d.join("templates")).unwrap();
    std::fs::write(d.join("templates/t.toml"), NES_TPL).unwrap();
    std::fs::write(d.join("pxl.toml"), "[project]\nconsole = \"nes\"\n").unwrap();
    let part = |name: &str, grid: &str| format!("name = \"{}\"\nmode = \"tile\"\nsize = [16, 16]\norigin = [0, 0]\ntile = [8, 8]\ntemplate = \"templates/t.toml\"\n\n[[frames]]\nname = \"blocks\"\ngrid = \"\"\"\n{}\"\"\"\n", name, grid);
    // block 1: tiles use {a,b,c} and {a,d,e} = 5 colours in one 16x16 area: too many for one NES palette
    std::fs::write(d.join("bad.pxl"), part("bad", &tile_frame(["aaaabbbbaaaaddee", "aaccccccaaaaaaaa"]))).unwrap();
    // the same colours spread over two blocks (32 px wide) pass
    let ok_grid = format!("{}{}", "", {
        let mut s = String::new();
        for _ in 0..8 {
            s.push_str("aabbccddaabbccdd\n");
        }
        for _ in 0..8 {
            s.push_str("aabbccddaabbccdd\n");
        }
        s
    });
    std::fs::write(d.join("ok.pxl"), part("ok", &ok_grid)).unwrap();
    let o = pxl_in(&d, &["lint", "bad.pxl", "--rules", "attr_block"]);
    assert!(!o.status.success(), "{}", stdout(&o));
    assert!(stdout(&o).contains("attribute block") && stdout(&o).contains("16x16") && stdout(&o).contains("holds 4"), "{}", stdout(&o));
    // 'ok' uses 4 colours (a,b,c,d) in one block: fine
    let o = pxl_in(&d, &["lint", "ok.pxl", "--rules", "attr_block"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    // the Game Boy Color has no such rule
    std::fs::write(d.join("pxl.toml"), "[project]\nconsole = \"gbc\"\n").unwrap();
    let o = pxl_in(&d, &["lint", "bad.pxl", "--rules", "attr_block"]);
    assert!(o.status.success(), "{}", stdout(&o));
}

#[test]
fn tile_palettes_use_the_colours_per_tile_of_the_console() {
    // 16 colours per tile on the Mega Drive: a tile with 9 colours is fine, on the NES (4) it is not
    let d = tmpdir("r2_tilecol");
    std::fs::create_dir_all(d.join("templates")).unwrap();
    let mut tpl = String::from("name = \"t\"\n[layers.base]\n");
    for (i, k) in "abcdefghij".chars().enumerate() {
        tpl.push_str(&format!("\"{}\" = {{ role = \"c_{}\", color = \"#{:02x}2040\" }}\n", k, k, (i * 24) % 256));
    }
    tpl.push_str("[tiles]\nair = [\"a\"]\n");
    std::fs::write(d.join("templates/t.toml"), tpl).unwrap();
    let grid = "abcdefghabcdefgh\n".repeat(8) + &"aaaaaaaaaaaaaaaa\n".repeat(8);
    std::fs::write(d.join("t.pxl"), format!("name = \"t\"\nmode = \"tile\"\nsize = [16, 16]\norigin = [0, 0]\ntile = [8, 8]\ntemplate = \"templates/t.toml\"\n\n[[frames]]\nname = \"blocks\"\ngrid = \"\"\"\n{}\"\"\"\n", grid)).unwrap();
    let run = |c: &str| {
        std::fs::write(d.join("pxl.toml"), format!("[project]\nconsole = \"{}\"\n", c)).unwrap();
        pxl_in(&d, &["lint", "t.pxl", "--rules", "tile_colors,tile_palettes"])
    };
    assert!(run("md").status.success(), "{}", stdout(&run("md")));
    let o = run("nes");
    assert!(!o.status.success() && stdout(&o).contains("max 4"), "{}", stdout(&o));
    let o = run("gbc");
    assert!(!o.status.success() && stdout(&o).contains("max 4"));
}

#[test]
fn derived_templates_of_a_fixed_palette_console_use_only_its_colours() {
    for id in ["gb", "nes"] {
        let d = tmpdir(&format!("r2_fixed_{}", id));
        pic(&d.join("blob.png"), 4);
        ok(&pxl_in(&d, &["image", "blob.png", "--console", id, "--size", "16x16"]));
        let prof = prof(id, "sprite");
        let tpl = std::fs::read_to_string(d.join("blob.pxl/template.toml")).unwrap();
        let mut n = 0;
        for l in tpl.lines().filter(|l| l.contains("role =") && l.contains("color = \"#")) {
            let i = l.find("color = \"#").unwrap() + 9;
            assert!(prof.legal(parse_hex(&l[i..i + 7]).unwrap()), "{}: {}", id, l);
            n += 1;
        }
        assert!(n >= 2, "{}: {}", id, tpl);
    }
}

#[test]
fn gbc_stays_byte_identical() {
    // the Game Boy Color export has no palette file and the same configuration hash inputs
    let d = tmpdir("r2_gbc");
    pic(&d.join("blob.png"), 3);
    ok(&pxl_in(&d, &["image", "blob.png", "--size", "16x16"]));
    ok(&pxl_in(&d, &["export", "blob.pxl"]));
    let mut names: Vec<String> = std::fs::read_dir(d.join("blob.pxl/export")).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names, ["blob.aseprite", "blob.cells.json", "blob.png"], "the palette-set files of the Game Boy Color; no palette.json, no sprites.json");
}

// ---------------------------------------------------------------------------------------- R3: snes, pce, gba

fn blocks_part(n: usize, bw: usize, spacing: usize, bh: usize) -> (Part, template::Template) {
    md_part(n * spacing, bh, |g| {
        for i in 0..n {
            for y in 0..bh {
                for x in 0..bw {
                    g.set((i * spacing + x) as i32, y as i32, b'1');
                }
            }
        }
    })
}

fn hw_errors(part: &Part, tpl: &template::Template, p: &profile::Profile) -> Vec<String> {
    lint::lint_part(part, tpl, p, &LintOpts { rules: vec!["hw_limit".into()], ..Default::default() }).into_iter().map(|i| i.message).collect()
}

#[test]
fn snes_two_global_sizes_32_sprites_and_272_pixels_per_line() {
    // the default pair is 16x16 / 32x32: an 8x8 dot costs a 16x16 sprite, so 17 per line (272 px = 34 slivers) is the limit
    let snes = prof("snes", "sprite");
    let (p, t) = blocks_part(17, 8, 40, 8);
    assert!(hw_errors(&p, &t, &snes).is_empty(), "{:?}", hw_errors(&p, &t, &snes));
    let (p, t) = blocks_part(18, 8, 40, 8);
    let e = hw_errors(&p, &t, &snes);
    assert!(e.len() == 1 && e[0].contains("288 sprite pixels") && e[0].contains("at most 272"), "{:?}", e);
    // another OBSEL pair (8x8 / 16x16) as a project override: now the sprite count binds at 32
    let d = tmpdir("r3_snes_pair");
    std::fs::create_dir_all(d.join("consoles/snes")).unwrap();
    std::fs::write(d.join("consoles/snes/sprite.toml"), "sprite_sizes = [[8, 8], [16, 16]]\n").unwrap();
    let small = console::resolve("snes", "sprite", Some(&d)).unwrap();
    assert_eq!(small.sprite_sizes, vec![(8, 8), (16, 16)]);
    let (p, t) = blocks_part(32, 8, 40, 8);
    assert!(hw_errors(&p, &t, &small).is_empty(), "32 sprites of 8 px = 256 px fit");
    let (p, t) = blocks_part(33, 8, 40, 8);
    let e = hw_errors(&p, &t, &small);
    assert!(e.iter().any(|m| m.contains("33 sprites on the scanline") && m.contains("at most 32")), "{:?}", e);
    // 128 sprites on screen
    let (p, t) = md_part(40 * 12, 40 * 11, |g| {
        for j in 0..11 {
            for i in 0..12 {
                for y in 0..8 {
                    for x in 0..8 {
                        g.set((i * 40 + x) as i32, (j * 40 + y) as i32, b'1');
                    }
                }
            }
        }
    });
    assert!(hw_errors(&p, &t, &small).iter().any(|m| m.contains("needs 132 sprites") && m.contains("at most 128")), "{:?}", hw_errors(&p, &t, &small));
}

#[test]
fn pce_sixteen_sprites_and_256_pixels_per_line() {
    let pce = prof("pce", "sprite");
    let (p, t) = blocks_part(16, 16, 40, 16);
    assert!(hw_errors(&p, &t, &pce).is_empty(), "{:?}", hw_errors(&p, &t, &pce));
    let (p, t) = blocks_part(17, 16, 40, 16);
    let e = hw_errors(&p, &t, &pce);
    assert!(!e.is_empty() && e.iter().any(|m| m.contains("at most 16") || m.contains("at most 256")), "{:?}", e);
    // a tiny dot still takes the smallest sprite, 16x16
    let (p, t) = blocks_part(16, 4, 40, 4);
    assert!(hw_errors(&p, &t, &pce).is_empty());
    let (p, t) = blocks_part(17, 4, 40, 4);
    assert!(!hw_errors(&p, &t, &pce).is_empty());
    // sizes: 16 or 32 wide, 16, 32 or 64 high: a 32x64 figure is one sprite
    let solid = |w: usize, h: usize| {
        let mut g = Grid::new(w, h, b'.');
        for y in 0..h {
            for x in 0..w {
                g.set(x as i32, y as i32, b'1');
            }
        }
        g
    };
    assert_eq!(sprites::place(&solid(32, 64), &pce.sprite_sizes).len(), 1);
    assert_eq!(sprites::place(&solid(48, 16), &pce.sprite_sizes).len(), 2, "32 + 16 wide");
}

#[test]
fn gba_cycle_budget_is_a_pixel_limit_per_scanline() {
    let gba = prof("gba", "sprite");
    // 64x64 OBJs: 18 x 64 = 1152 cycles fit the 1210 budget, 19 x 64 = 1216 do not
    let (p, t) = blocks_part(18, 64, 72, 64);
    assert!(hw_errors(&p, &t, &gba).is_empty(), "{:?}", hw_errors(&p, &t, &gba));
    let (p, t) = blocks_part(19, 64, 72, 64);
    let e = hw_errors(&p, &t, &gba);
    assert!(e.iter().any(|m| m.contains("1216 sprite pixels") && m.contains("at most 1210")), "{:?}", e);
    // the 12 shapes: a 64x32 and an 8x32 are single OBJs
    let solid = |w: usize, h: usize| {
        let mut g = Grid::new(w, h, b'.');
        for y in 0..h {
            for x in 0..w {
                g.set(x as i32, y as i32, b'1');
            }
        }
        g
    };
    for (w, h) in [(64, 32), (8, 32), (32, 8), (16, 32), (32, 64), (64, 64)] {
        assert_eq!(sprites::place(&solid(w, h), &gba.sprite_sizes).len(), 1, "{}x{}", w, h);
    }
    // 8bpp: one palette of 255 as a project override
    let d = tmpdir("r3_gba8");
    std::fs::create_dir_all(d.join("consoles/gba")).unwrap();
    std::fs::write(d.join("consoles/gba/sprite.toml"), "colors = 255\nobj_palettes = 1\n").unwrap();
    let eight = console::resolve("gba", "sprite", Some(&d)).unwrap();
    assert_eq!((eight.colors, eight.obj_palettes), (255, 1));
}
