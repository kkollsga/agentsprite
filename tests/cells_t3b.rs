//! M5 T3b: parts used in several zones (`palette_set = ["zone_1", "zone_3"]`).
mod e2e_common;

use agentsprite::color::parse_hex;
use agentsprite::{cells, profile, pxl, template};
use e2e_common::*;
use std::path::{Path, PathBuf};

const TPL: &str = r##"name = "t"
[layers.base]
"." = { role = "transparent" }
"1" = { role = "outline", color = "#182078" }
"2" = { role = "body", color = "#4850c0" }
"d" = { role = "dark", color = "#206020" }
"m" = { role = "mid", color = "#40c040" }
"l" = { role = "light", color = "#a0f0a0" }
"##;

fn set(slot4: Option<[&str; 3]>) -> String {
    let mut s = "[[slots]]\nslot = 0\ncolors = [\"#182078\", \"#4850c0\", \"#f8b890\"]\n".to_string();
    if let Some(c) = slot4 {
        s.push_str(&format!("[[slots]]\nslot = 4\nname = \"enemy\"\ncolors = [\"{}\", \"{}\", \"{}\"]\n", c[0], c[1], c[2]));
    }
    s
}

fn project(tag: &str) -> PathBuf {
    let d = tmpdir(tag);
    std::fs::create_dir_all(d.join("palettes")).unwrap();
    std::fs::create_dir_all(d.join("templates")).unwrap();
    std::fs::write(d.join("templates/t.toml"), TPL).unwrap();
    std::fs::write(d.join("palettes/zone_1.toml"), set(Some(["#004000", "#00a000", "#80f080"]))).unwrap();
    std::fs::write(d.join("palettes/zone_3.toml"), set(Some(["#004040", "#00a0a0", "#80f0f0"]))).unwrap();
    std::fs::write(d.join("palettes/zone_5.toml"), set(None)).unwrap();
    d
}

fn write_part(d: &Path, name: &str, header: &str, rows: &[String]) -> PathBuf {
    let text = format!("name = \"{}\"\nsize = [{}, {}]\norigin = [0, 0]\ntemplate = \"templates/t.toml\"\n{}\n[[frames]]\nname = \"a\"\ngrid = \"\"\"\n{}\n\"\"\"\n", name, rows[0].len(), rows.len(), header, rows.join("\n"));
    let p = d.join(format!("{}.pxl", name));
    std::fs::write(&p, text).unwrap();
    p
}

/// 16x16: left cell outline/body, right cell the three greens.
fn art() -> Vec<String> {
    (0..16).map(|y| (0..16).map(|x| if x < 8 { if y % 5 == 0 { '1' } else { '2' } } else { ['d', 'm', 'l'][(x + y) % 3] }).collect()).collect()
}

#[test]
fn the_header_takes_a_string_or_a_list() {
    let d = project("t3b1");
    let p = write_part(&d, "one", "palette_set = \"zone_1\"\n", &art());
    let t = pxl::to_string(&pxl::load(&p).unwrap());
    assert!(t.contains("palette_set = \"zone_1\"\n"));
    let p = write_part(&d, "two", "palette_set = [\"zone_1\", \"zone_3\"]\n", &art());
    let part = pxl::load(&p).unwrap();
    assert_eq!(part.palette_sets(), vec!["zone_1", "zone_3"]);
    let t = pxl::to_string(&part);
    assert!(t.contains("palette_set = [\"zone_1\", \"zone_3\"]"), "{}", t);
    assert_eq!(pxl::to_string(&pxl::parse_str(&t, &p).unwrap()), t);
}

