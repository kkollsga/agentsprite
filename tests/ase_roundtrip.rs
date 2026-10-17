use agentsprite::ase::*;
use agentsprite::color::Rgb;

fn prop_maps() -> Vec<PropMap> {
    let all = vec![
        ("null".to_string(), PropValue::Null),
        ("bool".to_string(), PropValue::Bool(true)),
        ("i8".to_string(), PropValue::I8(-5)),
        ("u8".to_string(), PropValue::U8(200)),
        ("i16".to_string(), PropValue::I16(-300)),
        ("u16".to_string(), PropValue::U16(60000)),
        ("i32".to_string(), PropValue::I32(-70000)),
        ("u32".to_string(), PropValue::U32(4_000_000_000)),
        ("i64".to_string(), PropValue::I64(-5_000_000_000)),
        ("u64".to_string(), PropValue::U64(18_000_000_000_000_000_000)),
        ("fixed".to_string(), PropValue::Fixed(0x18000)),
        ("float".to_string(), PropValue::Float(1.5)),
        ("double".to_string(), PropValue::Double(-2.25)),
        ("str".to_string(), PropValue::Str("héllo".into())),
        ("point".to_string(), PropValue::Point(1, -2)),
        ("size".to_string(), PropValue::Size(3, 4)),
        ("rect".to_string(), PropValue::Rect { x: 1, y: 2, w: 3, h: 4 }),
        ("vec_typed".to_string(), PropValue::Vector { elem_type: 3, items: vec![PropValue::U8(1), PropValue::U8(2)] }),
        (
            "vec_mixed".to_string(),
            PropValue::Vector { elem_type: 0, items: vec![PropValue::Bool(false), PropValue::Str("x".into()), PropValue::I32(7)] },
        ),
        ("vec_empty".to_string(), PropValue::Vector { elem_type: 0, items: vec![] }),
        ("map".to_string(), PropValue::Map(vec![("inner".to_string(), PropValue::U16(9)), ("deep".to_string(), PropValue::Map(vec![]))])),
        ("uuid".to_string(), PropValue::Uuid([7; 16])),
    ];
    vec![PropMap { key: 0, props: all }, PropMap { key: 5, props: vec![("ext".to_string(), PropValue::U8(1))] }]
}

