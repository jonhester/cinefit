"""Turns cinefit's own renders of the on-screen display into the guide's
screenshots, so they always show what the current code draws.

From the repository root:

    CINEFIT_OSD_PREVIEW=/tmp/osd cargo test -p cinefit test_card_preview
    uv run --with pillow --with numpy docs/guide/figs/osd.py /tmp/osd

The test writes each overlay as raw BGRA, premultiplied, at 4096 x 2160; this
lays each one over black (the slate) or over a film frame from figs/frames
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


def picture(name):
    """A film frame filling the screen's band, scaled to cover it and cut to
    fit, so no black shows above, below or beside it. figs/frames holds the
    frames: Charade (1963, public domain in the US) where the shape of the
    picture does not matter, Tears of Steel (CC BY 3.0, Blender Foundation)
    where the overlay says 2.40 : 1."""
    frame = Image.open(f"docs/guide/figs/frames/{name}.jpg").convert("RGB")
    s = max(W / frame.width, BAND[1] / frame.height)
    big = frame.resize((round(frame.width * s), round(frame.height * s)), Image.LANCZOS)
    x0, y0 = (big.width - W) // 2, (big.height - BAND[1]) // 2
    canvas = Image.new("RGB", (W, H))
    canvas.paste(big.crop((x0, y0, x0 + W, y0 + BAND[1])), (0, BAND[0]))
    return np.asarray(canvas, np.float32)


def over(rgb, alpha, bg):
    return np.clip(rgb + bg * (1 - alpha), 0, 255).astype(np.uint8)


def around(alpha, margin, right=None):
    """The overlay's drawn part, with a margin of picture round it (`right`
    on that side, to take in more of the picture), kept inside the screen's
    band: the rows outside it are black."""
    ys, xs = np.where(alpha[..., 0] > 0)
    return (max(xs.min() - margin, 0), max(ys.min() - margin, BAND[0]),
            min(xs.max() + (margin if right is None else right), W), min(ys.max() + margin, BAND[0] + BAND[1]))


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
    # The slate: over black, the whole screen band, as it looks in the room.
    rgb, a = overlay(dir, "slate-0")
    save(over(rgb, a, np.zeros((H, W, 3), np.float32)), (0, BAND[0], W, BAND[0] + BAND[1]), 2400, "osd-slate")
    # The banner, top left, over an SDR film, as its text says.
    rgb, a = overlay(dir, "banner")
    save(over(rgb, a, picture("charade-megeve")), (0, BAND[0], 2304, BAND[0] + 922), 2304, "osd-banner")
    # The menu over Charade; the info panel, which reads 2.40 : 1, over a
    # 2.40 film.
    # The menu takes in Hepburn beside it.
    for name, frame, right in (("menu", "charade-salon", 1000), ("info", "tos-closeup", None)):
        rgb, a = overlay(dir, name)
        save(over(rgb, a, picture(frame)), around(a, 90, right), 4096, f"osd-{name}")


main(sys.argv[1])
