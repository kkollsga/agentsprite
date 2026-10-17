//! Edit worker tests: AC-AD1..AD4 plus per-command behaviour. Synthetic parts and templates only.

use agentsprite::cli::*;
use agentsprite::color::Rgb;
use agentsprite::edit;
use agentsprite::journal;
use agentsprite::model::*;
use agentsprite::output::Out;
use agentsprite::pxl;
use agentsprite::show;
use std::path::{Path, PathBuf};

const TPL: &str = r##"
name = "t"
[layers.base]
"." = { role = "transparent" }
"1" = { role = "outline", color = "#182078" }
"2" = { role = "body", color = "#4850b8" }
"3" = { role = "skin", color = "#f0b090" }
[layers.detail]
"W" = { role = "white", color = "#f8f8f8", under = "3" }
"##;

fn dir(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("edit_{}", name));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("templates")).unwrap();
    std::fs::write(d.join("templates/t.toml"), TPL).unwrap();
    d
}

fn make_part(d: &Path, w: usize, h: usize, frames: &[&str]) -> PathBuf {
    let mut p = Part::new("demo", (w, h));
    p.template = Some("templates/t.toml".into());
    for f in frames {
        p.frames.push(Frame { name: f.to_string(), src: None, locked: false, grid: Grid::new(w, h, b'.') });
    }
    let path = d.join("demo.pxl");
    pxl::save(&path, &p).unwrap();
    path
}

fn out() -> Out {
    Out::new(false, None)
}
fn jout() -> Out {
    Out::new(true, None)
}
fn fl() -> EditFlags {
    EditFlags::default()
}
fn pt(x: i32, y: i32) -> Point {
    Point { x, y }
}
fn set(path: &Path, frame: &str, px: &[(i32, i32, &str)]) -> agentsprite::output::Result<()> {
    let a = SetArgs { part: path.display().to_string(), frame: frame.into(), pixels: px.iter().map(|(x, y, k)| Pixel { x: *x, y: *y, key: k.to_string() }).collect(), flags: fl() };
    edit::run_set(&a, &mut out())
}
fn apply(path: &Path, json: &str) -> agentsprite::output::Result<()> {
    let f = path.with_extension("ops.json");
    std::fs::write(&f, json).unwrap();
    edit::run_apply(&ApplyArgs { part: path.display().to_string(), ops: f.display().to_string(), flags: fl() }, &mut out())
}
fn undo(path: &Path, steps: usize) -> agentsprite::output::Result<()> {
    journal::run_undo(&UndoArgs { part: path.display().to_string(), steps, force_approved: false }, &mut out())
}
fn bytes(p: &Path) -> Vec<u8> {
    std::fs::read(p).unwrap()
}
fn grid(p: &Path, frame: &str) -> Grid {
    pxl::load(p).unwrap().frame(frame).unwrap().grid.clone()
}

/// Tiny deterministic LCG.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) as u32
    }
    fn below(&mut self, n: u32) -> u32 {
        self.next() % n
    }
}

// ---------------------------------------------------------------- AD1

