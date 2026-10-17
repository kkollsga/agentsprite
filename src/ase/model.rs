//! The typed document model.

/// Pixel format of the file (header "color depth").
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ColorMode {
    Indexed,
    Grayscale,
    Rgba,
}

impl ColorMode {
    pub fn bits(self) -> u16 {
        match self {
            ColorMode::Indexed => 8,
            ColorMode::Grayscale => 16,
            ColorMode::Rgba => 32,
        }
    }
    pub fn bytes_per_pixel(self) -> usize {
        self.bits() as usize / 8
    }
    pub fn from_bits(b: u16) -> Option<ColorMode> {
        match b {
            8 => Some(ColorMode::Indexed),
            16 => Some(ColorMode::Grayscale),
            32 => Some(ColorMode::Rgba),
            _ => None,
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct AseDoc {
    pub width: u16,
    pub height: u16,
    pub mode: ColorMode,
    /// Header flags: 1 layer opacity valid, 2 group opacity/blend valid, 4 layers carry UUIDs.
    pub flags: u32,
    /// Deprecated default frame duration (ms).
    pub speed: u16,
    /// Palette index that is transparent in indexed mode.
    pub transparent_index: u8,
    /// Header colour count (0 means 256).
    pub num_colors: u16,
    pub pixel_w: u8,
    pub pixel_h: u8,
    pub grid_x: i16,
    pub grid_y: i16,
    pub grid_w: u16,
    pub grid_h: u16,
    pub frames: Vec<Frame>,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Frame {
    pub duration: u16,
    pub chunks: Vec<Chunk>,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Chunk {
    /// 0x0004 (`six_bit` false) or 0x0011 (`six_bit` true).
    OldPalette { six_bit: bool, packets: Vec<OldPalPacket> },
    Layer(Layer),
    Cel(Cel),
    CelExtra(CelExtra),
    ColorProfile(ColorProfile),
    ExternalFiles(Vec<ExternalFile>),
    Mask(Mask),
    /// 0x2017, no defined content; kept verbatim.
    Path(Vec<u8>),
    Tags(Vec<Tag>),
    Palette(PaletteChunk),
    UserData(UserData),
    Slice(Slice),
    Tileset(Tileset),
    /// Any chunk we do not understand (or could not parse strictly), data without the 6 byte header.
    Unknown { chunk_type: u16, bytes: Vec<u8> },
}

impl Chunk {
    pub fn chunk_type(&self) -> u16 {
        use super::*;
        match self {
            Chunk::OldPalette { six_bit: false, .. } => CHUNK_OLD_PALETTE_256,
            Chunk::OldPalette { six_bit: true, .. } => CHUNK_OLD_PALETTE_64,
            Chunk::Layer(_) => CHUNK_LAYER,
            Chunk::Cel(_) => CHUNK_CEL,
            Chunk::CelExtra(_) => CHUNK_CEL_EXTRA,
            Chunk::ColorProfile(_) => CHUNK_COLOR_PROFILE,
            Chunk::ExternalFiles(_) => CHUNK_EXTERNAL_FILES,
            Chunk::Mask(_) => CHUNK_MASK,
            Chunk::Path(_) => CHUNK_PATH,
            Chunk::Tags(_) => CHUNK_TAGS,
            Chunk::Palette(_) => CHUNK_PALETTE,
            Chunk::UserData(_) => CHUNK_USER_DATA,
            Chunk::Slice(_) => CHUNK_SLICE,
            Chunk::Tileset(_) => CHUNK_TILESET,
            Chunk::Unknown { chunk_type, .. } => *chunk_type,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct OldPalPacket {
    /// Entries to skip from the end of the previous packet.
    pub skip: u8,
    /// RGB triples (0..=255, or 0..=63 for the 6 bit chunk). 1..=256 colours.
    pub colors: Vec<[u8; 3]>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LayerType {
    Normal,
    Group,
    Tilemap,
}

impl LayerType {
    pub fn code(self) -> u16 {
        match self {
            LayerType::Normal => 0,
            LayerType::Group => 1,
            LayerType::Tilemap => 2,
        }
    }
}

pub mod layer_flags {
    pub const VISIBLE: u16 = 1;
    pub const EDITABLE: u16 = 2;
    pub const LOCK_MOVEMENT: u16 = 4;
    pub const BACKGROUND: u16 = 8;
    pub const PREFER_LINKED: u16 = 16;
    pub const COLLAPSED: u16 = 32;
    pub const REFERENCE: u16 = 64;
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Layer {
    pub flags: u16,
    pub layer_type: LayerType,
    pub child_level: u16,
    pub default_w: u16,
    pub default_h: u16,
    /// 0 normal, 1 multiply, 2 screen, ... 18 divide.
    pub blend_mode: u16,
    pub opacity: u8,
    pub name: String,
    /// Tilemap layers only.
    pub tileset_index: u32,
    /// Present when header flag 4 is set.
    pub uuid: Option<[u8; 16]>,
}

impl Layer {
    pub fn visible(&self) -> bool {
        self.flags & layer_flags::VISIBLE != 0
    }
    /// A normal, visible, editable layer with full opacity.
    pub fn normal(name: &str) -> Layer {
        Layer {
            flags: layer_flags::VISIBLE | layer_flags::EDITABLE,
            layer_type: LayerType::Normal,
            child_level: 0,
            default_w: 0,
            default_h: 0,
            blend_mode: 0,
            opacity: 255,
            name: name.to_string(),
            tileset_index: 0,
            uuid: None,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CelData {
    /// Cel type 0: uncompressed pixels (`w*h*bpp` bytes).
    Raw { w: u16, h: u16, pixels: Vec<u8> },
    /// Cel type 1: link to the cel of the same layer in frame `frame`.
    Linked { frame: u16 },
    /// Cel type 2: zlib compressed pixels (stored decompressed).
    Image { w: u16, h: u16, pixels: Vec<u8> },
    /// Cel type 3: zlib compressed tilemap (stored decompressed as `tiles`, row major).
    Tilemap {
        w: u16,
        h: u16,
        bits_per_tile: u16,
        id_mask: u32,
        xflip_mask: u32,
        yflip_mask: u32,
        diag_mask: u32,
        tiles: Vec<u32>,
    },
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Cel {
    pub layer: u16,
    pub x: i16,
    pub y: i16,
    pub opacity: u8,
    pub z_index: i16,
    pub data: CelData,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CelExtra {
    pub flags: u32,
    /// 16.16 fixed point.
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ColorProfile {
    /// 0 none, 1 sRGB, 2 embedded ICC.
    pub profile_type: u16,
    /// 1 = fixed gamma is used.
    pub flags: u16,
    pub gamma: i32,
    pub icc: Vec<u8>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ExternalFile {
    pub id: u32,
    /// 0 palette, 1 tileset, 2 properties extension name, 3 tile management extension.
    pub kind: u8,
    pub name: String,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Mask {
    pub x: i16,
    pub y: i16,
    pub w: u16,
    pub h: u16,
    pub name: String,
    pub bitmap: Vec<u8>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LoopDir {
    Forward,
    Reverse,
    PingPong,
    PingPongReverse,
}

impl LoopDir {
    pub fn code(self) -> u8 {
        match self {
            LoopDir::Forward => 0,
            LoopDir::Reverse => 1,
            LoopDir::PingPong => 2,
            LoopDir::PingPongReverse => 3,
        }
    }
    pub fn from_code(c: u8) -> Option<LoopDir> {
        Some(match c {
            0 => LoopDir::Forward,
            1 => LoopDir::Reverse,
            2 => LoopDir::PingPong,
            3 => LoopDir::PingPongReverse,
            _ => return None,
        })
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Tag {
    pub from: u16,
    pub to: u16,
    pub direction: LoopDir,
    /// 0 = infinite.
    pub repeat: u16,
    pub color: [u8; 3],
    pub name: String,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PaletteEntry {
    pub rgba: [u8; 4],
    pub name: Option<String>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PaletteChunk {
    /// Total palette size after this chunk.
    pub new_size: u32,
    pub first: u32,
    /// `entries.len() == last - first + 1`.
    pub entries: Vec<PaletteEntry>,
}

impl PaletteChunk {
    pub fn full(colors: &[[u8; 4]]) -> PaletteChunk {
        PaletteChunk {
            new_size: colors.len() as u32,
            first: 0,
            entries: colors.iter().map(|c| PaletteEntry { rgba: *c, name: None }).collect(),
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum PropValue {
    Null,
    Bool(bool),
    I8(i8),
    U8(u8),
    I16(i16),
    U16(u16),
    I32(i32),
    U32(u32),
    I64(i64),
    U64(u64),
    Fixed(i32),
    Float(f32),
    Double(f64),
    Str(String),
    Point(i32, i32),
    Size(i32, i32),
    Rect { x: i32, y: i32, w: i32, h: i32 },
    /// `elem_type` 0: every item is stored with its own type; otherwise all items share that type code.
    Vector { elem_type: u16, items: Vec<PropValue> },
    Map(Vec<(String, PropValue)>),
    Uuid([u8; 16]),
}

impl PropValue {
    pub fn type_code(&self) -> u16 {
        match self {
            PropValue::Null => 0,
            PropValue::Bool(_) => 1,
            PropValue::I8(_) => 2,
            PropValue::U8(_) => 3,
            PropValue::I16(_) => 4,
            PropValue::U16(_) => 5,
            PropValue::I32(_) => 6,
            PropValue::U32(_) => 7,
            PropValue::I64(_) => 8,
            PropValue::U64(_) => 9,
            PropValue::Fixed(_) => 10,
            PropValue::Float(_) => 11,
            PropValue::Double(_) => 12,
            PropValue::Str(_) => 13,
            PropValue::Point(..) => 14,
            PropValue::Size(..) => 15,
            PropValue::Rect { .. } => 16,
            PropValue::Vector { .. } => 17,
            PropValue::Map(_) => 18,
            PropValue::Uuid(_) => 19,
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct PropMap {
    /// 0 = user properties, otherwise an external-file extension id.
    pub key: u32,
    pub props: Vec<(String, PropValue)>,
}

#[derive(Clone, PartialEq, Debug, Default)]
pub struct UserData {
    pub text: Option<String>,
    pub color: Option<[u8; 4]>,
    pub props: Option<Vec<PropMap>>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SliceKey {
    pub frame: u32,
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    /// 9 patch centre (x, y, w, h), relative to the slice.
    pub center: Option<(i32, i32, u32, u32)>,
    pub pivot: Option<(i32, i32)>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Slice {
    pub name: String,
    /// Slice flags (authoritative on write): bit 1 nine patch (every key carries `center`), bit 2 pivot
    /// (every key carries `pivot`).
    pub flags: u32,
    pub keys: Vec<SliceKey>,
}

pub mod tileset_flags {
    pub const EXTERNAL: u32 = 1;
    pub const EMBEDDED: u32 = 2;
    pub const EMPTY_ZERO: u32 = 4;
    pub const AUTO_XFLIP: u32 = 8;
    pub const AUTO_YFLIP: u32 = 16;
    pub const AUTO_DFLIP: u32 = 32;
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Tileset {
    pub id: u32,
    pub flags: u32,
    pub num_tiles: u32,
    pub tile_w: u16,
    pub tile_h: u16,
    pub base_index: i16,
    pub name: String,
    /// (external file id, tileset id in that file) when flag 1 is set.
    pub external: Option<(u32, u32)>,
    /// Decompressed tile pixels (`tile_w*tile_h*num_tiles*bpp`) when flag 2 is set.
    pub tiles: Option<Vec<u8>>,
}

impl AseDoc {
    pub fn new(width: u16, height: u16, mode: ColorMode) -> AseDoc {
        AseDoc {
            width,
            height,
            mode,
            flags: 3,
            speed: 100,
            transparent_index: 0,
            num_colors: 0,
            pixel_w: 1,
            pixel_h: 1,
            grid_x: 0,
            grid_y: 0,
            grid_w: 16,
            grid_h: 16,
            frames: Vec::new(),
        }
    }

    /// All layer chunks in file order (the order cels index into).
    pub fn layers(&self) -> Vec<&Layer> {
        let mut v = Vec::new();
        for f in &self.frames {
            for c in &f.chunks {
                if let Chunk::Layer(l) = c {
                    v.push(l);
                }
            }
        }
        v
    }

    pub fn tilesets(&self) -> Vec<&Tileset> {
        let mut v = Vec::new();
        for f in &self.frames {
            for c in &f.chunks {
                if let Chunk::Tileset(t) = c {
                    v.push(t);
                }
            }
        }
        v
    }

    pub fn tags(&self) -> Vec<&Tag> {
        let mut v = Vec::new();
        for f in &self.frames {
            for c in &f.chunks {
                if let Chunk::Tags(t) = c {
                    v.extend(t.iter());
                }
            }
        }
        v
    }

    pub fn slices(&self) -> Vec<&Slice> {
        let mut v = Vec::new();
        for f in &self.frames {
            for c in &f.chunks {
                if let Chunk::Slice(s) = c {
                    v.push(s);
                }
            }
        }
        v
    }

    /// Effective palette (last new palette chunk applied over earlier ones; old palette chunks used when no
    /// new chunk exists), as RGBA.
    pub fn palette(&self) -> Vec<[u8; 4]> {
        let mut pal: Vec<[u8; 4]> = Vec::new();
        let mut have_new = false;
        for f in &self.frames {
            for c in &f.chunks {
                match c {
                    Chunk::Palette(p) => {
                        have_new = true;
                        pal.resize(p.new_size as usize, [0, 0, 0, 255]);
                        for (i, e) in p.entries.iter().enumerate() {
                            let idx = p.first as usize + i;
                            if idx < pal.len() {
                                pal[idx] = e.rgba;
                            }
                        }
                    }
                    Chunk::OldPalette { six_bit, packets } if !have_new => {
                        let mut idx = 0usize;
                        for pk in packets {
                            idx += pk.skip as usize;
                            for col in &pk.colors {
                                if idx >= pal.len() {
                                    pal.resize(idx + 1, [0, 0, 0, 255]);
                                }
                                let e = |v: u8| if *six_bit { ((v as u16 * 255 + 31) / 63) as u8 } else { v };
                                pal[idx] = [e(col[0]), e(col[1]), e(col[2]), 255];
                                idx += 1;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        pal
    }
}
