//! Command line surface (DESIGN 5-8). Argument structs are public so module owners can read them.
//! Each command dispatches (in `main.rs`) to `crate::<module>::run_<cmd>(&Args, &mut Out)`.

use crate::model::{Frame, Part};
use crate::output::{PxlError, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::str::FromStr;

// ---------------------------------------------------------------- value types

/// `x,y`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

/// `x,y,w,h`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    pub x: i32,
    pub y: i32,
    pub w: usize,
    pub h: usize,
}

/// `WxH`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Size {
    pub w: usize,
    pub h: usize,
}

/// `x,y=K` where K is a key character or a template role name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pixel {
    pub x: i32,
    pub y: i32,
    pub key: String,
}

/// `A=B`: replace key/role A by B.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyMap {
    pub from: String,
    pub to: String,
}

/// `K=#rrggbb`
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyColor {
    pub key: String,
    pub color: String,
}

fn ints(s: &str, n: usize, what: &str) -> std::result::Result<Vec<i64>, String> {
    let v: std::result::Result<Vec<i64>, _> = s.split(',').map(|p| p.trim().parse::<i64>()).collect();
    match v {
        Ok(v) if v.len() == n => Ok(v),
        _ => Err(format!("expected {} (e.g. {})", what, if n == 2 { "17,11" } else { "9,6,14,10" })),
    }
}

