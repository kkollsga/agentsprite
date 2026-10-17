//! Pixel access for verification: render a cel/layer to indices or RGBA and flatten the visible layers.
//!
//! Blend modes other than Normal are treated as Normal; group opacity is multiplied into children.

use super::model::*;
use crate::img::Rgba;
use crate::output::{PxlError, Result};

fn err(msg: impl Into<String>) -> PxlError {
    PxlError::validation(msg.into())
}

/// The cel on `layer` in `frame`, if any (no link resolution).
pub fn cel_at(doc: &AseDoc, frame: usize, layer: usize) -> Option<&Cel> {
    doc.frames.get(frame)?.chunks.iter().find_map(|c| match c {
        Chunk::Cel(c) if c.layer as usize == layer => Some(c),
        _ => None,
    })
}

/// Like [`cel_at`] but follows linked cels to the one that owns the pixels.
pub fn resolve_cel(doc: &AseDoc, frame: usize, layer: usize) -> Option<&Cel> {
    let mut f = frame;
    for _ in 0..doc.frames.len() + 1 {
        let c = cel_at(doc, f, layer)?;
        match c.data {
            CelData::Linked { frame } => f = frame as usize,
            _ => return Some(c),
        }
    }
    None
}

/// Cel pixels as raw bytes in the document colour mode: (width, height, bytes).
fn cel_bytes(doc: &AseDoc, layer: &Layer, cel: &Cel) -> Result<(usize, usize, Vec<u8>)> {
    let bpp = doc.mode.bytes_per_pixel();
    match &cel.data {
        CelData::Raw { w, h, pixels } | CelData::Image { w, h, pixels } => Ok((*w as usize, *h as usize, pixels.clone())),
        CelData::Linked { .. } => Err(err("linked cel passed to cel_bytes")),
        CelData::Tilemap { w, h, id_mask, xflip_mask, yflip_mask, diag_mask, tiles, .. } => {
            let ts = doc
                .tilesets()
                .into_iter()
                .find(|t| t.id == layer.tileset_index)
                .ok_or_else(|| err(format!("tilemap layer `{}` refers to missing tileset {}", layer.name, layer.tileset_index)))?;
            let data = ts.tiles.as_ref().ok_or_else(|| err("tileset has no embedded tiles"))?;
            let (tw, th) = (ts.tile_w as usize, ts.tile_h as usize);
            let (ow, oh) = (*w as usize * tw, *h as usize * th);
            let mut out = vec![0u8; ow * oh * bpp];
            if doc.mode == ColorMode::Indexed {
                out.iter_mut().for_each(|b| *b = doc.transparent_index);
            }
            for ty in 0..*h as usize {
                for tx in 0..*w as usize {
                    let t = tiles[ty * *w as usize + tx];
                    let id = (t & id_mask) as usize;
                    if id == 0 && ts.flags & tileset_flags::EMPTY_ZERO != 0 {
                        continue;
                    }
                    if id >= ts.num_tiles as usize {
                        continue;
                    }
                    let (fx, fy, fd) = (t & xflip_mask != 0, t & yflip_mask != 0, t & diag_mask != 0);
                    for py in 0..th {
                        for px in 0..tw {
                            // Diagonal flip swaps axes (tiles are square when it is used).
                            let (mut sx, mut sy) = if fd { (py, px) } else { (px, py) };
                            if fx {
                                sx = tw - 1 - sx;
                            }
                            if fy {
                                sy = th - 1 - sy;
                            }
                            if sx >= tw || sy >= th {
                                continue;
                            }
                            let s = ((id * th + sy) * tw + sx) * bpp;
                            let d = ((ty * th + py) * ow + tx * tw + px) * bpp;
                            out[d..d + bpp].copy_from_slice(&data[s..s + bpp]);
                        }
                    }
                }
            }
            Ok((ow, oh, out))
        }
    }
}

fn layer_and_cel<'a>(doc: &'a AseDoc, frame: usize, layer: usize) -> Result<(&'a Layer, Option<&'a Cel>)> {
    let layers = doc.layers();
    let l = *layers.get(layer).ok_or_else(|| err(format!("layer {} does not exist", layer)))?;
    if frame >= doc.frames.len() {
        return Err(err(format!("frame {} does not exist", frame)));
    }
    Ok((l, resolve_cel(doc, frame, layer)))
}

/// Canvas sized (`width*height`) palette indices of one layer in one frame; empty areas hold the
/// transparent index. Indexed documents only.
pub fn render_layer_indices(doc: &AseDoc, frame: usize, layer: usize) -> Result<Vec<u8>> {
    if doc.mode != ColorMode::Indexed {
        return Err(err("render_layer_indices needs an indexed document"));
    }
    let (cw, ch) = (doc.width as usize, doc.height as usize);
    let mut out = vec![doc.transparent_index; cw * ch];
    let (l, cel) = layer_and_cel(doc, frame, layer)?;
    if let Some(cel) = cel {
        let (w, h, px) = cel_bytes(doc, l, cel)?;
        for y in 0..h {
            for x in 0..w {
                let (dx, dy) = (cel.x as i32 + x as i32, cel.y as i32 + y as i32);
                if dx < 0 || dy < 0 || dx >= cw as i32 || dy >= ch as i32 {
                    continue;
                }
                let v = px[y * w + x];
                if v != doc.transparent_index {
                    out[dy as usize * cw + dx as usize] = v;
                }
            }
        }
    }
    Ok(out)
}

