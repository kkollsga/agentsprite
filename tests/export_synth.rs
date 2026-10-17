//! Export / build / check / import on synthetic parts (no private data needed):
//! AC-TP1, AC-EX2 proxy, AC-EX3, AC-EX4, plus the import round trips.

use agentsprite::ase;
use agentsprite::color::Rgb;
use agentsprite::model::Part;
use agentsprite::{png, pxl};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

const TPL: &str = r##"name = "t"
[layers.base]
"." = { role = "transparent" }
"1" = { role = "outline", color = "#182078" }
"2" = { role = "body",    color = "#4850b8" }
"3" = { role = "skin",    color = "#f0b090" }
[layers.detail]
"W" = { role = "white", color = "#f0f0f0", under = "3" }
"R" = { role = "shoe",  color = "#f00000", under = "1" }
"##;

fn sandbox(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("export_{}", name));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("templates")).unwrap();
    std::fs::create_dir_all(d.join("parts")).unwrap();
    std::fs::write(d.join("templates/t.toml"), TPL).unwrap();
    // the old per-asset model: overlays allowed (a project profile), no scenes demanded (legacy = true in pxl.toml)
    std::fs::create_dir_all(d.join("profiles")).unwrap();
    std::fs::write(d.join("profiles/gbc-sprite.toml"), "overlays = \"allow\"\n").unwrap();
    d
}

fn grid_rows(i: usize) -> Vec<String> {
    let mut g = vec![vec![b'.'; 32]; 32];
    let o = i % 8;
    for y in 4..18 {
        for x in 4 + o..16 + o {
            let edge = y == 4 || y == 17 || x == 4 + o || x == 15 + o;
            g[y][x] = if edge { b'1' } else { b'2' };
        }
    }
    for y in 6..9 {
        for x in 6 + o..9 + o {
            g[y][x] = b'3';
        }
    }
    g[7][7 + o] = b'W';
    g[17][6 + o] = b'R';
    g.into_iter().map(|r| String::from_utf8(r).unwrap()).collect()
}

fn part_text(name: &str, frames: usize) -> String {
    let mut s = format!("name = \"{}\"\nprofile = \"gbc-sprite\"\nsize = [32, 32]\norigin = [16, 16]\ntemplate = \"templates/t.toml\"\n", name);
    for i in 0..frames {
        s.push_str(&format!("\n[[frames]]\nname = \"{:02}_f{}\"\ngrid = \"\"\"\n{}\n\"\"\"\n", i + 1, i % 3, grid_rows(i).join("\n")));
    }
    s
}

fn write_project(d: &Path, parts: &[(&str, usize)]) {
    let mut m = String::new();
    for (n, f) in parts {
        std::fs::write(d.join(format!("parts/{}.pxl", n)), part_text(n, *f)).unwrap();
        m.push_str(&format!("[parts.{n}]\npath = \"parts/{n}.pxl\"\nout = \"out/{n}\"\n"));
    }
    std::fs::write(d.join("pxl.toml"), format!("legacy = true\n{}", m)).unwrap();
}

fn pxl_cmd(d: &Path, args: &[&str]) -> (i32, String, String) {
    let o = Command::new(env!("CARGO_BIN_EXE_pxl")).current_dir(d).args(args).output().unwrap();
    (o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).into(), String::from_utf8_lossy(&o.stderr).into())
}

fn json(d: &Path, args: &[&str]) -> (i32, serde_json::Value) {
    let mut a = vec!["--json"];
    a.extend_from_slice(args);
    let (c, o, e) = pxl_cmd(d, &a);
    (c, serde_json::from_str(&o).unwrap_or_else(|_| panic!("not json: {:?} / {}", o, e)))
}

fn tree(d: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(d: &Path, base: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for e in std::fs::read_dir(d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(&p, base, out);
            } else {
                out.insert(agentsprite::fsutil::rel_slash(p.strip_prefix(base).unwrap()), std::fs::read(&p).unwrap());
            }
        }
    }
    let mut m = BTreeMap::new();
    if d.exists() {
        walk(d, d, &mut m);
    }
    m
}

