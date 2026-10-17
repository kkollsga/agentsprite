//! Review packets, per-frame rework notes that close on edit, rework metrics, `tile_base` and per-frame `phase`.

use agentsprite::img::{Rgba, RgbaImage};
use agentsprite::{png, pxl, status};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("pxl_review_{}_{}", tag, std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("refs")).unwrap();
    std::fs::create_dir_all(d.join("templates")).unwrap();
    std::fs::write(
        d.join("templates/syn.toml"),
        "name = \"syn\"\n[layers.base]\n\".\" = { role = \"transparent\" }\n\"1\" = { role = \"outline\", color = \"#182078\" }\n\"2\" = { role = \"body\", color = \"#4850b8\" }\n\"3\" = { role = \"skin\", color = \"#f0b090\" }\n",
    )
    .unwrap();
    d
}

fn pxl_in(d: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pxl")).current_dir(d).args(args).output().unwrap()
}

fn json(o: &Output) -> Value {
    serde_json::from_slice(&o.stdout).unwrap_or_else(|e| panic!("bad json {}: {}\n{}", e, String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)))
}

fn ok(o: Output) -> Output {
    assert!(o.status.success(), "{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    o
}

/// A 8x8 part with two frames whose references are exact 2x upscales of the grids.
fn make_part(d: &Path, name: &str, extra: &str) {
    let mut grids = Vec::new();
    for fr in ["a", "b"] {
        let mut im = RgbaImage::new(16, 16);
        let mut g = String::new();
        for y in 0..8 {
            for x in 0..8 {
                let k = if (2..6).contains(&x) && (2..6).contains(&y) { if (x + y) % 2 == 0 { b'1' } else { b'2' } } else { b'.' };
                g.push(k as char);
                if k != b'.' {
                    let c = if k == b'1' { [0x18, 0x20, 0x78] } else { [0x48, 0x50, 0xb8] };
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        im.px[(y * 2 + dy) * 16 + x * 2 + dx] = Rgba::opaque(c[0], c[1], c[2]);
                    }
                }
            }
            g.push('\n');
        }
        png::write_rgba(&d.join(format!("refs/{}_{}.png", name, fr)), &im).unwrap();
        grids.push(g);
    }
    let text = format!(
        "name = '{name}'\nsize = [8, 8]\norigin = [0, 0]\ntemplate = 'templates/syn.toml'\n{extra}[input]\nimages = 'refs/{name}_{{frame}}.png'\nfactor = 2\n[reduce]\nfactor = 2\n[[frames]]\nname = 'a'\ngrid = '''\n{}'''\n[[frames]]\nname = 'b'\ngrid = '''\n{}'''\n",
        grids[0], grids[1]
    );
    std::fs::write(d.join(format!("{}.pxl", name)), text).unwrap();
}

fn set_done(d: &Path, name: &str) {
    status::update(&d.join(format!("{}.pxl", name)), |s| s.state = "done".into()).unwrap();
}

#[test]
fn packet_has_compare_lineup_siblings_and_review_md() {
    let d = dir("packet");
    make_part(&d, "hero", "family = 'hero'\n");
    make_part(&d, "sidekick", "family = 'hero'\n");
    make_part(&d, "other", "family = 'bg'\n");
    make_part(&d, "listed", "[review]\nsiblings = ['other']\n");
    std::fs::copy(d.join("templates/syn.toml"), d.join("templates/bg.toml")).unwrap();
    for n in ["other", "listed"] {
        let t = std::fs::read_to_string(d.join(format!("{}.pxl", n))).unwrap().replace("templates/syn.toml", "templates/bg.toml");
        std::fs::write(d.join(format!("{}.pxl", n)), t).unwrap();
    }
    for n in ["hero", "listed"] {
        set_done(&d, n);
    }
    let v = json(&ok(pxl_in(&d, &["--json", "review", "hero.pxl", "listed.pxl", "--state", "done", "--out", "rv"])));
    let parts = v["parts"].as_array().unwrap();
    assert_eq!(parts.len(), 2);
    let hero = &parts[0];
    assert_eq!(hero["name"], "hero");
    assert!(!hero["sheets"].as_array().unwrap().is_empty());
    for f in hero["sheets"].as_array().unwrap().iter().chain(hero["lineup"].as_array().unwrap()) {
        assert!(d.join(f.as_str().unwrap()).is_file(), "{}", f);
    }
    let sibs: Vec<&str> = hero["siblings"].as_array().unwrap().iter().map(|s| s["part"].as_str().unwrap()).collect();
    assert!(sibs.iter().any(|s| s.starts_with("sidekick")), "{:?}", sibs);
    assert!(!sibs.iter().any(|s| s.starts_with("other")), "{:?}", sibs);
    let listed: Vec<&str> = parts[1]["siblings"].as_array().unwrap().iter().map(|s| s["part"].as_str().unwrap()).collect();
    assert!(listed.iter().any(|s| s.starts_with("other")), "{:?}", listed);
    assert_eq!(hero["frames"].as_array().unwrap().len(), 2);
    let md = std::fs::read_to_string(d.join("rv/hero/review.md")).unwrap();
    assert!(md.contains("CHECKLIST") || md.contains("Checklist"), "{}", md);
    assert!(md.contains("| frame |") && md.contains("| a |") && md.contains("| b |"));
    assert!(md.contains("pxl rework") && md.contains("Raw frames"));
    assert!(d.join("rv/hero/compare").is_dir());
    // --family filters
    let v = json(&ok(pxl_in(&d, &["--json", "review", "--state", "done", "--family", "bg", "--out", "rv2"])));
    assert!(v["parts"].as_array().unwrap().is_empty());
}

