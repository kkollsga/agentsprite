//! `pxl.toml` project manifest (DESIGN 4.4): maps parts to delivery paths.
//!
//! ```toml
//! out_root = "expected_output"            # optional; prefix of every relative `out` / `ase`
//!
//! [parts]
//! enemy = "parts/enemy.pxl"               # short form: path only, outputs next to the part
//!
//! [parts.player]                          # long form
//! path     = "parts/player.pxl"           # required, relative to the folder holding pxl.toml
//! priority = 5                            # queue order (higher first), default 0
//! tags     = ["hero"]                     # queue tags
//! out      = "characters/player"          # folder of the PNGs (default `<part folder>/<part stem>_export`)
//! ase      = "characters/player/player.aseprite"   # default `<out>/<part name>.aseprite`
//! png      = "{frame}.png"                # base layer file name (default shown)
//! overlay  = "{frame}_{layer}.png"        # every further template layer; `{layer}` = template layer name
//! ```
//!
//! * Unknown keys (top level, `[parts.x]`) are errors that name the file and the key.
//! * Relative `out` / `ase` are joined to `out_root` (when set), then to the manifest folder; absolute paths are kept.
//! * `png` and `overlay` are file-name patterns without folders. `{frame}` is the `.pxl` frame name.
//! * The declared outputs of a part are exactly: per frame one file per template layer, plus the `.aseprite`.
//!   `build` writes nothing else (apart from the part's `.status.json`); `check` flags other images in `out`.
//! * `queue.rs` reads the same `[parts]` table for `path`, `priority` and `tags` and ignores the output keys.

use crate::fsutil::PathSlash;
use crate::fsutil;
use crate::output::{Loc, PxlError, Result};
use std::path::{Path, PathBuf};

pub const DEFAULT_PNG: &str = "{frame}.png";
pub const DEFAULT_OVERLAY: &str = "{frame}_{layer}.png";

/// Where one part's files go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutSpec {
    /// Folder of the PNGs.
    pub dir: PathBuf,
    /// The `.aseprite` file.
    pub ase: PathBuf,
    pub png: String,
    pub overlay: String,
    /// Output keys recorded in the status file are relative to this folder (the manifest folder).
    pub rel_base: PathBuf,
}

impl OutSpec {
    /// Outputs next to a part that is not in a manifest: `<part folder>/<stem>_export/`.
    pub fn default_for(part_path: &Path) -> OutSpec {
        let dir = part_path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        if crate::asset::is_asset_file(part_path) {
            // an asset folder exports into its own `export/`
            let name = crate::asset::name_of(part_path);
            let out = dir.join("export");
            return OutSpec { ase: out.join(format!("{}.aseprite", name)), dir: out, png: DEFAULT_PNG.into(), overlay: DEFAULT_OVERLAY.into(), rel_base: dir };
        }
        let stem = part_path.file_stem().and_then(|s| s.to_str()).unwrap_or("part");
        let out = dir.join(format!("{}_export", stem));
        OutSpec { ase: out.join(format!("{}.aseprite", stem)), dir: out, png: DEFAULT_PNG.into(), overlay: DEFAULT_OVERLAY.into(), rel_base: dir }
    }

    /// Same patterns, outputs in `dir` (the `--out` override).
    pub fn with_dir(&self, dir: &Path, stem: &str) -> OutSpec {
        OutSpec { dir: dir.to_path_buf(), ase: dir.join(format!("{}.aseprite", stem)), png: self.png.clone(), overlay: self.overlay.clone(), rel_base: self.rel_base.clone() }
    }

    /// File name of the layer image of a frame (`layer_index` 0 = base).
    pub fn file_name(&self, frame: &str, layer_index: usize, layer: &str) -> String {
        let pat = if layer_index == 0 { &self.png } else { &self.overlay };
        pat.replace("{frame}", frame).replace("{layer}", layer)
    }

