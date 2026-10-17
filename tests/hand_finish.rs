//! T6 tools: the raw-output audit (`status --raw`, `check --hand-finished`), `pattern` and `lineup`.

mod e2e_common;
mod tiles_common;
use e2e_common::*;
use std::path::{Path, PathBuf};
use tiles_common::synth_level;

fn data(v: &serde_json::Value) -> &serde_json::Value {
    v.get("data").unwrap_or(v)
}

fn status_raw(d: &Path, part: &str) -> serde_json::Value {
    let o = pxl_in(d, &["--json", "status", part, "--raw"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    data(&json_of(&o))["parts"][0].clone()
}

#[test]
fn sprite_frames_are_raw_until_touched_and_pick_makes_them_raw_again() {
    let d = tmpdir("hf_sprite");
    let paths = gen_project(&d, 1, 2, 5);
    let p = paths[0].display().to_string();
    let o = pxl_in(&d, &["load", &p]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let s = status_raw(&d, &p);
    assert_eq!(s["raw"], 2, "{}", s);
    let o = pxl_in(&d, &["status", &p, "--raw"]);
    let t = stdout(&o);
    assert!(t.contains("raw") && t.contains("frame f0 is still the automatic output") && t.contains("frame f1"), "{}", t);
    // one hand edit
    assert!(pxl_in(&d, &["set", &p, "f0", "8,8=2"]).status.success());
    // the pixel may already have been '2'; force a visible change
    assert!(pxl_in(&d, &["set", &p, "f0", "0,0=1"]).status.success());
    let s = status_raw(&d, &p);
    assert_eq!(s["raw"], 1, "{}", s);
    assert_eq!(s["raw_frames"][0]["frame"], "f1");
    // check --hand-finished fails while a frame is raw; without the flag it does not mention it
    let o = pxl_in(&d, &["--json", "check", "--manifest", d.join("pxl.toml").to_str().unwrap(), "--hand-finished"]);
    assert!(!o.status.success());
    assert!(stdout(&o).contains("still the automatic output"), "{}", stdout(&o));
    let o = pxl_in(&d, &["--json", "check", "--manifest", d.join("pxl.toml").to_str().unwrap()]);
    assert!(!stdout(&o).contains("still the automatic output"));
    // a whole-frame pick writes automatic output again
    let o = pxl_in(&d, &["pick", &p, "f0", "vote"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    assert_eq!(status_raw(&d, &p)["raw"], 2);
    // undo of the pick: back to the edited frame
    assert!(pxl_in(&d, &["undo", &p]).status.success());
    assert_eq!(status_raw(&d, &p)["raw"], 1);
    // finishing the last frame clears the audit
    assert!(pxl_in(&d, &["set", &p, "f1", "0,0=1"]).status.success());
    assert_eq!(status_raw(&d, &p)["raw"], 0);
    let o = pxl_in(&d, &["--json", "check", "--manifest", d.join("pxl.toml").to_str().unwrap(), "--hand-finished"]);
    assert!(!stdout(&o).contains("still the automatic output"), "{}", stdout(&o));
}

#[test]
fn tile_raw_is_counted_per_tile() {
    let d = tmpdir("hf_tiles");
    let p = synth_level(&d);
    let ps = p.display().to_string();
    assert!(pxl_in(&d, &["load", &ps]).status.success());
    let s = status_raw(&d, &ps);
    let before = s["raw"].as_u64().unwrap();
    assert!(before >= 2, "{}", s);
    let tiles: Vec<u64> = s["raw_frames"][0]["tiles"].as_array().unwrap().iter().map(|t| t.as_u64().unwrap()).collect();
    let t = tiles[0];
    // edit that tile by hand: it leaves the raw list, the others stay
    let o = pxl_in(&d, &["set", &ps, "blocks", "0,0=c", "1,0=c", "2,0=b", "--tile", &t.to_string()]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let s = status_raw(&d, &ps);
    assert_eq!(s["raw"].as_u64().unwrap(), before - 1, "{}", s);
    let now: Vec<u64> = s["raw_frames"][0]["tiles"].as_array().unwrap().iter().map(|t| t.as_u64().unwrap()).collect();
    assert!(!now.contains(&t) && now.len() == tiles.len() - 1);
    let o = pxl_in(&d, &["status", &ps, "--raw"]);
    assert!(stdout(&o).contains("tiles raw:"), "{}", stdout(&o));
}

// ------------------------------------------------------------------ pattern

const TPL: &str = "name = \"t\"\n[layers.base]\n\".\" = { role = \"transparent\" }\n\"1\" = { role = \"outline\", color = \"#182078\" }\n\"2\" = { role = \"body\", color = \"#4850b8\" }\n";

fn pat_setup(name: &str) -> PathBuf {
    let d = tmpdir(name);
    std::fs::create_dir_all(d.join("templates/snippets")).unwrap();
    std::fs::write(d.join("templates/t.toml"), TPL).unwrap();
    std::fs::write(d.join("templates/snippets/chk.txt"), "12\n21\n").unwrap();
    std::fs::write(d.join("templates/snippets/dot.txt"), "1 \n  \n").unwrap();
    let row = ".".repeat(16);
    let grid = vec![row; 8].join("\n");
    std::fs::write(d.join("p.pxl"), format!("name = \"p\"\nsize = [16, 8]\norigin = [0, 0]\ntemplate = \"templates/t.toml\"\n\n[[frames]]\nname = \"f\"\ngrid = \"\"\"\n{}\n\"\"\"\n", grid)).unwrap();
    d
}

fn rows(d: &Path) -> Vec<String> {
    let o = pxl_in(d, &["--json", "show", "p", "f"]);
    data(&json_of(&o))["rows"].as_array().unwrap().iter().map(|r| r.as_str().unwrap().to_string()).collect()
}

#[test]
fn pattern_repeats_a_unit_with_a_fixed_phase() {
    let d = pat_setup("hf_pat");
    let o = pxl_in(&d, &["pattern", "p", "f", "chk", "--region", "0,0,16,8"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let r = rows(&d);
    assert_eq!(r[0], "1212121212121212");
    assert_eq!(r[1], "2121212121212121");
    // a region in the middle keeps the same global phase: neighbouring pieces line up
    assert!(pxl_in(&d, &["undo", "p"]).status.success());
    assert!(pxl_in(&d, &["pattern", "p", "f", "chk", "--region", "3,1,4,2"]).status.success());
    let r = rows(&d);
    assert_eq!(&r[1][3..7], "1212");
    assert_eq!(&r[2][3..7], "2121");
    assert_eq!(&r[0][0..4], "....");
    // phase shifts the unit
    assert!(pxl_in(&d, &["undo", "p"]).status.success());
    assert!(pxl_in(&d, &["pattern", "p", "f", "chk", "--region", "0,0,4,1", "--phase", "1,0"]).status.success());
    assert_eq!(&rows(&d)[0][0..4], "2121");
    // the same pattern twice changes nothing
    let o = pxl_in(&d, &["pattern", "p", "f", "chk", "--region", "0,0,4,1", "--phase", "1,0"]);
    assert!(stdout(&o).contains("no pixel changed"), "{}", stdout(&o));
}

#[test]
fn pattern_mask_unit_rect_apply_and_errors() {
    let d = pat_setup("hf_pat2");
    // a shape of '2' with an outline of '1' around it must keep its outline under a masked pattern
    assert!(pxl_in(&d, &["rect", "p", "f", "2,1,8,6", "1"]).status.success());
    assert!(pxl_in(&d, &["rect", "p", "f", "3,2,6,4", "2"]).status.success());
    let before = rows(&d);
    let o = pxl_in(&d, &["pattern", "p", "f", "chk", "--region", "0,0,16,8", "--mask", "2"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let after = rows(&d);
    assert_eq!(after[1], before[1], "outline row untouched");
    assert_eq!(&after[0], &before[0], "outside the shape untouched");
    assert_eq!(&after[2][3..9], "212121", "the body takes the unit, phase from the canvas origin");
    // unit taken from the frame itself (before the fill), through apply
    assert!(pxl_in(&d, &["undo", "p"]).status.success());
    let ops = r#"[{"op":"set","frame":"f","pixels":["0,0=1","1,0=2"]},{"op":"pattern","frame":"f","unit":[0,0,2,1],"region":[0,4,16,1]}]"#;
    let o = pxl_stdin(&d, &["apply", "p", "-"], ops);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    assert_eq!(rows(&d)[4], "1212121212121212");
    // a space in a snippet leaves the cell alone
    assert!(pxl_in(&d, &["pattern", "p", "f", "dot", "--region", "12,6,4,2"]).status.success());
    assert_eq!(&rows(&d)[6][12..16], "1.1.");
    assert_eq!(&rows(&d)[7][12..16], "....");
    // errors: no unit, unknown snippet, region off the canvas; nothing written
    let snap = std::fs::read(d.join("p.pxl")).unwrap();
    for args in [vec!["pattern", "p", "f", "--region", "0,0,2,2"], vec!["pattern", "p", "f", "nope", "--region", "0,0,2,2"], vec!["pattern", "p", "f", "chk", "--region", "10,0,9,2"]] {
        let o = pxl_in(&d, &args);
        assert!(!o.status.success(), "{:?}", args);
    }
    assert_eq!(snap, std::fs::read(d.join("p.pxl")).unwrap());
}

#[test]
fn pattern_on_a_tile_uses_tile_coordinates() {
    let d = tmpdir("hf_pat_tile");
    let p = synth_level(&d);
    let ps = p.display().to_string();
    assert!(pxl_in(&d, &["load", &ps]).status.success());
    std::fs::create_dir_all(d.join("snippets")).unwrap();
    std::fs::write(d.join("snippets/bc.txt"), "bc\ncb\n").unwrap();
    let o = pxl_in(&d, &["pattern", &ps, "blocks", "bc", "--tile", "6"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let g = agentsprite::pxl::load(&p).unwrap().frames[0].grid.clone();
    let cell = |t: usize, x: usize, y: usize| g.cells[y * g.w + t * 8 + x];
    assert_eq!((cell(6, 0, 0), cell(6, 1, 0), cell(6, 0, 1), cell(6, 7, 7)), (b'b', b'c', b'c', b'b'));
    assert_ne!(cell(5, 0, 0), b'b', "tile 5 untouched");
    // outside the tile is an error
    let o = pxl_in(&d, &["pattern", &ps, "blocks", "bc", "--tile", "6", "--region", "6,6,4,4"]);
    assert!(!o.status.success());
}

// ------------------------------------------------------------------ lineup

#[test]
fn lineup_is_deterministic_labelled_and_paginated() {
    let d = tmpdir("hf_lineup");
    let paths = gen_project(&d, 2, 3, 9);
    for p in &paths {
        assert!(pxl_in(&d, &["load", p.to_str().unwrap()]).status.success());
    }
    let (a, b) = (paths[0].display().to_string(), paths[1].display().to_string());
    let args1 = ["--json", "lineup", &format!("{}:f0,f1", a), &format!("{}:all", b), "--out", "lu/one.png"];
    let o = pxl_in(&d, &args1);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let v = json_of(&o);
    let v = data(&v);
    assert_eq!(v["items"].as_array().unwrap().len(), 5);
    assert_eq!(v["items"][0], "p00/f0");
    let first = std::fs::read(d.join("lu/one.png")).unwrap();
    let img = agentsprite::png::read_rgba(&d.join("lu/one.png")).unwrap();
    assert!(img.w <= 1568 && img.h <= 1568 && img.w > 100);
    let o = pxl_in(&d, &["lineup", &format!("{}:f0,f1", a), &format!("{}:all", b), "--out", "lu/two.png"]);
    assert!(o.status.success());
    assert_eq!(first, std::fs::read(d.join("lu/two.png")).unwrap(), "same input, same bytes");
    // other options: backdrop, zoom, region
    let o = pxl_in(&d, &["lineup", &a, "--on", "#204060", "--zoom", "3", "--region", "2,2,8,8", "--out", "lu/three.png"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    assert_ne!(std::fs::read(d.join("lu/three.png")).unwrap(), first);
    // many large zoomed items paginate, every page within 1568 px
    let many: Vec<String> = (0..30).map(|_| format!("{}:all", a)).collect();
    let mut args: Vec<&str> = vec!["--json", "lineup"];
    args.extend(many.iter().map(|s| s.as_str()));
    args.extend(["--zoom", "20", "--out", "lu/many.png"]);
    let o = pxl_in(&d, &args);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let v = json_of(&o);
    let pages = data(&v)["pages"].as_array().unwrap().clone();
    assert!(pages.len() >= 2, "{:?}", pages);
    for p in pages {
        assert!(p["w"].as_u64().unwrap() <= 1568 && p["h"].as_u64().unwrap() <= 1568);
        assert!(Path::new(p["file"].as_str().unwrap()).is_file() || d.join(p["file"].as_str().unwrap()).is_file());
    }
    // errors
    assert!(!pxl_in(&d, &["lineup", &format!("{}:nope", a)]).status.success());
    assert!(!pxl_in(&d, &["lineup", &a, "--zoom", "400"]).status.success());
}

// ---- recomputed raw baseline

fn flip(path: &Path, frame: &str, n: usize) {
    let mut p = agentsprite::pxl::load(path).unwrap();
    let f = p.frames.iter_mut().find(|f| f.name == frame).unwrap();
    let mut done = 0;
    for c in f.grid.cells.iter_mut() {
        if done < n && *c != b'.' {
            *c = if *c == b'1' { b'2' } else { b'1' };
            done += 1;
        }
    }
    assert_eq!(done, n);
    agentsprite::pxl::save(path, &p).unwrap();
}

#[test]
fn recompute_marks_method_output_and_near_misses_raw() {
    let d = tmpdir("hf_recompute");
    let paths = gen_project(&d, 1, 3, 11);
    let p = paths[0].display().to_string();
    assert!(pxl_in(&d, &["load", &p]).status.success());
    // an old part: no baseline at all
    let _ = std::fs::remove_file(agentsprite::status::status_path(&paths[0]));
    assert_eq!(status_raw(&d, &p)["raw"], 0);
    flip(&paths[0], "f1", 10);
    flip(&paths[0], "f2", 2);
    let o = pxl_in(&d, &["--json", "status", &p, "--raw", "--recompute", "--tolerance", "2"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let s = status_raw(&d, &p);
    let frames: Vec<String> = s["raw_frames"].as_array().unwrap().iter().map(|f| f["frame"].as_str().unwrap().to_string()).collect();
    assert_eq!(frames, vec!["f0", "f2"], "{}", s);
    let binding = json_of(&o);
    let rec = &data(&binding)["recomputed"][0]["frames"];
    assert_eq!(rec[0]["distance"], 0, "{}", rec);
    assert!(rec[1]["distance"].as_u64().unwrap() <= 2 && !rec[1]["method"].as_str().unwrap().is_empty(), "{}", rec);
    let st = agentsprite::status::read(&paths[0]);
    assert_eq!(st.raw_source.as_deref(), Some("recomputed"));
    // tolerance 0 drops the near miss; the baseline now tracks later edits
    let o = pxl_in(&d, &["--json", "status", &p, "--raw", "--recompute", "--tolerance", "0"]);
    assert!(o.status.success());
    assert_eq!(status_raw(&d, &p)["raw"], 1);
    assert!(pxl_in(&d, &["set", &p, "f0", "0,0=1"]).status.success());
    assert!(pxl_in(&d, &["set", &p, "f0", "1,0=2"]).status.success());
    assert!(status_raw(&d, &p)["raw"].as_u64().unwrap() <= 1);
    // check --hand-finished --recompute reports the raw frame
    let _ = std::fs::remove_file(agentsprite::status::status_path(&paths[0]));
    let o = pxl_in(&d, &["--json", "check", "--manifest", d.join("pxl.toml").to_str().unwrap(), "--hand-finished", "--recompute", "--tolerance", "2"]);
    assert!(!o.status.success());
    assert!(stdout(&o).contains("still the automatic output"), "{}", stdout(&o));
}

// ---- recompute honours the edit journal

fn raw_names(d: &Path, p: &str) -> Vec<String> {
    status_raw(d, p)["raw_frames"].as_array().unwrap().iter().map(|f| f["frame"].as_str().unwrap().to_string()).collect()
}

#[test]
fn recompute_never_flags_journaled_frames_and_is_stable() {
    let d = tmpdir("hf_recompute_journal");
    let paths = gen_project(&d, 1, 3, 11);
    let p = paths[0].display().to_string();
    assert!(pxl_in(&d, &["load", &p]).status.success());
    // f0: a copy of the finished sibling f1 (faithful, so equal to what an exemplar bank would reproduce)
    flip(&paths[0], "f1", 10);
    assert!(pxl_in(&d, &["lock", &p, "f1"]).status.success());
    let o = pxl_in(&d, &["copy", &p, "f1", "f0"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    // f2: a one-pixel hand touch on an otherwise method-equal frame (within any tolerance)
    let o = pxl_in(&d, &["set", &p, "f2", "0,0=1"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let _ = std::fs::remove_file(agentsprite::status::status_path(&paths[0]));
    for tol in [None, Some("50")] {
        let mut a = vec!["--json", "status", &p, "--raw", "--recompute"];
        if let Some(t) = tol {
            a.extend(["--tolerance", t]);
        }
        let o = pxl_in(&d, &a);
        assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
        let names = raw_names(&d, &p);
        assert!(!names.contains(&"f0".to_string()) && !names.contains(&"f2".to_string()), "{:?}", names);
        let st = agentsprite::status::read(&paths[0]);
        assert!(st.edited.contains_key("f0") && st.edited.contains_key("f2"));
    }
    // recompute twice: stable
    let before = std::fs::read_to_string(agentsprite::status::status_path(&paths[0])).unwrap();
    assert!(pxl_in(&d, &["status", &p, "--raw", "--recompute"]).status.success());
    assert_eq!(before, std::fs::read_to_string(agentsprite::status::status_path(&paths[0])).unwrap());
}

#[test]
fn recompute_keeps_untouched_loaded_frames_raw_and_reload_forgets_edits() {
    let d = tmpdir("hf_recompute_untouched");
    let paths = gen_project(&d, 1, 3, 11);
    let p = paths[0].display().to_string();
    assert!(pxl_in(&d, &["load", &p]).status.success());
    assert!(pxl_in(&d, &["set", &p, "f1", "0,0=1"]).status.success());
    let _ = std::fs::remove_file(agentsprite::status::status_path(&paths[0]));
    assert!(pxl_in(&d, &["status", &p, "--raw", "--recompute"]).status.success());
    let names = raw_names(&d, &p);
    assert!(names.contains(&"f0".to_string()) && names.contains(&"f2".to_string()) && !names.contains(&"f1".to_string()), "{:?}", names);
    // a reload regenerates f1: edits before it no longer count
    assert!(pxl_in(&d, &["reload", &p, "--force"]).status.success());
    let _ = std::fs::remove_file(agentsprite::status::status_path(&paths[0]));
    assert!(pxl_in(&d, &["status", &p, "--raw", "--recompute"]).status.success());
    assert!(raw_names(&d, &p).contains(&"f1".to_string()));
}
