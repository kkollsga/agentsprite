//! M6 R4: single-image mode, project mode, assets and versions, ingest, groups, tasks, board, decisions, exceptions, variants,
//! manifest commands, reviewer workflow, silent-zero lint rules, export manifest, `migrate --layout assets`.
mod e2e_common;

use agentsprite::{asset, manedit, png};
use e2e_common::*;
use std::path::{Path, PathBuf};

fn pic(path: &Path, seed: u64) {
    png::write_rgba(path, &upscale(&sprite_grid(16, 16, seed), 4)).unwrap();
}

fn ok(o: &std::process::Output) -> String {
    assert!(o.status.success(), "{}{}", stdout(o), stderr(o));
    stdout(o)
}

fn project(tag: &str) -> PathBuf {
    let d = tmpdir(tag);
    let p = d.join("proj");
    ok(&pxl_in(&d, &["new", "proj", "--console", "gbc", "--name", "Demo", "--no-gates"]));
    p
}

fn text(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap()
}

// ------------------------------------------------------------------------------------------ single image

#[test]
fn image_makes_a_self_contained_asset_folder() {
    let d = tmpdir("r4_image");
    pic(&d.join("blob.png"), 3);
    let o = pxl_in(&d, &["image", "blob.png", "--size", "16x16", "--console", "gbc"]);
    let t = ok(&o);
    assert!(t.contains("created") && t.contains("stored blob v001"), "{}", t);
    let a = d.join("blob.pxl");
    for f in ["asset.pxl", "input/blob.png", "versions/blob_v001.pxl", "versions/blob_v001.png", "compare.png", "template.toml"] {
        assert!(a.join(f).is_file(), "missing {}", f);
    }
    let head = text(&a.join("asset.pxl"));
    assert!(head.contains("console = \"gbc\""), "the settings live in the asset header: {}", head);
    // every command takes the folder, asset.pxl, or (inside a project) the name
    assert!(ok(&pxl_in(&d, &["lint", "blob.pxl"])).contains("gbc/sprite"));
    assert!(ok(&pxl_in(&d, &["lint", "blob.pxl/asset.pxl"])).contains("0 error"));
    ok(&pxl_in(&d, &["export", "blob.pxl"]));
    assert!(a.join("export/blob.png").is_file() && a.join("export/blob.aseprite").is_file(), "export beside the asset");
    // the same picture again changes nothing
    let t = ok(&pxl_in(&d, &["image", "blob.png", "--size", "16x16"]));
    assert!(t.contains("unchanged"), "{}", t);
    assert_eq!(ok(&pxl_in(&d, &["versions", "blob.pxl"])).matches("  v0").count(), 1);
}

#[test]
fn every_store_makes_a_version_and_refreshes_compare() {
    let d = tmpdir("r4_versions");
    pic(&d.join("blob.png"), 3);
    ok(&pxl_in(&d, &["image", "blob.png", "--size", "16x16"]));
    let a = d.join("blob.pxl");
    let cmp0 = std::fs::read(a.join("compare.png")).unwrap();
    let v1 = text(&a.join("versions/blob_v001.pxl"));
    let o = pxl_in(&d, &["set", "blob.pxl", "blob", "3,3=1", "4,4=1"]);
    assert!(ok(&o).contains("stored blob v002"));
    assert!(a.join("versions/blob_v002.png").is_file());
    assert_ne!(std::fs::read(a.join("compare.png")).unwrap(), cmp0, "compare.png is regenerated");
    // writing the same text again stores nothing
    ok(&pxl_in(&d, &["set", "blob.pxl", "blob", "3,3=1"]).clone());
    assert_eq!(ok(&pxl_in(&d, &["versions", "blob.pxl"])).matches("  v0").count(), 2);
    // --no-version
    let o = pxl_in(&d, &["--no-version", "set", "blob.pxl", "blob", "5,3=1"]);
    assert!(!stdout(&o).contains("stored"), "{}", stdout(&o));
    assert_eq!(ok(&pxl_in(&d, &["versions", "blob.pxl"])).matches("  v0").count(), 2);
    // revert restores v001 as a new version
    let t = ok(&pxl_in(&d, &["revert", "blob.pxl", "v001"]));
    assert!(t.contains("restored from v001 as v003"), "{}", t);
    assert_eq!(text(&a.join("asset.pxl")), v1);
    assert_eq!(text(&a.join("versions/blob_v003.pxl")), v1);
    assert!(!pxl_in(&d, &["revert", "blob.pxl", "v099"]).status.success());
    // undo is a store too
    ok(&pxl_in(&d, &["set", "blob.pxl", "blob", "6,6=1"]));
    assert!(ok(&pxl_in(&d, &["undo", "blob.pxl"])).contains("undid 1"));
}

