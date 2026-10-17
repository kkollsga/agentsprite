//! Hardware profiles (DESIGN "Consoles and image modes"): a profile is `<console>/<mode>`, built from the console's
//! `console.toml` plus the mode file (see `console`), or a legacy `profiles/<name>.toml` / path.

use crate::fsutil::PathSlash;
use crate::color::Rgb;
use crate::console;
use crate::fsutil;
use crate::output::{Loc, PxlError, Result};
use std::collections::BTreeMap;
use std::path::Path;

/// Sprite limits of one budget category (palette-set parts).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CatBudget {
    /// Sprites (8x16 cells, a two-slot cell counts 2) on one scanline.
    pub per_line: u32,
    /// Sprites per frame.
    pub per_frame: u32,
    /// Scanline limit on lines that carry a two-slot (overlay) cell.
    pub overlay_line: u32,
}

/// Categories a part may declare, with the default limits of the hardware feedback (2026-10-14, section 2).
pub const CATEGORIES: [&str; 9] = ["player", "enemy", "item", "hazard", "effect", "platform", "scenery", "boss", "screen"];

fn default_budgets() -> BTreeMap<String, CatBudget> {
    let b = |l: u32, f: u32, o: u32| CatBudget { per_line: l, per_frame: f, overlay_line: o };
    [("player", b(4, 7, 5)), ("enemy", b(4, 8, 4)), ("item", b(3, 6, 3)), ("hazard", b(3, 6, 3)), ("effect", b(3, 6, 3)), ("platform", b(6, 16, 6)), ("scenery", b(2, 20, 2)), ("boss", b(6, 24, 6)), ("screen", b(10, 30, 10))]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect()
}

/// Hardware profile (DESIGN 5.5): sprite fields and tile fields.
#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    pub name: String,
    /// Design-language hints of the console (`[style]`), see `console::Style`.
    pub style: crate::console::Style,
    // sprite
    pub colors: u32,
    pub shared_palette: bool,
    pub channel_step: u32,
    pub hw_cell: (u32, u32),
    pub hw_cells_warn: u32,
    pub hw_cells_per_row_warn: u32,
    /// Hard limits (errors): sprite cells per 16 px band (scanline) and per frame; 0 = unchecked.
    pub hw_cells_per_row_max: u32,
    pub hw_cells_max: u32,
    /// Palette-set parts: category used when the part declares none.
    pub default_category: String,
    /// Palette-set parts: sprite limits by category (`[budget.<category>]`).
    pub budgets: BTreeMap<String, CatBudget>,
    /// Palette-set parts of category `player`: cells per frame that may need two slots (each costs 2 sprites).
    pub multi_slot_max: u32,
    // tile / screen
    pub tile: (u32, u32),
    pub colors_per_tile: u32,
    pub palettes: u32,
    pub transparent: bool,
    pub unique_tiles_max: u32,
    /// Animated tile slots count against `unique_tiles_max` (level tiles + animated slots).
    pub count_animated: bool,
    /// `per_frame` (the tiles one animation frame needs) or `all_frames` (frames x tiles) per animation set.
    pub anim_slots: String,
    /// Tiles with the same 2-bit index pattern (flips included) are one tile whatever their palette.
    pub pattern_unique: bool,
    /// BG palettes kept free (HUD / text): tiles may not use them and `tiles fit` packs into the rest.
    pub bg_reserved: Vec<u8>,
    // shared hardware resources (M5 phase T5); `palettes` above is the number of BG palettes, `hw_cells_max` the OAM and
    // `hw_cells_per_row_max` the sprites per line
    pub obj_palettes: u32,
    pub vram_banks: u32,
    pub vram_tiles_per_bank: u32,
    /// Window / HUD tiles (BG tiles of the reserved palette).
    pub window_tiles: u32,
    /// Sprite VRAM in 8x16 cells (resident groups plus the double-buffered streamed ones).
    pub sprite_cells: u32,
    pub dma_bytes_per_frame: u32,
    /// BG tiles a scene does not use may hold sprite cells (the 0x8800-0x8FFF block is addressable by both).
    pub vram_share: bool,
    /// Tiles in that shared block over both banks.
    pub vram_shared_tiles: u32,
    /// Hard addressable limits in 8x8 tiles: OBJ tiles 0x8000-0x8FFF, BG (incl. window) 0x8800-0x97FF, per bank x banks.
    pub obj_tiles_max: u32,
    pub bg_tiles_max: u32,
    /// `allow`, `off`, or `category` (overlay layers only for `overlay_categories`, at most `overlay_cap` cells per frame).
    pub overlays: String,
    pub overlay_categories: Vec<String>,
    pub overlay_cap: u32,
    pub dedupe_flips: bool,
    pub sheet_width: u32,
    pub size: Option<(u32, u32)>,
    // generalised hardware model (M6 R2); the defaults are the Game Boy Color behaviour
    /// Bits per colour channel of the hardware (0 = the colours are a fixed list, see `fixed_colours`).
    pub colour_bits: u32,
    /// A fixed set of allowed colours (NES master palette, GB shades); empty = channels in steps of `channel_step`.
    pub fixed_colours: Vec<Rgb>,
    /// Sprite sizes in pixels: with `sprite_size_mode = "variable"` every sprite is one of these (Mega Drive 8x8 .. 32x32).
    pub sprite_sizes: Vec<(u32, u32)>,
    /// `fixed` (the cell `hw_cell`), `global` (one size for the whole game, `hw_cell` is the default) or `variable` (per sprite).
    pub sprite_size_mode: String,
    /// Hard limit of sprite pixels on one scanline (Mega Drive 320); 0 = unchecked.
    pub hw_pixels_per_line_max: u32,
    /// Background palette attribute granularity in pixels (NES 16x16); 0 = per tile.
    pub attr_block: (u32, u32),
    /// Native palette encoding written next to the export (`md`, `nes`, `gb`, `sms`, `gg`); empty = none (Game Boy Color).
    pub export_palette: String,
    /// Background and sprites draw from the same palette lines (Mega Drive).
    pub shared_bg_obj: bool,
    /// How `image` / `ingest` derive the first template: `flat` (one layer of `colors` colours) or `set` (an asset-local palette set,
    /// one slot per OBJ palette, a palette per cell).
    pub auto_template: String,
}

