//! Captured RGB: a CPU reference for the GPU unpack of the two formats the
//! card delivers RGB in, and the packer the checks build their frames with.
//!
//! - `r210`: one big-endian 32-bit word a pixel, 2 unused bits then 10-bit
//!   red, green and blue. The SDK documents it as SMPTE levels (64-940), but
//!   the card was measured passing 8-bit RGB through as `code << 2`, 0 to
//!   1020, untouched: which range the codes are in is the source's choice.
//! - 8-bit ARGB: bytes A, R, G, B.

/// The capture format of an RGB frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    R210,
    Argb8,
}

impl Format {
    /// Bits per component.
    pub fn depth(self) -> u8 {
        match self {
            Format::R210 => 10,
            Format::Argb8 => 8,
        }
    }
}

/// An RGB picture: R, G, B codes per pixel, rows packed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgb {
    pub width: usize,
    pub height: usize,
    pub format: Format,
    pub rgb: Vec<u16>,
}

/// Unpacks a frame into codes. `None` if the stride does not hold the width
/// or `bytes` is shorter than `height` rows.
pub fn unpack(
    bytes: &[u8],
    width: usize,
    height: usize,
    row_bytes: usize,
    format: Format,
) -> Option<Rgb> {
    // A zero stride passes the width check for width 0, and chunks(0) panics.
    if row_bytes == 0
        || row_bytes < width.checked_mul(4)?
        || bytes.len() < row_bytes.checked_mul(height)?
    {
        return None;
    }
    let mut rgb = Vec::with_capacity(width * height * 3);
    for row in bytes.chunks(row_bytes).take(height) {
        for px in row[..width * 4].as_chunks::<4>().0 {
            match format {
                Format::R210 => {
                    let w = u32::from_be_bytes(*px);
                    rgb.extend([w >> 20, w >> 10, w].map(|c| (c & 0x3FF) as u16));
                }
                Format::Argb8 => rgb.extend([px[1], px[2], px[3]].map(u16::from)),
            }
        }
    }
    Some(Rgb {
        width,
        height,
        format,
        rgb,
    })
}

/// Packs codes into rows of `row_bytes`, padding with zeros; ARGB8's alpha
/// is opaque. `None` if the stride is too small or the codes do not match
/// the size.
pub fn pack(p: &Rgb, row_bytes: usize) -> Option<Vec<u8>> {
    if row_bytes == 0 || row_bytes < p.width * 4 || p.rgb.len() != p.width * p.height * 3 {
        return None;
    }
    let mut out = vec![0u8; row_bytes * p.height];
    for (row, codes) in out.chunks_mut(row_bytes).zip(p.rgb.chunks(p.width * 3)) {
        for (px, c) in row[..p.width * 4]
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(codes.as_chunks::<3>().0)
        {
            *px = match p.format {
                Format::R210 => {
                    let [r, g, b] = c.map(|c| u32::from(c) & 0x3FF);
                    (r << 20 | g << 10 | b).to_be_bytes()
                }
                Format::Argb8 => [0xFF, c[0] as u8, c[1] as u8, c[2] as u8],
            };
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numbered(width: usize, height: usize, format: Format) -> Rgb {
        let max = (1usize << format.depth()) - 1;
        Rgb {
            width,
            height,
            format,
            rgb: (0..width * height * 3)
                .map(|i| (i * 7 % (max + 1)) as u16)
                .collect(),
        }
    }

    #[test]
    fn r210_is_big_endian_red_first() {
        // R 0x3FF, G 0x001, B 0x200.
        let word = (0x3FFu32 << 20 | 1 << 10 | 0x200).to_be_bytes();
        let p = unpack(&word, 1, 1, 4, Format::R210).unwrap();
        assert_eq!(p.rgb, [0x3FF, 1, 0x200]);
    }

    #[test]
    fn r210_ignores_the_top_two_bits() {
        let mut bytes = pack(&numbered(5, 2, Format::R210), 32).unwrap();
        for px in bytes.as_chunks_mut::<4>().0 {
            px[0] |= 0xC0;
        }
        assert_eq!(
            unpack(&bytes, 5, 2, 32, Format::R210),
            Some(numbered(5, 2, Format::R210))
        );
    }

    #[test]
    fn argb8_skips_alpha() {
        let p = unpack(&[0x12, 1, 2, 3], 1, 1, 4, Format::Argb8).unwrap();
        assert_eq!(p.rgb, [1, 2, 3]);
    }

    #[test]
    fn round_trips_with_a_padded_stride() {
        for format in [Format::R210, Format::Argb8] {
            for width in [1, 7, 1920] {
                let p = numbered(width, 3, format);
                let row_bytes = width * 4 + 64;
                let bytes = pack(&p, row_bytes).unwrap();
                assert_eq!(unpack(&bytes, width, 3, row_bytes, format), Some(p));
            }
        }
    }

    #[test]
    fn rejects_a_stride_that_cannot_hold_the_width() {
        assert!(unpack(&[0; 16], 2, 2, 4, Format::R210).is_none());
        assert!(unpack(&[], 0, 1, 0, Format::Argb8).is_none());
        assert!(pack(&numbered(2, 1, Format::R210), 4).is_none());
    }
}
