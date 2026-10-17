# Operator playbook, step by step

Read `SKILL.md` first. This is the detail behind each step, with the commands and what to show.

## 0. The request

Write the request down verbatim (`request.md` in the project, or `pxl intake --request "..."`). Note what it names:
console, source, a deadline, a client, words about style ("faithful", "redraw", "mood"). `pxl intake` reads them.

## 1. Intake (mostly inferred)

```
pxl new game --console gbc --name Game        # console from the request or the client spec; pxl console list shows all
pxl ingest inbox/hero --group player --factor 2
pxl intake --request "NES port of the first zone, faithful, weekly updates"
```
Read what it printed: every `key = value (why)` is an assumption. Check the three that matter most:
- **console and modes**: a wrong console is redone work. If the request names another console than the project, intake opens a question.
- **identity list** per asset (eyes, key colours, materials): open the compare images and correct the list with a command, not a note:
  `pxl style identity hero add #a05020` (what it missed), `pxl style identity hero remove #f8f8f8` (a false positive), `pxl style identity hero set
  #f09030,#209898,#e02828`, `pxl style identity hero eyes 2` (or `pxl decide "why" --set identity.hero=#f09030,#209898`). The list is weighted by area,
  saturation and how distinct the material is, so the dominant hair and overalls come first; a corrected list is confirmed and a new `pxl intake`
  keeps it. The `identity_colour` gate checks every colour on it, tone by tone, in every frame where the source has it.
- **trade-off policy**: the default order is in `references/defaults.toml`. It is strict limits, least visible cut first, no exceptions.

Questions come back as open decisions with an image page (`review/decisions/dNNNN.html`). Present them together.

## 2. Design language

```
pxl style analyze          # outline, tones per material, light, highlights, dither, palette size, eyes, identity colours
pxl style propose          # style.md with rule ids and what each limit forces
```
Read `style.md` against the console limits and write down the consequences that matter, for example:
"3 colours per cell: outline, mid, light per material; the highlight is the light tone, not a 4th colour";
"factor 2: features under 2 px vanish unless redrawn: eyes need at least 2x3 white and a 1 px pupil";
"one palette per cell: put colour changes on cell borders". The console's own `[style]` (`pxl console show ID`) is the starting
point; a project overrides it with `pxl project set style.white balanced` (also `style.outline`, `style.value_map`, `style.min_value_spread`).

## 3. Plan and show the fit

```
pxl plan                   # proposal: scenes from groups, VRAM map, group budgets, palette plan, fit report, options per infeasible scene
pxl palette plan SCENE --what-if
pxl plan accept
```
Show: the fit table, swatches per scene (render them on a source frame), the infeasible scenes with options and costs. Recommend one option
per question with a reason. Decisions in one batch (`pxl ask`), not a trickle.

## 4. Style frames

`pxl style mark hero hero_01 hero_02`, give the drawing to an artist agent (`pxl-sprite`), review it yourself against the
source and `style.md` (`pxl review checklist ASSET`), `pxl style approve --by user` once the user has seen the compare images.
The nearest style frame appears on every compare and review page.

## 5. Dry run

`pxl deliver --dry-run` runs the build, our checks, the package and its verifier on the style frames and every part of the scenes (parts in no scene are listed as not covered) and records the milestone. A client
checker (`[delivery] client_checker`) runs here too. Fix format surprises now.

## 6. Production and review

`pxl task add`, `pxl next --agent A`, `pxl brief ASSET` (a fresh agent starts there), `pxl review next | note | approve`, `pxl board` for you.
Escalate only round 3 and user decisions. Update `pxl report` at the cadence.

## 7. Delivery

`pxl deliver` (snapshot in `deliveries/N/`), `pxl changes --since N` for the change log. Read `delivery.md`.
