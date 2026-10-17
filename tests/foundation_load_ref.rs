use agentsprite::load;
use agentsprite::model::*;

#[test]
fn reference_images_path() {
    let root = std::env::temp_dir().join(format!("pxl_loadref_{}", std::process::id()));
    std::fs::create_dir_all(root.join("parts")).unwrap();
    std::fs::create_dir_all(root.join("refs")).unwrap();
    agentsprite::png::write_rgba(&root.join("refs/01_stand.png"), &agentsprite::img::RgbaImage::new(64, 64)).unwrap();
    let mut p = Part::new("player", (32, 32));
    p.path = root.join("parts/player.pxl");
    p.origin = (17, 16);
    p.input.images = Some("refs/{frame}.png".into());
    p.input.factor = Some(2);
    let f = Frame { name: "01_stand".into(), src: None, locked: false, grid: Grid::new(32, 32, b'.') };
    // pattern resolves relative to the part folder, then its parents
    let r = load::reference(&p, &root.join("parts"), &f).unwrap();
    assert_eq!((r.img.w, r.img.h, r.origin), (64, 64, (34, 32)));
    p.input.ref_origin = Some((1, 2));
    assert_eq!(load::reference(&p, &root, &f).unwrap().origin, (1, 2));
    let missing = Frame { name: "nope".into(), ..f };
    assert!(load::reference(&p, &root, &missing).is_err());
}
