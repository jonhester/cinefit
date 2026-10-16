"""Turns cinefit's own renders of the on-screen display into the guide's
screenshots, so they always show what the current code draws.

From the repository root:

    CINEFIT_OSD_PREVIEW=/tmp/osd cargo test -p cinefit test_card_preview
    uv run --with pillow --with numpy docs/guide/figs/osd.py /tmp/osd

The test writes each overlay as raw BGRA, premultiplied, at 4096 x 2160; this
lays each one over black (the slate) or over a frame of Tears of Steel
(the rest) and crops it for the page.
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
    """A real picture behind the overlay: a frame of Tears of Steel (CC BY
    3.0, Blender Foundation), 2.40:1, scaled to the panel's width and
    placed in the screen's band, black above and below as on a scope screen."""
    frame = Image.open(f"{OUT}/frames/tos-street.jpg").convert("RGB")
    h = round(W * frame.height / frame.width)
    canvas = Image.new("RGB", (W, H))
    canvas.paste(frame.resize((W, h), Image.LANCZOS), (0, BAND[0] + (BAND[1] - h) // 2))
    return np.asarray(canvas, np.float32)


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
