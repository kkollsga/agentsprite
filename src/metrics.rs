//! Owner: compare worker (PLAN.md). Per-frame comparison metrics (DESIGN 5.3).
//!
//! All shape metrics are cell-level, the AC-LD4 definition of `reduce::silhouette_vs_ref`: a reference cell is
//! "opaque" when at least half of its `factor` x `factor` block has alpha >= 128; an output cell is opaque
//! when its key is not `.`.
//!
//! `score` (higher = worse, what `pxl next` ranks by):
//! `100*(1-iou) + 8*max(0,bbox_max-1) + 40*oklab + 4*lost_details + 40*lost_eyes + 3*eye_err + 1.5*share_deficit + 3*lint_errors + 1*lint_warnings`
//! with `bbox_max` the largest absolute bbox edge delta in cells and `oklab` the mean Oklab distance.

use crate::color::{self, Rgb};
use crate::lint::{self, LostFeature};
use crate::model::{Frame, Part, RefFrame};
use crate::template::Template;

pub const TRANSPARENT: u8 = b'.';
/// Score cost of one reference eye the output does not draw (white + pupil on the gaze side): more than the
/// silhouette and colour differences of a hand drawn frame.
pub const EYE_PENALTY: f64 = 40.0;
/// Score cost per cell where the eye drawing differs from the reference eye (`features::eye_mismatch`).
pub const EYE_CELL: f64 = 3.0;
/// Score cost per cell an identity colour (salient key) is short of 75 % of its reference share.
pub const SHARE_CELL: f64 = 1.5;
pub const SCORE_FORMULA: &str = "100*(1-iou) + 8*max(0,bbox_max-1) + 40*oklab + 4*lost_details + 40*lost_eyes + 3*eye_err + 1.5*share_deficit + 3*lint_errors + 1*lint_warnings";

/// Reference geometry of a part: factor and the reference-pixel position of cell (0,0).
#[derive(Clone, Copy, Debug)]
pub struct Geometry {
    pub f: i32,
    pub org: (i32, i32),
    pub gw: usize,
    pub gh: usize,
}

pub fn geometry(part: &Part, rf: &RefFrame) -> Geometry {
    let f = part.reduce.factor.or(part.input.factor).unwrap_or(1).max(1) as i32;
    let ph = part.reduce.phase.unwrap_or((0, 0));
    let org = (rf.origin.0 - f * part.origin.0 + ph.0, rf.origin.1 - f * part.origin.1 + ph.1);
    Geometry { f, org, gw: part.size.0, gh: part.size.1 }
}

#[derive(Clone, Debug)]
pub struct FrameMetrics {
    pub iou: f64,
    /// Output edge minus reference edge in cells: `[left, top, right, bottom]` (bbox edges inclusive).
    pub bbox: [i32; 4],
    pub bbox_max: i32,
    /// Mean Oklab distance over cells opaque in both (None without a template or overlap).
    pub oklab: Option<f64>,
    /// Lost details and lost eyes (`kind` tells them apart).
    pub lost: Vec<LostFeature>,
    pub eyes_found: usize,
    pub eyes_kept: usize,
    /// Cells that differ from the reference eye at target scale.
    pub eye_err: usize,
    /// Output vs reference cells of the salient (identity colour) keys.
    pub shares: Vec<crate::features::KeyShare>,
    pub errors: u32,
    pub warnings: u32,
    pub score: f64,
    /// Per cell: 0 = neither, 1 = both, 2 = reference only (red), 3 = output only (green).
    pub diff: Vec<u8>,
    pub ref_cells: usize,
    pub out_cells: usize,
}

/// Mean colour of the opaque pixels of a reference block.
pub fn block_color(rf: &RefFrame, g: &Geometry, cx: i32, cy: i32) -> Option<Rgb> {
    let (mut n, mut sr, mut sg, mut sb) = (0u32, 0u32, 0u32, 0u32);
    for j in 0..g.f {
        for i in 0..g.f {
            let p = rf.img.get(g.org.0 + g.f * cx + i, g.org.1 + g.f * cy + j);
            if p.a >= 128 {
                n += 1;
                sr += p.r as u32;
                sg += p.g as u32;
                sb += p.b as u32;
            }
        }
    }
    if n == 0 {
        None
    } else {
        Some(Rgb { r: ((sr + n / 2) / n) as u8, g: ((sg + n / 2) / n) as u8, b: ((sb + n / 2) / n) as u8 })
    }
}

