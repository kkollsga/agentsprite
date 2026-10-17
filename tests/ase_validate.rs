use agentsprite::ase::*;
use agentsprite::color::Rgb;

fn good_doc() -> AseDoc {
    let pal = [Rgb::new(0, 0, 0), Rgb::new(255, 0, 0), Rgb::new(0, 255, 0)];
    let mut b = AseBuilder::new_indexed(4, 4, &pal);
    b.add_tileset(7, "ts", 2, 2, 2, vec![0, 0, 0, 0, 1, 1, 1, 1]);
    let a = b.add_layer("a");
    let t = b.add_tilemap_layer("t", 7);
    b.add_frame(100);
    b.add_frame(100);
    let mut c = vec![0u8; 16];
    c[5] = 1;
    b.set_cel_canvas(0, a, &c);
    b.link_cel(1, a, 0);
    b.set_tilemap_cel(0, t, 0, 0, 2, 2, vec![1, 0, 0, 1]);
    b.add_tag("all", 0, 1, LoopDir::Forward, 0);
    b.add_slice(SliceSpec { name: "s".into(), x: 0, y: 0, w: 4, h: 4, center: Some((1, 1, 2, 2)), pivot: Some((0, 0)) });
    b.finish().unwrap()
}

fn msgs(b: &[u8]) -> String {
    validate(b).iter().map(|p| p.to_string()).collect::<Vec<_>>().join("\n")
}

fn expect(bytes: &[u8], needle: &str) {
    let m = msgs(bytes);
    assert!(m.contains(needle), "expected a problem containing {:?}, got:\n{}", needle, m);
}

fn expect_doc(f: impl FnOnce(&mut AseDoc), needle: &str) {
    let mut d = good_doc();
    f(&mut d);
    expect(&write(&d), needle);
}

#[test]
fn baseline_is_clean() {
    assert_eq!(msgs(&write(&good_doc())), "");
}

#[test]
fn header_and_frame_corruptions() {
    let good = write(&good_doc());
    let mut b = good.clone();
    b[4] = 0;
    expect(&b, "header magic");
    expect(&good[..good.len() - 3], "file size");
    expect(&good[..100], "shorter than the 128 byte header");
    expect(&[], "shorter");
    let mut b = good.clone();
    b.push(0);
    expect(&b, "file size");
    let mut b = good.clone();
    b[128 + 4] ^= 0xFF; // frame magic
    expect(&b, "frame magic");
    let mut b = good.clone();
    let fs = u32::from_le_bytes(b[128..132].try_into().unwrap()) + 5;
    b[128..132].copy_from_slice(&fs.to_le_bytes());
    assert!(!validate(&b).is_empty());
    let mut b = good.clone();
    b[128 + 12] = b[128 + 12].wrapping_add(1); // new chunk count
    expect(&b, "new chunk count");
    let mut b = good.clone();
    b[128 + 6] = b[128 + 6].wrapping_add(1); // old chunk count
    expect(&b, "old chunk count");
    let mut b = good.clone();
    b[12..14].copy_from_slice(&7u16.to_le_bytes()); // depth
    expect(&b, "colour depth");
    let mut b = good.clone();
    b[6] = 9; // frame count too large
    assert!(!validate(&b).is_empty());
    let mut b = good.clone();
    b[128 + 16] = 3; // first chunk size < 6
    b[128 + 17] = 0;
    b[128 + 18] = 0;
    b[128 + 19] = 0;
    expect(&b, "chunk size");
    let mut b = good.clone();
    b[100] = 1; // reserved header byte
    expect(&b, "reserved header");
}

