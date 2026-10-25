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

/// `f`, compiled for AVX2 when the CPU has it. The counting loops are
/// written for the compiler to vectorise, and AVX2 runs twice the samples of
/// the baseline's SSE2 to an instruction: a build for any x86-64 (as
/// `tools/install.sh` makes) counts in half the time on a CPU that has it.
/// The same source either way, so the same answers.
///
/// Pass the closure `#[inline(always)]`: otherwise it is compiled once, for
/// the baseline, and the AVX2 copy only calls it (measured: no faster).
#[inline(always)]
fn wide<R>(f: impl FnOnce() -> R) -> R {
    #[cfg(target_arch = "x86_64")]
    if avx2() {
        #[target_feature(enable = "avx2")]
        fn with_avx2<R>(f: impl FnOnce() -> R) -> R {
            f()
        }
        // SAFETY: `with_avx2` needs only AVX2, which the CPU was just found
        // to have.
        return unsafe { with_avx2(f) };
    }
    f()
}

#[cfg(target_arch = "x86_64")]
fn avx2() -> bool {
    #[cfg(test)]
    if tests::PORTABLE.get() {
        return false;
    }
    std::arch::is_x86_feature_detected!("avx2")
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

    /// Whole groups' lit pixels, and those lit by chroma alone. Always
    /// inlined, as the loops below are, so that [`wide`] compiles them.
    #[inline(always)]
    fn groups_lit(row: &[[u8; 16]], thr: u32, dc: u32) -> (u32, u32) {
        let (mut n, mut co) = (0u32, 0u32);
        for g in row {
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
    }

    /// Whole groups' lit pixels, each pair's chroma taken from `flags`,
    /// three a group.
    #[inline(always)]
    fn groups_lit_flags(row: &[[u8; 16]], flags: &[u8], thr: u32) -> u32 {
        let mut n = 0u32;
        for (bytes, f) in row.iter().zip(flags.as_chunks::<3>().0) {
            let v = words(bytes);
            for (i, &(word, slot)) in Y_AT.iter().enumerate() {
                n += u32::from(u32::from(field(v[word], slot)) > thr) | u32::from(f[i / 2]);
            }
        }
        n
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
        wide(
            #[inline(always)]
            || {
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
                let (mut n, mut mask, mut co_groups) = (0u32, 0u8, 0u32);
                for q in 0..4 {
                    let (x0, x1) = (q * w / 4, (q + 1) * w / 4);
                    let (g0, g1) = (x0.div_ceil(6), x1 / 6);
                    let mut m = 0u32;
                    if g0 < g1 {
                        m += (x0..g0 * 6).map(&mut pixel).sum::<u32>();
                        let (gn, gco) = Self::groups_lit(&row[g0..g1], thr, dc);
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
            },
        )
    }

    fn luma_at(&self, y: u32, xs: &[u32], out: &mut [u16], _: &mut RowScratch) {
        let row = self.row(y);
        for (o, &x) in out.iter_mut().zip(xs) {
            let (word, slot) = Y_AT[x as usize % 6];
            *o = field(words(&row[x as usize / 6])[word], slot);
        }
    }

    fn chroma_flags(&self, y: u32, dc: u32, out: &mut Vec<u8>, _: &mut RowScratch) {
        wide(
            #[inline(always)]
            || {
                let pairs = (self.width as usize).div_ceil(2);
                out.clear();
                for g in self.row(y) {
                    let v = words(g);
                    for k in 0..3 {
                        out.push(Self::pair_off(&v, k, dc) as u8);
                    }
                }
                out.truncate(pairs);
            },
        )
    }

    fn count_lit_flags(&self, y: u32, thr: u16, flags: &[u8], _: &mut RowScratch) -> (u32, u8) {
        wide(
            #[inline(always)]
            || {
                let (row, w, thr) = (self.row(y), self.width as usize, u32::from(thr));
                let pixel = |x: usize| {
                    let (word, slot) = Y_AT[x % 6];
                    u32::from(u32::from(field(words(&row[x / 6])[word], slot)) > thr)
                        | u32::from(flags[x / 2])
                };
                let (mut n, mut mask) = (0u32, 0u8);
                for q in 0..4 {
                    let (x0, x1) = (q * w / 4, (q + 1) * w / 4);
                    let (g0, g1) = (x0.div_ceil(6), x1 / 6);
                    let m = if g0 < g1 {
                        (x0..g0 * 6).map(pixel).sum::<u32>()
                            + Self::groups_lit_flags(&row[g0..g1], &flags[3 * g0..3 * g1], thr)
                            + (g1 * 6..x1).map(pixel).sum::<u32>()
                    } else {
                        (x0..x1).map(pixel).sum()
                    };
                    n += m;
                    mask |= u8::from(m > 0) << q;
                }
                (n, mask)
            },
        )
    }
}

/// A tunneled Dolby Vision frame, read in place (#109): the source's 12-bit
/// 4:2:2 packed into 8-bit RGB, which the card delivers as `r210`, each
/// 8-bit code shifted left by two, one big-endian word a pixel:
///
/// ```text
/// G = Y[11:4]    R = C[11:4]    B = C[3:0] << 4 | Y[3:0]
/// ```
///
/// with C Cb on even pixels and Cr on odd ones (as `cinefit_render::tunnel`
/// unpacks it, which the GPU's unpack is checked against). Given as 10-bit
/// codes, the 12-bit ones' top ten bits, like v210's. For cuts and
/// fingerprints: the tunnel's codes are the source's own (ICtCp or YCbCr,
/// often full range), so its levels are not the HDR10 or LLDV picture's;
/// the bar detector reads it through [`TunnelBars`].
pub struct Tunnel<'a> {
    bytes: &'a [u8],
    width: u32,
    height: u32,
    row_bytes: usize,
}

impl<'a> Tunnel<'a> {
    /// `None` for an odd width, or unless `bytes` holds `height` rows of
    /// `row_bytes`, each with room for `width` pixels.
    pub fn new(bytes: &'a [u8], width: u32, height: u32, row_bytes: usize) -> Option<Self> {
        (width.is_multiple_of(2)
            && row_bytes >= width as usize * 4
            && bytes.len() >= row_bytes.checked_mul(height as usize)?)
        .then_some(Self {
            bytes,
            width,
            height,
            row_bytes,
        })
    }

    /// Pixel `x` of row `y` as (luma, chroma), 12-bit.
    fn pixel(&self, x: usize, y: u32) -> (u16, u16) {
        let o = y as usize * self.row_bytes + 4 * x;
        let w = u32::from_be_bytes(self.bytes[o..o + 4].try_into().expect("4 bytes"));
        let [r, g, b] = [w >> 22, w >> 12, w >> 2].map(|c| (c & 0xFF) as u16);
        (g << 4 | b & 0xF, r << 4 | b >> 4)
    }
}

impl Samples for Tunnel<'_> {
    fn width(&self) -> u32 {
        self.width
    }

    fn height(&self) -> u32 {
        self.height
    }

    fn luma_row(&self, y: u32, out: &mut [u16]) {
        for (x, o) in out.iter_mut().enumerate().take(self.width as usize) {
            *o = self.pixel(x, y).0 >> 2;
        }
    }

    fn chroma_row(&self, y: u32, out: &mut [[u16; 2]]) {
        for (k, o) in out.iter_mut().enumerate().take(self.width as usize / 2) {
            *o = [self.pixel(2 * k, y).1 >> 2, self.pixel(2 * k + 1, y).1 >> 2];
        }
    }
}

/// A tunnel frame in v210's scale, for the bar detector (#69): its
/// constants were measured on limited-range YCbCr, where bars sit at 64. A
/// tunnel's codes are full range, bars at 0 with chroma within a few codes of
/// neutral (Top Gun: Maverick, 2026-10-23), so luma 0-4095 becomes 64-940 and
/// chroma 0-4095 becomes 64-960, keeping 2048 at 512. The thumbnails and cuts
/// keep [`Tunnel`]'s own codes, which the recorded pairs were made with.
pub struct TunnelBars<'a>(pub Tunnel<'a>);

