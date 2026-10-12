//! Tunneled (TV-led) Dolby Vision over HDMI: the display-management metadata a
//! source hides in the first pixels of every frame, as specified in ETSI GS
//! CCM 001 clause 6.
//!
//! The source packs a 12-bit YCbCr 4:2:2 picture into 8-bit RGB, and each
//! pixel's 24 bits XOR to one metadata bit (the spec's parity scrambling,
//! undone). Those bits form 128-byte packets, each sent three times and each
//! ending in an MPEG-2 CRC-32. This crate turns pixels into packets
//! ([`packet`]) and packets into metadata ([`metadata`]); it knows nothing
//! about capture cards.
//!
//! Only the pixels identify a tunnel. Captured on a DeckLink Mini Recorder 4K,
//! a tunnel frame is signalled as RGB 4:4:4, BT.2020, SDR transfer function,
//! with no HDR metadata and no Dolby Vision flag: indistinguishable from an
//! ordinary SDR BT.2020 RGB signal, and from what some tools take as the sign
//! of player-led (LLDV) Dolby Vision. A packet whose CRC passes is the only
//! reliable test, which is why [`decode`] returning [`Error::NoPacket`] for
//! packet 0 doubles as "not a tunnel".

pub mod metadata;
pub mod packet;

pub use metadata::{DmMetadata, Level1, Level2, Level4, Level5};
pub use packet::{Error, Header, Tunnel};

/// Decodes the frame's Dolby Vision metadata from its pixels.
///
/// `parity(i)` is metadata bit `i`: the XOR of all 24 bits of pixel `i` in
/// raster order, or `None` past the end of the frame. [`r210_parity`] and
/// [`argb8_parity`] build it for the two capture formats.
///
/// # Errors
///
/// [`Error::NoPacket`] when no copy of a packet passes its CRC, which is what
/// a frame without the tunnel looks like; the other variants for packets that
/// pass their CRC but do not fit together.
pub fn decode(parity: impl Fn(usize) -> Option<bool>) -> Result<Tunnel, Error> {
    packet::decode(&parity)
}

/// A 12-bit PQ code (0-4095), as the metadata uses, in cd/m² (SMPTE ST 2084).
pub fn pq_to_nits(code: u16) -> f64 {
    const M1: f64 = 2610.0 / 16384.0;
    const M2: f64 = 2523.0 / 4096.0 * 128.0;
    const C1: f64 = 3424.0 / 4096.0;
    const C2: f64 = 2413.0 / 4096.0 * 32.0;
    const C3: f64 = 2392.0 / 4096.0 * 32.0;
    let e = (f64::from(code) / 4095.0).powf(1.0 / M2);
    10000.0 * ((e - C1).max(0.0) / (C2 - C3 * e)).powf(1.0 / M1)
}

/// Bit `i` of a frame captured as `r210`, where each pixel is a big-endian
/// word of 2 unused bits and 10-bit red, green and blue. The tunnel's 8-bit
/// values arrive padded to 10 bits, so the top 8 bits of each are the data.
pub fn r210_parity(bytes: &[u8], width: usize, row_bytes: usize) -> impl Fn(usize) -> Option<bool> {
    move |i| {
        let (y, x) = (i / width, i % width);
        let o = y.checked_mul(row_bytes)?.checked_add(x * 4)?;
        let w = u32::from_be_bytes(bytes.get(o..o + 4)?.try_into().ok()?);
        let rgb = (w >> 22) ^ (w >> 12) ^ (w >> 2);
        Some((rgb & 0xFF).count_ones() % 2 == 1)
    }
}

/// Bit `i` of a frame captured as 8-bit ARGB (bytes A, R, G, B per pixel).
pub fn argb8_parity(
    bytes: &[u8],
    width: usize,
    row_bytes: usize,
) -> impl Fn(usize) -> Option<bool> {
    move |i| {
        let (y, x) = (i / width, i % width);
        let o = y.checked_mul(row_bytes)?.checked_add(x * 4)?;
        let px = bytes.get(o..o + 4)?;
        Some((px[1] ^ px[2] ^ px[3]).count_ones() % 2 == 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r210(r: u32, g: u32, b: u32) -> [u8; 4] {
        (r << 20 | g << 10 | b).to_be_bytes()
    }

    #[test]
    fn r210_parity_uses_the_top_8_bits_of_each_component() {
        // One set bit in the top 8 of red: odd. The low 2 bits do not count.
        let row: Vec<u8> = [r210(1 << 2, 0, 0), r210(0b11, 0b11, 0b11), r210(4, 4, 0)].concat();
        let bit = r210_parity(&row, 3, 12);
        assert_eq!(
            (bit(0), bit(1), bit(2), bit(3)),
            (Some(true), Some(false), Some(false), None)
        );
    }

    #[test]
    fn parity_follows_rows_by_stride_not_width() {
        // Two pixels wide, with 4 bytes of padding per row.
        let rows: Vec<u8> = [r210(0, 0, 0), r210(0, 0, 0), [0xFF; 4], r210(4, 0, 0)].concat();
        let bit = r210_parity(&rows, 2, 12);
        assert_eq!(bit(2), Some(true)); // row 1, pixel 0: past the padding
    }

    #[test]
    fn argb8_parity_ignores_alpha() {
        let px = [0xFF, 0b1, 0b10, 0b100, 0x00, 0b1, 0b1, 0b0];
        let bit = argb8_parity(&px, 2, 8);
        assert_eq!((bit(0), bit(1)), (Some(true), Some(false)));
    }

    #[test]
    fn pq_codes_convert_to_nits() {
        let near = |code, nits: f64| (pq_to_nits(code) - nits).abs() < nits * 0.002 + 0.001;
        assert!(near(0, 0.0));
        assert!(near(2081, 100.1)); // the 100-nit L2 target a UB820 sends
        assert!(near(3079, 1000.6));
        assert!(near(4095, 10000.0));
    }
}
