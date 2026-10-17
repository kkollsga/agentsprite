//! Level layouts, zone completeness and real screens (M6 R6b).
//!
//! A scene may name the object layout of its level: `[scene.zone_1] layout = "layouts/zone_1.json"`. Two generic formats:
//! * JSON: `{"objects": {"0x26": "monitor"}, "acts": {"1": [{"object": "monitor", "x": 160, "y": 120}, ...]}}` (`objects` maps an id to a
//!   part name when they differ; a top-level array is one act "1");
//! * CSV: `act,object,x,y` (or `object,x,y`), one placement per line.
//!
//! **Zone completeness** (`pxl check`, `pxl scene NAME --layout`): every object the layout places must have art in this zone's slot
//! meaning: its part is listed in a group of the scene (the scene check already compares its palette set slot by slot), or the part's
//! `meta.toml` has a checked `[zone_remap.SCENE]` that moves every slot the part uses onto a slot the scene's set defines. Anything
//! else (no part, a part of another zone without remap, a remap that misses a slot) is a finding: art borrowed across zones shows the
//! wrong colours.
//!
//! **Real screens**: the objects of an act are composed with their widest frame on a 160x144 window that slides along the act; the sprites
//! on every absolute scanline and in the frame are summed against the hardware limits, so cuts and BG conversions are judged on the
//! screens the level actually builds, not on one object.

use crate::fsutil::PathSlash;
use crate::cli::SceneArgs;
use crate::manifest::{Manifest, Scene};
use crate::output::{Out, PxlError, Result};
use crate::profile::Profile;
use crate::scene;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Placed {
    pub act: String,
    pub object: String,
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Debug, Default)]
pub struct Layout {
    pub path: PathBuf,
    pub objects: BTreeMap<String, String>,
    pub placed: Vec<Placed>,
}

impl Layout {
    pub fn acts(&self) -> Vec<String> {
        let mut a: Vec<String> = self.placed.iter().map(|p| p.act.clone()).collect::<BTreeSet<_>>().into_iter().collect();
        a.sort_by(|x, y| x.parse::<i64>().ok().cmp(&y.parse::<i64>().ok()).then(x.cmp(y)));
        a
    }
    pub fn part_of(&self, object: &str) -> String {
        self.objects.get(object).cloned().unwrap_or_else(|| object.to_string())
    }
}

fn num(v: &Value) -> Option<i32> {
    match v {
        Value::Number(n) => n.as_i64().map(|x| x as i32),
        Value::String(s) => {
            let t = s.trim();
            match t.strip_prefix("0x") {
                Some(h) => i64::from_str_radix(h, 16).ok().map(|x| x as i32),
                None => t.parse::<i64>().ok().map(|x| x as i32),
            }
        }
        _ => None,
    }
}

pub fn parse(text: &str, path: &std::path::Path) -> Result<Layout> {
    let bad = |m: String| PxlError::validation(format!("{}: {}", path.slash(), m));
    let mut l = Layout { path: path.to_path_buf(), ..Default::default() };
    let t = text.trim_start();
    if t.starts_with('{') || t.starts_with('[') {
        let v: Value = serde_json::from_str(text).map_err(|e| bad(format!("invalid JSON: {}", e)))?;
        let (map, acts) = if v.is_array() { (None, json!({"1": v})) } else { (v.get("objects").cloned(), v.get("acts").cloned().unwrap_or(Value::Null)) };
        if let Some(Value::Object(m)) = map {
            for (k, p) in m {
                l.objects.insert(k, p.as_str().ok_or_else(|| bad("objects maps an id to a part name".into()))?.to_string());
            }
        }
        let acts = acts.as_object().ok_or_else(|| bad("needs \"acts\": {\"1\": [{\"object\", \"x\", \"y\"}]}".into()))?;
        for (act, list) in acts {
            for e in list.as_array().ok_or_else(|| bad(format!("act {} must be a list", act)))? {
                let obj = match e.get("object") {
                    Some(Value::String(s)) => s.clone(),
                    Some(Value::Number(n)) => n.to_string(),
                    _ => return Err(bad(format!("act {}: a placement has no object", act))),
                };
                let (x, y) = (e.get("x").and_then(num), e.get("y").and_then(num));
                let (Some(x), Some(y)) = (x, y) else { return Err(bad(format!("act {}: placement of {} needs numeric x and y", act, obj))) };
                l.placed.push(Placed { act: act.clone(), object: obj, x, y });
            }
        }
    } else {
        for (i, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let c: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
            if i == 0 && c.iter().any(|x| x.eq_ignore_ascii_case("object")) {
                continue;
            }
            let (act, obj, x, y) = match c.as_slice() {
                [a, o, x, y] => (a.to_string(), o.to_string(), *x, *y),
                [o, x, y] => ("1".to_string(), o.to_string(), *x, *y),
                _ => return Err(bad(format!("line {}: expected act,object,x,y", i + 1))),
            };
            let (Some(x), Some(y)) = (num(&json!(x)), num(&json!(y))) else { return Err(bad(format!("line {}: x and y must be numbers", i + 1))) };
            l.placed.push(Placed { act, object: obj, x, y });
        }
    }
    if l.placed.is_empty() {
        return Err(bad("the layout places no objects".into()));
    }
    Ok(l)
}