/// A full-range 12-bit code into limited-range 10-bit, `span` codes wide.
const fn limited(code: u16, span: u32) -> u16 {
    (64 + (code as u32 * span + 2047) / 4095) as u16
}

/// [`limited`] for every 12-bit code, worked out once: the detector reads
/// about 3 million samples a frame, and a division each cost 8 ms (#69).
const fn table(span: u32) -> [u16; 4096] {
    let mut t = [0u16; 4096];
    let mut c = 0;
    while c < 4096 {
        t[c] = limited(c as u16, span);
        c += 1;
    }
    t
}

const LUMA_10: [u16; 4096] = table(876);
const CHROMA_10: [u16; 4096] = table(896);

impl TunnelBars<'_> {
    /// Row `y`'s pixels as big-endian words, `width` of them.
    fn words(&self, y: u32) -> impl Iterator<Item = u32> + '_ {
        let t = &self.0;
        let start = y as usize * t.row_bytes;
        t.bytes[start..start + t.width as usize * 4]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| u32::from_be_bytes(*p))
    }

    /// Row `y`'s pixels in pairs, each pair's two words as bytes.
    fn pairs(&self, y: u32) -> &[[u8; 8]] {
        let t = &self.0;
        let start = y as usize * t.row_bytes;
        t.bytes[start..start + t.width as usize * 4]
            .as_chunks::<8>()
            .0
    }
}

