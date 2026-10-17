//! Unit tests of `scripts/release_gates.sh`: every gate through its pass path and its fail path, the release workflow
//! wiring, and the version lockstep of Cargo.toml, the plugin manifest and the changelog.
#![cfg(unix)] // scripts/release_gates.sh is a bash script run by the Linux release job
mod e2e_common;

use e2e_common::tmpdir;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn gate(args: &[&str]) -> Output {
    Command::new("bash").arg(root().join("scripts/release_gates.sh")).args(args).env_remove("GITHUB_OUTPUT").output().unwrap()
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn read(p: &str) -> String {
    std::fs::read_to_string(root().join(p)).unwrap()
}

#[test]
fn publish_ref_accepts_only_version_tags() {
    assert!(gate(&["assert_publish_ref", "v0.1.0", "tag", "push"]).status.success());
    let o = gate(&["assert_publish_ref", "main", "branch", "workflow_dispatch"]);
    assert!(!o.status.success() && err(&o).contains("::error::"), "{}", err(&o));
    let o = gate(&["assert_publish_ref", "release-1", "tag", "push"]);
    assert!(!o.status.success(), "a tag that is not v<N.N.N>");
    assert!(!gate(&["assert_publish_ref", "v1.0.0", "", "push"]).status.success(), "unknown ref type");
}

#[test]
fn tag_must_match_the_manifest_version() {
    assert!(gate(&["assert_tag_matches_manifest", "0.1.0", "v0.1.0", "tag"]).status.success());
    let o = gate(&["assert_tag_matches_manifest", "0.1.0", "v0.1.1", "tag"]);
    assert!(!o.status.success() && err(&o).contains("skew"), "{}", err(&o));
    // not a tag: reported and skipped, never invented
    let o = gate(&["assert_tag_matches_manifest", "0.1.0", "main", "branch"]);
    assert!(o.status.success() && err(&o).contains("not applicable"));
}

#[test]
fn version_shape_and_extraction() {
    assert!(gate(&["assert_version_shape", "0.1.0"]).status.success());
    assert!(gate(&["assert_version_shape", "1.0.0-rc.1"]).status.success());
    for bad in ["", "1.2", "abc"] {
        assert!(!gate(&["assert_version_shape", bad]).status.success(), "{:?}", bad);
    }
    let o = gate(&["extract_version", "Cargo.toml"]);
    assert_eq!(String::from_utf8_lossy(&o.stdout).trim(), env!("CARGO_PKG_VERSION"));
    assert!(!gate(&["extract_version", "/nonexistent/Cargo.toml"]).status.success());
}

#[test]
fn the_repository_is_in_lockstep_for_the_current_version() {
    let v = env!("CARGO_PKG_VERSION");
    let plugin = root().join(".claude-plugin/plugin.json");
    assert!(gate(&["assert_plugin_version", v, plugin.to_str().unwrap()]).status.success());
    let o = gate(&["assert_plugin_version", "9.9.9", root().join(".claude-plugin/plugin.json").to_str().unwrap()]);
    assert!(!o.status.success() && err(&o).contains("::error::"));
    let tmp = tmpdir("rg_notes");
    let notes = tmp.join("notes.md");
    let o = gate(&["changelog_notes", v, root().join("CHANGELOG.md").to_str().unwrap(), notes.to_str().unwrap(), "tag"]);
    assert!(o.status.success(), "{}", err(&o));
    assert!(std::fs::read_to_string(&notes).unwrap().trim().len() > 20, "the release notes are the changelog section");
}

#[test]
fn changelog_notes_are_required_on_a_tag_build() {
    let tmp = tmpdir("rg_cl");
    let cl = tmp.join("CHANGELOG.md");
    std::fs::write(&cl, "# Changelog\n\n## [Unreleased]\n\n## [0.2.0] - 2026-01-01\n\n### Added\n- thing\n\n## [0.1.0]\n- old\n").unwrap();
    let notes = tmp.join("notes.md");
    let o = gate(&["extract_changelog_section", "0.2.0", cl.to_str().unwrap()]);
    let s = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(s.contains("- thing") && !s.contains("- old"), "{}", s);
    assert!(gate(&["changelog_notes", "0.2.0", cl.to_str().unwrap(), notes.to_str().unwrap(), "tag"]).status.success());
    let o = gate(&["changelog_notes", "0.3.0", cl.to_str().unwrap(), notes.to_str().unwrap(), "tag"]);
    assert!(!o.status.success() && err(&o).contains("::error::"), "missing section on a tag");
    let o = gate(&["changelog_notes", "0.3.0", cl.to_str().unwrap(), notes.to_str().unwrap(), "branch"]);
    assert!(o.status.success(), "off a tag it degrades");
    // an empty section is as bad as a missing one
    let cl2 = tmp.join("empty.md");
    std::fs::write(&cl2, "## [Unreleased]\n\n## [0.4.0]\n\n## [0.3.0]\n- x\n").unwrap();
    assert!(!gate(&["changelog_notes", "0.4.0", cl2.to_str().unwrap(), notes.to_str().unwrap(), "tag"]).status.success());
}

fn make_set(dir: &Path, tag: &str, skip: Option<&str>, extra: Option<&str>) {
    std::fs::create_dir_all(dir).unwrap();
    let wf = root().join(".github/workflows/release.yml");
    let targets = gate(&["release_targets", wf.to_str().unwrap()]);
    assert!(targets.status.success(), "{}", err(&targets));
    for t in String::from_utf8_lossy(&targets.stdout).lines() {
        let a = String::from_utf8_lossy(&gate(&["archive_name", tag, t]).stdout).trim().to_string();
        if skip == Some(t) {
            continue;
        }
        std::fs::write(dir.join(&a), "x").unwrap();
        std::fs::write(dir.join(format!("{}.sha256", a)), format!("abc  {}\n", a)).unwrap();
    }
    if let Some(e) = extra {
        std::fs::write(dir.join(e), "x").unwrap();
    }
}

#[test]
fn artifact_set_is_derived_from_the_workflow_matrix_and_must_be_exact() {
    let wf = root().join(".github/workflows/release.yml");
    let wf = wf.to_str().unwrap();
    let targets = String::from_utf8_lossy(&gate(&["release_targets", wf]).stdout).into_owned();
    let t: Vec<&str> = targets.lines().collect();
    assert_eq!(t.len(), 5, "{:?}", t);
    for want in ["x86_64-unknown-linux-musl", "aarch64-unknown-linux-musl", "x86_64-apple-darwin", "aarch64-apple-darwin", "x86_64-pc-windows-msvc"] {
        assert!(t.contains(&want), "{}", want);
    }
    assert_eq!(String::from_utf8_lossy(&gate(&["archive_name", "v1.2.3", "x86_64-pc-windows-msvc"]).stdout).trim(), "pxl-v1.2.3-x86_64-pc-windows-msvc.zip");
    assert_eq!(String::from_utf8_lossy(&gate(&["archive_name", "v1.2.3", "aarch64-apple-darwin"]).stdout).trim(), "pxl-v1.2.3-aarch64-apple-darwin.tar.gz");

    let ok = tmpdir("rg_set_ok");
    make_set(&ok, "v0.1.0", None, None);
    let o = gate(&["assert_artifact_set", ok.to_str().unwrap(), "v0.1.0", wf]);
    assert!(o.status.success(), "{}", err(&o));

    let short = tmpdir("rg_set_short");
    make_set(&short, "v0.1.0", Some("aarch64-apple-darwin"), None);
    let o = gate(&["assert_artifact_set", short.to_str().unwrap(), "v0.1.0", wf]);
    assert!(!o.status.success() && err(&o).contains("aarch64-apple-darwin"), "{}", err(&o));

    let extra = tmpdir("rg_set_extra");
    make_set(&extra, "v0.1.0", None, Some("pxl-v0.1.0-stray.tar.gz"));
    assert!(!gate(&["assert_artifact_set", extra.to_str().unwrap(), "v0.1.0", wf]).status.success());

    // archives named for another tag do not count
    let wrong = tmpdir("rg_set_wrongtag");
    make_set(&wrong, "v0.0.9", None, None);
    assert!(!gate(&["assert_artifact_set", wrong.to_str().unwrap(), "v0.1.0", wf]).status.success());
    assert!(!gate(&["assert_artifact_set", "/nonexistent-dir", "v0.1.0", wf]).status.success());
}

#[test]
fn unknown_commands_are_refused_and_no_function_calls_exit() {
    assert_eq!(gate(&["rm", "-rf"]).status.code(), Some(2));
    let text = read("scripts/release_gates.sh");
    for l in text.lines().filter(|l| !l.trim_start().starts_with('#')) {
        assert!(!l.trim_start().starts_with("exit "), "functions must not exit: {}", l);
    }
}

/// Whole stripped lines, comments removed: a workflow assertion must not be satisfied by a comment that explains it.
fn code_lines(text: &str) -> Vec<String> {
    text.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty() && !l.starts_with('#')).collect()
}

