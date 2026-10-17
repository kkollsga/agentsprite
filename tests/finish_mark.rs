//! `pxl finish` (hand-finished without a pixel change) and the approved-part edit guard.

mod e2e_common;
mod tiles_common;
use e2e_common::*;
use std::path::Path;
use tiles_common::synth_level;

fn data(v: &serde_json::Value) -> &serde_json::Value {
    v.get("data").unwrap_or(v)
}

fn raw(d: &Path, part: &str) -> serde_json::Value {
    let o = pxl_in(d, &["--json", "status", part, "--raw"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    data(&json_of(&o))["parts"][0].clone()
}

#[test]
fn finish_marks_frames_shows_in_review_undoes_and_rework_reopens() {
    let d = tmpdir("fm_sprite");
    let paths = gen_project(&d, 1, 2, 5);
    let p = paths[0].display().to_string();
    assert!(pxl_in(&d, &["load", &p]).status.success());
    assert_eq!(raw(&d, &p)["raw"], 2);
    let before = std::fs::read_to_string(&paths[0]).unwrap();
    let o = pxl_in(&d, &["finish", &p, "f0", "--reason", "equals the source within the palette"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    assert_eq!(std::fs::read_to_string(&paths[0]).unwrap(), before, "pixels untouched");
    let s = raw(&d, &p);
    assert_eq!(s["raw"], 1, "{}", s);
    assert_eq!(s["raw_frames"][0]["frame"], "f1");
    // an empty reason is refused
    assert!(!pxl_in(&d, &["finish", &p, "f1", "--reason", " "]).status.success());
    // review.md lists it with the reason
    assert!(pxl_in(&d, &["release", &p, "--state", "done", "--agent", "A"]).status.success());
    let o = pxl_in(&d, &["--json", "review", &p, "--out", "rv"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let md = std::fs::read_dir(d.join("rv")).unwrap().flatten().map(|e| e.path().join("review.md")).find(|p| p.is_file()).expect("review.md");
    let md = std::fs::read_to_string(md).unwrap();
    assert!(md.contains("Marked finished without edit") && md.contains("f0") && md.contains("equals the source within the palette"), "{}", md);
    // undo removes the mark
    assert!(pxl_in(&d, &["undo", &p]).status.success());
    assert_eq!(raw(&d, &p)["raw"], 2);
    // finish again, then a rework note on that frame reopens it
    assert!(pxl_in(&d, &["finish", &p, "f0,f1", "--reason", "ok"]).status.success());
    assert_eq!(raw(&d, &p)["raw"], 0);
    assert!(pxl_in(&d, &["rework", &p, "--frames", "f0", "--note", "not really finished"]).status.success());
    let s = raw(&d, &p);
    assert_eq!(s["raw"], 1, "{}", s);
    assert_eq!(s["raw_frames"][0]["frame"], "f0");
}

#[test]
fn finish_tile_range() {
    let d = tmpdir("fm_tiles");
    let p = synth_level(&d).display().to_string();
    assert!(pxl_in(&d, &["load", &p]).status.success());
    let s = raw(&d, &p);
    let before = s["raw"].as_u64().unwrap();
    let tiles: Vec<u64> = s["raw_frames"][0]["tiles"].as_array().unwrap().iter().map(|t| t.as_u64().unwrap()).collect();
    let t = tiles[0];
    let o = pxl_in(&d, &["finish", &p, "blocks", "--tiles", &t.to_string(), "--reason", "tile is right"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let s = raw(&d, &p);
    assert_eq!(s["raw"].as_u64().unwrap(), before - 1, "{}", s);
    assert!(!pxl_in(&d, &["finish", &p, "blocks", "--tiles", "99999", "--reason", "x"]).status.success());
}

#[test]
fn approved_parts_refuse_edits_until_reopened() {
    let d = tmpdir("fm_approved");
    let paths = gen_project(&d, 1, 2, 7);
    let p = paths[0].display().to_string();
    assert!(pxl_in(&d, &["load", &p]).status.success());
    let o = pxl_in(&d, &["--agent", "R", "approve", &p, "--note", "ok"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    for args in [
        vec!["set", &p, "f0", "0,0=1"],
        vec!["finish", &p, "f0", "--reason", "x"],
        vec!["reload", &p],
        vec!["load", &p],
        vec!["pick", &p, "f0", "vote"],
    ] {
        let o = pxl_in(&d, &args);
        assert!(!o.status.success(), "{:?} should be refused", args);
        let t = format!("{}{}", stdout(&o), stderr(&o));
        assert!(t.contains("part is approved; reopen with `pxl rework"), "{:?}: {}", args, t);
    }
    // the part is untouched, and --force-approved overrides
    let o = pxl_in(&d, &["set", &p, "f0", "0,0=1", "--force-approved"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    // rework reopens it
    let d2 = tmpdir("fm_approved2");
    let p2 = gen_project(&d2, 1, 2, 7)[0].display().to_string();
    assert!(pxl_in(&d2, &["load", &p2]).status.success());
    assert!(pxl_in(&d2, &["--agent", "R", "approve", &p2, "--note", "ok"]).status.success());
    assert!(pxl_in(&d2, &["rework", &p2, "--note", "again"]).status.success());
    assert!(pxl_in(&d2, &["set", &p2, "f0", "0,0=1"]).status.success());
}

#[test]
fn approved_parts_refuse_undo_import_lock_remap_and_tiles_fit() {
    let d = tmpdir("fm_approved_more");
    let paths = gen_project(&d, 1, 2, 7);
    let p = paths[0].display().to_string();
    assert!(pxl_in(&d, &["load", &p]).status.success());
    assert!(pxl_in(&d, &["set", &p, "f0", "0,0=1"]).status.success());
    assert!(pxl_in(&d, &["--agent", "R", "approve", &p, "--note", "ok"]).status.success());
    let src = d.join("in.txt");
    std::fs::write(&src, "0\n").unwrap();
    let srcs = src.display().to_string();
    let before = std::fs::read_to_string(&paths[0]).unwrap();
    for args in [
        vec!["undo", &p],
        vec!["import", &srcs, "--part", &p, "--frame", "f0"],
        vec!["lock", &p, "--all"],
        vec!["unlock", &p, "--all"],
        vec!["template", "remap", &p, "1=2"],
    ] {
        let o = pxl_in(&d, &args);
        assert!(!o.status.success(), "{:?} should be refused", args);
        let t = format!("{}{}", stdout(&o), stderr(&o));
        assert!(t.contains("part is approved; reopen with `pxl rework"), "{:?}: {}", args, t);
    }
    assert_eq!(std::fs::read_to_string(&paths[0]).unwrap(), before, "part untouched");
    assert!(pxl_in(&d, &["undo", &p, "--force-approved"]).status.success());

    let td = tmpdir("fm_approved_tiles");
    let tp = synth_level(&td).display().to_string();
    assert!(pxl_in(&td, &["--agent", "R", "approve", &tp, "--note", "ok"]).status.success());
    let o = pxl_in(&td, &["tiles", "fit", &tp, "--apply"]);
    let t = format!("{}{}", stdout(&o), stderr(&o));
    assert!(!o.status.success() && t.contains("part is approved"), "{}", t);
}
