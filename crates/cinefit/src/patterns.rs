//! Test patterns for setting up the projector, chosen in the menu: one for
//! alignment, two for focus, and a white window for measuring light. Drawn
//! by the CPU into the overlay plane, opaque, so the video path, the mode
//! and the signalling are untouched: showing one is never a resync, and it
//! comes in the output's own encoding, SDR or HDR10, whichever is up.
//!
//! The overlay is the raster's own size and scanned out 1:1, so a 1-pixel
//! line here is one pixel on the panel. That is what the focus and grid
//! detail relies on: if it shimmers or goes grey, something between here and
//! the panel is scaling.
//!
//! A pattern is never a setting: it is not saved, and a run never starts on
//! one.

use cinefit_geometry::{Rect, dest_rect};

use crate::osd::{self, Align, Canvas, Encoding, Fonts, Rgb, Style, palette};

/// Which pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The screen's edges, the raster's edges, a grid of squares, circles,
    /// and where each common ratio lands.
    Alignment,
    /// Nine zones of fine detail across the screen.
    Focus,
    /// The same in green only: the lens brings red, green and blue to focus
    /// at slightly different depths, and green carries most of what reads
    /// as sharpness, so focusing on green alone is sharper than on white.
    FocusGreen,
    /// A centred white window on black, for a meter.
    Window,
}

/// The patterns in the order the menu steps through them.
pub const KINDS: [Kind; 4] = [Kind::Alignment, Kind::Focus, Kind::FocusGreen, Kind::Window];

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Alignment => "Alignment",
            Kind::Focus => "Focus",
            Kind::FocusGreen => "Focus, green only",
            Kind::Window => "White window",
        }
    }
}

/// The window's share of the screen's area. Smaller than the whole screen
/// by default: the projector's dynamic light control and the room's
/// reflections both respond to a full field, so a window reads closer to
/// the projector's own peak; 100% is there to see what the room adds.
pub const SIZES: [f64; 4] = [0.10, 0.25, 0.50, 1.00];

/// The window's level under SDR, a fraction of white. 100% is the most the
/// projector shows in SDR, and so its peak.
pub const SDR_LEVELS: [f32; 5] = [1.0, 0.75, 0.5, 0.25, 0.1];

/// The window's level under HDR10, in nits. These measure where the
/// projector's own tone mapping puts each level, not its peak: that is
/// measured in SDR.
pub const PQ_NITS: [f32; 6] = [10_000.0, 4_000.0, 1_000.0, 400.0, 203.0, 100.0];

/// How many window levels `enc` has.
pub fn level_count(enc: Encoding) -> usize {
    match enc {
        Encoding::Sdr => SDR_LEVELS.len(),
        Encoding::Pq => PQ_NITS.len(),
    }
}

/// Window level `i` (clamped to the list) as the menu names it.
pub fn level_label(i: usize, enc: Encoding) -> String {
    let i = i.min(level_count(enc) - 1);
    match enc {
        Encoding::Sdr => format!("{:.0}% white", SDR_LEVELS[i] * 100.0),
        Encoding::Pq => format!("{:.0} nits", PQ_NITS[i]),
    }
}

/// Window level `i` as an 8-bit code value for `enc`.
fn level_code(i: usize, enc: Encoding) -> u8 {
    let i = i.min(level_count(enc) - 1);
    match enc {
        Encoding::Sdr => osd::encode(SDR_LEVELS[i], Encoding::Sdr),
        Encoding::Pq => (osd::pq(PQ_NITS[i]) * 255.0).round() as u8,
    }
}

/// What an 8-bit PQ code means in nits: the overlay is 8 bits, so a level
/// goes out as the nearest code, a few percent off what was asked. The
/// pattern says what was sent.
fn pq_nits(code: u8) -> f32 {
    let (m1, m2) = (0.159_301_76_f32, 78.843_75_f32);
    let (c1, c2, c3) = (0.835_937_5_f32, 18.851_563_f32, 18.6875_f32);
    let e = (f32::from(code) / 255.0).powf(1.0 / m2);
    10_000.0 * ((e - c1).max(0.0) / (c2 - c3 * e)).powf(1.0 / m1)
}

/// A pattern and its window's size and level, as shown now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spec {
    pub kind: Kind,
    /// An index into [`SIZES`].
    pub size: usize,
    /// An index into the level list of the output's encoding.
    pub level: usize,
}

