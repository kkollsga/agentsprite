//! Image insight shared by `pxl intake`, `pxl style analyze` and the design-language lint rules (M6 R7): material (hue) families
//! with their tones and ramps, identity colours, outline, light direction, highlights, dithering, palette size, eye detection by
//! pixels, and the flat-block measure of the quality harness. Everything is pure and deterministic and works on any
//! `Option<Rgb>` pixel function, so the same code measures an input image and a reduced grid.

use crate::color::{srgb_to_oklab, Oklab, Rgb};
use crate::img::RgbaImage;

pub fn lab(c: Rgb) -> Oklab {
    srgb_to_oklab(c)
}

pub fn chroma(c: Rgb) -> f64 {
    let o = lab(c);
    (o.a * o.a + o.b * o.b).sqrt()
}

pub fn hue(c: Rgb) -> f64 {
    let o = lab(c);
    o.b.atan2(o.a)
}

/// Hue distance in radians (0..pi).
pub fn hue_gap(a: f64, b: f64) -> f64 {
    let d = (a - b).abs();
    d.min(std::f64::consts::TAU - d)
}

/// Oklab distance x 100.
pub fn de(a: Rgb, b: Rgb) -> f64 {
    let (x, y) = (lab(a), lab(b));
    ((x.l - y.l).powi(2) + (x.a - y.a).powi(2) + (x.b - y.b).powi(2)).sqrt() * 100.0
}

pub fn hex(c: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
}

pub fn parse_hex(s: &str) -> Option<Rgb> {
    crate::color::parse_hex(s).ok()
}

/// A plain name for a hue angle (Oklab, radians).
pub fn hue_name(h: f64) -> &'static str {
    let deg = (h.to_degrees() + 360.0) % 360.0;
    match deg {
        d if !(12.0..345.0).contains(&d) => "pink",
        d if d < 40.0 => "red",
        d if d < 70.0 => "orange",
        d if d < 110.0 => "yellow",
        d if d < 170.0 => "green",
        d if d < 215.0 => "teal",
        d if d < 285.0 => "blue",
        _ => "purple",
    }
}

/// Name of a colour: hue name with a lightness word, or a grey word.
pub fn colour_name(c: Rgb) -> String {
    let o = lab(c);
    let ch = chroma(c);
    if ch < 0.04 {
        return if o.l > 0.92 { "white".into() } else if o.l > 0.7 { "light grey".into() } else if o.l > 0.4 { "grey".into() } else if o.l > 0.2 { "dark grey".into() } else { "black".into() };
    }
    let base = hue_name(hue(c));
    if o.l > 0.82 {
        format!("light {}", base)
    } else if o.l < 0.35 {
        format!("dark {}", base)
    } else {
        base.to_string()
    }
}

/// Opaque pixels of an image as `(x, y, colour)`.
pub fn opaque(im: &RgbaImage) -> Vec<(i32, i32, Rgb)> {
    let mut v = Vec::new();
    for y in 0..im.h {
        for x in 0..im.w {
            let p = im.px[y * im.w + x];
            if p.a >= 128 {
                v.push((x as i32, y as i32, Rgb { r: p.r, g: p.g, b: p.b }));
            }
        }
    }
    v
}

// ---------------------------------------------------------------------------------------------- families

/// A material: pixels of one hue family (chromatic), or the neutral family (`neutral = true`).
#[derive(Clone, Debug)]
pub struct Family {
    pub hue: f64,
    pub neutral: bool,
    pub n: usize,
    /// Share of the opaque pixels.
    pub share: f64,
    pub mean: Rgb,
    /// Mean chroma.
    pub chroma: f64,
    /// Distinct lightness bands holding at least 6 % of the family (the tones of the material).
    pub tones: usize,
    /// Dark, mid and light representative colours (means of the lightness thirds).
    pub ramp: [Rgb; 3],
    /// The material's tones as exact source colours, dark to light, all distinct: the most frequent colour of every significant tone band
    /// (two for a two-tone material, one for a flat one, never more than three).
    pub ramp_src: Vec<Rgb>,
    /// Share of the material held by each significant tone band, dark to light (sums to about 1).
    pub tone_shares: Vec<f64>,
    pub l_lo: f64,
    pub l_hi: f64,
}