#[test]
fn rework_notes_close_when_the_frame_is_edited_and_metrics_count_rounds() {
    let d = dir("rework");
    make_part(&d, "hero", "");
    make_part(&d, "zed", "");
    set_done(&d, "hero");
    set_done(&d, "zed");
    ok(pxl_in(&d, &["--agent", "R", "rework", "hero.pxl", "--frames", "a,b", "--note", "eyes are missing; outline broken on b"]));
    let v = json(&ok(pxl_in(&d, &["--json", "status", "--rework"])));
    let r = &v["rework"][0];
    assert_eq!(r["round"], 1);
    assert_eq!(r["open"][0]["open_frames"], serde_json::json!(["a", "b"]));
    assert_eq!(r["open"][0]["by"], "R");
    // editing a closes its part of the note
    ok(pxl_in(&d, &["set", "hero.pxl", "a", "2,2=3", "--no-ref"]));
    let v = json(&ok(pxl_in(&d, &["--json", "status", "--rework"])));
    assert_eq!(v["rework"][0]["open"][0]["open_frames"], serde_json::json!(["b"]));
    ok(pxl_in(&d, &["set", "hero.pxl", "b", "2,2=3", "--no-ref"]));
    let v = json(&ok(pxl_in(&d, &["--json", "status", "--rework"])));
    assert!(v["rework"].as_array().unwrap().is_empty(), "{}", v);
    // second round on hero, zed rework once, then approve zed
    set_done(&d, "hero");
    ok(pxl_in(&d, &["--agent", "R", "rework", "hero.pxl", "--frames", "a", "--note", "shading flat"]));
    ok(pxl_in(&d, &["--agent", "R", "rework", "zed.pxl", "--note", "palette wrong"]));
    ok(pxl_in(&d, &["--agent", "R", "approve", "zed.pxl", "--note", "ok now"]));
    let v = json(&ok(pxl_in(&d, &["--json", "status", "--metrics"])));
    let m = &v["metrics"];
    assert_eq!(m["reworked_twice_plus"], 1);
    assert_eq!(m["reworked_once"], 1);
    assert_eq!(m["approved_first_time"], 0);
    let reasons: Vec<String> = m["top_reasons"].as_array().unwrap().iter().map(|r| format!("{}:{}", r["reason"], r["count"])).collect();
    assert!(reasons.iter().any(|r| r.contains("eyes")), "{:?}", reasons);
    let hero = m["per_part"].as_array().unwrap().iter().find(|p| p["name"] == "hero").unwrap();
    assert_eq!(hero["rounds"], 2);
    assert_eq!(hero["frames"]["a"]["rounds"], 2);
    // text mode prints the summary
    let t = String::from_utf8_lossy(&ok(pxl_in(&d, &["status", "--metrics"])).stdout).to_string();
    assert!(t.contains("reworked twice or more"), "{}", t);
    // the claim report shows frame-scoped notes
    let c = json(&ok(pxl_in(&d, &["--json", "--agent", "W", "claim", "hero.pxl"])));
    assert_eq!(c["task"], "rework");
}

#[test]
fn keywords_come_from_the_checklist() {
    let k = agentsprite::review::keywords("# List\n- Eyes: both?\n- **Outline**: closed\n2. Shoes present\nkeywordsx\n");
    assert_eq!(k, ["eyes", "outline", "shoes"]);
    let k = agentsprite::review::keywords("keywords: face, Limbs\n- other\n");
    assert_eq!(k, ["face", "limbs"]);
    assert!(agentsprite::review::keywords("CHECKLIST\n").len() >= 8);
    let kws = agentsprite::review::keywords("- eyes\n- outline\n");
    assert_eq!(agentsprite::review::reason_of("The Outline is doubled, eyes gone", &kws), "eyes");
    assert_eq!(agentsprite::review::reason_of("weird", &kws), "other");
}