/// Opaque BGRA pixels over the whole raster, with writes clipped to it.
struct Px<'a> {
    px: &'a mut [u8],
    w: usize,
    h: usize,
}

impl Px<'_> {
    fn put(&mut self, x: usize, y: usize, rgb: [u8; 3]) {
        if x < self.w && y < self.h {
            let o = (y * self.w + x) * 4;
            self.px[o..o + 4].copy_from_slice(&[rgb[2], rgb[1], rgb[0], 255]);
        }
    }

    /// The brighter of what is there and `rgb`, per channel: for edges
    /// blended over black that cross other lines.
    fn max(&mut self, x: usize, y: usize, rgb: [u8; 3]) {
        if x < self.w && y < self.h {
            let o = (y * self.w + x) * 4;
            for (c, v) in self.px[o..o + 3].iter_mut().zip([rgb[2], rgb[1], rgb[0]]) {
                *c = (*c).max(v);
            }
        }
    }

    fn fill(&mut self, r: Rect, rgb: [u8; 3]) {
        for y in r.y as usize..(r.y + r.h) as usize {
            for x in r.x as usize..(r.x + r.w) as usize {
                self.put(x, y, rgb);
            }
        }
    }

    fn hline(&mut self, x0: usize, x1: usize, y: usize, rgb: [u8; 3]) {
        for x in x0..x1 {
            self.put(x, y, rgb);
        }
    }

    fn vline(&mut self, x: usize, y0: usize, y1: usize, rgb: [u8; 3]) {
        for y in y0..y1 {
            self.put(x, y, rgb);
        }
    }

    /// A 1-pixel outline on the inside edge of `r`.
    fn frame(&mut self, r: Rect, rgb: [u8; 3]) {
        if r.w == 0 || r.h == 0 {
            return;
        }
        let (x0, y0) = (r.x as usize, r.y as usize);
        let (x1, y1) = (x0 + r.w as usize, y0 + r.h as usize);
        self.hline(x0, x1, y0, rgb);
        self.hline(x0, x1, y1 - 1, rgb);
        self.vline(x0, y0, y1, rgb);
        self.vline(x1 - 1, y0, y1, rgb);
    }

    /// A circle about 1 pixel wide, its edge blended over black in light.
    fn circle(&mut self, cx: f32, cy: f32, r: f32, level: f32, enc: Encoding) {
        let (x0, x1) = ((cx - r - 2.0).max(0.0) as usize, (cx + r + 2.0) as usize);
        let (y0, y1) = ((cy - r - 2.0).max(0.0) as usize, (cy + r + 2.0) as usize);
        for y in y0..y1 {
            for x in x0..x1 {
                let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
                let cover = (1.0 - (d - r).abs()).clamp(0.0, 1.0);
                if cover > 0.0 {
                    self.max(x, y, Rgb::grey(level * cover).encode(enc));
                }
            }
        }
    }
}

fn black(px: &mut [u8]) {
    for p in px.as_chunks_mut::<4>().0 {
        p.copy_from_slice(&[0, 0, 0, 255]);
    }
}

/// Draws `spec` over the whole of `px` (BGRA, `raster` sized, no padding),
/// with the screen at `win` of the raster.
pub fn draw(
    px: &mut [u8],
    raster: (usize, usize),
    win: Rect,
    enc: Encoding,
    fonts: &Fonts,
    spec: Spec,
) {
    black(px);
    match spec.kind {
        Kind::Alignment => alignment(px, raster, win, enc, fonts),
        Kind::Focus => focus(px, raster, win, enc, fonts),
        Kind::FocusGreen => {
            focus(px, raster, win, enc, fonts);
            // Drawn in white and grey only, so taking red and blue away
            // leaves exactly the green of the same light, text included.
            for p in px.as_chunks_mut::<4>().0 {
                p[0] = 0;
                p[2] = 0;
            }
        }
        Kind::Window => window(px, raster, win, enc, fonts, spec),
    }
}

/// The common ratios marked on the alignment pattern.
const RATIOS: [(f64, &str); 5] = [
    (16.0 / 9.0, "16:9"),
    (1.85, "1.85"),
    (2.00, "2.00"),
    (2.20, "2.20"),
    (2.39, "2.39"),
];

