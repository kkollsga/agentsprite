use agentsprite::model::*;
use agentsprite::output::{ErrKind, Loc};
use agentsprite::pxl;
use std::path::Path;

/// A synthetic 32x32 sprite grid: a filled disc with an outline, an eye and a shadow.
fn golden() -> String {
    let mut rows = Vec::new();
    for y in 0..32i32 {
        let mut r = String::new();
        for x in 0..32i32 {
            let d2 = (x - 16) * (x - 16) + (y - 16) * (y - 16);
            r.push(if d2 > 14 * 14 { '.' } else if d2 > 12 * 12 { '1' } else if (x, y) == (20, 12) { 'W' } else if y > 20 { '3' } else { '2' });
        }
        rows.push(r);
    }
    rows.join("\n")
}

fn synthetic() -> Part {
    let mut p = Part::new("player", (32, 32));
    p.profile = Some("gbc-sprite".into());
    p.origin = (17, 16);
    p.template = Some("templates/player.toml".into());
    p.input.images = Some("refs/{frame}.png".into());
    p.input.factor = Some(2);
    p.input.ref_origin = Some((34, 32));
    p.reduce = ReduceCfg { factor: Some(2), phase: Some((-1, 0)), outline: vec!["outline".into()], thin: true, ..Default::default() };
    p.colors.insert("2".into(), "#112233".into());
    let g = pxl::parse_grid(&golden(), 32, 32, &Loc::file("golden")).unwrap();
    p.frames.push(Frame { name: "01_stand".into(), src: Some("Stand".into()), locked: true, grid: g });
    p.frames.push(Frame { name: "02_wait1".into(), src: None, locked: false, grid: Grid::new(32, 32, b'.') });
    p
}

#[test]
fn round_trip_is_fixed_point() {
    let p = synthetic();
    let s1 = pxl::to_string(&p);
    let q = pxl::parse_str(&s1, Path::new("player.pxl")).unwrap();
    assert_eq!(q.frames, p.frames);
    assert_eq!(q.input, p.input);
    assert_eq!(q.reduce, p.reduce);
    assert_eq!(q.colors, p.colors);
    let s2 = pxl::to_string(&q);
    assert_eq!(s1, s2, "parse -> serialize must be byte stable");
    assert!(s1.contains("grid = \"\"\"\n"));
    assert!(s1.contains("images = \"refs/{frame}.png\""));
    assert!(s1.ends_with("\"\"\"\n"));
}

#[test]
fn hand_written_file_normalizes_then_stays() {
    let row = ".".repeat(4);
    let src = format!(
        "# comment\nname='p'\nsize=[4,2]\n[reduce]\nthin=false\nfactor=2\n[[frames]]\nname='a'\nlocked=false\ngrid='''\n{row}\n1..2\n'''\n"
    );
    let p = pxl::parse_str(&src, Path::new("p.pxl")).unwrap();
    let s1 = pxl::to_string(&p);
    let s2 = pxl::to_string(&pxl::parse_str(&s1, Path::new("p.pxl")).unwrap());
    assert_eq!(s1, s2);
    assert!(!s1.contains("thin") && !s1.contains("locked"));
    assert_eq!(p.frames[0].grid.get(3, 1), b'2');
}

#[test]
fn unknown_keys_are_errors_with_position() {
    let src = "name = \"p\"\nsize = [4, 1]\nbogus = 1\n";
    let e = pxl::parse_str(src, Path::new("p.pxl")).unwrap_err();
    assert_eq!(e.kind, ErrKind::Validation);
    assert!(e.msg.contains("bogus") && e.msg.contains("line 3"), "{}", e.msg);
    assert_eq!(e.loc.as_ref().unwrap().file, "p.pxl");
    let src = "name = \"p\"\nsize = [4, 1]\n[reduce]\nfactr = 2\n";
    assert!(pxl::parse_str(src, Path::new("p.pxl")).is_err());
    let src = "name = \"p\"\nsize = [4, 1]\n[[frames]]\nname = \"a\"\nnope = 1\n";
    assert!(pxl::parse_str(src, Path::new("p.pxl")).is_err());
}

#[test]
fn row_length_and_count_errors_name_frame_row_col() {
    let bad_len = "name = \"p\"\nsize = [4, 2]\n[[frames]]\nname = \"a\"\ngrid = \"\"\"\n....\n...\n\"\"\"\n";
    let e = pxl::parse_str(bad_len, Path::new("p.pxl")).unwrap_err();
    let l = e.loc.unwrap();
    assert_eq!((l.file.as_str(), l.frame.as_deref(), l.row, l.col), ("p.pxl", Some("a"), Some(1), Some(3)));
    let bad_count = "name = \"p\"\nsize = [4, 2]\n[[frames]]\nname = \"b\"\ngrid = \"\"\"\n....\n\"\"\"\n";
    let e = pxl::parse_str(bad_count, Path::new("p.pxl")).unwrap_err();
    assert!(e.msg.contains("1 rows, expected 2"), "{}", e.msg);
    assert_eq!(e.loc.unwrap().frame.as_deref(), Some("b"));
    let bad_char = "name = \"p\"\nsize = [4, 1]\n[[frames]]\nname = \"c\"\ngrid = \"\"\"\n.. .\n\"\"\"\n";
    let e = pxl::parse_str(bad_char, Path::new("p.pxl")).unwrap_err();
    assert_eq!(e.loc.unwrap().col, Some(2));
}

#[test]
fn save_and_load_use_atomic_file() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("foundation_pxl_save");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("player.pxl");
    let p = synthetic();
    pxl::save(&path, &p).unwrap();
    let q = pxl::load(&path).unwrap();
    assert_eq!(q.frames, p.frames);
    assert_eq!(q.path, path);
    let names: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    assert_eq!(names, vec!["player.pxl".to_string()], "no temp files left behind");
}
