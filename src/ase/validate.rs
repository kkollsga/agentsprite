//! Strict byte level validator for `.aseprite` files, written directly against the published format
//! (independent of the typed reader). Our own output must produce zero problems.

use super::props::*;
use super::*;
use std::collections::BTreeSet;

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Problem {
    /// Byte offset in the file the problem refers to.
    pub offset: usize,
    pub frame: Option<usize>,
    pub chunk: Option<u16>,
    pub msg: String,
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "@{}", self.offset)?;
        if let Some(fr) = self.frame {
            write!(f, " frame {}", fr)?;
        }
        if let Some(c) = self.chunk {
            write!(f, " chunk 0x{:04X}", c)?;
        }
        write!(f, ": {}", self.msg)
    }
}

struct CelInfo {
    frame: usize,
    layer: usize,
    off: usize,
    cel_type: u16,
    link: Option<usize>,
    tile_max: Option<u32>,
}

struct V {
    probs: Vec<Problem>,
    frame: Option<usize>,
    chunk: Option<u16>,
    off: usize,
    nframes: usize,
    bpp: usize,
    indexed: bool,
    header_colors: u16,
    layers: Vec<(u16, u32, u16, usize)>, // type, tileset index, child level, offset
    tilesets: Vec<(u32, u32, usize)>,    // id, num tiles, offset
    cels: Vec<CelInfo>,
    palette_size: Option<usize>,
    new_pal: Vec<[u8; 4]>,
    max_index_used: Option<(u8, usize)>,
    uuid: bool,
    old_pal: Option<(Vec<[u8; 3]>, usize)>,
}

impl V {
    fn p(&mut self, msg: impl Into<String>) {
        self.probs.push(Problem { offset: self.off, frame: self.frame, chunk: self.chunk, msg: msg.into() });
    }
}

fn inflate_exact(data: &[u8], expected: usize) -> R<Vec<u8>> {
    let out = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(data, expected.max(1) + 1)
        .map_err(|e| format!("bad zlib stream ({:?})", e.status))?;
    if out.len() != expected {
        return Err(format!("zlib stream inflates to {} bytes, expected {}", out.len(), expected));
    }
    Ok(out)
}

/// Checks `bytes` against the file format; an empty result means the file is well formed.
pub fn validate(bytes: &[u8]) -> Vec<Problem> {
    let mut v = V {
        probs: Vec::new(),
        frame: None,
        chunk: None,
        off: 0,
        nframes: 0,
        bpp: 1,
        indexed: false,
        header_colors: 0,
        layers: Vec::new(),
        tilesets: Vec::new(),
        cels: Vec::new(),
        palette_size: None,
        new_pal: Vec::new(),
        max_index_used: None,
        uuid: false,
        old_pal: None,
    };
    run(&mut v, bytes);
    v.probs
}