/// Rows per grid square: the screen's height in eight.
const GRID_ROWS: u32 = 8;

fn alignment(px: &mut [u8], raster: (usize, usize), win: Rect, enc: Encoding, fonts: &Fonts) {
    let (w, h) = raster;
    let s = h as f32 / 2160.0;
    let white = Rgb::grey(1.0).encode(enc);
    let grid = Rgb::grey(0.3).encode(enc);
    let inner = Rgb::grey(0.5).encode(enc);
    let accent = palette::accent().encode(enc);
    let mut p = Px { px, w, h };
    // The grid: squares a screen's height in eight, from the centre out, so
    // the centre cross is one of its lines and keystone shows as squares
    // that are not.
    let cell = (win.h / GRID_ROWS).max(1);
    let (cx, cy) = (win.x + win.w / 2, win.y + win.h / 2);
    let (wx0, wx1) = (win.x as usize, (win.x + win.w) as usize);
    let (wy0, wy1) = (win.y as usize, (win.y + win.h) as usize);
    let mut x = win.x + (cx - win.x) % cell;
    while x < win.x + win.w {
        p.vline(x as usize, wy0, wy1, grid);
        x += cell;
    }
    let mut y = win.y + (cy - win.y) % cell;
    while y < win.y + win.h {
        p.hline(wx0, wx1, y as usize, grid);
        y += cell;
    }
    // Lines 4, 8 and 16 pixels inside the screen's edge: where the screen's
    // border falls among them is how far out the picture is, in pixels.
    for d in [4, 8, 16] {
        if win.w > 2 * d && win.h > 2 * d {
            p.frame(
                Rect {
                    x: win.x + d,
                    y: win.y + d,
                    w: win.w - 2 * d,
                    h: win.h - 2 * d,
                },
                inner,
            );
        }
    }
    // Where each common ratio lands on the screen, constant height.
    for (ar, _) in RATIOS {
        let r = dest_rect(win, ar);
        if r != win {
            p.frame(r, accent);
        }
    }
    // Circles: one large in the centre, one in each corner. Geometry errors
    // show as ovals sooner than as bent lines.
    let level = 1.0;
    let big = win.h as f32 * 0.45;
    p.circle(cx as f32, cy as f32, big, level, enc);
    let small = win.h as f32 / 8.0;
    for (fx, fy) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
        let x = win.x as f32 + small * 1.25 + fx * (win.w as f32 - 2.5 * small);
        let y = win.y as f32 + small * 1.25 + fy * (win.h as f32 - 2.5 * small);
        p.circle(x, y, small, level, enc);
    }
    // The centre cross, across the whole screen, and the screen's edge.
    p.hline(wx0, wx1, cy as usize, white);
    p.vline(cx as usize, wy0, wy1, white);
    p.frame(win, white);
    // The raster's own edge, in red, apart from the screen's: whether the
    // picture fills the screen and whether the screen is where cinefit
    // thinks are separate questions.
    let red = Rgb(1.0, 0.0, 0.0).encode(enc);
    p.frame(
        Rect {
            x: 0,
            y: 0,
            w: w as u32,
            h: h as u32,
        },
        red,
    );
    // The ratios' names, inside each frame's top left.
    let mut c = Canvas::new(px, w * 4, w, h, enc);
    let label = Style::new(&fonts.regular, 34.0 * s, palette::accent(), Align::Left);
    // A row each, stepping down: neighbouring ratios' edges are only tens
    // of pixels apart, so their names would run together on one row.
    for (i, (ar, name)) in RATIOS.iter().enumerate() {
        let r = dest_rect(win, *ar);
        if r != win {
            c.text(
                label,
                r.x as f32 + 14.0 * s,
                r.y as f32 + (60.0 + 48.0 * i as f32) * s,
                name,
            );
        }
    }
    let note = Style::new(&fonts.regular, 34.0 * s, Rgb::grey(0.6), Align::Centre);
    c.text(
        note,
        cx as f32,
        (win.y + win.h) as f32 - 40.0 * s,
        &format!(
            "screen {:.2} : 1 \u{b7} {w} \u{d7} {h} \u{b7} red is the raster's edge",
            win.aspect()
        ),
    );
}

/// The periods of the stripe blocks, in pixels on and off: 1 is the finest
/// the panel shows.
const STRIPES: [usize; 3] = [1, 2, 3];
/// Spokes on each zone's star.
const SPOKES: f32 = 24.0;

