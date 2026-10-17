//! File helpers: atomic writes, part resolution and expansion, scoped-thread map, content hash.

use crate::output::{PxlError, Result};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// Portable text form of a project-relative path: `/` separators on every OS. Every project-relative path pxl
/// writes to a file or JSON, prints or compares as a string goes through this (absolute OS paths keep the native form).
pub fn rel_slash(p: &Path) -> String {
    let s = p.to_string_lossy();
    if p.is_absolute() {
        s.into_owned()
    } else {
        norm_slash(&s)
    }
}

/// Normalise a path string read from a file or the command line: both `\` and `/` separators are accepted
/// (a file written by an older Windows build still loads), `/` is returned.
pub fn norm_slash(s: &str) -> String {
    s.replace('\\', "/")
}

/// `.slash()` on any path: its portable text form (see [`rel_slash`]).
pub trait PathSlash {
    fn slash(&self) -> String;
}

impl PathSlash for Path {
    fn slash(&self) -> String {
        rel_slash(self)
    }
}

static TMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Write `bytes` to a temp file in the same directory, then rename into place.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    atomic_write_plain(path, bytes)?;
    if crate::asset::is_asset_file(path) {
        crate::versions::after_write(path, bytes);
    }
    Ok(())
}

pub fn atomic_write_plain(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("out");
    let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = dir.join(format!(".{}.tmp.{}.{}", name, std::process::id(), n));
    let ctx = |e: std::io::Error| PxlError::io(format!("cannot write {}: {}", path.display(), e));
    if let Err(e) = std::fs::write(&tmp, bytes) {
        let _ = std::fs::remove_file(&tmp);
        return Err(ctx(e));
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(ctx(e));
    }
    Ok(())
}

/// Exclusive per-part OS lock (`<part>.pxl.lock`) that serialises read-modify-write cycles of one part across
/// processes. Re-entrant inside a process (a path-keyed refcount), released on drop; `hold_part_lock` keeps it
/// until the process exits. Waits up to `PXL_LOCK_WAIT_SECS` (default 10) and then fails with a "busy" error.
pub struct PartLock {
    key: PathBuf,
}

static HELD: Mutex<Vec<(PathBuf, usize, std::fs::File)>> = Mutex::new(Vec::new());
static FOREVER: Mutex<Vec<PartLock>> = Mutex::new(Vec::new());

fn lock_wait_secs() -> f64 {
    std::env::var("PXL_LOCK_WAIT_SECS").ok().and_then(|s| s.trim().parse::<f64>().ok()).filter(|v| *v >= 0.0).unwrap_or(10.0)
}

pub fn lock_part(part: &Path) -> Result<PartLock> {
    let name = part.file_name().and_then(|n| n.to_str()).unwrap_or("part");
    let dir = match part.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let dir = std::fs::canonicalize(&dir).unwrap_or(dir);
    let key = dir.join(format!("{}.lock", name));
    {
        let mut h = HELD.lock().unwrap();
        if let Some(e) = h.iter_mut().find(|e| e.0 == key) {
            e.1 += 1;
            return Ok(PartLock { key });
        }
    }
    let f = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&key)
        .map_err(|e| PxlError::io(format!("cannot open lock {}: {}", key.display(), e)))?;
    let wait = lock_wait_secs();
    let start = std::time::Instant::now();
    loop {
        match f.try_lock() {
            Ok(()) => break,
            Err(std::fs::TryLockError::WouldBlock) => {
                if start.elapsed().as_secs_f64() >= wait {
                    return Err(PxlError::validation(format!("{} is busy: another pxl command has been modifying it for over {:.0}s", part.display(), wait))
                        .with_hint("retry shortly; modifying commands on one part run one at a time (PXL_LOCK_WAIT_SECS sets the wait)"));
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Err(std::fs::TryLockError::Error(e)) => return Err(PxlError::io(format!("cannot lock {}: {}", key.display(), e))),
        }
    }
    let mut h = HELD.lock().unwrap();
    if let Some(e) = h.iter_mut().find(|e| e.0 == key) {
        e.1 += 1; // another thread of this process got there first; our handle closes and drops its lock state
    } else {
        h.push((key.clone(), 1, f));
    }
    Ok(PartLock { key })
}

/// Lock `part` for the rest of the process (modifying commands call this before reading the part).
pub fn hold_part_lock(part: &Path) -> Result<()> {
    let l = lock_part(part)?;
    FOREVER.lock().unwrap().push(l);
    Ok(())
}

/// Release every lock this process holds on parts inside `dir` (held-for-the-process ones included). Windows cannot
/// rename a folder while a file in it is open, so a command that moves a part folder calls this first; the lock files
/// move with the folder.
pub fn release_locks_in(dir: &Path) {
    let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    FOREVER.lock().unwrap().retain(|l| !l.key.starts_with(&dir));
    let mut h = HELD.lock().unwrap();
    let mut i = 0;
    while i < h.len() {
        if h[i].0.starts_with(&dir) {
            let (_, _, f) = h.remove(i);
            let _ = f.unlock();
        } else {
            i += 1;
        }
    }
}

impl Drop for PartLock {
    fn drop(&mut self) {
        let mut h = HELD.lock().unwrap();
        if let Some(i) = h.iter().position(|e| e.0 == self.key) {
            h[i].1 -= 1;
            if h[i].1 == 0 {
                let (_, _, f) = h.remove(i);
                let _ = f.unlock();
            }
        }
    }
}

pub fn read_file(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|e| PxlError::io(format!("cannot read {}: {}", path.display(), e)).with_hint("check the path; relative paths start at the current folder"))
}

