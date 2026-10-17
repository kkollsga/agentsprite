//! Gates harness: measures the five design gates (eye_parity, identity_colour, flat, consistency, readability) the way the quality harness
//! measures the first version.
//!   * Negatives must stay quiet: the unmutated synthetic assets (Mega Drive console, where the automatic version is faithful) report nothing, and the
//!     approved real parts (real art, only when present: `$PXL_GATES_REAL` = the `pxl/` folder of an approved real project copy whose `../art` is reachable,) report no more than `tests/fixtures/gates/baseline.json`.
//!   * Positives are mutated copies built here: an erased eye, a removed identity colour (red shoes folded into the dark body colour), a flattened material
//!     (two tones of the body merged) and a recoloured ramp in one sibling. Each must fire its gate. With real art the player's eye in `01_stand`
//!     is painted over and eye_parity must fire.
mod e2e_common;

use agentsprite::img::{Rgba, RgbaImage};
use agentsprite::png;
use e2e_common::*;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const GATES: &str = "eye_parity,identity_colour,flat,consistency,readability";
const OUT: (u8, u8, u8) = (16, 24, 40);
const SKIN: (u8, u8, u8) = (240, 176, 144);
const BODY: (u8, u8, u8) = (48, 80, 200);
const BODY_D: (u8, u8, u8) = (28, 44, 128);
const RED: (u8, u8, u8) = (224, 32, 32);
const WHITE: (u8, u8, u8) = (255, 255, 255);

fn ok(o: &std::process::Output) -> String {
    assert!(o.status.success(), "{}{}", stdout(o), stderr(o));
    stdout(o)
}

fn put(g: &mut [Vec<Option<(u8, u8, u8)>>], x: i32, y: i32, c: (u8, u8, u8)) {
    if x >= 0 && y >= 0 && (y as usize) < g.len() && (x as usize) < g[0].len() {
        g[y as usize][x as usize] = Some(c);
    }
}

/// The hero: outline, skin head with two white 2x3 eyes (2 pupil cells), two-tone blue body, red shoes. 16x24 cells at scale 4.
fn hero(pose: usize) -> RgbaImage {
    let (w, h, scale) = (16usize, 24usize, 4usize);
    let mut g: Vec<Vec<Option<(u8, u8, u8)>>> = vec![vec![None; w]; h];
    for y in 3..9 {
        for x in 4..12 {
            put(&mut g, x, y, SKIN);
        }
    }
    for y in 9..16 {
        for x in 4..12 {
            put(&mut g, x, y, if x < 9 { BODY } else { BODY_D });
        }
    }
    let legs = [(5, 10), (4, 11), (6, 9)][pose % 3];
    for y in 16..21 {
        for x in [legs.0, legs.0 + 1, legs.1, legs.1 + 1] {
            put(&mut g, x, y, BODY_D);
        }
    }
    for x in (legs.0 - 1)..(legs.0 + 3) {
        put(&mut g, x, 20, RED);
        put(&mut g, x, 21, RED);
    }
    for x in (legs.1 - 1)..(legs.1 + 3) {
        put(&mut g, x, 20, RED);
        put(&mut g, x, 21, RED);
    }
    for ex in [5, 9] {
        for y in 4..7 {
            put(&mut g, ex, y, WHITE);
            put(&mut g, ex + 1, y, WHITE);
        }
        put(&mut g, ex + 1, 5, OUT);
        put(&mut g, ex + 1, 6, OUT);
    }
    let src = g.clone();
    for y in 0..h {
        for x in 0..w {
            if src[y][x].is_none() && [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|(dx, dy)| {
                let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h && src[ny as usize][nx as usize].is_some()
            }) {
                g[y][x] = Some(OUT);
            }
        }
    }
    let mut im = RgbaImage::new(w * scale, h * scale);
    for y in 0..h * scale {
        for x in 0..w * scale {
            if let Some(c) = g[y / scale][x / scale] {
                im.px[y * w * scale + x] = Rgba::opaque(c.0, c.1, c.2);
            }
        }
    }
    im
}

fn nearest_key(tpl: &str, rgb: (u8, u8, u8)) -> char {
    let mut best: Option<(i32, char)> = None;
    for l in tpl.lines() {
        let t = l.trim();
        if !t.starts_with('"') || !t.contains("color") {
            continue;
        }
        let key = t.chars().nth(1).unwrap();
        let Some(i) = t.find("color = \"#") else { continue };
        let hex = &t[i + 10..i + 16];
        let v = |k: usize| i32::from_str_radix(&hex[k..k + 2], 16).unwrap();
        let d = (v(0) - rgb.0 as i32).pow(2) + (v(2) - rgb.1 as i32).pow(2) + (v(4) - rgb.2 as i32).pow(2);
        if best.map_or(true, |b| d < b.0) {
            best = Some((d, key));
        }
    }
    best.unwrap().1
}