impl Family {
    pub fn name(&self) -> String {
        if self.neutral {
            "grey".into()
        } else {
            hue_name(self.hue).to_string()
        }
    }
}

pub const HUE_TOL: f64 = 0.4;

/// Group colours into hue families (a pixel joins the first family whose first-seen hue is within 0.45 rad) plus one neutral family
/// (chroma < 0.05). `pixels` may repeat colours; each entry counts once.
pub fn families(pixels: &[Rgb]) -> Vec<Family> {
    let total = pixels.len().max(1);
    let mut groups: Vec<(f64, bool, Vec<Rgb>)> = Vec::new();
    // the most frequent colours seed the families (a stable order, whatever the pixel order of the image)
    let mut counts: std::collections::BTreeMap<Rgb, usize> = std::collections::BTreeMap::new();
    for &c in pixels {
        *counts.entry(c).or_insert(0) += 1;
    }
    let mut by_count: Vec<(Rgb, usize)> = counts.into_iter().collect();
    by_count.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    for (c, n) in by_count {
        let neutral = chroma(c) < 0.05;
        let h = hue(c);
        match groups.iter_mut().find(|g| g.1 == neutral && (neutral || hue_gap(g.0, h) < HUE_TOL)) {
            Some(g) => g.2.extend(std::iter::repeat(c).take(n)),
            None => groups.push((h, neutral, vec![c; n])),
        }
    }
    let mut out: Vec<Family> = groups
        .into_iter()
        .map(|(h, neutral, mut cs)| {
            cs.sort_by(|a, b| lab(*a).l.partial_cmp(&lab(*b).l).unwrap_or(std::cmp::Ordering::Equal));
            let n = cs.len();
            let mean_of = |s: &[Rgb]| {
                let k = s.len().max(1) as f64;
                Rgb { r: (s.iter().map(|c| c.r as f64).sum::<f64>() / k).round() as u8, g: (s.iter().map(|c| c.g as f64).sum::<f64>() / k).round() as u8, b: (s.iter().map(|c| c.b as f64).sum::<f64>() / k).round() as u8 }
            };
            // tones: lightness bands separated by gaps of more than 0.05; the significant ones hold at least 6 % of the family
            let ls: Vec<f64> = cs.iter().map(|c| lab(*c).l).collect();
            let mut bands: Vec<(usize, usize)> = Vec::new();
            let mut start = 0;
            for i in 1..=n {
                if i == n || ls[i] - ls[i - 1] > 0.05 {
                    bands.push((start, i));
                    start = i;
                }
            }
            let mut sig: Vec<(usize, usize)> = bands.iter().copied().filter(|b| (b.1 - b.0) * 100 >= 6 * n && b.1 - b.0 >= 2).collect();
            if sig.is_empty() {
                sig = bands.clone();
            }
            let tones = sig.len();
            let (dark, light) = (sig[0], sig[sig.len() - 1]);
            let target = (ls[dark.0] + ls[light.1 - 1]) / 2.0;
            let mid = *sig.iter().min_by(|x, y| ((ls[x.0] + ls[x.1 - 1]) / 2.0 - target).abs().partial_cmp(&((ls[y.0] + ls[y.1 - 1]) / 2.0 - target).abs()).unwrap_or(std::cmp::Ordering::Equal)).unwrap();
            let ramp = [mean_of(&cs[dark.0..dark.1]), mean_of(&cs[mid.0..mid.1]), mean_of(&cs[light.0..light.1])];
            let top = |b: (usize, usize)| -> Rgb {
                let mut c: std::collections::BTreeMap<Rgb, usize> = std::collections::BTreeMap::new();
                for x in &cs[b.0..b.1] {
                    *c.entry(*x).or_insert(0) += 1;
                }
                c.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0))).map(|e| e.0).unwrap_or(cs[b.0])
            };
            let mut ramp_src: Vec<Rgb> = Vec::new();
            let picks: Vec<(usize, usize)> = match sig.len() {
                0 | 1 => vec![sig[0]],
                2 => vec![sig[0], sig[1]],
                k => {
                    // dark, light and the band nearest the middle of the two
                    let m = (1..k - 1).min_by(|&x, &y| ((ls[sig[x].0] + ls[sig[x].1 - 1]) / 2.0 - target).abs().partial_cmp(&((ls[sig[y].0] + ls[sig[y].1 - 1]) / 2.0 - target).abs()).unwrap_or(std::cmp::Ordering::Equal)).unwrap_or(1);
                    vec![sig[0], sig[m], sig[k - 1]]
                }
            };
            for b in picks {
                let c = top(b);
                if ramp_src.iter().all(|x| de(*x, c) >= 4.0) {
                    ramp_src.push(c);
                }
            }
            Family {
                hue: h,
                neutral,
                n,
                share: n as f64 / total as f64,
                mean: mean_of(&cs),
                chroma: cs.iter().map(|c| chroma(*c)).sum::<f64>() / n.max(1) as f64,
                tones: tones.max(1),
                ramp,
                ramp_src,
                tone_shares: sig.iter().map(|b| (b.1 - b.0) as f64 / n.max(1) as f64).collect(),
                l_lo: ls[(n as f64 * 0.05) as usize],
                l_hi: ls[((n as f64 * 0.95) as usize).min(n - 1)],
            }
        })
        .collect();
    out.sort_by(|a, b| b.n.cmp(&a.n).then(a.neutral.cmp(&b.neutral)));
    out
}