#[test]
fn release_workflow_is_wired_to_the_gates() {
    let r = code_lines(&read(".github/workflows/release.yml"));
    let has = |s: &str| r.iter().any(|l| l == s);
    assert!(has("- 'v*'"), "tag trigger");
    assert!(!r.iter().any(|l| l.starts_with("workflow_dispatch") || l.starts_with("branches:")), "tags are the only trigger");
    assert!(has("fail-fast: false"), "one red leg must not cancel the others");
    assert!(has("scripts/release_gates.sh assert_publish_ref \\"), "the ref guard");
    assert!(has("scripts/release_gates.sh assert_tag_matches_manifest \"$version\" \"$GITHUB_REF_NAME\" \"$GITHUB_REF_TYPE\""));
    assert!(has("scripts/release_gates.sh assert_plugin_version \"$version\" .claude-plugin/plugin.json"));
    assert!(has("run: scripts/release_gates.sh assert_artifact_set out \"$GITHUB_REF_NAME\""));
    assert!(has("if-no-files-found: error"));
    assert!(!r.iter().any(|l| l.contains("cargo publish") || l.contains("pypi") || l.contains("maturin")), "binaries only: no registry publish");
    // every job that names files after the ref has the guard
    let guards = r.iter().filter(|l| *l == "scripts/release_gates.sh assert_publish_ref \\").count();
    assert!(guards >= 3, "{}", guards);
    let manifest = read("Cargo.toml");
    assert!(manifest.contains("publish = false"));
    let ci = code_lines(&read(".github/workflows/ci.yml"));
    assert!(ci.iter().any(|l| l.contains("--test release_gates")), "ci runs this suite");
    assert!(ci.iter().any(|l| l.contains("shellcheck")), "ci shellchecks");
}