/// A document touching every chunk and cel type.
pub fn full_doc() -> AseDoc {
    let mut d = AseDoc::new(8, 8, ColorMode::Indexed);
    d.flags = 3;
    d.num_colors = 4;
    d.grid_w = 8;
    d.grid_h = 8;
    let pal = vec![[0, 0, 0, 0], [255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]];
    let mut pc = PaletteChunk::full(&pal);
    pc.entries[1].name = Some("red".into());
    let mut group = Layer::normal("group");
    group.layer_type = LayerType::Group;
    group.blend_mode = 1;
    group.opacity = 128;
    let mut child = Layer::normal("child");
    child.child_level = 1;
    child.flags |= layer_flags::PREFER_LINKED | layer_flags::BACKGROUND;
    let mut tm = Layer::normal("tiles");
    tm.layer_type = LayerType::Tilemap;
    tm.tileset_index = 3;
    tm.opacity = 200;
    let mut hidden = Layer::normal("hidden");
    hidden.flags = layer_flags::EDITABLE;
    let ud_all = UserData { text: Some("note".into()), color: Some([1, 2, 3, 4]), props: Some(prop_maps()) };
    let f0 = vec![
        Chunk::ColorProfile(ColorProfile { profile_type: 1, flags: 0, gamma: 0, icc: vec![] }),
        Chunk::ExternalFiles(vec![ExternalFile { id: 1, kind: 2, name: "ext.json".into() }]),
        Chunk::OldPalette { six_bit: false, packets: vec![OldPalPacket { skip: 0, colors: vec![[0, 0, 0], [255, 0, 0], [0, 255, 0], [0, 0, 255]] }] },
        Chunk::OldPalette { six_bit: true, packets: vec![OldPalPacket { skip: 1, colors: vec![[63, 0, 0]] }] },
        Chunk::Palette(pc),
        Chunk::Tags(vec![
            Tag { from: 0, to: 1, direction: LoopDir::PingPong, repeat: 3, color: [9, 8, 7], name: "walk".into() },
            Tag { from: 1, to: 1, direction: LoopDir::Reverse, repeat: 0, color: [0, 0, 0], name: "idle".into() },
        ]),
        Chunk::UserData(UserData { text: Some("tag one".into()), ..Default::default() }),
        Chunk::UserData(UserData { color: Some([5, 6, 7, 255]), ..Default::default() }),
        Chunk::Tileset(Tileset {
            id: 3,
            flags: tileset_flags::EMBEDDED | tileset_flags::EMPTY_ZERO | tileset_flags::AUTO_XFLIP,
            num_tiles: 2,
            tile_w: 2,
            tile_h: 2,
            base_index: 1,
            name: "ts".into(),
            external: None,
            tiles: Some(vec![0, 0, 0, 0, 1, 2, 3, 1]),
        }),
        Chunk::Tileset(Tileset {
            id: 4,
            flags: tileset_flags::EXTERNAL,
            num_tiles: 0,
            tile_w: 4,
            tile_h: 4,
            base_index: 1,
            name: "ext-ts".into(),
            external: Some((1, 9)),
            tiles: None,
        }),
        Chunk::Layer(Layer::normal("base")),
        Chunk::UserData(ud_all.clone()),
        Chunk::Layer(group),
        Chunk::Layer(child),
        Chunk::Layer(tm),
        Chunk::Layer(hidden),
        Chunk::Cel(Cel {
            layer: 0,
            x: 1,
            y: 1,
            opacity: 255,
            z_index: 0,
            data: CelData::Raw { w: 2, h: 2, pixels: vec![1, 2, 3, 1] },
        }),
        Chunk::CelExtra(CelExtra { flags: 1, x: 0x10000, y: 0x20000, w: 0x30000, h: 0x40000 }),
        Chunk::UserData(UserData { text: Some("cel".into()), ..Default::default() }),
        Chunk::Cel(Cel {
            layer: 2,
            x: -1,
            y: 0,
            opacity: 100,
            z_index: -1,
            data: CelData::Image { w: 3, h: 2, pixels: vec![1, 1, 2, 2, 3, 3] },
        }),
        Chunk::Cel(Cel {
            layer: 3,
            x: 0,
            y: 0,
            opacity: 255,
            z_index: 0,
            data: CelData::Tilemap {
                w: 2,
                h: 2,
                bits_per_tile: 32,
                id_mask: 0x1fff_ffff,
                xflip_mask: 0x8000_0000,
                yflip_mask: 0x4000_0000,
                diag_mask: 0x2000_0000,
                tiles: vec![0, 1, 0x8000_0001, 0x4000_0001],
            },
        }),
        Chunk::Slice(Slice {
            name: "nine".into(),
            flags: 3,
            keys: vec![
                SliceKey { frame: 0, x: 1, y: 1, w: 6, h: 6, center: Some((1, 1, 4, 4)), pivot: Some((3, 3)) },
                SliceKey { frame: 1, x: 0, y: 0, w: 8, h: 8, center: Some((2, 2, 4, 4)), pivot: Some((-1, 5)) },
            ],
        }),
        Chunk::UserData(UserData { text: Some("slice".into()), ..Default::default() }),
        Chunk::Slice(Slice { name: "plain".into(), flags: 0, keys: vec![SliceKey { frame: 0, x: 0, y: 0, w: 2, h: 2, center: None, pivot: None }] }),
        Chunk::Mask(Mask { x: 0, y: 0, w: 9, h: 2, name: "m".into(), bitmap: vec![0xFF, 0x80, 0x00, 0x00] }),
        Chunk::Path(vec![]),
    ];
    let f1 = vec![
        Chunk::Cel(Cel { layer: 0, x: 0, y: 0, opacity: 255, z_index: 0, data: CelData::Linked { frame: 0 } }),
        Chunk::Cel(Cel {
            layer: 2,
            x: 0,
            y: 0,
            opacity: 255,
            z_index: 0,
            data: CelData::Raw { w: 1, h: 1, pixels: vec![3] },
        }),
    ];
    d.frames.push(Frame { duration: 100, chunks: f0 });
    d.frames.push(Frame { duration: 250, chunks: f1 });
    d
}