/// The pixels that belong to materials: the outline (the darkest tone, Oklab L < 0.27) and the pure whites (eye whites, glints) are left out.
/// They are carried by their own rules (outline, eyes, highlight), so they never make a material of their own ("grey x2" on a hero without grey).
pub fn material_pixels(pixels: &[Rgb]) -> Vec<Rgb> {
    pixels.iter().copied().filter(|c| !(lab(*c).l < 0.27 || (lab(*c).l > 0.93 && chroma(*c) < 0.05))).collect()
}

/// One identity colour: the colour, its share of the opaque pixels, its score and the material it belongs to.
#[derive(Clone, Debug)]
pub struct IdentityColour {
    pub colour: Rgb,
    pub share: f64,
    pub score: f64,
    pub material: String,
}

/// The identity of an image, per material (hue family, outline and whites left out). Every material that covers at least 3 % of the figure
/// contributes the exact colour of each of its tones that holds at least 12 % of the material (a 6 pixel collar highlight is not a tone, the
/// orange of a head of hair and the teal of overalls are). A small material (0.5 % to 3 %) counts only when it is saturated and its hue is clearly
/// apart from every larger material (red shoes, a yellow mouth). Colours are ranked by area x (1 + 2 x saturation) x (1 + distinctness of the
/// material), at most eight, so the dominant materials come first.
pub fn identity_list(pixels: &[Rgb]) -> Vec<IdentityColour> {
    let total = pixels.len().max(1) as f64;
    let mat = material_pixels(pixels);
    if mat.is_empty() {
        return Vec::new();
    }
    // group the exact colours by hue family
    let mut counts: std::collections::BTreeMap<Rgb, usize> = std::collections::BTreeMap::new();
    for &c in &mat {
        *counts.entry(c).or_insert(0) += 1;
    }
    let mut by_count: Vec<(Rgb, usize)> = counts.into_iter().collect();
    by_count.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    struct Mat {
        hue: f64,
        neutral: bool,
        colours: Vec<(Rgb, usize)>,
        n: usize,
    }
    let mut mats: Vec<Mat> = Vec::new();
    for (c, n) in by_count {
        let neutral = chroma(c) < 0.05;
        let h = hue(c);
        match mats.iter_mut().find(|m| m.neutral == neutral && (neutral || hue_gap(m.hue, h) < HUE_TOL)) {
            Some(m) => {
                m.colours.push((c, n));
                m.n += n;
            }
            None => mats.push(Mat { hue: h, neutral, colours: vec![(c, n)], n }),
        }
    }
    let big: Vec<usize> = (0..mats.len()).filter(|&i| mats[i].n as f64 / total >= 0.03).collect();
    let mut out: Vec<IdentityColour> = Vec::new();
    for (i, m) in mats.iter().enumerate() {
        let share = m.n as f64 / total;
        let is_big = big.contains(&i);
        // the gap of this material's hue to the nearest other material (a grey is far from everything chromatic)
        let gap = mats.iter().enumerate().filter(|(j, o)| *j != i && o.n as f64 / total >= 0.03).map(|(_, o)| if o.neutral != m.neutral { 1.0 } else { hue_gap(o.hue, m.hue) }).fold(f64::MAX, f64::min);
        let gap = if gap == f64::MAX { 1.0 } else { gap };
        let sat_m = m.colours.iter().map(|(c, n)| chroma(*c) * *n as f64).sum::<f64>() / m.n.max(1) as f64;
        if !is_big {
            if !(share >= 0.005 && sat_m >= 0.10 && gap >= 0.5) {
                continue;
            }
        }
        if m.neutral && share < 0.08 {
            continue; // a few grey pixels are shading, not a material
        }
        // tone bands of the material: lightness gaps of more than 0.05
        let mut cs = m.colours.clone();
        cs.sort_by(|a, b| lab(a.0).l.partial_cmp(&lab(b.0).l).unwrap_or(std::cmp::Ordering::Equal));
        let mut bands: Vec<Vec<(Rgb, usize)>> = Vec::new();
        for c in cs {
            match bands.last_mut() {
                Some(b) if lab(c.0).l - lab(b.last().unwrap().0).l <= 0.05 => b.push(c),
                _ => bands.push(vec![c]),
            }
        }
        let mut picked: Vec<(Rgb, usize)> = Vec::new();
        for b in &bands {
            let area: usize = b.iter().map(|x| x.1).sum();
            if (area as f64) >= 0.12 * m.n as f64 || !is_big {
                if let Some(top) = b.iter().max_by(|x, y| x.1.cmp(&y.1).then(y.0.cmp(&x.0))) {
                    picked.push((top.0, area));
                }
            }
        }
        if picked.is_empty() {
            if let Some(top) = m.colours.iter().max_by(|x, y| x.1.cmp(&y.1).then(y.0.cmp(&x.0))) {
                picked.push((top.0, m.n));
            }
        }
        for (c, area) in picked {
            let sat = (chroma(c) / 0.2).min(1.0);
            let a = area as f64 / total;
            out.push(IdentityColour { colour: c, share: a, score: a * (1.0 + 2.0 * sat) * (1.0 + gap.min(1.0)), material: if m.neutral { "grey".into() } else { hue_name(m.hue).to_string() } });
        }
    }
    out.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal).then(a.colour.cmp(&b.colour)));
    out.truncate(8);
    out
}

