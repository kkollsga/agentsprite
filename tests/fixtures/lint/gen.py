#!/usr/bin/env python3
"""Generates the synthetic lint fixtures (no third party art). Run from this folder: python3 gen.py
Layout: <rule>/{pos1..3,neg1..3}.pxl ; family/<case>/*.pxl ; refs/*.png ; templates/*.toml
A file `<case>.profile.toml` next to a case overrides the profile for that case."""
import os, shutil
from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))
os.chdir(HERE)
for d in os.listdir('.'):
    if os.path.isdir(d):
        shutil.rmtree(d)

COL = {'1': (0x18, 0x20, 0x78), '2': (0x48, 0x50, 0xb8), '3': (0xf0, 0xb0, 0x90),
       'W': (0xf8, 0xf8, 0xf8), 'K': (0, 0, 0), 'R': (0xf8, 0, 0), 'r': (0x90, 0, 0), '4': (0x80, 0xc0, 0x80)}

BASE = '''name = "%s"

[layers.base]
"." = { role = "transparent" }
"1" = { role = "outline", color = "#182078" }
"2" = { role = "body", color = "%s" }
"3" = { role = "skin", color = "#f0b090" }
%s
[layers.detail]
"W" = { role = "white", color = "#f8f8f8", under = "3" }
"R" = { role = "shoe", color = "#f80000", under = "1" }
"r" = { role = "shoe_dark", color = "#900000", under = "1" }
%s'''

def w(path, text):
    os.makedirs(os.path.dirname(path) or '.', exist_ok=True)
    open(path, 'w').write(text)

w('templates/t.toml', BASE % ('t', '#4850b8', '', ''))
w('templates/t_map.toml', BASE % ('t_map', '#4850b8', '', '\n[map]\n"#182078" = "1"\n"#4850b8" = "2"\n"#f0b090" = "3"\n"#f8f8f8" = "W"\n"#f80000" = "R"\n"#900000" = "r"\n'))
w('templates/t4.toml', BASE % ('t4', '#4850b8', '"4" = { role = "grass", color = "#80c080" }\n', '"w" = { role = "glow", color = "#f8f8c0", under = "2" }\n'))
w('templates/t_step.toml', BASE % ('t_step', '#4851b9', '', ''))
w('templates/t_alpha.toml', BASE % ('t_alpha', '#4850b8', '"x" = { role = "transparent" }\n', ''))
w('templates/t_dup.toml', BASE % ('t_dup', '#182078', '', ''))
w('templates/t_dup2.toml', (BASE % ('t_dup2', '#4850b8', '', '')).replace('color = "#f8f8f8"', 'color = "#4850b8"'))

def canvas(wd=16, ht=16):
    return [['.'] * wd for _ in range(ht)]

def rect(c, x0, y0, x1, y1, o='1', f='2'):
    for y in range(y0, y1 + 1):
        for x in range(x0, x1 + 1):
            edge = x in (x0, x1) or y in (y0, y1)
            c[y][x] = o if edge else f
    return c

def blob(wd=16, ht=16, x0=3, y0=3, x1=12, y1=12):
    c = rect(canvas(wd, ht), x0, y0, x1, y1)
    for y in range(y0 + 2, y0 + 4):
        for x in range(x0 + 2, x0 + 5):
            c[y][x] = '3'
    return c

def rows(c):
    return [''.join(r) for r in c]

def frame(name, c, src=None):
    s = '\n[[frames]]\nname = "%s"\n' % name
    if src:
        s += 'src = "%s"\n' % src
    return s + 'grid = """\n' + '\n'.join(rows(c)) + '\n"""\n'

