//! M10: the acceptance findings of a fresh agent running a GBC project from the example inputs through the whole operator flow.
//! Stacked (two-slot) cells as a first-class option, plans measured against the source, a command for everything the skills forbid
//! hand-editing, stricter gates (flat tones, secondary identity colours and tones, one eye check shared with `compare`), a better identity list,
//! honest defaults and audit, and a style.md that is correct.
mod e2e_common;

use agentsprite::features::{detect, eye_drawn_size, Feature, Geo};
use agentsprite::img::{Rgba, RgbaImage};
use agentsprite::color::Rgb;
use e2e_common::*;
use serde_json::Value;
use std::path::{Path, PathBuf};

fn ok(o: &std::process::Output) -> String {
    assert!(o.status.success(), "{}{}", stdout(o), stderr(o));
    stdout(o)
}

fn fails(o: &std::process::Output) -> String {
    assert!(!o.status.success(), "expected a failure: {}", stdout(o));
    format!("{}{}", stdout(o), stderr(o))
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap()
}

/// A GBC project from the original example inputs: hero (3 poses, group player), crawler (enemies), gem (items). `ingest` only; nothing else is run.
fn example_project(tag: &str) -> PathBuf {
    let d = tmpdir(tag);
    ok(&pxl_in(&d, &["new", "game", "--console", "gbc", "--name", "Game"]));
    let p = d.join("game");
    for (dir, group) in [("hero", "player"), ("enemy", "enemies"), ("item", "items")] {
        e2e_common::copy_dir(&root().join("examples/inputs").join(dir), &p.join("inbox").join(dir));
        ok(&pxl_in(&p, &["ingest", &format!("inbox/{}", dir), "--group", group, "--factor", "2"]));
    }
    p
}

fn lint_json(p: &Path, parts: &[&str], rules: &str) -> Value {
    let mut a = vec!["lint"];
    a.extend(parts);
    a.extend(["--rules", rules, "--json"]);
    json_of(&pxl_in(p, &a))
}

fn issues(j: &Value, rule: &str) -> Vec<Value> {
    j["parts"].as_array().unwrap().iter().flat_map(|p| p["issues"].as_array().unwrap().clone()).filter(|i| i["rule"] == rule).collect()
}

// ------------------------------------------------------------------------------------------ 1. stacked sprites

#[test]
fn ingest_group_player_implies_the_player_category_and_the_mapping_is_configurable() {
    let p = example_project("m10_cat");
    assert!(read(&p.join("assets/player/hero/asset.pxl")).contains("category = \"player\""), "{}", read(&p.join("assets/player/hero/asset.pxl")));
    assert!(!read(&p.join("assets/enemies/crawler/asset.pxl")).contains("category"), "only the player group implies a category");
    // the mapping is a project setting; a group maps to any category
    ok(&pxl_in(&p, &["project", "set", "defaults.group_categories.hazards", "hazard"]));
    assert!(read(&p.join("pxl.toml")).contains("[project.defaults.group_categories]") && read(&p.join("pxl.toml")).contains("hazards = \"hazard\""));
    e2e_common::copy_dir(&root().join("examples/inputs/item"), &p.join("inbox/spike"));
    std::fs::rename(p.join("inbox/spike/gem.png"), p.join("inbox/spike/spike.png")).unwrap();
    let o = pxl_in(&p, &["ingest", "inbox/spike", "--group", "hazards", "--factor", "2"]);
    assert!(ok(&o).contains("category hazard"), "{}", stdout(&o));
    assert!(read(&p.join("assets/hazards/spike/asset.pxl")).contains("category = \"hazard\""));
    // the setting is shown
    assert!(ok(&pxl_in(&p, &["project", "show"])).contains("hazards=hazard"));
    // a bad category is refused
    assert!(fails(&pxl_in(&p, &["project", "set", "defaults.group_categories.x", "wizard"])).contains("unknown category"));
}

#[test]
fn a_command_sets_the_category_of_an_asset() {
    let p = example_project("m10_setcat");
    let o = pxl_in(&p, &["asset", "set", "crawler", "category", "player"]);
    let t = ok(&o);
    assert!(t.contains("category (none) -> player") && t.contains("two-slot"), "{}", t);
    assert!(read(&p.join("assets/enemies/crawler/asset.pxl")).contains("category = \"player\""));
    // the other spelling, and a change back
    ok(&pxl_in(&p, &["meta", "crawler", "set", "category", "enemy"]));
    let a = read(&p.join("assets/enemies/crawler/asset.pxl"));
    assert!(a.contains("category = \"enemy\"") && !a.contains("category = \"player\""), "{}", a);
    assert!(ok(&pxl_in(&p, &["asset", "set", "crawler", "category", "enemy"])).contains("already enemy"));
    assert!(fails(&pxl_in(&p, &["asset", "set", "crawler", "category", "wizard"])).contains("unknown category"));
    assert!(fails(&pxl_in(&p, &["asset", "set", "crawler", "colour", "red"])).contains("cannot be set here"));
}