#[test]
fn png_layout_base_under_overlay() {
    let d = sandbox("layout");
    write_project(&d, &[("a", 2)]);
    let (c, v) = json(&d, &["export", "parts/a.pxl"]);
    assert_eq!(c, 0, "{}", v);
    let out = d.join("out/a");
    let part = pxl::load(&d.join("parts/a.pxl")).unwrap();
    let (w, h, base, pal, trns) = png::read_indexed(&out.join("01_f0.png")).unwrap();
    assert_eq!((w, h), (32, 32));
    assert_eq!(pal, vec![Rgb::new(0, 0, 0), Rgb::new(0x18, 0x20, 0x78), Rgb::new(0x48, 0x50, 0xb8), Rgb::new(0xf0, 0xb0, 0x90)]);
    assert_eq!(trns.first(), Some(&0));
    assert!(trns.iter().skip(1).all(|&a| a == 255));
    let (_, _, over, opal, _) = png::read_indexed(&out.join("01_f0_detail.png")).unwrap();
    assert_eq!(opal, vec![Rgb::new(0, 0, 0), Rgb::new(0xf0, 0xf0, 0xf0), Rgb::new(0xf0, 0, 0)]);
    let g = &part.frames[0].grid;
    for i in 0..32 * 32 {
        let k = g.cells[i];
        let (b, o) = (base[i], over[i]);
        match k {
            b'.' => assert_eq!((b, o), (0, 0)),
            b'1' | b'2' | b'3' => assert_eq!((b, o), (k - b'0', 0)),
            b'W' => assert_eq!((b, o), (3, 1)),
            b'R' => assert_eq!((b, o), (1, 2)),
            _ => unreachable!(),
        }
    }
    assert!(out.join("a.aseprite").is_file());
}

#[test]
fn ac_tp1_template_colour_change_changes_only_that_colour() {
    let d = sandbox("tp1");
    write_project(&d, &[("a", 3), ("b", 2)]);
    assert_eq!(pxl_cmd(&d, &["build"]).0, 0);
    let before = tree(&d.join("out"));
    let t = std::fs::read_to_string(d.join("templates/t.toml")).unwrap().replace("#4850b8", "#58a0c8");
    std::fs::write(d.join("templates/t.toml"), t).unwrap();
    let (c, v) = json(&d, &["build"]);
    assert_eq!(c, 0, "{}", v);
    assert_eq!(v["built"].as_array().unwrap().len(), 2, "both parts use the template, both rebuild");
    let after = tree(&d.join("out"));
    assert_eq!(before.keys().collect::<Vec<_>>(), after.keys().collect::<Vec<_>>());
    let (old, new) = (Rgb::new(0x48, 0x50, 0xb8), Rgb::new(0x58, 0xa0, 0xc8));
    let mut pngs = 0;
    for (k, b) in &before {
        if k.ends_with(".png") {
            pngs += 1;
            let (p1, p2) = (d.join("out").join(k), d.join("out").join(k));
            let _ = p2;
            // decode the old bytes through a temp file
            let tmp = d.join("old.png");
            std::fs::write(&tmp, b).unwrap();
            let (w, h, i1, pal1, t1) = png::read_indexed(&tmp).unwrap();
            let (w2, h2, i2, pal2, t2) = png::read_indexed(&p1).unwrap();
            assert_eq!((w, h, &i1, &t1), (w2, h2, &i2, &t2), "{}: pixels and transparency unchanged", k);
            let expect: Vec<Rgb> = pal1.iter().map(|c| if *c == old { new } else { *c }).collect();
            assert_eq!(pal2, expect, "{}: only the body colour changed", k);
        } else {
            // .aseprite: same layer indices, palette differs only where the body colour was
            let (d1, d2) = (ase::read(b).unwrap(), ase::read(&after[k]).unwrap());
            let (p1, p2) = (d1.palette(), d2.palette());
            let diff: Vec<usize> = (0..p1.len()).filter(|&i| p1[i] != p2[i]).collect();
            assert_eq!(diff.len(), 1, "{}: exactly one palette entry changes", k);
            assert_eq!(p1[diff[0]][..3], [old.r, old.g, old.b]);
            assert_eq!(p2[diff[0]][..3], [new.r, new.g, new.b]);
            for f in 0..d1.frames.len() {
                for l in 0..2 {
                    assert_eq!(ase::render_layer_indices(&d1, f, l).unwrap(), ase::render_layer_indices(&d2, f, l).unwrap());
                }
            }
        }
    }
    assert_eq!(pngs, 5 * 2);
}

