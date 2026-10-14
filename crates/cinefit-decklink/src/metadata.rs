//! Per-frame colour metadata: colorspace, transfer function and HDR static
//! metadata (SMPTE ST 2086 mastering display, plus MaxCLL and MaxFALL).

use std::fmt;

use crate::ffi;

/// The frame's colorspace, as the SDK reports it (`BMDColorspace`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Colorspace {
    Rec601,
    Rec709,
    Rec2020,
    Other(u32),
}

impl Colorspace {
    fn from_raw(raw: i64) -> Option<Self> {
        let raw = u32::try_from(raw).ok()?;
        Some(match raw {
            0x7236_3031 => Colorspace::Rec601,  // 'r601'
            0x7237_3039 => Colorspace::Rec709,  // 'r709'
            0x3230_3230 => Colorspace::Rec2020, // '2020'
            _ => Colorspace::Other(raw),
        })
    }
}

impl fmt::Display for Colorspace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Colorspace::Rec601 => f.write_str("Rec.601"),
            Colorspace::Rec709 => f.write_str("Rec.709"),
            Colorspace::Rec2020 => f.write_str("Rec.2020"),
            Colorspace::Other(raw) => write!(f, "colorspace {raw:#010x}"),
        }
    }
}

/// The transfer function, numbered as in CTA-861.3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Eotf {
    /// Traditional gamma, SDR luminance range.
    Sdr,
    /// Traditional gamma, HDR luminance range. Rare; also what a 1080p menu
    /// was once seen to send with no metadata at all.
    TraditionalHdr,
    /// SMPTE ST 2084: HDR10.
    Pq,
    /// Hybrid log-gamma.
    Hlg,
    Other(u8),
}

impl Eotf {
    fn from_raw(raw: i64) -> Option<Self> {
        let raw = u8::try_from(raw).ok()?;
        Some(match raw {
            0 => Eotf::Sdr,
            1 => Eotf::TraditionalHdr,
            2 => Eotf::Pq,
            3 => Eotf::Hlg,
            _ => Eotf::Other(raw),
        })
    }
}

impl fmt::Display for Eotf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Eotf::Sdr => f.write_str("SDR"),
            Eotf::TraditionalHdr => f.write_str("traditional HDR"),
            Eotf::Pq => f.write_str("PQ"),
            Eotf::Hlg => f.write_str("HLG"),
            Eotf::Other(raw) => write!(f, "EOTF {raw}"),
        }
    }
}

/// A CIE 1931 xy chromaticity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Chromaticity {
    pub x: f64,
    pub y: f64,
}

/// HDR static metadata. Each field is `None` when the frame did not report it,
/// which is not the same as reporting zero.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HdrMetadata {
    /// Primaries and white point of the mastering display.
    pub red: Option<Chromaticity>,
    pub green: Option<Chromaticity>,
    pub blue: Option<Chromaticity>,
    pub white: Option<Chromaticity>,
    /// Mastering display luminance, cd/m².
    pub max_mastering: Option<f64>,
    pub min_mastering: Option<f64>,
    /// Content light levels, cd/m².
    pub max_cll: Option<f64>,
    pub max_fall: Option<f64>,
}

