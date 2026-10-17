//! AC-PA1, AC-PA2 and lease semantics. Spawns the built binary against synthetic parts in temp dirs.
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

pub fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("pxl_queue_{}_{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

pub fn part_text(name: &str, template: Option<&str>) -> String {
    let t = template.map(|t| format!("template = '{}'\n", t)).unwrap_or_default();
    format!("name = '{}'\nsize = [4, 2]\n{}[[frames]]\nname = 'a'\ngrid = '''\n....\n....\n'''\n[[frames]]\nname = 'b'\nlocked = true\ngrid = '''\n....\n....\n'''\n", name, t)
}

pub fn pxl_in(dir: &Path, envs: &[(&str, &str)], args: &[&str]) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_pxl"));
    c.current_dir(dir).args(args);
    for (k, v) in envs {
        c.env(k, v);
    }
    c.output().unwrap()
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn ac_pa1_twenty_concurrent_claims_one_winner() {
    let d = tmp("pa1");
    std::fs::write(d.join("p.pxl"), part_text("p", None)).unwrap();
    for round in 0..3 {
        let _ = std::fs::remove_file(d.join("p.lease"));
        let kids: Vec<_> = (0..20)
            .map(|i| {
                Command::new(env!("CARGO_BIN_EXE_pxl"))
                    .current_dir(&d)
                    .args(["claim", "p.pxl", "--agent", &format!("agent{}", i)])
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()
                    .unwrap()
            })
            .collect();
        let outs: Vec<Output> = kids.into_iter().map(|k| k.wait_with_output().unwrap()).collect();
        let wins = outs.iter().filter(|o| o.status.code() == Some(0)).count();
        assert_eq!(wins, 1, "round {}: exactly one claim succeeds", round);
        for o in outs.iter().filter(|o| o.status.code() != Some(0)) {
            assert_eq!(o.status.code(), Some(1));
            let e = err(o);
            assert!(e.contains("held by agent") && e.contains("since"), "got: {}", e);
        }
    }
}

#[test]
fn ac_pa2_lease_expiry_makes_part_claimable() {
    let d = tmp("pa2");
    std::fs::write(d.join("p.pxl"), part_text("p", None)).unwrap();
    let env = [("PXL_LEASE_SECS", "0.4")];
    assert!(pxl_in(&d, &env, &["claim", "p.pxl", "--agent", "A"]).status.success());
    let o = pxl_in(&d, &env, &["claim", "p.pxl", "--agent", "B"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(err(&o).contains("held by A since"));
    // modifying guard: B refused, no agent refused
    std::thread::sleep(Duration::from_millis(600));
    let o = pxl_in(&d, &env, &["claim", "p.pxl", "--agent", "B"]);
    assert!(o.status.success(), "{}", err(&o));
    let lease = std::fs::read_to_string(d.join("p.lease")).unwrap();
    assert!(lease.contains("\"agent\":\"B\""));
    // renew by non-holder fails; holder succeeds
    assert_eq!(pxl_in(&d, &env, &["renew", "p.pxl", "--agent", "A"]).status.code(), Some(1));
    assert!(pxl_in(&d, &env, &["renew", "p.pxl", "--agent", "B"]).status.success());
}

#[test]
fn ac_pa2_killed_process_leaves_valid_files() {
    let d = tmp("kill");
    std::fs::write(d.join("p.pxl"), part_text("p", None)).unwrap();
    let bin = env!("CARGO_BIN_EXE_pxl");
    // pxl itself is killed mid-command (no `sh` loop: killing a shell does not kill its children on every OS)
    for delay in [1u64, 3, 6, 10, 15, 25, 40, 70] {
        for _round in 0..3 {
            for args in [&["claim", "p.pxl", "--agent", "K"][..], &["renew", "p.pxl", "--agent", "K"], &["release", "p.pxl", "--agent", "K", "--note", "x"]] {
                let mut c = Command::new(bin)
                    .current_dir(&d)
                    .env("PXL_LEASE_SECS", "0.3")
                    .args(args)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .unwrap();
                std::thread::sleep(Duration::from_millis(delay));
                let _ = c.kill(); // already exited is fine
                let _ = c.wait();
            }
        }
        // status.json (atomic rename) is always complete, valid JSON
        if let Ok(t) = std::fs::read_to_string(d.join("p.status.json")) {
            let v: serde_json::Value = serde_json::from_str(&t).expect("status.json valid after kill");
            assert_eq!(v["schema"], 1);
        }
        // the part is untouched and a lease file is complete or (killed mid create) empty and expiring
        assert_eq!(std::fs::read_to_string(d.join("p.pxl")).unwrap(), part_text("p", None));
        if let Ok(t) = std::fs::read_to_string(d.join("p.lease")) {
            assert!(t.is_empty() || serde_json::from_str::<serde_json::Value>(&t).is_ok(), "lease: {:?}", t);
        }
        // after expiry the part is claimable again with its last complete state
        std::thread::sleep(Duration::from_millis(400));
        let o = pxl_in(&d, &[("PXL_LEASE_SECS", "0.3")], &["claim", "p.pxl", "--agent", "R"]);
        assert!(o.status.success(), "delay {}: {}", delay, err(&o));
        assert!(pxl_in(&d, &[], &["release", "p.pxl", "--agent", "R"]).status.success());
    }
}

#[test]
fn guard_semantics() {
    use agentsprite::lease;
    let d = tmp("guard");
    let p = d.join("p.pxl");
    std::fs::write(&p, part_text("p", None)).unwrap();
    // no lease: allowed with or without agent, and it does not auto-claim
    lease::guard(&p, Some("A")).unwrap();
    lease::guard(&p, None).unwrap();
    assert!(!d.join("p.lease").exists());
    lease::acquire(&p, "A").unwrap();
    lease::guard(&p, Some("A")).unwrap();
    let e = lease::guard(&p, Some("B")).unwrap_err();
    assert!(e.msg.contains("held by A since"), "{}", e.msg);
    assert_eq!(e.exit_code(), 1);
    let e = lease::guard(&p, None).unwrap_err();
    assert!(e.msg.contains("held by A"));
    assert!(e.hint.unwrap().contains("--agent"));
}

#[test]
fn stealers_race_one_winner() {
    use agentsprite::lease;
    let d = tmp("steal");
    let p = d.join("p.pxl");
    std::fs::write(&p, part_text("p", None)).unwrap();
    // an expired lease written by hand
    std::fs::write(d.join("p.lease"), r#"{"agent":"old","since":1.0,"renewed":1.0}"#).unwrap();
    let wins: Vec<bool> = std::thread::scope(|s| {
        let hs: Vec<_> = (0..16).map(|i| {
            let p = p.clone();
            s.spawn(move || lease::acquire(&p, &format!("t{}", i)).is_ok())
        }).collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert_eq!(wins.iter().filter(|w| **w).count(), 1);
    assert!(!d.join("p.lease.lock").exists());
}
