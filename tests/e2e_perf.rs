//! Performance acceptance on the built binary (meaningful with `--release`; debug builds only print):
//! AC-LD5, AC-AD4, AC-CP2, AC-EX4, AC-PA4, AC-PA6. Timing tests run one at a time (shared mutex). Times are the
//! wall clock of the whole process, start-up included. Run `cargo test --release --test e2e_perf -- --nocapture`
//! to see the table.

mod e2e_common;
use agentsprite::model::{Frame, Grid, Part};
use agentsprite::pxl;
use e2e_common::*;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

static SERIAL: Mutex<()> = Mutex::new(());

/// Serialises the timing tests and warms the binary up (the first exec after a build is slower on macOS).
fn serial() -> std::sync::MutexGuard<'static, ()> {
    let g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    for _ in 0..3 {
        let _ = pxl_in(&std::env::temp_dir(), &["--version"]);
    }
    g
}

fn strict() -> bool {
    !cfg!(debug_assertions)
}

fn time_once(dir: &Path, args: &[&str]) -> (Duration, std::process::Output) {
    let t = Instant::now();
    let o = pxl_in(dir, args);
    (t.elapsed(), o)
}

/// Median of `n` runs.
fn median(dir: &Path, args: &[&str], n: usize) -> Duration {
    let mut v: Vec<Duration> = (0..n)
        .map(|_| {
            let (d, o) = time_once(dir, args);
            assert!(o.status.success(), "pxl {} failed: {}", args.join(" "), stderr(&o));
            d
        })
        .collect();
    v.sort();
    v[n / 2]
}

fn ms(d: Duration) -> String {
    format!("{:.1} ms", d.as_secs_f64() * 1000.0)
}

fn big_part(root: &Path) -> String {
    let mut part = Part::new("big", (160, 144));
    part.profile = Some("gbc-sprite".into());
    part.template = Some("templates/t.toml".into());
    let g = Grid::new(160, 144, b'.');
    part.frames.push(Frame { name: "a".into(), src: None, locked: false, grid: g.clone() });
    part.frames.push(Frame { name: "b".into(), src: None, locked: false, grid: g });
    std::fs::create_dir_all(root.join("templates")).unwrap();
    std::fs::write(root.join("templates/t.toml"), TEMPLATE).unwrap();
    std::fs::write(root.join("big.pxl"), pxl::to_string(&part)).unwrap();
    "big.pxl".into()
}

#[test]
fn ac_ad4_every_edit_is_fast_and_quiet_on_160x144() {
    let root = tmpdir("perf_edit");
    let p = big_part(&root);
    let _g = serial();
    let cmds: Vec<Vec<&str>> = vec![
        vec!["set", &p, "a", "80,70=1"],
        vec!["row", &p, "a", "10", "10", "12321"],
        vec!["patch", &p, "a", "20", "20", "1 2\n3 W"],
        vec!["line", &p, "a", "5,5", "60,40", "1"],
        vec!["rect", &p, "a", "30,30,20,12", "2"],
        vec!["fill", &p, "a", "35,35", "3"],
        vec!["replace", &p, "a", "3=2", "--region", "30,30,20,12"],
        vec!["copy", &p, "a", "b", "--region", "0,0,60,60"],
        vec!["mirror", &p, "b", "x"],
        vec!["flip", &p, "b", "y", "--region", "0,0,40,40"],
        vec!["clean", &p, "--orphans", "--jaggies", "--dry-run"],
        vec!["undo", &p],
    ];
    let mut worst = Duration::ZERO;
    for c in &cmds {
        let (d, o) = time_once(&root, c);
        assert!(o.status.success(), "pxl {}: {}", c.join(" "), stderr(&o));
        eprintln!("AC-AD4 {:<8} {}", c[0], ms(d));
        worst = worst.max(d);
    }
    // 1 pixel edits print under 15 lines
    let o = pxl_in(&root, &["set", &p, "a", "81,70=2"]);
    assert!(stdout(&o).lines().count() < 15, "{}", stdout(&o));
    let o = pxl_in(&root, &["row", &p, "a", "90", "90", "1"]);
    assert!(stdout(&o).lines().count() < 15, "{}", stdout(&o));
    let (d, _) = time_once(&root, &["show", &p, "a", "--region", "0,0,40,20"]);
    eprintln!("AC-AD4 show      {}", ms(d));
    if strict() {
        assert!(worst < Duration::from_millis(20), "AC-AD4 worst {:?}", worst);
    }
}

#[test]
fn ac_ex4_pa4_pa6_on_a_project_of_800_frames() {
    let root = tmpdir("perf_proj");
    gen_project(&root, 60, 14, 3);
    assert!(pxl_in(&root, &["load", "parts"]).status.success());
    let _g = serial();
    let (d_build, o) = time_once(&root, &["build"]);
    assert!(o.status.success(), "{}", stderr(&o));
    eprintln!("(build 60 parts / 840 frames from scratch: {})", ms(d_build));
    let d_check = median(&root, &["check"], 3);
    eprintln!("AC-EX4 check 840 frames: {}", ms(d_check));
    let d_status = median(&root, &["--json", "status"], 5);
    eprintln!("AC-PA4 status --json 60 parts: {}", ms(d_status));
    let d_ver = median(&root, &["--version"], 30);
    let d_guide = median(&root, &["guide"], 30);
    let d_stat1 = median(&root, &["status", "parts/p00.pxl"], 30);
    eprintln!("AC-PA6 cold start: --version {} | guide {} | status (1 part) {}", ms(d_ver), ms(d_guide), ms(d_stat1));
    if strict() {
        assert!(d_check < Duration::from_secs(1), "AC-EX4: {:?}", d_check);
        assert!(d_status < Duration::from_millis(100), "AC-PA4: {:?}", d_status);
        for d in [d_ver, d_guide, d_stat1] {
            assert!(d < Duration::from_millis(10), "AC-PA6: {:?}", d);
        }
    }
}