impl FromStr for Point {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Point, String> {
        let v = ints(s, 2, "x,y")?;
        Ok(Point { x: v[0] as i32, y: v[1] as i32 })
    }
}
impl FromStr for Region {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Region, String> {
        let v = ints(s, 4, "x,y,w,h")?;
        if v[2] < 0 || v[3] < 0 {
            return Err("width and height must not be negative".into());
        }
        Ok(Region { x: v[0] as i32, y: v[1] as i32, w: v[2] as usize, h: v[3] as usize })
    }
}
impl FromStr for Size {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Size, String> {
        let (a, b) = s.split_once(['x', 'X']).ok_or("expected WxH (e.g. 32x32)")?;
        match (a.trim().parse::<usize>(), b.trim().parse::<usize>()) {
            (Ok(w), Ok(h)) if w > 0 && h > 0 => Ok(Size { w, h }),
            _ => Err("expected WxH with positive integers (e.g. 32x32)".into()),
        }
    }
}
impl FromStr for Pixel {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Pixel, String> {
        let (c, k) = s.split_once('=').ok_or("expected x,y=KEY (e.g. 17,11=W)")?;
        let p: Point = c.parse()?;
        if k.is_empty() {
            return Err("missing key after '='".into());
        }
        Ok(Pixel { x: p.x, y: p.y, key: k.to_string() })
    }
}
impl FromStr for KeyMap {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<KeyMap, String> {
        let (a, b) = s.split_once('=').ok_or("expected A=B (e.g. 2=1)")?;
        if a.is_empty() || b.is_empty() {
            return Err("expected A=B (e.g. 2=1)".into());
        }
        Ok(KeyMap { from: a.into(), to: b.into() })
    }
}
impl FromStr for KeyColor {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<KeyColor, String> {
        let (a, b) = s.split_once('=').ok_or("expected KEY=#rrggbb (e.g. 2=#4850b8)")?;
        Ok(KeyColor { key: a.into(), color: b.into() })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Axis {
    X,
    Y,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum KindArg {
    Sprite,
    Tileset,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum ReleaseState {
    Done,
    Blocked,
}

// ---------------------------------------------------------------- shared arg groups

/// Frame selection: `--frames a,b` or `--all` (no flag = all frames).
#[derive(Args, Clone, Debug, Default)]
pub struct FrameSel {
    /// Frame names, comma separated
    #[arg(long, value_delimiter = ',')]
    pub frames: Vec<String>,
    /// Every frame (the default when --frames is absent)
    #[arg(long)]
    pub all: bool,
}

impl FrameSel {
    /// The selected frames in part order, deduplicated. Each `--frames` entry is an exact name, a
    /// wildcard (`walk*`, `0?_*`), a 1-based inclusive index range (`6-11`) or `tag:<tag>`.
    /// Unknown names and empty matches are a usage error listing the available frames and tags.
    pub fn select<'a>(&self, part: &'a Part) -> Result<Vec<&'a Frame>> {
        if self.all || self.frames.is_empty() {
            return Ok(part.frames.iter().collect());
        }
        let n = part.frames.len();
        let mut hit = vec![false; n];
        for e in &self.frames {
            let e = e.trim();
            if let Some(i) = part.frames.iter().position(|f| f.name == e) {
                hit[i] = true;
                continue;
            }
            let mut any = false;
            if let Some(t) = e.strip_prefix("tag:") {
                for (i, f) in part.frames.iter().enumerate() {
                    if crate::lint::tag_of(&f.name) == t {
                        hit[i] = true;
                        any = true;
                    }
                }
            } else if let Some((a, b)) = e.split_once('-').filter(|(a, b)| !a.is_empty() && !b.is_empty() && a.bytes().all(|c| c.is_ascii_digit()) && b.bytes().all(|c| c.is_ascii_digit())) {
                let (a, b): (usize, usize) = (a.parse().unwrap_or(0), b.parse().unwrap_or(0));
                if a >= 1 && a <= b && b <= n {
                    hit[a - 1..b].iter_mut().for_each(|h| *h = true);
                    any = true;
                } else {
                    return Err(PxlError::usage(format!("index range '{}' is outside 1-{} (part '{}' has {} frames)", e, n, part.name, n))
                        .with_hint(format!("indices are 1-based and inclusive, e.g. 6-11 or 1-{}; or use a name, walk*, tag:walk", n)));
                }
            } else if e.contains('*') || e.contains('?') {
                for (i, f) in part.frames.iter().enumerate() {
                    // also match without the leading `NN_` order prefix (`walk1*` finds `06_walk11`)
                    let bare = match f.name.split_once('_') {
                        Some((p, r)) if !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit()) => r,
                        _ => f.name.as_str(),
                    };
                    if crate::fsutil::wildcard(e.as_bytes(), f.name.as_bytes()) || crate::fsutil::wildcard(e.as_bytes(), bare.as_bytes()) {
                        hit[i] = true;
                        any = true;
                    }
                }
            }
            if !any {
                return Err(unknown_frame(part, e));
            }
        }
        Ok(part.frames.iter().zip(hit).filter(|(_, h)| *h).map(|(f, _)| f).collect())
    }
}

/// Usage error for a frame name that does not exist in `part`.
pub fn unknown_frame(part: &Part, name: &str) -> PxlError {
    let names: Vec<&str> = part.frames.iter().take(8).map(|f| f.name.as_str()).collect();
    let mut tags: Vec<(String, usize)> = Vec::new();
    for f in &part.frames {
        let t = crate::lint::tag_of(&f.name);
        match tags.iter_mut().find(|(x, _)| *x == t) {
            Some(e) => e.1 += 1,
            None => tags.push((t, 1)),
        }
    }
    let tg: Vec<String> = tags.iter().take(12).map(|(t, c)| format!("{}({})", t, c)).collect();
    let all: Vec<String> = part.frames.iter().map(|f| f.name.clone()).collect();
    let did = crate::snippet::closest(name, &all).map(|c| format!("did you mean '{}'? ", c)).unwrap_or_default();
    PxlError::usage(format!("part '{}' has no frame matching '{}'", part.name, name)).with_hint(format!(
        "{}frames: {}{} ({} total); tags: {}{}; selectors: name, walk*, 6-11, tag:walk",
        did,
        names.join(", "),
        if part.frames.len() > 8 { ", ..." } else { "" },
        part.frames.len(),
        tg.join(" "),
        if tags.len() > 12 { " ..." } else { "" }
    ))
}

/// Flags every modifying edit command accepts.
#[derive(Args, Clone, Debug, Default)]
pub struct EditFlags {
    /// Do not lock the frame after editing
    #[arg(long)]
    pub no_lock: bool,
    /// Show the result without writing the part
    #[arg(long)]
    pub dry_run: bool,
    /// Also write a PNG (reference | before | after, region marked) and print its path; optional path
    #[arg(long, num_args = 0..=1, default_missing_value = "auto", value_name = "PATH")]
    pub preview: Option<String>,
    /// Echo the reference letters next to the changed region (default: on when the echo stays under 15 lines)
    #[arg(long = "ref")]
    pub reference: bool,
    /// Never echo the reference letters
    #[arg(long = "no-ref")]
    pub no_ref: bool,
    /// set / row / patch on a tile set: edit tile N with tile-relative coordinates 0..7
    #[arg(long, value_name = "N")]
    pub tile: Option<usize>,
    /// Edit a part whose state is `approved` anyway (normally: `pxl rework PART --note ...` first)
    #[arg(long)]
    pub force_approved: bool,
}

/// Text argument: `-` reads stdin, anything else is the text itself.
pub fn read_text_arg(s: &str) -> Result<String> {
    if s == "-" {
        use std::io::Read;
        let mut t = String::new();
        std::io::stdin().read_to_string(&mut t)?;
        Ok(t)
    } else {
        Ok(s.to_string())
    }
}

// ---------------------------------------------------------------- top level

#[derive(Parser, Debug)]
#[command(name = "pxl", version, about = "Pixel art for AI agents: load, adjust, compare, export. Run `pxl guide` first.", propagate_version = true)]
pub struct Cli {
    /// Print exactly one JSON object on stdout
    #[arg(long, global = true)]
    pub json: bool,
    /// Agent identity (leases; renewed by every modifying command)
    #[arg(long, global = true, value_name = "NAME")]
    pub agent: Option<String>,
    /// Do not store versions / compare.png for this run (bulk internal operations)
    #[arg(long, global = true)]
    pub no_version: bool,
    /// Allow changing an asset the user approved personally (a review task is created)
    #[arg(long, global = true)]
    pub force_user_approved: bool,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Read inputs, render references, write a first reduction (or --new to create a part)
    Load(LoadArgs),
    /// Regenerate unlocked frames after input, template or reduce changes
    Reload(ReloadArgs),
    /// Colour templates: suggest | set
    #[command(subcommand)]
    Template(TemplateCmd),
    /// Set individual pixels: pxl set PART FRAME 17,11=W 18,11=W
    Set(SetArgs),
    /// Reusable shapes: save | list (then `pxl stamp`)
    #[command(subcommand)]
    Snippet(SnippetCmd),
    /// Paste a saved snippet: pxl stamp PART FRAME eye 17,11 [--flip x]
    Stamp(StampArgs),
    /// Frames of several parts side by side at 1x and zoomed, to judge textures and neighbours: pxl lineup wall_a:1,2 wall_b:all
    Lineup(LineupArgs),
    /// Fill a region by repeating a unit with a fixed phase: pxl pattern PART FRAME brick --region 0,0,32,16 [--mask 2]
    Pattern(PatternArgs),
    /// Move the pixels of a region: pxl move PART FRAME --region 12,6,6,4 --by 1,0
    Move(MoveArgs),
    /// Shift a whole frame: pxl nudge PART FRAME 0,-1
    Nudge(NudgeArgs),
    /// Re-close the outline ring around the shape: pxl outline PART FRAME [--key outline]
    Outline(OutlineArgs),
    /// Apply the lint fixes of a frame in one call: pxl fix PART FRAME [--rules orphan,jaggy,eye]
    Fix(FixArgs),
    /// One frame next to its reference as a PNG with a cell grid and coordinate labels
    View(ViewArgs),
    /// Write a run of pixels on one row: pxl row PART FRAME 16 13 "1222221" (x=16 y=13 also works)
    Row(RowArgs),
    /// Overwrite a rectangle from a grid (space = unchanged; "-" reads stdin)
    Patch(PatchArgs),
    /// 1 px line: pxl line PART FRAME 9,12 14,12 1
    Line(LineArgs),
    /// Rectangle: pxl rect PART FRAME 9,6,6,8 3
    Rect(RectArgs),
    /// Flood fill (4-connected): pxl fill PART FRAME 15,17 3
    Fill(FillArgs),
    /// Swap keys in a region: pxl replace PART FRAME 2=1 --region 9,6,6,8
    Replace(ReplaceArgs),
    /// Copy pixels between frames: pxl copy PART FROM TO --region x,y,w,h
    Copy(CopyArgs),
    /// Make a frame symmetric: pxl mirror PART FRAME x
    Mirror(MirrorArgs),
    /// Flip a frame (or region) along an axis
    Flip(FlipArgs),
    /// Replace a whole frame from a grid ("-" reads stdin)
    #[command(name = "write_grid", alias = "write-grid")]
    WriteGrid(WriteGridArgs),
    /// Alternative reductions of frames side by side (read-only)
    Candidates(CandidatesArgs),
    /// Write one method's reduction into a frame, whole or per region
    Pick(PickArgs),
    /// Lock frames (reload/load keep locked frames): PART FRAMES | --all. One undo step
    Lock(LockArgs),
    /// Mark frames (or --tiles A-B) as hand-finished without changing pixels, with a --reason the reviewer sees
    Finish(FinishArgs),
    /// Unlock frames so load/reload may regenerate them again: PART FRAMES | --all
    Unlock(LockArgs),
    /// Atomic batch of edits from a JSON file ("-" reads stdin)
    Apply(ApplyArgs),
    /// Revert the last edits (journal next to the part)
    Undo(UndoArgs),
    /// Report or fix stray pixels and jaggies
    Clean(CleanArgs),
    /// Show a frame as text with rulers (--ref adds the input)
    Show(ShowArgs),
    /// Comparison sheets and metrics for frames
    Compare(CompareArgs),
    /// Pixel differences between two parts/versions
    Diff(DiffArgs),
    /// Palette-set parts: the 8x16 cell layout, slot per cell, sprites per scanline (read-only); `cells dedupe PART --max N` merges near-identical cells
    Cells(CellsArgs),
    /// Palette sets: fit a part into its set; `palette plan SCENE` plans one set for all inputs of a scene
    #[command(subcommand)]
    Palette(PaletteCmd),
    /// Facts no pixel holds (collision, repeat, compose, residency, frame semantics, BG form, zone remap) in the asset's meta.toml
    Meta(MetaArgs),
    /// Contact sheet of a group (or an asset): latest frames in the palette set, on a background colour
    Sheet(SheetArgs),
    /// Assemble the delivery package from the manifest (drops and replacements from the asset records; no stale files)
    Package(PackageArgs),
    /// Check a package against its manifest and the console (the verify_m5 rules, generalised)
    VerifyPackage(VerifyPackageArgs),
    /// Run our checks, the package verifier and the client's checker / pipeline, then snapshot to deliveries/N/
    Deliver(DeliverArgs),
    /// Size, frame, origin, footprint and palette changes between two delivery snapshots
    Changes(ChangesArgs),
    /// Source QA: noise references, art of another zone, labels marked unused; each finding becomes a question task
    Qa(QaArgs),
    /// Plan before pixels: propose scenes, VRAM fit, group budgets and a shared palette from all inputs; `plan accept` signs it off
    Plan(PlanArgs),
    /// Scene budgets (palettes, BG and sprite tiles, window, DMA); `--worst` composes the worst-case screen
    Scene(SceneArgs),
    /// Fill DELIVERY_TEMPLATE.md: assets, checker output, per-scene budgets, parts, exceptions
    Delivery(DeliveryArgs),
    /// Sprite tile memory: unique 8x16 cells per part and per scene group (pxl.toml [scene.NAME])
    Budget(BudgetArgs),
    /// Lint parts against the profile rules
    Lint(LintArgs),
    /// Export indexed PNGs and .aseprite
    Export(ExportArgs),
    /// Export every part in pxl.toml (incremental), then the sprite palette files of the zone scenes
    Build(BuildArgs),
    /// Write zones/zone_N/sprite_palettes.json (hardware feedback section 7) from the scenes' palette sets
    #[command(name = "export-palettes")]
    ExportPalettes(ExportPalettesArgs),
    /// Lint everything plus exact output files; exit 0 only when all pass
    Check(CheckArgs),
    /// Bring a PNG, grid text or .aseprite back into a part
    Import(ImportArgs),
    /// Tile sets: stats | fit | dedupe | wrap | solid
    #[command(subcommand)]
    Tiles(TilesCmd),
    /// Claim the next part that needs work
    Next(NextArgs),
    /// Claim a specific part
    Claim(PartArgs),
    /// Extend the lease on a part
    Renew(PartArgs),
    /// Give a part back with a state and a note
    Release(ReleaseArgs),
    /// Project status at a glance
    Status(StatusArgs),
    /// Coordinator review sheets
    Review(ReviewArgs),
    /// Mark a part approved
    Approve(ApproveArgs),
    /// Send a part back to rework with a note
    Rework(ReworkArgs),
    /// Print the short quick reference (the skills hold the craft: `pxl skill show pxl-sprite`)
    Guide {
        /// `project`: the second page (single image, project mode, versions, tasks, review)
        topic: Option<String>,
    },
    /// Skills shipped with the tool: list | show | install | uninstall | path
    #[command(subcommand)]
    Skill(SkillCmd),
    /// Consoles and their image modes: list | show [ID] | set ID (writes `[project] console`)
    #[command(subcommand)]
    Console(ConsoleCmd),
    /// One image, no project: `pxl image FILE.png` makes the asset folder FILE.pxl/ beside it (input, first reduction, versions, compare.png)
    Image(ImageArgs),
    /// Create a project folder: pxl.toml, inbox/, assets/, palettes/, consoles/, templates/, export/, review/
    New(NewArgs),
    /// Turn images (inbox/, files, globs, folders) into assets; a changed input becomes a new input and a new version
    Ingest(IngestArgs),
    /// Asset groups: list | add NAME | move ASSET GROUP
    #[command(subcommand)]
    Group(GroupCmd),
    /// Asset settings kept in the asset header: pxl asset set ASSET category player
    #[command(subcommand)]
    Asset(AssetCmd),
    /// The project: groups, assets, state, latest version, open tasks
    Tree,
    /// Project settings: show
    #[command(subcommand)]
    Project(ProjectCmd),
    /// Versions of an asset
    Versions(VersionsArgs),
    /// Restore a version of an asset as a new version: pxl revert ASSET v003
    Revert(RevertArgs),
    /// Work list: add | done
    #[command(subcommand)]
    Task(TaskCmd),
    /// Open tasks: --open (default) --mine --assign NAME --group G --all
    Tasks(TasksArgs),
    /// One-page overview per group: assets by state, open tasks, decisions (--html FILE for a static page)
    Board(BoardArgs),
    /// Record a decision (decisions.md): pxl decide "moustache stays orange" --asset A --by user
    Decide(DecideArgs),
    /// Exceptions stored with an asset: add ASSET "change" --reason ...
    #[command(subcommand)]
    Exception(ExceptionCmd),
    /// Variants of an asset: add | list | deliver ASSET NAME
    #[command(subcommand)]
    Variant(VariantCmd),
    /// Everything a fresh agent needs for an asset or group: decisions, tasks, review history, budgets, paths
    Brief(BriefArgs),
    /// Intake, mostly inferred: reads the request and the inputs, writes brief.toml and the assumptions, asks only real questions (`--interview` opts in)
    Intake(IntakeArgs),
    /// The design language: analyze | propose | approve | mark | show (style.md, style.toml)
    #[command(subcommand)]
    Style(StyleCmd),
    /// Milestones: status (plan accepted, style approved, dry run passed)
    #[command(subcommand)]
    Milestone(MilestoneCmd),
    /// Open a decision with options and an image page: pxl ask TOPIC --option A=img --option B=img --recommend A "why"
    Ask(AskArgs),
    /// Open decisions with their options and recommendations (--all: closed ones too)
    Decisions(DecisionsArgs),
    /// Progress digest for the user: done, in review, new images, pending decisions with recommendations, budgets, milestones (--html)
    Report(ReportArgs),
    /// Rewrite an old project to the console model: `[project] console`, `mode` for `kind`, `profile = "gbc-*"` -> console / mode
    Migrate(MigrateArgs),
}

#[derive(Subcommand, Debug)]
pub enum ConsoleCmd {
    /// Every console: name, screen and short facts (the project's is marked *)
    List,
    /// The merged hardware facts of a console per image mode (default: the project's console)
    Show {
        /// Console id, e.g. gbc
        id: Option<String>,
    },
    /// Write `[project] console = "ID"` into pxl.toml
    Set {
        /// Console id, e.g. gbc
        id: String,
    },
}

#[derive(Args, Clone, Debug)]
pub struct MigrateArgs {
    /// Report what would change; write nothing
    #[arg(long)]
    pub dry_run: bool,
    /// `assets`: also move the parts of the manifest into the asset layout (assets/<group>/<name>/asset.pxl)
    #[arg(long, value_name = "assets")]
    pub layout: Option<String>,
    #[arg(long)]
    pub manifest: Option<String>,
}

// ---------------------------------------------------------------- load

#[derive(Args, Clone, Debug, Default)]
pub struct LoadArgs {
    /// Parts, globs or directories (omit with --new)
    pub parts: Vec<String>,
    /// Create a part with this name
    #[arg(long, value_name = "NAME")]
    pub new: Option<String>,
    /// Input image or spec (with --new)
    #[arg(long)]
    pub input: Option<String>,
    /// Canvas size WxH (with --new)
    #[arg(long, value_name = "WxH")]
    pub size: Option<Size>,
    /// Origin X,Y (with --new)
    #[arg(long, value_name = "X,Y", allow_hyphen_values = true)]
    pub origin: Option<Point>,
    /// Template path (with --new)
    #[arg(long)]
    pub template: Option<String>,
    /// Profile name (with --new)
    #[arg(long)]
    pub profile: Option<String>,
    /// Output path for --new (default ./NAME.pxl)
    #[arg(long)]
    pub out: Option<String>,
    /// Integer factor of the input (with --new)
    #[arg(long)]
    pub factor: Option<u32>,
    /// Downscale method for this run: vote | nearest | contour | exemplar | auto
    #[arg(long)]
    pub method: Option<String>,
    /// Also store --method in the part's [reduce]
    #[arg(long)]
    pub set_method: bool,
    /// Also regenerate LOCKED frames (their hand edits are lost); same as `reload --force`. Without it locked frames are kept
    #[arg(long)]
    pub force: bool,
    #[command(flatten)]
    pub sel: FrameSel,
    /// Load a part whose state is `approved` anyway
    #[arg(long)]
    pub force_approved: bool,
}

#[derive(Args, Clone, Debug, Default)]
pub struct ReloadArgs {
    pub parts: Vec<String>,
    /// Downscale method for this run: vote | nearest | contour | exemplar | auto
    #[arg(long)]
    pub method: Option<String>,
    /// Also store --method in the part's [reduce]
    #[arg(long)]
    pub set_method: bool,
    /// Compare methods (comma separated) without writing the part: metrics table per frame
    #[arg(long = "try", value_delimiter = ',', value_name = "METHODS")]
    pub try_methods: Vec<String>,
    /// With --try: let exemplar-based methods use the frame's own locked/exemplar grid (default: hold it out)
    #[arg(long)]
    pub no_holdout: bool,
    /// With --try: directory for side-by-side preview PNGs (reference | method A | method B ...)
    #[arg(long = "preview-dir", visible_alias = "out", value_name = "DIR")]
    pub preview_dir: Option<String>,
    #[command(flatten)]
    pub sel: FrameSel,
    /// Also overwrite locked frames
    #[arg(long)]
    pub force: bool,
    /// Reload a part whose state is `approved` anyway
    #[arg(long)]
    pub force_approved: bool,
}

// ---------------------------------------------------------------- template / tiles

#[derive(Args, Clone, Debug, Default)]
pub struct SuggestArgs {
    /// Part whose inputs are analysed
    pub part: String,
    /// Number of candidates
    #[arg(long, default_value_t = 4)]
    pub candidates: usize,
    /// Write the chosen candidate (1-based) to this new template file; never overwrites
    #[arg(long, value_name = "FILE")]
    pub write: Option<String>,
    /// Candidate number to write (with --write)
    #[arg(long, default_value_t = 1)]
    pub pick: usize,
    /// Full derivation from the native resolution inputs: colours per layer, overlay, explicit [map], error report
    #[arg(long)]
    pub derive: bool,
    /// With --derive: 1 = base only, 2 = force an overlay (default: add one only when it pays)
    #[arg(long, value_name = "1|2")]
    pub layers: Option<u8>,
    /// With --derive: `real` (best snapped real source colours) or `optimised` (hill-climbed on the snapped grid)
    #[arg(long, default_value = "optimised", value_name = "real|optimised")]
    pub mode: String,
    /// Keep the colours of this template and derive only its [map] (with --derive output and --write)
    #[arg(long, value_name = "TEMPLATE")]
    pub keep_colors: Option<String>,
    /// With --derive: also re-centre key colours on the block means of the reduction cells (lower `compare` error, washed-out colours)
    #[arg(long)]
    pub refine_cells: bool,
    /// Print the error report of two or more templates side by side: --compare A B
    #[arg(long, num_args = 2.., value_name = "TEMPLATE")]
    pub compare: Vec<String>,
}

#[derive(Args, Clone, Debug)]
pub struct TemplateSetArgs {
    /// Template file (or name under templates/)
    pub template: String,
    /// KEY=#rrggbb (key character or role name)
    #[arg(required = true)]
    pub assigns: Vec<String>,
}

#[derive(Args, Clone, Debug)]
pub struct AddSetArgs {
    /// Template file (or name under templates/)
    pub template: String,
    /// Palette set (palettes/NAME.toml above the template); repeat for several
    #[arg(long = "set", required = true)]
    pub sets: Vec<String>,
}

#[derive(Args, Clone, Debug)]
pub struct RemapArgs {
    pub part: String,
    /// OLD=NEW key swaps (NEW: key or role name; OLD need not exist in the template any more)
    #[arg(required = true)]
    pub maps: Vec<KeyMap>,
    #[command(flatten)]
    pub sel: FrameSel,
    #[command(flatten)]
    pub flags: EditFlags,
}

#[derive(Subcommand, Debug)]
pub enum TemplateCmd {
    /// Rewrite keys in a part's stored frames after a template change (edit; undoable)
    Remap(RemapArgs),
    /// Propose templates from a part's inputs (never overwrites)
    Suggest(SuggestArgs),
    /// Change template colours (coordinator)
    Set(TemplateSetArgs),
    /// Add every colour of palette sets as keys of [layers.set]
    AddSet(AddSetArgs),
}

#[derive(Args, Clone, Debug)]
pub struct TilesArgs {
    pub tileset: String,
    /// priority: tile ids to mark as drawn over sprites (`3,7,10-12`)
    #[arg(long, value_name = "IDS")]
    pub set: Option<String>,
    /// priority: tile ids to unmark
    #[arg(long, value_name = "IDS")]
    pub clear: Option<String>,
    /// Frame to work on (default `blocks`, else the first)
    #[arg(long)]
    pub frame: Option<String>,
    /// fit: write the recolour ops as an `apply` JSON file
    #[arg(long, value_name = "FILE")]
    pub write: Option<String>,
    /// fit / dedupe: edit the part (one journaled edit, `undo` reverts it)
    #[arg(long)]
    pub apply: bool,
    /// dedupe: unique tiles to keep at most (default: the profile's unique_tiles_max)
    #[arg(long, value_name = "N")]
    pub max: Option<usize>,
    /// How many items to list
    #[arg(long, value_name = "N")]
    pub top: Option<usize>,
    /// Act on a part whose state is `approved` anyway (normally: `pxl rework PART --note ...` first)
    #[arg(long)]
    pub force_approved: bool,
}

#[derive(Subcommand, Debug)]
pub enum TilesCmd {
    /// Assign tiles to template palettes
    Fit(TilesArgs),
    /// Colours per tile, unique count, duplicates, palette packing
    Stats(TilesArgs),
    /// Merge near-duplicate tiles until at most N unique tiles remain
    Dedupe(TilesArgs),
    /// Foreground priority mask: --set/--clear IDS
    Priority(TilesArgs),
}

// ---------------------------------------------------------------- edit commands

#[derive(Args, Clone, Debug)]
pub struct SetArgs {
    pub part: String,
    pub frame: String,
    /// x,y=KEY pixels (KEY = key character or template role); flags may follow
    #[arg(required = true)]
    pub pixels: Vec<Pixel>,
    #[command(flatten)]
    pub flags: EditFlags,
}

/// `row PART FRAME X Y KEYS` or the named form `row PART FRAME x=16 y=13 KEYS`.
#[derive(Args, Clone, Debug)]
pub struct RowArgs {
    pub part: String,
    pub frame: String,
    /// `X Y KEYS` (x = first column, y = the row) or `x=X y=Y KEYS` in any order; KEYS are written left to right, space = unchanged
    #[arg(required = true, value_name = "X Y KEYS")]
    pub rest: Vec<String>,
    #[command(flatten)]
    pub flags: EditFlags,
}

impl RowArgs {
    /// (x, y, keys) from the positional or named form.
    pub fn parse(&self) -> crate::output::Result<(i32, i32, String)> {
        use crate::output::PxlError;
        let hint = "row PART FRAME X Y KEYS (X = first column, Y = the row) or row PART FRAME x=16 y=13 KEYS";
        let [a, b, keys] = match self.rest.as_slice() {
            [a, b, k] => [a, b, k],
            _ => return Err(PxlError::usage("row takes X Y KEYS").with_hint(hint)),
        };
        let num = |s: &str, what: &str| s.parse::<i32>().map_err(|_| PxlError::usage(format!("row: {} must be a whole number, not '{}'", what, s)).with_hint(hint));
        let named = |s: &str| s.split_once('=').map(|(k, v)| (k.to_string(), v.to_string()));
        let (x, y) = match (named(a), named(b)) {
            (Some((ka, va)), Some((kb, vb))) => match (ka.as_str(), kb.as_str()) {
                ("x", "y") => (num(&va, "x")?, num(&vb, "y")?),
                ("y", "x") => (num(&vb, "x")?, num(&va, "y")?),
                _ => return Err(PxlError::usage(format!("row: named form needs x=.. and y=.., got '{}' '{}'", a, b)).with_hint(hint)),
            },
            (None, None) => (num(a, "X")?, num(b, "Y")?),
            _ => return Err(PxlError::usage("row: use either `X Y` or `x=.. y=..`, not a mix").with_hint(hint)),
        };
        Ok((x, y, keys.clone()))
    }
}

/// `patch PART FRAME [X Y] GRID`: GRID is plain rows (then X Y are needed) or the text `pxl show` printed
/// (rulers and row labels are stripped, the labels place the rows, `--ref` columns are ignored). "-" reads stdin.
#[derive(Args, Clone, Debug)]
pub struct PatchArgs {
    pub part: String,
    pub frame: String,
    /// `GRID` or `X Y GRID`; rows split by newline, space = unchanged, "-" = stdin
    #[arg(required = true, num_args = 1..=3, value_name = "[X Y] GRID")]
    pub rest: Vec<String>,
    #[command(flatten)]
    pub flags: EditFlags,
}

#[derive(Args, Clone, Debug)]
pub struct StampArgs {
    pub part: String,
    pub frame: String,
    /// Snippet name (`pxl snippet list`)
    pub name: String,
    /// Top left x,y
    #[arg(allow_negative_numbers = true)]
    pub at: Point,
    /// Mirror the snippet: x (left/right) or y (top/bottom)
    #[arg(long, value_enum)]
    pub flip: Option<Axis>,
    #[command(flatten)]
    pub flags: EditFlags,
}

/// `lineup PART:FRAMES ...`
#[derive(Args, Clone, Debug)]
pub struct LineupArgs {
    /// `part:frame1,frame2` (or `part:all`, or just `part`)
    pub parts: Vec<String>,
    /// Backdrop #rrggbb behind transparent cells (default grey checker)
    #[arg(long)]
    pub on: Option<String>,
    /// Zoom factor of the large copy (default: automatic, 2..8)
    #[arg(long)]
    pub zoom: Option<usize>,
    /// Crop every item to x,y,w,h
    #[arg(long, allow_hyphen_values = true)]
    pub region: Option<Region>,
    /// PNG path (further pages get -2, -3, ...); default preview/lineup.png next to the first part
    #[arg(long)]
    pub out: Option<String>,
}

/// `pattern PART FRAME [SNIPPET]`: repeat a unit over a region with a fixed phase (textures).
#[derive(Args, Clone, Debug)]
pub struct PatternArgs {
    pub part: String,
    pub frame: String,
    /// Snippet holding the repeating unit (`pxl snippet list`); or use --unit
    pub snippet: Option<String>,
    /// Unit = this rectangle of the frame (x,y,w,h; tile-relative with --tile)
    #[arg(long, allow_hyphen_values = true)]
    pub unit: Option<Region>,
    /// Cells to fill, x,y,w,h (default: the whole frame, or the whole tile with --tile)
    #[arg(long, allow_hyphen_values = true)]
    pub region: Option<Region>,
    /// Phase px,py: the unit's top left repeats at (px + k*w, py + m*h). Default: the part origin for sprites, 0,0 for
    /// tile sets and screens, so neighbouring pieces, tiles and frames line up
    #[arg(long, allow_hyphen_values = true)]
    pub phase: Option<Point>,
    /// Only replace cells currently holding these keys (comma separated), so a shape keeps its outline
    #[arg(long)]
    pub mask: Option<String>,
    /// `pxl pattern suggest PART FRAME`: largest period (target pixels, per axis) to look for
    #[arg(long, value_name = "N")]
    pub max_period: Option<usize>,
    /// suggest: save the proposed unit as the snippet `pattern_NAME`
    #[arg(long, value_name = "NAME")]
    pub save: Option<String>,
    /// suggest, tile sets: analyse these tiles (ranges, e.g. 37-40,52); each tile is read as a repeating block
    #[arg(long, value_name = "RANGES")]
    pub tiles: Option<String>,
    #[command(flatten)]
    pub flags: EditFlags,
}

#[derive(Args, Clone, Debug)]
pub struct SnippetSaveArgs {
    pub part: String,
    pub frame: String,
    /// Snippet name (letters, digits, - and _)
    pub name: String,
    /// x,y,w,h to copy
    #[arg(long, allow_hyphen_values = true)]
    pub region: Region,
    /// Keep '.' as transparent (default: '.' is saved as a space = leave the target pixel unchanged)
    #[arg(long)]
    pub solid: bool,
}

#[derive(Args, Clone, Debug)]
pub struct SnippetListArgs {
    pub part: String,
}

#[derive(Subcommand, Debug)]
pub enum SkillCmd {
    /// List the bundled skills with their one-line descriptions and reference files
    List,
    /// Print a skill (SKILL.md) or one of its reference files
    Show(SkillShowArgs),
    /// Install the bundled skills for Claude Code and/or Codex (~/.claude/skills, ~/.codex/skills); replaces only folders it installed
    Install(SkillInstallArgs),
    /// Remove the skills this command installed; refuses folders it did not install
    Uninstall(SkillInstallArgs),
    /// Print the skill folders per host and whether they are installed and current
    Path(SkillPathArgs),
}

/// Agent host a skill is installed for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum SkillHost {
    Claude,
    Codex,
}

