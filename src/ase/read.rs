//! `.aseprite` reader.

use super::model::*;
use super::props::*;
use super::*;
use crate::output::{PxlError, Result};

fn inflate(data: &[u8], expected: usize) -> R<Vec<u8>> {
    let out = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(data, expected.max(1) + 1)
        .map_err(|e| format!("zlib: {:?}", e.status))?;
    if out.len() != expected {
        return Err(format!("zlib data is {} bytes, expected {}", out.len(), expected));
    }
    Ok(out)
}

/// Parses an `.aseprite` file. Structure errors (header, frame sizes) are errors; chunks that cannot be parsed
/// exactly are preserved as [`Chunk::Unknown`].
pub fn read(bytes: &[u8]) -> Result<AseDoc> {
    read_inner(bytes).map_err(|e| PxlError::validation(format!("invalid .aseprite file: {}", e)))
}

fn read_inner(bytes: &[u8]) -> R<AseDoc> {
    let mut r = Rd::new(bytes);
    if bytes.len() < 128 {
        return Err("shorter than the 128 byte header".into());
    }
    let size = r.u32()? as usize;
    if r.u16()? != HEADER_MAGIC {
        return Err("bad header magic (expected 0xA5E0)".into());
    }
    if size != bytes.len() {
        return Err(format!("header file size {} does not match actual size {}", size, bytes.len()));
    }
    let nframes = r.u16()? as usize;
    let width = r.u16()?;
    let height = r.u16()?;
    let depth = r.u16()?;
    let mode = ColorMode::from_bits(depth).ok_or_else(|| format!("unsupported colour depth {}", depth))?;
    let flags = r.u32()?;
    let speed = r.u16()?;
    r.skip(8)?;
    let transparent_index = r.u8()?;
    r.skip(3)?;
    let num_colors = r.u16()?;
    let pixel_w = r.u8()?;
    let pixel_h = r.u8()?;
    let grid_x = r.i16()?;
    let grid_y = r.i16()?;
    let grid_w = r.u16()?;
    let grid_h = r.u16()?;
    let mut doc = AseDoc {
        width,
        height,
        mode,
        flags,
        speed,
        transparent_index,
        num_colors,
        pixel_w,
        pixel_h,
        grid_x,
        grid_y,
        grid_w,
        grid_h,
        frames: Vec::new(),
    };
    r.pos = 128;
    for fi in 0..nframes {
        let start = r.pos;
        let fsize = r.u32().map_err(|e| format!("frame {}: {}", fi, e))? as usize;
        if fsize < 16 || start + fsize > bytes.len() {
            return Err(format!("frame {}: bad frame size {}", fi, fsize));
        }
        if r.u16()? != FRAME_MAGIC {
            return Err(format!("frame {}: bad frame magic", fi));
        }
        let old = r.u16()? as usize;
        let duration = r.u16()?;
        r.skip(2)?;
        let new = r.u32()? as usize;
        let count = if new != 0 { new } else { old };
        let mut fr = Rd::new(&bytes[..start + fsize]);
        fr.pos = r.pos;
        let mut chunks = Vec::new();
        for ci in 0..count {
            let cstart = fr.pos;
            let csize = fr.u32().map_err(|e| format!("frame {} chunk {}: {}", fi, ci, e))? as usize;
            let ctype = fr.u16().map_err(|e| format!("frame {} chunk {}: {}", fi, ci, e))?;
            if csize < 6 || cstart + csize > start + fsize {
                return Err(format!("frame {} chunk {}: bad chunk size {}", fi, ci, csize));
            }
            let data = &bytes[cstart + 6..cstart + csize];
            fr.pos = cstart + csize;
            let chunk = match parse_chunk(&doc, ctype, data) {
                Ok(c) => c,
                Err(_) => Chunk::Unknown { chunk_type: ctype, bytes: data.to_vec() },
            };
            chunks.push(chunk);
        }
        if fr.pos != start + fsize {
            return Err(format!("frame {}: {} stray bytes after the last chunk", fi, start + fsize - fr.pos));
        }
        r.pos = start + fsize;
        doc.frames.push(Frame { duration, chunks });
    }
    if r.pos != bytes.len() {
        return Err(format!("{} trailing bytes after the last frame", bytes.len() - r.pos));
    }
    Ok(doc)
}