/// Saturated colours of small area: the identity colours of an image (shoes, mouth, moustache, gem). Exact colours are grouped by hue
/// (0.2 rad, most frequent first); a group that covers 0.5 % to 12 % of the opaque pixels and has chroma >= 0.10 is an identity colour,
/// represented by its most frequent exact colour. Large materials (more than 12 %) and their tones are not identity colours. Sorted by share.
pub fn small_saturated_colours(pixels: &[Rgb]) -> Vec<(Rgb, f64)> {
    let total = pixels.len().max(1) as f64;
    let mut counts: std::collections::BTreeMap<Rgb, usize> = std::collections::BTreeMap::new();
    for &c in pixels.iter().filter(|c| chroma(**c) >= 0.05) {
        *counts.entry(c).or_insert(0) += 1;
    }
    let mut by_count: Vec<(Rgb, usize)> = counts.into_iter().collect();
    by_count.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut groups: Vec<(f64, Rgb, usize, usize)> = Vec::new(); // (hue, dominant, dominant count, total)
    for (c, n) in by_count {
        match groups.iter_mut().find(|g| hue_gap(g.0, hue(c)) < 0.2) {
            Some(g) => g.3 += n,
            None => groups.push((hue(c), c, n, n)),
        }
    }
    let mut out: Vec<(Rgb, f64)> = groups
        .into_iter()
        .filter(|g| {
            let share = g.3 as f64 / total;
            share >= 0.005 && share <= 0.12 && chroma(g.1) >= 0.10
        })
        .map(|g| (g.1, g.3 as f64 / total))
        .collect();
    out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// `(colour, share)` of the identity colours of an image (see [`identity_list`]), the most important first.
pub fn identity_colours(pixels: &[Rgb]) -> Vec<(Rgb, f64)> {
    identity_list(pixels).into_iter().map(|c| (c.colour, c.share)).collect()
}

// ---------------------------------------------------------------------------------------------- outline

#[derive(Clone, Debug, Default)]
pub struct Outline {
    /// Share of the boundary pixels that are a dark outline (0..1).
    pub presence: f64,
    pub colour: Option<Rgb>,
    /// Median run of dark pixels inward from the boundary, in image pixels.
    pub thickness: f64,
    pub boundary: usize,
}

/// Outline of an opaque image: boundary pixels (an opaque pixel with a transparent 4-neighbour) that are clearly darker than the interior.
pub fn outline(im: &RgbaImage) -> Outline {
    let (w, h) = (im.w as i32, im.h as i32);
    let op = |x: i32, y: i32| x >= 0 && y >= 0 && x < w && y < h && im.px[(y * w + x) as usize].a >= 128;
    let col = |x: i32, y: i32| {
        let p = im.px[(y * w + x) as usize];
        Rgb { r: p.r, g: p.g, b: p.b }
    };
    let mut boundary = Vec::new();
    let mut interior_l = Vec::new();
    for y in 0..h {
        for x in 0..w {
            if !op(x, y) {
                continue;
            }
            let edge = [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|(dx, dy)| !op(x + dx, y + dy));
            if edge {
                boundary.push((x, y));
            } else {
                interior_l.push(lab(col(x, y)).l);
            }
        }
    }
    if boundary.is_empty() {
        return Outline::default();
    }
    let mean_int = if interior_l.is_empty() { 0.6 } else { interior_l.iter().sum::<f64>() / interior_l.len() as f64 };
    let dark_limit = (mean_int - 0.12).min(0.45);
    let dark: Vec<(i32, i32)> = boundary.iter().copied().filter(|&(x, y)| lab(col(x, y)).l < dark_limit).collect();
    let presence = dark.len() as f64 / boundary.len() as f64;
    let colour = if dark.is_empty() {
        None
    } else {
        let k = dark.len() as f64;
        let (r, g, b) = dark.iter().fold((0.0, 0.0, 0.0), |a, &(x, y)| (a.0 + col(x, y).r as f64, a.1 + col(x, y).g as f64, a.2 + col(x, y).b as f64));
        Some(Rgb { r: (r / k).round() as u8, g: (g / k).round() as u8, b: (b / k).round() as u8 })
    };
    // thickness: dark run length going inward, opposite to the transparent neighbour
    let mut runs: Vec<usize> = Vec::new();
    for &(x, y) in &dark {
        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            if op(x + dx, y + dy) {
                continue;
            }
            // the transparent side is (dx,dy); go the other way
            let (mut cx, mut cy, mut n) = (x, y, 0usize);
            while op(cx, cy) && lab(col(cx, cy)).l < dark_limit && n < 64 {
                n += 1;
                cx -= dx;
                cy -= dy;
            }
            runs.push(n);
            break;
        }
    }
    runs.sort_unstable();
    let thickness = if runs.is_empty() { 0.0 } else { runs[runs.len() / 2] as f64 };
    Outline { presence, colour, thickness, boundary: boundary.len() }
}