#[test]
fn ac_ex2_proxy_ase_validates_and_renders_like_the_pngs() {
    let d = sandbox("ase");
    write_project(&d, &[("a", 5)]);
    assert_eq!(pxl_cmd(&d, &["build"]).0, 0);
    let bytes = std::fs::read(d.join("out/a/a.aseprite")).unwrap();
    assert!(ase::validate(&bytes).is_empty(), "{:?}", ase::validate(&bytes));
    let doc = ase::read(&bytes).unwrap();
    assert_eq!(ase::write(&doc), bytes, "read -> write keeps every chunk, byte for byte");
    assert_eq!(ase::read(&ase::write(&doc)).unwrap(), doc);
    assert_eq!(doc.mode, ase::ColorMode::Indexed);
    assert_eq!((doc.width, doc.height), (32, 32));
    assert_eq!((doc.grid_w, doc.grid_h), (8, 16));
    let layers = doc.layers();
    assert_eq!(layers.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), ["base", "detail"]);
    assert_eq!(doc.frames.len(), 5);
    let part = pxl::load(&d.join("parts/a.pxl")).unwrap();
    let tags = doc.tags();
    for (i, f) in part.frames.iter().enumerate() {
        assert!(tags.iter().any(|t| t.name == f.name && t.from as usize == i && t.to as usize == i), "tag {}", f.name);
    }
    assert_eq!(tags.len(), part.frames.len(), "no MD input: no animation tags");
    let sl = doc.slices();
    assert_eq!(sl.len(), 1);
    assert_eq!(sl[0].keys[0].pivot, Some((16, 16)));
    let pal = doc.palette();
    for (fi, f) in part.frames.iter().enumerate() {
        for (li, suffix) in [(0usize, ""), (1, "_detail")] {
            let (_, _, idx, ppal, trns) = png::read_indexed(&d.join(format!("out/a/{}{}.png", f.name, suffix))).unwrap();
            let ai = ase::render_layer_indices(&doc, fi, li).unwrap();
            for i in 0..idx.len() {
                let png_transparent = idx[i] == 0 && trns[0] == 0;
                let ase_transparent = ai[i] == doc.transparent_index;
                assert_eq!(png_transparent, ase_transparent, "frame {} layer {} px {}", f.name, li, i);
                if !png_transparent {
                    let c = ppal[idx[i] as usize];
                    assert_eq!(pal[ai[i] as usize][..3], [c.r, c.g, c.b]);
                }
            }
        }
    }
}

