//! M5 T2: palette fit, tile memory budget, cell dedupe, show --cells, aseprite of palette-set parts.
mod e2e_common;

use agentsprite::cellmem;
use agentsprite::{ase, pxl};
use e2e_common::*;
use std::path::{Path, PathBuf};

const TPL: &str = r##"name = "t"
[layers.base]
"." = { role = "transparent" }
"1" = { role = "outline", color = "#182078" }
"2" = { role = "body", color = "#4850c0" }
"x" = { role = "body_drift", color = "#4850c8" }
"R" = { role = "red", color = "#f80000" }
"r" = { role = "red_drift", color = "#f00000" }
"W" = { role = "white", color = "#f8f8f8" }
"w" = { role = "white_drift", color = "#f0f0f8" }
"##;

const SET: &str = r##"[[slots]]
slot = 0
name = "body"
colors = ["#182078", "#4850c0", "#f8b890"]
[[slots]]
slot = 1
name = "red"
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

fn part_text(name: &str, header: &str, w: usize, h: usize, frames: &[(&str, Vec<String>)]) -> String {
    let mut s = format!("name = \"{}\"\nsize = [{}, {}]\norigin = [0, 0]\ntemplate = \"templates/t.toml\"\n{}\n", name, w, h, header);
    for (n, rows) in frames {
        assert_eq!(rows.len(), h);
        s.push_str(&format!("\n[[frames]]\nname = \"{}\"\ngrid = \"\"\"\n{}\n\"\"\"\n", n, rows.join("\n")));
    }
    s
}

fn write_part(d: &Path, name: &str, header: &str, w: usize, h: usize, frames: &[(&str, Vec<String>)]) -> PathBuf {
    let p = d.join(format!("{}.pxl", name));
    std::fs::write(&p, part_text(name, header, w, h, frames)).unwrap();
    p
}

/// 16x16: left half outline/body with drifted body pixels, right half red and white with drift.
fn drifted() -> Vec<String> {
    (0..16)
        .map(|y| {
            (0..16)
                .map(|x| {
                    if x < 8 {
                        if y == 0 || y == 15 || x == 0 { '1' } else if (x + y) % 3 == 0 { 'x' } else { '2' }
                    } else if (x + y) % 5 == 0 { 'W' } else if (x + y) % 5 == 1 { 'w' } else if (x * y) % 4 == 0 { 'R' } else { 'r' }
                })
                .collect()
        })
        .collect()
}

fn colours_of(p: &Path) -> std::collections::BTreeSet<String> {
    let part = pxl::load(p).unwrap();
    let tpl = agentsprite::template::load_for(&part).unwrap();
    let mut s = std::collections::BTreeSet::new();
    for f in &part.frames {
        for &k in &f.grid.cells {
            if k != b'.' {
                s.insert(agentsprite::color::to_hex(agentsprite::lint::effective_color(&part, &tpl, k).unwrap()));
            }
        }
    }
    s
}

