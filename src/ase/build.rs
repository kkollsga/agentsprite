//! [`AseBuilder`]: assembles a canonical document the way Aseprite itself lays one out.
//!
//! Frame 0 order: colour profile (sRGB), old palette 0x0004 (indexed only), new palette 0x2019, tags, tilesets,
//! layers, cels, slices. Later frames hold only cels (ordered by layer index).

use super::model::*;
use crate::color::Rgb;
use crate::output::{PxlError, Result};

/// A slice with one key on frame 0. `pivot` is relative to the slice origin `(x, y)`; `center` is the 9-patch
/// centre rectangle (x, y, w, h), also relative to the slice.
#[derive(Clone, Debug)]
pub struct SliceSpec {
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub center: Option<(i32, i32, u32, u32)>,
    pub pivot: Option<(i32, i32)>,
}

pub struct AseBuilder {
    doc: AseDoc,
    palette: Vec<[u8; 4]>,
    layers: Vec<Layer>,
    durations: Vec<u16>,
    cels: Vec<Vec<Cel>>,
    tags: Vec<Tag>,
    tilesets: Vec<Tileset>,
    slices: Vec<Slice>,
}

impl AseBuilder {
    /// Indexed document. `palette[0]` is the transparent slot (alpha is written as given; pass opaque colours
    /// as `Rgb` and they get alpha 255, like Aseprite stores them).
    pub fn new_indexed(width: u16, height: u16, palette: &[Rgb]) -> AseBuilder {
        let pal: Vec<[u8; 4]> = palette.iter().map(|c| [c.r, c.g, c.b, 255]).collect();
        AseBuilder::new_indexed_rgba(width, height, &pal)
    }

    /// Indexed document with an RGBA palette.
    pub fn new_indexed_rgba(width: u16, height: u16, palette: &[[u8; 4]]) -> AseBuilder {
        let mut doc = AseDoc::new(width, height, ColorMode::Indexed);
        doc.num_colors = palette.len().min(256) as u16;
        AseBuilder::with(doc, palette.to_vec())
    }

    /// RGBA document (no palette chunk unless [`AseBuilder::set_palette`] is called).
    pub fn new_rgba(width: u16, height: u16) -> AseBuilder {
        AseBuilder::with(AseDoc::new(width, height, ColorMode::Rgba), Vec::new())
    }

    fn with(doc: AseDoc, palette: Vec<[u8; 4]>) -> AseBuilder {
        AseBuilder {
            doc,
            palette,
            layers: Vec::new(),
            durations: Vec::new(),
            cels: Vec::new(),
            tags: Vec::new(),
            tilesets: Vec::new(),
            slices: Vec::new(),
        }
    }

    pub fn set_palette(&mut self, palette: &[[u8; 4]]) -> &mut Self {
        self.palette = palette.to_vec();
        self.doc.num_colors = palette.len().min(256) as u16;
        self
    }

    pub fn set_transparent_index(&mut self, i: u8) -> &mut Self {
        self.doc.transparent_index = i;
        self
    }

    /// Grid settings stored in the header.
    pub fn set_grid(&mut self, x: i16, y: i16, w: u16, h: u16) -> &mut Self {
        self.doc.grid_x = x;
        self.doc.grid_y = y;
        self.doc.grid_w = w;
        self.doc.grid_h = h;
        self
    }

    /// Appends a normal visible layer; returns its layer index (bottom layer is 0).
    pub fn add_layer(&mut self, name: &str) -> usize {
        self.add_layer_with(Layer::normal(name))
    }

    pub fn add_group(&mut self, name: &str, child_level: u16) -> usize {
        let mut l = Layer::normal(name);
        l.layer_type = LayerType::Group;
        l.child_level = child_level;
        self.add_layer_with(l)
    }

