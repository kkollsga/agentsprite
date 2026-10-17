//! `pxl template suggest <part>` (DESIGN 4.3): scored colour template candidates from the part's inputs.
//!
//! Four generators run on the weighted colour histogram of every reference frame (distances in Oklab):
//! lightness tones, Oklab clusters, darkest tone plus clusters, weighted medoids. Every colour in a candidate is
//! a real source colour snapped to RGB555 (channel step 8). Candidates are scored by the mean Oklab error of
//! all source pixels against their nearest candidate colour (lower is better). The command never overwrites a
//! file: `--write FILE` creates a new template and refuses when FILE exists.

use crate::fsutil::PathSlash;
use crate::cli::SuggestArgs;
use crate::color::{self, Oklab, Rgb};
use crate::output::{Loc, Out, PxlError, Result};
use crate::{fsutil, load, profile, pxl, template};
pub mod derive;

use serde_json::json;
use std::collections::BTreeMap;

/// One distinct source colour with its pixel count and Oklab value.
struct Col {
    rgb: Rgb,
    n: u64,
    lab: Oklab,
}

pub struct Candidate {
    pub name: &'static str,
    /// Palette colours, darkest first (key 1 = outline).
    pub colors: Vec<Rgb>,
    /// Mean Oklab error x 1000 (integer, lower is better).
    pub score: u64,
}

fn d2(a: Oklab, b: Oklab) -> f64 {
    color::oklab_dist2(a, b)
}

/// Weighted medoid of `idx` (the member minimising the weighted squared distance to the others).
fn medoid(cols: &[Col], idx: &[usize]) -> Option<usize> {
    let mut best: Option<(usize, f64)> = None;
    for &i in idx {
        let cost: f64 = idx.iter().map(|&j| cols[j].n as f64 * d2(cols[i].lab, cols[j].lab)).sum();
        if best.map_or(true, |b| cost < b.1) {
            best = Some((i, cost));
        }
    }
    best.map(|b| b.0)
}

/// Deterministic weighted k-means in Oklab. Farthest point initialisation
/// starting from the heaviest colour. Returns the member lists of the clusters.
fn kmeans(cols: &[Col], k: usize) -> Vec<Vec<usize>> {
    let mut centers: Vec<Oklab> = Vec::new();
    {
        let first = (0..cols.len()).max_by(|&a, &b| cols[a].n.cmp(&cols[b].n).then(b.cmp(&a))).unwrap_or(0);
        centers.push(cols[first].lab);
    }
    while centers.len() < k.min(cols.len()) {
        let far = (0..cols.len())
            .max_by(|&a, &b| {
                let da = centers.iter().map(|&c| d2(cols[a].lab, c)).fold(f64::MAX, f64::min) * cols[a].n as f64;
                let db = centers.iter().map(|&c| d2(cols[b].lab, c)).fold(f64::MAX, f64::min) * cols[b].n as f64;
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal).then(b.cmp(&a))
            })
            .unwrap_or(0);
        centers.push(cols[far].lab);
    }
    let mut assign = vec![0usize; cols.len()];
    for _ in 0..30 {
        for (i, c) in cols.iter().enumerate() {
            let mut best = (0usize, f64::MAX);
            for (j, &ce) in centers.iter().enumerate() {
                let d = d2(c.lab, ce);
                if d < best.1 {
                    best = (j, d);
                }
            }
            assign[i] = best.0;
        }
        let mut changed = false;
        for j in 0..centers.len() {
            let (mut sl, mut sa, mut sb, mut sw) = (0.0, 0.0, 0.0, 0.0);
            for (i, c) in cols.iter().enumerate() {
                if assign[i] == j {
                    let w = c.n as f64;
                    sl += c.lab.l * w;
                    sa += c.lab.a * w;
                    sb += c.lab.b * w;
                    sw += w;
                }
            }
            if sw > 0.0 {
                let n = Oklab { l: sl / sw, a: sa / sw, b: sb / sw };
                if d2(n, centers[j]) > 1e-12 {
                    changed = true;
                }
                centers[j] = n;
            }
        }
        if !changed {
            break;
        }
    }
    let mut groups = vec![Vec::new(); centers.len()];
    for (i, &a) in assign.iter().enumerate() {
        groups[a].push(i);
    }
    groups
}