#[test]
fn fit_keeps_identity_colours_is_deterministic_and_undoable() {
    let d = project("fit");
    let p = write_part(&d, "f", "palette_set = \"s\"\n", 16, 16, &[("a", drifted())]);
    let before = std::fs::read_to_string(&p).unwrap();
    let dry1 = json_of(&pxl_in(&d, &["palette", "fit", "f.pxl", "--json"]));
    let dry2 = json_of(&pxl_in(&d, &["palette", "fit", "f.pxl", "--json"]));
    assert_eq!(dry1, dry2, "deterministic");
    assert_eq!(std::fs::read_to_string(&p).unwrap(), before, "a dry run writes nothing");
    let fr = &dry1["fit"]["frames"][0];
    assert!(fr["changed"].as_u64().unwrap() > 0);
    assert_eq!(fr["layout"]["invalid"], 0);
    assert_eq!(fr["folded_cells"], 0, "drift is not a fold: {}", fr);
    // compare page
    let o = pxl_in(&d, &["palette", "fit", "f.pxl", "--png", "cmp/fit.png", "--zoom", "4"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    assert!(d.join("cmp/fit.png").is_file());
    // apply
    let o = pxl_in(&d, &["palette", "fit", "f.pxl", "--apply", "--agent", "A1"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let cols = colours_of(&p);
    let allowed: std::collections::BTreeSet<String> = ["#182078", "#4850c0", "#f80000", "#f8f8f8", "#900000"].iter().map(|s| s.to_string()).collect();
    assert!(cols.is_subset(&allowed), "{:?}", cols);
    assert!(cols.contains("#f8f8f8") && cols.contains("#f80000") && cols.contains("#4850c0") && cols.contains("#182078"), "identity colours survive: {:?}", cols);
    let lint = pxl_in(&d, &["lint", "f.pxl", "--rules", "palette_set,cell_palette,hw_limit"]);
    assert!(lint.status.success(), "{}", stdout(&lint));
    // idempotent: a second fit changes nothing
    let again = json_of(&pxl_in(&d, &["palette", "fit", "f.pxl", "--json"]));
    assert_eq!(again["fit"]["changed"], 0);
    // undo restores the pixels
    let grid_before = pxl::parse_str(&before, &p).unwrap().frames[0].grid.clone();
    let o = pxl_in(&d, &["undo", "f.pxl", "--agent", "A1"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    assert_eq!(pxl::load(&p).unwrap().frames[0].grid, grid_before);
}

#[test]
fn fit_moves_the_layout_before_folding_and_adds_template_keys() {
    let d = project("fit2");
    // a template without the red/white keys of the set: R/W colours map to the set's, keys get added
    std::fs::write(d.join("templates/t.toml"), TPL.replace("\"W\" = { role = \"white\", color = \"#f8f8f8\" }\n", "")).unwrap();
    let rows: Vec<String> = (0..16).map(|y| (0..16).map(|x| if x < 8 { '2' } else if y % 2 == 0 { 'R' } else { 'r' }).collect()).collect();
    let p = write_part(&d, "g", "palette_set = \"s\"\n", 16, 16, &[("a", rows)]);
    let o = pxl_in(&d, &["palette", "fit", "g.pxl", "--apply", "--json"]);
    let j = json_of(&o);
    assert_eq!(j["fit"]["frames"][0]["folded_cells"], 0, "{}", j);
    assert_eq!(j["fit"]["frames"][0]["layout"]["invalid"], 0);
    let _ = p;
}

#[test]
fn fit_respects_locks_and_the_approved_guard() {
    let d = project("fitlock");
    let p = write_part(&d, "l", "palette_set = \"s\"\n", 16, 16, &[("a", drifted())]);
    let text = std::fs::read_to_string(&p).unwrap().replace("name = \"a\"\n", "name = \"a\"\nlocked = true\n");
    std::fs::write(&p, &text).unwrap();
    let j = json_of(&pxl_in(&d, &["palette", "fit", "l.pxl", "--apply", "--json"]));
    assert_eq!(j["fit"]["skipped_locked"][0], "a");
    assert_eq!(std::fs::read_to_string(&p).unwrap(), text);
    let j = json_of(&pxl_in(&d, &["palette", "fit", "l.pxl", "--include-locked", "--json"]));
    assert!(j["fit"]["changed"].as_u64().unwrap() > 0);
}

// ---------------------------------------------------------------------------------------------- budget

/// 8x16 frame with a block of `rows` rows of body colour, at the top or (flipped) at the bottom.
fn block(rows: usize, bottom: bool) -> Vec<String> {
    (0..16).map(|y| { let on = if bottom { y >= 16 - rows } else { y < rows }; if on { "22222222".to_string() } else { "........".to_string() } }).collect()
}

fn scene_project(tag: &str, frames: &[(&str, Vec<String>)], budget: u32) -> PathBuf {
    let d = project(tag);
    write_part(&d, "m", "palette_set = \"s\"\n", 8, 16, frames);
    std::fs::write(
        d.join("pxl.toml"),
        format!("[parts]\nm = \"m.pxl\"\n\n[scene.z]\npalette_set = \"s\"\ngroups = {{ objects = {{ budget = {}, parts = [\"m\"] }}, player = {{ streamed = true, per_frame = 2, parts = [\"m\"] }} }}\n", budget),
    )
    .unwrap();
    d
}

#[test]
fn budget_counts_a_mirrored_cell_once() {
    // top block of 4 rows and the same block at the bottom (Y flip) are one tile; 6 rows is another
    let d = scene_project("bud", &[("a", block(4, false)), ("b", block(4, true)), ("c", block(6, false))], 2);
    let j = json_of(&pxl_in(&d, &["budget", "z", "--json"]));
    let g = &j["scenes"][0]["groups"][0];
    assert_eq!(g["total"], 2, "{}", j);
    assert_eq!(g["over"], false);
    let s = &j["scenes"][0]["groups"][1];
    assert_eq!(s["streamed"], true);
    assert_eq!(s["total"], 1, "one unique cell per frame");
    // a budget of 1 fails, names the group and `check` carries tile_budget
    let d2 = scene_project("bud2", &[("a", block(4, false)), ("b", block(4, true)), ("c", block(6, false))], 1);
    let o = pxl_in(&d2, &["budget", "z"]);
    assert_eq!(o.status.code(), Some(1), "{}", stdout(&o));
    assert!(stdout(&o).contains("OVER"));
    let c = json_of(&pxl_in(&d2, &["check", "--json"]));
    assert_eq!(c["tile_budget"].as_array().unwrap().len(), 1, "{}", c);
    assert_eq!(c["tile_budget"][0]["rule"], "tile_budget");
}

#[test]
fn canonical_patterns_ignore_flips() {
    let a = block(4, false).concat().bytes().map(|b| if b == b'2' { 2 } else { 0 }).collect::<Vec<u8>>();
    let b = block(4, true).concat().bytes().map(|b| if b == b'2' { 2 } else { 0 }).collect::<Vec<u8>>();
    assert_eq!(cellmem::canonical(&a, 8, 16).0, cellmem::canonical(&b, 8, 16).0);
    assert_ne!(cellmem::canonical(&a, 8, 16).0, cellmem::canonical(&block(5, false).concat().bytes().map(|b| if b == b'2' { 2 } else { 0 }).collect::<Vec<u8>>(), 8, 16).0);
}

// ---------------------------------------------------------------------------------------------- dedupe

#[test]
fn dedupe_reaches_max_and_flipped_duplicates_are_free() {
    let frames: Vec<(&str, Vec<String>)> = vec![("a", block(4, false)), ("b", block(5, false)), ("c", block(6, false)), ("d", block(7, false)), ("e", block(4, true))];
    let d = scene_project("ded", &frames, 10);
    let j = json_of(&pxl_in(&d, &["cells", "dedupe", "m.pxl", "--max", "4", "--json"]));
    assert_eq!(j["dedupe"]["before"], 4, "e is a flipped copy of a: {}", j);
    assert_eq!(j["dedupe"]["after"], 4);
    assert_eq!(j["dedupe"]["merges"].as_array().unwrap().len(), 0, "flipped duplicates cost nothing");
    // reach 2
    let j = json_of(&pxl_in(&d, &["cells", "dedupe", "m.pxl", "--max", "2", "--json"]));
    assert_eq!(j["dedupe"]["after"], 2);
    let merges = j["dedupe"]["merges"].as_array().unwrap();
    assert_eq!(merges.len(), 2);
    assert!(merges.iter().all(|m| m["px_diff"].as_u64().unwrap() > 0));
    let before = std::fs::read_to_string(d.join("m.pxl")).unwrap();
    // apply
    let o = pxl_in(&d, &["cells", "dedupe", "m.pxl", "--max", "2", "--apply", "--agent", "A1"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let b = json_of(&pxl_in(&d, &["budget", "z", "--json"]));
    assert!(b["scenes"][0]["groups"][0]["total"].as_u64().unwrap() <= 2, "{}", b);
    assert!(d.join("a.cells.json").is_file() || d.join("c.cells.json").is_file());
    // undo restores the pixels
    let o = pxl_in(&d, &["undo", "m.pxl", "--agent", "A1"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let (x, y) = (pxl::parse_str(&before, &d.join("m.pxl")).unwrap(), pxl::load(&d.join("m.pxl")).unwrap());
    for (fa, fb) in x.frames.iter().zip(&y.frames) {
        assert_eq!(fa.grid, fb.grid, "frame {}", fa.name);
    }
}

// ---------------------------------------------------------------------------------------------- editing, reduce, ase

#[test]
fn show_cells_and_edit_warning() {
    let d = project("showc");
    let rows: Vec<String> = (0..16).map(|_| "22222222".to_string()).collect();
    write_part(&d, "s", "palette_set = \"s\"\n", 8, 16, &[("a", rows)]);
    let o = pxl_in(&d, &["show", "s.pxl", "a", "--cells"]);
    let t = stdout(&o);
    assert!(o.status.success() && t.contains("cells of s/a") && t.contains("slot 0 (body)"), "{}", t);
    // an edit that puts a white pixel into a body cell needs a second slot: the echo warns
    let o = pxl_in(&d, &["set", "s.pxl", "a", "3,5=W", "--agent", "A1"]);
    let t = stdout(&o);
    assert!(t.contains("warning") && t.contains("cell 0,0"), "{}", t);
}

#[test]
fn export_ase_has_the_32_entry_palette_and_slot_indexes() {
    let d = project("ase");
    let rows: Vec<String> = (0..16).map(|y| (0..16).map(|x| if x < 8 { '2' } else if y < 8 { 'R' } else { 'W' }).collect()).collect();
    write_part(&d, "e", "palette_set = \"s\"\ncategory = \"enemy\"\n", 16, 16, &[("a", rows.clone()), ("b", rows)]);
    let o = pxl_in(&d, &["export", "e.pxl"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let bytes = std::fs::read(d.join("e_export/e.aseprite")).unwrap();
    assert!(ase::validate(&bytes).is_empty(), "{:?}", ase::validate(&bytes));
    let doc = ase::read(&bytes).unwrap();
    assert_eq!(doc.mode, ase::ColorMode::Indexed);
    assert_eq!(doc.num_colors, 32);
    let idx = ase::render_layer_indices(&doc, 0, 0).unwrap();
    let (_, _, png_idx, _, _) = agentsprite::png::read_indexed(&d.join("e_export/a.png")).unwrap();
    assert_eq!(idx, png_idx, "the aseprite holds the shipped indexes");
    assert!(idx.iter().any(|i| *i / 4 == 1));
}

#[test]
fn reduce_pass_chooses_slots_from_the_set() {
    // fit_to_plan is the final pass of reduce: drifted keys become the set's colours where the template has keys
    let d = project("red");
    let p = write_part(&d, "r", "palette_set = \"s\"\n", 16, 16, &[("a", drifted())]);
    let part = pxl::load(&p).unwrap();
    let tpl = agentsprite::template::load_for(&part).unwrap();
    let mut g = part.frames[0].grid.clone();
    agentsprite::fit::fit_to_plan(&part, &tpl, &mut g).unwrap();
    assert!(!g.cells.contains(&b'x') && !g.cells.contains(&b'w') && !g.cells.contains(&b'r'));
    assert!(g.cells.contains(&b'2') && g.cells.contains(&b'W') && g.cells.contains(&b'R'));
}

#[test]
fn a_part_on_an_included_set_fits_the_scene() {
    let d = project("scn");
    std::fs::write(d.join("palettes/g.toml"), SET).unwrap();
    std::fs::write(d.join("palettes/z.toml"), "include = \"g\"\n[[slots]]\nslot = 2\ncolors = [\"#686000\", \"#f8e800\", \"#f8f8f8\"]\n").unwrap();
    std::fs::write(d.join("palettes/bad.toml"), SET.replace("#182078", "#182070")).unwrap();
    write_part(&d, "m", "palette_set = \"g\"\n", 8, 16, &[("a", block(4, false))]);
    write_part(&d, "n", "palette_set = \"bad\"\n", 8, 16, &[("a", block(4, false))]);
    std::fs::write(d.join("pxl.toml"), "[parts]\nm = \"m.pxl\"\nn = \"n.pxl\"\n\n[scene.z]\npalette_set = \"z\"\ngroups = { a = { budget = 5, parts = [\"m\"] }, b = { budget = 5, parts = [\"n\"] } }\n").unwrap();
    let j = json_of(&pxl_in(&d, &["budget", "z", "--json"]));
    let g = j["scenes"][0]["groups"].as_array().unwrap();
    assert_eq!(g[0]["problems"].as_array().unwrap().len(), 0, "{}", j);
    assert_eq!(g[1]["problems"].as_array().unwrap().len(), 1, "{}", j);
}

#[test]
fn only_the_slots_a_part_uses_must_agree_with_the_scene_and_include_brings_slot_7() {
    let d = project("slot7");
    let dynamic = "[[slots]]\nslot = 7\nname = \"dynamic\"\nvariants = { boss = [\"#581810\", \"#e00000\", \"#f8b090\"], fire = [\"#c81000\", \"#f8b090\", \"#f8f8d0\"] }\n";
    std::fs::write(d.join("palettes/glob.toml"), format!("{}{}", SET, dynamic)).unwrap();
    // zone includes the global set: slot 7 comes along (production: zone_1 includes global)
    std::fs::write(d.join("palettes/zoneinc.toml"), "include = \"glob\"\n[[slots]]\nslot = 4\ncolors = [\"#101030\", \"#6868d8\", \"#f8f8f8\"]\n").unwrap();
    // a zone that has no slot 7 at all
    std::fs::write(d.join("palettes/zoneno7.toml"), format!("{}[[slots]]\nslot = 4\ncolors = [\"#101030\", \"#6868d8\", \"#f8f8f8\"]\n", SET)).unwrap();
    let set = agentsprite::cells::load_set("zoneinc", &d).unwrap();
    assert!(set.slots[7].is_some(), "include pulls the dynamic slot");
    write_part(&d, "m", "palette_set = \"glob\"\n", 8, 16, &[("a", block(4, false))]);
    for (scene, want) in [("zoneinc", 0), ("zoneno7", 0)] {
        std::fs::write(d.join("pxl.toml"), format!("[parts]\nm = \"m.pxl\"\n\n[scene.z]\npalette_set = \"{}\"\ngroups = {{ a = {{ budget = 5, parts = [\"m\"] }} }}\n", scene)).unwrap();
        let j = json_of(&pxl_in(&d, &["budget", "z", "--json"]));
        assert_eq!(j["scenes"][0]["groups"][0]["problems"].as_array().unwrap().len(), want, "{} {}", scene, j);
    }
    // a part that really uses slot 7 does not fit a scene without it
    let uses7 = "name = \"u\"\nsize = [8, 16]\norigin = [0, 0]\ntemplate = \"templates/t.toml\"\npalette_set = \"glob\"\nvariant = \"boss\"\nslots = [7]\n\n[[frames]]\nname = \"a\"\ngrid = \"\"\"\n".to_string() + &vec!["xxxxxxxx"; 16].join("\n") + "\n\"\"\"\n";
    std::fs::write(d.join("templates/t.toml"), format!("{}\"x\" = {{ role = \"boss\", color = \"#e00000\" }}\n", TPL)).unwrap();
    std::fs::write(d.join("u.pxl"), uses7).unwrap();
    std::fs::write(d.join("pxl.toml"), "[parts]\nu = \"u.pxl\"\n\n[scene.z]\npalette_set = \"zoneno7\"\ngroups = { a = { budget = 5, parts = [\"u\"] } }\n").unwrap();
    let j = json_of(&pxl_in(&d, &["budget", "z", "--json"]));
    assert_eq!(j["scenes"][0]["groups"][0]["problems"].as_array().unwrap().len(), 1, "{}", j);
}

// ---------------------------------------------------------------------------------------------- overlapping cells

fn pole_project(tag: &str) -> (PathBuf, PathBuf) {
    let d = project(tag);
    std::fs::write(d.join("templates/t.toml"), format!("{}\"G\" = {{ role = \"gold\", color = \"#f8e800\" }}\n\"P\" = {{ role = \"pole\", color = \"#909098\" }}\n", TPL)).unwrap();
    std::fs::write(
        d.join("palettes/s.toml"),
        "[[slots]]\nslot = 0\ncolors = [\"#182078\", \"#4850c0\", \"#f8b890\"]\n[[slots]]\nslot = 2\ncolors = [\"#686000\", \"#f8e800\", \"#f8f8f8\"]\n[[slots]]\nslot = 3\ncolors = [\"#484848\", \"#909098\", \"#e0e0e8\"]\n",
    )
    .unwrap();
    // 8x24: a gold base from row 8 down, a grey pole in columns 2..5 from row 0 down to row 11 (over the base's top rows)
    let rows: Vec<String> = (0..24)
        .map(|y| (0..8).map(|x| if y < 12 && (2..6).contains(&x) { 'P' } else if y >= 8 { 'G' } else { '.' }).collect())
        .collect();
    let p = write_part(&d, "pole", "palette_set = \"s\"\n", 8, 24, &[("a", rows)]);
    (d, p)
}

#[test]
fn authored_cells_may_overlap_a_pole_over_a_base() {
    let (d, p) = pole_project("overlap");
    // without a layout the search needs 2 cells too, but it cannot make the pole cell overlap; author it: base first, pole over it
    std::fs::write(d.join("a.cells.json"), "[{\"x\":0,\"y\":8,\"slot\":2},{\"x\":0,\"y\":-4,\"slot\":3}]").unwrap();
    let j = json_of(&pxl_in(&d, &["cells", "pole.pxl", "a", "--json", "--png", "o/pole.png", "--zoom", "6"]));
    let f = &j["frames"][0];
    assert_eq!(f["overlap_px"], 32, "{}", f);
    assert_eq!(f["invalid"], 0, "{}", f);
    assert_eq!(f["sprites"], 2);
    assert_eq!(f["max_per_line"], 2, "overlapping cells both count on their shared lines");
    assert!(d.join("o/pole.png").is_file());
    let lint = pxl_in(&d, &["lint", "pole.pxl", "--rules", "cell_palette,hw_limit,palette_set"]);
    assert!(lint.status.success(), "{}", stdout(&lint));
    // the overlap costs sprites: a budget of one per line is broken on rows 8..11
    let t = std::fs::read_to_string(&p).unwrap().replace("palette_set = \"s\"\n", "palette_set = \"s\"\n").replace("\n[[frames]]", "\n[budget]\nper_line = 1\n\n[[frames]]");
    std::fs::write(&p, t).unwrap();
    let lint = pxl_in(&d, &["lint", "pole.pxl", "--rules", "hw_limit"]);
    assert_eq!(lint.status.code(), Some(1), "{}", stdout(&lint));
    assert!(stdout(&lint).contains("y=8..11"), "{}", stdout(&lint));
    // export: each pixel's index comes from the cell that owns it
    let o = pxl_in(&d, &["export", "pole.pxl"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let (_, _, idx, _, _) = agentsprite::png::read_indexed(&d.join("pole_export/a.png")).unwrap();
    assert_eq!(idx[9 * 8 + 3] / 4, 3, "pole pixel over the base: slot 3");
    assert_eq!(idx[9 * 8 + 0] / 4, 2, "base pixel beside the pole: slot 2");
    assert_eq!(idx[20 * 8 + 3] / 4, 2);
    // the order decides only when both cells could hold the colour; an explicit region assigns by area
    std::fs::write(d.join("a.cells.json"), "[{\"x\":0,\"y\":-4,\"slot\":3},{\"x\":0,\"y\":8,\"slot\":2}]").unwrap();
    let j = json_of(&pxl_in(&d, &["cells", "pole.pxl", "a", "--json"]));
    assert_eq!(j["frames"][0]["invalid"], 0);
    // a pixel no covering cell can hold is an error
    std::fs::write(d.join("a.cells.json"), "[{\"x\":0,\"y\":8,\"slot\":2},{\"x\":0,\"y\":-4,\"slot\":0}]").unwrap();
    let lint = pxl_in(&d, &["lint", "pole.pxl", "--rules", "cell_palette"]);
    assert_eq!(lint.status.code(), Some(1));
    assert!(stdout(&lint).contains("cannot be held by any cell"), "{}", stdout(&lint));
}