#[derive(Args, Clone, Debug)]
pub struct SkillShowArgs {
    /// Skill name, e.g. pxl-sprite
    pub name: String,
    /// A file under the skill's references/ (with or without .md)
    #[arg(long = "ref")]
    pub reference: Option<String>,
}

#[derive(Args, Clone, Debug)]
pub struct SkillInstallArgs {
    /// Host to use; repeat for several (default: claude and codex)
    #[arg(long, value_enum)]
    pub host: Vec<SkillHost>,
    /// Use the current folder (./.claude/skills, ./.codex/skills) instead of the home folder
    #[arg(long)]
    pub project: bool,
    /// Print what would change; write nothing
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Args, Clone, Debug)]
pub struct SkillPathArgs {
    /// Host to show; repeat for several (default: claude and codex)
    #[arg(long, value_enum)]
    pub host: Vec<SkillHost>,
    /// Use the current folder instead of the home folder
    #[arg(long)]
    pub project: bool,
}

#[derive(Subcommand, Debug)]
pub enum SnippetCmd {
    /// Store a region of a frame as a reusable snippet (per template family)
    Save(SnippetSaveArgs),
    /// List the snippets of a part's family
    List(SnippetListArgs),
}

#[derive(Args, Clone, Debug)]
pub struct MoveArgs {
    pub part: String,
    pub frame: String,
    /// x,y,w,h of the pixels to move
    #[arg(long, allow_hyphen_values = true)]
    pub region: Region,
    /// dx,dy (e.g. 1,0 or -1,2)
    #[arg(long, allow_hyphen_values = true)]
    pub by: Point,
    /// Key for the vacated cells (default '.'); key or role name
    #[arg(long)]
    pub fill: Option<String>,
    #[command(flatten)]
    pub flags: EditFlags,
}