#[test]
fn ad1_invalid_input_leaves_part_identical_and_names_the_problem() {
    let d = dir("ad1");
    let p = make_part(&d, 16, 16, &["a", "b"]);
    set(&p, "a", &[(1, 1, "1")]).unwrap();
    let before = bytes(&p);
    let jbefore = bytes(&p.with_extension("journal"));

    // unknown key: names pixel and key
    let e = set(&p, "a", &[(2, 2, "2"), (3, 3, "Q")]).unwrap_err();
    assert!(e.msg.contains("'Q'") && e.msg.contains("3,3"), "{}", e);
    let loc = e.loc.unwrap();
    assert_eq!((loc.frame.as_deref(), loc.row, loc.col), (Some("a"), Some(3), Some(3)));
    // outside the canvas
    let e = set(&p, "a", &[(2, 2, "2"), (16, 3, "2")]).unwrap_err();
    assert!(e.msg.contains("16,3") && e.msg.contains("outside"), "{}", e);
    let e = set(&p, "a", &[(-1, 3, "2")]).unwrap_err();
    assert!(e.msg.contains("-1,3"), "{}", e);
    // unknown frame, unknown role
    assert!(set(&p, "zz", &[(0, 0, "1")]).is_err());
    assert!(set(&p, "a", &[(0, 0, "nosuchrole")]).is_err());
    // batch: first op fine, third op bad => nothing applied
    let e = apply(&p, r#"{"ops":[{"op":"set","frame":"a","pixels":["0,0=1"]},{"op":"line","frame":"b","from":[0,0],"to":[5,5],"key":"2"},{"op":"rect","frame":"a","region":[10,10,9,2],"key":"1"}]}"#).unwrap_err();
    assert!(e.msg.contains("rect") && e.msg.contains("outside"), "{}", e);
    // unknown field / bad json
    assert!(apply(&p, r#"{"ops":[{"op":"set","frame":"a","pixels":["0,0=1"],"bogus":1}]}"#).is_err());
    assert!(apply(&p, "{not json").is_err());
    // bad patch character, bad write_grid
    let e = edit::run_patch(&PatchArgs { part: p.display().to_string(), frame: "a".into(), rest: vec!["0".into(), "0".into(), "12\n1X".into()], flags: fl() }, &mut out()).unwrap_err();
    assert!(e.msg.contains("'X'"), "{}", e);
    let e = edit::run_write_grid(&WriteGridArgs { part: p.display().to_string(), frame: "a".into(), grid: "1\n2".into(), flags: fl() }, &mut out()).unwrap_err();
    assert!(e.msg.contains("rows"), "{}", e);
    assert_eq!(bytes(&p), before);
    assert_eq!(bytes(&p.with_extension("journal")), jbefore, "failed edits write no journal entry");
}

// ---------------------------------------------------------------- AD2

#[test]
fn ad2_edits_touch_only_their_target() {
    let d = dir("ad2");
    let p = make_part(&d, 16, 16, &["a", "b"]);
    apply(&p, r#"[{"op":"rect","frame":"a","region":[0,0,16,16],"key":"2"},{"op":"rect","frame":"b","region":[0,0,16,16],"key":"3"}]"#).unwrap();
    let a0 = grid(&p, "a");
    let b0 = grid(&p, "b");

    // patch: spaces leave pixels untouched
    edit::run_patch(&PatchArgs { part: p.display().to_string(), frame: "a".into(), rest: vec!["4".into(), "4".into(), "1 1\n W \n1 1".into()], flags: fl() }, &mut out()).unwrap();
    let a1 = grid(&p, "a");
    let mut expect = a0.clone();
    for (x, y, k) in [(4, 4, b'1'), (6, 4, b'1'), (5, 5, b'W'), (4, 6, b'1'), (6, 6, b'1')] {
        expect.set(x, y, k);
    }
    assert_eq!(a1, expect);
    assert_eq!(grid(&p, "b"), b0, "other frames untouched");

    // roles work as keys
    set(&p, "a", &[(0, 0, "outline")]).unwrap();
    assert_eq!(grid(&p, "a").get(0, 0), b'1');

    // row, line, rect outline, replace in region, copy region
    let mut cur = grid(&p, "a");
    edit::run_row(&RowArgs { part: p.display().to_string(), frame: "a".into(), rest: vec!["8".into(), "1".into(), "1 3 1".into()], flags: fl() }, &mut out()).unwrap();
    let n = grid(&p, "a");
    let diffs: Vec<(i32, i32)> = (0..16).flat_map(|y| (0..16).map(move |x| (x, y))).filter(|&(x, y)| n.get(x, y) != cur.get(x, y)).collect();
    assert_eq!(diffs, vec![(8, 1), (10, 1), (12, 1)]);
    cur = n;
    edit::run_replace(&ReplaceArgs { part: p.display().to_string(), frame: "a".into(), maps: vec![KeyMap { from: "2".into(), to: "3".into() }], region: Some(Region { x: 8, y: 8, w: 4, h: 4 }), flags: fl() }, &mut out()).unwrap();
    let n = grid(&p, "a");
    for y in 0..16 {
        for x in 0..16 {
            let inside = (8..12).contains(&x) && (8..12).contains(&y);
            if !inside {
                assert_eq!(n.get(x, y), cur.get(x, y));
            } else if cur.get(x, y) == b'2' {
                assert_eq!(n.get(x, y), b'3');
            }
        }
    }
    edit::run_copy(&CopyArgs { part: p.display().to_string(), from: "a".into(), to: "b".into(), region: Some(Region { x: 2, y: 2, w: 5, h: 5 }), to_at: None, skip_transparent: false, flags: fl() }, &mut out()).unwrap();
    let b = grid(&p, "b");
    for y in 0..16 {
        for x in 0..16 {
            if (2..7).contains(&x) && (2..7).contains(&y) {
                assert_eq!(b.get(x, y), n.get(x, y));
            } else {
                assert_eq!(b.get(x, y), b0.get(x, y));
            }
        }
    }
    assert_eq!(grid(&p, "a"), n, "copy source untouched");
}

#[test]
fn edits_lock_frames_unless_no_lock() {
    let d = dir("lock");
    let p = make_part(&d, 8, 8, &["a", "b"]);
    let a = SetArgs { part: p.display().to_string(), frame: "a".into(), pixels: vec![Pixel { x: 0, y: 0, key: "1".into() }], flags: EditFlags { no_lock: true, ..Default::default() } };
    edit::run_set(&a, &mut out()).unwrap();
    assert!(!pxl::load(&p).unwrap().frame("a").unwrap().locked);
    set(&p, "b", &[(0, 0, "1")]).unwrap();
    let part = pxl::load(&p).unwrap();
    assert!(part.frame("b").unwrap().locked && !part.frame("a").unwrap().locked);
    // a no-op edit neither locks nor writes
    let before = bytes(&p);
    set(&p, "a", &[(0, 0, "1")]).unwrap();
    assert_eq!(bytes(&p), before);
    // dry run changes nothing
    let a = SetArgs { part: p.display().to_string(), frame: "a".into(), pixels: vec![Pixel { x: 3, y: 3, key: "2".into() }], flags: EditFlags { dry_run: true, ..Default::default() } };
    edit::run_set(&a, &mut out()).unwrap();
    assert_eq!(bytes(&p), before);
}

// ---------------------------------------------------------------- shapes

#[test]
fn line_is_bresenham_without_doubled_steps() {
    let mut lcg = Lcg(7);
    for _ in 0..300 {
        let (x0, y0, x1, y1) = (lcg.below(40) as i32, lcg.below(40) as i32, lcg.below(40) as i32, lcg.below(40) as i32);
        let pts = edit::line_points(x0, y0, x1, y1);
        assert_eq!(pts.len() as i32, (x1 - x0).abs().max((y1 - y0).abs()) + 1);
        assert_eq!(pts[0], (x0, y0));
        assert_eq!(*pts.last().unwrap(), (x1, y1));
        for w in pts.windows(2) {
            let (dx, dy) = ((w[1].0 - w[0].0).abs(), (w[1].1 - w[0].1).abs());
            assert!(dx <= 1 && dy <= 1 && dx + dy >= 1, "{:?}", w);
        }
        // no pixel whose both 4-neighbours-in-line form an L (a doubled step): consecutive triples never turn 90 degrees
        for w in pts.windows(3) {
            let (a, b, c) = (w[0], w[1], w[2]);
            let l = (a.0 == b.0 && b.1 == c.1 && a.0 != c.0 && a.1 != b.1) || (a.1 == b.1 && b.0 == c.0 && a.1 != c.1 && a.0 != b.0);
            assert!(!l, "L corner in {:?}", w);
        }
    }
    let d = dir("line");
    let p = make_part(&d, 16, 16, &["a"]);
    edit::run_line(&LineArgs { part: p.display().to_string(), frame: "a".into(), from: pt(1, 1), to: pt(9, 4), key: "1".into(), flags: fl() }, &mut out()).unwrap();
    let g = grid(&p, "a");
    assert_eq!(g.cells.iter().filter(|&&c| c == b'1').count(), 9);
}

#[test]
fn fill_is_four_connected_and_mirror_flip_replace_work() {
    let d = dir("shapes");
    let p = make_part(&d, 8, 8, &["a"]);
    // diagonal wall of 1s from (0,3)..(3,0) plus a diagonal-only leak at (4,4)
    apply(&p, r#"[{"op":"line","frame":"a","from":[0,3],"to":[3,0],"key":"1"},{"op":"set","frame":"a","pixels":["4,4=1"]}]"#).unwrap();
    edit::run_fill(&FillArgs { part: p.display().to_string(), frame: "a".into(), at: pt(0, 0), key: "2".into(), flags: fl() }, &mut out()).unwrap();
    let g = grid(&p, "a");
    assert_eq!(g.get(0, 0), b'2');
    assert_eq!(g.get(1, 1), b'2', "inside the wall");
    assert_eq!(g.get(2, 2), b'.', "wall holds against a diagonal leak");
    assert_eq!(g.get(3, 3), b'.');
    assert_eq!(g.get(1, 2), b'1');

    // mirror low half onto high half
    let p2 = make_part(&d.join(""), 6, 2, &["m"]);
    apply(&p2, r#"[{"op":"row","frame":"m","x":0,"y":0,"pixels":"123"},{"op":"row","frame":"m","x":0,"y":1,"pixels":"W.2"}]"#).unwrap();
    edit::run_mirror(&MirrorArgs { part: p2.display().to_string(), frame: "m".into(), axis: Axis::X, keep: "low".into(), region: None, flags: fl() }, &mut out()).unwrap();
    assert_eq!(grid(&p2, "m").to_lines(), vec!["123321", "W.22.W"]);
    edit::run_flip(&FlipArgs { part: p2.display().to_string(), frame: "m".into(), axis: Axis::Y, region: None, flags: fl() }, &mut out()).unwrap();
    assert_eq!(grid(&p2, "m").to_lines(), vec!["W.22.W", "123321"]);
    edit::run_mirror(&MirrorArgs { part: p2.display().to_string(), frame: "m".into(), axis: Axis::X, keep: "high".into(), region: None, flags: fl() }, &mut out()).unwrap();
    assert_eq!(grid(&p2, "m").to_lines(), vec!["W.22.W", "123321"].iter().map(|r| { let b: Vec<char> = r.chars().collect(); let mut o = b.clone(); for i in 0..3 { o[i] = b[5 - i]; } o.iter().collect::<String>() }).collect::<Vec<_>>());
    // replace swaps simultaneously
    edit::run_replace(&ReplaceArgs { part: p2.display().to_string(), frame: "m".into(), maps: vec![KeyMap { from: "1".into(), to: "3".into() }, KeyMap { from: "3".into(), to: "1".into() }], region: None, flags: fl() }, &mut out()).unwrap();
    assert_eq!(grid(&p2, "m").to_lines()[1], "321123");
}

#[test]
fn apply_batch_spans_frames_and_undoes_as_one_step() {
    let d = dir("apply");
    let p = make_part(&d, 8, 8, &["a", "b"]);
    let orig = bytes(&p);
    apply(&p, r#"{"ops":[
        {"op":"rect","frame":"a","region":[0,0,4,4],"key":"body"},
        {"op":"copy","from":"a","to":"b","region":[0,0,4,4]},
        {"op":"set","frame":"b","pixels":["1,1=W",{"x":2,"y":2,"key":"outline"}]},
        {"op":"write_grid","frame":"a","grid":["........","........","........","........","........","........","........","1......."]},
        {"op":"replace","frame":"b","maps":{"2":"3"}}
    ]}"#)
    .unwrap();
    let b = grid(&p, "b");
    assert_eq!(b.get(1, 1), b'W');
    assert_eq!(b.get(2, 2), b'1');
    assert_eq!(b.get(0, 0), b'3');
    assert_eq!(grid(&p, "a").get(0, 7), b'1');
    undo(&p, 1).unwrap();
    assert_eq!(bytes(&p), orig);
    assert!(undo(&p, 1).is_err(), "nothing left");
}

// ---------------------------------------------------------------- AD3

fn random_op(l: &mut Lcg, frames: &[&str], w: u32, h: u32) -> String {
    let f = frames[l.below(frames.len() as u32) as usize];
    let keys = ["1", "2", "3", "W", ".", "outline", "body"];
    let k = keys[l.below(keys.len() as u32) as usize];
    let (x, y) = (l.below(w) as i32, l.below(h) as i32);
    match l.below(9) {
        0 => format!(r#"{{"op":"set","frame":"{}","pixels":["{},{}={}","{},{}={}"]}}"#, f, x, y, k, l.below(w), l.below(h), k),
        1 => format!(r#"{{"op":"row","frame":"{}","x":{},"y":{},"pixels":"12 3W"}}"#, f, x.min(w as i32 - 5), y),
        2 => format!(r#"{{"op":"line","frame":"{}","from":[{},{}],"to":[{},{}],"key":"{}"}}"#, f, x, y, l.below(w), l.below(h), k),
        3 => format!(r#"{{"op":"rect","frame":"{}","region":[{},{},{},{}],"key":"{}","outline":{}}}"#, f, x.min(w as i32 - 4), y.min(h as i32 - 3), 1 + l.below(4), 1 + l.below(3), k, l.below(2) == 0),
        4 => format!(r#"{{"op":"fill","frame":"{}","at":[{},{}],"key":"{}"}}"#, f, x, y, k),
        5 => format!(r#"{{"op":"replace","frame":"{}","maps":["{}={}"],"region":[0,0,{},{}]}}"#, f, keys[l.below(5) as usize], k, w, h),
        6 => format!(r#"{{"op":"copy","from":"{}","to":"{}","region":[0,0,{},{}]}}"#, f, frames[l.below(frames.len() as u32) as usize], 1 + l.below(w), 1 + l.below(h)),
        7 => format!(r#"{{"op":"mirror","frame":"{}","axis":"{}","keep":"{}"}}"#, f, if l.below(2) == 0 { "x" } else { "y" }, if l.below(2) == 0 { "low" } else { "high" }),
        _ => format!(r#"{{"op":"flip","frame":"{}","axis":"{}","region":[0,0,{},{}]}}"#, f, if l.below(2) == 0 { "x" } else { "y" }, 1 + l.below(w), 1 + l.below(h)),
    }
}

#[test]
fn ad3_n_random_edits_then_undo_n_is_byte_identical() {
    for (case, seed, n) in [("ad3a", 1u64, 40usize), ("ad3b", 99, 25)] {
        let d = dir(case);
        let p = make_part(&d, 24, 20, &["a", "b", "c"]);
        let orig = bytes(&p);
        let mut l = Lcg(seed);
        let mut states = vec![orig.clone()];
        let mut applied = 0;
        while applied < n {
            let ops: Vec<String> = (0..1 + l.below(3)).map(|_| random_op(&mut l, &["a", "b", "c"], 24, 20)).collect();
            let doc = format!("[{}]", ops.join(","));
            let before = bytes(&p);
            match apply(&p, &doc) {
                Ok(()) => {
                    if bytes(&p) != before {
                        states.push(bytes(&p));
                        applied += 1;
                    }
                }
                Err(e) => panic!("random op failed: {} in {}", e, doc),
            }
        }
        // undo one at a time, comparing with every earlier state
        for k in (0..states.len() - 1).rev() {
            undo(&p, 1).unwrap();
            assert_eq!(bytes(&p), states[k], "state {} after undo", k);
        }
        assert_eq!(bytes(&p), orig);
        assert!(undo(&p, 1).is_err());
    }
    // N at once
    let d = dir("ad3c");
    let p = make_part(&d, 24, 20, &["a", "b"]);
    let orig = bytes(&p);
    let mut l = Lcg(5);
    let mut count = 0;
    while count < 12 {
        let before = bytes(&p);
        apply(&p, &format!("[{}]", random_op(&mut l, &["a", "b"], 24, 20))).unwrap();
        if bytes(&p) != before {
            count += 1;
        }
    }
    assert!(undo(&p, 13).is_err());
    assert_eq!(undo(&p, 12).is_ok(), true);
    assert_eq!(bytes(&p), orig);
}

#[test]
fn undo_is_journaled_and_not_undone_by_later_undo_and_restores_noncanonical_text() {
    let d = dir("undo_sem");
    let p = make_part(&d, 8, 8, &["a"]);
    set(&p, "a", &[(0, 0, "1")]).unwrap(); // e1
    let after1 = bytes(&p);
    set(&p, "a", &[(1, 1, "2")]).unwrap(); // e2
    undo(&p, 1).unwrap(); // undoes e2
    assert_eq!(bytes(&p), after1);
    set(&p, "a", &[(2, 2, "3")]).unwrap(); // e3
    undo(&p, 1).unwrap(); // undoes e3, not the undo
    assert_eq!(bytes(&p), after1);
    undo(&p, 1).unwrap(); // undoes e1
    assert!(!pxl::load(&p).unwrap().frame("a").unwrap().locked);
    let j = std::fs::read_to_string(p.with_extension("journal")).unwrap();
    assert_eq!(j.lines().filter(|l| l.contains("\"type\":\"undo\"")).count(), 3);
    assert_eq!(j.lines().filter(|l| l.contains("\"type\":\"edit\"")).count(), 3);
    assert_eq!(journal::active_edits(&p).unwrap().len(), 0);

    // non canonical file text is restored byte for byte
    let q = d.join("odd.pxl");
    let canon = std::fs::read_to_string(&p).unwrap();
    let odd = canon.replace("name = \"demo\"", "name   =   \"demo\"  # hand written");
    assert_ne!(odd, canon);
    std::fs::write(&q, &odd).unwrap();
    set(&q, "a", &[(0, 0, "1")]).unwrap();
    assert_ne!(std::fs::read_to_string(&q).unwrap(), odd);
    undo(&q, 1).unwrap();
    assert_eq!(std::fs::read_to_string(&q).unwrap(), odd);
}

// ---------------------------------------------------------------- AD4

#[test]
fn ad4_speed_and_output_size_on_160x144() {
    let d = dir("ad4");
    let p = make_part(&d, 160, 144, &["f"]);
    set(&p, "f", &[(0, 0, "2")]).unwrap();
    // warm up, then measure the mean of several single pixel edits (in process, no startup cost)
    let n = 20;
    let t = std::time::Instant::now();
    for i in 0..n {
        set(&p, "f", &[(10 + i, 20, if i % 2 == 0 { "1" } else { "3" })]).unwrap();
    }
    let ms = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
    eprintln!("single pixel edit: {:.2} ms", ms);
    let limit = if cfg!(debug_assertions) { 60.0 } else { 20.0 };
    assert!(ms < limit, "{} ms", ms);
    // output size
    let mut o = out();
    edit::run_set(&SetArgs { part: p.display().to_string(), frame: "f".into(), pixels: vec![Pixel { x: 100, y: 100, key: "W".into() }], flags: fl() }, &mut o).unwrap();
    let (text, _, code) = o.render(&Ok(()));
    assert_eq!(code, 0);
    assert!(text.lines().count() < 15, "{}", text);
    assert!(text.contains("1 px changed"), "{}", text);
    // json shape
    let mut o = jout();
    edit::run_set(&SetArgs { part: p.display().to_string(), frame: "f".into(), pixels: vec![Pixel { x: 5, y: 5, key: "W".into() }], flags: fl() }, &mut o).unwrap();
    let (text, _, _) = o.render(&Ok(()));
    let v: serde_json::Value = serde_json::from_str(text.trim()).unwrap();
    assert_eq!(v["changed"], 1);
    assert_eq!(v["region"], serde_json::json!([4, 4, 3, 3]));
    assert_eq!(v["rows"].as_array().unwrap().len(), 3);
}

// ---------------------------------------------------------------- clean, show

#[test]
fn clean_reports_and_fixes_orphans_and_jaggies() {
    let d = dir("clean");
    let p = make_part(&d, 12, 8, &["a"]);
    // an orphan, a clean diagonal, a doubled diagonal, and an L-corner of a rectangle outline that must stay
    apply(
        &p,
        r#"[{"op":"set","frame":"a","pixels":["11,0=1"]},
            {"op":"row","frame":"a","x":0,"y":1,"pixels":"11"},{"op":"row","frame":"a","x":1,"y":2,"pixels":"11"},{"op":"row","frame":"a","x":2,"y":3,"pixels":"11"},
            {"op":"rect","frame":"a","region":[6,3,5,4],"key":"1","outline":true}]"#,
    )
    .unwrap();
    let before = bytes(&p);
    let mut o = jout();
    edit::run_clean(&CleanArgs { part: p.display().to_string(), sel: FrameSel::default(), orphans: true, jaggies: true, flags: EditFlags { dry_run: true, ..Default::default() } }, &mut o).unwrap();
    let (text, _, _) = o.render(&Ok(()));
    let v: serde_json::Value = serde_json::from_str(text.trim()).unwrap();
    let findings = v["findings"].as_array().unwrap();
    assert!(findings.iter().any(|f| f.as_str().unwrap().contains("orphan at 11,0")), "{}", text);
    assert!(findings.iter().any(|f| f.as_str().unwrap().contains("jaggy")), "{}", text);
    assert!(!findings.iter().any(|f| f.as_str().unwrap().contains("at 6,3") || f.as_str().unwrap().contains("at 10,6")), "rectangle corners are not jaggies: {}", text);
    assert_eq!(bytes(&p), before, "dry run writes nothing");
    edit::run_clean(&CleanArgs { part: p.display().to_string(), sel: FrameSel::default(), orphans: true, jaggies: true, flags: fl() }, &mut out()).unwrap();
    let g = grid(&p, "a");
    assert_eq!(g.get(11, 0), b'.');
    // the doubled diagonal became a thin one: no 4-neighbour pairs remain in the staircase area
    let staircase: usize = (0..4).flat_map(|y| (0..6).map(move |x| (x, y))).filter(|&(x, y)| g.get(x, y) == b'1').count();
    assert!(staircase < 6, "{:?}", g.to_lines());
    assert_eq!(g.get(6, 3), b'1');
    assert_eq!(g.get(10, 6), b'1');
    undo(&p, 1).unwrap();
    assert_eq!(grid(&p, "a").get(11, 0), b'1');
}

#[test]
fn show_prints_rulers_and_region() {
    let d = dir("show");
    let p = make_part(&d, 24, 12, &["a"]);
    set(&p, "a", &[(12, 5, "W")]).unwrap();
    let mut o = out();
    show::run_show(&ShowArgs { part: p.display().to_string(), frame: "a".into(), region: Some(Region { x: 10, y: 4, w: 5, h: 3 }), reference: false, raw: false, tile: None, cells: false }, &mut o).unwrap();
    let (text, _, _) = o.render(&Ok(()));
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[0].contains("demo/a"), "{}", text);
    assert!(lines[1].trim_start().starts_with('1'), "tens ruler: {}", text);
    assert!(lines[2].ends_with("01234"), "ones ruler: {}", text);
    assert!(lines[4].ends_with("|..W.."), "{}", text);
    assert!(lines[4].trim_start().starts_with("5 |") || lines[4].trim_start().starts_with("5|"), "{}", text);
}

#[test]
fn show_ref_aligns_with_factor_and_phase() {
    let d = dir("showref");
    std::fs::create_dir_all(d.join("refs")).unwrap();
    // 8x8 input for a 4x4 target (factor 2), palette: transparent, red, blue
    let mut idx = vec![0u8; 64];
    idx[0 * 8 + 3] = 1; // input (3,0): target X=2 covers input x in {3,4}  (phase -1)
    idx[1 * 8 + 4] = 2; // input (4,1): row 0 covers input y {0,1}; x=4 is the second column of X=2
    agentsprite::png::write_indexed(&d.join("refs/a.png"), 8, 8, &idx, &[Rgb { r: 0, g: 0, b: 0 }, Rgb { r: 255, g: 0, b: 0 }, Rgb { r: 0, g: 0, b: 255 }], true).unwrap();
    let mut part = Part::new("demo", (4, 4));
    part.template = Some("templates/t.toml".into());
    part.input.images = Some("refs/{frame}.png".into());
    part.input.factor = Some(2);
    part.reduce.factor = Some(2);
    part.reduce.phase = Some((-1, 0));
    part.frames.push(Frame { name: "a".into(), src: None, locked: false, grid: Grid::new(4, 4, b'.') });
    let p = d.join("demo.pxl");
    pxl::save(&p, &part).unwrap();
    let mut o = out();
    show::run_show(&ShowArgs { part: p.display().to_string(), frame: "a".into(), region: None, reference: true, raw: false, tile: None, cells: false }, &mut o).unwrap();
    let (text, _, _) = o.render(&Ok(()));
    // 12a/12b style labels
    let l0a = text.lines().find(|l| l.contains("0a |")).expect(&text);
    let l0b = text.lines().find(|l| l.contains("0b |")).expect(&text);
    let ra = l0a.split("0a |").nth(1).unwrap();
    let rb = l0b.split("0b |").nth(1).unwrap();
    // target X=2 -> letters at ref columns 4,5 of the row string (sub columns of X=2 are input x 3,4)
    assert_eq!(ra.len(), 8);
    let (ra, rb) = (ra.as_bytes(), rb.as_bytes());
    assert!(ra[4] != b'.' && ra[5] == b'.' && ra[3] == b'.', "red pixel at the first column of X=2: {}", text);
    assert!(rb[4] == b'.' && rb[5] != b'.', "blue pixel at the second column of X=2: {}", text);
    assert!(text.contains("legend:") && text.contains("#ff0000") && text.contains("#0000ff"), "{}", text);
}
