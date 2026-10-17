//! AC-I1 to AC-I3 over every command: with `--json` stdout is exactly one JSON object (success and error paths),
//! exit codes are 0/1/2/3 as specified, and located errors name file, frame, row and column plus a hint.

mod e2e_common;
use e2e_common::*;

struct Case {
    args: Vec<&'static str>,
    stdin: Option<&'static str>,
    code: i32,
}

fn c(code: i32, args: &[&'static str]) -> Case {
    Case { args: args.to_vec(), stdin: None, code }
}

fn cin(code: i32, args: &[&'static str], stdin: &'static str) -> Case {
    Case { args: args.to_vec(), stdin: Some(stdin), code }
}

/// Runs the case with `--json`; checks the stdout contract; returns the parsed object.
fn run_json(root: &std::path::Path, case: &Case) -> serde_json::Value {
    let mut args = vec!["--json"];
    args.extend(case.args.iter().copied());
    let o = match case.stdin {
        Some(s) => pxl_stdin(root, &args, s),
        None => pxl_in(root, &args),
    };
    let out = stdout(&o);
    let label = format!("pxl {}", args.join(" "));
    assert_eq!(out.matches('\n').count(), 1, "{}: stdout must be exactly one line of JSON, got {:?}", label, out);
    let v: serde_json::Value = serde_json::from_str(out.trim_end()).unwrap_or_else(|e| panic!("{}: stdout is not JSON ({}): {:?}", label, e, out));
    assert!(v.is_object(), "{}: not an object", label);
    assert_eq!(o.status.code(), Some(case.code), "{}: exit code\n{}\n{}", label, out, stderr(&o));
    assert_eq!(v["ok"], case.code == 0, "{}: ok flag\n{}", label, out);
    if case.code != 0 {
        assert!(v["error"]["message"].is_string(), "{}: error.message missing: {}", label, out);
        let kind = match case.code {
            1 => ["validation", "lint"].to_vec(),
            2 => ["usage"].to_vec(),
            _ => ["io"].to_vec(),
        };
        assert!(kind.contains(&v["error"]["kind"].as_str().unwrap()), "{}: error.kind vs exit code: {}", label, out);
    }
    v
}

#[test]
fn every_command_prints_one_json_object_and_uses_the_exit_codes() {
    let root = tmpdir("iface");
    gen_project(&root, 2, 3, 11);
    std::fs::write(root.join("grid.txt"), sprite_grid(16, 16, 4).to_lines().join("\n") + "\n").unwrap();
    std::fs::write(root.join("bad.json"), "{\"ops\": [ {\"op\": \"nope\"} ]}").unwrap();
    std::fs::write(root.join("junk.png"), b"not a png").unwrap();
    std::fs::write(root.join("tiles.unc"), vec![0x11u8; 64]).unwrap();
    let ok_ops = "{\"ops\":[{\"op\":\"set\",\"frame\":\"f0\",\"pixels\":[\"2,2=1\",\"3,2=2\"]},{\"op\":\"line\",\"frame\":\"f1\",\"from\":[1,1],\"to\":[5,1],\"key\":\"1\"}]}";
    let p0 = "parts/p00.pxl";
    let cases: Vec<Case> = vec![
        // guide and usage errors
        c(0, &["guide"]),
        c(2, &["set"]),
        c(2, &["frobnicate"]),
        c(2, &["compare"]),
        c(2, &["tiles", "fit", "x"]),
        // load / reload
        c(0, &["load", p0]),
        c(0, &["load", "parts"]),
        c(2, &["load", "parts/nope.pxl"]),
        c(2, &["load", p0, "--frames", "zzz"]),
        c(0, &["load", "--new", "p99", "--input", "refs/p00/f0.png", "--size", "16x16", "--origin", "8,8", "--template", "templates/t.toml", "--factor", "2", "--out", "parts/p99.pxl"]),
        c(0, &["reload", p0, "--force"]),
        c(0, &["reload", p0, "--try", "vote,contour"]),
        c(2, &["reload", p0, "--method", "bogus"]),
        // src / template
        c(0, &["template", "suggest", p0]),
        c(2, &["template", "suggest", "parts/nope.pxl"]),
        // edits
        c(0, &["set", p0, "f0", "1,1=1", "2,1=outline"]),
        c(1, &["set", p0, "f0", "99,1=1"]),
        c(1, &["set", p0, "f0", "1,1=Q"]),
        c(2, &["set", p0, "nope", "1,1=1"]),
        c(2, &["set", "parts/nope.pxl", "f0", "1,1=1"]),
        c(0, &["row", p0, "f0", "2", "3", "1221"]),
        c(1, &["row", p0, "f0", "14", "3", "1221"]),
        c(0, &["patch", p0, "f0", "4", "4", "12\n21"]),
        c(1, &["patch", p0, "f0", "15", "15", "12\n21"]),
        c(0, &["line", p0, "f0", "1,5", "8,5", "1"]),
        c(1, &["line", p0, "f0", "1,5", "30,5", "1"]),
        c(0, &["rect", p0, "f0", "2,8,4,3", "2"]),
        c(0, &["fill", p0, "f0", "3,9", "3"]),
        c(0, &["replace", p0, "f0", "3=2", "--region", "2,8,4,3"]),
        c(0, &["copy", p0, "f0", "f1", "--region", "0,0,8,8"]),
        c(2, &["copy", p0, "f0", "nope"]),
        c(0, &["mirror", p0, "f1", "x"]),
        c(0, &["flip", p0, "f1", "y", "--region", "0,0,8,8"]),
        cin(0, &["write_grid", p0, "f2", "-"], "................\n................\n................\n................\n................\n................\n................\n................\n................\n................\n................\n................\n................\n................\n................\n.....11.........\n"),
        cin(1, &["write_grid", p0, "f2", "-"], "short\n"),
        cin(0, &["apply", p0, "-"], ok_ops),
        c(1, &["apply", p0, "bad.json"]),
        c(3, &["apply", p0, "missing.json"]),
        c(0, &["candidates", p0, "f0,f1", "--out", "cand"]),
        c(0, &["candidates", p0, "f0", "--methods", "vote,contour", "--out", "cand"]),
        c(2, &["candidates", p0, "f0", "--methods", "bogus", "--out", "cand"]),
        c(2, &["candidates", p0, "nope", "--out", "cand"]),
        c(0, &["pick", p0, "f0", "contour"]),
        c(0, &["pick", p0, "f1", "vote", "--region", "0,0,8,8", "--dry-run"]),
        c(1, &["pick", p0, "f0", "vote", "--region", "10,10,9,9"]),
        c(2, &["pick", p0, "f0", "bogus"]),
        c(2, &["pick", p0, "nope", "vote"]),
        cin(0, &["apply", p0, "-"], "[{\"op\":\"pick\",\"frame\":\"f1\",\"method\":\"auto\"}]"),
        c(0, &["clean", p0, "--orphans", "--jaggies", "--dry-run"]),
        // editux: snippets, stamp, move, nudge, outline, fix, previews, view, show --raw, pasted show text
        c(0, &["snippet", "save", p0, "f0", "sn", "--region", "1,1,3,3"]),
        c(2, &["snippet", "save", p0, "f0", "bad name", "--region", "1,1,3,3"]),
        c(1, &["snippet", "save", p0, "f0", "sn2", "--region", "10,10,9,9"]),
        c(0, &["snippet", "list", p0]),
        c(0, &["stamp", p0, "f0", "sn", "9,9"]),
        c(0, &["stamp", p0, "f1", "sn", "9,9", "--flip", "y"]),
        c(1, &["stamp", p0, "f0", "sn", "15,15"]),
        c(2, &["stamp", p0, "f0", "nope", "1,1"]),
        c(0, &["move", p0, "f0", "--region", "9,9,3,3", "--by", "-1,-1"]),
        c(1, &["move", p0, "f0", "--region", "0,0,16,16", "--by", "-9,0"]),
        c(0, &["nudge", p0, "f1", "0,0"]),
        c(0, &["outline", p0, "f0"]),
        c(2, &["outline", p0, "nope"]),
        c(0, &["fix", p0, "f0", "--rules", "orphan,jaggy"]),
        c(2, &["fix", p0, "f0", "--rules", "bogus"]),
        c(0, &["set", p0, "f0", "1,1=1", "--preview"]),
        c(0, &["set", p0, "f0", "2,1=1", "--ref"]),
        c(0, &["view", p0, "f0"]),
        c(0, &["view", p0, "f0", "--no-ref", "--zoom", "4", "--out", "v.png"]),
        c(2, &["view", p0, "nope"]),
        c(0, &["show", p0, "f0", "--raw"]),
        cin(0, &["patch", p0, "f0", "-"], "p00/f0 16x16 region 2,2,3,2\n    234\n 2 |12 \n 3 | 21\n"),
        cin(2, &["patch", p0, "f0", "-"], "12\n"),
        c(0, &["undo", p0]),
        c(1, &["undo", "parts/p01.pxl"]),
        c(2, &["undo", p0, "--steps", "0"]),
        // read-only
        c(0, &["show", p0, "f0"]),
        c(0, &["show", p0, "f0", "--ref", "--region", "0,0,8,8"]),
        c(2, &["show", p0, "nope"]),
        c(0, &["compare", p0, "--out", "cmp"]),
        c(0, &["compare", p0, "--frames", "f0", "--zoom", "--out", "cmp"]),
        c(2, &["compare", p0, "--frames", "nope", "--out", "cmp"]),
        c(0, &["diff", p0, "f0", "--frames", "f1"]),
        c(2, &["diff", p0, "nope", "--frames", "f1"]),
        c(0, &["lint", "parts/p01.pxl"]),
        c(2, &["lint", "parts/nope.pxl"]),
        // export / build / check / import
        c(0, &["load", "parts/p01.pxl", "--frames", "f0"]),
        c(0, &["export", "parts/p01.pxl"]),
        c(0, &["export", "--all", "--out", "alt"]),
        c(0, &["build"]),
        c(0, &["check"]),
        c(0, &["import", "grid.txt", "--part", "parts/p01.pxl", "--frame", "f0"]),
        c(2, &["import", "grid.txt", "--part", "parts/p01.pxl", "--frame", "f0", "--template", "templates/nope.toml"]),
        c(3, &["import", "missing.txt", "--part", "parts/p01.pxl"]),
        c(1, &["import", "junk.png", "--part", "parts/p01.pxl", "--frame", "f0"]),
        // queue
        c(2, &["next"]),
        c(0, &["--agent", "A", "claim", "parts/p00.pxl"]),
        c(0, &["--agent", "A", "renew", "parts/p00.pxl"]),
        c(1, &["--agent", "B", "claim", "parts/p00.pxl"]),
        c(0, &["--agent", "A", "release", "parts/p00.pxl", "--state", "done", "--note", "n"]),
        c(0, &["--agent", "A", "next"]),
        c(2, &["--agent", "A", "release", "parts/p01.pxl", "--state", "blocked"]),
        c(0, &["--agent", "A", "release", "parts/p01.pxl"]),
        c(0, &["status"]),
        c(0, &["status", "--stale"]),
        c(0, &["review", "--out", "rv"]),
        c(0, &["approve", "parts/p00.pxl"]),
        c(0, &["rework", "parts/p00.pxl", "--note", "again"]),
        c(2, &["approve", "parts/nope.pxl"]),
    ];
    for case in &cases {
        run_json(&root, case);
    }
}

#[test]
fn located_errors_name_file_frame_row_col_and_a_hint() {
    let root = tmpdir("iface_loc");
    gen_project(&root, 1, 2, 2);
    assert!(pxl_in(&root, &["load", "parts/p00.pxl"]).status.success());
    // out-of-canvas pixel: frame + coordinates + hint
    let o = pxl_in(&root, &["--json", "set", "parts/p00.pxl", "f0", "99,3=1"]);
    let v = json_of(&o);
    assert_eq!(o.status.code(), Some(1));
    let e = &v["error"];
    assert!(e["loc"]["file"].as_str().unwrap().contains("p00.pxl"), "{}", v);
    assert_eq!(e["loc"]["frame"], "f0");
    assert!(e["message"].as_str().unwrap().contains("99,3"), "{}", v);
    assert!(e["hint"].is_string(), "{}", v);
    // unknown key in a hand-edited grid: file, frame, row and column (AC-TP2) and a hint
    let p = root.join("parts/p00.pxl");
    let t = std::fs::read_to_string(&p).unwrap();
    let a = t.find("grid = \"\"\"\n").unwrap() + "grid = \"\"\"\n".len();
    let mut bad = t.clone();
    bad.replace_range(a + 3 * 17 + 4..a + 3 * 17 + 5, "Z"); // row 3, col 4 of the first frame
    std::fs::write(&p, &bad).unwrap();
    for args in [vec!["--json", "lint", "parts/p00.pxl"], vec!["--json", "export", "parts/p00.pxl"]] {
        let o = pxl_in(&root, &args);
        assert_eq!(o.status.code(), Some(1), "{:?}: {}", args, stdout(&o));
        let s = stdout(&o);
        assert!(s.contains("\"Z\"") || s.contains("'Z'") || s.contains(" Z"), "{:?}: {}", args, s);
        assert!(s.contains("p00.pxl"), "{:?} names the file: {}", args, s);
        assert!(s.contains("f0"), "{:?} names the frame: {}", args, s);
    }
    let v = json_of(&pxl_in(&root, &["--json", "export", "parts/p00.pxl"]));
    let loc = &v["error"]["loc"];
    let all = v.to_string();
    assert!(all.contains("\"row\":3") && all.contains("\"col\":4"), "row and column of the bad key: {}", all);
    let _ = loc;
    // text mode: error on stderr with location and hint, nothing on stdout
    let o = pxl_in(&root, &["set", "parts/p00.pxl", "nope", "1,1=1"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(stdout(&o).is_empty());
    assert!(stderr(&o).contains("hint:"), "{}", stderr(&o));
}

#[test]
fn common_errors_carry_a_hint() {
    let root = tmpdir("iface_hint");
    gen_project(&root, 1, 2, 2);
    assert!(pxl_in(&root, &["load", "parts/p00.pxl"]).status.success());
    let p = "parts/p00.pxl";
    let cases: Vec<Vec<&str>> = vec![
        vec!["set", p, "f0", "1,1=Q"],
        vec!["set", p, "f0", "99,1=1"],
        vec!["row", p, "f0", "14", "3", "1221"],
        vec!["rect", p, "f0", "10,10,9,9", "1"],
        vec!["set", p, "nope", "1,1=1"],
        vec!["compare", p, "--frames", "5-9"],
        vec!["apply", p, "missing.json"],
        vec!["--agent", "A", "release", p, "--state", "blocked"],
        vec!["claim", "parts/nope.pxl", "--agent", "A"],
        vec!["template", "set", "templates/t.toml", "2=#12345"],
        vec!["check", "--manifest", "nope.toml"],
    ];
    for args in cases {
        let o = pxl_in(&root, &args);
        let code = o.status.code().unwrap();
        assert!((1..=3).contains(&code), "{:?} exit {}", args, code);
        assert!(stdout(&o).is_empty(), "{:?}: errors go to stderr", args);
        assert!(stderr(&o).contains("hint:"), "{:?} needs a hint: {}", args, stderr(&o));
    }
    // out-of-canvas errors name row and column
    let v = json_of(&pxl_in(&root, &["--json", "set", p, "f0", "99,1=1"]));
    assert_eq!(v["error"]["loc"]["row"], 1);
    assert_eq!(v["error"]["loc"]["col"], 99);
}
