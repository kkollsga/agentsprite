# Changelog

All notable changes to AgentSprite (`pxl`) are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Add user-visible changes to `[Unreleased]` as you land them. At release time promote it to `[x.y.z]`, bump
`Cargo.toml` and `.claude-plugin/plugin.json` to the same version, and tag `vX.Y.Z`. The release workflow refuses
a tag whose version, plugin version or changelog section disagree.

## [Unreleased]

## [0.1.0] - 2026-10-17

First public release, distributed as prebuilt binaries on GitHub Releases.

### Added
- `pxl`, a pixel-art tool for retro consoles: reduce, hand-finish, lint, export and deliver sprites, tiles and screens
  within real hardware limits. Nine consoles: Game Boy, Game Boy Color, NES, Master System, Game Gear, Mega Drive,
  SNES, PC Engine, Game Boy Advance (`pxl console list`).
- Single-image mode (`pxl image`) and managed projects (`pxl new`, `ingest`, `intake`, `style`, `plan`, `build`,
  `deliver`) with versions, automatic compare images, work lists, review gates and a machine-readable delivery manifest.
- Six agent skills (pxl-operator, pxl-extract, pxl-sprite, pxl-tiles, pxl-review, pxl-coordinate) embedded in the
  binary and shipped as a Claude Code plugin.
- `pxl skill list | show | install | uninstall | path`: install the skills for Claude Code and Codex, at user or
  project level, with `--dry-run`; installed folders carry a `.agentsprite-managed` marker and a version stamp, and
  unmanaged folders are never replaced or removed.
- `install.sh` (macOS/Linux) and `install.ps1` (Windows): download the release archive, verify its sha256, install
  `pxl` and, by default, its skills.
- Release binaries for Linux (x86_64, aarch64; static musl), macOS (x86_64, aarch64) and Windows (x86_64), each with a
  `.sha256` file.
- Original CC0 example art and two demo scripts in `examples/`.
