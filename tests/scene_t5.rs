//! M5 T4/T5: scenes and the shared hardware model, `pxl check` fail closed, `pxl scene --worst`, overlays, `pxl delivery`.
mod e2e_common;

use e2e_common::*;
use std::path::{Path, PathBuf};

const TPL: &str = r##"name = "t"
[layers.base]
"." = { role = "transparent" }
"1" = { role = "outline", color = "#182078" }
"2" = { role = "body", color = "#4850c0" }
"3" = { role = "skin", color = "#f8b890" }
"b" = { role = "t_a", color = "#f8f8f8" }
"c" = { role = "t_b", color = "#183058" }
"d" = { role = "t_c", color = "#28a048" }
"a" = { role = "air", color = "#88c8f8" }
[layers.detail]
"R" = { role = "red", color = "#f80000", under = "1" }
[tiles]
air = ["a"]
"##;

const SET: &str = r##"[[slots]]
slot = 0
colors = ["#182078", "#4850c0", "#f8b890"]
[[slots]]
slot = 1
colors = ["#900000", "#f80000", "#f8f8f8"]
"##;

fn project(tag: &str) -> PathBuf {
    let d = tmpdir(tag);
    std::fs::create_dir_all(d.join("palettes")).unwrap();
    std::fs::create_dir_all(d.join("templates")).unwrap();
    std::fs::write(d.join("palettes/s.toml"), SET).unwrap();
    std::fs::write(d.join("templates/t.toml"), TPL).unwrap();
    d
}

fn solid(w: usize, h: usize) -> Vec<String> {
    (0..h).map(|_| "2".repeat(w)).collect()
}

fn write_part(d: &Path, name: &str, header: &str, rows: &[String]) -> PathBuf {
    let text = format!("name = \"{}\"\nsize = [{}, {}]\norigin = [0, 0]\ntemplate = \"templates/t.toml\"\n{}\n[[frames]]\nname = \"a\"\ngrid = \"\"\"\n{}\n\"\"\"\n", name, rows[0].len(), rows.len(), header, rows.join("\n"));
    let p = d.join(format!("{}.pxl", name));
    std::fs::write(&p, text).unwrap();
    p
}

/// Parts: player 16 wide, objects 'wide' x 16 wide, enemy 16, ring 8; a scene linking them.
fn scene_project(tag: &str, object_w: usize, extra_scene: &str, groups_extra: &str) -> PathBuf {
    let d = project(tag);
    write_part(&d, "player", "palette_set = \"s\"\ncategory = \"player\"\n", &solid(16, 16));
    write_part(&d, "obj", "palette_set = \"s\"\ncategory = \"platform\"\n", &solid(object_w, 16));
    write_part(&d, "enemy", "palette_set = \"s\"\ncategory = \"enemy\"\n", &solid(16, 16));
    write_part(&d, "ring", "palette_set = \"s\"\ncategory = \"item\"\n", &solid(8, 16));
    std::fs::write(
        d.join("pxl.toml"),
        format!(
            "[parts]\nplayer = \"player.pxl\"\nobj = \"obj.pxl\"\nenemy = \"enemy.pxl\"\nring = \"ring.pxl\"\n\n[scene.z]\npalette_set = \"s\"\n{}\ngroups = {{ player = {{ streamed = true, per_frame = 7, parts = [\"player\"] }}, objects = {{ budget = 44, parts = [\"obj\"] }}, enemies = {{ budget = 48, parts = [\"enemy\"] }}, common = {{ budget = 56, parts = [\"ring\"] }}{} }}\n",
            extra_scene, groups_extra
        ),
    )
    .unwrap();
    d
}

#[test]
fn the_hardware_model_is_declared_in_the_profiles() {
    let p = agentsprite::profile::load("gbc-sprite").unwrap();
    assert_eq!((p.obj_palettes, p.hw_cells_max, p.hw_cells_per_row_max, p.sprite_cells, p.dma_bytes_per_frame, p.overlays.as_str()), (8, 40, 10, 178, 1024, "category"));
    let t = agentsprite::profile::load("gbc-tile").unwrap();
    assert_eq!((t.palettes, t.bg_reserved.clone(), t.window_tiles, t.unique_tiles_max), (8, vec![7], 64, 348));
}

