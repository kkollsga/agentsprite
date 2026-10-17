//! Quality gates of the design language (M6 R7): lint rules that cannot be argued away. They are warnings; in a project with `[project] gates = true`
//! they run on every lint and block `pxl review approve` unless the reviewer passes `--justify "..."`. `lint --rules NAME` runs them anywhere.
//!
//! * `eye_parity`: the eyes (white with a pupil) the source frame shows are the eyes the art draws, frame by frame;
//! * `identity_colour`: every identity colour of the asset (brief.toml, else detected in the input) that the source frame holds is in the art;
//! * `flat`: the light / mid / dark structure of every material of the source survives, the flat-block share (the quality harness metric)
//!   and the value range against the console style;
//! * `consistency`: the same colour persists across the frames of a character, and a material drawn the same in the source uses the same ramp
//!   as the sibling assets and the style frames;
//! * `readability`: the figure reads at 1x against the console's preview background (silhouette contrast) and keeps a value spread.

use crate::color::Rgb;
use crate::img::RgbaImage;
use crate::insight::{self, de, hex};
use crate::model::{Part, PartKind, RefFrame};
use crate::profile::Profile;
use crate::template::Template;
use crate::{asset, console, intake, load, pxl, style};
use std::path::Path;

pub const DESIGN_RULES: [&str; 5] = ["eye_parity", "identity_colour", "flat", "consistency", "readability"];
/// Not a gate: the warning that a gate had no source image to compare with. It never blocks an approval (the reviewer sees it in the lint and `check`).
pub const REFERENCE_MISSING: &str = "reference_missing";

#[derive(Clone, Debug)]
pub struct Finding {
    /// Information for the palette plan, not a design failure: never blocks an approval.
    pub info: bool,
    pub rule: &'static str,
    pub frame_idx: Option<usize>,
    pub frame: Option<String>,
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub message: String,
    pub fix: Option<String>,
}

fn home_of(part: &Part) -> Option<std::path::PathBuf> {
    crate::manifest::Manifest::find_file(&part.dir()).map(|m| asset::dir_of(&m))
}

/// Does the project of this part have the design gates on (`[project] gates = true`)?
pub fn design_project(part: &Part) -> bool {
    home_of(part).map_or(false, |h| crate::milestone::gates_on(&h))
}

/// Which design rules run: all of them in a gated project when no rules are named, else the named ones.
pub fn enabled(part: &Part, named: &[String]) -> Vec<&'static str> {
    if named.is_empty() {
        if design_project(part) {
            DESIGN_RULES.to_vec()
        } else {
            Vec::new()
        }
    } else {
        DESIGN_RULES.iter().copied().filter(|r| named.iter().any(|n| n == r)).collect()
    }
}

fn asset_name_of(part: &Part) -> String {
    if asset::is_asset_file(&part.path) {
        asset::asset_name(&asset::dir_of(&part.path))
    } else {
        part.name.clone()
    }
}

fn factor_of(part: &Part) -> usize {
    part.reduce.factor.or(part.input.factor).unwrap_or(1).max(1) as usize
}

fn opaque_colours(im: &RgbaImage) -> Vec<Rgb> {
    insight::opaque(im).into_iter().map(|p| p.2).collect()
}

