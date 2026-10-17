//! Data model of a `.pxl` part.

use crate::img::RgbaImage;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum PartKind {
    #[default]
    Sprite,
    Tileset,
    Screen,
}

impl PartKind {
    /// Image mode of a console: `sprite`, `tile` or `screen`.
    pub fn mode_name(self) -> &'static str {
        match self {
            PartKind::Sprite => "sprite",
            PartKind::Tileset => "tile",
            PartKind::Screen => "screen",
        }
    }
    /// Mode / kind text from a `.pxl` header: `mode = "tile"` and the old `kind = "tileset"` both work.
    pub fn from_text(s: &str) -> Option<PartKind> {
        match s {
            "sprite" => Some(PartKind::Sprite),
            "tile" | "tileset" => Some(PartKind::Tileset),
            "screen" => Some(PartKind::Screen),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            PartKind::Sprite => "sprite",
            PartKind::Tileset => "tileset",
            PartKind::Screen => "screen",
        }
    }
}

/// Per-frame overrides of `[input]` (`[[frames]]` key `phase`).
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct FrameInput {
    /// Reduction phase of this frame (replaces `[reduce] phase` for it).
    pub phase: Option<(i32, i32)>,
}
impl FrameInput {
    pub fn is_empty(&self) -> bool {
        *self == FrameInput::default()
    }
}

/// High resolution source (`[input]`, DESIGN 4.1).
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Input {
    /// Image pattern, e.g. "refs/{frame}.png" (`{frame}` = frame `src`, else name).
    pub images: Option<String>,
    /// Integer scale factor of the source relative to the target.
    pub factor: Option<u32>,
    /// Explicit target size (area reduction) instead of `factor`.
    pub target: Option<(usize, usize)>,
    /// Object origin inside the source images. Default: `origin * factor`.
    pub ref_origin: Option<(i32, i32)>,
}

/// Downscale method (`[reduce] method`), see `reduce` module docs.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Method {
    #[default]
    Vote,
    Nearest,
    Contour,
    Exemplar,
    Auto,
    /// Feature-aware downscale (the first version of `pxl image` / `ingest` at factor 2 and up): `vote` fills, then outline, identity colours and eyes are re-stamped.
    Feature,
}

impl Method {
    pub const ALL: [Method; 5] = [Method::Vote, Method::Nearest, Method::Contour, Method::Exemplar, Method::Auto];
    pub fn name(self) -> &'static str {
        match self {
            Method::Vote => "vote",
            Method::Nearest => "nearest",
            Method::Contour => "contour",
            Method::Exemplar => "exemplar",
            Method::Auto => "auto",
            Method::Feature => "feature",
        }
    }
    pub fn parse(s: &str) -> Option<Method> {
        Method::ALL.iter().cloned().chain(std::iter::once(Method::Feature)).find(|m| m.name() == s.trim())
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ReduceCfg {
    /// Downscale method; default `vote`, serialized only when different.
    pub method: Method,
    /// Extra exemplar grids (text files, paths relative to the part), paired with their reference by frame
    /// name (file stem). Used by `exemplar` (and `auto`).
    pub exemplars: Vec<String>,
    pub factor: Option<u32>,
    pub phase: Option<(i32, i32)>,
    /// Template roles that get an outline.
    pub outline: Vec<String>,
    pub thin: bool,
}

/// Screen settings (`[screen]`): `unique_max` is the unique tile limit of the screen if its spec has one (screens have none by default).
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ScreenCfg {
    pub unique_max: Option<u32>,
}

/// One palette cycle (`[[cycle]]`, M4): colours `first..first+count` of palette line `line` in `file`.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct CycleCfg {
    /// Name of the template palette the cycle belongs to.
    pub palette: Option<String>,
    /// File name of the JSON (`palette_cycle/<name>.json`); default `cycle_NN`.
    pub name: Option<String>,
    /// BG palette number the colours belong to (default: found from the tile palettes).
    pub bg_palette: Option<u8>,
    pub frames_per_step: Option<u32>,
    /// Colour indexes 0..3 of the BG palette that change, and the colours of every step.
    pub targets: Vec<u8>,
    pub steps: Vec<Vec<String>>,
}

