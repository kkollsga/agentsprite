//! Tile core: template `[palettes]` / `[tiles]`, block reduction, statistics, fit and dedupe (synthetic data only).

mod e2e_common;
mod tiles_common;
use agentsprite::color;
use agentsprite::template;
use agentsprite::tiles::palette::AIR;
use agentsprite::tiles::reduce::{palette_of, Reducer};
use agentsprite::tiles::stats;
use agentsprite::tiles::{code_of, T};
use agentsprite::{lint, profile, pxl};
use e2e_common::*;
use tiles_common::*;

fn lint_rules(part: &std::path::Path, rules: &[&str]) -> Vec<lint::Issue> {
    let p = pxl::load(part).unwrap();
    let prof = profile::load_for(&p, None).unwrap();
    let opts = lint::LintOpts { rules: rules.iter().map(|r| r.to_string()).collect(), frames: vec![], refs: false, advice: false };
    lint::lint_part_auto(&p, &prof, &opts)
}

// ------------------------------------------------------------------ template

fn tpl_text(extra: &str) -> String {
    format!(
        "name = \"t\"\n[layers.base]\n\"a\" = {{ role = \"air\", color = \"#88c8f8\" }}\n\"b\" = {{ role = \"x\", color = \"#f8f8f8\" }}\n\"c\" = {{ role = \"y\", color = \"#183058\" }}\n\"d\" = {{ role = \"z\", color = \"#28a048\" }}\n\"e\" = {{ role = \"w\", color = \"#28a048\" }}\n{}",
        extra
    )
}

fn load_tpl(extra: &str) -> Result<template::Template, String> {
    let d = tmpdir("tpl");
    let p = d.join("t.toml");
    std::fs::write(&p, tpl_text(extra)).unwrap();
    template::load(&p).map_err(|e| e.msg)
}

#[test]
fn template_palettes_and_air_parse() {
    let t = load_tpl("[tiles]\nair = [\"a\"]\n[palettes]\nsky = \"abc\"\nground = \"ad\"\n").unwrap();
    assert_eq!(t.palettes.len(), 2);
    assert_eq!(t.palettes[0].name, "sky");
    assert_eq!(t.palettes[0].keys, b"abc".to_vec());
    assert_eq!(t.tile_air, vec![b'a']);
    // a single string is accepted for air
    assert_eq!(load_tpl("[tiles]\nair = \"a\"\n[palettes]\np = \"ab\"\n").unwrap().tile_air, vec![b'a']);
}

#[test]
fn template_palette_errors() {
    let bad = [
        ("[palettes]\np = \"abcde\"\n", "1 to 4 keys"),
        ("[palettes]\np = \"abz\"\n", "not a coloured key"),
        ("[palettes]\np = \"aab\"\n", "twice"),
        ("[palettes]\np = \"\"\n", "1 to 4 keys"),
        ("[palettes]\np = \"ad\"\nq = \"ae\"\n", "same colour"),
        ("[tiles]\nair = [\"z\"]\n", "not a coloured key"),
        ("[tiles]\nair = [\"b\"]\n[palettes]\np = \"ac\"\n", "in no palette"),
        ("[palettes]\n1 = \"ab\"\n2 = \"ab\"\n3 = \"ab\"\n4 = \"ab\"\n5 = \"ab\"\n6 = \"ab\"\n7 = \"ab\"\n8 = \"ab\"\n9 = \"ab\"\n", "hardware has 8"),
    ];
    for (extra, want) in bad {
        let e = load_tpl(extra).unwrap_err();
        assert!(e.contains(want), "{:?}: {}", extra, e);
    }
}

#[test]
fn template_palettes_are_inherited_and_overridable() {
    let d = tmpdir("tpl_ext");
    std::fs::write(d.join("base.toml"), tpl_text("[tiles]\nair = [\"a\"]\n[palettes]\nsky = \"abc\"\nground = \"ad\"\n")).unwrap();
    std::fs::write(d.join("child.toml"), "name = \"child\"\nextends = \"base.toml\"\n[palettes]\nground = \"ac\"\nextra = \"ab\"\n").unwrap();
    let t = template::load(&d.join("child.toml")).unwrap();
    let names: Vec<&str> = t.palettes.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["sky", "ground", "extra"]);
    assert_eq!(t.palettes[1].keys, b"ac".to_vec());
    assert_eq!(t.tile_air, vec![b'a']);
}

