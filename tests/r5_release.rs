//! M6 R5: release polish. No silent skips (`reference_missing`), the example scripts on three consoles, the shipped skills and the plugin
//! layout, the operator defaults data shared by the skill and `pxl intake`, README and package metadata.
mod e2e_common;

use e2e_common::*;
use std::path::PathBuf;
#[cfg(unix)]
use std::path::Path;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[cfg(unix)]
fn ok(o: &std::process::Output) -> String {
    assert!(o.status.success(), "{}{}", stdout(o), stderr(o));
    stdout(o)
}

/// The example scripts are bash; they run on macOS/Linux (Windows has no `bash` on PATH that works).
#[cfg(unix)]
fn run_script(script: &str, console: &str, work: &Path) -> std::process::Output {
    std::process::Command::new("bash").arg(root().join(script)).arg(console).arg(work).env("PXL", bin()).output().expect("bash runs the example script")
}

// ------------------------------------------------------------------------------------------ no silent skips

/// A gated project whose hero lost its input pictures: the gates cannot compare, and say so by part and frame; `check` counts them.
#[test]
#[cfg(unix)] // runs examples/*/run.sh with bash
fn a_missing_source_image_is_a_reference_missing_warning_not_a_silent_skip() {
    let work = tmpdir("r5_refmissing");
    let o = run_script("examples/project/run.sh", "gbc", &work);
    ok(&o);
    let p = work.join("demo");
    // with the inputs present: nothing is missing, and check says how many frames it compared
    let c = ok(&pxl_in(&p, &["check"]));
    assert!(c.contains("references: checked 6 of 6 frame(s) with references"), "{}", c);
    assert!(!c.contains("reference_missing"), "{}", c);
    // take the hero's pictures away (hero_02 and hero_03 only)
    for n in ["hero_02", "hero_03"] {
        std::fs::remove_file(p.join(format!("assets/player/hero/input/{}.png", n))).unwrap();
    }
    let l = stdout(&pxl_in(&p, &["lint", "hero"]));
    assert!(l.contains("reference_missing") && l.contains("part hero frame hero_02") && l.contains("part hero frame hero_03"), "{}", l);
    assert!(l.contains("eye_parity") && l.contains("not checked without it"), "the warning names the gates that could not run: {}", l);
    assert!(!l.contains("part hero frame hero_01"), "hero_01 still has its picture: {}", l);
    // the gate warnings of the frames that have a picture still run
    let j = json_of(&pxl_in(&p, &["lint", "hero", "--json"]));
    let issues = j["parts"][0]["issues"].as_array().unwrap();
    assert_eq!(issues.iter().filter(|i| i["rule"] == "reference_missing").count(), 2, "{}", j);
    // check counts them, in text and JSON
    let c = stdout(&pxl_in(&p, &["check"]));
    assert!(c.contains("references: checked 4 of 6 frame(s) with references (2 reference_missing)"), "{}", c);
    assert!(c.contains("reference_missing: part hero frame hero_02"), "{}", c);
    let cj = json_of(&pxl_in(&p, &["check", "--json"]));
    assert_eq!(cj["references"]["checked"], 4);
    assert_eq!(cj["references"]["of"], 6);
    assert_eq!(cj["references"]["reference_missing"], 2);
    // compare on the frame that has a picture warns about the neighbours it lost
    let cmp = pxl_in(&p, &["compare", "hero", "--frames", "hero_01"]);
    let t = format!("{}{}", stdout(&cmp), stderr(&cmp));
    assert!(t.contains("reference_missing: part hero frame hero_02"), "{}", t);
    // asking compare for a frame without its picture is an error that names it
    let bad = pxl_in(&p, &["compare", "hero", "--frames", "hero_02"]);
    assert!(!bad.status.success() && format!("{}{}", stdout(&bad), stderr(&bad)).contains("hero_02"));
    // style analyze names the asset and the frames
    let s = ok(&pxl_in(&p, &["style", "analyze"]));
    assert!(s.contains("reference_missing: asset hero frame(s) hero_02, hero_03"), "{}", s);
    // a missing source never blocks an approval by itself: the other gates decide
    let rm = agentsprite::design::REFERENCE_MISSING;
    assert_eq!(rm, "reference_missing");
}

// ------------------------------------------------------------------------------------------ examples

