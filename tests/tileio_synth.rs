//! M4 T3: tile set / screen export, import, `--tile` editing, tile compare (synthetic data only).

mod e2e_common;
mod tiles_common;
use agentsprite::model::{Frame, Grid, Part, PartKind};
use agentsprite::{ase, png, pxl};
use e2e_common::*;
use std::path::{Path, PathBuf};
use tiles_common::*;

const PALS: [&str; 8] = ["abcd", "aefg", "ahij", "aklm", "anop", "aqrs", "atuv", "awxy"];

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
}

/// A sheet of `n` tiles (16 per row), tile i uses palette i % 8 with all four colours; the cells after the
/// last tile are all `a` (air).
fn sheet_grid(n: usize, rows: usize, seed: u64) -> Grid {
    let mut g = Grid::new(128, rows * 8, b'a');
    let mut r = Rng(seed);
    for i in 0..n {
        let keys: Vec<u8> = PALS[i % 8].bytes().collect();
        for y in 0..8 {
            for x in 0..8 {
                let k = if y == 0 && x < 4 { keys[x] } else { keys[(r.next() % 4) as usize] };
                g.cells[(i / 16 * 8 + y) * 128 + (i % 16) * 8 + x] = k;
            }
        }
    }
    g
}

fn write_part(dir: &Path, file: &str, name: &str, kind: PartKind, frames: Vec<(&str, Grid)>, extra: &str) -> PathBuf {
    let (w, h) = (frames[0].1.w, frames[0].1.h);
    let mut p = Part::new(name, (w, h));
    p.kind = kind;
    p.tile = Some((8, 8));
    p.template = Some("templates/tiles.toml".into());
    for (n, g) in frames {
        p.frames.push(Frame { name: n.into(), src: None, locked: false, grid: g });
    }
    let mut text = pxl::to_string(&p);
    text.push_str(extra);
    std::fs::create_dir_all(dir.join("templates")).unwrap();
    std::fs::copy(fixtures().join("templates/tiles.toml"), dir.join("templates/tiles.toml")).unwrap();
    let path = dir.join(file);
    std::fs::write(&path, text).unwrap();
    path
}