/// A word's 12-bit luma and chroma, as [`Tunnel`] unpacks them.
fn unpack(w: u32) -> (usize, usize) {
    let [r, g, b] = [w >> 22, w >> 12, w >> 2].map(|c| (c & 0xFF) as usize);
    (g << 4 | b & 0xF, r << 4 | b >> 4)
}

/// The 12-bit luma above which a sample's v210-scale code is above `thr`.
fn luma_from(thr: u16) -> usize {
    LUMA_10.partition_point(|&c| c <= thr)
}

/// The 12-bit chroma window whose v210-scale codes are within `dc` of
/// neutral, as its first code and width: off is `c - lo > span`, one
/// unsigned compare, as [`chroma_off`] tests it.
fn chroma_window(dc: u32) -> (u32, u32) {
    let lo = CHROMA_10.partition_point(|&c| u32::from(c) < 512u32.saturating_sub(dc));
    let hi = CHROMA_10.partition_point(|&c| u32::from(c) <= 512 + dc);
    (
        lo as u32,
        (hi as u32).wrapping_sub(lo as u32).wrapping_sub(1),
    )
}

impl Samples for TunnelBars<'_> {
    fn width(&self) -> u32 {
        self.0.width
    }

    fn height(&self) -> u32 {
        self.0.height
    }

    fn luma_row(&self, y: u32, out: &mut [u16]) {
        for (o, w) in out.iter_mut().zip(self.words(y)) {
            *o = LUMA_10[unpack(w).0];
        }
    }

    fn chroma_row(&self, y: u32, out: &mut [[u16; 2]]) {
        let mut words = self.words(y);
        for o in out.iter_mut().take(self.0.width as usize / 2) {
            let (Some(a), Some(b)) = (words.next(), words.next()) else {
                break;
            };
            *o = [CHROMA_10[unpack(a).1], CHROMA_10[unpack(b).1]];
        }
    }

    // The counting methods straight from the words, as v210's: the
    // thresholds moved into 12-bit codes once a call, so no sample is
    // rescaled, and each chroma pair tested once for its two pixels.

    fn count_lit(&self, y: u32, thr: u16, dc: u32, _: &mut RowScratch) -> (u32, u8, u32) {
        wide(
            #[inline(always)]
            || {
                let (t, row) = (Codes::new(thr, dc), self.pairs(y));
                let w = 2 * row.len();
                // Each quarter's whole pairs in one loop, with no test of
                // where a pixel is; a pixel whose pair it shares with the
                // next quarter, at an odd edge, goes on its own.
                let pixel = |x: usize| {
                    let p = &row[x / 2];
                    let l = t.luma(word(p, x % 2));
                    (l, t.pair_off(p) & (l ^ 1))
                };
                let (mut n, mut mask, mut chroma_only) = (0u32, 0u8, 0u32);
                for q in 0..4 {
                    let (x0, x1) = (q * w / 4, (q + 1) * w / 4);
                    let (mut l, mut co) = t.pairs(&row[whole(x0, x1)]);
                    for x in edges(x0, x1) {
                        let (pl, pco) = pixel(x);
                        l += pl;
                        co += pco;
                    }
                    // Lit is lit by luma, or by chroma alone.
                    let m = l + co;
                    n += m;
                    chroma_only += co;
                    mask |= u8::from(m > 0) << q;
                }
                (n, mask, chroma_only)
            },
        )
    }

    fn luma_at(&self, y: u32, xs: &[u32], out: &mut [u16], _: &mut RowScratch) {
        let t = &self.0;
        let row = &t.bytes[y as usize * t.row_bytes..];
        for (o, &x) in out.iter_mut().zip(xs) {
            let p = &row[4 * x as usize..4 * x as usize + 4];
            *o = LUMA_10[unpack(u32::from_be_bytes([p[0], p[1], p[2], p[3]])).0];
        }
    }

    fn chroma_flags(&self, y: u32, dc: u32, out: &mut Vec<u8>, _: &mut RowScratch) {
        wide(
            #[inline(always)]
            || {
                let (t, row) = (Codes::new(0, dc), self.pairs(y));
                out.clear();
                out.extend(row.iter().map(|p| t.pair_off(p) as u8));
            },
        )
    }

    fn count_lit_flags(&self, y: u32, thr: u16, flags: &[u8], _: &mut RowScratch) -> (u32, u8) {
        wide(
            #[inline(always)]
            || {
                let (t, row) = (Codes::new(thr, 0), self.pairs(y));
                let w = 2 * row.len();
                let pixel = |x: usize| t.luma(word(&row[x / 2], x % 2)) | u32::from(flags[x / 2]);
                let (mut n, mut mask) = (0u32, 0u8);
                for q in 0..4 {
                    let (x0, x1) = (q * w / 4, (q + 1) * w / 4);
                    let k = whole(x0, x1);
                    let mut m = 0u32;
                    for (p, &f) in row[k.clone()].iter().zip(&flags[k]) {
                        let f = u32::from(f);
                        m += (t.luma(word(p, 0)) | f) + (t.luma(word(p, 1)) | f);
                    }
                    m += edges(x0, x1).map(pixel).sum::<u32>();
                    n += m;
                    mask |= u8::from(m > 0) << q;
                }
                (n, mask)
            },
        )
    }
}

