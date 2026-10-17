//! Tool side of "skills, not rules": advice never fails anything, skills install without surprises, `row` cannot
//! silently corrupt, images carry coordinates, and the new commands follow the interface conventions.

mod e2e_common;
use agentsprite::img::Rgba;
use e2e_common::*;

const TEXT: Rgba = Rgba::opaque(0xdc, 0xde, 0xe4);

fn project(tag: &str) -> (std::path::PathBuf, &'static str) {
    let root = tmpdir(tag);
    gen_project(&root, 1, 2, 2);
    assert!(pxl_in(&root, &["load", "parts/p00.pxl"]).status.success());
    (root, "parts/p00.pxl")
}

// ------------------------------------------------------------------------------------------ severities

#[test]
fn advice_never_fails_check_lint_or_release() {
    let (root, p) = project("ts_sev");
    // a lone pixel far from the sprite: the orphan observation fires, every hard rule is fine
    assert!(pxl_in(&root, &["set", p, "f0", "0,0=1", "--agent", "A"]).status.success());
    let o = pxl_in(&root, &["lint", "--json", "--advice", p]);
    assert!(o.status.success(), "{}", stdout(&o));
    let v = json_of(&o);
    assert!(v["summary"]["notes"].as_u64().unwrap() >= 1, "{}", v);
    assert_eq!(v["summary"]["warnings"], 0);
    assert_eq!(v["summary"]["errors"], 0);
    let o = pxl_in(&root, &["lint", "--json", p]);
    assert_eq!(json_of(&o)["summary"]["notes"], 0, "look rules are opt in");
    // check counts hard rules only
    let o = pxl_in(&root, &["check", "--json"]);
    let v = json_of(&o);
    assert_eq!(v["totals"]["lint_errors"], 0, "{}", v);
    assert_eq!(v["totals"]["lint_warnings"], 0, "{}", v);
    // release done is not gated by notes
    assert!(pxl_in(&root, &["claim", p, "--agent", "A"]).status.success());
    let o = pxl_in(&root, &["release", p, "--state", "done", "--note", "ok", "--agent", "A"]);
    assert!(o.status.success(), "{}", stderr(&o));
    // an explicit --rules asks for the rule itself
    let v = json_of(&pxl_in(&root, &["lint", "--json", "--rules", "orphan", p]));
    assert_eq!(v["parts"][0]["issues"][0]["severity"], "advice");
    assert!(!v["parts"][0]["issues"][0]["message"].as_str().unwrap().contains("remove"), "neutral wording");
}

#[test]
fn hard_rules_still_fail() {
    let fx = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lint");
    let d = tmpdir("ts_hard");
    for rule in ["rgb_step", "palette_size", "shared_palette"] {
        let f = fx.join(rule).join("pos1.pxl");
        let o = pxl_in(&d, &["lint", "--advice", f.to_str().unwrap()]);
        assert_eq!(o.status.code(), Some(1), "{} must still fail: {}", rule, stdout(&o));
    }
}

// ------------------------------------------------------------------------------------------ skills