#[test]
fn slots_missing_in_one_set_are_not_used_and_explicit_slots_are_errors() {
    let d = project("t3b2");
    // zone_5 has no slot 4: the part keeps slot 0 only, the green cell is then not drawable
    let p = write_part(&d, "m", "palette_set = [\"zone_1\", \"zone_5\"]\n", &art());
    let part = pxl::load(&p).unwrap();
    let prof = profile::load_for(&part, None).unwrap();
    let res = cells::resolve(&part, &prof).unwrap();
    assert_eq!(res.slots.len(), 1);
    let o = pxl_in(&d, &["lint", "m.pxl", "--rules", "cell_palette,palette_set"]);
    assert_eq!(o.status.code(), Some(1), "greens need slot 4: {}", stdout(&o));
    let p = write_part(&d, "e", "palette_set = [\"zone_1\", \"zone_5\"]\nslots = [4]\n", &art());
    let o = pxl_in(&d, &["lint", p.to_str().unwrap(), "--rules", "palette_set"]);
    assert!(stdout(&o).contains("every set of `palette_set` must define"), "{}", stdout(&o));
    // both sets define slot 4: clean (art in zone_1 colours)
    std::fs::write(d.join("templates/t.toml"), TPL.replace("#206020", "#004000").replace("#40c040", "#00a000").replace("#a0f0a0", "#80f080")).unwrap();
    write_part(&d, "ok", "palette_set = [\"zone_1\", \"zone_3\"]\n", &art());
    let o = pxl_in(&d, &["lint", "ok.pxl", "--rules", "cell_palette,palette_set,hw_limit"]);
    assert!(o.status.success(), "{}", stdout(&o));
}