#[derive(Args, Clone, Debug)]
pub struct NudgeArgs {
    pub part: String,
    /// Frame selector (name, walk*, 6-11, tag:walk; comma separated)
    pub frame: String,
    /// dx,dy (e.g. 0,-1)
    #[arg(allow_negative_numbers = true)]
    pub by: Point,
    #[command(flatten)]
    pub flags: EditFlags,
}

#[derive(Args, Clone, Debug)]
pub struct OutlineArgs {
    pub part: String,
    /// Frame selector (name, walk*, 6-11, tag:walk; comma separated)
    pub frame: String,
    /// Outline key or role (default: the template's `outline` role)
    #[arg(long)]
    pub key: Option<String>,
    #[command(flatten)]
    pub flags: EditFlags,
}

#[derive(Args, Clone, Debug)]
pub struct FixArgs {
    pub part: String,
    /// Frame selector (name, walk*, 6-11, tag:walk; comma separated)
    pub frame: String,
    /// orphan, jaggy, eye (default: all)
    #[arg(long, value_delimiter = ',')]
    pub rules: Vec<String>,
    #[command(flatten)]
    pub flags: EditFlags,
}

#[derive(Args, Clone, Debug)]
pub struct ViewArgs {
    pub part: String,
    pub frame: String,
    /// Show the reference next to the frame (the default when the part has one)
    #[arg(long = "ref")]
    pub reference: bool,
    /// Leave the reference out
    #[arg(long = "no-ref")]
    pub no_ref: bool,
    /// Display pixels per reference pixel (default: as large as fits in 1568 px)
    #[arg(long)]
    pub zoom: Option<usize>,
    /// x,y,w,h to show (default: the shape plus a margin)
    #[arg(long, allow_hyphen_values = true)]
    pub region: Option<Region>,
    /// Ring the eye-like features the detector finds in the reference (a hint layer, information only)
    #[arg(long)]
    pub features: bool,
    /// Show tile N of a tile set (reference block | tile) in tile coordinates 0..7
    #[arg(long, value_name = "N")]
    pub tile: Option<usize>,
    /// PNG path (default: preview/<part>-<frame>-view.png next to the part)
    #[arg(long)]
    pub out: Option<String>,
}

