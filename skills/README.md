# AgentSprite skills

Agent Skills for working with `pxl`, on any of the nine consoles (`pxl console list`). Each skill is a folder with a `SKILL.md` (YAML front matter
`name` and `description`, then a concise body) and `references/` files that are loaded on demand. Plain text only.

| Skill | For | What it gives |
|---|---|---|
| `pxl-operator` | the agent leading a project | The experienced-operator playbook: infer, assume and show; ask only what cannot be inferred (options, images, a recommendation); intake, design language, plan, style frames, early dry run, production, delivery; learn the client's engine model first; reports; record every decision. References: `playbook.md`, `defaults.toml` (shared with `pxl intake`), `client-engine.md`, `reports.md`, `delivery.md`. |
| `pxl-extract` | the agent getting source graphics out | ROMs the user provides, disassemblies, rips and emulator dumps into the inbox contract (one folder per object, `<object>_NN.png`, the source palette, `source.json`, a contact sheet), QA before handover, the legal note. References: `inbox-contract.md`, `decoders.md`, `qa.md`. |
| `pxl-sprite` | artist agents | Converting high-resolution art to target-resolution pixel art and owning the look: the general rules (eyes, identity colours, depth, cells, consistency, verify at zoom), the working loop, fast pxl use, craft, self-review. References: `craft.md`, `cells.md`, `tool.md`, `self-review.md`. |
| `pxl-tiles` | tile artist agents | Hand-finishing a tile set or screen within the console's tile limits: stats, fit, dedupe, textures as repeating units, animated tiles, exit criteria. References: `tile-workflow.md`, `textures.md`, `gbc-bg.md`. |
| `pxl-review` | reviewer | Judging against the source, `style.md` and the style frames; writing actionable per-frame notes; approving. Reference: `checklist.md`. |
| `pxl-coordinate` | coordinator | Many agents in parallel: claims and leases, templates and palettes, the hardware plan, snippets, tasks and briefs. Reference: `sources.md` (Mega Drive source setup). |

The tool provides fast views, cheap edits and hard hardware limits. The skills carry the craft: the agent decides what looks good. The
console data (`pxl console show`) and the project's design language (`style.md`) say what "good" means here.

## Install

- **Claude Code plugin** (this repository is also the plugin; the skills live in `skills/`): see "Skills" in the README.
- `pxl skill install` writes the skills embedded in the binary to `~/.claude/skills` and `~/.codex/skills` (`--host`, `--project`, `--dry-run`); `pxl skill uninstall` removes them; `pxl skill list | show NAME [--ref FILE] | path` inspect them.
- Or copy the `skills/<name>` folders into your agent's skills directory (Claude Code: `~/.claude/skills/` or `<project>/.claude/skills/`).

## Layout (keep stable: `build.rs` embeds it, the plugin reads it)

```
skills/<name>/SKILL.md
skills/<name>/references/*.md        (plus data files such as pxl-operator/references/defaults.toml)
```
A SKILL.md stays under 500 lines; detail goes to references/. The examples in `pxl-sprite/references/tool.md` are run against a copy of a real
project; re-run them after changing the CLI.