/// The pairs wholly inside `x0..x1`: none for a quarter of no pixels, as a
/// quarter of a narrow row can be.
fn whole(x0: usize, x1: usize) -> std::ops::Range<usize> {
    x0.div_ceil(2)..(x1 / 2).max(x0.div_ceil(2))
}

/// The pixels of `x0..x1` outside its whole pairs: `x0` when odd, the last
/// when `x1` is.
fn edges(x0: usize, x1: usize) -> impl Iterator<Item = usize> {
    let first = (x0 < x1 && x0 % 2 == 1).then_some(x0);
    // Never `x0` twice: with both odd, `x1` is at least `x0 + 2`.
    let last = (x0 < x1 && x1 % 2 == 1).then(|| x1 - 1);
    first.into_iter().chain(last)
}

/// Pixel `i` (0 or 1) of a pair's two big-endian words.
fn word(p: &[u8; 8], i: usize) -> u32 {
    u32::from_be_bytes([p[4 * i], p[4 * i + 1], p[4 * i + 2], p[4 * i + 3]])
}

/// One call's thresholds in 12-bit codes: luma lit from `from`, chroma off
/// outside the window `lo` and `span` wide, as [`chroma_window`] gives it.
#[derive(Clone, Copy)]
struct Codes {
    from: u32,
    lo: u32,
    span: u32,
}

impl Codes {
    fn new(thr: u16, dc: u32) -> Self {
        let (lo, span) = chroma_window(dc);
        Self {
            from: luma_from(thr) as u32,
            lo,
            span,
        }
    }

