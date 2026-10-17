//! Agent-facing edit ergonomics: show -> patch round trip, previews and `view` PNGs, snippets, move / nudge /
//! outline / fix. Everything runs through the built `pxl` binary on a synthetic project.

mod e2e_common;
use e2e_common::*;
use std::path::{Path, PathBuf};

const P: &str = "parts/p00.pxl";

fn project(tag: &str) -> PathBuf {
    let root = tmpdir(tag);
    gen_project(&root, 1, 3, 5);
    let o = pxl_in(&root, &["load", P]);
    assert!(o.status.success(), "{}", stderr(&o));
    root
}

fn ok(root: &Path, args: &[&str]) -> String {
    let o = pxl_in(root, args);
    assert!(o.status.success(), "pxl {}: {}{}", args.join(" "), stdout(&o), stderr(&o));
    stdout(&o)
}

fn ok_in(root: &Path, args: &[&str], stdin: &str) -> String {
    let o = pxl_stdin(root, args, stdin);
    assert!(o.status.success(), "pxl {}: {}{}", args.join(" "), stdout(&o), stderr(&o));
    stdout(&o)
}

fn rows(root: &Path, frame: &str) -> Vec<String> {
    let v = json_of(&pxl_in(root, &["--json", "show", P, frame]));
    v["rows"].as_array().unwrap().iter().map(|r| r.as_str().unwrap().to_string()).collect()
}

fn file(root: &Path) -> Vec<u8> {
    std::fs::read(root.join(P)).unwrap()
}

fn png_size(p: &Path) -> (u32, u32) {
    let b = std::fs::read(p).unwrap();
    assert_eq!(&b[1..4], b"PNG");
    (u32::from_be_bytes(b[16..20].try_into().unwrap()), u32::from_be_bytes(b[20..24].try_into().unwrap()))
}

#[test]
fn show_output_pasted_into_patch_changes_nothing() {
    let root = project("ux_rt");
    let before = file(&root);
    for region in ["0,0,16,16", "10,2,5,6", "3,3,9,9"] {
        for extra in [vec![], vec!["--raw"], vec!["--ref"]] {
            let mut args = vec!["show", P, "f0", "--region", region];
            args.extend(extra.iter());
            let text = ok(&root, &args);
            let out = ok_in(&root, &["patch", P, "f0", "-"], &text);
            assert!(out.contains("no pixel changed"), "{:?} {}: {}", args, region, out);
            assert_eq!(file(&root), before, "{:?} {}", args, region);
        }
    }
}

#[test]
fn patch_with_rulers_and_labels_places_rows_without_x_y() {
    let root = project("ux_place");
    // a region right of column 10 so the ruler has a tens line
    let text = ok(&root, &["show", P, "f1", "--region", "10,4,4,3"]);
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[0].contains("region 10,4,4,3"), "{}", text);
    // rewrite the three rows
    let mut edited = String::new();
    for l in &lines {
        if let Some((lab, _)) = l.split_once(" |") {
            edited.push_str(&format!("{} |WW1 \n", lab));
        } else {
            edited.push_str(l);
            edited.push('\n');
        }
    }
    let out = ok_in(&root, &["patch", P, "f1", "-"], &edited);
    assert!(out.contains("px changed"), "{}", out);
    let r = rows(&root, "f1");
    for y in 4..7 {
        assert_eq!(&r[y][10..13], "WW1", "row {}: {}", y, r[y]);
    }
    // without header, the ruler alone gives the column (ones digit 2 with tens 1 -> x=12)
    let ruler_only = "    1   \n    2345\n 8 |W   \n";
    ok_in(&root, &["patch", P, "f1", "-"], ruler_only);
    assert_eq!(rows(&root, "f1")[8].as_bytes()[12], b'W');
    // an explicit X Y still moves plain rows; plain rows without X Y are a usage error with a hint
    ok_in(&root, &["patch", P, "f1", "1", "1", "-"], "12\n21\n");
    assert_eq!(&rows(&root, "f1")[2][1..3], "21");
    let o = pxl_stdin(&root, &["patch", P, "f1", "-"], "12\n21\n");
    assert_eq!(o.status.code(), Some(2));
    assert!(stderr(&o).contains("hint:"), "{}", stderr(&o));
    // garbage lines in pasted text are rejected, not skipped
    let o = pxl_stdin(&root, &["patch", P, "f1", "-"], " 4 |12\nnonsense here\n 5 |12\n");
    assert_eq!(o.status.code(), Some(2), "{}", stderr(&o));
}