// ---------------------------------------------------------------------------------------------- light, highlight, dither

/// `(direction, strength)`: where the light comes from (top, bottom, left, right and corners) from the correlation of lightness with
/// position; "even" when no direction stands out.
pub fn light_direction(pixels: &[(i32, i32, Rgb)]) -> (String, f64) {
    let n = pixels.len() as f64;
    if pixels.len() < 12 {
        return ("even".into(), 0.0);
    }
    let (mx, my) = (pixels.iter().map(|p| p.0 as f64).sum::<f64>() / n, pixels.iter().map(|p| p.1 as f64).sum::<f64>() / n);
    let ls: Vec<f64> = pixels.iter().map(|p| lab(p.2).l).collect();
    let ml = ls.iter().sum::<f64>() / n;
    let (mut sxx, mut syy, mut sll, mut sxl, mut syl) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (p, l) in pixels.iter().zip(&ls) {
        let (dx, dy, dl) = (p.0 as f64 - mx, p.1 as f64 - my, l - ml);
        sxx += dx * dx;
        syy += dy * dy;
        sll += dl * dl;
        sxl += dx * dl;
        syl += dy * dl;
    }
    if sll < 1e-9 {
        return ("even".into(), 0.0);
    }
    let cx = if sxx > 0.0 { sxl / (sxx * sll).sqrt() } else { 0.0 };
    let cy = if syy > 0.0 { syl / (syy * sll).sqrt() } else { 0.0 };
    let strength = cx.abs().max(cy.abs());
    if strength < 0.12 {
        return ("even".into(), strength);
    }
    // lightness grows toward the light: negative correlation with x means the light is on the left
    let v = if cy.abs() >= 0.12 { Some(if cy < 0.0 { "top" } else { "bottom" }) } else { None };
    let hz = if cx.abs() >= 0.12 { Some(if cx < 0.0 { "left" } else { "right" }) } else { None };
    let dir = match (v, hz) {
        (Some(a), Some(b)) => format!("{}-{}", a, b),
        (Some(a), None) => a.into(),
        (None, Some(b)) => b.into(),
        _ => "even".into(),
    };
    (dir, strength)
}

