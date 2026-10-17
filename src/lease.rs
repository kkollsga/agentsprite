//! Part leases (DESIGN 6.2). Owner: queue worker.
//!
//! A lease is the file `<part>.lease` next to the part (`player.pxl` -> `player.lease`) holding
//! `{"agent": A, "since": t0, "renewed": t1}` (unix seconds, fractional). It expires when it has not been
//! renewed for `PXL_LEASE_SECS` seconds (default 900 = 15 min). A lease file that is empty or unreadable
//! (writer killed between create and write) is judged by its mtime, so it expires like any other.
//!
//! # Claiming and the steal protocol
//!
//! 1. *Fresh claim*: `OpenOptions::create_new` on the lease path. The filesystem lets exactly one process
//!    create the file; all others get `AlreadyExists` and then read the winner's lease ("held by A since ...").
//! 2. *Renew*: the holder rewrites the file via temp file + rename, only while the lease is still live, so a
//!    live lease can never be stolen from under a renewal.
//! 3. *Steal of an expired lease*: a stealer first takes the mutex `<part>.lease.lock` with `create_new`
//!    (only one stealer gets it; a lock older than 10 s belonged to a crashed stealer and is removed). Inside
//!    the mutex it re-reads the lease: if it is gone or live again somebody else already took over and it
//!    backs off. Otherwise it renames the expired lease to a unique grave name (the atomic rename-away), deletes
//!    the grave, and `create_new`s its own lease, then drops the mutex. Re-reading under the mutex avoids the
//!    ABA problem of renaming whatever file happens to sit at the path; a late `create_new` by a fresh claimer
//!    can still win the race, in which case the stealer sees the live lease and reports "held by".
//!
//! Killing a process at any point leaves valid files: lease writes are `create_new` + short write (empty file
//! tolerated, see above) or atomic rename; a leftover lock is cleaned after 10 s.

