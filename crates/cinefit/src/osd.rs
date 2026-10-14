//! On-screen text, drawn by the CPU into the overlay plane's buffer
//! (`cinefit_kms::Output::create_overlay`), which the display engine
//! composites over the video. Drawn only when it changes; the video path
//! never sees it.
//!
//! The overlay is blended in the output's own signal space, and the kernel
//! offers no per-plane colour conversion, so its code values must already
//! mean the right light for what the output signals: a gamma of 2.4 for SDR,
//! absolute light through PQ for HDR10. UI shades are given as a fraction of
//! UI white, which is SDR white, or [`UI_WHITE_NITS`] under PQ. The output
//! is BT.2020 in both (SDR is signalled as BT.2020 too), so the palette's
//! colours, picked on BT.709 screens, are converted to its primaries.
//!
//! Text is Inter (SIL Open Font License, `assets/fonts/OFL-Inter.txt`),
//! rasterised by fontdue.

use fontdue::{Font, FontSettings};

/// UI white under PQ: SDR reference white's level, so the OSD is no brighter
/// in HDR10 than in SDR. (BT.2408 puts graphics white at 203 nits for
/// television; a projector in a dark room wants less.)
pub const UI_WHITE_NITS: f32 = 100.0;

/// What the output's code values mean.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Encoding {
    /// SDR: relative, gamma 2.4.
    Sdr,
    /// HDR10: absolute light, SMPTE ST 2084.
    Pq,
}

/// A UI shade (0 black, 1 UI white) as an 8-bit code value for `enc`.
pub fn encode(level: f32, enc: Encoding) -> u8 {
    let l = level.clamp(0.0, 1.0);
    let v = match enc {
        Encoding::Sdr => l.powf(1.0 / 2.4),
        Encoding::Pq => pq(l * UI_WHITE_NITS),
    };
    (v * 255.0).round() as u8
}

/// A UI colour: linear light in BT.709 primaries, each a fraction of UI
/// white.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgb(pub f32, pub f32, pub f32);

impl Rgb {
    pub const fn grey(level: f32) -> Self {
        Self(level, level, level)
    }

    /// A palette colour as written, `#rrggbb`: code values as a gamma-2.4
    /// screen shows them, so a grey comes out with the same code in SDR.
    pub fn hex(rgb: u32) -> Self {
        let c = |shift: u32| (((rgb >> shift) & 0xff) as f32 / 255.0).powf(2.4);
        Self(c(16), c(8), c(0))
    }

    /// As 8-bit code values for `enc`, red first: in BT.2020 primaries,
    /// then encoded. Greys are unchanged by the conversion.
    pub fn encode(self, enc: Encoding) -> [u8; 3] {
        // BT.2087's BT.709-to-BT.2020 matrix, on linear light.
        let Rgb(r, g, b) = self;
        [
            0.6274 * r + 0.3293 * g + 0.0433 * b,
            0.0691 * r + 0.9195 * g + 0.0114 * b,
            0.0164 * r + 0.0880 * g + 0.8956 * b,
        ]
        .map(|l| encode(l, enc))
    }
}

/// The palette (#72): text in three tiers, the accent, and two status
/// colours that tint words only.
pub mod palette {
    use super::Rgb;

    pub fn text() -> Rgb {
        Rgb::hex(0xece8e1)
    }
    pub fn secondary() -> Rgb {
        Rgb::hex(0xb3aec2)
    }
    pub fn hint() -> Rgb {
        Rgb::hex(0x9f99b2)
    }
    pub fn accent() -> Rgb {
        Rgb::hex(0xb9a6ff)
    }
    pub fn attention() -> Rgb {
        Rgb::hex(0xf6b26b)
    }
    pub fn error() -> Rgb {
        Rgb::hex(0xf2666b)
    }
    /// The banner's panel, behind its text at 80% opacity.
    pub fn panel() -> Rgb {
        Rgb::hex(0x0b0b10)
    }
}

/// SMPTE ST 2084's inverse EOTF: nits to a signal value in 0..1.
fn pq(nits: f32) -> f32 {
    let (m1, m2) = (0.159_301_76_f32, 78.843_75_f32);
    let (c1, c2, c3) = (0.835_937_5_f32, 18.851_563_f32, 18.6875_f32);
    let y = (nits / 10_000.0).clamp(0.0, 1.0).powf(m1);
    ((c1 + c2 * y) / (1.0 + c3 * y)).powf(m2)
}