#[test]
fn ac_ex3_deterministic_declared_outputs_only_and_incremental() {
    let (d1, d2) = (sandbox("det1"), sandbox("det2"));
    write_project(&d1, &[("a", 4), ("b", 3)]);
    write_project(&d2, &[("a", 4), ("b", 3)]);
    let (c, v) = json(&d1, &["build"]);
    assert_eq!(c, 0, "{}", v);
    assert_eq!(pxl_cmd(&d2, &["build"]).0, 0);
    let (t1, t2) = (tree(&d1.join("out")), tree(&d2.join("out")));
    assert_eq!(t1, t2, "two builds are byte identical");
    // exactly the declared files
    let mut declared: Vec<String> = Vec::new();
    for (n, f) in [("a", 4usize), ("b", 3)] {
        for i in 0..f {
            declared.push(format!("{}/{:02}_f{}.png", n, i + 1, i % 3));
            declared.push(format!("{}/{:02}_f{}_detail.png", n, i + 1, i % 3));
        }
        declared.push(format!("{}/{}.aseprite", n, n));
    }
    declared.sort();
    assert_eq!(t1.keys().cloned().collect::<Vec<_>>(), declared, "build writes only declared outputs");
    // nothing else appeared in the project besides the status and lock files next to the parts
    let mut top: Vec<String> = tree(&d1).keys().filter(|k| !k.starts_with("out/")).cloned().collect();
    top.sort();
    assert_eq!(top, ["parts/a.pxl", "parts/a.pxl.lock", "parts/a.status.json", "parts/b.pxl", "parts/b.pxl.lock", "parts/b.status.json", "profiles/gbc-sprite.toml", "pxl.toml", "templates/t.toml"]);
    // incremental
    let (_, v) = json(&d1, &["build"]);
    assert_eq!(v["skipped"].as_array().unwrap().len(), 2);
    assert_eq!(v["built"].as_array().unwrap().len(), 0);
    // a damaged output, a deleted output and an edited part each trigger a rebuild of just that part
    std::fs::write(d1.join("out/a/01_f0.png"), b"junk").unwrap();
    std::fs::remove_file(d1.join("out/b/b.aseprite")).unwrap();
    let (_, v) = json(&d1, &["build"]);
    assert_eq!(v["built"].as_array().unwrap().len(), 2, "{}", v);
    assert_eq!(tree(&d1.join("out")), t1, "outputs restored");
    let txt = part_text("a", 4).replace("[[frames]]\nname = \"01_f0\"", "[[frames]]\nname = \"01_f0\"\nlocked = true");
    std::fs::write(d1.join("parts/a.pxl"), txt).unwrap();
    let (_, v) = json(&d1, &["build"]);
    assert_eq!(v["built"].as_array().unwrap().len(), 1, "{}", v);
    assert_eq!(v["skipped"].as_array().unwrap().len(), 1);
    let (_, v) = json(&d1, &["build", "--force"]);
    assert_eq!(v["built"].as_array().unwrap().len(), 2);
    assert_eq!(v["files_written"], 0, "forced rebuild rewrites nothing: bytes are identical");
}

#[test]
fn export_out_override_frames_and_errors() {
    let d = sandbox("ovr");
    write_project(&d, &[("a", 3)]);
    let (c, _) = json(&d, &["export", "parts/a.pxl", "--out", "elsewhere", "--frames", "02_f1"]);
    assert_eq!(c, 0);
    let t = tree(&d.join("elsewhere"));
    assert_eq!(t.keys().cloned().collect::<Vec<_>>(), ["02_f1.png", "02_f1_detail.png", "a.aseprite"]);
    let (c, v) = json(&d, &["export", "parts/a.pxl", "--frames", "nope"]);
    assert_eq!(c, 2, "{}", v);
    // a key the template lacks fails with file, frame, row, column (AC-TP2) and writes nothing
    let bad = part_text("a", 1).replacen("1", "Z", 1);
    std::fs::write(d.join("parts/a.pxl"), bad).unwrap();
    let _ = std::fs::remove_dir_all(d.join("out"));
    let (c, v) = json(&d, &["export", "--all"]);
    assert_eq!(c, 1, "{}", v);
    assert!(!d.join("out").exists());
}