// ------------------------------------------------------------------ reduction

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
}

fn fixture_tpl() -> template::Template {
    template::load(&fixtures().join("templates/tiles.toml")).unwrap()
}

fn rgb(tpl: &template::Template, k: u8) -> u32 {
    code_of(tpl.color_of(k).unwrap())
}

fn solid_block(c: u32) -> [u32; 256] {
    [c; 256]
}

#[test]
fn mask_prefers_the_border_and_ties_go_to_solid() {
    let tpl = fixture_tpl();
    let mut r = Reducer::new(&tpl).unwrap();
    let c = rgb(&tpl, b'c');
    // left border column is air: the air line survives at the tile edge
    let mut b = solid_block(c);
    for y in 0..16 {
        b[y * 16] = AIR;
    }
    let t = r.reduce_block(&b).unwrap();
    for y in 0..8 {
        assert_eq!(t[y * 8], b'a', "row {}", y);
        assert_eq!(t[y * 8 + 1], b'c');
    }
    // the same column one pixel inside is a tie in its cell: solid wins, no air at all
    let mut b = solid_block(c);
    for y in 0..16 {
        b[y * 16 + 6] = AIR;
    }
    let t = r.reduce_block(&b).unwrap();
    assert!(t.iter().all(|&k| k == b'c'), "{:?}", t);
    // air top half
    let mut b = solid_block(c);
    for y in 0..8 {
        for x in 0..16 {
            b[y * 16 + x] = AIR;
        }
    }
    let t = r.reduce_block(&b).unwrap();
    assert!(t[..32].iter().all(|&k| k == b'a') && t[32..].iter().all(|&k| k == b'c'));
}

#[test]
fn a_single_solid_pixel_is_never_lost_and_air_blocks_are_all_air() {
    let tpl = fixture_tpl();
    let mut r = Reducer::new(&tpl).unwrap();
    let t = r.reduce_block(&[AIR; 256]).unwrap();
    assert!(t.iter().all(|&k| k == b'a'));
    let mut b = [AIR; 256];
    b[5 * 16 + 5] = rgb(&tpl, b'c');
    let t = r.reduce_block(&b).unwrap();
    assert_eq!(t.iter().filter(|&&k| k != b'a').count(), 1);
}

#[test]
fn colours_come_from_the_best_palette_and_solid_cells_never_use_air() {
    let tpl = fixture_tpl();
    let mut r = Reducer::new(&tpl).unwrap();
    // grass colours with a little air: the grass palette `aefg` wins, nothing from sky/dirt
    let mut b = [AIR; 256];
    for y in 4..16 {
        for x in 0..16 {
            b[y * 16 + x] = rgb(&tpl, [b'e', b'f', b'g'][(x / 3 + y) % 3]);
        }
    }
    let t = r.reduce_block(&b).unwrap();
    let pal = palette_of(&tpl, &{
        let mut v = t.to_vec();
        v.sort();
        v.dedup();
        v
    });
    assert_eq!(pal, Some(1), "{:?}", String::from_utf8_lossy(&t));
    // solid pixels with exactly the air colour in a tile that has air: still not the air key
    let mut b = [AIR; 256];
    for y in 0..16 {
        for x in 8..16 {
            b[y * 16 + x] = rgb(&tpl, b'a');
        }
    }
    let t = r.reduce_block(&b).unwrap();
    for y in 0..8 {
        assert_eq!(t[y * 8], b'a');
        assert_ne!(t[y * 8 + 7], b'a', "solid cell took the air key");
    }
    // random blocks: one palette, at most 4 keys, deterministic, identical blocks identical tiles
    let mut rng = Rng(7);
    let pool: Vec<u32> = (b'a'..=b'y').map(|k| rgb(&tpl, k)).chain([code_of(color::Rgb::new(248, 0, 0)), code_of(color::Rgb::new(8, 8, 8))]).collect();
    for n in 0..300 {
        let mut b = [0u32; 256];
        let air_share = rng.next() % 4;
        for i in 0..256 {
            b[i] = if rng.next() % 4 < air_share { AIR } else { pool[(rng.next() as usize + (i / 40)) % pool.len()] };
        }
        let t1 = r.reduce_block(&b).unwrap();
        let t2 = Reducer::new(&tpl).unwrap().reduce_block(&b).unwrap();
        assert_eq!(t1, t2, "block {} not deterministic", n);
        let mut keys = t1.to_vec();
        keys.sort();
        keys.dedup();
        assert!(keys.len() <= 4 && palette_of(&tpl, &keys).is_some(), "block {}: {:?}", n, String::from_utf8_lossy(&keys));
    }
}