/// One underwater palette (`[[underwater]]`): the colours a BG or OBJ palette takes below the water line.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct UnderwaterCfg {
    /// `bg` or `obj`.
    pub kind: String,
    /// BG palette / OBJ slot number.
    pub palette: u8,
    /// The explicit colours of the palette below the water line.
    pub colors: Vec<String>,
    /// Swap order (lower first = most visible); default config order.
    pub order: Option<u32>,
}

/// Cell grid: one ASCII key per cell, row major.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Grid {
    pub w: usize,
    pub h: usize,
    pub cells: Vec<u8>,
}

impl Grid {
    pub fn new(w: usize, h: usize, fill: u8) -> Grid {
        Grid { w, h, cells: vec![fill; w * h] }
    }
    pub fn in_bounds(&self, x: i32, y: i32) -> bool {
        x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h
    }
    /// Cell at (x, y); out of bounds reads as '.'.
    pub fn get(&self, x: i32, y: i32) -> u8 {
        if self.in_bounds(x, y) {
            self.cells[y as usize * self.w + x as usize]
        } else {
            b'.'
        }
    }
    /// Set a cell; returns false (and does nothing) when out of bounds.
    pub fn set(&mut self, x: i32, y: i32, k: u8) -> bool {
        if self.in_bounds(x, y) {
            self.cells[y as usize * self.w + x as usize] = k;
            true
        } else {
            false
        }
    }
    pub fn row(&self, y: usize) -> &[u8] {
        &self.cells[y * self.w..(y + 1) * self.w]
    }
    pub fn rows(&self) -> impl Iterator<Item = &[u8]> {
        self.cells.chunks(self.w.max(1)).take(self.h)
    }
    /// Rows as strings (what the `.pxl` file stores).
    pub fn to_lines(&self) -> Vec<String> {
        (0..self.h).map(|y| String::from_utf8_lossy(self.row(y)).into_owned()).collect()
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Frame {
    pub name: String,
    /// Mapping label or image path of the input for this frame.
    pub src: Option<String>,
    pub locked: bool,
    pub grid: Grid,
}

/// Per-part override of the category sprite limits (`[budget]`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct PartBudget {
    pub per_line: Option<u32>,
    pub per_frame: Option<u32>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Part {
    /// File the part was loaded from (not serialized).
    pub path: PathBuf,
    pub name: String,
    pub kind: PartKind,
    pub profile: Option<String>,
    /// `console = "gbc"` in the header (single-image assets); else the project's `[project] console`.
    pub console: Option<String>,
    pub size: (usize, usize),
    pub origin: (i32, i32),
    /// Tile size for tile sets.
    pub tile: Option<(usize, usize)>,
    pub template: Option<String>,
    /// Explicit template family (`family = "name"`): only parts declaring the same family are compared by the `template` lint.
    pub family: Option<String>,
    /// Shared sprite palette set (`palette_set = "zone_2"` -> `palettes/zone_2.toml`): switches the part to the
    /// flat per-cell palette model (see `cells`).
    pub palette_set: Option<String>,
    /// Further sets of `palette_set = ["zone_1", "zone_3"]`: the part is recoloured by each zone's slots at runtime.
    pub palette_more: Vec<String>,
    /// `slots = [0, 1, 4]`: restrict the cells of this part to these OBJ slots (empty = every slot of the set).
    pub slots: Vec<u8>,
    /// Hardware budget category (`player`, `enemy`, `item`, `effect`, `platform`, `scenery`, `boss`, `screen`);
    /// default from the profile.
    pub category: Option<String>,
    /// Variant name of the dynamic slot 7 this part is drawn with.
    pub variant: Option<String>,
    /// `[budget] per_line = N, per_frame = N`: overrides of the category limits.
    pub budget: PartBudget,
    /// `[review] siblings`: parts shown next to this one in the review packet's lineup (names or paths).
    pub review_siblings: Vec<String>,
    pub input: Input,
    pub reduce: ReduceCfg,
    pub screen: Option<ScreenCfg>,
    pub cycles: Vec<CycleCfg>,
    pub underwater: Vec<UnderwaterCfg>,
    /// Animated tile parts (paths relative to this part) whose slots count against this tile set's budget.
    pub animated: Vec<String>,
    /// Tile limit of this part (replaces the profile's `unique_tiles_max`; applies to every frame).
    pub max_tiles: Option<u32>,
    /// BG palette this whole part uses (HUD and card tiles live on palette 7).
    /// Reason why a sprite part is not in any scene group (debug-only objects, fallback variants): `pxl check` lists it as exempt.
    pub budget_exempt: Option<String>,
    pub bg_palette: Option<u8>,
    /// BG palettes this part leaves free (replaces the profile's `bg_reserved`; `[]` = all 8 usable).
    pub bg_reserved: Option<Vec<u8>>,
    /// `[priority] tiles`: tile ids drawn in front of sprites (`None` = no priority mask).
    pub priority: Option<Vec<usize>>,
    /// Local colour overrides (key -> "#rrggbb"); flagged by the `template` lint.
    pub colors: BTreeMap<String, String>,
    pub frames: Vec<Frame>,
    /// Per-frame `[input]` overrides, by frame name.
    pub frame_input: BTreeMap<String, FrameInput>,
}

impl Part {
    /// Every palette set of the part, the authoring (first) set first.
    pub fn palette_sets(&self) -> Vec<&str> {
        self.palette_set.iter().map(|s| s.as_str()).chain(self.palette_more.iter().map(|s| s.as_str())).collect()
    }
    pub fn new(name: &str, size: (usize, usize)) -> Part {
        Part {
            path: PathBuf::new(),
            name: name.to_string(),
            kind: PartKind::Sprite,
            profile: None,
            console: None,
            size,
            origin: (0, 0),
            tile: None,
            template: None,
            family: None,
            palette_set: None,
            palette_more: Vec::new(),
            slots: Vec::new(),
            category: None,
            variant: None,
            budget: PartBudget::default(),
            review_siblings: Vec::new(),
            input: Input::default(),
            reduce: ReduceCfg::default(),
            screen: None,
            cycles: Vec::new(),
            underwater: Vec::new(),
            animated: Vec::new(),
            max_tiles: None,
            bg_palette: None,
            bg_reserved: None,
            budget_exempt: None,
            priority: None,
            colors: BTreeMap::new(),
            frames: Vec::new(),
            frame_input: BTreeMap::new(),
        }
    }
    /// Reduction phase of a frame: its own `[[frames]] phase`, else `[reduce] phase`, else 0,0.
    pub fn phase_for(&self, frame: &str) -> (i32, i32) {
        self.frame_input.get(frame).and_then(|o| o.phase).or(self.reduce.phase).unwrap_or((0, 0))
    }
    pub fn frame(&self, name: &str) -> Option<&Frame> {
        self.frames.iter().find(|f| f.name == name)
    }
    pub fn frame_mut(&mut self, name: &str) -> Option<&mut Frame> {
        self.frames.iter_mut().find(|f| f.name == name)
    }
    pub fn frame_index(&self, name: &str) -> Option<usize> {
        self.frames.iter().position(|f| f.name == name)
    }
    /// Directory containing the part file (relative paths in the part resolve against it).
    pub fn dir(&self) -> PathBuf {
        match self.path.parent() {
            Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
            _ => Path::new(".").to_path_buf(),
        }
    }
}

/// A rendered or loaded high resolution reference for one frame.
#[derive(Clone, Debug)]
pub struct RefFrame {
    pub img: RgbaImage,
    /// Object origin inside `img`.
    pub origin: (i32, i32),
}
