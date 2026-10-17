# Examples

Original art (CC0, see `LICENSE`) and two demo scripts. Nothing here is derived from a third-party game.

```
inputs/hero/hero_01..03.png         a character, three poses: two white eyes with pupils, an identity colour (red shoes), orange fur, teal suit (32x48 = 2x of 16x24)
inputs/enemy/crawler_01..02.png     a small enemy with eyes, two frames (32x32)
inputs/item/gem.png                 an item (32x32)
inputs/tiles/ground.png             a 32x32 tile region at 2x (64x64): grass, edge, soil, stones
inputs/screens/title_WxH.png        a title screen for each console's screen size, at 2x (320x288 for the 160x144 handhelds)
gen_inputs.rs                       the generator: cargo run --example gen_inputs   (deterministic; rewrites inputs/)
single/run.sh [console] [workdir]   single-image demo: pxl image, lint, export
project/run.sh [console] [workdir]  project demo: new, ingest, intake, style propose, plan, build, deliver --dry-run
```

Run them with `pxl` on PATH or `PXL=/path/to/pxl`. The console is the first argument or the `CONSOLE` variable (any id of `pxl console list`); the work
folder defaults to a new temp dir. Both scripts print where the compare image and the manifest are.

The inputs use a "source style": 16-bit-era shading, outline, a palette of about 25 colours, 2x the target size. The automatic first version is a
rough sketch (small eyes and accents can be lost: `pxl lint --rules eye_parity,identity_colour` says where); hand-finishing is the agents' job (see the skills).
