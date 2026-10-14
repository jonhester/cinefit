//! On-screen text, drawn by the CPU into the overlay plane's buffer
//! (`cinefit_kms::Output::create_overlay`), which the display engine
//! composites over the video. Drawn only when it changes; the video path
//! never sees it.
//!
//! The overlay is blended in the output's own signal space, and the kernel
//! offers no per-plane colour conversion, so its code values must already
//! mean the right light for what the output signals: a gamma of 2.4 for SDR,
//! absolute light through PQ for HDR10. UI shades are given as a fraction of
//! UI white, which is SDR white, or [`UI_WHITE_NITS`] under PQ.
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
}

/// An overlay buffer to draw into: BGRA bytes, premultiplied alpha.
pub struct Canvas<'a> {
    px: &'a mut [u8],
    pitch: usize,
    w: usize,
    h: usize,
    enc: Encoding,
}

impl<'a> Canvas<'a> {
    pub fn new(px: &'a mut [u8], pitch: usize, w: usize, h: usize, enc: Encoding) -> Self {
        Self {
            px,
            pitch,
            w,
            h,
            enc,
        }
    }

    pub fn size(&self) -> (usize, usize) {
        (self.w, self.h)
    }

    /// Everything transparent.
    pub fn clear(&mut self) {
        for row in self.px.chunks_mut(self.pitch).take(self.h) {
            row[..self.w * 4].fill(0);
        }
    }

    /// One pixel of grey `level` at coverage `a`, over what is there.
    fn blend(&mut self, x: usize, y: usize, code: u8, a: f32) {
        if x >= self.w || y >= self.h || a <= 0.0 {
            return;
        }
        let o = y * self.pitch + x * 4;
        let p = &mut self.px[o..o + 4];
        let src = f32::from(code) * a;
        for c in &mut p[..3] {
            *c = (src + f32::from(*c) * (1.0 - a)).round() as u8;
        }
        p[3] = (255.0 * a + f32::from(p[3]) * (1.0 - a)).round() as u8;
    }

