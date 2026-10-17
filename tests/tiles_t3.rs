//! M5 T3: shared tile budget (348 incl. animated slots), palette-independent tiles, reserved BG palette, priority mask,
//! cycle JSON, underwater palettes, HUD / card tile sets.
mod e2e_common;

use agentsprite::model::{Frame, Grid, Part, PartKind};
use agentsprite::tiles::{pattern, Colors, Sheet};
use agentsprite::{lint, png, profile, pxl, template};
use e2e_common::*;
use std::path::{Path, PathBuf};

const TPL: &str = r##"name = "tiles"

[layers.base]
"a" = { role = "air", color = "#88c8f8" }
"z" = { role = "air2", color = "#98d8f8" }
"b" = { role = "c_b", color = "#f8f8f8" }
"c" = { role = "c_c", color = "#183058" }
"d" = { role = "c_d", color = "#28a048" }
"e" = { role = "c_e", color = "#80e060" }
"f" = { role = "c_f", color = "#386818" }
"g" = { role = "c_g", color = "#a86830" }
"h" = { role = "c_h", color = "#704018" }
"i" = { role = "c_i", color = "#d8a060" }
"j" = { role = "c_j", color = "#808890" }
"k" = { role = "c_k", color = "#505860" }
"l" = { role = "c_l", color = "#c0c8d0" }
"m" = { role = "c_m", color = "#2060c8" }
"n" = { role = "c_n", color = "#60a0f8" }
"o" = { role = "c_o", color = "#a8d8f8" }
"p" = { role = "c_p", color = "#c88820" }
"q" = { role = "c_q", color = "#f8d848" }
"r" = { role = "c_r", color = "#f8a090" }
"s" = { role = "c_s", color = "#c83030" }
"t" = { role = "c_t", color = "#701818" }
"u" = { role = "c_u", color = "#303030" }
"v" = { role = "c_v", color = "#a0a0a0" }
"w" = { role = "c_w", color = "#e8e8e8" }
"x" = { role = "c_x", color = "#885088" }
"y" = { role = "c_y", color = "#d0a8d0" }

[tiles]
air = ["a"]
"##;

const PALS: [&str; 8] = ["abcd", "aefg", "ahij", "aklm", "anop", "aqrs", "atuv", "awxy"];

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
}

fn project(tag: &str, tpl: &str) -> PathBuf {
    let d = tmpdir(tag);
    std::fs::create_dir_all(d.join("templates")).unwrap();
    std::fs::write(d.join("templates/tiles.toml"), tpl).unwrap();
    d
}

fn write_set(dir: &Path, file: &str, name: &str, header: &Part, frames: Vec<(&str, Grid)>, extra: &str) -> PathBuf {
    let (w, h) = (frames[0].1.w, frames[0].1.h);
    let mut p = header.clone();
    p.name = name.into();
    p.size = (w, h);
    p.kind = PartKind::Tileset;
    p.tile = Some((8, 8));
    p.template = Some("templates/tiles.toml".into());
    p.frames.clear();
    for (n, g) in frames {
        p.frames.push(Frame { name: n.into(), src: None, locked: false, grid: g });
    }
    let path = dir.join(file);
    std::fs::write(&path, pxl::to_string(&p) + extra).unwrap();
    path
}

fn base() -> Part {
    Part::new("x", (8, 8))
}

fn put_tile(g: &mut Grid, i: usize, f: impl Fn(usize, usize) -> u8) {
    let per = g.w / 8;
    for y in 0..8 {
        for x in 0..8 {
            g.cells[((i / per) * 8 + y) * g.w + (i % per) * 8 + x] = f(x, y);
        }
    }
}

/// Role pattern of shape `s` (0 air, 1 dark, 2 mid, 3 light); every role occurs.
fn shape(s: u64) -> [u8; 64] {
    let mut r = Rng(s * 7919 + 13);
    let mut p = [0u8; 64];
    for v in p.iter_mut() {
        *v = (r.next() % 4) as u8;
    }
    p[0] = 0;
    p[1] = 1;
    p[2] = 2;
    p[3] = 3;
    p
}