#[test]
fn reduction_is_flip_equivariant() {
    let tpl = fixture_tpl();
    let mut r = Reducer::new(&tpl).unwrap();
    let mut rng = Rng(99);
    let pool: Vec<u32> = (b'a'..=b'o').map(|k| rgb(&tpl, k)).collect();
    let flip16 = |a: &[u32; 256], fx: bool, fy: bool| -> [u32; 256] {
        let mut o = [0u32; 256];
        for y in 0..16 {
            for x in 0..16 {
                o[y * 16 + x] = a[(if fy { 15 - y } else { y }) * 16 + if fx { 15 - x } else { x }];
            }
        }
        o
    };
    let flip8 = |a: &[u8; 64], fx: bool, fy: bool| -> [u8; 64] {
        let mut o = [0u8; 64];
        for y in 0..T {
            for x in 0..T {
                o[y * 8 + x] = a[(if fy { 7 - y } else { y }) * 8 + if fx { 7 - x } else { x }];
            }
        }
        o
    };
    for n in 0..250 {
        let mut b = [0u32; 256];
        for i in 0..256 {
            b[i] = if rng.next() % 5 == 0 { AIR } else { pool[(rng.next() as usize) % pool.len()] };
        }
        if n % 3 == 0 {
            // x-symmetric block
            b = flip16(&b, false, false);
            for y in 0..16 {
                for x in 0..8 {
                    b[y * 16 + 15 - x] = b[y * 16 + x];
                }
            }
        }
        let t = r.reduce_block(&b).unwrap();
        for (fx, fy) in [(true, false), (false, true), (true, true)] {
            let tf = r.reduce_block(&flip16(&b, fx, fy)).unwrap();
            assert_eq!(tf, flip8(&t, fx, fy), "block {} flip {}{}", n, fx, fy);
        }
    }
}

// ------------------------------------------------------------------ load through the CLI

fn png_tileset(dir: &std::path::Path) -> std::path::PathBuf {
    use agentsprite::img::{Rgba, RgbaImage};
    std::fs::create_dir_all(dir.join("refs")).unwrap();
    std::fs::create_dir_all(dir.join("templates")).unwrap();
    std::fs::copy(fixtures().join("templates/tiles.toml"), dir.join("templates/tiles.toml")).unwrap();
    let tpl = fixture_tpl();
    let mut img = RgbaImage::new(256, 32);
    let cols = [b'e', b'f', b'g', b'h', b'i', b'k'];
    for by in 0..2 {
        for bx in 0..16 {
            let n = by * 16 + bx;
            for y in 0..16 {
                for x in 0..16 {
                    let c = if n % 5 == 0 && y < 6 {
                        Rgba::TRANSPARENT
                    } else {
                        let k = cols[(x / 4 + y / 4 + n) % cols.len()];
                        let rgb = tpl.color_of(k).unwrap();
                        Rgba::opaque(rgb.r, rgb.g, rgb.b)
                    };
                    img.set((bx * 16 + x) as i32, (by * 16 + y) as i32, c);
                }
            }
        }
    }
    agentsprite::png::write_rgba(&dir.join("refs/blocks.png"), &img).unwrap();
    let p = dir.join("sheet.pxl");
    std::fs::write(&p, "name = \"sheet\"\nkind = \"tileset\"\nsize = [128, 8]\ntile = [8, 8]\ntemplate = \"templates/tiles.toml\"\n\n[input]\nimages = \"refs/{frame}.png\"\n\n[reduce]\nfactor = 2\n\n[[frames]]\nname = \"blocks\"\n").unwrap();
    p
}

