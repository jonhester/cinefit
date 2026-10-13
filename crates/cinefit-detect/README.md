# cinefit-detect

cinefit's bar detector: frames in, rows of black bar to crop out. The crop
drives constant-image-height geometry (`cinefit-geometry`); the detector
never touches the picture itself.

Ported from the C++ version's `detect/`, with its tests. It must read every
still in the stills set exactly as the C++ version did (`tests/stills.rs`).

The detector has three parts. Only the first is ported so far:

1. **Bars** (`bars.rs`): one frame in, one *observation* out: where each
   side's bar ends, and how much that edge looks like a matte.
2. **Cuts**: whether this frame starts a new shot. *Not ported yet.*
3. **The AR policy**: when the evidence is strong enough to change the crop,
   without flapping. *Not ported yet.*

## Input

The detector reads rows through the `Samples` trait: "row y's luma" and
"row y's chroma", one `[Cb, Cr]` pair per two pixels. Samples are in
**v210's scale**: 10-bit limited range, black 64, white 940, chroma centred
on 512 with ±448.

Each capture format has an adapter that converts into that scale. The
constants below were measured on v210, so every format can share them.
`V210` reads the card's bytes in place. Adapters for the Dolby Vision tunnel
and plain RGB are still to come.

The detector asks for rows as it needs them, and reads every sample of each
row. That way no small bright point, such as a star, falls between samples,
and a frame with no bars costs a handful of rows.

## Part 1: one frame's bars

Each side, top and bottom, is measured on its own, scanning inward from the
frame edge. Then the two sides are judged together.

### The band

Only a **band** of rows is searched from each edge: 3/16 of the height, 405
rows at 2160. That is deep enough for 2.76's bars, which are 384 rows.

### The bar level

The bar's level comes from the side's outermost row:

- If that row is flat and neutral, it is bar, and its median luma is the
  level.
  - Flat means the 10th to 90th percentile spread is at most 32 codes.
  - Neutral means chroma within 64 of 512.
  - The median must also be at least 32.
- Otherwise, the level is nominal black: 64.

Real bars measure flat at 64: 99.99 % of Interstellar's bar samples are
within 64–67.

### Lit samples

A sample is **lit**, meaning picture rather than bar, if either:

- its **luma** is more than 16 above the bar level, or
- its **chroma pair** is more than 16 from neutral, in Cb or Cr.

Each threshold has a reason:

- **Luma margin, 16.** It is set by the rows next to the picture, not by
  the bar's flat interior. Compression ringing spreads a few rows out from
  the edge. On Interstellar, the 1–6 rows outside the picture have 0.2–3.9 %
  of samples more than 4 above the bar, but under 0.1 % more than 16. A
  margin of 4 moved edges out by up to 8 rows. Dark picture still clears 16:
  0.01 nits is about code 83.
- **Chroma.** Dark coloured picture, such as a teal night sky, has luma
  barely above black. Its colour is what shows it's picture.
- **The chroma-run rule.** Compression also rings in the chroma of the black
  rows next to the picture: up to 31 codes off neutral, at isolated pairs.
  So a sample lit **by chroma alone** counts only if its pair is part of a
  run of at least **3** consecutive off-neutral pairs. Ringing is scattered;
  coloured picture comes in stretches. Without the rule, ringing caused a
  false switch and an edge read a row out (C++ #106).

A **row is lit** when it has at least **8** lit samples. One speck or a
little ringing can't end a bar.

### Scanning inward, and islands

From the edge inward, the first lit row usually starts the picture. The
exception is a short lit run followed by black: that is an **island**,
something sitting inside the bar.

- An island can be at most 10 % of the height tall, and must be followed by
  at least 8 black rows (at 2160).
- **Sparse islands are subtitles.** They are skipped, and the scan carries
  on past them. The crop will cut them; that is a design decision.
- **Dense islands are UI**, such as a disc's pop-up menu or a player's
  transport bar. An island is dense when at least half its samples are lit.
  It is recorded with its row, because UI must not be cropped away.

The first lit run that is not an island is the **edge**. `bar` is its
distance from the frame edge.

### How much the edge looks like a matte

Just inside the edge, the detector measures:

| measure | what | why |
|---|---|---|
| coverage | the lit fraction of the 4 rows inside the edge | a matte's picture starts across the whole width |
| strength | their mean luma above the bar, over 48 codes, capped at 1 | a bright edge is surer than a dim one |
| sharpness | row edge+1's excess over the excess 12 rows in | a matte steps up within a row or two; a dark scene ramps over many. Row edge+1 skips the one ringing row |
| fill | the lit fraction of the 64 rows inside the edge | picture fills in behind a real edge |
| zones | the horizontal quarters with picture in those rows | a matte spans all four |

There is also a check for **continuation**: picture in the same columns on
both sides of the edge. Three rows are probed: one inside the edge, one
outside, and one further outside. If two adjacent columns are lit in all
three, something crosses the edge, so it is scenery, not a bar. For example,
a lamp post rising out of a dark sky. The probes skip one step either side
of the edge, because the row just outside carries ringing that would match
by chance across thousands of columns.

### Judging the frame

| class | when |
|---|---|
| EMPTY | neither side found an edge: black, or only islands, such as a title card |
| FULL | both edges at row 0, with picture in all four quarters |
| WEAK | an edge was found, but one of the hard gates below failed |
| LETTERBOX | it passed every gate |

The hard gates, in order. The first to fail names the WEAK frame, for the
log:

1. **No edge on one side.**
2. **Full height on one side:** for example, dark sky over lit ground on a
   16:9 frame. That is not a letterbox.
3. **Asymmetric:** the top and bottom bars differ by more than 2 rows
   (at 2160). Film mattes are symmetric to within one row of rounding.
4. **Picture continues across the edge.**
5. **Low fill:** under half the rows inside an edge are lit.
6. **Picture missing from a zone.**

A LETTERBOX frame's **strength** is its weakest signal: the minimum of each
side's coverage, strength and sharpness. A frame is only as trustworthy as
its worst evidence. The AR policy uses this strength as its evidence.

Rows are counted at 2160 and scaled with the height, so the same rules
apply to 1080p.

### Cost

Measured on the rig: 6.76 ms per 2160p scope frame on one thread. The C++
version takes 6.85 ms. A frame with no bars costs far less, since the scan
stops at the first lit row.

## Tests

- `cargo test -p cinefit-detect` runs every C++ `bars_test` case on the
  same synthetic frames. The palette and hash are ported, so the textures
  are identical.
- The stills set: real frames, copyrighted, so never in the repository.

      CINEFIT_STILLS=DIR cargo test --release -p cinefit-detect --test stills -- --nocapture

  It reads all 513 stills and compares each with the C++ baseline
  (`golden.csv`). It must report 0 differences. The test is sharp: with the
  luma margin at 15 instead of 16, 43 stills differ.
