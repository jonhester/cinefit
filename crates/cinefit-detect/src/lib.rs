//! The bar detector: frames in, rows of bar to crop out. Ported from the C++
//! version's `detect/`, with its tests. The README has the short version of
//! how it works; the reasons for each constant are on `bars::Params`.
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

pub mod ar_policy;
pub mod bars;
pub mod cuts;
pub mod detector;
pub mod hindsight;
pub mod readings;

pub use ar_policy::{Crop, Decision, Diagnostics};
pub use detector::Detector;
#[cfg(test)]
mod synth;

/// One frame as the detector reads it.
///
/// An adapter needs only the first four methods. The counting methods are
/// what the bar scan calls on hundreds of rows a frame; their defaults unpack
/// the row and count, and a format can count in place instead, as v210 does
/// (the C++ version's #9: 6.5 to 0.9 ms a 2160p frame). Any override must
/// give exactly the default's answers.
pub trait Samples {
    fn width(&self) -> u32;
    fn height(&self) -> u32;
    /// Row `y`'s luma, one value per pixel, into `out` (`width` long).
    fn luma_row(&self, y: u32, out: &mut [u16]);
    /// Row `y`'s chroma, one `[Cb, Cr]` per two pixels, into `out`
    /// (`width.div_ceil(2)` long).
    fn chroma_row(&self, y: u32, out: &mut [[u16; 2]]);

    /// Row `y`'s lit samples: luma above `thr`, or either chroma of the
    /// pixel's pair more than `dc` from neutral. Returns the count, a bit per
    /// horizontal quarter with a lit sample, and how many are lit by chroma
    /// alone.
    fn count_lit(&self, y: u32, thr: u16, dc: u32, s: &mut RowScratch) -> (u32, u8, u32) {
        s.read(self, y);
        pair_flags(&s.chroma, dc, &mut s.flags);
        count_quarters(&s.luma, &s.flags, thr)
    }

    /// Row `y`'s chroma pairs more than `dc` from neutral, one flag each,
    /// into `out` (`width.div_ceil(2)` long).
    fn chroma_flags(&self, y: u32, dc: u32, out: &mut Vec<u8>, s: &mut RowScratch) {
        s.read(self, y);
        pair_flags(&s.chroma, dc, out);
    }

    /// Row `y`'s luma at the columns `xs`, into `out` (as long as `xs`): for
    /// a sampler that reads a grid of points, not whole rows.
    fn luma_at(&self, y: u32, xs: &[u32], out: &mut [u16], s: &mut RowScratch) {
        s.luma.resize(self.width() as usize, 0);
        self.luma_row(y, &mut s.luma);
        for (o, &x) in out.iter_mut().zip(xs) {
            *o = s.luma[x as usize];
        }
    }

    /// As [`Samples::count_lit`], with each pair's chroma taken from `flags`
    /// instead of tested: the count and the quarters.
    fn count_lit_flags(&self, y: u32, thr: u16, flags: &[u8], s: &mut RowScratch) -> (u32, u8) {
        s.read(self, y);
        let (n, mask, _) = count_quarters(&s.luma, flags, thr);
        (n, mask)
    }
}

/// Rows for the default counting methods, kept between calls so counting
/// allocates nothing.
#[derive(Default)]
pub struct RowScratch {
    luma: Vec<u16>,
    chroma: Vec<[u16; 2]>,
    flags: Vec<u8>,
}

impl RowScratch {
    fn read<S: Samples + ?Sized>(&mut self, f: &S, y: u32) {
        let w = f.width() as usize;
        self.luma.resize(w, 0);
        self.chroma.resize(w.div_ceil(2), [512, 512]);
        f.luma_row(y, &mut self.luma);
        f.chroma_row(y, &mut self.chroma);
    }
}

/// Chroma more than `dc` from neutral, as the C++ version tests it: one
/// unsigned compare, `c - (512 - dc)` wrapping for `c` below.
pub(crate) fn chroma_off(c: u32, dc: u32) -> bool {
    c.wrapping_sub(512 - dc) > 2 * dc
}

fn pair_flags(chroma: &[[u16; 2]], dc: u32, out: &mut Vec<u8>) {
    out.clear();
    out.extend(
        chroma
            .iter()
            .map(|c| u8::from(chroma_off(c[0].into(), dc) | chroma_off(c[1].into(), dc))),
    );
}