fn replace_in_grids(text: &str, from: char, to: char) -> String {
    text.lines().map(|l| if l.len() >= 8 && !l.contains(|c: char| " =\"[]#,".contains(c)) { l.replace(from, &to.to_string()) } else { l.to_string() }).collect::<Vec<_>>().join("\n") + "\n"
}

#[cfg(unix)]
fn copy_dir(from: &Path, to: &Path) {
    let st = std::process::Command::new("cp").arg("-R").arg(from).arg(to).status().unwrap();
    assert!(st.success());
}

/// A Mega Drive project with the hero (3 poses) and a sibling `hero2` made from the same picture, in one group.
fn base_project(tag: &str) -> PathBuf {
    let d = tmpdir(tag);
    ok(&pxl_in(&d, &["new", "proj", "--console", "md", "--no-gates"]));
    let p = d.join("proj");
    for i in 0..3 {
        png::write_rgba(&p.join(format!("inbox/hero_{:02}.png", i + 1)), &hero(i)).unwrap();
    }
    ok(&pxl_in(&p, &["ingest", "inbox", "--group", "player", "--factor", "4"]));
    png::write_rgba(&p.join("inbox/hero2.png"), &hero(0)).unwrap();
    ok(&pxl_in(&p, &["ingest", "inbox/hero2.png", "--group", "player", "--factor", "4"]));
    ok(&pxl_in(&p, &["intake", "--request", "mega drive"]));
    p
}

/// Findings of the five gates on one asset: rule -> count.
fn gates(p: &Path, asset: &str) -> BTreeMap<String, usize> {
    let o = pxl_in(p, &["lint", asset, "--rules", GATES, "--json"]);
    let j: Value = serde_json::from_str(stdout(&o).lines().last().unwrap()).unwrap();
    let mut m = BTreeMap::new();
    for i in j["parts"][0]["issues"].as_array().unwrap() {
        *m.entry(i["rule"].as_str().unwrap().to_string()).or_insert(0) += 1;
    }
    m
}

#[test]
fn unmutated_synthetic_assets_pass_every_gate() {
    let p = base_project("gates_neg");
    for a in ["hero", "hero2"] {
        let m = gates(&p, a);
        assert!(m.is_empty(), "false positives on {}: {:?}", a, m);
    }
}

#[test]
fn an_erased_eye_fires_eye_parity() {
    let p = base_project("gates_eye");
    let tpl = std::fs::read_to_string(p.join("assets/player/hero/template.toml")).unwrap();
    let skin = nearest_key(&tpl, SKIN).to_string();
    let mut args = vec!["set".to_string(), "hero".into(), "hero_02".into()];
    for c in ["5,4", "6,4", "5,5", "6,5", "5,6", "6,6"] {
        args.push(format!("{}={}", c, skin));
    }
    let a: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    ok(&pxl_in(&p, &a));
    let m = gates(&p, "hero");
    assert_eq!(m.get("eye_parity"), Some(&1), "{:?}", m);
}

#[test]
fn a_removed_identity_colour_fires_identity_colour() {
    let p = base_project("gates_ident");
    let f = p.join("assets/player/hero/asset.pxl");
    let tpl = std::fs::read_to_string(p.join("assets/player/hero/template.toml")).unwrap();
    let (red, dark) = (nearest_key(&tpl, RED), nearest_key(&tpl, BODY_D));
    std::fs::write(&f, replace_in_grids(&std::fs::read_to_string(&f).unwrap(), red, dark)).unwrap();
    let m = gates(&p, "hero");
    assert!(m.get("identity_colour").copied().unwrap_or(0) >= 3, "one per frame: {:?}", m);
}

#[test]
fn a_flattened_material_fires_flat() {
    let p = base_project("gates_flat");
    let f = p.join("assets/player/hero/asset.pxl");
    let tpl = std::fs::read_to_string(p.join("assets/player/hero/template.toml")).unwrap();
    let (body, dark) = (nearest_key(&tpl, BODY), nearest_key(&tpl, BODY_D));
    // shoes stay red; both blue tones become one
    std::fs::write(&f, replace_in_grids(&std::fs::read_to_string(&f).unwrap(), dark, body)).unwrap();
    let m = gates(&p, "hero");
    assert!(m.get("flat").copied().unwrap_or(0) >= 1, "{:?}", m);
}

