//! `install.sh` against a local fake release (a file:// base URL): no network. Covers the happy path with the real
//! `pxl` binary, checksum verification, version selection, the skills step and its opt-out.
#![cfg(unix)] // install.sh is the macOS/Linux installer (Windows uses install.ps1)
mod e2e_common;

use e2e_common::*;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const TARGET: &str = "x86_64-unknown-linux-musl";

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn sha256(file: &Path) -> String {
    let o = Command::new("sh").arg("-c").arg("command -v sha256sum >/dev/null 2>&1 && sha256sum \"$1\" || shasum -a 256 \"$1\"").arg("sh").arg(file).output().unwrap();
    String::from_utf8_lossy(&o.stdout).split_whitespace().next().unwrap().to_string()
}

/// A fake release folder `base/download/<tag>/pxl-<tag>-TARGET.tar.gz` (+ .sha256) holding `binary` as `pxl`, and `base/LATEST`.
fn fake_release(base: &Path, tag: &str, binary: &Path, corrupt_sum: bool) {
    let dl = base.join("download").join(tag);
    std::fs::create_dir_all(&dl).unwrap();
    let stage = base.join(format!("stage-{}", tag));
    std::fs::create_dir_all(&stage).unwrap();
    std::fs::copy(binary, stage.join("pxl")).unwrap();
    let asset = format!("pxl-{}-{}.tar.gz", tag, TARGET);
    let st = Command::new("tar").arg("-C").arg(&stage).arg("-czf").arg(dl.join(&asset)).arg("pxl").status().unwrap();
    assert!(st.success());
    let sum = if corrupt_sum { "0".repeat(64) } else { sha256(&dl.join(&asset)) };
    std::fs::write(dl.join(format!("{}.sha256", asset)), format!("{}  {}\n", sum, asset)).unwrap();
    std::fs::write(base.join("LATEST"), format!("{}\n", tag)).unwrap();
}

fn install(base: &Path, home: &Path, dir: &Path, args: &[&str]) -> Output {
    Command::new("sh")
        .arg(root().join("install.sh"))
        .args(args)
        .env("HOME", home)
        .env("PXL_INSTALL_DIR", dir)
        .env("PXL_RELEASE_BASE_URL", format!("file://{}", base.display()))
        .env("PXL_TARGET", TARGET)
        .env_remove("PXL_VERSION")
        .env_remove("PXL_INSTALL_SKILLS")
        .output()
        .unwrap()
}

#[test]
fn installs_the_latest_release_and_its_skills() {
    let t = tmpdir("inst_ok");
    let base = t.join("rel");
    fake_release(&base, "v9.9.9", Path::new(bin()), false);
    let home = t.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let dir = t.join("bin");
    let o = install(&base, &home, &dir, &[]);
    let (so, se) = (stdout(&o), stderr(&o));
    assert!(o.status.success(), "{}{}", so, se);
    assert!(so.contains("sha256 verified") && so.contains("installed:"), "{}", so);
    assert!(so.contains("is not on your PATH"), "PATH hint when the folder is not on PATH: {}", so);
    let v = stdout(&Command::new(dir.join("pxl")).arg("--version").output().unwrap());
    assert_eq!(v.trim(), format!("pxl {}", env!("CARGO_PKG_VERSION")));
    assert!(home.join(".claude/skills/pxl-operator/.agentsprite-managed").is_file(), "skills installed by default: {}", so);
    assert!(home.join(".codex/skills/pxl-sprite/SKILL.md").is_file());
    assert!(so.contains("installing the agent skills"), "{}", so);
}

#[test]
fn skills_can_be_switched_off_and_a_version_can_be_chosen() {
    let t = tmpdir("inst_opts");
    let base = t.join("rel");
    fake_release(&base, "v1.0.0", Path::new(bin()), false);
    fake_release(&base, "v2.0.0", Path::new(bin()), false); // LATEST is now v2.0.0
    let home = t.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let dir = t.join("bin");
    let o = install(&base, &home, &dir, &["--version", "1.0.0", "--no-skills"]);
    let so = stdout(&o);
    assert!(o.status.success(), "{}{}", so, stderr(&o));
    assert!(so.contains("pxl installer: v1.0.0"), "{}", so);
    assert!(so.contains("skills not installed"), "{}", so);
    assert!(!home.join(".claude").exists());
    // environment variable form, and the latest when nothing is given
    let o = Command::new("sh")
        .arg(root().join("install.sh"))
        .env("HOME", &home)
        .env("PXL_INSTALL_DIR", &dir)
        .env("PXL_INSTALL_SKILLS", "0")
        .env("PXL_RELEASE_BASE_URL", format!("file://{}", base.display()))
        .env("PXL_TARGET", TARGET)
        .output()
        .unwrap();
    assert!(stdout(&o).contains("pxl installer: v2.0.0"), "{}", stdout(&o));
    assert!(!home.join(".claude").exists());
}

#[test]
fn a_bad_checksum_installs_nothing() {
    let t = tmpdir("inst_bad");
    let base = t.join("rel");
    fake_release(&base, "v1.0.0", Path::new(bin()), true);
    let home = t.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let dir = t.join("bin");
    let o = install(&base, &home, &dir, &[]);
    assert!(!o.status.success());
    assert!(stderr(&o).contains("checksum mismatch"), "{}", stderr(&o));
    assert!(!dir.join("pxl").exists() && !home.join(".claude").exists());
}

#[test]
fn a_missing_release_and_bad_options_fail_with_a_message() {
    let t = tmpdir("inst_missing");
    let base = t.join("rel");
    std::fs::create_dir_all(&base).unwrap();
    std::fs::write(base.join("LATEST"), "v3.0.0\n").unwrap();
    let home = t.join("home");
    let o = install(&base, &home, &t.join("bin"), &[]);
    assert!(!o.status.success() && stderr(&o).contains("download failed"), "{}", stderr(&o));
    let o = install(&base, &home, &t.join("bin"), &["--bogus"]);
    assert!(!o.status.success() && stderr(&o).contains("unknown option"));
}
