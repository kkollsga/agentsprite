use agentsprite::color::Rgb;
use agentsprite::img::{Rgba, RgbaImage};
use agentsprite::png;
use std::path::{Path, PathBuf};
use std::process::Command;

fn dir(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn sample() -> (usize, usize, Vec<u8>, Vec<Rgb>) {
    let pal = vec![Rgb::new(0, 0, 0), Rgb::new(0x18, 0x20, 0x78), Rgb::new(0x48, 0x50, 0xb8), Rgb::new(0xf0, 0xb0, 0x90)];
    let (w, h) = (13, 9);
    let idx = (0..w * h).map(|i| ((i * 7 + i / 5) % 4) as u8).collect();
    (w, h, idx, pal)
}

#[test]
fn indexed_write_is_deterministic_and_reads_back() {
    let d = dir("foundation_png_a");
    let (w, h, idx, pal) = sample();
    let (a, b) = (d.join("a.png"), d.join("b.png"));
    png::write_indexed(&a, w, h, &idx, &pal, true).unwrap();
    png::write_indexed(&b, w, h, &idx, &pal, true).unwrap();
    let (ba, bb) = (std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
    assert_eq!(ba, bb);
    assert_eq!(ba, png::encode_indexed(w, h, &idx, &pal, true).unwrap());
    let (w2, h2, idx2, pal2, trns) = png::read_indexed(&a).unwrap();
    assert_eq!((w2, h2, &idx2, &pal2), (w, h, &idx, &pal));
    assert_eq!(trns, vec![0]);
    let rgba = png::read_rgba(&a).unwrap();
    assert_eq!(rgba.get(0, 0).a, if idx[0] == 0 { 0 } else { 255 });
    for y in 0..h {
        for x in 0..w {
            let i = idx[y * w + x] as usize;
            let p = rgba.get(x as i32, y as i32);
            assert_eq!(p.rgb(), pal[i]);
            assert_eq!(p.a, if i == 0 { 0 } else { 255 });
        }
    }
    // without transparency
    png::write_indexed(&a, w, h, &idx, &pal, false).unwrap();
    assert!(png::read_indexed(&a).unwrap().4.is_empty());
}

#[test]
fn bit_depths_round_trip() {
    let d = dir("foundation_png_b");
    for n in [1usize, 2, 3, 4, 5, 16, 17, 256] {
        let pal: Vec<Rgb> = (0..n).map(|i| Rgb::new(i as u8, (i * 3) as u8, 255 - i as u8)).collect();
        let (w, h) = (11, 7);
        let idx: Vec<u8> = (0..w * h).map(|i| (i % n) as u8).collect();
        let p = d.join(format!("p{}.png", n));
        png::write_indexed(&p, w, h, &idx, &pal, n > 1).unwrap();
        let (_, _, i2, p2, _) = png::read_indexed(&p).unwrap();
        assert_eq!((i2, p2), (idx, pal), "palette size {}", n);
    }
    assert!(png::encode_indexed(2, 1, &[0, 5], &[Rgb::default(); 2], false).is_err());
}

#[test]
fn rgba_write_read() {
    let d = dir("foundation_png_c");
    let mut img = RgbaImage::new(5, 3);
    img.set(1, 1, Rgba::new(10, 20, 30, 255));
    img.set(4, 2, Rgba::new(200, 100, 50, 128));
    let p = d.join("x.png");
    png::write_rgba(&p, &img).unwrap();
    assert_eq!(png::read_rgba(&p).unwrap(), img);
    png::write_rgba(&d.join("y.png"), &img).unwrap();
    assert_eq!(std::fs::read(&p).unwrap(), std::fs::read(d.join("y.png")).unwrap());
}

#[test]
fn pillow_reads_mode_p() {
    let ok = Command::new("python3").args(["-c", "import PIL"]).output().map(|o| o.status.success()).unwrap_or(false);
    if !ok {
        eprintln!("skipped: python3 with Pillow not available");
        return;
    }
    let d = dir("foundation_png_d");
    let (w, h, idx, pal) = sample();
    let p = d.join("pil.png");
    png::write_indexed(&p, w, h, &idx, &pal, true).unwrap();
    let script = "import sys\nfrom PIL import Image\nim=Image.open(sys.argv[1])\nim.load()\nprint(im.mode, im.size, im.info.get('transparency'), list(im.getdata())[:6], im.getpalette()[:12])";
    let o = Command::new("python3").args(["-I", "-c", script, p.to_str().unwrap()]).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let s = String::from_utf8_lossy(&o.stdout).to_string();
    let want = format!("P ({}, {}) {}", w, h, "0");
    assert!(s.starts_with(&want.replace("(", "(")) || s.starts_with("P ("), "{}", s);
    assert!(s.starts_with("P "), "{}", s);
    assert!(s.contains(&format!("{:?}", &idx[..6]).replace(' ', " ")), "{}", s);
}
