#!/usr/bin/env python3
"""Quality benchmark for one part: run `pxl compare`, append one record to <project>/bench/history.jsonl and print
the delta against the previous record.

Usage (run with `python3 -I`, standard library only):

    python3 -I scripts/bench.py PART.pxl --label "what changed" [--pxl PATH] [--project DIR] [--dry-run]

    python3 -I scripts/bench.py projects/hero/parts/hero.pxl --label "P4 auto + derived.toml"

* PART.pxl   the part to measure. The project folder is the nearest parent holding `bench/` or `pxl.toml`
             (override with --project); the record goes to `<project>/bench/history.jsonl`.
* --label    free text stored in the record (required, so the history says what each run measured).
* --pxl      the pxl binary (default: $PXL, then target/release/pxl and target/e2e/release/pxl of this repo, then
             `pxl` on PATH).
* --dry-run  print the record and the delta, append nothing.

`pxl compare` runs with its sheets in a temporary directory (deleted afterwards). Like any `compare` it refreshes
`<part>.status.json` next to the part. Record fields (same as the earlier records):

    label, date, mean_iou, worst_iou, mean_oklab, lost_total, mean_score, golden_01_stand_diff, lint_warnings, lint_errors

Metrics cover every frame EXCEPT the part's exemplars: frames named by `[reduce] exemplars = [...]` (file stems)
and locked frames, because the exemplar methods learn from those and would flatter the numbers. lint counts are the
per-frame counts of `compare` summed over the same frames. `golden_01_stand_diff` is the number of cells in frame
01_stand that differ from golden/01_stand.txt next to the project (null when 01_stand is an exemplar or absent).
mean_iou, worst_iou, mean_oklab, mean_score are rounded to 4 decimals (mean_score to 2).
"""
import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time

try:
    import tomllib
except ImportError:  # Python < 3.11
    tomllib = None

# (field, better direction) for the delta table: -1 = lower is better, +1 = higher is better
FIELDS = [
    ("mean_iou", +1),
    ("worst_iou", +1),
    ("mean_oklab", -1),
    ("lost_total", -1),
    ("mean_score", -1),
    ("golden_01_stand_diff", -1),
    ("lint_warnings", -1),
    ("lint_errors", -1),
]


def find_pxl(arg):
    here = os.path.dirname(os.path.abspath(__file__))
    repo = os.path.dirname(here)
    cands = [arg, os.environ.get("PXL"), os.path.join(repo, "target/release/pxl"), os.path.join(repo, "target/e2e/release/pxl")]
    for c in cands:
        if c and os.path.isfile(c) and os.access(c, os.X_OK):
            return c
    found = shutil.which("pxl")
    if found:
        return found
    sys.exit("bench: no pxl binary found (build with `cargo build --release`, or pass --pxl PATH)")


def project_of(part, explicit):
    if explicit:
        return os.path.abspath(explicit)
    d = os.path.dirname(os.path.abspath(part))
    while True:
        if os.path.isdir(os.path.join(d, "bench")) or os.path.isfile(os.path.join(d, "pxl.toml")):
            return d
        up = os.path.dirname(d)
        if up == d:
            sys.exit("bench: no project folder (a parent with bench/ or pxl.toml) found; pass --project DIR")
        d = up


def read_part(path):
    with open(path, "rb") as f:
        raw = f.read()
    if tomllib is None:
        sys.exit("bench: Python 3.11+ is needed (tomllib)")
    return tomllib.loads(raw.decode("utf-8"))


def exemplar_frames(part):
    names = set()
    for e in part.get("reduce", {}).get("exemplars", []) or []:
        stem = os.path.splitext(os.path.basename(e.split("@")[0]))[0]
        names.add(stem)
    for f in part.get("frames", []):
        if f.get("locked"):
            names.add(f["name"])
    return names