#[test]
fn check_reports_missing_unlisted_and_lint() {
    let d = sandbox("check");
    write_project(&d, &[("a", 3), ("b", 2)]);
    let (c, v) = json(&d, &["check"]);
    assert_eq!(c, 1, "nothing exported yet: {}", v);
    assert_eq!(v["totals"]["missing"], 3 * 2 + 1 + 2 * 2 + 1);
    assert_eq!(pxl_cmd(&d, &["build"]).0, 0);
    let (c, v) = json(&d, &["check"]);
    assert_eq!(c, 0, "{}", v);
    assert_eq!(v["pass"], true);
    assert_eq!(v["parts"].as_array().unwrap().len(), 2);
    assert_eq!(v["parts"][0]["export"], "fresh");
    std::fs::copy(d.join("out/a/01_f0.png"), d.join("out/a/stray.png")).unwrap();
    std::fs::remove_file(d.join("out/b/02_f1_detail.png")).unwrap();
    let (c, v) = json(&d, &["check"]);
    assert_eq!(c, 1);
    assert_eq!(v["unlisted"].as_array().unwrap().len(), 1);
    assert!(v["unlisted"][0].as_str().unwrap().ends_with("stray.png"));
    assert_eq!(v["parts"][1]["missing"][0], "out/b/02_f1_detail.png");
    // lint errors fail the check: a colour override departs from the template
    std::fs::remove_file(d.join("out/a/stray.png")).unwrap();
    assert_eq!(pxl_cmd(&d, &["build", "--force"]).0, 0);
    let txt = part_text("a", 3).replace("origin = [16, 16]", "origin = [16, 16]\n\n[colors]\n\"2\" = \"#000000\"");
    std::fs::write(d.join("parts/a.pxl"), txt).unwrap();
    let (c, v) = json(&d, &["check", "parts/a.pxl"]);
    assert_eq!(c, 1, "{}", v);
    assert!(v["parts"][0]["lint"]["errors"].as_u64().unwrap() > 0);
}

#[test]
fn ac_ex4_check_800_frames_under_one_second() {
    let d = sandbox("perf");
    let parts: Vec<(String, usize)> = (0..10).map(|i| (format!("p{}", i), 80)).collect();
    let refs: Vec<(&str, usize)> = parts.iter().map(|(n, f)| (n.as_str(), *f)).collect();
    write_project(&d, &refs);
    let (c, v) = json(&d, &["build"]);
    assert_eq!(c, 0, "{}", v);
    let args = agentsprite::cli::CheckArgs { parts: vec![], manifest: Some(d.join("pxl.toml").display().to_string()), force: false, hand_finished: false, recompute: false, tolerance: None };
    let mut best = f64::MAX;
    for _ in 0..3 {
        let t = std::time::Instant::now();
        let mut out = agentsprite::output::Out::new(true, None);
        let r = agentsprite::export::run_check(&args, &mut out);
        let e = t.elapsed().as_secs_f64();
        assert!(r.is_ok(), "{:?}", r);
        best = best.min(e);
    }
    eprintln!("AC-EX4: check over 800 frames in {:.3}s", best);
    assert!(best < 1.0, "check took {:.3}s", best);
}

// ---------------------------------------------------------------------------------------------- import

fn imp(d: &Path, args: &[&str]) -> (i32, serde_json::Value) {
    let mut a = vec!["import"];
    a.extend_from_slice(args);
    json(d, &a)
}