pub fn read_string(path: &Path) -> Result<String> {
    let b = read_file(path)?;
    String::from_utf8(b).map_err(|_| PxlError::validation(format!("{} is not valid UTF-8", path.display())))
}

/// Find `rel` starting at `start_dir` and walking up through its ancestors. Absolute paths are returned as is.
pub fn find_up(start_dir: &Path, rel: &str) -> Option<PathBuf> {
    let rel = norm_slash(rel);
    let rel = rel.as_str();
    let p = Path::new(rel);
    if p.is_absolute() {
        return p.exists().then(|| p.to_path_buf());
    }
    // Make the start absolute, so a relative dir like `parts` still walks up through `.` and its parents.
    let abs = std::fs::canonicalize(start_dir)
        .or_else(|_| std::env::current_dir().map(|cwd| cwd.join(start_dir)))
        .unwrap_or_else(|_| start_dir.to_path_buf());
    let mut cur = Some(abs.as_path());
    while let Some(d) = cur {
        let c = d.join(rel);
        if c.exists() {
            return Some(c);
        }
        cur = d.parent().filter(|x| !x.as_os_str().is_empty());
    }
    None
}

/// Resolve a part argument: an existing file, or a name (`player` -> `player.pxl` in the cwd).
pub fn resolve_part(arg: &str) -> Result<PathBuf> {
    let p = Path::new(arg);
    if p.is_file() {
        return Ok(p.to_path_buf());
    }
    if let Some(a) = crate::asset::find(arg) {
        return Ok(a);
    }
    if p.extension().is_none() {
        let q = PathBuf::from(format!("{}.pxl", arg));
        if q.is_file() {
            return Ok(q);
        }
    }
    // a manifest part key (`enemy_07` -> the path pxl.toml declares for it)
    if !arg.contains('/') && !arg.contains('\\') {
        if let Ok(Some(m)) = crate::manifest::Manifest::discover(None) {
            if let Some(d) = m.parts.iter().find(|d| d.name == arg) {
                if d.path.is_file() {
                    return Ok(d.path.clone());
                }
            }
        }
    }
    Err(PxlError::usage(format!("part '{}' not found", arg))
        .with_hint("give a path to a .pxl file, a part name (player -> ./player.pxl) or a part key of the nearest pxl.toml"))
}

fn has_glob(s: &str) -> bool {
    s.contains('*') || s.contains('?')
}

/// Simple wildcard match: `*` any run (no '/'), `?` one char.
pub fn wildcard(pat: &[u8], s: &[u8]) -> bool {
    let (mut p, mut i) = (0usize, 0usize);
    let (mut star, mut mark) = (usize::MAX, 0usize);
    while i < s.len() {
        if p < pat.len() && (pat[p] == b'?' || pat[p] == s[i]) && pat[p] != b'*' {
            p += 1;
            i += 1;
        } else if p < pat.len() && pat[p] == b'*' {
            star = p;
            mark = i;
            p += 1;
        } else if star != usize::MAX {
            p = star + 1;
            mark += 1;
            i = mark;
        } else {
            return false;
        }
    }
    while p < pat.len() && pat[p] == b'*' {
        p += 1;
    }
    p == pat.len()
}

fn sorted_entries(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut v: Vec<(String, PathBuf)> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
            .collect(),
        Err(_) => Vec::new(),
    };
    v.sort();
    v
}

fn glob_walk(base: PathBuf, comps: &[&str], out: &mut Vec<PathBuf>) {
    let Some((first, rest)) = comps.split_first() else {
        out.push(base);
        return;
    };
    if *first == "**" {
        glob_walk(base.clone(), rest, out);
        for (name, p) in sorted_entries(&base) {
            if !name.starts_with('.') && p.is_dir() {
                glob_walk(p, comps, out);
            }
        }
    } else if has_glob(first) {
        for (name, p) in sorted_entries(&base) {
            if name.starts_with('.') && !first.starts_with('.') {
                continue;
            }
            if wildcard(first.as_bytes(), name.as_bytes()) && (rest.is_empty() || p.is_dir()) {
                glob_walk(p, rest, out);
            }
        }
    } else {
        let p = base.join(first);
        if p.exists() {
            glob_walk(p, rest, out);
        }
    }
}

