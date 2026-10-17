use std::process::Command;

/// Runs `pxl` in an empty temp dir, so commands never touch the repo or the test projects.
fn pxl(args: &[&str]) -> (i32, String, String) {
    let dir = std::env::temp_dir().join(format!("pxl_foundation_cli_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_pxl")).args(args).current_dir(&dir).output().unwrap();
    (o.status.code().unwrap(), String::from_utf8_lossy(&o.stdout).into(), String::from_utf8_lossy(&o.stderr).into())
}

#[test]
fn help_lists_every_command() {
    let (c, out, _) = pxl(&["--help"]);
    assert_eq!(c, 0);
    for cmd in [
        "load", "reload", "template", "set", "row", "patch", "line", "rect", "fill", "replace", "copy", "mirror", "flip",
        "write_grid", "apply", "undo", "clean", "show", "compare", "diff", "lint", "export", "build", "check", "import", "tiles",
        "next", "claim", "renew", "release", "status", "review", "approve", "rework", "guide",
    ] {
        assert!(out.lines().any(|l| l.trim_start().starts_with(&format!("{} ", cmd))), "missing {}", cmd);
    }
}

#[test]
fn json_is_one_object_and_exit_codes_follow_ac_i2() {
    let (c, out, _) = pxl(&["--json", "guide"]);
    assert_eq!(c, 0);
    let v: serde_json::Value = serde_json::from_str(out.trim_end()).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(out.matches('\n').count(), 1);
    let (c, out, _) = pxl(&["--json", "tiles", "fit", "x"]);
    assert_eq!(c, 2);
    let v: serde_json::Value = serde_json::from_str(out.trim_end()).unwrap();
    assert_eq!(v["ok"], false);
    assert_eq!(v["error"]["kind"], "usage");
    let (c, _, _) = pxl(&["set", "p", "f", "bad-pixel"]);
    assert_eq!(c, 2, "clap usage errors exit 2");
    let (c, out, _) = pxl(&["--json", "set", "p", "f", "1,2=W"]);
    assert_eq!(c, 2, "unknown part name is a usage error");
    let v: serde_json::Value = serde_json::from_str(out.trim_end()).unwrap();
    assert_eq!(v["ok"], false);
}

/// True when clap rejected the arguments (as opposed to the command running and failing).
fn is_clap_error(stderr: &str) -> bool {
    stderr.contains("error: unexpected argument")
        || stderr.contains("error: invalid value")
        || stderr.contains("error: the following required arguments")
        || stderr.contains("error: unrecognized subcommand")
        || stderr.contains("\nUsage: pxl")
}

#[test]
fn arg_shapes_from_the_design_parse() {
    // every example from DESIGN 5.2 must get past argument parsing; the command itself may then fail
    // (missing files in the empty temp dir), but never with a clap usage error
    for args in [
        vec!["set", "player", "01_stand", "17,11=W", "18,11=W", "19,11=outline"],
        vec!["row", "player", "01_stand", "16", "13", "1222221"],
        vec!["patch", "player", "01_stand", "12", "6", "-"],
        vec!["line", "player", "01_stand", "9,12", "14,12", "1"],
        vec!["fill", "player", "01_stand", "15,17", "3"],
        vec!["rect", "player", "01_stand", "9,6,6,8", "3", "--outline"],
        vec!["replace", "player", "01_stand", "2=1", "--region", "9,6,6,8"],
        vec!["copy", "player", "01_stand", "02_wait1", "--region", "9,6,14,10"],
        vec!["mirror", "enemy", "03_fly", "x"],
        vec!["write_grid", "player", "05_look_up", "-"],
        vec!["apply", "player", "ops.json", "--dry-run", "--no-lock"],
        vec!["undo", "player", "--steps", "3"],
        vec!["clean", "player", "--orphans", "--jaggies", "--dry-run"],
        vec!["show", "player", "01_stand", "--region", "0,0,8,8", "--ref"],
        vec!["compare", "parts/*.pxl", "--frames", "a,b", "--on", "#112233", "--zoom"],
        vec!["lint", "parts/"],
        vec!["export", "--all"],
        vec!["--agent", "A", "next", "--kind", "sprite", "--tag", "zone_1"],
        vec!["release", "player", "--state", "done", "--note", "ok"],
        vec!["status", "--json", "--stale"],
        vec!["load", "--new", "p", "--input", "refs/", "--size", "32x32", "--origin", "17,16", "--template", "t.toml"],
        vec!["template", "set", "templates/player.toml", "2=#4850b8"],
    ] {
        let (c, o, e) = pxl(&args);
        assert!(!is_clap_error(&e), "{:?} -> {} {}{}", args, c, o, e);
    }
}
