//! M5 T1: palette sets, per-cell palettes, placement search, category hw_limit, `pxl cells`, 32-entry export.
mod e2e_common;

use agentsprite::cells;
use agentsprite::lint::{self, LintOpts};
use agentsprite::{profile, png, pxl, template};
use e2e_common::*;
use std::path::{Path, PathBuf};

fn fx() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lint")
}

fn issues(file: &Path, rule: &str) -> Vec<lint::Issue> {
    let part = pxl::load(file).unwrap();
    let prof = profile::load("gbc-sprite").unwrap();
    let opts = LintOpts { rules: vec![rule.to_string()], frames: vec![], refs: false, advice: false };
    lint::lint_part_auto(&part, &prof, &opts).into_iter().filter(|i| i.rule == rule).collect()
}

#[test]
fn category_hw_limit_fixtures() {
    for n in 5..=8 {
        let f = fx().join(format!("hw_limit/pos{}.pxl", n));
        let v = issues(&f, "hw_limit");
        assert!(!v.is_empty(), "{} should break its category limit", f.display());
    }
    for n in 5..=8 {
        let f = fx().join(format!("hw_limit/neg{}.pxl", n));
        let v = issues(&f, "hw_limit");
        assert!(v.is_empty(), "{} should fit its category: {:?}", f.display(), v);
    }
}

#[test]
fn cell_palette_names_cell_colours_and_fold() {
    let v = issues(&fx().join("cell_palette/pos1.pxl"), "cell_palette");
    assert!(!v.is_empty());
    let m = &v[0].message;
    assert!(m.contains("cell 0,0"), "{}", m);
    assert!(m.contains("#182078") && m.contains("#f8f8f8"), "{}", m);
    assert!(m.contains("best slot") && m.contains("fold"), "{}", m);
    assert!(v[0].x.is_some() && v[0].y.is_some());
}

#[test]
fn palette_set_hint_names_nearest() {
    let v = issues(&fx().join("palette_set/pos1.pxl"), "palette_set");
    assert_eq!(v.len(), 1);
    assert!(v[0].message.contains("#80c080") && v[0].message.contains("nearest set colour"), "{}", v[0].message);
}

#[test]
fn player_overlay_line_is_five_and_six_is_too_many() {
    // pos7: 4 cells + 2 two-slot cells = 6 on a line (limit 5); neg7: one two-slot cell = 5
    assert!(!issues(&fx().join("hw_limit/pos7.pxl"), "hw_limit").is_empty());
    assert!(issues(&fx().join("hw_limit/neg7.pxl"), "hw_limit").is_empty());
}

#[test]
fn old_parts_keep_the_grid_counting() {
    // no palette_set: the profile's 10 per band applies and the new rules stay silent
    let f = fx().join("hw_limit/pos1.pxl");
    assert!(!issues(&f, "hw_limit").is_empty());
    assert!(issues(&f, "palette_set").is_empty() && issues(&f, "cell_palette").is_empty());
}

// ---------------------------------------------------------------------------------------------- palette set files

fn parse(text: &str) -> agentsprite::output::Result<cells::PaletteSet> {
    cells::parse_set("t", text, Path::new("palettes/t.toml"), 0)
}

#[test]
fn set_parse_and_validation() {
    let ok = r##"
[[slots]]
slot = 0
name = "a"
colors = ["#182078", "#4850c0", "#f8b890"]
[[slots]]
slot = 1
colors = ["#182078", "#4850c0", "#f8f8f8"]
[[slots]]
slot = 7
variants = { boss = ["#000000", "#808080", "#f8f8f8"], fire = ["#900000", "#f80000", "#f8e800"] }
"##;
    let s = parse(ok).unwrap();
    assert!(s.slots[0].is_some() && s.slots[7].as_ref().unwrap().variants.len() == 2 && s.slots[3].is_none());
    // the same RGB may sit in several slots
    assert_eq!(s.slots[0].as_ref().unwrap().colors[0], s.slots[1].as_ref().unwrap().colors[0]);
    let bad = |t: &str, needle: &str| {
        let e = parse(t).unwrap_err();
        assert!(e.msg.contains(needle), "{} (wanted {})", e.msg, needle);
    };
    bad("[[slots]]\nslot = 0\ncolors = [\"#182078\", \"#4850c0\"]\n", "exactly 3 colours");
    bad("[[slots]]\nslot = 0\ncolors = [\"#182079\", \"#4850c0\", \"#f8b890\"]\n", "RGB555");
    bad("[[slots]]\nslot = 0\ncolors = [\"#182078\", \"#182078\", \"#f8b890\"]\n", "twice");
    bad("[[slots]]\nslot = 8\ncolors = [\"#182078\", \"#4850c0\", \"#f8b890\"]\n", "0..7");
    bad("[[slots]]\nslot = 2\nvariants = { x = [\"#182078\", \"#4850c0\", \"#f8b890\"] }\n", "only slot 7");
    bad("[[slots]]\nslot = 0\ncolors = [\"#182078\", \"#4850c0\", \"#f8b890\"]\n[[slots]]\nslot = 0\ncolors = [\"#182078\", \"#4850c0\", \"#f8b890\"]\n", "twice");
    bad("[[slots]]\nslot = 0\n", "needs `colors`");
    bad("bogus = 1\n", "unknown key");
}

