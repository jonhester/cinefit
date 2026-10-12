//! `v210`, the capture card's 10-bit YCbCr 4:2:2: a CPU reference for the GPU
//! unpack, and the packer the checks build their frames with.
//!
//! Six pixels pack into a 16-byte group of four little-endian words, three
//! 10-bit components to a word, the top two bits unused:
//!
//! ```text
//! word 0:  bits  0-9 Cb0   10-19 Y0    20-29 Cr0
//! word 1:  bits  0-9 Y1    10-19 Cb1   20-29 Y2
//! word 2:  bits  0-9 Cr1   10-19 Y3    20-29 Cb2
//! word 3:  bits  0-9 Y4    10-19 Cr2   20-29 Y5
//! ```
//!
//! A row is whole groups, so a width that is not a multiple of six ends in a
//! partly used group; the card pads each row further, which is why the stride
//! always comes from the card.

const GROUP_PIXELS: usize = 6;
const GROUP_BYTES: usize = 16;

/// The bytes a row's groups take: the smallest stride a `width` allows. The
/// card's own stride is larger; never use this in its place.
pub fn min_row_bytes(width: usize) -> usize {
    width.div_ceil(GROUP_PIXELS) * GROUP_BYTES
}

/// A 4:2:2 picture as planes, rows packed: one luma sample per pixel, and
/// one Cb, Cr pair per two pixels, interleaved. An odd width's last pixel has
/// a pair of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planes {
    pub width: usize,
    pub height: usize,
    pub luma: Vec<u16>,
    /// Cb, Cr, Cb, Cr, ...: [`chroma_width`](Self::chroma_width) pairs a row.
    pub chroma: Vec<u16>,
}

impl Planes {
    pub fn chroma_width(&self) -> usize {
        self.width.div_ceil(2)
    }
}

/// Unpacks a `v210` frame into 10-bit codes. `None` if the stride does not
/// hold the width's groups or `bytes` is shorter than `height` rows.
pub fn unpack(bytes: &[u8], width: usize, height: usize, row_bytes: usize) -> Option<Planes> {
    if row_bytes < min_row_bytes(width) || bytes.len() < row_bytes.checked_mul(height)? {
        return None;
    }
    let cw = width.div_ceil(2);
    let mut luma = Vec::with_capacity(width * height);
    let mut chroma = Vec::with_capacity(cw * 2 * height);
    for row in bytes.chunks(row_bytes).take(height) {
        let (groups, _) = row[..min_row_bytes(width)].as_chunks::<GROUP_BYTES>();
        let row_luma = luma.len();
        let row_chroma = chroma.len();
        for g in groups {
            let w: [u32; 4] = std::array::from_fn(|i| {
                u32::from_le_bytes(g[4 * i..4 * i + 4].try_into().unwrap())
            });
            let c = |word: usize, slot: u32| (w[word] >> (10 * slot) & 0x3FF) as u16;
            luma.extend([c(0, 1), c(1, 0), c(1, 2), c(2, 1), c(3, 0), c(3, 2)]);
            chroma.extend([c(0, 0), c(0, 2), c(1, 1), c(2, 0), c(2, 2), c(3, 1)]);
        }
        // Drop the last group's unused pixels.
        luma.truncate(row_luma + width);
        chroma.truncate(row_chroma + cw * 2);
    }
    Some(Planes {
        width,
        height,
        luma,
        chroma,
    })
}

/// Packs planes of 10-bit codes into `v210` rows of `row_bytes`, padding with
/// zeros. `None` if the stride is too small or not whole words, or the planes
/// do not match their size.
pub fn pack(p: &Planes, row_bytes: usize) -> Option<Vec<u8>> {
    let cw = p.chroma_width();
    if row_bytes < min_row_bytes(p.width)
        || !row_bytes.is_multiple_of(4)
        || p.luma.len() != p.width * p.height
        || p.chroma.len() != cw * 2 * p.height
    {
        return None;
    }
    let mut out = vec![0u8; row_bytes * p.height];
    for (y, row) in out.chunks_mut(row_bytes).enumerate() {
        let luma = |x: usize| {
            u32::from(if x < p.width {
                p.luma[y * p.width + x]
            } else {
                0
            })
        };
        let chroma = |x: usize, c: usize| {
            u32::from(if x < cw {
                p.chroma[(y * cw + x) * 2 + c]
            } else {
                0
            })
        };
        for (g, bytes) in row[..min_row_bytes(p.width)]
            .as_chunks_mut::<GROUP_BYTES>()
            .0
            .iter_mut()
            .enumerate()
        {
            let (x, cx) = (g * GROUP_PIXELS, g * 3);
            let word = |a: u32, b: u32, c: u32| (a & 0x3FF) | (b & 0x3FF) << 10 | (c & 0x3FF) << 20;
            let words = [
                word(chroma(cx, 0), luma(x), chroma(cx, 1)),
                word(luma(x + 1), chroma(cx + 1, 0), luma(x + 2)),
                word(chroma(cx + 1, 1), luma(x + 3), chroma(cx + 2, 0)),
                word(luma(x + 4), chroma(cx + 2, 1), luma(x + 5)),
            ];
            for (i, w) in words.iter().enumerate() {
                bytes[4 * i..4 * i + 4].copy_from_slice(&w.to_le_bytes());
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every component distinct, so a swap anywhere shows.
    fn numbered(width: usize, height: usize) -> Planes {
        let cw = width.div_ceil(2);
        Planes {
            width,
            height,
            luma: (0..width * height).map(|i| (i % 1024) as u16).collect(),
            chroma: (0..cw * 2 * height)
                .map(|i| (1023 - i % 1024) as u16)
                .collect(),
        }
    }

    #[test]
    fn unpacks_the_documented_layout() {
        // One group: Y0..Y5 = 10..15, Cb0..2 = 20..22, Cr0..2 = 30..32.
        let w = |a: u32, b: u32, c: u32| (a | b << 10 | c << 20).to_le_bytes();
        let group = [w(20, 10, 30), w(11, 21, 12), w(31, 13, 22), w(14, 32, 15)].concat();
        let p = unpack(&group, 6, 1, 16).unwrap();
        assert_eq!(p.luma, [10, 11, 12, 13, 14, 15]);
        assert_eq!(p.chroma, [20, 30, 21, 31, 22, 32]);
    }

    #[test]
    fn round_trips_at_widths_that_end_in_a_partial_group() {
        for width in [1, 2, 5, 6, 7, 1918, 1920, 3838, 3840] {
            let p = numbered(width, 3);
            // Padded past the groups, as the card pads.
            let row_bytes = min_row_bytes(width).next_multiple_of(128) + 64;
            let bytes = pack(&p, row_bytes).unwrap();
            assert_eq!(
                unpack(&bytes, width, 3, row_bytes),
                Some(p),
                "width {width}"
            );
        }
    }

    #[test]
    fn ignores_the_top_two_bits_of_each_word() {
        let mut bytes = pack(&numbered(6, 1), 16).unwrap();
        for word in bytes.as_chunks_mut::<4>().0 {
            word[3] |= 0xC0;
        }
        assert_eq!(unpack(&bytes, 6, 1, 16), Some(numbered(6, 1)));
    }

    #[test]
    fn rejects_a_stride_that_cannot_hold_the_width() {
        assert_eq!(min_row_bytes(7), 32);
        assert!(unpack(&[0; 64], 7, 2, 16).is_none());
        assert!(unpack(&[0; 63], 7, 2, 32).is_none());
        assert!(pack(&numbered(7, 1), 16).is_none());
    }
}