fn run(v: &mut V, bytes: &[u8]) {
    if bytes.len() < 128 {
        v.p(format!("file is {} bytes, shorter than the 128 byte header", bytes.len()));
        return;
    }
    let mut r = Rd::new(bytes);
    let size = r.u32().unwrap() as usize;
    let magic = r.u16().unwrap();
    let nframes = r.u16().unwrap() as usize;
    let (w, h) = (r.u16().unwrap(), r.u16().unwrap());
    let depth = r.u16().unwrap();
    let flags = r.u32().unwrap();
    let _speed = r.u16().unwrap();
    let res1 = r.take(8).unwrap().to_vec();
    let _transparent = r.u8().unwrap();
    let res2 = r.take(3).unwrap().to_vec();
    v.header_colors = r.u16().unwrap();
    let (pw, ph) = (r.u8().unwrap(), r.u8().unwrap());
    r.skip(8).unwrap(); // grid
    let res3 = r.take(84).unwrap().to_vec();
    if magic != HEADER_MAGIC {
        v.p(format!("header magic is 0x{:04X}, expected 0xA5E0", magic));
    }
    if size != bytes.len() {
        v.p(format!("header file size {} != actual {}", size, bytes.len()));
    }
    if w == 0 || h == 0 {
        v.p("canvas width or height is 0");
    }
    match depth {
        8 => v.indexed = true,
        16 | 32 => {}
        d => v.p(format!("colour depth {} is not 8, 16 or 32", d)),
    }
    v.bpp = (depth as usize / 8).max(1);
    v.uuid = flags & 4 != 0;
    if flags & !7 != 0 {
        v.p(format!("unknown header flags 0x{:X}", flags));
    }
    if pw == 0 || ph == 0 {
        v.p("pixel ratio has a zero component");
    }
    if res1.iter().chain(res2.iter()).chain(res3.iter()).any(|b| *b != 0) {
        v.p("reserved header bytes are not zero");
    }
    if nframes == 0 {
        v.p("file has no frames");
    }
    v.nframes = nframes;

    let mut pos = 128usize;
    for fi in 0..nframes {
        v.frame = Some(fi);
        v.chunk = None;
        v.off = pos;
        if bytes.len() < pos + 16 {
            v.p("truncated frame header");
            return;
        }
        let mut fr = Rd::new(&bytes[pos..]);
        let fsize = fr.u32().unwrap() as usize;
        let fmagic = fr.u16().unwrap();
        let old = fr.u16().unwrap() as usize;
        let dur = fr.u16().unwrap();
        let res = fr.take(2).unwrap().to_vec();
        let new = fr.u32().unwrap() as usize;
        if fmagic != FRAME_MAGIC {
            v.p(format!("frame magic is 0x{:04X}, expected 0xF1FA", fmagic));
        }
        if dur == 0 {
            v.p("frame duration is 0");
        }
        if res.iter().any(|b| *b != 0) {
            v.p("reserved frame bytes are not zero");
        }
        if fsize < 16 || pos + fsize > bytes.len() {
            v.p(format!("frame size {} runs past the end of the file or is below 16", fsize));
            return;
        }
        let end = pos + fsize;
        let mut cpos = pos + 16;
        let mut n = 0usize;
        let mut prev: Option<u16> = None;
        while cpos < end {
            v.off = cpos;
            v.chunk = None;
            if end - cpos < 6 {
                v.p("fewer than 6 bytes left for a chunk header");
                break;
            }
            let csize = u32::from_le_bytes(bytes[cpos..cpos + 4].try_into().unwrap()) as usize;
            let ctype = u16::from_le_bytes(bytes[cpos + 4..cpos + 6].try_into().unwrap());
            v.chunk = Some(ctype);
            if csize < 6 || cpos + csize > end {
                v.p(format!("chunk size {} is below 6 or runs past the frame", csize));
                break;
            }
            let data = &bytes[cpos + 6..cpos + csize];
            if let Err(e) = check_chunk(v, fi, n, ctype, data, prev, cpos) {
                v.off = cpos;
                v.p(e);
            }
            prev = Some(ctype);
            n += 1;
            cpos += csize;
        }
        v.chunk = None;
        v.off = pos;
        if cpos != end {
            v.p(format!("chunks end at {} but the frame ends at {}", cpos, end));
        }
        if new != 0 && new != n {
            v.p(format!("new chunk count {} but the frame holds {} chunks", new, n));
        }
        let want_old = if n >= 0xFFFF { 0xFFFF } else { n };
        if old != want_old && old != 0xFFFF {
            v.p(format!("old chunk count {} but the frame holds {} chunks", old, n));
        }
        if new == 0 && old == 0xFFFF {
            v.p("both chunk counts are unusable (old = 0xFFFF, new = 0)");
        }
        pos = end;
    }
    v.frame = None;
    v.chunk = None;
    v.off = pos;
    if pos != bytes.len() {
        v.p(format!("frames end at {} but the file is {} bytes", pos, bytes.len()));
    }
    finish(v, w, h);
}