/// Where each block of a focus zone sits, from the zone's top left: three
/// stripe blocks across, vertical stripes over horizontal, then the star
/// over the dots. Block `b` pixels square, `gap` apart.
fn focus_zone(p: &mut Px<'_>, x0: usize, y0: usize, b: usize, gap: usize, white: [u8; 3]) {
    for (i, &period) in STRIPES.iter().enumerate() {
        let bx = x0 + i * (b + gap);
        for y in 0..b {
            for x in 0..b {
                // Vertical stripes resolve detail across; horizontal, down.
                // One coming sharp before the other is astigmatism.
                if (x / period) % 2 == 0 {
                    p.put(bx + x, y0 + y, white);
                }
                if (y / period) % 2 == 0 {
                    p.put(bx + x, y0 + b + gap + y, white);
                }
            }
        }
    }
    let sx = x0 + 3 * (b + gap);
    let r = b as f32 / 2.0;
    for y in 0..b {
        for x in 0..b {
            let (dx, dy) = (x as f32 + 0.5 - r, y as f32 + 0.5 - r);
            if dx * dx + dy * dy <= r * r {
                let a = dy.atan2(dx) + std::f32::consts::PI;
                if ((a / std::f32::consts::TAU * 2.0 * SPOKES) as usize).is_multiple_of(2) {
                    p.put(sx + x, y0 + y, white);
                }
            }
            // Single pixels, eight apart: each one's spread is the lens's
            // blur, to see with binoculars from a seat.
            if x % 8 == 4 && y % 8 == 4 {
                p.put(sx + x, y0 + b + gap + y, white);
            }
        }
    }
}

/// Block size, gap and the zone's width and height for a screen `win`.
fn focus_sizes(win: Rect) -> (usize, usize, usize, usize) {
    let b = (win.h as usize / 14).max(8);
    let gap = (b / 6).max(2);
    (b, gap, 4 * b + 3 * gap, 2 * b + gap + b / 2)
}

/// The top left of each of the nine zones: the centre, the edges' middles
/// and the corners, a third of a block in from the screen's edge.
fn focus_origins(win: Rect) -> Vec<(usize, usize)> {
    let (b, _, zw, zh) = focus_sizes(win);
    let m = b / 3;
    let (x0, y0) = (win.x as usize, win.y as usize);
    let (ww, wh) = (win.w as usize, win.h as usize);
    let xs = [
        x0 + m,
        x0 + ww / 2 - zw / 2,
        (x0 + ww).saturating_sub(m + zw),
    ];
    let ys = [
        y0 + m,
        y0 + wh / 2 - zh / 2,
        (y0 + wh).saturating_sub(m + zh),
    ];
    ys.iter()
        .flat_map(|&y| xs.iter().map(move |&x| (x, y)))
        .collect()
}

fn focus(px: &mut [u8], raster: (usize, usize), win: Rect, enc: Encoding, fonts: &Fonts) {
    let (w, h) = raster;
    let white = Rgb::grey(1.0).encode(enc);
    let (b, gap, _, _) = focus_sizes(win);
    let origins = focus_origins(win);
    let mut p = Px { px, w, h };
    for &(x, y) in &origins {
        focus_zone(&mut p, x, y, b, gap, white);
    }
    // Small type under each zone: the most natural thing to judge by eye.
    let mut c = Canvas::new(px, w * 4, w, h, enc);
    let style = Style::new(&fonts.regular, b as f32 / 5.0, Rgb::grey(1.0), Align::Left);
    for &(x, y) in &origins {
        c.text(
            style,
            x as f32,
            (y + 2 * b + gap) as f32 + b as f32 * 0.35,
            "Sharp focus 0123456789 \u{b7} the quick brown fox",
        );
    }
}

/// The window's rectangle: centred, the screen's shape, `fraction` of its
/// area.
fn window_rect(win: Rect, fraction: f64) -> Rect {
    let k = fraction.clamp(0.0, 1.0).sqrt();
    let (ww, wh) = (
        (f64::from(win.w) * k).round() as u32,
        (f64::from(win.h) * k).round() as u32,
    );
    Rect {
        x: win.x + (win.w - ww) / 2,
        y: win.y + (win.h - wh) / 2,
        w: ww,
        h: wh,
    }
}