/// Lit samples in a row by quarter, from its luma and per-pair chroma flags.
fn count_quarters(luma: &[u16], flags: &[u8], thr: u16) -> (u32, u8, u32) {
    let w = luma.len();
    let (mut n, mut mask, mut chroma_only) = (0u32, 0u8, 0u32);
    for q in 0..4 {
        let (x0, x1) = (q * w / 4, (q + 1) * w / 4);
        let mut m = 0u32;
        for (x, &l) in luma[x0..x1].iter().enumerate() {
            let yl = u32::from(l > thr);
            let cl = u32::from(flags[(x0 + x) / 2]);
            m += yl | cl;
            chroma_only += cl & (yl ^ 1);
        }
        n += m;
        mask |= u8::from(m > 0) << q;
    }
    (n, mask, chroma_only)
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

    /// The frame's bytes, as given.
    pub fn bytes(&self) -> &'a [u8] {
        self.bytes
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

fn words(g: &[u8; 16]) -> [u32; 4] {
    std::array::from_fn(|i| {
        u32::from_le_bytes([g[4 * i], g[4 * i + 1], g[4 * i + 2], g[4 * i + 3]])
    })
}

/// The six luma and three chroma pairs of a group, as (word, slot).
const Y_AT: [(usize, u32); 6] = [(0, 1), (1, 0), (1, 2), (2, 1), (3, 0), (3, 2)];
const CB_AT: [(usize, u32); 3] = [(0, 0), (1, 1), (2, 2)];
const CR_AT: [(usize, u32); 3] = [(0, 2), (2, 0), (3, 1)];

impl V210<'_> {
    /// Row `y`'s whole groups of six pixels, in place.
    fn row(&self, y: u32) -> &[[u8; 16]] {
        let len = (self.width as usize).div_ceil(6) * 16;
        self.bytes[y as usize * self.row_bytes..][..len]
            .as_chunks::<16>()
            .0
    }

    /// Group `g`'s pair `k` lit by chroma.
    fn pair_off(v: &[u32; 4], k: usize, dc: u32) -> u32 {
        let (cb, cr) = (CB_AT[k], CR_AT[k]);
        u32::from(
            chroma_off(field(v[cb.0], cb.1).into(), dc)
                | chroma_off(field(v[cr.0], cr.1).into(), dc),
        )
    }
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

    // The counting methods straight from the packed words: no unpack, each
    // chroma pair tested once for the two pixels that share it. Each quarter's
    // whole groups run as one branch-free loop the compiler can vectorise; the
    // pixels of a group cut by a quarter's edge go one at a time.

    fn count_lit(&self, y: u32, thr: u16, dc: u32, _: &mut RowScratch) -> (u32, u8, u32) {
        let (row, w, thr) = (self.row(y), self.width as usize, u32::from(thr));
        let mut chroma_only = 0u32;
        let mut pixel = |x: usize| {
            let v = words(&row[x / 6]);
            let i = x % 6;
            let yl = u32::from(u32::from(field(v[Y_AT[i].0], Y_AT[i].1)) > thr);
            let cl = Self::pair_off(&v, i / 2, dc);
            chroma_only += cl & (yl ^ 1);
            yl | cl
        };
        let groups = |g0: usize, g1: usize| {
            let (mut n, mut co) = (0u32, 0u32);
            for g in &row[g0..g1] {
                let v = words(g);
                let c = [
                    Self::pair_off(&v, 0, dc),
                    Self::pair_off(&v, 1, dc),
                    Self::pair_off(&v, 2, dc),
                ];
                for (i, &(word, slot)) in Y_AT.iter().enumerate() {
                    let yl = u32::from(u32::from(field(v[word], slot)) > thr);
                    n += yl | c[i / 2];
                    co += c[i / 2] & (yl ^ 1);
                }
            }
            (n, co)
        };
        let (mut n, mut mask, mut co_groups) = (0u32, 0u8, 0u32);
        for q in 0..4 {
            let (x0, x1) = (q * w / 4, (q + 1) * w / 4);
            let (g0, g1) = (x0.div_ceil(6), x1 / 6);
            let mut m = 0u32;
            if g0 < g1 {
                m += (x0..g0 * 6).map(&mut pixel).sum::<u32>();
                let (gn, gco) = groups(g0, g1);
                m += gn;
                co_groups += gco;
                m += (g1 * 6..x1).map(&mut pixel).sum::<u32>();
            } else {
                m += (x0..x1).map(&mut pixel).sum::<u32>();
            }
            n += m;
            mask |= u8::from(m > 0) << q;
        }
        (n, mask, chroma_only + co_groups)
    }

    fn luma_at(&self, y: u32, xs: &[u32], out: &mut [u16], _: &mut RowScratch) {
        let row = self.row(y);
        for (o, &x) in out.iter_mut().zip(xs) {
            let (word, slot) = Y_AT[x as usize % 6];
            *o = field(words(&row[x as usize / 6])[word], slot);
        }
    }

    fn chroma_flags(&self, y: u32, dc: u32, out: &mut Vec<u8>, _: &mut RowScratch) {
        let pairs = (self.width as usize).div_ceil(2);
        out.clear();
        for g in self.row(y) {
            let v = words(g);
            for k in 0..3 {
                out.push(Self::pair_off(&v, k, dc) as u8);
            }
        }
        out.truncate(pairs);
    }

    fn count_lit_flags(&self, y: u32, thr: u16, flags: &[u8], _: &mut RowScratch) -> (u32, u8) {
        let (row, w, thr) = (self.row(y), self.width as usize, u32::from(thr));
        let pixel = |x: usize| {
            let (word, slot) = Y_AT[x % 6];
            u32::from(u32::from(field(words(&row[x / 6])[word], slot)) > thr)
                | u32::from(flags[x / 2])
        };
        let groups = |g0: usize, g1: usize| {
            let mut n = 0u32;
            for (g, bytes) in row.iter().enumerate().take(g1).skip(g0) {
                let v = words(bytes);
                for (i, &(word, slot)) in Y_AT.iter().enumerate() {
                    n += u32::from(u32::from(field(v[word], slot)) > thr)
                        | u32::from(flags[3 * g + i / 2]);
                }
            }
            n
        };
        let (mut n, mut mask) = (0u32, 0u8);
        for q in 0..4 {
            let (x0, x1) = (q * w / 4, (q + 1) * w / 4);
            let (g0, g1) = (x0.div_ceil(6), x1 / 6);
            let m = if g0 < g1 {
                (x0..g0 * 6).map(pixel).sum::<u32>()
                    + groups(g0, g1)
                    + (g1 * 6..x1).map(pixel).sum::<u32>()
            } else {
                (x0..x1).map(pixel).sum()
            };
            n += m;
            mask |= u8::from(m > 0) << q;
        }
        (n, mask)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A v210 frame seen only through the four required methods, so the
    /// counting methods are the trait's defaults.
    struct Plain<'a>(V210<'a>);

    impl Samples for Plain<'_> {
        fn width(&self) -> u32 {
            self.0.width()
        }
        fn height(&self) -> u32 {
            self.0.height()
        }
        fn luma_row(&self, y: u32, out: &mut [u16]) {
            self.0.luma_row(y, out);
        }
        fn chroma_row(&self, y: u32, out: &mut [[u16; 2]]) {
            self.0.chroma_row(y, out);
        }
    }

    /// Rows of random v210 near the thresholds: luma around black, chroma
    /// around neutral, so every test goes both ways.
    fn frame(w: u32, h: u32, seed: u64) -> (Vec<u8>, usize) {
        let row_bytes = (w as usize).div_ceil(6) * 16 + 16;
        let mut bytes = vec![0u8; row_bytes * h as usize];
        let mut x = seed;
        let mut next = |lo: u32, span: u32| {
            x = x
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            lo + ((x >> 33) as u32 % span)
        };
        for y in 0..h as usize {
            for g in 0..(w as usize).div_ceil(6) {
                // Word layout: Cb Y Cr | Y Cb Y | Cr Y Cb | Y Cr Y.
                let mut words = [0u32; 4];
                for &(word, slot) in &Y_AT {
                    words[word] |= next(40, 80) << (10 * slot);
                }
                for &(word, slot) in CB_AT.iter().chain(CR_AT.iter()) {
                    words[word] |= next(470, 85) << (10 * slot);
                }
                for (k, wd) in words.iter().enumerate() {
                    let o = y * row_bytes + g * 16 + 4 * k;
                    bytes[o..o + 4].copy_from_slice(&wd.to_le_bytes());
                }
            }
        }
        (bytes, row_bytes)
    }

    #[test]
    fn v210_counts_in_place_exactly_as_the_defaults_do() {
        // Widths that cut quarters mid-group, end mid-group, and are odd.
        for (w, seed) in [(3840, 1), (1920, 2), (1918, 3), (1001, 4), (70, 5), (64, 6)] {
            let h = 8;
            let (bytes, row_bytes) = frame(w, h, seed);
            let fast = V210::new(&bytes, w, h, row_bytes).expect("fits");
            let plain = Plain(V210::new(&bytes, w, h, row_bytes).expect("fits"));
            let mut s = RowScratch::default();
            let (mut a, mut b) = (Vec::new(), Vec::new());
            for y in 0..h {
                for (thr, dc) in [(64, 16), (80, 16), (100, 30), (63, 0)] {
                    assert_eq!(
                        fast.count_lit(y, thr, dc, &mut s),
                        plain.count_lit(y, thr, dc, &mut s),
                        "count_lit w {w} y {y} thr {thr} dc {dc}"
                    );
                    fast.chroma_flags(y, dc, &mut a, &mut s);
                    plain.chroma_flags(y, dc, &mut b, &mut s);
                    assert_eq!(a, b, "chroma_flags w {w} y {y} dc {dc}");
                    let xs: Vec<u32> = (0..w).step_by(7).chain([w - 1]).collect();
                    let (mut pa, mut pb) = (vec![0u16; xs.len()], vec![0u16; xs.len()]);
                    fast.luma_at(y, &xs, &mut pa, &mut s);
                    plain.luma_at(y, &xs, &mut pb, &mut s);
                    assert_eq!(pa, pb, "luma_at w {w} y {y}");
                    // Flags with runs cut out, as the bar scan passes them.
                    let flags: Vec<u8> = a
                        .iter()
                        .enumerate()
                        .map(|(i, &f)| f & u8::from(i % 3 != 1))
                        .collect();
                    assert_eq!(
                        fast.count_lit_flags(y, thr, &flags, &mut s),
                        plain.count_lit_flags(y, thr, &flags, &mut s),
                        "count_lit_flags w {w} y {y} thr {thr}"
                    );
                }
            }
        }
    }
}