#[test]
fn import_png_and_overlay_round_trip_and_errors() {
    let d = sandbox("imp_png");
    write_project(&d, &[("a", 3)]);
    assert_eq!(pxl_cmd(&d, &["build"]).0, 0);
    let before = pxl::load(&d.join("parts/a.pxl")).unwrap();
    // wipe frame 2, then bring it back from its PNG pair
    let mut p: Part = before.clone();
    p.frames[1].grid.cells.iter_mut().for_each(|c| *c = b'.');
    pxl::save(&d.join("parts/a.pxl"), &p).unwrap();
    let (c, v) = imp(&d, &["out/a/02_f1.png", "--overlay", "out/a/02_f1_detail.png", "--part", "parts/a.pxl"]);
    assert_eq!(c, 0, "{}", v);
    let after = pxl::load(&d.join("parts/a.pxl")).unwrap();
    assert_eq!(after.frames[1].grid, before.frames[1].grid, "base + overlay restore the exact grid");
    assert!(after.frames[1].locked);
    // journal entry exists and undo restores the wiped frame
    let j = std::fs::read_to_string(d.join("parts/a.journal")).unwrap();
    assert!(j.contains("\"cmd\":\"import\""));
    let (c, _) = json(&d, &["undo", "parts/a.pxl"]);
    assert_eq!(c, 0);
    assert!(pxl::load(&d.join("parts/a.pxl")).unwrap().frames[1].grid.cells.iter().all(|&c| c == b'.'));
    // a base PNG alone: new frame name from the file stem
    let (c, v) = imp(&d, &["out/a/01_f0.png", "--part", "parts/a.pxl", "--frame", "copy"]);
    assert_eq!(c, 0, "{}", v);
    let p = pxl::load(&d.join("parts/a.pxl")).unwrap();
    let f = p.frame("copy").unwrap();
    assert!(f.locked);
    assert_eq!(f.grid.cells.iter().filter(|&&c| c == b'W' || c == b'R').count(), 0, "base alone has no overlay keys");
    // a colour the template does not know: error names the pixel
    let mut img = png::read_rgba(&d.join("out/a/01_f0.png")).unwrap();
    img.px[5 * 32 + 6] = agentsprite::img::Rgba::opaque(1, 2, 3);
    png::write_rgba(&d.join("bad.png"), &img).unwrap();
    let (c, v) = imp(&d, &["bad.png", "--part", "parts/a.pxl"]);
    assert_eq!(c, 1);
    let msg = v["error"]["message"].as_str().unwrap();
    assert!(msg.contains("(6,5)") && msg.contains("#010203"), "{}", msg);
    assert_eq!(v["error"]["loc"]["row"], 5);
    assert_eq!(v["error"]["loc"]["col"], 6);
    // wrong size
    let small = agentsprite::img::RgbaImage::new(4, 4);
    png::write_rgba(&d.join("small.png"), &small).unwrap();
    assert_eq!(imp(&d, &["small.png", "--part", "parts/a.pxl"]).0, 1);
}

#[test]
fn import_grid_txt_glob_and_key_errors() {
    let d = sandbox("imp_txt");
    write_project(&d, &[("a", 2)]);
    std::fs::create_dir_all(d.join("grids")).unwrap();
    std::fs::write(d.join("grids/01_f0.txt"), grid_rows(5).join("\n") + "\n").unwrap();
    std::fs::write(d.join("grids/zz_new.txt"), grid_rows(6).join("\n") + "\n").unwrap();
    let (c, v) = imp(&d, &["grids/*.txt", "--part", "parts/a.pxl"]);
    assert_eq!(c, 0, "{}", v);
    let p = pxl::load(&d.join("parts/a.pxl")).unwrap();
    assert_eq!(p.frames.len(), 3);
    assert_eq!(p.frame("01_f0").unwrap().grid.to_lines(), grid_rows(5));
    assert!(p.frame("01_f0").unwrap().locked && p.frame("zz_new").unwrap().locked);
    // unknown key -> file, frame, row, col
    let mut rows = grid_rows(0);
    rows[3].replace_range(9..10, "Q");
    std::fs::write(d.join("grids/bad.txt"), rows.join("\n") + "\n").unwrap();
    let (c, v) = imp(&d, &["grids/bad.txt", "--part", "parts/a.pxl"]);
    assert_eq!(c, 1);
    assert_eq!(v["error"]["loc"]["row"], 3);
    assert_eq!(v["error"]["loc"]["col"], 9);
    assert_eq!(v["error"]["loc"]["frame"], "bad");
    // short grid
    std::fs::write(d.join("grids/short.txt"), "....\n").unwrap();
    assert_eq!(imp(&d, &["grids/short.txt", "--part", "parts/a.pxl"]).0, 1);
    // missing --part
    assert_eq!(imp(&d, &["grids/01_f0.txt"]).0, 2);
}