pub fn compute(part: &Part, tpl: Option<&Template>, frame: &Frame, rf: &RefFrame, errors: u32, warnings: u32) -> FrameMetrics {
    let g = geometry(part, rf);
    let cov = crate::reduce::ref_coverage(rf, part);
    let grid = &frame.grid;
    let (gw, gh) = (g.gw.min(grid.w), g.gh.min(grid.h));
    let mut diff = vec![0u8; g.gw * g.gh];
    let (mut inter, mut uni, mut nref, mut nout) = (0usize, 0usize, 0usize, 0usize);
    let mut ob = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    let mut rb = ob;
    let (mut esum, mut en) = (0.0f64, 0usize);
    for y in 0..gh {
        for x in 0..gw {
            let o = grid.cells[y * grid.w + x] != TRANSPARENT;
            let r = cov[y * g.gw + x] >= 50;
            diff[y * g.gw + x] = match (o, r) {
                (true, true) => 1,
                (false, true) => 2,
                (true, false) => 3,
                _ => 0,
            };
            let (xi, yi) = (x as i32, y as i32);
            if o {
                nout += 1;
                ob = (ob.0.min(xi), ob.1.min(yi), ob.2.max(xi), ob.3.max(yi));
            }
            if r {
                nref += 1;
                rb = (rb.0.min(xi), rb.1.min(yi), rb.2.max(xi), rb.3.max(yi));
            }
            if o && r {
                inter += 1;
                if let (Some(t), Some(rc)) = (tpl, block_color(rf, &g, xi, yi)) {
                    if let Some(oc) = lint::effective_color(part, t, grid.cells[y * grid.w + x]) {
                        esum += color::oklab_dist2(color::srgb_to_oklab(oc), color::srgb_to_oklab(rc)).sqrt();
                        en += 1;
                    }
                }
            }
            if o || r {
                uni += 1;
            }
        }
    }
    let iou = if uni == 0 { 1.0 } else { inter as f64 / uni as f64 };
    let bbox = if ob.0 == i32::MAX || rb.0 == i32::MAX {
        if ob.0 == rb.0 { [0; 4] } else { [99; 4] }
    } else {
        [ob.0 - rb.0, ob.1 - rb.1, ob.2 - rb.2, ob.3 - rb.3]
    };
    let bbox_max = bbox.iter().map(|d| d.abs()).max().unwrap_or(0);
    let oklab = if en > 0 { Some(esum / en as f64) } else { None };
    let mut lost = match tpl {
        Some(t) => lint::lost_features(part, t, frame, rf),
        None => Vec::new(),
    };
    let (eyes_found, lost_eyes) = match tpl {
        Some(t) => lint::eye_losses(part, t, grid, rf),
        None => (0, Vec::new()),
    };
    let eyes_kept = eyes_found - lost_eyes.len();
    let shares = tpl.map_or(Vec::new(), |t| crate::features::colour_shares(part, t, grid, rf));
    let share_def: f64 = shares.iter().map(|s| s.deficit()).sum();
    let eye_err = tpl.map_or(0, |t| lint::eye_error(part, t, grid, rf));
    let n_details = lost.len();
    lost.extend(lost_eyes.into_iter().map(|e| e.0));
    lost.sort_by_key(|l| (l.y, l.x));
    let score = 100.0 * (1.0 - iou)
        + 8.0 * (bbox_max - 1).max(0) as f64
        + 40.0 * oklab.unwrap_or(0.0)
        + 4.0 * n_details as f64
        + EYE_PENALTY * (eyes_found - eyes_kept) as f64
        + EYE_CELL * eye_err as f64
        + SHARE_CELL * share_def
        + 3.0 * errors as f64
        + warnings as f64;
    FrameMetrics { iou, bbox, bbox_max, oklab, lost, eyes_found, eyes_kept, eye_err, shares, errors, warnings, score, diff, ref_cells: nref, out_cells: nout }
}

pub fn round4(v: f64) -> f64 {
    (v * 10000.0).round() / 10000.0
}