#[test]
fn fit_minimises_the_summed_error_and_reports_each_set() {
    let d = project("t3b3");
    // the source greens sit between the two ramps; every pixel has one shared index
    let p = write_part(&d, "f", "palette_set = [\"zone_1\", \"zone_3\"]\n", &art());
    let j = json_of(&pxl_in(&d, &["palette", "fit", "f.pxl", "--json"]));
    let f = &j["fit"]["frames"][0];
    let sets = f["sets"].as_array().unwrap();
    assert_eq!(sets.len(), 2, "{}", f);
    assert_eq!(sets[0]["set"], "zone_1");
    assert_eq!(sets[1]["set"], "zone_3");
    // zone_3 is teal, the source green: it differs more than the authoring set
    assert!(sets[1]["mean_de"].as_f64().unwrap() > sets[0]["mean_de"].as_f64().unwrap(), "{}", f);
    assert_eq!(f["layout"]["invalid"], 0);
    let o = pxl_in(&d, &["palette", "fit", "f.pxl", "--apply", "--agent", "A1"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let part = pxl::load(&p).unwrap();
    let tpl = template::load_for(&part).unwrap();
    let allowed: Vec<_> = ["#004000", "#00a000", "#80f080", "#182078", "#4850c0", "#f8b890"].iter().map(|h| parse_hex(h).unwrap()).collect();
    for &k in &part.frames[0].grid.cells {
        if k != b'.' {
            assert!(allowed.contains(&agentsprite::lint::effective_color(&part, &tpl, k).unwrap()));
        }
    }
    // --set names several sets
    let j = json_of(&pxl_in(&d, &["palette", "fit", "f.pxl", "--set", "zone_1,zone_3", "--json"]));
    assert_eq!(j["fit"]["changed"], 0);
}

#[test]
fn render_in_another_set_and_export_one_png() {
    let d = project("t3b4");
    // art already in zone_1 colours
    let rows: Vec<String> = (0..16).map(|y| (0..16).map(|x| if x < 8 { '2' } else { ['d', 'm', 'l'][(x + y) % 3] }).collect()).collect();
    let fixed = TPL.replace("#206020", "#004000").replace("#40c040", "#00a000").replace("#a0f0a0", "#80f080");
    std::fs::write(d.join("templates/t.toml"), fixed).unwrap();
    let p = write_part(&d, "r", "palette_set = [\"zone_1\", \"zone_3\"]\n", &rows);
    let part = pxl::load(&p).unwrap();
    let tpl = template::load_for(&part).unwrap();
    let prof = profile::load_for(&part, None).unwrap();
    let res = cells::resolve(&part, &prof).unwrap();
    let (t3, grids) = cells::recoloured_grids(&part, &tpl, &res, &prof, "zone_3").unwrap();
    let cols: std::collections::BTreeSet<_> = grids[0].cells.iter().filter(|k| **k != b'.').map(|k| t3.color_of(*k).unwrap()).collect();
    assert!(cols.contains(&parse_hex("#00a0a0").unwrap()) && cols.contains(&parse_hex("#80f0f0").unwrap()), "{:?}", cols);
    assert!(!cols.contains(&parse_hex("#00a000").unwrap()));
    // cells --png --set renders it
    let o = pxl_in(&d, &["cells", "r.pxl", "a", "--png", "o/z3.png", "--set", "zone_3", "--zoom", "4"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    assert!(d.join("o/z3.png").is_file());
    assert_eq!(pxl_in(&d, &["cells", "r.pxl", "a", "--set", "zone_9"]).status.code(), Some(2));
    // export: one image of indexes plus cells.json
    let o = pxl_in(&d, &["export", "r.pxl"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let files: Vec<String> = std::fs::read_dir(d.join("r_export")).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    assert!(files.contains(&"a.png".to_string()) && files.contains(&"a.cells.json".to_string()), "{:?}", files);
    assert_eq!(files.iter().filter(|f| f.ends_with(".png")).count(), 1);
}

#[test]
fn a_part_is_counted_in_every_scene_that_lists_it() {
    let d = project("t3b5");
    write_part(&d, "e", "palette_set = [\"zone_1\", \"zone_3\"]\n", &art().iter().map(|r| r.replace(['d', 'm', 'l'], "2")).collect::<Vec<_>>());
    std::fs::write(
        d.join("pxl.toml"),
        "[parts]\ne = \"e.pxl\"\n\n[scene.zone_1]\npalette_set = \"zone_1\"\ngroups = { enemies = { budget = 5, parts = [\"e\"] } }\n\n[scene.zone_3]\npalette_set = \"zone_3\"\ngroups = { enemies = { budget = 5, parts = [\"e\"] } }\n\n[scene.zone_5]\npalette_set = \"zone_5\"\ngroups = { enemies = { budget = 5, parts = [\"e\"] } }\n",
    )
    .unwrap();
    let j = json_of(&pxl_in(&d, &["budget", "--all", "--json"]));
    let sc = j["scenes"].as_array().unwrap();
    assert_eq!(sc.len(), 3);
    for s in &sc[..2] {
        assert!(s["groups"][0]["total"].as_u64().unwrap() > 0, "{}", s);
        assert_eq!(s["groups"][0]["problems"].as_array().unwrap().len(), 0, "{}", s);
    }
    assert!(sc[2]["groups"][0]["total"].as_u64().unwrap() > 0, "zone_5 lists the part too: {}", sc[2]);
}

#[test]
fn compare_set_only_changes_the_rendering_colours() {
    let d = project("t3bcmp");
    // art in zone_1 colours, with a reference image of the same picture at factor 2
    let fixed = TPL.replace("#206020", "#004000").replace("#40c040", "#00a000").replace("#a0f0a0", "#80f080");
    std::fs::write(d.join("templates/t.toml"), &fixed).unwrap();
    let rows: Vec<String> = (0..16).map(|y| (0..16).map(|x| if x < 8 { '2' } else { ['d', 'm', 'l'][(x + y) % 3] }).collect()).collect();
    let colour = |k: char| match k {
        '2' => (0x48, 0x50, 0xc0),
        'd' => (0, 0x40, 0),
        'm' => (0, 0xa0, 0),
        _ => (0x80, 0xf0, 0x80),
    };
    let mut im = agentsprite::img::RgbaImage::new(32, 32);
    for (y, r) in rows.iter().enumerate() {
        for (x, k) in r.chars().enumerate() {
            let c = colour(k);
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                im.set((x * 2 + dx) as i32, (y * 2 + dy) as i32, agentsprite::img::Rgba::opaque(c.0, c.1, c.2));
            }
        }
    }
    std::fs::create_dir_all(d.join("refs")).unwrap();
    agentsprite::png::write_rgba(&d.join("refs/a.png"), &im).unwrap();
    write_part(&d, "c", "palette_set = [\"zone_1\", \"zone_3\"]\n\n[input]\nimages = \"refs/{frame}.png\"\nfactor = 2\n\n[reduce]\nfactor = 2\n", &rows);
    // the part's own lint is clean
    assert!(pxl_in(&d, &["lint", "c.pxl", "--rules", "palette_set,cell_palette,hw_limit"]).status.success());
    let plain = json_of(&pxl_in(&d, &["compare", "c.pxl", "--json", "--out", "o1"]));
    let other = json_of(&pxl_in(&d, &["compare", "c.pxl", "--set", "zone_3", "--json", "--out", "o2"]));
    assert_eq!(plain["frames"], other["frames"], "lint counts and metrics are the part's own, whatever colours are rendered");
    let errs = |j: &serde_json::Value| j["frames"].as_object().map(|m| m.values().map(|f| f["errors"].as_u64().unwrap_or(0)).sum::<u64>()).unwrap_or(0);
    assert_eq!(errs(&other), 0, "{}", other);
}