pub fn load(m: &Manifest, sc: &Scene) -> Result<Option<Layout>> {
    let Some(f) = &sc.layout else { return Ok(None) };
    let p = m.dir.join(f);
    let text = std::fs::read_to_string(&p).map_err(|_| PxlError::validation(format!("scene {}: layout {} cannot be read", sc.name, p.slash())))?;
    parse(&text, &p).map(Some)
}

/// The scene's layout as the manifest lists it.
pub fn layout_of(m: &Manifest, sc: &Scene) -> Value {
    match load(m, sc) {
        Ok(Some(l)) => json!({"file": sc.layout, "objects": l.placed.len(), "acts": l.acts(), "distinct": l.placed.iter().map(|p| p.object.clone()).collect::<BTreeSet<_>>().len()}),
        Ok(None) => Value::Null,
        Err(e) => json!({"file": sc.layout, "error": e.msg}),
    }
}

#[derive(Clone, Debug)]
pub struct Finding {
    pub object: String,
    pub part: Option<String>,
    pub kind: &'static str,
    pub message: String,
    pub count: usize,
}

/// Zone completeness of a scene's layout.
pub fn completeness(m: &Manifest, sc: &Scene, l: &Layout) -> Vec<Finding> {
    let mut v = Vec::new();
    let in_scene: BTreeSet<String> = sc.groups.iter().flat_map(|g| g.parts.iter().flat_map(|p| crate::cellmem::resolve_parts(m, p).into_iter().map(|d| d.name.clone()))).collect();
    let set = sc.palette_set.as_ref().and_then(|n| crate::cells::load_set(n, &m.dir).ok());
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for p in &l.placed {
        *counts.entry(p.object.clone()).or_insert(0) += 1;
    }
    for (obj, n) in counts {
        let pname = l.part_of(&obj);
        let Some(d) = m.parts.iter().find(|d| d.name == pname) else {
            v.push(Finding { object: obj.clone(), part: None, kind: "no_part", message: format!("object {} ({}x) has no part in the project: no art for zone {}", obj, n, sc.name), count: n });
            continue;
        };
        if in_scene.contains(&d.name) {
            continue;
        }
        // not in this zone's groups: art drawn for a set that holds the same colours in the slots it uses is fine here
        if let Some(ss) = &sc.palette_set {
            let used = crate::cellmem::part_mem(&d.path, &d.name).map(|p| p.used_slots).ok();
            if crate::cellmem::set_mismatch(&d.path, ss, used.as_ref()).is_none() {
                continue;
            }
        }
        // otherwise only an explicit, checked slot remap makes the art usable here
        let meta = crate::dmanifest::load_meta(&d.path).unwrap_or_default();
        match meta.remap.get(&sc.name) {
            None => v.push(Finding { object: obj.clone(), part: Some(d.name.clone()), kind: "borrowed", message: format!("object {} ({}x): part {} is not drawn for zone {} (slots mean different colours per zone); draw a zone variant or add `pxl meta {} zone-remap {} 4=5`", obj, n, d.name, sc.name, d.name, sc.name), count: n }),
            Some(remap) => {
                let used = crate::cellmem::part_mem(&d.path, &d.name).map(|p| p.used_slots).unwrap_or_default();
                let mut missing: Vec<u8> = Vec::new();
                let mut dead: Vec<(u8, u8)> = Vec::new();
                for s in used {
                    let to = remap.get(&s).copied().unwrap_or(s);
                    if !remap.contains_key(&s) && sc.palette_set.is_some() && crate::cellmem::set_mismatch(&d.path, sc.palette_set.as_ref().unwrap(), Some(&[s].into_iter().collect())).is_some() {
                        missing.push(s);
                    }
                    if set.as_ref().map_or(true, |st| st.slots.get(to as usize).map_or(true, |x| x.is_none())) {
                        dead.push((s, to));
                    }
                }
                if !missing.is_empty() {
                    v.push(Finding { object: obj.clone(), part: Some(d.name.clone()), kind: "remap_incomplete", message: format!("object {}: the remap of {} for zone {} does not cover slot(s) {:?} that differ in this zone", obj, d.name, sc.name, missing), count: n });
                }
                for (s, to) in dead {
                    v.push(Finding { object: obj.clone(), part: Some(d.name.clone()), kind: "remap_dead", message: format!("object {}: slot {} of {} maps to slot {} which zone {}'s palette set does not define", obj, s, d.name, to, sc.name), count: n });
                }
            }
        }
    }
    v
}