    // Each field taken from the word where it lies, as [`unpack`] has them,
    // in plain u32 arithmetic: no table and no branch, so the compiler can
    // run a row's pairs several to an instruction.

    fn luma(self, w: u32) -> u32 {
        u32::from(((w >> 8) & 0xFF0 | (w >> 2) & 0xF) >= self.from)
    }

    fn off(self, w: u32) -> bool {
        ((w >> 18) & 0xFF0 | (w >> 6) & 0xF).wrapping_sub(self.lo) > self.span
    }

    fn pair_off(self, p: &[u8; 8]) -> u32 {
        u32::from(self.off(word(p, 0)) | self.off(word(p, 1)))
    }

    /// The pairs' pixels lit by luma, and lit by chroma alone.
    #[inline(always)]
    fn pairs(self, row: &[[u8; 8]]) -> (u32, u32) {
        let (mut l, mut co) = (0u32, 0u32);
        for p in row {
            let (la, lb) = (self.luma(word(p, 0)), self.luma(word(p, 1)));
            l += la + lb;
            co += self.pair_off(p) * (2 - la - lb);
        }
        (l, co)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bars::Bars;

    thread_local! {
        /// Set: [`wide`] runs its loops as built, without AVX2, so a test
        /// can check both.
        pub(super) static PORTABLE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    /// The tables and the row walk give what unpacking each pixel and
    /// rescaling it gives, on every code and on a frame of noise.
    #[test]
    fn tunnel_bars_read_as_each_pixel_rescaled() {
        for c in 0..4096u16 {
            assert_eq!(LUMA_10[c as usize], limited(c, 876));
            assert_eq!(CHROMA_10[c as usize], limited(c, 896));
        }
        let (w, h) = (6u32, 3u32);
        let row_bytes = 4 * w as usize + 8;
        let mut s = 0x1234_5678u32;
        let bytes: Vec<u8> = (0..row_bytes * h as usize)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                s as u8
            })
            .collect();
        let t = TunnelBars(Tunnel::new(&bytes, w, h, row_bytes).unwrap());
        let mut luma = vec![0u16; w as usize];
        let mut chroma = vec![[0u16; 2]; w as usize / 2];
        for y in 0..h {
            t.luma_row(y, &mut luma);
            t.chroma_row(y, &mut chroma);
            for x in 0..w as usize {
                let (l, c) = t.0.pixel(x, y);
                assert_eq!(luma[x], limited(l, 876));
                assert_eq!(chroma[x / 2][x % 2], limited(c, 896));
            }
        }
    }

    /// Black, neutral and the ends of the range land where v210 has them.
    #[test]
    fn tunnel_codes_go_into_v210_scale() {
        assert_eq!(limited(0, 876), 64);
        assert_eq!(limited(4095, 876), 940);
        assert_eq!(limited(0, 896), 64);
        assert_eq!(limited(2048, 896), 512);
        assert_eq!(limited(4095, 896), 960);
        // Dark picture: a 12-bit 100 is about 0.013 nits as PQ, and clears
        // the bar level by more than the detector's lit margin (16).
        assert!(limited(100, 876) > 64 + 16);
    }