#[test]
fn import_aseprite_round_trip_and_reports_unsupported() {
    let d = sandbox("imp_ase");
    write_project(&d, &[("a", 3)]);
    assert_eq!(pxl_cmd(&d, &["build"]).0, 0);
    let before = pxl::load(&d.join("parts/a.pxl")).unwrap();
    let mut p = before.clone();
    for f in &mut p.frames {
        f.grid.cells.iter_mut().for_each(|c| *c = b'.');
    }
    pxl::save(&d.join("parts/a.pxl"), &p).unwrap();
    let (c, v) = imp(&d, &["out/a/a.aseprite", "--part", "parts/a.pxl"]);
    assert_eq!(c, 0, "{}", v);
    let after = pxl::load(&d.join("parts/a.pxl")).unwrap();
    for (a, b) in after.frames.iter().zip(&before.frames) {
        assert_eq!(a.grid, b.grid, "{}", a.name);
        assert!(a.locked);
    }
    assert!(v["unsupported"].as_array().unwrap().is_empty(), "our own export loses nothing: {}", v["unsupported"]);

    // an artist file: odd layer, durations, a multi frame tag, a hidden layer -> reported, not dropped
    let pal = [Rgb::new(0, 0, 0), Rgb::new(0x18, 0x20, 0x78), Rgb::new(0xf0, 0xf0, 0xf0)];
    let mut b = ase::AseBuilder::new_indexed(32, 32, &pal);
    let l0 = b.add_layer("base");
    let mut hidden = ase::Layer::normal("detail");
    hidden.flags = ase::layer_flags::EDITABLE;
    hidden.opacity = 128;
    let l1 = b.add_layer_with(hidden);
    for dur in [200u16, 200, 50] {
        let f = b.add_frame(dur);
        let mut c0 = vec![0u8; 1024];
        c0[40] = 1;
        b.set_cel_canvas(f, l0, &c0);
        let mut c1 = vec![0u8; 1024];
        c1[41] = 2;
        b.set_cel_canvas(f, l1, &c1);
    }
    b.add_tag("walk", 0, 2, ase::LoopDir::PingPong, 3);
    std::fs::write(d.join("artist.aseprite"), b.build().unwrap()).unwrap();
    let (c, v) = imp(&d, &["artist.aseprite", "--part", "parts/a.pxl"]);
    assert_eq!(c, 0, "{}", v);
    let un: Vec<String> = v["unsupported"].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect();
    let all = un.join("\n");
    for needle in ["durations", "walk", "hidden", "opacity"] {
        assert!(all.contains(needle), "missing '{}' in {}", needle, all);
    }
    let p = pxl::load(&d.join("parts/a.pxl")).unwrap();
    assert_eq!(p.frames[0].grid.cells[40], b'1');
    assert_eq!(p.frames[0].grid.cells[41], b'W');

    // a layer the template does not know, with pixels: refuse, name it
    let mut b = ase::AseBuilder::new_indexed(32, 32, &pal);
    let l = b.add_layer("Layer 1");
    let f = b.add_frame(100);
    let mut c0 = vec![0u8; 1024];
    c0[0] = 1;
    b.set_cel_canvas(f, l, &c0);
    std::fs::write(d.join("odd.aseprite"), b.build().unwrap()).unwrap();
    let (c, v) = imp(&d, &["odd.aseprite", "--part", "parts/a.pxl"]);
    assert_eq!(c, 1);
    assert!(v["error"]["message"].as_str().unwrap().contains("Layer 1"));
}