use crate::fsutil::PathSlash;
use crate::fsutil;
use crate::output::{PxlError, Result};
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DEFAULT_TTL: f64 = 900.0;
const LOCK_STALE_SECS: f64 = 10.0;
static N: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Debug, PartialEq)]
pub struct Lease {
    pub agent: String,
    pub since: f64,
    pub renewed: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LeaseInfo {
    pub lease: Lease,
    pub live: bool,
    /// Seconds since last renewal.
    pub idle: f64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Acquired {
    Fresh,
    Renewed,
    /// Took over an expired lease of this agent.
    Stolen(String),
}

pub fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

/// Idle expiry in seconds: env `PXL_LEASE_SECS` (fractions allowed), default 900.
pub fn ttl_secs() -> f64 {
    std::env::var("PXL_LEASE_SECS").ok().and_then(|s| s.trim().parse::<f64>().ok()).filter(|v| *v >= 0.0).unwrap_or(DEFAULT_TTL)
}

pub fn lease_path(part: &Path) -> PathBuf {
    part.with_extension("lease")
}

fn lock_path(part: &Path) -> PathBuf {
    part.with_extension("lease.lock")
}

/// `2026-10-11 12:00:00 UTC` for a unix time.
pub fn fmt_time(t: f64) -> String {
    let secs = t.max(0.0) as i64;
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC", y, m, d, rem / 3600, rem % 3600 / 60, rem % 60)
}

fn parse(text: &str) -> Option<Lease> {
    let v: Value = serde_json::from_str(text).ok()?;
    Some(Lease { agent: v.get("agent")?.as_str()?.to_string(), since: v.get("since")?.as_f64()?, renewed: v.get("renewed")?.as_f64()? })
}

fn body(l: &Lease) -> String {
    json!({"agent": l.agent, "since": l.since, "renewed": l.renewed}).to_string()
}

fn mtime(p: &Path) -> Option<f64> {
    std::fs::metadata(p).ok()?.modified().ok()?.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs_f64())
}

/// Read the lease of a part, if any. Half-written files are retried briefly, then judged by mtime.
pub fn read(part: &Path) -> Option<LeaseInfo> {
    let p = lease_path(part);
    let mut tries = 0;
    loop {
        let text = match std::fs::read_to_string(&p) {
            Ok(t) => t,
            Err(_) => return None,
        };
        let t = now();
        if let Some(lease) = parse(&text) {
            let idle = (t - lease.renewed).max(0.0);
            return Some(LeaseInfo { live: idle < ttl_secs(), idle, lease });
        }
        tries += 1;
        if tries > 8 {
            let m = mtime(&p).unwrap_or(t);
            let idle = (t - m).max(0.0);
            return Some(LeaseInfo { live: idle < ttl_secs(), idle, lease: Lease { agent: "?".into(), since: m, renewed: m } });
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn held_err(part: &Path, i: &LeaseInfo) -> PxlError {
    let left = (ttl_secs() - i.idle).max(0.0);
    PxlError::validation(format!(
        "{} is held by {} since {} (renewed {:.0}s ago, expires in {:.0}s)",
        part.slash(),
        i.lease.agent,
        fmt_time(i.lease.since),
        i.idle,
        left
    ))
    .with_hint("work on another part (`pxl next --agent <you>`) or wait for the lease to expire")
}

fn write_new(path: &Path, l: &Lease) -> std::io::Result<()> {
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(path)?;
    f.write_all(body(l).as_bytes())
}

fn renew_file(part: &Path, cur: &Lease) -> Result<()> {
    let l = Lease { agent: cur.agent.clone(), since: cur.since, renewed: now() };
    fsutil::atomic_write(&lease_path(part), body(&l).as_bytes())
}

/// Claim `part` for `agent`: fresh create, renewal of its own live lease, or takeover of an expired one.
/// Another agent's live lease is an error ("held by A since ...").
pub fn acquire(part: &Path, agent: &str) -> Result<Acquired> {
    let _pl = fsutil::lock_part(part)?;
    let lp = lease_path(part);
    for _ in 0..200 {
        let t = now();
        match write_new(&lp, &Lease { agent: agent.to_string(), since: t, renewed: t }) {
            Ok(()) => return Ok(Acquired::Fresh),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(PxlError::io(format!("cannot create {}: {}", lp.slash(), e))),
        }
        let Some(cur) = read(part) else { continue };
        if cur.live {
            if cur.lease.agent == agent {
                renew_file(part, &cur.lease)?;
                return Ok(Acquired::Renewed);
            }
            return Err(held_err(part, &cur));
        }
        // Expired: steal under the lock.
        let lk = lock_path(part);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&lk) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if mtime(&lk).map_or(false, |m| now() - m > LOCK_STALE_SECS) {
                    let _ = std::fs::remove_file(&lk);
                }
                std::thread::sleep(Duration::from_millis(2));
                continue;
            }
            Err(e) => return Err(PxlError::io(format!("cannot create {}: {}", lk.slash(), e))),
        }
        let res = (|| -> Option<Result<Acquired>> {
            let again = read(part);
            if let Some(a) = &again {
                if a.live || a.lease != cur.lease {
                    return None; // someone else took over meanwhile: re-evaluate
                }
            }
            if again.is_some() {
                let grave = part.with_extension(format!("lease.dead.{}.{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
                if std::fs::rename(&lp, &grave).is_err() {
                    return None;
                }
                let _ = std::fs::remove_file(&grave);
            }
            let t = now();
            match write_new(&lp, &Lease { agent: agent.to_string(), since: t, renewed: t }) {
                Ok(()) => Some(Ok(Acquired::Stolen(cur.lease.agent.clone()))),
                Err(_) => None,
            }
        })();
        let _ = std::fs::remove_file(&lk);
        if let Some(r) = res {
            return r;
        }
    }
    Err(PxlError::io(format!("could not claim {} (lease contention)", part.slash())))
}

/// Extend the caller's own live lease. Errors when someone else holds it or when it is gone/expired.
pub fn renew(part: &Path, agent: &str) -> Result<()> {
    let _pl = fsutil::lock_part(part)?;
    match read(part) {
        Some(i) if i.lease.agent == agent && i.live => renew_file(part, &i.lease),
        Some(i) if i.live => Err(held_err(part, &i)),
        _ => Err(PxlError::validation(format!("{} has no live lease for {}", part.slash(), agent)).with_hint("claim it again with `pxl claim`")),
    }
}

/// Drop the caller's lease. Missing or expired leases are fine; a live lease of another agent is an error.
pub fn release(part: &Path, agent: &str) -> Result<()> {
    let _pl = fsutil::lock_part(part)?;
    if let Some(i) = read(part) {
        if i.live && i.lease.agent != agent {
            return Err(held_err(part, &i));
        }
        let _ = std::fs::remove_file(lease_path(part));
    }
    Ok(())
}

/// Every modifying command calls this first. Another agent's live lease refuses (exit 1); the holder's own
/// lease is renewed; no lease (or an expired one) is allowed without claiming; a live lease with no `--agent`
/// refuses because the caller cannot prove it is the holder. Read-only commands never call this.
pub fn guard(part_path: &Path, agent: Option<&str>) -> Result<()> {
    fsutil::hold_part_lock(part_path)?;
    let Some(i) = read(part_path) else { return Ok(()) };
    if !i.live {
        return Ok(());
    }
    match agent {
        Some(a) if a == i.lease.agent => renew_file(part_path, &i.lease),
        Some(_) => Err(held_err(part_path, &i)),
        None => Err(held_err(part_path, &i).with_hint(format!("pass --agent {} to modify a leased part", i.lease.agent))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn time_format() {
        assert_eq!(fmt_time(0.0), "1970-01-01 00:00:00 UTC");
        assert_eq!(fmt_time(1_700_000_000.0), "2023-11-14 22:13:20 UTC");
    }
}
