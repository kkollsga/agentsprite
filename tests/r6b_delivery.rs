//! M6 R6b: delivery manifest, meta, dual-use forms, zone completeness and real screens, source QA, sheet / package / verify-package /
//! deliver / changes, and the lint rules near_duplicate and stacked.
mod e2e_common;

use agentsprite::img::{Rgba, RgbaImage};
use agentsprite::png;
use e2e_common::*;
use serde_json::Value;
use std::path::{Path, PathBuf};

fn ok(o: &std::process::Output) -> String {
    assert!(o.status.success(), "{}{}", stdout(o), stderr(o));
    stdout(o)
}

fn bands(w: usize, h: usize, cols: &[(u8, u8, u8)]) -> RgbaImage {
    let mut im = RgbaImage::new(w, h);
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let (r, g, b) = cols[(y - 1) * cols.len() / (h - 2)];
            im.px[y * w + x] = Rgba::opaque(r, g, b);
        }
    }
    im
}

/// A planned and accepted project with three assets (hero, bug, ring), the manifest on, and one build.
fn project(tag: &str) -> PathBuf {
    let d = tmpdir(tag);
    ok(&pxl_in(&d, &["new", "proj", "--console", "gbc"]));
    let p = d.join("proj");
    let imgs: [(&str, &str, RgbaImage); 3] = [
        ("player", "hero", bands(16, 32, &[(255, 200, 160), (0, 80, 248), (200, 0, 0), (64, 40, 16)])),
        ("enemies", "bug", bands(16, 16, &[(248, 16, 8), (32, 56, 200), (88, 88, 96), (248, 248, 248)])),
        ("objects", "ring", bands(16, 16, &[(248, 200, 16), (248, 248, 248)])),
    ];
    for (g, n, im) in imgs {
        png::write_rgba(&p.join("inbox").join(format!("{}.png", n)), &im).unwrap();
        ok(&pxl_in(&p, &["ingest", &format!("inbox/{}.png", n), "--group", g]));
    }
    ok(&pxl_in(&p, &["plan", "--apply"]));
    ok(&pxl_in(&p, &["plan", "accept", "--by", "user"]));
    ok(&pxl_in(&p, &["build"]));
    p
}

fn toml_text(p: &Path) -> String {
    std::fs::read_to_string(p.join("pxl.toml")).unwrap()
}

fn manifest(p: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p.join("export/manifest.json")).unwrap()).unwrap()
}

fn part<'a>(m: &'a Value, n: &str) -> &'a Value {
    m["parts"].as_array().unwrap().iter().find(|x| x["name"] == n).unwrap_or_else(|| panic!("no part {}", n))
}

