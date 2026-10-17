//! Shared helpers of the `e2e_*` integration tests: temp dirs, running `pxl`, and a deterministic synthetic project
//! generator (no private data). Every test binary uses a different subset, hence the `allow(dead_code)`.
#![allow(dead_code)]

use agentsprite::img::{Rgba, RgbaImage};
use agentsprite::model::{Frame, Grid, Part};
use agentsprite::{png, pxl};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

pub const TEMPLATE: &str = r##"name = "t"
[layers.base]
"." = { role = "transparent" }
"1" = { role = "outline", color = "#182078" }
"2" = { role = "body",    color = "#4850b8" }
"3" = { role = "skin",    color = "#f0b090" }
[layers.detail]
"W" = { role = "white", color = "#f0f0f0", under = "3" }
[map]
"#182078" = "1"
"#4850b8" = "2"
"#f0b090" = "3"
"#f0f0f0" = "W"
"##;

/// The old per-asset model for a synthetic project: `legacy = true` in pxl.toml (no scenes or palette sets demanded) and
/// overlay layers allowed by a project profile.
pub fn legacy(root: &Path) {
    std::fs::create_dir_all(root.join("profiles")).unwrap();
    std::fs::write(root.join("profiles/gbc-sprite.toml"), "overlays = \"allow\"\n").unwrap();
    let m = root.join("pxl.toml");
    if let Ok(t) = std::fs::read_to_string(&m) {
        if !t.contains("legacy") {
            std::fs::write(&m, format!("legacy = true\n{}", t)).unwrap();
        }
    }
}

pub fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_pxl")
}

pub fn tmpdir(tag: &str) -> PathBuf {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let d = std::env::temp_dir().join(format!("pxl_e2e_{}_{}_{}", tag, std::process::id(), n));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

pub fn pxl_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(bin()).current_dir(dir).args(args).stdin(Stdio::null()).output().unwrap()
}

pub fn pxl_stdin(dir: &Path, args: &[&str], stdin: &str) -> Output {
    use std::io::Write;
    let mut c = Command::new(bin()).current_dir(dir).args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    c.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
    c.wait_with_output().unwrap()
}

pub fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

pub fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

pub fn json_of(o: &Output) -> serde_json::Value {
    serde_json::from_slice(&o.stdout).unwrap_or_else(|e| panic!("stdout is not one JSON object ({}): {:?}", e, String::from_utf8_lossy(&o.stdout)))
}

fn color_of(k: u8) -> Rgba {
    match k {
        b'1' => Rgba::opaque(0x18, 0x20, 0x78),
        b'2' => Rgba::opaque(0x48, 0x50, 0xb8),
        b'3' => Rgba::opaque(0xf0, 0xb0, 0x90),
        b'W' => Rgba::opaque(0xf0, 0xf0, 0xf0),
        _ => Rgba::TRANSPARENT,
    }
}

/// A deterministic blobby sprite: a filled ellipse with an outline, skin and body areas and a white eye.
pub fn sprite_grid(w: usize, h: usize, seed: u64) -> Grid {
    let mut g = Grid::new(w, h, b'.');
    let (cx, cy) = (w as f64 / 2.0 + (seed % 3) as f64 - 1.0, h as f64 / 2.0 + ((seed / 3) % 3) as f64 - 1.0);
    let (rx, ry) = (w as f64 / 2.0 - 2.5 - (seed % 2) as f64, h as f64 / 2.0 - 2.5 - ((seed / 2) % 2) as f64);
    for y in 0..h {
        for x in 0..w {
            let d = ((x as f64 + 0.5 - cx) / rx).powi(2) + ((y as f64 + 0.5 - cy) / ry).powi(2);
            g.cells[y * w + x] = if d > 1.0 {
                b'.'
            } else if d > 0.62 {
                b'1'
            } else if y as f64 > cy {
                b'2'
            } else {
                b'3'
            };
        }
    }
    let (ex, ey) = ((cx + 1.0) as i32, (cy - 2.0) as i32);
    g.set(ex, ey, b'W');
    g.set(ex + 1, ey, b'W');
    g
}