    pub fn file_path(&self, frame: &str, layer_index: usize, layer: &str) -> PathBuf {
        self.dir.join(self.file_name(frame, layer_index, layer))
    }

    /// Key under which an output is recorded in the status file.
    pub fn key(&self, p: &Path) -> String {
        let rel = p.strip_prefix(&self.rel_base).unwrap_or(p);
        rel.to_string_lossy().replace('\\', "/")
    }
}

/// One `[parts]` entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartDecl {
    pub name: String,
    /// Path of the `.pxl`, joined to the manifest folder.
    pub path: PathBuf,
    pub priority: i64,
    pub tags: Vec<String>,
    /// Template family for the `template` lint (a `family` in the part itself wins).
    pub family: Option<String>,
    pub spec: OutSpec,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub path: PathBuf,
    pub dir: PathBuf,
    pub out_root: Option<String>,
    pub parts: Vec<PartDecl>,
    /// `[review] siblings`: groups of part names that are shown together in review lineups.
    pub review_groups: Vec<Vec<String>>,
    /// `[scene.NAME]` sprite tile memory groups (`pxl budget`).
    pub scenes: Vec<Scene>,
    /// `legacy = true`: the project predates the shared-budget model; `pxl check` does not demand scenes and palette sets.
    pub legacy: bool,
    /// `[project] console = "gbc"`: the console this project targets (None = not set; `gbc` is assumed with a note).
    pub console: Option<String>,
}

/// A scene: parts that are in sprite memory together, in groups with a tile budget (unique 8x16 cells) or, for
/// streamed groups, a per-frame cap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scene {
    pub name: String,
    pub palette_set: Option<String>,
    /// BG tile set part (key) shown in the scene: its tiles and palettes are checked against the BG budget.
    pub tileset: Option<String>,
    /// Window / HUD / card tile parts (keys, paths, folders or globs) on the reserved BG palette.
    pub window: Vec<String>,
    /// Resident sprite cells this scene allots (checked against the effective sprite VRAM when BG tiles are left over).
    pub resident_budget: Option<u32>,
    /// Where `sprite_palettes.json` of this scene goes (below `out_root`); default `zones/<scene>/sprite_palettes.json` for `zone_N`.
    pub palettes_out: Option<String>,
    /// BG palettes reserved in this scene (default: the profile's `bg_reserved` when the scene has window parts, else none).
    pub bg_reserved: Option<Vec<u8>>,
    /// `[scene.NAME.reserve]`: slack held back for the other producer (see `Reserve`).
    pub reserve: Reserve,
    /// `layout = "layouts/zone_1.json"`: the level object layout (object id + position per act) of the scene, for zone completeness and real screens.
    pub layout: Option<String>,
    pub groups: Vec<SceneGroup>,
}

/// Reservations of a scene, so sprite and BG producers cannot eat each other's slack silently.
/// * `sprite_cells`: sprite cells held for sprite growth (counted as used in the sprite VRAM check);
/// * `bg_tiles`: BG tiles held for BG growth (never lent to sprites);
/// * `groups`: cells held per sprite group (counted as used in that group's budget);
/// * `palette_slots`: OBJ palette slots that `pxl palette plan` keeps out of the plan (a dynamic slot).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reserve {
    pub sprite_cells: u32,
    pub bg_tiles: u32,
    pub groups: std::collections::BTreeMap<String, u32>,
    pub palette_slots: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SceneGroup {
    pub name: String,
    pub budget: Option<u32>,
    pub streamed: bool,
    pub per_frame: Option<u32>,
    /// Part names, paths, folders or globs (`*` matches across folders).
    pub parts: Vec<String>,
}

fn merr(path: &Path, msg: impl Into<String>) -> PxlError {
    PxlError::validation(msg).with_loc(Loc::file(path.slash()))
}

