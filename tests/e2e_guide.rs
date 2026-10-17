//! AC-I4: `pxl guide` is one page and every `pxl ...` line in it parses, and neither
//! the guide nor the shipped skills mention commands that no longer exist.

mod e2e_common;
use agentsprite::cli::Cli;
use clap::Parser;
use e2e_common::*;

/// Shell-like split: single and double quotes group, an unquoted `#` at the start of a word ends the line.
fn split(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut have = false;
    let mut q: Option<char> = None;
    for c in line.chars() {
        match (q, c) {
            (Some(qc), c) if c == qc => q = None,
            (Some(_), c) => cur.push(c),
            (None, '\'') | (None, '"') => {
                q = Some(c);
                have = true;
            }
            (None, '#') if !have && cur.is_empty() => return out,
            (None, c) if c.is_whitespace() => {
                if have || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    have = false;
                }
            }
            (None, c) => cur.push(c),
        }
    }
    if have || !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// The `pxl ...` command lines of the guide (indented lines starting with `pxl `), as argument lists after `pxl`.
fn commands() -> Vec<Vec<String>> {
    agentsprite::guide::TEXT
        .lines()
        .map(|l| l.trim_start())
        .filter(|l| l.starts_with("pxl "))
        .map(|l| split(l)[1..].to_vec())
        .collect()
}

fn subst(a: &str) -> String {
    a.replace("PART", "parts/player.pxl").replace("FRAME", "06_walk11").replace("OTHER", "07_walk12")
}

#[test]
fn guide_is_one_page_with_the_required_sections() {
    let t = agentsprite::guide::TEXT;
    assert!(t.lines().count() <= 50, "{} lines", t.lines().count());
    for need in [
        "SKILLS", "LOOK AND DECIDE", "EDIT", "DELIVER", "METHODS", "SELECTORS", "tag:walk", "6-11", "walk*", "--ref", "--zoom", "--try",
        "vote", "nearest", "contour", "exemplar", "auto", "apply", "undo", "lint", "export", "release", "--agent", "Exit codes", "locked", "candidates", "pick",
    ] {
        assert!(t.contains(need), "guide lacks {:?}", need);
    }
    assert!(t.starts_with("Read the skill: `pxl skill show pxl-sprite` (or install with `pxl skill install`)"), "the guide starts by pointing to the skill");
    for gone in ["QUALITY BAR", "no orphan pixels", "EYES FIRST", "is done when all hold"] {
        assert!(!t.contains(gone), "rule-like text {:?} belongs in the skills, not the guide", gone);
    }
    let o = pxl_in(&std::env::temp_dir(), &["guide"]);
    assert!(o.status.success());
    assert_eq!(stdout(&o), t, "`pxl guide` prints the page unchanged");
}

#[test]
fn every_guide_command_parses() {
    let cmds = commands();
    assert!(cmds.len() >= 25, "only {} commands extracted", cmds.len());
    for c in &cmds {
        let mut argv: Vec<String> = vec!["pxl".into()];
        argv.extend(c.iter().map(|a| subst(a)));
        if let Err(e) = Cli::try_parse_from(&argv) {
            panic!("guide command does not parse: pxl {}\n{}", c.join(" "), e);
        }
    }
}


#[test]
fn guide_and_skills_name_no_removed_commands() {
    let mut texts = vec![("guide".to_string(), agentsprite::guide::TEXT.to_string())];
    for name in agentsprite::skill::names() {
        for (f, body) in agentsprite::skill::skill_files(name) {
            texts.push((format!("{}/{}", name, f), body.to_string()));
        }
    }
    for (n, t) in &texts {
        for gone in ["pxl src ", "tiles wrap", "tiles solid", "tiles anim", "--level", "solid_match", "wrap_seam", "pxl view PART FRAME --char"] {
            assert!(!t.contains(gone), "{} mentions the removed `{}`", n, gone);
        }
    }
}
