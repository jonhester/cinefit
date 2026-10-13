//! The bar detector: frames in, rows of bar to crop out. Ported from the C++
//! version's `detect/`, cinefit's own detector, with its tests. The algorithm,
//! with the reason for each constant, is in this crate's README.md.
//!
//! The detector reads rows of luma and chroma through [`Samples`], in v210's
//! scale (10-bit limited: black 64, white 940, chroma centred on 512 with
//! +-448). One adapter per capture format converts into that scale, so the
//! C++ version's constants, measured on v210, apply to every format; the v210
//! adapter is a straight read, which keeps parity with the C++ results exact.
//!
//! Pure: no capture, GPU or display types. It reads rows as it needs them,
//! every sample of each, so a star in a night sky is never missed between
//! samples, and a frame with no bars costs a handful of rows.

pub mod bars;
#[cfg(test)]
mod synth;

/// One frame as the detector reads it.
pub trait Samples {
    fn width(&self) -> u32;
    fn height(&self) -> u32;
    /// Row `y`'s luma, one value per pixel, into `out` (`width` long).
    fn luma_row(&self, y: u32, out: &mut [u16]);
    /// Row `y`'s chroma, one `[Cb, Cr]` per two pixels, into `out`
    /// (`width.div_ceil(2)` long).
    fn chroma_row(&self, y: u32, out: &mut [[u16; 2]]);
}

/// A v210 frame, read in place: six pixels in four little-endian words,
///
/// ```text
/// word 0:  Cb0 Y0 Cr0    word 1:  Y1 Cb1 Y2
/// word 2:  Cr1 Y3 Cb2    word 3:  Y4 Cr2 Y5
/// ```
///
/// ten bits each, low first.
pub struct V210<'a> {
    bytes: &'a [u8],
    width: u32,
    height: u32,
    row_bytes: usize,
}

impl<'a> V210<'a> {
    /// `None` unless `bytes` holds `height` rows of `row_bytes`, each with
    /// room for `width` pixels.
    pub fn new(bytes: &'a [u8], width: u32, height: u32, row_bytes: usize) -> Option<Self> {
        let groups = (width as usize).div_ceil(6);
        (row_bytes >= groups * 16 && bytes.len() >= row_bytes.checked_mul(height as usize)?)
            .then_some(Self {
                bytes,
                width,
                height,
                row_bytes,
            })
    }

    /// Row `y`'s groups, each as its four words, read once.
    fn groups(&self, y: u32) -> impl Iterator<Item = [u32; 4]> + '_ {
        let len = (self.width as usize).div_ceil(6) * 16;
        self.bytes[y as usize * self.row_bytes..][..len]
            .as_chunks::<16>()
            .0
            .iter()
            .map(|g| {
                std::array::from_fn(|i| {
                    u32::from_le_bytes([g[4 * i], g[4 * i + 1], g[4 * i + 2], g[4 * i + 3]])
                })
            })
    }
}

fn field(w: u32, slot: u32) -> u16 {
    (w >> (10 * slot) & 0x3FF) as u16
}

impl Samples for V210<'_> {
    fn width(&self) -> u32 {
        self.width
    }

    fn height(&self) -> u32 {
        self.height
    }

    fn luma_row(&self, y: u32, out: &mut [u16]) {
        let w = self.width as usize;
        for (g, v) in self.groups(y).enumerate() {
            let six = [
                field(v[0], 1),
                field(v[1], 0),
                field(v[1], 2),
                field(v[2], 1),
                field(v[3], 0),
                field(v[3], 2),
            ];
            let x = g * 6;
            let n = (w - x).min(6);
            out[x..x + n].copy_from_slice(&six[..n]);
        }
    }

    fn chroma_row(&self, y: u32, out: &mut [[u16; 2]]) {
        let pairs = (self.width as usize).div_ceil(2);
        for (g, v) in self.groups(y).enumerate() {
            let three = [
                [field(v[0], 0), field(v[0], 2)],
                [field(v[1], 1), field(v[2], 0)],
                [field(v[2], 2), field(v[3], 1)],
            ];
            let p = g * 3;
            let n = (pairs - p).min(3);
            out[p..p + n].copy_from_slice(&three[..n]);
        }
    }
}