fn to_rgba(doc: &AseDoc, pal: &[[u8; 4]], b: &[u8]) -> Rgba {
    match doc.mode {
        ColorMode::Rgba => Rgba::new(b[0], b[1], b[2], b[3]),
        ColorMode::Grayscale => Rgba::new(b[0], b[0], b[0], b[1]),
        ColorMode::Indexed => {
            if b[0] == doc.transparent_index {
                Rgba::TRANSPARENT
            } else {
                match pal.get(b[0] as usize) {
                    Some(c) => Rgba::new(c[0], c[1], c[2], c[3]),
                    None => Rgba::TRANSPARENT,
                }
            }
        }
    }
}

/// Canvas sized RGBA of one layer in one frame (cel opacity applied, layer opacity and blend mode not).
pub fn render_layer_rgba(doc: &AseDoc, frame: usize, layer: usize) -> Result<Vec<Rgba>> {
    let (cw, ch) = (doc.width as usize, doc.height as usize);
    let mut out = vec![Rgba::TRANSPARENT; cw * ch];
    let (l, cel) = layer_and_cel(doc, frame, layer)?;
    if let Some(cel) = cel {
        let pal = doc.palette();
        let bpp = doc.mode.bytes_per_pixel();
        let (w, h, px) = cel_bytes(doc, l, cel)?;
        for y in 0..h {
            for x in 0..w {
                let (dx, dy) = (cel.x as i32 + x as i32, cel.y as i32 + y as i32);
                if dx < 0 || dy < 0 || dx >= cw as i32 || dy >= ch as i32 {
                    continue;
                }
                let mut c = to_rgba(doc, &pal, &px[(y * w + x) * bpp..(y * w + x + 1) * bpp]);
                c.a = mul255(c.a, cel.opacity);
                out[dy as usize * cw + dx as usize] = c;
            }
        }
    }
    Ok(out)
}

fn mul255(a: u8, b: u8) -> u8 {
    let t = a as u32 * b as u32 + 128;
    ((t + (t >> 8)) >> 8) as u8
}

/// True when the layer and all its ancestor groups are visible.
pub fn layer_effectively_visible(layers: &[&Layer], i: usize) -> bool {
    layer_vis_and_opacity(layers, i).0
}

fn layer_vis_and_opacity(layers: &[&Layer], i: usize) -> (bool, u8) {
    let mut vis = layers[i].visible();
    let mut op = layers[i].opacity;
    let mut level = layers[i].child_level;
    let mut j = i;
    while level > 0 && j > 0 {
        j -= 1;
        if layers[j].child_level < level {
            level = layers[j].child_level;
            vis &= layers[j].visible();
            op = mul255(op, layers[j].opacity);
        }
    }
    (vis, op)
}

/// Alpha-composites `src` over `dst` (straight alpha, Normal blend).
fn over(dst: Rgba, src: Rgba) -> Rgba {
    if src.a == 0 {
        return dst;
    }
    if dst.a == 0 || src.a == 255 {
        return src;
    }
    let (sa, da) = (src.a as u32, dst.a as u32);
    let ra = sa + da - (sa * da + 127) / 255;
    let ch = |s: u8, d: u8| -> u8 {
        let num = s as u32 * sa * 255 + d as u32 * da * (255 - sa);
        ((num + ra * 255 / 2) / (ra * 255)).min(255) as u8
    };
    Rgba::new(ch(src.r, dst.r), ch(src.g, dst.g), ch(src.b, dst.b), ra as u8)
}

/// Flattens all visible layers of `frame` (bottom to top, layer opacity applied) to canvas sized RGBA.
pub fn flatten_rgba(doc: &AseDoc, frame: usize) -> Result<Vec<Rgba>> {
    let layers = doc.layers();
    let mut out = vec![Rgba::TRANSPARENT; doc.width as usize * doc.height as usize];
    for (i, l) in layers.iter().enumerate() {
        if l.layer_type == LayerType::Group {
            continue;
        }
        let (vis, op) = layer_vis_and_opacity(&layers, i);
        if !vis {
            continue;
        }
        let px = render_layer_rgba(doc, frame, i)?;
        for (d, s) in out.iter_mut().zip(px) {
            let mut s = s;
            s.a = mul255(s.a, op);
            *d = over(*d, s);
        }
    }
    Ok(out)
}

/// Flattened visible layers as palette indices (topmost non-transparent index wins; no blending).
/// Indexed documents only.
pub fn flatten_indices(doc: &AseDoc, frame: usize) -> Result<Vec<u8>> {
    let layers = doc.layers();
    let mut out = vec![doc.transparent_index; doc.width as usize * doc.height as usize];
    for (i, l) in layers.iter().enumerate() {
        if l.layer_type == LayerType::Group || !layer_effectively_visible(&layers, i) {
            continue;
        }
        let px = render_layer_indices(doc, frame, i)?;
        for (d, s) in out.iter_mut().zip(px) {
            if s != doc.transparent_index {
                *d = s;
            }
        }
    }
    Ok(out)
}
