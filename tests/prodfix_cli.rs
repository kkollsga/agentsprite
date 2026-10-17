//! Production fixes: explicit template families, manifest keys as part arguments, lock/unlock, stable staleness hashes.
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn pxl_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pxl")).current_dir(dir).args(args).output().unwrap()
}

fn proj(name: &str, family: bool, manifest_family: bool) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("prodfix_{}", name));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("parts")).unwrap();
    std::fs::create_dir_all(d.join("templates")).unwrap();
    let fx = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lint");
    for t in ["t.toml", "t_step.toml"] {
        std::fs::copy(fx.join("templates").join(t), d.join("templates").join(t)).unwrap();
    }
    for (n, src) in [("a", "pos2/a"), ("b", "pos2/b"), ("c", "pos2/c")] {
        let mut s = std::fs::read_to_string(fx.join("family").join(format!("{}.pxl", src))).unwrap().replace("\r\n", "\n");
        if !family {
            s = s.replace("family = \"fam\"\n", "");
        }
        std::fs::write(d.join("parts").join(format!("{}.pxl", n)), s).unwrap();
    }
    let fam = if manifest_family { ", family = \"fam\"" } else { "" };
    std::fs::write(
        d.join("pxl.toml"),
        format!("[parts]\nhero = {{ path = \"parts/a.pxl\"{f} }}\nsidekick = {{ path = \"parts/b.pxl\"{f} }}\nodd = {{ path = \"parts/c.pxl\"{f} }}\n", f = fam),
    )
    .unwrap();
    d
}

#[test]
fn template_check_only_compares_declared_families() {
    // no family anywhere: parts with different templates in one folder are fine
    let d = proj("nofam", false, false);
    let o = pxl_in(&d, &["lint", "--rules", "template", "parts"]);
    assert_eq!(o.status.code(), Some(0), "{}", String::from_utf8_lossy(&o.stdout));
    // family in the part header
    let d = proj("partfam", true, false);
    let o = pxl_in(&d, &["lint", "--rules", "template", "parts"]);
    assert_eq!(o.status.code(), Some(1), "{}", String::from_utf8_lossy(&o.stdout));
    assert!(String::from_utf8_lossy(&o.stdout).contains("family 'fam'"));
    // family in the manifest, one part named by its key: the siblings are still compared
    let d = proj("manfam", false, true);
    let o = pxl_in(&d, &["lint", "--rules", "template", "odd"]);
    assert_eq!(o.status.code(), Some(1), "{}", String::from_utf8_lossy(&o.stdout));
}

#[test]
fn manifest_keys_resolve_as_parts() {
    let d = proj("keys", false, false);
    let o = pxl_in(&d, &["lint", "--rules", "size", "hero"]);
    assert_eq!(o.status.code(), Some(0), "{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    assert!(String::from_utf8_lossy(&o.stdout).contains("a.pxl"));
    let o = pxl_in(&d, &["lint", "nonexistent_key"]);
    assert_eq!(o.status.code(), Some(2));
}

#[test]
fn lock_and_unlock_are_journaled_and_undoable() {
    let d = proj("lock", false, false);
    let text = |d: &Path| std::fs::read_to_string(d.join("parts/a.pxl")).unwrap();
    assert_eq!(pxl_in(&d, &["lock", "hero"]).status.code(), Some(2), "needs frames or --all");
    let o = pxl_in(&d, &["lock", "hero", "--all"]);
    assert!(o.status.success(), "{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    assert!(text(&d).contains("locked = true"));
    let o = pxl_in(&d, &["--json", "lock", "hero", "a"]);
    assert!(String::from_utf8_lossy(&o.stdout).contains("\"changed\":[]"), "already locked: nothing changes");
    assert!(pxl_in(&d, &["unlock", "hero", "a"]).status.success());
    assert!(!text(&d).contains("locked = true"));
    assert!(pxl_in(&d, &["undo", "hero"]).status.success());
    assert!(text(&d).contains("locked = true"));
    let o = pxl_in(&d, &["status", "hero"]);
    assert!(o.status.success());
}

#[test]
fn input_hash_ignores_where_the_part_lives() {
    let d = proj("hash", false, false);
    let a = agentsprite::pxl::load(&d.join("parts/a.pxl")).unwrap();
    let mut moved = a.clone();
    moved.path = d.join("elsewhere/deeper/a.pxl");
    assert_eq!(agentsprite::status::inputs_hash(&a), agentsprite::status::inputs_hash(&moved));
}
