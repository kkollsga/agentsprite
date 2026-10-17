use agentsprite::color::*;

#[test]
fn oklab_known_values() {
    let w = srgb_to_oklab(Rgb::new(255, 255, 255));
    assert!((w.l - 1.0).abs() < 1e-6 && w.a.abs() < 1e-6 && w.b.abs() < 1e-6, "{:?}", w);
    let k = srgb_to_oklab(Rgb::new(0, 0, 0));
    assert_eq!((k.l, k.a, k.b), (0.0, 0.0, 0.0));
    // reference values from the Oklab definition (Ottosson)
    let r = srgb_to_oklab(Rgb::new(255, 0, 0));
    assert!((r.l - 0.627955).abs() < 1e-5 && (r.a - 0.224863).abs() < 1e-5 && (r.b - 0.125846).abs() < 1e-5, "{:?}", r);
}

#[test]
fn oklab_is_bit_stable() {
    // pinned bit patterns: any change to the maths or the table breaks determinism
    let v = srgb_to_oklab(Rgb::new(0x48, 0x50, 0xb8));
    let again = srgb_to_oklab(Rgb::new(0x48, 0x50, 0xb8));
    assert_eq!(v.l.to_bits(), again.l.to_bits());
    assert_eq!(v.a.to_bits(), again.a.to_bits());
    assert_eq!(v.b.to_bits(), again.b.to_bits());
    assert_eq!(v.l.to_bits(), PIN_L, "L bits {:#x}", v.l.to_bits());
    assert_eq!(v.a.to_bits(), PIN_A, "a bits {:#x}", v.a.to_bits());
    assert_eq!(v.b.to_bits(), PIN_B, "b bits {:#x}", v.b.to_bits());
}
const PIN_L: u64 = 0x3fdefe0764c7bce3;
const PIN_A: u64 = 0x3f8e5d614430d520;
const PIN_B: u64 = 0xbfc4a55c682a3aba;

#[test]
fn cbrt_matches_cube() {
    for x in [1e-9, 0.001, 0.125, 0.5, 1.0, 8.0, 27.0] {
        let y = cbrt(x);
        assert!((y * y * y - x).abs() <= x * 1e-12, "{} -> {}", x, y);
    }
    assert_eq!(cbrt(0.0), 0.0);
}

#[test]
fn snapping_hex_and_distance() {
    assert_eq!(snap_rgb555(Rgb::new(0, 3, 4)), Rgb::new(0, 0, 8));
    assert_eq!(snap_rgb555(Rgb::new(255, 252, 247)), Rgb::new(248, 248, 248));
    assert_eq!(snap_rgb555(Rgb::new(0x48, 0x50, 0xb8)), Rgb::new(0x48, 0x50, 0xb8));
    assert!(is_step_aligned(Rgb::new(0xf8, 0x00, 0x18), 8) && !is_step_aligned(Rgb::new(1, 0, 0), 8));
    assert_eq!(parse_hex("#4850B8").unwrap(), Rgb::new(0x48, 0x50, 0xb8));
    assert_eq!(to_hex(Rgb::new(0x48, 0x50, 0xb8)), "#4850b8");
    for bad in ["4850b8", "#4850b", "#4850bg", "#4850b8f"] {
        assert!(parse_hex(bad).is_err(), "{}", bad);
    }
    let a = srgb_to_oklab(Rgb::new(0, 0, 0));
    let b = srgb_to_oklab(Rgb::new(255, 255, 255));
    assert!((oklab_dist2(a, b) - 1.0).abs() < 1e-6);
    assert_eq!(oklab_dist2(a, a), 0.0);
}