#[test]
fn every_chunk_round_trips_byte_identical() {
    let d = full_doc();
    let b1 = write(&d);
    let probs = validate(&b1);
    assert!(probs.is_empty(), "{:#?}", probs);
    let d2 = read(&b1).unwrap();
    assert_eq!(d, d2);
    let b2 = write(&d2);
    assert_eq!(b1, b2);
    assert!(!d2.frames[0].chunks.iter().any(|c| matches!(c, Chunk::Unknown { .. })));
}

#[test]
fn rgba_and_grayscale_cels_round_trip() {
    for (mode, bpp) in [(ColorMode::Rgba, 4usize), (ColorMode::Grayscale, 2)] {
        let mut d = AseDoc::new(3, 2, mode);
        let l = Layer::normal("l");
        let px: Vec<u8> = (0..6 * bpp).map(|i| (i * 7) as u8).collect();
        d.frames.push(Frame {
            duration: 10,
            chunks: vec![
                Chunk::Layer(l),
                Chunk::Cel(Cel { layer: 0, x: 0, y: 0, opacity: 255, z_index: 0, data: CelData::Image { w: 3, h: 2, pixels: px } }),
            ],
        });
        let b = write(&d);
        assert!(validate(&b).is_empty(), "{:?}", validate(&b));
        assert_eq!(read(&b).unwrap(), d);
    }
}

#[test]
fn unknown_chunk_is_preserved_verbatim() {
    let mut d = full_doc();
    d.frames[1].chunks.push(Chunk::Unknown { chunk_type: 0x7777, bytes: vec![1, 2, 3, 4, 5] });
    d.frames[0].chunks.insert(0, Chunk::Unknown { chunk_type: 0x2021, bytes: vec![] });
    let b = write(&d);
    let d2 = read(&b).unwrap();
    assert_eq!(d, d2);
    assert_eq!(write(&d2), b);
    // the validator is strict about unknown chunks
    assert!(validate(&b).iter().any(|p| p.msg.contains("unknown chunk type")));
}

#[test]
fn unparsable_known_chunk_is_kept_as_unknown() {
    // A tags chunk with trailing future bytes cannot be parsed strictly: it must survive untouched.
    let mut d = full_doc();
    let tags_pos = d.frames[0].chunks.iter().position(|c| matches!(c, Chunk::Tags(_))).unwrap();
    d.frames[0].chunks[tags_pos] = Chunk::Unknown { chunk_type: CHUNK_TAGS, bytes: vec![0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 99] };
    let b = write(&d);
    let d2 = read(&b).unwrap();
    assert_eq!(d2.frames[0].chunks[tags_pos], d.frames[0].chunks[tags_pos]);
    assert_eq!(write(&d2), b);
}

#[test]
fn foreign_compression_is_chunk_identical() {
    // Stored (level 0) zlib blocks, as another encoder might write: content survives, bytes may differ.
    let d = full_doc();
    let canonical = write(&d);
    let stored = miniz_stored(&canonical);
    let d_stored = read(&stored).expect("stored-zlib file reads");
    assert_eq!(d_stored, d);
    assert!(validate(&stored).is_empty(), "{:?}", validate(&stored));
    assert_eq!(write(&d_stored), canonical);
}

/// Re-encodes every image cel of `bytes` with an uncompressed zlib stream (header + stored block + adler32).
fn miniz_stored(bytes: &[u8]) -> Vec<u8> {
    let d = read(bytes).unwrap();
    // Build the file by hand from the typed model: swap the compressor via a manual writer for cels only.
    // Simplest: write canonical, then patch each image cel's zlib payload in place.
    let mut out = bytes[..128].to_vec();
    let mut pos = 128;
    for _ in 0..d.frames.len() {
        let fsize = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap()) as usize;
        let mut body = Vec::new();
        let mut c = pos + 16;
        while c < pos + fsize {
            let cs = u32::from_le_bytes(bytes[c..c + 4].try_into().unwrap()) as usize;
            let ty = u16::from_le_bytes(bytes[c + 4..c + 6].try_into().unwrap());
            let mut chunk = bytes[c..c + cs].to_vec();
            if ty == CHUNK_CEL {
                let ct = u16::from_le_bytes(chunk[6 + 7..6 + 9].try_into().unwrap());
                if ct == 2 {
                    let head = 6 + 16 + 4; // chunk hdr + cel fixed + w,h
                    let z = &chunk[head..];
                    let raw = miniz_oxide::inflate::decompress_to_vec_zlib(z).unwrap();
                    let mut ns = chunk[..head].to_vec();
                    ns.extend(stored_zlib(&raw));
                    chunk = ns;
                    let l = chunk.len() as u32;
                    chunk[0..4].copy_from_slice(&l.to_le_bytes());
                }
            }
            body.extend(chunk);
            c += cs;
        }
        let mut fh = bytes[pos..pos + 16].to_vec();
        let fl = body.len() as u32 + 16;
        fh[0..4].copy_from_slice(&fl.to_le_bytes());
        out.extend(fh);
        out.extend(body);
        pos += fsize;
    }
    let l = out.len() as u32;
    out[0..4].copy_from_slice(&l.to_le_bytes());
    out
}

