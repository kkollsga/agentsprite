# Delivery

## What the client receives

A package that loads without questions: the console-format files, `export/manifest.json` (machine readable), DELIVERY.md (the human
version), the change log and a handover note. The manifest is the contract: prose is not.

## The manifest carries (written by `pxl build`)

- per part: palette variants and stable palette ids, origins, frame semantics (distinct or engine-flipped), dropped frames with reasons;
- per scene: palette tables resolved per context, ready to load in the console's colour format;
- `repeat` (stride, count), `compose` (body plus icon with offsets), `collision` vs visual width;
- dual-use objects: the BG-palette version beside the sprite version;
- residency (resident or streamed) per group.

Set them with commands, not by hand: `pxl meta ASSET set collision 48x8`, `pxl meta ASSET frame NAME dropped true --reason WHY`,
`pxl meta ASSET zone-remap SCENE 4=5`, `pxl variant add | deliver`, `pxl exception add ASSET "change" --reason "why"`.

## Checks before handing over

```
pxl check --hand-finished        # lint, exact outputs, scenes, "checked N of M" (and references checked)
pxl verify-package               # the package against the manifest
pxl deliver --dry-run            # all of it, nothing snapshotted
pxl deliver [--client-checker CMD] [--client-pipeline CMD]   # snapshot to deliveries/N/ and refuse on our failures
pxl changes --since 1            # sizes, frames and footprint changes since a delivery
```

A client checker that passes is necessary, not sufficient: it checks files one at a time, and only the scene checks prove that the
player, an enemy, a platform and a ring fit on one screen. When the client's converter or emulator is available, run it on the package
and look at in-game screenshots before the handover.

## After delivery

Keep the delivery snapshot. The next delivery's change log is computed against it. Record what the client changed in their converters:
each workaround is a feature request to your own manifest.
