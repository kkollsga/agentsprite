//! Embeds the repo's `skills/` folder: walks it (sorted, so the build is deterministic) and writes
//! `$OUT_DIR/skills_gen.rs` with one `include_str!` entry per text file. A missing or empty `skills/` still
//! compiles (the table is just empty).

use std::path::{Path, PathBuf};

fn walk(dir: &Path, rel: &str, out: &mut Vec<(String, PathBuf)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = e.path();
        let r = if rel.is_empty() { name.clone() } else { format!("{}/{}", rel, name) };
        if path.is_dir() {
            walk(&path, &r, out);
        } else if matches!(path.extension().and_then(|x| x.to_str()), Some("md" | "txt" | "toml" | "json")) {
            out.push((r, path));
        } else {
            // never skip a file silently: a skill file the binary cannot embed must stop the build
            panic!("skills/{} has a file type build.rs does not embed (md, txt, toml, json); extend build.rs or remove it", r);
        }
    }
}

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("skills");
    println!("cargo:rerun-if-changed=skills");
    println!("cargo:rerun-if-changed=build.rs");
    let mut files = Vec::new();
    walk(&root, "", &mut files);
    files.sort();
    let mut s = String::from("pub static FILES: &[(&str, &str)] = &[\n");
    for (rel, path) in &files {
        println!("cargo:rerun-if-changed={}", path.display());
        s.push_str(&format!("    ({:?}, include_str!({:?})),\n", rel, path.display().to_string()));
    }
    s.push_str("];\n");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("skills_gen.rs");
    std::fs::write(out, s).unwrap();
}