#[test]
fn semantic_corruptions() {
    expect_doc(|d| d.frames[0].duration = 0, "duration is 0");
    expect_doc(
        |d| {
            for c in d.frames[0].chunks.iter_mut() {
                if let Chunk::Cel(c) = c {
                    c.layer = 9;
                }
            }
        },
        "refers to layer 9",
    );
    expect_doc(
        |d| {
            for c in d.frames[0].chunks.iter_mut() {
                if let Chunk::Tags(t) = c {
                    t[0].to = 5;
                }
            }
        },
        "ends at frame 5",
    );
    expect_doc(
        |d| {
            for c in d.frames[0].chunks.iter_mut() {
                if let Chunk::Tags(t) = c {
                    t[0].from = 1;
                    t[0].to = 0;
                }
            }
        },
        "reversed",
    );
    expect_doc(
        |d| {
            for c in d.frames[0].chunks.iter_mut() {
                if let Chunk::Palette(p) = c {
                    p.new_size = 2;
                }
            }
        },
        "palette range",
    );
    expect_doc(|d| d.num_colors = 9, "header colour count");
    expect_doc(
        |d| {
            for c in d.frames[0].chunks.iter_mut() {
                if let Chunk::OldPalette { packets, .. } = c {
                    packets[0].colors[1] = [1, 2, 3];
                }
            }
        },
        "old palette entry 1 differs",
    );
    expect_doc(
        |d| {
            for c in d.frames[0].chunks.iter_mut() {
                if let Chunk::Cel(cel) = c {
                    if let CelData::Image { pixels, .. } = &mut cel.data {
                        pixels[0] = 200;
                    }
                }
            }
        },
        "outside the 3 entry palette",
    );
    expect_doc(
        |d| {
            d.frames[1].chunks[0] = Chunk::Cel(Cel { layer: 0, x: 0, y: 0, opacity: 255, z_index: 0, data: CelData::Linked { frame: 1 } });
        },
        "not before frame",
    );
    expect_doc(
        |d| {
            d.frames[1].chunks[0] = Chunk::Cel(Cel { layer: 0, x: 0, y: 0, opacity: 255, z_index: 0, data: CelData::Linked { frame: 0 } });
            d.frames[1].chunks.push(Chunk::Cel(Cel { layer: 0, x: 0, y: 0, opacity: 255, z_index: 0, data: CelData::Linked { frame: 0 } }));
        },
        "two cels for layer 0",
    );
    expect_doc(
        |d| {
            for c in d.frames[0].chunks.iter_mut() {
                if let Chunk::Layer(l) = c {
                    if l.layer_type == LayerType::Tilemap {
                        l.tileset_index = 99;
                    }
                }
            }
        },
        "missing tileset 99",
    );
    expect_doc(
        |d| {
            for c in d.frames[0].chunks.iter_mut() {
                if let Chunk::Cel(cel) = c {
                    if let CelData::Tilemap { tiles, .. } = &mut cel.data {
                        tiles[0] = 5;
                    }
                }
            }
        },
        "uses tile id 5",
    );
    expect_doc(
        |d| {
            for c in d.frames[0].chunks.iter_mut() {
                if let Chunk::Cel(cel) = c {
                    if let CelData::Image { w, .. } = &mut cel.data {
                        *w += 1;
                    }
                }
            }
        },
        "inflates to",
    );
    expect_doc(
        |d| {
            for c in d.frames[0].chunks.iter_mut() {
                if let Chunk::Slice(s) = c {
                    s.keys[0].frame = 4;
                }
            }
        },
        "key on frame 4",
    );
    expect_doc(
        |d| {
            for c in d.frames[0].chunks.iter_mut() {
                if let Chunk::Slice(s) = c {
                    s.keys[0].center = Some((3, 3, 4, 4));
                }
            }
        },
        "9-patch centre",
    );
    expect_doc(|d| d.frames[0].chunks.insert(0, Chunk::UserData(UserData::default())), "first chunk of its frame");
    expect_doc(|d| d.frames[0].chunks.push(Chunk::CelExtra(CelExtra { flags: 0, x: 0, y: 0, w: 0, h: 0 })), "directly follow a cel");
    expect_doc(|d| d.frames[0].chunks.push(Chunk::Unknown { chunk_type: 0x4242, bytes: vec![] }), "unknown chunk type 0x4242");
    expect_doc(|d| d.frames[0].chunks.retain(|c| !matches!(c, Chunk::Palette(_) | Chunk::OldPalette { .. })), "no palette");
    expect_doc(|d| d.frames[0].chunks.retain(|c| !matches!(c, Chunk::Layer(_) | Chunk::Cel(_))), "no layers");
    expect_doc(
        |d| {
            for c in d.frames[0].chunks.iter_mut() {
                if let Chunk::Layer(l) = c {
                    l.child_level = 3;
                }
            }
        },
        "child level",
    );
    expect_doc(
        |d| {
            for c in d.frames[0].chunks.iter_mut() {
                if let Chunk::Layer(l) = c {
                    l.blend_mode = 40;
                    break;
                }
            }
        },
        "blend mode",
    );
    expect_doc(
        |d| {
            for c in d.frames[0].chunks.iter_mut() {
                if let Chunk::Layer(l) = c {
                    l.layer_type = LayerType::Group;
                    break;
                }
            }
        },
        "cel on group layer",
    );
}

#[test]
fn corrupt_zlib_is_reported() {
    let mut b = write(&good_doc());
    // flip bytes inside the first compressed cel payload
    let doc = read(&b).unwrap();
    assert!(!doc.frames.is_empty());
    let pos = b.windows(2).position(|w| w == [0x78, 0x9c]).or_else(|| b.windows(2).position(|w| w[0] == 0x78)).unwrap();
    for i in 0..6 {
        b[pos + 2 + i] ^= 0xA5;
    }
    assert!(!validate(&b).is_empty());
}

#[test]
fn reader_rejects_structural_damage_without_panicking() {
    let good = write(&good_doc());
    assert!(read(&good[..good.len() - 1]).is_err());
    assert!(read(&good[..64]).is_err());
    let mut b = good.clone();
    b[4] = 1;
    assert!(read(&b).is_err());
    // every truncation point must return, never panic
    for n in 0..good.len() {
        let _ = read(&good[..n]);
        let _ = validate(&good[..n]);
    }
    // single-byte flips never panic either
    for i in (0..good.len()).step_by(3) {
        let mut b = good.clone();
        b[i] ^= 0xFF;
        let _ = read(&b);
        let _ = validate(&b);
    }
}