fn parse_chunk(doc: &AseDoc, ctype: u16, data: &[u8]) -> R<Chunk> {
    let mut r = Rd::new(data);
    let bpp = doc.mode.bytes_per_pixel();
    let chunk = match ctype {
        CHUNK_OLD_PALETTE_256 | CHUNK_OLD_PALETTE_64 => {
            let n = r.u16()? as usize;
            let mut packets = Vec::new();
            for _ in 0..n {
                let skip = r.u8()?;
                let cnt = match r.u8()? {
                    0 => 256,
                    c => c as usize,
                };
                let mut colors = Vec::with_capacity(cnt);
                for _ in 0..cnt {
                    let c = r.take(3)?;
                    colors.push([c[0], c[1], c[2]]);
                }
                packets.push(OldPalPacket { skip, colors });
            }
            Chunk::OldPalette { six_bit: ctype == CHUNK_OLD_PALETTE_64, packets }
        }
        CHUNK_LAYER => {
            let flags = r.u16()?;
            let lt = match r.u16()? {
                0 => LayerType::Normal,
                1 => LayerType::Group,
                2 => LayerType::Tilemap,
                t => return Err(format!("layer type {}", t)),
            };
            let child_level = r.u16()?;
            let default_w = r.u16()?;
            let default_h = r.u16()?;
            let blend_mode = r.u16()?;
            let opacity = r.u8()?;
            r.skip(3)?;
            let name = r.string()?;
            let tileset_index = if lt == LayerType::Tilemap { r.u32()? } else { 0 };
            let uuid = if doc.flags & 4 != 0 {
                let mut u = [0u8; 16];
                u.copy_from_slice(r.take(16)?);
                Some(u)
            } else {
                None
            };
            Chunk::Layer(Layer {
                flags,
                layer_type: lt,
                child_level,
                default_w,
                default_h,
                blend_mode,
                opacity,
                name,
                tileset_index,
                uuid,
            })
        }
        CHUNK_CEL => {
            let layer = r.u16()?;
            let x = r.i16()?;
            let y = r.i16()?;
            let opacity = r.u8()?;
            let ct = r.u16()?;
            let z_index = r.i16()?;
            r.skip(5)?;
            let data = match ct {
                0 => {
                    let w = r.u16()?;
                    let h = r.u16()?;
                    let pixels = r.take(w as usize * h as usize * bpp)?.to_vec();
                    CelData::Raw { w, h, pixels }
                }
                1 => CelData::Linked { frame: r.u16()? },
                2 => {
                    let w = r.u16()?;
                    let h = r.u16()?;
                    let rest = r.take(r.left())?;
                    CelData::Image { w, h, pixels: inflate(rest, w as usize * h as usize * bpp)? }
                }
                3 => {
                    let w = r.u16()?;
                    let h = r.u16()?;
                    let bits = r.u16()?;
                    if !matches!(bits, 8 | 16 | 32) {
                        return Err("tile bits".into());
                    }
                    let id_mask = r.u32()?;
                    let xflip_mask = r.u32()?;
                    let yflip_mask = r.u32()?;
                    let diag_mask = r.u32()?;
                    r.skip(10)?;
                    let n = w as usize * h as usize;
                    let tb = bits as usize / 8;
                    let rest = r.take(r.left())?;
                    let raw = inflate(rest, n * tb)?;
                    let tiles = raw
                        .chunks(tb)
                        .map(|c| c.iter().rev().fold(0u32, |a, &b| (a << 8) | b as u32))
                        .collect();
                    CelData::Tilemap { w, h, bits_per_tile: bits, id_mask, xflip_mask, yflip_mask, diag_mask, tiles }
                }
                t => return Err(format!("cel type {}", t)),
            };
            Chunk::Cel(Cel { layer, x, y, opacity, z_index, data })
        }
        CHUNK_CEL_EXTRA => {
            let flags = r.u32()?;
            let (x, y, w, h) = (r.i32()?, r.i32()?, r.i32()?, r.i32()?);
            r.skip(16)?;
            Chunk::CelExtra(CelExtra { flags, x, y, w, h })
        }
        CHUNK_COLOR_PROFILE => {
            let profile_type = r.u16()?;
            let flags = r.u16()?;
            let gamma = r.i32()?;
            r.skip(8)?;
            let icc = if profile_type == 2 {
                let n = r.u32()? as usize;
                r.take(n)?.to_vec()
            } else {
                Vec::new()
            };
            Chunk::ColorProfile(ColorProfile { profile_type, flags, gamma, icc })
        }
        CHUNK_EXTERNAL_FILES => {
            let n = r.u32()? as usize;
            r.skip(8)?;
            let mut v = Vec::new();
            for _ in 0..n {
                let id = r.u32()?;
                let kind = r.u8()?;
                r.skip(7)?;
                v.push(ExternalFile { id, kind, name: r.string()? });
            }
            Chunk::ExternalFiles(v)
        }
        CHUNK_MASK => {
            let x = r.i16()?;
            let y = r.i16()?;
            let w = r.u16()?;
            let h = r.u16()?;
            r.skip(8)?;
            let name = r.string()?;
            let bitmap = r.take(h as usize * ((w as usize + 7) / 8))?.to_vec();
            Chunk::Mask(Mask { x, y, w, h, name, bitmap })
        }
        CHUNK_PATH => return Ok(Chunk::Path(data.to_vec())),
        CHUNK_TAGS => {
            let n = r.u16()? as usize;
            r.skip(8)?;
            let mut v = Vec::new();
            for _ in 0..n {
                let from = r.u16()?;
                let to = r.u16()?;
                let direction = LoopDir::from_code(r.u8()?).ok_or("tag direction")?;
                let repeat = r.u16()?;
                r.skip(6)?;
                let c = r.take(3)?;
                let color = [c[0], c[1], c[2]];
                r.skip(1)?;
                v.push(Tag { from, to, direction, repeat, color, name: r.string()? });
            }
            Chunk::Tags(v)
        }
        CHUNK_PALETTE => {
            let new_size = r.u32()?;
            let first = r.u32()?;
            let last = r.u32()?;
            if last < first || last - first > 0xFFFF {
                return Err("palette range".into());
            }
            r.skip(8)?;
            let mut entries = Vec::new();
            for _ in first..=last {
                let fl = r.u16()?;
                let c = r.take(4)?;
                let rgba = [c[0], c[1], c[2], c[3]];
                let name = if fl & 1 != 0 { Some(r.string()?) } else { None };
                entries.push(PaletteEntry { rgba, name });
            }
            Chunk::Palette(PaletteChunk { new_size, first, entries })
        }
        CHUNK_USER_DATA => {
            let flags = r.u32()?;
            if flags & !7 != 0 {
                return Err("user data flags".into());
            }
            let mut ud = UserData::default();
            if flags & 1 != 0 {
                ud.text = Some(r.string()?);
            }
            if flags & 2 != 0 {
                let c = r.take(4)?;
                ud.color = Some([c[0], c[1], c[2], c[3]]);
            }
            if flags & 4 != 0 {
                ud.props = Some(read_prop_maps(&mut r)?);
            }
            Chunk::UserData(ud)
        }
        CHUNK_SLICE => {
            let n = r.u32()? as usize;
            let flags = r.u32()?;
            r.skip(4)?;
            let name = r.string()?;
            if n > r.left() {
                return Err("slice keys".into());
            }
            let mut keys = Vec::new();
            for _ in 0..n {
                let frame = r.u32()?;
                let (x, y, w, h) = (r.i32()?, r.i32()?, r.u32()?, r.u32()?);
                let center = if flags & 1 != 0 { Some((r.i32()?, r.i32()?, r.u32()?, r.u32()?)) } else { None };
                let pivot = if flags & 2 != 0 { Some((r.i32()?, r.i32()?)) } else { None };
                keys.push(SliceKey { frame, x, y, w, h, center, pivot });
            }
            Chunk::Slice(Slice { name, flags, keys })
        }
        CHUNK_TILESET => {
            let id = r.u32()?;
            let flags = r.u32()?;
            let num_tiles = r.u32()?;
            let tile_w = r.u16()?;
            let tile_h = r.u16()?;
            let base_index = r.i16()?;
            r.skip(14)?;
            let name = r.string()?;
            let external = if flags & 1 != 0 { Some((r.u32()?, r.u32()?)) } else { None };
            let tiles = if flags & 2 != 0 {
                let n = r.u32()? as usize;
                let comp = r.take(n)?;
                Some(inflate(comp, tile_w as usize * tile_h as usize * num_tiles as usize * bpp)?)
            } else {
                None
            };
            Chunk::Tileset(Tileset { id, flags, num_tiles, tile_w, tile_h, base_index, name, external, tiles })
        }
        _ => return Err("unknown".into()),
    };
    if r.left() != 0 {
        return Err("trailing bytes".into());
    }
    Ok(chunk)
}
