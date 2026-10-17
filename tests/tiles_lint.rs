//! AC-L1 for the tile rules (3 positive and 3 negative fixtures each), AC-L2 fields, and parity with the real
//! `check_assets.py` (skipped when python3, Pillow or `$PXL_CHECK_ASSETS` are missing).

mod tiles_common;
use agentsprite::lint::{self, LintOpts};
use agentsprite::{profile, pxl};
use tiles_common::*;

const RULES: [&str; 3] = ["tile_colors", "tile_palettes", "unique_tiles"];

fn issues(rule: &str, file: &std::path::Path) -> Vec<lint::Issue> {
    let part = pxl::load(file).unwrap();
    let prof = profile::load_for(&part, None).unwrap();
    let opts = LintOpts { rules: vec![rule.to_string()], frames: vec![], refs: false, advice: false };
    lint::lint_part_auto(&part, &prof, &opts).into_iter().filter(|i| i.rule == rule).collect()
}

#[test]
fn every_tile_rule_has_three_positive_and_three_negative_fixtures() {
    let mut n = 0;
    for rule in RULES {
        for kind in ["pos", "neg"] {
            for i in 1..=3 {
                let f = fixtures().join(format!("lint/{}/{}{}.pxl", rule, kind, i));
                assert!(f.is_file(), "missing fixture {}", f.display());
                let found = issues(rule, &f);
                if kind == "pos" {
                    assert!(!found.is_empty(), "{} should trigger {}", f.display(), rule);
                } else {
                    assert!(found.is_empty(), "{} should not trigger {}: {:?}", f.display(), rule, found);
                }
                n += 1;
            }
        }
    }
    assert_eq!(n, 18);
}

#[test]
fn issues_name_the_tile_the_position_and_a_fix() {
    let f = fixtures().join("lint/tile_colors/pos3.pxl");
    let found = issues("tile_colors", &f);
    assert_eq!(found.len(), 2, "{:?}", found);
    let i = &found[0];
    assert_eq!(i.severity, lint::Severity::Error);
    assert!(i.message.contains("tile #0") && i.message.contains("x=0, y=0") && i.message.contains("6 colours"), "{}", i.message);
    assert_eq!((i.x, i.y), (Some(0), Some(0)));
    assert_eq!(i.frame.as_deref(), Some("blocks"));
    assert!(i.fix.as_deref().unwrap().contains("pxl tiles fit") && i.fix.as_deref().unwrap().ends_with("--apply"));
    let j = &found[1];
    assert!(j.message.contains("tile #2") && j.message.contains("x=16, y=0") && j.message.contains("7 colours"), "{}", j.message);
    assert_eq!((j.x, j.y), (Some(16), Some(0)));

    let p = issues("tile_palettes", &fixtures().join("lint/tile_palettes/pos1.pxl"));
    assert_eq!(p.len(), 1);
    assert!(p[0].message.contains("9 different 4-colour palettes") && p[0].message.contains("max 8") && p[0].message.contains("is #8"), "{}", p[0].message);
    assert_eq!((p[0].x, p[0].y), (Some(64), Some(0)));
    assert!(p[0].fix.as_deref().unwrap().contains("tiles fit"));

    let u = issues("unique_tiles", &fixtures().join("lint/unique_tiles/pos1.pxl"));
    assert_eq!(u.len(), 1);
    assert!(u[0].message.contains("301 unique tiles") && u[0].message.contains("max 300") && u[0].message.contains("tile #300"), "{}", u[0].message);
    assert!(u[0].fix.as_deref().unwrap().contains("tiles dedupe") && u[0].fix.as_deref().unwrap().contains("--max 300"));
}

#[test]
fn json_issue_fields_follow_schema_one() {
    let o = std::process::Command::new(env!("CARGO_BIN_EXE_pxl"))
        .current_dir(fixtures().join("lint/tile_colors"))
        .args(["lint", "pos1.pxl", "--rules", "tile_colors", "--json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["schema"], 1);
    let i = &v["parts"][0]["issues"][0];
    for k in ["rule", "severity", "file", "frame", "x", "y", "message", "fix"] {
        assert!(i.get(k).is_some(), "missing {}", k);
    }
    assert_eq!(i["rule"], "tile_colors");
    
    assert!(!o.status.success());
}

#[test]
fn verdicts_match_check_assets_py() {
    let Some(script) = checker_available() else { return };
    let tmp = out_dir("tiles_lint_png");
    for rule in RULES {
        for kind in ["pos", "neg"] {
            for i in 1..=3 {
                let f = fixtures().join(format!("lint/{}/{}{}.pxl", rule, kind, i));
                let part = pxl::load(&f).unwrap();
                let png = tmp.join(format!("{}_{}{}.png", rule, kind, i));
                export_png(&part, 0, &png);
                let (pass, text) = run_checker(&script, &png);
                let theirs = checker_rules(&text);
                let prof = profile::load_for(&part, None).unwrap();
                let opts = LintOpts { rules: RULES.iter().map(|r| r.to_string()).collect(), frames: vec![], refs: false, advice: false };
                let mut ours: Vec<&str> = lint::lint_part_auto(&part, &prof, &opts).iter().filter(|x| RULES.contains(&x.rule.as_str())).map(|x| RULES.iter().find(|r| **r == x.rule).unwrap().to_owned()).collect();
                ours.sort();
                ours.dedup();
                let mut t = theirs.clone();
                t.sort();
                assert_eq!(ours, t, "{}: lint {:?} vs checker {:?}\n{}", f.display(), ours, t, text);
                assert_eq!(pass, ours.is_empty(), "{}\n{}", f.display(), text);
                if kind == "pos" {
                    assert!(theirs.contains(&rule), "{} should fail the checker for {}\n{}", f.display(), rule, text);
                }
            }
        }
    }
}