#[test]
fn include_global_pulls_slots_0_to_3() {
    let d = tmpdir("incl");
    std::fs::create_dir_all(d.join("palettes")).unwrap();
    let c = |n: u8| format!("[[slots]]\nslot = {}\ncolors = [\"#{:02x}0000\", \"#{:02x}0800\", \"#f8f8f8\"]\n", n, n * 8, n * 8);
    std::fs::write(d.join("palettes/global.toml"), (0..4).map(c).collect::<String>()).unwrap();
    std::fs::write(d.join("palettes/zone.toml"), format!("include = \"global\"\n{}{}", c(4), c(5))).unwrap();
    let s = cells::load_set("zone", &d).unwrap();
    assert!((0..6).all(|i| s.slots[i].is_some()) && s.slots[6].is_none());
    assert_eq!(s.files.len(), 2);
    std::fs::write(d.join("palettes/clash.toml"), format!("include = \"global\"\n{}", c(2))).unwrap();
    assert!(cells::load_set("clash", &d).unwrap_err().msg.contains("slots 0-3 are global"));
    assert!(cells::load_set("nope", &d).unwrap_err().msg.contains("not found"));
}

// ---------------------------------------------------------------------------------------------- placement

const TPL: &str = r##"name = "t"
[layers.base]
"." = { role = "transparent" }
"1" = { role = "outline", color = "#182078" }
"2" = { role = "body", color = "#4850c0" }
"3" = { role = "skin", color = "#f8b890" }
"R" = { role = "red", color = "#f80000" }
"r" = { role = "dred", color = "#900000" }
"W" = { role = "white", color = "#f8f8f8" }
"G" = { role = "gold", color = "#f8e800" }
"##;

const SET: &str = r##"[[slots]]
slot = 0
name = "player"
colors = ["#182078", "#4850c0", "#f8b890"]
[[slots]]
slot = 1
name = "red"
colors = ["#900000", "#f80000", "#f8f8f8"]
[[slots]]
slot = 2
name = "gold"
colors = ["#686000", "#f8e800", "#f8f8f8"]
"##;

fn project(tag: &str) -> PathBuf {
    let d = tmpdir(tag);
    std::fs::create_dir_all(d.join("palettes")).unwrap();
    std::fs::create_dir_all(d.join("templates")).unwrap();
    std::fs::write(d.join("palettes/s.toml"), SET).unwrap();
    std::fs::write(d.join("templates/t.toml"), TPL).unwrap();
    d
}

fn write_part(d: &Path, name: &str, header: &str, w: usize, h: usize, rows: &[String]) -> PathBuf {
    assert_eq!(rows.len(), h);
    let text = format!("name = \"{}\"\nsize = [{}, {}]\norigin = [0, 0]\ntemplate = \"templates/t.toml\"\npalette_set = \"s\"\n{}\n[[frames]]\nname = \"a\"\ngrid = \"\"\"\n{}\n\"\"\"\n", name, w, h, header, rows.join("\n"));
    let p = d.join(format!("{}.pxl", name));
    std::fs::write(&p, text).unwrap();
    p
}

/// Rows with `fill` in the box x0..x1, y0..y1 (end exclusive), '.' elsewhere.
fn boxed(w: usize, h: usize, x0: usize, y0: usize, x1: usize, y1: usize, fill: char) -> Vec<String> {
    (0..h).map(|y| (0..w).map(|x| if x >= x0 && x < x1 && y >= y0 && y < y1 { fill } else { '.' }).collect()).collect()
}

fn place(p: &Path) -> cells::Placement {
    let part = pxl::load(p).unwrap();
    let prof = profile::load_for(&part, None).unwrap();
    let res = cells::resolve(&part, &prof).unwrap();
    let tpl = template::load_for(&part).unwrap();
    let pix = cells::frame_pix(&part, &tpl, &res, 0, prof.channel_step);
    cells::place_frame(&part, &res, &pix, 0).unwrap()
}

