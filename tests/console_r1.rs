//! M6 R1: the console model. Resolution (console + mode), project overrides, `kind` as the old name of `mode`,
//! deprecated profile names, `pxl console list | show | set`, `pxl migrate`.
mod e2e_common;

use agentsprite::model::PartKind;
use agentsprite::{console, profile, pxl};
use e2e_common::*;
use std::path::Path;

fn old(name: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/console/old_profiles").join(name)).unwrap()
}

#[test]
fn gbc_modes_carry_exactly_the_numbers_of_the_old_profiles() {
    for (mode, file) in [("sprite", "gbc-sprite.toml"), ("tile", "gbc-tile.toml"), ("screen", "gbc-screen.toml")] {
        let want = profile::parse(mode, &old(file), file).unwrap();
        let mut got = console::resolve("gbc", mode, None).unwrap();
        assert_eq!(got.name, format!("gbc/{}", mode));
        got.name = mode.to_string();
        assert_eq!(got.auto_template, if mode == "sprite" { "set" } else { "set" }, "gbc derives palette sets for new assets");
        got.auto_template = want.auto_template.clone();
        got.style = want.style.clone();
        assert_eq!(got, want, "gbc/{} differs from the old {}", mode, file);
    }
    assert_eq!(console::resolve("gbc", "screen", None).unwrap().size, Some((160, 144)), "the screen mode takes the console's screen size");
    assert_eq!(console::resolve("gbc", "tile", None).unwrap().size, None);
}

#[test]
fn old_profile_names_still_resolve() {
    for (old_name, new_name) in [("gbc-sprite", "gbc/sprite"), ("gbc-tile", "gbc/tile"), ("gbc-screen", "gbc/screen")] {
        assert_eq!(profile::load(old_name).unwrap(), profile::load(new_name).unwrap());
    }
    assert!(profile::load("gbc-tile-legacy").is_err(), "gbc-tile-legacy is removed");
    let e = profile::load("zx81/sprite").unwrap_err();
    assert!(e.msg.contains("unknown console 'zx81'"), "{}", e.msg);
}

#[test]
fn a_project_overrides_single_keys_of_any_console_file() {
    let d = tmpdir("r1_override");
    std::fs::create_dir_all(d.join("consoles/gbc")).unwrap();
    std::fs::write(d.join("consoles/gbc/sprite.toml"), "hw_cells_max = 20\n[budget.enemy]\nper_frame = 5\n").unwrap();
    std::fs::write(d.join("consoles/gbc/console.toml"), "channel_step = 4\n").unwrap();
    let s = console::resolve("gbc", "sprite", Some(&d)).unwrap();
    assert_eq!(s.hw_cells_max, 20);
    assert_eq!(s.channel_step, 4, "console.toml override reaches the mode");
    assert_eq!(s.budgets["enemy"].per_frame, 5);
    assert_eq!(s.budgets["enemy"].per_line, 4, "tables merge: per_line stays");
    assert_eq!(s.colors, 3, "untouched keys keep the built-in value");
    let t = console::resolve("gbc", "tile", Some(&d)).unwrap();
    assert_eq!((t.hw_cells_max, t.channel_step), (40, 4));
    // outside the project the built-in is unchanged
    assert_eq!(console::resolve("gbc", "sprite", None).unwrap().hw_cells_max, 40);
    // a typo in an override is an error that names the file
    std::fs::write(d.join("consoles/gbc/tile.toml"), "unique_tilez = 3\n").unwrap();
    let e = console::resolve("gbc", "tile", Some(&d)).unwrap_err();
    assert!(e.msg.contains("unknown profile key 'unique_tilez'"), "{}", e.msg);
    // a console only the project defines
    std::fs::create_dir_all(d.join("consoles/mini")).unwrap();
    std::fs::write(d.join("consoles/mini/console.toml"), "[console]\nname = \"Mini\"\nscreen = [96, 64]\nchannel_step = 16\n").unwrap();
    assert_eq!(console::resolve("mini", "screen", Some(&d)).unwrap().size, Some((96, 64)));
    assert!(console::list(Some(&d)).iter().any(|c| c.id == "mini" && c.source == "project"));
}

