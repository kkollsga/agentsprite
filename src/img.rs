//! RGBA images: the pixel buffer shared by loaders, reducers, compare and export.

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Hash)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const TRANSPARENT: Rgba = Rgba { r: 0, g: 0, b: 0, a: 0 };
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Rgba {
        Rgba { r, g, b, a }
    }
    pub const fn opaque(r: u8, g: u8, b: u8) -> Rgba {
        Rgba { r, g, b, a: 255 }
    }
    pub fn rgb(self) -> crate::color::Rgb {
        crate::color::Rgb { r: self.r, g: self.g, b: self.b }
    }
}

impl From<crate::color::Rgb> for Rgba {
    fn from(c: crate::color::Rgb) -> Rgba {
        Rgba::opaque(c.r, c.g, c.b)
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RgbaImage {
    pub w: usize,
    pub h: usize,
    pub px: Vec<Rgba>,
}

impl RgbaImage {
    /// New fully transparent image.
    pub fn new(w: usize, h: usize) -> RgbaImage {
        RgbaImage { w, h, px: vec![Rgba::TRANSPARENT; w * h] }
    }
    pub fn filled(w: usize, h: usize, c: Rgba) -> RgbaImage {
        RgbaImage { w, h, px: vec![c; w * h] }
    }
    pub fn from_raw(w: usize, h: usize, px: Vec<Rgba>) -> RgbaImage {
        assert_eq!(px.len(), w * h);
        RgbaImage { w, h, px }
    }
    /// Pixel at (x, y); out of bounds reads as transparent.
    pub fn get(&self, x: i32, y: i32) -> Rgba {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            Rgba::TRANSPARENT
        } else {
            self.px[y as usize * self.w + x as usize]
        }
    }
    /// Set pixel; returns false (and does nothing) when out of bounds.
    pub fn set(&mut self, x: i32, y: i32, c: Rgba) -> bool {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return false;
        }
        self.px[y as usize * self.w + x as usize] = c;
        true
    }
    /// Copy of the rectangle; parts outside the image are transparent.
    pub fn crop(&self, x: i32, y: i32, w: usize, h: usize) -> RgbaImage {
        let mut out = RgbaImage::new(w, h);
        for j in 0..h {
            for i in 0..w {
                out.px[j * w + i] = self.get(x + i as i32, y + j as i32);
            }
        }
        out
    }
    /// Draw `src` with its top left at (x, y); only pixels with alpha > 0 overwrite (no blending).
    pub fn blit(&mut self, src: &RgbaImage, x: i32, y: i32) {
        for j in 0..src.h {
            for i in 0..src.w {
                let c = src.px[j * src.w + i];
                if c.a > 0 {
                    self.set(x + i as i32, y + j as i32, c);
                }
            }
        }
    }
    /// Exact nearest-neighbour upscale by an integer factor.
    pub fn upscale(&self, f: usize) -> RgbaImage {
        let mut out = RgbaImage::new(self.w * f, self.h * f);
        for y in 0..out.h {
            for x in 0..out.w {
                out.px[y * out.w + x] = self.px[(y / f) * self.w + x / f];
            }
        }
        out
    }
    /// Bounding box of pixels with alpha > 0 as (x0, y0, x1_exclusive, y1_exclusive).
    pub fn bbox(&self) -> Option<(usize, usize, usize, usize)> {
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
        for y in 0..self.h {
            for x in 0..self.w {
                if self.px[y * self.w + x].a > 0 {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x + 1);
                    y1 = y1.max(y + 1);
                }
            }
        }
        if x1 == 0 {
            None
        } else {
            Some((x0, y0, x1, y1))
        }
    }
    /// Flat RGBA8 bytes, row major.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(self.px.len() * 4);
        for p in &self.px {
            v.extend_from_slice(&[p.r, p.g, p.b, p.a]);
        }
        v
    }
}