    /// Tilemap layer using the tileset with `tileset_id` (see [`AseBuilder::add_tileset`]).
    pub fn add_tilemap_layer(&mut self, name: &str, tileset_id: u32) -> usize {
        let mut l = Layer::normal(name);
        l.layer_type = LayerType::Tilemap;
        l.tileset_index = tileset_id;
        self.add_layer_with(l)
    }

    pub fn add_layer_with(&mut self, l: Layer) -> usize {
        self.layers.push(l);
        self.layers.len() - 1
    }

    /// Adds an embedded tileset. `tiles` holds `tile_w*tile_h*num_tiles` pixels in document format; tile 0 is
    /// the empty tile.
    pub fn add_tileset(&mut self, id: u32, name: &str, tile_w: u16, tile_h: u16, num_tiles: u32, tiles: Vec<u8>) {
        self.tilesets.push(Tileset {
            id,
            flags: tileset_flags::EMBEDDED | tileset_flags::EMPTY_ZERO,
            num_tiles,
            tile_w,
            tile_h,
            base_index: 1,
            name: name.to_string(),
            external: None,
            tiles: Some(tiles),
        });
    }

    /// Appends a frame (duration in ms); returns its index.
    pub fn add_frame(&mut self, duration_ms: u16) -> usize {
        self.durations.push(duration_ms);
        self.cels.push(Vec::new());
        self.durations.len() - 1
    }

    /// Number of frames added so far.
    pub fn frame_count(&self) -> usize {
        self.durations.len()
    }

    fn frame_cels(&mut self, frame: usize) -> &mut Vec<Cel> {
        &mut self.cels[frame]
    }