#[test]
fn check_fails_closed_without_scenes_or_palette_sets() {
    let d = project("t5closed");
    write_part(&d, "old", "", &solid(8, 16));
    std::fs::write(d.join("pxl.toml"), "[parts]\nold = \"old.pxl\"\n").unwrap();
    let o = pxl_in(&d, &["check", "--json"]);
    assert_eq!(o.status.code(), Some(1));
    let j = json_of(&o);
    let h = j["hardware"].as_array().unwrap();
    assert!(h.iter().any(|x| x.as_str().unwrap().contains("has no palette_set")), "{}", j);
    assert!(h.iter().any(|x| x.as_str().unwrap().contains("declares no [scene")), "{}", j);
    // the escape hatch is explicit
    std::fs::write(d.join("pxl.toml"), "legacy = true\n[parts]\nold = \"old.pxl\"\n").unwrap();
    std::fs::create_dir_all(d.join("profiles")).unwrap();
    assert!(pxl_in(&d, &["export", "old.pxl"]).status.success());
    let o = pxl_in(&d, &["check", "--json"]);
    assert!(json_of(&o)["hardware"].as_array().unwrap().is_empty(), "{}", stdout(&o));
}

#[test]
fn a_sprite_part_in_no_scene_is_an_error_and_a_fitting_scene_passes() {
    let d = scene_project("t5scene", 16, "", "");
    write_part(&d, "stray", "palette_set = \"s\"\n", &solid(8, 16));
    let mut m = std::fs::read_to_string(d.join("pxl.toml")).unwrap();
    m = m.replacen("ring = \"ring.pxl\"\n", "ring = \"ring.pxl\"\nstray = \"stray.pxl\"\n", 1);
    std::fs::write(d.join("pxl.toml"), &m).unwrap();
    let j = json_of(&pxl_in(&d, &["check", "--json"]));
    assert!(j["hardware"].as_array().unwrap().iter().any(|x| x.as_str().unwrap().contains("stray is in no scene")), "{}", j["hardware"]);
    // without the stray part the scene passes (the missing tileset is a note, not a failure)
    let d = scene_project("t5scene2", 16, "", "");
    for p in ["player", "obj", "enemy", "ring"] {
        assert!(pxl_in(&d, &["export", &format!("{}.pxl", p)]).status.success());
    }
    let o = pxl_in(&d, &["check", "--json"]);
    let j = json_of(&o);
    assert_eq!(j["scenes"][0]["ok"], true, "{}", j["scenes"]);
    assert!(j["hardware"].as_array().unwrap().is_empty(), "{}", j["hardware"]);
    let rows = j["scenes"][0]["rows"].as_array().unwrap();
    for item in ["OBJ palettes", "sprite VRAM", "DMA bytes/frame", "group objects"] {
        assert!(rows.iter().any(|r| r["item"] == item), "row {} in {}", item, j["scenes"]);
    }
}

#[test]
fn sprite_vram_and_group_budgets_are_summed_per_scene() {
    // a resident group with a tiny budget fails its own row; a huge budget cannot hide the VRAM sum (178)
    let d = scene_project("t5vram", 16, "", "");
    let m = std::fs::read_to_string(d.join("pxl.toml")).unwrap().replace("objects = { budget = 44", "objects = { budget = 0");
    std::fs::write(d.join("pxl.toml"), m).unwrap();
    let j = json_of(&pxl_in(&d, &["scene", "z", "--json"]));
    let rows = j["scene"]["rows"].as_array().unwrap();
    assert!(rows.iter().any(|r| r["item"] == "group objects" && r["ok"] == false));
    let sum = rows.iter().find(|r| r["item"] == "sprite VRAM").unwrap();
    assert_eq!(sum["limit"], "178");
}