#[test]
fn console_show_and_the_cells_reference_explain_stacked_cells() {
    let p = example_project("m10_show");
    let t = ok(&pxl_in(&p, &["console", "show", "gbc"]));
    assert!(t.contains("stacked sprites: category `player` only") && t.contains("slot2") && t.contains("2 sprites") && t.contains("multi_slot_max"), "{}", t);
    // a flat console has no palette-set cells to stack
    assert!(!ok(&pxl_in(&p, &["console", "show", "md"])).contains("stacked sprites"));
    let cells = read(&root().join("skills/pxl-sprite/references/cells.md"));
    for needle in ["Stacked sprites (two-slot cells, the player only)", "`slot2`", "multi_slot_max", "pxl asset set hero category player", "band offset", "not counted from the canvas corner"] {
        assert!(cells.contains(needle), "cells.md lacks {:?}", needle);
    }
    assert!(!cells.contains("a palette-set part is flat"), "the wrong statement is gone");
    for f in ["skills/pxl-sprite/references/cells.md", "skills/pxl-sprite/references/tool.md"] {
        assert!(!read(&root().join(f)).contains("counted from the canvas top-left"), "{}", f);
    }
}

#[test]
fn palette_plan_proposes_stacked_cells_for_player_cells_that_need_four_to_six_colours() {
    let p = example_project("m10_stack");
    ok(&pxl_in(&p, &["plan", "--apply"]));
    let j = json_of(&pxl_in(&p, &["palette", "plan", "main", "--what-if", "--json"]));
    let hero = j["plan"]["parts"].as_array().unwrap().iter().find(|x| x["part"] == "hero").unwrap().clone();
    assert_eq!(hero["player"], true, "{}", hero);
    assert_eq!(hero["stacked_allowed"], true, "the ingest category allows them");
    let cells = hero["stacked_cells"].as_array().unwrap();
    assert!(!cells.is_empty(), "the hero's face band needs more than three colours: {}", hero);
    for frame in ["hero_01", "hero_02", "hero_03"] {
        let n = cells.iter().filter(|c| c["frame"] == frame).count();
        assert!(n <= 2, "at most multi_slot_max (2) stacked cells per frame, {} has {}", frame, n);
    }
    for c in cells {
        let colours = c["colours"].as_u64().unwrap();
        assert!((4..=6).contains(&colours), "{}", c);
        assert!(c["after_de"].as_f64().unwrap() < c["before_de"].as_f64().unwrap(), "a stacked cell must gain: {}", c);
        assert_ne!(c["slots"][0], c["slots"][1]);
    }
    // only the player gets them
    for other in ["crawler", "gem"] {
        let o = j["plan"]["parts"].as_array().unwrap().iter().find(|x| x["part"] == other).unwrap();
        assert!(o["stacked_cells"].as_array().unwrap().is_empty(), "{}", o);
    }
    let t = ok(&pxl_in(&p, &["palette", "plan", "main", "--what-if"]));
    assert!(t.contains("stacked (two-slot) cell(s) proposed") && t.contains("2 sprites each"), "{}", t);
}

// ------------------------------------------------------------------------------------------ 2. plan against the source

#[test]
fn the_plan_measures_loss_per_frame_against_the_source_and_names_the_lost_hues() {
    let p = example_project("m10_src");
    let j = json_of(&pxl_in(&p, &["plan", "--json"]));
    let pp = &j["plan"]["scenes"][0]["palette"];
    let hero = pp["parts"].as_array().unwrap().iter().find(|x| x["part"] == "hero").unwrap();
    let frames = hero["frames"].as_array().unwrap();
    assert_eq!(frames.len(), 3);
    assert!(frames.iter().all(|f| f["against"] == "source" && f["mean_de"].as_f64().unwrap() > 0.0), "{}", hero);
    // the example hero has teal overalls, orange hair and red shoes that three-colour palette lines cannot all keep
    let lost: Vec<String> = frames.iter().flat_map(|f| f["identity_lost"].as_array().unwrap().iter().map(|l| l["source"].as_str().unwrap().to_string())).collect();
    assert!(!lost.is_empty(), "source colours the plan cannot keep are reported per frame: {}", hero);
    let text = ok(&pxl_in(&p, &["plan"]));
    assert!(text.contains("hero hero_01 loses") || text.contains("hero hero_02 loses"), "{}", text);
}

#[test]
fn no_row_prints_fail_for_a_value_under_its_limit_and_plan_apply_leaves_no_stale_comment() {
    let p = example_project("m10_rows");
    ok(&pxl_in(&p, &["plan", "--apply"]));
    let t = read(&p.join("pxl.toml"));
    assert!(!t.contains("legacy"), "the legacy line and its comment are gone: {}", t);
    // a group under its budget whose palette set does not fit the scene's: the count row stays ok, the problem has its own row
    std::fs::write(p.join("palettes/other.toml"), read(&p.join("palettes/main.toml")).replace("#e02828", "#e02820")).unwrap();
    ok(&pxl_in(&p, &["scene", "set", "main", "palette_set", "other"]));
    let j = json_of(&pxl_in(&p, &["scene", "main", "--json"]));
    let rows = j["scene"]["rows"].as_array().cloned().or_else(|| j["rows"].as_array().cloned()).unwrap_or_else(|| panic!("no rows: {}", j));
    let mut saw_problem_row = false;
    for r in &rows {
        if r["ok"] == false {
            if let (Ok(u), Ok(l)) = (r["used"].as_str().unwrap().parse::<u64>(), r["limit"].as_str().unwrap().parse::<u64>()) {
                assert!(u > l, "a FAIL row must be over its limit: {}", r);
            }
        }
        saw_problem_row |= r["item"].as_str().unwrap().ends_with("problems") && r["ok"] == false;
    }
    assert!(saw_problem_row, "the mismatch is reported as its own row: {}", j);
    let text = ok_or_text(&pxl_in(&p, &["scene", "main"]));
    assert!(!text.lines().any(|l| l.contains("FAIL") && l.contains("group player ") && !l.contains("problems")), "{}", text);
}