#[test]
fn the_build_writes_a_manifest_with_frames_cells_palettes_and_the_facts_from_meta() {
    let p = project("r6b_manifest");
    // the facts no pixel holds
    ok(&pxl_in(&p, &["meta", "ring", "set", "collision", "48x8"]));
    ok(&pxl_in(&p, &["meta", "ring", "set", "repeat", "stride=16,0", "count=4"]));
    ok(&pxl_in(&p, &["meta", "ring", "set", "residency", "streamed"]));
    ok(&pxl_in(&p, &["meta", "hero", "compose", "add", "ring", "8", "-8"]));
    ok(&pxl_in(&p, &["meta", "bug", "frame", "bug", "dropped", "false"]));
    let t = pxl_in(&p, &["meta", "ring", "show"]);
    assert!(stdout(&t).contains("collision"), "{}", stdout(&t));
    // the manifest is stale until the build regenerates it
    let c = pxl_in(&p, &["check"]);
    assert!(!c.status.success() && stdout(&c).contains("out of date"), "{}{}", stdout(&c), stderr(&c));
    ok(&pxl_in(&p, &["build"]));
    ok(&pxl_in(&p, &["check"]));
    let m = manifest(&p);
    assert_eq!(m["schema"], 1);
    assert_eq!(m["console"], "gbc");
    let ring = part(&m, "ring");
    assert_eq!(ring["collision"]["w"], 48);
    assert_eq!(ring["collision"]["differs"], true, "48 px collision against 16 px art");
    assert_eq!(ring["repeat"]["count"], 4);
    assert_eq!(ring["residency"], "streamed");
    assert_eq!(part(&m, "hero")["compose"][0]["part"], "ring");
    let f = &ring["frames"][0];
    assert_eq!(f["name"], "ring");
    assert_eq!(f["semantics"], "distinct");
    assert_eq!(f["size"][0], 16);
    assert!(f["cells"].as_array().unwrap().len() >= 2, "cells with x, y, slot: {}", f);
    assert!(f["cells"][0]["slot"].is_number());
    assert!(f["files"].as_array().unwrap().iter().any(|x| x.as_str().unwrap().ends_with("ring.png")));
    assert_eq!(ring["zones"][0], "main");
    // stable palette ids and native values per scene
    let pal = &m["scenes"][0]["obj_palettes"];
    let first = &pal[0];
    assert!(first["id"].as_str().unwrap().starts_with("main/"), "{}", first);
    assert_eq!(first["colors"].as_array().unwrap().len(), 3);
    assert_eq!(first["native"].as_array().unwrap().len(), 3);
    assert!(first["used_by"].is_array());
    assert!(m["palettes"].as_object().unwrap().contains_key(first["id"].as_str().unwrap()));
    // deterministic: a second build changes nothing
    let a = std::fs::read(p.join("export/manifest.json")).unwrap();
    ok(&pxl_in(&p, &["build", "--force"]));
    assert_eq!(a, std::fs::read(p.join("export/manifest.json")).unwrap());
    // a frame flip needs flip_of, and the meta must name real frames
    let bad = pxl_in(&p, &["meta", "bug", "frame", "bug", "semantics", "flip_x"]);
    assert!(!bad.status.success(), "flip without --of is refused: {}", stderr(&bad));
    ok(&pxl_in(&p, &["meta", "bug", "frame", "bug", "semantics", "flip_x", "--of", "ghost"]));
    let b = pxl_in(&p, &["build"]);
    assert!(!b.status.success() && (stdout(&b) + &stderr(&b)).contains("ghost"), "{}{}", stdout(&b), stderr(&b));
}

#[test]
fn slot_seven_variants_are_listed_with_the_parts_that_use_them() {
    let p = project("r6b_variant");
    let set = std::fs::read_to_string(p.join("palettes/main.toml")).unwrap();
    std::fs::write(p.join("palettes/main.toml"), format!("{}\n[[slots]]\nslot = 7\nname = \"dynamic\"\nvariants = {{ boss = [\"#c80000\", \"#f8c8a0\", \"#0050f8\"], sig = [\"#f81008\", \"#f8f8f8\", \"#2038c8\"] }}\n", set)).unwrap();
    let a = std::fs::read_to_string(p.join("assets/player/hero/asset.pxl")).unwrap();
    std::fs::write(p.join("assets/player/hero/asset.pxl"), a.replacen("name = \"hero\"\n", "name = \"hero\"\nvariant = \"sig\"\n", 1)).unwrap();
    ok(&pxl_in(&p, &["build", "--force"]));
    let m = manifest(&p);
    assert_eq!(part(&m, "hero")["palette_variant"], "sig");
    assert_eq!(m["scenes"][0]["palette_variants"]["sig"][0], "hero", "{}", m["scenes"][0]["palette_variants"]);
    let dynamic = m["scenes"][0]["obj_palettes"].as_array().unwrap().iter().find(|x| x["slot"] == 7).unwrap();
    assert_eq!(dynamic["id"], "main/dynamic");
    assert_eq!(dynamic["variants"]["sig"]["id"], "main/dynamic@sig");
    assert!(dynamic["variants"]["boss"]["colors"].as_array().unwrap().len() == 3);
}