fn finish(v: &mut V, _w: u16, _h: u16) {
    v.frame = None;
    v.chunk = None;
    if v.layers.is_empty() {
        v.off = 128;
        v.p("file has no layers");
    }
    if v.indexed && v.palette_size.is_none() {
        v.off = 128;
        v.p("indexed file has no palette chunk");
    }
    if let (Some(ps), hc) = (v.palette_size, v.header_colors) {
        if hc != 0 && hc as usize != ps {
            v.off = 128;
            v.p(format!("header colour count {} != palette size {}", hc, ps));
        }
    }
    if let Some((cols, off)) = v.old_pal.clone() {
        if let Some(i) = cols.iter().enumerate().position(|(i, c)| v.new_pal.get(i).map_or(false, |n| n[..3] != c[..])) {
            v.off = off;
            v.chunk = Some(CHUNK_OLD_PALETTE_256);
            v.p(format!("old palette entry {} differs from the new palette", i));
        }
    }
    // layers: tilemap tileset references, level sanity
    let mut prev_level = 0u16;
    for (i, &(ty, ts, lvl, off)) in v.layers.clone().iter().enumerate() {
        v.off = off;
        if (i == 0 && lvl != 0) || lvl > prev_level + 1 {
            v.p(format!("layer {} child level {} jumps from {}", i, lvl, prev_level));
        }
        prev_level = lvl;
        if ty == 2 && !v.tilesets.iter().any(|t| t.0 == ts) {
            v.p(format!("tilemap layer {} refers to missing tileset {}", i, ts));
        }
    }
    let mut seen: BTreeSet<(usize, usize)> = BTreeSet::new();
    let cels: Vec<(usize, usize, usize, u16, Option<usize>, Option<u32>)> =
        v.cels.iter().map(|c| (c.frame, c.layer, c.off, c.cel_type, c.link, c.tile_max)).collect();
    for (frame, layer, off, ct, link, tile_max) in cels.iter().cloned() {
        v.frame = Some(frame);
        v.chunk = Some(CHUNK_CEL);
        v.off = off;
        if !seen.insert((frame, layer)) {
            v.p(format!("two cels for layer {} in the same frame", layer));
        }
        let Some(&(lt, ts, _, _)) = v.layers.get(layer) else {
            v.p(format!("cel refers to layer {} but only {} layers exist", layer, v.layers.len()));
            continue;
        };
        if lt == 1 {
            v.p(format!("cel on group layer {}", layer));
        }
        if (lt == 2) != (ct == 3) {
            v.p(format!("cel type {} does not match layer {} type {}", ct, layer, lt));
        }
        if let Some(src) = link {
            let target = cels.iter().find(|c| c.0 == src && c.1 == layer);
            match target {
                None => v.p(format!("linked cel points to frame {} which has no cel on layer {}", src, layer)),
                Some(t) if t.3 == 1 => {
                    // chains are legal but must terminate on an earlier frame; check target is not linked back
                    if t.4.map_or(false, |s| s >= src) {
                        v.p("linked cel chain does not move to earlier frames");
                    }
                }
                _ => {}
            }
        }
        if let (Some(max), true) = (tile_max, lt == 2) {
            if let Some(t) = v.tilesets.iter().find(|t| t.0 == ts) {
                if max >= t.1 {
                    let n = t.1;
                    v.p(format!("tilemap uses tile id {} but tileset {} has {} tiles", max, ts, n));
                }
            }
        }
    }
    if let Some((m, off)) = v.max_index_used {
        if let Some(ps) = v.palette_size {
            if m as usize >= ps {
                v.frame = None;
                v.chunk = Some(CHUNK_CEL);
                v.off = off;
                v.p(format!("pixel index {} is outside the {} entry palette", m, ps));
            }
        }
    }
}