fn finish(cols: &[Col], groups: &[Vec<usize>], name: &'static str) -> Option<Candidate> {
    let mut chosen: Vec<Rgb> = Vec::new();
    for g in groups {
        if let Some(m) = medoid(cols, g) {
            chosen.push(color::snap_rgb555(cols[m].rgb));
        }
    }
    chosen.sort_by(|a, b| {
        let (la, lb) = (color::srgb_to_oklab(*a).l, color::srgb_to_oklab(*b).l);
        la.partial_cmp(&lb).unwrap_or(std::cmp::Ordering::Equal).then(a.cmp(b))
    });
    chosen.dedup();
    if chosen.is_empty() {
        return None;
    }
    let labs: Vec<Oklab> = chosen.iter().map(|c| color::srgb_to_oklab(*c)).collect();
    let (mut err, mut tot) = (0.0f64, 0.0f64);
    for c in cols {
        let m = labs.iter().map(|&l| d2(c.lab, l)).fold(f64::MAX, f64::min);
        err += m.sqrt() * c.n as f64;
        tot += c.n as f64;
    }
    let score = if tot > 0.0 { (err / tot * 1000.0).round() as u64 } else { 0 };
    Some(Candidate { name, colors: chosen, score })
}

/// Candidate palettes of `k` colours for the colour histogram `hist`.
pub fn candidates(hist: &BTreeMap<Rgb, u64>, k: usize) -> Vec<Candidate> {
    let cols: Vec<Col> = hist.iter().map(|(&rgb, &n)| Col { rgb, n, lab: color::srgb_to_oklab(rgb) }).collect();
    let k = k.max(1);
    if cols.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();

    // 1. lightness tones: equal pixel weight bins along L
    let mut order: Vec<usize> = (0..cols.len()).collect();
    order.sort_by(|&a, &b| cols[a].lab.l.partial_cmp(&cols[b].lab.l).unwrap_or(std::cmp::Ordering::Equal).then(a.cmp(&b)));
    let total: u64 = cols.iter().map(|c| c.n).sum();
    let mut groups = vec![Vec::new(); k];
    let mut acc = 0u64;
    for &i in &order {
        let mid = acc + cols[i].n / 2;
        let g = ((mid * k as u64) / total.max(1)).min(k as u64 - 1) as usize;
        groups[g].push(i);
        acc += cols[i].n;
    }
    groups.retain(|g| !g.is_empty());
    out.extend(finish(&cols, &groups, "lightness tones"));

    // 2. Oklab clusters
    let groups = kmeans(&cols, k);
    out.extend(finish(&cols, &groups, "Oklab clusters"));

    // 3. darkest tone plus clusters: the darkest colour that carries at least 1 % of the pixels is the outline
    let thr = (total / 100).max(1);
    let dark = order.iter().copied().find(|&i| cols[i].n >= thr).unwrap_or(order[0]);
    if k >= 2 {
        let rest: Vec<usize> = (0..cols.len()).filter(|&i| d2(cols[i].lab, cols[dark].lab) > 0.0004).collect();
        let sub: Vec<Col> = rest.iter().map(|&i| Col { rgb: cols[i].rgb, n: cols[i].n, lab: cols[i].lab }).collect();
        if !sub.is_empty() {
            let g = kmeans(&sub, k - 1);
            let mut groups: Vec<Vec<usize>> = g.into_iter().map(|m| m.into_iter().map(|j| rest[j]).collect()).collect();
            groups.push(vec![dark]);
            groups.retain(|g| !g.is_empty());
            out.extend(finish(&cols, &groups, "darkest tone + clusters"));
        }
    }

    // 4. weighted medoids: k-medoids with farthest-first seeds, members move to the nearest medoid
    let mut meds: Vec<usize> = vec![(0..cols.len()).max_by(|&a, &b| cols[a].n.cmp(&cols[b].n).then(b.cmp(&a))).unwrap_or(0)];
    while meds.len() < k.min(cols.len()) {
        let far = (0..cols.len())
            .max_by(|&a, &b| {
                let da = meds.iter().map(|&m| d2(cols[a].lab, cols[m].lab)).fold(f64::MAX, f64::min) * cols[a].n as f64;
                let db = meds.iter().map(|&m| d2(cols[b].lab, cols[m].lab)).fold(f64::MAX, f64::min) * cols[b].n as f64;
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal).then(b.cmp(&a))
            })
            .unwrap_or(0);
        meds.push(far);
    }
    for _ in 0..20 {
        let mut groups = vec![Vec::new(); meds.len()];
        for i in 0..cols.len() {
            let mut best = (0usize, f64::MAX);
            for (j, &m) in meds.iter().enumerate() {
                let d = d2(cols[i].lab, cols[m].lab);
                if d < best.1 {
                    best = (j, d);
                }
            }
            groups[best.0].push(i);
        }
        let new: Vec<usize> = groups.iter().enumerate().map(|(j, g)| medoid(&cols, g).unwrap_or(meds[j])).collect();
        if new == meds {
            break;
        }
        meds = new;
    }
    let mut groups = vec![Vec::new(); meds.len()];
    for i in 0..cols.len() {
        let mut best = (0usize, f64::MAX);
        for (j, &m) in meds.iter().enumerate() {
            let d = d2(cols[i].lab, cols[m].lab);
            if d < best.1 {
                best = (j, d);
            }
        }
        groups[best.0].push(i);
    }
    out.extend(finish(&cols, &groups, "weighted medoids"));

    // identical palettes from different generators are reported once (the first generator keeps the name)
    let mut uniq: Vec<Candidate> = Vec::new();
    for c in out {
        if !uniq.iter().any(|u| u.colors == c.colors) {
            uniq.push(c);
        }
    }
    uniq.sort_by(|a, b| a.score.cmp(&b.score));
    uniq
}