#[test]
fn import_makes_a_new_version() {
    let d = tmpdir("r4_import");
    pic(&d.join("blob.png"), 3);
    ok(&pxl_in(&d, &["image", "blob.png", "--size", "16x16"]));
    let a = d.join("blob.pxl");
    ok(&pxl_in(&d, &["export", "blob.pxl"]));
    ok(&pxl_in(&d, &["set", "blob.pxl", "blob", "3,3=1"]));
    let before = ok(&pxl_in(&d, &["versions", "blob.pxl"])).matches("  v0").count();
    let o = pxl_in(&d, &["import", "blob.pxl", a.join("export/blob.png").to_str().unwrap()]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    assert_eq!(ok(&pxl_in(&d, &["versions", "blob.pxl"])).matches("  v0").count(), before + 1, "the import is a new version: {}", stdout(&o));
}

// ------------------------------------------------------------------------------------------ project

#[test]
fn new_creates_the_layout_and_project_show_merges_settings() {
    let p = project("r4_new");
    for d in ["inbox", "assets", "palettes", "consoles", "templates", "export", "review"] {
        assert!(p.join(d).is_dir(), "{}", d);
    }
    let m = text(&p.join("pxl.toml"));
    assert!(m.contains("[project]") && m.contains("console = \"gbc\"") && m.contains("[project.defaults]") && m.contains("[project.settings]"), "{}", m);
    let mut m2 = m.clone();
    m2 = m2.replace("mode = \"sprite\"", "mode = \"sprite\"\nfactor = 4\ncategory = \"enemy\"");
    m2.push_str("zone_style = \"meadow\"\nreviewers = [\"anna\"]\n");
    std::fs::write(p.join("pxl.toml"), m2).unwrap();
    let t = ok(&pxl_in(&p, &["project", "show"]));
    assert!(t.contains("project Demo") && t.contains("console gbc") && t.contains("factor 4") && t.contains("category enemy") && t.contains("zone_style = \"meadow\"") && t.contains("reviewers"), "{}", t);
    let j = json_of(&pxl_in(&p, &["project", "show", "--json"]));
    assert_eq!(j["settings"]["zone_style"], "meadow");
    assert_eq!(j["defaults"]["factor"], 4);
    assert!(!pxl_in(&p, &["new", ".", "--console", "gbc"]).status.success(), "no second project in the same folder");
    assert!(!pxl_in(&p, &["new", "x2", "--console", "zx81"]).status.success(), "unknown console");
}

#[test]
fn ingest_creates_assets_and_a_changed_input_adds_versions() {
    let p = project("r4_ingest");
    pic(&p.join("inbox/blob.png"), 3);
    pic(&p.join("inbox/orb.png"), 5);
    let t = ok(&pxl_in(&p, &["ingest", "--group", "enemies", "--size", "16x16"]));
    assert!(t.contains("ingested 2 of 2"), "{}", t);
    assert!(!p.join("inbox/blob.png").exists(), "ingested files move out of inbox/");
    let a = p.join("assets/enemies/blob");
    assert!(a.join("input/blob.png").is_file() && a.join("asset.pxl").is_file() && a.join("versions/blob_v001.png").is_file());
    let m = text(&p.join("pxl.toml"));
    assert!(m.contains("[parts.blob]") && m.contains("path = \"assets/enemies/blob/asset.pxl\"") && m.contains("out = \"enemies/blob\""), "{}", m);
    // the project export was rebuilt by the store
    assert!(p.join("export/enemies/blob/blob.png").is_file());
    assert!(ok(&pxl_in(&p, &["check"])).contains("PASS"));
    // the same file again: unchanged
    std::fs::copy(a.join("input/blob.png"), p.join("inbox/blob.png")).unwrap();
    assert!(ok(&pxl_in(&p, &["ingest"])).contains("unchanged"));
    // a hand edit, then a changed input: the edited frame keeps its art, there is a new input and a new version
    ok(&pxl_in(&p, &["set", "blob", "blob", "3,3=1"]));
    let art_before = text(&a.join("asset.pxl"));
    pic(&p.join("inbox/blob.png"), 8);
    let t = ok(&pxl_in(&p, &["ingest"]));
    assert!(t.contains("updated"), "{}", t);
    assert!(a.join("input/history/blob_i001.png").is_file(), "the old input is kept");
    assert_eq!(text(&a.join("asset.pxl")), art_before, "hand-edited art survives a re-ingest");
    assert!(ok(&pxl_in(&p, &["versions", "blob"])).matches("  v0").count() >= 2);
    assert!(std::fs::read(a.join("input/blob.png")).unwrap() != std::fs::read(a.join("input/history/blob_i001.png")).unwrap());
}

#[test]
fn ingest_splits_sheets_and_folders_of_frames() {
    let p = project("r4_sheets");
    // a 2x2 sheet of four 64x64 pictures
    let mut sheet = agentsprite::img::RgbaImage::new(128, 128);
    for i in 0..4 {
        let im = upscale(&sprite_grid(16, 16, i as u64 + 1), 4);
        sheet.blit(&im, (i % 2) as i32 * 64, (i / 2) as i32 * 64);
    }
    std::fs::create_dir_all(p.join("in")).unwrap();
    png::write_rgba(&p.join("in/walk.png"), &sheet).unwrap();
    let t = ok(&pxl_in(&p, &["ingest", "in/walk.png", "--grid", "64x64", "--size", "16x16"]));
    assert!(t.contains("created"), "{}", t);
    let part = agentsprite::pxl::load(&p.join("assets/walk/asset.pxl")).unwrap();
    assert_eq!(part.frames.len(), 4, "grid split");
    assert_eq!(part.frames[0].name, "walk_00");
    std::fs::remove_dir_all(p.join("assets/walk")).unwrap();
    std::fs::write(p.join("pxl.toml"), text(&p.join("pxl.toml")).replace("[parts.walk]\npath = \"assets/walk/asset.pxl\"\nout = \"walk\"\n", "")).unwrap();
    let t = ok(&pxl_in(&p, &["ingest", "in/walk.png", "--frames", "auto", "--size", "16x16"]));
    assert!(t.contains("created"), "{}", t);
    assert_eq!(agentsprite::pxl::load(&p.join("assets/walk/asset.pxl")).unwrap().frames.len(), 4, "connected regions");
    // a folder of frames is one asset
    std::fs::create_dir_all(p.join("in/hero")).unwrap();
    for i in 0..3 {
        pic(&p.join(format!("in/hero/hero_{:02}.png", i)), i + 1);
    }
    ok(&pxl_in(&p, &["ingest", "in/hero", "--name-from", "folder", "--size", "16x16"]));
    let part = agentsprite::pxl::load(&p.join("assets/hero/asset.pxl")).unwrap();
    assert_eq!(part.frames.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["hero_00", "hero_01", "hero_02"]);
}

#[test]
fn groups_move_assets_and_keep_pxl_toml_in_sync() {
    let p = project("r4_groups");
    pic(&p.join("inbox/blob.png"), 3);
    ok(&pxl_in(&p, &["ingest", "--group", "enemies", "--size", "16x16"]));
    ok(&pxl_in(&p, &["group", "add", "items"]));
    let t = ok(&pxl_in(&p, &["group", "list"]));
    assert!(t.contains("enemies") && t.contains("blob") && t.contains("items"), "{}", t);
    ok(&pxl_in(&p, &["group", "move", "blob", "items/misc"]));
    assert!(p.join("assets/items/misc/blob/asset.pxl").is_file() && !p.join("assets/enemies/blob").exists());
    let m = text(&p.join("pxl.toml"));
    assert!(m.contains("path = \"assets/items/misc/blob/asset.pxl\"") && m.contains("out = \"items/misc/blob\"") && m.contains("tags = [\"items\"]"), "{}", m);
    assert!(ok(&pxl_in(&p, &["check"])).contains("PASS"), "export moved with it");
    let t = ok(&pxl_in(&p, &["tree"]));
    assert!(t.contains("items/misc/") && t.contains("blob") && t.contains("v00"), "{}", t);
    // pxl build moves outputs no part produces any more
    assert!(p.join("export/enemies/blob/blob.png").is_file(), "the old place still holds the old output");
    ok(&pxl_in(&p, &["build"]));
    ok(&pxl_in(&p, &["build"]));
    assert!(p.join("export/items/misc/blob/blob.png").is_file());
}

// ------------------------------------------------------------------------------------------ work lists

#[test]
fn tasks_have_stable_ids_and_next_prefers_assigned_work() {
    let p = project("r4_tasks");
    pic(&p.join("inbox/blob.png"), 3);
    pic(&p.join("inbox/orb.png"), 5);
    ok(&pxl_in(&p, &["ingest", "--size", "16x16"]));
    let t = ok(&pxl_in(&p, &["task", "add", "orb", "fix the eye", "--kind", "fix", "--assign", "bob", "--priority", "2"]));
    assert!(t.contains("t0001"), "{}", t);
    ok(&pxl_in(&p, &["task", "add", "blob", "draw the tail"]));
    assert!(text(&p.join("tasks.toml")).contains("id = \"t0002\""));
    let t = ok(&pxl_in(&p, &["tasks"]));
    assert!(t.contains("t0001") && t.contains("t0002") && t.contains("2 task(s)"), "{}", t);
    let t = ok(&pxl_in(&p, &["tasks", "--mine", "--agent", "bob"]));
    assert!(t.contains("t0001") && !t.contains("t0002"), "{}", t);
    assert!(!pxl_in(&p, &["tasks", "--mine"]).status.success(), "--mine needs an identity");
    assert!(ok(&pxl_in(&p, &["tasks", "--assign", "bob"])).contains("1 task(s)"));
    assert!(!pxl_in(&p, &["task", "add", "orb", "x", "--kind", "bogus"]).status.success());
    // next: orb (open task for bob) before blob, although blob sorts first
    let t = ok(&pxl_in(&p, &["next", "--agent", "bob"]));
    assert!(t.contains("orb") && t.contains("open tasks") && t.contains("fix the eye"), "{}", t);
    ok(&pxl_in(&p, &["release", "orb", "--agent", "bob"]));
    ok(&pxl_in(&p, &["task", "done", "t0001", "--note", "fixed in v003"]));
    assert!(ok(&pxl_in(&p, &["task", "done", "t0001"])).contains("already done"));
    let t = ok(&pxl_in(&p, &["tasks"]));
    assert!(!t.contains("t0001") && t.contains("t0002"), "{}", t);
    assert!(ok(&pxl_in(&p, &["tasks", "--all"])).contains("t0001"));
    // ids are never reused
    ok(&pxl_in(&p, &["task", "add", "orb", "again"]));
    assert!(text(&p.join("tasks.toml")).contains("id = \"t0003\""));
    // rework creates a fix task
    ok(&pxl_in(&p, &["rework", "blob", "--note", "outline too thin", "--agent", "boss"]));
    assert!(ok(&pxl_in(&p, &["tasks"])).contains("outline too thin"));
}

#[test]
fn board_decisions_exceptions_and_brief() {
    let p = project("r4_board");
    pic(&p.join("inbox/blob.png"), 3);
    ok(&pxl_in(&p, &["ingest", "--group", "enemies", "--size", "16x16"]));
    ok(&pxl_in(&p, &["task", "add", "blob", "make the tail longer", "--assign", "ann"]));
    ok(&pxl_in(&p, &["decide", "the outline stays navy", "--asset", "blob", "--by", "user"]));
    ok(&pxl_in(&p, &["decide", "all enemies face left"]));
    let dec = text(&p.join("decisions.md"));
    assert!(dec.contains("[user] (asset blob) the outline stays navy") && dec.contains("all enemies face left"), "{}", dec);
    ok(&pxl_in(&p, &["exception", "add", "blob", "width 24 -> 32", "--reason", "the tail would be cut"]));
    assert!(!pxl_in(&p, &["exception", "add", "blob", "x", "--reason", " "]).status.success());
    assert!(ok(&pxl_in(&p, &["exception", "list"])).contains("width 24 -> 32"));
    assert!(text(&p.join("assets/enemies/blob/exceptions.md")).contains("(reason: the tail would be cut)"));
    let t = ok(&pxl_in(&p, &["board", "--html", "board.html"]));
    assert!(t.contains("enemies: 1 asset(s)") && t.contains("make the tail longer") && t.contains("the outline stays navy"), "{}", t);
    let h = text(&p.join("board.html"));
    assert!(h.contains("<table>") && h.contains("make the tail longer") && h.contains("outline stays navy") && !h.contains("<script"), "static html");
    let page = text(&p.join("assets/enemies/blob/compare.html"));
    assert!(page.contains("compare.png") && page.contains("outline stays navy") && page.contains("make the tail longer") && page.contains("width 24"), "{}", page);
    let b = ok(&pxl_in(&p, &["brief", "blob"]));
    for want in ["BRIEF blob", "outline stays navy", "all enemies face left", "make the tail longer", "width 24 -> 32", "compare", "review history", "budgets"] {
        assert!(b.contains(want), "missing {:?} in\n{}", want, b);
    }
    let g = ok(&pxl_in(&p, &["brief", "enemies"]));
    assert!(g.contains("blob state"), "{}", g);
    assert!(!pxl_in(&p, &["brief", "nothing_here"]).status.success());
}

// ------------------------------------------------------------------------------------------ variants

#[test]
fn variants_have_exactly_one_delivered() {
    let p = project("r4_variants");
    pic(&p.join("inbox/blob.png"), 3);
    ok(&pxl_in(&p, &["ingest", "--group", "items", "--size", "16x16"]));
    let a = p.join("assets/items/blob");
    ok(&pxl_in(&p, &["variant", "add", "blob", "readable"]));
    assert!(a.join("asset.readable.pxl").is_file() && a.join("variants.toml").is_file());
    ok(&pxl_in(&p, &["set", a.join("asset.readable.pxl").to_str().unwrap(), "blob", "4,4=1", "5,4=1"]));
    assert!(a.join("variants.png").is_file(), "compare shows the variants side by side");
    let t = ok(&pxl_in(&p, &["variant", "list", "blob"]));
    assert!(t.contains("main  (delivered)") && t.contains("readable"), "{}", t);
    let t = ok(&pxl_in(&p, &["variant", "deliver", "blob", "readable"]));
    assert!(t.contains("readable is delivered"), "{}", t);
    let m = text(&p.join("pxl.toml"));
    assert!(m.contains("path = \"assets/items/blob/asset.readable.pxl\""), "the delivery path now builds the delivered variant: {}", m);
    assert!(m.contains("[parts.blob__main]") && m.contains("out = \"variants/blob/main\""), "{}", m);
    assert!(p.join("export/variants/blob/main/blob.png").is_file() && p.join("export/items/blob/blob.png").is_file());
    assert!(text(&a.join("asset.pxl")).contains("budget_exempt"), "non-delivered variants are exempt from the scene budgets");
    assert!(!text(&a.join("asset.readable.pxl")).contains("budget_exempt"));
    assert!(text(&a.join("variants.toml")).contains("delivered = \"readable\""));
    // switching back is one command
    ok(&pxl_in(&p, &["variant", "deliver", "blob", "main"]));
    assert!(text(&p.join("pxl.toml")).contains("path = \"assets/items/blob/asset.pxl\""));
    assert!(!pxl_in(&p, &["variant", "deliver", "blob", "nope"]).status.success());
    assert!(ok(&pxl_in(&p, &["check"])).contains("PASS"));
}

// ------------------------------------------------------------------------------------------ review workflow

#[test]
fn reviewer_notes_carry_regions_and_create_fix_tasks() {
    let p = project("r4_review");
    pic(&p.join("inbox/blob.png"), 3);
    ok(&pxl_in(&p, &["ingest", "--size", "16x16"]));
    let a = p.join("assets/blob");
    // a colour claim without a region is rejected
    let o = pxl_in(&p, &["review", "note", "blob", "blob", "the source is red here", "--agent", "rev"]);
    assert!(!o.status.success() && stderr(&o).contains("--region"), "{}", stderr(&o));
    let o = pxl_in(&p, &["review", "note", "blob", "blob", "the outline is broken", "--region", "3,3,8,8", "--agent", "rev"]);
    let t = ok(&o);
    assert!(t.contains("fix task t0001"), "{}", t);
    assert!(a.join("review/note_001.md").is_file() && a.join("review/note_001.png").is_file(), "note and crop image");
    let im = png::read_rgba(&a.join("review/note_001.png")).unwrap();
    assert!(im.w > im.h, "source | ours side by side");
    assert!(ok(&pxl_in(&p, &["tasks"])).contains("the outline is broken"));
    // a note without colour or direction words needs no region
    ok(&pxl_in(&p, &["review", "note", "blob", "blob", "eyes missing", "--agent", "rev"]));
    assert!(!pxl_in(&p, &["review", "approve", "blob"]).status.success(), "the reviewer identity is required");
    let t = ok(&pxl_in(&p, &["review", "next"]));
    assert!(t.contains("nothing to review"), "{}", t);
    ok(&pxl_in(&p, &["release", "blob", "--state", "done", "--agent", "ann"]));
    assert!(ok(&pxl_in(&p, &["review", "next"])).contains("blob"));
    ok(&pxl_in(&p, &["review", "approve", "blob", "--agent", "rev"]));
    assert!(ok(&pxl_in(&p, &["brief", "blob"])).contains("rev approved"));
    ok(&pxl_in(&p, &["review", "rework", "blob", "--note", "eyes still missing", "--agent", "rev"]));
    assert!(ok(&pxl_in(&p, &["tasks"])).contains("eyes still missing"));
}

#[test]
fn user_approved_assets_need_the_force_flag_and_a_review_task() {
    let p = project("r4_user");
    pic(&p.join("inbox/blob.png"), 3);
    ok(&pxl_in(&p, &["ingest", "--size", "16x16"]));
    ok(&pxl_in(&p, &["review", "approve", "blob", "--agent", "user"]));
    assert!(ok(&pxl_in(&p, &["tree"])).contains("approved by user"));
    let o = pxl_in(&p, &["set", "blob", "blob", "3,3=1"]);
    assert!(!o.status.success() && stderr(&o).contains("approved by the user"), "{}", stderr(&o));
    // even the ordinary --force-approved is not enough
    let o = pxl_in(&p, &["set", "blob", "blob", "3,3=1", "--force-approved"]);
    assert!(!o.status.success());
    ok(&pxl_in(&p, &["--force-user-approved", "set", "blob", "blob", "3,3=1", "--force-approved"]));
    let t = ok(&pxl_in(&p, &["tasks"]));
    assert!(t.contains("[review]") && t.contains("changed after user approval"), "{}", t);
    ok(&pxl_in(&p, &["--force-user-approved", "set", "blob", "blob", "4,4=1", "--force-approved"]));
    assert_eq!(ok(&pxl_in(&p, &["tasks"])).matches("changed after user approval").count(), 1, "one task, not one per store");
}

// ------------------------------------------------------------------------------------------ manifest commands

#[test]
fn manifest_edits_are_text_edits_that_keep_comments() {
    let m = "# head\nout_root = \"export\"\n\n[parts.a]\npath = \"a.pxl\"\n[parts.b]\npath = \"b.pxl\"\n\n[scene.s1]\npalette_set = \"p1\"\n# note\nresident_budget = 10\n[scene.s1.groups.g]\nbudget = 4\nparts = [\"a\"]\n[scene.s1.groups.h]\nbudget = 4\nparts = [\"a\", \"b\"]\n";
    let t = manedit::set_table_key(m, "scene.s1", "resident_budget", "12").unwrap();
    assert!(t.contains("resident_budget = 12") && t.contains("# note") && t.contains("# head"));
    let t = manedit::set_table_key(m, "scene.s1", "tileset", "\"s1_blocks\"").unwrap();
    assert!(t.contains("tileset = \"s1_blocks\"\n[scene.s1.groups.g]"), "{}", t);
    assert!(manedit::set_table_key(m, "scene.nope", "x", "1").is_none());
    let t = manedit::edit_parts(m, "s1", Some("g"), "b", true).unwrap();
    assert!(t.contains("parts = [\"a\", \"b\"]\n[scene.s1.groups.h]"), "{}", t);
    let t = manedit::edit_parts(m, "s1", None, "a", false).unwrap();
    assert!(!t.contains("parts = [\"a\"]") && t.contains("parts = [\"b\"]") && t.contains("parts = []"), "removed from every group: {}", t);
    assert!(manedit::edit_parts(m, "s1", Some("zzz"), "a", true).unwrap_err().contains("no group"));
    assert!(manedit::edit_parts(m, "nope", None, "a", true).unwrap_err().contains("no scene"));
    let pal = "include = \"global\"\n\n[[slots]]\nslot = 4\nname = \"x\"\n# comment\ncolors = [\"#000000\", \"#111111\", \"#222222\"]\n";
    let t = manedit::set_slot_text(pal, 4, &["#101010".into(), "#202020".into(), "#303030".into()]).unwrap();
    assert!(t.contains("colors = [\"#101010\", \"#202020\", \"#303030\"]") && t.contains("# comment"), "{}", t);
    let t = manedit::set_slot_text(pal, 5, &["#101010".into(), "#202020".into(), "#303030".into()]).unwrap();
    assert!(t.contains("slot = 5") && t.contains("name = \"slot_5\""), "{}", t);
    assert!(manedit::set_slot_text(pal, 4, &["#101010".into()]).is_err());
    assert!(manedit::set_slot_text(pal, 4, &["red".into(), "#202020".into(), "#303030".into()]).is_err());
}

#[test]
fn scene_and_palette_commands_edit_the_files() {
    let p = project("r4_manifest");
    pic(&p.join("inbox/blob.png"), 3);
    ok(&pxl_in(&p, &["ingest", "--size", "16x16"]));
    let mut m = text(&p.join("pxl.toml"));
    m.push_str("\n[scene.s1]\npalette_set = \"p1\"\n[scene.s1.groups.g]\nbudget = 4\nparts = []\n");
    std::fs::write(p.join("pxl.toml"), m).unwrap();
    std::fs::write(p.join("palettes/p1.toml"), "[[slots]]\nslot = 0\nname = \"a\"\ncolors = [\"#080808\", \"#404040\", \"#f8f8f8\"]\n").unwrap();
    ok(&pxl_in(&p, &["scene", "add-part", "s1", "g", "blob"]));
    assert!(text(&p.join("pxl.toml")).contains("parts = [\"blob\"]"));
    ok(&pxl_in(&p, &["scene", "set", "s1", "resident_budget", "9"]));
    assert!(text(&p.join("pxl.toml")).contains("resident_budget = 9"));
    assert!(!pxl_in(&p, &["scene", "set", "s1", "bogus", "1"]).status.success());
    assert!(!pxl_in(&p, &["scene", "add-part", "s1", "g", "ghost"]).status.success(), "unknown part");
    ok(&pxl_in(&p, &["scene", "remove-part", "s1", "blob"]));
    assert!(text(&p.join("pxl.toml")).contains("parts = []"));
    ok(&pxl_in(&p, &["palette", "slot", "set", "p1", "0", "#101010", "#808080", "#e0e0e0"]));
    assert!(text(&p.join("palettes/p1.toml")).contains("#808080"));
    // a held lease on pxl.toml refuses another agent
    ok(&pxl_in(&p, &["--agent", "a1", "scene", "set", "s1", "resident_budget", "8"]));
    assert!(p.join("pxl.toml").is_file());
}

// ------------------------------------------------------------------------------------------ silent zeros

const TILE_TPL: &str = "name = \"t\"\n[layers.base]\n\"a\" = { role = \"air\", color = \"#88c8f8\" }\n\"b\" = { role = \"c_b\", color = \"#f8f8f8\" }\n\"c\" = { role = \"c_c\", color = \"#183058\" }\n\"d\" = { role = \"c_d\", color = \"#28a048\" }\n[tiles]\nair = [\"a\"]\n";

fn tile_part(name: &str, frames: &[&str], extra: &str) -> String {
    let mut s = format!("name = \"{}\"\nmode = \"tile\"\nsize = [16, 8]\norigin = [0, 0]\ntile = [8, 8]\ntemplate = \"templates/t.toml\"\n{}\n", name, extra);
    for f in frames {
        s.push_str(&format!("[[frames]]\nname = \"{}\"\ngrid = \"\"\"\n{}\"\"\"\n", f, "abcdabcdabcdabcd\n".repeat(8).replace("abcdabcdabcdabcd", "abababababababab")));
    }
    s
}

#[test]
fn lint_refuses_animated_frames_that_form_no_set() {
    let d = tmpdir("r4_anim");
    std::fs::create_dir_all(d.join("templates")).unwrap();
    std::fs::write(d.join("templates/t.toml"), TILE_TPL).unwrap();
    std::fs::write(d.join("water.pxl"), tile_part("water", &["water", "water2"], "")).unwrap();
    std::fs::write(d.join("zone.pxl"), tile_part("zone", &["blocks"], "animated = [\"water.pxl\"]\n")).unwrap();
    let o = pxl_in(&d, &["lint", "zone.pxl", "--rules", "animated_set"]);
    assert!(!o.status.success(), "{}", stdout(&o));
    assert!(stdout(&o).contains("animated_set") && stdout(&o).contains("no _NN suffix") && stdout(&o).contains("count 0 slots"), "{}", stdout(&o));
    std::fs::write(d.join("water.pxl"), tile_part("water", &["water_00", "water_01"], "")).unwrap();
    assert!(pxl_in(&d, &["lint", "zone.pxl", "--rules", "animated_set"]).status.success(), "an _NN set passes");
}

#[test]
fn an_unreferenced_tile_set_is_an_error_in_a_strict_project() {
    let d = tmpdir("r4_unref");
    std::fs::create_dir_all(d.join("templates")).unwrap();
    std::fs::create_dir_all(d.join("parts")).unwrap();
    std::fs::write(d.join("templates/t.toml"), TILE_TPL).unwrap();
    std::fs::write(d.join("parts/lost.pxl"), tile_part("lost", &["blocks"], "")).unwrap();
    std::fs::write(d.join("parts/used.pxl"), tile_part("used", &["blocks"], "")).unwrap();
    let manifest = |strict: &str| format!("[project]\nconsole = \"gbc\"\n{}\n[parts.lost]\npath = \"parts/lost.pxl\"\n[parts.used]\npath = \"parts/used.pxl\"\n[scene.s1]\npalette_set = \"x\"\ntileset = \"used\"\n", strict);
    std::fs::write(d.join("pxl.toml"), manifest("strict = true")).unwrap();
    let o = pxl_in(&d, &["lint", "parts/lost.pxl", "--rules", "tile_unreferenced"]);
    assert!(!o.status.success() && stdout(&o).contains("referenced by no scene"), "{}{}", stdout(&o), stderr(&o));
    assert!(pxl_in(&d, &["lint", "parts/used.pxl", "--rules", "tile_unreferenced"]).status.success(), "the scene's tileset is referenced");
    // a project without strict reports a warning only
    std::fs::write(d.join("pxl.toml"), manifest("")).unwrap();
    let o = pxl_in(&d, &["lint", "parts/lost.pxl", "--rules", "tile_unreferenced"]);
    assert!(o.status.success() && stdout(&o).contains("1 warning"), "{}", stdout(&o));
}

#[test]
fn check_says_how_many_it_checked() {
    let p = project("r4_checked");
    pic(&p.join("inbox/blob.png"), 3);
    ok(&pxl_in(&p, &["ingest", "--size", "16x16"]));
    let t = ok(&pxl_in(&p, &["check"]));
    assert!(t.contains("checked 1 of 1 part(s), 0 of 0 scene(s)"), "{}", t);
    assert_eq!(json_of(&pxl_in(&p, &["check", "--json"]))["checked"]["parts"], 1);
}

// ------------------------------------------------------------------------------------------ export manifest

#[test]
fn build_moves_outputs_no_part_produces_any_more() {
    let p = project("r4_removed");
    pic(&p.join("inbox/blob.png"), 3);
    pic(&p.join("inbox/orb.png"), 5);
    ok(&pxl_in(&p, &["ingest", "--size", "16x16"]));
    ok(&pxl_in(&p, &["build"]));
    assert!(p.join("export/.export_manifest.json").is_file());
    assert!(p.join("export/orb/orb.png").is_file());
    // the part leaves the manifest
    let m = text(&p.join("pxl.toml"));
    std::fs::write(p.join("pxl.toml"), asset::remove_part_entry(&m, "orb")).unwrap();
    let t = ok(&pxl_in(&p, &["build"]));
    assert!(t.contains("removed orb/orb.png -> .removed/orb/orb.png"), "{}", t);
    assert!(!p.join("export/orb/orb.png").exists() && p.join("export/.removed/orb/orb.png").is_file(), "moved, never deleted");
    assert!(p.join("export/blob/blob.png").is_file());
}

// ------------------------------------------------------------------------------------------ scene delta

#[test]
fn a_store_prints_the_scene_delta() {
    let p = project("r4_delta");
    pic(&p.join("inbox/blob.png"), 3);
    ok(&pxl_in(&p, &["ingest", "--size", "16x16"]));
    let mut m = text(&p.join("pxl.toml"));
    m = m.replace("legacy = true\n", "");
    m.push_str("\n[scene.s1]\npalette_set = \"p1\"\nresident_budget = 100\n[scene.s1.groups.g]\nbudget = 1\nparts = [\"blob\"]\n");
    std::fs::write(p.join("pxl.toml"), m).unwrap();
    std::fs::write(p.join("palettes/p1.toml"), "[[slots]]\nslot = 0\nname = \"a\"\ncolors = [\"#080808\", \"#404040\", \"#f8f8f8\"]\n").unwrap();
    let o = pxl_in(&p, &["set", "blob", "blob", "3,3=1"]);
    let t = stdout(&o);
    assert!(t.contains("stored blob v00"), "{}", t);
    assert!(t.contains("s1 group g") || t.contains("s1 "), "the store names the scene: {}", t);
    assert!(t.contains("s1 group g"), "{}", t);
    // a part the cell model cannot count (no palette set) is a silent zero: a failing row
    let a = p.join("assets/blob/asset.pxl");
    let txt = std::fs::read_to_string(&a).unwrap().lines().filter(|l| !l.starts_with("palette_set")).collect::<Vec<_>>().join("\n") + "\n";
    std::fs::write(&a, txt).unwrap();
    let t = stdout(&pxl_in(&p, &["set", "blob", "blob", "6,6=1"]));
    assert!(t.contains("count g 0 / > 0 FAIL"), "a group that lists parts but counts 0 is a failure: {}", t);
    let j = json_of(&pxl_in(&p, &["set", "blob", "blob", "5,5=1", "--json"]));
    assert!(j["store"].as_array().map_or(false, |a| !a.is_empty()), "{}", j);
}

// ------------------------------------------------------------------------------------------ migrate --layout assets

#[test]
fn migrate_layout_assets_moves_parts_with_their_folders() {
    let d = tmpdir("r4_layout");
    std::fs::create_dir_all(d.join("templates")).unwrap();
    std::fs::write(d.join("templates/t.toml"), TEMPLATE).unwrap();
    let mut manifest = String::from("legacy = true\nout_root = \"out\"\n");
    for n in ["a", "b"] {
        let dir = d.join(format!("parts/g/{}", n));
        std::fs::create_dir_all(dir.join("ref")).unwrap();
        png::write_rgba(&dir.join("ref/f0.png"), &upscale(&sprite_grid(16, 16, 3), 2)).unwrap();
        let mut part = agentsprite::model::Part::new(n, (16, 16));
        part.template = Some("templates/t.toml".into());
        part.origin = (8, 8);
        part.input.images = Some("ref/{frame}.png".into());
        part.input.factor = Some(2);
        part.reduce.factor = Some(2);
        part.frames.push(agentsprite::model::Frame { name: "f0".into(), src: None, locked: false, grid: agentsprite::model::Grid::new(16, 16, b'.') });
        std::fs::write(dir.join(format!("{}.pxl", n)), agentsprite::pxl::to_string(&part)).unwrap();
        manifest.push_str(&format!("[parts.{}]\npath = \"parts/g/{}/{}.pxl\"\nout = \"{}\"\n", n, n, n, n));
    }
    // a third part shares its folder with a fourth: it must stay
    std::fs::create_dir_all(d.join("parts/shared")).unwrap();
    for n in ["c", "e"] {
        let mut part = agentsprite::model::Part::new(n, (8, 8));
        part.frames.push(agentsprite::model::Frame { name: "f0".into(), src: None, locked: false, grid: agentsprite::model::Grid::new(8, 8, b'.') });
        std::fs::write(d.join(format!("parts/shared/{}.pxl", n)), agentsprite::pxl::to_string(&part)).unwrap();
        manifest.push_str(&format!("[parts.{}]\npath = \"parts/shared/{}.pxl\"\nout = \"{}\"\n", n, n, n));
    }
    std::fs::write(d.join("pxl.toml"), manifest).unwrap();
    legacy(&d);
    ok(&pxl_in(&d, &["load", "parts/g/a/a.pxl"]).clone());
    let status_before = std::fs::read_to_string(d.join("parts/g/a/a.status.json")).is_ok();
    assert!(status_before);
    let t = ok(&pxl_in(&d, &["migrate", "--layout", "assets", "--dry-run"]));
    assert!(t.contains("parts/g/a/a.pxl -> assets/g/a/asset.pxl") && t.contains("c.pxl stays") && t.contains("dry run: 2 part(s) of 4 would move"), "{}", t);
    assert!(d.join("parts/g/a/a.pxl").is_file(), "a dry run moves nothing");
    let t = ok(&pxl_in(&d, &["migrate", "--layout", "assets"]));
    assert!(t.contains("migrated: 2 part(s) of 4 moved"), "{}", t);
    assert!(d.join("assets/g/a/asset.pxl").is_file() && d.join("assets/g/a/ref/f0.png").is_file() && d.join("assets/g/a/asset.status.json").is_file() && d.join("assets/g/a/asset.journal").is_file());
    assert!(!d.join("parts/g/a").exists() && d.join("parts/shared/c.pxl").is_file());
    let m = text(&d.join("pxl.toml"));
    assert!(m.contains("path = \"assets/g/a/asset.pxl\"") && m.contains("out = \"a\"") && m.contains("path = \"parts/shared/c.pxl\""), "{}", m);
    let o = pxl_in(&d, &["lint", "a"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    assert!(ok(&pxl_in(&d, &["status", "a"])).contains("todo") || stdout(&pxl_in(&d, &["status"])).contains("asset.pxl"));
    assert!(d.join("assets/g/a/versions/a_v001.pxl").is_file(), "a baseline version");
    // a second run has nothing left to move beyond the shared folder
    let t = ok(&pxl_in(&d, &["migrate", "--layout", "assets"]));
    assert!(t.contains("migrated: 0 part(s) of 2 moved"), "{}", t);
    // undo restores the old text byte for byte
    let o = pxl_in(&d, &["undo", "a"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
}

// ------------------------------------------------------------------------------------------ guide

#[test]
fn the_project_guide_names_real_commands() {
    let d = tmpdir("r4_guide");
    let o = pxl_in(&d, &["guide", "project"]);
    let t = stdout(&o);
    assert!(o.status.success() && t.starts_with("PROJECT GUIDE"), "{}", t);
    assert!(t.lines().count() <= 52, "{} lines", t.lines().count());
    assert!(!pxl_in(&d, &["guide", "nonsense"]).status.success());
    let mut seen = std::collections::BTreeSet::new();
    for seg in t.lines().flat_map(|l| l.split(" ; ").map(|s| s.trim().to_string()).collect::<Vec<_>>()) {
        if let Some(rest) = seg.strip_prefix("pxl ") {
            if let Some(w) = rest.split_whitespace().next() {
                seen.insert(w.trim_end_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-').to_string());
            }
        }
    }
    for w in ["image", "new", "ingest", "tree", "group", "project", "versions", "revert", "import", "variant", "task", "tasks", "next", "rework", "board", "brief", "decide", "exception", "review", "scene", "palette", "build", "check", "migrate"] {
        assert!(seen.contains(w), "the guide does not mention `pxl {}`", w);
    }
    for w in &seen {
        let o = pxl_in(&d, &[w.as_str(), "--help"]);
        assert!(o.status.success(), "`pxl {} --help` fails: {}", w, stderr(&o));
    }
}

// ------------------------------------------------------------------------------------------ R4b

#[test]
fn numbered_files_in_a_flat_inbox_become_one_asset() {
    use agentsprite::project::sequence_of;
    assert_eq!(sequence_of("walker_01"), Some(("walker".into(), 1, 2)));
    assert_eq!(sequence_of("walk1"), Some(("walk".into(), 1, 1)));
    assert_eq!(sequence_of("walk 2"), Some(("walk".into(), 2, 1)));
    assert_eq!(sequence_of("walk-03"), Some(("walk".into(), 3, 2)));
    assert_eq!(sequence_of("level"), None);
    assert_eq!(sequence_of("2024"), None, "a bare number has no stem");
    let p = project("r4_seq");
    for (i, n) in ["walker_01", "walker_02", "walker_10", "walker_03", "spin1", "spin 2", "single"].iter().enumerate() {
        pic(&p.join(format!("inbox/{}.png", n)), i as u64 + 1);
    }
    let t = ok(&pxl_in(&p, &["ingest", "--size", "16x16"]));
    assert!(t.contains("grouped 4 files into one asset walker (4 frames: walker_01 walker_02 walker_03 walker_10)"), "{}", t);
    assert!(t.contains("grouped 2 files into one asset spin"), "{}", t);
    let part = agentsprite::pxl::load(&p.join("assets/walker/asset.pxl")).unwrap();
    assert_eq!(part.frames.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["walker_01", "walker_02", "walker_03", "walker_10"], "ordered by number, named from the suffix");
    assert!(p.join("assets/single/asset.pxl").is_file(), "an unnumbered picture stays an asset of its own");
    assert!(p.join("assets/walker/input/walker_10.png").is_file() && p.join("assets/spin/input/spin 2.png").is_file());
    assert_eq!(json_of(&pxl_in(&p, &["tree", "--json"]))["assets"].as_array().unwrap().len(), 3);
    // --separate opts out
    let q = project("r4_seq_sep");
    for n in ["a_01", "a_02"] {
        pic(&q.join(format!("inbox/{}.png", n)), 2);
    }
    let t = ok(&pxl_in(&q, &["ingest", "--separate", "--size", "16x16"]));
    assert!(t.contains("ingested 2 of 2") && !t.contains("grouped"), "{}", t);
    assert!(q.join("assets/a_01").is_dir() && q.join("assets/a_02").is_dir());
}

#[test]
fn compare_png_is_reviewable_and_compare_1x_shows_the_real_size() {
    let p = project("r4_cmp");
    for i in 1..=3u64 {
        pic(&p.join(format!("inbox/orb_{:02}.png", i)), i);
    }
    ok(&pxl_in(&p, &["ingest", "--size", "16x16"]));
    let a = p.join("assets/orb");
    let cmp = png::read_rgba(&a.join("compare.png")).unwrap();
    assert!(cmp.w <= 2048 && cmp.h >= 128 + 14 + 20, "{}x{}", cmp.w, cmp.h);
    // three cells of (input | latest), each panel 16 px x factor 4 x zoom 2 = 128 px
    assert!(cmp.w >= 3 * (2 * 128), "input and latest side by side per frame: {}", cmp.w);
    let one = png::read_rgba(&a.join("compare_1x.png")).unwrap();
    assert!(one.w < 128 && one.h < 40, "the real size: {}x{}", one.w, one.h);
    assert_eq!(one.get(0, 0), agentsprite::img::Rgba::opaque(0xd8, 0xd8, 0xd8), "the console's typical background");
    // a project setting overrides it
    let m = text(&p.join("pxl.toml"));
    std::fs::write(p.join("pxl.toml"), m.replace("[project.settings]\n", "[project.settings]\npreview_bg = \"#102030\"\n")).unwrap();
    ok(&pxl_in(&p, &["set", "orb", "orb_01", "3,3=1"]));
    assert_eq!(png::read_rgba(&a.join("compare_1x.png")).unwrap().get(0, 0), agentsprite::img::Rgba::opaque(0x10, 0x20, 0x30));
    // no input: the latest frames alone, still enlarged
    std::fs::remove_dir_all(a.join("input")).unwrap();
    std::fs::write(a.join("asset.pxl"), text(&a.join("asset.pxl")).replace("src = \"input/", "src = \"gone/")).unwrap();
    ok(&pxl_in(&p, &["set", "orb", "orb_01", "5,5=1"]));
    let alone = png::read_rgba(&a.join("compare.png")).unwrap();
    assert!(alone.w < cmp.w, "latest only is narrower: {} vs {}", alone.w, cmp.w);
}
