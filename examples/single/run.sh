#!/usr/bin/env bash
# Single-image demo: one picture, no project. Reduces examples/inputs/hero/hero_02.png for a console and shows the result.
#
#   examples/single/run.sh [console] [workdir]
#   CONSOLE=nes examples/single/run.sh
#
# Needs `pxl` on PATH (or PXL=/path/to/pxl). Works in a scratch folder (default: a new temp dir); nothing in the repo is changed.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PXL="${PXL:-pxl}"
CONSOLE="${1:-${CONSOLE:-gbc}}"
WORK="${2:-${WORK:-$(mktemp -d)}}"
mkdir -p "$WORK"
cp "$HERE/../inputs/hero/hero_02.png" "$WORK/hero.png"
cd "$WORK"

# 1. one command: makes hero.pxl/ beside the picture (input, first reduction, versions, compare.png)
#    --factor 2: the picture is drawn at 2x the target size (32x48 -> 16x24)
"$PXL" image hero.png --console "$CONSOLE" --mode sprite --factor 2

# 2. the automatic first version is a rough sketch. Lint says what to look at (limits are errors, looks are warnings).
"$PXL" lint hero.pxl --rules eye_parity,identity_colour,flat || true

# 3. console-format files next to the asset
"$PXL" export hero.pxl

echo
echo "compare image (input | latest): $WORK/hero.pxl/compare.png"
echo "console files:                  $WORK/hero.pxl/export/"