/// Template text for a candidate: base layer `1..k` (1 = outline) plus a `[map]` of the most frequent source
/// colours to their nearest key.
pub fn template_text(part_name: &str, c: &Candidate, hist: &BTreeMap<Rgb, u64>) -> String {
    let mut s = format!("# suggested by `pxl template suggest` ({}, error {})\nname = \"{}\"\n\n[layers.base]\n", c.name, c.score, part_name);
    s.push_str("\".\" = { role = \"transparent\" }\n");
    let roles = ["outline", "shade", "base", "light", "highlight", "tone6", "tone7", "tone8", "tone9"];
    for (i, col) in c.colors.iter().enumerate() {
        let key = char::from(b'1' + i as u8);
        let role = roles.get(i).copied().unwrap_or("extra");
        s.push_str(&format!("\"{}\" = {{ role = \"{}\", color = \"{}\" }}\n", key, role, color::to_hex(*col)));
    }
    let labs: Vec<Oklab> = c.colors.iter().map(|x| color::srgb_to_oklab(*x)).collect();
    let mut freq: Vec<(&Rgb, &u64)> = hist.iter().collect();
    freq.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    s.push_str("\n[map]\n");
    let mut lines: Vec<(Rgb, char)> = Vec::new();
    for (rgb, _) in freq.into_iter().take(24) {
        let o = color::srgb_to_oklab(*rgb);
        let mut best = (0usize, f64::MAX);
        for (i, l) in labs.iter().enumerate() {
            let d = d2(o, *l);
            if d < best.1 {
                best = (i, d);
            }
        }
        lines.push((*rgb, char::from(b'1' + best.0 as u8)));
    }
    lines.sort();
    for (rgb, key) in lines {
        s.push_str(&format!("\"{}\" = \"{}\"\n", color::to_hex(rgb), key));
    }
    s
}

pub(crate) fn write_new(f: &str, text: &str) -> Result<()> {
    let p = std::path::Path::new(f);
    if p.exists() {
        return Err(PxlError::usage(format!("{} already exists; suggest never overwrites a template", f)).with_hint("choose a new file name"));
    }
    if let Some(d) = p.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(d)?;
    }
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(p)?;
    file.write_all(text.as_bytes())?;
    Ok(())
}

/// Native resolution reference frames of a part as derivation data.
pub fn load_data(part: &crate::model::Part) -> Result<derive::Data> {
    let dir = part.dir();
    let refs = fsutil::par_map(&part.frames, |f| load::reference(part, &dir, f));
    let mut imgs = Vec::new();
    let mut orgs = Vec::new();
    for r in refs {
        let r = r?;
        orgs.push(crate::metrics::geometry(part, &r).org);
        imgs.push(r.img);
    }
    let factor = part.reduce.factor.or(part.input.factor).unwrap_or(1).max(1);
    let d = derive::Data::build_with(imgs, factor, orgs);
    if d.cols.is_empty() {
        return Err(PxlError::validation("the inputs have no opaque pixels").with_loc(Loc::file(part.path.slash())));
    }
    Ok(d)
}

fn find_template(part: &crate::model::Part, arg: &str) -> Result<std::path::PathBuf> {
    let p = std::path::Path::new(arg);
    if p.is_file() {
        return Ok(p.to_path_buf());
    }
    for cand in [arg.to_string(), format!("templates/{}", arg), format!("templates/{}.toml", arg), format!("{}.toml", arg)] {
        if let Some(f) = fsutil::find_up(&part.dir(), &cand) {
            if f.is_file() {
                return Ok(f);
            }
        }
    }
    Err(PxlError::usage(format!("template '{}' not found", arg)).with_hint("give a path such as templates/player.toml"))
}

