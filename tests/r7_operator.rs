//! M6 R7: operator and design-language tool support. `pxl intake`, `pxl style analyze | propose | approve | mark`, the quality gates
//! (eye_parity, identity_colour, flat, consistency, readability), the reviewer checklist, milestones and gates, decisions with options,
//! `pxl report`, `deliver --dry-run`, and the end-to-end scenario B.
mod e2e_common;

use agentsprite::img::{Rgba, RgbaImage};
use agentsprite::png;
use e2e_common::*;
use std::path::{Path, PathBuf};

fn ok(o: &std::process::Output) -> String {
    assert!(o.status.success(), "{}{}", stdout(o), stderr(o));
    stdout(o)
}

fn fails(o: &std::process::Output) -> String {
    assert!(!o.status.success(), "expected a failure: {}", stdout(o));
    format!("{}{}", stdout(o), stderr(o))
}

const OUT: (u8, u8, u8) = (16, 24, 40);
const SKIN: (u8, u8, u8) = (240, 176, 144);
const BODY: (u8, u8, u8) = (48, 80, 200);
const BODY_D: (u8, u8, u8) = (28, 44, 128);
const RED: (u8, u8, u8) = (224, 32, 32);
const WHITE: (u8, u8, u8) = (255, 255, 255);

fn put(g: &mut Vec<Vec<Option<(u8, u8, u8)>>>, x: i32, y: i32, c: (u8, u8, u8)) {
    if x >= 0 && y >= 0 && (y as usize) < g.len() && (x as usize) < g[0].len() {
        g[y as usize][x as usize] = Some(c);
    }
}

/// A 16x24 character at scale 4 (a native frame of 64x96): outline, skin head with two white eyes (2x3 white, 2 pupil cells), blue body,
/// red shoes. `eyes` = 2 draws both, 1 draws one. `shoes` = false leaves them blue.
fn hero(pose: usize, scale: usize, eyes: usize, shoes: bool) -> RgbaImage {
    let (w, h) = (16usize, 24usize);
    let mut g: Vec<Vec<Option<(u8, u8, u8)>>> = vec![vec![None; w]; h];
    for y in 3..9 {
        for x in 4..12 {
            put(&mut g, x, y, SKIN);
        }
    }
    for y in 9..16 {
        for x in 4..12 {
            put(&mut g, x, y, if x < 9 { BODY } else { BODY_D });
        }
    }
    let legs = [(5, 10), (4, 11), (6, 9)][pose % 3];
    for y in 16..21 {
        for x in [legs.0, legs.0 + 1, legs.1, legs.1 + 1] {
            put(&mut g, x, y, BODY_D);
        }
    }
    let shoe = if shoes { RED } else { BODY_D };
    for x in (legs.0 - 1)..(legs.0 + 3) {
        put(&mut g, x, 20, shoe);
        put(&mut g, x, 21, shoe);
    }
    for x in (legs.1 - 1)..(legs.1 + 3) {
        put(&mut g, x, 20, shoe);
        put(&mut g, x, 21, shoe);
    }
    for (n, ex) in [5, 9].iter().enumerate() {
        if n >= eyes {
            continue;
        }
        for y in 4..7 {
            put(&mut g, *ex, y, WHITE);
            put(&mut g, *ex + 1, y, WHITE);
        }
        put(&mut g, *ex + 1, 5, OUT);
        put(&mut g, *ex + 1, 6, OUT);
    }
    let src = g.clone();
    for y in 0..h {
        for x in 0..w {
            if src[y][x].is_none() && [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|(dx, dy)| {
                let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h && src[ny as usize][nx as usize].is_some()
            }) {
                g[y][x] = Some(OUT);
            }
        }
    }
    let mut im = RgbaImage::new(w * scale, h * scale);
    for y in 0..h * scale {
        for x in 0..w * scale {
            if let Some(c) = g[y / scale][x / scale] {
                im.px[y * w * scale + x] = Rgba::opaque(c.0, c.1, c.2);
            }
        }
    }
    im
}

fn bug(scale: usize) -> RgbaImage {
    let (w, h) = (16usize, 16usize);
    let mut im = RgbaImage::new(w * scale, h * scale);
    for y in 3..13usize {
        for x in 2..14usize {
            let d = ((x as f64 - 8.0) / 6.5).powi(2) + ((y as f64 - 8.0) / 5.5).powi(2);
            if d <= 1.0 {
                let c = if d > 0.8 { OUT } else if y < 8 { RED } else { (144, 16, 16) };
                for dy in 0..scale {
                    for dx in 0..scale {
                        im.px[(y * scale + dy) * w * scale + x * scale + dx] = Rgba::opaque(c.0, c.1, c.2);
                    }
                }
            }
        }
    }
    for ex in [5usize, 9] {
        for y in [6usize, 7] {
            for x in [ex, ex + 1] {
                for dy in 0..scale {
                    for dx in 0..scale {
                        im.px[(y * scale + dy) * w * scale + x * scale + dx] = Rgba::opaque(255, 255, 255);
                    }
                }
            }
        }
        for dy in 0..scale {
            for dx in 0..scale {
                im.px[(7 * scale + dy) * w * scale + (ex + 1) * scale + dx] = Rgba::opaque(OUT.0, OUT.1, OUT.2);
            }
        }
    }
    im
}

fn bands(w: usize, h: usize, cols: &[(u8, u8, u8)]) -> RgbaImage {
    let mut im = RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let c = cols[y * cols.len() / h];
            im.px[y * w + x] = Rgba::opaque(c.0, c.1, c.2);
        }
    }
    im
}