    /// `area` in grey `level` at opacity `alpha`, corners rounded by `radius`.
    pub fn rect(&mut self, area: Area, radius: usize, level: f32, alpha: f32) {
        let Area { x, y, w, h } = area;
        let code = encode(level, self.enc);
        let r = radius as f32;
        for yy in y..(y + h).min(self.h) {
            for xx in x..(x + w).min(self.w) {
                // Distance outside the rounded corner, for a soft edge.
                let (dx, dy) = (
                    (r - (xx - x) as f32 - 0.5).max((xx - x) as f32 + 0.5 - (w as f32 - r)),
                    (r - (yy - y) as f32 - 0.5).max((yy - y) as f32 + 0.5 - (h as f32 - r)),
                );
                let cover = if dx > 0.0 && dy > 0.0 {
                    (r + 0.5 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                self.blend(xx, yy, code, alpha * cover);
            }
        }
    }

    /// `text` set in `style`, its baseline at `y`; `x` is its left edge or
    /// its centre, by the style's alignment. Returns its width.
    pub fn text(&mut self, style: Style<'_>, x: f32, y: f32, text: &str) -> f32 {
        let Style {
            font,
            size,
            level,
            align,
        } = style;
        let width = measure(font, size, text);
        let mut pen = match align {
            Align::Left => x,
            Align::Centre => x - width / 2.0,
        };
        let code = encode(level, self.enc);
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

/// A box on the canvas, in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Area {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

/// How a line of text is set: font, size in pixels, grey (a fraction of UI
/// white) and alignment.
#[derive(Clone, Copy)]
pub struct Style<'f> {
    pub font: &'f Font,
    pub size: f32,
    pub level: f32,
    pub align: Align,
}

impl<'f> Style<'f> {
    pub fn new(font: &'f Font, size: f32, level: f32, align: Align) -> Self {
        Self {
            font,
            size,
            level,
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

/// The waiting slate: a state in plain words and a line of detail, centred
/// at `(cx, cy)`, dim. The caller moves the centre slowly over time.
pub fn slate(c: &mut Canvas<'_>, fonts: &Fonts, cx: f32, cy: f32, state: &str, detail: &str) {
    let s = c.size().1 as f32 / 2160.0;
    let centred = |font, size, level| Style::new(font, size * s, level, Align::Centre);
    c.text(
        centred(&fonts.semibold, 34.0, 0.10),
        cx,
        cy - 70.0 * s,
        "CINEFIT",
    );
    c.text(
        centred(&fonts.regular, 88.0, 0.22),
        cx,
        cy + 20.0 * s,
        state,
    );
    c.text(
        centred(&fonts.regular, 40.0, 0.08),
        cx,
        cy + 90.0 * s,
        detail,
    );
}

/// A banner: a title and a line of detail on a dark translucent panel, its
/// top-left corner at `(x, y)`.
pub fn banner(c: &mut Canvas<'_>, fonts: &Fonts, x: usize, y: usize, title: &str, detail: &str) {
    let s = c.size().1 as f32 / 2160.0;
    let title_style = Style::new(&fonts.semibold, 56.0 * s, 0.85, Align::Left);
    let detail_style = Style::new(&fonts.regular, 38.0 * s, 0.55, Align::Left);
    let pad = (40.0 * s) as usize;
    let w = measure(title_style.font, title_style.size, title).max(measure(
        detail_style.font,
        detail_style.size,
        detail,
    )) as usize
        + 2 * pad;
    let h = (200.0 * s) as usize;
    c.rect(Area { x, y, w, h }, (24.0 * s) as usize, 0.0, 0.6);
    c.text(title_style, (x + pad) as f32, y as f32 + 92.0 * s, title);
    c.text(detail_style, (x + pad) as f32, y as f32 + 152.0 * s, detail);
}

/// For `cinefit pattern --slate`: the slate, a banner, and grey steps at
/// known fractions of UI white, to judge on the screen that the overlay
/// shows, and that SDR and HDR10 give it the same light.
pub fn test_card(c: &mut Canvas<'_>, fonts: &Fonts, enc: Encoding) {
    let (w, h) = c.size();
    let s = h as f32 / 2160.0;
    let px = |v: f32| (v * s) as usize;
    c.clear();
    // The 2.35 band of a 17:9 raster is where the screen is.
    c.rect(
        Area {
            x: 0,
            y: px(380.0),
            w,
            h: px(1400.0),
        },
        0,
        0.0,
        0.85,
    );
    banner(
        c,
        fonts,
        px(160.0),
        px(440.0),
        "Dolby Vision \u{b7} low latency",
        "3840 \u{d7} 2160 \u{b7} 23.976 \u{b7} SDR BT.2020 out",
    );
    slate(
        c,
        fonts,
        w as f32 / 2.0,
        h as f32 / 2.0,
        "No signal",
        "Overlay test card",
    );
    let steps = [0.05, 0.1, 0.25, 0.5, 0.75, 1.0];
    let (sw, sh) = (px(260.0), px(110.0));
    let x0 = w / 2 - steps.len() * sw / 2;
    let y0 = px(1500.0);
    let label = Style::new(&fonts.regular, 32.0 * s, 0.5, Align::Centre);
    for (i, &l) in steps.iter().enumerate() {
        c.rect(
            Area {
                x: x0 + i * sw,
                y: y0,
                w: sw,
                h: sh,
            },
            0,
            l,
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
            Style::new(&fonts.regular, 40.0, 1.0, Align::Centre),
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
            1.0,
            0.5,
        );
        // Premultiplied: half-opaque white is 128 in every channel.
        assert_eq!(&px[..4], &[128, 128, 128, 128]);
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
        let (w, h) = (2048, 1080);
        for enc in [Encoding::Sdr, Encoding::Pq] {
            let mut px = vec![0u8; w * 4 * h];
            test_card(&mut Canvas::new(&mut px, w * 4, w, h, enc), &fonts, enc);
            let path = std::path::Path::new(&dir).join(format!("osd-{enc:?}-{w}x{h}.bgra"));
            std::fs::write(path, &px).expect("write");
        }
    }
}