#[test]
fn mode_maps_from_kind_and_kind_stays_accepted() {
    let parse = |h: &str| pxl::parse_str(&format!("name = \"x\"\n{}size = [16, 16]\n", h), Path::new("x.pxl"));
    assert_eq!(parse("").unwrap().kind, PartKind::Sprite);
    assert_eq!(parse("kind = \"tileset\"\n").unwrap().kind, PartKind::Tileset);
    assert_eq!(parse("mode = \"tile\"\n").unwrap().kind, PartKind::Tileset);
    assert_eq!(parse("mode = \"screen\"\n").unwrap().kind, PartKind::Screen);
    assert_eq!(parse("mode = \"tile\"\nkind = \"tileset\"\n").unwrap().kind, PartKind::Tileset);
    assert!(parse("mode = \"tile\"\nkind = \"screen\"\n").unwrap_err().msg.contains("disagree"));
    assert!(parse("mode = \"bogus\"\n").unwrap_err().msg.contains("unknown mode"));
    // the canonical form writes `mode`, and parses back
    let p = parse("kind = \"tileset\"\n").unwrap();
    let text = pxl::to_string(&p);
    assert!(text.contains("mode = \"tile\"") && !text.contains("kind ="), "{}", text);
    assert_eq!(pxl::to_string(&pxl::parse_str(&text, Path::new("x.pxl")).unwrap()), text);
    // profile resolution follows the mode
    let d = tmpdir("r1_mode");
    let mut part = p.clone();
    part.path = d.join("x.pxl");
    assert_eq!(profile::load_for(&part, None).unwrap().name, "gbc/tile");
    part.kind = PartKind::Screen;
    assert_eq!(profile::load_for(&part, None).unwrap().size, Some((160, 144)));
    part.profile = Some("gbc-sprite".into());
    assert_eq!(profile::load_for(&part, None).unwrap().name, "gbc/sprite");
    part.profile = Some("gbc/tile".into());
    assert_eq!(profile::load_for(&part, None).unwrap().name, "gbc/tile");
    part.profile = Some("tile".into());
    assert_eq!(profile::load_for(&part, None).unwrap().name, "gbc/tile");
}

#[test]
fn the_project_console_comes_from_pxl_toml() {
    let d = tmpdir("r1_project");
    std::fs::write(d.join("pxl.toml"), "[project]\nconsole = \"mini\"\n").unwrap();
    std::fs::create_dir_all(d.join("consoles/mini")).unwrap();
    std::fs::write(d.join("consoles/mini/console.toml"), "channel_step = 16\n").unwrap();
    let pc = console::project_console(&d);
    assert_eq!((pc.id.as_str(), pc.explicit), ("mini", true));
    let mut part = pxl::parse_str("name = \"x\"\nsize = [8, 8]\n", Path::new("x.pxl")).unwrap();
    part.path = d.join("x.pxl");
    assert_eq!(profile::load_for(&part, None).unwrap().channel_step, 16);
    std::fs::write(d.join("pxl.toml"), "[project]\nconsole = \"nope\"\n").unwrap();
    assert!(profile::load_for(&part, None).unwrap_err().msg.contains("unknown console 'nope'"));
    std::fs::write(d.join("pxl.toml"), "").unwrap();
    let pc = console::project_console(&d);
    assert_eq!((pc.id.as_str(), pc.explicit, pc.manifest.is_some()), ("gbc", false, true));
}

