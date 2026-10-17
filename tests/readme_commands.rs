//! Every `pxl ...` command in the README's code blocks names a real subcommand and real flags (checked against `pxl --help`
//! output), the install one-liners point at files that exist, and the skills named in the README are the shipped ones.
mod e2e_common;

use e2e_common::bin;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn help(path: &[&str]) -> String {
    let o = Command::new(bin()).args(path).arg("--help").output().unwrap();
    assert!(o.status.success(), "pxl {} --help failed", path.join(" "));
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// The names under a `Commands:` heading of a help text.
fn subcommands(h: &str) -> Vec<String> {
    let mut v = Vec::new();
    let mut on = false;
    for l in h.lines() {
        if l.starts_with("Commands:") {
            on = true;
            continue;
        }
        if on {
            if l.trim().is_empty() {
                break;
            }
            if let Some(n) = l.split_whitespace().next() {
                v.push(n.to_string());
            }
        }
    }
    v
}

/// Split a shell-ish line into words, honouring double and single quotes.
fn words(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut q: Option<char> = None;
    let mut started = false;
    for c in line.chars() {
        match q {
            Some(qc) if c == qc => q = None,
            Some(_) => cur.push(c),
            None if c == '"' || c == '\'' => {
                q = Some(c);
                started = true;
            }
            None if c.is_whitespace() => {
                if started || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            None => cur.push(c),
        }
    }
    if started || !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// The `pxl ...` command segments of every fenced code block.
fn readme_commands() -> Vec<Vec<String>> {
    let text = std::fs::read_to_string(root().join("README.md")).unwrap();
    let mut cmds = Vec::new();
    let mut in_block = false;
    for l in text.lines() {
        if l.trim_start().starts_with("```") {
            in_block = !in_block;
            continue;
        }
        if !in_block {
            continue;
        }
        // drop trailing comments, split on && and ;
        let code = match l.find("  #") {
            Some(i) => &l[..i],
            None => l,
        };
        for seg in code.split("&&").flat_map(|s| s.split(';')) {
            let w = words(seg.trim());
            if w.first().map(String::as_str) == Some("pxl") {
                cmds.push(w);
            }
        }
    }
    cmds
}

#[test]
fn every_readme_pxl_command_and_flag_exists() {
    let cmds = readme_commands();
    assert!(cmds.len() >= 30, "the README should carry its commands in code blocks, found {}", cmds.len());
    let top = subcommands(&help(&[]));
    let mut cache: HashMap<String, String> = HashMap::new();
    for c in &cmds {
        let line = c.join(" ");
        let args = &c[1..];
        let first = match args.first() {
            Some(a) => a.as_str(),
            None => continue,
        };
        if first.starts_with('-') {
            continue; // pxl --version
        }
        assert!(top.iter().any(|t| t == first), "`{}`: no such command `{}`", line, first);
        let mut path = vec![first.to_string()];
        let h = cache.entry(first.to_string()).or_insert_with(|| help(&[first])).clone();
        let subs = subcommands(&h);
        let mut h = h;
        if let Some(second) = args.get(1).map(String::as_str).filter(|s| !s.starts_with('-')) {
            if !subs.is_empty() && !subs.iter().all(|s| s == "help") {
                assert!(subs.iter().any(|s| s == second), "`{}`: `{}` has no subcommand `{}` (has {:?})", line, first, second, subs);
                path.push(second.to_string());
                h = cache.entry(path.join(" ")).or_insert_with(|| help(&[first, second])).clone();
            }
        }
        for a in args.iter().filter(|a| a.starts_with("--")) {
            let flag = a.split('=').next().unwrap();
            assert!(h.contains(flag), "`{}`: `pxl {}` has no flag {}", line, path.join(" "), flag);
        }
    }
}

#[test]
fn readme_names_the_shipped_skills_and_real_install_files() {
    let text = std::fs::read_to_string(root().join("README.md")).unwrap();
    for n in agentsprite::skill::names() {
        assert!(text.contains(&format!("`{}`", n)), "README lacks skill {}", n);
    }
    for f in ["install.sh", "install.ps1", "CHANGELOG.md", "LICENSE"] {
        assert!(root().join(f).is_file(), "{}", f);
    }
    assert!(text.contains("raw.githubusercontent.com/kkollsga/agentsprite/main/install.sh"));
    assert!(text.contains("raw.githubusercontent.com/kkollsga/agentsprite/main/install.ps1"));
    assert!(text.contains("/plugin marketplace add kkollsga/agentsprite"));
    // images referenced exist
    for l in text.lines().filter(|l| l.starts_with("![")) {
        let p = l.split('(').nth(1).unwrap().trim_end_matches(')');
        assert!(root().join(p).is_file(), "{}", p);
    }
    // the examples and the skills are game- and client-neutral
    for w in ["Sonic", "Nemesis", "Cold Front", "ModRetro"] {
        assert!(!text.contains(w), "README mentions {}", w);
    }
}

#[test]
fn the_checker_itself_notices_a_bad_command() {
    // proof the extractor is not vacuous: a made-up command line fails the same lookup
    let top = subcommands(&help(&[]));
    assert!(top.iter().any(|t| t == "skill"));
    assert!(!top.iter().any(|t| t == "frobnicate"));
    let skill = subcommands(&help(&["skill"]));
    for s in ["list", "show", "install", "uninstall", "path"] {
        assert!(skill.iter().any(|t| t == s), "{}", s);
    }
    assert_eq!(words("pxl ask \"a b\" --option A=x.png|l"), vec!["pxl", "ask", "a b", "--option", "A=x.png|l"]);
}