fn ok_or_text(o: &std::process::Output) -> String {
    format!("{}{}", stdout(o), stderr(o))
}

// ------------------------------------------------------------------------------------------ 3. commands for everything

#[test]
fn a_scene_group_budget_is_set_by_command() {
    let p = example_project("m10_budget");
    ok(&pxl_in(&p, &["plan", "--apply"]));
    ok(&pxl_in(&p, &["scene", "set", "main", "group.player.budget", "30"]));
    let t = read(&p.join("pxl.toml"));
    let block = t.split("[scene.main.groups.player]").nth(1).unwrap();
    assert!(block.lines().take(4).any(|l| l.trim() == "budget = 30"), "{}", t);
    let j = json_of(&pxl_in(&p, &["plan", "--json"]));
    let g = j["plan"]["scenes"][0]["groups"].as_array().unwrap().iter().find(|g| g["group"] == "player").unwrap().clone();
    assert_eq!(g["budget"], 30, "{}", g);
    assert!(fails(&pxl_in(&p, &["scene", "set", "main", "group.ghost.budget", "3"])).contains("no group 'ghost'"));
    assert!(fails(&pxl_in(&p, &["scene", "set", "main", "group.player.budget", "many"])).contains("whole number"));
    assert!(fails(&pxl_in(&p, &["scene", "set", "main", "group.player.colour", "3"])).contains("cannot be set"));
    // an over-budget scene fails `check`, and the message says what failed
    ok(&pxl_in(&p, &["scene", "set", "main", "group.player.budget", "1"]));
    let c = fails(&pxl_in(&p, &["check"]));
    assert!(c.contains("check failed:") && c.contains("hardware/scene issue") && c.contains("exceeds a shared budget"), "{}", c);
}

#[test]
fn the_identity_list_is_edited_by_command_and_the_gates_use_it() {
    let p = example_project("m10_ident");
    ok(&pxl_in(&p, &["intake", "--request", "gbc, faithful"]));
    let show = ok(&pxl_in(&p, &["style", "identity", "hero"]));
    assert!(show.contains("#f09030") && show.contains("[inferred]"), "{}", show);
    // the light teal collar is not on the inferred list (a 6 pixel highlight); put it there and the gate finds it missing
    assert!(!show.contains("#60d0c0"), "{}", show);
    assert!(!issues(&lint_json(&p, &["hero"], "identity_colour"), "identity_colour").iter().any(|i| i["message"].as_str().unwrap().contains("#60d0c0")));
    ok(&pxl_in(&p, &["style", "identity", "hero", "add", "#60D0C0"]));
    let j = lint_json(&p, &["hero"], "identity_colour");
    let collar: Vec<Value> = issues(&j, "identity_colour").into_iter().filter(|i| i["message"].as_str().unwrap().contains("#60d0c0")).collect();
    assert!(!collar.is_empty(), "the lost light teal collar fires once it is on the list: {}", j);
    assert!(read(&p.join("brief.toml")).contains("confirmed = true"));
    // remove it again: silent
    ok(&pxl_in(&p, &["style", "identity", "hero", "remove", "#60d0c0"]));
    assert!(!issues(&lint_json(&p, &["hero"], "identity_colour"), "identity_colour").iter().any(|i| i["message"].as_str().unwrap().contains("#60d0c0")));
    assert!(fails(&pxl_in(&p, &["style", "identity", "hero", "remove", "#123456"])).contains("not on the identity list"));
    assert!(fails(&pxl_in(&p, &["style", "identity", "ghost", "add", "#123456"])).contains("no asset 'ghost'"));
    assert!(fails(&pxl_in(&p, &["style", "identity", "hero", "add", "teal"])).contains("not a colour"));
    // the same edit through `decide --set`, which also sets the eyes; a list that is set replaces the old one and survives a new intake
    ok(&pxl_in(&p, &["decide", "the overalls and the hair are the hero", "--set", "identity.hero=#209898,#f09030", "--by", "user"]));
    ok(&pxl_in(&p, &["decide", "two eyes", "--set", "identity.hero.eyes=2", "--by", "user"]));
    let again = ok(&pxl_in(&p, &["intake", "--request", "gbc"]));
    assert!(again.contains("identity hero: 2 eye(s); colours #209898, #f09030 (confirmed, kept)"), "{}", again);
    let md = ok(&pxl_in(&p, &["style", "propose"]));
    let _ = md;
    assert!(read(&p.join("style.md")).contains("`hero`: 2 eye(s) with white and pupil; colours #209898 (teal), #f09030 (orange)"), "{}", read(&p.join("style.md")));
    // an edit after the proposal keeps style.md in step
    ok(&pxl_in(&p, &["style", "identity", "hero", "add", "#e02828"]));
    assert!(read(&p.join("style.md")).contains("#e02828 (red)"));
    // an empty confirmed list means no checks
    ok(&pxl_in(&p, &["style", "identity", "hero", "set"]));
    assert!(issues(&lint_json(&p, &["hero"], "identity_colour"), "identity_colour").is_empty());
}