fn old_project(tag: &str) -> std::path::PathBuf {
    let d = tmpdir(tag);
    gen_project(&d, 2, 1, 5);
    // a tile part in the old form
    std::fs::write(d.join("templates/tiles.toml"), TEMPLATE).unwrap();
    std::fs::write(d.join("parts/t.pxl"), "name = \"t\"\nkind = \"tileset\"\nsize = [128, 8]\ntile = [8, 8]\ntemplate = \"templates/tiles.toml\"\n\n[[frames]]\nname = \"blocks\"\ngrid = \"\"\"\n".to_string() + &"1".repeat(128) + "\n" + &"1".repeat(128) + "\n" + &"1".repeat(128) + "\n" + &"1".repeat(128) + "\n" + &"1".repeat(128) + "\n" + &"1".repeat(128) + "\n" + &"1".repeat(128) + "\n" + &"1".repeat(128) + "\n\"\"\"\n").unwrap();
    // a screen part whose old profile is a different mode than its own
    std::fs::write(d.join("parts/odd.pxl"), "name = \"odd\"\nprofile = \"gbc-tile\"\nsize = [16, 16]\n\n[[frames]]\nname = \"f\"\ngrid = \"\"\"\n".to_string() + &(".".repeat(16) + "\n").repeat(16) + "\"\"\"\n").unwrap();
    let mut m = std::fs::read_to_string(d.join("pxl.toml")).unwrap();
    m.push_str("[parts.t]\npath = \"parts/t.pxl\"\n[parts.odd]\npath = \"parts/odd.pxl\"\n");
    std::fs::write(d.join("pxl.toml"), m).unwrap();
    d
}

