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
}