impl Profile {
    /// gbc-sprite defaults.
    pub fn sprite_default() -> Profile {
        Profile {
            name: "gbc-sprite".into(),
            style: crate::console::Style::default(),
            colors: 3,
            shared_palette: true,
            channel_step: 8,
            hw_cell: (8, 16),
            hw_cells_warn: 12,
            hw_cells_per_row_warn: 5,
            hw_cells_per_row_max: 10,
            hw_cells_max: 40,
            default_category: "item".into(),
            budgets: default_budgets(),
            multi_slot_max: 2,
            tile: (8, 8),
            colors_per_tile: 4,
            palettes: 8,
            transparent: true,
            unique_tiles_max: 300,
            count_animated: false,
            anim_slots: "per_frame".into(),
            pattern_unique: false,
            bg_reserved: Vec::new(),
            obj_palettes: 8,
            vram_banks: 2,
            vram_tiles_per_bank: 384,
            window_tiles: 64,
            sprite_cells: 178,
            dma_bytes_per_frame: 1024,
            vram_share: false,
            vram_shared_tiles: 256,
            obj_tiles_max: 512,
            bg_tiles_max: 512,
            overlays: "allow".into(),
            overlay_categories: vec!["player".into()],
            overlay_cap: 2,
            dedupe_flips: true,
            sheet_width: 128,
            size: None,
            colour_bits: 5,
            fixed_colours: Vec::new(),
            sprite_sizes: Vec::new(),
            sprite_size_mode: "fixed".into(),
            hw_pixels_per_line_max: 0,
            attr_block: (0, 0),
            export_palette: String::new(),
            shared_bg_obj: false,
            auto_template: "flat".into(),
        }
    }
    /// True when `c` is a colour the hardware can show: in the fixed list, else every channel a multiple of `channel_step`.
    pub fn legal(&self, c: Rgb) -> bool {
        if self.fixed_colours.is_empty() {
            crate::color::is_step_aligned(c, self.channel_step)
        } else {
            self.fixed_colours.contains(&c)
        }
    }
    /// The nearest colour the hardware can show (Oklab distance for a fixed list).
    pub fn snap(&self, c: Rgb) -> Rgb {
        if self.fixed_colours.is_empty() {
            return crate::color::snap_step(c, self.channel_step);
        }
        // lightness may move, the hue family may not: chroma counts three times, and a neutral stays a neutral
        let o = crate::color::srgb_to_oklab(c);
        let chroma = |x: Rgb| {
            let l = crate::color::srgb_to_oklab(x);
            (l.a * l.a + l.b * l.b).sqrt()
        };
        let neutral_src = (o.a * o.a + o.b * o.b).sqrt() < 0.035;
        let pool: Vec<Rgb> = if neutral_src && self.fixed_colours.iter().any(|x| chroma(*x) < 0.035) { self.fixed_colours.iter().copied().filter(|x| chroma(*x) < 0.035).collect() } else { self.fixed_colours.clone() };
        let d = |x: &Rgb| {
            let l = crate::color::srgb_to_oklab(*x);
            (l.l - o.l).powi(2) + 3.0 * ((l.a - o.a).powi(2) + (l.b - o.b).powi(2))
        };
        pool.into_iter().min_by(|a, b| d(a).partial_cmp(&d(b)).unwrap_or(std::cmp::Ordering::Equal)).unwrap_or(c)
    }
    pub fn tile_default() -> Profile {
        Profile { name: "gbc-tile".into(), transparent: false, ..Profile::sprite_default() }
    }
    pub fn screen_default() -> Profile {
        Profile { name: "gbc-screen".into(), size: Some((160, 144)), ..Profile::tile_default() }
    }
}

