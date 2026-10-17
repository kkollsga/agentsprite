//! Edit commands through the built `pxl` binary.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const TPL: &str = "name = \"t\"\n[layers.base]\n\".\" = { role = \"transparent\" }\n\"1\" = { role = \"outline\", color = \"#182078\" }\n\"2\" = { role = \"body\", color = \"#4850b8\" }\n";

fn setup(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("edit_cli_{}", name));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("templates")).unwrap();
    std::fs::write(d.join("templates/t.toml"), TPL).unwrap();
    let row = ".".repeat(8);
    let grid = vec![row; 6].join("\n");
    std::fs::write(d.join("p.pxl"), format!("name = \"p\"\nsize = [8, 6]\norigin = [0, 0]\ntemplate = \"templates/t.toml\"\n\n[[frames]]\nname = \"f\"\ngrid = \"\"\"\n{}\n\"\"\"\n", grid)).unwrap();
    d
}

fn pxl(d: &Path, args: &[&str], stdin: Option<&str>) -> (i32, String, String) {
    let mut c = Command::new(env!("CARGO_BIN_EXE_pxl"));
    c.current_dir(d).args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut ch = c.spawn().unwrap();
    if let Some(s) = stdin {
        ch.stdin.take().unwrap().write_all(s.as_bytes()).unwrap();
    } else {
        drop(ch.stdin.take());
    }
    let o = ch.wait_with_output().unwrap();
    (o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).into(), String::from_utf8_lossy(&o.stderr).into())
}

#[test]
fn binary_edit_cycle_with_stdin_json_and_exit_codes() {
    let d = setup("cycle");
    let orig = std::fs::read(d.join("p.pxl")).unwrap();
    let (c, o, e) = pxl(&d, &["set", "p", "f", "3,2=outline", "4,2=2"], None);
    assert_eq!(c, 0, "{} {}", o, e);
    assert!(o.contains("2 px changed"), "{}", o);
    let (c, _, e) = pxl(&d, &["patch", "p", "f", "0", "0", "-"], Some("12\n 1\n"));
    assert_eq!(c, 0, "{}", e);
    let (c, o, _) = pxl(&d, &["--json", "show", "p", "f", "--region", "0,0,6,4"], None);
    assert_eq!(c, 0);
    let v: serde_json::Value = serde_json::from_str(o.trim()).unwrap();
    assert_eq!(v["rows"][0], "12....");
    assert_eq!(v["rows"][1], ".1....");
    assert_eq!(v["rows"][2], "...12.");
    // bad key: exit 1, file untouched
    let before = std::fs::read(d.join("p.pxl")).unwrap();
    let (c, _, e) = pxl(&d, &["set", "p", "f", "1,1=Z"], None);
    assert_eq!(c, 1);
    assert!(e.contains("'Z'") && e.contains("1,1"), "{}", e);
    let (c, _, _) = pxl(&d, &["set", "p", "f", "9,1=1"], None);
    assert_eq!(c, 1);
    assert_eq!(std::fs::read(d.join("p.pxl")).unwrap(), before);
    // apply from stdin, then undo everything
    let (c, o, e) = pxl(&d, &["--json", "apply", "p", "-"], Some(r#"{"ops":[{"op":"line","frame":"f","from":[0,5],"to":[7,5],"key":"1"}]}"#));
    assert_eq!(c, 0, "{} {}", o, e);
    let v: serde_json::Value = serde_json::from_str(o.trim()).unwrap();
    assert_eq!(v["changed"], 8);
    let (c, _, e) = pxl(&d, &["undo", "p", "--steps", "3"], None);
    assert_eq!(c, 0, "{}", e);
    assert_eq!(std::fs::read(d.join("p.pxl")).unwrap(), orig);
    let (c, _, _) = pxl(&d, &["undo", "p"], None);
    assert_eq!(c, 1);
}
