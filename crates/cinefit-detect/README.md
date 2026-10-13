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

## Not ported yet

- **Cuts:** spotting a new shot.
- **The policy:** deciding when the evidence is strong enough to change the
  crop, without flapping back and forth.

## Tests

`cargo test -p cinefit-detect` runs the C++ version's tests on the same
synthetic frames.

The stills are real frames, so they're not in the repo. On the rig:

    CINEFIT_STILLS=DIR cargo test --release -p cinefit-detect --test stills -- --nocapture

All 513 must read the same as the C++ version.
