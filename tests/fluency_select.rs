//! Frame selectors (`walk*`, `6-8`, `tag:walk`) and multi-frame `show` through the binary.

use std::path::{Path, PathBuf};
use std::process::Command;

const TPL: &str = "name = \"t\"\n[layers.base]\n\".\" = { role = \"transparent\" }\n\"1\" = { role = \"outline\", color = \"#182078\" }\n";
const NAMES: &[&str] = &["01_stand", "02_walk1", "03_walk2", "04_walk3", "05_run1", "06_run2", "extra"];

fn setup(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("fluency_{}", name));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("templates")).unwrap();
    std::fs::write(d.join("templates/t.toml"), TPL).unwrap();
    let mut s = "name = \"p\"\nsize = [4, 3]\norigin = [0, 0]\ntemplate = \"templates/t.toml\"\n".to_string();
    for (i, n) in NAMES.iter().enumerate() {
        let mut rows = vec!["....".to_string(); 3];
        rows[1].replace_range(i % 4..i % 4 + 1, "1");
        s.push_str(&format!("\n[[frames]]\nname = \"{}\"\ngrid = \"\"\"\n{}\n\"\"\"\n", n, rows.join("\n")));
    }
    std::fs::write(d.join("p.pxl"), s).unwrap();
    d
}

fn pxl(d: &Path, args: &[&str]) -> (i32, String, String) {
    let o = Command::new(env!("CARGO_BIN_EXE_pxl")).current_dir(d).args(args).output().unwrap();
    (o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).into(), String::from_utf8_lossy(&o.stderr).into())
}

fn shown(d: &Path, sel: &str) -> Vec<String> {
    let (c, o, e) = pxl(d, &["--json", "show", "p", sel]);
    assert_eq!(c, 0, "{} {}", o, e);
    let v: serde_json::Value = serde_json::from_str(&o).unwrap_or_else(|_| panic!("json: {}", o));
    let v = v.get("data").unwrap_or(&v);
    match v.get("frames") {
        Some(a) => a.as_array().unwrap().iter().map(|f| f["frame"].as_str().unwrap().to_string()).collect(),
        None => vec![v["frame"].as_str().unwrap().to_string()],
    }
}

#[test]
fn patterns_ranges_tags_order_and_dedup() {
    let d = setup("sel");
    assert_eq!(shown(&d, "walk*"), ["02_walk1", "03_walk2", "04_walk3"]);
    assert_eq!(shown(&d, "0?_run*"), ["05_run1", "06_run2"]);
    assert_eq!(shown(&d, "3-5"), ["03_walk2", "04_walk3", "05_run1"]);
    assert_eq!(shown(&d, "tag:run"), ["05_run1", "06_run2"]);
    assert_eq!(shown(&d, "tag:walk,2-3,extra"), ["02_walk1", "03_walk2", "04_walk3", "extra"]);
    assert_eq!(shown(&d, "extra,01_stand"), ["01_stand", "extra"]);
    assert_eq!(shown(&d, "02_walk1"), ["02_walk1"]);
}

#[test]
fn unknown_selector_lists_frames_and_tags() {
    let d = setup("err");
    for bad in ["nope*", "tag:fly", "zzz", "5-99"] {
        let (c, _, e) = pxl(&d, &["show", "p", bad]);
        assert_eq!(c, 2, "{}: {}", bad, e);
        assert!(e.contains("p"), "{}", e);
    }
    let (_, _, e) = pxl(&d, &["show", "p", "tag:fly"]);
    assert!(e.contains("tags:") && e.contains("walk(3)") && e.contains("01_stand"), "{}", e);
}

#[test]
fn multi_frame_show_side_by_side() {
    let d = setup("multi");
    let (c, o, e) = pxl(&d, &["show", "p", "walk*"]);
    assert_eq!(c, 0, "{}", e);
    let l: Vec<&str> = o.lines().collect();
    assert!(l[0].contains("3 frames"), "{}", o);
    assert!(l[1].contains("02_walk1") && l[1].contains("03_walk2") && l[1].contains("04_walk3"), "{}", o);
    // one ruler line + 3 rows, all three frames on each row
    assert_eq!(l.len(), 2 + 1 + 3, "{}", o);
    assert!(l[3].starts_with("0 |") && l[4].starts_with("1 |"), "{}", o);
    assert!(l[4].contains(".1..") && l[4].contains("..1.") && l[4].contains("...1"), "{}", o);
    // single frame output unchanged
    let (_, o1, _) = pxl(&d, &["show", "p", "02_walk1"]);
    assert!(o1.starts_with("p/02_walk1 4x3 region 0,0,4,3"), "{}", o1);
}