/// `(style, share)`: near-white pixels (L > 0.92, nearly neutral) as a share of the opaque pixels, and the style: none, spots (small clusters)
/// or areas (large light regions).
pub fn highlight(im: &RgbaImage) -> (String, f64) {
    let (w, h) = (im.w, im.h);
    let white = |x: usize, y: usize| {
        let p = im.px[y * w + x];
        p.a >= 128 && {
            let c = Rgb { r: p.r, g: p.g, b: p.b };
            lab(c).l > 0.92 && chroma(c) < 0.06
        }
    };
    let opaque_n = im.px.iter().filter(|p| p.a >= 128).count().max(1);
    let mut seen = vec![false; w * h];
    let (mut total, mut comps, mut biggest) = (0usize, 0usize, 0usize);
    for y in 0..h {
        for x in 0..w {
            if seen[y * w + x] || !white(x, y) {
                continue;
            }
            let (mut st, mut n) = (vec![(x, y)], 0usize);
            seen[y * w + x] = true;
            while let Some((cx, cy)) = st.pop() {
                n += 1;
                for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
                    let (nx, ny) = (cx as i32 + dx, cy as i32 + dy);
                    if nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h && !seen[ny as usize * w + nx as usize] && white(nx as usize, ny as usize) {
                        seen[ny as usize * w + nx as usize] = true;
                        st.push((nx as usize, ny as usize));
                    }
                }
            }
            total += n;
            comps += 1;
            biggest = biggest.max(n);
        }
    }
    let share = total as f64 / opaque_n as f64;
    let style = if share < 0.003 || comps == 0 {
        "none"
    } else if share < 0.06 || biggest * 100 < 4 * opaque_n {
        "spots"
    } else {
        "areas"
    };
    (style.to_string(), share)
}