def part(path, frames, size=(16, 16), tpl='templates/t.toml', refs=False, colors=None, name='fx', family=None, extra='', budget=None):
    s = 'name = "%s"\nsize = [%d, %d]\norigin = [0, 0]\n' % (name, size[0], size[1])
    if tpl:
        s += 'template = "%s"\n' % tpl
    if family:
        s += 'family = "%s"\n' % family
    s += extra
    if budget:
        s += '\n[budget]\n' + budget
    if refs:
        s += '\n[input]\nimages = "refs/{frame}.png"\nfactor = 2\n'
    if colors:
        s += '\n[colors]\n' + ''.join('"%s" = "%s"\n' % kv for kv in colors.items())
    w(path, s + ''.join(frames))

def ref_png(name, c):
    h, wd = len(c), len(c[0])
    im = Image.new('RGBA', (wd * 2, h * 2), (0, 0, 0, 0))
    for y in range(h):
        for x in range(wd):
            k = c[y][x]
            if k != '.':
                for dy in (0, 1):
                    for dx in (0, 1):
                        im.putpixel((x * 2 + dx, y * 2 + dy), COL[k] + (255,))
    os.makedirs('refs', exist_ok=True)
    im.save('refs/%s.png' % name)

def put(c, pts, k):
    c = [r[:] for r in c]
    for x, y in pts:
        c[y][x] = k
    return c

# ---- size (profile with a canvas size)
for i, (prof, sz) in enumerate([('size = [32, 32]', (16, 16)), ('size = [16, 32]', (16, 16)), ('size = [8, 8]', (16, 16))], 1):
    part('size/pos%d.pxl' % i, [frame('a', blob())], size=sz)
    w('size/pos%d.profile.toml' % i, prof + '\n')
part('size/neg1.pxl', [frame('a', blob(32, 32, 3, 3, 12, 12))], size=(32, 32)); w('size/neg1.profile.toml', 'size = [32, 32]\n')
part('size/neg2.pxl', [frame('a', blob())])
part('size/neg3.pxl', [frame('a', blob())]); w('size/neg3.profile.toml', 'size = [16, 16]\n')

# ---- palette_size
part('palette_size/pos1.pxl', [frame('a', put(blob(), [(5, 8), (6, 8)], '4'))], tpl='templates/t4.toml')
c = blob(); c = put(c, [(5, 8)], '4'); c = put(c, [(6, 8)], '2')
part('palette_size/pos2.pxl', [frame('a', blob()), frame('b', [[('4' if ch == '3' else ch) for ch in r] for r in c])], tpl='templates/t4.toml')
c = blob(); c = put(c, [(5, 9)], 'W'); c = put(c, [(6, 9)], 'R'); c = put(c, [(7, 9)], 'r'); c = put(c, [(8, 9)], 'w')
part('palette_size/pos3.pxl', [frame('a', c)], tpl='templates/t4.toml')
part('palette_size/neg1.pxl', [frame('a', blob())])
c = blob(); c = put(c, [(5, 9)], 'W'); c = put(c, [(6, 9)], 'R'); c = put(c, [(7, 9)], 'r')
part('palette_size/neg2.pxl', [frame('a', c)])
part('palette_size/neg3.pxl', [frame('a', rect(canvas(), 3, 3, 12, 12))])

# ---- rgb_step
part('rgb_step/pos1.pxl', [frame('a', blob())], tpl='templates/t_step.toml')
part('rgb_step/pos2.pxl', [frame('a', rect(canvas(), 2, 2, 8, 8)), frame('b', blob())], tpl='templates/t_step.toml')
part('rgb_step/pos3.pxl', [frame('a', blob())], colors={'2': '#010203'})
part('rgb_step/neg1.pxl', [frame('a', blob())])
part('rgb_step/neg2.pxl', [frame('a', rect(canvas(), 3, 3, 12, 12, '1', '3'))], tpl='templates/t_step.toml')
part('rgb_step/neg3.pxl', [frame('a', blob())], colors={'2': '#404040'})

# ---- alpha
for i, pts in enumerate([[(5, 8)], [(6, 8), (7, 8)], [(9, 9)]], 1):
    part('alpha/pos%d.pxl' % i, [frame('a', put(blob(), pts, 'x'))], tpl='templates/t_alpha.toml')
