//! How a picture's colour is described to the renderer, and a CPU reference
//! for the YCbCr to RGB conversion the GPU checks compare against.

/// The YCbCr matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Matrix {
    Bt709,
    /// BT.2020 non-constant luminance, which is what HDMI carries.
    Bt2020Nc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Primaries {
    Bt709,
    Bt2020,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transfer {
    /// SDR.
    Bt1886,
    Pq,
    Hlg,
}

/// Captured YCbCr: always limited range, as HDMI sends it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct YcbcrColor {
    pub matrix: Matrix,
    pub primaries: Primaries,
    pub transfer: Transfer,
}

/// The range of RGB codes. HDMI RGB is limited for video formats unless the
/// source signals full, and the capture card does not report which.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Levels {
    Full,
    Limited,
}

/// Captured RGB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RgbSourceColor {
    pub levels: Levels,
    pub primaries: Primaries,
    pub transfer: Transfer,
}

/// An RGB target: always full range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RgbColor {
    pub primaries: Primaries,
    pub transfer: Transfer,
}

impl Matrix {
    /// The luma coefficients (Kr, Kb).
    fn kr_kb(self) -> (f64, f64) {
        match self {
            Matrix::Bt709 => (0.2126, 0.0722),
            Matrix::Bt2020Nc => (0.2627, 0.0593),
        }
    }
}

/// Limited-range 10-bit YCbCr codes to non-linear R'G'B' in 0-1 (outside it
/// for colours the matrix maps out of range). Black is 64 and white 940; the
/// chroma's zero is 512, with 448 either side of it. Codes may be fractional,
/// for values between samples.
pub fn ycbcr10_to_rgb(y: f64, cb: f64, cr: f64, matrix: Matrix) -> [f64; 3] {
    let (kr, kb) = matrix.kr_kb();
    let kg = 1.0 - kr - kb;
    let y = (y - 64.0) / 876.0;
    let pb = (cb - 512.0) / 896.0;
    let pr = (cr - 512.0) / 896.0;
    let r = y + 2.0 * (1.0 - kr) * pr;
    let b = y + 2.0 * (1.0 - kb) * pb;
    let g = (y - kr * r - kb * b) / kg;
    [r, g, b]
}

/// An RGB code of `depth` bits to 0-1 (outside it for limited-range codes
/// past black or white). Limited range puts black at 16 and white at 235,
/// scaled to the depth.
pub fn rgb_code_to_unit(code: u16, depth: u8, levels: Levels) -> f64 {
    let scale = f64::from(1u32 << (depth - 8));
    match levels {
        Levels::Full => f64::from(code) / f64::from((1u32 << depth) - 1),
        Levels::Limited => (f64::from(code) - 16.0 * scale) / (219.0 * scale),
    }
}

/// A tunnel frame's colour conversion, from its metadata: see
/// `cinefit_dovi::Conversion`, which this mirrors so the renderer needs no
/// dependency on the metadata crate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DoviColor {
    /// ICtCp to non-linear LMS; rows are outputs.
    pub ycc_to_lms: [[f64; 3]; 3],
    /// Subtracted from the normalised signal first.
    pub offset: [f64; 3],
    /// The metadata's RGB-to-LMS matrix (libplacebo's `linear`).
    pub linear: [[f64; 3]; 3],
}

/// SMPTE ST 2084, signal 0-1 to linear 0-1 (of 10 000 nits).
pub fn pq_eotf(e: f64) -> f64 {
    const M1: f64 = 2610.0 / 16384.0;
    const M2: f64 = 2523.0 / 4096.0 * 128.0;
    const C1: f64 = 3424.0 / 4096.0;
    const C2: f64 = 2413.0 / 4096.0 * 32.0;
    const C3: f64 = 2392.0 / 4096.0 * 32.0;
    let p = e.max(0.0).powf(1.0 / M2);
    ((p - C1).max(0.0) / (C2 - C3 * p)).powf(1.0 / M1)
}

/// The inverse of [`pq_eotf`].
pub fn pq_oetf(y: f64) -> f64 {
    const M1: f64 = 2610.0 / 16384.0;
    const M2: f64 = 2523.0 / 4096.0 * 128.0;
    const C1: f64 = 3424.0 / 4096.0;
    const C2: f64 = 2413.0 / 4096.0 * 32.0;
    const C3: f64 = 2392.0 / 4096.0 * 32.0;
    let p = y.max(0.0).powf(M1);
    ((C1 + C2 * p) / (1.0 + C3 * p)).powf(M2)
}

/// Hunt-Pointer-Estevez LMS to BT.2020 RGB, the fixed step Dolby Vision's
/// metadata does not carry. Taken from libplacebo (`shaders/colorspace.c`),
/// so this reference checks how the conversion is wired, not the constant.
const HPE_LMS_TO_BT2020: [[f64; 3]; 3] = [
    [3.06441879, -2.16597676, 0.10155818],
    [-0.65612108, 1.78554118, -0.12943749],
    [0.01736321, -0.04725154, 1.03004253],
];