#[test]
fn a_palette_slot_is_removed_by_command() {
    let p = example_project("m10_slot");
    ok(&pxl_in(&p, &["plan", "--apply"]));
    let before = read(&p.join("palettes/main.toml"));
    assert!(before.contains("slot = 5"));
    ok(&pxl_in(&p, &["palette", "slot", "remove", "main", "5"]));
    let after = read(&p.join("palettes/main.toml"));
    assert!(!after.contains("slot = 5") && after.contains("slot = 4"), "{}", after);
    assert!(fails(&pxl_in(&p, &["palette", "slot", "remove", "main", "5"])).contains("no own slot 5"));
    assert!(fails(&pxl_in(&p, &["palette", "slot", "remove", "nope", "1"])).contains("not found"));
    // the check now says which parts used it
    let c = pxl_in(&p, &["check"]);
    assert!(!c.status.success() || stdout(&c).contains("PASS"));
}

#[test]
fn project_settings_have_a_command_and_the_skills_name_it() {
    let p = example_project("m10_pset");
    ok(&pxl_in(&p, &["project", "set", "style.white", "balanced"]));
    ok(&pxl_in(&p, &["project", "set", "defaults.size", "32x32"]));
    ok(&pxl_in(&p, &["project", "set", "settings.note", "hello"]));
    let t = read(&p.join("pxl.toml"));
    assert!(t.contains("[style]") && t.contains("white = \"balanced\"") && t.contains("size = [32, 32]") && t.contains("note = \"hello\""), "{}", t);
    assert!(fails(&pxl_in(&p, &["project", "set", "defaults.colour", "red"])).contains("cannot set"));
    assert!(fails(&pxl_in(&p, &["project", "set", "defaults.size", "big"])).contains("WxH"));
    // the audit of the skills: no instruction asks for a hand edit of the manifest, the brief or the palettes
    for f in ["skills/pxl-operator/SKILL.md", "skills/pxl-operator/references/playbook.md", "skills/pxl-coordinate/SKILL.md"] {
        let s = read(&root().join(f));
        assert!(!s.contains("can override it in `[style]` of `pxl.toml`"), "{}", f);
        assert!(!s.contains("add what it missed\n  (`pxl decide"), "{}", f);
    }
    let op = read(&root().join("skills/pxl-operator/SKILL.md"));
    for cmd in ["pxl scene set SCENE group.GROUP.budget N", "pxl palette slot remove SET SLOT", "pxl asset set ASSET", "pxl style identity ASSET add|remove|set", "pxl project set"] {
        assert!(op.contains(cmd), "the operator skill lacks {:?}", cmd);
    }
}

// ------------------------------------------------------------------------------------------ 4. gates

#[test]
fn flat_flags_a_material_drawn_with_one_tone_where_the_source_has_two_or_more() {
    let p = example_project("m10_flat");
    // the first version draws the hero's orange hair and red shoes with one tone
    let j = lint_json(&p, &["hero"], "flat");
    let msgs: Vec<String> = issues(&j, "flat").iter().map(|i| format!("{} {}", i["frame"].as_str().unwrap(), i["message"].as_str().unwrap())).collect();
    assert!(msgs.iter().any(|m| m.starts_with("hero_01") && m.contains("the orange material")), "{:?}", msgs);
    // the message names the tone shares of the source and the share of the art that is one colour
    assert!(msgs.iter().any(|m| m.contains("% / ") && m.contains("% of its ")), "{:?}", msgs);
    // a material of a few cells is not judged, and the outline and the white are no material ("grey")
    assert!(!msgs.iter().any(|m| m.contains("the grey material")), "{:?}", msgs);
    // the crawler's red body drawn with one tone (the dark and the light red folded into the body red) fires too
    let tpl = read(&p.join("assets/enemies/crawler/template.toml"));
    let key = |hex: &str| tpl.lines().find(|l| l.contains(&format!("\"{}\"", hex)) && l.contains("color")).and_then(|l| l.trim().chars().nth(1));
    let f = p.join("assets/enemies/crawler/asset.pxl");
    let mut t = read(&f);
    if let Some(mid) = key("#e02828") {
        for dark_or_light in ["#901820", "#ff7860"] {
            if let Some(k) = key(dark_or_light) {
                t = t.lines().map(|l| if l.len() >= 8 && !l.contains(|c: char| " =\"[]#,".contains(c)) { l.replace(k, &mid.to_string()) } else { l.to_string() }).collect::<Vec<_>>().join("\n") + "\n";
            }
        }
    }
    std::fs::write(&f, t).unwrap();
    let j = lint_json(&p, &["crawler"], "flat");
    let msgs: Vec<String> = issues(&j, "flat").iter().map(|i| i["message"].as_str().unwrap().to_string()).collect();
    assert!(msgs.iter().any(|m| m.contains("the red material has") && m.contains("one flat colour (#e02828)")), "the flat red body fires: {:?}", msgs);
}

#[test]
fn identity_colour_checks_secondary_colours_and_tone_structure() {
    let p = example_project("m10_tone");
    ok(&pxl_in(&p, &["intake", "--request", "gbc"]));
    let j = lint_json(&p, &["hero"], "identity_colour");
    let is = issues(&j, "identity_colour");
    // the dark teal of the overalls and the light of the hair are secondary colours of materials the art kept in one tone
    let tone: Vec<String> = is.iter().map(|i| i["message"].as_str().unwrap().to_string()).filter(|m| m.contains("tone structure of the material is lost")).collect();
    assert!(tone.iter().any(|m| m.contains("#106070") || m.contains("#209898")), "{:?}", tone);
    assert!(tone.iter().any(|m| m.contains("lighter") || m.contains("darker")), "{:?}", tone);
}