part('alpha/neg1.pxl', [frame('a', blob())])
part('alpha/neg2.pxl', [frame('a', blob())], tpl='templates/t_alpha.toml')
part('alpha/neg3.pxl', [frame('a', put(blob(), [(5, 8)], 'W'))])

# ---- shared_palette
part('shared_palette/pos1.pxl', [frame('a', blob())], tpl='templates/t_dup.toml')
part('shared_palette/pos2.pxl', [frame('a', rect(canvas(), 3, 3, 12, 12, '1', '3')), frame('b', blob())], tpl='templates/t_dup.toml')
part('shared_palette/pos3.pxl', [frame('a', put(blob(), [(9, 9)], '2'))], tpl='templates/t_dup.toml')
part('shared_palette/neg1.pxl', [frame('a', blob())])
part('shared_palette/neg2.pxl', [frame('a', rect(canvas(), 3, 3, 12, 12, '1', '3'))], tpl='templates/t_dup.toml')
part('shared_palette/neg3.pxl', [frame('a', put(blob(), [(9, 9)], 'W'))], tpl='templates/t_dup2.toml')

# ---- empty_frame
part('empty_frame/pos1.pxl', [frame('a', canvas())])
part('empty_frame/pos2.pxl', [frame('a', blob()), frame('b', canvas())])
part('empty_frame/pos3.pxl', [frame('a', canvas()), frame('b', canvas())], size=(16, 16))
part('empty_frame/neg1.pxl', [frame('a', blob())])
part('empty_frame/neg2.pxl', [frame('a', put(canvas(), [(0, 0)], '1'))])
part('empty_frame/neg3.pxl', [frame('a', blob()), frame('b', blob(x0=4))])

# ---- template (key missing, local override, no template)
part('template/pos1.pxl', [frame('a', put(blob(), [(5, 8)], 'Z'))])
part('template/pos2.pxl', [frame('a', blob())], colors={'2': '#4850b8'})
part('template/pos3.pxl', [frame('a', blob())], tpl=None)
part('template/neg1.pxl', [frame('a', blob())])
part('template/neg2.pxl', [frame('a', put(blob(), [(5, 8)], 'W'))])
part('template/neg3.pxl', [frame('a', blob()), frame('b', canvas())], tpl='templates/t4.toml')

# ---- family (AC-TP3): each case is a directory of parts
def fam(case, specs):
    for n, tpl in specs:
        part('family/%s/%s.pxl' % (case, n), [frame('a', blob())], tpl=tpl, name=n, family='fam')
fam('pos1', [('a', 'templates/t.toml'), ('b', 'templates/t4.toml')])
fam('pos2', [('a', 'templates/t.toml'), ('b', 'templates/t.toml'), ('c', 'templates/t_step.toml')])
fam('pos3', [('a', 'templates/t_dup.toml'), ('b', 'templates/t.toml'), ('c', 'templates/t.toml'), ('d', 'templates/t.toml')])
fam('neg1', [('a', 'templates/t.toml'), ('b', 'templates/t.toml')])
fam('neg2', [('a', 'templates/t4.toml')])
fam('neg3', [('a', 'templates/t.toml'), ('b', 'templates/t.toml'), ('c', 'templates/t.toml')])

# ---- hw_cells
def full(wd, ht):
    return rect(canvas(wd, ht), 0, 0, wd - 1, ht - 1)
part('hw_cells/pos1.pxl', [frame('a', full(64, 32))], size=(64, 32))
part('hw_cells/pos2.pxl', [frame('a', full(48, 16))], size=(48, 16))
part('hw_cells/pos3.pxl', [frame('a', full(40, 48))], size=(40, 48))
part('hw_cells/neg1.pxl', [frame('a', blob())])
part('hw_cells/neg2.pxl', [frame('a', full(40, 16))], size=(40, 16))
part('hw_cells/neg3.pxl', [frame('a', full(32, 48))], size=(32, 48))

