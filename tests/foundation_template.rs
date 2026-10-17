use agentsprite::cli::TemplateSetArgs;
use agentsprite::color::Rgb;
use agentsprite::model::*;
use agentsprite::output::Out;
use agentsprite::template;
use std::path::{Path, PathBuf};

fn dir(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("templates")).unwrap();
    d
}

const COMMON: &str = r##"
name = "common"
[layers.base]
"." = { role = "transparent" }
"1" = { role = "outline", color = "#182078" }
"##;

const PLAYER: &str = r##"
name = "player"
extends = "templates/common.toml"   # tried from the project root too

[layers.base]
"2" = { role = "body", color = "#4850b8" }
"3" = { role = "skin", color = "#f0b090" }

[layers.detail]
"W" = { role = "white", color = "#f8f8f8", under = "3" }
"R" = { role = "shoe", color = "#f80000", under = "1" }

[map]
"#000000" = "1"
"#242492" = "2"
"#ffffff" = { key = "W", near = ["#000000", 2] }
"##;

fn setup(name: &str) -> (PathBuf, PathBuf) {
    let d = dir(name);
    std::fs::write(d.join("templates/common.toml"), COMMON).unwrap();
    std::fs::write(d.join("templates/player.toml"), PLAYER).unwrap();
    let p = d.join("templates/player.toml");
    (d, p)
}

#[test]
fn extends_layers_in_order_and_map() {
    let (_d, p) = setup("foundation_tpl_a");
    let t = template::load(&p).unwrap();
    assert_eq!(t.name, "player");
    let names: Vec<_> = t.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["base", "detail"]);
    let base: Vec<u8> = t.layers[0].keys.iter().map(|k| k.key).collect();
    assert_eq!(base, b"123", "parent keys first, then child keys");
    assert_eq!(t.key_for_role("outline"), Some(b'1'));
    assert_eq!(t.key_for_role("white"), Some(b'W'));
    assert_eq!(t.layer_of(b'W'), Some(1));
    assert_eq!(t.layer_of(b'1'), Some(0));
    assert_eq!(t.def(b'W').unwrap().under, Some(b'3'));
    assert_eq!(t.def(b'.').unwrap().role, "transparent");
    assert_eq!(t.resolve("skin"), Some(b'3'));
    assert_eq!(t.resolve("R"), Some(b'R'));
    assert_eq!(t.map.len(), 3);
    assert_eq!(t.map[2].near, Some((Rgb::new(0, 0, 0), 2)));
    assert_eq!(t.map[2].key, b'W');
}

fn part_with(path: &Path, rows: &[&str]) -> Part {
    let mut p = Part::new("player", (4, rows.len()));
    p.path = path.to_path_buf();
    let mut g = Grid::new(4, rows.len(), b'.');
    for (y, r) in rows.iter().enumerate() {
        for (x, c) in r.bytes().enumerate() {
            g.set(x as i32, y as i32, c);
        }
    }
    p.frames.push(Frame { name: "01_stand".into(), src: None, locked: false, grid: g });
    p
}

#[test]
fn ac_tp2_missing_key_names_file_frame_row_col() {
    let (_d, tp) = setup("foundation_tpl_b");
    let t = template::load(&tp).unwrap();
    let ok = part_with(Path::new("player.pxl"), &["12W.", "R33."]);
    template::validate_grid(&t, &ok).unwrap();
    let bad = part_with(Path::new("parts/player.pxl"), &["12W.", "R3x.", "..z."]);
    let e = template::validate_grid(&t, &bad).unwrap_err();
    let l = e.loc.unwrap();
    assert_eq!(l.file, "parts/player.pxl");
    assert_eq!(l.frame.as_deref(), Some("01_stand"));
    assert_eq!((l.row, l.col), (Some(1), Some(2)));
    assert!(e.msg.contains("'x'") && e.msg.contains("1 more"), "{}", e.msg);
    assert!(e.hint.unwrap().contains("pxl set player 01_stand 2,1="));
}

#[test]
fn template_errors_have_the_file() {
    let d = dir("foundation_tpl_c");
    let cases = [
        ("[layers.a]\n\"1\" = { role = \"x\", color = \"nope\" }\n", "invalid colour"),
        ("[layers.a]\n\"1\" = { role = \"x\", colour = \"#000000\" }\n", "unknown key"),
        ("[layers.a]\n\"1\" = { role = \"x\", color = \"#000000\" }\n[layers.b]\n\"1\" = { role = \"y\", color = \"#111111\" }\n", "already defined"),
        ("[layers.a]\n\"1\" = { role = \"x\", color = \"#000000\" }\n[map]\n\"#ffffff\" = \"9\"\n", "not defined"),
        ("bogus = 1\n", "unknown key"),
        ("extends = \"templates/missing.toml\"\n", "not found"),
    ];
    for (i, (txt, want)) in cases.iter().enumerate() {
        let p = d.join(format!("t{}.toml", i));
        std::fs::write(&p, txt).unwrap();
        let e = template::load(&p).unwrap_err();
        assert!(e.msg.contains(want), "case {}: {}", i, e.msg);
        assert!(e.loc.unwrap().file.ends_with(&format!("t{}.toml", i)));
    }
}

#[test]
fn template_set_changes_only_the_color_text() {
    let (_d, p) = setup("foundation_tpl_d");
    let mut out = Out::new(false, None);
    let a = TemplateSetArgs { template: p.display().to_string(), assigns: vec!["2=#112233".into(), "white=#ffffff".into()] };
    template::run_set(&a, &mut out).unwrap();
    let after = std::fs::read_to_string(&p).unwrap();
    assert_eq!(after, PLAYER.replace("#4850b8", "#112233").replace("#f8f8f8", "#ffffff"));
    let t = template::load(&p).unwrap();
    assert_eq!(t.color_of(b'2'), Some(Rgb::new(0x11, 0x22, 0x33)));
    // inherited key cannot be edited here
    let a = TemplateSetArgs { template: p.display().to_string(), assigns: vec!["1=#000000".into()] };
    assert!(template::run_set(&a, &mut out).is_err());
}
