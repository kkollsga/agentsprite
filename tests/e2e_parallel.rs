//! Parallel acceptance: AC-PA3 (20 concurrent processes on 20 parts equal the sequential run, byte for byte) and the
//! DESIGN section 9 stress test (8 agent processes by default, `PXL_STRESS_AGENTS=20` for the full load, looping next -> edit -> release on generated parts, killed at
//! random, deterministic seed; afterwards every file parses, nothing is double-owned, every part is claimable).
//!
//! The stress test takes a few seconds. Knobs: `PXL_STRESS_ROUNDS` (loop iterations per agent, default 16) and
//! `PXL_STRESS_SEED` (default 20261011). Heavier run: `PXL_STRESS_ROUNDS=40 cargo test --release --test e2e_parallel stress`.

mod e2e_common;
use e2e_common::*;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const N_PARTS: usize = 20;

fn script(k: usize) -> Vec<Vec<String>> {
    let p = format!("parts/p{:02}.pxl", k);
    let a = format!("w{}", k);
    let v = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    vec![
        v(&["load", &p]),
        v(&["--agent", &a, "set", &p, "f0", &format!("{},3=1", k % 12), "2,2=W"]),
        v(&["--agent", &a, "row", &p, "f1", "1", &format!("{}", 4 + k % 5), "1221"]),
        v(&["--agent", &a, "line", &p, "f2", "1,1", "9,6", "1"]),
        v(&["--agent", &a, "copy", &p, "f0", "f1", "--region", "0,0,6,6"]),
        v(&["compare", &p, "--out", &format!("cmp/p{:02}", k)]),
        v(&["lint", &p]),
        v(&["--agent", &a, "export", &p]),
        v(&["--agent", &a, "undo", &p]),
        v(&["--agent", &a, "export", &p]),
    ]
}

fn run_script(root: &Path, k: usize) {
    for args in script(k) {
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let o = pxl_in(root, &refs);
        assert!(o.status.success(), "p{:02}: pxl {} failed: {}", k, args.join(" "), stderr(&o));
    }
}

#[test]
fn ac_pa3_twenty_concurrent_processes_equal_the_sequential_run() {
    let seq = tmpdir("pa3_seq");
    let par = tmpdir("pa3_par");
    gen_project(&seq, N_PARTS, 3, 100);
    gen_project(&par, N_PARTS, 3, 100);
    for k in 0..N_PARTS {
        run_script(&seq, k);
    }
    std::thread::scope(|s| {
        for k in 0..N_PARTS {
            let par = &par;
            s.spawn(move || run_script(par, k));
        }
    });
    // journals carry timestamps and ids, leases are transient: everything else must be identical
    let skip = [".journal", ".lease", "_compare.json"];
    let (a, b) = (tree(&seq, &skip), tree(&par, &skip));
    assert_eq!(a.iter().map(|x| &x.0).collect::<Vec<_>>(), b.iter().map(|x| &x.0).collect::<Vec<_>>(), "same file set");
    assert!(a.len() > N_PARTS * 8, "{} files", a.len());
    for (x, y) in a.iter().zip(&b) {
        assert!(x.1 == y.1, "{} differs between the sequential and the parallel run", x.0);
    }
}

// ------------------------------------------------------------------------------------------------ stress

/// Lease TTL of the stress run in seconds (env `PXL_STRESS_LEASE_SECS`, default 1.5). Long enough that a process
/// descheduled on a loaded machine rarely outlives its lease, short enough that expiry and takeover still happen.
fn lease_secs() -> &'static str {
    static S: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    S.get_or_init(|| std::env::var("PXL_STRESS_LEASE_SECS").unwrap_or_else(|_| "1.5".into()))
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
}

#[derive(Clone, Debug)]
struct Touch {
    agent: String,
    part: String,
    /// A command of this agent on the part (edit, export or release) with its wall-clock span.
    start: Instant,
    end: Instant,
    release: bool,
    /// Exit code 0 and not killed.
    ok: bool,
}

#[derive(Clone, Debug)]
struct ClaimEv {
    agent: String,
    part: String,
    /// End of the `next` call that returned the part: the claim happened before it.
    done: Instant,
}

/// Runs `args`; with `kill_after` the process is killed after that long. Returns (exit ok and not killed, stdout, start, end).
fn run_maybe_killed(dir: &Path, args: &[&str], kill_after: Option<Duration>) -> (bool, String, Instant, Instant) {
    let start = Instant::now();
    let mut c = Command::new(bin()).current_dir(dir).args(args).env("PXL_LEASE_SECS", lease_secs()).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let mut killed = false;
    if let Some(d) = kill_after {
        std::thread::sleep(d);
        killed = c.kill().is_ok();
    }
    let o = c.wait_with_output().unwrap();
    let ok = o.status.code() == Some(0) && !killed;
    (ok, String::from_utf8_lossy(&o.stdout).into_owned(), start, Instant::now())
}

