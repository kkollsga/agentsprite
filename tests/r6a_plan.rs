//! M6 R6a: `pxl plan` (propose scenes, fit before drawing, options for what does not fit, dual-use flags, accept gate),
//! `pxl palette plan` (one set for all inputs, reservations, what-if, alignment) and `pxl budget --what-if` with reservations.
mod e2e_common;

use agentsprite::img::{Rgba, RgbaImage};
use agentsprite::png;
use e2e_common::*;
use std::path::{Path, PathBuf};

fn ok(o: &std::process::Output) -> String {
    assert!(o.status.success(), "{}{}", stdout(o), stderr(o));
    stdout(o)
}

/// Horizontal bands of the given colours, `w` x `h`, with a one pixel transparent border.
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

/// A project with three assets in three groups.
fn project(tag: &str) -> PathBuf {
    let d = tmpdir(tag);
    ok(&pxl_in(&d, &["new", "proj", "--console", "gbc", "--no-gates"]));
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
    p
}

fn toml_text(p: &Path) -> String {
    std::fs::read_to_string(p.join("pxl.toml")).unwrap()
}

#[test]
fn plan_proposes_scenes_from_the_groups_and_measures_the_fit() {
    let p = project("r6a_propose");
    let before = toml_text(&p);
    let j = json_of(&pxl_in(&p, &["plan", "--json"]));
    let sc = &j["plan"]["scenes"][0];
    assert_eq!(sc["scene"], "main");
    assert_eq!(sc["proposed"], true);
    assert_eq!(sc["groups"].as_array().unwrap().len(), 3, "{}", sc);
    assert_eq!(sc["ok"], true, "{}", sc);
    assert!(sc["fit"]["vram"]["sprite_resident_cells"].as_u64().unwrap() > 0, "VRAM is measured: {}", sc["fit"]);
    let slots = sc["palette"]["slots"].as_array().unwrap();
    assert!(!slots.is_empty() && slots.len() <= 8, "{:?}", slots);
    assert_eq!(j["plan"]["saved"], false);
    assert_eq!(toml_text(&p), before, "a plain `plan` writes nothing");
    assert!(!p.join("plan.toml").exists());
    // a scene name per act
    let j2 = json_of(&pxl_in(&p, &["plan", "--scenes", "act1,act2", "--json"]));
    assert_eq!(j2["plan"]["scenes"].as_array().unwrap().len(), 2);
}

#[test]
fn plan_apply_writes_scenes_and_a_shared_palette_and_the_gate_holds_until_accept() {
    let p = project("r6a_apply");
    let o = pxl_in(&p, &["plan", "--apply"]);
    let t = ok(&o);
    assert!(t.contains("added scene(s) main") && t.contains("plan.toml written"), "{}", t);
    let toml = toml_text(&p);
    assert!(toml.contains("[scene.main]") && toml.contains("palette_set = \"main\"") && toml.contains("[scene.main.groups.player]"), "{}", toml);
    assert!(p.join("palettes/main.toml").is_file());
    assert!(std::fs::read_to_string(p.join("plan.toml")).unwrap().contains("state = \"proposed\""));
    // the assets were reduced into the shared set and the scene check passes
    let c = pxl_in(&p, &["check"]);
    assert!(c.status.success(), "{}{}", stdout(&c), stderr(&c));
    assert!(stdout(&c).contains("1 of 1 scene(s)"), "{}", stdout(&c));
    // the gate
    let n = pxl_in(&p, &["next", "--agent", "a1"]);
    assert!(!n.status.success() && stderr(&n).contains("plan is not accepted"), "{}{}", stdout(&n), stderr(&n));
    let cl = pxl_in(&p, &["claim", "assets/player/hero/asset.pxl", "--agent", "a1"]);
    assert!(!cl.status.success() && stderr(&cl).contains("plan is not accepted"), "{}{}", stdout(&cl), stderr(&cl));
    ok(&pxl_in(&p, &["plan", "accept", "--by", "user"]));
    assert!(std::fs::read_to_string(p.join("plan.toml")).unwrap().contains("state = \"accepted\""));
    let n2 = pxl_in(&p, &["next", "--agent", "a1"]);
    assert!(n2.status.success(), "{}{}", stdout(&n2), stderr(&n2));
    // changing the scenes invalidates the accepted plan
    let t = toml_text(&p).replace("budget = ", "budget = 1");
    std::fs::write(p.join("pxl.toml"), t).unwrap();
    let n3 = pxl_in(&p, &["claim", "assets/enemies/bug/asset.pxl", "--agent", "a2"]);
    assert!(!n3.status.success() && stderr(&n3).contains("scenes changed"), "{}{}", stdout(&n3), stderr(&n3));
}