#[derive(Args, Clone, Debug)]
pub struct LineArgs {
    pub part: String,
    pub frame: String,
    #[arg(allow_negative_numbers = true)]
    pub from: Point,
    #[arg(allow_negative_numbers = true)]
    pub to: Point,
    pub key: String,
    #[command(flatten)]
    pub flags: EditFlags,
}

#[derive(Args, Clone, Debug)]
pub struct RectArgs {
    pub part: String,
    pub frame: String,
    /// x,y,w,h
    #[arg(allow_negative_numbers = true)]
    pub region: Region,
    pub key: String,
    /// Border only
    #[arg(long)]
    pub outline: bool,
    #[command(flatten)]
    pub flags: EditFlags,
}

#[derive(Args, Clone, Debug)]
pub struct FillArgs {
    pub part: String,
    pub frame: String,
    /// Seed point x,y
    #[arg(allow_negative_numbers = true)]
    pub at: Point,
    pub key: String,
    #[command(flatten)]
    pub flags: EditFlags,
}

#[derive(Args, Clone, Debug)]
pub struct ReplaceArgs {
    pub part: String,
    pub frame: String,
    /// A=B swaps (key or role names)
    #[arg(required = true)]
    pub maps: Vec<KeyMap>,
    /// x,y,w,h (default: the whole frame)
    #[arg(long, allow_hyphen_values = true)]
    pub region: Option<Region>,
    #[command(flatten)]
    pub flags: EditFlags,
}

#[derive(Args, Clone, Debug)]
pub struct CopyArgs {
    pub part: String,
    /// Source frame
    pub from: String,
    /// Target frame
    pub to: String,
    /// x,y,w,h in the source (default: the whole frame)
    #[arg(long, allow_hyphen_values = true)]
    pub region: Option<Region>,
    /// Paste at x,y instead of the same place
    #[arg(long, allow_hyphen_values = true)]
    pub to_at: Option<Point>,
    /// Do not copy transparent ('.') cells
    #[arg(long)]
    pub skip_transparent: bool,
    #[command(flatten)]
    pub flags: EditFlags,
}

#[derive(Args, Clone, Debug)]
pub struct MirrorArgs {
    pub part: String,
    pub frame: String,
    /// x: left/right symmetric; y: top/bottom symmetric
    #[arg(value_enum)]
    pub axis: Axis,
    /// Which half is kept: low (left/top, default) or high
    #[arg(long, default_value = "low")]
    pub keep: String,
    #[arg(long, allow_hyphen_values = true)]
    pub region: Option<Region>,
    #[command(flatten)]
    pub flags: EditFlags,
}

#[derive(Args, Clone, Debug)]
pub struct FlipArgs {
    pub part: String,
    pub frame: String,
    #[arg(value_enum)]
    pub axis: Axis,
    #[arg(long, allow_hyphen_values = true)]
    pub region: Option<Region>,
    #[command(flatten)]
    pub flags: EditFlags,
}

#[derive(Args, Clone, Debug)]
pub struct WriteGridArgs {
    pub part: String,
    pub frame: String,
    /// Grid rows; "-" reads stdin
    pub grid: String,
    #[command(flatten)]
    pub flags: EditFlags,
}

#[derive(Args, Clone, Debug)]
pub struct CandidatesArgs {
    pub part: String,
    /// Frame selector (names, wildcards, 6-11, tag:walk; comma separated)
    pub frames: String,
    /// Methods to show next to the reference and the current frame
    #[arg(long, value_delimiter = ',', default_value = "vote,contour,exemplar,auto,nearest")]
    pub methods: Vec<String>,
    /// Output folder (default: compare/ next to the part)
    #[arg(long)]
    pub out: Option<String>,
}

#[derive(Args, Clone, Debug)]
pub struct PickArgs {
    pub part: String,
    pub frame: String,
    /// vote | nearest | contour | exemplar | auto
    pub method: String,
    /// Only write cells inside x,y,w,h (default: the whole frame)
    #[arg(long, allow_hyphen_values = true)]
    pub region: Option<Region>,
    #[command(flatten)]
    pub flags: EditFlags,
}

/// `finish`: mark frames (or tiles) hand-finished without changing a pixel.
#[derive(Args, Clone, Debug)]
pub struct FinishArgs {
    pub part: String,
    /// Frame selector (name, walk*, 6-11, tag:walk; comma separated)
    pub frames: String,
    /// Why the automatic output is already right (the reviewer sees this and may reject it with `pxl rework`)
    #[arg(long)]
    pub reason: String,
    /// Tile part: only these tiles (N or A-B) of the selected frames
    #[arg(long, value_name = "A-B")]
    pub tiles: Option<String>,
    /// Edit a part whose state is `approved` anyway
    #[arg(long)]
    pub force_approved: bool,
}

/// `lock` / `unlock`: set or clear the lock of frames without touching a pixel.
#[derive(Args, Clone, Debug)]
pub struct LockArgs {
    pub part: String,
    /// Frame selector (name, walk*, 6-11, tag:walk; comma separated)
    pub frames: Option<String>,
    /// Every frame of the part
    #[arg(long)]
    pub all: bool,
    /// Show what would change without writing the part
    #[arg(long)]
    pub dry_run: bool,
    /// Act on a part whose state is `approved` anyway (normally: `pxl rework PART --note ...` first)
    #[arg(long)]
    pub force_approved: bool,
}

#[derive(Args, Clone, Debug)]
#[command(after_help = crate::edit::APPLY_HELP)]
pub struct ApplyArgs {
    pub part: String,
    /// JSON ops file; "-" reads stdin
    pub ops: String,
    #[command(flatten)]
    pub flags: EditFlags,
}

#[derive(Args, Clone, Debug)]
pub struct UndoArgs {
    pub part: String,
    #[arg(long, default_value_t = 1)]
    pub steps: usize,
    /// Act on a part whose state is `approved` anyway (normally: `pxl rework PART --note ...` first)
    #[arg(long)]
    pub force_approved: bool,
}

#[derive(Args, Clone, Debug)]
pub struct CleanArgs {
    pub part: String,
    #[command(flatten)]
    pub sel: FrameSel,
    /// Stray single pixels
    #[arg(long)]
    pub orphans: bool,
    /// Doubled diagonals and L corners
    #[arg(long)]
    pub jaggies: bool,
    #[command(flatten)]
    pub flags: EditFlags,
}

#[derive(Args, Clone, Debug, Default)]
pub struct ShowArgs {
    pub part: String,
    pub frame: String,
    #[arg(long, allow_hyphen_values = true)]
    pub region: Option<Region>,
    /// Add the input at native resolution (rows 12a/12b)
    #[arg(long = "ref")]
    pub reference: bool,
    /// Row labels only, no rulers (still pasteable into `patch`)
    #[arg(long)]
    pub raw: bool,
    /// Show tile N of a tile set in tile coordinates 0..7 (with --ref: its 16x16 reference rows)
    #[arg(long, value_name = "N")]
    pub tile: Option<usize>,
    /// Palette-set parts: add the cell layout, the slot of every cell and a pixel map of the slots
    #[arg(long)]
    pub cells: bool,
}

// ---------------------------------------------------------------- compare / lint / export

#[derive(Args, Clone, Debug)]
pub struct CompareArgs {
    /// Parts, globs or directories
    #[arg(required = true)]
    pub parts: Vec<String>,
    #[command(flatten)]
    pub sel: FrameSel,
    /// Background (colour #rrggbb or image path) to judge readability at 1x
    #[arg(long)]
    pub on: Option<String>,
    /// One frame large; optional zoom factor (0 = automatic)
    #[arg(long, num_args = 0..=1, default_missing_value = "0")]
    pub zoom: Option<usize>,
    /// Output folder (default: next to the part)
    #[arg(long)]
    pub out: Option<String>,
    /// Palette-set parts with several sets: render in this set's colours (default: the first)
    #[arg(long, value_name = "NAME")]
    pub set: Option<String>,
}

#[derive(Args, Clone, Debug)]
pub struct DiffArgs {
    pub a: String,
    pub b: String,
    #[command(flatten)]
    pub sel: FrameSel,
}

