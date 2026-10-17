//! `pxl skill install | uninstall | list | path`: the bundled skills equal the canonical `skills/` folder, install writes
//! them atomically under a temp HOME, refuses unmanaged folders, and stamps the binary version.
mod e2e_common;

use e2e_common::*;
use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn pxl_home(home: &Path, cwd: &Path, args: &[&str]) -> std::process::Output {
    Command::new(bin()).current_dir(cwd).env("HOME", home).env_remove("USERPROFILE").args(args).output().unwrap()
}

fn walk(dir: &Path, rel: &str, out: &mut Vec<String>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let r = if rel.is_empty() { name.clone() } else { format!("{}/{}", rel, name) };
        if e.path().is_dir() {
            walk(&e.path(), &r, out);
        } else {
            out.push(r);
        }
    }
}

#[test]
fn embedded_skills_equal_the_canonical_files_in_both_directions() {
    let mut on_disk = Vec::new();
    walk(&root().join("skills"), "", &mut on_disk);
    on_disk.sort();
    let embedded: Vec<&str> = agentsprite::skill::files().iter().map(|f| f.0).collect();
    // nothing on disk is forgotten by the binary, nothing in the binary is missing on disk
    assert_eq!(on_disk, embedded, "skills/ and the embedded table list different files");
    for (rel, text) in agentsprite::skill::files() {
        let disk = std::fs::read_to_string(root().join("skills").join(rel)).unwrap();
        assert_eq!(&disk, text, "embedded copy of skills/{} differs from the file", rel);
    }
    assert_eq!(agentsprite::skill::names().len(), 6);
}

#[test]
fn install_writes_stamped_managed_copies_and_is_idempotent() {
    let t = tmpdir("skill_install");
    let home = t.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let o = pxl_home(&home, &t, &["skill", "install"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let ver = env!("CARGO_PKG_VERSION");
    for host in [".claude", ".codex"] {
        for n in agentsprite::skill::names() {
            let d = home.join(host).join("skills").join(n);
            assert_eq!(std::fs::read_to_string(d.join(".agentsprite-managed")).unwrap(), ver);
            for (rel, text) in agentsprite::skill::skill_files(n) {
                let got = std::fs::read_to_string(d.join(rel)).unwrap();
                if rel == "SKILL.md" {
                    assert!(got.contains(&format!("installed by pxl {}", ver)), "stamp in {}", n);
                    assert_eq!(got.replace(&format!("{}\n", agentsprite::skill::stamp_line()), ""), text, "{} equals the canonical text plus the stamp", n);
                } else {
                    assert_eq!(&got, text, "{}/{}", n, rel);
                }
            }
        }
    }
    // no temp folders left behind
    let left: Vec<String> = std::fs::read_dir(home.join(".claude/skills")).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with('.')).collect();
    assert!(left.is_empty(), "{:?}", left);
    // again: an update, same result; path says current
    let o = pxl_home(&home, &t, &["skill", "install", "--host", "claude"]);
    assert!(stdout(&o).contains("updated"), "{}", stdout(&o));
    let p = stdout(&pxl_home(&home, &t, &["skill", "path"]));
    assert!(p.contains("(current)"), "{}", p);
}

#[test]
fn dry_run_and_project_scope_write_where_told() {
    let t = tmpdir("skill_dry");
    let home = t.join("home");
    let proj = t.join("proj");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&proj).unwrap();
    let o = pxl_home(&home, &proj, &["skill", "install", "--project", "--dry-run", "--host", "claude"]);
    assert!(o.status.success());
    assert!(stdout(&o).contains("would install") && stdout(&o).contains("dry run"));
    assert!(!proj.join(".claude").exists() && !home.join(".claude").exists(), "dry run writes nothing");
    let o = pxl_home(&home, &proj, &["skill", "install", "--project", "--host", "codex"]);
    assert!(o.status.success());
    assert!(proj.join(".codex/skills/pxl-operator/SKILL.md").is_file());
    assert!(!proj.join(".claude").exists() && !home.join(".codex").exists());
}

#[test]
fn unmanaged_folders_are_never_replaced_or_removed() {
    let t = tmpdir("skill_unmanaged");
    let home = t.join("home");
    let mine = home.join(".claude/skills/pxl-sprite");
    std::fs::create_dir_all(&mine).unwrap();
    std::fs::write(mine.join("SKILL.md"), "my own skill").unwrap();
    let o = pxl_home(&home, &t, &["skill", "install", "--host", "claude"]);
    assert!(!o.status.success());
    assert!(stderr(&o).contains("refusing") || stdout(&o).contains("refusing"), "{}{}", stdout(&o), stderr(&o));
    assert_eq!(std::fs::read_to_string(mine.join("SKILL.md")).unwrap(), "my own skill");
    // the refusal happens before anything is written: no other skill appeared
    assert!(!home.join(".claude/skills/pxl-operator").exists());
    let o = pxl_home(&home, &t, &["skill", "uninstall", "--host", "claude"]);
    assert!(!o.status.success());
    assert!(mine.join("SKILL.md").is_file());
}

#[test]
fn uninstall_removes_only_managed_skills() {
    let t = tmpdir("skill_uninstall");
    let home = t.join("home");
    std::fs::create_dir_all(&home).unwrap();
    assert!(pxl_home(&home, &t, &["skill", "install", "--host", "claude"]).status.success());
    let other = home.join(".claude/skills/other-skill");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("SKILL.md"), "x").unwrap();
    let dry = pxl_home(&home, &t, &["skill", "uninstall", "--host", "claude", "--dry-run"]);
    assert!(stdout(&dry).contains("would remove") && home.join(".claude/skills/pxl-sprite").exists());
    let o = pxl_home(&home, &t, &["skill", "uninstall", "--host", "claude"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    assert!(!home.join(".claude/skills/pxl-sprite").exists());
    assert!(other.join("SKILL.md").is_file(), "foreign skills stay");
    // nothing left: a second uninstall is a no-op, not an error
    assert!(pxl_home(&home, &t, &["skill", "uninstall", "--host", "claude"]).status.success());
}

#[test]
fn list_describes_every_skill_and_version_works() {
    let t = tmpdir("skill_list");
    let o = pxl_in(&t, &["skill", "list"]);
    let s = stdout(&o);
    for n in agentsprite::skill::names() {
        assert!(s.contains(n), "{}", n);
    }
    assert!(s.contains("Use when"), "descriptions are listed");
    let v = stdout(&pxl_in(&t, &["--version"]));
    assert_eq!(v.trim(), format!("pxl {}", env!("CARGO_PKG_VERSION")));
}