#[test]
fn frame_phase_override_round_trips_and_moves_the_reference() {
    let d = dir("phase");
    make_part(&d, "hero", "");
    // shift the reference of frame b by one source pixel through the per-frame phase
    let text = std::fs::read_to_string(d.join("hero.pxl")).unwrap();
    let text = text.replacen("name = 'b'\n", "name = 'b'\nphase = [1, 0]\n", 1);
    std::fs::write(d.join("hero.pxl"), &text).unwrap();
    let part = pxl::load(&d.join("hero.pxl")).unwrap();
    assert_eq!(part.phase_for("a"), (0, 0));
    assert_eq!(part.phase_for("b"), (1, 0));
    let again = pxl::to_string(&part);
    assert!(again.contains("phase = [1, 0]"), "{}", again);
    let rf_a = agentsprite::load::reference(&part, &d, &part.frames[0]).unwrap();
    let rf_b = agentsprite::load::reference(&part, &d, &part.frames[1]).unwrap();
    assert_eq!(rf_b.origin.0, rf_a.origin.0 + 1);
    // the phase is part of the inputs hash: changing it makes the part stale
    let h1 = status::inputs_hash(&part);
    let mut p2 = part.clone();
    p2.frame_input.get_mut("b").unwrap().phase = Some((0, 1));
    assert_ne!(h1, status::inputs_hash(&p2));
}

#[test]
fn tile_set_packet_has_ranked_tile_pages() {
    let d = dir("tileset");
    let mut im = RgbaImage::new(32, 16);
    for y in 0..16 {
        for x in 0..32 {
            let c = if (x / 4 + y / 4) % 2 == 0 { [0x18, 0x20, 0x78] } else { [0x48, 0x50, 0xb8] };
            im.px[y * 32 + x] = Rgba::opaque(c[0], c[1], c[2]);
        }
    }
    png::write_rgba(&d.join("refs/blocks.png"), &im).unwrap();
    std::fs::write(
        d.join("templates/tiles.toml"),
        "name = \"tiles\"\n[layers.base]\n\"a\" = { role = \"air\", color = \"#88c8f8\" }\n\"1\" = { role = \"outline\", color = \"#182078\" }\n\"2\" = { role = \"body\", color = \"#4850b8\" }\n",
    )
    .unwrap();
    let blank = vec!["1".repeat(16); 8].join("\n");
    std::fs::write(
        d.join("ts.pxl"),
        format!("name = 'ts'\nkind = 'tileset'\nsize = [16, 8]\ntile = [8, 8]\ntemplate = 'templates/tiles.toml'\n[input]\nimages = 'refs/{{frame}}.png'\nfactor = 2\n[reduce]\nfactor = 2\n[[frames]]\nname = 'blocks'\ngrid = '''\n{blank}\n'''\n"),
    )
    .unwrap();
    set_done(&d, "ts");
    let v = json(&ok(pxl_in(&d, &["--json", "review", "ts.pxl", "--state", "done", "--out", "rv"])));
    let p = &v["parts"][0];
    assert!(!p["sheets"].as_array().unwrap().is_empty(), "{}", p);
    assert!(!p["lineup"].as_array().unwrap().is_empty(), "{}", p);
    assert!(d.join("rv/ts/review.md").is_file());
}

#[test]
fn rereview_removes_stale_generated_files_keeps_foreign_and_rehashes() {
    let d = dir("stale");
    make_part(&d, "hero", "");
    set_done(&d, "hero");
    let run = |d: &Path| json(&ok(pxl_in(d, &["--json", "review", "hero.pxl", "--state", "done", "--out", "rv", "--worst", "1"])));
    let head = |d: &Path| std::fs::read_to_string(d.join("rv/hero/review.md")).unwrap().lines().find(|l| l.starts_with("generated:")).unwrap().to_string();
    let v = run(&d);
    let h1 = head(&d);
    let zoom1: Vec<String> = v["parts"][0]["zoom"].as_array().unwrap().iter().map(|s| s.as_str().unwrap().to_string()).collect();
    assert!(!zoom1.is_empty());
    std::fs::write(d.join("rv/hero/notes.txt"), "mine").unwrap();
    std::fs::write(d.join("rv/hero/zoom/mine.txt"), "mine").unwrap();
    // a stale file the generator owns: plant it in the manifest
    std::fs::write(d.join("rv/hero/zoom/review-old.png"), "x").unwrap();
    let mf = d.join("rv/hero/.packet.json");
    let mut m: Value = serde_json::from_str(&std::fs::read_to_string(&mf).unwrap()).unwrap();
    m["files"].as_array_mut().unwrap().push(Value::String("zoom/review-old.png".into()));
    std::fs::write(&mf, m.to_string()).unwrap();
    ok(pxl_in(&d, &["set", "hero.pxl", "a", "2,2=3", "--no-ref"]));
    set_done(&d, "hero");
    run(&d);
    assert!(!d.join("rv/hero/zoom/review-old.png").exists());
    assert!(d.join("rv/hero/notes.txt").is_file() && d.join("rv/hero/zoom/mine.txt").is_file());
    assert_ne!(head(&d).split("part hash:").nth(1), h1.split("part hash:").nth(1));
    // frames with open rework notes get zoom pages beyond --worst
    ok(pxl_in(&d, &["--agent", "R", "rework", "hero.pxl", "--frames", "a,b", "--note", "eyes missing"]));
    set_done(&d, "hero");
    let md_zooms = |v: &Value| v["parts"][0]["zoom"].as_array().unwrap().len();
    let v = run(&d);
    assert!(md_zooms(&v) >= 2, "{}", v);
}
