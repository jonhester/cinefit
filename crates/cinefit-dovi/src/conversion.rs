//! The colour conversion a frame's metadata describes, normalised for a
//! renderer: YCC (here ICtCp) to non-linear LMS, then a linear LMS matrix.
//!
//! Every frame states its own conversion, and with it its range: luma gain
//! 1.0 and offset 0 is full range, the spec's default matrix (luma 9575 /
//! 8192, the 4095 / 3504 stretch) limited. Nothing is inferred from pixels.

use crate::DmMetadata;

/// The conversion, as real numbers. Rows are outputs: `ycc_to_rgb[c][i]`
/// is input `i`'s weight in output `c`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Conversion {
    /// YCC to non-linear RGB (LMS for ICtCp), applied to the signal
    /// normalised to 0-1 after subtracting `offset`.
    pub ycc_to_rgb: [[f64; 3]; 3],
    pub offset: [f64; 3],
    /// Linear RGB to LMS as sent; libplacebo's `pl_dovi_metadata.linear`.
    pub rgb_to_lms: [[f64; 3]; 3],
}

fn matrix(m: &[i16; 9], scale: f64) -> [[f64; 3]; 3] {
    std::array::from_fn(|c| std::array::from_fn(|i| f64::from(m[3 * c + i]) / scale))
}

impl DmMetadata {
    /// The conversion, or `None` when the offsets fit no known scale.
    ///
    /// The matrices are fixed point: YCC to RGB over 8192, RGB to LMS over
    /// 16384. The offsets' scale is not one thing. The UB820 sends chroma
    /// offsets of 2^27, which centre chroma only over 2^28 (FFmpeg's and
    /// libplacebo's convention for the same field in a disc's RPU); the
    /// spec's defaults (2^29 chroma, 2^26 luma) make sense only over 2^30.
    /// Chroma is centred at 0.5 in either range, so the chroma offsets say
    /// which scale a frame uses, and anything else is not trusted.
    pub fn conversion(&self) -> Option<Conversion> {
        let [y, cb, cr] = self.ycc_to_rgb_offset;
        if cb != cr {
            return None;
        }
        let scale = match cb {
            0x0800_0000 => f64::from(1u32 << 28),
            0x2000_0000 => f64::from(1u32 << 30),
            _ => return None,
        };
        Some(Conversion {
            ycc_to_rgb: matrix(&self.ycc_to_rgb, 8192.0),
            offset: [y, cb, cr].map(|o| f64::from(o) / scale),
            rgb_to_lms: matrix(&self.rgb_to_lms, 16384.0),
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::DmMetadata;
    use crate::metadata::tests::structure;

    fn with_offsets(offsets: [u32; 3]) -> DmMetadata {
        let mut s = structure(&[]);
        for (j, o) in offsets.iter().enumerate() {
            s[20 + 4 * j..24 + 4 * j].copy_from_slice(&o.to_be_bytes());
        }
        DmMetadata::parse(&s).expect("parses")
    }

    #[test]
    fn a_ub820_frame_is_full_range_with_chroma_centred() {
        let c = with_offsets([0, 0x0800_0000, 0x0800_0000])
            .conversion()
            .expect("conversion");
        assert_eq!(c.offset, [0.0, 0.5, 0.5]);
        assert_eq!(c.ycc_to_rgb[0], [1.0, 799.0 / 8192.0, 1681.0 / 8192.0]);
        assert_eq!(c.rgb_to_lms[1][1], 17081.0 / 16384.0);
    }

    #[test]
    fn the_specs_defaults_are_limited_range_over_2_to_the_30() {
        // Luma black at 256 of 4096; chroma centred.
        let c = with_offsets([0x0400_0000, 0x2000_0000, 0x2000_0000])
            .conversion()
            .expect("conversion");
        assert_eq!(c.offset, [0.0625, 0.5, 0.5]);
    }

    #[test]
    fn offsets_that_fit_no_scale_are_refused() {
        assert!(
            with_offsets([0, 0x0800_0000, 0x2000_0000])
                .conversion()
                .is_none()
        );
        assert!(
            with_offsets([0, 0x0400_0000, 0x0400_0000])
                .conversion()
                .is_none()
        );
    }
}
