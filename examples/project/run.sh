#!/usr/bin/env bash
# Project demo: new -> ingest -> intake -> style propose -> plan -> build -> deliver --dry-run, on any console.
#
#   examples/project/run.sh [console] [workdir]
#   CONSOLE=nes examples/project/run.sh
#
# Needs `pxl` on PATH (or PXL=/path/to/pxl). Builds a scratch project (default: a new temp dir); nothing in the repo is changed.
# The inputs are original art (examples/inputs, CC0): a character in 3 poses, an enemy (2 frames), an item, a 32x32 tile region
# and a title screen, all drawn at 2x the target size (the screen picture is chosen by the console's screen size).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PXL="${PXL:-pxl}"
CONSOLE="${1:-${CONSOLE:-gbc}}"
WORK="${2:-${WORK:-$(mktemp -d)}}"
PROJ="$WORK/demo"
step() { echo; echo "== $*"; }

mkdir -p "$WORK"
# the screen of the console (title screens are drawn for every console's size, at 2x)
SCREEN="$("$PXL" console show "$CONSOLE" --json | sed -n 's/.*"screen":\[\([0-9]*\),\([0-9]*\)\].*/\1x\2/p' | head -1)"

step "new: a project for $CONSOLE"
"$PXL" new "$PROJ" --console "$CONSOLE" --name Demo
cd "$PROJ"

step "ingest: drop the inputs in inbox/, one folder per kind of asset"
for d in hero enemy item tiles; do cp -R "$HERE/../inputs/$d" inbox/; done
mkdir -p inbox/screens && cp "$HERE/../inputs/screens/title_$SCREEN.png" inbox/screens/title.png
"$PXL" ingest inbox/hero    --group player  --factor 2
"$PXL" ingest inbox/enemy   --group enemies --factor 2
"$PXL" ingest inbox/item    --group items   --factor 2
"$PXL" ingest inbox/tiles   --group zone    --mode tile   --factor 2
"$PXL" ingest inbox/screens --group screens --mode screen --factor 2
"$PXL" tree

step "intake: infer the brief (console, fidelity, identity list, cadence); every item is an assumption you can reverse"
"$PXL" intake --request "Demo for the $CONSOLE, faithful to the source"

step "style: measure the inputs and propose the design language (style.md)"
"$PXL" style analyze
"$PXL" style propose

step "plan: scenes, budgets, palette plan before anyone draws"
"$PXL" plan --apply
"$PXL" plan accept

step "fit the screen's tiles to the console's palettes (the dry run below checks it)"
"$PXL" tiles fit title --apply > /dev/null || true

step "build: console-format files and export/manifest.json"
"$PXL" build

step "deliver --dry-run: the checks a delivery runs, on the style frames, nothing is snapshotted"
"$PXL" deliver --dry-run

echo
echo "project:  $PROJ"
echo "compare:  $PROJ/assets/player/hero/compare.png  (input | latest)"
echo "manifest: $PROJ/export/manifest.json"