fn lint_rule(p: &Path, rule: &str) -> Vec<lint::Issue> {
    let part = pxl::load(p).unwrap();
    let prof = profile::load_for(&part, None).unwrap();
    lint::lint_part_auto(&part, &prof, &lint::LintOpts { rules: vec![rule.into()], frames: vec![], refs: false, advice: false }).into_iter().filter(|i| i.rule == rule).collect()
}

// ------------------------------------------------------------------------------------------------ profile, budget

#[test]
fn builtin_profile_has_the_new_budget_and_an_override_restores_the_old_rules() {
    let t = profile::load("gbc/tile").unwrap();
    assert_eq!((t.unique_tiles_max, t.count_animated, t.pattern_unique, t.bg_reserved.clone()), (348, true, true, vec![7]));
    assert_eq!(profile::load("gbc-tile").unwrap().unique_tiles_max, 348, "the old name still means gbc/tile");
    assert!(profile::load("gbc-tile-legacy").is_err(), "the legacy profile is gone");
    // the old rules as a project override of consoles/gbc/tile.toml
    let d = tmpdir("t3_override");
    std::fs::create_dir_all(d.join("consoles/gbc")).unwrap();
    std::fs::write(d.join("consoles/gbc/tile.toml"), include_str!("fixtures/tiles/consoles/gbc/tile.toml")).unwrap();
    let l = agentsprite::console::resolve("gbc", "tile", Some(&d)).unwrap();
    assert_eq!((l.unique_tiles_max, l.count_animated, l.pattern_unique, l.bg_reserved.len()), (300, false, false, 0));
}

#[test]
fn level_tiles_plus_animated_slots_stay_within_348() {
    let d = project("t3slots", TPL);
    // 340 distinct tiles in blocks (all of palette 0), an animation set of 4 tiles per frame
    let mut g = Grid::new(128, 22 * 8, b'a');
    let mut r = Rng(5);
    for i in 0..340 {
        let mut seen = [0u8; 64];
        for v in seen.iter_mut() {
            *v = b"abcd"[(r.next() % 4) as usize];
        }
        seen[0..4].copy_from_slice(b"abcd");
        seen[4] = b"abcd"[(i % 4) as usize];
        seen[5] = b"abcd"[(i / 4 % 4) as usize];
        seen[6] = b"abcd"[(i / 16 % 4) as usize];
        seen[7] = b"abcd"[(i / 64 % 4) as usize];
        put_tile(&mut g, i, |x, y| seen[y * 8 + x]);
    }
    let p = write_set(&d, "z.pxl", "z", &base(), vec![("blocks", g.clone())], "");
    let st = json_of(&pxl_in(&d, &["tiles", "stats", "z.pxl", "--json"]));
    let uniq = st["unique"].as_u64().unwrap();
    assert!(uniq > 300 && uniq <= 341, "unique {}", uniq);
    assert!(lint_rule(&p, "unique_tiles").is_empty(), "340 level tiles fit 348 without animation");
    // an animated part of 12 tiles per frame -> 340 + 12 > 348
    let mut f0 = Grid::new(16, 8, b'a');
    let mut f1 = Grid::new(16, 8, b'a');
    for y in 0..8 {
        for x in 0..16 {
            f0.cells[y * 16 + x] = b"abcd"[(x + y) % 4];
            f1.cells[y * 16 + x] = b"abcd"[(x + 2 * y + 1) % 4];
        }
    }
    // 4 unique-tiles-per-frame animation: make 10 tiles per frame
    let mut a0 = Grid::new(128, 16, b'a');
    let mut a1 = Grid::new(128, 16, b'a');
    let mut rr = Rng(77);
    for i in 0..20 {
        let (t0, t1): (Vec<u8>, Vec<u8>) = (0..64).map(|_| (b"abcd"[(rr.next() % 4) as usize], b"abcd"[(rr.next() % 4) as usize])).unzip();
        put_tile(&mut a0, i, |x, y| t0[y * 8 + x]);
        put_tile(&mut a1, i, |x, y| t1[y * 8 + x]);
    }
    write_set(&d, "wf.pxl", "wf", &base(), vec![("wf_00", a0), ("wf_01", a1)], "");
    let text = std::fs::read_to_string(&p).unwrap();
    std::fs::write(&p, text.replace("size = ", "animated = [\"wf.pxl\"]\nsize = ")).unwrap();
    let v = lint_rule(&p, "unique_tiles");
    assert_eq!(v.len(), 1, "{:?}", v);
    assert!(v[0].message.contains("animated slots"), "{}", v[0].message);
    // the old rule: a project override of consoles/gbc/tile.toml ignores the slots
    std::fs::create_dir_all(d.join("consoles/gbc")).unwrap();
    std::fs::write(d.join("consoles/gbc/tile.toml"), include_str!("fixtures/tiles/consoles/gbc/tile.toml")).unwrap();
    assert!(lint_rule(&p, "unique_tiles").iter().all(|i| i.message.contains("max 300")));
}

