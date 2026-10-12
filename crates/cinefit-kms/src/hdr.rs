//! HDR static metadata as the kernel takes it in an `HDR_OUTPUT_METADATA`
//! property blob: `struct hdr_output_metadata` holding a CTA-861.3 static
//! metadata type 1 infoframe.
//!
//! The layout is easy to get silently wrong, so the bytes are written one
//! field at a time at fixed offsets rather than through a packed struct:
//!
//! - the primaries are interleaved `{x, y}` pairs in red, green, blue order,
//!   not separate x and y arrays (same size, different meaning);
//! - the 30 bytes of payload are padded to 32, and the kernel rejects a blob
//!   of any other size.

/// The transfer function signalled to the display (CTA-861.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Eotf {
    Sdr = 0,
    TraditionalHdr = 1,
    Pq = 2,
    Hlg = 3,
}

/// HDR10 static metadata in natural units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HdrStatic {
    pub eotf: Eotf,
    /// Mastering display primaries, red, green, blue, as CIE 1931 xy.
    pub primaries: [(f64, f64); 3],
    pub white: (f64, f64),
    /// Mastering display luminance, cd/m².
    pub max_mastering: f64,
    pub min_mastering: f64,
    /// Content light levels, cd/m²; 0 means unknown, which is valid to send.
    pub max_cll: f64,
    pub max_fall: f64,
}

impl HdrStatic {
    /// PQ with BT.2020 primaries and D65, mastered at 0.0001-1000 nits.
    pub const BT2020_1000: Self = Self {
        eotf: Eotf::Pq,
        primaries: [(0.708, 0.292), (0.170, 0.797), (0.131, 0.046)],
        white: (0.3127, 0.3290),
        max_mastering: 1000.0,
        min_mastering: 0.0001,
        max_cll: 1000.0,
        max_fall: 400.0,
    };

    /// MaxFALL above MaxCLL, or MaxCLL above the mastering peak, is a
    /// contradiction: a frame's average cannot outshine its brightest pixel.
    pub fn is_coherent(&self) -> bool {
        !(self.max_cll > 0.0 && self.max_fall > self.max_cll)
            && !(self.max_mastering > 0.0 && self.max_cll > self.max_mastering)
    }

    /// The 32 bytes of `struct hdr_output_metadata`, in host byte order as
    /// the kernel reads it.
    pub fn blob(&self) -> [u8; BLOB_BYTES] {
        let chroma = |v: f64| (v * 50_000.0).round().clamp(0.0, 65_535.0) as u16;
        let nits = |v: f64| v.round().clamp(0.0, 65_535.0) as u16;
        let mut b = [0u8; BLOB_BYTES];
        // metadata_type 0: HDMI static metadata type 1.
        b[0..4].copy_from_slice(&0u32.to_ne_bytes());
        b[4] = self.eotf as u8;
        b[5] = 0; // the infoframe's own metadata type: static type 1
        let mut put = |at: usize, v: u16| b[at..at + 2].copy_from_slice(&v.to_ne_bytes());
        for (i, &(x, y)) in self.primaries.iter().enumerate() {
            put(6 + 4 * i, chroma(x));
            put(8 + 4 * i, chroma(y));
        }
        put(18, chroma(self.white.0));
        put(20, chroma(self.white.1));
        put(22, nits(self.max_mastering));
        // The one field in 0.0001 cd/m² units rather than cd/m².
        put(
            24,
            (self.min_mastering * 10_000.0).round().clamp(0.0, 65_535.0) as u16,
        );
        put(26, nits(self.max_cll));
        put(28, nits(self.max_fall));
        // 30..32: padding to the kernel struct's 4-byte alignment.
        b
    }
}

/// `sizeof(struct hdr_output_metadata)`.
pub const BLOB_BYTES: usize = 32;

#[cfg(test)]
mod tests {
    use super::*;

    fn u16_at(b: &[u8], at: usize) -> u16 {
        u16::from_ne_bytes([b[at], b[at + 1]])
    }

    #[test]
    fn fields_land_at_the_kernel_offsets() {
        let b = HdrStatic::BT2020_1000.blob();
        assert_eq!(u32::from_ne_bytes(b[0..4].try_into().unwrap()), 0);
        assert_eq!((b[4], b[5]), (2, 0)); // PQ, static type 1
        // Interleaved: red x, red y, green x, ...
        assert_eq!((u16_at(&b, 6), u16_at(&b, 8)), (35_400, 14_600));
        assert_eq!((u16_at(&b, 10), u16_at(&b, 12)), (8_500, 39_850));
        assert_eq!((u16_at(&b, 14), u16_at(&b, 16)), (6_550, 2_300));
        assert_eq!((u16_at(&b, 18), u16_at(&b, 20)), (15_635, 16_450));
        assert_eq!(u16_at(&b, 22), 1000);
        assert_eq!(u16_at(&b, 24), 1); // 0.0001 nits
        assert_eq!((u16_at(&b, 26), u16_at(&b, 28)), (1000, 400));
        assert_eq!(&b[30..], &[0, 0]);
    }

    #[test]
    fn values_clamp_to_the_fields() {
        let s = HdrStatic {
            max_cll: 1e9,
            min_mastering: -1.0,
            ..HdrStatic::BT2020_1000
        };
        let b = s.blob();
        assert_eq!(u16_at(&b, 26), u16::MAX);
        assert_eq!(u16_at(&b, 24), 0);
    }

    #[test]
    fn incoherent_light_levels_are_caught() {
        assert!(HdrStatic::BT2020_1000.is_coherent());
        let fall_over_cll = HdrStatic {
            max_cll: 300.0,
            max_fall: 400.0,
            ..HdrStatic::BT2020_1000
        };
        assert!(!fall_over_cll.is_coherent());
        let unknown_cll = HdrStatic {
            max_cll: 0.0,
            ..HdrStatic::BT2020_1000
        };
        assert!(unknown_cll.is_coherent());
    }
}