pub struct Fonts {
    pub regular: Font,
    pub semibold: Font,
}

impl Fonts {
    pub fn load() -> Self {
        let font = |bytes: &[u8]| {
            Font::from_bytes(bytes, FontSettings::default()).expect("the bundled font parses")
        };
        Self {
            regular: font(include_bytes!("../assets/fonts/Inter-Regular.ttf")),
            semibold: font(include_bytes!("../assets/fonts/Inter-SemiBold.ttf")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Align {
    Left,
    Centre,
    Right,
}

/// An overlay buffer to draw into: BGRA bytes, premultiplied alpha. It may
/// hold only a window of the raster ([`Canvas::window`]); coordinates are
/// always the raster's.
pub struct Canvas<'a> {
    px: &'a mut [u8],
    pitch: usize,
    /// The part of the raster `px` holds.
    win: Area,
    /// The whole raster's size, which sizes the type.
    raster: (usize, usize),
    enc: Encoding,
}

impl<'a> Canvas<'a> {
    pub fn new(px: &'a mut [u8], pitch: usize, w: usize, h: usize, enc: Encoding) -> Self {
        Self::window(px, pitch, Area { x: 0, y: 0, w, h }, (w, h), enc)
    }

    /// A canvas holding only `win` of a `raster`-sized overlay: drawing is
    /// clipped to it. For drawing off the overlay's own memory, which is
    /// slow to read, and copying in the result.
    pub fn window(
        px: &'a mut [u8],
        pitch: usize,
        win: Area,
        raster: (usize, usize),
        enc: Encoding,
    ) -> Self {
        Self {
            px,
            pitch,
            win,
            raster,
            enc,
        }
    }

    /// The raster's size.
    pub fn size(&self) -> (usize, usize) {
        self.raster
    }

    /// Everything transparent.
    pub fn clear(&mut self) {
        for row in self.px.chunks_mut(self.pitch).take(self.win.h) {
            row[..self.win.w * 4].fill(0);
        }
    }

    /// One pixel of `rgb` (code values, red first) at coverage `a`, over
    /// what is there.
    fn blend(&mut self, x: usize, y: usize, rgb: [u8; 3], a: f32) {
        let Area { x: x0, y: y0, w, h } = self.win;
        if x < x0 || y < y0 || x >= x0 + w || y >= y0 + h || a <= 0.0 {
            return;
        }
        let o = (y - y0) * self.pitch + (x - x0) * 4;
        let p = &mut self.px[o..o + 4];
        // BGRA in memory.
        for (c, code) in p[..3].iter_mut().zip([rgb[2], rgb[1], rgb[0]]) {
            *c = (f32::from(code) * a + f32::from(*c) * (1.0 - a)).round() as u8;
        }
        p[3] = (255.0 * a + f32::from(p[3]) * (1.0 - a)).round() as u8;
    }

    /// `area` in `colour` at opacity `alpha`, corners rounded by `radius`.
    pub fn rect(&mut self, area: Area, radius: usize, colour: Rgb, alpha: f32) {
        let Area { x, y, w, h } = area;
        let code = colour.encode(self.enc);
        let (rw, rh) = self.raster;
        for yy in y..(y + h).min(rh) {
            for xx in x..(x + w).min(rw) {
                self.blend(xx, yy, code, alpha * cover(area, radius, xx, yy));
            }
        }
    }

    /// A line `width` wide just inside `area`'s rounded edge.
    pub fn outline(&mut self, area: Area, radius: usize, width: usize, colour: Rgb, alpha: f32) {
        let Area { x, y, w, h } = area;
        let inner = Area {
            x: x + width,
            y: y + width,
            w: w.saturating_sub(2 * width),
            h: h.saturating_sub(2 * width),
        };
        let r_in = radius.saturating_sub(width);
        let code = colour.encode(self.enc);
        let (rw, rh) = self.raster;
        for yy in y..(y + h).min(rh) {
            for xx in x..(x + w).min(rw) {
                let inside = (inner.x..inner.x + inner.w).contains(&xx)
                    && (inner.y..inner.y + inner.h).contains(&yy);
                let c_in = if inside {
                    cover(inner, r_in, xx, yy)
                } else {
                    0.0
                };
                let c = (cover(area, radius, xx, yy) - c_in).max(0.0);
                self.blend(xx, yy, code, alpha * c);
            }
        }
    }

    /// `text` set in `style`, its baseline at `y`; `x` is its left edge or
    /// its centre, by the style's alignment. Returns its width.
    pub fn text(&mut self, style: Style<'_>, x: f32, y: f32, text: &str) -> f32 {
        let Style {
            font,
            size,
            colour,
            align,
        } = style;
        let width = measure(font, size, text);
        let mut pen = match align {
            Align::Left => x,
            Align::Centre => x - width / 2.0,
            Align::Right => x - width,
        };
        let code = colour.encode(self.enc);
        let mut prev = None;
        for c in text.chars() {
            if let Some(p) = prev {
                pen += font.horizontal_kern(p, c, size).unwrap_or(0.0);
            }
            let (m, bitmap) = font.rasterize(c, size);
            let left = (pen + m.xmin as f32).round() as i64;
            let top = (y - m.height as f32 - m.ymin as f32).round() as i64;
            for (i, &cov) in bitmap.iter().enumerate() {
                let (gx, gy) = (left + (i % m.width) as i64, top + (i / m.width) as i64);
                if gx >= 0 && gy >= 0 {
                    self.blend(gx as usize, gy as usize, code, f32::from(cov) / 255.0);
                }
            }
            pen += m.advance_width;
            prev = Some(c);
        }
        width
    }
}

/// How much of pixel (`xx`, `yy`) of `area` its rounded shape covers: 1
/// inside, a soft edge at the corners.
fn cover(area: Area, radius: usize, xx: usize, yy: usize) -> f32 {
    let Area { x, y, w, h } = area;
    let r = radius as f32;
    // Distance into the corner squares, for a soft edge.
    let (dx, dy) = (
        (r - (xx - x) as f32 - 0.5).max((xx - x) as f32 + 0.5 - (w as f32 - r)),
        (r - (yy - y) as f32 - 0.5).max((yy - y) as f32 + 0.5 - (h as f32 - r)),
    );
    if dx > 0.0 && dy > 0.0 {
        (r + 0.5 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0)
    } else {
        1.0
    }
}

/// A box on the canvas, in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Area {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

/// How a line of text is set: font, size in pixels, colour and alignment.
#[derive(Clone, Copy)]
pub struct Style<'f> {
    pub font: &'f Font,
    pub size: f32,
    pub colour: Rgb,
    pub align: Align,
}

impl<'f> Style<'f> {
    pub fn new(font: &'f Font, size: f32, colour: Rgb, align: Align) -> Self {
        Self {
            font,
            size,
            colour,
            align,
        }
    }
}

/// The width of `text` in `font` at `size` pixels.
pub fn measure(font: &Font, size: f32, text: &str) -> f32 {
    let mut w = 0.0;
    let mut prev = None;
    for c in text.chars() {
        if let Some(p) = prev {
            w += font.horizontal_kern(p, c, size).unwrap_or(0.0);
        }
        w += font.metrics(c, size).advance_width;
        prev = Some(c);
    }
    w
}

/// How a slate's state reads: plain, or tinted for attention or an error.
/// The tint is on the words only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Plain,
    Attention,
    Error,
}

/// The four corners of the screen's band the slate moves between.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Corner {
    BottomLeft,
    BottomRight,
    TopRight,
    TopLeft,
}

