//! `.aseprite` writer. Deterministic: fixed zlib level, canonical reserved bytes.

use super::model::*;
use super::props::*;
use super::*;

/// zlib level used for every compressed block (fixed so output is byte identical across runs).
pub const ZLIB_LEVEL: u8 = 6;

fn deflate(data: &[u8]) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec_zlib(data, ZLIB_LEVEL)
}

/// Serialises a document. Never fails; inconsistent models produce files that [`validate`] flags.
pub fn write(doc: &AseDoc) -> Vec<u8> {
    let mut w = Wr::default();
    w.u32(0);
    w.u16(HEADER_MAGIC);
    w.u16(doc.frames.len() as u16);
    w.u16(doc.width);
    w.u16(doc.height);
    w.u16(doc.mode.bits());
    w.u32(doc.flags);
    w.u16(doc.speed);
    w.zeros(8);
    w.u8(doc.transparent_index);
    w.zeros(3);
    w.u16(doc.num_colors);
    w.u8(doc.pixel_w);
    w.u8(doc.pixel_h);
    w.i16(doc.grid_x);
    w.i16(doc.grid_y);
    w.u16(doc.grid_w);
    w.u16(doc.grid_h);
    w.zeros(84);
    debug_assert_eq!(w.b.len(), 128);
    for f in &doc.frames {
        let mut body = Wr::default();
        for c in &f.chunks {
            let data = chunk_bytes(doc, c);
            body.u32(data.len() as u32 + 6);
            body.u16(c.chunk_type());
            body.bytes(&data);
        }
        let n = f.chunks.len();
        w.u32(body.b.len() as u32 + 16);
        w.u16(FRAME_MAGIC);
        w.u16(if n >= 0xFFFF { 0xFFFF } else { n as u16 });
        w.u16(f.duration);
        w.zeros(2);
        w.u32(n as u32);
        w.bytes(&body.b);
    }
    let len = w.b.len() as u32;
    w.b[0..4].copy_from_slice(&len.to_le_bytes());
    w.b
}