/// Run the named design rules on a sprite part. Pure apart from reading reference images and the brief.
pub fn run(part: &Part, tpl: &Template, prof: &Profile, on: &[&str]) -> Vec<Finding> {
    let mut out = Vec::new();
    if on.is_empty() || part.kind != PartKind::Sprite || part.frames.is_empty() {
        return out;
    }
    let dir = part.dir();
    let loaded: Vec<crate::output::Result<RefFrame>> = part.frames.iter().map(|f| load::reference(part, &dir, f)).collect();
    // no silent skips: every frame without a source image says so, naming the part, the frame and the gates that could not run
    let needs_ref: Vec<&str> = on.iter().copied().filter(|r| DESIGN_RULES.contains(r)).collect();
    for (i, r) in loaded.iter().enumerate() {
        if let Err(e) = r {
            push(&mut out, part, REFERENCE_MISSING, Some(i), None, None, load::reference_missing_text(&part.name, &part.frames[i].name, &e.msg, &needs_ref).trim_start_matches("reference_missing: ").to_string(), None);
        }
    }
    let refs: Vec<Option<RefFrame>> = loaded.into_iter().map(|r| r.ok()).collect();
    let rendered: Vec<RgbaImage> = (0..part.frames.len()).map(|i| style::render_frame(part, Some(tpl), i)).collect();
    let f = factor_of(part);
    let has = |r: &str| on.contains(&r);
    if has("eye_parity") {
        eye_parity(part, tpl, prof, &refs, &rendered, &mut out);
    }
    if has("identity_colour") {
        identity_colour(part, prof, &refs, &rendered, f, &mut out);
    }
    if has("flat") {
        flat(part, tpl, prof, &refs, &rendered, f, &mut out);
    }
    if has("consistency") {
        consistency(part, tpl, &refs, &rendered, &mut out);
    }
    if has("readability") {
        let imgs: Vec<Option<RgbaImage>> = refs.iter().map(|r| r.as_ref().map(|rf| rf.img.clone())).collect();
        readability(part, prof, &imgs, &rendered, &mut out);
    }
    out
}

fn push(out: &mut Vec<Finding>, part: &Part, rule: &'static str, fi: Option<usize>, x: Option<i32>, y: Option<i32>, message: String, fix: Option<String>) {
    out.push(Finding { info: false, rule, frame_idx: fi, frame: fi.map(|i| part.frames[i].name.clone()), x, y, message, fix });
}

// ---------------------------------------------------------------------------------------------- eye_parity