#[test]
fn eye_parity_agrees_with_compare_and_checks_the_style_minimum() {
    let p = example_project("m10_eyes");
    ok(&pxl_in(&p, &["intake", "--request", "gbc"]));
    ok(&pxl_in(&p, &["style", "propose"]));
    // compare's "eyes found N kept M" and the gate use one check: per frame, found - kept = the lost eyes the gate reports
    let c = json_of(&pxl_in(&p, &["compare", "hero", "--json"]));
    let frames = c["frames"].as_array().unwrap_or_else(|| panic!("{}", c)).clone();
    let j = lint_json(&p, &["hero"], "eye_parity");
    for f in &frames {
        let name = f["frame"].as_str().unwrap();
        let lost = f["eyes_found"].as_u64().unwrap() - f["eyes_kept"].as_u64().unwrap();
        let reported = issues(&j, "eye_parity").iter().filter(|i| i["frame"] == name && i["message"].as_str().unwrap().contains("lost its white or its pupil")).count() as u64;
        assert_eq!(lost, reported, "{}: compare says {} lost, the gate reports {}", name, lost, reported);
    }
    // the minimum comes from style.md and is derived from the source, never below 2x3
    let md = read(&p.join("style.md"));
    assert!(md.contains("Minimum size at factor 2: white 2x3 cells"), "{}", md);
    assert!(read(&p.join("style.toml")).contains("min_white = [2, 3]"));
    assert!(read(&root().join("skills/pxl-sprite/SKILL.md")).contains("each at least 2x3 target pixels of white"));
}

#[test]
fn the_eye_minimum_is_checked_on_the_drawn_white() {
    // a 1x2 white with a pupil beside it: drawn, but below 2x3
    let eye = Feature {
        white: vec![],
        pupil: vec![],
        wbox: (0, 0, 3, 3),
        wc: (3.5, 3.5),
        pc: (4.5, 3.5),
        gaze: (1.0, 0.0),
        cells: (3, 3, 4, 4),
        wcells: vec![(3, 3, 4), (3, 4, 4)],
        pcells: vec![(4, 3, 4)],
        cell_px: 4,
    };
    let white = Rgb { r: 248, g: 248, b: 248 };
    let dark = Rgb { r: 16, g: 24, b: 48 };
    let body = Rgb { r: 200, g: 100, b: 20 };
    let small = |x: i32, y: i32| -> Option<Rgb> {
        match (x, y) {
            (3, 3) | (3, 4) => Some(white),
            (4, 3) => Some(dark),
            (4, 4) => Some(body),
            _ => None,
        }
    };
    assert_eq!(eye_drawn_size(&small, &eye, 0.85), Some((1, 2)));
    let big = |x: i32, y: i32| -> Option<Rgb> {
        match (x, y) {
            (2..=3, 3..=5) => Some(white),
            (4, 3) => Some(dark),
            _ => None,
        }
    };
    assert_eq!(eye_drawn_size(&big, &eye, 0.85), Some((2, 3)));
    let none = |_x: i32, _y: i32| -> Option<Rgb> { Some(body) };
    assert_eq!(eye_drawn_size(&none, &eye, 0.85), None);
}

#[test]
fn review_approve_requires_justify_while_a_gate_warns_and_the_note_says_which() {
    let p = example_project("m10_approve");
    // the hero warns (flat, identity_colour): the verdict is refused without a written reason, whoever asks
    let b = fails(&pxl_in(&p, &["review", "approve", "hero", "--agent", "rev"]));
    assert!(b.contains("approval blocked by") && b.contains("flat") && b.contains("identity_colour") && b.contains("--justify"), "{}", b);
    assert!(fails(&pxl_in(&p, &["approve", "hero", "--agent", "rev"])).contains("approval blocked"));
    assert!(fails(&pxl_in(&p, &["review", "approve", "hero", "--agent", "rev", "--justify", "  "])).contains("approval blocked"), "a blank reason is no reason");
    ok(&pxl_in(&p, &["review", "approve", "hero", "--agent", "rev", "--justify", "the hair is one tone on purpose: the slot holds three colours"]));
    let st = read(&p.join("assets/player/hero/asset.status.json"));
    assert!(st.contains("justified design warnings (") && st.contains("flat") && st.contains("on purpose"), "{}", st);
    // the crawler's gate warnings (a lost key colour) block it the same way
    assert!(fails(&pxl_in(&p, &["review", "approve", "crawler", "--agent", "rev"])).contains("--justify"));
}

// ------------------------------------------------------------------------------------------ 5. intake