#[test]
fn a_bg_form_is_listed_next_to_the_sprite_and_plan_knows_it() {
    let p = project("r6b_bg");
    png::write_rgba(&p.join("inbox/bridge.png"), &bands(64, 16, &[(130, 90, 40), (90, 60, 20)])).unwrap();
    ok(&pxl_in(&p, &["ingest", "inbox/bridge.png", "--group", "objects"]));
    ok(&pxl_in(&p, &["scene", "add-part", "main", "objects", "bridge"]));
    png::write_rgba(&p.join("inbox/bridge_bg.png"), &bands(64, 16, &[(130, 90, 40), (90, 60, 20)])).unwrap();
    ok(&pxl_in(&p, &["ingest", "inbox/bridge_bg.png", "--group", "bgforms"]));
    let t = ok(&pxl_in(&p, &["plan"]));
    assert!(t.contains("dual-use candidate: bridge") && t.contains("no BG form yet"), "{}", t);
    ok(&pxl_in(&p, &["meta", "bridge", "set", "bg_variant", "bridge_bg"]));
    let t2 = ok(&pxl_in(&p, &["plan"]));
    assert!(t2.contains("BG form: bridge_bg"), "{}", t2);
    ok(&pxl_in(&p, &["build"]));
    let m = manifest(&p);
    assert_eq!(part(&m, "bridge")["bg_variant"]["part"], "bridge_bg");
    assert_eq!(part(&m, "bridge")["forms"][1], "bg");
    // a BG form that is not a part is a build error
    ok(&pxl_in(&p, &["meta", "bridge", "set", "bg_variant", "nope"]));
    assert!(!pxl_in(&p, &["build"]).status.success());
}

