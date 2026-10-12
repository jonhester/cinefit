//! Scanout pixel formats: which to allocate, and how to read one back.
//!
//! 10 bits a channel where the display plane takes them: a PQ picture in 8
//! bits bands visibly, and the link carries 10 (`ModeTiming::hdmi_bpc`). The
//! C++ version made the same choice.

/// DRM fourccs, deepest first: the first one the plane accepts is used.
pub const PREFERRED: [u32; 3] = [XRGB2101010, XBGR2101010, XRGB8888];

pub const XRGB8888: u32 = fourcc(b"XR24");
pub const XRGB2101010: u32 = fourcc(b"XR30");
pub const XBGR2101010: u32 = fourcc(b"XB30");

const fn fourcc(c: &[u8; 4]) -> u32 {
    u32::from_le_bytes(*c)
}

/// Reads one pixel of a scanout buffer as R, G, B in 0-1, or `None` for a
/// format not in [`PREFERRED`]. All three are one little-endian word a pixel.
pub fn decode(fourcc: u32, px: [u8; 4]) -> Option<[f64; 3]> {
    let w = u32::from_le_bytes(px);
    let ten = |shift: u32| f64::from(w >> shift & 0x3FF) / 1023.0;
    let eight = |shift: u32| f64::from(w >> shift & 0xFF) / 255.0;
    match fourcc {
        XRGB8888 => Some([eight(16), eight(8), eight(0)]),
        XRGB2101010 => Some([ten(20), ten(10), ten(0)]),
        XBGR2101010 => Some([ten(0), ten(10), ten(20)]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fourccs_match_drm() {
        // drm_fourcc.h: XR24 0x34325258, XR30 0x30335258, XB30 0x30334258.
        assert_eq!(XRGB8888, 0x3432_5258);
        assert_eq!(XRGB2101010, 0x3033_5258);
        assert_eq!(XBGR2101010, 0x3033_4258);
    }

    #[test]
    fn decodes_each_layout() {
        // Red full, green half, blue zero.
        let rgb10 = (1023u32 << 20 | 512 << 10).to_le_bytes();
        let [r, g, b] = decode(XRGB2101010, rgb10).unwrap();
        assert_eq!((r, b), (1.0, 0.0));
        assert!((g - 512.0 / 1023.0).abs() < 1e-9);
        let bgr10 = (1023u32 | 512 << 10).to_le_bytes();
        assert_eq!(decode(XBGR2101010, bgr10), decode(XRGB2101010, rgb10));
        let rgb8 = [0x00, 0x80, 0xFF, 0x00]; // B, G, R, X in memory
        let [r, _, b] = decode(XRGB8888, rgb8).unwrap();
        assert_eq!((r, b), (1.0, 0.0));
        assert!(decode(0, rgb8).is_none());
    }
}