fn check_pattern(path: &Path, ctx: &str, pat: &str, need_layer: bool) -> Result<()> {
    if !pat.contains("{frame}") {
        return Err(merr(path, format!("{}: pattern '{}' needs {{frame}}", ctx, pat)));
    }
    if pat.contains('/') || pat.contains('\\') {
        return Err(merr(path, format!("{}: pattern '{}' must be a file name without folders (set `out` for the folder)", ctx, pat)));
    }
    if !pat.to_ascii_lowercase().ends_with(".png") {
        return Err(merr(path, format!("{}: pattern '{}' must end in .png", ctx, pat)));
    }
    let stripped = pat.replace("{frame}", "").replace("{layer}", "");
    if stripped.contains('{') || stripped.contains('}') {
        return Err(merr(path, format!("{}: pattern '{}' knows only {{frame}} and {{layer}}", ctx, pat)));
    }
    let _ = need_layer;
    Ok(())
}

impl Manifest {
    pub fn parse(text: &str, path: &Path) -> Result<Manifest> {
        let root: toml::Table = text.parse().map_err(|e: toml::de::Error| {
            let (l, c) = e.span().map_or((0, 0), |s| {
                let b = &text[..s.start.min(text.len())];
                (b.matches('\n').count() + 1, b.rsplit('\n').next().map_or(0, |x| x.chars().count()) + 1)
            });
            merr(path, format!("{} (line {}, column {})", e.message(), l, c))
        })?;
        for k in root.keys() {
            if k != "out_root" && k != "parts" && k != "review" && k != "scene" && k != "legacy" && k != "project" && k != "style" && k != "delivery" {
                return Err(merr(path, format!("pxl.toml: unknown key '{}'", k)).with_hint("allowed: project, style, delivery, out_root, legacy, parts, review, scene"));
            }
        }
        if let Some(sv) = root.get("style") {
            let st = sv.as_table().ok_or_else(|| merr(path, "`style` must be a table: [style] white = \"active\""))?;
            crate::console::check_style(st).map_err(|e| merr(path, e))?;
        }
        if let Some(dv) = root.get("delivery") {
            let dt = dv.as_table().ok_or_else(|| merr(path, "`delivery` must be a table: [delivery] spec = \"export/spec.json\""))?;
            for (k, v) in dt {
                if !["spec", "client_spec", "client_checker", "client_pipeline", "package"].contains(&k.as_str()) {
                    return Err(merr(path, format!("[delivery]: unknown key '{}'", k)).with_hint("allowed: spec, client_spec, client_checker, client_pipeline, package"));
                }
                v.as_str().ok_or_else(|| merr(path, format!("[delivery] {} must be a string", k)))?;
            }
        }
        let mut console: Option<String> = None;
        if let Some(pv) = root.get("project") {
            let pt = pv.as_table().ok_or_else(|| merr(path, "`project` must be a table: [project] console = \"gbc\""))?;
            for (k, v) in pt {
                match k.as_str() {
                    "console" => console = Some(v.as_str().filter(|c| !c.is_empty()).ok_or_else(|| merr(path, "[project] console must be a console id such as \"gbc\""))?.to_string()),
                    "name" => {
                        v.as_str().ok_or_else(|| merr(path, "[project] name must be a string"))?;
                    }
                    "strict" => {
                        v.as_bool().ok_or_else(|| merr(path, "[project] strict must be true or false"))?;
                    }
                    "manifest" => {
                        v.as_bool().ok_or_else(|| merr(path, "[project] manifest must be true or false"))?;
                    }
                    "gates" => {
                        v.as_bool().ok_or_else(|| merr(path, "[project] gates must be true or false"))?;
                    }
                    "plan" => {
                        if !matches!(v.as_str(), Some("required") | Some("optional")) {
                            return Err(merr(path, "[project] plan must be \"required\" or \"optional\"").with_hint("\"required\": `next` and `claim` refuse until `pxl plan accept`"));
                        }
                    }
                    "defaults" | "settings" => {
                        v.as_table().ok_or_else(|| merr(path, format!("[project.{}] must be a table", k)))?;
                    }
                    o => return Err(merr(path, format!("[project]: unknown key '{}'", o)).with_hint("allowed: name, console, strict, plan, manifest, gates, defaults, settings")),
                }
            }
        }
        let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let out_root = match root.get("out_root") {
            None => None,
            Some(v) => Some(v.as_str().ok_or_else(|| merr(path, "out_root must be a string"))?.to_string()),
        };
        let join_root = |rel: &str| -> PathBuf {
            // both separators are accepted: a pxl.toml written by an older Windows build uses backslashes
            let rel = crate::fsutil::norm_slash(rel);
            let p = Path::new(&rel);
            if p.is_absolute() {
                return p.to_path_buf();
            }
            match &out_root {
                Some(r) => dir.join(r).join(p),
                None => dir.join(p),
            }
        };
        let mut parts = Vec::new();
        if let Some(pt) = root.get("parts") {
            let pt = pt.as_table().ok_or_else(|| merr(path, "`parts` must be a table"))?;
            for (name, val) in pt {
                let ctx = format!("parts.{}", name);
                let mut ppath: Option<String> = None;
                let (mut priority, mut tags) = (0i64, Vec::new());
                let mut family: Option<String> = None;
                let (mut out, mut ase, mut png, mut overlay) = (None::<String>, None::<String>, DEFAULT_PNG.to_string(), DEFAULT_OVERLAY.to_string());
                match val {
                    toml::Value::String(s) => ppath = Some(s.clone()),
                    toml::Value::Table(t) => {
                        for (k, v) in t {
                            let s = |what: &str| -> Result<String> {
                                v.as_str().map(String::from).ok_or_else(|| merr(path, format!("{}.{} must be a string", ctx, what)))
                            };
                            match k.as_str() {
                                "path" => ppath = Some(s("path")?),
                                "family" => family = Some(s("family")?),
                                "out" => out = Some(s("out")?),
                                "ase" => ase = Some(s("ase")?),
                                "png" => png = s("png")?,
                                "overlay" => overlay = s("overlay")?,
                                "priority" => priority = v.as_integer().ok_or_else(|| merr(path, format!("{}.priority must be an integer", ctx)))?,
                                "tags" => {
                                    let a = v.as_array().ok_or_else(|| merr(path, format!("{}.tags must be a list of strings", ctx)))?;
                                    for x in a {
                                        tags.push(x.as_str().ok_or_else(|| merr(path, format!("{}.tags must be a list of strings", ctx)))?.to_string());
                                    }
                                }
                                other => {
                                    return Err(merr(path, format!("{}: unknown key '{}'", ctx, other)).with_hint("allowed: path, family, priority, tags, out, ase, png, overlay"))
                                }
                            }
                        }
                    }
                    _ => return Err(merr(path, format!("{} must be a path string or a table", ctx))),
                }
                let ppath = crate::fsutil::norm_slash(&ppath.ok_or_else(|| merr(path, format!("{} has no `path`", ctx)))?);
                check_pattern(path, &format!("{}.png", ctx), &png, false)?;
                check_pattern(path, &format!("{}.overlay", ctx), &overlay, true)?;
                let part_path = dir.join(&ppath);
                let spec = match &out {
                    Some(o) => {
                        let od = join_root(o);
                        let ase_p = match &ase {
                            Some(a) => join_root(a),
                            None => od.join(format!("{}.aseprite", name)),
                        };
                        OutSpec { dir: od, ase: ase_p, png, overlay, rel_base: dir.clone() }
                    }
                    None => {
                        let mut d = OutSpec::default_for(&part_path);
                        d.png = png;
                        d.overlay = overlay;
                        d.rel_base = dir.clone();
                        if let Some(a) = &ase {
                            d.ase = join_root(a);
                        }
                        d
                    }
                };
                parts.push(PartDecl { name: name.clone(), path: part_path, priority, tags, family, spec });
            }
        }
        let mut review_groups: Vec<Vec<String>> = Vec::new();
        if let Some(r) = root.get("review") {
            let bad = || merr(path, "[review] siblings must be a list of part names, or a list of such lists");
            let t = r.as_table().ok_or_else(|| merr(path, "`review` must be a table"))?;
            for (k, v) in t {
                if k != "siblings" {
                    return Err(merr(path, format!("[review]: unknown key '{}'", k)).with_hint("allowed: siblings"));
                }
                let a = v.as_array().ok_or_else(bad)?;
                let strs = |x: &[toml::Value]| -> Result<Vec<String>> { x.iter().map(|e| e.as_str().map(String::from).ok_or_else(bad)).collect() };
                if a.iter().all(|e| e.is_str()) {
                    review_groups.push(strs(a)?);
                } else {
                    for g in a {
                        review_groups.push(strs(g.as_array().ok_or_else(bad)?)?);
                    }
                }
            }
        }
        let mut scenes = Vec::new();
        if let Some(sv) = root.get("scene") {
            let st = sv.as_table().ok_or_else(|| merr(path, "`scene` must be a table of scenes: [scene.zone_2]"))?;
            for (sname, v) in st {
                let ctx = format!("[scene.{}]", sname);
                let t = v.as_table().ok_or_else(|| merr(path, format!("{} must be a table", ctx)))?;
                let mut sc = Scene { name: sname.clone(), palette_set: None, tileset: None, window: Vec::new(), resident_budget: None, palettes_out: None, bg_reserved: None, reserve: Reserve::default(), layout: None, groups: Vec::new() };
                for (k, x) in t {
                    match k.as_str() {
                        "palette_set" => sc.palette_set = Some(x.as_str().ok_or_else(|| merr(path, format!("{}: palette_set must be a string", ctx)))?.to_string()),
                        "tileset" => sc.tileset = Some(x.as_str().ok_or_else(|| merr(path, format!("{}: tileset must be a part key", ctx)))?.to_string()),
                        "resident_budget" => sc.resident_budget = Some(x.as_integer().filter(|i| *i >= 0).map(|i| i as u32).ok_or_else(|| merr(path, format!("{}: resident_budget must be a non-negative integer", ctx)))?),
                        "layout" => sc.layout = Some(fsutil::norm_slash(x.as_str().ok_or_else(|| merr(path, format!("{}: layout must be a path", ctx)))?)),
                        "palettes_out" => sc.palettes_out = Some(fsutil::norm_slash(x.as_str().ok_or_else(|| merr(path, format!("{}: palettes_out must be a path", ctx)))?)),
                        "bg_reserved" => {
                            sc.bg_reserved = Some(x.as_array().and_then(|a| a.iter().map(|e| e.as_integer().filter(|n| (0..8).contains(n)).map(|n| n as u8)).collect::<Option<Vec<_>>>()).ok_or_else(|| merr(path, format!("{}: bg_reserved must be a list of BG palette numbers 0..7", ctx)))?)
                        }
                        "reserve" => {
                            let rt = x.as_table().ok_or_else(|| merr(path, format!("{}: reserve must be a table: [scene.NAME.reserve]", ctx)))?;
                            let uint = |what: &str, v: &toml::Value| v.as_integer().filter(|i| *i >= 0).map(|i| i as u32).ok_or_else(|| merr(path, format!("{} reserve: {} must be a non-negative integer", ctx, what)));
                            for (rk, rv) in rt {
                                match rk.as_str() {
                                    "sprite_cells" => sc.reserve.sprite_cells = uint("sprite_cells", rv)?,
                                    "bg_tiles" => sc.reserve.bg_tiles = uint("bg_tiles", rv)?,
                                    "palette_slots" => {
                                        sc.reserve.palette_slots = rv.as_array().and_then(|a| a.iter().map(|e| e.as_integer().filter(|n| (0..8).contains(n)).map(|n| n as u8)).collect::<Option<Vec<_>>>()).ok_or_else(|| merr(path, format!("{} reserve: palette_slots must be a list of slot numbers 0..7", ctx)))?
                                    }
                                    "groups" => {
                                        let gt = rv.as_table().ok_or_else(|| merr(path, format!("{} reserve: groups must be a table {{ group = cells }}", ctx)))?;
                                        for (gn, gv) in gt {
                                            sc.reserve.groups.insert(gn.clone(), uint(&format!("groups.{}", gn), gv)?);
                                        }
                                    }
                                    o => return Err(merr(path, format!("{} reserve: unknown key '{}'", ctx, o)).with_hint("allowed: sprite_cells, bg_tiles, groups, palette_slots")),
                                }
                            }
                        }
                        "window" => {
                            sc.window = x.as_array().and_then(|a| a.iter().map(|e| e.as_str().map(String::from)).collect::<Option<Vec<_>>>()).ok_or_else(|| merr(path, format!("{}: window must be a list of part keys", ctx)))?
                        }
                        "groups" => {
                            let gt = x.as_table().ok_or_else(|| merr(path, format!("{}: groups must be a table of groups", ctx)))?;
                            for (gname, gv) in gt {
                                let gctx = format!("{} group '{}'", ctx, gname);
                                let g = gv.as_table().ok_or_else(|| merr(path, format!("{}: expected {{ budget = N, parts = [..] }}", gctx)))?;
                                let mut sg = SceneGroup { name: gname.clone(), budget: None, streamed: false, per_frame: None, parts: Vec::new() };
                                for (gk, gx) in g {
                                    let int = |what: &str| gx.as_integer().filter(|i| *i >= 0).map(|i| i as u32).ok_or_else(|| merr(path, format!("{}: {} must be a non-negative integer", gctx, what)));
                                    match gk.as_str() {
                                        "budget" => sg.budget = Some(int("budget")?),
                                        "per_frame" => sg.per_frame = Some(int("per_frame")?),
                                        "streamed" => sg.streamed = gx.as_bool().ok_or_else(|| merr(path, format!("{}: streamed must be true or false", gctx)))?,
                                        "parts" => {
                                            sg.parts = gx.as_array().and_then(|a| a.iter().map(|e| e.as_str().map(String::from)).collect::<Option<Vec<_>>>()).ok_or_else(|| merr(path, format!("{}: parts must be a list of strings", gctx)))?
                                        }
                                        o => return Err(merr(path, format!("{}: unknown key '{}'", gctx, o)).with_hint("allowed: budget, parts, streamed, per_frame")),
                                    }
                                }
                                if sg.streamed && sg.per_frame.is_none() {
                                    return Err(merr(path, format!("{}: a streamed group needs per_frame", gctx)));
                                }
                                if !sg.streamed && sg.budget.is_none() {
                                    return Err(merr(path, format!("{}: needs budget = N (or streamed = true, per_frame = N)", gctx)));
                                }
                                sc.groups.push(sg);
                            }
                        }
                        o => return Err(merr(path, format!("{}: unknown key '{}'", ctx, o)).with_hint("allowed: palette_set, tileset, window, resident_budget, bg_reserved, palettes_out, reserve, layout, groups")),
                    }
                }
                scenes.push(sc);
            }
        }
        let legacy = match root.get("legacy") {
            None => false,
            Some(v) => v.as_bool().ok_or_else(|| merr(path, "legacy must be true or false"))?,
        };
        Ok(Manifest { path: path.to_path_buf(), dir, out_root, parts, review_groups, scenes, legacy, console })
    }