#[test]
fn the_example_inputs_are_original_cc0_art() {
    let r = root().join("examples");
    let lic = std::fs::read_to_string(r.join("LICENSE")).unwrap();
    assert!(lic.contains("CC0"), "{}", lic);
    for f in ["hero/hero_01.png", "hero/hero_02.png", "hero/hero_03.png", "enemy/crawler_01.png", "enemy/crawler_02.png", "item/gem.png", "tiles/ground.png", "screens/title_160x144.png"] {
        let im = agentsprite::png::read_rgba(&r.join("inputs").join(f)).unwrap_or_else(|e| panic!("{}: {:?}", f, e));
        assert!(im.px.iter().any(|p| p.a > 0), "{} is empty", f);
    }
    let hero = agentsprite::png::read_rgba(&r.join("inputs/hero/hero_01.png")).unwrap();
    assert_eq!((hero.w, hero.h), (32, 48), "2x the 16x24 target");
}

#[test]
#[cfg(unix)] // runs examples/*/run.sh with bash
fn the_single_image_example_runs_and_makes_a_compare_image() {
    let work = tmpdir("r5_single");
    let o = std::process::Command::new("bash").arg(root().join("examples/single/run.sh")).arg("nes").arg(&work).env("PXL", bin()).output().unwrap();
    let t = ok(&o);
    assert!(work.join("hero.pxl/compare.png").is_file() && work.join("hero.pxl/export").is_dir(), "{}", t);
    assert!(t.contains("compare image"), "{}", t);
}

#[test]
#[cfg(unix)] // runs examples/*/run.sh with bash
fn the_project_example_runs_on_gbc_nes_and_md() {
    for console in ["gbc", "nes", "md"] {
        let work = tmpdir(&format!("r5_proj_{}", console));
        let o = run_script("examples/project/run.sh", console, &work);
        let t = format!("{}{}", stdout(&o), stderr(&o));
        assert!(o.status.success(), "{}: {}", console, t);
        let p = work.join("demo");
        assert!(t.contains("dry run PASSED"), "{}: {}", console, t);
        for f in ["brief.toml", "style.md", "plan.toml", "export/manifest.json", "assets/player/hero/compare.png", "assets/screens/title/compare.png"] {
            assert!(p.join(f).is_file(), "{}: {} missing\n{}", console, f, t);
        }
        let m: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(p.join("export/manifest.json")).unwrap()).unwrap();
        assert_eq!(m["console"], console);
    }
}

// ------------------------------------------------------------------------------------------ skills and plugin

fn frontmatter(text: &str) -> Vec<(String, String)> {
    let mut lines = text.lines();
    assert_eq!(lines.next().map(str::trim), Some("---"), "front matter starts with ---");
    let mut v = Vec::new();
    for l in lines {
        if l.trim() == "---" {
            return v;
        }
        if let Some((k, val)) = l.split_once(':') {
            v.push((k.trim().to_string(), val.trim().to_string()));
        }
    }
    panic!("front matter is not closed");
}

#[test]
fn every_skill_has_correct_frontmatter_and_a_concise_body() {
    let names = agentsprite::skill::names();
    for n in ["pxl-sprite", "pxl-tiles", "pxl-review", "pxl-coordinate", "pxl-operator", "pxl-extract"] {
        assert!(names.contains(&n), "skill {} is not shipped: {:?}", n, names);
    }
    for n in &names {
        let files = agentsprite::skill::skill_files(n);
        let md = files.iter().find(|f| f.0 == "SKILL.md").unwrap().1;
        let fm = frontmatter(md);
        let get = |k: &str| fm.iter().find(|f| f.0 == k).map(|f| f.1.clone()).unwrap_or_default();
        assert_eq!(get("name"), *n, "name matches the folder");
        let d = get("description");
        assert!(d.len() > 60 && d.len() <= 1024 && d.contains("Use when"), "{}: description should say what and when ({} chars)", n, d.len());
        assert!(md.lines().count() <= 500, "{}: SKILL.md over 500 lines", n);
        assert!(files.iter().any(|f| f.0.starts_with("references/")), "{} has references/", n);
        // every references/ file named in the body exists
        for w in md.split(|c: char| !(c.is_alphanumeric() || "-_./".contains(c))) {
            if let Some(f) = w.strip_prefix("references/").filter(|f| f.ends_with(".md") || f.ends_with(".toml")) {
                assert!(files.iter().any(|x| x.0 == format!("references/{}", f)), "{}: references/{} is named but missing", n, f);
            }
        }
    }
}

#[test]
fn the_generic_skills_name_no_single_console_as_the_rule() {
    // the hard-won lessons stay as general rules: each is in the craft text
    let craft = agentsprite::skill::skill_files("pxl-sprite").into_iter().find(|f| f.0 == "SKILL.md").unwrap().1;
    for needle in ["eyes", "identity colour", "style.md", "console", "consistency"] {
        assert!(craft.to_lowercase().contains(needle), "pxl-sprite lacks {:?}", needle);
    }
    for n in ["pxl-sprite", "pxl-tiles", "pxl-review", "pxl-coordinate"] {
        let md = agentsprite::skill::skill_files(n).into_iter().find(|f| f.0 == "SKILL.md").unwrap().1;
        assert!(md.contains("style.md") || md.contains("[style]"), "{} does not read the design language", n);
        assert!(!md.contains("300 unique tiles hardware") && !md.contains("4-colour, 8-palette"), "{} still hard-codes one console", n);
    }
}

