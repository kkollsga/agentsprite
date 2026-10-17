//! Byte cursor/writer helpers and the user-data property codec.

use super::model::*;

pub(crate) type R<T> = std::result::Result<T, String>;

pub(crate) struct Rd<'a> {
    pub b: &'a [u8],
    pub pos: usize,
}

impl<'a> Rd<'a> {
    pub fn new(b: &'a [u8]) -> Rd<'a> {
        Rd { b, pos: 0 }
    }
    pub fn left(&self) -> usize {
        self.b.len() - self.pos
    }
    pub fn take(&mut self, n: usize) -> R<&'a [u8]> {
        if self.left() < n {
            return Err(format!("need {} bytes at offset {}, only {} left", n, self.pos, self.left()));
        }
        let s = &self.b[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    pub fn skip(&mut self, n: usize) -> R<()> {
        self.take(n).map(|_| ())
    }
    pub fn u8(&mut self) -> R<u8> {
        Ok(self.take(1)?[0])
    }
    pub fn i8(&mut self) -> R<i8> {
        Ok(self.u8()? as i8)
    }
    pub fn u16(&mut self) -> R<u16> {
        let s = self.take(2)?;
        Ok(u16::from_le_bytes([s[0], s[1]]))
    }
    pub fn i16(&mut self) -> R<i16> {
        Ok(self.u16()? as i16)
    }
    pub fn u32(&mut self) -> R<u32> {
        let s = self.take(4)?;
        Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }
    pub fn i32(&mut self) -> R<i32> {
        Ok(self.u32()? as i32)
    }
    pub fn u64(&mut self) -> R<u64> {
        let s = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(s);
        Ok(u64::from_le_bytes(a))
    }
    pub fn string(&mut self) -> R<String> {
        let n = self.u16()? as usize;
        let s = self.take(n)?;
        String::from_utf8(s.to_vec()).map_err(|_| "string is not valid UTF-8".to_string())
    }
}

#[derive(Default)]
pub(crate) struct Wr {
    pub b: Vec<u8>,
}

impl Wr {
    pub fn u8(&mut self, v: u8) {
        self.b.push(v);
    }
    pub fn u16(&mut self, v: u16) {
        self.b.extend_from_slice(&v.to_le_bytes());
    }
    pub fn i16(&mut self, v: i16) {
        self.u16(v as u16);
    }
    pub fn u32(&mut self, v: u32) {
        self.b.extend_from_slice(&v.to_le_bytes());
    }
    pub fn i32(&mut self, v: i32) {
        self.u32(v as u32);
    }
    pub fn u64(&mut self, v: u64) {
        self.b.extend_from_slice(&v.to_le_bytes());
    }
    pub fn zeros(&mut self, n: usize) {
        self.b.resize(self.b.len() + n, 0);
    }
    pub fn bytes(&mut self, v: &[u8]) {
        self.b.extend_from_slice(v);
    }
    pub fn string(&mut self, s: &str) {
        let n = s.len().min(u16::MAX as usize);
        self.u16(n as u16);
        self.b.extend_from_slice(&s.as_bytes()[..n]);
    }
}

// ---- properties ----

fn read_value_data(r: &mut Rd, ty: u16, depth: u32) -> R<PropValue> {
    if depth > 32 {
        return Err("properties nested too deeply".into());
    }
    Ok(match ty {
        0 => PropValue::Null,
        1 => PropValue::Bool(r.u8()? != 0),
        2 => PropValue::I8(r.i8()?),
        3 => PropValue::U8(r.u8()?),
        4 => PropValue::I16(r.i16()?),
        5 => PropValue::U16(r.u16()?),
        6 => PropValue::I32(r.i32()?),
        7 => PropValue::U32(r.u32()?),
        8 => PropValue::I64(r.u64()? as i64),
        9 => PropValue::U64(r.u64()?),
        10 => PropValue::Fixed(r.i32()?),
        11 => PropValue::Float(f32::from_bits(r.u32()?)),
        12 => PropValue::Double(f64::from_bits(r.u64()?)),
        13 => PropValue::Str(r.string()?),
        14 => PropValue::Point(r.i32()?, r.i32()?),
        15 => PropValue::Size(r.i32()?, r.i32()?),
        16 => PropValue::Rect { x: r.i32()?, y: r.i32()?, w: r.i32()?, h: r.i32()? },
        17 => {
            let n = r.u32()? as usize;
            let et = r.u16()?;
            if n > r.left() {
                return Err("vector longer than the data".into());
            }
            let mut items = Vec::with_capacity(n);
            for _ in 0..n {
                let t = if et == 0 { r.u16()? } else { et };
                items.push(read_value_data(r, t, depth + 1)?);
            }
            PropValue::Vector { elem_type: et, items }
        }
        18 => PropValue::Map(read_props(r, depth + 1)?),
        19 => {
            let mut u = [0u8; 16];
            u.copy_from_slice(r.take(16)?);
            PropValue::Uuid(u)
        }
        t => return Err(format!("unknown property type {}", t)),
    })
}

fn read_props(r: &mut Rd, depth: u32) -> R<Vec<(String, PropValue)>> {
    let n = r.u32()? as usize;
    if n > r.left() {
        return Err("property count larger than the data".into());
    }
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        let name = r.string()?;
        let ty = r.u16()?;
        v.push((name, read_value_data(r, ty, depth)?));
    }
    Ok(v)
}

/// Reads the properties block (starting at its total-size field).
pub(crate) fn read_prop_maps(r: &mut Rd) -> R<Vec<PropMap>> {
    let start = r.pos;
    let total = r.u32()? as usize;
    let nmaps = r.u32()? as usize;
    if nmaps > r.left() {
        return Err("properties map count larger than the data".into());
    }
    let mut maps = Vec::with_capacity(nmaps);
    for _ in 0..nmaps {
        let key = r.u32()?;
        let props = read_props(r, 0)?;
        maps.push(PropMap { key, props });
    }
    if r.pos - start != total {
        return Err(format!("properties size field {} does not match {} bytes parsed", total, r.pos - start));
    }
    Ok(maps)
}

fn write_value_data(w: &mut Wr, v: &PropValue) {
    match v {
        PropValue::Null => {}
        PropValue::Bool(b) => w.u8(*b as u8),
        PropValue::I8(x) => w.u8(*x as u8),
        PropValue::U8(x) => w.u8(*x),
        PropValue::I16(x) => w.i16(*x),
        PropValue::U16(x) => w.u16(*x),
        PropValue::I32(x) => w.i32(*x),
        PropValue::U32(x) => w.u32(*x),
        PropValue::I64(x) => w.u64(*x as u64),
        PropValue::U64(x) => w.u64(*x),
        PropValue::Fixed(x) => w.i32(*x),
        PropValue::Float(x) => w.u32(x.to_bits()),
        PropValue::Double(x) => w.u64(x.to_bits()),
        PropValue::Str(s) => w.string(s),
        PropValue::Point(x, y) | PropValue::Size(x, y) => {
            w.i32(*x);
            w.i32(*y);
        }
        PropValue::Rect { x, y, w: ww, h } => {
            w.i32(*x);
            w.i32(*y);
            w.i32(*ww);
            w.i32(*h);
        }
        PropValue::Vector { elem_type, items } => {
            w.u32(items.len() as u32);
            w.u16(*elem_type);
            for it in items {
                if *elem_type == 0 {
                    w.u16(it.type_code());
                }
                write_value_data(w, it);
            }
        }
        PropValue::Map(m) => write_props(w, m),
        PropValue::Uuid(u) => w.bytes(u),
    }
}

fn write_props(w: &mut Wr, props: &[(String, PropValue)]) {
    w.u32(props.len() as u32);
    for (name, v) in props {
        w.string(name);
        w.u16(v.type_code());
        write_value_data(w, v);
    }
}

pub(crate) fn write_prop_maps(w: &mut Wr, maps: &[PropMap]) {
    let mut inner = Wr::default();
    inner.u32(maps.len() as u32);
    for m in maps {
        inner.u32(m.key);
        write_props(&mut inner, &m.props);
    }
    w.u32(inner.b.len() as u32 + 4);
    w.bytes(&inner.b);
}
