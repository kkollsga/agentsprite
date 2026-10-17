//! Queue ordering, status.json, staleness (AC-PA4, AC-PA5), verdict commands.
use agentsprite::{pxl, status};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Instant;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("pxl_qstatus_{}_{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn part_text(name: &str, template: Option<&str>) -> String {
    let t = template.map(|t| format!("template = '{}'\n", t)).unwrap_or_default();
    format!("name = '{}'\nsize = [4, 2]\n{}[[frames]]\nname = 'a'\ngrid = '''\n....\n....\n'''\n[[frames]]\nname = 'b'\nlocked = true\ngrid = '''\n....\n....\n'''\n", name, t)
}

fn pxl_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pxl")).current_dir(dir).args(args).output().unwrap()
}

fn json_of(o: &Output) -> serde_json::Value {
    serde_json::from_slice(&o.stdout).unwrap_or_else(|e| panic!("bad json {}: {}", e, String::from_utf8_lossy(&o.stdout)))
}

fn load_and_record(path: &Path, ev: status::StatusEvent) {
    let p = pxl::load(path).unwrap();
    status::record(path, &p, ev);
}

#[test]
fn ac_pa5_template_change_marks_exactly_its_parts_stale() {
    let d = tmp("pa5");
    std::fs::create_dir_all(d.join("templates")).unwrap();
    std::fs::write(d.join("templates/t1.toml"), "name = 't1'\n").unwrap();
    std::fs::write(d.join("templates/t2.toml"), "name = 't2'\n").unwrap();
    for i in 0..6 {
        let t = if i % 2 == 0 { "templates/t1.toml" } else { "templates/t2.toml" };
        let f = d.join(format!("p{}.pxl", i));
        std::fs::write(&f, part_text(&format!("p{}", i), Some(t))).unwrap();
        load_and_record(&f, status::StatusEvent::Loaded);
    }
    let v = json_of(&pxl_in(&d, &["--json", "status", "--stale"]));
    assert_eq!(v["parts"].as_array().unwrap().len(), 0);
    std::fs::write(d.join("templates/t1.toml"), "name = 't1'\n# changed\n").unwrap();
    let v = json_of(&pxl_in(&d, &["--json", "status", "--stale"]));
    let names: Vec<&str> = v["parts"].as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["p0", "p2", "p4"]);
    // reloading clears it
    load_and_record(&d.join("p0.pxl"), status::StatusEvent::Loaded);
    let v = json_of(&pxl_in(&d, &["--json", "status", "--stale"]));
    assert_eq!(v["parts"].as_array().unwrap().len(), 2);
}

#[test]
fn input_change_is_stale_and_export_freshness() {
    let d = tmp("inputs");
    std::fs::write(d.join("in.bin"), [1u8, 2, 3]).unwrap();
    let f = d.join("p.pxl");
    let txt = part_text("p", None).replace("size = [4, 2]\n", "size = [4, 2]\n[input]\nimages = 'in.bin'\n");
    std::fs::write(&f, &txt).unwrap();
    load_and_record(&f, status::StatusEvent::Loaded);
    load_and_record(&f, status::StatusEvent::Exported);
    let row = |d: &Path| json_of(&pxl_in(d, &["--json", "status"]))["parts"][0].clone();
    assert_eq!(row(&d)["stale"], false);
    assert_eq!(row(&d)["export"], "fresh");
    std::fs::write(d.join("in.bin"), [9u8]).unwrap();
    assert_eq!(row(&d)["stale"], true);
    assert_eq!(row(&d)["export"], "stale");
}

#[test]
fn status_file_schema_and_events() {
    let d = tmp("schema");
    let f = d.join("p.pxl");
    std::fs::write(&f, part_text("p", None)).unwrap();
    load_and_record(&f, status::StatusEvent::Loaded);
    let mut m = std::collections::BTreeMap::new();
    m.insert("a".to_string(), serde_json::json!({"score": 0.5, "dE": 3}));
    load_and_record(&f, status::StatusEvent::Metrics(m));
    load_and_record(&f, status::StatusEvent::LintSummary { errors: 1, warnings: 2, infos: 0 });
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.join("p.status.json")).unwrap()).unwrap();
    assert_eq!(v["schema"], 1);
    assert_eq!(v["state"], "todo");
    assert_eq!(v["locked"], serde_json::json!(["b"]));
    assert_eq!(v["metrics"]["a"]["score"], 0.5);
    assert_eq!(v["lint"]["warnings"], 2);
    assert!(v["hashes"]["part"].is_string() && v["hashes"]["loaded_inputs"].is_string());
    let s = pxl_in(&d, &["--json", "status"]);
    let r = &json_of(&s)["parts"][0];
    assert_eq!(r["frames_remaining"], 1);
    assert_eq!(r["frames_total"], 2);
    assert_eq!(r["worst"]["frame"], "a");
}