fn save(p: &Path, im: &RgbaImage) {
    png::write_rgba(p, im).unwrap();
}

/// A project (gates on unless `gates` is false) with the hero (3 poses), a bug, a tile region and a screen.
fn project(tag: &str, gates: bool) -> PathBuf {
    let d = tmpdir(tag);
    let mut args = vec!["new", "proj", "--console", "gbc", "--name", "Demo"];
    if !gates {
        args.push("--no-gates");
    }
    ok(&pxl_in(&d, &args));
    let p = d.join("proj");
    for i in 0..3 {
        save(&p.join(format!("inbox/hero_{:02}.png", i + 1)), &hero(i, 4, 2, true));
    }
    ok(&pxl_in(&p, &["ingest", "inbox", "--group", "player", "--factor", "4"]));
    save(&p.join("inbox/bug.png"), &bug(4));
    ok(&pxl_in(&p, &["ingest", "inbox/bug.png", "--group", "enemies", "--factor", "4"]));
    save(&p.join("inbox/ground.png"), &bands(64, 32, &[(64, 160, 64), (48, 104, 48), (138, 90, 42), (106, 64, 24)]));
    ok(&pxl_in(&p, &["ingest", "inbox/ground.png", "--group", "zone", "--mode", "tile", "--factor", "2"]));
    save(&p.join("inbox/title.png"), &bands(160, 144, &[(128, 192, 240), (128, 192, 240), (64, 160, 64), (138, 90, 42)]));
    ok(&pxl_in(&p, &["ingest", "inbox/title.png", "--group", "screens", "--mode", "screen", "--factor", "1"]));
    p
}