const KEYS: [&str; 46] = [
    "colors",
    "shared_palette",
    "channel_step",
    "hw_cell",
    "hw_cells_warn",
    "hw_cells_per_row_warn",
    "hw_cells_per_row_max",
    "hw_cells_max",
    "category",
    "budget",
    "multi_slot_max",
    "tile",
    "colors_per_tile",
    "palettes",
    "transparent",
    "unique_tiles_max",
    "count_animated",
    "anim_slots",
    "pattern_unique",
    "bg_reserved",
    "bg_palettes",
    "obj_palettes",
    "vram_banks",
    "vram_tiles_per_bank",
    "window_tiles",
    "sprite_cells",
    "dma_bytes_per_frame",
    "vram_share",
    "vram_shared_tiles",
    "obj_tiles_max",
    "bg_tiles_max",
    "overlays",
    "overlay_categories",
    "overlay_cap",
    "dedupe_flips",
    "sheet_width",
    "size",
    "colour_bits",
    "fixed_colours",
    "sprite_sizes",
    "sprite_size_mode",
    "hw_pixels_per_line_max",
    "attr_block",
    "export_palette",
    "shared_bg_obj",
    "auto_template",
];

fn perr(src: &str, msg: String) -> PxlError {
    PxlError::validation(msg).with_loc(Loc::file(src.to_string()))
}

/// Parse profile TOML text. Missing keys keep the `gbc-sprite` defaults; unknown keys are errors.
pub fn parse(name: &str, text: &str, src: &str) -> Result<Profile> {
    let t: toml::Table = text.parse().map_err(|e: toml::de::Error| perr(src, format!("invalid profile TOML: {}", e.message())))?;
    parse_table(name, &t, src)
}

