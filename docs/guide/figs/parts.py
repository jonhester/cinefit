"""Line drawings of the parts on What you need, in the figures' style.

Product photos belong to their makers and sellers, so the shopping list is
drawn: python3 figs/parts.py prints each drawing's SVG, keyed by name, and
the page pastes them between <!-- part:NAME --> markers (as make.py does).
Colors come from assets/css/guide.css."""
import os, re

def svg(body, label):
    return (f'<svg viewBox="0 0 200 120" role="img" aria-label="{label}" class="part-art">{body}</svg>')

def bracket(x):
    # The card's metal bracket at the left, with its ports.
    return f'<rect x="{x}" y="22" width="8" height="84" rx="1" class="velvet"/>'

def pcb(x, y, w, h):
    return f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="4" class="card"/>'

def fingers(x, w):
    # The PCIe contacts along the bottom edge.
    return ''.join(f'<rect x="{x + 6 + k * 5}" y="96" width="3" height="8" class="pic"/>' for k in range(int((w - 12) / 5)))

def fan(cx, cy, r):
    blades = ''.join(f'<path d="M{cx} {cy} Q{cx + r * 0.5} {cy - r * 0.9} {cx + r * 0.15} {cy - r}" class="ln" transform="rotate({a} {cx} {cy})"/>' for a in range(0, 360, 45))
    return f'<circle cx="{cx}" cy="{cy}" r="{r}" class="box"/>{blades}<circle cx="{cx}" cy="{cy}" r="{r * 0.25}" class="hot"/>'

def gpu(fans, w, label):
    x0 = 20
    b = bracket(x0) + pcb(x0 + 8, 26, w, 70) + fingers(x0 + 40, min(w - 40, 90))
    step = w / (fans + 1)
    for k in range(fans):
        b += fan(x0 + 8 + step * (k + 1), 61, min(28, step / 2 - 4))
    # The power connector the bigger cards need.
    if fans > 1:
        b += f'<rect x="{x0 + 8 + w - 26}" y="18" width="18" height="8" rx="1" class="pic"/>'
    return svg(b, label)

def capture():
    x0 = 30
    b = bracket(x0) + pcb(x0 + 8, 40, 110, 56) + fingers(x0 + 30, 70)
    # The HDMI port on the bracket and a chip on the board.
    b += f'<path d="M{x0 - 4} 60 h14 l-2 8 h-10 z" class="pic"/>'
    b += f'<rect x="{x0 + 55}" y="52" width="30" height="24" rx="2" class="velvet"/>'
    return svg(b, 'A capture card: a short circuit board with an HDMI port on its bracket and no fan')

def dongle():
    b = ('<rect x="60" y="48" width="70" height="28" rx="8" class="card"/>'
         '<rect x="130" y="54" width="34" height="16" rx="2" class="velvet"/>'
         '<rect x="136" y="58" width="6" height="4" class="pic"/><rect x="150" y="58" width="6" height="4" class="pic"/>'
         '<circle cx="78" cy="62" r="5" class="hot"/>')
    return svg(b, 'A USB receiver for a remote: a small stick with a USB plug')

PARTS = {
    'rx6400': gpu(1, 120, 'A small graphics card with one fan, no power connector'),
    'gpu2': gpu(2, 160, 'A larger graphics card with two fans and a power connector'),
    'capture': capture(),
    'remote': dongle(),
}

for page in os.listdir('docs'):
    p = os.path.join('docs', page)
    s = open(p).read()
    t = re.sub(r'<!-- part:(\w+) -->.*?<!-- /part -->',
               lambda m: f'<!-- part:{m.group(1)} -->{PARTS[m.group(1)]}<!-- /part -->', s, flags=re.S)
    if t != s:
        open(p, 'w').write(t)
        print('updated', page)
