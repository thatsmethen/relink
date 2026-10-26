#!/usr/bin/env python3
"""Render ReLink's original geometric mixer mark. Requires Pillow 10.3+.

No downloaded artwork, font or vendor mark is used. Geometry is shared with
an editable SVG. PNG, ICO, ICNS and MSVC resource outputs are checked in, so
normal Rust application builds do not need Python or Pillow.
"""
from pathlib import Path
import io
import struct
from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / 'packaging/branding'
OUT.mkdir(parents=True, exist_ok=True)
BG = '#171e29'
MINT = '#70dec7'
LIGHT = '#e4fff9'
RAILS = [(320, 400), (512, 608), (704, 320)]


def render(size):
    # Render analytic geometry at four samples per pixel before downsampling.
    scale = size * 4 / 1024
    canvas = Image.new('RGBA', (size * 4, size * 4), (0, 0, 0, 0))
    draw = ImageDraw.Draw(canvas)
    def rect(box, radius, fill):
        draw.rounded_rectangle(tuple(round(p * scale) for p in box), radius=round(radius * scale), fill=fill)
    rect((48, 48, 976, 976), 216, BG)
    for x, knob_y in RAILS:
        rect((x - 24, 240, x + 24, 784), 24, MINT)
        rect((x - 80, knob_y - 44, x + 80, knob_y + 44), 24, LIGHT)
    return canvas.resize((size, size), Image.Resampling.LANCZOS)


svg = ['<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024">',
       '<title>ReLink mixer mark</title>',
       f'<rect x="48" y="48" width="928" height="928" rx="216" fill="{BG}"/>']
for x, y in RAILS:
    svg += [f'<rect x="{x-24}" y="240" width="48" height="544" rx="24" fill="{MINT}"/>',
            f'<rect x="{x-80}" y="{y-44}" width="160" height="88" rx="24" fill="{LIGHT}"/>']
(OUT / 'relink.svg').write_text('\n'.join(svg + ['</svg>', '']), encoding='utf-8')
render(1024).save(OUT / 'relink.png')
render(256).save(OUT / 'relink-256.png')
render(1024).save(OUT / 'relink.icns')

# PNG icon entries are supported by Windows Vista and later. Each level is
# independently rendered so small icons retain clean anti-aliased geometry.
images = []
for size in (16, 24, 32, 48, 64, 128, 256):
    stream = io.BytesIO()
    render(size).save(stream, format='PNG')
    images.append((size, stream.getvalue()))
header = struct.pack('<HHH', 0, 1, len(images))
entries, payload, offset = [], [], 6 + 16 * len(images)
for size, data in images:
    entries.append(struct.pack('<BBBBHHII', size % 256, size % 256, 0, 0, 1, 32, len(data), offset))
    payload.append(data)
    offset += len(data)
(OUT / 'relink.ico').write_bytes(header + b''.join(entries + payload))


def resource(kind, name, data, flags=0x1030):
    header = struct.pack('<IIHHHHIHHII', len(data), 32, 0xffff, kind, 0xffff, name, 0, flags, 0, 0, 0)
    return header + data + b'\0' * (-len(data) % 4)


# Standard Win32 .res records, consumed directly by the MSVC linker. This
# avoids an extra Rust build dependency or a local Resource Compiler install.
records = [resource(0, 0, b'', flags=0)]
group = [header]
for index, (size, data) in enumerate(images, 1):
    records.append(resource(3, index, data))  # RT_ICON
    group.append(struct.pack('<BBBBHHIH', size % 256, size % 256, 0, 0, 1, 32, len(data), index))
records.append(resource(14, 1, b''.join(group)))  # RT_GROUP_ICON
(OUT / 'relink.res').write_bytes(b''.join(records))
print('Rendered original ReLink mark, macOS icon and Windows resources.')
