//! `pxl template remap` and forced regeneration of parts that store keys the template no longer defines.
use agentsprite::img::{Rgba, RgbaImage};
use agentsprite::model::{Frame, Grid, Part};
use agentsprite::{png, pxl};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

// the template no longer has "W"; the white is now key "4"
const TEMPLATE: &str = r##"
name = "t"
[layers.base]
"1" = { role = "outline", color = "#182078" }
"3" = { role = "skin",    color = "#f0b090" }
"4" = { role = "white",   color = "#f0f0f0" }
[map]
"#182078" = "1"
"#f0b090" = "3"
"#f0f0f0" = "4"
"##;

fn pxl_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pxl")).current_dir(dir).args(args).output().unwrap()
}
fn text(o: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
}

fn setup(tag: &str) -> (PathBuf, PathBuf) {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("remap_{}", tag));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("refs")).unwrap();
    std::fs::write(dir.join("t.toml"), TEMPLATE).unwrap();
    let (w, h, f) = (6usize, 6usize, 2usize);
    let mut p = Part::new("t", (w, h));
    p.path = dir.join("t.pxl");
    p.template = Some("t.toml".into());
    p.input.images = Some("refs/{frame}.png".into());
    p.input.factor = Some(f as u32);
    p.reduce.factor = Some(f as u32);
    for (n, locked) in [("a", false), ("b", true)] {
        let mut img = RgbaImage::new(w * f, h * f);
        for y in 0..h * f {
            for x in 0..w * f {
                img.set(x as i32, y as i32, if (x / f + y / f) % 3 == 0 { Rgba::opaque(0xf0, 0xf0, 0xf0) } else { Rgba::opaque(0xf0, 0xb0, 0x90) });
            }
        }
        png::write_rgba(&dir.join(format!("refs/{}.png", n)), &img).unwrap();
        let mut g = Grid::new(w, h, b'3');
        for y in 0..h {
            for x in 0..w {
                if (x + y) % 3 == 0 {
                    g.cells[y * w + x] = b'W';
                }
            }
        }
        p.frames.push(Frame { name: n.to_string(), src: None, locked, grid: g });
    }
    pxl::save(&p.path, &p).unwrap();
    (dir.clone(), p.path)
}

#[test]
fn remap_rewrites_keys_dry_run_lock_and_undo() {
    let (d, pp) = setup("remap");
    let before = std::fs::read_to_string(&pp).unwrap();
    let o = pxl_in(&d, &["template", "remap", "t.pxl", "W=white", "--dry-run"]);
    assert!(o.status.success(), "{}", text(&o));
    assert_eq!(std::fs::read_to_string(&pp).unwrap(), before, "dry run writes nothing");
    let o = pxl_in(&d, &["template", "remap", "t.pxl", "W=nosuchrole"]);
    assert!(!o.status.success(), "NEW must exist in the template");
    assert_eq!(std::fs::read_to_string(&pp).unwrap(), before);
    let o = pxl_in(&d, &["template", "remap", "t.pxl", "W=white", "--frames", "a"]);
    assert!(o.status.success(), "{}", text(&o));
    let p = pxl::load(&pp).unwrap();
    assert!(!p.frames[0].grid.cells.contains(&b'W') && p.frames[0].grid.cells.contains(&b'4'));
    assert!(p.frames[1].grid.cells.contains(&b'W'), "frame b was not selected");
    let o = pxl_in(&d, &["template", "remap", "t.pxl", "W=4"]);
    assert!(o.status.success(), "{}", text(&o));
    let p = pxl::load(&pp).unwrap();
    assert!(p.frames.iter().all(|f| !f.grid.cells.contains(&b'W')));
    assert!(p.frames[1].locked && !p.frames[0].locked, "remap keeps lock flags");
    let o = pxl_in(&d, &["lint", "t.pxl"]);
    assert!(!text(&o).contains("not defined in template"), "{}", text(&o));
    let o = pxl_in(&d, &["undo", "t.pxl"]);
    assert!(o.status.success(), "{}", text(&o));
    assert!(pxl::load(&pp).unwrap().frames[1].grid.cells.contains(&b'W'));
}

#[test]
fn forced_reload_skips_unknown_keys_only_in_regenerated_frames() {
    let (d, pp) = setup("force");
    let before = std::fs::read_to_string(&pp).unwrap();
    // plain reload and load refuse the part
    let o = pxl_in(&d, &["reload", "t.pxl"]);
    assert!(!o.status.success());
    assert!(text(&o).contains("template remap"), "{}", text(&o));
    // forcing only frame a leaves b (still holding W) unvalidated -> clear error naming frame b, key, count, remap
    let o = pxl_in(&d, &["load", "t.pxl", "--frames", "a", "--force"]);
    let t = text(&o);
    assert!(!o.status.success());
    assert!(t.contains("b 'W' x12") && t.contains("template remap t W=<new key>"), "{}", t);
    assert!(!t.contains("a 'W'"), "{}", t);
    assert_eq!(std::fs::read_to_string(&pp).unwrap(), before, "nothing written on refusal");
    // forcing every frame regenerates them even though both stored 'W'
    let o = pxl_in(&d, &["load", "t.pxl", "--force"]);
    assert!(o.status.success(), "{}", text(&o));
    let p = pxl::load(&pp).unwrap();
    assert!(p.frames.iter().all(|f| !f.grid.cells.contains(&b'W') && f.grid.cells.contains(&b'4')));
    // remap then plain reload also works on frame a's twin: restore W in a and force just a after remapping b
    let (d, pp) = setup("force2");
    assert!(pxl_in(&d, &["template", "remap", "t.pxl", "W=white", "--frames", "b"]).status.success());
    let o = pxl_in(&d, &["reload", "t.pxl", "--frames", "a", "--force"]);
    assert!(o.status.success(), "{}", text(&o));
    assert!(pxl::load(&pp).unwrap().frames.iter().all(|f| !f.grid.cells.contains(&b'W')));
}

#[test]
fn lint_and_export_errors_suggest_remap() {
    let (d, _pp) = setup("hints");
    let o = pxl_in(&d, &["lint", "t.pxl"]);
    assert!(text(&o).contains("template remap"), "{}", text(&o));
    let o = pxl_in(&d, &["set", "t", "a", "0,0=Q"]);
    assert!(text(&o).contains("template remap"), "{}", text(&o));
}