#[test]
fn bg_tiles_palettes_and_window_are_checked_per_scene() {
    let d = scene_project("t5bg", 16, "tileset = \"blocks\"\nwindow = [\"hud\"]", "");
    // a zone tile set: 8 palettes of tiles -> over the 7 left after the reserved one
    let mut g: Vec<Vec<u8>> = vec![vec![b'a'; 128]; 8];
    let pals = ["abcd", "aefg", "ahij", "aklm", "anop", "aqrs", "atuv", "awxy"];
    let _ = pals;
    // use the keys of the template we have: only 4 coloured ones, so build the palettes from 1/2/3 mixes instead
    for (i, set) in ["123a", "b2c3", "bcda", "12bc", "1d2a", "3bcd", "13bd", "2c3a"].iter().enumerate() {
        let k: Vec<u8> = set.bytes().collect();
        for y in 0..8 {
            for x in 0..8 {
                g[y][i * 8 + x] = k[(x + y * 3) % 4];
            }
        }
    }
    let rows: Vec<String> = g.iter().map(|r| String::from_utf8(r.clone()).unwrap()).collect();
    let tile_part = |name: &str, header: &str, rows: &[String]| {
        let text = format!("name = \"{}\"\nkind = \"tileset\"\nsize = [128, {}]\norigin = [0, 0]\ntile = [8, 8]\ntemplate = \"templates/t.toml\"\n{}\n[[frames]]\nname = \"blocks\"\ngrid = \"\"\"\n{}\n\"\"\"\n", name, rows.len(), header, rows.join("\n"));
        std::fs::write(d.join(format!("{}.pxl", name)), text).unwrap();
    };
    tile_part("blocks", "", &rows);
    // the HUD tile part on a reserved palette: 3 colours only
    let hud: Vec<String> = (0..8).map(|y| (0..128).map(|x| ['b', 'c', 'd'][(x + y) % 3]).collect()).collect();
    tile_part("hud", "bg_palette = 7\nmax_tiles = 64\n", &hud);
    let m = std::fs::read_to_string(d.join("pxl.toml")).unwrap().replace("obj = \"obj.pxl\"\n", "obj = \"obj.pxl\"\nblocks = \"blocks.pxl\"\nhud = \"hud.pxl\"\n");
    std::fs::write(d.join("pxl.toml"), m).unwrap();
    let j = json_of(&pxl_in(&d, &["scene", "z", "--json"]));
    let rows = j["scene"]["rows"].as_array().unwrap();
    let pal = rows.iter().find(|r| r["item"] == "BG palettes").unwrap();
    assert_eq!(pal["limit"], "7", "{}", j);
    let win = rows.iter().find(|r| r["item"] == "window tiles").unwrap();
    assert_eq!(win["ok"], true, "{}", win);
    assert_eq!(win["limit"], "64");
    // the HUD off the reserved palette fails
    let t = std::fs::read_to_string(d.join("hud.pxl")).unwrap().replace("bg_palette = 7", "bg_palette = 2");
    std::fs::write(d.join("hud.pxl"), t).unwrap();
    let j = json_of(&pxl_in(&d, &["scene", "z", "--json"]));
    let win = j["scene"]["rows"].as_array().unwrap().iter().find(|r| r["item"] == "window tiles").unwrap().clone();
    assert_eq!(win["ok"], false, "{}", win);
}

#[test]
fn dma_counts_the_cells_that_change_between_consecutive_frames() {
    let d = project("t5dma");
    // 9 frames, every one a different 8x16 pattern: 8 changed cells (new ones) per step is not possible with one cell,
    // so use 3-cell frames whose cells all change: 3 cells x 32 bytes per frame
    let mut frames = String::new();
    for i in 0..4 {
        let rows: Vec<String> = (0..16).map(|y| (0..24).map(|x| if (x * 7 + y * (i + 2) + i * x * y) % 5 < 3 { '2' } else { '1' }).collect::<String>()).collect();
        frames.push_str(&format!("\n[[frames]]\nname = \"f{}\"\ngrid = \"\"\"\n{}\n\"\"\"\n", i, rows.join("\n")));
    }
    std::fs::write(d.join("fx.pxl"), format!("name = \"fx\"\nsize = [24, 16]\norigin = [0, 0]\ntemplate = \"templates/t.toml\"\npalette_set = \"s\"\ncategory = \"effect\"\n{}", frames)).unwrap();
    std::fs::write(d.join("pxl.toml"), "[parts]\nfx = \"fx.pxl\"\n\n[scene.z]\npalette_set = \"s\"\ngroups = { effects = { streamed = true, per_frame = 8, parts = [\"fx\"] } }\n").unwrap();
    let j = json_of(&pxl_in(&d, &["scene", "z", "--json"]));
    let dma = j["scene"]["rows"].as_array().unwrap().iter().find(|r| r["item"] == "DMA bytes/frame").unwrap().clone();
    let bytes: u64 = dma["used"].as_str().unwrap().parse().unwrap();
    assert!(bytes > 0 && bytes % 32 == 0 && bytes <= 96, "{}", dma);
    // a tighter hardware profile fails closed
    std::fs::create_dir_all(d.join("profiles")).unwrap();
    std::fs::write(d.join("profiles/gbc-sprite.toml"), "dma_bytes_per_frame = 32\n").unwrap();
    let o = pxl_in(&d, &["scene", "z", "--json"]);
    assert_eq!(o.status.code(), Some(1), "{}", stdout(&o));
}

