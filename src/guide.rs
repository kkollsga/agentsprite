//! `pxl guide` (AC-I4): the one-page agent workflow, quality bar and command table. The text lives in `guide.txt`;
//! `tests/e2e_guide.rs` extracts every `pxl ...` line from it and checks that it parses, so the page cannot drift from the real
//! command surface.

pub const TEXT: &str = include_str!("guide.txt");

/// `pxl guide project`: single-image and project mode, versions, work lists, review.
pub const PROJECT: &str = include_str!("guide_project.txt");
