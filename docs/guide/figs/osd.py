"""Turns cinefit's own renders of the on-screen display into the guide's
screenshots, so they always show what the current code draws.

From the repository root:

    CINEFIT_OSD_PREVIEW=/tmp/osd cargo test -p cinefit test_card_preview
    uv run --with pillow --with numpy docs/guide/figs/osd.py /tmp/osd

The test writes each overlay as raw BGRA, premultiplied, at 4096 x 2160; this
lays each one over black (the slate) or over a stand-in picture (the rest)
and crops it for the page. The stand-in is a drawn landscape, never footage.
"""
import sys
import numpy as np
from PIL import Image, ImageDraw

W, H = 4096, 2160
BAND = (209, 1742)  # the 2.35:1 screen's rows, as the test draws
OUT = "docs/guide/assets/images"


def overlay(dir, name):
    a = np.fromfile(f"{dir}/{name}-{W}x{H}.bgra", dtype=np.uint8).reshape(H, W, 4).astype(np.float32)
    return a[..., [2, 1, 0]], a[..., 3:4] / 255.0


def stand_in():
    """A dusk landscape, dimmed as a film's dark scene would be."""
    bg = Image.new("RGB", (W, H))
    d = ImageDraw.Draw(bg)
    top, mid, low = np.array([43, 35, 80]), np.array([154, 107, 143]), np.array([240, 178, 122])
    for y in range(H):
        t = y / H
        c = top + (mid - top) * min(t / 0.62, 1) if t < 0.62 else mid + (low - mid) * ((t - 0.62) / 0.38)
        d.line([(0, y), (W, y)], fill=tuple(int(v * 0.55) for v in c))
    d.polygon([(0, 1500), (700, 1150), (1350, 1420), (2050, 1080), (2800, 1460), (3500, 1190),
               (4096, 1380), (4096, 2160), (0, 2160)], fill=(36, 28, 50))
    d.polygon([(0, 1730), (900, 1460), (1650, 1690), (2400, 1420), (3250, 1730), (4096, 1540),
               (4096, 2160), (0, 2160)], fill=(18, 14, 26))
    return np.asarray(bg, np.float32)


def over(rgb, alpha, bg):
    return np.clip(rgb + bg * (1 - alpha), 0, 255).astype(np.uint8)


def around(alpha, margin):
    """The overlay's drawn part, with a margin of picture round it."""
    ys, xs = np.where(alpha[..., 0] > 0)
    return (max(xs.min() - margin, 0), max(ys.min() - margin, 0),
            min(xs.max() + margin, W), min(ys.max() + margin, H))


def save(img, box, width, name):
    """Crops to `box` and scales to `width`, never up: the renders are at
    the panel's full 4096 x 2160, and the page shows them at about half
    that on a high-density screen, so they stay sharp."""
    x0, y0, x1, y1 = box
    im = Image.fromarray(img[y0:y1, x0:x1])
    width = min(width, im.width)
    if width < im.width:
        im = im.resize((width, round(width * im.height / im.width)), Image.LANCZOS)
    im.save(f"{OUT}/{name}.png", optimize=True)
    print(f"{name}.png {im.size[0]}x{im.size[1]}")


def main(dir):
    picture = stand_in()
    # The slate: over black, the whole screen band, as it looks in the room.
    rgb, a = overlay(dir, "slate-0")
    save(over(rgb, a, np.zeros_like(picture)), (0, BAND[0], W, BAND[0] + BAND[1]), 2400, "osd-slate")
    # The banner: the top left of the picture, where it appears.
    rgb, a = overlay(dir, "banner")
    save(over(rgb, a, picture), (0, BAND[0], 2304, BAND[0] + 922), 2304, "osd-banner")
    # The info panel and the menu: each with a margin of picture.
    for name, width in (("info", 4096), ("menu", 4096)):
        rgb, a = overlay(dir, name)
        save(over(rgb, a, picture), around(a, 90), width, f"osd-{name}")


main(sys.argv[1])