/// Build a profile from a parsed (and, for consoles, merged) table.
pub fn parse_table(name: &str, t: &toml::Table, src: &str) -> Result<Profile> {
    let mut p = Profile::sprite_default();
    p.name = name.to_string();
    p.size = None;
    for (k, v) in t {
        let int = |v: &toml::Value| -> Result<u32> {
            v.as_integer().filter(|i| *i >= 0 && *i <= u32::MAX as i64).map(|i| i as u32).ok_or_else(|| perr(src, format!("`{}` must be a non-negative integer", k)))
        };
        let pair = |v: &toml::Value| -> Result<(u32, u32)> {
            match v.as_array() {
                Some(a) if a.len() == 2 => Ok((int(&a[0])?, int(&a[1])?)),
                _ => Err(perr(src, format!("`{}` must be [w, h]", k))),
            }
        };
        let boolean = |v: &toml::Value| -> Result<bool> { v.as_bool().ok_or_else(|| perr(src, format!("`{}` must be true or false", k))) };
        match k.as_str() {
            "colors" => p.colors = int(v)?,
            "shared_palette" => p.shared_palette = boolean(v)?,
            "channel_step" => p.channel_step = int(v)?.max(1),
            "hw_cell" => p.hw_cell = pair(v)?,
            "hw_cells_warn" => p.hw_cells_warn = int(v)?,
            "hw_cells_per_row_warn" => p.hw_cells_per_row_warn = int(v)?,
            "hw_cells_per_row_max" => p.hw_cells_per_row_max = int(v)?,
            "hw_cells_max" => p.hw_cells_max = int(v)?,
            "category" => {
                let c = v.as_str().filter(|c| CATEGORIES.contains(c)).ok_or_else(|| perr(src, format!("`category` must be one of: {}", CATEGORIES.join(", "))))?;
                p.default_category = c.to_string();
            }
            "multi_slot_max" => p.multi_slot_max = int(v)?,
            "budget" => {
                let t = v.as_table().ok_or_else(|| perr(src, "`budget` must be a table of categories: [budget.enemy] per_line = 4".into()))?;
                for (cat, bv) in t {
                    if !CATEGORIES.contains(&cat.as_str()) {
                        return Err(perr(src, format!("unknown budget category '{}'", cat)).with_hint(format!("categories: {}", CATEGORIES.join(", "))));
                    }
                    let bt = bv.as_table().ok_or_else(|| perr(src, format!("[budget.{}] must be a table", cat)))?;
                    let mut b = p.budgets.get(cat).copied().unwrap_or(CatBudget { per_line: 0, per_frame: 0, overlay_line: 0 });
                    let mut ov = false;
                    for (bk, x) in bt {
                        let n = x.as_integer().filter(|i| *i >= 0 && *i <= u32::MAX as i64).map(|i| i as u32).ok_or_else(|| perr(src, format!("[budget.{}] `{}` must be a non-negative integer", cat, bk)))?;
                        match bk.as_str() {
                            "per_line" => b.per_line = n,
                            "per_frame" => b.per_frame = n,
                            "overlay_line" => {
                                b.overlay_line = n;
                                ov = true;
                            }
                            _ => return Err(perr(src, format!("[budget.{}]: unknown key '{}'", cat, bk)).with_hint("allowed: per_line, per_frame, overlay_line")),
                        }
                    }
                    if !ov {
                        b.overlay_line = b.overlay_line.max(b.per_line);
                    }
                    p.budgets.insert(cat.clone(), b);
                }
            }
            "tile" => p.tile = pair(v)?,
            "colors_per_tile" => p.colors_per_tile = int(v)?,
            "palettes" => p.palettes = int(v)?,
            "transparent" => p.transparent = boolean(v)?,
            "unique_tiles_max" => p.unique_tiles_max = int(v)?,
            "count_animated" => p.count_animated = boolean(v)?,
            "bg_palettes" => p.palettes = int(v)?,
            "obj_palettes" => p.obj_palettes = int(v)?,
            "vram_banks" => p.vram_banks = int(v)?,
            "vram_tiles_per_bank" => p.vram_tiles_per_bank = int(v)?,
            "window_tiles" => p.window_tiles = int(v)?,
            "sprite_cells" => p.sprite_cells = int(v)?,
            "dma_bytes_per_frame" => p.dma_bytes_per_frame = int(v)?,
            "overlay_cap" => p.overlay_cap = int(v)?,
            "vram_share" => p.vram_share = boolean(v)?,
            "vram_shared_tiles" => p.vram_shared_tiles = int(v)?,
            "obj_tiles_max" => p.obj_tiles_max = int(v)?,
            "bg_tiles_max" => p.bg_tiles_max = int(v)?,
            "overlays" => {
                let t = v.as_str().filter(|t| ["allow", "off", "category"].contains(t)).ok_or_else(|| perr(src, "`overlays` must be \"allow\", \"off\" or \"category\"".into()))?;
                p.overlays = t.to_string();
            }
            "overlay_categories" => {
                let a = v.as_array().ok_or_else(|| perr(src, "`overlay_categories` must be a list of categories".into()))?;
                p.overlay_categories = a.iter().filter_map(|e| e.as_str().map(String::from)).collect();
            }
            "pattern_unique" => p.pattern_unique = boolean(v)?,
            "anim_slots" => {
                let t = v.as_str().filter(|t| *t == "per_frame" || *t == "all_frames").ok_or_else(|| perr(src, "`anim_slots` must be \"per_frame\" or \"all_frames\"".into()))?;
                p.anim_slots = t.to_string();
            }
            "bg_reserved" => {
                let a = v.as_array().ok_or_else(|| perr(src, "`bg_reserved` must be a list of palette numbers, e.g. [7]".into()))?;
                let mut r = Vec::new();
                for e in a {
                    let n = e.as_integer().filter(|n| (0..8).contains(n)).ok_or_else(|| perr(src, "`bg_reserved` entries must be palette numbers 0..7".into()))?;
                    r.push(n as u8);
                }
                p.bg_reserved = r;
            }
            "dedupe_flips" => p.dedupe_flips = boolean(v)?,
            "sheet_width" => p.sheet_width = int(v)?,
            "size" => p.size = Some(pair(v)?),
            "colour_bits" => p.colour_bits = int(v)?,
            "hw_pixels_per_line_max" => p.hw_pixels_per_line_max = int(v)?,
            "attr_block" => p.attr_block = pair(v)?,
            "shared_bg_obj" => p.shared_bg_obj = boolean(v)?,
            "auto_template" => {
                let t = v.as_str().filter(|t| ["flat", "set"].contains(t)).ok_or_else(|| perr(src, "`auto_template` must be \"flat\" or \"set\"".into()))?;
                p.auto_template = t.to_string();
            }
            "export_palette" => {
                let t = v.as_str().filter(|t| ["", "gb", "nes", "sms", "gg", "md", "snes", "pce", "gba"].contains(t)).ok_or_else(|| perr(src, "`export_palette` must be one of: gb, nes, sms, gg, md, snes, pce, gba".into()))?;
                p.export_palette = t.to_string();
            }
            "sprite_size_mode" => {
                let t = v.as_str().filter(|t| ["fixed", "global", "variable"].contains(t)).ok_or_else(|| perr(src, "`sprite_size_mode` must be \"fixed\", \"global\" or \"variable\"".into()))?;
                p.sprite_size_mode = t.to_string();
            }
            "sprite_sizes" => {
                let a = v.as_array().ok_or_else(|| perr(src, "`sprite_sizes` must be a list of [w, h] pixel sizes".into()))?;
                let mut r = Vec::new();
                for e in a {
                    match e.as_array() {
                        Some(x) if x.len() == 2 => r.push((int(&x[0])?, int(&x[1])?)),
                        _ => return Err(perr(src, "`sprite_sizes` entries must be [w, h]".into())),
                    }
                }
                p.sprite_sizes = r;
            }
            "fixed_colours" => {
                let a = v.as_array().ok_or_else(|| perr(src, "`fixed_colours` must be a list of \"#rrggbb\" colours".into()))?;
                let mut r = Vec::new();
                for e in a {
                    let s = e.as_str().ok_or_else(|| perr(src, "`fixed_colours` entries must be \"#rrggbb\" strings".into()))?;
                    r.push(crate::color::parse_hex(s).map_err(|e| perr(src, e.msg))?);
                }
                p.fixed_colours = r;
            }
            _ => return Err(perr(src, format!("unknown profile key '{}'", k)).with_hint(format!("allowed: {}", KEYS.join(", ")))),
        }
    }
    if p.hw_cell.0 == 0 || p.hw_cell.1 == 0 {
        return Err(perr(src, "`hw_cell` must be non-zero".into()));
    }
    Ok(p)
}