/// What the slate says: the theater's name (empty for none), the state,
/// and a line of detail (empty for none).
#[derive(Debug, Clone, PartialEq)]
pub struct SlateText {
    pub venue: String,
    pub state: String,
    pub detail: String,
    pub tone: Tone,
}

/// Sizes and places, in pixels of a 2160-row raster (the mockups' 4096 x
/// 2160, `docs/ui/slate.png`).
const SLATE_MARGIN: f32 = 190.0;
const SLATE_NAME: f32 = 120.0;
const SLATE_STATE: f32 = 88.0;
const SLATE_DETAIL: f32 = 72.0;
/// Baselines below the name's: the state's and the detail's.
const SLATE_STATE_DY: f32 = 162.0;
const SLATE_DETAIL_DY: f32 = 290.0;
/// The block's distance from the band's edge, cap height to descender.
const SLATE_INSET: f32 = 140.0;

/// The slate's box on the canvas for `corner`, with the screen's band at
/// rows `band` (top, height): where [`slate`] draws, so that only it need be
/// cleared or copied. Half the width, on the corner's side.
pub fn slate_area(size: (usize, usize), band: (usize, usize), corner: Corner) -> Area {
    let (w, h) = size;
    let s = h as f32 / 2160.0;
    let block = ((SLATE_DETAIL_DY + SLATE_NAME) * s + 2.0 * SLATE_INSET * s) as usize;
    let block = block.min(band.1);
    let y = match corner {
        Corner::TopLeft | Corner::TopRight => band.0,
        Corner::BottomLeft | Corner::BottomRight => band.0 + band.1 - block,
    };
    let x = match corner {
        Corner::BottomLeft | Corner::TopLeft => 0,
        Corner::BottomRight | Corner::TopRight => w / 2,
    };
    Area {
        x,
        y,
        w: w - w / 2,
        h: block,
    }
}

