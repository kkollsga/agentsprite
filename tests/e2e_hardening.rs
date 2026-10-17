//! P4 hardening: queue/export manifest agreement, incremental build with `exported_profile`, `import` + `undo`,
//! exemplar key translation, compare page budget.

mod e2e_common;
use e2e_common::*;
use std::path::Path;

#[test]
fn queue_and_export_agree_on_manifest_paths_from_any_cwd() {
    let root = tmpdir("manifest");
    gen_project(&root, 3, 2, 5);
    let sub = root.join("parts");
    let names = |v: &serde_json::Value| -> Vec<String> { v["parts"].as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap().to_string()).collect() };
    assert!(pxl_in(&sub, &["load", "."]).status.success());
    let top = json_of(&pxl_in(&root, &["--json", "status"]));
    let deep = json_of(&pxl_in(&sub, &["--json", "status"]));
    assert_eq!(names(&top), ["p00", "p01", "p02"]);
    assert_eq!(names(&top), names(&deep), "status from parts/ must see the same parts as from the project root");
    // next claims a real file from a different cwd, and export --all from there agrees on the same parts
    let o = pxl_in(&sub, &["--json", "--agent", "A", "next"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let v = json_of(&o);
    let claimed = v["part"].as_str().unwrap();
    assert!(Path::new(claimed).is_file() || sub.join(claimed).is_file(), "claimed path {} must exist", claimed);
    let o = pxl_in(&sub, &["--json", "--agent", "A", "export", "--all"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(json_of(&o)["exported"].as_array().unwrap().len(), 3);
    assert!(root.join("out/p01/f0.png").is_file(), "manifest `out` is relative to the manifest folder, not the cwd");
    assert!(!sub.join("out").exists());
}

#[test]
fn incremental_build_tracks_the_export_profile_without_a_pseudo_output() {
    let root = tmpdir("incr");
    gen_project(&root, 2, 2, 9);
    assert!(pxl_in(&root, &["load", "parts"]).status.success());
    let b1 = json_of(&pxl_in(&root, &["--json", "build"]));
    assert_eq!(b1["built"].as_array().unwrap().len(), 2);
    let st = std::fs::read_to_string(root.join("parts/p00.status.json")).unwrap();
    assert!(!st.contains("(config)"), "no pseudo output: {}", st);
    assert!(st.contains("exported_profile"), "{}", st);
    let b2 = json_of(&pxl_in(&root, &["--json", "build"]));
    assert_eq!(b2["skipped"].as_array().unwrap().len(), 2, "unchanged parts are skipped: {}", b2);
    // a changed output pattern (config) rebuilds that part only
    let m = std::fs::read_to_string(root.join("pxl.toml")).unwrap();
    std::fs::write(root.join("pxl.toml"), m.replace("path = \"parts/p00.pxl\"\n", "path = \"parts/p00.pxl\"\npng = \"{frame}_x.png\"\n")).unwrap();
    let b3 = json_of(&pxl_in(&root, &["--json", "build"]));
    assert_eq!(b3["built"].as_array().unwrap().len(), 1, "{}", b3);
    assert_eq!(b3["skipped"].as_array().unwrap().len(), 1);
    // an edit rebuilds too
    assert!(pxl_in(&root, &["set", "parts/p01.pxl", "f0", "0,0=1"]).status.success());
    let b4 = json_of(&pxl_in(&root, &["--json", "build"]));
    assert_eq!(b4["built"].as_array().unwrap().len(), 1);
    // `status` reports the export as fresh after a build, stale after an edit
    let s = json_of(&pxl_in(&root, &["--json", "status"]));
    assert!(s["parts"].as_array().unwrap().iter().all(|p| p["export"] == "fresh"), "{}", s);
    assert!(pxl_in(&root, &["set", "parts/p01.pxl", "f0", "1,0=1"]).status.success());
    let s = json_of(&pxl_in(&root, &["--json", "status"]));
    assert_eq!(s["parts"][1]["export"], "stale");
}

#[test]
fn undo_removes_frames_added_by_import() {
    let root = tmpdir("impundo");
    gen_project(&root, 1, 2, 3);
    let part = root.join("parts/p00.pxl");
    assert!(pxl_in(&root, &["load", "parts/p00.pxl"]).status.success());
    let before = std::fs::read(&part).unwrap();
    // a grid file for a frame the part does not have, and one for an existing frame
    let g = sprite_grid(16, 16, 77);
    let txt: String = g.to_lines().join("\n") + "\n";
    std::fs::write(root.join("new_frame.txt"), &txt).unwrap();
    std::fs::write(root.join("f0.txt"), &txt).unwrap();
    let o = pxl_in(&root, &["import", "new_frame.txt", "--part", "parts/p00.pxl"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let part_text = std::fs::read_to_string(&part).unwrap();
    assert!(part_text.contains("new_frame"), "frame was added");
    let o = pxl_in(&root, &["undo", "parts/p00.pxl"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    assert_eq!(std::fs::read(&part).unwrap(), before, "undo restores the part byte for byte");
    let journal = std::fs::read_to_string(root.join("parts/p00.journal")).unwrap();
    assert!(journal.contains("\"added\":[\"new_frame\"]"), "{}", journal);
    // mixed: replace f0 and add new_frame in one import call, then undo
    let o = pxl_in(&root, &["import", "f0.txt", "--part", "parts/p00.pxl", "--frame", "f0"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let o = pxl_in(&root, &["import", "new_frame.txt", "--part", "parts/p00.pxl"]);
    assert!(o.status.success());
    assert!(pxl_in(&root, &["undo", "parts/p00.pxl", "--steps", "2"]).status.success());
    assert_eq!(std::fs::read(&part).unwrap(), before);
}

fn exemplar_part(root: &Path, extra_exemplar: &str) {
    // The part's template uses keys 1,2,3,W; the exemplar grid is written in a template that uses a,b,c,d.
    std::fs::create_dir_all(root.join("golden")).unwrap();
    std::fs::write(
        root.join("templates/old.toml"),
        "name = \"old\"\n[layers.base]\n\".\" = { role = \"transparent\" }\n\"a\" = { role = \"outline\", color = \"#182078\" }\n\"b\" = { role = \"body\", color = \"#4850b8\" }\n\"c\" = { role = \"skin\", color = \"#f0b090\" }\n[layers.detail]\n\"d\" = { role = \"white\", color = \"#f0f0f0\", under = \"c\" }\n",
    )
    .unwrap();
    let g = sprite_grid(16, 16, 5); // identical to part p00 frame f0 (seed 5 + 0)
    let foreign: String = g
        .to_lines()
        .iter()
        .map(|l| l.chars().map(|c| match c { '1' => 'a', '2' => 'b', '3' => 'c', 'W' => 'd', o => o }).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    std::fs::write(root.join("golden/f0.txt"), &foreign).unwrap();
    let p = root.join("parts/p00.pxl");
    let t = std::fs::read_to_string(&p).unwrap();
    let t = t.replace("[reduce]\n", &format!("[reduce]\nmethod = \"auto\"\nexemplars = [\"{}\"]\n", extra_exemplar));
    std::fs::write(&p, t).unwrap();
}

#[test]
fn exemplar_with_foreign_keys_is_skipped_with_a_warning_or_translated() {
    let root = tmpdir("exempl");
    gen_project(&root, 1, 3, 5);
    exemplar_part(&root, "../golden/f0.txt");
    // no source template: keys a,b,c,d do not exist in the part's template -> skipped, load still succeeds
    let o = pxl_in(&root, &["--json", "load", "parts/p00.pxl"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let v = json_of(&o);
    let w = v["warnings"].as_array().expect("a warning is reported").iter().map(|w| w.as_str().unwrap()).collect::<Vec<_>>().join("|");
    assert!(w.contains("golden/f0.txt") && w.contains("skipped") && w.contains("no counterpart"), "{}", w);
    let skipped_text = std::fs::read_to_string(root.join("parts/p00.pxl")).unwrap();
    assert!(!skipped_text.contains('a') || !skipped_text.contains("aaa"), "no foreign keys leak into the part");
    // with the source template named, keys are translated by colour and the exemplar is used
    exemplar_part_fix(&root, "../golden/f0.txt@templates/old.toml");
    let o = pxl_in(&root, &["--json", "load", "parts/p00.pxl"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let v = json_of(&o);
    let w = v["warnings"].as_array().map(|a| a.iter().map(|w| w.as_str().unwrap()).collect::<Vec<_>>().join("|")).unwrap_or_default();
    assert!(w.contains("translated 4 key(s)"), "{}", w);
    // frame f0's reference equals the exemplar's, so the translated grid is copied exactly
    let part = agentsprite::pxl::load(&root.join("parts/p00.pxl")).unwrap();
    assert_eq!(part.frame("f0").unwrap().grid, sprite_grid(16, 16, 5));
}

fn exemplar_part_fix(root: &Path, entry: &str) {
    let p = root.join("parts/p00.pxl");
    let t = std::fs::read_to_string(&p).unwrap();
    let a = t.find("exemplars = [").unwrap();
    let b = a + t[a..].find('\n').unwrap();
    std::fs::write(&p, format!("{}exemplars = [\"{}\"]{}", &t[..a], entry, &t[b..])).unwrap();
}

#[test]
fn bench_script_appends_a_record_and_prints_the_delta() {
    let root = tmpdir("bench");
    gen_project(&root, 1, 4, 8);
    assert!(pxl_in(&root, &["load", "parts/p00.pxl"]).status.success());
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/bench.py");
    let run = |label: &str| {
        std::process::Command::new("python3")
            .args(["-I", script.to_str().unwrap(), root.join("parts/p00.pxl").to_str().unwrap(), "--label", label, "--pxl", bin()])
            .output()
    };
    let Ok(o) = run("first") else {
        eprintln!("skip: python3 not available");
        return;
    };
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let o = run("second").unwrap();
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let text = stdout(&o);
    assert!(text.contains("previous: first") && text.contains("mean_iou"), "{}", text);
    let hist = std::fs::read_to_string(root.join("bench/history.jsonl")).unwrap();
    let recs: Vec<serde_json::Value> = hist.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(recs.len(), 2);
    for k in ["label", "date", "mean_iou", "worst_iou", "mean_oklab", "lost_total", "mean_score", "golden_01_stand_diff", "lint_warnings", "lint_errors"] {
        assert!(recs[1].get(k).is_some(), "field {}", k);
    }
    assert_eq!(recs[0]["mean_iou"], recs[1]["mean_iou"], "deterministic");
}