fn run_derive(a: &SuggestArgs, part: &crate::model::Part, out: &mut Out) -> Result<()> {
    let prof = profile::load_for(part, None).ok();
    let k = prof.as_ref().map(|p| p.colors as usize).unwrap_or(3).max(1);
    let fixed = prof.as_ref().map_or(false, |p| !p.fixed_colours.is_empty());
    // a fixed palette (NES, GB): derive with free channels, then move every key to the nearest console colour
    let step = if fixed { 1 } else { prof.as_ref().map(|p| p.channel_step).unwrap_or(8).max(1) };
    let mode = derive::Mode::parse(&a.mode).ok_or_else(|| PxlError::usage(format!("--mode '{}' is not real or optimised", a.mode)))?;
    if let Some(l) = a.layers {
        if l != 1 && l != 2 {
            return Err(PxlError::usage("--layers must be 1 or 2"));
        }
    }
    let data = load_data(part)?;
    let opts = derive::Opts { k, step, layers: a.layers, mode, name: format!("{}_derived", part.name), cells: a.refine_cells };
    let d = match &a.keep_colors {
        Some(t) => {
            let tp = find_template(part, t)?;
            let tpl = template::load(&tp)?;
            derive::derive_map(&data, &tpl, &derive::Opts { name: format!("{}_map", tpl.name), ..opts })
        }
        None => derive::derive_from_data(&data, &opts),
    };
    let mut d = d;
    if let Some(p) = prof.as_ref() {
        let (t, notes) = snap_template_text(&d.text, p);
        d.text = t;
        d.trace.extend(notes);
    }
    out.line(format!("{}: {} source colours, {} frames, {} opaque pixels (weighted {:.0})", part.name, data.cols.len(), data.frames.len(), data.total_n, data.total_w));
    for t in &d.trace {
        out.line(format!("  {}", t));
    }
    for l in d.report.lines(&d.template) {
        out.line(l);
    }
    out.set("derived", d.report.to_json());
    out.set("decisions", json!(d.trace));
    match &a.write {
        Some(f) => {
            write_new(f, &d.text)?;
            out.line(format!("wrote derived template to {}", f));
            out.set("written", json!(f));
        }
        None => {
            out.line("");
            out.line("(save with --write FILE)");
        }
    }
    Ok(())
}

fn run_compare(a: &SuggestArgs, part: &crate::model::Part, out: &mut Out) -> Result<()> {
    if a.compare.len() < 2 {
        return Err(PxlError::usage("--compare needs at least two templates"));
    }
    let prof = profile::load_for(part, None).ok();
    let k = prof.as_ref().map(|p| p.colors as usize).unwrap_or(3).max(1);
    let step = prof.as_ref().map(|p| p.channel_step).unwrap_or(8).max(1);
    let data = load_data(part)?;
    let mut tpls = Vec::new();
    for t in &a.compare {
        tpls.push(template::load(&find_template(part, t)?)?);
    }
    let mut reps: Vec<derive::Report> = tpls.iter().map(|t| derive::evaluate(&data, t, k, step)).collect();
    for (r, label) in reps.iter_mut().zip(&a.compare) {
        r.template = std::path::Path::new(label).file_stem().and_then(|x| x.to_str()).unwrap_or(label).to_string();
    }
    let pairs: Vec<(&derive::Report, &crate::template::Template)> = reps.iter().zip(&tpls).collect();
    out.line(format!("{}: {} source colours, {} frames, {} opaque pixels (errors are Oklab, weighted by area and feature importance)", part.name, data.cols.len(), data.frames.len(), data.total_n));
    for l in derive::compare_lines(&pairs) {
        out.line(l);
    }
    for r in &reps {
        for i in &r.issues {
            out.line(format!("  {}: {}", r.template, i));
        }
    }
    out.set("compare", json!(reps.iter().map(|r| r.to_json()).collect::<Vec<_>>()));
    Ok(())
}

