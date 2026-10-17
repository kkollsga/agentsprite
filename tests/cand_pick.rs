//! `pxl candidates` (read-only alternative reductions) and `pxl pick` (write one into a frame): read-only
//! guarantee, page limit, panel == picked pixels for every method, region picks, undo, dry run, apply op, lease.

mod e2e_common;
use agentsprite::model::Method;
use agentsprite::{candidates, pxl, template};
use e2e_common::*;
use std::path::{Path, PathBuf};

const METHODS: [&str; 5] = ["vote", "contour", "exemplar", "auto", "nearest"];
const P: &str = "parts/p00.pxl";

fn project(tag: &str) -> PathBuf {
    let root = tmpdir(tag);
    gen_project(&root, 1, 4, 5);
    assert!(pxl_in(&root, &["load", P]).status.success());
    // f1 is approved work (locked): feeds exemplar/auto; f0 is blanked so a pick has something to change
    assert!(pxl_in(&root, &["set", P, "f1", "3,3=1"]).status.success());
    let blank = vec!["................"; 16].join("\n") + "\n";
    assert!(pxl_stdin(&root, &["write_grid", P, "f0", "-", "--no-lock"], &blank).status.success());
    root
}

fn grid_of(root: &Path, frame: &str) -> Vec<u8> {
    pxl::load(&root.join(P)).unwrap().frame(frame).unwrap().grid.cells.clone()
}

fn png_size(p: &Path) -> (u32, u32) {
    let b = std::fs::read(p).unwrap();
    assert_eq!(&b[1..4], b"PNG");
    (u32::from_be_bytes(b[16..20].try_into().unwrap()), u32::from_be_bytes(b[20..24].try_into().unwrap()))
}

#[test]
fn candidates_is_read_only_and_pages_stay_within_1568() {
    let root = project("cand_ro");
    let out = tmpdir("cand_ro_out");
    let before = tree(&root, &[]);
    let bytes = std::fs::read(root.join(P)).unwrap();
    let o = pxl_in(&root, &["--json", "candidates", P, "*", "--out", out.to_str().unwrap()]);
    assert!(o.status.success(), "{}", stderr(&o));
    let v = json_of(&o);
    assert_eq!(v["frames"].as_array().unwrap().len(), 4);
    assert_eq!(tree(&root, &[]), before, "the project directory is unchanged");
    assert_eq!(std::fs::read(root.join(P)).unwrap(), bytes);
    let imgs = v["images"].as_array().unwrap();
    assert!(!imgs.is_empty());
    for i in imgs {
        let (w, h) = png_size(Path::new(i.as_str().unwrap()));
        assert!(w <= 1568 && h <= 1568 && w > 100 && h > 50, "{}x{}", w, h);
    }
    // text mode lists paths and a score row per frame
    let o = pxl_in(&root, &["candidates", P, "f0,f1", "--methods", "vote,contour", "--out", out.to_str().unwrap()]);
    let t = stdout(&o);
    assert!(t.contains(".png") && t.contains("f0") && t.contains("f1") && t.contains('*'), "{}", t);
    assert!(!out.join("compare").exists());
    // bad inputs
    assert_eq!(pxl_in(&root, &["candidates", P, "f0", "--methods", "bogus"]).status.code(), Some(2));
    assert_eq!(pxl_in(&root, &["candidates", P, "nope"]).status.code(), Some(2));
}

#[test]
fn many_frames_go_on_few_pages() {
    let root = tmpdir("cand_pages");
    gen_project(&root, 1, 12, 9);
    assert!(pxl_in(&root, &["load", P]).status.success());
    let out = tmpdir("cand_pages_out");
    let v = json_of(&pxl_in(&root, &["--json", "candidates", P, "*", "--out", out.to_str().unwrap()]));
    let imgs = v["images"].as_array().unwrap();
    assert!(imgs.len() <= 4, "{} pages", imgs.len());
    for i in imgs {
        let (w, h) = png_size(Path::new(i.as_str().unwrap()));
        assert!(w <= 1568 && h <= 1568, "{}x{}", w, h);
    }
}

#[test]
fn pick_writes_exactly_the_reduction_the_library_gives_for_every_method() {
    for m in METHODS {
        let root = project(&format!("cand_pick_{}", m));
        let part = pxl::load(&root.join(P)).unwrap();
        let tpl = template::load_for(&part).unwrap();
        let method = Method::parse(m).unwrap();
        let bank = candidates::bank_for(&part, &tpl, &[method]).unwrap();
        let (want, _) = candidates::reduce_frame(&part, &tpl, 0, method, bank.as_ref()).unwrap();
        let o = pxl_in(&root, &["pick", P, "f0", m, "--no-lock"]);
        assert!(o.status.success(), "{}: {}", m, stderr(&o));
        assert_eq!(grid_of(&root, "f0"), want.cells, "pick {} == reduction", m);
        // and the candidates panel of that method is now identical to the current frame
        let out = tmpdir("cand_same");
        let v = json_of(&pxl_in(&root, &["--json", "candidates", P, "f0", "--out", out.to_str().unwrap()]));
        assert_eq!(v["frames"][0]["methods"][m]["same_as"], "current", "{}: {}", m, v);
        let part = pxl::load(&root.join(P)).unwrap();
        assert!(!part.frame("f0").unwrap().locked, "--no-lock");
    }
}

