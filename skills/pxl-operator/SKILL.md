---
name: pxl-operator
description: Lead a pxl pixel-art project from the user's request to a verified delivery as the experienced operator, on any console. Use when someone gives you images or a source and a goal ("port these to the NES", "make sprites for this engine"), when starting or planning a pxl project, running intake, style frames, plan, decisions, reports or delivery, or when deciding what to ask the user and what to assume.
---

# pxl-operator: you are the experienced party

**Before anything: is `pxl` there?** Run `pxl --version`. If the command is missing, install it: `curl -fsSL https://raw.githubusercontent.com/kkollsga/agentsprite/main/install.sh | sh` (macOS/Linux), `irm https://raw.githubusercontent.com/kkollsga/agentsprite/main/install.ps1 | iex` (Windows PowerShell), or download a release from github.com/kkollsga/agentsprite/releases, or `cargo build --release` in a clone. The installer also runs `pxl skill install`. If the version differs from the one stamped under this skill's front matter, run `pxl skill install` to refresh the skills.

The user usually knows the game, not the pipeline. You do. Lead the project so that decisions come early, in
one batch, with pictures and a recommendation, and so that nothing they care about is discovered late.
Your craft partners are `pxl-sprite` and `pxl-tiles` (drawing), `pxl-review` (judging), `pxl-coordinate`
(many agents) and `pxl-extract` (getting the source graphics out). `pxl guide project` is the command table.

## The rule: infer, assume, show, then ask

1. **Infer** from the request and the material: console (request, client spec, source format), fidelity,
   identity list (eyes, saturated key colours), cadence, trade-off policy. `pxl intake --request "..."` does this
   non-interactively and writes `brief.toml`.
2. **Assume** the best-practice default for everything else and write it down as an assumption
   (`decisions.md`, `[operator]`). Defaults and questions are data: `references/defaults.toml`, the same file
   `pxl intake` reads. Each assumption is reversible with one command: `pxl decide "why" --set key=value --by user`.
3. **Show**, do not describe: make the style frames under the assumptions and let the user correct what they dislike.
4. **Ask only** what is both high-impact and not knowable from the inputs: style direction with real alternatives,
   irreversible scope, anything that touches the client. Always as options, with an image each and a
   recommendation, in one small batch (`pxl ask "topic" --option A=img.png|label --option B=img.png --recommend A "why"`).
   Never an open question; never a question about a hardware fact (look it up: `pxl console show`).

## The run, in order (`references/playbook.md` has the detail per step)

0. **Check the tool.** `pxl --version` (see the note under the title): install if missing, `pxl skill install` if the
   version differs from the skill stamp, then `pxl console list` and `pxl guide project`.
1. **Learn the target before the art.** Console and image modes; screen and regions; the client's spec, checker and
   engine model (per act or per zone, streaming, BG objects, OAM order, what is on screen together); deadline.
   Infer it from the client's repo, spec.json or checker when you have access (`references/client-engine.md`).
2. **`pxl new DIR --console ID`**, put inputs in `inbox/` (from `pxl-extract` if they come from a ROM), `pxl ingest`.
3. **`pxl intake`**, then read `pxl style analyze` and write the design language with `pxl style propose`
   (`style.md`: outline, eyes, identity, ramps, light, white, highlight, dither, fold, readability, with what each
   limit does to this input). The console's `[style]` defaults apply (the classic Game Boy uses white as a main tone).
4. **Plan before pixels.** `pxl plan`, `pxl palette plan`, `pxl budget --what-if`. Present the fit per scene, the
   palette swatches with mood options, and every infeasible scene with its options and costs. One batch of decisions.
   `pxl plan accept` when settled (recorded as `agent`; `--by user` only when the user said so).
5. **Style frames (the quality bar).** 3 to 6 key assets drawn to final quality before production: the player in 3
   poses, an enemy, an object, a tile region in context, a screen. `pxl style mark ASSET`, draw them (hand them to
   artist agents with the `pxl-sprite` skill), `pxl style approve` (recorded as `agent`; `--by user` once the user has approved). They become the reference for every producer.
6. **Early dry run.** `pxl deliver --dry-run` on the style frames and every part of the scenes (it prints what it does not cover): format and checker surprises appear on day one.
7. **Production.** Tasks, claims, reviews (`pxl-coordinate`, `pxl-review`). `pxl next` hands out work only after the
   gates (plan accepted, style approved, dry run passed). Only real decisions reach the user.
8. **Deliver.** `pxl deliver` with every check, the manifest, DELIVERY.md and the change log. Our checks are necessary
   and the client's checker is necessary; neither alone is sufficient (`references/delivery.md`).

## Keep the user informed

Send images as work lands, at the cadence of the brief (`pxl report --html`, `--if-due`): done, in review, new
images, pending decisions with your recommendation, budgets, milestones, unconfirmed assumptions. Do not wait
for feedback to keep working on the assumptions. Format: `references/reports.md`.

## Rules that save projects

- Hardware facts are not user decisions; quality trade-offs are. The user's order of cuts: cut animation steps, fold
  colours, narrow sizes, convert to BG, counter-propose to the client. Never ask a client for a hardware exception
  unless the brief says exceptions are allowed.
- Record every decision the moment it is made (`pxl decide`); an asset the user approved personally changes only with
  `--force-user-approved` and a review task.
- Never ship on a per-asset checker alone: check at scene level (`pxl check`, `pxl scene`).
- Verify every reviewer claim at zoom before relaying it (a "source is red" note carries a region and a crop).
- The design-language gates (`eye_parity`, `identity_colour`, `flat`, `consistency`, `readability`) block approval
  unless justified in writing. Never approve a face with one eye. A `reference_missing` warning means a gate could
  not compare: restore the source image or say so in the note.
- Never hand-edit `pxl.toml`, `brief.toml`, `style.toml`, palettes or scenes: every change has a command. Scenes: `pxl scene set SCENE KEY VALUE`
  (palette_set, tileset, window, ...) and `pxl scene set SCENE group.GROUP.budget N`, `pxl scene add-part | remove-part`. Palettes:
  `pxl palette slot set SET SLOT #a #b #c`, `pxl palette slot remove SET SLOT`, `pxl palette plan SCENE --apply`. Assets: `pxl asset set ASSET
  category player` (also `pxl meta ASSET set category player`), `pxl group move`, `pxl meta ASSET ...`. Identity list:
  `pxl style identity ASSET add|remove|set #rrggbb ...` (the gates and style.md read it). Project settings: `pxl project set defaults.category enemy`,
  `style.white balanced`, `delivery.client_checker CMD`. Decisions: `pxl decide`.
- Only process material the user has the right to use. Say so in the brief when the source is a commercial game.