/// The waiting slate, the quiet corner (#76): the theater's name, a notch
/// larger, with the state and its detail beneath, small and dim, in `area`
/// (from [`slate_area`]). The name sits at a fixed place and the lines below
/// have their space kept, so a line coming or going never moves it; without a
/// name the state keeps its own place.
pub fn slate(c: &mut Canvas<'_>, fonts: &Fonts, area: Area, corner: Corner, text: &SlateText) {
    let s = c.size().1 as f32 / 2160.0;
    let right = matches!(corner, Corner::BottomRight | Corner::TopRight);
    let (x, align) = if right {
        ((area.x + area.w) as f32 - SLATE_MARGIN * s, Align::Right)
    } else {
        (area.x as f32 + SLATE_MARGIN * s, Align::Left)
    };
    // The name's cap height is about 0.73 of its size.
    let name_y = area.y as f32 + SLATE_INSET * s + 0.73 * SLATE_NAME * s;
    if !text.venue.is_empty() {
        c.text(
            Style::new(&fonts.semibold, SLATE_NAME * s, palette::hint(), align),
            x,
            name_y,
            &text.venue,
        );
    }
    let (font, colour) = match text.tone {
        Tone::Plain => (&fonts.regular, palette::secondary()),
        Tone::Attention => (&fonts.semibold, palette::attention()),
        Tone::Error => (&fonts.semibold, palette::error()),
    };
    c.text(
        Style::new(font, SLATE_STATE * s, colour, align),
        x,
        name_y + SLATE_STATE_DY * s,
        &text.state,
    );
    if !text.detail.is_empty() {
        c.text(
            Style::new(&fonts.regular, SLATE_DETAIL * s, palette::hint(), align),
            x,
            name_y + SLATE_DETAIL_DY * s,
            &text.detail,
        );
    }
}

/// What a banner says: the picture type, and a quiet line about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BannerText {
    pub title: String,
    pub detail: String,
}

const BANNER_X: f32 = 140.0;
/// Below the band's top edge.
const BANNER_DY: f32 = 95.0;
const BANNER_TITLE: f32 = 86.0;
const BANNER_DETAIL: f32 = 64.0;
const BANNER_PAD: f32 = 104.0;
const BANNER_H: f32 = 345.0;
/// The banner's size against the mockup's, which drew it for a screen seen
/// from a page: at the screen it was too big (2026-10-14).
const BANNER_SCALE: f32 = 0.6;

/// The banner's box: top left inside the band, as wide as its text needs.
pub fn banner_area(
    size: (usize, usize),
    band_top: usize,
    fonts: &Fonts,
    text: &BannerText,
) -> Area {
    let (w, h) = size;
    let r = h as f32 / 2160.0;
    let s = r * BANNER_SCALE;
    let text_w = measure(&fonts.semibold, BANNER_TITLE * s, &text.title).max(measure(
        &fonts.regular,
        BANNER_DETAIL * s,
        &text.detail,
    ));
    // Where it sits is the raster's, not scaled with it.
    let x = (BANNER_X * r) as usize;
    Area {
        x,
        y: band_top + (BANNER_DY * r) as usize,
        w: ((text_w + 1.8 * BANNER_PAD * s) as usize).min(w - x),
        h: (BANNER_H * s) as usize,
    }
}