fn mul(m: &[[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    m.map(|row| row[0] * v[0] + row[1] * v[1] + row[2] * v[2])
}

/// 12-bit tunnel codes to BT.2020 PQ RGB, 0-1: offset and ICtCp to L'M'S',
/// PQ to linear, LMS to RGB, back to PQ. Codes may be fractional.
///
/// The planes hold each code as `code << 4` of 16 bits, so code `c` is the
/// signal `16c / 65535` (within 2e-5 of `c / 4096`), and libplacebo scales
/// the offsets by `65536 / 65535` to match: chroma code 2048 is exactly
/// centred.
pub fn dovi12_to_pq_rgb(y: f64, cb: f64, cr: f64, c: &DoviColor) -> [f64; 3] {
    let x = [y, cb, cr].map(|v| v * 16.0 / 65535.0);
    let d: [f64; 3] = std::array::from_fn(|i| x[i] - c.offset[i] * 65536.0 / 65535.0);
    let lms = mul(&c.ycc_to_lms, d).map(pq_eotf);
    let to_rgb: [[f64; 3]; 3] = std::array::from_fn(|r| {
        std::array::from_fn(|k| {
            (0..3)
                .map(|j| HPE_LMS_TO_BT2020[r][j] * c.linear[j][k])
                .sum()
        })
    });
    mul(&to_rgb, lms).map(pq_oetf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: [f64; 3], b: [f64; 3]) -> bool {
        a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-3)
    }

    #[test]
    fn black_and_white_are_grey_in_either_matrix() {
        for m in [Matrix::Bt709, Matrix::Bt2020Nc] {
            assert!(near(ycbcr10_to_rgb(64.0, 512.0, 512.0, m), [0.0; 3]));
            assert!(near(ycbcr10_to_rgb(940.0, 512.0, 512.0, m), [1.0; 3]));
        }
    }

    #[test]
    fn full_primaries_come_back_from_their_codes() {
        // BT.709 100% red in 10-bit: Y 250, Cb 409, Cr 960.
        assert!(near(
            ycbcr10_to_rgb(250.0, 409.0, 960.0, Matrix::Bt709),
            [1.0, 0.0, 0.0]
        ));
        // BT.2020 100% blue: Y 116, Cb 960, Cr 476.
        let b = ycbcr10_to_rgb(116.0, 960.0, 476.0, Matrix::Bt2020Nc);
        assert!(near(b, [0.0, 0.0, 1.0]), "{b:?}");
    }

    #[test]
    fn rgb_codes_scale_by_depth_and_range() {
        assert_eq!(rgb_code_to_unit(1023, 10, Levels::Full), 1.0);
        assert_eq!(rgb_code_to_unit(255, 8, Levels::Full), 1.0);
        assert_eq!(rgb_code_to_unit(64, 10, Levels::Limited), 0.0);
        assert_eq!(rgb_code_to_unit(940, 10, Levels::Limited), 1.0);
        assert_eq!(rgb_code_to_unit(235, 8, Levels::Limited), 1.0);
    }

    #[test]
    fn pq_round_trips() {
        // Not exact at 0: the curve's foot puts 0 nits at 7e-7.
        for e in [0.0, 0.1, 0.5, 0.75, 1.0] {
            assert!((pq_oetf(pq_eotf(e)) - e).abs() < 1e-6);
        }
        // 1000 nits.
        assert!((pq_eotf(0.751_827) - 0.1).abs() < 1e-5);
    }

    #[test]
    fn neutral_ictcp_is_grey_and_keeps_its_level() {
        // A UB820's conversion (luma 1.0, offset 0, chroma centred).
        let c = DoviColor {
            ycc_to_lms: [
                [1.0, 799.0 / 8192.0, 1681.0 / 8192.0],
                [1.0, -933.0 / 8192.0, 1091.0 / 8192.0],
                [1.0, 267.0 / 8192.0, -5545.0 / 8192.0],
            ],
            offset: [0.0, 0.5, 0.5],
            linear: [
                [17081.0 / 16384.0, -349.0 / 16384.0, -349.0 / 16384.0],
                [-349.0 / 16384.0, 17081.0 / 16384.0, -349.0 / 16384.0],
                [-349.0 / 16384.0, -349.0 / 16384.0, 17081.0 / 16384.0],
            ],
        };
        for i in [64.0, 1024.0, 2048.0, 3079.0, 4000.0] {
            let rgb = dovi12_to_pq_rgb(i, 2048.0, 2048.0, &c);
            assert!(
                (rgb[0] - rgb[1]).abs() < 1e-4 && (rgb[1] - rgb[2]).abs() < 1e-4,
                "{i}: {rgb:?}"
            );
            // Grey I stays I: the matrices' rows each sum to about 1.
            assert!((rgb[1] - i * 16.0 / 65535.0).abs() < 1e-3, "{i}: {rgb:?}");
        }
    }
}

/// A tone curve between two brightness ranges: libplacebo's, by its names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Curve {
    Clip,
    Hable,
    Spline,
    Bt2390,
    Bt2446a,
    St2094_10,
    St2094_40,
    Mobius,
    Reinhard,
}

impl Curve {
    /// Every curve with its libplacebo name, in a stable order.
    pub const ALL: [(Curve, &'static str); 9] = [
        (Curve::Spline, "spline"),
        (Curve::Hable, "hable"),
        (Curve::Bt2390, "bt2390"),
        (Curve::Bt2446a, "bt2446a"),
        (Curve::St2094_10, "st2094-10"),
        (Curve::St2094_40, "st2094-40"),
        (Curve::Mobius, "mobius"),
        (Curve::Reinhard, "reinhard"),
        (Curve::Clip, "clip"),
    ];

    pub fn name(self) -> &'static str {
        Self::ALL
            .iter()
            .find(|(c, _)| *c == self)
            .map_or("clip", |(_, n)| n)
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().find(|(_, n)| *n == name).map(|(c, _)| *c)
    }
}

/// A mapping for a render into a target that cannot show the source's range:
/// the source's peak and the target's, in nits, and the curve between. A peak
/// of 0 leaves libplacebo's default for that side.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mapping {
    pub src_peak: f32,
    pub dst_peak: f32,
    pub curve: Curve,
    /// Measure each frame, so the curve follows the scene's peak and average
    /// rather than `src_peak` alone (libplacebo's peak detection).
    pub peak_detect: bool,
}