// ------------------------------------------------------------------------------------------------ patterns

fn tile_set_of_shapes(n: u64) -> Grid {
    assert_eq!(n % 8, 0);
    // every shape in palette 0 (a b c d) and in palette 1 (a e f g) with roles dark/mid/light in the same positions
    let mut g = Grid::new(128, ((2 * n as usize + 15) / 16) * 8, b'a');
    for s in 0..n {
        let sh = shape(s);
        put_tile(&mut g, 2 * s as usize, |x, y| [b'a', b'c', b'd', b'b'][sh[y * 8 + x] as usize]);
        put_tile(&mut g, 2 * s as usize + 1, |x, y| [b'a', b'f', b'g', b'e'][sh[y * 8 + x] as usize]);
    }
    g
}

fn canon_roles(sh: &[u8; 64]) -> [u8; 64] {
    pattern::canon_pat(sh)
}

#[test]
fn same_shape_in_another_palette_is_one_tile() {
    let d = project("t3pat", TPL);
    let g = tile_set_of_shapes(16);
    let p = write_set(&d, "z.pxl", "z", &base(), vec![("blocks", g)], "");
    let want: std::collections::BTreeSet<[u8; 64]> = (0..16).map(|s| canon_roles(&shape(s))).collect();
    let st = json_of(&pxl_in(&d, &["tiles", "stats", "z.pxl", "--json"]));
    assert_eq!(st["unique_by_colours"], 32);
    assert_eq!(st["unique"].as_u64().unwrap() as usize, want.len(), "{}", st);
    // export: the PLTE order lets a checker see the same count
    let o = pxl_in(&d, &["export", "z.pxl"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let (w, h, idx, pal, _) = png::read_indexed(&d.join("z_export/blocks.png")).unwrap();
    // checker: first-fit palettes of the colour sets in sheet order, index = PLTE position inside the palette
    let tiles: Vec<Vec<u8>> = (0..w / 8 * (h / 8)).map(|t| (0..64).map(|i| idx[((t / (w / 8)) * 8 + i / 8) * w + (t % (w / 8)) * 8 + i % 8]).collect()).collect();
    let mut bins: Vec<std::collections::BTreeSet<u8>> = Vec::new();
    let mut bin_of = Vec::new();
    for t in &tiles {
        let set: std::collections::BTreeSet<u8> = t.iter().copied().collect();
        let at = bins.iter().position(|b| b.union(&set).count() <= 4);
        let b = match at {
            Some(b) => {
                bins[b].extend(set);
                b
            }
            None => {
                bins.push(set);
                bins.len() - 1
            }
        };
        bin_of.push(b);
    }
    let mut seen = std::collections::BTreeSet::new();
    for (t, b) in tiles.iter().zip(&bin_of) {
        let order: Vec<u8> = bins[*b].iter().copied().collect();
        let mut p = [0u8; 64];
        for i in 0..64 {
            p[i] = order.iter().position(|c| *c == t[i]).unwrap() as u8;
        }
        seen.insert(pattern::canon_pat(&p));
    }
    assert_eq!(seen.len(), want.len(), "a checker reading blocks.png counts {} (palettes {})", seen.len(), pal.len());
    // the lint count and the dedupe count use the same rule
    assert!(lint_rule(&p, "unique_tiles").is_empty());
    let part = pxl::load(&p).unwrap();
    let tpl = template::load_for(&part).unwrap();
    let cols = Colors::new(&part, &tpl);
    let air = agentsprite::tiles::air_codes(&tpl, &cols);
    assert_eq!(pattern::groups(&Sheet::from_grid(&part.frames[0].grid), &cols, &air, true).len(), want.len());
    let j = json_of(&pxl_in(&d, &["tiles", "dedupe", "z.pxl", "--max", "6", "--json"]));
    assert_eq!(j["unique_before"].as_u64().unwrap() as usize, want.len());
    assert!(j["unique_after"].as_u64().unwrap() <= 6);
}

// ------------------------------------------------------------------------------------------------ reserved palette

fn palette_sheet(n_pal: usize) -> Grid {
    let mut g = Grid::new(128, 8, b'a');
    for i in 0..n_pal {
        let keys: Vec<u8> = PALS[i].bytes().collect();
        put_tile(&mut g, i, |x, y| keys[(x + 2 * y) % 4]);
    }
    g
}

#[test]
fn reserved_palette_leaves_seven() {
    let d = project("t3res", TPL);
    let p8 = write_set(&d, "p8.pxl", "p8", &base(), vec![("blocks", palette_sheet(8))], "");
    let v = lint_rule(&p8, "tile_palettes");
    assert_eq!(v.len(), 1, "{:?}", v);
    assert!(v[0].message.contains("max 7") && v[0].message.contains("reserved"), "{}", v[0].message);
    let p7 = write_set(&d, "p7.pxl", "p7", &base(), vec![("blocks", palette_sheet(7))], "");
    assert!(lint_rule(&p7, "tile_palettes").is_empty());
    // fit packs into 7
    let o = pxl_in(&d, &["tiles", "fit", "p8.pxl", "--apply", "--agent", "A1"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    assert!(lint_rule(&p8, "tile_palettes").is_empty());
    let st = stdout(&pxl_in(&d, &["tiles", "stats", "p8.pxl"]));
    assert!(st.contains("max 7: BG palette 7 reserved"), "{}", st);
}

// ------------------------------------------------------------------------------------------------ priority

fn priority_part(d: &Path, name: &str, rows: impl Fn(usize, usize) -> u8, ids: &str) -> PathBuf {
    let mut g = Grid::new(128, 8, b'b');
    put_tile(&mut g, 0, &rows);
    put_tile(&mut g, 1, |x, y| b"bcd"[(x + y) % 3]);
    write_set(d, &format!("{}.pxl", name), name, &base(), vec![("blocks", g)], ids)
}

#[test]
fn priority_color0_fixtures() {
    let d = project("t3prio", &TPL.replace("air = [\"a\"]", "air = [\"a\", \"z\"]"));
    let one_air = |x: usize, y: usize| if y < 3 { b'a' } else { b"bcd"[(x + y) % 3] };
    let two_air = |x: usize, y: usize| if y < 2 { b'a' } else if y < 4 { b'z' } else { b"bc"[(x + y) % 2] };
    let no_air = |x: usize, y: usize| b"bcd"[(x + y) % 3];
    // positive: two different air colours in a priority tile; an id beyond the sheet; ...
    let pos1 = priority_part(&d, "pos1", two_air, "\n[priority]\ntiles = [0]\n");
    let pos2 = priority_part(&d, "pos2", one_air, "\n[priority]\ntiles = [99]\n");
    assert!(!lint_rule(&pos1, "priority_color0").is_empty());
    assert!(!lint_rule(&pos2, "priority_color0").is_empty());
    // ... and a template with no air key
    let d3 = project("t3prio3", &TPL.replace("[tiles]\nair = [\"a\"]\n", ""));
    let pos3 = priority_part(&d3, "pos3", one_air, "\n[priority]\ntiles = [0]\n");
    assert!(!lint_rule(&pos3, "priority_color0").is_empty());
    // negative
    let neg1 = priority_part(&d, "neg1", one_air, "\n[priority]\ntiles = [0]\n");
    let neg2 = priority_part(&d, "neg2", no_air, "\n[priority]\ntiles = [0]\n");
    let neg3 = priority_part(&d, "neg3", two_air, "");
    for n in [neg1, neg2, neg3] {
        assert!(lint_rule(&n, "priority_color0").is_empty(), "{}", n.display());
    }
    // the exported pattern order puts the air colour at index 0 of its palette
    let o = pxl_in(&d, &["export", "neg1.pxl"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
}

#[test]
fn priority_command_and_export() {
    let d = project("t3prio2", &TPL);
    let one_air = |x: usize, y: usize| if y < 3 { b'a' } else { b"bcd"[(x + y) % 3] };
    priority_part(&d, "pr", one_air, "");
    let j = json_of(&pxl_in(&d, &["tiles", "priority", "pr.pxl", "--set", "0,1", "--json", "--agent", "A1"]));
    assert_eq!(j["priority"], serde_json::json!([0, 1]));
    let j = json_of(&pxl_in(&d, &["tiles", "priority", "pr.pxl", "--clear", "1", "--json", "--agent", "A1"]));
    assert_eq!(j["priority"], serde_json::json!([0]));
    assert!(pxl::load(&d.join("pr.pxl")).unwrap().priority == Some(vec![0]));
    let o = pxl_in(&d, &["export", "pr.pxl"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let (w, h, idx, _, _) = png::read_indexed(&d.join("pr_export/priority.png")).unwrap();
    assert_eq!((w, h), (128, 8));
    assert!(idx[..8].iter().all(|v| *v == 1) && idx[8..16].iter().all(|v| *v == 0), "white = over sprites, tile 0 only");
    assert_eq!(pxl_in(&d, &["tiles", "priority", "pr.pxl", "--set", "999"]).status.code(), Some(1));
    // header round trip
    let part = pxl::load(&d.join("pr.pxl")).unwrap();
    let t = pxl::to_string(&part);
    assert_eq!(pxl::to_string(&pxl::parse_str(&t, &d.join("pr.pxl")).unwrap()), t);
}

// ------------------------------------------------------------------------------------------------ cycles, underwater, HUD

#[test]
fn cycles_export_json() {
    let d = project("t3cyc", TPL);
    // the cycle colours sit in the blocks as palette 0 (keys b c d + air)
    let g = palette_sheet(2);
    let extra = "\n[[cycle]]\nname = \"lamps\"\nbg_palette = 2\nframes_per_step = 6\ntargets = [1, 2, 3]\nsteps = [[\"#102030\", \"#304050\", \"#506070\"], [\"#304050\", \"#506070\", \"#102030\"], [\"#506070\", \"#102030\", \"#304050\"]]\n\n[[cycle]]\nname = \"belt\"\nbg_palette = 3\ntargets = [1, 2]\nsteps = [[\"#f8f8f8\", \"#183058\"], [\"#183058\", \"#f8f8f8\"], [\"#28a048\", \"#f8f8f8\"]]\n\n[[cycle]]\nname = \"four\"\nbg_palette = 1\ntargets = [0, 1, 2, 3]\nsteps = [[\"#102030\", \"#304050\", \"#506070\", \"#708090\"]]\n";
    write_set(&d, "c.pxl", "c", &base(), vec![("blocks", g)], extra);
    let o = pxl_in(&d, &["export", "c.pxl"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let out = d.join("c_export/palette_cycle");
    let lamps: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(out.join("lamps.json")).unwrap()).unwrap();
    assert_eq!(lamps["bg_palette"], 2);
    assert_eq!(lamps["frames_per_step"], 6);
    assert_eq!(lamps["steps"].as_array().unwrap().len(), 3);
    assert_eq!(lamps["steps"][0].as_object().unwrap().len(), 3);
    let belt: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(out.join("belt.json")).unwrap()).unwrap();
    assert_eq!(belt["bg_palette"], 3);
    assert_eq!(belt["frames_per_step"], 8);
    assert_eq!(belt["steps"][2]["1"], "#28a048");
    assert_eq!(belt["steps"].as_array().unwrap().len(), 3);
    assert!(out.join("four.json").is_file());
    // more than four colours in a step is refused
    let bad = std::fs::read_to_string(d.join("c.pxl")).unwrap().replace("targets = [1, 2]", "targets = [0, 1, 2, 3, 3]");
    std::fs::write(d.join("c.pxl"), bad).unwrap();
    assert!(!pxl_in(&d, &["export", "c.pxl"]).status.success());
}

#[test]
fn underwater_palettes_export_in_swap_order() {
    let d = project("t3uw", TPL);
    let extra = "\n[[underwater]]\nkind = \"obj\"\npalette = 1\ncolors = [\"#183058\", \"#2060c8\"]\norder = 2\n\n[[underwater]]\nkind = \"bg\"\npalette = 2\ncolors = [\"#102030\", \"#304050\", \"#506070\", \"#708090\"]\norder = 1\n";
    write_set(&d, "u.pxl", "u", &base(), vec![("blocks", palette_sheet(2))], extra);
    let o = pxl_in(&d, &["export", "u.pxl"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let j: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.join("u_export/palettes_underwater.json")).unwrap()).unwrap();
    let ps = j["palettes"].as_array().unwrap();
    assert_eq!(ps.len(), 2);
    assert_eq!((ps[0]["kind"].as_str(), ps[0]["palette"].as_u64(), ps[0]["colors"].as_array().unwrap().len()), (Some("bg"), Some(2), 4));
    assert_eq!(ps[1]["kind"], "obj");
}

#[test]
fn hud_and_card_tile_sets_export_on_palette_7_with_a_tile_cap() {
    let d = project("t3hud", TPL);
    let mut hud = Grid::new(128, 16, b'b');
    let mut r = Rng(11);
    for i in 0..20 {
        let t: Vec<u8> = (0..64).map(|_| b"bcd"[(r.next() % 3) as usize]).collect();
        put_tile(&mut hud, i, |x, y| t[y * 8 + x]);
    }
    let header = {
        let mut p = base();
        p.bg_palette = Some(7);
        p.max_tiles = Some(8);
        p
    };
    let p = write_set(&d, "hud.pxl", "hud", &header, vec![("hud_tiles", hud.clone())], "\n[reduce]\nfactor = 1\n");
    // over the cap of 8 unique tiles; one palette only
    let v = lint_rule(&p, "unique_tiles");
    assert_eq!(v.len(), 1, "{:?}", v);
    assert!(v[0].message.contains("max 8"), "{}", v[0].message);
    assert!(lint_rule(&p, "tile_palettes").is_empty(), "one palette (a part on BG palette 7 may use it)");
    // a part on one palette that needs two is an error
    let mut two = Grid::new(128, 8, b'b');
    put_tile(&mut two, 0, |x, y| b"bcd"[(x + y) % 3]);
    put_tile(&mut two, 1, |x, y| b"efg"[(x + y) % 3]);
    let p2 = write_set(&d, "two.pxl", "two", &header, vec![("cards_tiles", two)], "");
    assert_eq!(lint_rule(&p2, "tile_palettes").len(), 1);
    // the frame name is the output name: hud_tiles.png through the default pattern
    std::fs::write(d.join("pxl.toml"), "[parts.hud]\npath = \"hud.pxl\"\nout = \"ui/hud\"\n").unwrap();
    let mut ok = Grid::new(128, 8, b'b');
    for i in 0..8 {
        let t: Vec<u8> = (0..64).map(|_| b"bcd"[(r.next() % 3) as usize]).collect();
        put_tile(&mut ok, i, |x, y| t[y * 8 + x]);
    }
    write_set(&d, "hud.pxl", "hud", &header, vec![("hud_tiles", ok)], "");
    let o = pxl_in(&d, &["build"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    assert!(d.join("ui/hud/hud_tiles.png").is_file());
    let (w, _, _, _, _) = png::read_indexed(&d.join("ui/hud/hud_tiles.png")).unwrap();
    assert_eq!(w, 128);
}