fn window(
    px: &mut [u8],
    raster: (usize, usize),
    win: Rect,
    enc: Encoding,
    fonts: &Fonts,
    spec: Spec,
) {
    let (w, h) = raster;
    let size = SIZES[spec.size.min(SIZES.len() - 1)];
    let code = level_code(spec.level, enc);
    let r = window_rect(win, size);
    Px { px, w, h }.fill(r, [code; 3]);
    if size >= 1.0 {
        // Nothing on a full field: the meter may be anywhere on it.
        return;
    }
    // What was sent, small and dim in the screen's corner, far from the
    // window and the meter.
    let s = h as f32 / 2160.0;
    let sent = match enc {
        // In light, not signal: gamma 2.4 back from the code.
        Encoding::Sdr => format!(
            "{:.1}% of white's light",
            (f32::from(code) / 255.0).powf(2.4) * 100.0
        ),
        Encoding::Pq => format!("{:.0} nits (PQ code {code} of 255)", pq_nits(code)),
    };
    let text = format!(
        "{:.0}% window \u{b7} {} \u{b7} sent {sent}",
        size * 100.0,
        level_label(spec.level, enc)
    );
    let mut c = Canvas::new(px, w * 4, w, h, enc);
    c.text(
        Style::new(&fonts.regular, 30.0 * s, Rgb::grey(0.15), Align::Left),
        win.x as f32 + 40.0 * s,
        (win.y + win.h) as f32 - 40.0 * s,
        &text,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    const RASTER: (usize, usize) = (4096, 2160);

    fn win() -> Rect {
        cinefit_geometry::screen_window(4096, 2160, 2.35)
    }

    fn drawn(spec: Spec, enc: Encoding) -> Vec<u8> {
        let fonts = Fonts::load();
        let mut px = vec![0u8; RASTER.0 * RASTER.1 * 4];
        draw(&mut px, RASTER, win(), enc, &fonts, spec);
        px
    }

    /// The pixel at (x, y), red first.
    fn at(px: &[u8], x: u32, y: u32) -> [u8; 3] {
        let o = (y as usize * RASTER.0 + x as usize) * 4;
        [px[o + 2], px[o + 1], px[o]]
    }

    fn spec(kind: Kind) -> Spec {
        Spec {
            kind,
            size: 1,
            level: 0,
        }
    }

    #[test]
    fn every_pattern_is_opaque() {
        for kind in KINDS {
            let px = drawn(spec(kind), Encoding::Pq);
            assert!(px.chunks(4).all(|p| p[3] == 255), "{kind:?}");
        }
    }

    #[test]
    fn alignment_marks_the_screen_the_raster_and_the_ratios() {
        for enc in [Encoding::Sdr, Encoding::Pq] {
            let px = drawn(spec(Kind::Alignment), enc);
            let w = win();
            let white = Rgb::grey(1.0).encode(enc);
            let red = Rgb(1.0, 0.0, 0.0).encode(enc);
            // The raster's corners and edges are red; the screen's edge,
            // its centre cross, are white.
            assert_eq!(at(&px, 0, 0), red);
            assert_eq!(at(&px, 4095, 1000), red);
            assert_eq!(at(&px, 1000, w.y), white);
            assert_eq!(at(&px, 1000, w.y + w.h - 1), white);
            assert_eq!(at(&px, w.x + w.w / 2, w.y + 100), white);
            // Outside the screen, inside the raster: black.
            assert_eq!(at(&px, 1000, w.y / 2), [0, 0, 0]);
            // 1.85 lands at constant height, its left edge in the accent.
            let r = dest_rect(w, 1.85);
            assert!(r.x > 0 && r.h == w.h);
            assert_eq!(
                at(&px, r.x, w.y + w.h / 2 + w.h / 16 + 3),
                palette::accent().encode(enc)
            );
            // The lines inside the edge, at 4 px and not at 2 (a grid line
            // happens to fall at 3 on this screen).
            let grey = Rgb::grey(0.5).encode(enc);
            assert_eq!(at(&px, 1000, w.y + 4), grey);
            assert_eq!(at(&px, 1000, w.y + 2), [0, 0, 0]);
        }
    }

    #[test]
    fn focus_stripes_have_their_periods() {
        let px = drawn(spec(Kind::Focus), Encoding::Sdr);
        let (b, gap, _, _) = focus_sizes(win());
        let white = Rgb::grey(1.0).encode(Encoding::Sdr);
        for (x0, y0) in focus_origins(win()) {
            let (x0, y0) = (x0 as u32, y0 as u32);
            let y = y0 + 5;
            for (i, &period) in STRIPES.iter().enumerate() {
                let bx = x0 + (i * (b + gap)) as u32;
                let row: Vec<bool> = (0..12).map(|x| at(&px, bx + x, y) == white).collect();
                let want: Vec<bool> = (0..12).map(|x| (x / period) % 2 == 0).collect();
                assert_eq!(row, want, "period {period} at {bx},{y}");
                // And down, in the block below.
                let by = y0 + (b + gap) as u32;
                let col: Vec<bool> = (0..12).map(|y| at(&px, bx + 5, by + y) == white).collect();
                assert_eq!(col, want, "period {period} down at {bx}");
            }
        }
        // Nine zones, the corners inside the screen.
        assert_eq!(focus_origins(win()).len(), 9);
    }

    #[test]
    fn focus_green_is_green_alone_with_the_same_light() {
        let white = drawn(spec(Kind::Focus), Encoding::Pq);
        let green = drawn(spec(Kind::FocusGreen), Encoding::Pq);
        let mut lit = 0;
        for (w, g) in white.chunks(4).zip(green.chunks(4)) {
            assert_eq!((g[0], g[2]), (0, 0));
            assert_eq!(g[1], w[1]);
            lit += usize::from(g[1] > 0);
        }
        assert!(lit > 100_000, "{lit}");
    }

    #[test]
    fn the_window_has_its_share_of_the_screen_and_its_level() {
        for (size, frac) in SIZES.iter().enumerate() {
            let s = Spec {
                kind: Kind::Window,
                size,
                level: 2,
            };
            let px = drawn(s, Encoding::Pq);
            let code = (osd::pq(1000.0) * 255.0).round() as u8;
            let w = win();
            let lit = px.chunks(4).filter(|p| p[..3] == [code; 3]).count();
            let share = lit as f64 / f64::from(w.w * w.h);
            assert!((share - frac).abs() < 0.002, "{frac}: {share}");
            // Centred.
            assert_eq!(at(&px, w.x + w.w / 2, w.y + w.h / 2), [code; 3]);
        }
        // SDR's top level is full white.
        let px = drawn(
            Spec {
                kind: Kind::Window,
                size: 0,
                level: 0,
            },
            Encoding::Sdr,
        );
        let w = win();
        assert_eq!(at(&px, w.x + w.w / 2, w.y + w.h / 2), [255; 3]);
    }

    #[test]
    fn pq_codes_read_back_close_to_what_was_asked() {
        for nits in PQ_NITS {
            let code = (osd::pq(nits) * 255.0).round() as u8;
            let back = pq_nits(code);
            // 8 bits of PQ: steps of a few percent.
            assert!((back / nits - 1.0).abs() < 0.04, "{nits}: {back}");
        }
        assert_eq!(pq_nits(255).round(), 10_000.0);
        assert_eq!(level_label(9, Encoding::Pq), "100 nits", "clamped");
        assert_eq!(level_label(0, Encoding::Sdr), "100% white");
    }
}

#[cfg(test)]
mod preview {
    use super::*;

    /// `CINEFIT_OSD_PREVIEW=DIR cargo test -p cinefit patterns::preview`:
    /// each pattern as the overlay holds it, raw BGRA at 4096 x 2160 on a
    /// 2.35 screen, to look at without a display.
    #[test]
    fn pattern_preview() {
        let Some(dir) = std::env::var_os("CINEFIT_OSD_PREVIEW") else {
            return;
        };
        let fonts = Fonts::load();
        let (w, h) = (4096, 2160);
        let win = cinefit_geometry::screen_window(4096, 2160, 2.35);
        for kind in KINDS {
            let mut px = vec![0u8; w * h * 4];
            let spec = Spec {
                kind,
                size: 1,
                level: 0,
            };
            draw(&mut px, (w, h), win, Encoding::Sdr, &fonts, spec);
            let path = std::path::Path::new(&dir).join(format!("pattern-{kind:?}-{w}x{h}.bgra"));
            std::fs::write(path, &px).expect("write");
        }
    }
}