#[test]
fn a_project_without_a_plan_hands_out_work_as_before() {
    let p = project("r6a_noplan");
    let n = pxl_in(&p, &["next", "--agent", "a1"]);
    assert!(n.status.success(), "{}{}", stdout(&n), stderr(&n));
}

#[test]
fn a_required_plan_blocks_next_until_there_is_one() {
    let p = project("r6a_required");
    let t = toml_text(&p).replace("strict = true", "strict = true\nplan = \"required\"");
    std::fs::write(p.join("pxl.toml"), t).unwrap();
    let n = pxl_in(&p, &["next", "--agent", "a1"]);
    assert!(!n.status.success() && stderr(&n).contains("requires a plan"), "{}{}", stdout(&n), stderr(&n));
}

#[test]
fn a_scene_that_does_not_fit_lists_options_with_costs_and_accept_needs_a_decision() {
    let p = project("r6a_infeasible");
    ok(&pxl_in(&p, &["plan", "--apply"]));
    // the BG producer holds back 170 sprite cells: the sprites no longer fit
    let t = format!("{}\n[scene.main.reserve]\nsprite_cells = 176\n", toml_text(&p));
    std::fs::write(p.join("pxl.toml"), t).unwrap();
    let j = json_of(&pxl_in(&p, &["plan", "--json"]));
    let sc = &j["plan"]["scenes"][0];
    assert_eq!(sc["ok"], false, "{}", sc);
    assert_eq!(j["plan"]["infeasible"], 1);
    let kinds: Vec<&str> = sc["options"].as_array().unwrap().iter().map(|o| o["kind"].as_str().unwrap()).collect();
    for k in ["cut", "bg_conversion", "act_split"] {
        assert!(kinds.contains(&k), "option {} missing: {:?}", k, kinds);
    }
    assert!(sc["options"].as_array().unwrap().iter().all(|o| o["cost"].is_string() && o["text"].is_string()));
    let text = ok(&pxl_in(&p, &["plan"]));
    assert!(text.contains("NOT FEASIBLE") && text.contains("Options"), "{}", text);
    // accept refuses, then accepts with a recorded decision
    let a = pxl_in(&p, &["plan", "accept"]);
    assert!(!a.status.success() && stderr(&a).contains("do not fit"), "{}{}", stdout(&a), stderr(&a));
    ok(&pxl_in(&p, &["plan", "accept", "--by", "user", "--decision", "BG gets the slack; the ring becomes BG"]));
    let d = std::fs::read_to_string(p.join("decisions.md")).unwrap();
    assert!(d.contains("plan accepted") && d.contains("the ring becomes BG"), "{}", d);
}

#[test]
fn wide_objects_are_flagged_as_dual_use_candidates() {
    let p = project("r6a_dual");
    png::write_rgba(&p.join("inbox/bridge.png"), &bands(64, 16, &[(130, 90, 40), (90, 60, 20)])).unwrap();
    ok(&pxl_in(&p, &["ingest", "inbox/bridge.png", "--group", "objects"]));
    let j = json_of(&pxl_in(&p, &["plan", "--json"]));
    let dual = j["plan"]["scenes"][0]["dual_use"].as_array().unwrap();
    assert!(dual.iter().any(|d| d["part"] == "bridge"), "{:?}", dual);
    assert!(!dual.iter().any(|d| d["part"] == "ring"), "the small ring stays a sprite");
}