#[derive(Subcommand, Clone, Debug)]
pub enum PaletteCmd {
    /// Remap a part into its palette set per cell (dry run unless --apply)
    Fit(FitArgs),
    /// Change a slot of a palette set: pxl palette slot set SET SLOT #rrggbb #rrggbb ...
    #[command(subcommand)]
    Slot(SlotCmd),
    /// One palette set for all inputs of a scene, with the loss per part, reservations and what-if (dry run unless --apply)
    Plan(PalettePlanArgs),
}

#[derive(Args, Clone, Debug)]
pub struct PalettePlanArgs {
    /// Scene name from pxl.toml `[scene.NAME]`
    pub scene: String,
    /// Plan with this many slots (default: every OBJ palette not reserved)
    #[arg(long)]
    pub slots: Option<usize>,
    /// Slots kept out of the plan (they keep their colours from the scene's current set), also `[scene.X.reserve] palette_slots`
    #[arg(long, value_delimiter = ',')]
    pub reserve: Vec<u8>,
    /// What-if: plan without these parts
    #[arg(long, value_delimiter = ',')]
    pub without: Vec<String>,
    /// What-if: plan with these further parts (names, folders or globs)
    #[arg(long, value_delimiter = ',')]
    pub parts: Vec<String>,
    /// Line the slots up with the slots of this palette set (shared materials keep one slot number across scenes)
    #[arg(long)]
    pub align: Option<String>,
    /// Name of the set to write (default: the scene name)
    #[arg(long)]
    pub name: Option<String>,
    /// Try an option: write nothing
    #[arg(long = "what-if")]
    pub what_if: bool,
    /// Write palettes/<set>.toml, link it to the scene and reduce the parts into it
    #[arg(long)]
    pub apply: bool,
    #[arg(long)]
    pub manifest: Option<String>,
}

#[derive(Args, Clone, Debug)]
pub struct PlanArgs {
    /// `accept` signs the plan off; `show` is the default report
    pub action: Option<String>,
    /// Scene names to propose when the project has none (default: main)
    #[arg(long, value_delimiter = ',')]
    pub scenes: Vec<String>,
    /// Write the proposed scenes into pxl.toml, build the shared palette sets, reduce the assets into them and write plan.toml
    #[arg(long)]
    pub apply: bool,
    /// With accept: who signs off (default `agent`; `--by user` records the user's own sign-off)
    #[arg(long)]
    pub by: Option<String>,
    /// With accept: accept scenes that do not fit, recording what was decided and why
    #[arg(long)]
    pub decision: Option<String>,
    #[arg(long)]
    pub manifest: Option<String>,
}

#[derive(Subcommand, Clone, Debug)]
pub enum SlotCmd {
    /// Replace the colours of one slot (with a lease on the palette file)
    Set {
        set: String,
        slot: u8,
        colors: Vec<String>,
    },
    /// Remove a slot from a palette set (the cells that used it must be fitted again: `pxl palette fit PART --apply`)
    Remove { set: String, slot: u8 },
}

#[derive(Args, Clone, Debug)]
pub struct FitArgs {
    pub part: String,
    /// Palette set (default: the part's `palette_set`; written to the header with --apply)
    #[arg(long)]
    pub set: Option<String>,
    /// Write the result (journaled, `pxl undo` reverts the pixels)
    #[arg(long)]
    pub apply: bool,
    #[command(flatten)]
    pub sel: FrameSel,
    /// Write a compare page (before | after | changed pixels) to this PNG
    #[arg(long)]
    pub png: Option<String>,
    #[arg(long)]
    pub zoom: Option<usize>,
    /// Fit locked frames too
    #[arg(long)]
    pub include_locked: bool,
    /// Only add the template keys the fit picks (default: every colour of the part's palette sets, so hand work has them all)
    #[arg(long)]
    pub no_all_keys: bool,
    /// Edit a part whose state is `approved` anyway
    #[arg(long)]
    pub force_approved: bool,
}

#[derive(Args, Clone, Debug)]
pub struct SceneArgs {
    /// Scene name from pxl.toml `[scene.NAME]`; or `set SCENE KEY VALUE` | `add-part SCENE GROUP PART` | `remove-part SCENE PART`
    pub name: String,
    /// Words of `set` / `add-part` / `remove-part`
    pub rest: Vec<String>,
    /// Compose the worst-case screen: player + the two widest objects + an enemy + a ring on the same lines
    #[arg(long)]
    pub worst: bool,
    /// Compose the real screens of the scene's object layout (`layout = \"file\"`): sprites per scanline and per frame along each act, and the zone completeness of the objects placed
    #[arg(long)]
    pub layout: bool,
    /// With --layout: one act only
    #[arg(long)]
    pub act: Option<String>,
    /// With --worst: write the composition with the per-line load
    #[arg(long)]
    pub png: Option<String>,
    #[arg(long)]
    pub zoom: Option<usize>,
    /// Player part (default: the scene's `player` group or a category player part)
    #[arg(long)]
    pub player: Option<String>,
    /// Object parts, comma separated (default: the two widest frames of the `objects` group)
    #[arg(long, value_delimiter = ',')]
    pub objects: Option<Vec<String>>,
    #[arg(long)]
    pub enemy: Option<String>,
    #[arg(long)]
    pub ring: Option<String>,
    #[arg(long)]
    pub manifest: Option<String>,
}

#[derive(Args, Clone, Debug)]
pub struct DeliveryArgs {
    /// DELIVERY_TEMPLATE.md of the package
    #[arg(long)]
    pub template: String,
    /// Markdown table of agreed exceptions/changes (Folder | Change | Reason, or ID | File | Exception | Agreed by)
    #[arg(long)]
    pub exceptions: Option<String>,
    /// Output file (default ./DELIVERY.md)
    #[arg(long)]
    pub out: Option<String>,
    /// Text file with the output of the client's check_assets.py (last lines are pasted)
    #[arg(long)]
    pub checker: Option<String>,
    /// Package folder: runs `python3 <package>/check_assets.py <package>` for the checker output
    #[arg(long)]
    pub package: Option<String>,
    #[arg(long)]
    pub studio: Option<String>,
    #[arg(long)]
    pub number: Option<String>,
    #[arg(long)]
    pub date: Option<String>,
    #[arg(long)]
    pub contact: Option<String>,
    #[arg(long)]
    pub manifest: Option<String>,
}

#[derive(Args, Clone, Debug)]
pub struct BudgetArgs {
    /// Scene name from pxl.toml `[scene.NAME]` (omit with --all)
    pub scene: Option<String>,
    /// Every scene
    #[arg(long)]
    pub all: bool,
    /// Biggest contributors listed per group
    #[arg(long, default_value_t = 5)]
    pub top: usize,
    /// Show the rows before and after a change, writing nothing: "remove PART", "add PART GROUP", "budget GROUP N", "reserve GROUP|sprite_cells|bg_tiles N", "resident N" (repeat)
    #[arg(long = "what-if")]
    pub what_if: Vec<String>,
    #[arg(long)]
    pub manifest: Option<String>,
}

#[derive(Clone, Debug)]
pub struct DedupeArgs {
    pub part: String,
    pub max: usize,
    pub apply: bool,
    pub force_approved: bool,
}

#[derive(Clone, Debug)]
pub struct CellsShowArgs {
    pub part: String,
    pub frame: Option<String>,
    pub png: Option<String>,
    pub zoom: Option<usize>,
    pub set: Option<String>,
}

/// `pxl cells PART [FRAME]` shows the layout; `pxl cells dedupe PART --max N [--apply]` merges near-identical cells.
#[derive(Args, Clone, Debug)]
pub struct CellsArgs {
    /// PART, or the word `dedupe` followed by PART
    pub part: String,
    /// FRAME (layout), or PART when the first word is `dedupe`
    pub rest: Vec<String>,
    /// Write the frame with the cell rectangles (coloured by slot, invalid cells red) to this PNG
    #[arg(long)]
    pub png: Option<String>,
    /// Display pixels per canvas pixel for --png (default: as large as fits)
    #[arg(long)]
    pub zoom: Option<usize>,
    /// Palette set to draw the PNG in (multi-set parts; default: the first set)
    #[arg(long, value_name = "NAME")]
    pub set: Option<String>,
    /// dedupe: target number of unique cells
    #[arg(long)]
    pub max: Option<usize>,
    /// dedupe: write the merges (journaled; authored cells.json files freeze the layout)
    #[arg(long)]
    pub apply: bool,
    /// dedupe: edit a part whose state is `approved` anyway
    #[arg(long)]
    pub force_approved: bool,
}

#[derive(Args, Clone, Debug)]
pub struct LintArgs {
    #[arg(required = true)]
    pub parts: Vec<String>,
    /// Profile name or path (default: the part's profile)
    #[arg(long)]
    pub profile: Option<String>,
    #[command(flatten)]
    pub sel: FrameSel,
    /// Only these rules, comma separated
    #[arg(long, value_delimiter = ',')]
    pub rules: Vec<String>,
    /// Also print notes: observations about looks (stray pixels, jaggies, drift against the reference). Information only, never a failure
    #[arg(long)]
    pub advice: bool,
}

#[derive(Args, Clone, Debug)]
pub struct ExportArgs {
    /// Parts, globs or directories
    pub parts: Vec<String>,
    /// Every part in pxl.toml
    #[arg(long)]
    pub all: bool,
    /// Output folder (default: from pxl.toml, else next to the part)
    #[arg(long)]
    pub out: Option<String>,
    #[arg(long, value_delimiter = ',')]
    pub frames: Vec<String>,
    #[arg(long)]
    pub manifest: Option<String>,
}

