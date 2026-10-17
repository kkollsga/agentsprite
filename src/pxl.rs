//! `.pxl` parse and canonical serialize (DESIGN 4.2). `to_string(parse(x))` is a fixed point.

use crate::fsutil::PathSlash;
use crate::fsutil;
use crate::model::*;
use crate::output::{Loc, PxlError, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPart {
    name: String,
    kind: Option<String>,
    mode: Option<String>,
    profile: Option<String>,
    console: Option<String>,
    size: Option<[usize; 2]>,
    origin: Option<[i32; 2]>,
    tile: Option<[usize; 2]>,
    template: Option<String>,
    family: Option<String>,
    palette_set: Option<OneOrMany>,
    slots: Option<Vec<u8>>,
    category: Option<String>,
    variant: Option<String>,
    budget: Option<RawBudget>,
    review: Option<RawReview>,
    input: Option<RawInput>,
    reduce: Option<RawReduce>,
    screen: Option<RawScreen>,
    cycle: Option<Vec<RawCycle>>,
    underwater: Option<Vec<RawUnderwater>>,
    animated: Option<Vec<String>>,
    max_tiles: Option<u32>,
    bg_palette: Option<u8>,
    budget_exempt: Option<String>,
    bg_reserved: Option<Vec<u8>>,
    priority: Option<RawPriority>,
    colors: Option<BTreeMap<String, String>>,
    frames: Option<Vec<RawFrame>>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBudget {
    per_line: Option<u32>,
    per_frame: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawReview {
    siblings: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawInput {
    images: Option<String>,
    factor: Option<u32>,
    target: Option<[usize; 2]>,
    ref_origin: Option<[i32; 2]>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawReduce {
    factor: Option<u32>,
    phase: Option<[i32; 2]>,
    outline: Option<Vec<String>>,
    thin: Option<bool>,
    method: Option<String>,
    exemplars: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawScreen {
    unique_max: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCycle {
    palette: Option<String>,
    name: Option<String>,
    bg_palette: Option<u8>,
    frames_per_step: Option<u32>,
    targets: Option<Vec<u8>>,
    steps: Option<Vec<Vec<String>>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawUnderwater {
    kind: String,
    palette: u8,
    colors: Option<Vec<String>>,
    order: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPriority {
    tiles: Option<Vec<usize>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFrame {
    name: String,
    src: Option<String>,
    locked: Option<bool>,
    grid: Option<String>,
    phase: Option<[i32; 2]>,
}

fn line_col(src: &str, off: usize) -> (usize, usize) {
    let off = off.min(src.len());
    let before = &src[..off];
    let line = before.matches('\n').count() + 1;
    let col = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    (line, col)
}

/// Parse a grid given as text rows (a trailing newline is allowed). Used for `.pxl` frames and for
/// grids given to `write_grid` / `patch` / `import`. `loc` names the file and frame for errors.
pub fn parse_grid(text: &str, w: usize, h: usize, loc: &Loc) -> Result<Grid> {
    let t = text.strip_suffix('\n').unwrap_or(text);
    let rows: Vec<&str> = if t.is_empty() && h == 0 { Vec::new() } else { t.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect() };
    if rows.len() != h {
        return Err(PxlError::validation(format!("grid has {} rows, expected {}", rows.len(), h))
            .with_loc(Loc { row: Some(rows.len()), ..loc.clone() })
            .with_hint(format!("every frame needs exactly {} rows of {} characters", h, w)));
    }
    let mut g = Grid::new(w, h, b'.');
    for (y, r) in rows.iter().enumerate() {
        let b = r.as_bytes();
        if b.len() != w {
            return Err(PxlError::validation(format!("row has {} characters, expected {}", b.len(), w))
                .with_loc(Loc { row: Some(y), col: Some(b.len().min(w)), ..loc.clone() })
                .with_hint(format!("rows must be exactly {} characters wide", w)));
        }
        for (x, &c) in b.iter().enumerate() {
            if !valid_key(c) {
                return Err(PxlError::validation(format!("invalid cell character {:?}", c as char))
                    .with_loc(Loc { row: Some(y), col: Some(x), ..loc.clone() })
                    .with_hint("cells are printable ASCII except space, '\"' and '\\'"));
            }
            g.cells[y * w + x] = c;
        }
    }
    Ok(g)
}

/// A usable grid key: printable ASCII, no space, no `"` or `\`.
pub fn valid_key(c: u8) -> bool {
    (0x21..=0x7e).contains(&c) && c != b'"' && c != b'\\'
}

fn tup<T: Copy>(a: Option<[T; 2]>) -> Option<(T, T)> {
    a.map(|v| (v[0], v[1]))
}

pub fn parse_str(src: &str, path: &Path) -> Result<Part> {
    let file = path.slash();
    let raw: RawPart = toml::from_str(src).map_err(|e| {
        let at = e.span().map(|s| line_col(src, s.start));
        let msg = match at {
            Some((l, c)) => format!("{} (line {}, column {})", e.message(), l, c),
            None => e.message().to_string(),
        };
        PxlError::validation(msg).with_loc(Loc::file(file.clone())).with_hint("check the .pxl syntax; unknown keys are errors")
    })?;
    let floc = Loc::file(file.clone());
    let parse_kind = |k: &str| {
        PartKind::from_text(k).ok_or_else(|| PxlError::validation(format!("unknown mode '{}'", k)).with_loc(floc.clone()).with_hint("mode is one of: sprite, tile, screen (the old `kind` also accepts tileset)"))
    };
    let kind = match (raw.mode.as_deref(), raw.kind.as_deref()) {
        (None, None) => PartKind::Sprite,
        (Some(m), None) => parse_kind(m)?,
        (None, Some(k)) => parse_kind(k)?,
        (Some(m), Some(k)) => {
            let (a, b) = (parse_kind(m)?, parse_kind(k)?);
            if a != b {
                return Err(PxlError::validation(format!("`mode = \"{}\"` and `kind = \"{}\"` disagree", m, k)).with_loc(floc.clone()).with_hint("`kind` is the old name of `mode`; keep only `mode`"));
            }
            a
        }
    };
    let size = match (raw.size, kind) {
        (Some(s), _) => (s[0], s[1]),
        (None, PartKind::Screen) => (160, 144),
        (None, _) => {
            return Err(PxlError::validation("missing `size = [w, h]`").with_loc(floc).with_hint("add size = [32, 32]"))
        }
    };
    let mut part = Part::new(&raw.name, size);
    part.path = path.to_path_buf();
    part.kind = kind;
    part.profile = raw.profile;
    part.console = raw.console;
    part.origin = tup(raw.origin).unwrap_or((0, 0));
    part.tile = tup(raw.tile);
    part.template = raw.template;
    part.family = raw.family;
    match raw.palette_set {
        Some(OneOrMany::One(s)) => part.palette_set = Some(s),
        Some(OneOrMany::Many(mut v)) if !v.is_empty() => {
            part.palette_set = Some(v.remove(0));
            part.palette_more = v;
        }
        _ => {}
    }
    part.slots = raw.slots.unwrap_or_default();
    if let Some(bad) = part.slots.iter().find(|s| **s > 7) {
        return Err(PxlError::validation(format!("slots: {} is not an OBJ palette slot (0..7)", bad)).with_loc(floc).with_hint("slots = [0, 1, 4]"));
    }
    part.category = raw.category;
    part.variant = raw.variant;
    if let Some(b) = raw.budget {
        part.budget = PartBudget { per_line: b.per_line, per_frame: b.per_frame };
    }
    part.review_siblings = raw.review.and_then(|r| r.siblings).unwrap_or_default();
    if let Some(i) = raw.input {
        part.input = Input {
            images: i.images,
            factor: i.factor,
            target: tup(i.target),
            ref_origin: tup(i.ref_origin),
        };
    }
    if let Some(r) = raw.reduce {
        let method = match &r.method {
            None => Method::Vote,
            Some(m) => Method::parse(m).ok_or_else(|| {
                PxlError::validation(format!("reduce.method: unknown method '{}'", m))
                    .with_loc(floc.clone())
                    .with_hint("methods: vote, nearest, contour, exemplar, auto")
            })?,
        };
        part.reduce = ReduceCfg { method, exemplars: r.exemplars.unwrap_or_default(), factor: r.factor, phase: tup(r.phase), outline: r.outline.unwrap_or_default(), thin: r.thin.unwrap_or(false) };
    }
    if let Some(s) = raw.screen {
        part.screen = Some(ScreenCfg { unique_max: s.unique_max });
    }
    for c in raw.cycle.unwrap_or_default() {
        part.cycles.push(CycleCfg {
            palette: c.palette,
            name: c.name,
            bg_palette: c.bg_palette,
            frames_per_step: c.frames_per_step,
            targets: c.targets.unwrap_or_default(),
            steps: c.steps.unwrap_or_default(),
        });
    }
    for u in raw.underwater.unwrap_or_default() {
        if u.kind != "bg" && u.kind != "obj" {
            return Err(PxlError::validation(format!("[[underwater]] kind '{}' must be \"bg\" or \"obj\"", u.kind)).with_loc(floc.clone()));
        }
        if u.palette > 7 {
            return Err(PxlError::validation(format!("[[underwater]] palette {} must be 0..7", u.palette)).with_loc(floc.clone()));
        }
        part.underwater.push(UnderwaterCfg { kind: u.kind, palette: u.palette, colors: u.colors.unwrap_or_default(), order: u.order });
    }
    part.animated = raw.animated.unwrap_or_default();
    part.max_tiles = raw.max_tiles;
    part.bg_palette = raw.bg_palette;
    part.bg_reserved = raw.bg_reserved;
    part.budget_exempt = raw.budget_exempt;
    if let Some(b) = part.bg_palette {
        if b > 7 {
            return Err(PxlError::validation(format!("bg_palette {} must be 0..7", b)).with_loc(floc.clone()));
        }
    }
    part.priority = raw.priority.map(|p| {
        let mut v = p.tiles.unwrap_or_default();
        v.sort_unstable();
        v.dedup();
        v
    });
    part.colors = raw.colors.unwrap_or_default();
    if let Some(t) = part.tile {
        if t.0 == 0 || t.1 == 0 {
            return Err(PxlError::validation("tile size must be non-zero").with_loc(floc));
        }
    }
    for rf in raw.frames.unwrap_or_default() {
        if rf.name.is_empty() || rf.name.chars().any(|c| c.is_whitespace() || c == '/' || c == '\\') {
            return Err(PxlError::validation(format!("invalid frame name '{}'", rf.name))
                .with_loc(floc.clone())
                .with_hint("frame names are used as file names: no spaces or slashes"));
        }
        if part.frames.iter().any(|f| f.name == rf.name) {
            return Err(PxlError::validation(format!("duplicate frame '{}'", rf.name)).with_loc(floc.clone().frame(rf.name.clone())));
        }
        let loc = floc.clone().frame(rf.name.clone());
        let grid = match &rf.grid {
            Some(g) => parse_grid(g, size.0, size.1, &loc)?,
            None => Grid::new(size.0, size.1, b'.'),
        };
        let over = FrameInput {
            phase: tup(rf.phase),
        };
        if !over.is_empty() {
            part.frame_input.insert(rf.name.clone(), over);
        }
        part.frames.push(Frame { name: rf.name, src: rf.src, locked: rf.locked.unwrap_or(false), grid });
    }
    if let (Some(t), PartKind::Tileset) = (part.tile, part.kind) {
        if size.0 % t.0 != 0 || size.1 % t.1 != 0 {
            return Err(PxlError::validation(format!("size {}x{} is not a multiple of tile {}x{}", size.0, size.1, t.0, t.1)).with_loc(floc));
        }
    }
    Ok(part)
}

pub fn load(path: &Path) -> Result<Part> {
    let text = fsutil::read_string(path)?;
    parse_str(&text, path)
}

pub fn save(path: &Path, part: &Part) -> Result<()> {
    fsutil::atomic_write(path, to_string(part).as_bytes())
}

fn q(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\t' => o.push_str("\\t"),
            '\r' => o.push_str("\\r"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => o.push_str(&format!("\\u{:04X}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn qlist(v: &[String]) -> String {
    format!("[{}]", v.iter().map(|s| q(s)).collect::<Vec<_>>().join(", "))
}

/// Canonical text of a part: fixed key order, fixed spacing, one blank line between sections.
pub fn to_string(p: &Part) -> String {
    let mut s = String::new();
    s.push_str(&format!("name = {}\n", q(&p.name)));
    if p.kind != PartKind::Sprite {
        s.push_str(&format!("mode = {}\n", q(p.kind.mode_name())));
    }
    if let Some(v) = &p.console {
        s.push_str(&format!("console = {}\n", q(v)));
    }
    if let Some(v) = &p.profile {
        s.push_str(&format!("profile = {}\n", q(v)));
    }
    s.push_str(&format!("size = [{}, {}]\n", p.size.0, p.size.1));
    s.push_str(&format!("origin = [{}, {}]\n", p.origin.0, p.origin.1));
    if let Some(t) = p.tile {
        s.push_str(&format!("tile = [{}, {}]\n", t.0, t.1));
    }
    if let Some(v) = &p.template {
        s.push_str(&format!("template = {}\n", q(v)));
    }
    if let Some(v) = &p.family {
        s.push_str(&format!("family = {}\n", q(v)));
    }
    if let Some(v) = &p.palette_set {
        if p.palette_more.is_empty() {
            s.push_str(&format!("palette_set = {}\n", q(v)));
        } else {
            s.push_str(&format!("palette_set = {}\n", qlist(&p.palette_sets().iter().map(|x| x.to_string()).collect::<Vec<_>>())));
        }
    }
    if !p.slots.is_empty() {
        s.push_str(&format!("slots = [{}]\n", p.slots.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(", ")));
    }
    if let Some(v) = &p.category {
        s.push_str(&format!("category = {}\n", q(v)));
    }
    if let Some(v) = &p.variant {
        s.push_str(&format!("variant = {}\n", q(v)));
    }
    if let Some(v) = p.max_tiles {
        s.push_str(&format!("max_tiles = {}\n", v));
    }
    if let Some(v) = p.bg_palette {
        s.push_str(&format!("bg_palette = {}\n", v));
    }
    if let Some(v) = &p.budget_exempt {
        s.push_str(&format!("budget_exempt = {}\n", q(v)));
    }
    if let Some(v) = &p.bg_reserved {
        s.push_str(&format!("bg_reserved = [{}]\n", v.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(", ")));
    }
    if !p.animated.is_empty() {
        s.push_str(&format!("animated = {}\n", qlist(&p.animated)));
    }
    if p.budget != PartBudget::default() {
        s.push_str("\n[budget]\n");
        if let Some(v) = p.budget.per_line {
            s.push_str(&format!("per_line = {}\n", v));
        }
        if let Some(v) = p.budget.per_frame {
            s.push_str(&format!("per_frame = {}\n", v));
        }
    }
    if !p.review_siblings.is_empty() {
        s.push_str(&format!("\n[review]\nsiblings = {}\n", qlist(&p.review_siblings)));
    }
    let i = &p.input;
    if *i != Input::default() {
        s.push_str("\n[input]\n");
        if let Some(v) = &i.images {
            s.push_str(&format!("images = {}\n", q(v)));
        }
        if let Some(v) = i.factor {
            s.push_str(&format!("factor = {}\n", v));
        }
        if let Some(v) = i.target {
            s.push_str(&format!("target = [{}, {}]\n", v.0, v.1));
        }
        if let Some(v) = i.ref_origin {
            s.push_str(&format!("ref_origin = [{}, {}]\n", v.0, v.1));
        }
    }
    let r = &p.reduce;
    if *r != ReduceCfg::default() {
        s.push_str("\n[reduce]\n");
        if r.method != Method::Vote {
            s.push_str(&format!("method = {}\n", q(r.method.name())));
        }
        if !r.exemplars.is_empty() {
            s.push_str(&format!("exemplars = {}\n", qlist(&r.exemplars)));
        }
        if let Some(v) = r.factor {
            s.push_str(&format!("factor = {}\n", v));
        }
        if let Some(v) = r.phase {
            s.push_str(&format!("phase = [{}, {}]\n", v.0, v.1));
        }
        if !r.outline.is_empty() {
            s.push_str(&format!("outline = {}\n", qlist(&r.outline)));
        }
        if r.thin {
            s.push_str("thin = true\n");
        }
    }
    if let Some(sc) = &p.screen {
        s.push_str("\n[screen]\n");
        if let Some(v) = sc.unique_max {
            s.push_str(&format!("unique_max = {}\n", v));
        }
    }
    if !p.colors.is_empty() {
        s.push_str("\n[colors]\n");
        for (k, v) in &p.colors {
            s.push_str(&format!("{} = {}\n", q(k), q(v)));
        }
    }
    if let Some(pr) = &p.priority {
        s.push_str(&format!("\n[priority]\ntiles = [{}]\n", pr.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", ")));
    }
    for c in &p.cycles {
        s.push_str("\n[[cycle]]\n");
        if let Some(v) = &c.palette {
            s.push_str(&format!("palette = {}\n", q(v)));
        }
        if let Some(v) = &c.name {
            s.push_str(&format!("name = {}\n", q(v)));
        }
        if let Some(v) = c.bg_palette {
            s.push_str(&format!("bg_palette = {}\n", v));
        }
        if let Some(v) = c.frames_per_step {
            s.push_str(&format!("frames_per_step = {}\n", v));
        }
        if !c.targets.is_empty() {
            s.push_str(&format!("targets = [{}]\n", c.targets.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", ")));
        }
        if !c.steps.is_empty() {
            s.push_str(&format!("steps = [{}]\n", c.steps.iter().map(|st| qlist(st)).collect::<Vec<_>>().join(", ")));
        }
    }
    for u in &p.underwater {
        s.push_str("\n[[underwater]]\n");
        s.push_str(&format!("kind = {}\npalette = {}\n", q(&u.kind), u.palette));
        if !u.colors.is_empty() {
            s.push_str(&format!("colors = {}\n", qlist(&u.colors)));
        }
        if let Some(o) = u.order {
            s.push_str(&format!("order = {}\n", o));
        }
    }
    for f in &p.frames {
        s.push_str("\n[[frames]]\n");
        s.push_str(&format!("name = {}\n", q(&f.name)));
        if let Some(v) = &f.src {
            s.push_str(&format!("src = {}\n", q(v)));
        }
        if f.locked {
            s.push_str("locked = true\n");
        }
        if let Some(o) = p.frame_input.get(&f.name) {
            if let Some(v) = o.phase {
                s.push_str(&format!("phase = [{}, {}]\n", v.0, v.1));
            }
        }
        s.push_str("grid = \"\"\"\n");
        for y in 0..f.grid.h {
            s.push_str(&String::from_utf8_lossy(f.grid.row(y)));
            s.push('\n');
        }
        s.push_str("\"\"\"\n");
    }
    s
}