/// Expand a glob pattern (`*`, `?`, `**`) into existing paths, sorted.
pub fn glob(pattern: &str) -> Vec<PathBuf> {
    let norm = norm_slash(pattern);
    let pattern = norm.as_str();
    let b = pattern.as_bytes();
    let (base, rest) = if let Some(r) = pattern.strip_prefix('/') {
        (PathBuf::from("/"), r)
    } else if cfg!(windows) && b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        (PathBuf::from(format!("{}/", &pattern[..2])), pattern[2..].trim_start_matches('/'))
    } else {
        (PathBuf::from("."), pattern)
    };
    let comps: Vec<&str> = rest.split('/').filter(|c| !c.is_empty() && *c != ".").collect();
    let mut out = Vec::new();
    glob_walk(base, &comps, &mut out);
    out.into_iter()
        .map(|p| p.strip_prefix("./").map(|x| x.to_path_buf()).unwrap_or(p))
        .collect()
}

fn collect_pxl(dir: &Path, out: &mut Vec<PathBuf>) {
    // an asset folder is one part (its versions/ snapshots are not parts)
    if dir.join(crate::asset::ASSET_FILE).is_file() {
        out.push(dir.join(crate::asset::ASSET_FILE));
        return;
    }
    for (name, p) in sorted_entries(dir) {
        if name.starts_with('.') {
            continue;
        }
        if p.is_dir() {
            collect_pxl(&p, out);
        } else if p.extension().map_or(false, |e| e == "pxl") {
            out.push(p);
        }
    }
}

/// Expand part arguments (files, names, directories, globs) into a deterministic, de-duplicated list of
/// `.pxl` paths. Directories are searched recursively, in sorted order.
pub fn expand_parts(args: &[String]) -> Result<Vec<PathBuf>> {
    let mut out: Vec<PathBuf> = Vec::new();
    let push = |p: PathBuf, out: &mut Vec<PathBuf>| {
        if !out.contains(&p) {
            out.push(p);
        }
    };
    for a in args {
        let mut found: Vec<PathBuf> = Vec::new();
        if has_glob(a) {
            for p in glob(a) {
                if p.is_dir() {
                    collect_pxl(&p, &mut found);
                } else if p.extension().map_or(false, |e| e == "pxl") {
                    found.push(p);
                }
            }
            if found.is_empty() {
                return Err(PxlError::usage(format!("no parts match '{}'", a)));
            }
        } else if Path::new(a).is_dir() {
            collect_pxl(Path::new(a), &mut found);
            if found.is_empty() {
                return Err(PxlError::usage(format!("no .pxl files in '{}'", a)));
            }
        } else {
            found.push(resolve_part(a)?);
        }
        for p in found {
            push(p, &mut out);
        }
    }
    Ok(out)
}

/// Map `f` over `items` on all cores (scoped threads). Results are in input order.
pub fn par_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let n = items.len();
    let threads = std::thread::available_parallelism().map_or(1, |x| x.get()).min(n);
    if threads <= 1 {
        return items.iter().map(&f).collect();
    }
    let next = AtomicUsize::new(0);
    let slots: Mutex<Vec<Option<R>>> = Mutex::new((0..n).map(|_| None).collect());
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= n {
                    break;
                }
                let r = f(&items[i]);
                slots.lock().unwrap()[i] = Some(r);
            });
        }
    });
    slots.into_inner().unwrap().into_iter().map(|r| r.unwrap()).collect()
}

/// Deterministic content hash: FNV-1a 64 as 16 lower-case hex digits.
pub fn content_hash(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rel_slash_is_forward_slash_for_relative_paths() {
        assert_eq!(rel_slash(Path::new("parts/a.pxl")), "parts/a.pxl");
        assert_eq!(rel_slash(Path::new("parts\\zones\\z1\\c.pxl")), "parts/zones/z1/c.pxl");
        assert_eq!(rel_slash(&Path::new("parts").join("a").join("01_f0.png")), "parts/a/01_f0.png");
        assert_eq!(Path::new("a\\b").slash(), "a/b");
        assert_eq!(rel_slash(Path::new("")), "");
    }

    #[test]
    fn norm_slash_accepts_both_separators() {
        assert_eq!(norm_slash("a\\01_f0.png"), "a/01_f0.png");
        assert_eq!(norm_slash("a/01_f0.png"), "a/01_f0.png");
        assert_eq!(norm_slash("..\\refs/{frame}.png"), "../refs/{frame}.png");
    }

    #[test]
    fn find_up_and_glob_accept_backslash_paths() {
        let d = std::env::temp_dir().join(format!("pxl_fsutil_slash_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("parts/zones")).unwrap();
        std::fs::write(d.join("parts/zones/c.pxl"), "x").unwrap();
        assert!(find_up(&d, "parts\\zones\\c.pxl").is_some());
        let pat = format!("{}/parts\\zones\\*.pxl", d.to_string_lossy().replace('\\', "/"));
        assert_eq!(glob(&pat).len(), 1, "{}", pat);
        let _ = std::fs::remove_dir_all(&d);
    }
}