#[test]
fn next_orders_by_priority_then_worst_metrics_and_respects_states() {
    let d = tmp("next");
    for n in ["a", "b", "c", "d", "e"] {
        std::fs::write(d.join(format!("{}.pxl", n)), part_text(n, None)).unwrap();
    }
    std::fs::write(d.join("pxl.toml"), "[parts]\na = 'a.pxl'\nb = 'b.pxl'\n[parts.c]\npath = 'c.pxl'\npriority = 5\ntags = ['zone_1']\n[parts.d]\npath = 'd.pxl'\n[parts.e]\npath = 'e.pxl'\n").unwrap();
    let score = |n: &str, s: f64| {
        let mut m = std::collections::BTreeMap::new();
        m.insert("a".to_string(), serde_json::json!({ "score": s }));
        load_and_record(&d.join(format!("{}.pxl", n)), status::StatusEvent::Metrics(m));
    };
    score("a", 1.0);
    score("b", 9.0);
    score("c", 0.1);
    score("d", 5.0);
    score("e", 7.0);
    // d is already done, e is blocked
    assert!(pxl_in(&d, &["--agent", "X", "claim", "d.pxl"]).status.success());
    assert!(pxl_in(&d, &["--agent", "X", "release", "d.pxl", "--state", "done"]).status.success());
    assert!(pxl_in(&d, &["--agent", "X", "claim", "e.pxl"]).status.success());
    assert_eq!(pxl_in(&d, &["--agent", "X", "release", "e.pxl", "--state", "blocked"]).status.code(), Some(2), "blocked needs a note");
    assert!(pxl_in(&d, &["--agent", "X", "release", "e.pxl", "--state", "blocked", "--note", "wrong offset"]).status.success());
    let order: Vec<String> = (0..4)
        .map(|i| {
            let o = pxl_in(&d, &["--json", "--agent", &format!("w{}", i), "next"]);
            json_of(&o)["part"].as_str().map(String::from).unwrap_or_else(|| "none".into())
        })
        .collect();
    assert_eq!(order, ["c.pxl", "b.pxl", "a.pxl", "none"]);
    // tag filter
    let o = pxl_in(&d, &["--json", "--agent", "z", "next", "--tag", "nope"]);
    assert!(json_of(&o)["part"].is_null());
    let v = json_of(&pxl_in(&d, &["--json", "status", "--blocked"]));
    assert_eq!(v["parts"][0]["name"], "e");
    assert_eq!(v["parts"][0]["notes"][0]["text"], "wrong offset");
}

#[test]
fn verdicts_review_and_rework_return_to_queue() {
    let d = tmp("verdict");
    std::fs::write(d.join("p.pxl"), part_text("p", None)).unwrap();
    assert!(pxl_in(&d, &["--agent", "A", "claim", "p.pxl"]).status.success());
    assert!(pxl_in(&d, &["--agent", "A", "release", "p.pxl", "--state", "done", "--note", "all good"]).status.success());
    let v = json_of(&pxl_in(&d, &["--json", "review"]));
    assert_eq!(v["parts"][0]["name"], "p");
    assert!(pxl_in(&d, &["approve", "p.pxl"]).status.success());
    assert_eq!(pxl_in(&d, &["--agent", "B", "claim", "p.pxl"]).status.code(), Some(1), "approved parts are not claimable");
    assert!(pxl_in(&d, &["rework", "p.pxl", "--note", "outline broken"]).status.success());
    let v = json_of(&pxl_in(&d, &["--json", "--agent", "B", "next"]));
    assert_eq!(v["part"], "p.pxl");
    assert_eq!(v["task"], "rework");
    assert_eq!(v["review"][1]["text"], "outline broken");
}

#[test]
fn ac_pa4_status_json_on_60_parts_is_fast() {
    let d = tmp("pa4");
    std::fs::create_dir_all(d.join("templates")).unwrap();
    std::fs::write(d.join("templates/t.toml"), "name = 't'\n").unwrap();
    for i in 0..60 {
        let f = d.join(format!("part_{:02}.pxl", i));
        std::fs::write(&f, part_text(&format!("part_{:02}", i), Some("templates/t.toml"))).unwrap();
        load_and_record(&f, status::StatusEvent::Loaded);
        load_and_record(&f, status::StatusEvent::Exported);
    }
    let mut best = u128::MAX;
    for _ in 0..7 {
        let t = Instant::now();
        let o = pxl_in(&d, &["--json", "status"]);
        best = best.min(t.elapsed().as_millis());
        assert_eq!(json_of(&o)["parts"].as_array().unwrap().len(), 60);
    }
    eprintln!("AC-PA4: status --json on 60 parts: best of 7 = {} ms (debug_assertions={})", best, cfg!(debug_assertions));
    let limit = if cfg!(debug_assertions) { 400 } else { 100 };
    assert!(best < limit, "{} ms", best);
}

#[test]
fn status_table_has_a_header_row_explaining_the_counts() {
    let d = tmp("hdr");
    std::fs::write(d.join("a.pxl"), part_text("a", None)).unwrap();
    let o = pxl_in(&d, &["status"]);
    let text = String::from_utf8_lossy(&o.stdout).into_owned();
    let mut lines = text.lines();
    let head = lines.next().unwrap();
    assert!(head.starts_with("PART") && head.contains("TODO/ALL") && head.contains("STATE"), "{}", text);
    assert!(lines.next().unwrap().contains(" 1/2 "), "{}", text);
    assert!(text.contains("TODO/ALL = frames not locked yet"));
}

#[test]
fn manifest_in_pxl_subfolder_is_found_from_the_project_root() {
    let d = tmp("sub");
    std::fs::create_dir_all(d.join("pxl/parts")).unwrap();
    std::fs::write(d.join("pxl/parts/a.pxl"), part_text("a", None)).unwrap();
    std::fs::write(d.join("pxl/pxl.toml"), "[parts.a]\npath = \"parts/a.pxl\"\npriority = 3\n").unwrap();
    let o = pxl_in(&d, &["status", "--json"]);
    let j = json_of(&o);
    let parts = j["parts"].as_array().unwrap();
    assert_eq!(parts.len(), 1, "{}", String::from_utf8_lossy(&o.stdout));
    assert!(parts[0]["path"].as_str().unwrap().ends_with("parts/a.pxl"));
}
