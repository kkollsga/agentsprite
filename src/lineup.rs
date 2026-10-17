//! `pxl lineup PART:FRAMES [PART:FRAMES ...]`: frames of several parts side by side on one sheet, each at 1x and
//! zoomed, labelled `part/frame`, to judge whether textures and neighbouring pieces repeat and line up.
//!
//! * `PART:a,b,c` names frames, `PART:all` or just `PART` takes every frame.
//! * Items flow left to right and wrap; pages are at most `compare::MAX_PX` (1568 px) in both directions and are
//!   trimmed to their content. The zoom is automatic (2..8, as large as fits) or `--zoom N`.
//! * `--on #rrggbb` sets the backdrop behind transparent cells (default a grey checker); `--region x,y,w,h`
//!   crops every item (needed for frames too large for a page, such as tile sheets).
//! * Deterministic: the same parts and options give byte-identical PNGs.

use crate::fsutil::PathSlash;
use crate::cli::LineupArgs;
use crate::compare::{self, BG, DIM, HEADER_H, LABEL_H, MARGIN, MAX_PX, PLATE, TEXT};
use crate::img::{Rgba, RgbaImage};
use crate::model::Part;
use crate::output::{Out, PxlError, Result};
use crate::template::{self, Template};
use crate::{color, fsutil, pxl, show};
use serde_json::json;
use std::path::PathBuf;

const GAP: usize = 4;
const CELL_GAP: usize = 6;
const FS: usize = 2;

struct Item {
    label: String,
    img: RgbaImage,
}

fn render(part: &Part, tpl: Option<&Template>, fi: usize, region: Option<(i32, i32, usize, usize)>) -> RgbaImage {
    let g = &part.frames[fi].grid;
    let (rx, ry, rw, rh) = region.unwrap_or((0, 0, g.w, g.h));
    let mut im = RgbaImage::new(rw, rh);
    for y in 0..rh {
        for x in 0..rw {
            let k = g.get(rx + x as i32, ry + y as i32);
            if k != b'.' {
                let c = compare::key_color(part, tpl, k);
                im.px[y * rw + x] = Rgba::opaque(c.r, c.g, c.b);
            }
        }
    }
    im
}

/// Composite over the backdrop in screen space: a solid colour, or a checker of 4 px squares.
fn backdrop(im: &RgbaImage, on: Option<Rgba>) -> RgbaImage {
    let mut o = RgbaImage::new(im.w, im.h);
    for y in 0..im.h {
        for x in 0..im.w {
            let p = im.px[y * im.w + x];
            o.px[y * im.w + x] = if p.a > 0 {
                p
            } else if let Some(c) = on {
                c
            } else if ((x / 4) + (y / 4)) % 2 == 0 {
                compare::CHECK_A
            } else {
                compare::CHECK_B
            };
        }
    }
    o
}

fn parse_spec(spec: &str) -> (String, Option<Vec<String>>) {
    match spec.rsplit_once(':') {
        Some((p, f)) if !p.is_empty() && !f.is_empty() && !f.contains('/') && !f.contains('\\') => {
            if f == "all" || f == "*" {
                (p.to_string(), None)
            } else {
                (p.to_string(), Some(f.split(',').filter(|s| !s.is_empty()).map(|s| s.to_string()).collect()))
            }
        }
        _ => (spec.to_string(), None),
    }
}

