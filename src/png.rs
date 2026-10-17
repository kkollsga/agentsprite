//! PNG reading (via the `png` crate) and deterministic writing (own encoder over `miniz_oxide`).
//! Output bytes depend only on the input pixels, never on platform or crate versions of an encoder.

use crate::fsutil::PathSlash;
use crate::color::Rgb;
use crate::fsutil;
use crate::img::{Rgba, RgbaImage};
use crate::output::{PxlError, Result};
use std::path::Path;

fn open(path: &Path) -> Result<png::Reader<std::io::Cursor<Vec<u8>>>> {
    open_with(path, |_| {})
}

fn open_with(path: &Path, f: impl FnOnce(&mut png::Decoder<std::io::Cursor<Vec<u8>>>)) -> Result<png::Reader<std::io::Cursor<Vec<u8>>>> {
    let bytes = fsutil::read_file(path)?;
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    f(&mut dec);
    dec.read_info().map_err(|e| PxlError::validation(format!("cannot decode PNG {}: {}", path.slash(), e)))
}

/// Read any PNG as RGBA8 (indexed + tRNS, grey, 16 bit, sub-byte depths included).
pub fn read_rgba(path: &Path) -> Result<RgbaImage> {
    let mut r = open_with(path, |d| d.set_transformations(png::Transformations::normalize_to_color8()))?;
    let mut buf = vec![0u8; r.output_buffer_size()];
    let info = r
        .next_frame(&mut buf)
        .map_err(|e| PxlError::validation(format!("cannot decode PNG {}: {}", path.slash(), e)))?;
    let (w, h) = (info.width as usize, info.height as usize);
    let data = &buf[..info.buffer_size()];
    let mut px = Vec::with_capacity(w * h);
    match info.color_type {
        png::ColorType::Rgba => data.chunks_exact(4).for_each(|c| px.push(Rgba::new(c[0], c[1], c[2], c[3]))),
        png::ColorType::Rgb => data.chunks_exact(3).for_each(|c| px.push(Rgba::opaque(c[0], c[1], c[2]))),
        png::ColorType::GrayscaleAlpha => data.chunks_exact(2).for_each(|c| px.push(Rgba::new(c[0], c[0], c[0], c[1]))),
        png::ColorType::Grayscale => data.iter().for_each(|&c| px.push(Rgba::opaque(c, c, c))),
        png::ColorType::Indexed => return Err(PxlError::validation(format!("PNG {} stayed indexed after expansion", path.slash()))),
    }
    Ok(RgbaImage { w, h, px })
}

/// Read an indexed PNG: (width, height, one palette index per pixel, palette, tRNS alpha per entry).
pub fn read_indexed(path: &Path) -> Result<(usize, usize, Vec<u8>, Vec<Rgb>, Vec<u8>)> {
    let mut r = open(path)?;
    let (ct, depth) = r.output_color_type();
    if ct != png::ColorType::Indexed {
        return Err(PxlError::validation(format!("{} is not an indexed PNG (colour type {:?})", path.slash(), ct))
            .with_hint("export indexed PNGs with `pxl export`"));
    }
    let mut buf = vec![0u8; r.output_buffer_size()];
    let info = r
        .next_frame(&mut buf)
        .map_err(|e| PxlError::validation(format!("cannot decode PNG {}: {}", path.slash(), e)))?;
    let (w, h) = (info.width as usize, info.height as usize);
    let bits = depth as usize;
    let stride = (w * bits + 7) / 8;
    let mut idx = Vec::with_capacity(w * h);
    for y in 0..h {
        let row = &buf[y * stride..(y + 1) * stride];
        for x in 0..w {
            let v = match bits {
                8 => row[x],
                _ => {
                    let per = 8 / bits;
                    let byte = row[x / per];
                    let shift = 8 - bits * (x % per + 1);
                    (byte >> shift) & ((1u8 << bits) - 1)
                }
            };
            idx.push(v);
        }
    }
    let info = r.info();
    let pal = info
        .palette
        .as_ref()
        .map(|p| p.chunks_exact(3).map(|c| Rgb::new(c[0], c[1], c[2])).collect())
        .unwrap_or_default();
    let trns = info.trns.as_ref().map(|t| t.to_vec()).unwrap_or_default();
    Ok((w, h, idx, pal, trns))
}

fn crc_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    for (n, e) in t.iter_mut().enumerate() {
        let mut c = n as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB88320 ^ (c >> 1) } else { c >> 1 };
        }
        *e = c;
    }
    t
}