fn chunk_bytes(doc: &AseDoc, c: &Chunk) -> Vec<u8> {
    let mut w = Wr::default();
    match c {
        Chunk::OldPalette { packets, .. } => {
            w.u16(packets.len() as u16);
            for p in packets {
                w.u8(p.skip);
                w.u8(if p.colors.len() >= 256 { 0 } else { p.colors.len() as u8 });
                for col in &p.colors {
                    w.bytes(col);
                }
            }
        }
        Chunk::Layer(l) => {
            w.u16(l.flags);
            w.u16(l.layer_type.code());
            w.u16(l.child_level);
            w.u16(l.default_w);
            w.u16(l.default_h);
            w.u16(l.blend_mode);
            w.u8(l.opacity);
            w.zeros(3);
            w.string(&l.name);
            if l.layer_type == LayerType::Tilemap {
                w.u32(l.tileset_index);
            }
            if doc.flags & 4 != 0 {
                w.bytes(&l.uuid.unwrap_or([0; 16]));
            }
        }
        Chunk::Cel(c) => {
            w.u16(c.layer);
            w.i16(c.x);
            w.i16(c.y);
            w.u8(c.opacity);
            w.u16(match c.data {
                CelData::Raw { .. } => 0,
                CelData::Linked { .. } => 1,
                CelData::Image { .. } => 2,
                CelData::Tilemap { .. } => 3,
            });
            w.i16(c.z_index);
            w.zeros(5);
            match &c.data {
                CelData::Raw { w: cw, h, pixels } => {
                    w.u16(*cw);
                    w.u16(*h);
                    w.bytes(pixels);
                }
                CelData::Linked { frame } => w.u16(*frame),
                CelData::Image { w: cw, h, pixels } => {
                    w.u16(*cw);
                    w.u16(*h);
                    w.bytes(&deflate(pixels));
                }
                CelData::Tilemap { w: cw, h, bits_per_tile, id_mask, xflip_mask, yflip_mask, diag_mask, tiles } => {
                    w.u16(*cw);
                    w.u16(*h);
                    w.u16(*bits_per_tile);
                    w.u32(*id_mask);
                    w.u32(*xflip_mask);
                    w.u32(*yflip_mask);
                    w.u32(*diag_mask);
                    w.zeros(10);
                    let tb = *bits_per_tile as usize / 8;
                    let mut raw = Vec::with_capacity(tiles.len() * tb);
                    for t in tiles {
                        raw.extend_from_slice(&t.to_le_bytes()[..tb.min(4)]);
                    }
                    w.bytes(&deflate(&raw));
                }
            }
        }
        Chunk::CelExtra(e) => {
            w.u32(e.flags);
            w.i32(e.x);
            w.i32(e.y);
            w.i32(e.w);
            w.i32(e.h);
            w.zeros(16);
        }
        Chunk::ColorProfile(p) => {
            w.u16(p.profile_type);
            w.u16(p.flags);
            w.i32(p.gamma);
            w.zeros(8);
            if p.profile_type == 2 {
                w.u32(p.icc.len() as u32);
                w.bytes(&p.icc);
            }
        }
        Chunk::ExternalFiles(v) => {
            w.u32(v.len() as u32);
            w.zeros(8);
            for e in v {
                w.u32(e.id);
                w.u8(e.kind);
                w.zeros(7);
                w.string(&e.name);
            }
        }
        Chunk::Mask(m) => {
            w.i16(m.x);
            w.i16(m.y);
            w.u16(m.w);
            w.u16(m.h);
            w.zeros(8);
            w.string(&m.name);
            w.bytes(&m.bitmap);
        }
        Chunk::Path(b) => w.bytes(b),
        Chunk::Tags(v) => {
            w.u16(v.len() as u16);
            w.zeros(8);
            for t in v {
                w.u16(t.from);
                w.u16(t.to);
                w.u8(t.direction.code());
                w.u16(t.repeat);
                w.zeros(6);
                w.bytes(&t.color);
                w.u8(0);
                w.string(&t.name);
            }
        }
        Chunk::Palette(p) => {
            w.u32(p.new_size);
            w.u32(p.first);
            w.u32(p.first.wrapping_add(p.entries.len() as u32).wrapping_sub(1));
            w.zeros(8);
            for e in &p.entries {
                w.u16(e.name.is_some() as u16);
                w.bytes(&e.rgba);
                if let Some(n) = &e.name {
                    w.string(n);
                }
            }
        }
        Chunk::UserData(u) => {
            let flags = u.text.is_some() as u32 | (u.color.is_some() as u32) << 1 | (u.props.is_some() as u32) << 2;
            w.u32(flags);
            if let Some(t) = &u.text {
                w.string(t);
            }
            if let Some(c) = &u.color {
                w.bytes(c);
            }
            if let Some(p) = &u.props {
                write_prop_maps(&mut w, p);
            }
        }
        Chunk::Slice(s) => {
            w.u32(s.keys.len() as u32);
            w.u32(s.flags);
            w.zeros(4);
            w.string(&s.name);
            for k in &s.keys {
                w.u32(k.frame);
                w.i32(k.x);
                w.i32(k.y);
                w.u32(k.w);
                w.u32(k.h);
                if s.flags & 1 != 0 {
                    let (cx, cy, cw, ch) = k.center.unwrap_or((0, 0, 0, 0));
                    w.i32(cx);
                    w.i32(cy);
                    w.u32(cw);
                    w.u32(ch);
                }
                if s.flags & 2 != 0 {
                    let (px, py) = k.pivot.unwrap_or((0, 0));
                    w.i32(px);
                    w.i32(py);
                }
            }
        }
        Chunk::Tileset(t) => {
            w.u32(t.id);
            w.u32(t.flags);
            w.u32(t.num_tiles);
            w.u16(t.tile_w);
            w.u16(t.tile_h);
            w.i16(t.base_index);
            w.zeros(14);
            w.string(&t.name);
            if t.flags & 1 != 0 {
                let (a, b) = t.external.unwrap_or((0, 0));
                w.u32(a);
                w.u32(b);
            }
            if t.flags & 2 != 0 {
                let z = deflate(t.tiles.as_deref().unwrap_or(&[]));
                w.u32(z.len() as u32);
                w.bytes(&z);
            }
        }
        Chunk::Unknown { bytes, .. } => w.bytes(bytes),
    }
    w.b
}