#[test]
fn write_grid_takes_the_whole_frame_text_of_show() {
    let root = project("ux_wg");
    let before = file(&root);
    let text = ok(&root, &["show", P, "f2"]);
    let out = ok_in(&root, &["write_grid", P, "f2", "-"], &text);
    assert!(out.contains("no pixel changed"), "{}", out);
    assert_eq!(file(&root), before);
    let part = ok(&root, &["show", P, "f2", "--region", "2,2,4,4"]);
    let o = pxl_stdin(&root, &["write_grid", P, "f2", "-"], &part);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("patch"), "{}", stderr(&o));
}

#[test]
fn preview_and_view_pngs_are_small_and_deterministic() {
    let root = project("ux_png");
    let o = pxl_in(&root, &["--json", "set", P, "f0", "5,5=W", "6,5=W", "--preview"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let v = json_of(&o);
    let p = root.join(v["preview"].as_str().unwrap());
    let (w, h) = png_size(&p);
    assert!(w <= 1568 && h <= 1568 && w > 100, "{}x{}", w, h);
    let a = std::fs::read(&p).unwrap();
    // same edit result rendered again (undo then redo) gives the same bytes
    ok(&root, &["undo", P]);
    ok(&root, &["set", P, "f0", "5,5=W", "6,5=W", "--preview"]);
    assert_eq!(std::fs::read(&p).unwrap(), a);
    // custom path, dry run
    let out = ok(&root, &["set", P, "f0", "9,9=W", "--dry-run", "--preview", "pv/x.png"]);
    assert!(out.contains("preview: pv/x.png"), "{}", out);
    assert!(root.join("pv/x.png").is_file());
    // view
    let v = json_of(&pxl_in(&root, &["--json", "view", P, "f0"]));
    let vp = root.join(v["path"].as_str().unwrap());
    let (w, h) = png_size(&vp);
    assert!(w <= 1568 && h <= 1568);
    let first = std::fs::read(&vp).unwrap();
    ok(&root, &["view", P, "f0", "--ref"]);
    assert_eq!(std::fs::read(&vp).unwrap(), first);
    let v = json_of(&pxl_in(&root, &["--json", "view", P, "f0", "--zoom", "3", "--region", "0,0,8,8", "--out", "v2.png"]));
    assert_eq!(v["zoom"], 3);
    assert!(png_size(&root.join("v2.png")).0 < 1568);
    // zoom that cannot fit is an error, not a huge image
    let o = pxl_in(&root, &["view", P, "f0", "--zoom", "60"]);
    assert_eq!(o.status.code(), Some(2));
}

#[test]
fn edit_echo_shows_reference_letters_for_small_edits_within_the_line_limit() {
    let root = project("ux_ref");
    let out = ok(&root, &["set", P, "f0", "8,8=W"]);
    assert!(out.contains("legend:"), "reference echo expected: {}", out);
    assert!(out.lines().count() < 15, "{}", out);
    let out = ok(&root, &["set", P, "f0", "8,8=3", "--no-ref"]);
    assert!(!out.contains("legend:") && out.lines().count() < 15, "{}", out);
    let out = ok(&root, &["rect", P, "f0", "2,2,6,6", "2", "--ref"]);
    assert!(out.contains("legend:"), "{}", out);
    // a bigger edit does not get the reference by default
    let out = ok(&root, &["rect", P, "f0", "1,1,12,12", "1"]);
    assert!(!out.contains("legend:"), "{}", out);
}

#[test]
fn snippets_save_stamp_flip_and_undo() {
    let root = project("ux_snip");
    ok_in(&root, &["patch", P, "f0", "2", "2", "-"], "W1 \n 2W\n");
    let before = file(&root);
    let out = ok(&root, &["snippet", "save", P, "f0", "eye", "--region", "2,2,3,2"]);
    assert!(out.contains("saved eye"), "{}", out);
    let snip = root.join("templates/snippets/eye.txt");
    let txt = std::fs::read_to_string(&snip).unwrap();
    assert_eq!(txt.lines().count(), 2);
    assert!(txt.chars().all(|c| c == '\n' || c == ' ' || "123W".contains(c)), "'.' is saved as a space: {:?}", txt);
    let list = ok(&root, &["snippet", "list", P]);
    assert!(list.contains("eye 3x2"), "{}", list);
    // stamp is an edit: rows land at the position, unchanged cells stay, undo restores the file exactly
    let b2 = file(&root);
    assert_eq!(b2, before, "saving a snippet does not touch the part");
    let out = ok(&root, &["stamp", P, "f1", "eye", "9,9"]);
    assert!(out.contains("stamp") && out.contains("px changed"), "{}", out);
    let r = rows(&root, "f1");
    assert_eq!(&r[9][9..11], "W1");
    assert_eq!(&r[10][10..12], "2W");
    ok(&root, &["stamp", P, "f2", "eye", "9,9", "--flip", "x"]);
    let r = rows(&root, "f2");
    assert_eq!(&r[9][10..12], "1W", "{}", r[9]);
    assert_eq!(&r[10][9..11], "W2", "{}", r[10]);
    ok(&root, &["stamp", P, "f2", "eye", "4,4", "--flip", "y"]);
    assert_eq!(&rows(&root, "f2")[4][5..7], "2W");
    // locked by the edit
    let st = ok(&root, &["show", P, "f1"]);
    assert!(st.lines().next().unwrap().contains("locked"), "{}", st);
    ok(&root, &["undo", P, "--steps", "3"]);
    assert_eq!(file(&root), before);
    // errors: unknown name with a suggestion, canvas overflow with the nearest valid position
    let o = pxl_in(&root, &["stamp", P, "f1", "ey", "1,1"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(stderr(&o).contains("did you mean 'eye'"), "{}", stderr(&o));
    let o = pxl_in(&root, &["stamp", P, "f1", "eye", "15,15"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("nearest valid is 13,14"), "{}", stderr(&o));
    assert_eq!(file(&root), before);
    // apply op
    ok_in(&root, &["apply", P, "-"], r#"[{"op":"stamp","frame":"f1","name":"eye","at":[1,1],"flip":"x"},{"op":"stamp","frame":"f2","name":"eye","at":[1,1]}]"#);
    ok(&root, &["undo", P]);
    assert_eq!(file(&root), before);
    // --solid keeps '.' as transparent
    ok(&root, &["snippet", "save", P, "f0", "eye_solid", "--region", "1,1,4,3", "--solid"]);
    let t = std::fs::read_to_string(root.join("templates/snippets/eye_solid.txt")).unwrap();
    assert!(t.contains('.'), "{:?}", t);
}

fn region_unchanged(a: &[String], b: &[String], keep: impl Fn(usize, usize) -> bool) {
    for y in 0..a.len() {
        for x in 0..a[y].len() {
            if keep(x, y) {
                assert_eq!(a[y].as_bytes()[x], b[y].as_bytes()[x], "pixel {},{} changed outside the target", x, y);
            }
        }
    }
}

#[test]
fn move_and_nudge_are_local_and_undo_is_byte_identical() {
    let root = project("ux_move");
    ok_in(&root, &["patch", P, "f0", "2", "2", "-"], "W1\n2W\n");
    let before = file(&root);
    let r0 = rows(&root, "f0");
    let o1 = rows(&root, "f1");
    let out = ok(&root, &["move", P, "f0", "--region", "2,2,2,2", "--by", "3,1"]);
    assert!(out.contains("move") && out.contains("px changed"), "{}", out);
    let r1 = rows(&root, "f0");
    assert_eq!(&r1[3][5..7], "W1");
    assert_eq!(&r1[4][5..7], "2W");
    assert_eq!(&r1[2][2..4], "..");
    region_unchanged(&r0, &r1, |x, y| !((2..8).contains(&x) && (2..5).contains(&y)));
    assert_eq!(rows(&root, "f1"), o1, "other frames untouched");
    // fill the vacated cells with a key / role
    ok(&root, &["undo", P]);
    ok(&root, &["move", P, "f0", "--region", "2,2,2,2", "--by", "-1,0", "--fill", "body"]);
    let r2 = rows(&root, "f0");
    assert_eq!(&r2[2][1..3], "W1");
    assert_eq!(r2[2].as_bytes()[3], b'2');
    ok(&root, &["undo", P]);
    // moving pixels off the canvas is an error and writes nothing
    let o = pxl_in(&root, &["move", P, "f0", "--region", "2,2,2,2", "--by", "-3,0"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("outside"), "{}", stderr(&o));
    assert_eq!(file(&root), before);
    // nudge: whole frame, only that frame; blank margins make room
    let g = rows(&root, "f2");
    let shape = g.iter().map(|r| r.trim_matches('.').len()).collect::<Vec<_>>();
    assert!(shape.iter().any(|n| *n > 0));
    let o = pxl_in(&root, &["nudge", P, "f2", "0,-1"]);
    let ok_nudge = o.status.success();
    if ok_nudge {
        let n = rows(&root, "f2");
        assert_eq!(&n[..15], &g[1..], "shifted up by one");
        assert_eq!(rows(&root, "f1"), o1);
        ok(&root, &["undo", P]);
        assert_eq!(file(&root), before);
    } else {
        assert_eq!(o.status.code(), Some(1));
    }
    // several frames in one call, one undo step
    ok(&root, &["nudge", P, "f*", "0,0"]);
    ok_in(&root, &["apply", P, "-"], r#"[{"op":"move","frame":"f1","region":[2,2,2,2],"by":[0,0]},{"op":"nudge","frame":"f0","by":[0,0]}]"#);
}

fn blob_without_outline(root: &Path) {
    // a 6x5 body with a transparent border, no outline
    let mut text = String::new();
    for y in 0..16 {
        for x in 0..16 {
            text.push(if (4..10).contains(&x) && (5..10).contains(&y) { '2' } else { '.' });
        }
        text.push('\n');
    }
    ok_in(root, &["write_grid", P, "f0", "-"], &text);
}

#[test]
fn outline_closes_the_ring_locally_and_is_idempotent() {
    let root = project("ux_outline");
    blob_without_outline(&root);
    let before = file(&root);
    let r0 = rows(&root, "f0");
    let o1 = rows(&root, "f1");
    ok(&root, &["outline", P, "f0"]);
    let r1 = rows(&root, "f0");
    // every transparent cell next to the body (4-neighbourhood) became outline, nothing else moved
    for y in 0..16usize {
        for x in 0..16usize {
            let c0 = r0[y].as_bytes()[x];
            let c1 = r1[y].as_bytes()[x];
            if c0 != b'.' {
                assert_eq!(c0, c1, "body pixel {},{} changed", x, y);
            } else if c1 != b'.' {
                assert_eq!(c1, b'1', "new pixel {},{} is not the outline key", x, y);
                assert!((3..=10).contains(&x) && (4..=10).contains(&y), "{},{} is not next to the body", x, y);
            }
        }
    }
    // closed: no body pixel touches a transparent cell sideways
    for y in 1..15usize {
        for x in 1..15usize {
            if r1[y].as_bytes()[x] == b'2' {
                for (dx, dy) in [(0i32, 1i32), (1, 0), (0, -1), (-1, 0)] {
                    assert_ne!(r1[(y as i32 + dy) as usize].as_bytes()[(x as i32 + dx) as usize], b'.', "gap at {},{}", x, y);
                }
            }
        }
    }
    assert_eq!(rows(&root, "f1"), o1);
    let again = ok(&root, &["outline", P, "f0"]);
    assert!(again.contains("no pixel changed"), "{}", again);
    ok(&root, &["undo", P]);
    assert_eq!(file(&root), before);
    // an explicit key by role name, on a selector
    ok(&root, &["outline", P, "f0", "--key", "outline"]);
    assert_eq!(rows(&root, "f0"), r1);
}

#[test]
fn fix_repairs_orphans_only_in_the_target_frame_and_undoes() {
    let root = project("ux_fix");
    blob_without_outline(&root);
    ok(&root, &["set", P, "f0", "14,14=2"]); // an orphan far from the body
    let before = file(&root);
    let o1 = rows(&root, "f1");
    let dry = ok(&root, &["fix", P, "f0", "--rules", "orphan", "--dry-run"]);
    assert!(dry.contains("fixed orphan at 14,14"), "{}", dry);
    assert_eq!(file(&root), before);
    let out = ok(&root, &["fix", P, "f0", "--rules", "orphan,jaggy"]);
    assert!(out.contains("fixed orphan at 14,14") && out.contains("1 px changed"), "{}", out);
    assert_eq!(rows(&root, "f0")[14].as_bytes()[14], b'.');
    assert_eq!(rows(&root, "f1"), o1);
    ok(&root, &["undo", P]);
    assert_eq!(file(&root), before);
    // nothing to fix is not an error
    let none = ok(&root, &["fix", P, "f1", "--rules", "orphan"]);
    assert!(none.contains("no pixel changed") || none.contains("nothing to fix"), "{}", none);
    let o = pxl_in(&root, &["fix", P, "f0", "--rules", "bogus"]);
    assert_eq!(o.status.code(), Some(2));
}

#[test]
fn errors_suggest_the_nearest_frame_key_and_pixel() {
    let root = project("ux_hint");
    let o = pxl_in(&root, &["set", P, "f", "1,1=1"]);
    assert!(stderr(&o).contains("did you mean 'f0'") || stderr(&o).contains("f0"), "{}", stderr(&o));
    let o = pxl_in(&root, &["set", P, "f0", "1,1=bodyy"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("did you mean 'body'"), "{}", stderr(&o));
    let o = pxl_in(&root, &["set", P, "f0", "40,3=1"]);
    assert!(stderr(&o).contains("nearest valid pixel is 15,3"), "{}", stderr(&o));
    // roles work as keys in every command that takes one
    ok(&root, &["rect", P, "f0", "1,1,2,2", "skin"]);
    ok(&root, &["fill", P, "f0", "1,1", "white"]);
    ok(&root, &["line", P, "f0", "5,5", "9,5", "outline"]);
    ok(&root, &["replace", P, "f0", "outline=body"]);
}
