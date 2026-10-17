//! `shimmer` compares reference deltas: no advice where the reference also changes around the differing pixel.

use agentsprite::img::Rgba;
use agentsprite::lint::{self, LintOpts};
use agentsprite::{load, profile, pxl, template};
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lint")
}

fn shimmer_issues(edit: impl Fn(&mut agentsprite::model::RefFrame, (i32, i32))) -> usize {
    let file = root().join("shimmer/pos1.pxl");
    let part = pxl::load(&file).unwrap();
    let prof = profile::load("gbc-sprite").unwrap();
    let tpl = template::load_for(&part).unwrap();
    let mut refs: Vec<Option<agentsprite::model::RefFrame>> = part.frames.iter().map(|f| Some(load::reference(&part, &part.dir(), f).unwrap())).collect();
    let (ga, gb) = (&part.frames[0].grid, &part.frames[1].grid);
    let mut at = None;
    for y in 0..ga.h as i32 {
        for x in 0..ga.w as i32 {
            if at.is_none() && ga.get(x, y) != gb.get(x, y) {
                at = Some((x, y));
            }
        }
    }
    let at = at.expect("fixture frames differ");
    edit(refs[1].as_mut().unwrap(), at);
    let opts = LintOpts { rules: vec!["shimmer".into()], refs: true, ..Default::default() };
    lint::lint_part_refs(&part, &tpl, &prof, &opts, &refs).iter().filter(|i| i.rule == "shimmer").count()
}

#[test]
fn flagged_when_the_reference_is_unchanged() {
    assert!(shimmer_issues(|_, _| {}) > 0);
}

#[test]
fn not_flagged_when_the_reference_changes_nearby() {
    // change the reference one target pixel away from the differing output pixel (factor 2)
    let n = shimmer_issues(|rf, (x, y)| {
        let (rx, ry) = (x * 2 + 2, y * 2);
        rf.img.set(rx, ry, Rgba::opaque(255, 0, 255));
    });
    assert_eq!(n, 0);
}