/// Replace a key in the grid lines of an asset text only (a grid line has no spaces, quotes or equals signs).
fn replace_in_grids(text: &str, from: char, to: char) -> String {
    text.lines()
        .map(|l| if l.len() >= 8 && !l.contains(|c: char| " =\"[]#,".contains(c)) { l.replace(from, &to.to_string()) } else { l.to_string() })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn grid_of(p: &Path, asset: &str) -> String {
    std::fs::read_to_string(p.join(asset).join("asset.pxl")).unwrap()
}

// ------------------------------------------------------------------------------------------ intake

#[test]
fn consoles_are_found_in_a_request_longest_name_first() {
    use agentsprite::intake::consoles_in;
    assert_eq!(consoles_in("Please port it to the Game Boy Color, later the NES"), vec!["gbc", "nes"]);
    assert_eq!(consoles_in("a classic game boy cartridge"), vec!["gb"]);
    assert_eq!(consoles_in("for SNES and Genesis"), vec!["snes", "md"]);
    assert!(consoles_in("it is a game about gbcs and genesisx").is_empty());
}

#[test]
fn intake_is_non_interactive_writes_the_brief_and_records_assumptions() {
    let p = project("r7_intake", true);
    let o = pxl_in(&p, &["intake", "--request", "Make the hero and the bug for the Game Boy Color, faithful to the source. Send progress weekly. Deadline 2026-12-01."]);
    let t = ok(&o);
    assert!(t.contains("console = gbc") && t.contains("fidelity = faithful") && t.contains("cadence = weekly") && t.contains("deadline = 2026-12-01"), "{}", t);
    assert!(t.contains("questions: none"), "everything was inferred: {}", t);
    // the identity list: eyes and the red shoes
    assert!(t.contains("identity hero: 2 eye(s) with white and pupil; colours #") && t.contains("#e02020 (red)") && t.contains("#3050c8 (blue)"), "{}", t);
    let brief = std::fs::read_to_string(p.join("brief.toml")).unwrap();
    assert!(brief.contains("fidelity = \"faithful\"") && brief.contains("[identity.hero]") && brief.contains("eyes = 2") && brief.contains("\"#e02020\""), "{}", brief);
    assert!(brief.contains("exceptions = \"never\"") && brief.contains("[assumptions]"), "{}", brief);
    let dec = std::fs::read_to_string(p.join("decisions.md")).unwrap();
    assert!(dec.contains("[operator] (assumption console)") && dec.contains("[operator] (assumption tradeoff)"), "{}", dec);
    // reversible with one command, and a rerun keeps what was confirmed
    ok(&pxl_in(&p, &["decide", "the client wants it redrawn", "--set", "fidelity=reinterpreted", "--by", "user"]));
    assert!(std::fs::read_to_string(p.join("brief.toml")).unwrap().contains("fidelity = \"reinterpreted\""));
    let again = ok(&pxl_in(&p, &["intake", "--request", "faithful please"]));
    assert!(again.contains("fidelity = reinterpreted") && again.contains("confirmed earlier"), "{}", again);
    let dec = std::fs::read_to_string(p.join("decisions.md")).unwrap();
    assert_eq!(dec.matches("(assumption fidelity)").count(), 1, "one line per item: {}", dec);
    assert!(dec.contains("[user] (assumption fidelity)"));
    // a bad value is refused
    assert!(fails(&pxl_in(&p, &["decide", "x", "--set", "fidelity=loud"])).contains("fidelity is one of"));
    assert!(fails(&pxl_in(&p, &["decide", "x", "--set", "colour=red"])).contains("unknown brief item"));
}

#[test]
fn intake_asks_only_real_questions_with_options_and_a_recommendation() {
    let p = project("r7_intake_q", true);
    let o = pxl_in(&p, &["intake", "--request", "This is for the NES, in a faithful but also reinterpreted style"]);
    let t = ok(&o);
    assert!(t.contains("questions (2)") && t.contains("which console is the target") && t.contains("style direction"), "{}", t);
    assert!(t.contains("recommend B") && t.contains("recommend A"), "{}", t);
    let j = json_of(&pxl_in(&p, &["decisions", "--json"]));
    assert_eq!(j["decisions"].as_array().unwrap().len(), 2);
    let d = &j["decisions"][0];
    assert!(d["page"].as_str().unwrap().ends_with("d0001.html") && std::path::Path::new(d["page"].as_str().unwrap()).is_file());
    // asking twice does not repeat the question
    let t2 = ok(&pxl_in(&p, &["intake", "--request", "This is for the NES, in a faithful but also reinterpreted style"]));
    assert!(t2.contains("questions: none"), "{}", t2);
    // answering closes it
    ok(&pxl_in(&p, &["decide", "d0001", "A", "--by", "user"]));
    assert!(ok(&pxl_in(&p, &["decisions"])).contains("1 decision(s) open"));
}

#[test]
fn intake_interview_and_answers_are_opt_in() {
    let p = project("r7_interview", true);
    ok(&pxl_in(&p, &["intake", "--request", "gbc"]));
    // --answers FILE: the user's decisions
    std::fs::write(p.join("answers.toml"), "cadence = \"daily\"\nexceptions = \"ask\"\ntradeoff = [\"fold colours\", \"cut animation steps\"]\n").unwrap();
    ok(&pxl_in(&p, &["intake", "--request", "gbc", "--answers", "answers.toml"]));
    let b = std::fs::read_to_string(p.join("brief.toml")).unwrap();
    assert!(b.contains("cadence = \"daily\"") && b.contains("exceptions = \"ask\"") && b.contains("\"fold colours\",\n    \"cut animation steps\"") || b.contains("tradeoff = [\"fold colours\", \"cut animation steps\"]"), "{}", b);
    // --interview reads stdin: console (keep), fidelity (change), the rest empty
    let o = pxl_stdin(&p, &["intake", "--request", "gbc", "--interview"], "\nreinterpreted\n\n\n\n\n");
    ok(&o);
    assert!(std::fs::read_to_string(p.join("brief.toml")).unwrap().contains("fidelity = \"reinterpreted\""));
}

// ------------------------------------------------------------------------------------------ style

#[test]
fn style_analyze_measures_the_input_and_propose_writes_rules_with_limit_impacts() {
    let p = project("r7_style", true);
    ok(&pxl_in(&p, &["intake", "--request", "gbc"]));
    let a = json_of(&pxl_in(&p, &["style", "analyze", "--json"]));
    let hero_m = a["analysis"]["assets"].as_array().unwrap().iter().find(|x| x["asset"] == "hero").unwrap().clone();
    assert!(hero_m["outline"]["presence"].as_f64().unwrap() > 0.9, "{}", hero_m);
    assert_eq!(hero_m["outline"]["colour"], "#101828");
    assert_eq!(hero_m["eyes"]["max_per_frame"], 2);
    assert_eq!(hero_m["factor"], 4);
    assert!(hero_m["identity"].as_array().unwrap().iter().any(|c| c["colour"] == "#e02020"), "red shoes are an identity colour: {}", hero_m["identity"]);
    assert_eq!(hero_m["dither"].as_f64().unwrap(), 0.0);
    assert!(p.join(".pxl/style_analysis.json").is_file());
    let t = ok(&pxl_in(&p, &["style", "propose"]));
    assert!(t.contains("outline, eyes, identity, ramps, light, white, highlight, dither, fold, readability"), "{}", t);
    let md = std::fs::read_to_string(p.join("style.md")).unwrap();
    for needle in [
        "Status: proposed",
        "## The input style (measured)",
        "## The limits and what they do to this style",
        "- **outline**: keep the outline",
        "- **eyes**: every face keeps its eyes",
        "white 2x3 cells",
        "`hero`: 2 eye(s) with white and pupil; colours #",
        "#e02020 (red)",
        "3 colours per palette line",
        "factor 4",
        "one palette per 8x16 cell",
        "10 sprite cell(s) per scanline",
        "- **white**: white is only for highlights and eye whites",
        "- **fold**: when the hardware does not fit, apply in this order: cut animation steps",
        "## Shared material ramps",
    ] {
        assert!(md.contains(needle), "style.md lacks {:?}:\n{}", needle, md);
    }
    // approving is a milestone; editing the rules afterwards clears it; proposing over an approved style needs --force
    assert!(ok(&pxl_in(&p, &["milestone", "status"])).contains("[ ] style approved"));
    ok(&pxl_in(&p, &["style", "approve", "--by", "user"]));
    assert!(std::fs::read_to_string(p.join("style.md")).unwrap().contains("Status: approved by user on "));
    let ms = json_of(&pxl_in(&p, &["milestone", "status", "--json"]));
    assert_eq!(ms["style_approved"]["style_md"]["ok"], true);
    assert_eq!(ms["style_approved"]["ok"], false, "the style frames are not approved yet");
    let again = fails(&pxl_in(&p, &["style", "propose"]));
    assert!(again.contains("approved") && again.contains("--force"), "{}", again);
    let md = std::fs::read_to_string(p.join("style.md")).unwrap();
    std::fs::write(p.join("style.md"), md.replace("never merge an eye into the outline", "merge an eye into the outline")).unwrap();
    assert!(ok(&pxl_in(&p, &["style", "show"])).contains("changed since it was approved"));
    ok(&pxl_in(&p, &["style", "propose", "--force"]));
}

#[test]
fn the_game_boy_style_says_white_is_active_and_the_limits_name_the_greys() {
    let d = tmpdir("r7_style_gb");
    ok(&pxl_in(&d, &["new", "gbproj", "--console", "gb", "--name", "GB"]));
    let p = d.join("gbproj");
    save(&p.join("inbox/bug.png"), &bug(4));
    ok(&pxl_in(&p, &["ingest", "inbox/bug.png", "--group", "enemies", "--factor", "4"]));
    ok(&pxl_in(&p, &["style", "propose"]));
    let md = std::fs::read_to_string(p.join("style.md")).unwrap();
    assert!(md.contains("white is a main tone on this console"), "{}", md);
    assert!(md.contains("fixed greys") && md.contains("white is a main tone, not a highlight"), "{}", md);
}

#[test]
fn style_frames_are_marked_and_shown_next_to_every_asset() {
    let p = project("r7_frames", true);
    ok(&pxl_in(&p, &["style", "mark", "hero", "hero_01", "hero_02"]));
    let s = json_of(&pxl_in(&p, &["style", "show", "--json"]));
    assert_eq!(s["style_frames"][0]["asset"], "hero");
    assert_eq!(s["style_frames"][0]["frames"][1], "hero_02");
    assert!(fails(&pxl_in(&p, &["style", "mark", "hero", "nope"])).contains("frame"));
    // the nearest style frame: same group first, else the same mode
    ok(&pxl_in(&p, &["set", "bug", "bug", "3,3=1", "--no-version"]));
    ok(&pxl_in(&p, &["task", "add", "bug", "look again"]));
    let html = std::fs::read_to_string(p.join("assets/enemies/bug/compare.html")).unwrap();
    assert!(html.contains("Nearest style frame: hero") && html.contains("../../player/hero/compare.png"), "{}", html);
    // the hero's own page does not name itself
    ok(&pxl_in(&p, &["task", "add", "hero", "look"]));
    assert!(!std::fs::read_to_string(p.join("assets/player/hero/compare.html")).unwrap().contains("Nearest style frame"));
    ok(&pxl_in(&p, &["style", "mark", "hero", "--remove"]));
    assert!(ok(&pxl_in(&p, &["style", "show"])).contains("style frames: none"));
}

// ------------------------------------------------------------------------------------------ the lint gates

/// Write a clean first version to the asset by hand so the tests do not depend on the automatic palette fit: the grids are replaced.
fn lint_rules(p: &Path, asset: &str, rules: &str) -> serde_json::Value {
    json_of(&pxl_in(p, &["lint", asset, "--rules", rules, "--json"]))
}

fn issues(j: &serde_json::Value, rule: &str) -> Vec<serde_json::Value> {
    j["parts"][0]["issues"].as_array().unwrap().iter().filter(|i| i["rule"] == rule).cloned().collect()
}

#[test]
fn eye_parity_counts_the_eyes_of_every_frame_against_the_source() {
    let p = project("r7_eye", false);
    // the first version keeps the eyes
    let j = lint_rules(&p, "hero", "eye_parity");
    assert!(issues(&j, "eye_parity").is_empty(), "{}", j);
    // wipe one eye of hero_02 into skin
    let g = grid_of(&p, "assets/player/hero");
    let skin = p.join("assets/player/hero/template.toml");
    let tpl = std::fs::read_to_string(&skin).unwrap();
    let skin_key = tpl.lines().find(|l| l.contains("\"#f0b090\"") && l.contains("color")).and_then(|l| l.trim().chars().nth(1)).unwrap();
    assert!(g.contains("hero_02"));
    ok(&pxl_in(&p, &["set", "hero", "hero_02", &format!("5,4={k}", k = skin_key), &format!("6,4={k}", k = skin_key), &format!("5,5={k}", k = skin_key), &format!("6,5={k}", k = skin_key), &format!("5,6={k}", k = skin_key), &format!("6,6={k}", k = skin_key)]));
    let j = lint_rules(&p, "hero", "eye_parity");
    let es = issues(&j, "eye_parity");
    assert_eq!(es.len(), 1, "{}", j);
    assert_eq!(es[0]["frame"], "hero_02");
    assert_eq!(es[0]["severity"], "warning");
    assert!(es[0]["message"].as_str().unwrap().contains("source frame shows 2 eye(s)"), "{}", es[0]);
    // a palette-set part has no ready-made stamp (its keys are the set colours); undoing the wipe is the repair here
    ok(&pxl_in(&p, &["undo", "hero"]));
    assert!(issues(&lint_rules(&p, "hero", "eye_parity"), "eye_parity").is_empty());
    // the bug's single-frame asset has two eyes, too
    let j = lint_rules(&p, "bug", "eye_parity");
    assert!(issues(&j, "eye_parity").is_empty(), "{}", j);
}

#[test]
fn identity_colour_flags_a_colour_the_source_has_and_the_art_lost() {
    let p = project("r7_ident", false);
    ok(&pxl_in(&p, &["intake", "--request", "gbc"]));
    // the first version keeps the red shoes (it may fold the skin tone into the white: that is a tone finding of its own, not the shoes)
    let j0 = lint_rules(&p, "hero", "identity_colour");
    assert!(!issues(&j0, "identity_colour").iter().any(|i| i["message"].as_str().unwrap().starts_with("identity colour #e02020")), "{}", j0);
    // replace the red shoes (the identity colour) by the body colour in every frame of the grid text
    let f = p.join("assets/player/hero/asset.pxl");
    let t = std::fs::read_to_string(&f).unwrap();
    let tpl = std::fs::read_to_string(p.join("assets/player/hero/template.toml")).unwrap();
    let red_key = tpl.lines().find(|l| l.contains("\"#e02020\"") && l.contains("color")).and_then(|l| l.trim().chars().nth(1)).unwrap();
    let new_key = tpl.lines().find(|l| l.contains("\"#3050c8\"") && l.contains("color")).and_then(|l| l.trim().chars().nth(1)).unwrap();
    std::fs::write(&f, replace_in_grids(&t, red_key, new_key)).unwrap();
    let j = lint_rules(&p, "hero", "identity_colour");
    let is = issues(&j, "identity_colour");
    let shoes: Vec<&serde_json::Value> = is.iter().filter(|i| i["message"].as_str().unwrap().starts_with("identity colour #e02020")).collect();
    assert!(!shoes.is_empty(), "{}", j);
    assert!(shoes[0]["message"].as_str().unwrap().contains("red") && shoes[0]["message"].as_str().unwrap().contains("hue family"), "{}", shoes[0]);
}

#[test]
fn flat_flags_a_material_that_lost_its_tones_and_a_wrong_hue_block() {
    let p = project("r7_flat", false);
    // the automatic version folds the bug's two reds (light and dark) into one cell colour
    let j = lint_rules(&p, "bug", "flat");
    let fs = issues(&j, "flat");
    assert!(fs.iter().any(|i| i["message"].as_str().unwrap().contains("lost structure") && i["message"].as_str().unwrap().contains("red")), "{}", j);
}

#[test]
fn consistency_and_readability_rules_report_with_a_frame_and_numbers() {
    let p = project("r7_cons", false);
    // a colour the character has in 2 of 3 frames and not in the third: remove the shoes from the last frame
    let f = p.join("assets/player/hero/asset.pxl");
    let tpl = std::fs::read_to_string(p.join("assets/player/hero/template.toml")).unwrap();
    let red_key = tpl.lines().find(|l| l.contains("\"#e02020\"") && l.contains("color")).and_then(|l| l.trim().chars().nth(1)).unwrap();
    let body_key = tpl.lines().find(|l| l.contains("\"#3050c8\"") && l.contains("color")).and_then(|l| l.trim().chars().nth(1)).unwrap();
    let t = std::fs::read_to_string(&f).unwrap();
    let cut = t.find("name = \"hero_03\"").unwrap();
    std::fs::write(&f, format!("{}{}", &t[..cut], replace_in_grids(&t[cut..], red_key, body_key))).unwrap();
    let j = lint_rules(&p, "hero", "consistency");
    let cs = issues(&j, "consistency");
    assert!(cs.iter().any(|i| i["frame"] == "hero_03" && i["message"].as_str().unwrap().contains("in 2 of 3 frames but not in this one")), "{}", j);
    // readability: a figure of two almost identical dark colours has no value spread
    let q = project("r7_read", false);
    let qf = q.join("assets/enemies/bug/asset.pxl");
    let qt = std::fs::read_to_string(&qf).unwrap();
    let flat: String = qt.lines().map(|l| if l.chars().all(|c| matches!(c, '.' | '1'..='9')) && l.len() == 16 { l.chars().enumerate().map(|(i, c)| if c == '.' { '.' } else if i % 2 == 0 { '1' } else { '2' }).collect::<String>() } else { l.to_string() }).collect::<Vec<_>>().join("\n");
    std::fs::write(&qf, flat).unwrap();
    // keys 1 and 2 become two almost identical dark colours
    let qtp = q.join("assets/enemies/bug/template.toml");
    let qtpl: String = std::fs::read_to_string(&qtp).unwrap().lines().map(|l| if l.trim_start().starts_with("\"2\" =") && l.contains("color = \"#") { let i = l.find("color = \"#").unwrap() + 10; format!("{}181c38{}", &l[..i], &l[i + 6..]) } else if l.trim_start().starts_with("\"1\" =") && l.contains("color = \"#") { let i = l.find("color = \"#").unwrap() + 10; format!("{}101830{}", &l[..i], &l[i + 6..]) } else { l.to_string() }).collect::<Vec<_>>().join("\n");
    std::fs::write(&qtp, qtpl).unwrap();
    let j = lint_rules(&q, "bug", "readability");
    assert!(issues(&j, "readability").iter().any(|i| i["message"].as_str().unwrap().contains("squint test")), "{}", j);
}

#[test]
fn the_gates_lint_rules_run_by_default_only_in_gated_projects() {
    let g = project("r7_gated", true);
    let ng = project("r7_ungated", false);
    let jg = json_of(&pxl_in(&g, &["lint", "bug", "--json"]));
    let jn = json_of(&pxl_in(&ng, &["lint", "bug", "--json"]));
    assert!(!issues(&jg, "flat").is_empty(), "gated: {}", jg);
    assert!(issues(&jn, "flat").is_empty() && issues(&jn, "eye_parity").is_empty() && jn["summary"]["warnings"] == 0, "ungated projects lint as before: {}", jn);
    assert!(fails(&pxl_in(&g, &["lint", "bug", "--rules", "nope"])).contains("eye_parity"), "unknown rules list the design rules");
}

// ------------------------------------------------------------------------------------------ review checklist, rule ids, approval gate

#[test]
fn the_checklist_comes_from_style_md_and_notes_cite_a_rule() {
    let p = project("r7_check", true);
    ok(&pxl_in(&p, &["intake", "--request", "gbc"]));
    assert!(fails(&pxl_in(&p, &["review", "checklist", "hero"])).contains("style.md"));
    ok(&pxl_in(&p, &["style", "propose"]));
    ok(&pxl_in(&p, &["style", "mark", "hero"]));
    let t = ok(&pxl_in(&p, &["review", "checklist", "bug"]));
    assert!(t.contains("[ ] eyes:") && t.contains("[ ] outline:") && t.contains("design gates:") && t.contains("nearest style frame: hero"), "{}", t);
    let j = json_of(&pxl_in(&p, &["review", "checklist", "bug", "--json"]));
    assert!(j["rules"].as_array().unwrap().iter().any(|r| r["rule"] == "eyes"));
    // a note quotes the rule it enforces
    let n = pxl_in(&p, &["review", "note", "bug", "bug", "the right eye has no pupil", "--region", "9,6,2,2", "--rule", "eyes", "--agent", "rev"]);
    ok(&n);
    assert!(std::fs::read_to_string(p.join("assets/enemies/bug/review/note_001.md")).unwrap().contains("rule eyes"));
    assert!(ok(&pxl_in(&p, &["tasks"])).contains("[rule eyes]"));
    let bad = fails(&pxl_in(&p, &["review", "note", "bug", "bug", "text", "--region", "9,6,2,2", "--rule", "glitter", "--agent", "rev"]));
    assert!(bad.contains("no rule 'glitter'") && bad.contains("eyes"), "{}", bad);
}

#[test]
fn approval_is_blocked_by_the_gates_unless_the_reviewer_justifies_it() {
    let p = project("r7_approve", true);
    let b = fails(&pxl_in(&p, &["review", "approve", "bug", "--agent", "rev"]));
    assert!(b.contains("approval blocked by") && b.contains("flat") && b.contains("--justify"), "{}", b);
    // plain approve is gated the same way
    assert!(fails(&pxl_in(&p, &["approve", "bug", "--agent", "rev"])).contains("approval blocked"));
    ok(&pxl_in(&p, &["review", "approve", "bug", "--agent", "rev", "--justify", "the red shell is folded on purpose, the cell holds three colours"]));
    let st = std::fs::read_to_string(p.join("assets/enemies/bug/asset.status.json")).unwrap();
    assert!(st.contains("justified design warnings (") && st.contains("flat") && st.contains("folded on purpose"), "{}", st);
    // an ungated project approves without it
    let q = project("r7_approve_ng", false);
    ok(&pxl_in(&q, &["review", "approve", "bug", "--agent", "rev"]));
}

// ------------------------------------------------------------------------------------------ decisions

#[test]
fn ask_opens_a_decision_with_an_image_page_and_tasks_wait_for_it() {
    let p = project("r7_ask", false);
    let img = p.join("assets/player/hero/compare.png");
    assert!(fails(&pxl_in(&p, &["ask", "moustache", "--option", "A=x"])).contains("--recommend"));
    assert!(fails(&pxl_in(&p, &["ask", "moustache", "--option", "A=x", "--recommend", "A", "why"])).contains("at least two options"));
    assert!(fails(&pxl_in(&p, &["ask", "moustache", "--option", "A=x", "--option", "B=y", "--recommend", "C", "why"])).contains("not one of the options"));
    let i = img.display().to_string();
    let t = ok(&pxl_in(&p, &["ask", "hero skin tone", "--option", &format!("A={}|warm, like the source", i), "--option", &format!("B={}|cooler", i), "--recommend", "A", "it is the colour in the source", "--asset", "hero"]));
    assert!(t.contains("d0001 opened") && t.contains("recommend A"), "{}", t);
    let page = std::fs::read_to_string(p.join("review/decisions/d0001.html")).unwrap();
    assert!(page.contains("hero skin tone") && page.contains("A (recommended)") && page.contains("d0001/A.png") && page.contains("warm, like the source"), "{}", page);
    assert!(p.join("review/decisions/d0001/A.png").is_file() && p.join("review/decisions/d0001/B.png").is_file());
    // a task after the decision stays blocked: it is listed, not handed out
    ok(&pxl_in(&p, &["task", "add", "hero", "recolour the skin per the decision", "--after", "d0001"]));
    assert!(fails(&pxl_in(&p, &["task", "add", "hero", "x", "--after", "d9999"])).contains("no such decision"));
    let ts = ok(&pxl_in(&p, &["tasks"]));
    assert!(ts.contains("blocked until d0001 is decided"), "{}", ts);
    let c = ok(&pxl_in(&p, &["claim", "assets/player/hero/asset.pxl", "--agent", "a1"]));
    assert!(!c.contains("recolour the skin"), "a blocked task is not handed out: {}", c);
    ok(&pxl_in(&p, &["release", "assets/player/hero/asset.pxl", "--agent", "a1"]));
    // deciding: the option must exist
    assert!(fails(&pxl_in(&p, &["decide", "d0001", "Z"])).contains("no option 'Z'"));
    assert!(fails(&pxl_in(&p, &["decide", "d0001"])).contains("needs an option"));
    let d = ok(&pxl_in(&p, &["decide", "d0001", "A", "--by", "user"]));
    assert!(d.contains("d0001 decided: A") && d.contains("unblocked task(s): t0001"), "{}", d);
    assert!(!ok(&pxl_in(&p, &["tasks"])).contains("blocked until"));
    let c2 = ok(&pxl_in(&p, &["claim", "assets/player/hero/asset.pxl", "--agent", "a1"]));
    assert!(c2.contains("recolour the skin"), "{}", c2);
    let page = std::fs::read_to_string(p.join("review/decisions/d0001.html")).unwrap();
    assert!(page.contains("Decided: A"), "{}", page);
    assert!(std::fs::read_to_string(p.join("decisions.md")).unwrap().contains("decided d0001 (hero skin tone): option A - warm, like the source"));
    assert!(fails(&pxl_in(&p, &["decide", "d0001", "B"])).contains("already decided"));
    // an asset whose every open task waits for a decision is not handed out by next
    ok(&pxl_in(&p, &["release", "assets/player/hero/asset.pxl", "--agent", "a1", "--state", "done"]));
}

// ------------------------------------------------------------------------------------------ milestones and gates

#[test]
fn production_starts_after_the_gates_and_until_then_only_style_frames_are_handed_out() {
    let p = project("r7_gates", true);
    // no plan: nothing
    let n = fails(&pxl_in(&p, &["next", "--agent", "a1"]));
    assert!(n.contains("requires a plan"), "{}", n);
    ok(&pxl_in(&p, &["plan", "--apply"]));
    assert!(fails(&pxl_in(&p, &["next", "--agent", "a1"])).contains("not accepted"));
    ok(&pxl_in(&p, &["plan", "accept", "--by", "user", "--decision", "test"]));
    // the plan is accepted but no style frame is marked
    let n = fails(&pxl_in(&p, &["next", "--agent", "a1"]));
    assert!(n.contains("wait for the gates") && n.contains("pxl style mark"), "{}", n);
    let ms = ok(&pxl_in(&p, &["milestone", "status"]));
    assert!(ms.contains("[x] plan accepted") && ms.contains("[ ] style approved") && ms.contains("[ ] dry run passed") && ms.contains("closed"), "{}", ms);
    ok(&pxl_in(&p, &["style", "propose"]));
    ok(&pxl_in(&p, &["style", "mark", "hero"]));
    // only the style frame is handed out; claiming another asset is refused
    let c = fails(&pxl_in(&p, &["claim", "assets/enemies/bug/asset.pxl", "--agent", "a2"]));
    assert!(c.contains("not a style frame"), "{}", c);
    let c = ok(&pxl_in(&p, &["next", "--agent", "a1"]));
    assert!(c.contains("player/hero"), "{}", c);
    ok(&pxl_in(&p, &["release", "assets/player/hero/asset.pxl", "--agent", "a1", "--state", "done"]));
    // the dry run needs the style frames; it passes and is recorded
    let dr = ok(&pxl_in(&p, &["deliver", "--dry-run"]));
    assert!(dr.contains("build: PASS") && dr.contains("verify-package: PASS") && dr.contains("dry run PASSED"), "{}", dr);
    assert!(p.join(".pxl/dryrun/report_images/hero.png").is_file() && p.join(".pxl/dryrun/report.json").is_file());
    assert!(!p.join("deliveries").exists(), "a dry run snapshots nothing");
    ok(&pxl_in(&p, &["style", "approve", "--by", "user"]));
    ok(&pxl_in(&p, &["review", "approve", "hero", "--agent", "rev", "--justify", "automatic first version of the test"]));
    let ms = json_of(&pxl_in(&p, &["milestone", "status", "--json"]));
    assert_eq!(ms["production_open"], true, "{}", ms);
    let c = ok(&pxl_in(&p, &["next", "--agent", "a3"]));
    assert!(c.contains("claimed"), "production is open: {}", c);
    // a changed style frame makes the dry run stale
    ok(&pxl_in(&p, &["set", "hero", "hero_01", "8,8=3", "--force-approved"]));
    assert!(ok(&pxl_in(&p, &["milestone", "status"])).contains("the style frames changed since"));
}

#[test]
fn projects_without_the_key_keep_working_without_gates() {
    let p = project("r7_nogates", false);
    let ms = ok(&pxl_in(&p, &["milestone", "status"]));
    assert!(ms.contains("gates are off in this project"), "{}", ms);
    let c = ok(&pxl_in(&p, &["next", "--agent", "a1"]));
    assert!(c.contains("claimed"), "{}", c);
    let toml = std::fs::read_to_string(p.join("pxl.toml")).unwrap();
    assert!(!toml.contains("gates"));
    let g = project("r7_gatesline", true);
    assert!(std::fs::read_to_string(g.join("pxl.toml")).unwrap().contains("gates = true"));
}

// ------------------------------------------------------------------------------------------ report

#[test]
fn the_report_digests_progress_decisions_budgets_and_follows_the_cadence() {
    let p = project("r7_report", true);
    ok(&pxl_in(&p, &["intake", "--request", "gbc, send progress as work lands"]));
    ok(&pxl_in(&p, &["plan", "--apply"]));
    ok(&pxl_in(&p, &["ask", "palette mood", "--option", "A=vivid grass", "--option", "B=muted grass", "--recommend", "A", "it matches the source"]));
    let t = ok(&pxl_in(&p, &["report", "--html"]));
    for needle in ["MILESTONES", "APPROVED (0)", "IN REVIEW (0)", "NEW IMAGES SINCE THE LAST REPORT (4)", "PENDING DECISIONS (1)", "recommended A because it matches the source", "BUDGET", "main:", "ASSUMPTIONS NOT CONFIRMED YET"] {
        assert!(t.contains(needle), "report lacks {:?}:\n{}", needle, t);
    }
    let html = std::fs::read_to_string(p.join("review/report.html")).unwrap();
    assert!(html.contains("Pending decisions (1)") && html.contains("recommended A") && html.contains("<img src=\"../assets/"), "{}", html);
    // marked as the last report: nothing new, nothing due
    let t2 = ok(&pxl_in(&p, &["report", "--if-due"]));
    assert!(t2.contains("no report due (cadence as_work_lands; 0 new image(s)"), "{}", t2);
    // new work makes one due, a weekly cadence waits
    std::thread::sleep(std::time::Duration::from_millis(1100));
    ok(&pxl_in(&p, &["set", "bug", "bug", "3,3=1"]));
    assert!(ok(&pxl_in(&p, &["report", "--if-due", "--peek"])).contains("Progress report"));
    ok(&pxl_in(&p, &["decide", "weekly please", "--set", "cadence=weekly", "--by", "user"]));
    assert!(ok(&pxl_in(&p, &["report", "--if-due", "--peek"])).contains("no report due (cadence weekly"));
    // board and brief show the assumptions and the open decisions
    let b = ok(&pxl_in(&p, &["board"]));
    assert!(b.contains("open decisions") && b.contains("palette mood") && b.contains("assumptions not confirmed"), "{}", b);
    let br = ok(&pxl_in(&p, &["brief", "bug"]));
    assert!(br.contains("project brief: console gbc") && br.contains("open decision d0001"), "{}", br);
}

// ------------------------------------------------------------------------------------------ scenario B

#[test]
fn scenario_b_a_small_project_from_inbox_to_a_delivery_dry_run() {
    let p = project("r7_scenario_b", true);
    // intake: assumptions, no needless questions
    let t = ok(&pxl_in(&p, &["intake", "--request", "A hero with two eyes, a bug, a ground tile and a title screen for the Game Boy Color, faithful to the source."]));
    assert!(t.contains("console = gbc") && t.contains("questions: none") && t.contains("identity hero: 2 eye(s)"), "{}", t);
    // design language, style frames, plan
    ok(&pxl_in(&p, &["style", "propose"]));
    ok(&pxl_in(&p, &["style", "mark", "hero"]));
    ok(&pxl_in(&p, &["plan", "--apply"]));
    ok(&pxl_in(&p, &["plan", "accept", "--by", "user", "--decision", "scenario B accepts the automatic plan"]));
    // the producer takes the style frame and wrecks one eye by mistake
    let c = ok(&pxl_in(&p, &["next", "--agent", "artist"]));
    assert!(c.contains("player/hero"), "{}", c);
    let tpl = std::fs::read_to_string(p.join("assets/player/hero/template.toml")).unwrap();
    let _ = tpl;
    let g = grid_of(&p, "assets/player/hero");
    // find the key the head uses (the skin or what the palette fit folded it into) from the cell above the eye
    let line = g.lines().skip_while(|l| !l.contains("name = \"hero_01\"")).skip_while(|l| !l.starts_with("grid")).nth(4).unwrap().to_string();
    let head_key = line.chars().nth(4).unwrap();
    let wipe: Vec<String> = ["5,4", "6,4", "5,5", "6,5", "5,6", "6,6"].iter().map(|c| format!("{}={}", c, head_key)).collect();
    let mut args = vec!["set", "hero", "hero_01"];
    args.extend(wipe.iter().map(|s| s.as_str()));
    args.extend(["--agent", "artist"]);
    ok(&pxl_in(&p, &args));
    ok(&pxl_in(&p, &["release", "assets/player/hero/asset.pxl", "--agent", "artist", "--state", "done"]));
    // review: a note with a region and a rule, a fix task
    ok(&pxl_in(&p, &["review", "note", "hero", "hero_01", "the left eye lost its white and pupil", "--region", "4,3,4,5", "--rule", "eyes", "--agent", "reviewer"]));
    let ts = ok(&pxl_in(&p, &["tasks"]));
    assert!(ts.contains("[fix]") && ts.contains("[rule eyes]"), "{}", ts);
    assert!(p.join("assets/player/hero/review/note_001.png").is_file(), "the crop: source | ours");
    // approval is blocked by eye_parity until it is fixed
    let b = fails(&pxl_in(&p, &["review", "approve", "hero", "--agent", "reviewer"]));
    assert!(b.contains("eye_parity [hero_01]"), "{}", b);
    ok(&pxl_in(&p, &["undo", "hero"]));
    // with the eyes back, eye_parity is quiet; the tone gates still speak (the first version folds the skin into the white), so the reviewer
    // either fixes the tones or justifies them in writing
    ok(&pxl_in(&p, &["task", "done", "t0001", "--note", "eye redrawn"]));
    let b = fails(&pxl_in(&p, &["review", "approve", "hero", "--agent", "reviewer"]));
    assert!(b.contains("approval blocked by") && !b.contains("eye_parity [") && b.contains("--justify"), "{}", b);
    ok(&pxl_in(&p, &["review", "approve", "hero", "--agent", "reviewer", "--justify", "the skin tone is folded into the white by the 3-colour palette line"]));
    // a decision with options for the user, and a task that waits for it
    let img = p.join("assets/player/hero/compare.png").display().to_string();
    ok(&pxl_in(&p, &["ask", "ground palette mood", "--option", &format!("A={}|vivid", img), "--option", &format!("B={}|muted", img), "--recommend", "A", "the source is vivid"]));
    ok(&pxl_in(&p, &["task", "add", "ground", "colour the ground per the decision", "--after", "d0001"]));
    // the early dry run, then the gates
    ok(&pxl_in(&p, &["style", "approve", "--by", "user"]));
    let dr = ok(&pxl_in(&p, &["deliver", "--dry-run"]));
    assert!(dr.contains("dry run PASSED"), "{}", dr);
    let ms = json_of(&pxl_in(&p, &["milestone", "status", "--json"]));
    assert_eq!(ms["production_open"], true, "{}", ms);
    ok(&pxl_in(&p, &["decide", "d0001", "A", "--by", "user"]));
    let rep = ok(&pxl_in(&p, &["report"]));
    assert!(rep.contains("APPROVED (1)") && rep.contains("hero") && rep.contains("PENDING DECISIONS (0)"), "{}", rep);
}