#[test]
fn the_intake_identity_list_is_weighted_by_area_saturation_and_distinctness() {
    let p = example_project("m10_intake");
    let t = ok(&pxl_in(&p, &["intake", "--request", "gbc"]));
    let line = t.lines().find(|l| l.contains("identity hero:")).unwrap().to_string();
    // the dominant orange hair and the teal overalls first; not the 6 pixel collar highlight, no grey material on a hero without grey
    let first = line.split("colours ").nth(1).unwrap();
    assert!(first.starts_with("#f09030 (orange)"), "{}", line);
    assert!(line.contains("#209898 (teal)") && !line.contains("#60d0c0"), "{}", line);
    assert!(!line.contains("grey"), "{}", line);
    assert!(line.contains("orange x") && line.contains("teal x"), "{}", line);
    // the small hair highlight (#ffc870, 16 px of 73) is a tone of the hair, not a colour of its own beyond the material's tones
    let brief = read(&p.join("brief.toml"));
    assert!(brief.contains("[identity.hero]") && brief.contains("\"#209898\""), "{}", brief);
    // the gem: no eyes, and a glint is not an eye
    assert!(line.contains("2 eye(s)"));
    assert!(t.contains("identity gem: no eyes"), "{}", t);
    let again = pxl_in(&p, &["ingest", "inbox", "--factor", "2"]);
    let _ = again;
}

#[test]
fn a_glint_is_not_an_eye_when_the_first_version_protects_eyes() {
    let d = tmpdir("m10_glint");
    ok(&pxl_in(&d, &["new", "game", "--console", "gbc", "--name", "G"]));
    let p = d.join("game");
    e2e_common::copy_dir(&root().join("examples/inputs/item"), &p.join("inbox/item"));
    let o = ok(&pxl_in(&p, &["ingest", "inbox/item", "--group", "items", "--factor", "2"]));
    assert!(!o.contains("eye pixel"), "the gem's white glint is no eye: {}", o);
    e2e_common::copy_dir(&root().join("examples/inputs/hero"), &p.join("inbox/hero"));
    let o = ok(&pxl_in(&p, &["ingest", "inbox/hero", "--group", "player", "--factor", "2"]));
    assert!(o.contains("eye pixel(s) protected"), "the hero's eyes are: {}", o);
}

// ------------------------------------------------------------------------------------------ 6. defaults and audit

#[test]
fn plan_accept_and_style_approve_record_the_agent_unless_the_user_signs() {
    let p = example_project("m10_by");
    ok(&pxl_in(&p, &["plan", "--apply"]));
    ok(&pxl_in(&p, &["plan", "accept"]));
    assert!(read(&p.join("plan.toml")).contains("accepted_by = \"agent\""), "{}", read(&p.join("plan.toml")));
    ok(&pxl_in(&p, &["plan", "accept", "--by", "user"]));
    assert!(read(&p.join("plan.toml")).contains("accepted_by = \"user\""));
    ok(&pxl_in(&p, &["intake", "--request", "gbc"]));
    ok(&pxl_in(&p, &["style", "propose"]));
    ok(&pxl_in(&p, &["style", "approve"]));
    assert!(read(&p.join("style.md")).contains("Status: approved by agent on "), "{}", read(&p.join("style.md")));
    ok(&pxl_in(&p, &["style", "propose", "--force"]));
    ok(&pxl_in(&p, &["style", "approve", "--by", "user"]));
    assert!(read(&p.join("style.md")).contains("Status: approved by user on "));
    // an `--agent` name does not turn into the user
    ok(&pxl_in(&p, &["--agent", "artist", "style", "propose", "--force"]));
    ok(&pxl_in(&p, &["--agent", "artist", "style", "approve"]));
    assert!(read(&p.join("style.md")).contains("Status: approved by agent on "));
}

#[test]
fn status_lint_counts_equal_lint_and_ingested_sprites_are_raw() {
    let p = example_project("m10_status");
    // straight from ingest every sprite frame is still the automatic output
    let raw = ok(&pxl_in(&p, &["status", "--raw"]));
    assert!(raw.contains("hero/asset.pxl: 3 raw") && raw.contains("crawler/asset.pxl: 2 raw") && raw.contains("gem/asset.pxl: 1 raw"), "{}", raw);
    let c = fails(&pxl_in(&p, &["check", "--hand-finished"]));
    assert!(c.contains("still the automatic output") && c.contains("check failed:") && c.contains("part problem(s)"), "{}", c);
    // `pxl lint` writes what it found where `status` shows it, warnings included
    let l = json_of(&pxl_in(&p, &["lint", "hero", "crawler", "gem", "--json"]));
    let s = json_of(&pxl_in(&p, &["status", "--json"]));
    for part in l["parts"].as_array().unwrap() {
        let name = part["part"].as_str().unwrap();
        let st = s["parts"].as_array().unwrap().iter().find(|x| x["part"].as_str().map_or(false, |n| n.contains(name)) || x["path"].as_str().map_or(false, |n| n.contains(&format!("/{}/", name)))).unwrap_or_else(|| panic!("no status for {}: {}", name, s));
        assert_eq!(st["lint"]["warnings"], part["warnings"], "{}: status {} vs lint {}", name, st["lint"], part["warnings"]);
        assert_eq!(st["lint"]["errors"], part["errors"]);
    }
    assert!(l["parts"].as_array().unwrap().iter().any(|x| x["warnings"].as_u64().unwrap() > 0), "the example has warnings to count");
}