/// A banner (#77): a dark panel, 80% opaque, a lavender edge on its left,
/// the picture type over a quiet line about it, filling `area` (from
/// [`banner_area`]).
pub fn banner(c: &mut Canvas<'_>, fonts: &Fonts, area: Area, text: &BannerText) {
    let s = c.size().1 as f32 / 2160.0 * BANNER_SCALE;
    let px = |v: f32| (v * s) as usize;
    c.rect(area, px(39.0), palette::panel(), 0.8);
    c.rect(
        Area {
            x: area.x + px(10.0),
            y: area.y + px(68.0),
            w: px(13.0).max(2),
            h: px(205.0),
        },
        px(6.0),
        palette::accent(),
        1.0,
    );
    let x = area.x as f32 + BANNER_PAD * s;
    c.text(
        Style::new(
            &fonts.semibold,
            BANNER_TITLE * s,
            palette::text(),
            Align::Left,
        ),
        x,
        area.y as f32 + 150.0 * s,
        &text.title,
    );
    c.text(
        Style::new(
            &fonts.regular,
            BANNER_DETAIL * s,
            palette::secondary(),
            Align::Left,
        ),
        x,
        area.y as f32 + 270.0 * s,
        &text.detail,
    );
}

/// What the info overlay says: sections, each a label, a line and a quieter
/// line under it (`docs/ui/info.png`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfoText {
    pub sections: Vec<InfoSection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfoSection {
    /// In capitals, in the accent: IN, OUT, CROP.
    pub label: String,
    pub line: String,
    pub detail: String,
}

/// Sizes and places in pixels of a 2160-row raster, as the mockup draws
/// them, scaled down as the banner is.
const INFO_W: f32 = 1625.0;
const INFO_RIGHT: f32 = 78.0;
const INFO_DY: f32 = 72.0;
const INFO_PAD: f32 = 94.0;
const INFO_SECTION: f32 = 355.0;
const INFO_LABEL: f32 = 56.0;
const INFO_LINE: f32 = 68.0;
const INFO_DETAIL: f32 = 55.0;
const INFO_SCALE: f32 = BANNER_SCALE;

/// The info panel's box: the right of the screen's band (rows `band`),
/// below its top edge, as tall as its sections need and never past the
/// band.
pub fn info_area(size: (usize, usize), band: (usize, usize), text: &InfoText) -> Area {
    let (w, h) = size;
    let r = h as f32 / 2160.0;
    let s = r * INFO_SCALE;
    let pw = ((INFO_W * s) as usize).min(w);
    let ph = ((2.0 * INFO_PAD + INFO_SECTION * text.sections.len() as f32 - 0.25 * INFO_SECTION)
        * s) as usize;
    let y = band.0 + (INFO_DY * r) as usize;
    Area {
        x: w.saturating_sub(pw + (INFO_RIGHT * r) as usize),
        y,
        w: pw,
        h: ph.min((band.0 + band.1).saturating_sub(y)),
    }
}

/// The info overlay (#72): a dark panel, 80% opaque, with a quiet edge;
/// each section a label in the accent, a line, and a quieter line.
pub fn info(c: &mut Canvas<'_>, fonts: &Fonts, area: Area, text: &InfoText) {
    let s = c.size().1 as f32 / 2160.0 * INFO_SCALE;
    let px = |v: f32| (v * s) as usize;
    c.rect(area, px(58.0), palette::panel(), 0.8);
    c.outline(area, px(58.0), px(5.0).max(1), palette::hint(), 0.35);
    let x = area.x as f32 + INFO_PAD * s;
    let mut y = area.y as f32 + INFO_PAD * s;
    for sec in &text.sections {
        c.text(
            Style::new(
                &fonts.semibold,
                INFO_LABEL * s,
                palette::accent(),
                Align::Left,
            ),
            x,
            y + 0.73 * INFO_LABEL * s,
            &sec.label,
        );
        c.text(
            Style::new(&fonts.regular, INFO_LINE * s, palette::text(), Align::Left),
            x,
            y + 2.0 * INFO_LINE * s,
            &sec.line,
        );
        c.text(
            Style::new(
                &fonts.regular,
                INFO_DETAIL * s,
                palette::secondary(),
                Align::Left,
            ),
            x,
            y + 3.25 * INFO_LINE * s,
            &sec.detail,
        );
        y += INFO_SECTION * s;
    }
}

