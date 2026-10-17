//! `nearest`: pure phase-aligned sampling. Each target cell takes the template key of the source pixel at the
//! centre of its block (`x0 + f/2`, `y0 + f/2`). Exact for nearest-upscaled pixel art (every pixel of a block
//! is identical), the AC-LD3 baseline. No smoothing, no feature protection.

use super::{Prep, TRANSPARENT};
use crate::img::RgbaImage;
use crate::model::{Grid, Part};
use crate::output::Result;

pub fn reduce(prep: &Prep, _img: &RgbaImage, _part: &Part) -> Result<Grid> {
    let mut g = Grid::new(prep.gw, prep.gh, TRANSPARENT);
    for cy in 0..prep.gh as i32 {
        for cx in 0..prep.gw as i32 {
            let (x0, y0, x1, y1) = prep.block(cx, cy);
            let k = prep.cls((x0 + x1) / 2, (y0 + y1) / 2);
            g.cells[cy as usize * prep.gw + cx as usize] = k;
        }
    }
    Ok(g)
}