pub fn upscale(g: &Grid, f: usize) -> RgbaImage {
    let mut img = RgbaImage::new(g.w * f, g.h * f);
    for y in 0..g.h * f {
        for x in 0..g.w * f {
            img.set(x as i32, y as i32, color_of(g.cells[(y / f) * g.w + x / f]));
        }
    }
    img
}

/// Writes `root/templates/t.toml`, `root/refs/<part>/<frame>.png`, `root/parts/<part>.pxl` (blank grids, input
/// `images`, factor 2) and `root/pxl.toml` for `n_parts` parts of `n_frames` frames each (16x16 cells).
/// Returns the part paths. Parts are named `p00`, `p01`, ...; frames `f0`, `f1`, ... (tag `f`).
pub fn gen_project(root: &Path, n_parts: usize, n_frames: usize, seed: u64) -> Vec<PathBuf> {
    std::fs::create_dir_all(root.join("templates")).unwrap();
    std::fs::create_dir_all(root.join("parts")).unwrap();
    std::fs::write(root.join("templates/t.toml"), TEMPLATE).unwrap();
    let mut manifest = String::new();
    let mut paths = Vec::new();
    for k in 0..n_parts {
        let name = format!("p{:02}", k);
        let rdir = root.join("refs").join(&name);
        std::fs::create_dir_all(&rdir).unwrap();
        let mut part = Part::new(&name, (16, 16));
        part.profile = Some("gbc-sprite".into());
        part.origin = (8, 8);
        part.template = Some("templates/t.toml".into());
        part.input.images = Some(format!("refs/{}/{{frame}}.png", name));
        part.input.factor = Some(2);
        part.reduce.factor = Some(2);
        part.reduce.phase = Some((0, 0));
        part.reduce.outline = vec!["outline".into()];
        for i in 0..n_frames {
            let fname = format!("f{}", i);
            let g = sprite_grid(16, 16, seed.wrapping_add((k * 31 + i * 7) as u64));
            png::write_rgba(&rdir.join(format!("{}.png", fname)), &upscale(&g, 2)).unwrap();
            part.frames.push(Frame { name: fname, src: None, locked: false, grid: Grid::new(16, 16, b'.') });
        }
        let path = root.join("parts").join(format!("{}.pxl", name));
        std::fs::write(&path, pxl::to_string(&part)).unwrap();
        manifest.push_str(&format!("[parts.{}]\npath = \"parts/{}.pxl\"\nout = \"out/{}\"\n", name, name, name));
        paths.push(path);
    }
    std::fs::write(root.join("pxl.toml"), manifest).unwrap();
    legacy(root);
    paths
}

/// Every file below `dir` (relative path, bytes), sorted; dot files and `skip` names excluded.
pub fn tree(dir: &Path, skip: &[&str]) -> Vec<(String, Vec<u8>)> {
    fn walk(base: &Path, d: &Path, skip: &[&str], out: &mut Vec<(String, Vec<u8>)>) {
        let mut es: Vec<_> = std::fs::read_dir(d).unwrap().map(|e| e.unwrap().path()).collect();
        es.sort();
        for p in es {
            let n = p.file_name().unwrap().to_string_lossy().into_owned();
            if n.starts_with('.') || skip.iter().any(|s| n.ends_with(s)) {
                continue;
            }
            if p.is_dir() {
                walk(base, &p, skip, out);
            } else {
                out.push((agentsprite::fsutil::rel_slash(p.strip_prefix(base).unwrap()), std::fs::read(&p).unwrap()));
            }
        }
    }
    let mut v = Vec::new();
    walk(dir, dir, skip, &mut v);
    v
}

pub fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let (s, d) = (e.path(), to.join(e.file_name()));
        if s.is_dir() {
            copy_dir(&s, &d);
        } else {
            std::fs::copy(&s, &d).unwrap();
        }
    }
}