fn stored_zlib(raw: &[u8]) -> Vec<u8> {
    let mut v = vec![0x78, 0x01, 0x01];
    v.extend((raw.len() as u16).to_le_bytes());
    v.extend((!(raw.len() as u16)).to_le_bytes());
    v.extend(raw);
    let (mut a, mut b) = (1u32, 0u32);
    for &x in raw {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    v.extend(((b << 16) | a).to_be_bytes());
    v
}

#[test]
fn uuid_layers_round_trip() {
    let mut d = AseDoc::new(2, 2, ColorMode::Rgba);
    d.flags = 7;
    let mut l = Layer::normal("u");
    l.uuid = Some([9; 16]);
    d.frames.push(Frame { duration: 1, chunks: vec![Chunk::Layer(l)] });
    let b = write(&d);
    assert_eq!(read(&b).unwrap(), d);
    assert!(validate(&b).is_empty(), "{:?}", validate(&b));
}

fn builder_doc() -> Vec<u8> {
    let pal = [Rgb::new(0, 0, 0), Rgb::new(255, 0, 0), Rgb::new(0, 255, 0), Rgb::new(0, 0, 255)];
    let mut b = AseBuilder::new_indexed(6, 4, &pal);
    b.set_grid(0, 0, 8, 8);
    let base = b.add_layer("base");
    let detail = b.add_layer("detail");
    for i in 0..3 {
        b.add_frame(100 + i as u16 * 50);
    }
    let mut c0 = vec![0u8; 24];
    c0[7] = 1;
    c0[8] = 2;
    c0[13] = 3;
    assert!(b.set_cel_canvas(0, base, &c0));
    b.link_cel(1, base, 0);
    let mut c2 = vec![0u8; 24];
    c2[0] = 3;
    b.set_cel_canvas(2, base, &c2);
    assert!(!b.set_cel_canvas(0, detail, &[0u8; 24]), "empty canvas adds no cel");
    let mut d1 = vec![0u8; 24];
    d1[23] = 2;
    b.set_cel_canvas(1, detail, &d1);
    b.add_tag("stand", 0, 0, LoopDir::Forward, 0);
    b.add_tag("walk", 0, 2, LoopDir::PingPong, 2);
    b.add_slice(SliceSpec { name: "origin".into(), x: 0, y: 0, w: 6, h: 4, center: None, pivot: Some((3, 4)) });
    b.build().unwrap()
}

#[test]
fn builder_output_is_valid_deterministic_and_round_trips() {
    let a = builder_doc();
    let b = builder_doc();
    assert_eq!(a, b, "deterministic");
    let probs = validate(&a);
    assert!(probs.is_empty(), "{:#?}", probs);
    let doc = read(&a).unwrap();
    assert_eq!(write(&doc), a, "read -> write is byte identical for our files");
    assert_eq!(doc.frames.len(), 3);
    assert_eq!(doc.frames[2].duration, 200);
    assert_eq!(doc.layers().len(), 2);
    assert_eq!(doc.tags().len(), 2);
    assert_eq!(doc.tags()[1].repeat, 2);
    assert_eq!(doc.slices()[0].keys[0].pivot, Some((3, 4)));
    assert_eq!(doc.palette().len(), 4);
    assert_eq!(doc.palette()[2], [0, 255, 0, 255]);
    // chunk order of frame 0 follows Aseprite
    let types: Vec<u16> = doc.frames[0].chunks.iter().map(|c| c.chunk_type()).take(6).collect();
    assert_eq!(types, vec![CHUNK_COLOR_PROFILE, CHUNK_OLD_PALETTE_256, CHUNK_PALETTE, CHUNK_TAGS, CHUNK_LAYER, CHUNK_LAYER]);
}

#[test]
fn render_matches_input() {
    let doc = read(&builder_doc()).unwrap();
    let f0 = render_layer_indices(&doc, 0, 0).unwrap();
    let mut want = vec![0u8; 24];
    want[7] = 1;
    want[8] = 2;
    want[13] = 3;
    assert_eq!(f0, want);
    // linked frame renders the same pixels
    assert_eq!(render_layer_indices(&doc, 1, 0).unwrap(), want);
    // detail layer is empty on frame 0, drawn on frame 1
    assert!(render_layer_indices(&doc, 0, 1).unwrap().iter().all(|&i| i == 0));
    let flat1 = flatten_indices(&doc, 1).unwrap();
    let mut w1 = want.clone();
    w1[23] = 2;
    assert_eq!(flat1, w1);
    let rgba = flatten_rgba(&doc, 1).unwrap();
    assert_eq!(rgba[7], agentsprite::img::Rgba::opaque(255, 0, 0));
    assert_eq!(rgba[0].a, 0);
    assert_eq!(rgba[23], agentsprite::img::Rgba::opaque(0, 255, 0));
}

#[test]
fn flatten_respects_visibility_and_opacity() {
    let mut doc = read(&builder_doc()).unwrap();
    // hide the detail layer
    for c in doc.frames[0].chunks.iter_mut() {
        if let Chunk::Layer(l) = c {
            if l.name == "detail" {
                l.flags &= !layer_flags::VISIBLE;
            }
        }
    }
    let w1 = flatten_indices(&doc, 1).unwrap();
    assert_eq!(w1[23], 0);
    // half opacity on base
    for c in doc.frames[0].chunks.iter_mut() {
        if let Chunk::Layer(l) = c {
            if l.name == "base" {
                l.opacity = 128;
            }
        }
    }
    let rgba = flatten_rgba(&doc, 0).unwrap();
    assert_eq!(rgba[7].a, 128);
}

#[test]
fn tilemap_renders_through_tileset() {
    let doc = full_doc();
    // layer 3 = tilemap, frame 0; tiles [0,1,xflip 1,yflip 1], tile 1 pixels = 1,2,3,1
    let px = render_layer_indices(&doc, 0, 3).unwrap();
    // tile (1,0) is id 1 unflipped: rows at y=0..2, x=2..4 -> [1,2] / [3,1]
    assert_eq!(&px[2..4], &[1, 2]);
    assert_eq!(&px[8 + 2..8 + 4], &[3, 1]);
    // tile (0,1) is id 1 x flipped: [2,1] / [1,3]
    assert_eq!(&px[16..18], &[2, 1]);
    assert_eq!(&px[24..26], &[1, 3]);
    // tile (1,1) y flipped: [3,1] / [1,2]
    assert_eq!(&px[16 + 2..16 + 4], &[3, 1]);
    assert_eq!(&px[24 + 2..24 + 4], &[1, 2]);
}

#[test]
fn builder_tilemap_and_groups_validate() {
    let mut b = AseBuilder::new_indexed(4, 4, &[Rgb::new(0, 0, 0), Rgb::new(9, 9, 9)]);
    b.add_tileset(0, "tiles", 2, 2, 2, vec![0, 0, 0, 0, 1, 1, 1, 1]);
    let g = b.add_group("grp", 0);
    let _ = g;
    let tl = b.add_tilemap_layer("map", 0);
    let mut inner = Layer::normal("inner");
    inner.child_level = 1;
    b.add_layer_with(inner);
    b.add_frame(100);
    b.set_tilemap_cel(0, tl, 0, 0, 2, 2, vec![0, 1, 1, 0]);
    let bytes = b.build().unwrap();
    assert!(validate(&bytes).is_empty(), "{:#?}", validate(&bytes));
    let doc = read(&bytes).unwrap();
    let px = render_layer_indices(&doc, 0, 1).unwrap();
    assert_eq!(px[2], 1);
    assert_eq!(px[0], 0);
}

#[test]
fn builder_requires_frames_and_layers() {
    let mut b = AseBuilder::new_rgba(2, 2);
    b.add_layer("x");
    assert!(b.build().is_err());
    let mut b = AseBuilder::new_rgba(2, 2);
    b.add_frame(10);
    assert!(b.build().is_err());
}