    /// The tunnel adapter reads what the packing put there: a frame packed
    /// by hand, as `cinefit_render::tunnel::pack_r210` packs it.
    #[test]
    fn a_tunnel_frame_reads_back_its_luma_and_chroma() {
        let (w, h) = (4u32, 2u32);
        let row_bytes = 4 * w as usize + 8;
        let y12 = |x: u32, y: u32| (100 + 37 * x + 900 * y) as u16;
        let c12 = |x: u32, y: u32| (2048 + 11 * x - 300 * y) as u16;
        let mut bytes = vec![0u8; row_bytes * h as usize];
        for y in 0..h {
            for x in 0..w {
                let (l, c) = (y12(x, y), c12(x, y));
                let (r, g, b) = (c >> 4, l >> 4, (c & 0xF) << 4 | (l & 0xF));
                let word = u32::from(r) << 22 | u32::from(g) << 12 | u32::from(b) << 2;
                let o = y as usize * row_bytes + 4 * x as usize;
                bytes[o..o + 4].copy_from_slice(&word.to_be_bytes());
            }
        }
        let t = Tunnel::new(&bytes, w, h, row_bytes).unwrap();
        let mut luma = vec![0u16; w as usize];
        let mut chroma = vec![[0u16; 2]; w as usize / 2];
        t.luma_row(1, &mut luma);
        t.chroma_row(1, &mut chroma);
        assert_eq!(luma, (0..w).map(|x| y12(x, 1) >> 2).collect::<Vec<_>>());
        assert_eq!(
            chroma,
            [
                [c12(0, 1) >> 2, c12(1, 1) >> 2],
                [c12(2, 1) >> 2, c12(3, 1) >> 2]
            ]
        );
        assert!(Tunnel::new(&bytes, 3, h, row_bytes).is_none(), "odd width");
        assert!(Tunnel::new(&bytes[1..], w, h, row_bytes).is_none(), "short");
    }

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

    /// A pixel by kind, for the counting tests: each kind lands either side
    /// of the thresholds they count at, in each format's own codes.
    #[derive(Clone, Copy)]
    enum Px {
        /// Flat bar: black and neutral.
        Bar,
        /// Luma around black, chroma around neutral: every test goes both
        /// ways.
        Near,
        /// Lit by luma.
        Luma,
        /// Black, lit by chroma alone.
        Chroma,
    }

    struct Rng(u64);