#[test]
fn deliver_dry_run_covers_every_part_of_the_scenes_and_says_what_it_skipped() {
    let p = example_project("m10_dry");
    ok(&pxl_in(&p, &["intake", "--request", "gbc"]));
    ok(&pxl_in(&p, &["style", "propose"]));
    ok(&pxl_in(&p, &["style", "mark", "hero"]));
    ok(&pxl_in(&p, &["plan", "--apply"]));
    // a part that is in no scene
    e2e_common::copy_dir(&root().join("examples/inputs/item"), &p.join("inbox/orphan"));
    std::fs::rename(p.join("inbox/orphan/gem.png"), p.join("inbox/orphan/orphan.png")).unwrap();
    ok(&pxl_in(&p, &["ingest", "inbox/orphan", "--group", "misc", "--factor", "2"]));
    let o = pxl_in(&p, &["deliver", "--dry-run", "--json"]);
    let j = json_of(&o);
    let d = &j["deliver"];
    let parts: Vec<String> = d["parts"].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect();
    for n in ["hero", "crawler", "gem"] {
        assert!(parts.contains(&n.to_string()), "the dry run covers {}: {:?}", n, parts);
    }
    assert_eq!(d["skipped"], serde_json::json!(["orphan"]), "{}", d);
    let t = pxl_in(&p, &["deliver", "--dry-run"]);
    assert!(stdout(&t).contains("not covered by this dry run") && stdout(&t).contains("orphan"), "{}{}", stdout(&t), stderr(&t));
}

#[test]
fn a_review_note_and_a_rework_of_one_issue_make_one_task() {
    let p = example_project("m10_dup");
    ok(&pxl_in(&p, &["review", "note", "hero", "hero_02", "torso noise at the belly", "--agent", "rev"]));
    ok(&pxl_in(&p, &["review", "note", "hero", "hero_03", "the shoes are flat", "--agent", "rev"]));
    let r = ok(&pxl_in(&p, &["review", "rework", "hero", "--note", "torso noise and flat shoes: see the review notes", "--agent", "rev"]));
    assert!(r.contains("no duplicate created"), "{}", r);
    let tasks = json_of(&pxl_in(&p, &["tasks", "--all", "--json"]));
    let open = tasks["tasks"].as_array().unwrap().iter().filter(|t| t["asset"] == "hero" && t["status"] == "open" && t["kind"] == "fix").count();
    assert_eq!(open, 2, "one task per note, none for the rework: {}", tasks);
    // a rework without notes still makes its task; approving settles the tasks of the notes
    ok(&pxl_in(&p, &["review", "rework", "crawler", "--note", "the eyes are small", "--agent", "rev"]));
    let tasks = json_of(&pxl_in(&p, &["tasks", "--all", "--json"]));
    assert_eq!(tasks["tasks"].as_array().unwrap().iter().filter(|t| t["asset"] == "crawler" && t["status"] == "open").count(), 1);
}

#[test]
fn the_report_separates_approved_from_review_and_the_packet_knows_a_style_frame() {
    let p = example_project("m10_report");
    ok(&pxl_in(&p, &["intake", "--request", "gbc"]));
    ok(&pxl_in(&p, &["style", "propose"]));
    ok(&pxl_in(&p, &["style", "mark", "hero"]));
    ok(&pxl_in(&p, &["release", "hero", "--agent", "a", "--state", "done"]));
    ok(&pxl_in(&p, &["release", "crawler", "--agent", "a", "--state", "done"]));
    ok(&pxl_in(&p, &["review", "approve", "crawler", "--agent", "rev", "--justify", "test"]));
    let rep = ok(&pxl_in(&p, &["report"]));
    assert!(rep.contains("APPROVED (1)") && !rep.contains("DONE ("), "{}", rep);
    assert!(rep.contains("IN REVIEW (1)"), "{}", rep);
    let j = json_of(&pxl_in(&p, &["report", "--json"]));
    assert!(j["approved"].as_array().unwrap().iter().any(|x| x["asset"] == "crawler") && j.get("done").is_none(), "{}", j);
    // the review packet of a style frame does not point at another asset
    ok(&pxl_in(&p, &["review", "--state", "done"]));
    let md = read(&p.join("assets/player/hero/review/hero/review.md"));
    assert!(md.contains("This asset is a style frame itself") && !md.contains("Nearest style frame:"), "{}", md);
    assert!(ok(&pxl_in(&p, &["brief", "hero"])).len() > 0);
}

#[test]
fn the_delivery_manifest_names_the_project_and_the_palette_encoding() {
    let p = example_project("m10_manifest");
    ok(&pxl_in(&p, &["plan", "--apply"]));
    ok(&pxl_in(&p, &["build"]));
    let m: Value = serde_json::from_str(&read(&p.join("export/manifest.json"))).unwrap();
    assert_eq!(m["project"], "Game", "{}", m["project"]);
    let enc = m["palette_encoding"].as_str().unwrap();
    assert!(enc.contains("RGB555") && enc.contains("OBJ"), "{}", enc);
    assert_eq!(m["console"], "gbc");
}

// ------------------------------------------------------------------------------------------ 7. style.md