# ---- overlay layer is a separate palette: base cells + overlay cells per band
def with_overlay(wd, ht, xs, y=4):
    return put(full(wd, ht), [(x, y) for x in xs], 'W')
part('hw_cells/pos4.pxl', [frame('a', with_overlay(40, 16, [20]))], size=(40, 16))
part('hw_cells/neg4.pxl', [frame('a', with_overlay(40, 16, []))], size=(40, 16))

# ---- hw_limit (10 per band, 40 per frame)
part('hw_limit/pos4.pxl', [frame('a', with_overlay(48, 16, [4, 12, 20, 28, 36, 44]))], size=(48, 16))
part('hw_limit/neg4.pxl', [frame('a', with_overlay(40, 16, [4, 12, 20, 28, 36]))], size=(40, 16))
part('hw_limit/pos1.pxl', [frame('a', full(88, 16))], size=(88, 16))
part('hw_limit/pos2.pxl', [frame('a', full(96, 32))], size=(96, 32))
part('hw_limit/pos3.pxl', [frame('a', full(80, 80))], size=(80, 80))
part('hw_limit/neg1.pxl', [frame('a', full(80, 16))], size=(80, 16))
part('hw_limit/neg2.pxl', [frame('a', full(64, 64))], size=(64, 64))
part('hw_limit/neg3.pxl', [frame('a', blob())])

# ---- palette sets (M5): palettes/p.toml is the shared set of the palette_set / cell_palette / hw_limit fixtures
w('palettes/p.toml', '''[[slots]]
slot = 0
name = "body"
colors = ["#182078", "#4850b8", "#f0b090"]

[[slots]]
slot = 1
name = "red"
colors = ["#900000", "#f80000", "#f8f8f8"]
''')
PS = 'palette_set = "p"\n'

def solid(wd, ht):
    return rect(canvas(wd, ht), 0, 0, wd - 1, ht - 1)

# palette_set: a colour outside the set, a slot restriction, a missing set
part('palette_set/pos1.pxl', [frame('a', put(blob(), [(5, 8)], '4'))], tpl='templates/t4.toml', extra=PS)
part('palette_set/pos2.pxl', [frame('a', blob())], extra=PS + 'slots = [1]\n')
part('palette_set/pos3.pxl', [frame('a', blob())], extra='palette_set = "missing"\n')
part('palette_set/neg1.pxl', [frame('a', blob())], extra=PS)
part('palette_set/neg2.pxl', [frame('a', blob())], extra=PS + 'slots = [0]\n')
part('palette_set/neg3.pxl', [frame('a', put(canvas(), [(4, 4), (5, 4), (6, 4), (7, 4)], 'R'))], extra=PS)

# cell_palette: a cell that no slot holds, a cell that needs two slots outside the player, a third two-slot cell
chk = canvas(8, 16)
for y in range(16):
    for x in range(8):
        chk[y][x] = '123W'[(x + y) % 4]
part('cell_palette/pos1.pxl', [frame('a', chk)], size=(8, 16), extra=PS)
mix = canvas(8, 16)
for y in range(16):
    for x in range(8):
        mix[y][x] = '1R'[(x + y) % 2]
part('cell_palette/pos2.pxl', [frame('a', mix)], size=(8, 16), extra=PS)
part('cell_palette/pos3.pxl', [frame('a', put(solid(24, 16), [(2, 8), (10, 8), (18, 8)], 'R'))], size=(24, 16), extra=PS + 'category = "player"\n')
part('cell_palette/neg1.pxl', [frame('a', blob())], extra=PS)
part('cell_palette/neg2.pxl', [frame('a', put(solid(16, 16), [(4, 8), (12, 8)], 'R'))], extra=PS + 'category = "player"\n')
halves = canvas(16, 16)
for y in range(16):
    for x in range(16):
        halves[y][x] = '2' if x < 8 else 'R'
part('cell_palette/neg3.pxl', [frame('a', halves)], extra=PS)

