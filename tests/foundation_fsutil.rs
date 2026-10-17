use agentsprite::fsutil::*;
use std::path::{Path, PathBuf};

fn dir(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn atomic_write_replaces_and_leaves_no_temp() {
    let d = dir("foundation_fs_a");
    let p = d.join("x.bin");
    atomic_write(&p, b"one").unwrap();
    atomic_write(&p, b"two!").unwrap();
    assert_eq!(std::fs::read(&p).unwrap(), b"two!");
    let n = std::fs::read_dir(&d).unwrap().count();
    assert_eq!(n, 1);
    assert!(atomic_write(&d.join("missing/dir/x"), b"x").is_err());
}

#[test]
fn atomic_write_concurrent_writers_never_tear() {
    let d = dir("foundation_fs_b");
    let p = d.join("shared.txt");
    std::thread::scope(|s| {
        for t in 0..8u8 {
            let p = &p;
            s.spawn(move || {
                for _ in 0..50 {
                    atomic_write(p, &vec![b'a' + t; 4096]).unwrap();
                }
            });
        }
    });
    let b = std::fs::read(&p).unwrap();
    assert_eq!(b.len(), 4096);
    assert!(b.iter().all(|&c| c == b[0]));
}

#[test]
fn expand_parts_files_dirs_globs() {
    let d = dir("foundation_fs_c");
    for f in ["parts/a.pxl", "parts/b.pxl", "parts/zones/z1/c.pxl", "parts/notes.txt", "player.pxl"] {
        let p = d.join(f);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "x").unwrap();
    }
    let s = |p: &str| d.join(p).display().to_string();
    let r = expand_parts(&[s("parts")]).unwrap();
    let names: Vec<_> = r.iter().map(|p| rel_slash(p.strip_prefix(&d).unwrap())).collect();
    assert_eq!(names, ["parts/a.pxl", "parts/b.pxl", "parts/zones/z1/c.pxl"]);
    let r = expand_parts(&[s("parts/?.pxl"), s("parts/*.pxl"), s("player.pxl")]).unwrap();
    assert_eq!(r.len(), 3, "globs de-duplicate: {:?}", r);
    let r = expand_parts(&[s("parts/**/c.pxl")]).unwrap();
    assert_eq!(r.len(), 1);
    assert!(expand_parts(&[s("parts/*.zzz")]).is_err());
    assert!(expand_parts(&[s("nothing")]).is_err());
    assert_eq!(expand_parts(&[s("player")]).unwrap().len(), 1, "bare name resolves to .pxl");
}

#[test]
fn wildcard_cases() {
    assert!(wildcard(b"*.pxl", b"a.pxl"));
    assert!(wildcard(b"a?c*", b"abcdef"));
    assert!(!wildcard(b"*.pxl", b"a.png"));
    assert!(wildcard(b"*", b""));
    assert!(!wildcard(b"?", b""));
    assert!(wildcard(b"a*b*c", b"aXXbYYc"));
}

#[test]
fn par_map_keeps_order_and_hash_is_fixed() {
    let items: Vec<u64> = (0..1000).collect();
    let r = par_map(&items, |x| x * x);
    assert_eq!(r, items.iter().map(|x| x * x).collect::<Vec<_>>());
    assert!(par_map::<u8, u8>(&[], |x| *x).is_empty());
    assert_eq!(content_hash(b""), "cbf29ce484222325");
    assert_eq!(content_hash(b"a"), "af63dc4c8601ec8c");
    assert_eq!(content_hash(b"hello"), content_hash(b"hello"));
}