def golden_diff(project, part):
    frame = next((f for f in part.get("frames", []) if f["name"] == "01_stand"), None)
    g = os.path.join(project, "golden", "01_stand.txt")
    if frame is None or not os.path.isfile(g):
        return None
    mine = frame["grid"].strip("\n").split("\n")
    gold = open(g, encoding="utf-8").read().strip("\n").split("\n")
    n = 0
    for y in range(max(len(mine), len(gold))):
        a = mine[y] if y < len(mine) else ""
        b = gold[y] if y < len(gold) else ""
        n += sum(1 for x in range(max(len(a), len(b))) if (a[x:x + 1] or ".") != (b[x:x + 1] or "."))
    return n


def measure(pxl, part_path, project):
    part = read_part(part_path)
    skip = exemplar_frames(part)
    tmp = tempfile.mkdtemp(prefix="pxl_bench_")
    try:
        r = subprocess.run([pxl, "--json", "compare", part_path, "--out", tmp], capture_output=True, text=True)
        try:
            d = json.loads(r.stdout)
        except ValueError:
            sys.exit("bench: compare printed no JSON:\n" + r.stdout + r.stderr)
        if not d.get("ok"):
            sys.exit("bench: compare failed: " + json.dumps(d.get("error")))
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    frames = [f for f in d["frames"] if f["frame"] not in skip]
    if not frames:
        sys.exit("bench: every frame is an exemplar; nothing to measure")
    n = len(frames)
    oks = [f["oklab"] for f in frames if f.get("oklab") is not None]
    rec = {
        "mean_iou": round(sum(f["iou"] for f in frames) / n, 4),
        "worst_iou": round(min(f["iou"] for f in frames), 4),
        "mean_oklab": round(sum(oks) / len(oks), 4) if oks else None,
        "lost_total": sum(len(f.get("lost") or []) for f in frames),
        "mean_score": round(sum(f["score"] for f in frames) / n, 2),
        "lint_warnings": sum(f.get("lint_warnings", 0) for f in frames),
        "lint_errors": sum(f.get("lint_errors", 0) for f in frames),
        "golden_01_stand_diff": golden_diff(project, part) if "01_stand" not in skip else None,
    }
    return rec, n, sorted(skip)


def last_record(path):
    if not os.path.isfile(path):
        return None
    last = None
    with open(path, encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if line:
                last = json.loads(line)
    return last


def fmt(v):
    return "-" if v is None else (("%g" % v) if isinstance(v, float) else str(v))


def main():
    ap = argparse.ArgumentParser(description="append a quality record for a part to <project>/bench/history.jsonl")
    ap.add_argument("part")
    ap.add_argument("--label", required=True)
    ap.add_argument("--pxl")
    ap.add_argument("--project")
    ap.add_argument("--dry-run", action="store_true")
    a = ap.parse_args()
    part_path = os.path.abspath(a.part)
    if not os.path.isfile(part_path):
        sys.exit("bench: part not found: " + a.part)
    project = project_of(part_path, a.project)
    hist = os.path.join(project, "bench", "history.jsonl")
    pxl = find_pxl(a.pxl)
    metrics, n, skipped = measure(pxl, part_path, project)
    rec = {"label": a.label, "date": time.strftime("%Y-%m-%d")}
    rec.update(metrics)
    prev = last_record(hist)
    print("%d frames measured, %d exemplar(s) excluded: %s" % (n, len(skipped), ", ".join(skipped) or "-"))
    print("%-22s %10s %10s %10s" % ("", "previous", "now", "delta"))
    for k, better in FIELDS:
        now = rec[k]
        old = prev.get(k) if prev else None
        if now is None or old is None:
            delta = ""
        else:
            diff = now - old
            mark = "=" if diff == 0 else ("better" if diff * better > 0 else "WORSE")
            delta = "%+g  %s" % (round(diff, 4), mark)
        print("%-22s %10s %10s   %s" % (k, fmt(old), fmt(now), delta))
    if prev:
        print("previous: " + prev.get("label", "?"))
    line = json.dumps(rec)
    if a.dry_run:
        print("dry run, not written:\n" + line)
        return
    os.makedirs(os.path.dirname(hist), exist_ok=True)
    with open(hist, "a", encoding="utf-8") as f:
        f.write(line + "\n")
    print("appended to " + hist)


if __name__ == "__main__":
    main()