impl HdrMetadata {
    /// Decodes the shim's fields, or `None` when nothing real was sent.
    ///
    /// A value is unreported when the shim could not read it (NaN) or when it
    /// is zero: CTA-861.3 uses 0 for "unknown", and HDR10 discs commonly send
    /// MaxCLL 0. The one exception is the minimum mastering luminance, where 0
    /// is a plausible measurement, so it counts whenever a maximum is given.
    /// An SDR source seen sending the HDR flag with every value 0 so decodes
    /// as no HDR metadata at all.
    pub(crate) fn from_raw(raw: &ffi::FrameInfo) -> Option<Self> {
        let known = |d: f64| (!d.is_nan() && d != 0.0).then_some(d);
        let xy = |x: f64, y: f64| {
            Some(Chromaticity {
                x: known(x)?,
                y: known(y)?,
            })
        };
        let max_mastering = known(raw.max_mastering);
        let m = Self {
            red: xy(raw.red_x, raw.red_y),
            green: xy(raw.green_x, raw.green_y),
            blue: xy(raw.blue_x, raw.blue_y),
            white: xy(raw.white_x, raw.white_y),
            max_mastering,
            min_mastering: max_mastering
                .and((!raw.min_mastering.is_nan()).then_some(raw.min_mastering)),
            max_cll: known(raw.max_cll),
            max_fall: known(raw.max_fall),
        };
        (m != Self::EMPTY).then_some(m)
    }

    const EMPTY: Self = Self {
        red: None,
        green: None,
        blue: None,
        white: None,
        max_mastering: None,
        min_mastering: None,
        max_cll: None,
        max_fall: None,
    };
}

impl fmt::Display for HdrMetadata {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let xy = |c: Option<Chromaticity>| match c {
            Some(c) => format!("({:.4}, {:.4})", c.x, c.y),
            None => "unreported".into(),
        };
        let nits = |v: Option<f64>| match v {
            Some(v) => format!("{v}"),
            None => "unreported".into(),
        };
        write!(
            f,
            "R {} G {} B {} W {}, mastering {}-{} nits, MaxCLL {}, MaxFALL {}",
            xy(self.red),
            xy(self.green),
            xy(self.blue),
            xy(self.white),
            nits(self.min_mastering),
            nits(self.max_mastering),
            nits(self.max_cll),
            nits(self.max_fall),
        )
    }
}

/// What the frame says about its colour, all of it optional.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorInfo {
    pub colorspace: Option<Colorspace>,
    pub eotf: Option<Eotf>,
    /// Present only on frames flagged as carrying HDR metadata.
    pub hdr: Option<HdrMetadata>,
}

