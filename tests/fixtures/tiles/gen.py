#!/usr/bin/env python3
"""Generates the synthetic tile fixtures (no third party art). Run from this folder: python3 gen.py
Layout: templates/tiles.toml ; lint/<rule>/{pos1..3,neg1..3}.pxl for tile_colors, tile_palettes, unique_tiles."""
import os, shutil

HERE = os.path.dirname(os.path.abspath(__file__))
os.chdir(HERE)
for d in ('lint', 'templates'):
    if os.path.isdir(d):
        shutil.rmtree(d)

KEYS = 'abcdefghijklmnopqrstuvwxy'
COLS = ['#88c8f8', '#f8f8f8', '#183058', '#28a048', '#80e060', '#386818', '#a86830', '#704018', '#d8a060',
        '#808890', '#505860', '#c0c8d0', '#2060c8', '#60a0f8', '#a8d8f8', '#c88820', '#f8d848', '#f8a090',
        '#c83030', '#701818', '#303030', '#a0a0a0', '#e8e8e8', '#885088', '#d0a8d0']
PALS = {'sky': 'abcd', 'grass': 'aefg', 'dirt': 'ahij', 'rock': 'aklm', 'water': 'anop', 'wood': 'aqrs', 'metal': 'atuv', 'misc': 'awxy'}

def w(path, text):
    os.makedirs(os.path.dirname(path) or '.', exist_ok=True)
    open(path, 'w').write(text)

t = 'name = "tiles"\n\n[layers.base]\n'
for k, c in zip(KEYS, COLS):
    t += '"%s" = { role = "%s", color = "%s" }\n' % (k, 'air' if k == 'a' else 'c_' + k, c)
t += '\n[tiles]\nair = ["a"]\n\n[palettes]\n'
for n, ks in PALS.items():
    t += '%s = "%s"\n' % (n, ks)
w('templates/tiles.toml', t)

def tile(rows):
    assert len(rows) == 8 and all(len(r) == 8 for r in rows), rows
    return rows

def solid(k):
    return tile([k * 8] * 8)

def sheet(tiles, padfirst=False):
    n = len(tiles)
    rows = (n + 15) // 16
    out = []
    for r in range(rows):
        for y in range(8):
            line = ''
            for c in range(16):
                i = r * 16 + c
                line += tiles[i][y] if i < n else (tiles[0][y] if padfirst else 'a' * 8)
            out.append(line)
    return out

def part(path, tiles, name):
    g = sheet(tiles, 'unique_tiles' in path)
    text = 'name = "%s"\nkind = "tileset"\nsize = [128, %d]\ntile = [8, 8]\ntemplate = "templates/tiles.toml"\n\n[[frames]]\nname = "blocks"\ngrid = """\n%s\n"""\n' % (name, len(g), '\n'.join(g))
    w(path, text)

def numbered(n, ka='a', kb='b', kc='c'):
    """tile n: marker kc at (0,0), bits of n over 24 pixels of rows 1-3 -> unique up to flips."""
    rows = [[ka] * 8 for _ in range(8)]
    rows[0][0] = kc
    for bit in range(24):
        if n >> bit & 1:
            rows[1 + bit // 8][bit % 8] = kb
    return tile([''.join(r) for r in rows])

def flipx(t):
    return [r[::-1] for r in t]

def many(keys):
    """one tile with len(keys) distinct colours, one per pixel of the first rows."""
    rows = [[keys[0]] * 8 for _ in range(8)]
    for i, k in enumerate(keys):
        rows[i // 8][i % 8] = k
    return tile([''.join(r) for r in rows])

filler = [solid('a')] * 3
# ---- tile_colors
part('lint/tile_colors/pos1.pxl', filler + [many('abcde')], 'pos1')
part('lint/tile_colors/pos2.pxl', [many('abcdefgh')] + filler, 'pos2')
part('lint/tile_colors/pos3.pxl', [many('abcdef'), solid('b'), many('aeghijk')], 'pos3')
part('lint/tile_colors/neg1.pxl', filler + [many('abcd')], 'neg1')
part('lint/tile_colors/neg2.pxl', [solid('a'), many('ab'), many('abc')], 'neg2')
part('lint/tile_colors/neg3.pxl', [many('aefg'), many('ahij'), many('aklm'), many('anop')], 'neg3')

# ---- tile_palettes: distinct 4-sets cannot share a palette
SETS = ['abcd', 'efgh', 'ijkl', 'mnop', 'qrst', 'uvwx', 'abef', 'cdgh', 'ijmn', 'klop', 'qruv', 'stwx', 'aceg', 'bdfh']
def four(i):
    return many(SETS[i])
part('lint/tile_palettes/pos1.pxl', [four(i) for i in range(9)], 'pos1')
part('lint/tile_palettes/pos2.pxl', [four(i) for i in range(12)], 'pos2')
part('lint/tile_palettes/pos3.pxl', [four(i) for i in range(14)] + [solid('a')] * 3, 'pos3')
part('lint/tile_palettes/neg1.pxl', [four(i) for i in range(8)], 'neg1')
part('lint/tile_palettes/neg2.pxl', [many('ab'), many('cd'), many('ef'), many('gh'), many('ij'), many('kl'), many('mn'), many('op'), many('qr'), many('st')], 'neg2')
part('lint/tile_palettes/neg3.pxl', [solid(k) for k in 'abcdefghijklmnopqrstuvwxy'], 'neg3')

# ---- unique_tiles: > 300 unique tiles (flips count once)
part('lint/unique_tiles/pos1.pxl', [numbered(n) for n in range(301)], 'pos1')
part('lint/unique_tiles/pos2.pxl', [numbered(n) for n in range(320)], 'pos2')
part('lint/unique_tiles/pos3.pxl', [numbered(n) for n in range(400)], 'pos3')
part('lint/unique_tiles/neg1.pxl', [numbered(n) for n in range(300)], 'neg1')
part('lint/unique_tiles/neg2.pxl', [numbered(n % 100 + 1) if n < 200 else flipx(numbered(n % 100 + 1)) for n in range(400)], 'neg2')
part('lint/unique_tiles/neg3.pxl', [numbered(n) for n in range(100)], 'neg3')
print('ok')