    pub fn load(path: &Path) -> Result<Manifest> {
        Manifest::parse(&fsutil::read_string(path)?, path)
    }

    /// `--manifest` if given, else the nearest `pxl.toml` (`./pxl.toml`, then `./pxl/pxl.toml`, then the same in each parent folder) (`None` when absent).
    pub fn discover(explicit: Option<&str>) -> Result<Option<Manifest>> {
        if let Some(p) = explicit {
            let pb = PathBuf::from(p);
            if !pb.is_file() {
                return Err(PxlError::usage(format!("manifest '{}' not found", p)).with_hint("pass the path of a pxl.toml, or omit --manifest to search ./pxl.toml, ./pxl/pxl.toml and the parent folders"));
            }
            return Manifest::load(&pb).map(Some);
        }
        match Manifest::find_file(&std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))) {
            Some(p) => {
                // keep relative paths short when the manifest is in (or just under) the current folder
                let p = match std::env::current_dir().ok().and_then(|cwd| p.strip_prefix(&cwd).ok().map(|r| r.to_path_buf())) {
                    Some(r) if r.components().count() <= 2 => r,
                    _ => p,
                };
                Manifest::load(&p).map(Some)
            }
            None => Ok(None),
        }
    }

    /// Manifest search from `start`: `./pxl.toml`, then `./pxl/pxl.toml`, then the same two in each parent folder.
    pub fn find_file(start: &Path) -> Option<PathBuf> {
        let mut dir = Some(start);
        while let Some(d) = dir {
            for rel in ["pxl.toml", "pxl/pxl.toml"] {
                let c = d.join(rel);
                if c.is_file() {
                    return Some(c);
                }
            }
            dir = d.parent();
        }
        None
    }

    /// The entry declaring `part_path` (compared after canonicalisation).
    pub fn find(&self, part_path: &Path) -> Option<&PartDecl> {
        let want = canon(part_path);
        self.parts.iter().find(|d| canon(&d.path) == want)
    }
}