#[test]
fn twenty_px_figure_finds_three_columns() {
    let d = project("p20");
    // 20 px wide at x=6..26: the canvas grid needs 4 columns, an x offset of 6 needs 3
    let p = write_part(&d, "fig", "category = \"enemy\"\n", 32, 16, &boxed(32, 16, 6, 0, 26, 16, '2'));
    let pl = place(&p);
    assert_eq!(pl.cells.len(), 3, "{:?}", pl.cells.iter().map(|c| (c.x, c.y)).collect::<Vec<_>>());
    assert_eq!(pl.max_line, 3);
    assert_eq!(pl.invalid, 0);
    // the smallest offset that gets 3 columns wins (x offset 2: cells at 2, 10, 18)
    assert!(pl.cells.iter().all(|c| c.x.rem_euclid(8) == 2), "{:?}", pl.cells.iter().map(|c| c.x).collect::<Vec<_>>());
}

#[test]
fn band_offset_saves_a_row_of_cells() {
    let d = project("pband");
    // 8 wide, 16 tall figure at y=8..24 inside a 32 px canvas: the grid at oy=0 needs 2 cells, oy=8 needs 1
    let p = write_part(&d, "tall", "", 8, 32, &boxed(8, 32, 0, 8, 8, 24, '2'));
    let pl = place(&p);
    assert_eq!(pl.cells.len(), 1);
    assert_eq!(pl.cells[0].y, 8);
    assert_eq!(pl.oy, Some(8));
}

#[test]
fn placement_is_deterministic_and_fast() {
    let d = project("pperf");
    let mut rows = boxed(64, 64, 3, 5, 61, 59, '2');
    for (i, r) in rows.iter_mut().enumerate() {
        if i % 7 == 3 {
            *r = r.replace("22", "RR");
        }
    }
    let p = write_part(&d, "big", "category = \"boss\"\n", 64, 64, &rows);
    let a = place(&p);
    let b = place(&p);
    assert_eq!(a.cells.iter().map(|c| (c.x, c.y, c.slot)).collect::<Vec<_>>(), b.cells.iter().map(|c| (c.x, c.y, c.slot)).collect::<Vec<_>>());
    let part = pxl::load(&p).unwrap();
    let prof = profile::load_for(&part, None).unwrap();
    let res = cells::resolve(&part, &prof).unwrap();
    let tpl = template::load_for(&part).unwrap();
    let t0 = std::time::Instant::now();
    for _ in 0..20 {
        let pix = cells::frame_pix(&part, &tpl, &res, 0, 8);
        cells::place_frame(&part, &res, &pix, 0).unwrap();
    }
    let ms = t0.elapsed().as_secs_f64() * 1000.0 / 20.0;
    assert!(ms < 5.0, "placement took {:.2} ms per frame", ms);
}

#[test]
fn cells_split_by_colour_when_a_shift_helps() {
    let d = project("psplit");
    // left 12 px body colours, right 12 px red colours: the cell border must fall at x=12 (ox = 4)
    let mut rows = boxed(24, 16, 0, 0, 12, 16, '2');
    for r in rows.iter_mut() {
        r.replace_range(12..24, "RRRRRRRRRRRR");
    }
    let p = write_part(&d, "split", "", 24, 16, &rows);
    let pl = place(&p);
    assert_eq!(pl.invalid, 0, "{:?}", pl.cells);
    assert!(pl.cells.iter().any(|c| c.slot == 0) && pl.cells.iter().any(|c| c.slot == 1));
}

#[test]
fn authored_cells_override_and_must_cover() {
    let d = project("pauth");
    let p = write_part(&d, "au", "", 16, 16, &boxed(16, 16, 0, 0, 16, 16, '2'));
    std::fs::write(d.join("a.cells.json"), "[{\"x\":0,\"y\":0,\"slot\":0}]").unwrap();
    let pl = place(&p);
    assert!(pl.authored);
    assert_eq!(pl.cells.len(), 1);
    assert_eq!(pl.uncovered.len(), 8 * 16);
    let v = issues(&p, "cell_palette");
    assert!(v.iter().any(|i| i.message.contains("not covered")), "{:?}", v);
    std::fs::write(d.join("a.cells.json"), "[{\"x\":0,\"y\":0,\"slot\":0},{\"x\":8,\"y\":0,\"slot\":0}]").unwrap();
    assert!(issues(&p, "cell_palette").is_empty());
    // an authored slot that cannot hold the colours is an error naming the cell
    std::fs::write(d.join("a.cells.json"), "[{\"x\":0,\"y\":0,\"slot\":1},{\"x\":8,\"y\":0,\"slot\":0}]").unwrap();
    let v = issues(&p, "cell_palette");
    assert_eq!(v.len(), 1, "{:?}", v);
    assert!(v[0].message.contains("cell 0,0"));
}

