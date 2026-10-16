"""Draws the guide's figures as inline SVG, in the style of the on-screen UI.

Run from docs/guide: python3 figs/make.py. Each figure replaces whatever is
between <!-- fig:NAME --> and <!-- /fig --> in the pages, so a figure is
changed here and regenerated, never edited in the page. Colors come from
assets/css/guide.css, so the figures follow the light and dark themes. The
screen figure is to scale."""
import os, re, sys

def marker(i):
    return (f'<defs><marker id="ah-{i}" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" '
            f'markerHeight="7" orient="auto-start-reverse"><path d="M0 0L10 5L0 10z" class="ah"/></marker></defs>')

def box(x, y, w, h, lines, hot=False, rx=8):
    cls = 'hot' if hot else 'card'
    out = [f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{rx}" class="{cls}"/>']
    n = len(lines)
    for k, (text, bold) in enumerate(lines):
        cy = y + h / 2 + (k - (n - 1) / 2) * 16 + 4.5
        tcls = ('ht' if hot else 't') if (bold or k == 0) else ('ht' if hot else 'd')
        out.append(f'<text x="{x + w / 2}" y="{cy:.1f}" text-anchor="middle" class="{tcls}{" b" if bold else ""}">{text}</text>')
    return ''.join(out)

def arrow(i, pts):
    d = 'M' + ' L'.join(f'{x} {y}' for x, y in pts)
    return f'<path d="{d}" class="ln" marker-end="url(#ah-{i})"/>'

def text(x, y, s, cls='d', anchor='start', size=None):
    fs = f' font-size="{size}"' if size else ''
    return f'<text x="{x}" y="{y}" text-anchor="{anchor}" class="{cls}"{fs}>{s}</text>'

def figure(svg_w, svg_h, label, body, caption):
    return (f'<figure class="fig"><div class="scroll"><svg viewBox="0 0 {svg_w} {svg_h}" role="img" aria-label="{label}" font-size="13">'
            f'{body}</svg></div><figcaption>{caption}</figcaption></figure>')

figs = {}

# 1. The path of every frame.
i = 'path'
b = [marker(i)]
b.append(text(20, 18, 'Every frame takes the same path, from the player to the screen', 't b', size=15))
top = [(20, ['Player or streamer']), (230, ['Receiver', 'optional']), (440, ['HDMI splitter', 'chooses the EDID'])]
for x, l in top:
    b.append(box(x, 36, 170, 48, [(l[0], True)] + [(s, False) for s in l[1:]]))
b.append(arrow(i, [(190, 60), (228, 60)]))
b.append(arrow(i, [(400, 60), (438, 60)]))
b.append(f'<rect x="20" y="112" width="720" height="236" rx="10" class="box"/>')
b.append(text(728, 134, 'The cinefit PC: one frame every 42 ms at 24 fps', 't b', 'end'))
b.append(arrow(i, [(525, 84), (525, 98), (98, 98), (98, 156)]))
xs = [36, 179, 322, 465, 608]
steps = [(['Capture', 'the frame, and', 'what kind it is'], False),
         (['Find the bars', 'where the picture', 'really is'], True),
         (['Place', 'constant image', 'height'], False),
         (['Render', 'crop, scale, color', 'on the GPU'], False),
         (['Output', 'HDMI out, with', 'HDR signaling'], False)]
for x, (l, hot) in zip(xs, steps):
    b.append(box(x, 158, 124, 64, [(l[0], True)] + [(s, False) for s in l[1:]], hot))
for a, c in zip(xs, xs[1:]):
    b.append(arrow(i, [(a + 124, 190), (c - 2, 190)]))
b.append(box(36, 266, 124, 56, [('Dolby Vision', True), ('metadata read', False), ('from the pixels', False)]))
b.append(arrow(i, [(98, 222), (98, 264)]))
b.append(arrow(i, [(160, 294), (527, 294), (527, 224)]))
b.append(text(190, 286, 'tunneled Dolby Vision only'))
b.append(arrow(i, [(670, 222), (670, 372)]))
b.append(text(660, 364, 'HDMI', anchor='end'))
b.append(box(590, 374, 150, 40, [('Projector or TV', True)]))
figs['path'] = figure(760, 424, 'Every frame takes the same path, from the player to the screen', ''.join(b),
    'One frame\'s path through cinefit. Finding the bars is the clever part: everything after it follows from where the picture is.')

# 1b. The signal chain, and the EDID going the other way.
i = 'chain'
b = [marker(i)]
b.append(text(20, 18, 'The picture goes one way; the EDID goes the other', 't b', size=15))
# The EDID: from the splitter back to the player, dashed, above the boxes.
b.append(f'<path d="M435 72 V46 H90 V70" class="ln" stroke-dasharray="4 3" marker-end="url(#ah-{i})"/>')
b.append(text(262, 40, 'the EDID: what the player may send', 'ht', 'middle'))
b.append(box(20, 72, 140, 52, [('Player', True), ('or streamer', False)]))
b.append(box(190, 72, 140, 52, [('AV receiver', True), ('optional', False)]))
b.append(box(360, 72, 150, 52, [('HDMI splitter', True), ('chooses the EDID', False)], hot=True))
b.append(arrow(i, [(160, 98), (188, 98)]))
b.append(arrow(i, [(330, 98), (358, 98)]))
b.append(f'<rect x="540" y="56" width="200" height="190" rx="10" class="box"/>')
b.append(text(728, 76, 'The cinefit PC', 't b', 'end'))
b.append(box(556, 86, 168, 48, [('Capture card', True), ('HDMI in', False)]))
b.append(box(556, 176, 168, 48, [('Graphics card', True), ('HDMI out', False)]))
b.append(arrow(i, [(510, 110), (554, 110)]))
b.append(arrow(i, [(640, 134), (640, 174)]))
b.append(text(652, 158, 'cinefit', 'd'))
b.append(box(360, 176, 150, 48, [('Another display', True), ('optional', False)]))
b.append(arrow(i, [(435, 124), (435, 174)]))
b.append(arrow(i, [(640, 224), (640, 270)]))
b.append(box(560, 272, 160, 44, [('Projector or TV', True)]))
figs['chain'] = figure(760, 326, 'The signal chain: player, optional receiver, HDMI splitter, then the capture card and graphics card in the cinefit PC, then the projector; the splitter presents the EDID the player reads', ''.join(b),
    'The player reads the EDID at the end of its cable, which with a splitter is the splitter\'s: that choice decides whether HDR10, tunneled Dolby Vision or low-latency Dolby Vision arrives.')

# 2. Who does the tone mapping.
i = 'tm'
b = [marker(i)]
b.append(text(20, 18, 'Who maps the brightness depends on the kind of picture', 't b', size=15))
cols = [(318, 'PLAYER'), (494, 'CINEFIT'), (668, 'DISPLAY')]
for cx, name in cols:
    b.append(text(cx, 52, name, 'd b', 'middle', 12))
rows = [
    ('SDR', 'nothing to map', [('passes it on', 0), ('BT.2020 container', 0), ('shows it', 0)]),
    ('HDR10, passthrough', 'the default', [('passes it on', 0), ('passes it on', 0), ('maps it|its own mapping', 1)]),
    ('HDR10, sdr2020', 'output.mode = sdr2020', [('passes it on', 0), ('maps it|to SDR BT.2020', 1), ('shows it', 0)]),
    ('Dolby Vision, tunneled', 'Dolby Vision EDID', [('sends it', 0), ('maps it|from the pixels', 1), ('shows it', 0)]),
    ('Dolby Vision, low latency', 'LLDV EDID', [('maps it|to the EDID\'s peak', 1), ('passes it on', 0), ('shows it', 0)]),
]
for r, (name, sub, cells) in enumerate(rows):
    y = 70 + r * 62
    b.append(text(20, y + 18, name, 't b'))
    b.append(text(20, y + 35, sub))
    for k, ((cx, _), (label, hot)) in enumerate(zip(cols, cells)):
        parts = label.split('|')
        lines = [(parts[0], bool(hot))] + [(s, False) for s in parts[1:]]
        b.append(box(cx - 72, y, 144, 44, lines, bool(hot)))
        if k < 2:
            b.append(arrow(i, [(cx + 72, y + 22), (cols[k + 1][0] - 74, y + 22)]))
figs['tonemap'] = figure(760, 384, 'Who does the tone mapping, for each kind of picture', ''.join(b),
    'One row per kind of picture; the highlighted box is the one doing the mapping. Which kind arrives is decided upstream, by the EDID the player sees.')

# 3. Constant image height, to scale: a 4096x2160 panel, a 2.35:1 screen.
i = 'cih'
b = []
b.append(text(20, 18, 'Every film is as tall as the screen; wider films are just wider', 't b', size=15))
b.append(text(20, 38, 'A 4096 x 2160 panel to scale; the dashed band is what lands on the screen.'))
s = 0.05
pw, ph = 4096 * s, 2160 * s
band = 4096 / 2.35 * s
by = (ph - band) / 2
panels = [(20, '16:9 TV show', ['full height,', 'dark at the sides'], 16 / 9),
          (277, '2.39 scope film', ['fills the screen,', 'about 1% trimmed a side'], 2.39),
          (535, '2.76 Ben-Hur', ['fits the width,', 'thin bars above and below'], 2.76)]
y0 = 58
for x, title, lines, ar in panels:
    b.append(f'<rect x="{x}" y="{y0}" width="{pw:.1f}" height="{ph:.1f}" class="velvet"/>')
    if ar <= 2.40:
        h = band; w = min(band * ar, pw)
    else:
        w = pw; h = pw / ar
    px, py = x + (pw - w) / 2, y0 + (ph - h) / 2
    b.append(f'<rect x="{px:.1f}" y="{py:.1f}" width="{w:.1f}" height="{h:.1f}" class="pic"/>')
    b.append(f'<rect x="{x}" y="{y0 + by:.1f}" width="{pw:.1f}" height="{band:.1f}" class="band"/>')
    cx = x + pw / 2
    b.append(text(cx, y0 + ph + 24, title, 't b', 'middle'))
    for k, l in enumerate(lines):
        b.append(text(cx, y0 + ph + 42 + k * 17, l, 'd', 'middle'))
ly = y0 + ph + 92
b.append(f'<rect x="20" y="{ly - 11}" width="16" height="12" class="velvet"/>' + text(44, ly, 'panel rows that land off the screen'))
b.append(f'<rect x="290" y="{ly - 11}" width="16" height="12" class="band"/>' + text(314, ly, 'the 2.35:1 screen'))
b.append(f'<rect x="470" y="{ly - 11}" width="16" height="12" class="pic"/>' + text(494, ly, 'the picture'))
figs['cih'] = figure(760, round(ly + 12), 'Constant image height: a 16:9 show, a 2.39 film and a 2.76 film on a 2.35 screen', ''.join(b),
    'Defaults: geometry.screen_ar = 2.35, geometry.fill_max_ar = 2.40. Content up to the fill limit fills the screen\'s height; wider content keeps its whole width.')

# 4. The remote.
i = 'remote'
b = [marker(i)]
b.append(text(20, 18, 'Five kinds of button run everything', 't b', size=15))
rx, ry = 60, 40
b.append(f'<rect x="{rx}" y="{ry}" width="140" height="250" rx="40" class="card"/>')
cx, cy = rx + 70, ry + 88
for dx, dy, rot in [(0, -38, 0), (38, 0, 90), (0, 38, 180), (-38, 0, 270)]:
    b.append(f'<path d="M{cx + dx - 9} {cy + dy + 5} L{cx + dx} {cy + dy - 6} L{cx + dx + 9} {cy + dy + 5}z" class="ah" transform="rotate({rot} {cx + dx} {cy + dy})"/>')
b.append(f'<circle cx="{cx}" cy="{cy}" r="56" class="box"/>')
b.append(f'<circle cx="{cx}" cy="{cy}" r="20" class="hot"/>' + text(cx, cy + 4.5, 'OK', 'ht b', 'middle'))
btns = [(cx - 32, ry + 182, 'Back'), (cx + 32, ry + 182, 'Menu'), (cx, ry + 222, 'Info')]
for bx, byy, name in btns:
    b.append(f'<rect x="{bx - 26}" y="{byy - 14}" width="52" height="28" rx="14" class="card"/>' + text(bx, byy + 4.5, name, 't', 'middle', 12))
labels = [
    (cy - 38, 'Arrows', 'move around the menu; left and right change a value', 'arrow keys'),
    (cy, 'OK', 'enter a page of the menu', 'Enter or E'),
    (ry + 182, 'Back', 'up a level; closes the menu or the info panel', 'Backspace, Esc or B'),
    (ry + 182 + 34, 'Menu', 'opens and closes the menu', 'C or M'),
    (ry + 222 + 30, 'Info', 'what is coming in, going out, and the crop', 'I or D'),
]
for k, (yy, name, what, keys) in enumerate(labels):
    ty = 56 + k * 52
    b.append(text(300, ty, name, 't b'))
    b.append(text(300, ty + 17, what))
    b.append(text(740, ty, keys, 'd', 'end', 12))
figs['remote'] = figure(760, 310, 'The remote: arrows, OK, Back, Menu and Info, and the keyboard keys each one sends', ''.join(b),
    'Any remote that sends these keys works, through a USB receiver such as a flirc; so does a keyboard. cinefit keys shows what each button sends.')

# 5. When an update takes effect.
i = 'upd'
b = [marker(i)]
b.append(text(20, 18, 'An update waits for the next start', 't b', size=15))
y = 84
b.append(f'<rect x="20" y="{y}" width="460" height="36" rx="6" class="old"/>' + text(250, y + 23, 'the film plays on the build that started it', 't', 'middle'))
b.append(f'<rect x="490" y="{y}" width="250" height="36" rx="6" class="hot"/>' + text(615, y + 23, 'the new build', 'ht b', 'middle'))
b.append(arrow(i, [(250, 58), (250, y - 4)]))
b.append(text(250, 52, 'sudo apt install ./cinefit_amd64.deb', 't', 'middle', 12))
b.append(f'<path d="M485 {y - 10} V{y + 46}" class="ln" stroke-dasharray="3 3"/>')
b.append(text(485, y + 66, 'next start: a boot, or sudo systemctl restart cinefit', 'd', 'middle'))
b.append(text(485, y + 84, 'the display resyncs once, here', 'd', 'middle'))
figs['update'] = figure(760, 188, 'Installing an update does not interrupt playback; the new build runs from the next start', ''.join(b),
    'Installing never restarts a running service, so an update can go in during a film.')

# Put each figure between its markers in the pages.
for page in os.listdir('docs'):
    p = os.path.join('docs', page)
    s = open(p).read()
    def sub(m):
        return f'<!-- fig:{m.group(1)} -->\n{figs[m.group(1)]}\n<!-- /fig -->'
    t = re.sub(r'<!-- fig:(\w+) -->.*?<!-- /fig -->', sub, s, flags=re.S)
    if t != s:
        open(p, 'w').write(t)
        print('updated', page)
