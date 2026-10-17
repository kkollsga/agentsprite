//! AC-L3 (sprite part): `lint` gives the same error/warning categories as the client's `check_assets.py`.
//! Fixture parts are exported as indexed PNGs (base layer, overlay pixels hold their `under` key), a
//! matching spec.json is written next to a copy of the checker, and the real script is run.
//! Skipped (with a message) when python3, Pillow or `$PXL_CHECK_ASSETS` (an external checker script) is not set.

use agentsprite::color::Rgb;
use agentsprite::lint::{self, LintOpts};
use agentsprite::model::Part;
use agentsprite::{png, profile, pxl, template};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

const RULES: [&str; 6] = ["size", "palette_size", "rgb_step", "shared_palette", "empty_frame", "hw_cells"];

fn fx(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lint").join(rel)
}

fn category(line: &str) -> Option<&'static str> {
    let m = [
        ("canvas must be", "size"),
        ("only palette indexes", "palette_size"),
        ("multiples of 8", "rgb_step"),
        ("same colour", "shared_palette"),
        ("frame is empty", "empty_frame"),
        ("hardware sprites", "hw_cells"),
    ];
    m.iter().find(|(s, _)| line.contains(s)).map(|(_, c)| *c)
}

fn export(dir: &Path, part: &Part) {
    let tpl = template::load_for(part).unwrap();
    let base = &tpl.layers[0];
    let mut pal = vec![Rgb::new(0, 0, 0)];
    pal.extend(base.keys.iter().map(|k| lint::effective_color(part, &tpl, k.key).unwrap()));
    for f in &part.frames {
        let idx: Vec<u8> = f
            .grid
            .cells
            .iter()
            .map(|&k| {
                let b = if tpl.layer_of(k) == Some(0) { k } else { tpl.def(k).and_then(|d| d.under).unwrap_or(b'.') };
                base.keys.iter().position(|d| d.key == b).map_or(0, |p| p as u8 + 1)
            })
            .collect();
        png::write_indexed(&dir.join(format!("{}.png", f.name)), f.grid.w, f.grid.h, &idx, &pal, true).unwrap();
    }
}

#[test]
fn lint_matches_check_assets_py() {
    let script = std::path::PathBuf::from(std::env::var("PXL_CHECK_ASSETS").unwrap_or_default());
    if !script.is_file() {
        eprintln!("skipped: {} not present", script.display());
        return;
    }
    let ok = Command::new("python3").args(["-c", "import PIL"]).output().map(|o| o.status.success()).unwrap_or(false);
    if !ok {
        eprintln!("skipped: python3 or Pillow missing");
        return;
    }
    // (fixture, canvas override)
    let cases: Vec<(&str, Option<(usize, usize)>)> = vec![
        ("palette_size/neg1.pxl", None),
        ("palette_size/neg2.pxl", None),
        ("palette_size/pos1.pxl", None),
        ("palette_size/pos2.pxl", None),
        ("rgb_step/pos1.pxl", None),
        ("rgb_step/pos3.pxl", None),
        ("rgb_step/neg2.pxl", None),
        ("empty_frame/pos2.pxl", None),
        ("empty_frame/neg1.pxl", None),
        ("shared_palette/pos1.pxl", None),
        ("shared_palette/neg3.pxl", None),
        ("hw_cells/pos1.pxl", None),
        ("hw_cells/pos2.pxl", None),
        ("hw_cells/pos3.pxl", None),
        ("hw_cells/neg2.pxl", None),
        ("hw_cells/neg3.pxl", None),
        ("orphan/neg1.pxl", Some((17, 16))),
    ];
    let prof = profile::load("gbc-sprite").unwrap();
    for (rel, canvas) in cases {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("lint_checker_{}", rel.replace('/', "_")));
        let _ = std::fs::remove_dir_all(&dir);
        let out = dir.join("characters/x");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::copy(&script, dir.join("check_assets.py")).unwrap();
        let mut part = pxl::load(&fx(rel)).unwrap();
        export(&out, &part);
        if let Some(c) = canvas {
            part.size = c;
        }
        let frames: Vec<String> = part.frames.iter().map(|f| format!("{{\"file\": \"{}.png\"}}", f.name)).collect();
        std::fs::write(
            out.join("spec.json"),
            format!("{{\"type\": \"sprite\", \"groups\": [{{\"id\": \"x\", \"folder\": \"\", \"canvas\": [{}, {}], \"frames\": [{}]}}]}}", part.size.0, part.size.1, frames.join(","))).unwrap();
        let o = Command::new("python3").arg(dir.join("check_assets.py")).current_dir(&dir).output().unwrap();
        let text = String::from_utf8_lossy(&o.stdout).to_string();
        let mut theirs: BTreeSet<(String, &str)> = BTreeSet::new();
        for l in text.lines() {
            if l.starts_with("ERROR") || l.starts_with("WARNING") {
                let file = l.split(':').next().unwrap().split('/').last().unwrap().trim_end_matches(".png").to_string();
                if let Some(c) = category(l) {
                    theirs.insert((file, c));
                } else {
                    panic!("{}: unmapped checker line: {}", rel, l);
                }
            }
        }
        let opts = LintOpts { rules: RULES.iter().map(|s| s.to_string()).collect(), ..Default::default() };
        let issues = lint::lint_part_auto(&part, &prof, &opts);
        let ours: BTreeSet<(String, &str)> = issues.iter().map(|i| (i.frame.clone().unwrap_or_else(|| "?".into()), RULES.iter().find(|r| **r == i.rule).copied().unwrap())).collect();
        assert_eq!(ours, theirs, "{}: lint {:?} vs checker output:\n{}", rel, ours, text);
        // the verdict matches too: errors in the checker <=> errors in lint
        let our_err = issues.iter().any(|i| i.severity == lint::Severity::Error);
        assert_eq!(our_err, !o.status.success(), "{}: exit status differs\n{}", rel, text);
    }
}