/// A project with a zone (blocks + palette cycle) and an animated tile part, manifest, and checker-style spec.
fn project(tag: &str) -> PathBuf {
    let d = tmpdir(tag);
    std::fs::create_dir_all(d.join("consoles/gbc")).unwrap();
    std::fs::write(d.join("consoles/gbc/tile.toml"), include_str!("fixtures/tiles/consoles/gbc/tile.toml")).unwrap();
    write_part(&d, "zone.pxl", "zone_9", PartKind::Tileset, vec![("blocks", sheet_grid(40, 3, 7))], "\n[[cycle]]\npalette = \"sky\"\nbg_palette = 1\ntargets = [0, 1, 2, 3]\nsteps = [[\"#102030\", \"#304050\", \"#506070\", \"#708090\"], [\"#304050\", \"#506070\", \"#708090\", \"#102030\"]]\n");
    // 2x1 tiles: rebuild by hand (the sheet is 128 wide)
    let mut f0 = Grid::new(16, 8, b'a');
    let mut f1 = Grid::new(16, 8, b'a');
    for y in 0..8 {
        for x in 0..16 {
            f0.cells[y * 16 + x] = b"abcd"[(x + y) % 4];
            f1.cells[y * 16 + x] = b"abcd"[(x + 2 * y + 1) % 4];
        }
    }
    write_part(&d, "wf.pxl", "wf", PartKind::Tileset, vec![("waterfall_00", f0), ("waterfall_01", f1)], "");
    std::fs::create_dir_all(d.join("zones/zone_9")).unwrap();
    std::fs::write(d.join("zones/zone_9/spec.json"), r#"{"id": "A-9", "type": "tiles", "files": {"blocks.png": {"tile": [8, 8], "per_row": 16, "max_unique_tiles": 300}}, "optional": ["background_blocks.png", "animated/", "palette_cycle/"]}"#).unwrap();
    std::fs::write(
        d.join("pxl.toml"),
        "legacy = true\n[parts.zone_9]\npath = \"zone.pxl\"\nout = \"zones/zone_9\"\nase = \"zones/zone_9/blocks.aseprite\"\n[parts.wf]\npath = \"wf.pxl\"\nout = \"zones/zone_9\"\nase = \"zones/zone_9/animated/waterfall.aseprite\"\n",
    )
    .unwrap();
    d
}

fn out_tree(d: &Path) -> Vec<(String, Vec<u8>)> {
    tree(&d.join("zones"), &[])
}

#[test]
fn export_is_byte_identical_incremental_and_the_checker_passes() {
    let d = project("tileio_export");
    let o = pxl_in(&d, &["build"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let first = out_tree(&d);
    let names: Vec<&str> = first.iter().map(|x| x.0.as_str()).collect();
    for want in [
        "zone_9/blocks.png",
        "zone_9/blocks.aseprite",
        "zone_9/animated/waterfall_00.png",
        "zone_9/animated/waterfall_01.png",
        "zone_9/animated/waterfall.aseprite",
        "zone_9/palette_cycle/sky.json",
        "zone_9/spec.json",
    ] {
        assert!(names.contains(&want), "{} missing from {:?}", want, names);
    }
    assert_eq!(names.iter().filter(|n| n.ends_with(".png")).count(), 1 + 2, "{:?}", names);
    // nothing changes on a rebuild; a forced rebuild after deleting the outputs is byte-identical
    let o = pxl_in(&d, &["build"]);
    assert!(stdout(&o).contains("up to date"), "{}", stdout(&o));
    std::fs::remove_dir_all(d.join("zones/zone_9/animated")).unwrap();
    std::fs::remove_dir_all(d.join("zones/zone_9/palette_cycle")).unwrap();
    std::fs::remove_file(d.join("zones/zone_9/blocks.png")).unwrap();
    let o = pxl_in(&d, &["build", "--force"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    assert_eq!(first, out_tree(&d), "export is not byte identical across runs");
    // separate export runs into other folders agree too
    std::fs::copy(d.join("zone.pxl"), d.join("zone2.pxl")).unwrap();
    let o = pxl_in(&d, &["export", "zone2.pxl", "--out", "again"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    assert_eq!(std::fs::read(d.join("again/blocks.png")).unwrap(), std::fs::read(d.join("zones/zone_9/blocks.png")).unwrap());

    // the PNG: indexed, no tRNS, PLTE = the template palette keys in order
    let (w, h, idx, pal, alpha) = png::read_indexed(&d.join("zones/zone_9/blocks.png")).unwrap();
    assert_eq!((w, h), (128, 24));
    assert!(alpha.iter().all(|&a| a == 255) || alpha.is_empty(), "tRNS present: {:?}", alpha);
    let bytes = std::fs::read(d.join("zones/zone_9/blocks.png")).unwrap();
    assert!(!bytes.windows(4).any(|w| w == b"tRNS"), "blocks.png has a tRNS chunk");
    assert_eq!(pal.len(), 25, "keys of the 8 palettes, shared 'a' once");
    let hex = |c: agentsprite::color::Rgb| agentsprite::color::to_hex(c);
    assert_eq!((hex(pal[0]), hex(pal[1]), hex(pal[4]), hex(pal[24])), ("#88c8f8".into(), "#f8f8f8".into(), "#80e060".into(), "#d0a8d0".into()));
    // pixels are the part's keys
    let part = pxl::load(&d.join("zone.pxl")).unwrap();
    let order: Vec<u8> = PALS.iter().flat_map(|p| p.bytes()).fold(Vec::new(), |mut v, k| {
        if !v.contains(&k) {
            v.push(k)
        }
        v
    });
    for (i, &k) in part.frames[0].grid.cells.iter().enumerate() {
        assert_eq!(idx[i] as usize, order.iter().position(|&o| o == k).unwrap(), "pixel {}", i);
    }

    // .aseprite: valid, the plain image layer equals the tilemap layer equals the PNG
    let ab = std::fs::read(d.join("zones/zone_9/blocks.aseprite")).unwrap();
    assert!(ase::validate(&ab).is_empty());
    let doc = ase::read(&ab).unwrap();
    let layers = doc.layers();
    assert_eq!((layers[0].name.as_str(), layers[1].name.as_str()), ("image", "tiles"));
    assert!(layers[1].layer_type == ase::LayerType::Tilemap);
    let img = ase::render_layer_indices(&doc, 0, 0).unwrap();
    let til = ase::render_layer_indices(&doc, 0, 1).unwrap();
    assert_eq!(img, til, "tilemap layer differs from the image layer");
    assert!(img.iter().zip(&idx).all(|(a, b)| *a == *b + 1));
    let ts = doc.tilesets();
    assert_eq!(ts.len(), 1);
    assert!(ts[0].num_tiles <= 42, "unique tiles up to mirroring: {}", ts[0].num_tiles);

    // pxl check knows every expected file and the spec.json list
    let o = pxl_in(&d, &["check"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    assert!(stdout(&o).contains("PASS"), "{}", stdout(&o));
    std::fs::remove_file(d.join("zones/zone_9/palette_cycle/sky.json")).unwrap();
    let o = pxl_in(&d, &["check"]);
    assert!(!o.status.success() && stdout(&o).contains("missing") && stdout(&o).contains("sky.json"), "{}", stdout(&o));
    assert!(pxl_in(&d, &["build", "--force"]).status.success());
    std::fs::write(d.join("zones/zone_9/stray.png"), &bytes).unwrap();
    let o = pxl_in(&d, &["check"]);
    assert!(!o.status.success() && stdout(&o).contains("unlisted image"), "{}", stdout(&o));
    std::fs::remove_file(d.join("zones/zone_9/stray.png")).unwrap();
    std::fs::write(d.join("zones/zone_9/spec.json"), r#"{"type": "tiles", "files": {"blocks.png": {}, "background_blocks.png": {}}}"#).unwrap();
    let o = pxl_in(&d, &["check"]);
    assert!(!o.status.success() && stdout(&o).contains("background_blocks.png"), "{}", stdout(&o));

    // the real checker
    std::fs::write(d.join("zones/zone_9/spec.json"), r#"{"type": "tiles"}"#).unwrap();
    if let Some(script) = checker_available() {
        std::fs::copy(&script, d.join("check_assets.py")).unwrap();
        let o = std::process::Command::new("python3").args(["-I", "check_assets.py", "zones/zone_9"]).current_dir(&d).output().unwrap();
        let t = stdout(&o);
        assert!(t.contains("RESULT: PASS"), "{}", t);
        assert!(!t.contains("WARNING"), "{}", t);
    }
}

#[test]
fn export_rejects_transparent_cells_and_screens_export_flat() {
    let d = tmpdir("tileio_screen");
    let mut g = sheet_grid(4, 1, 5);
    g.cells[9] = b'.';
    write_part(&d, "z.pxl", "z", PartKind::Tileset, vec![("blocks", g)], "");
    let o = pxl_in(&d, &["export", "z.pxl", "--out", "o"]);
    assert!(!o.status.success());
    let t = format!("{}{}", stdout(&o), stderr(&o));
    assert!(t.contains("transparent") && t.contains("(9,0)"), "{}", t);
    // a screen: 160x144, flat file names
    let mut s = Grid::new(160, 144, b'a');
    for (i, c) in s.cells.iter_mut().enumerate() {
        *c = b"abcd"[(i / 8 + i / 1280) % 4];
    }
    write_part(&d, "splash.pxl", "splash", PartKind::Screen, vec![("splash", s)], "");
    let o = pxl_in(&d, &["export", "splash.pxl", "--out", "scr"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    assert!(d.join("scr/splash.png").is_file() && d.join("scr/splash.aseprite").is_file());
    let (w, h, _, _, _) = png::read_indexed(&d.join("scr/splash.png")).unwrap();
    assert_eq!((w, h), (160, 144));
    if let Some(script) = checker_available() {
        let root = d.join("pkg");
        std::fs::create_dir_all(root.join("screens/splash")).unwrap();
        std::fs::copy(&script, root.join("check_assets.py")).unwrap();
        std::fs::copy(d.join("scr/splash.png"), root.join("screens/splash/splash.png")).unwrap();
        std::fs::write(root.join("screens/splash/spec.json"), r#"{"id": "A", "type": "screen"}"#).unwrap();
        let o = std::process::Command::new("python3").args(["-I", "check_assets.py", "screens/splash"]).current_dir(&root).output().unwrap();
        assert!(stdout(&o).contains("RESULT: PASS"), "{}", stdout(&o));
    }
}

#[test]
fn import_round_trips_an_export_and_names_the_bad_pixel() {
    let d = project("tileio_import");
    assert!(pxl_in(&d, &["export", "zone.pxl", "--out", "o"]).status.success());
    let orig = pxl::load(&d.join("zone.pxl")).unwrap();
    // a copy with an empty sheet takes the PNG back
    let blank = Grid::new(128, 24, b'a');
    write_part(&d, "back.pxl", "back", PartKind::Tileset, vec![("blocks", blank)], "");
    let o = pxl_in(&d, &["import", "o/blocks.png", "--part", "back.pxl"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let back = pxl::load(&d.join("back.pxl")).unwrap();
    assert_eq!(back.frames[0].grid, orig.frames[0].grid, "import(export(x)) != x");
    // the .aseprite imports through its image layer
    write_part(&d, "back2.pxl", "back2", PartKind::Tileset, vec![("blocks", Grid::new(128, 24, b'a'))], "");
    let o = pxl_in(&d, &["import", "o/zone.aseprite", "--part", "back2.pxl"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    assert_eq!(pxl::load(&d.join("back2.pxl")).unwrap().frames[0].grid, orig.frames[0].grid);
    // a colour that is no template key: error names tile, pixel and the nearest key
    let (w, h, mut idx, pal, _) = png::read_indexed(&d.join("o/blocks.png")).unwrap();
    let mut pal2 = pal.clone();
    pal2[1] = agentsprite::color::Rgb::new(0xf8, 0xf0, 0xf8);
    for v in idx.iter_mut() {
        if *v == 1 {
            *v = 1;
        }
    }
    png::write_indexed(&d.join("bad.png"), w, h, &idx, &pal2, false).unwrap();
    let before = std::fs::read_to_string(d.join("back.pxl")).unwrap();
    let o = pxl_in(&d, &["import", "bad.png", "--part", "back.pxl", "--frame", "blocks"]);
    assert!(!o.status.success());
    let t = format!("{}{}", stdout(&o), stderr(&o));
    assert!(t.contains("tile #") && t.contains("#f8f0f8") && t.contains("nearest template key is 'b'"), "{}", t);
    assert_eq!(before, std::fs::read_to_string(d.join("back.pxl")).unwrap(), "a failed import must not write");
    // wrong size
    let small = vec![0u8; 16 * 8];
    png::write_indexed(&d.join("small.png"), 16, 8, &small, &pal, false).unwrap();
    let o = pxl_in(&d, &["import", "small.png", "--part", "back.pxl", "--frame", "blocks"]);
    assert!(!o.status.success() && format!("{}{}", stdout(&o), stderr(&o)).contains("canvas"));
}

fn grid_of(path: &Path) -> Grid {
    pxl::load(path).unwrap().frames[0].grid.clone()
}

fn tile_cells(g: &Grid, i: usize) -> Vec<u8> {
    let (ox, oy) = ((i % 16) * 8, (i / 16) * 8);
    (0..64).map(|k| g.cells[(oy + k / 8) * g.w + ox + k % 8]).collect()
}

#[test]
fn tile_edits_stay_in_their_tile_and_echo_the_status() {
    let d = tmpdir("tileio_edit");
    let p = write_part(&d, "z.pxl", "z", PartKind::Tileset, vec![("blocks", sheet_grid(40, 3, 11))], "");
    let before = grid_of(&p);
    let mut r = Rng(99);
    for round in 0..14 {
        let t = (r.next() % 40) as usize;
        let cur = grid_of(&p);
        let pal = PALS[t % 8].as_bytes();
        let (x, y) = ((r.next() % 8) as usize, (r.next() % 8) as usize);
        let key = pal[(r.next() % 4) as usize] as char;
        let ts = t.to_string();
        let o = match round % 3 {
            0 => pxl_in(&d, &["set", "z.pxl", "blocks", &format!("{},{}={}", x, y, key), "--tile", &ts]),
            1 => pxl_in(&d, &["row", "z.pxl", "blocks", "0", &y.to_string(), &format!("{}{}", key, key), "--tile", &ts]),
            _ => pxl_in(&d, &["patch", "z.pxl", "blocks", &x.to_string(), &y.to_string(), &format!("{}", key), "--tile", &ts]),
        };
        assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
        let out = stdout(&o);
        let last = out.lines().find(|l| l.starts_with(&format!("tile {}: ", t))).unwrap_or_else(|| panic!("no status line in {}", out));
        assert!(last.starts_with(&format!("tile {}: palette ", t)) && last.contains(" ok") && last.contains("unique"), "{:?}", last);
        let now = grid_of(&p);
        for other in 0..48 {
            if other != t {
                assert_eq!(tile_cells(&cur, other), tile_cells(&now, other), "round {} edit of tile {} changed tile {}", round, t, other);
            }
        }
    }
    assert_ne!(before, grid_of(&p));
    // out of tile, wrong tile, wrong command
    let o = pxl_in(&d, &["set", "z.pxl", "blocks", "8,0=a", "--tile", "3"]);
    assert!(!o.status.success() && format!("{}{}", stdout(&o), stderr(&o)).contains("outside tile 3"));
    let o = pxl_in(&d, &["row", "z.pxl", "blocks", "6", "0", "abc", "--tile", "3"]);
    assert!(!o.status.success());
    let o = pxl_in(&d, &["set", "z.pxl", "blocks", "0,0=a", "--tile", "48"]);
    assert!(!o.status.success() && format!("{}{}", stdout(&o), stderr(&o)).contains("does not exist"));
    let o = pxl_in(&d, &["fill", "z.pxl", "blocks", "1,1", "a", "--tile", "3"]);
    assert!(!o.status.success() && format!("{}{}", stdout(&o), stderr(&o)).contains("--tile works with set, row, patch and pattern"));
    // a key outside the tile's palette is reported, not refused
    let o = pxl_in(&d, &["set", "z.pxl", "blocks", "0,0=h", "--tile", "0"]);
    assert!(o.status.success());
    assert!(stdout(&o).contains("tile 0: ") && stdout(&o).contains("5 colours"), "{}", stdout(&o));
    // a copy of another tile is reported as a duplicate
    let g = grid_of(&p);
    let rows: Vec<String> = (0..8).map(|y| (0..8).map(|x| g.cells[(y) * 128 + 8 + x] as char).collect()).collect();
    let o = pxl_stdin(&d, &["patch", "z.pxl", "blocks", "0", "0", "-", "--tile", "2"], &rows.join("\n"));
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    assert!(stdout(&o).contains("tile 2: ") && stdout(&o).contains("duplicate of tile 1"), "{}", stdout(&o));
    // dry run writes nothing, json carries the status
    let snap = std::fs::read_to_string(&p).unwrap();
    let o = pxl_in(&d, &["set", "z.pxl", "blocks", "1,1=a", "--tile", "5", "--dry-run", "--json"]);
    assert!(o.status.success());
    assert_eq!(snap, std::fs::read_to_string(&p).unwrap());
    let v = json_of(&o);
    let data = v.get("data").unwrap_or(&v);
    assert_eq!(data["frames"][0]["tile"], 5);
    assert!(data["frames"][0]["status"]["fits"].is_boolean());
}

#[test]
fn show_tile_round_trips_through_patch_with_and_without_reference() {
    let d = tmpdir("tileio_showpatch");
    let p = synth_level(&d);
    assert!(pxl_in(&d, &["load", "syn.pxl"]).status.success());
    // tile 2 (a quarter of colour c) -> tile 5, via the printed text
    let o = pxl_in(&d, &["show", "syn.pxl", "blocks", "--tile", "2"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let text = stdout(&o);
    assert!(text.contains("tile 2: ") && text.contains("palette 1 (p0) ok"), "{}", text);
    let o = pxl_stdin(&d, &["patch", "syn.pxl", "blocks", "-", "--tile", "5"], &text);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let g = grid_of(&p);
    assert_eq!(tile_cells(&g, 5), tile_cells(&g, 2));
    assert_eq!(tile_cells(&g, 6), vec![b'a'; 64], "neighbour untouched");
    assert!(stdout(&o).contains("duplicate of tile 2"), "{}", stdout(&o));
    // with the 16x16 reference rows (12a/12b style)
    let o = pxl_in(&d, &["show", "syn.pxl", "blocks", "--tile", "3", "--ref"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let text = stdout(&o);
    assert!(text.contains("\n0 |") && text.contains("0a |") && text.contains("0b |") && text.contains("7b |") && text.contains("legend:"), "{}", text);
    let o = pxl_stdin(&d, &["patch", "syn.pxl", "blocks", "-", "--tile", "9"], &text);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let g = grid_of(&p);
    assert_eq!(tile_cells(&g, 9), tile_cells(&g, 3));
    // json + raw
    let o = pxl_in(&d, &["show", "syn.pxl", "blocks", "--tile", "3", "--json"]);
    let v = json_of(&o);
    let data = v.get("data").unwrap_or(&v);
    assert_eq!(data["rows"].as_array().unwrap().len(), 8);
    assert_eq!(data["at"], serde_json::json!([24, 0]));
    // the view PNG
    let o = pxl_in(&d, &["view", "syn.pxl", "blocks", "--tile", "3", "--out", "t3.png"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let im = png::read_rgba(&d.join("t3.png")).unwrap();
    assert!(im.w > 100 && im.w <= 1568 && im.h <= 1568);
    assert!(!pxl_in(&d, &["show", "syn.pxl", "blocks", "--tile", "99"]).status.success());
}

/// 400 tiles with a 2x reference: the pages respect the 1568 px limit and the ranking puts the damaged tile first.
#[test]
fn compare_pages_stay_within_the_limit_and_rank_the_damaged_tile_first() {
    let d = tmpdir("tileio_compare");
    let rows = 25;
    let g = sheet_grid(400, rows, 21);
    let p = write_part(&d, "z.pxl", "z", PartKind::Tileset, vec![("blocks", g.clone())], "");
    let text = std::fs::read_to_string(&p).unwrap().replace("[[frames]]", "[input]\nimages = \"ref_{frame}.png\"\n\n[reduce]\nfactor = 2\n\n[[frames]]");
    std::fs::write(&p, text).unwrap();
    // reference = the tiles at 2x with a few perturbed pixels; tile 17 is mostly wrong
    let part = pxl::load(&p).unwrap();
    let tpl = agentsprite::template::load_for(&part).unwrap();
    let mut im = agentsprite::img::RgbaImage::new(256, rows * 16);
    for y in 0..g.h {
        for x in 0..g.w {
            let c = tpl.color_of(g.cells[y * g.w + x]).unwrap();
            for k in 0..4 {
                im.set((x * 2 + k % 2) as i32, (y * 2 + k / 2) as i32, agentsprite::img::Rgba::opaque(c.r, c.g, c.b));
            }
        }
    }
    let mut r = Rng(5);
    for _ in 0..600 {
        let (x, y) = ((r.next() % 256) as i32, (r.next() % (rows as u64 * 16)) as i32);
        im.set(x, y, agentsprite::img::Rgba::opaque(0x20, 0x20, 0x20));
    }
    for y in 0..16 {
        for x in 0..16 {
            im.set(17 % 16 * 16 + x, 16 + y, agentsprite::img::Rgba::opaque(0xf8, 0x10, 0x10));
        }
    }
    png::write_rgba(&d.join("ref_blocks.png"), &im).unwrap();
    let o = pxl_in(&d, &["compare", "z.pxl", "--out", "cmp", "--json"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let v = json_of(&o);
    let data = v.get("data").unwrap_or(&v);
    let pages = data["pages"].as_array().unwrap();
    assert!(!pages.is_empty() && pages.len() <= 3, "{} pages", pages.len());
    for pg in pages {
        let path = d.join(pg["path"].as_str().unwrap());
        let im = png::read_rgba(&path).unwrap();
        assert!(im.w <= 1568 && im.h <= 1568, "{}x{}", im.w, im.h);
        assert_eq!([im.w as u64, im.h as u64], [pg["size"][0].as_u64().unwrap(), pg["size"][1].as_u64().unwrap()]);
    }
    let ranking = data["ranking"].as_array().unwrap();
    assert_eq!(ranking.len(), 400);
    assert_eq!(ranking[0]["tile"], 17, "{:?}", &ranking[..3]);
    let scores: Vec<f64> = ranking.iter().map(|r| r["score"].as_f64().unwrap()).collect();
    assert!(scores.windows(2).all(|w| w[0] >= w[1]));
    assert!(d.join("cmp/z-tiles.json").is_file());
    // identical runs write identical pages
    let a = std::fs::read(d.join("cmp/z-tiles-01.png")).unwrap();
    assert!(pxl_in(&d, &["compare", "z.pxl", "--out", "cmp"]).status.success());
    assert_eq!(a, std::fs::read(d.join("cmp/z-tiles-01.png")).unwrap());
    // zoom 1 fits a page of 400 tiles; a huge zoom is refused
    let o = pxl_in(&d, &["compare", "z.pxl", "--out", "cmp1", "--zoom", "1", "--json"]);
    assert!(o.status.success());
    let o = pxl_in(&d, &["compare", "z.pxl", "--out", "cmp9", "--zoom", "40"]);
    assert!(!o.status.success() && format!("{}{}", stdout(&o), stderr(&o)).contains("limit"));
}

/// T4 fix A: an `--out` scratch export never deletes the recorded outputs elsewhere and leaves the status alone.
#[test]
fn scratch_export_keeps_staging_and_status() {
    let d = project("tileio_scratch");
    assert!(pxl_in(&d, &["build"]).status.success());
    let before = out_tree(&d);
    let status_before = std::fs::read(d.join("zone.status.json")).unwrap();
    let o = pxl_in(&d, &["export", "zone.pxl", "--out", "scratch"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    assert!(d.join("scratch/blocks.png").is_file());
    assert_eq!(before, out_tree(&d), "staging files changed");
    assert_eq!(status_before, std::fs::read(d.join("zone.status.json")).unwrap(), "status changed by a scratch export");
    // a second scratch export into another folder still deletes nothing
    assert!(pxl_in(&d, &["export", "zone.pxl", "--out", "scratch2"]).status.success());
    assert_eq!(before, out_tree(&d));
}
