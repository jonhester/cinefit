//! Synthetic v210 frames for the detector's tests, as the C++ version builds
//! them: a palette of pre-packed rows, so a 3840x2160 frame costs a copy, not
//! a pack, and motion comes from choosing different palette rows per frame.

use crate::V210;

/// The C++ version's hash, so the textures match its tests'.
pub fn hash(a: u32, b: u32, c: u32) -> u32 {
    let mut h = a.wrapping_mul(2654435761)
        ^ (b.wrapping_add(0x9e3779b9).wrapping_add(a << 6))
        ^ c.wrapping_mul(40503);
    h ^= h >> 15;
    h = h.wrapping_mul(2246822519);
    h ^= h >> 13;
    h
}

pub fn row_bytes(width: u32) -> usize {
    (width as usize).div_ceil(6) * 16
}

/// One packed row, from 10-bit luma and chroma (Cb, Cr, Cb, ... per pixel
/// position, as the C++ version's planar chroma).
#[derive(Clone)]
pub struct Row(Vec<u8>);

impl Row {
    pub fn new(width: u32, luma: &[u16], chroma: &[u16]) -> Self {
        let w = width as usize;
        let mut out = vec![0u8; row_bytes(width)];
        let l = |x: usize| u32::from(if x < w { luma[x] } else { 0 });
        // Pair p's Cb and Cr sit at chroma[2p] and chroma[2p + 1].
        let c = |i: usize| u32::from(if i < w { chroma[i] } else { 0 });
        for (g, bytes) in out.chunks_mut(16).enumerate() {
            let (x, p) = (g * 6, g * 3);
            let word = |a: u32, b: u32, d: u32| (a & 0x3FF) | (b & 0x3FF) << 10 | (d & 0x3FF) << 20;
            let words = [
                word(c(2 * p), l(x), c(2 * p + 1)),
                word(l(x + 1), c(2 * p + 2), l(x + 2)),
                word(c(2 * p + 3), l(x + 3), c(2 * p + 4)),
                word(l(x + 4), c(2 * p + 5), l(x + 5)),
            ];
            for (i, w) in words.iter().enumerate() {
                bytes[4 * i..4 * i + 4].copy_from_slice(&w.to_le_bytes());
            }
        }
        Row(out)
    }

    /// A row from a function of x (luma), neutral chroma.
    pub fn luma(width: u32, f: impl Fn(u32) -> u32) -> Self {
        let luma: Vec<u16> = (0..width).map(|x| f(x) as u16).collect();
        Row::new(width, &luma, &vec![512; width as usize])
    }

    fn make(width: u32, fy: impl Fn(u32) -> u32, fc: impl Fn(u32) -> u32) -> Self {
        let luma: Vec<u16> = (0..width).map(|x| fy(x) as u16).collect();
        let chroma: Vec<u16> = (0..width).map(|x| fc(x) as u16).collect();
        Row::new(width, &luma, &chroma)
    }
}

pub const VARIANTS: u32 = 16;

/// Every row kind, with variants so frames can move.
pub struct Palette {
    pub width: u32,
    /// Flat black at 64, neutral chroma.
    pub bar: Row,
    /// 64 +- 1, the dithered case.
    pub bar_noisy: Row,
    /// Bright, textured, broad.
    pub picture: Vec<Row>,
    /// Textured, 64..100: broad but dim.
    #[expect(dead_code, reason = "for the policy and detector tests, next")]
    pub dark: Vec<Row>,
    /// Black except one bright object.
    #[expect(dead_code, reason = "for the policy and detector tests, next")]
    pub object: Vec<Row>,
    /// Letter strokes across the middle.
    pub text: Vec<Row>,
    /// Dark but coloured.
    pub teal: Row,
}

impl Palette {
    pub fn new(w: u32) -> Self {
        let neutral = |_| 512;
        let variants = |f: &dyn Fn(u32) -> Row| (0..VARIANTS).map(f).collect::<Vec<_>>();
        Palette {
            width: w,
            bar: Row::make(w, |_| 64, neutral),
            bar_noisy: Row::make(w, |x| 63 + hash(x, 7, 0) % 3, neutral),
            teal: Row::make(w, |_| 72, |x| if x % 2 == 1 { 430 } else { 590 }),
            picture: variants(&|v| {
                Row::make(
                    w,
                    |x| 180 + hash(x, v, 1) % 620,
                    |x| 430 + hash(x, v, 2) % 160,
                )
            }),
            dark: variants(&|v| {
                Row::make(w, |x| 64 + hash(x, v, 3) % 36, |x| 508 + hash(x, v, 4) % 8)
            }),
            object: variants(&|v| {
                let ox = 400 + v * w / (VARIANTS + 4);
                Row::make(
                    w,
                    |x| if x >= ox && x < ox + w / 40 { 700 } else { 64 },
                    neutral,
                )
            }),
            text: variants(&|v| {
                Row::make(
                    w,
                    |x| {
                        let span = x > w / 4 && x < 3 * w / 4;
                        if span && hash(x / 3, v, 5).is_multiple_of(4) {
                            900
                        } else {
                            64
                        }
                    },
                    neutral,
                )
            }),
        }
    }
}

/// A frame under construction: one palette row per line, black to start.
pub struct Frame<'a> {
    width: u32,
    rows: Vec<&'a Row>,
    bytes: Vec<u8>,
}

impl<'a> Frame<'a> {
    pub fn new(p: &'a Palette, height: u32) -> Self {
        Frame {
            width: p.width,
            rows: vec![&p.bar; height as usize],
            bytes: Vec::new(),
        }
    }

    pub fn height(&self) -> u32 {
        self.rows.len() as u32
    }

    pub fn set(&mut self, y: u32, r: &'a Row) {
        self.rows[y as usize] = r;
    }

    pub fn fill(&mut self, y0: u32, y1: u32, r: &'a Row) {
        for y in y0..y1.min(self.height()) {
            self.rows[y as usize] = r;
        }
    }

    /// Rows `top .. h - bottom` from `kind`, the variant chosen per row and
    /// frame.
    pub fn content(&mut self, top: u32, bottom: u32, kind: &'a [Row], frame: u32) {
        for y in top..self.height() - bottom {
            self.rows[y as usize] = &kind[(hash(y / 7, frame / 3, 0) % VARIANTS) as usize];
        }
    }

    /// The packed frame.
    pub fn pack(&mut self) -> V210<'_> {
        let rb = row_bytes(self.width);
        self.bytes.clear();
        for r in &self.rows {
            self.bytes.extend_from_slice(&r.0);
        }
        V210::new(&self.bytes, self.width, self.height(), rb).expect("whole rows")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Samples;

    #[test]
    fn packed_rows_read_back_through_the_v210_adapter() {
        let w = 13; // a partial last group
        let luma: Vec<u16> = (0..w).map(|x| 100 + x as u16).collect();
        let chroma: Vec<u16> = (0..w).map(|x| 400 + x as u16).collect();
        let row = Row::new(w, &luma, &chroma);
        let v = V210::new(&row.0, w, 1, row_bytes(w)).unwrap();
        let mut l = vec![0; w as usize];
        v.luma_row(0, &mut l);
        assert_eq!(l, luma);
        let mut c = vec![[0; 2]; (w as usize).div_ceil(2)];
        v.chroma_row(0, &mut c);
        assert_eq!(c[0], [400, 401]);
        assert_eq!(c[5], [410, 411]);
    }
}