    /// Cel from a full canvas image (`width*height` pixels in document format). The cel is trimmed to the
    /// bounding box of non-transparent pixels; an empty canvas adds no cel. Returns whether a cel was added.
    pub fn set_cel_canvas(&mut self, frame: usize, layer: usize, canvas: &[u8]) -> bool {
        let (w, h) = (self.doc.width as usize, self.doc.height as usize);
        let bpp = self.doc.mode.bytes_per_pixel();
        assert_eq!(canvas.len(), w * h * bpp, "canvas size mismatch");
        let ti = self.doc.transparent_index;
        let mode = self.doc.mode;
        let opaque = |i: usize| -> bool {
            match mode {
                ColorMode::Indexed => canvas[i] != ti,
                ColorMode::Grayscale => canvas[i * 2 + 1] != 0,
                ColorMode::Rgba => canvas[i * 4 + 3] != 0,
            }
        };
        let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0usize, 0usize);
        for y in 0..h {
            for x in 0..w {
                if opaque(y * w + x) {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x + 1);
                    y1 = y1.max(y + 1);
                }
            }
        }
        if x1 == 0 {
            return false;
        }
        let (cw, ch) = (x1 - x0, y1 - y0);
        let mut px = Vec::with_capacity(cw * ch * bpp);
        for y in y0..y1 {
            px.extend_from_slice(&canvas[(y * w + x0) * bpp..(y * w + x1) * bpp]);
        }
        self.set_cel(frame, layer, x0 as i16, y0 as i16, cw as u16, ch as u16, px);
        true
    }

    /// Compressed image cel at (x, y) with explicit pixels (`w*h*bpp` bytes).
    pub fn set_cel(&mut self, frame: usize, layer: usize, x: i16, y: i16, w: u16, h: u16, pixels: Vec<u8>) {
        self.add_cel(frame, Cel { layer: layer as u16, x, y, opacity: 255, z_index: 0, data: CelData::Image { w, h, pixels } });
    }

    /// Linked cel: reuses the cel of the same layer in the earlier frame `src_frame`.
    pub fn link_cel(&mut self, frame: usize, layer: usize, src_frame: usize) {
        self.add_cel(
            frame,
            Cel { layer: layer as u16, x: 0, y: 0, opacity: 255, z_index: 0, data: CelData::Linked { frame: src_frame as u16 } },
        );
    }

    /// Tilemap cel (32 bit tiles, Aseprite's default masks).
    pub fn set_tilemap_cel(&mut self, frame: usize, layer: usize, x: i16, y: i16, w: u16, h: u16, tiles: Vec<u32>) {
        self.add_cel(
            frame,
            Cel {
                layer: layer as u16,
                x,
                y,
                opacity: 255,
                z_index: 0,
                data: CelData::Tilemap {
                    w,
                    h,
                    bits_per_tile: 32,
                    id_mask: 0x1fff_ffff,
                    xflip_mask: 0x8000_0000,
                    yflip_mask: 0x4000_0000,
                    diag_mask: 0x2000_0000,
                    tiles,
                },
            },
        );
    }

    /// Adds or replaces the cel for `(frame, cel.layer)`.
    pub fn add_cel(&mut self, frame: usize, cel: Cel) {
        let v = self.frame_cels(frame);
        v.retain(|c| c.layer != cel.layer);
        v.push(cel);
    }

    /// Tag over frames `from..=to`. `repeat` 0 = infinite.
    pub fn add_tag(&mut self, name: &str, from: usize, to: usize, direction: LoopDir, repeat: u16) {
        self.tags.push(Tag { from: from as u16, to: to as u16, direction, repeat, color: [0, 0, 0], name: name.to_string() });
    }

    pub fn add_slice(&mut self, s: SliceSpec) {
        let flags = (s.center.is_some() as u32) | ((s.pivot.is_some() as u32) << 1);
        self.slices.push(Slice {
            name: s.name,
            flags,
            keys: vec![SliceKey { frame: 0, x: s.x, y: s.y, w: s.w, h: s.h, center: s.center, pivot: s.pivot }],
        });
    }

    /// Assembles the document. Fails when there are no frames or no layers.
    pub fn finish(mut self) -> Result<AseDoc> {
        if self.durations.is_empty() {
            return Err(PxlError::validation("aseprite export needs at least one frame"));
        }
        if self.layers.is_empty() {
            return Err(PxlError::validation("aseprite export needs at least one layer"));
        }
        self.doc.speed = self.durations[0];
        let mut first = vec![Chunk::ColorProfile(ColorProfile { profile_type: 1, flags: 0, gamma: 0, icc: Vec::new() })];
        if !self.palette.is_empty() {
            if self.doc.mode == ColorMode::Indexed {
                let colors: Vec<[u8; 3]> = self.palette.iter().take(256).map(|c| [c[0], c[1], c[2]]).collect();
                first.push(Chunk::OldPalette { six_bit: false, packets: vec![OldPalPacket { skip: 0, colors }] });
            }
            first.push(Chunk::Palette(PaletteChunk::full(&self.palette)));
        }
        if !self.tags.is_empty() {
            first.push(Chunk::Tags(std::mem::take(&mut self.tags)));
        }
        for t in std::mem::take(&mut self.tilesets) {
            first.push(Chunk::Tileset(t));
        }
        for l in std::mem::take(&mut self.layers) {
            first.push(Chunk::Layer(l));
        }
        let nframes = self.durations.len();
        for (fi, cels) in self.cels.iter_mut().enumerate() {
            cels.sort_by_key(|c| (c.layer, c.z_index));
            let mut chunks: Vec<Chunk> = Vec::new();
            if fi == 0 {
                chunks.append(&mut first);
            }
            for c in cels.drain(..) {
                chunks.push(Chunk::Cel(c));
            }
            if fi == 0 {
                for s in self.slices.drain(..) {
                    chunks.push(Chunk::Slice(s));
                }
            }
            self.doc.frames.push(Frame { duration: self.durations[fi], chunks });
        }
        debug_assert_eq!(self.doc.frames.len(), nframes);
        Ok(self.doc)
    }

    /// `finish` + [`super::write`].
    pub fn build(self) -> Result<Vec<u8>> {
        Ok(super::write(&self.finish()?))
    }
}
