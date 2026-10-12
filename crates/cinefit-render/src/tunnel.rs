//! Tunneled Dolby Vision as captured: the source's 12-bit YCbCr 4:2:2
//! (ICtCp) packed into 8-bit RGB, which the card delivers as `r210` with
//! each 8-bit code shifted left by two. A CPU reference for the GPU unpack,
//! and the packer the checks build their frames with.
//!
//! Each pixel carries one luma and one chroma sample, packed as HDMI packs
//! 12-bit 4:2:2:
//!
//! ```text
//! G = Y[11:4]    R = C[11:4]    B = C[3:0] << 4 | Y[3:0]
//! ```
//!
//! and C is Cb on even pixels, Cr on odd ones (ETSI GS CCM 001 clause
//! 6.4.1), so each Cb, Cr pair sits on its even pixel: co-sited. The packing
//! was measured on captured frames, where it alone gives smooth luma and
//! chroma centred near 2048. The metadata rides in the lowest bit of every
//! chroma sample, so chroma carries at most a code of noise in 4096.

use crate::v210::Planes;

/// The 8-bit R, G, B of pixel `px`, from `r210`.
fn rgb8(px: [u8; 4]) -> [u16; 3] {
    let w = u32::from_be_bytes(px);
    [w >> 22, w >> 12, w >> 2].map(|c| (c & 0xFF) as u16)
}

/// Unpacks a tunnel frame into 12-bit planes: luma, and Cb, Cr pairs. `None`
/// for an odd width (4:2:2 pairs pixels), a stride that does not hold the
/// width, or `bytes` shorter than `height` rows.
pub fn unpack_r210(bytes: &[u8], width: usize, height: usize, row_bytes: usize) -> Option<Planes> {
    if !width.is_multiple_of(2)
        || row_bytes == 0
        || row_bytes < width.checked_mul(4)?
        || bytes.len() < row_bytes.checked_mul(height)?
    {
        return None;
    }
    let mut luma = Vec::with_capacity(width * height);
    let mut chroma = Vec::with_capacity(width * height);
    for row in bytes.chunks(row_bytes).take(height) {
        for px in row[..width * 4].as_chunks::<4>().0 {
            let [r, g, b] = rgb8(*px);
            luma.push(g << 4 | b & 0xF);
            chroma.push(r << 4 | b >> 4);
        }
    }
    Some(Planes {
        width,
        height,
        luma,
        chroma,
    })
}

/// Packs 12-bit planes as a tunnel frame in `r210`, rows of `row_bytes`,
/// padding with zeros. `None` if the width is odd, the stride too small or
/// the planes do not match their size.
pub fn pack_r210(p: &Planes, row_bytes: usize) -> Option<Vec<u8>> {
    if !p.width.is_multiple_of(2)
        || row_bytes < p.width * 4
        || p.luma.len() != p.width * p.height
        || p.chroma.len() != p.width * p.height
    {
        return None;
    }
    let mut out = vec![0u8; row_bytes * p.height];
    for (y, row) in out.chunks_mut(row_bytes).enumerate() {
        for (x, px) in row[..p.width * 4]
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .enumerate()
        {
            let (l, c) = (
                u32::from(p.luma[y * p.width + x]),
                u32::from(p.chroma[y * p.width + x]),
            );
            let [r, g, b] = [c >> 4 & 0xFF, l >> 4 & 0xFF, (c & 0xF) << 4 | l & 0xF];
            *px = (r << 22 | g << 12 | b << 2).to_be_bytes();
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unpacks_the_hdmi_12_bit_layout() {
        // Pixel 0: Y 0xABC, Cb 0x123. Pixel 1: Y 0x456, Cr 0x789.
        let px = |r: u32, g: u32, b: u32| (r << 22 | g << 12 | b << 2).to_be_bytes();
        let bytes = [px(0x12, 0xAB, 0x3C), px(0x78, 0x45, 0x96)].concat();
        let p = unpack_r210(&bytes, 2, 1, 8).unwrap();
        assert_eq!(p.luma, [0xABC, 0x456]);
        assert_eq!(p.chroma, [0x123, 0x789]);
    }

    #[test]
    fn round_trips_with_a_padded_stride() {
        let (w, h) = (6, 3);
        let p = Planes {
            width: w,
            height: h,
            luma: (0..w * h).map(|i| (i * 677 % 4096) as u16).collect(),
            chroma: (0..w * h).map(|i| (4095 - i * 331 % 4096) as u16).collect(),
        };
        let bytes = pack_r210(&p, w * 4 + 16).unwrap();
        assert_eq!(unpack_r210(&bytes, w, h, w * 4 + 16), Some(p));
    }

    #[test]
    fn refuses_an_odd_width_or_a_short_stride() {
        assert!(unpack_r210(&[0; 12], 3, 1, 12).is_none());
        assert!(unpack_r210(&[0; 16], 2, 2, 4).is_none());
        assert!(unpack_r210(&[], 0, 1, 0).is_none());
    }
}
