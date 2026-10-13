# cinefit-detect

Finds the black bars in a frame, so cinefit can crop them and fill a scope
screen. Ported from the C++ version.

## How it works

For each frame, start at the top edge and walk down one row at a time until
a row has picture in it. Do the same from the bottom. The rows you walked
past are the bar.

A row has picture in it if enough of its pixels are clearly brighter than
the bar, or clearly coloured. A few bright pixels aren't enough, so noise
and compression artifacts don't end a bar early.

Some things sit inside a bar without being picture:

- **Subtitles** are skipped. They get cropped.
- **Menus and player UI** are dense, so they're kept and never cropped.

The frame is a letterbox only if the result looks like a real film matte:

- the top and bottom bars are the same height;
- the picture starts with a sharp edge across the whole width;
- nothing crosses the edge.

A dark scene can fail these checks, and then it doesn't count. Each
letterbox frame also gets a score for how sure it is.

## Cuts

Whether a frame starts a new shot. The picture inside the current crop is
split into a grid of 32 cells across (square, so 18 rows at 16:9 and 13 at
2.39), each the mean of a few luma samples. Against the previous frame, a
cut needs all of: the cells moved a lot on average, most of them moved, the
brightness histogram changed (or the move is too big for motion), and the
move is a spike, at least twice the previous frame's. The last rule is what
tells a cut from a camera rushing through texture, which passes the others on
every frame. Bars are outside the grid, so a subtitle or menu appearing in
them is never a cut, and a crop change starts a new baseline rather than
being one.

A cut only clears the policy's pending zoom-in candidate. A false one delays a
zoom-in by a few frames; a missed one lets old evidence count a little
longer; neither can crop picture.

## The policy

Observations over time to a crop. One asymmetry decides everything: zooming
out wrongly shows a sliver of bar, which is cheap; zooming in wrongly cuts
the top and bottom off a menu or an IMAX shot, which is the mistake people
notice.

- **Zoom out** on the first frame that shows picture beyond the crop: on
  both sides, or on one side running unbroken into the picture, or UI in a
  bar. A subtitle is one-sided and detached, so it never does. Picture a few
  rows past the crop widens it in place instead, so a dark scene's wandering
  edge doesn't flap between full frame and the crop.
- **Zoom in** only when evidence crosses a threshold: each clean letterbox
  frame adds its strength, times a persistence that grows while the edges
  hold still. A height seen recently needs half a frame's worth, so a film
  comes back on the first frame after its menu; a known one needs two; a
  never-seen one needs six, so a single frame never zooms in. Only heights
  near a standard ratio or already known count; anything else must hold its
  edges across a cut first, which a dark scene's edge and a static menu don't.
- A zoom-in no later frame agreed with is penalised for a minute; a cut
  clears the evidence of the previous shot; a format change, a new frame
  size or half an hour of nothing resets everything to full frame.

## The detector

`Detector` is the three together: Bars reads the frame, Cuts compares it with
the last one over the crop on screen, the policy decides. Frames in, a
`Decision` out: the crop, whether it changed, and diagnostics for a log or
an overlay. It never touches the image path.

## Not ported yet

- The adapters for the tunnel and RGB formats, and the wiring into `play`.

## Tests

`cargo test -p cinefit-detect` runs the C++ version's tests on the same
synthetic frames.

The stills are real frames, so they're not in the repo. On the rig:

    CINEFIT_STILLS=DIR cargo test --release -p cinefit-detect --test stills -- --nocapture

All 513 must read the same as the C++ version, through Bars and through the
whole detector.