#[test]
fn category_default_and_override() {
    let d = project("pcat");
    // four 8x16 cells on a line: enemy ok (4), item not (3), item with [budget] per_line = 4 ok
    let rows = boxed(32, 16, 0, 0, 32, 16, '2');
    let p = write_part(&d, "e", "category = \"enemy\"\n", 32, 16, &rows);
    assert!(issues(&p, "hw_limit").is_empty());
    let p = write_part(&d, "i", "", 32, 16, &rows);
    assert_eq!(issues(&p, "hw_limit").len(), 1);
    let p = write_part(&d, "b", "\n[budget]\nper_line = 4\n", 32, 16, &rows);
    assert!(issues(&p, "hw_limit").is_empty());
    let p = write_part(&d, "bad", "category = \"nonsense\"\n", 32, 16, &rows);
    let v = issues(&p, "palette_set");
    assert!(v[0].message.contains("unknown category"), "{:?}", v);
}

#[test]
fn part_header_round_trips() {
    let d = project("pround");
    let p = write_part(&d, "rt", "category = \"boss\"\nvariant = \"fire\"\nslots = [0, 1]\n\n[budget]\nper_line = 5\nper_frame = 20\n", 8, 16, &boxed(8, 16, 0, 0, 8, 16, '2'));
    let part = pxl::load(&p).unwrap();
    assert_eq!(part.category.as_deref(), Some("boss"));
    assert_eq!(part.slots, vec![0, 1]);
    assert_eq!(part.budget.per_line, Some(5));
    let text = pxl::to_string(&part);
    let again = pxl::parse_str(&text, &p).unwrap();
    assert_eq!(pxl::to_string(&again), text);
    assert_eq!(again.budget, part.budget);
}

// ---------------------------------------------------------------------------------------------- CLI and export

#[test]
fn cells_command_text_json_png() {
    let d = project("pcli");
    let mut rows = boxed(24, 16, 0, 0, 24, 16, '2');
    rows[8] = format!("{}{}", &rows[8][..4], "R".to_string() + &rows[8][5..]);
    write_part(&d, "c", "category = \"player\"\n", 24, 16, &rows);
    let o = pxl_in(&d, &["cells", "c.pxl"]);
    let t = stdout(&o);
    assert!(o.status.success(), "{}{}", t, stderr(&o));
    assert!(t.contains("palette set 's'") && t.contains("frame a:") && t.contains("two slots") && t.contains("line load"), "{}", t);
    let o = pxl_in(&d, &["cells", "c.pxl", "a", "--json", "--png", "out/c.png", "--zoom", "6"]);
    let j = json_of(&o);
    assert_eq!(j["frames"][0]["two_slot_cells"], 1);
    assert_eq!(j["frames"][0]["sprites"], 4);
    assert_eq!(j["frames"][0]["cells"].as_array().unwrap().len(), 3);
    assert!(d.join("out/c.png").is_file());
    // a part without a palette set is told so (exit 0, nothing to place)
    std::fs::write(d.join("old.pxl"), "name = \"old\"\nsize = [8, 8]\norigin = [0, 0]\n").unwrap();
    let o = pxl_in(&d, &["cells", "old.pxl"]);
    assert!(o.status.success() && stdout(&o).contains("no `palette_set`"), "{}", stdout(&o));
}

