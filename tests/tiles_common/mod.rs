//! Helpers of the `tiles_*` tests: fixtures, PNG export of a tile frame and the real `check_assets.py`.
#![allow(dead_code)]

use agentsprite::color::Rgb;
use agentsprite::model::Part;
use agentsprite::tiles::{self, Colors};
use agentsprite::{png, template};
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tiles")
}

pub fn out_dir(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/m4").join(name);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Copy a fixture part and the template next to it into `dir` (so edits do not touch the repository).
pub fn copy_fixture(rel: &str, dir: &Path) -> PathBuf {
    std::fs::create_dir_all(dir.join("templates")).unwrap();
    std::fs::copy(fixtures().join("templates/tiles.toml"), dir.join("templates/tiles.toml")).unwrap();
    // the fixtures are written for the old tile rules
    std::fs::create_dir_all(dir.join("consoles/gbc")).unwrap();
    std::fs::copy(fixtures().join("consoles/gbc/tile.toml"), dir.join("consoles/gbc/tile.toml")).unwrap();
    let dst = dir.join(Path::new(rel).file_name().unwrap());
    std::fs::copy(fixtures().join(rel), &dst).unwrap();
    dst
}

/// Write frame `fi` of a tile part as an indexed PNG (no transparency), the way the deliverable is exported.
pub fn export_png(part: &Part, fi: usize, path: &Path) {
    let tpl = template::load_for(part).unwrap();
    let cols = Colors::new(part, &tpl);
    let g = &part.frames[fi].grid;
    let mut pal: Vec<Rgb> = Vec::new();
    let mut idx = Vec::with_capacity(g.cells.len());
    for &k in &g.cells {
        let c = tiles::rgb_of(cols.code[k as usize]);
        let i = match pal.iter().position(|p| *p == c) {
            Some(i) => i,
            None => {
                pal.push(c);
                pal.len() - 1
            }
        };
        idx.push(i as u8);
    }
    png::write_indexed(path, g.w, g.h, &idx, &pal, false).unwrap();
}

pub fn checker_available() -> Option<PathBuf> {
    // optional: an external acceptance checker script (`$PXL_CHECK_ASSETS`); the tests that use it skip without it
    let script = PathBuf::from(std::env::var("PXL_CHECK_ASSETS").unwrap_or_default());
    if !script.is_file() {
        eprintln!("SKIP: $PXL_CHECK_ASSETS (an external check_assets.py) not set");
        return None;
    }
    let ok = Command::new("python3").args(["-c", "import PIL"]).output().map(|o| o.status.success()).unwrap_or(false);
    if !ok {
        eprintln!("SKIP: python3 with Pillow not available");
        return None;
    }
    Some(script)
}

/// Run the real checker on one `blocks.png`; returns (passed, output).
pub fn run_checker(script: &Path, png_file: &Path) -> (bool, String) {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let d = std::env::temp_dir().join(format!("pxl_chk_{}_{}", std::process::id(), n));
    let _ = std::fs::remove_dir_all(&d);
    let z = d.join("zones/zone_9");
    std::fs::create_dir_all(&z).unwrap();
    std::fs::copy(script, d.join("check_assets.py")).unwrap();
    std::fs::write(z.join("spec.json"), r#"{"type": "tiles"}"#).unwrap();
    std::fs::copy(png_file, z.join("blocks.png")).unwrap();
    let o = Command::new("python3").args(["-I", "check_assets.py"]).current_dir(&d).output().unwrap();
    let text = String::from_utf8_lossy(&o.stdout).into_owned();
    let _ = std::fs::remove_dir_all(&d);
    (o.status.success(), text)
}

/// Checker verdicts as rule names: `tile_colors`, `tile_palettes`, `unique_tiles`.
pub fn checker_rules(out: &str) -> Vec<&'static str> {
    let mut v = Vec::new();
    if out.contains("colours, max 4") {
        v.push("tile_colors");
    }
    if out.contains("different 4-colour palettes") {
        v.push("tile_palettes");
    }
    if out.contains("unique tiles (flips counted") {
        v.push("unique_tiles");
    }
    v
}

/// A tiny tile set from a picture: blocks 0 air, 1 solid colour 1, 2 = block 1 with a quarter of colour 2, 3 = block 1
/// with half of colour 2 (a 256x16 reference, 16 blocks per row, the rest air). Returns the part path; the template
/// (`syn_tiles.toml`: air `a`, `b` = colour 1, `c` = colour 2, one palette `abc`) sits next to it.
pub fn synth_level(dir: &Path) -> PathBuf {
    use agentsprite::img::{Rgba, RgbaImage};
    let (air, c1, c2) = ([0x48u8, 0x90, 0xd8], [0x90u8, 0x48, 0x20], [0xd8u8, 0xb0, 0x48]);
    let mut im = RgbaImage::new(256, 16);
    for y in 0..16 {
        for x in 0..256 {
            let (b, bx) = (x / 16, x % 16);
            let c = match b {
                1 => c1,
                2 => if bx >= 8 && y >= 8 { c2 } else { c1 },
                3 => if y >= 8 { c2 } else { c1 },
                _ => air,
            };
            im.px[y * 256 + x] = Rgba::opaque(c[0], c[1], c[2]);
        }
    }
    std::fs::create_dir_all(dir.join("refs")).unwrap();
    agentsprite::png::write_rgba(&dir.join("refs/blocks.png"), &im).unwrap();
    std::fs::write(
        dir.join("syn.pxl"),
        r#"name = "syn"
kind = "tileset"
size = [128, 8]
tile = [8, 8]
template = "syn_tiles.toml"

[input]
images = "refs/{frame}.png"
factor = 2

[reduce]
factor = 2

[[frames]]
name = "blocks"
"#,
    )
    .unwrap();
    let hex = |c: [u8; 3]| agentsprite::color::to_hex(agentsprite::color::snap_step(Rgb { r: c[0], g: c[1], b: c[2] }, 8));
    let tpl = format!(
        "name = \"syn_tiles\"\n[layers.base]\n\"a\" = {{ role = \"air\", color = \"{}\" }}\n\"b\" = {{ role = \"one\", color = \"{}\" }}\n\"c\" = {{ role = \"two\", color = \"{}\" }}\n[tiles]\nair = [\"a\"]\n[palettes]\np0 = \"abc\"\n",
        hex(air),
        hex(c1),
        hex(c2)
    );
    std::fs::write(dir.join("syn_tiles.toml"), tpl).unwrap();
    dir.join("syn.pxl")
}

/// Run the real checker on screen-type files: `(folder, file name, source png)`, e.g. `("screens/menu", "font.png", p)`.
/// Returns (passed, output).
pub fn run_checker_screens(script: &Path, files: &[(&str, &str, &Path)]) -> (bool, String) {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let d = std::env::temp_dir().join(format!("pxl_chks_{}_{}", std::process::id(), n));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::copy(script, d.join("check_assets.py")).unwrap();
    for (folder, name, src) in files {
        let f = d.join(folder);
        std::fs::create_dir_all(&f).unwrap();
        std::fs::write(f.join("spec.json"), r#"{"type": "screen"}"#).unwrap();
        std::fs::copy(src, f.join(name)).unwrap();
    }
    let o = Command::new("python3").args(["-I", "check_assets.py"]).current_dir(&d).output().unwrap();
    let text = String::from_utf8_lossy(&o.stdout).into_owned();
    let _ = std::fs::remove_dir_all(&d);
    (o.status.success(), text)
}

/// Write `old | new` side by side (nearest neighbour, `scale`) to `target/m4/review/screens/<name>.png`.
pub fn review_png(name: &str, old: Option<&Path>, new: &Path, scale: usize) -> PathBuf {
    use agentsprite::img::{Rgba, RgbaImage};
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/m4/review/screens");
    std::fs::create_dir_all(&dir).unwrap();
    let n = png::read_rgba(new).unwrap();
    let o = old.filter(|p| p.is_file()).and_then(|p| png::read_rgba(p).ok());
    let (ow, oh) = o.as_ref().map_or((0, 0), |i| (i.w, i.h));
    let gap = 8;
    let (w, h) = ((ow + n.w) * scale + if o.is_some() { gap } else { 0 }, oh.max(n.h) * scale);
    let mut img = RgbaImage::filled(w, h, Rgba::opaque(0x30, 0x30, 0x38));
    let mut x0 = 0;
    for im in o.iter().chain(std::iter::once(&n)) {
        img.blit(&im.upscale(scale), x0 as i32, 0);
        x0 += im.w * scale + gap;
    }
    let f = dir.join(format!("{}.png", name));
    png::write_rgba(&f, &img).unwrap();
    f
}