#[test]
fn pick_locks_reports_the_region_and_is_deterministic() {
    let root = project("cand_lock");
    let o = pxl_in(&root, &["pick", P, "f0", "contour"]);
    let s = stdout(&o);
    assert!(o.status.success() && s.contains("px changed") && s.contains("region"), "{}{}", s, stderr(&o));
    assert!(pxl::load(&root.join(P)).unwrap().frame("f0").unwrap().locked);
    let again = grid_of(&root, "f0");
    // second run on a copy gives identical bytes
    let root2 = project("cand_lock2");
    assert!(pxl_in(&root2, &["pick", P, "f0", "contour"]).status.success());
    assert_eq!(grid_of(&root2, "f0"), again);
}

#[test]
fn region_pick_changes_nothing_outside_the_region() {
    let root = project("cand_region");
    let full = {
        let r2 = project("cand_region_full");
        assert!(pxl_in(&r2, &["pick", P, "f0", "vote"]).status.success());
        grid_of(&r2, "f0")
    };
    assert!(pxl_in(&root, &["pick", P, "f0", "vote", "--region", "4,5,6,7"]).status.success());
    let got = grid_of(&root, "f0");
    for y in 0..16usize {
        for x in 0..16usize {
            let inside = (4..10).contains(&x) && (5..12).contains(&y);
            assert_eq!(got[y * 16 + x], if inside { full[y * 16 + x] } else { b'.' }, "cell {},{}", x, y);
        }
    }
    assert!(got.iter().any(|&c| c != b'.'), "something was picked");
    // out of canvas: validation error, file untouched
    let bytes = std::fs::read(root.join(P)).unwrap();
    let o = pxl_in(&root, &["pick", P, "f0", "vote", "--region", "10,10,9,9"]);
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(std::fs::read(root.join(P)).unwrap(), bytes);
    // unknown method / frame are usage errors
    assert_eq!(pxl_in(&root, &["pick", P, "f0", "bogus"]).status.code(), Some(2));
    assert_eq!(pxl_in(&root, &["pick", P, "nope", "vote"]).status.code(), Some(2));
}

#[test]
fn undo_restores_the_bytes_and_dry_run_writes_nothing() {
    let root = project("cand_undo");
    let bytes = std::fs::read(root.join(P)).unwrap();
    let o = pxl_in(&root, &["pick", P, "f0", "auto", "--dry-run"]);
    assert!(o.status.success() && stdout(&o).contains("dry run"), "{}", stdout(&o));
    assert_eq!(std::fs::read(root.join(P)).unwrap(), bytes, "dry run");
    assert!(pxl_in(&root, &["pick", P, "f0", "auto"]).status.success());
    assert_ne!(std::fs::read(root.join(P)).unwrap(), bytes);
    assert!(pxl_in(&root, &["set", P, "f2", "1,1=1"]).status.success());
    assert!(pxl_in(&root, &["undo", P, "--steps", "2"]).status.success());
    assert_eq!(std::fs::read(root.join(P)).unwrap(), bytes, "undo of a pick and a set");
}

#[test]
fn apply_pick_op_batches_atomically_with_other_edits() {
    let root = project("cand_apply");
    let bytes = std::fs::read(root.join(P)).unwrap();
    let ops = r#"{"ops":[{"op":"pick","frame":"f0","method":"vote","region":[0,0,16,8]},{"op":"set","frame":"f2","pixels":["1,1=1"]},{"op":"pick","frame":"f3","method":"contour"}]}"#;
    std::fs::write(root.join("ops.json"), ops).unwrap();
    let o = pxl_in(&root, &["--json", "apply", P, "ops.json"]);
    assert!(o.status.success(), "{}", stdout(&o));
    assert_eq!(json_of(&o)["frames"].as_array().unwrap().len(), 2, "f3 already equals its contour reduction: no change");
    assert!(pxl_in(&root, &["undo", P]).status.success(), "one undo step");
    assert_eq!(std::fs::read(root.join(P)).unwrap(), bytes);
    // a bad method / extra field aborts the whole batch
    for bad in [
        r#"[{"op":"set","frame":"f2","pixels":["1,1=1"]},{"op":"pick","frame":"f0","method":"nope"}]"#,
        r#"[{"op":"set","frame":"f2","pixels":["1,1=1"]},{"op":"pick","frame":"f0","method":"vote","x":1}]"#,
        r#"[{"op":"set","frame":"f2","pixels":["1,1=1"]},{"op":"pick","frame":"f0","method":"vote","region":[9,9,9,9]}]"#,
    ] {
        std::fs::write(root.join("bad.json"), bad).unwrap();
        assert_eq!(pxl_in(&root, &["apply", P, "bad.json"]).status.code(), Some(1));
        assert_eq!(std::fs::read(root.join(P)).unwrap(), bytes, "{}", bad);
    }
}

#[test]
fn pick_respects_the_lease_but_candidates_always_works() {
    let root = project("cand_lease");
    assert!(pxl_in(&root, &["--agent", "A", "claim", P]).status.success());
    let bytes = std::fs::read(root.join(P)).unwrap();
    for args in [vec!["pick", P, "f0", "vote"], vec!["--agent", "B", "pick", P, "f0", "vote"]] {
        let o = pxl_in(&root, &args);
        assert_eq!(o.status.code(), Some(1), "{:?}", args);
        assert!(stderr(&o).contains("A"), "{}", stderr(&o));
        assert_eq!(std::fs::read(root.join(P)).unwrap(), bytes);
    }
    let out = tmpdir("cand_lease_out");
    assert!(pxl_in(&root, &["candidates", P, "f0", "--out", out.to_str().unwrap()]).status.success());
    assert!(pxl_in(&root, &["--agent", "A", "pick", P, "f0", "vote"]).status.success());
    assert_ne!(std::fs::read(root.join(P)).unwrap(), bytes);
}