fn check_chunk(v: &mut V, fi: usize, idx: usize, ctype: u16, data: &[u8], prev: Option<u16>, off: usize) -> R<()> {
    let mut r = Rd::new(data);
    match ctype {
        CHUNK_OLD_PALETTE_256 | CHUNK_OLD_PALETTE_64 => {
            let n = r.u16()? as usize;
            let mut idx = 0usize;
            let mut cols: Vec<[u8; 3]> = Vec::new();
            for _ in 0..n {
                let skip = r.u8()? as usize;
                let cnt = match r.u8()? {
                    0 => 256,
                    c => c as usize,
                };
                idx += skip;
                cols.resize(idx + cnt, [0; 3]);
                for k in 0..cnt {
                    let c = r.take(3)?;
                    if ctype == CHUNK_OLD_PALETTE_64 && c.iter().any(|b| *b > 63) {
                        return Err("6 bit palette value above 63".into());
                    }
                    cols[idx + k] = [c[0], c[1], c[2]];
                }
                idx += cnt;
            }
            if idx > 256 {
                return Err(format!("old palette addresses {} entries (max 256)", idx));
            }
            if ctype == CHUNK_OLD_PALETTE_256 && v.old_pal.is_none() {
                v.old_pal = Some((cols, off));
            }
            if v.palette_size.is_none() {
                v.palette_size = Some(idx);
            }
        }
        CHUNK_LAYER => {
            let flags = r.u16()?;
            let ty = r.u16()?;
            let level = r.u16()?;
            r.skip(4)?;
            let blend = r.u16()?;
            let _op = r.u8()?;
            r.skip(3)?;
            let name = r.string()?;
            if ty > 2 {
                return Err(format!("layer type {} is not 0, 1 or 2", ty));
            }
            if flags & !127 != 0 {
                return Err(format!("unknown layer flags 0x{:X}", flags));
            }
            if blend > 18 {
                return Err(format!("blend mode {} above 18", blend));
            }
            if name.is_empty() {
                return Err("layer has an empty name".into());
            }
            let ts = if ty == 2 { r.u32()? } else { 0 };
            if v.layers.is_empty() && fi != 0 {
                return Err("first layer chunk is not in frame 0".into());
            }
            if v.uuid {
                r.skip(16)?;
            }
            v.layers.push((ty, ts, level, off));
        }
        CHUNK_CEL => {
            let layer = r.u16()? as usize;
            r.skip(4)?;
            let _op = r.u8()?;
            let ct = r.u16()?;
            let _z = r.i16()?;
            r.skip(5)?;
            let mut info = CelInfo { frame: fi, layer, off, cel_type: ct, link: None, tile_max: None };
            match ct {
                0 | 2 => {
                    let w = r.u16()? as usize;
                    let h = r.u16()? as usize;
                    let want = w * h * v.bpp;
                    let px = if ct == 0 {
                        r.take(want)?.to_vec()
                    } else {
                        let rest = r.take(r.left())?;
                        inflate_exact(rest, want)?
                    };
                    if w == 0 || h == 0 {
                        return Err("cel with zero width or height".into());
                    }
                    if v.indexed {
                        let m = *px.iter().max().unwrap();
                        if v.max_index_used.map_or(true, |(o, _)| m > o) {
                            v.max_index_used = Some((m, off));
                        }
                    }
                }
                1 => {
                    let src = r.u16()? as usize;
                    if src >= fi {
                        return Err(format!("linked cel points at frame {} which is not before frame {}", src, fi));
                    }
                    info.link = Some(src);
                }
                3 => {
                    let w = r.u16()? as usize;
                    let h = r.u16()? as usize;
                    let bits = r.u16()? as usize;
                    let idm = r.u32()?;
                    r.skip(12)?;
                    r.skip(10)?;
                    if !matches!(bits, 8 | 16 | 32) {
                        return Err(format!("tile bit width {} is not 8, 16 or 32", bits));
                    }
                    if idm == 0 {
                        return Err("tile id mask is 0".into());
                    }
                    let rest = r.take(r.left())?;
                    let raw = inflate_exact(rest, w * h * bits / 8)?;
                    let tb = bits / 8;
                    let mut max = 0u32;
                    for c in raw.chunks(tb) {
                        let t = c.iter().rev().fold(0u32, |a, &b| (a << 8) | b as u32);
                        max = max.max(t & idm);
                    }
                    info.tile_max = Some(max);
                }
                t => return Err(format!("cel type {} is not 0..3", t)),
            }
            v.cels.push(info);
        }
        CHUNK_CEL_EXTRA => {
            if prev != Some(CHUNK_CEL) {
                return Err("cel extra chunk does not directly follow a cel chunk".into());
            }
            let flags = r.u32()?;
            if flags & !1 != 0 {
                return Err("unknown cel extra flags".into());
            }
            r.skip(16 + 16)?;
        }
        CHUNK_COLOR_PROFILE => {
            let t = r.u16()?;
            let _f = r.u16()?;
            r.skip(4 + 8)?;
            match t {
                0 | 1 => {}
                2 => {
                    let n = r.u32()? as usize;
                    r.skip(n)?;
                }
                _ => return Err(format!("colour profile type {} is not 0..2", t)),
            }
        }
        CHUNK_EXTERNAL_FILES => {
            let n = r.u32()?;
            r.skip(8)?;
            for _ in 0..n {
                r.skip(4)?;
                let k = r.u8()?;
                r.skip(7)?;
                r.string()?;
                if k > 3 {
                    return Err(format!("external file type {} above 3", k));
                }
            }
        }
        CHUNK_MASK => {
            r.skip(4)?;
            let w = r.u16()? as usize;
            let h = r.u16()? as usize;
            r.skip(8)?;
            r.string()?;
            r.skip(h * ((w + 7) / 8))?;
        }
        CHUNK_PATH => {}
        CHUNK_TAGS => {
            let n = r.u16()? as usize;
            r.skip(8)?;
            if n == 0 {
                return Err("tags chunk with 0 tags".into());
            }
            for _ in 0..n {
                let from = r.u16()? as usize;
                let to = r.u16()? as usize;
                let dir = r.u8()?;
                let _rep = r.u16()?;
                r.skip(6 + 3 + 1)?;
                let name = r.string()?;
                if from > to {
                    return Err(format!("tag `{}` range {}..{} is reversed", name, from, to));
                }
                if to >= v.nframes {
                    return Err(format!("tag `{}` ends at frame {} but there are {} frames", name, to, v.nframes));
                }
                if dir > 3 {
                    return Err(format!("tag `{}` direction {} above 3", name, dir));
                }
                if name.is_empty() {
                    return Err("tag with an empty name".into());
                }
            }
        }
        CHUNK_PALETTE => {
            let new_size = r.u32()? as usize;
            let first = r.u32()? as usize;
            let last = r.u32()? as usize;
            r.skip(8)?;
            if new_size == 0 || new_size > 65536 {
                return Err(format!("palette size {} out of range", new_size));
            }
            if first > last || last >= new_size {
                return Err(format!("palette range {}..={} outside size {}", first, last, new_size));
            }
            if v.new_pal.len() < new_size {
                v.new_pal.resize(new_size, [0, 0, 0, 255]);
            }
            for i in first..=last {
                let fl = r.u16()?;
                let c = r.take(4)?;
                v.new_pal[i] = [c[0], c[1], c[2], c[3]];
                if fl & !1 != 0 {
                    return Err(format!("palette entry {} has unknown flags", i));
                }
                if fl & 1 != 0 {
                    r.string()?;
                }
            }
            v.palette_size = Some(new_size);
        }
        CHUNK_USER_DATA => {
            let flags = r.u32()?;
            if flags & !7 != 0 {
                return Err(format!("unknown user data flags 0x{:X}", flags));
            }
            if idx == 0 {
                return Err("user data chunk is the first chunk of its frame".into());
            }
            if flags & 1 != 0 {
                r.string()?;
            }
            if flags & 2 != 0 {
                r.skip(4)?;
            }
            if flags & 4 != 0 {
                read_prop_maps(&mut r)?;
            }
        }
        CHUNK_SLICE => {
            let n = r.u32()? as usize;
            let flags = r.u32()?;
            r.skip(4)?;
            let name = r.string()?;
            if flags & !3 != 0 {
                return Err(format!("unknown slice flags 0x{:X}", flags));
            }
            if n == 0 {
                return Err(format!("slice `{}` has no keys", name));
            }
            let mut last_frame: Option<u32> = None;
            for _ in 0..n {
                let frame = r.u32()?;
                r.skip(8)?;
                let (sw, sh) = (r.u32()?, r.u32()?);
                if frame as usize >= v.nframes {
                    return Err(format!("slice `{}` key on frame {} of {}", name, frame, v.nframes));
                }
                if last_frame.map_or(false, |l| frame <= l) {
                    return Err(format!("slice `{}` keys are not in increasing frame order", name));
                }
                last_frame = Some(frame);
                if sw == 0 || sh == 0 {
                    return Err(format!("slice `{}` has an empty rectangle", name));
                }
                if flags & 1 != 0 {
                    let (cx, cy, cw, ch) = (r.i32()?, r.i32()?, r.u32()?, r.u32()?);
                    if cx < 0 || cy < 0 || cx as i64 + cw as i64 > sw as i64 || cy as i64 + ch as i64 > sh as i64 {
                        return Err(format!("slice `{}` 9-patch centre is outside the slice", name));
                    }
                }
                if flags & 2 != 0 {
                    r.skip(8)?;
                }
            }
        }
        CHUNK_TILESET => {
            let id = r.u32()?;
            let flags = r.u32()?;
            let n = r.u32()?;
            let tw = r.u16()? as usize;
            let th = r.u16()? as usize;
            r.skip(2 + 14)?;
            r.string()?;
            if flags & !63 != 0 {
                return Err(format!("unknown tileset flags 0x{:X}", flags));
            }
            if flags & 3 == 0 || flags & 3 == 3 {
                // Aseprite requires exactly one storage; both is legal per the spec text but we never write it.
                if flags & 3 == 0 {
                    return Err("tileset has neither external link nor embedded tiles".into());
                }
            }
            if tw == 0 || th == 0 {
                return Err("tileset with zero tile size".into());
            }
            if flags & 1 != 0 {
                r.skip(8)?;
            }
            if flags & 2 != 0 {
                let len = r.u32()? as usize;
                let comp = r.take(len)?;
                inflate_exact(comp, tw * th * n as usize * v.bpp)?;
            }
            if v.tilesets.iter().any(|t| t.0 == id) {
                return Err(format!("duplicate tileset id {}", id));
            }
            v.tilesets.push((id, n, off));
        }
        t => return Err(format!("unknown chunk type 0x{:04X}", t)),
    }
    if r.left() != 0 {
        return Err(format!("{} unread bytes at the end of the chunk", r.left()));
    }
    Ok(())
}