#[test]
fn migrate_rewrites_an_old_project_and_is_idempotent() {
    let d = old_project("r1_migrate");
    let before_lint = pxl_in(&d, &["lint", "parts/p00.pxl", "parts/t.pxl", "--json"]);
    let lint_before = json_of(&before_lint)["summary"].clone();
    let note = stderr(&pxl_in(&d, &["lint", "parts/p00.pxl"]));
    assert!(note.contains("deprecated profile") && note.contains("no `[project] console`"), "{}", note);
    let raw: Vec<String> = ["p00", "p01", "t", "odd"].iter().map(|n| std::fs::read_to_string(d.join(format!("parts/{}.pxl", n))).unwrap()).collect();
    // dry run changes nothing
    let o = pxl_in(&d, &["migrate", "--dry-run"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let t = stdout(&o);
    assert!(t.contains("add [project] console = \"gbc\"") && t.contains("kind = \"tileset\" -> mode = \"tile\"") && t.contains("remove profile = \"gbc-sprite\"") && t.contains("profile = \"gbc-tile\" -> \"gbc/tile\"") && t.contains("dry run"), "{}", t);
    assert!(!std::fs::read_to_string(d.join("pxl.toml")).unwrap().contains("[project]"));
    for (i, n) in ["p00", "p01", "t", "odd"].iter().enumerate() {
        assert_eq!(std::fs::read_to_string(d.join(format!("parts/{}.pxl", n))).unwrap(), raw[i]);
    }
    // the real run
    let o = pxl_in(&d, &["migrate"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let manifest = std::fs::read_to_string(d.join("pxl.toml")).unwrap();
    assert!(manifest.contains("[project]\nconsole = \"gbc\""), "{}", manifest);
    assert!(manifest.starts_with("legacy = true"), "top-level keys stay above the first table: {}", manifest);
    let p00 = std::fs::read_to_string(d.join("parts/p00.pxl")).unwrap();
    assert!(!p00.contains("profile ="), "{}", p00);
    let t_after = std::fs::read_to_string(d.join("parts/t.pxl")).unwrap();
    assert!(t_after.contains("mode = \"tile\"") && !t_after.contains("kind ="), "{}", t_after);
    let odd = std::fs::read_to_string(d.join("parts/odd.pxl")).unwrap();
    assert!(odd.contains("profile = \"gbc/tile\""), "{}", odd);
    let j = std::fs::read_to_string(d.join("parts/t.journal")).unwrap();
    assert!(j.contains("\"cmd\":\"migrate\""), "{}", j);
    // same lint result, no notes any more
    let after = pxl_in(&d, &["lint", "parts/p00.pxl", "parts/t.pxl", "--json"]);
    assert_eq!(json_of(&after)["summary"], lint_before);
    assert!(!stderr(&pxl_in(&d, &["lint", "parts/p00.pxl"])).contains("deprecated"));
    // idempotent: a second run does nothing, and writes nothing
    let snapshot = tree(&d, &[]);
    let o = pxl_in(&d, &["migrate"]);
    assert!(o.status.success() && stdout(&o).contains("nothing to migrate"), "{}", stdout(&o));
    assert_eq!(tree(&d, &[]), snapshot);
    // the journal makes it undoable, byte for byte
    let o = pxl_in(&d, &["undo", "parts/t.pxl"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    assert_eq!(std::fs::read_to_string(d.join("parts/t.pxl")).unwrap(), raw[2]);
}

#[test]
fn console_list_show_and_set() {
    let d = old_project("r1_cli");
    let o = pxl_in(&d, &["console", "list"]);
    let t = stdout(&o);
    assert!(o.status.success() && t.contains("gbc") && t.contains("Game Boy Color") && t.contains("160x144") && t.contains("RGB555"), "{}", t);
    assert!(t.contains("project console: gbc (default"), "{}", t);
    let o = pxl_in(&d, &["console", "show", "gbc"]);
    let t = stdout(&o);
    assert!(o.status.success(), "{}{}", t, stderr(&o));
    for want in ["[sprite]", "[tile]", "[screen]", "channel_step = 8   (console)", "unique_tiles_max = 348", "unique_tiles_max = 360", "size = [160, 144]", "budget.enemy", "Pan Docs"] {
        assert!(t.contains(want), "missing {:?} in:\n{}", want, t);
    }
    let j = json_of(&pxl_in(&d, &["console", "show", "gbc", "--json"]));
    assert_eq!(j["modes"]["tile"]["facts"]["bg_reserved"], "[7]");
    assert_eq!(j["console"]["screen"], serde_json::json!([160, 144]));
    let e = pxl_in(&d, &["console", "show", "zx81"]);
    assert!(!e.status.success() && stderr(&e).contains("unknown console 'zx81'"), "{}", stderr(&e));
    // set writes [project] console before the first table, then replaces it
    let o = pxl_in(&d, &["console", "set", "gbc"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let m = std::fs::read_to_string(d.join("pxl.toml")).unwrap();
    assert!(m.starts_with("legacy = true\n[project]\nconsole = \"gbc\"\n") || m.contains("[project]\nconsole = \"gbc\"\n"), "{}", m);
    assert_eq!(m.matches("[project]").count(), 1);
    let o = pxl_in(&d, &["console", "set", "gbc"]);
    assert!(o.status.success());
    assert_eq!(std::fs::read_to_string(d.join("pxl.toml")).unwrap(), m, "setting the same console twice changes nothing");
    assert!(stdout(&pxl_in(&d, &["console", "list"])).contains("project console: gbc\n") || stdout(&pxl_in(&d, &["console", "list"])).trim_end().ends_with("project console: gbc"));
    assert!(!pxl_in(&d, &["console", "set", "nope"]).status.success());
    let t_text = console::set_console_text("# head\nout_root = \"x\"\n\n# the parts\n[parts.a]\npath = \"a.pxl\"\n", "gbc");
    assert_eq!(t_text, "# head\nout_root = \"x\"\n\n[project]\nconsole = \"gbc\"\n\n# the parts\n[parts.a]\npath = \"a.pxl\"\n");
    assert_eq!(console::set_console_text("[project]\nname = \"p\"\n[parts.a]\npath = \"a\"\n", "x"), "[project]\nconsole = \"x\"\nname = \"p\"\n[parts.a]\npath = \"a\"\n");
}