fn load_file(f: &Path) -> Result<Profile> {
    let text = fsutil::read_string(f)?;
    let stem = f.file_stem().and_then(|s| s.to_str()).unwrap_or("profile");
    parse(stem, &text, &f.slash())
}

/// Load a profile by file path, `<console>/<mode>` (`gbc/sprite`) or a legacy name (`gbc-sprite`, `gbc-tile`,
/// `gbc-screen`, which mean `gbc/sprite` ...). An existing file wins. Built-in consoles only; see `load_in` for
/// project overrides.
pub fn load(name_or_path: &str) -> Result<Profile> {
    load_in(name_or_path, None)
}

/// Like `load`, with the project overrides found by walking up from `start`.
pub fn load_in(name_or_path: &str, start: Option<&Path>) -> Result<Profile> {
    let path = Path::new(name_or_path);
    if path.is_file() {
        return load_file(path);
    }
    if let Some((c, m)) = console::legacy_profile(name_or_path) {
        if let Some(f) = start.and_then(|d| fsutil::find_up(d, &format!("profiles/{}.toml", name_or_path))) {
            return load_file(&f);
        }
        return console::resolve(c, m, start);
    }
    if let Some((c, m)) = name_or_path.split_once('/') {
        if console::MODES.contains(&m) {
            return console::resolve(c, m, start);
        }
    }
    if let Some(f) = start.and_then(|d| fsutil::find_up(d, &format!("profiles/{}.toml", name_or_path))) {
        return load_file(&f);
    }
    Err(PxlError::usage(format!("unknown profile '{}'", name_or_path)).with_hint("a profile is <console>/<mode>, e.g. gbc/sprite, gbc/tile, gbc/screen (see `pxl console list`); or give a path to a .toml file"))
}