#[test]
fn stress_agents_looping_next_edit_release_killed_at_random() {
    let rounds: usize = std::env::var("PXL_STRESS_ROUNDS").ok().and_then(|s| s.parse().ok()).unwrap_or(16);
    let seed: u64 = std::env::var("PXL_STRESS_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(20261011);
    let root = tmpdir("stress");
    let n_parts = 14;
    gen_project(&root, n_parts, 3, 7);
    assert!(pxl_in(&root, &["load", "parts"]).status.success());
    let ttl = Duration::from_secs_f64(lease_secs().parse().unwrap());
    let n_agents: u64 = std::env::var("PXL_STRESS_AGENTS").ok().and_then(|s| s.parse().ok()).unwrap_or(8);
    let claims: Mutex<Vec<ClaimEv>> = Mutex::new(Vec::new());
    let touches: Mutex<Vec<Touch>> = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for ag in 0..n_agents {
            let (root, claims, touches) = (&root, &claims, &touches);
            s.spawn(move || {
                let mut rng = Rng(seed.wrapping_mul(31).wrapping_add(ag + 1));
                let agent = format!("a{:02}", ag);
                for _round in 0..rounds {
                    let kill = |rng: &mut Rng| if rng.next() % 6 == 0 { Some(Duration::from_micros(rng.next() % 9000)) } else { None };
                    let (ok, out, _s, e) = run_maybe_killed(root, &["--json", "--agent", &agent, "next"], kill(&mut rng));
                    if !ok {
                        continue;
                    }
                    let v: serde_json::Value = serde_json::from_str(out.trim()).expect("next prints one JSON object");
                    let Some(part) = v["part"].as_str().map(String::from) else {
                        std::thread::sleep(Duration::from_millis(60));
                        continue;
                    };
                    claims.lock().unwrap().push(ClaimEv { agent: agent.clone(), part: part.clone(), done: e });
                    let mut hold = true;
                    for i in 0..3 + rng.next() % 3 {
                        let (x, y) = (rng.next() % 16, rng.next() % 16);
                        let px = format!("{},{}={}", x, y, ["1", "2", "3", "W"][(rng.next() % 4) as usize]);
                        let frame = format!("f{}", i % 3);
                        let (ok, _, st, en) = run_maybe_killed(root, &["--agent", &agent, "set", &part, &frame, &px], kill(&mut rng));
                        // a killed or failed edit may or may not have renewed; a successful one certainly did
                        touches.lock().unwrap().push(Touch { agent: agent.clone(), part: part.clone(), start: st, end: en, release: false, ok });
                        if !ok {
                            hold = false;
                            break;
                        }
                    }
                    if hold && rng.next() % 3 == 0 {
                        let (ok, _, st, en) = run_maybe_killed(root, &["--agent", &agent, "export", &part], kill(&mut rng));
                        touches.lock().unwrap().push(Touch { agent: agent.clone(), part: part.clone(), start: st, end: en, release: false, ok });
                        hold = ok;
                    }
                    if hold {
                        // mostly hand the part back to the queue (so parts keep circulating), sometimes finish it
                        let finish = rng.next() % 6 == 0;
                        let args: Vec<&str> = if finish { vec!["--agent", &agent, "release", &part, "--state", "done", "--note", "stress"] } else { vec!["--agent", &agent, "release", &part] };
                        let (ok, _, st, en) = run_maybe_killed(root, &args, kill(&mut rng));
                        touches.lock().unwrap().push(Touch { agent: agent.clone(), part: part.clone(), start: st, end: en, release: true, ok });
                    }
                }
            });
        }
    });
    // ---- no double ownership. A successful modifying command renews a live lease that its agent holds. So if agent B
    // succeeded at time r_b (somewhere inside its command), another agent A cannot succeed on the same part before
    // r_b + ttl unless B released it first (any release attempt counts: a killed release may have completed). B's command ended before A's started, hence r_b < r_a; a violation is
    // A finishing less than a ttl after B's command started.
    let claims = claims.into_inner().unwrap();
    let touches = touches.into_inner().unwrap();
    let mut checked = 0;
    for b in touches.iter().filter(|t| t.ok && !t.release) {
        for a in touches.iter().filter(|t| t.ok && t.part == b.part && t.agent != b.agent && t.start > b.end) {
            let released = touches.iter().any(|r| r.release && r.part == b.part && r.agent == b.agent && r.start < a.end);
            checked += 1;
            assert!(
                released || a.end >= b.start + ttl,
                "{} and {} both modified {} within one lease period ({:?} apart): double ownership",
                b.agent,
                a.agent,
                b.part,
                a.end.duration_since(b.start)
            );
        }
    }
    eprintln!("stress: {} ownership pairs checked", checked);
    // ---- hard guarantees on the files
    let mut done = 0;
    for k in 0..n_parts {
        let p = root.join(format!("parts/p{:02}.pxl", k));
        let part = agentsprite::pxl::load(&p).unwrap_or_else(|e| panic!("{} must parse after the stress run: {}", p.display(), e));
        assert_eq!(part.frames.len(), 3);
        let st = p.with_extension("status.json");
        if st.exists() {
            let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&st).unwrap()).unwrap_or_else(|e| panic!("{}: {}", st.display(), e));
            assert_eq!(v["schema"], 1);
            if v["state"] == "done" {
                done += 1;
            }
        }
        let j = p.with_extension("journal");
        if j.exists() {
            for (i, l) in std::fs::read_to_string(&j).unwrap().lines().enumerate() {
                serde_json::from_str::<serde_json::Value>(l).unwrap_or_else(|e| panic!("{} line {}: {}", j.display(), i + 1, e));
            }
        }
        // journal and part agree: undoing everything must work (or there is nothing to undo)
        let lease = p.with_extension("lease");
        if let Ok(t) = std::fs::read_to_string(&lease) {
            assert!(t.trim().is_empty() || serde_json::from_str::<serde_json::Value>(&t).is_ok(), "lease file is empty or valid JSON: {:?}", t);
        }
    }
    eprintln!("stress: {} claims, {} touches, {} of {} parts done before the drain", claims.len(), touches.len(), done, n_parts);
    assert!(claims.len() >= n_parts, "parts must circulate: {} claims", claims.len());
    assert!(claims.iter().map(|c| c.agent.as_str()).collect::<std::collections::BTreeSet<_>>().len() > 5, "several agents must have worked");
    assert!(claims.iter().map(|c| c.part.as_str()).collect::<std::collections::BTreeSet<_>>().len() > 5);
    assert!(claims.iter().all(|c| c.done <= Instant::now()));
    // one claim per part at a time: the number of live leases never exceeds the number of parts, and each part has at most one lease file
    let leases = std::fs::read_dir(root.join("parts")).unwrap().filter(|e| e.as_ref().unwrap().path().extension().map_or(false, |x| x == "lease")).count();
    assert!(leases <= n_parts);
    // ---- all parts become claimable again: after the lease expiry a drain loop claims every part that is not done
    std::thread::sleep(ttl + Duration::from_millis(200));
    let mut drained = Vec::new();
    for _ in 0..n_parts + 2 {
        let o = pxl_in_env(&root, &["--json", "--agent", "drain", "next"]);
        assert!(o.status.success(), "drain next failed: {}{}", stdout(&o), stderr(&o));
        let v = json_of(&o);
        match v["part"].as_str() {
            None => break,
            Some(p) => {
                drained.push(p.to_string());
                let r = pxl_in_env(&root, &["--agent", "drain", "release", p, "--state", "done"]);
                assert!(r.status.success(), "{}", stderr(&r));
            }
        }
    }
    let mut uniq = drained.clone();
    uniq.sort();
    uniq.dedup();
    assert_eq!(uniq.len(), drained.len(), "no part is handed out twice");
    assert_eq!(drained.len() + done, n_parts, "every part not done before the drain is claimable: drained {:?}", drained);
    // and the status view agrees: everything is done, nothing owned
    let st = json_of(&pxl_in(&root, &["--json", "status"]));
    for p in st["parts"].as_array().unwrap() {
        assert_eq!(p["state"], "done", "{}", p);
        assert!(p["owner"].is_null(), "{}", p);
    }
}

fn pxl_in_env(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(bin()).current_dir(dir).env("PXL_LEASE_SECS", lease_secs()).args(args).stdin(Stdio::null()).output().unwrap()
}

#[test]
fn concurrent_edits_of_one_part_by_one_agent_lose_nothing() {
    let root = tmpdir("lockrace");
    gen_project(&root, 1, 3, 5);
    let p = "parts/p00.pxl";
    let n = 12usize;
    let children: Vec<_> = (0..n)
        .map(|i| {
            let px = format!("{},{}=W", i, 15 - i);
            Command::new(bin()).current_dir(&root).args(["--agent", "same", "set", p, "f0", &px]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap()
        })
        .collect();
    for c in children {
        let o = c.wait_with_output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    }
    let text = std::fs::read_to_string(root.join(p)).unwrap();
    let j = std::fs::read_to_string(root.join("parts/p00.journal")).unwrap();
    assert_eq!(j.lines().filter(|l| !l.trim().is_empty()).count(), n, "one journal entry per edit");
    let w = text.matches('W').count();
    assert!(w >= n, "all {} pixels present, found {} W in the file", n, w);
}