#[test]
fn export_round_trip_32_entries_one_slot_per_cell() {
    let d = project("pexp");
    // 24x16 figure: body cells, a red cell and a gold cell
    let mut rows = boxed(24, 16, 0, 0, 24, 16, '2');
    for r in rows.iter_mut() {
        r.replace_range(8..16, "RRrrRRrr");
        r.replace_range(16..24, "GGGGWWWW");
    }
    write_part(&d, "e", "category = \"enemy\"\n", 24, 16, &rows);
    let o = pxl_in(&d, &["export", "e.pxl"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let out = d.join("e_export");
    let (w, h, idx, pal, trns) = png::read_indexed(&out.join("a.png")).unwrap();
    assert_eq!((w, h), (24, 16));
    assert_eq!(pal.len(), 32);
    assert!((0..8).all(|s| trns.get(s * 4).copied() == Some(0)));
    assert_eq!(pal[1], agentsprite::color::parse_hex("#182078").unwrap());
    assert_eq!(pal[4 + 2], agentsprite::color::parse_hex("#f80000").unwrap());
    let cj: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(out.join("a.cells.json")).unwrap()).unwrap();
    let cells_ = cj.as_array().unwrap();
    assert_eq!(cells_.len(), 3);
    for c in cells_ {
        let (cx, cy, slot) = (c["x"].as_i64().unwrap(), c["y"].as_i64().unwrap(), c["slot"].as_u64().unwrap() as u8);
        for y in cy.max(0)..(cy + 16).min(16) {
            for x in cx.max(0)..(cx + 8).min(24) {
                let i = idx[(y as usize) * 24 + x as usize];
                assert!(i == 0 || (i / 4 == slot && i % 4 != 0), "pixel {},{} index {} outside slot {}", x, y, i, slot);
            }
        }
    }
    assert_eq!(cells_.iter().map(|c| c["slot"].as_u64().unwrap()).collect::<Vec<_>>(), vec![0, 1, 2]);
    assert!(!out.join("a_detail.png").exists());
    // second export writes nothing new
    let again = json_of(&pxl_in(&d, &["export", "e.pxl", "--json"]));
    assert_eq!(again["exported"][0]["written"], 0);
}

#[test]
fn export_player_overlay_cell_goes_to_detail() {
    let d = project("pdet");
    let mut rows = boxed(16, 16, 0, 0, 16, 16, '2');
    rows[8].replace_range(4..6, "RR");
    write_part(&d, "pl", "category = \"player\"\n", 16, 16, &rows);
    let o = pxl_in(&d, &["export", "pl.pxl"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let out = d.join("pl_export");
    let (_, _, base, _, _) = png::read_indexed(&out.join("a.png")).unwrap();
    let (_, _, det, pal, _) = png::read_indexed(&out.join("a_detail.png")).unwrap();
    assert_eq!(pal.len(), 32);
    assert_eq!(det.iter().filter(|i| **i != 0).count(), 2);
    assert!(det.iter().filter(|i| **i != 0).all(|i| *i / 4 == 1));
    assert_eq!(base[8 * 16 + 4], 0, "overlay pixels leave the base image empty");
    assert_eq!(base[8 * 16 + 3] / 4, 0);
    // an invalid layout refuses to export
    let mut bad = boxed(8, 16, 0, 0, 8, 16, '2');
    bad[3].replace_range(0..8, "WWWWWWWW");
    bad[4].replace_range(0..8, "RGRGRGRG");
    write_part(&d, "bad", "", 8, 16, &bad);
    let o = pxl_in(&d, &["export", "bad.pxl"]);
    assert_eq!(o.status.code(), Some(1), "{}{}", stdout(&o), stderr(&o));
}

#[test]
fn search_prefers_fewer_sprites_once_the_limits_are_met() {
    // minimising the busiest scanline first picks a layout of 8 sprites here; one of 7 meets every limit
    let d = project("pregress");
    let fig = "........................................
........................................
........................................
........................................
........................................
........................................
........................................
........................................
........................................
........................................
........................................
........................................
........................................
........................................
........................................
........................................
........................................
........................................
...............................222222222
...............................222222222
...............................222222222
.............22222222..........222222222
.............22222222..........222222222
.............22222222..........222222222
.............22222222..........222222222
.............22222222..........222222222
.............22222222..........222222222
.............22222222..........222222222
.............22222222..........222222222
.............22222222..........222222222
.............22222222..........222222222
.............22222222..........222222222
.............22222222..........222222222
.............22222222..........222222222
.............22222222..........222222222
.............22222222..........222222222
.............22222222..........222222222
....................22222222222222222222
....................22222222222222222222
....................22222222222222222222
....................22222222222222222222
....................22222222222222222222
....................22222222222222222222
....................22222222222222222222
....................2222222222222222222.
....................2222222222222222222.
....................2222222222222222222.
....................2222222222222222222.";
    let rows: Vec<String> = fig.lines().map(String::from).collect();
    let p = write_part(&d, "reg", "category = \"player\"\n", 40, 48, &rows);
    let pl = place(&p);
    assert_eq!(pl.invalid, 0);
    assert!(pl.sprites <= 7, "{} sprites, band offset {:?}", pl.sprites, pl.oy);
    assert!(pl.max_line <= 4, "max {} per line", pl.max_line);
    assert!(issues(&p, "hw_limit").is_empty());
}