# hw_limit by category (the old grid fixtures are pos1-4 / neg1-4)
part('hw_limit/pos5.pxl', [frame('a', solid(40, 16))], size=(40, 16), extra=PS + 'category = "enemy"\n')
part('hw_limit/pos6.pxl', [frame('a', solid(32, 32))], size=(32, 32), extra=PS + 'category = "item"\n')
part('hw_limit/pos7.pxl', [frame('a', put(solid(32, 16), [(12, 8), (28, 8)], 'R'))], size=(32, 16), extra=PS + 'category = "player"\n')
part('hw_limit/pos8.pxl', [frame('a', solid(24, 48))], size=(24, 48), extra=PS + 'category = "enemy"\n')
part('hw_limit/neg5.pxl', [frame('a', solid(32, 16))], size=(32, 16), extra=PS + 'category = "enemy"\n')
part('hw_limit/neg6.pxl', [frame('a', solid(32, 32))], size=(32, 32), extra=PS + 'category = "item"\n', budget='per_line = 4\nper_frame = 8\n')
part('hw_limit/neg7.pxl', [frame('a', put(solid(32, 16), [(12, 8)], 'R'))], size=(32, 16), extra=PS + 'category = "player"\n')
part('hw_limit/neg8.pxl', [frame('a', solid(48, 16))], size=(48, 16), extra=PS + 'category = "platform"\n')

# ---- overlay (gbc profiles: detail layers only for the player, capped at 2 cells)
part('overlay/pos1.pxl', [frame('a', put(blob(), [(6, 7)], 'W'))])
part('overlay/pos2.pxl', [frame('a', put(blob(), [(6, 7)], 'R'))], extra='category = "enemy"\n')
part('overlay/pos3.pxl', [frame('a', put(rect(canvas(32, 16), 0, 0, 31, 15), [(2, 7), (10, 7), (18, 7)], 'W'))], size=(32, 16), extra='category = "player"\n')
part('overlay/neg1.pxl', [frame('a', blob())])
part('overlay/neg2.pxl', [frame('a', put(blob(), [(6, 7)], 'W'))], extra='category = "player"\n')
part('overlay/neg3.pxl', [frame('a', put(blob(), [(6, 7)], 'W'))]); w('overlay/neg3.profile.toml', 'overlays = "allow"\n')

# ---- orphan
part('orphan/pos1.pxl', [frame('a', put(blob(), [(14, 14)], '1'))])
part('orphan/pos2.pxl', [frame('a', put(blob(), [(0, 5)], '2'))])
part('orphan/pos3.pxl', [frame('a', put(blob(), [(1, 1), (14, 1)], '3'))])
part('orphan/neg1.pxl', [frame('a', blob())])
part('orphan/neg2.pxl', [frame('a', put(blob(), [(6, 6)], '1'))])
part('orphan/neg3.pxl', [frame('a', put(blob(), [(13, 13)], '1'))])

# ---- jaggy
def pts(l): return put(canvas(), l, '1')
part('jaggy/pos1.pxl', [frame('a', pts([(2, 2), (3, 2), (3, 3), (4, 3), (4, 4), (5, 4), (5, 5), (6, 5)]))])
part('jaggy/pos2.pxl', [frame('a', pts([(10, 2), (9, 2), (9, 3), (8, 3), (8, 4), (7, 4), (7, 5), (6, 5)]))])
part('jaggy/pos3.pxl', [frame('a', pts([(2, 2), (2, 3), (3, 3), (3, 4), (4, 4), (4, 5), (5, 5), (5, 6)]))])
part('jaggy/neg1.pxl', [frame('a', rect(canvas(), 3, 3, 12, 12, '1', '1'))])
part('jaggy/neg2.pxl', [frame('a', pts([(2, 2), (3, 3), (4, 4), (5, 5), (6, 6), (7, 7)]))])
part('jaggy/neg3.pxl', [frame('a', pts([(2, 2), (3, 2), (4, 3), (5, 3), (6, 4), (7, 4), (8, 5), (9, 5)]))])