#[test]
fn skill_list_show_install() {
    let names = agentsprite::skill::names();
    assert!(names.contains(&"pxl-sprite"), "{:?}", names);
    let d = tmpdir("ts_skill");
    let v = json_of(&pxl_in(&d, &["--json", "skill", "list"]));
    let listed: Vec<String> = v["skills"].as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap().to_string()).collect();
    assert_eq!(listed, names.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    // show = embedded text
    for n in &names {
        let o = pxl_in(&d, &["skill", "show", n]);
        assert!(o.status.success());
        let want = agentsprite::skill::skill_files(n).into_iter().find(|f| f.0 == "SKILL.md").unwrap().1;
        assert_eq!(stdout(&o).trim_end(), want.trim_end());
        for (rel, text) in agentsprite::skill::skill_files(n).into_iter().filter(|f| f.0.starts_with("references/")) {
            let file = rel.trim_start_matches("references/");
            let o = pxl_in(&d, &["skill", "show", n, "--ref", file]);
            assert!(o.status.success(), "{} {}", n, file);
            assert_eq!(stdout(&o).trim_end(), text.trim_end());
            let o = pxl_in(&d, &["skill", "show", n, "--ref", file.trim_end_matches(".md")]);
            assert!(o.status.success(), "reference without extension");
        }
    }
    let o = pxl_in(&d, &["skill", "show", "nope"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(stderr(&o).contains("hint:"));
    assert_eq!(pxl_in(&d, &["skill", "show", "pxl-sprite", "--ref", "nope"]).status.code(), Some(2));
    // project install: .claude/skills in the cwd (the full install behaviour is tested in skill_install.rs)
    let o = pxl_in(&d, &["skill", "install", "--project", "--host", "claude"]);
    assert!(o.status.success(), "{}", stderr(&o));
    for n in &names {
        for (rel, text) in agentsprite::skill::skill_files(n) {
            let got = std::fs::read_to_string(d.join(".claude/skills").join(n).join(rel)).unwrap();
            if rel == "SKILL.md" {
                assert!(got.contains("installed by pxl"), "the version stamp");
            } else {
                assert_eq!(got, text);
            }
        }
        assert!(stdout(&o).contains(n), "prints what it wrote");
    }
    let v = json_of(&pxl_in(&d, &["--json", "skill", "install", "--project", "--host", "claude", "--dry-run"]));
    assert_eq!(v["skills"].as_array().unwrap().len(), names.len());
    assert_eq!(v["dry_run"], true);
}

// ------------------------------------------------------------------------------------------ row safety

#[test]
fn row_echoes_and_refuses_to_leave_the_canvas() {
    let (root, p) = project("ts_row");
    let o = pxl_in(&root, &["row", p, "f0", "2", "3", "1221"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(stdout(&o).contains("row y=3 from x=2"), "{}", stdout(&o));
    let after = std::fs::read(root.join(p)).unwrap();
    // named forms, either order
    let o = pxl_in(&root, &["row", p, "f1", "x=2", "y=3", "1221"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(stdout(&o).contains("row y=3 from x=2"));
    let o = pxl_in(&root, &["row", p, "f1", "y=4", "x=2", "1221"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(stdout(&o).contains("row y=4 from x=2"));
    let before = std::fs::read(root.join(p)).unwrap();
    assert_ne!(before, after);
    // too long for the canvas: error with a hint, file untouched
    let o = pxl_in(&root, &["row", p, "f0", "14", "3", "1221"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("leaves the 16x16 canvas"), "{}", stderr(&o));
    assert!(stderr(&o).contains("swapped"), "{}", stderr(&o));
    // swapped x/y that still lands inside is what the echo is for: y=2, x=3 is plain to read
    let o = pxl_in(&root, &["row", p, "f0", "3", "2", "1"]);
    assert!(stdout(&o).contains("row y=2 from x=3"));
    // a mix of forms is a usage error
    assert_eq!(pxl_in(&root, &["row", p, "f0", "x=2", "3", "1"]).status.code(), Some(2));
    assert_eq!(pxl_in(&root, &["row", p, "f0", "x=2", "x=3", "1"]).status.code(), Some(2));
    assert_eq!(std::fs::read(root.join(p)).unwrap().len() > 0, true);
}

#[test]
fn apply_help_documents_every_op() {
    let d = tmpdir("ts_apply");
    let o = pxl_in(&d, &["apply", "--help"]);
    let t = stdout(&o);
    for op in ["set", "row", "patch", "line", "rect", "fill", "replace", "copy", "mirror", "flip", "write_grid", "stamp", "move", "nudge", "outline", "fix", "pick"] {
        assert!(t.lines().any(|l| l.trim_start().starts_with(&format!("{} ", op))), "apply --help lacks op {}", op);
    }
    for field in ["to_at", "skip_transparent", "region", "pixels", "maps", "from:[x,y]"] {
        assert!(t.contains(field), "apply --help lacks field {}", field);
    }
}

// ------------------------------------------------------------------------------------------ images

#[test]
fn images_carry_coordinates_and_stay_within_the_limit() {
    let (root, p) = project("ts_img");
    let o = pxl_in(&root, &["compare", p, "--json"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let v = json_of(&o);
    assert!(v["ranking_label"].as_str().unwrap().contains("where to look first"));
    assert!(v["frames"][0]["notes"].is_array());
    let crop = v["layout"]["crop"].as_array().unwrap();
    assert_eq!(crop.len(), 4);
    let page = v["pages"][0]["file"].as_str().unwrap();
    let im = agentsprite::png::read_rgba(&root.join(page)).unwrap();
    assert!(im.w <= 1568 && im.h <= 1568, "{}x{}", im.w, im.h);
    let out = &v["frames"][0]["panels"]["output"];
    let inp = &v["frames"][0]["panels"]["input"];
    let count = |o: &serde_json::Value| -> usize {
        let (ox, oy) = (o[0].as_u64().unwrap() as usize, o[1].as_u64().unwrap() as usize);
        let pw = v["layout"]["panel_px"][0].as_u64().unwrap() as usize;
        let mut n = 0;
        // labels live in the gutter above the panel's art, never over it
        let g = v["layout"]["coord_gutter"][1].as_u64().unwrap() as usize;
        for y in oy - g..oy {
            for x in ox..ox + pw {
                if im.px[y * im.w + x] == TEXT {
                    n += 1;
                }
            }
        }
        n
    };
    assert!(count(out) > 0, "output panel carries x labels in its top gutter");
    {
        // no label pixel over the art itself
        let (ox, oy) = (out[0].as_u64().unwrap() as usize, out[1].as_u64().unwrap() as usize);
        let (pw, ph) = (v["layout"]["panel_px"][0].as_u64().unwrap() as usize, v["layout"]["panel_px"][1].as_u64().unwrap() as usize);
        let covered = (oy..oy + ph).flat_map(|y| (ox..ox + pw).map(move |x| (x, y))).filter(|&(x, y)| im.px[y * im.w + x] == TEXT).count();
        assert_eq!(covered, 0, "coordinate labels must not cover art");
    }
    assert_eq!(count(inp), 0, "the input panel stays clean");
    // text output prints the crop origin
    let o = pxl_in(&root, &["compare", p]);
    let t = stdout(&o);
    assert!(t.contains("crop origin x="), "{}", t);
    assert!(t.contains("where to look first"), "{}", t);
    // zoom keeps labels and the limit
    let o = pxl_in(&root, &["compare", p, "--frames", "f0", "--zoom", "--json"]);
    let v = json_of(&o);
    let z = agentsprite::png::read_rgba(&root.join(v["pages"][0]["file"].as_str().unwrap())).unwrap();
    assert!(z.w <= 1568 && z.h <= 1568);
    // candidates: origin line, label, limit
    let o = pxl_in(&root, &["candidates", p, "f0"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let t = stdout(&o);
    assert!(t.contains("window origin x="), "{}", t);
    assert!(t.contains("not a verdict"), "{}", t);
    let v = json_of(&pxl_in(&root, &["--json", "candidates", p, "f0"]));
    let c = agentsprite::png::read_rgba(&root.join(v["images"][0].as_str().unwrap())).unwrap();
    assert!(c.w <= 1568 && c.h <= 1568);
    assert!(c.px.iter().any(|q| *q == TEXT), "candidate panels carry coordinate labels");
}

#[test]
fn view_features_is_an_optional_information_layer() {
    let (root, p) = project("ts_feat");
    assert!(pxl_in(&root, &["set", p, "f0", "5,5=W", "--agent", "A"]).status.success());
    let o = pxl_in(&root, &["--json", "view", p, "f0", "--features"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let v = json_of(&o);
    assert!(v["features"].is_array());
    let t = stdout(&pxl_in(&root, &["view", p, "f0", "--features"]));
    assert!(t.contains("information only"), "{}", t);
}

// ------------------------------------------------------------------------------------------ interface audit

#[test]
fn new_commands_follow_the_interface_conventions() {
    let d = tmpdir("ts_iface");
    for args in [vec!["skill", "list"], vec!["skill", "show", "pxl-sprite"], vec!["skill", "install", "--project", "--dry-run"], vec!["skill", "path", "--project"]] {
        let mut a = vec!["--json"];
        a.extend(&args);
        let o = pxl_in(&d, &a);
        assert!(o.status.success(), "{:?}: {}", args, stderr(&o));
        assert_eq!(stdout(&o).matches('\n').count(), 1, "{:?}: one JSON object", args);
        assert_eq!(json_of(&o)["ok"], true);
    }
    // usage errors: exit 2, JSON error object, hint
    for args in [vec!["--json", "skill", "show", "nope"], vec!["--json", "skill", "bogus"], vec!["--json", "skill", "show"]] {
        let o = pxl_in(&d, &args);
        assert_eq!(o.status.code(), Some(2), "{:?}", args);
        let v = json_of(&o);
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"]["kind"], "usage");
    }
    let h = stdout(&pxl_in(&d, &["--help"]));
    assert!(h.lines().any(|l| l.trim_start().starts_with("skill ")), "skill is listed in --help");
    // guide is short and points to the skill
    let g = stdout(&pxl_in(&d, &["guide"]));
    assert!(g.lines().count() <= 50);
    assert!(g.contains("pxl skill show pxl-sprite"));
}