fn chunk(out: &mut Vec<u8>, ty: &[u8; 4], data: &[u8], table: &[u32; 256]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(ty);
    out.extend_from_slice(data);
    let mut c = 0xFFFF_FFFFu32;
    for &b in &out[start..] {
        c = table[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    out.extend_from_slice(&(c ^ 0xFFFF_FFFF).to_be_bytes());
}

fn assemble(w: usize, h: usize, depth: u8, color_type: u8, plte: Option<&[u8]>, trns: Option<&[u8]>, raw: &[u8]) -> Vec<u8> {
    let table = crc_table();
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[depth, color_type, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr, &table);
    if let Some(p) = plte {
        chunk(&mut out, b"PLTE", p, &table);
    }
    if let Some(t) = trns {
        chunk(&mut out, b"tRNS", t, &table);
    }
    let z = miniz_oxide::deflate::compress_to_vec_zlib(raw, 9);
    chunk(&mut out, b"IDAT", &z, &table);
    chunk(&mut out, b"IEND", &[], &table);
    out
}

/// Bytes of a deterministic indexed PNG: PLTE in the given order, minimal bit depth, filter 0, zlib level 9;
/// with `transparent0`, palette index 0 is transparent through a one-byte tRNS and the rest opaque.
pub fn encode_indexed(w: usize, h: usize, indices: &[u8], palette: &[Rgb], transparent0: bool) -> Result<Vec<u8>> {
    encode_indexed_alpha(w, h, indices, palette, if transparent0 { Some(&[0]) } else { None })
}

/// Like [`encode_indexed`] with an explicit tRNS table: `alpha[i]` is the alpha of palette entry `i`, entries past the
/// table are opaque. Used by the 32-entry sprite palettes where every `slot*4 + 0` entry is transparent.
pub fn encode_indexed_alpha(w: usize, h: usize, indices: &[u8], palette: &[Rgb], alpha: Option<&[u8]>) -> Result<Vec<u8>> {
    if indices.len() != w * h {
        return Err(PxlError::validation(format!("indexed image needs {} indices, got {}", w * h, indices.len())));
    }
    if palette.is_empty() || palette.len() > 256 {
        return Err(PxlError::validation(format!("palette must have 1..=256 entries, got {}", palette.len())));
    }
    if let Some(&bad) = indices.iter().find(|&&i| i as usize >= palette.len()) {
        return Err(PxlError::validation(format!("pixel uses palette index {} but the palette has {} entries", bad, palette.len())));
    }
    let depth: usize = match palette.len() {
        0..=2 => 1,
        3..=4 => 2,
        5..=16 => 4,
        _ => 8,
    };
    let stride = (w * depth + 7) / 8;
    let mut raw = vec![0u8; (stride + 1) * h];
    for y in 0..h {
        let row = &mut raw[y * (stride + 1) + 1..(y + 1) * (stride + 1)];
        for x in 0..w {
            let v = indices[y * w + x];
            if depth == 8 {
                row[x] = v;
            } else {
                let per = 8 / depth;
                row[x / per] |= v << (8 - depth * (x % per + 1));
            }
        }
    }
    let plte: Vec<u8> = palette.iter().flat_map(|c| [c.r, c.g, c.b]).collect();
    Ok(assemble(w, h, depth as u8, 3, Some(&plte), alpha, &raw))
}

/// Write a deterministic indexed PNG atomically (see [`encode_indexed`]).
pub fn write_indexed(path: &Path, w: usize, h: usize, indices: &[u8], palette: &[Rgb], transparent0: bool) -> Result<()> {
    let bytes = encode_indexed(w, h, indices, palette, transparent0)?;
    fsutil::atomic_write(path, &bytes)
}

/// Bytes of a deterministic 8-bit RGBA PNG.
pub fn encode_rgba(img: &RgbaImage) -> Vec<u8> {
    let stride = img.w * 4;
    let mut raw = vec![0u8; (stride + 1) * img.h];
    for y in 0..img.h {
        let row = &mut raw[y * (stride + 1) + 1..(y + 1) * (stride + 1)];
        for x in 0..img.w {
            let p = img.px[y * img.w + x];
            row[x * 4..x * 4 + 4].copy_from_slice(&[p.r, p.g, p.b, p.a]);
        }
    }
    assemble(img.w, img.h, 8, 6, None, None, &raw)
}

/// Write a deterministic RGBA PNG atomically (comparison sheets, previews).
pub fn write_rgba(path: &Path, img: &RgbaImage) -> Result<()> {
    fsutil::atomic_write(path, &encode_rgba(img))
}