#[derive(Args, Clone, Debug)]
pub struct ExportPalettesArgs {
    #[arg(long)]
    pub manifest: Option<String>,
}

#[derive(Args, Clone, Debug)]
pub struct BuildArgs {
    /// Restrict to these parts (default: all in the manifest)
    pub parts: Vec<String>,
    #[arg(long)]
    pub manifest: Option<String>,
    /// Ignore hashes, rebuild everything
    #[arg(long)]
    pub force: bool,
}

#[derive(Args, Clone, Debug)]
pub struct CheckArgs {
    pub parts: Vec<String>,
    #[arg(long)]
    pub manifest: Option<String>,
    /// Ignore hashes, check everything
    #[arg(long)]
    pub force: bool,
    /// Also fail while any frame or tile is still the automatic output (see `pxl status --raw`)
    #[arg(long)]
    pub hand_finished: bool,
    /// With --hand-finished: first rebuild the raw baseline of the checked parts (see `pxl status --raw --recompute`)
    #[arg(long)]
    pub recompute: bool,
    /// With --recompute: pixel tolerance (default: 0.5% of the frame's opaque pixels, rounded; tiny sprites must match exactly)
    #[arg(long, value_name = "N")]
    pub tolerance: Option<usize>,
}

#[derive(Args, Clone, Debug)]
pub struct ImportArgs {
    /// .png, grid .txt or .aseprite (or the asset when a second argument follows)
    pub source: String,
    /// `pxl import ASSET FILE`: the file
    pub file: Option<String>,
    /// Target part
    #[arg(long)]
    pub part: Option<String>,
    /// Target frame (default: from the file name)
    #[arg(long)]
    pub frame: Option<String>,
    /// Overlay PNG for the same frame
    #[arg(long)]
    pub overlay: Option<String>,
    /// Template to map colours (default: the part's)
    #[arg(long)]
    pub template: Option<String>,
    /// Act on a part whose state is `approved` anyway (normally: `pxl rework PART --note ...` first)
    #[arg(long)]
    pub force_approved: bool,
}

// ---------------------------------------------------------------- queue

#[derive(Args, Clone, Debug)]
pub struct PartArgs {
    pub part: String,
}

#[derive(Args, Clone, Debug, Default)]
pub struct NextArgs {
    #[arg(long, value_enum)]
    pub kind: Option<KindArg>,
    #[arg(long)]
    pub tag: Option<String>,
    /// Folder or glob to search for parts (default: current folder)
    #[arg(long)]
    pub dir: Option<String>,
}

#[derive(Args, Clone, Debug)]
pub struct ReleaseArgs {
    pub part: String,
    #[arg(long, value_enum)]
    pub state: Option<ReleaseState>,
    #[arg(long)]
    pub note: Option<String>,
}

#[derive(Args, Clone, Debug, Default)]
pub struct StatusArgs {
    /// Parts, globs or directories (default: current folder)
    pub parts: Vec<String>,
    #[arg(long)]
    pub stale: bool,
    #[arg(long)]
    pub blocked: bool,
    /// Only parts in this state
    #[arg(long)]
    pub state: Option<String>,
    /// Only assets of this group (a folder below assets/)
    #[arg(long)]
    pub group: Option<String>,
    /// Only parts with this manifest tag (or folder name)
    #[arg(long)]
    pub tag: Option<String>,
    /// List, per part, the frames and tiles that still equal the automatic output (not hand-finished)
    #[arg(long)]
    pub raw: bool,
    /// With --raw: rebuild the raw baseline by re-running every automatic method (for parts loaded before the audit existed); writes status.json only
    #[arg(long)]
    pub recompute: bool,
    /// With --recompute: pixels a frame (tile) may differ from the closest source-driven method output and still count as raw (default: 0.5% of its opaque pixels, rounded)
    #[arg(long, value_name = "N")]
    pub tolerance: Option<usize>,
    /// List parts and frames with open rework notes (a note closes when its frame is edited afterwards)
    #[arg(long)]
    pub rework: bool,
    /// Review statistics: parts approved first time, reworked once, twice or more, and the top rework reasons
    #[arg(long)]
    pub metrics: bool,
}

#[derive(Args, Clone, Debug, Default)]
pub struct ReviewArgs {
    pub parts: Vec<String>,
    /// Only parts in this state (e.g. done)
    #[arg(long)]
    pub state: Option<String>,
    #[arg(long)]
    pub out: Option<String>,
    /// Worst frames listed per part in the summary
    #[arg(long, default_value_t = 6)]
    pub worst: usize,
    /// Only parts of this `family`
    #[arg(long)]
    pub family: Option<String>,
    /// `review note`: the region x,y,w,h the note is about (required)
    #[arg(long, value_name = "x,y,w,h")]
    pub region: Option<Region>,
    /// `review note`: frame zoom of the crop image
    #[arg(long)]
    pub zoom: Option<usize>,
    /// `review note` / `review rework`: the text
    #[arg(long)]
    pub note: Option<String>,
    /// `review note`: the style.md rule id the note enforces (eyes, outline, ramps, ...)
    #[arg(long)]
    pub rule: Option<String>,
    /// `review approve`: accept the design-gate warnings (eye_parity, identity_colour, flat, consistency, readability), with the reason
    #[arg(long)]
    pub justify: Option<String>,
}

#[derive(Args, Clone, Debug)]
pub struct ApproveArgs {
    pub part: String,
    /// Remark kept with the verdict (style reference frames, accepted flaws)
    #[arg(long)]
    pub note: Option<String>,
    /// Accept the design-gate warnings of a gated project, with the reason
    #[arg(long)]
    pub justify: Option<String>,
}

#[derive(Args, Clone, Debug)]
pub struct ReworkArgs {
    pub part: String,
    /// Frames the note is about (names, walk*, 6-11, tag:walk; comma separated). Default: the whole part
    #[arg(long, value_delimiter = ',')]
    pub frames: Vec<String>,
    #[arg(long)]
    pub note: String,
}

/// A decimal or `0x` hexadecimal number.
pub fn parse_num(s: &str) -> std::result::Result<usize, String> {
    parse_usize(s)
}

fn parse_usize(s: &str) -> std::result::Result<usize, String> {
    let t = s.trim();
    match t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        Some(h) => usize::from_str_radix(h, 16).map_err(|e| e.to_string()),
        None => t.parse::<usize>().map_err(|e| e.to_string()),
    }
}

// ---------------------------------------------------------------- R4: images, projects, assets

#[derive(Args, Clone, Debug)]
pub struct ImageArgs {
    /// PNG to manage
    pub file: String,
    /// Console (default gbc)
    #[arg(long)]
    pub console: Option<String>,
    /// sprite | tile | screen
    #[arg(long, default_value = "sprite")]
    pub mode: String,
    /// Canvas size WxH of the reduced art (default: the picture divided by --factor)
    #[arg(long, value_name = "WxH")]
    pub size: Option<Size>,
    /// Integer factor of the input (default: chosen so the art is at most 64 px)
    #[arg(long)]
    pub factor: Option<u32>,
}

#[derive(Args, Clone, Debug)]
pub struct NewArgs {
    pub dir: String,
    /// Do not set `[project] gates = true` (milestone gates and the design-language lint rules)
    #[arg(long)]
    pub no_gates: bool,
    #[arg(long, default_value = "gbc")]
    pub console: String,
    /// Project name (default: the folder name)
    #[arg(long)]
    pub name: Option<String>,
}

#[derive(Args, Clone, Debug, Default)]
pub struct IngestArgs {
    /// Folders, files or globs (default: inbox/)
    pub inputs: Vec<String>,
    /// Group (sub-folder of assets/)
    #[arg(long)]
    pub group: Option<String>,
    /// sprite | tile | screen (default: the project default)
    #[arg(long)]
    pub mode: Option<String>,
    /// Split a sheet into frames of WxH
    #[arg(long, value_name = "WxH")]
    pub grid: Option<Size>,
    /// `auto`: split a sheet into connected regions
    #[arg(long)]
    pub frames: Option<String>,
    /// file | folder: where the asset name comes from
    #[arg(long, default_value = "file")]
    pub name_from: String,
    /// One asset per picture: do not group numbered files (walker_01.png, walk2.png ...) into one asset of frames
    #[arg(long)]
    pub separate: bool,
    #[arg(long, value_name = "WxH")]
    pub size: Option<Size>,
    #[arg(long)]
    pub factor: Option<u32>,
}

#[derive(Subcommand, Debug)]
pub enum AssetCmd {
    /// Set a header key of an asset: `category` (player, enemy, item, hazard, effect, platform, scenery, boss, screen). `category player` allows two-slot (stacked) cells
    Set { asset: String, key: String, value: String },
}

#[derive(Subcommand, Debug)]
pub enum GroupCmd {
    /// Groups and their assets
    List,
    /// Create a group folder
    Add { name: String },
    /// Move an asset folder into a group and update pxl.toml
    Move { asset: String, group: String },
}

#[derive(Subcommand, Debug)]
pub enum ProjectCmd {
    /// Merged project settings: name, console, defaults, custom settings
    Show,
    /// Set a project setting in pxl.toml: `pxl project set defaults.category enemy`, `defaults.group_categories.enemies enemy`, `defaults.size 32x32`,
    /// `settings.KEY VALUE`, `style.white balanced`, `delivery.client_checker CMD`, `gates false`
    Set {
        /// defaults.KEY | defaults.group_categories.GROUP | settings.KEY | style.KEY | delivery.KEY | name | strict | gates | manifest | plan
        path: String,
        value: String,
    },
}