impl From<&ffi::FrameInfo> for ColorInfo {
    fn from(raw: &ffi::FrameInfo) -> Self {
        Self {
            colorspace: (raw.colorspace != ffi::UNKNOWN)
                .then(|| Colorspace::from_raw(raw.colorspace))
                .flatten(),
            eotf: (raw.eotf != ffi::UNKNOWN)
                .then(|| Eotf::from_raw(raw.eotf))
                .flatten(),
            hdr: HdrMetadata::from_raw(raw),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A raw frame with nothing reported, for tests to fill in.
    pub(crate) fn unreported() -> ffi::FrameInfo {
        ffi::FrameInfo {
            stream_time: ffi::UNKNOWN,
            stream_duration: ffi::UNKNOWN,
            row_bytes: 0,
            width: 0,
            height: 0,
            pixel_format: 0,
            flags: 0,
            colorspace: ffi::UNKNOWN,
            eotf: ffi::UNKNOWN,
            red_x: f64::NAN,
            red_y: f64::NAN,
            green_x: f64::NAN,
            green_y: f64::NAN,
            blue_x: f64::NAN,
            blue_y: f64::NAN,
            white_x: f64::NAN,
            white_y: f64::NAN,
            max_mastering: f64::NAN,
            min_mastering: f64::NAN,
            max_cll: f64::NAN,
            max_fall: f64::NAN,
            dolby_vision_bytes: ffi::UNKNOWN,
        }
    }

    #[test]
    fn nothing_reported_is_none_not_zero() {
        let c = ColorInfo::from(&unreported());
        assert_eq!(c.colorspace, None);
        assert_eq!(c.eotf, None);
        assert_eq!(c.hdr, None);
    }

    #[test]
    fn hdr10_decodes() {
        let mut r = unreported();
        r.colorspace = 0x3230_3230;
        r.eotf = 2;
        (r.red_x, r.red_y) = (0.68, 0.32);
        (r.green_x, r.green_y) = (0.265, 0.69);
        (r.blue_x, r.blue_y) = (0.15, 0.06);
        (r.white_x, r.white_y) = (0.3127, 0.329);
        r.max_mastering = 1000.0;
        r.min_mastering = 0.005;
        r.max_cll = 637.0;
        r.max_fall = 148.0;
        let c = ColorInfo::from(&r);
        assert_eq!(c.colorspace, Some(Colorspace::Rec2020));
        assert_eq!(c.eotf, Some(Eotf::Pq));
        let hdr = c.hdr.expect("HDR metadata");
        assert_eq!(hdr.red, Some(Chromaticity { x: 0.68, y: 0.32 }));
        assert_eq!(hdr.max_mastering, Some(1000.0));
        assert_eq!(hdr.max_cll, Some(637.0));
    }

    #[test]
    fn a_missing_half_of_a_chromaticity_makes_it_unreported() {
        let mut r = unreported();
        r.red_x = 0.68;
        r.max_cll = 400.0;
        let hdr = HdrMetadata::from_raw(&r).expect("MaxCLL alone still counts");
        assert_eq!(hdr.red, None);
        assert_eq!(hdr.max_cll, Some(400.0));
    }

    #[test]
    fn sdr_rec709_and_rec2020_are_told_apart() {
        let mut r = unreported();
        r.eotf = 0;
        r.colorspace = 0x7237_3039;
        let a = ColorInfo::from(&r);
        r.colorspace = 0x3230_3230;
        let b = ColorInfo::from(&r);
        assert_eq!(
            (a.eotf, a.colorspace),
            (Some(Eotf::Sdr), Some(Colorspace::Rec709))
        );
        assert_eq!(
            (b.eotf, b.colorspace),
            (Some(Eotf::Sdr), Some(Colorspace::Rec2020))
        );
        assert_eq!(a.hdr, None);
    }

    #[test]
    fn unknown_codes_are_kept_not_dropped() {
        let mut r = unreported();
        r.eotf = 7;
        r.colorspace = 0x4E63_6F6C; // 'Ncol'
        let c = ColorInfo::from(&r);
        assert_eq!(c.eotf, Some(Eotf::Other(7)));
        assert_eq!(c.colorspace, Some(Colorspace::Other(0x4E63_6F6C)));
    }

    fn all_zero() -> ffi::FrameInfo {
        ffi::FrameInfo {
            red_x: 0.0,
            red_y: 0.0,
            green_x: 0.0,
            green_y: 0.0,
            blue_x: 0.0,
            blue_y: 0.0,
            white_x: 0.0,
            white_y: 0.0,
            max_mastering: 0.0,
            min_mastering: 0.0,
            max_cll: 0.0,
            max_fall: 0.0,
            ..unreported()
        }
    }

    #[test]
    fn all_zero_metadata_is_no_metadata() {
        // Seen on an SDR clip: the HDR flag set, every value 0.
        assert_eq!(HdrMetadata::from_raw(&all_zero()), None);
    }

    #[test]
    fn zero_light_levels_are_unknown_but_the_rest_is_kept() {
        let mut r = all_zero();
        (r.red_x, r.red_y) = (0.708, 0.292);
        r.max_mastering = 1000.0;
        let hdr = HdrMetadata::from_raw(&r).expect("real primaries");
        assert_eq!(hdr.red, Some(Chromaticity { x: 0.708, y: 0.292 }));
        assert_eq!(hdr.max_cll, None);
        assert_eq!(hdr.max_fall, None);
        // A 0-nit minimum is a value when a maximum is given.
        assert_eq!(hdr.min_mastering, Some(0.0));
    }

    #[test]
    fn a_zero_minimum_alone_is_not_metadata() {
        let r = ffi::FrameInfo {
            min_mastering: 0.0,
            ..unreported()
        };
        assert_eq!(HdrMetadata::from_raw(&r), None);
    }
}
