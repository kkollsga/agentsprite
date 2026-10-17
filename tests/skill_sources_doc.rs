//! Every `toml` block in the sources reference parses: part blocks as parts, `toml manifest` blocks as manifests.

use agentsprite::manifest::Manifest;
use agentsprite::pxl;
use std::path::Path;

#[test]
fn sources_reference_examples_parse() {
    let text = agentsprite::skill::skill_files("pxl-coordinate").into_iter().find(|f| f.0 == "references/sources.md").expect("sources.md is shipped").1;
    let (mut parts, mut manifests) = (0, 0);
    let mut lines = text.lines();
    while let Some(l) = lines.next() {
        let Some(info) = l.strip_prefix("```toml") else { continue };
        let mut body = String::new();
        for b in lines.by_ref() {
            if b.starts_with("```") {
                break;
            }
            body.push_str(b);
            body.push('\n');
        }
        if info.trim() == "manifest" {
            Manifest::parse(&body, Path::new("pxl.toml")).unwrap_or_else(|e| panic!("manifest block fails: {:?}\n{}", e, body));
            manifests += 1;
        } else {
            let part = pxl::parse_str(&body, Path::new("p.pxl")).unwrap_or_else(|e| panic!("part block fails: {:?}\n{}", e, body));
            // canonical text round-trips
            let again = pxl::parse_str(&pxl::to_string(&part), Path::new("p.pxl")).unwrap();
            assert_eq!(again, part);
            parts += 1;
        }
    }
    assert!(parts >= 3 && manifests >= 1, "found {} part and {} manifest blocks", parts, manifests);
}