#[test]
fn style_md_has_a_feasible_eye_minimum_and_a_ramp_table_from_source_colours() {
    let p = example_project("m10_style");
    ok(&pxl_in(&p, &["intake", "--request", "gbc"]));
    ok(&pxl_in(&p, &["style", "propose"]));
    let md = read(&p.join("style.md"));
    assert!(md.contains("Minimum size at factor 2: white 2x3 cells and a 1x1 pupil"), "{}", md);
    // every ramp row: distinct dark / mid / light, a `-` where the material has no such tone, and only colours the sources hold
    let table: Vec<Vec<String>> = md.lines().skip_while(|l| !l.starts_with("| material")).skip(2).take_while(|l| l.starts_with('|')).map(|l| l.trim_matches('|').split('|').map(|c| c.trim().to_string()).collect()).collect();
    assert!(table.len() >= 4, "{:?}", table);
    let source: Vec<String> = ["#901820", "#e02828", "#ff7860", "#c08010", "#f8c828", "#b05820", "#f09030", "#ffc870", "#106070", "#209898", "#5888f8", "#88b8f8", "#a8d0f8", "#287030", "#48a838", "#88d858"].iter().map(|s| s.to_string()).collect();
    for row in &table {
        let colours: Vec<&String> = row[1..4].iter().filter(|c| c.starts_with('#')).collect();
        let mut seen: Vec<&String> = Vec::new();
        for c in &colours {
            assert!(!seen.contains(c), "a ramp repeats a colour: {:?}", row);
            seen.push(c);
            assert!(source.contains(c), "{} is not a source colour: {:?}", c, row);
        }
        assert!(colours.len() >= 2 || row[0] == "grey", "{:?}", row);
    }
    let teal = table.iter().find(|r| r[0] == "teal").expect("teal");
    assert_eq!((teal[1].as_str(), teal[2].as_str(), teal[3].as_str()), ("#106070", "-", "#209898"), "{:?}", teal);
    let orange = table.iter().find(|r| r[0] == "orange").expect("orange");
    assert!(orange[1] != orange[2] && orange[2] != orange[3] && orange[1] != orange[3], "{:?}", orange);
    assert!(read(&p.join("style.toml")).contains("min_white"));
}

// ------------------------------------------------------------------------------------------ 8. --ref legend

#[test]
fn the_ref_legend_gives_one_colour_one_letter_in_every_frame() {
    let p = example_project("m10_legend");
    let legend = |frame: &str| -> std::collections::BTreeMap<String, String> {
        let t = ok(&pxl_in(&p, &["show", "hero", frame, "--ref"]));
        let l = t.lines().find(|l| l.starts_with("legend:")).unwrap().to_string();
        l.trim_start_matches("legend:").split_whitespace().filter_map(|w| w.split_once('=')).map(|(k, v)| (k.to_string(), v.chars().take(7).collect::<String>())).collect()
    };
    let (a, b, c) = (legend("hero_01"), legend("hero_02"), legend("hero_03"));
    for (x, y) in [(&a, &b), (&a, &c), (&b, &c)] {
        for (letter, hex) in x {
            if let Some(other) = y.get(letter) {
                assert_eq!(hex, other, "letter {} is {} in one frame and {} in another", letter, hex, other);
            }
        }
        let inv = |m: &std::collections::BTreeMap<String, String>| -> std::collections::BTreeMap<String, String> { m.iter().map(|(k, v)| (v.clone(), k.clone())).collect() };
        for (hex, letter) in inv(x) {
            if let Some(other) = inv(y).get(&hex) {
                assert_eq!(&letter, other, "{} has two letters", hex);
            }
        }
    }
}

// ------------------------------------------------------------------------------------------ eye detection on real-art shapes

fn put(im: &mut RgbaImage, x: usize, y: usize, c: (u8, u8, u8)) {
    im.px[y * im.w + x] = Rgba::opaque(c.0, c.1, c.2);
}

#[test]
fn a_lone_highlight_beside_an_outline_is_not_an_eye_but_white_iris_and_pupil_is() {
    let grey = (146, 146, 146);
    let (white, black, yellow) = ((255, 255, 255), (0, 0, 0), (255, 255, 0));
    // factor 2, 24x24 source: a grey head with a 2x2 white highlight and a black bar beside it
    let mut a = RgbaImage::new(24, 24);
    for y in 4..22 {
        for x in 4..20 {
            put(&mut a, x, y, grey);
        }
    }
    for y in 10..12 {
        for x in 8..10 {
            put(&mut a, x, y, white);
        }
        put(&mut a, 7, y, black);
        put(&mut a, 6, y, black);
    }
    for x in 4..20 {
        put(&mut a, x, 21, black);
    }
    let geo = Geo { org: (0, 0), f: 2 };
    assert!(detect(&a, geo).is_empty(), "a highlight beside the outline is no eye");
    // the same head with one white pixel, a yellow iris and a black pupil: an eye
    let mut b = RgbaImage::new(24, 24);
    for y in 4..22 {
        for x in 4..20 {
            put(&mut b, x, y, grey);
        }
    }
    put(&mut b, 10, 10, white);
    put(&mut b, 11, 10, yellow);
    put(&mut b, 10, 11, yellow);
    put(&mut b, 11, 11, yellow);
    put(&mut b, 12, 10, black);
    put(&mut b, 12, 11, black);
    put(&mut b, 12, 12, black);
    assert_eq!(detect(&b, geo).len(), 1, "white + yellow iris + black pupil is an eye");
    // a diagonal sparkle with a dark spot is not
    let mut c = RgbaImage::new(24, 24);
    for y in 4..22 {
        for x in 4..20 {
            put(&mut c, x, y, (230, 130, 20));
        }
    }
    for k in 0..4 {
        put(&mut c, 8 + k, 8 + k, white);
    }
    put(&mut c, 12, 8, black);
    put(&mut c, 13, 8, black);
    assert!(detect(&c, geo).is_empty(), "a diagonal sparkle is no eye");
}