/// Profile for a part: `--profile` override, else the part's `profile`, else the project's console in the part's
/// image mode (`mode`, formerly `kind`). A project file `consoles/<id>/<mode>.toml` found above the part overrides
/// keys of the built-in; the older `profiles/<name>.toml` still replaces a legacy-named profile as a whole.
pub fn load_for(part: &crate::model::Part, over: Option<&str>) -> Result<Profile> {
    let dir = part.dir();
    let mode = part.kind.mode_name();
    match over.or(part.profile.as_deref()) {
        Some(n) if console::MODES.contains(&n) => console::resolve(&cid(part, &dir), n, Some(&dir)),
        Some(n) => load_in(n, Some(&dir)),
        None => default_in(&dir, mode, part.console.clone()),
    }
}

fn cid(part: &crate::model::Part, dir: &Path) -> String {
    part.console.clone().unwrap_or_else(|| console::project_console(dir).id)
}

fn default_in(dir: &Path, mode: &str, own: Option<String>) -> Result<Profile> {
    let id = own.unwrap_or_else(|| console::project_console(dir).id);
    if id == console::DEFAULT_CONSOLE {
        if let Some(f) = fsutil::find_up(dir, &format!("profiles/gbc-{}.toml", mode)) {
            return load_file(&f);
        }
    }
    console::resolve(&id, mode, Some(dir))
}

/// The profile of one image mode for a whole project (scene budgets): the manifest's console (default `gbc`).
pub fn for_project(dir: &Path, console: Option<&str>, mode: &str) -> Result<Profile> {
    let id = console.unwrap_or(console::DEFAULT_CONSOLE);
    if id == console::DEFAULT_CONSOLE {
        let f = dir.join("profiles").join(format!("gbc-{}.toml", mode));
        if f.is_file() {
            return load_file(&f);
        }
    }
    console::resolve(id, mode, Some(dir))
}