#[test]
fn a_stale_client_spec_fails_the_build_and_the_generated_spec_follows_the_manifest() {
    let p = project("r6b_spec");
    std::fs::create_dir_all(p.join("client")).unwrap();
    std::fs::write(p.join("client/spec.json"), r#"{"parts": {"ring": {"frames": ["ring", "ring_old"]}, "hero": {"frames": ["hero"]}, "bug": {"frames": ["bug"]}}}"#).unwrap();
    let t = toml_text(&p);
    std::fs::write(p.join("pxl.toml"), format!("{}\n[delivery]\nspec = \"export/spec.json\"\nclient_spec = \"client/spec.json\"\n", t)).unwrap();
    let b = pxl_in(&p, &["build"]);
    let txt = stdout(&b) + &stderr(&b);
    assert!(!b.status.success() && txt.contains("ring/ring_old") && txt.contains("stale spec"), "{}", txt);
    std::fs::write(p.join("client/spec.json"), r#"{"parts": {"ring": {"frames": ["ring"]}, "hero": {"frames": ["hero"]}, "bug": {"frames": ["bug"]}}}"#).unwrap();
    ok(&pxl_in(&p, &["build"]));
    let spec: Value = serde_json::from_str(&std::fs::read_to_string(p.join("export/spec.json")).unwrap()).unwrap();
    assert_eq!(spec["parts"]["ring"]["frames"][0], "ring");
    // a frame the spec lacks is reported too
    std::fs::write(p.join("client/spec.json"), r#"{"parts": {"ring": {"frames": ["ring"]}, "hero": {"frames": []}, "bug": {"frames": ["bug"]}}}"#).unwrap();
    let b2 = pxl_in(&p, &["build"]);
    assert!(!b2.status.success() && (stdout(&b2) + &stderr(&b2)).contains("hero/hero is delivered but the spec does not list it"), "{}{}", stdout(&b2), stderr(&b2));
}

fn two_zones(p: &Path) {
    // a second scene with its own set: slot 0 means other colours there
    let set = std::fs::read_to_string(p.join("palettes/main.toml")).unwrap();
    let alt = set.replacen("colors = [", "colors = [\"#080808\", ", 1);
    let _ = alt;
    // keep the set legal: rebuild slot 0 by hand
    let mut alt = String::from("console = \"gbc\"\n");
    for i in 0..8 {
        alt.push_str(&format!("\n[[slots]]\nslot = {}\nname = \"z2_{}\"\ncolors = [\"#080808\", \"#101010\", \"#18{:02x}18\"]\n", i, i, (i + 1) * 8));
    }
    std::fs::write(p.join("palettes/alt.toml"), alt).unwrap();
    let t = format!("{}\n[scene.other]\npalette_set = \"alt\"\nlayout = \"layouts/other.json\"\n[scene.other.groups.only]\nbudget = 10\nparts = [\"bug\"]\n", toml_text(p));
    std::fs::write(p.join("pxl.toml"), t).unwrap();
}

#[test]
fn zone_completeness_flags_art_borrowed_across_zones_unless_a_slot_remap_is_checked() {
    let p = project("r6b_zone");
    two_zones(&p);
    std::fs::create_dir_all(p.join("layouts")).unwrap();
    std::fs::write(p.join("layouts/other.json"), r#"{"objects": {"0x26": "ring"}, "acts": {"1": [{"object": "bug", "x": 100, "y": 120}, {"object": "0x26", "x": 140, "y": 120}, {"object": "ghost", "x": 10, "y": 10}]}}"#).unwrap();
    let o = pxl_in(&p, &["scene", "other", "--layout", "--json"]);
    let j = json_of(&o);
    let kinds: Vec<&str> = j["layout"]["completeness"].as_array().unwrap().iter().map(|f| f["kind"].as_str().unwrap()).collect();
    assert!(kinds.contains(&"borrowed") && kinds.contains(&"no_part"), "{:?}", kinds);
    assert!(!o.status.success());
    let borrowed = j["layout"]["completeness"].as_array().unwrap().iter().find(|f| f["kind"] == "borrowed").unwrap();
    assert_eq!(borrowed["part"], "ring", "the id 0x26 maps to ring: {}", borrowed);
    // check reports it too
    let c = pxl_in(&p, &["check"]);
    assert!(stdout(&c).contains("zone completeness"), "{}", stdout(&c));
    // a remap that covers the slots, and a layout without the unknown object
    std::fs::write(p.join("layouts/other.json"), r#"{"objects": {"0x26": "ring"}, "acts": {"1": [{"object": "bug", "x": 100, "y": 120}, {"object": "0x26", "x": 140, "y": 120}]}}"#).unwrap();
    let m = manifest_part_slots(&p, "ring");
    let pairs: Vec<String> = m.iter().map(|s| format!("{}={}", s, s)).collect();
    ok(&pxl_in(&p, &["meta", "ring", "zone-remap", "other", &pairs.join(",")]));
    let o2 = pxl_in(&p, &["scene", "other", "--layout", "--json"]);
    let j2 = json_of(&o2);
    assert!(j2["layout"]["completeness"].as_array().unwrap().is_empty(), "{}", j2["layout"]);
    // a remap to a slot the set does not define is dead
    std::fs::write(p.join("palettes/alt.toml"), std::fs::read_to_string(p.join("palettes/alt.toml")).unwrap().split("\n[[slots]]\nslot = 7").next().unwrap().to_string()).unwrap();
    ok(&pxl_in(&p, &["meta", "ring", "zone-remap", "other", &m.iter().map(|s| format!("{}=7", s)).collect::<Vec<_>>().join(",")]));
    let o3 = pxl_in(&p, &["scene", "other", "--layout", "--json"]);
    assert!(json_of(&o3)["layout"]["completeness"].as_array().unwrap().iter().any(|f| f["kind"] == "remap_dead"), "{}{}", stdout(&o3), stderr(&o3));
}

fn manifest_part_slots(p: &Path, n: &str) -> Vec<u64> {
    let m = manifest(p);
    part(&m, n)["slots_used"].as_array().unwrap().iter().map(|x| x.as_u64().unwrap()).collect()
}

#[test]
fn real_screens_are_composed_from_the_layout_and_overflow_is_found() {
    let p = project("r6b_screen");
    std::fs::create_dir_all(p.join("layouts")).unwrap();
    let t = toml_text(&p).replacen("[scene.main]\n", "[scene.main]\nlayout = \"layouts/main.json\"\n", 1);
    std::fs::write(p.join("pxl.toml"), t).unwrap();
    // two rings far apart: fine
    std::fs::write(p.join("layouts/main.csv"), "").unwrap();
    std::fs::write(p.join("layouts/main.json"), r#"{"acts": {"1": [{"object": "ring", "x": 20, "y": 100}, {"object": "ring", "x": 400, "y": 100}]}}"#).unwrap();
    let o = pxl_in(&p, &["scene", "main", "--layout", "--json"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let a = &json_of(&o)["layout"]["acts"][0];
    assert!(a["worst_line"]["sprites"].as_u64().unwrap() <= 2, "{}", a);
    // seven rings on one spot overflow the scanline limit of 10
    let many: Vec<String> = (0..7).map(|_| r#"{"object": "ring", "x": 60, "y": 100}"#.to_string()).collect();
    std::fs::write(p.join("layouts/main.json"), format!(r#"{{"acts": {{"1": [{}], "2": [{{"object": "ring", "x": 20, "y": 100}}]}}}}"#, many.join(","))).unwrap();
    let o2 = pxl_in(&p, &["scene", "main", "--layout", "--json"]);
    assert!(!o2.status.success());
    let j = json_of(&o2);
    assert_eq!(j["layout"]["acts"][0]["act"], "1");
    assert_eq!(j["layout"]["acts"][0]["worst_line"]["ok"], false, "{}", j["layout"]["acts"][0]);
    assert_eq!(j["layout"]["acts"][1]["worst_line"]["ok"], true, "act 2 alone is fine");
    // one act only, and a CSV layout works the same
    let one = pxl_in(&p, &["scene", "main", "--layout", "--act", "2", "--json"]);
    assert!(one.status.success(), "{}{}", stdout(&one), stderr(&one));
    std::fs::write(p.join("layouts/main.csv"), "act,object,x,y\n1,ring,20,100\n1,ring,400,100\n").unwrap();
    let t2 = std::fs::read_to_string(p.join("pxl.toml")).unwrap().replace("layouts/main.json", "layouts/main.csv");
    std::fs::write(p.join("pxl.toml"), t2).unwrap();
    assert!(pxl_in(&p, &["scene", "main", "--layout"]).status.success());
    // the layout is listed in the manifest
    ok(&pxl_in(&p, &["build"]));
    assert_eq!(manifest(&p)["scenes"][0]["layout"]["objects"], 2);
}

#[test]
fn source_qa_turns_noise_borrowed_art_and_unused_labels_into_question_tasks() {
    let d = tmpdir("r6b_qa");
    ok(&pxl_in(&d, &["new", "proj", "--console", "gbc"]));
    let p = d.join("proj");
    // random garbage
    let mut im = RgbaImage::new(32, 32);
    let mut s = 12345u64;
    for px in im.px.iter_mut() {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        *px = Rgba::opaque((s >> 33) as u8, (s >> 41) as u8, (s >> 49) as u8);
    }
    std::fs::create_dir_all(p.join("inbox/door")).unwrap();
    png::write_rgba(&p.join("inbox/door/door.png"), &im).unwrap();
    std::fs::write(p.join("inbox/door/source.json"), r#"{"zone": "zone_2", "art_zone": "zone_4", "labels": [{"name": "open_3", "unused": true}]}"#).unwrap();
    let o = pxl_in(&p, &["ingest", "inbox/door/door.png", "--group", "objects"]);
    let t = stdout(&o) + &stderr(&o);
    assert!(o.status.success(), "{}", t);
    assert!(t.contains("source QA noise") && t.contains("source QA borrowed_art") && t.contains("source QA unused"), "{}", t);
    let tasks = ok(&pxl_in(&p, &["tasks", "--all"]));
    assert!(tasks.matches("[question]").count() >= 3, "{}", tasks);
    let st = std::fs::read_to_string(p.join("assets/objects/door/asset.status.json")).unwrap();
    assert!(st.contains("\"state\": \"blocked\""), "{}", st);
    let n = pxl_in(&p, &["next", "--agent", "a1"]);
    assert!(!n.status.success() || !stdout(&n).contains("door"), "a blocked asset is not handed out: {}{}", stdout(&n), stderr(&n));
    // running it again creates no duplicates
    let q = ok(&pxl_in(&p, &["qa", "door"]));
    assert!(q.contains("0 new question task"), "{}", q);
    let dry = ok(&pxl_in(&p, &["qa", "--dry-run"]));
    assert!(dry.contains("3 finding(s)"), "{}", dry);
    // clean art makes no finding
    png::write_rgba(&p.join("inbox/ok.png"), &bands(16, 16, &[(255, 0, 0), (0, 0, 255)])).unwrap();
    let o2 = pxl_in(&p, &["ingest", "inbox/ok.png", "--group", "objects"]);
    assert!(!(stdout(&o2) + &stderr(&o2)).contains("source QA"), "{}{}", stdout(&o2), stderr(&o2));
}

#[test]
fn sheet_package_verify_deliver_and_changes_close_the_loop() {
    let p = project("r6b_deliver");
    // sheet
    let o = pxl_in(&p, &["sheet", "enemies", "--on", "#9bbc0f", "--out", "e.png"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let im = png::read_rgba(&p.join("e.png")).unwrap();
    assert!(im.w > 10 && im.h > 10);
    assert_eq!((im.px[0].r, im.px[0].g, im.px[0].b), (0x9b, 0xbc, 0x0f), "drawn on the background colour");
    assert!(!pxl_in(&p, &["sheet", "nothing"]).status.success());
    // package
    let t = ok(&pxl_in(&p, &["package"]));
    assert!(t.contains("0 dropped") && t.contains("file(s)"), "{}", t);
    assert!(p.join("package/manifest.json").is_file() && p.join("package/package.json").is_file());
    let v = ok(&pxl_in(&p, &["verify-package"]));
    assert!(v.contains("PASS") && v.contains("3 of 3 part"), "{}", v);
    // an edited file, and an extra file, fail the verifier
    let f = std::fs::read_dir(p.join("package/enemies/bug")).unwrap().flatten().find(|e| e.file_name().to_string_lossy().ends_with(".png")).unwrap().path();
    let keep = std::fs::read(&f).unwrap();
    let mut bad = keep.clone();
    let n = bad.len();
    bad[n / 2] ^= 0xff;
    std::fs::write(&f, &bad).unwrap();
    std::fs::write(p.join("package/extra.png"), &keep).unwrap();
    let vb = pxl_in(&p, &["verify-package"]);
    let vt = stdout(&vb);
    assert!(!vb.status.success() && vt.contains("differs from the recorded hash") && vt.contains("unlisted: extra.png"), "{}", vt);
    // deliver refuses on our own failure and snapshots nothing
    let d = pxl_in(&p, &["deliver"]);
    assert!(d.status.success(), "deliver rebuilds the package, so the edit is gone: {}{}", stdout(&d), stderr(&d));
    assert!(p.join("deliveries/1/manifest.json").is_file() && p.join("deliveries/1/package/manifest.json").is_file());
    assert!(!p.join("package/extra.png").exists(), "no stale file survives a new package");
    // a client checker's output and the images of a pipeline are captured
    let d2 = pxl_in(&p, &["deliver", "--client-checker", "echo client says ok", "--client-pipeline", if cfg!(windows) { "copy manifest.json \"%PXL_REPORT_DIR%\\shot.json\" & echo pipeline done" } else { "cp manifest.json \"$PXL_REPORT_DIR/shot.json\"; echo pipeline done" }]);
    assert!(d2.status.success(), "{}{}", stdout(&d2), stderr(&d2));
    let r: Value = serde_json::from_str(&std::fs::read_to_string(p.join("deliveries/2/report.json")).unwrap()).unwrap();
    assert!(r["client_checker"]["stdout"].as_str().unwrap().contains("client says ok"));
    assert_eq!(r["report_images"][0], "shot.json");
    // a failing client checker refuses unless allowed
    let d3 = pxl_in(&p, &["deliver", "--client-checker", if cfg!(windows) { "echo broken & exit 3" } else { "echo broken; exit 3" }]);
    assert!(!d3.status.success() && !p.join("deliveries/3").exists(), "{}{}", stdout(&d3), stderr(&d3));
    let d4 = pxl_in(&p, &["deliver", "--client-checker", "exit 3", "--allow-client-fail"]);
    assert!(d4.status.success() && p.join("deliveries/3/report.json").is_file(), "{}{}", stdout(&d4), stderr(&d4));
    // a dropped frame stays out of the package; changes since delivery 1
    ok(&pxl_in(&p, &["meta", "ring", "frame", "ring", "dropped", "true", "--reason", "engine draws it"]));
    ok(&pxl_in(&p, &["meta", "bug", "set", "collision", "32x8"]));
    ok(&pxl_in(&p, &["build"]));
    let ch = ok(&pxl_in(&p, &["changes", "--since", "1"]));
    assert!(ch.contains("[frame_removed] ring/ring") && ch.contains("[footprint] bug"), "{}", ch);
    let chj = json_of(&pxl_in(&p, &["changes", "--since", "1", "--to", "3", "--json"]));
    assert_eq!(chj["changes"]["count"], 0, "deliveries 1 and 3 are the same: {}", chj);
    ok(&pxl_in(&p, &["package"]));
    assert!(!p.join("package/objects/ring/ring.png").exists(), "the dropped frame is not delivered, and the old file is gone");
    let pk = std::fs::read_to_string(p.join("package/package.json")).unwrap();
    assert!(pk.contains("engine draws it"), "{}", pk);
    assert!(!pxl_in(&p, &["changes", "--since", "9"]).status.success());
}

#[test]
fn lint_finds_near_duplicate_colours_in_one_palette() {
    let d = tmpdir("r6b_dup");
    std::fs::write(d.join("template.toml"), "name = \"t\"\n\n[layers.base]\n\".\" = { role = \"transparent\" }\n\"1\" = { role = \"outline\", color = \"#000000\" }\n\"2\" = { role = \"a\", color = \"#f8e000\" }\n\"3\" = { role = \"b\", color = \"#f8d800\" }\n").unwrap();
    std::fs::write(d.join("dup.pxl"), "name = \"dup\"\nsize = [8, 8]\ntemplate = \"template.toml\"\n\n[[frames]]\nname = \"f\"\ngrid = \"\"\"\n........\n.111111.\n.122331.\n.122331.\n.122331.\n.122331.\n.111111.\n........\n\"\"\"\n").unwrap();
    // older projects keep their results: the rule is off by default outside a project with the delivery manifest
    let off = pxl_in(&d, &["lint", "dup.pxl"]);
    assert!(!(stdout(&off) + &stderr(&off)).contains("near_duplicate"), "{}{}", stdout(&off), stderr(&off));
    let o = pxl_in(&d, &["lint", "dup.pxl", "--rules", "near_duplicate"]);
    let t = stdout(&o) + &stderr(&o);
    assert!(t.contains("near_duplicate") && t.contains("#f8e000") && t.contains("#f8d800"), "{}", t);
    assert!(o.status.success(), "a warning, not an error: {}", t);
    // distinct colours are fine
    std::fs::write(d.join("template.toml"), "name = \"t\"\n\n[layers.base]\n\".\" = { role = \"transparent\" }\n\"1\" = { role = \"outline\", color = \"#000000\" }\n\"2\" = { role = \"a\", color = \"#f8e000\" }\n\"3\" = { role = \"b\", color = \"#d85000\" }\n").unwrap();
    let o2 = pxl_in(&d, &["lint", "dup.pxl", "--rules", "near_duplicate"]);
    assert!(!(stdout(&o2) + &stderr(&o2)).contains("near_duplicate"), "{}{}", stdout(&o2), stderr(&o2));
}

#[test]
fn stacked_cells_outside_the_allowed_categories_must_survive_a_flatten() {
    let p = project("r6b_stacked");
    let slots = manifest_part_slots(&p, "ring");
    let other = (0..8u64).find(|s| !slots.contains(s)).unwrap();
    let used = slots[0];
    // the cell of the left half plus a second cell on top of it in a slot that lacks the ring's colours
    let json = format!("[{{\"x\": -8, \"y\": -8, \"slot\": {}}}, {{\"x\": 0, \"y\": -8, \"slot\": {}}}, {{\"x\": -8, \"y\": -8, \"slot\": {}}}]", used, used, other);
    std::fs::write(p.join("assets/objects/ring/ring.cells.json"), &json).unwrap();
    let o = pxl_in(&p, &["lint", "assets/objects/ring/asset.pxl", "--rules", "stacked"]);
    let t = stdout(&o) + &stderr(&o);
    assert!(!o.status.success() && t.contains("stacked cells") && t.contains("flattened"), "{}", t);
    // where the colours survive the flatten, stacking is allowed
    let json2 = format!("[{{\"x\": -8, \"y\": -8, \"slot\": {}}}, {{\"x\": 0, \"y\": -8, \"slot\": {}}}, {{\"x\": -8, \"y\": -8, \"slot\": {}}}]", used, used, used);
    std::fs::write(p.join("assets/objects/ring/ring.cells.json"), &json2).unwrap();
    let o2 = pxl_in(&p, &["lint", "assets/objects/ring/asset.pxl", "--rules", "stacked"]);
    assert!(o2.status.success(), "{}{}", stdout(&o2), stderr(&o2));
    // in a project with the delivery manifest the rule runs by default
    std::fs::write(p.join("assets/objects/ring/ring.cells.json"), &json).unwrap();
    let o3 = pxl_in(&p, &["lint", "assets/objects/ring/asset.pxl"]);
    assert!(!o3.status.success() && (stdout(&o3) + &stderr(&o3)).contains("stacked cells"), "{}{}", stdout(&o3), stderr(&o3));
}