#[test]
fn worst_case_screen_reports_lines_sprites_and_palettes() {
    let d = scene_project("t5worst", 16, "", "");
    let o = pxl_in(&d, &["scene", "z", "--worst", "--png", "o/w.png", "--json"]);
    let j = json_of(&o);
    let w = &j["worst"];
    // player 2 + two objects 2+2 + enemy 2 + ring 1 on the same lines
    assert_eq!(w["per_line"], 9, "{}", w);
    assert_eq!(w["sprites"], 9);
    assert_eq!(w["palettes"], serde_json::json!([0]));
    assert!(o.status.success());
    assert!(d.join("o/w.png").is_file());
    assert_eq!(w["parts"].as_array().unwrap().len(), 5);
    // wide platforms break the 10 per line
    let d = scene_project("t5worst2", 48, "", "");
    let o = pxl_in(&d, &["scene", "z", "--worst", "--json"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(json_of(&o)["worst"]["per_line"].as_u64().unwrap() > 10);
    // options name the parts
    let o = pxl_in(&d, &["scene", "z", "--worst", "--objects", "enemy", "--enemy", "ring", "--json"]);
    assert!(json_of(&o)["worst"]["per_line"].as_u64().unwrap() <= 10, "{}", stdout(&o));
}

#[test]
fn overlays_are_errors_unless_the_category_allows_them() {
    let d = project("t5overlay");
    let rows: Vec<String> = (0..16).map(|y| (0..16).map(|x| if x == 4 && y == 5 { 'R' } else { '2' }).collect()).collect();
    write_part(&d, "e", "category = \"enemy\"\n", &rows);
    write_part(&d, "p", "category = \"player\"\n", &rows);
    let e = pxl_in(&d, &["lint", "e.pxl", "--rules", "overlay"]);
    assert_eq!(e.status.code(), Some(1), "{}", stdout(&e));
    assert!(stdout(&e).contains("overlay layer keys"));
    assert!(pxl_in(&d, &["lint", "p.pxl", "--rules", "overlay"]).status.success());
    // a palette-set part has no overlay layers at all
    write_part(&d, "s", "palette_set = \"s\"\n", &rows);
    assert!(pxl_in(&d, &["lint", "s.pxl", "--rules", "overlay"]).status.success());
}

#[test]
fn delivery_fills_the_template() {
    let d = scene_project("t5deliv", 16, "", "");
    let pkg = d.join("pkg");
    std::fs::create_dir_all(&pkg).unwrap();
    std::fs::write(pkg.join("DELIVERY_TEMPLATE.md"), "# Delivery note\n\n| | |\n|---|---|\n| Studio | |\n| Delivery number | |\n| Date | |\n| Contact | |\n\n## Assets delivered\n\n| ID | Folder | Complete / partial | Notes |\n|---|---|---|---|\n| A-0xx | | | |\n\n## Checker output\n\n```\nChecked … folders: 0 errors, … warnings.\nRESULT: PASS\n```\n\n## Warnings and agreed exceptions (README §6.5)\n\n| ID | File | Exception | Agreed by / date |\n|---|---|---|---|\n| | | | |\n\n## Open questions\n").unwrap();
    std::fs::write(pkg.join("CHECKLIST.md"), "| ID | Folder | Kind |\n|---|---|---|\n| A-003 | `characters/player` | sprite |\n| A-042 | `objects` | sprite |\n").unwrap();
    std::fs::write(d.join("exc.md"), "| Folder | Change | Reason |\n|---|---|---|\n| objects/obj | Platform category | stood on |\n").unwrap();
    std::fs::write(d.join("checker.txt"), "noise\nChecked 12 folders: 0 errors, 3 warnings.\nRESULT: PASS\n").unwrap();
    let m = std::fs::read_to_string(d.join("pxl.toml")).unwrap().replace("player = \"player.pxl\"", "player = \"player.pxl\"").replace("[parts]\n", "out_root = \"out\"\n[parts]\n");
    let m = m.replace("obj = \"obj.pxl\"", "obj = { path = \"obj.pxl\", out = \"objects/obj\" }").replace("player = \"player.pxl\"", "player = { path = \"player.pxl\", out = \"characters/player\" }");
    std::fs::write(d.join("pxl.toml"), m).unwrap();
    let o = pxl_in(&d, &["delivery", "--template", "pkg/DELIVERY_TEMPLATE.md", "--exceptions", "exc.md", "--checker", "checker.txt", "--studio", "Acme", "--number", "7", "--out", "DELIVERY.md"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let t = std::fs::read_to_string(d.join("DELIVERY.md")).unwrap();
    assert!(t.contains("| Studio | Acme |") && t.contains("| Delivery number | 7 |"), "{}", t);
    assert!(t.contains("| A-003 | `characters/player` | partial |"), "{}", t);
    assert!(t.contains("| A-042 | `objects` |"), "{}", t);
    assert!(t.contains("Checked 12 folders: 0 errors, 3 warnings.\nRESULT: PASS") && !t.contains("Checked …"), "{}", t);
    assert!(t.contains("| A-042 | objects/obj | Platform category Reason: stood on |"), "{}", t);
    assert!(t.contains("## Hardware budgets per scene") && t.contains("| z | sprite VRAM |") && t.contains("## Parts"), "{}", t);
    assert!(t.find("## Hardware budgets").unwrap() < t.find("## Open questions").unwrap());
    assert!(!t.contains("| A-0xx |"));
}

// ---------------------------------------------------------------------------------------------- T6: VRAM rebalancing

fn tile_set(d: &Path, name: &str, n_tiles: usize) {
    // n_tiles distinct 8x8 tiles in palette 0 (keys 1 2 3 a), 16 per row
    let rows_n = (n_tiles + 15) / 16;
    let mut g = vec![vec![b'a'; 128]; rows_n * 8];
    for i in 0..n_tiles {
        for y in 0..8 {
            for x in 0..8 {
                let v = (i * 31 + x * 7 + y * 13 + (i / 4) * x * y) % 4;
                g[(i / 16) * 8 + y][(i % 16) * 8 + x] = b"123a"[v];
            }
        }
    }
    let rows: Vec<String> = g.iter().map(|r| String::from_utf8(r.clone()).unwrap()).collect();
    let text = format!("name = \"{}\"\nkind = \"tileset\"\nsize = [128, {}]\norigin = [0, 0]\ntile = [8, 8]\ntemplate = \"templates/t.toml\"\n\n[[frames]]\nname = \"blocks\"\ngrid = \"\"\"\n{}\n\"\"\"\n", name, rows.len(), rows.join("\n"));
    std::fs::write(d.join(format!("{}.pxl", name)), text).unwrap();
}

fn vram_row<'a>(j: &'a serde_json::Value, item: &str) -> &'a serde_json::Value {
    j["scene"]["rows"].as_array().unwrap().iter().find(|r| r["item"] == item).unwrap_or_else(|| panic!("no row {} in {}", item, j))
}

#[test]
fn unused_bg_tiles_lend_sprite_cells_to_the_scene() {
    let d = scene_project("t6share", 16, "tileset = \"blocks\"", "");
    tile_set(&d, "blocks", 64);
    let m = std::fs::read_to_string(d.join("pxl.toml")).unwrap().replace("obj = \"obj.pxl\"\n", "obj = \"obj.pxl\"\nblocks = \"blocks.pxl\"\n");
    std::fs::write(d.join("pxl.toml"), m).unwrap();
    // a project profile with a tiny default sprite VRAM: the sum (3 resident + 2 streamed = 5) breaks it without sharing
    std::fs::create_dir_all(d.join("profiles")).unwrap();
    std::fs::write(d.join("profiles/gbc-sprite.toml"), "sprite_cells = 4\nvram_share = false\n").unwrap();
    let j = json_of(&pxl_in(&d, &["scene", "z", "--json"]));
    assert_eq!(vram_row(&j, "sprite VRAM")["ok"], false, "{}", j);
    // with sharing the BG slack (348 - used) / 2 cells are added, up to the addressable OBJ limit
    std::fs::write(d.join("profiles/gbc-sprite.toml"), "sprite_cells = 4\nvram_share = true\n").unwrap();
    let j = json_of(&pxl_in(&d, &["scene", "z", "--json"]));
    let v = &j["scene"]["vram"];
    let used = v["bg_used"].as_u64().unwrap();
    assert!(used > 0 && used < 100, "{}", v);
    let slack = 348 - used;
    assert_eq!(v["bg_slack"], slack);
    assert_eq!(v["extra_sprite_cells"].as_u64().unwrap(), (slack / 2).min(256 - 4), "{}", v);
    assert_eq!(v["effective_sprite_cells"].as_u64().unwrap(), 4 + (slack / 2).min(252));
    assert_eq!(vram_row(&j, "sprite VRAM")["ok"], true);
    assert!(j["scene"]["warnings"].as_array().unwrap().iter().any(|w| w.as_str().unwrap().contains("differs from the operator's default")), "{}", j["scene"]["warnings"]);
    // a resident budget is validated against the effective total
    let m = std::fs::read_to_string(d.join("pxl.toml")).unwrap().replace("palette_set = \"s\"\n", "palette_set = \"s\"\nresident_budget = 1000\n");
    std::fs::write(d.join("pxl.toml"), &m).unwrap();
    let o = pxl_in(&d, &["scene", "z", "--json"]);
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(vram_row(&json_of(&o), "resident budget")["ok"], false);
    std::fs::write(d.join("pxl.toml"), m.replace("resident_budget = 1000", "resident_budget = 40")).unwrap();
    let j = json_of(&pxl_in(&d, &["scene", "z", "--json"]));
    assert_eq!(vram_row(&j, "resident budget")["ok"], true, "{}", j);
    // the addressable OBJ limit caps the lending
    std::fs::write(d.join("profiles/gbc-sprite.toml"), "sprite_cells = 4\nvram_share = true\nobj_tiles_max = 20\n").unwrap();
    let j = json_of(&pxl_in(&d, &["scene", "z", "--json"]));
    assert_eq!(j["scene"]["vram"]["effective_sprite_cells"], 10, "{}", j["scene"]["vram"]);
    // the total never exceeds 768 tiles in the report and the delivery lists the table
    assert!(j["scene"]["vram"]["total_tiles"].as_u64().unwrap() <= 768);
}

#[test]
fn delivery_lists_the_vram_table_and_the_changed_split() {
    let d = scene_project("t6deliv", 16, "tileset = \"blocks\"", "");
    tile_set(&d, "blocks", 32);
    let m = std::fs::read_to_string(d.join("pxl.toml")).unwrap().replace("obj = \"obj.pxl\"\n", "obj = \"obj.pxl\"\nblocks = \"blocks.pxl\"\n");
    std::fs::write(d.join("pxl.toml"), m).unwrap();
    std::fs::write(d.join("T.md"), "# Delivery note\n\n## Open questions\n").unwrap();
    let o = pxl_in(&d, &["delivery", "--template", "T.md", "--out", "D.md"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let t = std::fs::read_to_string(d.join("D.md")).unwrap();
    assert!(t.contains("### VRAM per scene") && t.contains("| z | "), "{}", t);
    assert!(t.contains("differs from the operator's default"), "{}", t);
}

// ---------------------------------------------------------------------------------------------- T7

fn eight_palette_tiles(d: &Path, name: &str, header: &str) {
    let mut g: Vec<Vec<u8>> = vec![vec![b'a'; 128]; 8];
    for (i, set) in ["123a", "b2c3", "bcda", "12bc", "1d2a", "3bcd", "13bd", "2c3a"].iter().enumerate() {
        let k: Vec<u8> = set.bytes().collect();
        for y in 0..8 {
            for x in 0..8 {
                g[y][i * 8 + x] = k[(x + y * 3) % 4];
            }
        }
    }
    let rows: Vec<String> = g.iter().map(|r| String::from_utf8(r.clone()).unwrap()).collect();
    std::fs::write(d.join(format!("{}.pxl", name)), format!("name = \"{}\"\nkind = \"tileset\"\nsize = [128, 8]\norigin = [0, 0]\ntile = [8, 8]\ntemplate = \"templates/t.toml\"\n{}\n[[frames]]\nname = \"blocks\"\ngrid = \"\"\"\n{}\n\"\"\"\n", name, header, rows.join("\n"))).unwrap();
}

#[test]
fn bg_palettes_are_reserved_only_where_a_window_needs_them() {
    let d = scene_project("t7res", 16, "tileset = \"blocks\"", "");
    eight_palette_tiles(&d, "blocks", "bg_reserved = []\n");
    let m = std::fs::read_to_string(d.join("pxl.toml")).unwrap().replace("obj = \"obj.pxl\"\n", "obj = \"obj.pxl\"\nblocks = \"blocks.pxl\"\n");
    std::fs::write(d.join("pxl.toml"), &m).unwrap();
    // a title / ending scene has no window: all 8 BG palettes are usable
    let j = json_of(&pxl_in(&d, &["scene", "z", "--json"]));
    let pal = vram_row(&j, "BG palettes");
    assert_eq!((pal["limit"].as_str(), pal["ok"].as_bool()), (Some("8"), Some(true)), "{}", pal);
    // with a window part palette 7 is reserved
    let hud: Vec<String> = (0..8).map(|y| (0..128).map(|x| ['b', 'c', 'd'][(x + y) % 3]).collect()).collect();
    std::fs::write(d.join("hud.pxl"), format!("name = \"hud\"\nkind = \"tileset\"\nsize = [128, 8]\norigin = [0, 0]\ntile = [8, 8]\ntemplate = \"templates/t.toml\"\nbg_palette = 7\n\n[[frames]]\nname = \"hud_tiles\"\ngrid = \"\"\"\n{}\n\"\"\"\n", hud.join("\n"))).unwrap();
    let m2 = m.replace("blocks = \"blocks.pxl\"\n", "blocks = \"blocks.pxl\"\nhud = \"hud.pxl\"\n").replace("tileset = \"blocks\"", "tileset = \"blocks\"\nwindow = [\"hud\"]");
    std::fs::write(d.join("pxl.toml"), &m2).unwrap();
    let j = json_of(&pxl_in(&d, &["scene", "z", "--json"]));
    let pal = vram_row(&j, "BG palettes");
    assert_eq!((pal["limit"].as_str(), pal["ok"].as_bool()), (Some("7"), Some(false)), "{}", pal);
    // the scene can override
    std::fs::write(d.join("pxl.toml"), m2.replace("window = [\"hud\"]", "window = [\"hud\"]\nbg_reserved = [7]").replace("bg_reserved = [7]", "bg_reserved = [6, 7]")).unwrap();
    let j = json_of(&pxl_in(&d, &["scene", "z", "--json"]));
    assert_eq!(vram_row(&j, "BG palettes")["limit"], "6", "{}", j);
    // the part's own lint follows the part key / profile
    let lint = pxl_in(&d, &["lint", "blocks.pxl", "--rules", "tile_palettes"]);
    assert!(lint.status.success(), "bg_reserved = [] on the part: {}", stdout(&lint));
    let t = std::fs::read_to_string(d.join("blocks.pxl")).unwrap().replace("bg_reserved = []\n", "");
    std::fs::write(d.join("blocks.pxl"), t).unwrap();
    assert_eq!(pxl_in(&d, &["lint", "blocks.pxl", "--rules", "tile_palettes"]).status.code(), Some(1));
}

const SET_FILES: [(&str, &str); 2] = [
    ("global", "[[slots]]\nslot = 0\ncolors = [\"#182078\", \"#4850c0\", \"#f8b890\"]\n[[slots]]\nslot = 7\nvariants = { boss = [\"#581810\", \"#e00000\", \"#f8b090\"], fire = [\"#c81000\", \"#f8b090\", \"#f8f8d0\"] }\n"),
    ("zone_1", "include = \"global\"\n[[slots]]\nslot = 4\ncolors = [\"#101030\", \"#6868d8\", \"#f8f8f8\"]\n"),
];

#[test]
fn template_add_set_appends_every_set_colour_in_place() {
    let d = project("t7add");
    for (n, t) in SET_FILES {
        std::fs::write(d.join(format!("palettes/{}.toml", n)), t).unwrap();
    }
    let orig = std::fs::read_to_string(d.join("templates/t.toml")).unwrap();
    let o = pxl_in(&d, &["template", "add-set", "templates/t.toml", "--set", "zone_1", "--json"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let j = json_of(&o);
    // 3 (global slot 0) + 6 (two variants, one colour shared) + 3 (slot 4) = 11 distinct, minus 1 shared #f8f8f8? and the ones the template has
    let added = j["added"].as_array().unwrap();
    assert!(!added.is_empty());
    let t = std::fs::read_to_string(d.join("templates/t.toml")).unwrap();
    assert!(t.starts_with(&orig), "everything before the new table is byte-identical");
    assert!(t.contains("[layers.set]") && t.contains("role = \"set_581810\""), "{}", t);
    // existing colours (#182078, #4850c0, #f8b890, #f8f8f8 are keys of the template) are skipped; keys are unique
    assert!(!t[orig.len()..].contains("set_182078") && !t[orig.len()..].contains("set_4850c0"));
    let tpl = agentsprite::template::load(&d.join("templates/t.toml")).unwrap();
    let keys: Vec<u8> = tpl.keys().map(|k| k.key).collect();
    let uniq: std::collections::BTreeSet<_> = keys.iter().collect();
    assert_eq!(keys.len(), uniq.len());
    // a second run, also with another set, changes nothing that exists
    let before = std::fs::read_to_string(d.join("templates/t.toml")).unwrap();
    let o = pxl_in(&d, &["template", "add-set", "templates/t.toml", "--set", "zone_1", "--set", "global"]);
    assert!(o.status.success(), "{}", stdout(&o));
    assert_eq!(std::fs::read_to_string(d.join("templates/t.toml")).unwrap(), before);
    // an existing [layers.set] is edited where it is, even before other tables
    let moved = before.replace("[tiles]\nair = [\"a\"]\n", "").replace(&format!("{}", &before[before.find("# keys added").unwrap()..]), "");
    let with_set_first = format!("{}\n[layers.set]\n\"Z\" = {{ role = \"set_000000\", color = \"#000000\" }}\n\n[tiles]\nair = [\"a\"]\n", moved.trim_end());
    std::fs::write(d.join("templates/t.toml"), &with_set_first).unwrap();
    let o = pxl_in(&d, &["template", "add-set", "templates/t.toml", "--set", "zone_1"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let t2 = std::fs::read_to_string(d.join("templates/t.toml")).unwrap();
    assert_eq!(t2.matches("[layers.set]").count(), 1);
    assert!(t2.find("set_581810").unwrap() < t2.find("[tiles]").unwrap(), "{}", t2);
    assert!(t2.ends_with("[tiles]\nair = [\"a\"]\n"));
    assert!(pxl_in(&d, &["template", "add-set", "templates/t.toml", "--set", "nope"]).status.code().is_some_and(|c| c != 0));
}

#[test]
fn palette_fit_adds_every_set_colour_as_keys_by_default() {
    let d = project("t7fit");
    for (n, t) in SET_FILES {
        std::fs::write(d.join(format!("palettes/{}.toml", n)), t).unwrap();
    }
    write_part(&d, "p", "palette_set = \"zone_1\"\n", &solid(8, 16));
    let o = pxl_in(&d, &["palette", "fit", "p.pxl", "--apply", "--no-all-keys", "--agent", "A1"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    assert!(!std::fs::read_to_string(d.join("templates/t.toml")).unwrap().contains("set_581810"));
    let o = pxl_in(&d, &["palette", "fit", "p.pxl", "--apply", "--agent", "A1"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let t = std::fs::read_to_string(d.join("templates/t.toml")).unwrap();
    for c in ["581810", "e00000", "c81000", "101030", "6868d8"] {
        assert!(t.contains(&format!("set_{}", c)), "{} missing in {}", c, t);
    }
}

// ---------------------------------------------------------------------------------------------- T8

#[test]
fn budget_exempt_parts_are_listed_not_failed() {
    let d = scene_project("t8ex", 16, "", "");
    write_part(&d, "dbg", "palette_set = \"s\"\n", &solid(8, 16));
    let m = std::fs::read_to_string(d.join("pxl.toml")).unwrap().replacen("ring = \"ring.pxl\"\n", "ring = \"ring.pxl\"\ndbg = \"dbg.pxl\"\n", 1);
    std::fs::write(d.join("pxl.toml"), &m).unwrap();
    for p in ["player", "obj", "enemy", "ring", "dbg"] {
        assert!(pxl_in(&d, &["export", &format!("{}.pxl", p)]).status.success());
    }
    // without the key the part is an error
    let j = json_of(&pxl_in(&d, &["check", "--json"]));
    assert!(j["hardware"].as_array().unwrap().iter().any(|x| x.as_str().unwrap().contains("dbg is in no scene")), "{}", j["hardware"]);
    // with it: an exempt line, no failure
    let t = std::fs::read_to_string(d.join("dbg.pxl")).unwrap().replace("palette_set = \"s\"\n", "palette_set = \"s\"\nbudget_exempt = \"debug mode barriers\"\n");
    std::fs::write(d.join("dbg.pxl"), t).unwrap();
    assert!(pxl_in(&d, &["export", "dbg.pxl"]).status.success());
    let o = pxl_in(&d, &["check"]);
    assert!(stdout(&o).contains("exempt: dbg: debug mode barriers"), "{}", stdout(&o));
    assert!(o.status.success(), "{}", stdout(&o));
    let j = json_of(&pxl_in(&d, &["check", "--json"]));
    assert_eq!(j["exempt"][0]["reason"], "debug mode barriers");
    assert!(j["exempt_warnings"].as_array().unwrap().is_empty());
    // header round trip
    let part = agentsprite::pxl::load(&d.join("dbg.pxl")).unwrap();
    let txt = agentsprite::pxl::to_string(&part);
    assert!(txt.contains("budget_exempt = \"debug mode barriers\""));
    // delivery lists it with its reason
    std::fs::write(d.join("T.md"), "# D\n\n## Warnings and agreed exceptions\n\n| ID | File | Exception | Agreed by / date |\n|---|---|---|---|\n| | | | |\n\n## Open questions\n").unwrap();
    assert!(pxl_in(&d, &["delivery", "--template", "T.md", "--out", "D.md"]).status.success());
    assert!(std::fs::read_to_string(d.join("D.md")).unwrap().contains("Budget exempt: debug mode barriers"));
    // contradictory: exempt and listed in a scene group
    let m2 = std::fs::read_to_string(d.join("pxl.toml")).unwrap().replace("common = { budget = 56, parts = [\"ring\"] }", "common = { budget = 56, parts = [\"ring\", \"dbg\"] }");
    std::fs::write(d.join("pxl.toml"), m2).unwrap();
    let j = json_of(&pxl_in(&d, &["check", "--json"]));
    assert_eq!(j["exempt_warnings"].as_array().unwrap().len(), 1, "{}", j["exempt_warnings"]);
    assert!(j["exempt_warnings"][0].as_str().unwrap().contains("listed in scene group"));
}