#[derive(Args, Clone, Debug)]
pub struct VersionsArgs {
    pub asset: String,
}

#[derive(Args, Clone, Debug)]
pub struct RevertArgs {
    pub asset: String,
    /// v003
    #[arg(value_name = "VERSION")]
    pub ver: String,
    #[arg(long)]
    pub force_approved: bool,
}

#[derive(Subcommand, Debug)]
pub enum TaskCmd {
    /// pxl task add ASSET "text" [--kind draw|fix|review|question] [--assign NAME] [--priority N]
    Add {
        asset: String,
        text: String,
        #[arg(long, default_value = "draw")]
        kind: String,
        #[arg(long)]
        assign: Option<String>,
        #[arg(long, default_value_t = 0)]
        priority: i64,
        /// The task stays blocked until this decision (d0001) is decided
        #[arg(long)]
        after: Option<String>,
    },
    /// pxl task done ID [--note ...]
    Done {
        id: String,
        #[arg(long)]
        note: Option<String>,
    },
}

#[derive(Args, Clone, Debug, Default)]
pub struct TasksArgs {
    /// Only open tasks (the default)
    #[arg(long)]
    pub open: bool,
    /// Done tasks too
    #[arg(long)]
    pub all: bool,
    /// Tasks assigned to --agent
    #[arg(long)]
    pub mine: bool,
    #[arg(long)]
    pub assign: Option<String>,
    #[arg(long)]
    pub group: Option<String>,
    /// Only this asset (also for a single-image asset outside a project)
    #[arg(long)]
    pub asset: Option<String>,
}

#[derive(Args, Clone, Debug, Default)]
pub struct BoardArgs {
    /// Write a static HTML page
    #[arg(long, value_name = "FILE")]
    pub html: Option<String>,
}

#[derive(Args, Clone, Debug)]
pub struct DecideArgs {
    /// The decision text, or a decision id (d0001) when an option follows
    pub text: String,
    /// With an id: the option that closes the decision
    pub option: Option<String>,
    /// Change a brief item: key=value (console, fidelity, cadence, tradeoff, exceptions, deadline); marks it confirmed
    #[arg(long, value_name = "KEY=VALUE")]
    pub set: Vec<String>,
    #[arg(long)]
    pub asset: Option<String>,
    #[arg(long, default_value = "agent")]
    pub by: String,
}

#[derive(Subcommand, Debug)]
pub enum ExceptionCmd {
    /// pxl exception add ASSET "change" --reason "why"
    Add {
        asset: String,
        change: String,
        #[arg(long)]
        reason: String,
    },
    /// Exceptions of an asset (all assets without one)
    List { asset: Option<String> },
}

#[derive(Subcommand, Debug)]
pub enum VariantCmd {
    /// Copy the asset into a variant: pxl variant add ASSET NAME
    Add { asset: String, name: String },
    List { asset: String },
    /// Make a variant the delivered one (the others export under export/variants/)
    Deliver { asset: String, name: String },
}

#[derive(Args, Clone, Debug)]
pub struct BriefArgs {
    /// Asset name/path, or a group
    pub target: String,
}

#[derive(Args, Clone, Debug)]
pub struct MetaArgs {
    /// Asset (folder, asset.pxl, part path or name)
    pub asset: String,
    /// show | set KEY VALUE.. | compose add PART DX DY | compose clear | frame NAME KEY VALUE | zone-remap SCENE 4=5
    #[arg(allow_negative_numbers = true)]
    pub words: Vec<String>,
    /// With `frame NAME semantics flip_x`: the frame the engine flips
    #[arg(long)]
    pub of: Option<String>,
    /// With `frame NAME dropped true`: why
    #[arg(long)]
    pub reason: Option<String>,
}

#[derive(Args, Clone, Debug)]
pub struct SheetArgs {
    /// Group (folder under assets/) or asset name
    pub target: String,
    /// Palette set to draw in (multi-set parts; default: the part's first)
    #[arg(long)]
    pub set: Option<String>,
    /// Background colour (#rrggbb)
    #[arg(long, default_value = "#303040")]
    pub on: String,
    /// Output PNG (default: <group>_sheet.png)
    #[arg(long)]
    pub out: Option<String>,
    #[arg(long, default_value_t = 3)]
    pub zoom: usize,
    #[arg(long)]
    pub manifest: Option<String>,
}

#[derive(Args, Clone, Debug)]
pub struct PackageArgs {
    /// Package folder (default: [delivery] package, else `package`)
    #[arg(long)]
    pub out: Option<String>,
    #[arg(long)]
    pub manifest: Option<String>,
}

#[derive(Args, Clone, Debug)]
pub struct VerifyPackageArgs {
    /// Package folder (default: [delivery] package, else `package`)
    pub package: Option<String>,
    #[arg(long)]
    pub manifest: Option<String>,
}

#[derive(Args, Clone, Debug)]
pub struct DeliverArgs {
    /// Dry run on the style frames (our check, build, package and verifier in a scratch folder, nothing snapshotted); a pass is the milestone `dry run passed`
    #[arg(long)]
    pub dry_run: bool,
    /// The client's checker, run in the package folder (its output is captured; a failing exit is reported)
    #[arg(long)]
    pub client_checker: Option<String>,
    /// The client's converter / emulator pipeline, run in the package folder; images it writes to $PXL_REPORT_DIR are kept in the report
    #[arg(long)]
    pub client_pipeline: Option<String>,
    /// Package folder (default: [delivery] package, else `package`)
    #[arg(long)]
    pub package: Option<String>,
    /// Snapshot even when the client's checker fails (our own failures always refuse)
    #[arg(long)]
    pub allow_client_fail: bool,
    #[arg(long)]
    pub manifest: Option<String>,
}

#[derive(Args, Clone, Debug)]
pub struct ChangesArgs {
    /// Compare against this delivery number
    #[arg(long)]
    pub since: u32,
    /// ... up to this delivery (default: the current manifest)
    #[arg(long)]
    pub to: Option<u32>,
    #[arg(long)]
    pub manifest: Option<String>,
}

#[derive(Args, Clone, Debug)]
pub struct QaArgs {
    /// Asset to check (default: every asset of the project)
    pub asset: Option<String>,
    /// Do not create question tasks
    #[arg(long)]
    pub dry_run: bool,
    #[arg(long)]
    pub manifest: Option<String>,
}

#[derive(Args, Clone, Debug, Default)]
pub struct IntakeArgs {
    /// The request text, or a file that holds it (default: request.md in the project)
    #[arg(long)]
    pub request: Option<String>,
    /// Ask every item on stdin (opt-in; the default is non-interactive)
    #[arg(long)]
    pub interview: bool,
    /// A TOML file of answers (console, fidelity, cadence, tradeoff, exceptions, deadline), applied as the user's decisions
    #[arg(long, value_name = "FILE")]
    pub answers: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum StyleCmd {
    /// Measure the input style per asset and over the project (outline, tones per material, light, highlights, dithering, palette, eyes)
    Analyze {
        /// Assets (default: all)
        assets: Vec<String>,
    },
    /// Write style.md from the analysis, the console `[style]` defaults and limits
    Propose {
        /// Replace an approved style.md (the approval is cleared)
        #[arg(long)]
        force: bool,
    },
    /// Approve style.md (a milestone together with the approved style frames)
    Approve {
        /// Who approves (default `agent`; `--by user` records the user's own approval)
        #[arg(long)]
        by: Option<String>,
    },
    /// Mark an asset (or some of its frames) as a style frame, the reference every producer compares against
    Mark {
        asset: String,
        frames: Vec<String>,
        #[arg(long)]
        remove: bool,
    },
    /// The identity list of an asset (the colours the gates check): show | add #rrggbb.. | remove #rrggbb.. | set #rrggbb.. | eyes N
    Identity {
        asset: String,
        /// show (default) | add | remove | set | eyes
        op: Option<String>,
        values: Vec<String>,
    },
    /// style.md state, the style frames and the rules
    Show,
}

#[derive(Subcommand, Debug)]
pub enum MilestoneCmd {
    /// plan accepted / style approved / dry run passed, and whether production is open
    Status,
}

#[derive(Args, Clone, Debug)]
pub struct AskArgs {
    /// What is being decided
    pub topic: String,
    /// KEY=image, KEY=image|label or KEY=label (repeat; at least two)
    #[arg(long, value_name = "KEY=IMG")]
    pub option: Vec<String>,
    /// OPTION "why": the recommendation and its reason (required)
    #[arg(long, num_args = 1..=2, value_names = ["OPTION", "WHY"])]
    pub recommend: Vec<String>,
    /// The asset the decision is about
    #[arg(long)]
    pub asset: Option<String>,
}

#[derive(Args, Clone, Debug, Default)]
pub struct DecisionsArgs {
    /// Closed decisions too
    #[arg(long)]
    pub all: bool,
}

#[derive(Args, Clone, Debug, Default)]
pub struct ReportArgs {
    /// Write the digest as a static HTML page (default file review/report.html)
    #[arg(long, num_args = 0..=1, value_name = "FILE", default_missing_value = "review/report.html")]
    pub html: Option<String>,
    /// Only when the brief's cadence says a report is due (new work since the last one, or the daily / weekly interval)
    #[arg(long)]
    pub if_due: bool,
    /// Do not mark this report as the last one
    #[arg(long)]
    pub peek: bool,
}
