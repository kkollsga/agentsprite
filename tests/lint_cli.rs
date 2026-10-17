//! AC-L2: `pxl lint --json` schema and exit codes.
use std::path::Path;
use std::process::Command;

fn run(args: &[&str]) -> (String, i32) {
    let o = Command::new(env!("CARGO_BIN_EXE_pxl")).args(args).output().unwrap();
    (String::from_utf8_lossy(&o.stdout).to_string(), o.status.code().unwrap_or(-1))
}

fn fx(rel: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lint").join(rel).display().to_string()
}

#[test]
fn json_schema_and_exit_codes() {
    let (out, code) = run(&["lint", "--json", &fx("rgb_step/pos1.pxl"), &fx("orphan/pos1.pxl")]);
    assert_eq!(code, 1, "{}", out);
    let v: serde_json::Value = serde_json::from_str(out.trim()).expect("exactly one JSON object");
    assert_eq!(v["schema"], 1);
    assert_eq!(v["parts"].as_array().unwrap().len(), 2);
    assert!(v["summary"]["errors"].as_u64().unwrap() >= 1);
    let i = &v["parts"][0]["issues"][0];
    for k in ["rule", "severity", "file", "frame", "x", "y", "message", "fix"] {
        assert!(i.get(k).is_some(), "issue lacks {}", k);
    }
    // advice only: exit 0, counted as a note, never as a warning
    let (out, code) = run(&["lint", "--json", "--rules", "orphan", &fx("orphan/pos1.pxl")]);
    assert_eq!(code, 0, "{}", out);
    let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["summary"]["warnings"], 0);
    assert_eq!(v["summary"]["notes"], 1);
    assert_eq!(v["parts"][0]["issues"][0]["severity"], "advice");
    assert_eq!(v["parts"][0]["issues"][0]["fix"].as_str().unwrap().split(' ').next().unwrap(), "pxl");
    // unknown rule: usage error
    let (_, code) = run(&["lint", "--rules", "nope", &fx("orphan/pos1.pxl")]);
    assert_eq!(code, 2);
    // a directory lints every part inside; family check flags the odd template out
    let (out, code) = run(&["lint", "--json", "--rules", "template", &fx("family/pos2")]);
    assert_eq!(code, 1);
    let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert!(v["summary"]["errors"].as_u64().unwrap() >= 1);
}

#[test]
fn advice_is_opt_in_and_never_fails() {
    // without --advice the look rules are not even run
    let (out, code) = run(&["lint", "--json", &fx("orphan/pos1.pxl")]);
    assert_eq!(code, 0, "{}", out);
    let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert!(v["parts"][0]["issues"].as_array().unwrap().iter().all(|i| i["severity"] != "advice"), "{}", out);
    assert_eq!(v["summary"]["notes"], 0);
    // with --advice the notes appear, still exit 0, neutral wording
    let (out, code) = run(&["lint", "--json", "--advice", &fx("orphan/pos1.pxl")]);
    assert_eq!(code, 0, "{}", out);
    let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert!(v["summary"]["notes"].as_u64().unwrap() >= 1);
    assert_eq!(v["summary"]["warnings"], 0);
    let msg = v["parts"][0]["issues"].as_array().unwrap().iter().find(|i| i["rule"] == "orphan").unwrap()["message"].as_str().unwrap().to_string();
    assert!(msg.contains("possible stray pixel"), "{}", msg);
    // text mode says note(s), not warning(s) for them
    let (out, _) = run(&["lint", "--advice", &fx("orphan/pos1.pxl")]);
    assert!(out.contains("note(s)"), "{}", out);
    // hard rules still fail with --advice
    let (_, code) = run(&["lint", "--advice", &fx("rgb_step/pos1.pxl")]);
    assert_eq!(code, 1);
}