pub fn run(a: &LineupArgs, out: &mut Out) -> Result<()> {
    let on = match &a.on {
        Some(h) => {
            let c = color::parse_hex(h).map_err(|_| PxlError::usage(format!("--on '{}' is not #rrggbb", h)))?;
            Some(Rgba::opaque(c.r, c.g, c.b))
        }
        None => None,
    };
    if a.parts.is_empty() {
        return Err(PxlError::usage("lineup needs at least one PART:FRAMES").with_hint("pxl lineup wall_a:1,2 wall_b:all --zoom 4"));
    }
    let mut items: Vec<Item> = Vec::new();
    let mut first_dir: Option<PathBuf> = None;
    for spec in &a.parts {
        let (pname, frames) = parse_spec(spec);
        let path = fsutil::resolve_part(&pname)?;
        let part = pxl::load(&path)?;
        first_dir.get_or_insert_with(|| part.dir());
        let tpl = if part.template.is_some() { template::load_for(&part).ok() } else { None };
        let names: Vec<String> = match frames {
            Some(v) => v,
            None => part.frames.iter().map(|f| f.name.clone()).collect(),
        };
        for n in names {
            let fi = part.frame_index(&n).ok_or_else(|| crate::cli::unknown_frame(&part, &n))?;
            let g = &part.frames[fi].grid;
            let region = match a.region {
                Some(r) => Some(show::clip_region(r, g.w, g.h).ok_or_else(|| PxlError::usage(format!("--region {},{},{},{} is outside frame {}/{}", r.x, r.y, r.w, r.h, part.name, n)))?),
                None => None,
            };
            items.push(Item { label: format!("{}/{}", part.name, n), img: render(&part, tpl.as_ref(), fi, region) });
        }
    }
    if items.is_empty() {
        return Err(PxlError::usage("no frames selected"));
    }
    let maxw = items.iter().map(|i| i.img.w).max().unwrap_or(1);
    let maxh = items.iter().map(|i| i.img.h).max().unwrap_or(1);
    let room_w = MAX_PX - 2 * MARGIN;
    let room_h = MAX_PX - 2 * MARGIN - HEADER_H;
    let fits = |z: usize| maxw * (1 + z) + GAP <= room_w && maxh * z + LABEL_H <= room_h;
    let zoom = match a.zoom {
        Some(z) if z >= 1 => {
            if !fits(z) {
                return Err(PxlError::usage(format!("zoom {} does not fit a {} px page for the largest item ({}x{})", z, MAX_PX, maxw, maxh)).with_hint("lower --zoom or crop with --region x,y,w,h"));
            }
            z
        }
        Some(_) => return Err(PxlError::usage("--zoom must be at least 1")),
        None => {
            let mut z = (128 / maxw.max(maxh)).clamp(2, 8);
            while z > 1 && !fits(z) {
                z -= 1;
            }
            if !fits(z) {
                return Err(PxlError::usage(format!("the largest item ({}x{}) does not fit a {} px page", maxw, maxh, MAX_PX)).with_hint("crop with --region x,y,w,h"));
            }
            z
        }
    };
    // cells: label bar over [1x][gap][zoomed]
    struct Cell {
        x: usize,
        y: usize,
        w: usize,
        h: usize,
    }
    let mut cells: Vec<(usize, Cell)> = Vec::new(); // (page, cell)
    let (mut page, mut x, mut y, mut row_h) = (0usize, MARGIN, MARGIN + HEADER_H, 0usize);
    for it in &items {
        let w = (it.img.w * (1 + zoom) + GAP).max(compare::text_w(&it.label, FS).min(room_w) + 6);
        let h = LABEL_H + it.img.h * zoom;
        if x + w > MAX_PX - MARGIN && x > MARGIN {
            x = MARGIN;
            y += row_h + CELL_GAP;
            row_h = 0;
        }
        if y + h > MAX_PX - MARGIN && y > MARGIN + HEADER_H {
            page += 1;
            x = MARGIN;
            y = MARGIN + HEADER_H;
            row_h = 0;
        }
        cells.push((page, Cell { x, y, w, h }));
        x += w + CELL_GAP;
        row_h = row_h.max(h);
    }
    let pages = page + 1;
    let mut imgs = Vec::new();
    for p in 0..pages {
        let (mut pw, mut ph) = (0, 0);
        for (cp, c) in &cells {
            if *cp == p {
                pw = pw.max(c.x + c.w + MARGIN);
                ph = ph.max(c.y + c.h + MARGIN);
            }
        }
        let title = format!("lineup {}x  page {}/{}", zoom, p + 1, pages);
        let pw = pw.max(compare::text_w(&title, FS) + 2 * MARGIN);
        let mut cv = RgbaImage::filled(pw, ph, BG);
        compare::draw_text(&mut cv, MARGIN, (HEADER_H - 5 * FS) / 2, &title, FS, TEXT);
        for (it, (cp, c)) in items.iter().zip(&cells) {
            if *cp != p {
                continue;
            }
            compare::fill(&mut cv, c.x, c.y, c.w, c.h, PLATE);
            let maxc = (c.w.saturating_sub(6) + FS) / (4 * FS);
            compare::draw_text(&mut cv, c.x + 3, c.y + (LABEL_H - 5 * FS) / 2, &compare::truncate_chars(&it.label, maxc.max(1)), FS, TEXT);
            compare::paste(&mut cv, &backdrop(&it.img, on), c.x, c.y + LABEL_H);
            compare::paste(&mut cv, &backdrop(&it.img.upscale(zoom), on), c.x + it.img.w + GAP, c.y + LABEL_H);
            let _ = DIM;
        }
        imgs.push(cv);
    }
    let base = match &a.out {
        Some(o) => PathBuf::from(o),
        None => first_dir.unwrap_or_else(|| PathBuf::from(".")).join("preview").join("lineup.png"),
    };
    let base = if base.extension().map_or(false, |e| e.eq_ignore_ascii_case("png")) { base } else { base.join("lineup.png") };
    let stem = base.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "lineup".into());
    let dir = base.parent().map(|d| d.to_path_buf()).unwrap_or_default();
    let mut files = Vec::new();
    for (i, im) in imgs.iter().enumerate() {
        let f = if i == 0 { dir.join(format!("{}.png", stem)) } else { dir.join(format!("{}-{}.png", stem, i + 1)) };
        show::write_png(&f, im)?;
        out.line(format!("{}  ({}x{})", f.slash(), im.w, im.h));
        files.push(json!({"file": f.slash(), "w": im.w, "h": im.h}));
    }
    out.line(format!("lineup: {} item(s), zoom {}x, {} page(s)", items.len(), zoom, pages));
    out.set("items", json!(items.iter().map(|i| i.label.clone()).collect::<Vec<_>>()));
    out.set("zoom", json!(zoom));
    out.set("pages", json!(files));
    Ok(())
}