/// Findings of every scene with a layout, as `check` messages.
pub fn issues(m: &Manifest) -> Vec<String> {
    let mut out = Vec::new();
    for sc in &m.scenes {
        match load(m, sc) {
            Ok(Some(l)) => out.extend(completeness(m, sc, &l).into_iter().map(|f| format!("zone completeness, scene {}: {}", sc.name, f.message))),
            Ok(None) => {}
            Err(e) => out.push(format!("scene {}: {}", sc.name, e.msg)),
        }
    }
    out
}

struct Inst {
    part: String,
    x: i32,
    y: i32,
    comp: scene::Comp,
}

/// Compose the real screens of the layout: per act the worst window for sprites per line and per frame.
pub fn run_layout(a: &SceneArgs, m: &Manifest, sc: &Scene, sp: &Profile, out: &mut Out) -> Result<()> {
    let l = load(m, sc)?.ok_or_else(|| PxlError::usage(format!("scene {} has no layout", sc.name)).with_hint("pxl scene set SCENE layout layouts/zone_1.json"))?;
    let finds = completeness(m, sc, &l);
    let (sw, sh) = (160i32, 144i32);
    let mut comps: BTreeMap<String, Option<scene::Comp>> = BTreeMap::new();
    let mut skipped: Vec<String> = Vec::new();
    let mut js_acts = Vec::new();
    let mut over = 0;
    for act in l.acts() {
        if a.act.as_ref().map_or(false, |x| *x != act) {
            continue;
        }
        let mut insts: Vec<Inst> = Vec::new();
        for p in l.placed.iter().filter(|p| p.act == act) {
            let pn = l.part_of(&p.object);
            let c = comps.entry(pn.clone()).or_insert_with(|| match scene::comps_of(m, &pn, &pn) {
                Ok(v) => scene::widest(v),
                Err(e) => {
                    skipped.push(format!("{}: {}", pn, e.msg));
                    None
                }
            });
            if let Some(c) = c {
                insts.push(Inst { part: pn, x: p.x, y: p.y, comp: scene::Comp { label: c.label.clone(), part: c.part.clone(), frame: c.frame.clone(), width: c.width, sprites: c.sprites, slots: c.slots.clone(), loads: c.loads.clone(), bottom: c.bottom, w: c.w, h: c.h, rgb: c.rgb.clone() } });
            }
        }
        if insts.is_empty() {
            continue;
        }
        let (xmin, xmax) = (insts.iter().map(|i| i.x).min().unwrap(), insts.iter().map(|i| i.x).max().unwrap());
        let (ymin, ymax) = (insts.iter().map(|i| i.y).min().unwrap(), insts.iter().map(|i| i.y).max().unwrap());
        let mut worst = (0u32, 0i32, 0i32, Vec::<String>::new());
        let mut worst_oam = (0u32, 0i32, 0i32);
        let mut cx = xmin - sw / 2;
        while cx <= xmax + sw / 2 {
            let inw: Vec<&Inst> = insts.iter().filter(|i| i.x + i.comp.width as i32 / 2 > cx && i.x - i.comp.width as i32 / 2 < cx + sw).collect();
            if !inw.is_empty() {
                let mut line: BTreeMap<i32, (u32, Vec<String>)> = BTreeMap::new();
                for i in &inw {
                    for (d, ld) in &i.comp.loads {
                        let e = line.entry(i.y - d).or_default();
                        e.0 += ld;
                        if !e.1.contains(&i.part) {
                            e.1.push(i.part.clone());
                        }
                    }
                }
                for (y, (n, who)) in line {
                    if n > worst.0 {
                        worst = (n, cx, y, who);
                    }
                }
                // sprites in one camera frame: the densest 144 px band
                let mut cy = ymin - sh;
                while cy <= ymax {
                    let n: u32 = inw.iter().filter(|i| i.y >= cy && i.y - i.comp.h as i32 <= cy + sh).map(|i| i.comp.sprites).sum();
                    if n > worst_oam.0 {
                        worst_oam = (n, cx, cy);
                    }
                    cy += 16;
                }
            }
            cx += 8;
        }
        let ok_line = sp.hw_cells_per_row_max == 0 || worst.0 <= sp.hw_cells_per_row_max;
        let ok_oam = sp.hw_cells_max == 0 || worst_oam.0 <= sp.hw_cells_max;
        if !ok_line || !ok_oam {
            over += 1;
        }
        out.line(format!("act {}: {} objects; worst scanline {} sprites (limit {}) {} at camera x {} y {} [{}]; worst frame {} sprites (limit {}) {}", act, insts.len(), worst.0, sp.hw_cells_per_row_max, if ok_line { "ok" } else { "OVER" }, worst.1, worst.2, worst.3.join(", "), worst_oam.0, sp.hw_cells_max, if ok_oam { "ok" } else { "OVER" }));
        js_acts.push(json!({"act": act, "objects": insts.len(), "worst_line": {"sprites": worst.0, "limit": sp.hw_cells_per_row_max, "camera_x": worst.1, "y": worst.2, "parts": worst.3, "ok": ok_line}, "worst_frame": {"sprites": worst_oam.0, "limit": sp.hw_cells_max, "camera_x": worst_oam.1, "camera_y": worst_oam.2, "ok": ok_oam}}));
        if let Some(p) = &a.png {
            // the worst window: objects drawn at their positions, a bar per line
            let z = a.zoom.unwrap_or(3).max(1);
            let cam = (worst.1, worst.2 - sh / 2);
            let mut im = crate::img::RgbaImage::filled(sw as usize * z, sh as usize * z, crate::compare::BG);
            for i in &insts {
                for (k, px) in i.comp.rgb.iter().enumerate() {
                    let Some(c) = px else { continue };
                    let (px_x, px_y) = (i.x - i.comp.w as i32 / 2 + (k % i.comp.w) as i32 - cam.0, i.y - (i.comp.bottom - (k / i.comp.w) as i32) - cam.1);
                    if px_x >= 0 && px_x < sw && px_y >= 0 && px_y < sh {
                        crate::compare::fill(&mut im, px_x as usize * z, px_y as usize * z, z, z, crate::img::Rgba::opaque(c.r, c.g, c.b));
                    }
                }
            }
            let name = if a.act.is_some() { p.clone() } else { format!("{}.act{}.png", p.trim_end_matches(".png"), act) };
            crate::show::write_png(std::path::Path::new(&name), &im)?;
            out.line(format!("  wrote {}", name));
        }
    }
    for f in &finds {
        out.line(format!("zone completeness: {}", f.message));
    }
    for s in &skipped {
        out.line(format!("skipped {}", s));
    }
    out.set("layout", json!({"scene": sc.name, "file": sc.layout, "acts": js_acts, "completeness": finds.iter().map(|f| json!({"object": f.object, "part": f.part, "kind": f.kind, "message": f.message, "count": f.count})).collect::<Vec<_>>(), "skipped": skipped}));
    if over > 0 || !finds.is_empty() {
        return Err(PxlError::lint(format!("scene {}: {} act(s) over the hardware limits, {} completeness finding(s)", sc.name, over, finds.len())));
    }
    Ok(())
}