#[test]
fn the_plugin_packages_the_skills() {
    let r = root();
    let pj: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(r.join(".claude-plugin/plugin.json")).unwrap()).unwrap();
    let name = pj["name"].as_str().unwrap();
    assert!(name == "agentsprite" && !name.starts_with("claude-") && !name.contains(' '), "{}", name);
    assert_eq!(pj["license"], "MIT");
    assert_eq!(pj["version"], env!("CARGO_PKG_VERSION"), "plugin.json version follows Cargo.toml");
    assert!(pj["description"].as_str().unwrap().len() > 20 && pj["author"]["name"].is_string());
    let mk: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(r.join(".claude-plugin/marketplace.json")).unwrap()).unwrap();
    assert!(mk["name"].is_string() && mk["owner"]["name"].is_string());
    let entry = &mk["plugins"][0];
    assert_eq!(entry["name"], pj["name"], "entry name equals the manifest name");
    assert!(entry["source"] == "./" || entry["source"] == ".", "{}", entry["source"]);
    // the plugin's skills/ folder is the one the binary embeds: one source of truth
    for n in agentsprite::skill::names() {
        assert!(r.join("skills").join(n).join("SKILL.md").is_file(), "skills/{}/SKILL.md", n);
    }
}

// ------------------------------------------------------------------------------------------ operator data

#[test]
fn operator_defaults_are_data_shared_by_the_skill_and_intake() {
    let text = agentsprite::skill::skill_files("pxl-operator").into_iter().find(|f| f.0 == "references/defaults.toml").expect("defaults.toml ships in the skill").1;
    let t: toml::Table = text.parse().unwrap();
    let d = t["defaults"].as_table().unwrap();
    let tradeoff: Vec<&str> = d["tradeoff"].as_array().unwrap().iter().filter_map(|x| x.as_str()).collect();
    assert_eq!(tradeoff.iter().map(|s| s.to_string()).collect::<Vec<_>>(), agentsprite::intake::default_tradeoff(), "intake reads the skill's list");
    assert_eq!(tradeoff[0], "cut animation steps");
    assert_eq!(agentsprite::intake::fidelity_options(), vec!["faithful", "reinterpreted"]);
    assert!(agentsprite::intake::cadence_options().contains(&"as_work_lands".to_string()));
    let topics: Vec<&str> = t["question"].as_array().unwrap().iter().filter_map(|q| q["topic"].as_str()).collect();
    assert!(topics.contains(&"which console is the target") && topics.contains(&"style direction"));
}

// ------------------------------------------------------------------------------------------ README and metadata

#[test]
fn the_readme_covers_every_console_and_the_workflow() {
    let readme = std::fs::read_to_string(root().join("README.md")).unwrap();
    let list = stdout(&pxl_in(&root(), &["console", "list", "--json"]));
    let j: serde_json::Value = serde_json::from_str(&list).unwrap();
    for c in j["consoles"].as_array().unwrap() {
        let id = c["id"].as_str().unwrap();
        assert!(readme.contains(&format!("`{}`", id)), "README console table lacks {}", id);
    }
    for needle in ["For agents: start here", "plugin marketplace add", "ingest", "intake", "style", "plan", "deliver", "Licence", "right to use", "pxl image", "pxl new"] {
        assert!(readme.contains(needle), "README lacks {:?}", needle);
    }
}

#[test]
fn cargo_metadata_is_ready_and_points_at_the_public_repository() {
    let t: toml::Table = std::fs::read_to_string(root().join("Cargo.toml")).unwrap().parse().unwrap();
    let p = t["package"].as_table().unwrap();
    for k in ["description", "repository", "homepage", "keywords", "categories", "readme", "license", "version"] {
        assert!(p.contains_key(k), "Cargo.toml [package] lacks {}", k);
    }
    assert_eq!(p["version"].as_str().unwrap(), "0.1.0");
    assert_eq!(p["repository"].as_str(), Some("https://github.com/kkollsga/agentsprite"));
    assert_eq!(p["homepage"].as_str(), Some("https://github.com/kkollsga/agentsprite"));
    assert_eq!(p["publish"].as_bool(), Some(false), "distributed as prebuilt binaries, not on crates.io");
    for f in [".claude-plugin/plugin.json", ".claude-plugin/marketplace.json", "README.md"] {
        let text = std::fs::read_to_string(root().join(f)).unwrap();
        assert!(!text.contains("OWNER"), "{} still has the OWNER placeholder", f);
    }
}