fn eye_parity(part: &Part, tpl: &Template, _prof: &Profile, refs: &[Option<RefFrame>], rendered: &[RgbaImage], out: &mut Vec<Finding>) {
    // the minimum size of an eye white comes from style.md (2x3 cells unless the source eyes are bigger everywhere); no style.md, no size rule
    let min = home_of(part).and_then(|h| style::eye_min(&h));
    for (i, fr) in part.frames.iter().enumerate() {
        let Some(rf) = &refs[i] else { continue };
        let art = &rendered[i];
        if crate::features::Geo::of(part, rf).is_some() {
            // the same check `compare` prints as "eyes found N kept M" (features::eye_check)
            let chk = crate::features::eye_check(part, tpl, &fr.grid, rf);
            let (source, drawn) = (chk.found(), chk.kept());
            for j in chk.lost() {
                let e = &chk.eyes[j];
                let fix = if part.palette_set.is_none() {
                    crate::lint::eye_fix_pixels(part, tpl, &fr.grid, rf, e).map(|px| format!("pxl set {} {} {}", asset::asset_name(&part.dir()), fr.name, px))
                } else {
                    None
                };
                push(
                    out,
                    part,
                    "eye_parity",
                    Some(i),
                    Some(e.cells.0),
                    Some(e.cells.1),
                    format!("the source frame shows {} eye(s) (white with a pupil) but the art draws {}: the eye at cell {},{} has lost its white or its pupil", source, drawn, e.cells.0, e.cells.1),
                    fix,
                );
            }
            for (j, d) in chk.drawn.iter().enumerate() {
                let (Some((w, h)), Some(min)) = (*d, min) else { continue };
                let big = |a: i32, b: i32| w >= a && h >= b;
                if !(big(min.0 as i32, min.1 as i32) || big(min.1 as i32, min.0 as i32)) {
                    let e = &chk.eyes[j];
                    push(
                        out,
                        part,
                        "eye_parity",
                        Some(i),
                        Some(e.cells.0),
                        Some(e.cells.1),
                        format!("the eye at cell {},{} has a white of {}x{} cells, below the minimum {}x{} of style.md (eyes rule): draw it larger so it reads at 1x", e.cells.0, e.cells.1, w, h, min.0, min.1),
                        None,
                    );
                }
            }
        } else {
            let src = insight::eyes_by_pixels(rf.img.w, rf.img.h, 0.85, &insight::image_px(&rf.img));
            let n = insight::eyes_by_pixels(art.w, art.h, 0.85, &insight::image_px(art));
            if n < src {
                push(out, part, "eye_parity", Some(i), None, None, format!("the source frame shows {} eye(s) (white with a pupil) but the art draws {}", src, n), None);
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------- identity_colour

/// The identity list of the part and whether it is the project's own (brief.toml: tones are checked) or only detected on the sources (hue families only).
fn identity_list(part: &Part, refs: &[Option<RefFrame>]) -> (Vec<Rgb>, bool) {
    if let Some(h) = home_of(part) {
        if let Some((_, cols, confirmed)) = intake::identity_entry(&intake::read_brief(&h), &asset_name_of(part)) {
            let v: Vec<Rgb> = cols.iter().filter_map(|c| insight::parse_hex(c)).collect();
            // a list somebody confirmed is the list, even an empty one
            if !v.is_empty() || confirmed {
                return (v, true);
            }
        }
    }
    let all: Vec<Rgb> = refs.iter().flatten().flat_map(|rf| opaque_colours(&rf.img)).collect();
    (insight::small_saturated_colours(&all).into_iter().map(|c| c.0).collect(), false)
}

fn identity_colour(part: &Part, prof: &Profile, refs: &[Option<RefFrame>], rendered: &[RgbaImage], f: usize, out: &mut Vec<Finding>) {
    let (ident, tones) = identity_list(part, refs);
    if ident.is_empty() {
        return;
    }
    let greys = !prof.fixed_colours.is_empty() && prof.fixed_colours.iter().all(|c| insight::chroma(*c) < 0.03);
    if greys {
        return; // a grey console cannot hold a chromatic identity colour; the value structure carries it (rule flat)
    }
    for (i, fr) in part.frames.iter().enumerate() {
        let Some(rf) = &refs[i] else { continue };
        let src = opaque_colours(&rf.img);
        let art = opaque_colours(&rendered[i]);
        if art.is_empty() {
            continue;
        }
        for c in ident.iter().filter(|c| insight::lab(**c).l >= if tones { 0.30 } else { 0.35 }) {
            let n = src.iter().filter(|p| de(**p, *c) <= 8.0).count();
            // at least one cell of the source: a 6 pixel collar at factor 2 is an identity colour when the list names it
            // a list the project wrote down counts a colour from one cell of the source; a list only detected needs 3 cells or 0.3 % of the figure
            let need = if tones { (f * f).max(1) } else { (3 * f * f).max(src.len() * 3 / 1000) };
            if n < need {
                continue;
            }
            let (ch, hu, lc) = (insight::chroma(*c), insight::hue(*c), insight::lab(*c).l);
            let very_light = lc > 0.9;
            let cells = (n / (f * f)).max(1);
            // the colour family: the art holds a colour of the same hue that is not much greyer (a very light key colour is carried by the console's white)
            let same_hue: Vec<Rgb> = art.iter().copied().filter(|a| (insight::chroma(*a) >= (0.4 * ch).max(0.04) && insight::hue_gap(insight::hue(*a), hu) < 0.4) || (very_light && insight::lab(*a).l > 0.88)).collect();
            if same_hue.is_empty() {
                push(out, part, "identity_colour", Some(i), None, None, format!("identity colour {} ({}) covers about {} cell(s) of the source frame '{}' but the art holds no colour of that hue family", hex(*c), insight::colour_name(*c), cells, fr.name), None);
                continue;
            }
            // the tone: a colour of that hue within 0.10 of the lightness, so the dark teal and the light teal collar are two colours, not one
            let tone_gap = same_hue.iter().map(|a| (insight::lab(*a).l - lc).abs()).fold(f64::MAX, f64::min);
            if tones && tone_gap > 0.10 {
                let nearest = same_hue.iter().min_by(|a, b| (insight::lab(**a).l - lc).abs().partial_cmp(&(insight::lab(**b).l - lc).abs()).unwrap_or(std::cmp::Ordering::Equal)).copied().unwrap_or(same_hue[0]);
                let dir = if insight::lab(nearest).l > lc { "lighter" } else { "darker" };
                push(
                    out,
                    part,
                    "identity_colour",
                    Some(i),
                    None,
                    None,
                    format!(
                        "identity colour {} ({}) covers about {} cell(s) of the source frame '{}' but the art has no tone of it: its nearest {} colour {} is {:.2} {} (the tone structure of the material is lost)",
                        hex(*c),
                        insight::colour_name(*c),
                        cells,
                        fr.name,
                        insight::hue_name(hu),
                        hex(nearest),
                        tone_gap,
                        dir
                    ),
                    None,
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------- flat

/// Shares (largest first) of the tone bands of a list of lightness values: values closer than `gap` are one tone.
fn band_shares(mut ls: Vec<f64>, gap: f64) -> Vec<f64> {
    if ls.is_empty() {
        return Vec::new();
    }
    ls.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = ls.len() as f64;
    let mut out: Vec<f64> = Vec::new();
    let mut start = 0;
    for i in 1..=ls.len() {
        if i == ls.len() || ls[i] - ls[i - 1] > gap {
            out.push((i - start) as f64 / n);
            start = i;
        }
    }
    out.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    out
}

fn flat(part: &Part, tpl: &Template, prof: &Profile, refs: &[Option<RefFrame>], rendered: &[RgbaImage], _f: usize, out: &mut Vec<Finding>) {
    // the palettes a cell can draw with: the slots of the part's set (and whether a slot is spare), else the template's colours as one palette
    let (groups, spare): (Vec<Vec<Rgb>>, bool) = match crate::cells::resolve_for_lint(part, prof) {
        Ok(Some(r)) => (r.slots.iter().map(|s| s.2.to_vec()).collect(), (0..prof.obj_palettes as usize).any(|n| r.set.slots.get(n).map_or(true, |d| d.is_none()))), // a slot number the whole set leaves undefined is spare
        _ => (vec![tpl.keys().filter_map(|k| crate::lint::effective_color(part, tpl, k.key)).collect()], false),
    };
    let mut forced: Vec<(String, Rgb)> = Vec::new();
    for (i, fr) in part.frames.iter().enumerate() {
        let Some(rf) = &refs[i] else { continue };
        let Some(g) = crate::features::Geo::of(part, rf) else { continue };
        let f = g.f.max(1) as usize;
        let art = &rendered[i];
        let (gw, gh) = (art.w, art.h);
        let src_all = opaque_colours(&rf.img);
        // the outline and the pure whites are not materials (they have their own rules): a "grey" made of outline and eye whites is no material
        let src_px = insight::material_pixels(&src_all);
        if src_px.len() < 12 {
            continue;
        }
        let is_material = |c: Rgb| insight::lab(c).l >= 0.27 && !(insight::lab(c).l > 0.93 && insight::chroma(c) < 0.05);
        let fams = insight::families(&src_px);
        let fam_of = |c: Rgb| -> Option<usize> {
            let neutral = insight::chroma(c) < 0.05;
            fams.iter().position(|fm| fm.neutral == neutral && (neutral || insight::hue_gap(fm.hue, insight::hue(c)) < insight::HUE_TOL))
        };
        // output colours per source material (the family that holds most of the cell's source block)
        let mut per: Vec<Vec<Rgb>> = vec![Vec::new(); fams.len()];
        for y in 0..gh {
            for x in 0..gw {
                let p = art.px[y * gw + x];
                if p.a < 128 || !is_material(Rgb { r: p.r, g: p.g, b: p.b }) {
                    continue;
                }
                let mut votes = vec![0usize; fams.len()];
                for dy in 0..f {
                    for dx in 0..f {
                        let (ix, iy) = (g.org.0 + (x * f + dx) as i32, g.org.1 + (y * f + dy) as i32);
                        if ix >= 0 && iy >= 0 && (ix as usize) < rf.img.w && (iy as usize) < rf.img.h {
                            let q = rf.img.px[iy as usize * rf.img.w + ix as usize];
                            if q.a >= 128 {
                                let qc = Rgb { r: q.r, g: q.g, b: q.b };
                                if is_material(qc) {
                                    if let Some(k) = fam_of(qc) {
                                        votes[k] += 1;
                                    }
                                }
                            }
                        }
                    }
                }
                if let Some((k, n)) = votes.iter().enumerate().max_by_key(|e| *e.1) {
                    // the art colour must be of the material it is compared with: a cell the palette recoloured into another hue is a hue loss
                    // (rule identity_colour), not a tone of this material
                    let ac = Rgb { r: p.r, g: p.g, b: p.b };
                    let same = if fams[k].neutral { insight::chroma(ac) < 0.08 } else { insight::chroma(ac) >= 0.04 && insight::hue_gap(insight::hue(ac), fams[k].hue) < 0.6 };
                    if *n > 0 && same {
                        per[k].push(ac);
                    }
                }
            }
        }
        for (k, fm) in fams.iter().enumerate() {
            // a material of fewer than 16 cells (24 for the neutral one, mostly ink and shading) cannot show two tones: a tone needs a few cells to be a tone
            if fm.share < 0.08 || per[k].len() < if fm.neutral { 24 } else { 16 } {
                continue;
            }
            // the source material has two or more real tones (each holds at least 8 % of it, and they differ in lightness)...
            let src_tones = fm.tone_shares.iter().filter(|s| **s >= 0.08).count();
            if src_tones < 2 || fm.l_hi - fm.l_lo < 0.10 {
                continue;
            }
            // ...the art draws it with one tone over (nearly) all of its cells
            let shares = band_shares(per[k].iter().map(|c| insight::lab(*c).l).collect(), 0.04);
            let top = shares.first().copied().unwrap_or(1.0);
            if top >= 0.93 {
                let dom = per[k].iter().fold(std::collections::BTreeMap::new(), |mut m: std::collections::BTreeMap<Rgb, usize>, c| {
                    *m.entry(*c).or_insert(0) += 1;
                    m
                });
                let dom = dom.into_iter().max_by_key(|e| e.1).map(|e| e.0).unwrap_or(per[k][0]);
                // forced by the palette is no design failure: it is a failure only when a palette this material can use holds another tone of
                // its hue (or a slot is spare) that the art leaves unused
                let another_tone = spare
                    || groups.iter().filter(|gr| gr.contains(&dom)).any(|gr| {
                        gr.iter().any(|c2| {
                            *c2 != dom
                                && (insight::lab(*c2).l - insight::lab(dom).l).abs() >= 0.08
                                && if fm.neutral { insight::chroma(*c2) < 0.08 } else { insight::chroma(*c2) >= 0.04 && insight::hue_gap(insight::hue(*c2), fm.hue) < insight::HUE_TOL }
                        })
                    });
                if !another_tone {
                    if !forced.iter().any(|(n, c)| *n == fm.name() && *c == dom) {
                        forced.push((fm.name(), dom));
                    }
                    continue;
                }
                push(
                    out,
                    part,
                    "flat",
                    Some(i),
                    None,
                    None,
                    format!(
                        "lost structure: the {} material has {} tone(s) (light, mid, dark; {}) in the source but one flat colour ({}) over {:.0}% of its {} cells in the art",
                        fm.name(),
                        src_tones,
                        fm.tone_shares.iter().map(|s| format!("{:.0}%", s * 100.0)).collect::<Vec<_>>().join(" / "),
                        hex(dom),
                        top * 100.0,
                        per[k].len()
                    ),
                    None,
                );
            }
        }
        let cache = |x: usize, y: usize| -> Option<Rgb> {
            let p = art.px[y * gw + x];
            (p.a >= 128).then_some(Rgb { r: p.r, g: p.g, b: p.b })
        };
        let fb = insight::flat_block(&rf.img, gw, gh, g.org, f, &cache);
        if fb > 0.7 && part.variant.is_none() {
            push(out, part, "flat", Some(i), None, None, format!("flat block: a single-colour area of a hue the source does not have there covers {:.0}% of the figure", fb * 100.0), None);
        }
        let _ = fr;
    }
    if !forced.is_empty() {
        out.push(Finding {
            info: true,
            rule: "flat",
            frame_idx: None,
            frame: None,
            x: None,
            y: None,
            message: format!(
                "palette-forced one-tone material(s): the palette holds no second tone of {} that the source draws with several; a matter for the palette plan (`pxl palette plan`), not a drawing fault",
                forced.iter().map(|(n, c)| format!("{} ({})", n, hex(*c))).collect::<Vec<_>>().join(", ")
            ),
            fix: None,
        });
    }
    // value range against the console style, over the whole part
    let art: Vec<Rgb> = rendered.iter().flat_map(opaque_colours).collect();
    let src: Vec<Rgb> = refs.iter().flatten().flat_map(|rf| opaque_colours(&rf.img)).collect();
    if art.len() >= 24 && !src.is_empty() {
        let (sa, ss) = (insight::value_spread(&art), insight::value_spread(&src));
        let min = prof.style.min_value_spread;
        if sa < min && sa < 0.5 * ss {
            push(out, part, "flat", None, None, None, format!("value range {:.2} is below the console style minimum {:.2} (the source spans {:.2}): the figure lost its light and dark ends", sa, min, ss), None);
        }
    }
}

// ---------------------------------------------------------------------------------------------- consistency

#[derive(Clone, Debug)]
struct Ramp {
    hue: f64,
    n: usize,
    reps: [Rgb; 3],
}

fn ramps_of(colours: &[Rgb]) -> Vec<Ramp> {
    insight::families(colours).into_iter().filter(|f| !f.neutral && f.n >= 6).map(|f| Ramp { hue: f.hue, n: f.n, reps: f.ramp }).collect()
}

fn ramp_dist(a: &Ramp, b: &Ramp) -> f64 {
    (0..3).map(|i| de(a.reps[i], b.reps[i])).sum::<f64>() / 3.0
}

fn consistency(part: &Part, tpl: &Template, refs: &[Option<RefFrame>], rendered: &[RgbaImage], out: &mut Vec<Finding>) {
    let n = part.frames.len();
    // 1. colours the character carries in most frames
    if n >= 3 {
        let sets: Vec<std::collections::BTreeMap<Rgb, usize>> = rendered
            .iter()
            .map(|im| {
                let mut m = std::collections::BTreeMap::new();
                for c in opaque_colours(im) {
                    *m.entry(c).or_insert(0) += 1;
                }
                m
            })
            .collect();
        let areas: Vec<usize> = sets.iter().map(|s| s.values().sum()).collect();
        let max_area = areas.iter().copied().max().unwrap_or(0);
        let mut all: Vec<Rgb> = sets.iter().flat_map(|s| s.keys().copied()).collect();
        all.sort();
        all.dedup();
        for c in all {
            let near = |s: &std::collections::BTreeMap<Rgb, usize>| s.iter().filter(|(k, _)| de(**k, c) <= 6.0).map(|(_, n)| *n).sum::<usize>();
            if insight::chroma(c) < 0.10 {
                continue; // greys and whites come and go with lighting; the character's colours are the chromatic ones
            }
            let with = sets.iter().filter(|s| near(s) >= 3).count();
            if with * 3 < n * 2 || with == n {
                continue;
            }
            let (ch, hu, lc) = (insight::chroma(c), insight::hue(c), insight::lab(c).l);
            let f = factor_of(part);
            for (i, s) in sets.iter().enumerate() {
                // expected only where this frame's own source holds the colour family
                let Some(rf) = &refs[i] else { continue };
                let held = opaque_colours(&rf.img)
                    .iter()
                    .filter(|q| if ch < 0.05 { insight::chroma(**q) < 0.05 && (insight::lab(**q).l - lc).abs() < 0.2 } else { insight::chroma(**q) >= 0.4 * ch && insight::hue_gap(insight::hue(**q), hu) < 0.4 })
                    .count();
                if held >= 6 * f * f && near(s) == 0 && areas[i] * 2 >= max_area && areas[i] >= 24 {
                    push(out, part, "consistency", Some(i), None, None, format!("the character's colour {} ({}) is in {} of {} frames but not in this one", hex(c), insight::colour_name(c), with, n), None);
                }
            }
        }
    }
    // 2. the same material, the same ramp as the siblings and the style frames
    let Some(home) = home_of(part) else { return };
    if !asset::is_asset_file(&part.path) {
        return;
    }
    let me = asset_name_of(part);
    let my_out: Vec<Rgb> = rendered.iter().flat_map(opaque_colours).collect();
    let my_src: Vec<Rgb> = refs.iter().flatten().take(1).flat_map(|rf| opaque_colours(&rf.img)).collect();
    if my_src.is_empty() {
        return;
    }
    let (ra, rs) = (ramps_of(&my_out), ramps_of(&my_src));
    let all = asset::scan(&home);
    let my_group = all.iter().find(|a| a.name == me).map(|a| a.group.clone()).unwrap_or_default();
    let frames = style::style_frames(&home);
    let mut others: Vec<&asset::AssetInfo> = all.iter().filter(|a| a.name != me && (a.group == my_group || frames.iter().any(|f| f.0 == a.name))).collect();
    others.truncate(14);
    let _ = tpl;
    let mut said: Vec<(String, String)> = Vec::new();
    for o in others {
        let Ok(op) = pxl::load(&o.file) else { continue };
        if op.kind != PartKind::Sprite {
            continue;
        }
        let Ok(otpl) = crate::template::load_for(&op) else { continue };
        let odir = asset::dir_of(&o.file);
        let o_out: Vec<Rgb> = (0..op.frames.len()).flat_map(|i| opaque_colours(&style::render_frame(&op, Some(&otpl), i))).collect();
        let o_src: Vec<Rgb> = op.frames.iter().find_map(|fr| load::reference(&op, &odir, fr).ok()).map(|rf| opaque_colours(&rf.img)).unwrap_or_default();
        if o_src.is_empty() {
            continue;
        }
        let (oa, os) = (ramps_of(&o_out), ramps_of(&o_src));
        for a in &ra {
            for b in &oa {
                if insight::hue_gap(a.hue, b.hue) > 0.35 || ramp_dist(a, b) <= 16.0 {
                    continue;
                }
                // same material in the sources?
                let sa = rs.iter().find(|r| insight::hue_gap(r.hue, a.hue) < 0.6);
                let sb = os.iter().find(|r| insight::hue_gap(r.hue, b.hue) < 0.6);
                if let (Some(sa), Some(sb)) = (sa, sb) {
                    if ramp_dist(sa, sb) <= 8.0 {
                        let name = insight::hue_name(a.hue).to_string();
                        if said.iter().any(|s| s.0 == name && s.1 == o.name) {
                            continue;
                        }
                        said.push((name.clone(), o.name.clone()));
                        push(
                            out,
                            part,
                            "consistency",
                            None,
                            None,
                            None,
                            format!(
                                "the {} material is drawn with the ramp {} / {} / {} here but {} / {} / {} in '{}', while the source colours match: use one ramp",
                                name,
                                hex(a.reps[0]),
                                hex(a.reps[1]),
                                hex(a.reps[2]),
                                hex(b.reps[0]),
                                hex(b.reps[1]),
                                hex(b.reps[2]),
                                o.name
                            ),
                            None,
                        );
                    }
                }
            }
        }
        let _ = (a_n(&ra), a_n(&oa));
    }
}

fn a_n(r: &[Ramp]) -> usize {
    r.iter().map(|x| x.n).sum()
}

// ---------------------------------------------------------------------------------------------- readability

fn readability(part: &Part, prof: &Profile, refs: &[Option<RgbaImage>], rendered: &[RgbaImage], out: &mut Vec<Finding>) {
    let pc = console::project_console(&part.dir());
    let id = part.console.clone().unwrap_or(pc.id);
    let bg = console::info(&id, Some(&part.dir())).ok().and_then(|i| insight::parse_hex(&i.preview_bg));
    for (i, im) in rendered.iter().enumerate() {
        let px = insight::opaque(im);
        if px.len() < 16 {
            continue;
        }
        let cols: Vec<Rgb> = px.iter().map(|p| p.2).collect();
        let spread = insight::value_spread(&cols);
        let floor = (prof.style.min_value_spread * 0.25).max(0.12);
        let distinct = { let mut d = cols.clone(); d.sort(); d.dedup(); d.len() };
        // judged against the source's own spread: a dark source frame may read dark in the art too
        let src_spread = refs[i].as_ref().map(|r| insight::value_spread(&opaque_colours(r)));
        if distinct >= 2 && px.len() >= 40 && spread < floor && src_spread.map_or(true, |s| s >= floor && spread < 0.5 * s && s - spread > 0.1) {
            push(out, part, "readability", Some(i), None, None, format!("squint test: the figure's tones span only {:.2} of lightness (at least {:.2} is needed to read at 1x)", spread, floor), None);
        }
        if let Some(bg) = bg {
            let ol = insight::outline(im);
            let edge: Vec<Rgb> = {
                let (w, h) = (im.w as i32, im.h as i32);
                let op = |x: i32, y: i32| x >= 0 && y >= 0 && x < w && y < h && im.px[(y * w + x) as usize].a >= 128;
                px.iter().filter(|p| [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|(dx, dy)| !op(p.0 + dx, p.1 + dy))).map(|p| p.2).collect()
            };
            if !edge.is_empty() {
                let close = edge.iter().filter(|c| de(**c, bg) < 12.0).count() as f64 / edge.len() as f64;
                let src_close = refs[i].as_ref().map(|r| {
                    let (w, h) = (r.w as i32, r.h as i32);
                    let op = |x: i32, y: i32| x >= 0 && y >= 0 && x < w && y < h && r.px[(y * w + x) as usize].a >= 128;
                    let e: Vec<Rgb> = insight::opaque(r).into_iter().filter(|p| [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|(dx, dy)| !op(p.0 + dx, p.1 + dy))).map(|p| p.2).collect();
                    e.iter().filter(|c| de(**c, bg) < 12.0).count() as f64 / e.len().max(1) as f64
                });
                if close > 0.6 && ol.presence < 0.3 && src_close.map_or(true, |s| close > s + 0.35) {
                    push(out, part, "readability", Some(i), None, None, format!("the silhouette edge is within dE 12 of the console preview background {} on {:.0}% of its length: it disappears at 1x", hex(bg), close * 100.0), None);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------- gate findings

/// All design findings of an asset file, whatever the project setting (used by `review approve` and the checklist).
pub fn gate_findings(file: &Path) -> Vec<Finding> {
    let Ok(part) = pxl::load(file) else { return Vec::new() };
    let Ok(tpl) = crate::template::load_for(&part) else { return Vec::new() };
    let Ok(prof) = crate::profile::load_for(&part, None) else { return Vec::new() };
    run(&part, &tpl, &prof, &DESIGN_RULES)
}

/// `review approve` and `approve`: in a gated project the design warnings block the approval unless the reviewer justifies them in writing.
/// Returns the text to append to the verdict note.
pub fn approval_gate(file: &Path, justify: Option<&str>) -> crate::output::Result<String> {
    let home = crate::tasks::home_of(file);
    if !crate::milestone::gates_on(&home) || !asset::is_asset_file(file) {
        return Ok(String::new());
    }
    let findings: Vec<Finding> = gate_findings(file).into_iter().filter(|f| f.rule != REFERENCE_MISSING && !f.info).collect();
    if findings.is_empty() {
        return Ok(String::new());
    }
    let mut rules: Vec<&str> = findings.iter().map(|f| f.rule).collect();
    rules.dedup();
    match justify.map(str::trim).filter(|j| !j.is_empty()) {
        Some(j) => Ok(format!(" [justified design warnings ({}): {}]", rules.join(", "), j)),
        None => {
            let list: Vec<String> = findings.iter().take(5).map(|f| format!("{}{}: {}", f.rule, f.frame.as_ref().map_or(String::new(), |fr| format!(" [{}]", fr)), f.message)).collect();
            Err(crate::output::PxlError::validation(format!("approval blocked by {} design warning(s): {}{}", findings.len(), list.join(" | "), if findings.len() > 5 { " | ..." } else { "" }))
                .with_hint("fix them (`pxl lint ASSET` prints ready-to-run fixes), or pass --justify \"why this is acceptable\" and the reason is kept with the verdict"))
        }
    }
}