#[test]
fn palette_plan_reports_loss_per_part_and_respects_reservations_and_what_if() {
    let p = project("r6a_palette");
    ok(&pxl_in(&p, &["plan", "--apply"]));
    let toml = toml_text(&p);
    let set_before = std::fs::read_to_string(p.join("palettes/main.toml")).unwrap();
    // a reservation keeps slot 7 out of the plan
    let j = json_of(&pxl_in(&p, &["palette", "plan", "main", "--reserve", "7", "--what-if", "--json"]));
    let slots: Vec<u64> = j["plan"]["slots"].as_array().unwrap().iter().map(|s| s["slot"].as_u64().unwrap()).collect();
    assert!(!slots.contains(&7), "{:?}", slots);
    assert_eq!(j["plan"]["applied"], false);
    assert_eq!(j["plan"]["parts"].as_array().unwrap().len(), 3);
    // measured against the SOURCE, per frame: the bug's dark grey (#585860) is a colour the source has and the plan cannot keep as such
    let bug = j["plan"]["parts"].as_array().unwrap().iter().find(|x| x["part"] == "bug").unwrap();
    assert_eq!(bug["frames"][0]["against"], "source", "{}", bug);
    assert_eq!(j["plan"]["identity_lost"], 1, "{}", j["plan"]);
    assert_eq!(bug["frames"][0]["identity_lost"][0]["source"], "#585860", "{}", bug);
    for part in j["plan"]["parts"].as_array().unwrap() {
        assert!(part["frames"].as_array().unwrap().iter().all(|f| f["against"] == "source" && f["mean_de"].is_number()), "{}", part);
    }
    // what-if with too few slots: the loss shows up, nothing is written
    let w = json_of(&pxl_in(&p, &["palette", "plan", "main", "--slots", "2", "--what-if", "--json"]));
    assert!(w["plan"]["identity_lost"].as_u64().unwrap() > 0, "two slots cannot hold five hues: {}", w["plan"]);
    assert!(w["plan"]["mean_de"].as_f64().unwrap() > j["plan"]["mean_de"].as_f64().unwrap());
    // what-if without a part
    let wo = json_of(&pxl_in(&p, &["palette", "plan", "main", "--without", "ring", "--what-if", "--json"]));
    assert_eq!(wo["plan"]["parts"].as_array().unwrap().len(), 2);
    assert_eq!(toml_text(&p), toml);
    assert_eq!(std::fs::read_to_string(p.join("palettes/main.toml")).unwrap(), set_before);
    let bad = pxl_in(&p, &["palette", "plan", "main", "--what-if", "--apply"]);
    assert!(!bad.status.success());
    // the scene's own reservation
    let t = format!("{}\n[scene.main.reserve]\npalette_slots = [6, 7]\n", toml);
    std::fs::write(p.join("pxl.toml"), t).unwrap();
    let r = json_of(&pxl_in(&p, &["palette", "plan", "main", "--json"]));
    assert!(r["plan"]["slots"].as_array().unwrap().iter().all(|s| s["slot"].as_u64().unwrap() < 6 || s["reserved"] == true), "{}", r["plan"]);
}

#[test]
fn palette_plan_apply_writes_the_set_and_reduces_the_parts_into_it() {
    let p = project("r6a_pal_apply");
    // two scenes over the same parts: the first plan is applied, the second is lined up with it
    let t = format!(
        "{}\n[scene.a]\ngroups = {{ all = {{ budget = 60, parts = [\"hero\", \"bug\", \"ring\"] }} }}\n\n[scene.b]\ngroups = {{ all = {{ budget = 60, parts = [\"hero\", \"bug\", \"ring\"] }} }}\n",
        toml_text(&p).replace("legacy = true", "legacy = true")
    );
    std::fs::write(p.join("pxl.toml"), t).unwrap();
    let o = pxl_in(&p, &["palette", "plan", "a", "--apply", "--json"]);
    let j = json_of(&o);
    assert_eq!(j["plan"]["applied"], true, "{}{}", stdout(&o), stderr(&o));
    assert!(p.join("palettes/a.toml").is_file());
    assert!(toml_text(&p).contains("palette_set = \"a\""));
    let part = std::fs::read_to_string(p.join("assets/player/hero/asset.pxl")).unwrap();
    assert!(part.contains("palette_set = \"a\""), "the part is reduced into the plan");
    assert!(j["plan"]["identity_lost"].as_u64().unwrap() <= 1, "{}", j["plan"]);
    // lint stays clean after the reduction
    let l = pxl_in(&p, &["lint", "assets/player/hero/asset.pxl"]);
    assert!(l.status.success(), "{}{}", stdout(&l), stderr(&l));
    // aligned plan for scene b: the same materials keep the same slot numbers
    let b = json_of(&pxl_in(&p, &["palette", "plan", "b", "--align", "a", "--what-if", "--json"]));
    let aligned = b["plan"]["slots"].as_array().unwrap().iter().filter(|s| s["aligned_to"] == s["slot"]).count();
    assert_eq!(aligned, b["plan"]["slots"].as_array().unwrap().len(), "{}", b["plan"]["slots"]);
}