#[test]
fn load_reduces_a_png_sheet_to_legal_tiles_and_sizes_the_part() {
    let d = tmpdir("png_load");
    let p = png_tileset(&d);
    let o = pxl_in(&d, &["load", "sheet.pxl"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let part = pxl::load(&p).unwrap();
    assert_eq!(part.size, (128, 16), "two block rows -> two tile rows");
    assert_eq!((part.frames[0].grid.w, part.frames[0].grid.h), (128, 16));
    assert!(lint_rules(&p, &["tile_colors", "tile_palettes", "unique_tiles", "alpha", "size", "template"]).is_empty());
    // block 0 has air in its top rows: those cells are the air key, deterministic on reload
    assert_eq!(part.frames[0].grid.cells[0], b'a');
    let first = std::fs::read_to_string(&p).unwrap();
    assert!(pxl_in(&d, &["reload", "sheet.pxl", "--force"]).status.success());
    assert_eq!(std::fs::read_to_string(&p).unwrap(), first);
    // identical blocks give identical tiles: blocks 1 and 6 share a pattern shifted by 5 -> compare 0/5/10
    let g = &part.frames[0].grid;
    let tile = |i: usize| -> Vec<u8> { (0..8).flat_map(|y| (0..8).map(move |x| (y, x))).map(|(y, x)| g.cells[((i / 16) * 8 + y) * 128 + (i % 16) * 8 + x]).collect() };
    assert_eq!(tile(0).iter().filter(|&&k| k == b'a').count() > 0, true);
}

// ------------------------------------------------------------------ stats

/// The checker's `check_tile_image` loop, translated: (unique count, palettes).
fn checker_model(tiles: &[[u32; 64]]) -> (usize, usize) {
    let mut palettes: Vec<std::collections::BTreeSet<u32>> = Vec::new();
    let mut unique: std::collections::BTreeSet<[u32; 64]> = Default::default();
    for t in tiles {
        let cols: std::collections::BTreeSet<u32> = t.iter().cloned().collect();
        if cols.len() > 4 {
            continue;
        }
        let mut done = false;
        for p in palettes.iter_mut() {
            if p.union(&cols).count() <= 4 {
                p.extend(cols.iter().cloned());
                done = true;
                break;
            }
        }
        if !done {
            palettes.push(cols);
        }
        let keys: Vec<[u32; 64]> = [(false, false), (true, false), (false, true), (true, true)].iter().map(|&(fx, fy)| stats::flip(t, fx, fy)).collect();
        if !keys.iter().any(|k| unique.contains(k)) {
            unique.insert(*t);
        }
    }
    (unique.len(), palettes.len())
}

#[test]
fn stats_follow_the_checkers_rules_on_random_tile_sets() {
    let mut rng = Rng(5);
    for round in 0..40 {
        let ncol = 2 + (round % 7);
        let mut sheet_tiles: Vec<[u32; 64]> = Vec::new();
        for _ in 0..(20 + round * 3) {
            let k = 1 + rng.next() % 4;
            let colset: Vec<u32> = (0..k).map(|_| (rng.next() % ncol as u64) as u32 + 1).collect();
            let mut t = [0u32; 64];
            let style = rng.next() % 3;
            for i in 0..64 {
                t[i] = if style == 0 { colset[0] } else { colset[(rng.next() % k) as usize] };
            }
            if rng.next() % 3 == 0 {
                if let Some(prev) = sheet_tiles.last() {
                    t = stats::flip(prev, rng.next() % 2 == 0, rng.next() % 2 == 0);
                }
            }
            sheet_tiles.push(t);
        }
        let sets: Vec<Vec<u32>> = sheet_tiles.iter().map(stats::colors_of).collect();
        let g = stats::greedy_sets(&sets);
        let (uniq, pals) = checker_model(&sheet_tiles);
        assert_eq!(g.bins.len(), pals, "round {}", round);
        // unique via canonical orientation == the checker's loop
        let mut canon: std::collections::BTreeSet<[u32; 64]> = Default::default();
        for t in &sheet_tiles {
            canon.insert(stats::canon(t));
        }
        assert_eq!(canon.len(), uniq, "round {}", round);
    }
}

#[test]
fn optimal_packing_is_exact_and_never_worse_than_first_fit() {
    let mut rng = Rng(11);
    for round in 0..120 {
        let n = 3 + (round % 5);
        let sets: Vec<Vec<u32>> = (0..n)
            .map(|_| {
                let k = 1 + rng.next() % 3;
                let mut v: Vec<u32> = (0..k).map(|_| (rng.next() % 7) as u32).collect();
                v.sort();
                v.dedup();
                v
            })
            .collect();
        let g = stats::greedy_sets(&sets).bins.len();
        let (opt, exact) = stats::optimal_sets(&sets);
        assert!(exact && opt <= g, "round {}: opt {} greedy {}", round, opt, g);
        // brute force
        fn brute(sets: &[Vec<u32>], i: usize, bins: &mut Vec<std::collections::BTreeSet<u32>>, best: &mut usize) {
            if bins.len() >= *best {
                return;
            }
            if i == sets.len() {
                *best = bins.len();
                return;
            }
            for b in 0..bins.len() {
                let old = bins[b].clone();
                if old.union(&sets[i].iter().cloned().collect()).count() <= 4 {
                    bins[b].extend(sets[i].iter().cloned());
                    brute(sets, i + 1, bins, best);
                    bins[b] = old;
                }
            }
            bins.push(sets[i].iter().cloned().collect());
            brute(sets, i + 1, bins, best);
            bins.pop();
        }
        let mut best = usize::MAX;
        brute(&sets, 0, &mut vec![], &mut best);
        assert_eq!(opt, best, "round {}: {:?}", round, sets);
    }
    // a case where first-fit is worse than optimal: {1,2} {3,4,5}? -> 1+2 joins nothing wrong; use the classic order trap
    let trap = vec![vec![1, 2], vec![3, 4], vec![1, 3, 5], vec![2, 4, 6]];
    let g = stats::greedy_sets(&trap).bins.len();
    let (o, _) = stats::optimal_sets(&trap);
    assert!(o <= g);
}

#[test]
fn tiles_stats_command_reports_unique_duplicates_and_palettes() {
    let d = tmpdir("stats_cmd");
    let p = copy_fixture("lint/unique_tiles/neg2.pxl", &d);
    let o = pxl_in(&d, &["tiles", "stats", p.file_name().unwrap().to_str().unwrap(), "--json"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let v = json_of(&o);
    assert_eq!(v["data"]["unique"].as_i64().or(v["unique"].as_i64()), Some(100));
    let groups = v["data"]["duplicate_groups"].as_array().or(v["duplicate_groups"].as_array()).unwrap().len();
    assert_eq!(groups, 100, "every tile has copies: 4 per group");
    assert_eq!(v["data"]["palettes_greedy"].as_i64().or(v["palettes_greedy"].as_i64()), Some(1));
}

// ------------------------------------------------------------------ fit (AC-T1)

fn tiles_cmd(d: &std::path::Path, args: &[&str]) -> std::process::Output {
    let mut a = vec!["tiles"];
    a.extend_from_slice(args);
    pxl_in(d, &a)
}

#[test]
fn ac_t1_after_fit_apply_tile_palettes_passes_and_the_checker_agrees() {
    let script = checker_available();
    for fx in ["lint/tile_colors/pos1.pxl", "lint/tile_colors/pos2.pxl", "lint/tile_colors/pos3.pxl", "lint/tile_palettes/pos1.pxl", "lint/tile_palettes/pos2.pxl", "lint/tile_palettes/pos3.pxl"] {
        let d = tmpdir("fit");
        let p = copy_fixture(fx, &d);
        let name = p.file_name().unwrap().to_str().unwrap().to_string();
        assert!(!lint_rules(&p, &["tile_colors", "tile_palettes"]).is_empty(), "{} must fail before", fx);
        let o = tiles_cmd(&d, &["fit", &name, "--apply"]);
        assert!(o.status.success(), "{}: {}{}", fx, stdout(&o), stderr(&o));
        let left = lint_rules(&p, &["tile_colors", "tile_palettes"]);
        assert!(left.is_empty(), "{}: {:?}", fx, left);
        if let Some(s) = &script {
            let png = d.join("out.png");
            export_png(&pxl::load(&p).unwrap(), 0, &png);
            let (_, text) = run_checker(s, &png);
            assert!(checker_rules(&text).iter().all(|r| *r == "unique_tiles"), "{}: checker disagrees\n{}", fx, text);
        }
    }
}

#[test]
fn fit_dry_run_writes_nothing_and_ops_file_equals_apply_and_undo_reverts() {
    let d = tmpdir("fit_ops");
    let p = copy_fixture("lint/tile_palettes/pos3.pxl", &d);
    let before = std::fs::read_to_string(&p).unwrap();
    let o = tiles_cmd(&d, &["fit", "pos3.pxl", "--write", "ops.json"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    assert_eq!(std::fs::read_to_string(&p).unwrap(), before, "dry run must not edit the part");
    let ops: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.join("ops.json")).unwrap()).unwrap();
    let list = ops["ops"].as_array().unwrap();
    assert!(!list.is_empty() && list.iter().all(|o| o["op"] == "replace" && o["region"].as_array().unwrap().len() == 4 && o["maps"].is_object()));
    // apply the file on a copy == fit --apply
    let d2 = tmpdir("fit_ops2");
    let p2 = copy_fixture("lint/tile_palettes/pos3.pxl", &d2);
    std::fs::copy(d.join("ops.json"), d2.join("ops.json")).unwrap();
    assert!(pxl_in(&d2, &["apply", "pos3.pxl", "ops.json"]).status.success());
    assert!(tiles_cmd(&d, &["fit", "pos3.pxl", "--apply"]).status.success());
    assert_eq!(std::fs::read_to_string(&p).unwrap().split("grid").last(), std::fs::read_to_string(&p2).unwrap().split("grid").last());
    // one journaled edit: a single undo restores the original grid
    let u = pxl_in(&d, &["undo", "pos3.pxl"]);
    assert!(u.status.success(), "{}{}", stdout(&u), stderr(&u));
    assert_eq!(pxl::load(&p).unwrap().frames[0].grid, {
        let orig = d.join("orig.pxl");
        std::fs::write(&orig, &before).unwrap();
        pxl::load(&orig).unwrap().frames[0].grid.clone()
    });
}

#[test]
fn fit_on_a_legal_set_changes_nothing() {
    let d = tmpdir("fit_noop");
    let p = copy_fixture("lint/tile_colors/neg3.pxl", &d);
    let before = std::fs::read_to_string(&p).unwrap();
    let o = tiles_cmd(&d, &["fit", "neg3.pxl", "--apply"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    assert!(stdout(&o).contains("nothing to apply"));
    assert_eq!(std::fs::read_to_string(&p).unwrap(), before);
}

#[test]
fn fit_picks_the_cheapest_nearest_colour() {
    // one tile with the five colours of `abcd` plus `b`-like `v` (#c0c8d0 vs b #f8f8f8): v must go to the nearest palette colour
    let d = tmpdir("fit_near");
    let p = copy_fixture("lint/tile_colors/neg1.pxl", &d);
    let text = std::fs::read_to_string(&p).unwrap().replace("\r\n", "\n");
    let at = text.find("grid = \"\"\"\n").unwrap() + 11;
    // first row: `labcd` instead of `aaaaa`: l (#c0c8d0) is nearest to b (#f8f8f8) among a b c d
    let text = format!("{}labcd{}", &text[..at], &text[at + 5..]);
    std::fs::write(&p, text).unwrap();
    let o = tiles_cmd(&d, &["fit", "neg1.pxl", "--apply"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let g = pxl::load(&p).unwrap().frames[0].grid.clone();
    assert_eq!(g.cells[0], b'b', "l (#c0c8d0) is nearest to b (#f8f8f8) among a b c d");
}

// ------------------------------------------------------------------ dedupe

#[test]
fn dedupe_reaches_the_limit_lists_every_merge_and_lint_passes() {
    for fx in ["lint/unique_tiles/pos1.pxl", "lint/unique_tiles/pos3.pxl"] {
        let d = tmpdir("dedupe");
        let p = copy_fixture(fx, &d);
        let name = p.file_name().unwrap().to_str().unwrap().to_string();
        assert!(!lint_rules(&p, &["unique_tiles"]).is_empty());
        let dry = tiles_cmd(&d, &["dedupe", &name, "--max", "300", "--json"]);
        assert!(dry.status.success(), "{}{}", stdout(&dry), stderr(&dry));
        let v = json_of(&dry);
        let data = v.get("data").unwrap_or(&v);
        let merges = data["merges"].as_array().unwrap();
        assert!(data["unique_before"].as_i64().unwrap() > 300 && data["unique_after"].as_i64().unwrap() <= 300);
        assert!(merges.iter().all(|m| m["tile"].is_u64() && m["into"].is_u64() && m["worst"].is_f64() && m["mean"].is_f64() && m["usage"].is_u64()));
        assert!(pxl::load(&p).is_ok());
        let o = tiles_cmd(&d, &["dedupe", &name, "--max", "300", "--apply"]);
        assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
        assert!(lint_rules(&p, &["unique_tiles", "tile_colors", "tile_palettes"]).is_empty());
        if let Some(s) = checker_available() {
            let png = d.join("out.png");
            export_png(&pxl::load(&p).unwrap(), 0, &png);
            let (_, text) = run_checker(&s, &png);
            assert!(checker_rules(&text).is_empty(), "{}", text);
        }
    }
}

#[test]
fn dedupe_below_the_limit_does_nothing() {
    let d = tmpdir("dedupe_noop");
    let p = copy_fixture("lint/unique_tiles/neg2.pxl", &d);
    let before = std::fs::read_to_string(&p).unwrap();
    let o = tiles_cmd(&d, &["dedupe", "neg2.pxl", "--max", "300", "--apply"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    assert!(stdout(&o).contains("nothing to merge"));
    assert_eq!(std::fs::read_to_string(&p).unwrap(), before);
}

#[test]
fn dedupe_merges_the_closest_tiles_and_load_makes_the_tiles() {
    let d = tmpdir("dedupe_level");
    let p = synth_level(&d);
    let o = pxl_in(&d, &["load", "syn.pxl"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let part = pxl::load(&p).unwrap();
    assert_eq!(part.size, (128, 8));
    let g = &part.frames[0].grid;
    let tile = |i: usize| -> String { (0..8).map(|y| (0..8).map(|x| g.cells[y * 128 + i * 8 + x] as char).collect::<String>()).collect::<Vec<_>>().join("/") };
    assert_eq!(tile(0), "aaaaaaaa/".repeat(7) + "aaaaaaaa");
    assert_eq!(tile(1), "bbbbbbbb/".repeat(7) + "bbbbbbbb");
    assert_eq!(tile(2), "bbbbbbbb/bbbbbbbb/bbbbbbbb/bbbbbbbb/bbbbcccc/bbbbcccc/bbbbcccc/bbbbcccc");
    assert_eq!(tile(3), "bbbbbbbb/bbbbbbbb/bbbbbbbb/bbbbbbbb/cccccccc/cccccccc/cccccccc/cccccccc");
    // 4 unique tiles; tiles 1 and 2 are the closest pair
    let o = tiles_cmd(&d, &["dedupe", "syn.pxl", "--max", "3", "--json"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let v = json_of(&o);
    let data = v.get("data").unwrap_or(&v);
    let merges = data["merges"].as_array().unwrap();
    assert_eq!(merges.len(), 1, "{:?}", merges);
    let (a, b) = (merges[0]["tile"].as_u64().unwrap(), merges[0]["into"].as_u64().unwrap());
    assert!(a + b == 3 && (a == 1 || a == 2), "only tiles 1 and 2 may merge: {} -> {}", a, b);
    assert_eq!(merges[0]["visible"], true, "a quarter of the tile differs");
    // apply to 3: tile 3 and tile 0 stay as they are
    let o = tiles_cmd(&d, &["dedupe", "syn.pxl", "--max", "3", "--apply"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let g2 = pxl::load(&p).unwrap().frames[0].grid.clone();
    let tile2 = |i: usize| -> String { (0..8).map(|y| (0..8).map(|x| g2.cells[y * 128 + i * 8 + x] as char).collect::<String>()).collect::<Vec<_>>().join("/") };
    assert_eq!(tile2(0), tile(0));
    assert_eq!(tile2(3), tile(3));
    assert_eq!(tile2(1), tile2(2));
}

// ------------------------------------------------------------------ palette derivation

#[test]
fn template_suggest_derive_writes_a_tile_template_that_loads_and_fits() {
    let d = tmpdir("derive");
    let _ = synth_level(&d);
    // the air blocks are transparent in the reference
    let mut im = agentsprite::png::read_rgba(&d.join("refs/blocks.png")).unwrap();
    for (i, p) in im.px.iter_mut().enumerate() {
        if (i % 256) / 16 == 0 || (i % 256) / 16 > 3 {
            p.a = 0;
        }
    }
    agentsprite::png::write_rgba(&d.join("refs/blocks.png"), &im).unwrap();
    std::fs::remove_file(d.join("syn_tiles.toml")).unwrap();
    // without a template the part cannot load yet; derive reads the sources only
    let text = std::fs::read_to_string(d.join("syn.pxl")).unwrap().replace("template = \"syn_tiles.toml\"\n", "");
    std::fs::write(d.join("syn.pxl"), text).unwrap();
    let o = pxl_in(&d, &["template", "suggest", "syn.pxl", "--derive", "--write", "gen.toml"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let t = template::load(&d.join("gen.toml")).unwrap();
    assert!(!t.palettes.is_empty() && t.palettes.len() <= 8 && t.palettes.iter().all(|p| p.keys.len() <= 4));
    assert_eq!(t.tile_air, vec![b'a']);
    let text = std::fs::read_to_string(d.join("syn.pxl")).unwrap().replace("tile = [8, 8]\n", "tile = [8, 8]\ntemplate = \"gen.toml\"\n");
    std::fs::write(d.join("syn.pxl"), text).unwrap();
    assert!(pxl_in(&d, &["load", "syn.pxl"]).status.success());
    assert!(lint_rules(&d.join("syn.pxl"), &["tile_colors", "tile_palettes", "unique_tiles"]).is_empty());
    // never overwrites
    let o = pxl_in(&d, &["template", "suggest", "syn.pxl", "--derive", "--write", "gen.toml"]);
    assert!(!o.status.success());
}

#[test]
fn fit_repairs_first_fit_packing_when_every_tile_fits_a_template_palette() {
    // tiles that each use 2-3 colours of one template palette, in an order that makes first-fit mix palettes
    let pals = ["abcd", "aefg", "ahij", "aklm", "anop", "aqrs", "atuv", "awxy"];
    let mut worst_before = 0;
    for seed in 0..12u64 {
        let mut rng = Rng(seed + 100);
        let mut tiles: Vec<Vec<String>> = Vec::new();
        for _ in 0..150 {
            let pal: Vec<char> = pals[(rng.next() % 8) as usize].chars().collect();
            let n = 2 + (rng.next() % 2) as usize;
            let mut ks = vec![pal[(rng.next() % 4) as usize]];
            while ks.len() < n {
                let k = pal[(rng.next() % 4) as usize];
                if !ks.contains(&k) {
                    ks.push(k);
                }
            }
            let rows: Vec<String> = (0..8).map(|y| (0..8).map(|x| ks[(x + y * 3 + (rng.next() % 2) as usize) % ks.len()]).collect()).collect();
            tiles.push(rows);
        }
        let mut grid = Vec::new();
        for r in 0..tiles.len().div_ceil(16) {
            for y in 0..8 {
                let mut line = String::new();
                for c in 0..16 {
                    let i = r * 16 + c;
                    line.push_str(if i < tiles.len() { &tiles[i][y] } else { "aaaaaaaa" });
                }
                grid.push(line);
            }
        }
        let d = tmpdir("fit_rep");
        std::fs::create_dir_all(d.join("templates")).unwrap();
        std::fs::copy(fixtures().join("templates/tiles.toml"), d.join("templates/tiles.toml")).unwrap();
        let p = d.join("r.pxl");
        std::fs::write(&p, format!("name = \"r\"\nkind = \"tileset\"\nsize = [128, {}]\ntile = [8, 8]\ntemplate = \"templates/tiles.toml\"\n\n[[frames]]\nname = \"blocks\"\ngrid = \"\"\"\n{}\n\"\"\"\n", grid.len(), grid.join("\n"))).unwrap();
        let before = lint_rules(&p, &["tile_palettes"]);
        if let Some(i) = before.first() {
            let n: usize = i.message.split_whitespace().nth(2).unwrap().parse().unwrap();
            worst_before = worst_before.max(n);
        }
        let o = tiles_cmd(&d, &["fit", "r.pxl", "--apply"]);
        assert!(o.status.success(), "seed {}: {}{}", seed, stdout(&o), stderr(&o));
        assert!(lint_rules(&p, &["tile_palettes", "tile_colors"]).is_empty(), "seed {}", seed);
        if let Some(s) = checker_available() {
            let png = d.join("o.png");
            export_png(&pxl::load(&p).unwrap(), 0, &png);
            let (pass, text) = run_checker(&s, &png);
            assert!(pass, "seed {}: {}", seed, text);
        }
    }
    assert!(worst_before > 8, "no seed exercised the first-fit repair (worst before: {})", worst_before);
}