# ---- noise
def checker(wd, ht, x0=2, y0=2, a='1', b='2'):
    c = canvas()
    for y in range(ht):
        for x in range(wd):
            c[y0 + y][x0 + x] = a if (x + y) % 2 == 0 else b
    return c
part('noise/pos1.pxl', [frame('a', checker(4, 4))])
part('noise/pos2.pxl', [frame('a', checker(3, 2))])
part('noise/pos3.pxl', [frame('a', checker(6, 6, 4, 4, '2', '3'))])
part('noise/neg1.pxl', [frame('a', checker(2, 2))])
part('noise/neg2.pxl', [frame('a', [['1' if x % 2 == 0 else '2' for x in range(16)] if 2 <= y < 8 else ['.'] * 16 for y in range(16)])])
part('noise/neg3.pxl', [frame('a', blob())])

# ---- jitter (tags from frame names: walk1, walk2 -> "walk")
def at(dy=0, dx=0):
    return blob(16, 16, 3 + dx, 2 + dy, 10 + dx, 10 + dy)
part('jitter/pos1.pxl', [frame('walk1', at()), frame('walk2', at(1)), frame('walk3', at())])
part('jitter/pos2.pxl', [frame('run1', at()), frame('run2', at(2))])
part('jitter/pos3.pxl', [frame('walk1', at()), frame('walk2', at()), frame('walk3', at(-1)), frame('walk4', at())])
part('jitter/neg1.pxl', [frame('walk1', at()), frame('walk2', at()), frame('walk3', at())])
part('jitter/neg2.pxl', [frame('stand', at()), frame('walk1', at(2)), frame('roll1', at(-1))])
part('jitter/neg3.pxl', [frame('01_walk1', at(0, 0)), frame('02_walk2', at(0, 1))])

# ---- references: images = "refs/{frame}.png" with {frame} = frame src, factor 2
def with_ref(path, frames_art, frames_ref, names=None, tpl='templates/t.toml'):
    fr = []
    for i, (a, r) in enumerate(zip(frames_art, frames_ref)):
        nm = (names or ['idle%d' % (j + 1) for j in range(len(frames_art))])[i]
        src = '%s_%s' % (path.replace('/', '_').replace('.pxl', ''), nm)
        ref_png(src, r)
        fr.append(frame(nm, a, src))
    part(path, fr, refs=True, tpl=tpl)

A = blob()
# shimmer: art changes where the reference does not
with_ref('shimmer/pos1.pxl', [A, put(A, [(8, 8)], '3')], [A, A])
with_ref('shimmer/pos2.pxl', [A, put(A, [(5, 9), (6, 9)], '3')], [A, A])
with_ref('shimmer/pos3.pxl', [A, put(A, [(10, 5)], '3')], [A, put(A, [(5, 5)], '3')])
with_ref('shimmer/neg1.pxl', [A, put(A, [(8, 8)], '3')], [A, put(A, [(8, 8)], '3')])
with_ref('shimmer/neg2.pxl', [A, A], [A, A])
with_ref('shimmer/neg3.pxl', [A, put(A, [(8, 8)], '3')], [A, A], names=['idle1', 'walk1'])

# ref_scale: art bbox vs reference bbox at target scale
RW = blob(16, 16, 2, 2, 13, 13)
with_ref('ref_scale/pos1.pxl', [blob(16, 16, 2, 2, 9, 13)], [RW], names=['idle1'])
with_ref('ref_scale/pos2.pxl', [blob(16, 16, 2, 5, 13, 13)], [RW], names=['idle1'])
with_ref('ref_scale/pos3.pxl', [blob(16, 16, 4, 4, 15, 15)], [RW], names=['idle1'])
with_ref('ref_scale/neg1.pxl', [RW], [RW], names=['idle1'])
with_ref('ref_scale/neg2.pxl', [blob(16, 16, 2, 2, 12, 13)], [RW], names=['idle1'])
with_ref('ref_scale/neg3.pxl', [blob(16, 16, 3, 2, 12, 13)], [RW], names=['idle1'])