#[test]
fn budget_what_if_shows_before_and_after_and_writes_nothing() {
    let p = project("r6a_whatif");
    ok(&pxl_in(&p, &["plan", "--apply"]));
    let toml = toml_text(&p);
    let o = pxl_in(&p, &["budget", "main", "--what-if", "remove ring", "--what-if", "budget enemies 0", "--what-if", "reserve sprite_cells 100", "--json"]);
    let j = json_of(&o);
    let w = &j["what_if"];
    assert_eq!(w["scene"], "main");
    let rows = w["rows"].as_array().unwrap();
    let enemies = rows.iter().find(|r| r["item"] == "group enemies").unwrap();
    assert_eq!(enemies["ok"], false, "the enemies budget of 0 is over");
    let objects = rows.iter().find(|r| r["item"] == "group objects").unwrap();
    assert_eq!(objects["after"], "0", "the ring is removed: {}", objects);
    let vram = rows.iter().find(|r| r["item"] == "sprite VRAM").unwrap();
    assert!(vram["after"].as_str().unwrap().parse::<u32>().unwrap() >= 100);
    assert_eq!(w["before_ok"], true);
    assert_eq!(w["after_ok"], false);
    assert!(!o.status.success());
    assert_eq!(toml_text(&p), toml, "what-if never writes");
    let bad = pxl_in(&p, &["budget", "main", "--what-if", "juggle ring"]);
    assert!(!bad.status.success() && stderr(&bad).contains("cannot read"), "{}", stderr(&bad));
}

#[test]
fn reservations_hold_slack_back_in_the_scene_check() {
    let p = project("r6a_reserve");
    ok(&pxl_in(&p, &["plan", "--apply"]));
    let base = json_of(&pxl_in(&p, &["scene", "main", "--json"]));
    assert_eq!(base["scene"]["ok"], true);
    let enemies = base["scene"]["rows"].as_array().unwrap().iter().find(|r| r["item"] == "group enemies").unwrap().clone();
    let used: u64 = enemies["used"].as_str().unwrap().parse().unwrap();
    let limit: u64 = enemies["limit"].as_str().unwrap().parse().unwrap();
    // reserve more than the group has left
    let t = format!("{}\n[scene.main.reserve]\ngroups = {{ enemies = {} }}\nbg_tiles = 10\n", toml_text(&p), limit - used + 1);
    std::fs::write(p.join("pxl.toml"), t).unwrap();
    let o = pxl_in(&p, &["scene", "main", "--json"]);
    assert!(!o.status.success());
    let j = json_of(&o);
    let row = j["scene"]["rows"].as_array().unwrap().iter().find(|r| r["item"] == "group enemies").unwrap().clone();
    assert_eq!(row["ok"], false, "{}", row);
    assert!(row["used"].as_str().unwrap().contains('+'), "{}", row);
    assert!(row["note"].as_str().unwrap().contains("reserved"), "{}", row);
    // a typo in the reservation is an error
    let t2 = toml_text(&p).replace("bg_tiles = 10", "bg_tile = 10");
    std::fs::write(p.join("pxl.toml"), t2).unwrap();
    let e = pxl_in(&p, &["scene", "main"]);
    assert!(!e.status.success() && stderr(&e).contains("unknown key 'bg_tile'"), "{}", stderr(&e));
}
