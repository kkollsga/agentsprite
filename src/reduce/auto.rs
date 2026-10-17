//! `auto` (an exemplar whose reference is pixel-identical to the frame wins outright: approved work): run `vote`, `contour` and (when the bank is not empty) `exemplar` per frame and keep the candidate
//! with the lowest [`trial::Score::value`]
//! (`100*(1-iou) + 50*(1-nIoU) + 8*bbox + 40*oklab + 4*lost + 2*(orphans+jaggies)`). Ties go to the earlier
//! method in the order vote, contour, exemplar. `nearest` is not a candidate (it only wins on exact upscales,
//! where all methods agree).

use super::*;

pub fn reduce(prep: &Prep, img: &RgbaImage, origin: (i32, i32), part: &Part, tpl: &Template, frame: Option<&str>, bank: Option<&exemplar::Bank>) -> Result<Grid> {
    let rf = crate::model::RefFrame { img: img.clone(), origin };
    let mut base = part.clone();
    base.frames.clear();
    let name = frame.unwrap_or("frame");
    let owned;
    let bank = match bank {
        Some(b) => Some(b),
        None if !part.reduce.exemplars.is_empty() || part.frames.iter().any(|f| f.locked) => {
            owned = exemplar::Bank::build(part, tpl, frame)?;
            Some(&owned)
        }
        None => None,
    };
    if let Some(b) = bank {
        if let Some(g) = b.identical(img, frame) {
            return Ok(g.clone());
        }
    }
    let mut cands: Vec<Grid> = vec![vote_grid(prep, part, tpl)?, contour::reduce(prep, part, tpl)?];
    if let Some(b) = bank {
        if !b.is_empty() {
            cands.push(exemplar::reduce(prep, img, origin, part, tpl, frame, b)?);
        }
    }
    let mut best: Option<(f64, usize)> = None;
    for (i, g) in cands.iter().enumerate() {
        let s = trial::score_grid(&base, tpl, name, g, &rf).value();
        if best.map_or(true, |b| s < b.0 - 1e-9) {
            best = Some((s, i));
        }
    }
    Ok(cands.swap_remove(best.map_or(0, |b| b.1)))
}