/// For `cinefit pattern --slate`: the slate, a banner, and grey steps at
/// known fractions of UI white, to judge on the screen that the overlay
/// shows, and that SDR and HDR10 give it the same light.
pub fn test_card(c: &mut Canvas<'_>, fonts: &Fonts, enc: Encoding) {
    let (w, h) = c.size();
    let s = h as f32 / 2160.0;
    let px = |v: f32| (v * s) as usize;
    c.clear();
    // The 2.35 band of the raster is where the screen is.
    let band_h = ((w as f64 / 2.35).round() as usize).min(h);
    let band = ((h - band_h) / 2, band_h);
    c.rect(
        Area {
            x: 0,
            y: band.0,
            w,
            h: band.1,
        },
        0,
        Rgb::grey(0.0),
        0.85,
    );
    let text = BannerText {
        title: "Dolby Vision".into(),
        detail: "4K 23.976 \u{b7} mapped by the player, shown as is".into(),
    };
    let area = banner_area((w, h), band.0, fonts, &text);
    banner(c, fonts, area, &text);
    let corner = Corner::BottomLeft;
    slate(
        c,
        fonts,
        slate_area((w, h), band, corner),
        corner,
        &SlateText {
            venue: "The Screening Room".into(),
            state: "Signal lost".into(),
            detail: "Overlay test card".into(),
            tone: Tone::Attention,
        },
    );
    let steps = [0.05, 0.1, 0.25, 0.5, 0.75, 1.0];
    let (sw, sh) = (px(260.0), px(110.0));
    let x0 = w / 2 - steps.len() * sw / 2;
    let y0 = px(1100.0);
    let label = Style::new(&fonts.regular, 32.0 * s, Rgb::grey(0.5), Align::Centre);
    for (i, &l) in steps.iter().enumerate() {
        c.rect(
            Area {
                x: x0 + i * sw,
                y: y0,
                w: sw,
                h: sh,
            },
            0,
            Rgb::grey(l),
            1.0,
        );
        let text = format!("{:.0}%", l * 100.0);
        c.text(
            label,
            (x0 + i * sw + sw / 2) as f32,
            (y0 + sh) as f32 + 50.0 * s,
            &text,
        );
    }
    let what = match enc {
        Encoding::Sdr => "SDR: steps at gamma 2.4".to_string(),
        Encoding::Pq => {
            format!("HDR10: steps at their light in nits, UI white = {UI_WHITE_NITS:.0}")
        }
    };
    c.text(label, w as f32 / 2.0, (y0 + sh) as f32 + 110.0 * s, &what);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pq_and_sdr_encode_known_points() {
        // ST 2084 reference points: 100 nits is about 0.508, 10,000 is 1.
        assert!((pq(100.0) - 0.5081).abs() < 0.001, "{}", pq(100.0));
        assert!((pq(10_000.0) - 1.0).abs() < 1e-5);
        assert_eq!(pq(0.0).min(0.001), pq(0.0));
        assert_eq!(encode(1.0, Encoding::Sdr), 255);
        assert_eq!(encode(0.0, Encoding::Sdr), 0);
        // UI white under PQ is 100 nits: half the signal range, not full.
        assert_eq!(encode(1.0, Encoding::Pq), 130);
        // A 50% grey: gamma 2.4 in SDR; its light in PQ.
        assert_eq!(encode(0.5, Encoding::Sdr), 191);
        assert_eq!(encode(0.5, Encoding::Pq), (pq(50.0) * 255.0).round() as u8);
    }

    #[test]
    fn text_draws_inside_the_canvas_and_measures_its_width() {
        let fonts = Fonts::load();
        let (w, h) = (400, 120);
        let mut px = vec![0u8; w * 4 * h];
        let mut c = Canvas::new(&mut px, w * 4, w, h, Encoding::Sdr);
        let width = c.text(
            Style::new(&fonts.regular, 40.0, Rgb::grey(1.0), Align::Centre),
            200.0,
            80.0,
            "No signal",
        );
        assert!((width - measure(&fonts.regular, 40.0, "No signal")).abs() < 1e-3);
        assert!(width > 100.0 && width < 300.0, "{width}");
        // Something drawn, opaque white at its most covered pixel, and
        // nothing outside the text's box.
        let alpha: Vec<u8> = px.chunks(4).map(|p| p[3]).collect();
        assert!(alpha.contains(&255));
        let inked: Vec<usize> = (0..w * h)
            .filter(|&i| alpha[i] > 0)
            .map(|i| i % w)
            .collect();
        let (lo, hi) = (*inked.iter().min().unwrap(), *inked.iter().max().unwrap());
        assert!(lo as f32 >= 200.0 - width / 2.0 - 2.0 && hi as f32 <= 200.0 + width / 2.0 + 2.0);
    }

    #[test]
    fn premultiplied_blending_keeps_colour_under_alpha() {
        let (w, h) = (4, 1);
        let mut px = vec![0u8; w * 4 * h];
        let mut c = Canvas::new(&mut px, w * 4, w, h, Encoding::Sdr);
        c.rect(
            Area {
                x: 0,
                y: 0,
                w: 4,
                h: 1,
            },
            0,
            Rgb::grey(1.0),
            0.5,
        );
        // Premultiplied: half-opaque white is 128 in every channel.
        assert_eq!(&px[..4], &[128, 128, 128, 128]);
    }

    #[test]
    fn palette_colours_go_out_in_bt2020_and_bgra_order() {
        // A grey is the same code in SDR as written: the conversion keeps it.
        assert_eq!(Rgb::hex(0x808080).encode(Encoding::Sdr), [128, 128, 128]);
        // Saturated BT.709 red is inside BT.2020: less red, some green.
        let [r, g, b] = Rgb::hex(0xff0000).encode(Encoding::Sdr);
        assert!(r < 255 && r > 200 && g > 50 && b > 0, "{r} {g} {b}");
        // The apricot's red channel lands in the R byte of BGRA.
        let (w, h) = (1, 1);
        let mut px = vec![0u8; 4];
        let mut c = Canvas::new(&mut px, w * 4, w, h, Encoding::Sdr);
        let full = Area { x: 0, y: 0, w, h };
        c.rect(full, 0, palette::attention(), 1.0);
        let [r, g, b] = palette::attention().encode(Encoding::Sdr);
        assert_eq!(px, [b, g, r, 255]);
        assert!(r > g && g > b);
    }

    #[test]
    fn a_window_draws_what_the_whole_canvas_draws_there() {
        let fonts = Fonts::load();
        let (w, h) = (640, 360);
        let text = SlateText {
            venue: "The Screening Room".into(),
            state: "Can't show this format".into(),
            detail: "8-bit RGB 4:4:4 \u{b7} set the player to YCbCr".into(),
            tone: Tone::Error,
        };
        let band = (50, 260);
        for corner in [Corner::BottomLeft, Corner::TopRight] {
            let area = slate_area((w, h), band, corner);
            let mut whole = vec![0u8; w * 4 * h];
            let mut c = Canvas::new(&mut whole, w * 4, w, h, Encoding::Pq);
            slate(&mut c, &fonts, area, corner, &text);
            let mut part = vec![0u8; area.w * 4 * area.h];
            let mut c = Canvas::window(&mut part, area.w * 4, area, (w, h), Encoding::Pq);
            slate(&mut c, &fonts, area, corner, &text);
            // Everything drawn is inside the area, and the window holds it.
            let mut inside = 0;
            for y in 0..h {
                for x in 0..w {
                    let p = &whole[(y * w + x) * 4..][..4];
                    let within = (area.x..area.x + area.w).contains(&x)
                        && (area.y..area.y + area.h).contains(&y);
                    if within {
                        let q = &part[((y - area.y) * area.w + x - area.x) * 4..][..4];
                        assert_eq!(p, q, "{corner:?} at {x},{y}");
                        inside += usize::from(p[3] > 0);
                    } else {
                        assert_eq!(p[3], 0, "{corner:?}: drawn outside the area at {x},{y}");
                    }
                }
            }
            assert!(inside > 100, "{corner:?}: nothing drawn");
        }
    }
}

