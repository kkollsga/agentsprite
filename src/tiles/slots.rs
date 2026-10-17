//! Animated tile slots (M5 phase T3): the level budget is level tiles + the VRAM slots animations reserve.
//!
//! An animation set is the frames `<name>_<nn>` of a tile set part. The tiles one frame needs (its unique tiles) are
//! the slots the engine reserves for the set; with profile `anim_slots = "all_frames"` every frame is resident
//! (frames x tiles). A level tile set lists the animated parts that belong to it with `animated = ["../x/x.pxl"]`
//! (paths relative to the part); animation frames inside the part itself count too.

use super::{stats, Colors, Sheet};
use crate::model::Part;
use crate::profile::Profile;
use crate::{pxl, template};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default)]
pub struct Slots {
    pub total: usize,
    /// (set name, frames, tiles per frame, slots)
    pub sets: Vec<(String, usize, usize, usize)>,
}

impl Slots {
    pub fn describe(&self) -> String {
        if self.sets.is_empty() {
            return "no animated sets".into();
        }
        self.sets.iter().map(|(n, f, t, s)| format!("{}: {} frames x {} tiles = {}", n, f, t, s)).collect::<Vec<_>>().join("; ")
    }
}

/// A frame named `<set>_<nn>`.
pub fn is_set_frame(frame: &str) -> bool {
    set_name(frame).is_some()
}

fn set_name(frame: &str) -> Option<&str> {
    match frame.rfind('_') {
        Some(i) if i > 0 && !frame[i + 1..].is_empty() && frame[i + 1..].bytes().all(|b| b.is_ascii_digit()) => Some(&frame[..i]),
        _ => None,
    }
}

fn collect(part: &Part, prof: &Profile, out: &mut Slots, prefix: &str) {
    if part.kind != crate::model::PartKind::Tileset {
        return;
    }
    let Ok(tpl) = template::load_for(part) else { return };
    let cols = Colors::new(part, &tpl);
    let mut by: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, f) in part.frames.iter().enumerate() {
        if let Some(n) = set_name(&f.name) {
            by.entry(n.to_string()).or_default().push(i);
        }
    }
    for (name, fis) in by {
        let mut tiles = 0;
        for &fi in &fis {
            let g = &part.frames[fi].grid;
            if g.w % 8 != 0 || g.h % 8 != 0 {
                continue;
            }
            tiles = tiles.max(stats::unique_groups(&Sheet::from_grid(g), &cols).len());
        }
        let slots = if prof.anim_slots == "all_frames" { fis.len() * tiles } else { tiles };
        out.sets.push((format!("{}{}", prefix, name), fis.len(), tiles, slots));
        out.total += slots;
    }
}

/// Slots of the animation sets inside `part` and inside the parts it lists in `animated`.
pub fn animated_slots(part: &Part, prof: &Profile) -> Slots {
    let mut out = Slots::default();
    collect(part, prof, &mut out, "");
    for rel in &part.animated {
        let p = part.dir().join(rel);
        if let Ok(other) = pxl::load(&p) {
            collect(&other, prof, &mut out, &format!("{}:", other.name));
        }
    }
    out
}