# lost_feature: a reference detail (dark eye, light highlight, thin dark band) the art lacks
def disc(fill='3', extra=()):
    c = rect(canvas(), 3, 3, 12, 12, '1', fill)
    for (x, y, k) in extra:
        c[y][x] = k
    return c
EYE = [(8, 7, '1')]
HI = [(7, 7, 'W')]
BAND = [(6, 9, '1'), (7, 9, '1'), (8, 9, '1')]
with_ref('lost_feature/pos1.pxl', [disc()], [disc('3', EYE)], names=['idle1'])
with_ref('lost_feature/pos2.pxl', [disc('2')], [disc('2', HI)], names=['idle1'])
with_ref('lost_feature/pos3.pxl', [disc()], [disc('3', BAND)], names=['idle1'])
with_ref('lost_feature/neg1.pxl', [disc('3', EYE)], [disc('3', EYE)], names=['idle1'])
with_ref('lost_feature/neg2.pxl', [disc('2', HI)], [disc('2', HI)], names=['idle1'])
with_ref('lost_feature/neg3.pxl', [disc('3', BAND)], [disc('3', BAND)], names=['idle1'])

# lost_eye: the reference has an eye (white with a black pupil on the right); the art has no eye, a white blob
# without pupil, or the pupil on the wrong side
EYE_REF = put(put(disc(), [(7, 6), (8, 6), (7, 7), (8, 7)], 'W'), [(9, 6), (9, 7)], 'K')
def art_eye(whites, pupils):
    return put(put(disc(), whites, 'W'), pupils, '1')
with_ref('lost_eye/pos1.pxl', [disc()], [EYE_REF], names=['idle1'])
with_ref('lost_eye/pos2.pxl', [art_eye([(7, 6), (8, 6), (7, 7), (8, 7)], [])], [EYE_REF], names=['idle1'])
with_ref('lost_eye/pos3.pxl', [art_eye([(7, 6), (8, 6), (7, 7), (8, 7)], [(6, 6), (6, 7)])], [EYE_REF], names=['idle1'])
with_ref('lost_eye/neg1.pxl', [art_eye([(7, 6), (8, 6), (7, 7), (8, 7)], [(9, 6), (9, 7)])], [EYE_REF], names=['idle1'])
with_ref('lost_eye/neg2.pxl', [art_eye([(7, 6), (8, 6), (7, 7), (8, 7)], [(9, 7)])], [EYE_REF], names=['idle1'])
with_ref('lost_eye/neg3.pxl', [art_eye([(7, 6), (8, 6), (7, 7)], [(9, 6), (9, 7)])], [EYE_REF], names=['idle1'])

# colour_share: identity colour (overlay key R, a shoe patch) keeps its share of the reference, not far more
SHOE = [(x, y) for x in range(5, 9) for y in range(9, 11)]
SHOE_REF = put(disc('2'), SHOE, 'R')
with_ref('colour_share/pos1.pxl', [disc('2')], [SHOE_REF], names=['idle1'], tpl='templates/t_map.toml')
with_ref('colour_share/pos2.pxl', [put(disc('2'), SHOE[:2], 'R')], [SHOE_REF], names=['idle1'], tpl='templates/t_map.toml')
with_ref('colour_share/pos3.pxl', [put(disc('2'), [(x, y) for x in range(4, 12) for y in range(7, 11)], 'R')], [SHOE_REF], names=['idle1'], tpl='templates/t_map.toml')
with_ref('colour_share/neg1.pxl', [SHOE_REF], [SHOE_REF], names=['idle1'], tpl='templates/t_map.toml')
with_ref('colour_share/neg2.pxl', [put(disc('2'), SHOE[:6], 'R')], [SHOE_REF], names=['idle1'], tpl='templates/t_map.toml')
with_ref('colour_share/neg3.pxl', [disc('2')], [disc('2')], names=['idle1'], tpl='templates/t_map.toml')