/// Share of the block-sampled pixels that sit in a checkerboard pattern (a pixel unlike its four neighbours and like at least three diagonals).
pub fn dither(im: &RgbaImage, f: usize) -> f64 {
    let f = f.max(1);
    let (bw, bh) = (im.w / f, im.h / f);
    let at = |x: i32, y: i32| -> Option<Rgb> {
        if x < 0 || y < 0 || x as usize >= bw || y as usize >= bh {
            return None;
        }
        let p = im.px[(y as usize * f + f / 2) * im.w + x as usize * f + f / 2];
        (p.a >= 128).then_some(Rgb { r: p.r, g: p.g, b: p.b })
    };
    let (mut opaque_n, mut hits) = (0usize, 0usize);
    for y in 0..bh as i32 {
        for x in 0..bw as i32 {
            let Some(c) = at(x, y) else { continue };
            opaque_n += 1;
            let ortho = [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().all(|(dx, dy)| at(x + dx, y + dy).map_or(false, |o| de(o, c) > 3.0));
            let diag = [(1, 1), (-1, 1), (1, -1), (-1, -1)].iter().filter(|(dx, dy)| at(x + dx, y + dy).map_or(false, |o| de(o, c) <= 3.0)).count();
            if ortho && diag >= 3 {
                hits += 1;
            }
        }
    }
    hits as f64 / opaque_n.max(1) as f64
}

/// `(distinct colours, major colours)`; a major colour covers at least 0.5 % of the opaque pixels.
pub fn palette_size(pixels: &[Rgb]) -> (usize, usize) {
    let mut counts: std::collections::BTreeMap<Rgb, usize> = std::collections::BTreeMap::new();
    for &c in pixels {
        *counts.entry(c).or_insert(0) += 1;
    }
    let total = pixels.len().max(1);
    (counts.len(), counts.values().filter(|&&n| n * 200 >= total).count())
}

// ---------------------------------------------------------------------------------------------- eyes by pixels

/// Eyes: a group of at least three near-white pixels with a dark pupil next to it that the white surrounds (three or more white
/// neighbours). A white speck beside a dark edge is a glint, not an eye. `px(x, y)` is the colour of an opaque pixel.
pub fn eyes_by_pixels(w: usize, h: usize, white_l: f64, px: &dyn Fn(usize, usize) -> Option<Rgb>) -> usize {
    let white = |c: Rgb| lab(c).l > white_l && chroma(c) < 0.06;
    let dark = |c: Rgb| lab(c).l < 0.4;
    let mut seen = vec![false; w * h];
    let mut eyes = 0;
    for y in 0..h {
        for x in 0..w {
            if seen[y * w + x] || !px(x, y).map_or(false, white) {
                continue;
            }
            let (mut st, mut comp) = (vec![(x, y)], Vec::new());
            seen[y * w + x] = true;
            while let Some((cx, cy)) = st.pop() {
                comp.push((cx, cy));
                for dy in -1i32..=1 {
                    for dx in -1i32..=1 {
                        let (nx, ny) = (cx as i32 + dx, cy as i32 + dy);
                        if nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h && !seen[ny as usize * w + nx as usize] && px(nx as usize, ny as usize).map_or(false, white) {
                            seen[ny as usize * w + nx as usize] = true;
                            st.push((nx as usize, ny as usize));
                        }
                    }
                }
            }
            if comp.len() < 3 {
                continue;
            }
            let set: std::collections::HashSet<(usize, usize)> = comp.iter().copied().collect();
            let pupil = comp.iter().any(|&(cx, cy)| {
                (-1i32..=1).any(|dy| {
                    (-1i32..=1).any(|dx| {
                        let (nx, ny) = (cx as i32 + dx, cy as i32 + dy);
                        if nx < 0 || ny < 0 || nx as usize >= w || ny as usize >= h || !px(nx as usize, ny as usize).map_or(false, dark) {
                            return false;
                        }
                        let around = (-1i32..=1).map(|ey| (-1i32..=1).filter(|ex| nx + ex >= 0 && ny + ey >= 0 && set.contains(&((nx + ex) as usize, (ny + ey) as usize))).count()).sum::<usize>();
                        around >= 3
                    })
                })
            });
            if pupil {
                eyes += 1;
            }
        }
    }
    eyes
}

pub fn image_px(im: &RgbaImage) -> impl Fn(usize, usize) -> Option<Rgb> + '_ {
    move |x, y| {
        let p = im.px[y * im.w + x];
        (p.a >= 128).then_some(Rgb { r: p.r, g: p.g, b: p.b })
    }
}