#[test]
fn overlay_png_only_for_frames_with_overlay_pixels() {
    let d = sandbox("emptyover");
    write_project(&d, &[("a", 2)]);
    let (c, v) = json(&d, &["build"]);
    assert_eq!(c, 0, "{}", v);
    let out = d.join("out/a");
    assert!(out.join("01_f0_detail.png").is_file());
    // frame 1 loses its overlay pixels: the stale file is removed, check does not expect it
    let text = std::fs::read_to_string(d.join("parts/a.pxl")).unwrap();
    let (head, tail) = text.split_at(text.find("02_f1").unwrap());
    let tail = tail.replace('W', "3").replace('R', "1");
    std::fs::write(d.join("parts/a.pxl"), format!("{}{}", head, tail)).unwrap();
    let (c, v) = json(&d, &["build"]);
    assert_eq!(c, 0, "{}", v);
    assert!(out.join("02_f1.png").is_file());
    assert!(!out.join("02_f1_detail.png").exists(), "empty overlay file removed");
    assert!(out.join("01_f0_detail.png").is_file());
    let (c, v) = json(&d, &["check"]);
    assert_eq!(c, 0, "{}", v);
    let doc = ase::read(&std::fs::read(out.join("a.aseprite")).unwrap()).unwrap();
    assert_eq!(doc.layers().len(), 2, "the .aseprite keeps its overlay layer");
}

fn manifest_dirs(d: &Path, a: &str, b: &str) {
    let m = format!("[parts.a]\npath = \"parts/a.pxl\"\nout = \"{a}\"\n[parts.b]\npath = \"parts/b.pxl\"\nout = \"{b}\"\n");
    std::fs::write(d.join("pxl.toml"), format!("legacy = true\n{}", m)).unwrap();
}

/// Regression: two parts swapping output folders in one build must not delete each other's fresh files.
#[test]
fn build_swapping_output_folders_keeps_all_files() {
    let d = sandbox("swap");
    write_project(&d, &[("a", 3), ("b", 3)]);
    manifest_dirs(&d, "out/x", "out/y");
    assert_eq!(pxl_cmd(&d, &["build"]).0, 0);
    manifest_dirs(&d, "out/y", "out/x");
    for _ in 0..3 {
        let (c, v) = json(&d, &["build", "--force"]);
        assert_eq!(c, 0, "{}", v);
        for dir in ["out/x", "out/y"] {
            let n = std::fs::read_dir(d.join(dir)).unwrap().count();
            assert_eq!(n, 3 * 2 + 1, "{} must hold all its files", dir);
        }
        assert!(d.join("out/x/b.aseprite").exists() && d.join("out/y/a.aseprite").exists());
        assert!(!d.join("out/x/a.aseprite").exists(), "a's old ase is stale and removed");
    }
}

/// Two parts sharing one folder: rebuilding, and dropping a frame of one part, never touches the other's files.
#[test]
fn build_shared_folder_cleanup_is_per_part() {
    let d = sandbox("shared");
    write_project(&d, &[("a", 3), ("b", 3)]);
    let b = std::fs::read_to_string(d.join("parts/b.pxl")).unwrap().replace("_f", "_g");
    std::fs::write(d.join("parts/b.pxl"), b).unwrap();
    manifest_dirs(&d, "out/s", "out/s");
    assert_eq!(pxl_cmd(&d, &["build"]).0, 0);
    let full = tree(&d.join("out"));
    assert_eq!(full.len(), 2 * (3 * 2 + 1));
    assert_eq!(pxl_cmd(&d, &["build", "--force"]).0, 0);
    assert_eq!(tree(&d.join("out")), full);
    // a drops its last frame: only a's files for that frame go
    let mut a = part_text("a", 2);
    a.push('\n');
    std::fs::write(d.join("parts/a.pxl"), a).unwrap();
    assert_eq!(pxl_cmd(&d, &["build"]).0, 0);
    let t = tree(&d.join("out"));
    assert_eq!(t.len(), full.len() - 2);
    assert!(!t.contains_key("s/03_f2.png") && t.contains_key("s/03_g2.png"));
}