#[test]
fn a_recoloured_ramp_in_one_sibling_fires_consistency() {
    let p = base_project("gates_cons");
    // hero2 draws the same picture; its blue ramp is recoloured much lighter (same source colours, different art)
    let tp = p.join("assets/player/hero2/template.toml");
    let tpl = std::fs::read_to_string(&tp).unwrap();
    let (body, dark) = (nearest_key(&tpl, BODY), nearest_key(&tpl, BODY_D));
    let mut out = String::new();
    for l in tpl.lines() {
        let t = l.trim();
        let is = |k: char| t.starts_with(&format!("\"{}\" =", k)) && t.contains("color");
        if is(body) || is(dark) {
            let i = l.find("color = \"#").unwrap() + 10;
            out.push_str(&format!("{}{}{}\n", &l[..i], if is(body) { "a0b0ff" } else { "7080d8" }, &l[i + 6..]));
        } else {
            out.push_str(l);
            out.push('\n');
        }
    }
    std::fs::write(&tp, out).unwrap();
    let m = gates(&p, "hero2");
    assert!(m.get("consistency").copied().unwrap_or(0) >= 1, "{:?}", m);
}

// ------------------------------------------------------------------------------------------ real art (only when present)

#[cfg(unix)]
fn real_root() -> Option<PathBuf> {
    let p = std::env::var("PXL_GATES_REAL").map(PathBuf::from).unwrap_or_default();
    p.join("pxl.toml").is_file().then_some(p)
}

#[cfg(unix)]
fn part_paths(root: &Path) -> Vec<String> {
    std::fs::read_to_string(root.join("pxl.toml")).unwrap().lines().filter_map(|l| l.strip_prefix("path = \"").and_then(|r| r.strip_suffix('"')).map(String::from)).collect()
}

#[cfg(unix)]
#[test]
fn the_approved_production_parts_stay_within_the_gate_baseline_and_a_painted_over_eye_fires() {
    let Some(src) = real_root() else {
        eprintln!("no real art (set PXL_GATES_REAL to a project copy's pxl/ folder): skipped");
        return;
    };
    let base: Value = serde_json::from_str(&std::fs::read_to_string("tests/fixtures/gates/baseline.json").unwrap()).unwrap();
    // a private copy next to a link to the art
    let tmp = tmpdir("gates_real");
    let art = std::fs::canonicalize(src.parent().unwrap().join("art")).expect("../art next to the project folder");
    std::os::unix::fs::symlink(&art, tmp.join("art")).unwrap();
    copy_dir(&src, &tmp.join("pxl"));
    let root = tmp.join("pxl");
    let parts = part_paths(&root);
    assert!(parts.len() > 100, "{} parts", parts.len());
    let mut args: Vec<String> = vec!["lint".into()];
    args.extend(parts.iter().cloned());
    args.extend(["--rules".into(), GATES.into(), "--json".into()]);
    let a: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let o = pxl_in(&root, &a);
    let j: Value = serde_json::from_str(stdout(&o).lines().last().unwrap()).unwrap();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut parts_with: BTreeMap<String, std::collections::BTreeSet<String>> = BTreeMap::new();
    for p in j["parts"].as_array().unwrap() {
        for i in p["issues"].as_array().unwrap().iter().filter(|i| i["severity"] == "warning") {
            let r = i["rule"].as_str().unwrap().to_string();
            *counts.entry(r.clone()).or_insert(0) += 1;
            parts_with.entry(r).or_default().insert(p["part"].as_str().unwrap().to_string());
        }
    }
    for g in GATES.split(',') {
        let n = counts.get(g).copied().unwrap_or(0);
        eprintln!("gate {:<16} {:>3} finding(s) in {:>2} of {} approved part(s)", g, n, parts_with.get(g).map_or(0, |s| s.len()), parts.len());
        let allowed = base["real"][g].as_u64().unwrap() as usize;
        assert!(n <= allowed, "gate {} reports {} finding(s) on approved art, the baseline allows {}: {:?}", g, n, allowed, parts_with.get(g));
    }
    // the mutation: the player's eye (white x17-18, y10-13, pupil x19) in 01_stand painted over with the body colour
    let pf = root.join("parts/characters/player/player.pxl");
    let text = std::fs::read_to_string(&pf).unwrap();
    let start = text.find("name = \"01_stand\"").unwrap();
    let grid_at = start + text[start..].find("grid = \"\"\"\n").unwrap() + "grid = \"\"\"\n".len();
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    let first = text[..grid_at].lines().count();
    for y in 10..=13 {
        let l = &mut lines[first + y];
        let mut cs: Vec<char> = l.chars().collect();
        for x in [17usize, 18] {
            cs[x] = '2';
        }
        *l = cs.into_iter().collect();
    }
    std::fs::write(&pf, lines.join("\n") + "\n").unwrap();
    let o = pxl_in(&root, &["lint", "player", "--rules", "eye_parity", "--json"]);
    let j: Value = serde_json::from_str(stdout(&o).lines().last().unwrap()).unwrap();
    let hits: Vec<&Value> = j["parts"][0]["issues"].as_array().unwrap().iter().filter(|i| i["rule"] == "eye_parity" && i["frame"] == "01_stand").collect();
    assert_eq!(hits.len(), 1, "the one-eyed player must be caught: {}", j);
}