#[cfg(test)]
mod preview {
    use super::*;

    /// `CINEFIT_OSD_PREVIEW=DIR cargo test -p cinefit osd::preview`: the test
    /// card as the overlay buffer holds it, raw BGRA at 1/2 size per encoding,
    /// to look at without a display.
    #[test]
    fn test_card_preview() {
        let Some(dir) = std::env::var_os("CINEFIT_OSD_PREVIEW") else {
            return;
        };
        let fonts = Fonts::load();
        // The slate and the banner at full size, as the overlay holds them.
        let (w, h) = (4096, 2160);
        let band = (209, 1742);
        let states = [
            ("No signal", "", Tone::Plain, "The Screening Room"),
            ("No signal", "", Tone::Plain, ""),
            (
                "Signal lost",
                "12 min ago \u{b7} waiting for the source",
                Tone::Attention,
                "The Screening Room",
            ),
            (
                "Can\u{2019}t show this format",
                "pixel format 0x00000007 \u{b7} set the player to YCbCr",
                Tone::Error,
                "The Screening Room",
            ),
        ];
        for (i, (state, detail, tone, venue)) in states.into_iter().enumerate() {
            let mut px = vec![0u8; w * 4 * h];
            let corner = [
                Corner::BottomLeft,
                Corner::BottomRight,
                Corner::TopRight,
                Corner::TopLeft,
            ][i];
            let mut c = Canvas::new(&mut px, w * 4, w, h, Encoding::Sdr);
            let text = SlateText {
                venue: venue.into(),
                state: state.into(),
                detail: detail.into(),
                tone,
            };
            slate(
                &mut c,
                &fonts,
                slate_area((w, h), band, corner),
                corner,
                &text,
            );
            let path = std::path::Path::new(&dir).join(format!("slate-{i}-{w}x{h}.bgra"));
            std::fs::write(path, &px).expect("write");
        }
        let mut px = vec![0u8; w * 4 * h];
        let mut c = Canvas::new(&mut px, w * 4, w, h, Encoding::Sdr);
        let text = BannerText {
            title: "Dolby Vision".into(),
            detail: "4K 23.976 \u{b7} mapped by the player, shown as is".into(),
        };
        banner(
            &mut c,
            &fonts,
            banner_area((w, h), band.0, &fonts, &text),
            &text,
        );
        let path = std::path::Path::new(&dir).join(format!("banner-{w}x{h}.bgra"));
        std::fs::write(path, &px).expect("write");
        let mut px = vec![0u8; w * 4 * h];
        let mut c = Canvas::new(&mut px, w * 4, w, h, Encoding::Sdr);
        let sec = |label: &str, line: &str, detail: &str| InfoSection {
            label: label.into(),
            line: line.into(),
            detail: detail.into(),
        };
        let text = InfoText {
            sections: vec![
                sec(
                    "IN",
                    "Dolby Vision \u{b7} low latency",
                    "3840 \u{d7} 2160 \u{b7} 23.976 \u{b7} YCbCr 4:2:2 10-bit",
                ),
                sec(
                    "OUT",
                    "SDR BT.2020 \u{b7} unchanged from the player",
                    "4096 \u{d7} 2160 \u{b7} 23.976",
                ),
                sec(
                    "CROP",
                    "2.40 : 1 \u{b7} 280 / 279 rows",
                    "zoom in (recent), 12 s ago",
                ),
                sec(
                    "LAST 10 S",
                    "240 shown \u{b7} 0 repeated",
                    "frame work 7.9 ms of 41.7 (render 5.4, detect 1.2)",
                ),
            ],
        };
        info(&mut c, &fonts, info_area((w, h), band, &text), &text);
        let path = std::path::Path::new(&dir).join(format!("info-{w}x{h}.bgra"));
        std::fs::write(path, &px).expect("write");

        let (w, h) = (2048, 1080);
        for enc in [Encoding::Sdr, Encoding::Pq] {
            let mut px = vec![0u8; w * 4 * h];
            test_card(&mut Canvas::new(&mut px, w * 4, w, h, enc), &fonts, enc);
            let path = std::path::Path::new(&dir).join(format!("osd-{enc:?}-{w}x{h}.bgra"));
            std::fs::write(path, &px).expect("write");
        }
    }
}