    impl Rng {
        fn below(&mut self, n: u32) -> u32 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 33) as u32 % n
        }
    }

    /// `h` rows of `w` pixels, in each shape the bar scan meets: noise, flat
    /// bar, runs of every kind (some one pixel long), bar with a lit pixel or
    /// two beside a quarter's edge, and bar either side of picture. The first
    /// `top` and last `bottom` rows are bar.
    fn kinds(w: usize, h: usize, top: usize, bottom: usize, r: &mut Rng) -> Vec<Px> {
        let mut px = vec![Px::Bar; w * h];
        for y in top..h.saturating_sub(bottom) {
            let row = &mut px[y * w..(y + 1) * w];
            let runs = |row: &mut [Px], kinds: &[Px], r: &mut Rng| {
                let (mut x, len) = (0, row.len());
                while x < len {
                    let k = kinds[r.below(kinds.len() as u32) as usize];
                    let n = 1 + r.below(12) as usize;
                    row[x..(x + n).min(len)].fill(k);
                    x += n;
                }
            };
            match y % 5 {
                0 => row.fill(Px::Near),
                1 => {}
                2 => runs(row, &[Px::Bar, Px::Near, Px::Luma, Px::Chroma], r),
                3 => {
                    let edge = (1 + r.below(3) as usize) * w / 4;
                    let x = (edge + r.below(5) as usize).saturating_sub(2).min(w - 1);
                    let n = 1 + r.below(2) as usize;
                    row[x..(x + n).min(w)].fill([Px::Luma, Px::Chroma][r.below(2) as usize]);
                }
                _ => runs(&mut row[w / 8..w - w / 8], &[Px::Near, Px::Luma], r),
            }
        }
        px
    }

    /// Bar rows each with one lit pixel, at and either side of every
    /// quarter's edge and the row's ends, lit by luma or by chroma alone.
    fn edge_cases(w: usize) -> Vec<Px> {
        let mut px = Vec::new();
        for e in [0, w / 4, w / 2, 3 * w / 4, w - 1] {
            for x in e.saturating_sub(2)..(e + 3).min(w) {
                for k in [Px::Luma, Px::Chroma] {
                    let mut row = vec![Px::Bar; w];
                    row[x] = k;
                    px.extend(row);
                }
            }
        }
        px
    }

    /// Pixels as a tunnel frame, 12-bit full range.
    fn tunnel(px: &[Px], w: usize, r: &mut Rng) -> (Vec<u8>, usize) {
        let row_bytes = 4 * w + 12;
        let mut bytes = vec![0u8; row_bytes * (px.len() / w)];
        for (i, &k) in px.iter().enumerate() {
            let (l, c) = match k {
                Px::Bar => (r.below(4), 2046 + r.below(5)),
                Px::Near => (r.below(200), 1950 + r.below(200)),
                Px::Luma => (600 + r.below(3496), 1950 + r.below(200)),
                Px::Chroma => (r.below(4), [r.below(1000), 3096 + r.below(1000)][i % 2]),
            };
            let (red, g, b) = (c >> 4, l >> 4, (c & 0xF) << 4 | (l & 0xF));
            let word = red << 22 | g << 12 | b << 2;
            let o = i / w * row_bytes + 4 * (i % w);
            bytes[o..o + 4].copy_from_slice(&word.to_be_bytes());
        }
        (bytes, row_bytes)
    }

    /// Pixels as a v210 frame; a last group's pixels past `w` are bar.
    fn v210(px: &[Px], w: usize, r: &mut Rng) -> (Vec<u8>, usize) {
        let groups = w.div_ceil(6);
        let row_bytes = groups * 16 + 16;
        let mut bytes = vec![0u8; row_bytes * (px.len() / w)];
        for y in 0..px.len() / w {
            for g in 0..groups {
                // Word layout: Cb Y Cr | Y Cb Y | Cr Y Cb | Y Cr Y.
                let mut words = [0u32; 4];
                for i in 0..6 {
                    let x = 6 * g + i;
                    let k = if x < w { px[y * w + x] } else { Px::Bar };
                    let (l, c) = match k {
                        Px::Bar => (64, 511 + r.below(3)),
                        Px::Near => (40 + r.below(80), 470 + r.below(85)),
                        Px::Luma => (300 + r.below(640), 470 + r.below(85)),
                        Px::Chroma => (64, [64 + r.below(236), 700 + r.below(260)][i % 2]),
                    };
                    // A pixel carries its pair's Cb when even, its Cr when odd.
                    let (cw, cs) = [CB_AT, CR_AT][i % 2][i / 2];
                    words[Y_AT[i].0] |= l << (10 * Y_AT[i].1);
                    words[cw] |= c << (10 * cs);
                }
                for (k, wd) in words.iter().enumerate() {
                    let o = y * row_bytes + g * 16 + 4 * k;
                    bytes[o..o + 4].copy_from_slice(&wd.to_le_bytes());
                }
            }
        }
        (bytes, row_bytes)
    }

    /// Every counting method of `fast` gives what the defaults give over
    /// `plain`, on every row, at thresholds either side of the frames' codes,
    /// with AVX2 and without.
    fn same_counts(fast: &dyn Samples, plain: &dyn Samples, what: &str) {
        let (w, h) = (fast.width(), fast.height());
        let xs: Vec<u32> = (0..w).step_by(7).chain([w - 1]).collect();
        let (mut s, mut a, mut b) = (RowScratch::default(), Vec::new(), Vec::new());
        for portable in [false, true] {
            PORTABLE.set(portable);
            for y in 0..h {
                for (thr, dc) in [(64, 16), (80, 16), (100, 30), (63, 0), (70, 4)] {
                    assert_eq!(
                        fast.count_lit(y, thr, dc, &mut s),
                        plain.count_lit(y, thr, dc, &mut s),
                        "count_lit {what} w {w} y {y} thr {thr} dc {dc} portable {portable}"
                    );
                    fast.chroma_flags(y, dc, &mut a, &mut s);
                    plain.chroma_flags(y, dc, &mut b, &mut s);
                    assert_eq!(a, b, "chroma_flags {what} w {w} y {y} dc {dc}");
                    let (mut pa, mut pb) = (vec![0u16; xs.len()], vec![0u16; xs.len()]);
                    fast.luma_at(y, &xs, &mut pa, &mut s);
                    plain.luma_at(y, &xs, &mut pb, &mut s);
                    assert_eq!(pa, pb, "luma_at {what} w {w} y {y}");
                    // The flags as found, and with runs cut out, as the bar
                    // scan passes them.
                    for cut in [false, true] {
                        let flags: Vec<u8> = a
                            .iter()
                            .enumerate()
                            .map(|(i, &f)| f & u8::from(!cut || i % 3 != 1))
                            .collect();
                        assert_eq!(
                            fast.count_lit_flags(y, thr, &flags, &mut s),
                            plain.count_lit_flags(y, thr, &flags, &mut s),
                            "count_lit_flags {what} w {w} y {y} thr {thr} cut {cut} \
                             portable {portable}"
                        );
                    }
                }
            }
        }
        PORTABLE.set(false);
    }

    /// A tunnel frame seen only through the four required methods.
    struct PlainTunnel<'a>(TunnelBars<'a>);

    impl Samples for PlainTunnel<'_> {
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

    /// Frames to count over: rows of every shape, and the edge cases.
    fn frames(w: usize, seed: u64) -> impl Iterator<Item = Vec<Px>> {
        let mut r = Rng(seed);
        [kinds(w, 20, 0, 0, &mut r), edge_cases(w)].into_iter()
    }

    #[test]
    fn tunnel_bars_count_in_place_exactly_as_the_defaults_do() {
        // Widths whose quarters end mid-pair (1002, 70, 14, 6, 2) and do not.
        for (w, seed) in [3840, 1920, 1002, 70, 66, 64, 14, 8, 6, 4, 2]
            .into_iter()
            .zip(1..)
        {
            for px in frames(w, seed) {
                let (bytes, row_bytes) = tunnel(&px, w, &mut Rng(seed));
                let (w, h) = (w as u32, (px.len() / w) as u32);
                let fast = TunnelBars(Tunnel::new(&bytes, w, h, row_bytes).expect("fits"));
                let plain = PlainTunnel(TunnelBars(
                    Tunnel::new(&bytes, w, h, row_bytes).expect("fits"),
                ));
                same_counts(&fast, &plain, "tunnel");
            }
        }
    }

    #[test]
    fn v210_counts_in_place_exactly_as_the_defaults_do() {
        // Widths that cut quarters mid-group, end mid-group, and are odd.
        let widths = [3840, 1920, 1918, 1001, 70, 64, 13, 7, 6, 5, 3, 1];
        for (w, seed) in widths.into_iter().zip(1..) {
            for px in frames(w, seed) {
                let (bytes, row_bytes) = v210(&px, w, &mut Rng(seed));
                let (w, h) = (w as u32, (px.len() / w) as u32);
                let fast = V210::new(&bytes, w, h, row_bytes).expect("fits");
                let plain = Plain(V210::new(&bytes, w, h, row_bytes).expect("fits"));
                same_counts(&fast, &plain, "v210");
            }
        }
    }

    /// The whole bar reading, run rule and continuation probes included, is
    /// the same from the in-place methods as from the defaults: frames with
    /// bars of several heights over rows of every shape.
    #[test]
    fn bars_read_the_same_in_place_as_from_the_defaults() {
        for (w, h, top, bottom) in [
            (3840, 216, 27, 27),
            (1920, 120, 0, 0),
            (1002, 160, 3, 40),
            (70, 96, 1, 9),
            (64, 64, 12, 13),
        ] {
            let px = kinds(w, h, top, bottom, &mut Rng(w as u64));
            let read = |fast: &dyn Samples, plain: &dyn Samples, what: &str| {
                for portable in [false, true] {
                    PORTABLE.set(portable);
                    assert_eq!(
                        Bars::default().observe(fast),
                        Bars::default().observe(plain),
                        "{what} w {w} h {h} bars {top} {bottom} portable {portable}"
                    );
                }
                PORTABLE.set(false);
            };
            let (wu, hu) = (w as u32, h as u32);
            let (bytes, row_bytes) = tunnel(&px, w, &mut Rng(1));
            let t = || TunnelBars(Tunnel::new(&bytes, wu, hu, row_bytes).expect("fits"));
            read(&t(), &PlainTunnel(t()), "tunnel");
            let (bytes, row_bytes) = v210(&px, w, &mut Rng(2));
            let v = || V210::new(&bytes, wu, hu, row_bytes).expect("fits");
            read(&v(), &Plain(v()), "v210");
        }
    }
}