pub fn run_suggest(a: &SuggestArgs, out: &mut Out) -> Result<()> {
    let path = fsutil::resolve_part(&a.part)?;
    let part = pxl::load(&path)?;
    if !a.compare.is_empty() {
        return run_compare(a, &part, out);
    }
    if part.kind != crate::model::PartKind::Sprite && a.derive {
        return crate::tiles::palette::run(a, &part, out);
    }
    if a.derive || a.keep_colors.is_some() {
        return run_derive(a, &part, out);
    }
    let data = load_data(&part)?;
    let mut hist: BTreeMap<Rgb, u64> = BTreeMap::new();
    for c in &data.cols {
        hist.insert(c.rgb, c.n);
    }
    let k = profile::load_for(&part, None).map(|p| p.colors as usize).unwrap_or(3).max(1);
    let mut cands = candidates(&hist, k);
    cands.truncate(a.candidates.max(1));
    out.line(format!("{}: {} source colours, {} palette colours per layer", part.name, hist.len(), k));
    let mut jl = Vec::new();
    for (i, c) in cands.iter().enumerate() {
        let list: Vec<String> = c.colors.iter().enumerate().map(|(j, x)| format!("{}={}", j + 1, color::to_hex(*x))).collect();
        out.line(format!("candidate {}: {:<24} error {:>5}  {}", i + 1, c.name, c.score, list.join(" ")));
        jl.push(json!({"n": i + 1, "name": c.name, "error": c.score, "colors": c.colors.iter().map(|x| color::to_hex(*x)).collect::<Vec<_>>()}));
    }
    out.set("candidates", json!(jl));
    out.line("for the full derivation (weighted by feature importance, overlay, explicit map) use --derive");
    let pick = a.pick.max(1);
    match &a.write {
        Some(f) => {
            let c = cands.get(pick - 1).ok_or_else(|| PxlError::usage(format!("--pick {} but only {} candidates", pick, cands.len())))?;
            let text = template_text(&format!("{}_suggested", part.name), c, &hist);
            write_new(f, &text)?;
            out.line(format!("wrote candidate {} to {}", pick, f));
            out.set("written", json!(f));
        }
        None => {
            if let Some(c) = cands.first() {
                out.line("");
                out.line("best candidate as a template (save with --write FILE --pick N):");
                out.line(template_text(&format!("{}_suggested", part.name), c, &hist));
            }
        }
    }
    Ok(())
}

/// The nearest colour the console can show that `used` does not hold: from the fixed list, or the neighbours (up to two steps per
/// channel) of the snapped colour.
pub(crate) fn nearest_free(prof: &crate::profile::Profile, c: crate::color::Rgb, used: &[crate::color::Rgb]) -> Option<crate::color::Rgb> {
    let o = color::srgb_to_oklab(c);
    let mut cands: Vec<crate::color::Rgb> = Vec::new();
    if prof.fixed_colours.is_empty() {
        let s = prof.snap(c);
        let st = prof.channel_step.max(1) as i32;
        for dr in -2..=2 {
            for dg in -2..=2 {
                for db in -2..=2 {
                    let f = |v: u8, d: i32| (v as i32 + d * st).clamp(0, 255) as u8;
                    let n = crate::color::Rgb { r: f(s.r, dr), g: f(s.g, dg), b: f(s.b, db) };
                    if prof.legal(n) {
                        cands.push(n);
                    }
                }
            }
        }
    } else {
        cands = prof.fixed_colours.clone();
    }
    cands.into_iter().filter(|x| !used.contains(x)).min_by(|a, b| color::oklab_dist2(color::srgb_to_oklab(*a), o).partial_cmp(&color::oklab_dist2(color::srgb_to_oklab(*b), o)).unwrap_or(std::cmp::Ordering::Equal))
}

/// Move every key colour of a derived template through the console's colour model (a channel step or a fixed palette). Keys of one
/// layer stay distinct: a colour that is taken goes to the nearest free one.
pub fn snap_template_text(text: &str, prof: &crate::profile::Profile) -> (String, Vec<String>) {
    let mut out = Vec::new();
    let mut notes = Vec::new();
    let mut used: Vec<crate::color::Rgb> = Vec::new();
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with("[layers.") {
            used.clear();
        }
        let hex = line.find("color = \"#").map(|i| i + 9);
        if let (true, Some(i)) = (t.starts_with('"') && line.contains("role ="), hex) {
            if let Ok(c) = color::parse_hex(&line[i..i + 7]) {
                let mut s = prof.snap(c);
                if used.contains(&s) {
                    if let Some(n) = nearest_free(prof, c, &used) {
                        s = n;
                    }
                }
                used.push(s);
                if s != c {
                    notes.push(format!("{} -> {} (console colour model)", color::to_hex(c), color::to_hex(s)));
                }
                out.push(format!("{}{}{}", &line[..i], color::to_hex(s), &line[i + 7..]));
                continue;
            }
        }
        out.push(line.to_string());
    }
    let mut r = out.join("\n");
    if text.ends_with('\n') {
        r.push('\n');
    }
    (r, notes)
}