pub fn canon(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_both_forms() {
        let m = Manifest::parse(
            "out_root = \"o\"\n[parts]\na = \"parts/a.pxl\"\n[parts.b]\npath = \"parts/b.pxl\"\nout = \"x/b\"\ntags = [\"t\"]\npriority = 2\n",
            Path::new("m/pxl.toml"),
        )
        .unwrap();
        assert_eq!(m.parts.len(), 2);
        assert_eq!(m.parts[1].spec.dir, Path::new("m/o/x/b"));
        assert_eq!(m.parts[1].spec.ase, Path::new("m/o/x/b/b.aseprite"));
        assert_eq!(m.parts[0].spec.dir, Path::new("m/parts/a_export"));
        assert_eq!(m.parts[1].priority, 2);
    }

    #[test]
    fn a_manifest_with_backslash_paths_loads_like_slashes() {
        let a = Manifest::parse("[parts.a]\npath = \"parts\\\\g\\\\a.pxl\"\nout = \"x\\\\a\"\n", Path::new("m/pxl.toml")).unwrap();
        let b = Manifest::parse("[parts.a]\npath = \"parts/g/a.pxl\"\nout = \"x/a\"\n", Path::new("m/pxl.toml")).unwrap();
        assert_eq!(a.parts[0].path, b.parts[0].path);
        assert_eq!(a.parts[0].spec.dir, b.parts[0].spec.dir);
        assert_eq!(a.parts[0].spec.key(&a.parts[0].spec.file_path("f0", 1, "")), b.parts[0].spec.key(&b.parts[0].spec.file_path("f0", 1, "")));
    }

    #[test]
    fn unknown_keys_are_errors() {
        assert!(Manifest::parse("bogus = 1\n", Path::new("pxl.toml")).is_err());
        assert!(Manifest::parse("[parts.a]\npath = \"a.pxl\"\nnope = 1\n", Path::new("pxl.toml")).is_err());
        assert!(Manifest::parse("[parts.a]\npath = \"a.pxl\"\npng = \"x.png\"\n", Path::new("pxl.toml")).is_err());
    }
}