// ---------------------------------------------------------------------------------------------- flat block

/// Largest single-colour output region whose hue does not occur in the input under it, as a share of the opaque output pixels (the
/// `flat_block` metric of the quality harness). `org` is the input pixel under output cell (0, 0), `f` the factor.
pub fn flat_block(img: &RgbaImage, gw: usize, gh: usize, org: (i32, i32), f: usize, out_px: &dyn Fn(usize, usize) -> Option<Rgb>) -> f64 {
    let (mut seen, mut opaque_n, mut worst) = (vec![false; gw * gh], 0usize, 0usize);
    for y in 0..gh {
        for x in 0..gw {
            opaque_n += usize::from(out_px(x, y).is_some());
        }
    }
    for y in 0..gh {
        for x in 0..gw {
            let Some(c) = out_px(x, y) else { continue };
            if seen[y * gw + x] {
                continue;
            }
            let (mut st, mut reg) = (vec![(x, y)], Vec::new());
            seen[y * gw + x] = true;
            while let Some((cx, cy)) = st.pop() {
                reg.push((cx, cy));
                for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
                    let (nx, ny) = (cx as i32 + dx, cy as i32 + dy);
                    if nx >= 0 && ny >= 0 && (nx as usize) < gw && (ny as usize) < gh && !seen[ny as usize * gw + nx as usize] && out_px(nx as usize, ny as usize) == Some(c) {
                        seen[ny as usize * gw + nx as usize] = true;
                        st.push((nx as usize, ny as usize));
                    }
                }
            }
            let (mut total, mut chromatic, mut matching) = (0usize, 0usize, 0usize);
            for &(rx, ry) in &reg {
                for dy in 0..f {
                    for dx in 0..f {
                        let (ix, iy) = (org.0 + (rx * f + dx) as i32, org.1 + (ry * f + dy) as i32);
                        if ix >= 0 && iy >= 0 && (ix as usize) < img.w && (iy as usize) < img.h {
                            let p = img.px[iy as usize * img.w + ix as usize];
                            if p.a > 0 {
                                let ic = Rgb { r: p.r, g: p.g, b: p.b };
                                total += 1;
                                if chroma(ic) > 0.05 {
                                    chromatic += 1;
                                    if chroma(c) > 0.05 {
                                        matching += usize::from(hue_gap(hue(ic), hue(c)) < 0.7);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if total == 0 {
                continue;
            }
            let absent = if chroma(c) > 0.05 { chromatic > 0 && matching * 10 < chromatic.max(total / 2) } else { chromatic * 2 > total };
            if absent && chroma(c) > 0.05 && chromatic * 4 < total {
                continue; // mostly neutral input under a chromatic colour: a tint, not a wrong hue
            }
            if absent {
                worst = worst.max(reg.len());
            }
        }
    }
    worst as f64 / opaque_n.max(1) as f64
}

/// Lightness range (Oklab L, 2nd to 98th percentile) of colours.
pub fn value_spread(colours: &[Rgb]) -> f64 {
    let mut ls: Vec<f64> = colours.iter().map(|c| lab(*c).l).collect();
    if ls.is_empty() {
        return 0.0;
    }
    ls.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let q = |p: f64| ls[((ls.len() - 1) as f64 * p).round() as usize];
    q(0.98) - q(0.02)
}
