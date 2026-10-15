//! What the input is, and what follows from that: the output's signalling,
//! the banner's and the info panel's words, and which inputs the detector
//! cannot read. Pure: `play` classifies a frame from what the card reports
//! ([`Kind`]), and everything that depends on what that *means* asks here.

use cinefit_decklink::{HdrMetadata, InputFormat, PixelFormat};
use cinefit_render::{DoviColor, Primaries, RgbColor, Transfer, YcbcrColor, rgb};

use crate::config::{Config, OutputMode};
use crate::follow;
use crate::osd::{BannerText, Encoding, InfoSection};

/// What a frame is, as far as rendering goes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Kind {
    Ycbcr(YcbcrColor),
    Rgb(rgb::Format),
    Tunnel,
    /// A capture format nothing renders.
    Unsupported(PixelFormat),
}

/// The input as the output is chosen from: two frames with equal keys need
/// the same output.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Key {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) kind: Kind,
}

/// A decoded tunnel: its conversion, and the grade's range for the output's
/// static metadata.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Tunnel {
    pub(crate) color: DoviColor,
    pub(crate) source_min_pq: u16,
    pub(crate) source_max_pq: u16,
    /// This scene's brightest maxRGB, as a 12-bit PQ code (level 1
    /// `max_PQ`): the same measure as MaxCLL, per scene.
    pub(crate) scene_max_pq: Option<u16>,
    /// This scene's average maxRGB, as a 12-bit PQ code (level 1 `avg_PQ`),
    /// sent as MaxFALL.
    pub(crate) scene_avg_pq: Option<u16>,
}

/// What the output signals and renders into for an input.
pub(crate) fn output_for(
    kind: Kind,
    hdr: Option<&HdrMetadata>,
    tunnel: Option<&Tunnel>,
) -> (Option<cinefit_kms::HdrStatic>, RgbColor) {
    const BT2020: [(f64, f64); 3] = [(0.708, 0.292), (0.170, 0.797), (0.131, 0.046)];
    const D65: (f64, f64) = (0.3127, 0.3290);
    let pq_out = RgbColor {
        primaries: Primaries::Bt2020,
        transfer: Transfer::Pq,
    };
    let base = cinefit_kms::HdrStatic {
        eotf: cinefit_kms::Eotf::Pq,
        primaries: BT2020,
        white: D65,
        max_mastering: 0.0,
        min_mastering: 0.0,
        max_cll: 0.0,
        max_fall: 0.0,
    };
    match kind {
        // The grade's range as mastering, and this scene's peak as MaxCLL:
        // the JVC re-derives its curve from a MaxCLL change mid-stream with
        // no resync (measured 2026-10-12), so the curve follows the film
        // scene by scene. Capped at the grade's peak. With no level 1 yet,
        // the grade's peak too: 0 is "unknown", and was sent at the start of
        // a title until the first scene arrived (2026-10-13).
        Kind::Tunnel => {
            let mastering = tunnel.map_or(0.0, |t| cinefit_dovi::pq_to_nits(t.source_max_pq));
            let scene = tunnel
                .and_then(|t| t.scene_max_pq)
                .map_or(mastering, cinefit_dovi::pq_to_nits);
            (
                Some(cinefit_kms::HdrStatic {
                    max_mastering: mastering,
                    min_mastering: tunnel
                        .map_or(0.0, |t| cinefit_dovi::pq_to_nits(t.source_min_pq)),
                    // Whole nits: the blob carries no finer, and a value that
                    // rounds the same is no change to send.
                    max_cll: if mastering > 0.0 {
                        scene.min(mastering)
                    } else {
                        scene
                    }
                    .round(),
                    // An average taken in PQ, so below MaxFALL's linear one;
                    // the nearest measure the metadata has. 0, "unknown",
                    // with no level 1. Whether the JVC reads MaxFALL at all is
                    // not yet measured.
                    max_fall: tunnel
                        .and_then(|t| t.scene_avg_pq)
                        .map_or(0.0, cinefit_dovi::pq_to_nits)
                        .round(),
                    ..base
                }),
                pq_out,
            )
        }
        Kind::Ycbcr(c) if c.transfer == Transfer::Pq => {
            // The source's own values; any it does not send stay unknown (0)
            // or default to BT.2020 D65.
            let xy = |c: Option<cinefit_decklink::Chromaticity>, d: (f64, f64)| {
                c.map_or(d, |c| (c.x, c.y))
            };
            let field = |f: fn(&HdrMetadata) -> Option<f64>| hdr.and_then(f).unwrap_or(0.0);
            (
                Some(cinefit_kms::HdrStatic {
                    primaries: [
                        xy(hdr.and_then(|h| h.red), BT2020[0]),
                        xy(hdr.and_then(|h| h.green), BT2020[1]),
                        xy(hdr.and_then(|h| h.blue), BT2020[2]),
                    ],
                    white: xy(hdr.and_then(|h| h.white), D65),
                    max_mastering: field(|h| h.max_mastering),
                    min_mastering: field(|h| h.min_mastering),
                    max_cll: field(|h| h.max_cll),
                    max_fall: field(|h| h.max_fall),
                    ..base
                }),
                pq_out,
            )
        }
        Kind::Ycbcr(c) if c.transfer == Transfer::Hlg => (
            Some(cinefit_kms::HdrStatic {
                eotf: cinefit_kms::Eotf::Hlg,
                ..base
            }),
            RgbColor {
                primaries: Primaries::Bt2020,
                transfer: Transfer::Hlg,
            },
        ),
        _ => (
            None,
            RgbColor {
                primaries: Primaries::Bt709,
                transfer: Transfer::Bt1886,
            },
        ),
    }
}

/// SDR in a BT.2020 container: what the output sends unless HDR10 is passed
/// through.
pub(crate) const SDR_BT2020: RgbColor = RgbColor {
    primaries: Primaries::Bt2020,
    transfer: Transfer::Bt1886,
};

/// Whether a frame is a player's low-latency Dolby Vision: YCbCr labelled SDR
/// BT.2020, when the `input.lldv` setting says that label means it.
pub(crate) fn is_lldv(kind: Kind, lldv: bool) -> bool {
    matches!(kind, Kind::Ycbcr(c) if lldv && c.primaries == Primaries::Bt2020 && c.transfer == Transfer::Bt1886)
}

/// The output for an input under the `output.mode` setting. Dolby Vision, as
/// a tunnel or already mapped by the player, always goes out as SDR BT.2020:
/// passed through it would be HDR10 with derived metadata, which the display
/// maps by its own rules. Passthrough is for HDR10 (and HLG) only.
pub(crate) fn output_in(
    mode: OutputMode,
    lldv: bool,
    kind: Kind,
    hdr: Option<&HdrMetadata>,
    tunnel: Option<&Tunnel>,
) -> (Option<cinefit_kms::HdrStatic>, RgbColor) {
    let sdr = (None, SDR_BT2020);
    if kind == Kind::Tunnel || is_lldv(kind, lldv) || mode == OutputMode::Sdr2020 {
        return sdr;
    }
    match output_for(kind, hdr, tunnel) {
        (Some(h), color) => (Some(h), color),
        (None, _) => sdr,
    }
}

/// The policy's view of an output's signalling.
pub(crate) fn signal_of(hdr: Option<cinefit_kms::HdrStatic>) -> follow::Signal {
    hdr.map_or(follow::Signal::Sdr, follow::Signal::Hdr)
}

/// What a frame renders into for a signalling: BT.2020 PQ or HLG for HDR,
/// SDR BT.2020 otherwise.
pub(crate) fn color_for(signal: follow::Signal) -> RgbColor {
    match signal {
        follow::Signal::Hdr(h) if h.eotf == cinefit_kms::Eotf::Hlg => RgbColor {
            primaries: Primaries::Bt2020,
            transfer: Transfer::Hlg,
        },
        follow::Signal::Hdr(_) => RgbColor {
            primaries: Primaries::Bt2020,
            transfer: Transfer::Pq,
        },
        follow::Signal::Sdr => RgbColor {
            primaries: Primaries::Bt2020,
            transfer: Transfer::Bt1886,
        },
    }
}

/// How the overlay is encoded for an output's signalling: PQ for HDR10, a
/// gamma of 2.4 otherwise (HLG's lower range is close to it).
pub(crate) fn encoding_for(signal: follow::Signal) -> Encoding {
    match signal {
        follow::Signal::Hdr(h) if h.eotf == cinefit_kms::Eotf::Pq => Encoding::Pq,
        _ => Encoding::Sdr,
    }
}

/// The slate's detail for a format nothing renders: what it is, and what
/// to change.
pub(crate) fn cant_show(f: PixelFormat) -> String {
    format!("{f} \u{b7} set the player to YCbCr")
}

/// What the info panel says about the input: its picture type, then size,
/// rate and sampling as the card reports them.
pub(crate) fn input_section(
    key: Option<Key>,
    format: Option<InputFormat>,
    dark: bool,
    lldv: Option<u16>,
    peak: Option<u16>,
) -> InfoSection {
    let label = "IN".to_string();
    let Some(k) = key.filter(|_| !dark) else {
        return InfoSection {
            label,
            line: "No signal".into(),
            detail: "waiting for a source".into(),
        };
    };
    let line = match k.kind {
        Kind::Tunnel => "Dolby Vision \u{b7} tunnel".into(),
        kind if is_lldv(kind, lldv.is_some()) => "Dolby Vision \u{b7} low latency".into(),
        Kind::Ycbcr(c) if c.transfer == Transfer::Pq => "HDR10".into(),
        Kind::Ycbcr(c) if c.transfer == Transfer::Hlg => "HLG".into(),
        Kind::Ycbcr(c) if c.primaries == Primaries::Bt2020 => "SDR BT.2020".into(),
        Kind::Ycbcr(_) => "SDR".into(),
        Kind::Rgb(_) => "RGB".into(),
        Kind::Unsupported(f) => format!("{f}: nothing renders it"),
    };
    let mut detail = format!("{} \u{d7} {}", k.width, k.height);
    if let Some(f) = format {
        detail += &format!(" \u{b7} {}", rate_name(rate_mhz(&f)));
        detail += match f.sampling {
            cinefit_decklink::Sampling::Ycbcr422 => " \u{b7} YCbCr 4:2:2",
            cinefit_decklink::Sampling::Rgb444 => " \u{b7} RGB 4:4:4",
        };
        if let Some(d) = f.bit_depth {
            detail += &format!(" {d}-bit");
        }
    }
    if let Some(p) = peak {
        detail += &format!(" \u{b7} {}", peak_words(k.kind, p, lldv));
    }
    InfoSection {
        label,
        line,
        detail,
    }
}

/// What the info panel says about the output: what it signals and what
/// happened to the picture on the way, then the mode.
pub(crate) fn output_section(
    key: Option<Key>,
    dark: bool,
    lldv: bool,
    signal: follow::Signal,
    mode: cinefit_kms::ModeTiming,
    shown: Option<f64>,
    white_nits: u16,
) -> InfoSection {
    let (name, hdr) = match signal {
        follow::Signal::Sdr => ("SDR BT.2020", None),
        follow::Signal::Hdr(h) if h.eotf == cinefit_kms::Eotf::Hlg => ("HLG", None),
        follow::Signal::Hdr(h) => ("HDR10", Some(h)),
    };
    let what = match key.filter(|_| !dark).map(|k| k.kind) {
        None => "black, waiting",
        Some(Kind::Tunnel) => "cinefit\u{2019}s own mapping",
        Some(k) if is_lldv(k, lldv) => "unchanged from the player",
        Some(Kind::Ycbcr(c)) if matches!(c.transfer, Transfer::Pq | Transfer::Hlg) => {
            if hdr.is_some() || name == "HLG" {
                "passed through"
            } else {
                "tone-mapped by cinefit"
            }
        }
        Some(Kind::Ycbcr(c)) if c.primaries == Primaries::Bt709 => "BT.709 placed in BT.2020",
        Some(Kind::Unsupported(_)) => "black: nothing renders the input",
        Some(_) => "unchanged",
    };
    let mut detail = format!(
        "{} \u{d7} {} \u{b7} {}",
        mode.width,
        mode.height,
        rate_name((mode.refresh_hz() * 1000.0).round() as u32)
    );
    if let Some(h) = hdr {
        detail += &format!(" \u{b7} MaxCLL {:.0}", h.max_cll);
    }
    if let Some(n) = shown.and_then(|v| out_nits(v, signal, white_nits)) {
        detail += &format!(" \u{b7} brightest {n:.0} nits");
    }
    InfoSection {
        label: "OUT".into(),
        line: format!("{name} \u{b7} {what}"),
        detail,
    }
}

/// A level read back from the output (0-1) as light: SDR through its gamma
/// to cinefit's white, PQ as it is. HLG has no absolute level.
pub(crate) fn out_nits(v: f64, signal: follow::Signal, white_nits: u16) -> Option<f64> {
    match signal {
        follow::Signal::Sdr => Some(v.powf(2.4) * f64::from(white_nits)),
        follow::Signal::Hdr(h) if h.eotf == cinefit_kms::Eotf::Pq => Some(pq_nits(v)),
        follow::Signal::Hdr(_) => None,
    }
}

/// A picture size as people say it.
pub(crate) fn size_name(width: u32, height: u32) -> String {
    match (width, height) {
        (3840 | 4096, 2160) => "4K".into(),
        (1920, 1080) => "1080p".into(),
        (1280, 720) => "720p".into(),
        _ => format!("{width} \u{d7} {height}"),
    }
}

/// A frame rate as people say it: 23.976, 24, 59.94.
pub(crate) fn rate_name(rate_mhz: u32) -> String {
    let s = format!("{:.3}", f64::from(rate_mhz) / 1000.0);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The banner for a picture (#77): what is coming in, and what happens to
/// it. `None` for what nothing renders, which has the slate instead.
pub(crate) fn banner_for(
    kind: Kind,
    key: (u32, u32),
    rate_mhz: u32,
    out_width: u32,
    config: &Config,
) -> Option<BannerText> {
    let what = format!("{} {}", size_name(key.0, key.1), rate_name(rate_mhz));
    let scaled = if key.0 < out_width.min(3840) {
        " \u{b7} scaled to 4K"
    } else {
        ""
    };
    let passthrough = config.mode == OutputMode::Passthrough;
    let (title, detail) = match kind {
        Kind::Unsupported(_) => return None,
        Kind::Tunnel => (
            "Dolby Vision",
            format!("{what} \u{b7} tunnel, cinefit\u{2019}s own mapping"),
        ),
        k if is_lldv(k, config.lldv) => (
            "Dolby Vision",
            format!("{what} \u{b7} mapped by the player, shown as is"),
        ),
        Kind::Ycbcr(c) if matches!(c.transfer, Transfer::Pq | Transfer::Hlg) => (
            if c.transfer == Transfer::Pq {
                "HDR10"
            } else {
                "HLG"
            },
            if passthrough {
                format!("{what} \u{b7} to the projector")
            } else {
                format!("{what} \u{b7} tone-mapped by cinefit")
            },
        ),
        Kind::Ycbcr(_) => ("SDR", format!("{what}{scaled}")),
        Kind::Rgb(_) => ("SDR", format!("{what} \u{b7} RGB{scaled}")),
    };
    Some(BannerText {
        title: title.into(),
        detail,
    })
}

/// A source's frame rate in millihertz, from its exact fraction.
pub(crate) fn rate_mhz(f: &InputFormat) -> u32 {
    if f.frame_duration <= 0 {
        return 0;
    }
    ((f.time_scale as f64 * 1000.0) / f.frame_duration as f64).round() as u32
}

/// Why the detector does not read frames of this kind; `None` for those it
/// reads. Each is a crop change's reason and the info panel's CROP detail.
pub(crate) fn unread_by_detector(kind: Kind) -> Option<&'static str> {
    match kind {
        Kind::Ycbcr(_) => None,
        Kind::Rgb(_) => Some("the detector does not read RGB"),
        Kind::Tunnel => Some("the detector does not read the Dolby Vision tunnel"),
        Kind::Unsupported(_) => Some("nothing reads this format"),
    }
}

/// A limited-range 10-bit code as a signal value, 0 at black and 1 at
/// nominal peak.
pub(crate) fn signal(code: u16) -> f64 {
    ((f64::from(code) - 64.0) / 876.0).clamp(0.0, 1.0)
}

/// SMPTE ST 2084: a signal value (0-1) in nits.
pub(crate) fn pq_nits(e: f64) -> f64 {
    let (m1, m2) = (2610.0 / 16384.0, 2523.0 / 4096.0 * 128.0);
    let (c1, c2, c3) = (
        3424.0 / 4096.0,
        2413.0 / 4096.0 * 32.0,
        2392.0 / 4096.0 * 32.0,
    );
    let p = e.powf(1.0 / m2);
    10000.0 * ((p - c1).max(0.0) / (c2 - c3 * p)).powf(1.0 / m1)
}

/// The brightest level of the last ten seconds as the panel says it: in nits
/// where cinefit reads the input as PQ, as a share of white for SDR. For
/// low-latency Dolby Vision, against the peak the player is assumed to map
/// to (`lldv`, the setting, when it is on).
pub(crate) fn peak_words(kind: Kind, code: u16, lldv: Option<u16>) -> String {
    let as_lldv = lldv.is_some() && is_lldv(kind, true);
    let pq = as_lldv || matches!(kind, Kind::Ycbcr(c) if c.transfer == Transfer::Pq);
    if !pq {
        return format!("brightest {:.0}% of white", signal(code) * 100.0);
    }
    let nits = pq_nits(signal(code));
    match lldv {
        Some(assumed) if as_lldv && nits > f64::from(assumed) * 1.05 => {
            format!("brightest {nits:.0} nits, above the {assumed} assumed")
        }
        _ => format!("brightest {nits:.0} nits"),
    }
}

pub(crate) fn describe(kind: Kind) -> String {
    match kind {
        Kind::Ycbcr(c) => format!("YCbCr {:?} {:?} {:?}", c.matrix, c.primaries, c.transfer),
        Kind::Rgb(f) => format!("RGB {f:?}, range from input.rgb_range"),
        Kind::Tunnel => "Dolby Vision tunnel".into(),
        Kind::Unsupported(f) => format!("{f}, unsupported"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cinefit_render::Matrix;

    fn ycbcr(transfer: Transfer) -> Kind {
        Kind::Ycbcr(YcbcrColor {
            matrix: Matrix::Bt2020Nc,
            primaries: Primaries::Bt2020,
            transfer,
        })
    }

    #[test]
    fn dolby_vision_is_never_passed_through() {
        use OutputMode::{Passthrough, Sdr2020};
        let lldv = ycbcr(Transfer::Bt1886);
        let hdr10 = ycbcr(Transfer::Pq);
        for mode in [Sdr2020, Passthrough] {
            assert_eq!(
                output_in(mode, true, Kind::Tunnel, None, None),
                (None, SDR_BT2020)
            );
            assert_eq!(output_in(mode, true, lldv, None, None), (None, SDR_BT2020));
        }
        // HDR10 is passed through only when asked.
        assert_eq!(
            output_in(Sdr2020, true, hdr10, None, None),
            (None, SDR_BT2020)
        );
        let (h, out) = output_in(Passthrough, true, hdr10, None, None);
        assert_eq!(h.map(|h| h.eotf), Some(cinefit_kms::Eotf::Pq));
        assert_eq!(out.transfer, Transfer::Pq);
        // With the LLDV reading off, SDR BT.2020 is just SDR.
        assert!(!is_lldv(lldv, false));
        assert_eq!(
            output_in(Passthrough, false, lldv, None, None),
            (None, SDR_BT2020)
        );
    }

    #[test]
    fn hdr10_passes_the_sources_values_through() {
        let hdr = HdrMetadata {
            red: Some(cinefit_decklink::Chromaticity { x: 0.68, y: 0.32 }),
            green: None,
            blue: None,
            white: None,
            max_mastering: Some(1000.0),
            min_mastering: Some(0.005),
            max_cll: Some(637.0),
            max_fall: Some(148.0),
        };
        let (h, out) = output_for(ycbcr(Transfer::Pq), Some(&hdr), None);
        let h = h.expect("HDR");
        assert_eq!(h.eotf, cinefit_kms::Eotf::Pq);
        assert_eq!((h.max_cll, h.max_fall), (637.0, 148.0));
        assert_eq!((h.max_mastering, h.min_mastering), (1000.0, 0.005));
        // Sent where given, BT.2020 where not.
        assert_eq!(h.primaries[0], (0.68, 0.32));
        assert_eq!(h.primaries[1], (0.170, 0.797));
        assert_eq!(out.transfer, Transfer::Pq);
    }

    #[test]
    fn hdr10_without_metadata_signals_unknowns() {
        let (h, _) = output_for(ycbcr(Transfer::Pq), None, None);
        let h = h.expect("HDR");
        assert_eq!((h.max_cll, h.max_fall, h.max_mastering), (0.0, 0.0, 0.0));
    }

    #[test]
    fn a_tunnel_signals_pq_with_its_grade_as_mastering_and_its_scene_as_maxcll() {
        let t = Tunnel {
            color: DoviColor {
                ycc_to_lms: [[0.0; 3]; 3],
                offset: [0.0; 3],
                linear: [[0.0; 3]; 3],
            },
            source_min_pq: 62,
            source_max_pq: 3696,
            scene_max_pq: None,
            scene_avg_pq: None,
        };
        let (h, out) = output_for(Kind::Tunnel, None, Some(&t));
        let h = h.expect("HDR");
        assert_eq!(h.eotf, cinefit_kms::Eotf::Pq);
        assert!(
            (h.max_mastering - 4000.0).abs() < 10.0,
            "{}",
            h.max_mastering
        );
        assert!(h.min_mastering < 0.01);
        // No level 1: the grade's peak, never 0 ("unknown").
        assert!((h.max_cll - 4000.0).abs() < 10.0, "{}", h.max_cll);
        // No level 1, no average: MaxFALL unknown.
        assert_eq!(h.max_fall, 0.0);
        // A scene peaking at PQ 3079 (about 1000 nits) sends that as MaxCLL;
        // one brighter than the grade is capped at the grade's peak.
        let scene = |pq| Tunnel {
            scene_max_pq: Some(pq),
            ..t
        };
        let (h, _) = output_for(Kind::Tunnel, None, Some(&scene(3079)));
        assert!((h.expect("HDR").max_cll - 1001.0).abs() < 2.0);
        let (h, _) = output_for(Kind::Tunnel, None, Some(&scene(4000)));
        assert!((h.expect("HDR").max_cll - 4000.0).abs() < 10.0);
        // The scene's average, PQ 2081 (about 100 nits), goes out as MaxFALL.
        let avg = Tunnel {
            scene_avg_pq: Some(2081),
            ..scene(3079)
        };
        let (h, _) = output_for(Kind::Tunnel, None, Some(&avg));
        assert!((h.expect("HDR").max_fall - 100.0).abs() < 1.5);
        assert_eq!(
            (out.primaries, out.transfer),
            (Primaries::Bt2020, Transfer::Pq)
        );
    }

    #[test]
    fn banners_name_the_picture_and_what_happens_to_it() {
        let c = Config::default();
        let bt709 = Kind::Ycbcr(YcbcrColor {
            matrix: Matrix::Bt709,
            primaries: Primaries::Bt709,
            transfer: Transfer::Bt1886,
        });
        let b = |kind, size, rate, c: &Config| {
            banner_for(kind, size, rate, 4096, c).map(|b| format!("{} / {}", b.title, b.detail))
        };
        assert_eq!(
            b(ycbcr(Transfer::Pq), (3840, 2160), 23976, &c).as_deref(),
            Some("HDR10 / 4K 23.976 \u{b7} to the projector")
        );
        // Labelled SDR BT.2020: low-latency Dolby Vision, with input.lldv on.
        assert_eq!(
            b(ycbcr(Transfer::Bt1886), (3840, 2160), 23976, &c).as_deref(),
            Some("Dolby Vision / 4K 23.976 \u{b7} mapped by the player, shown as is")
        );
        assert_eq!(
            b(bt709, (1920, 1080), 59940, &c).as_deref(),
            Some("SDR / 1080p 59.94 \u{b7} scaled to 4K")
        );
        assert_eq!(
            b(Kind::Tunnel, (1920, 1080), 24000, &c).as_deref(),
            Some("Dolby Vision / 1080p 24 \u{b7} tunnel, cinefit\u{2019}s own mapping")
        );
        let sdr2020 = Config {
            mode: OutputMode::Sdr2020,
            ..Config::default()
        };
        assert_eq!(
            b(ycbcr(Transfer::Pq), (3840, 2160), 23976, &sdr2020).as_deref(),
            Some("HDR10 / 4K 23.976 \u{b7} tone-mapped by cinefit")
        );
        assert_eq!(
            b(
                Kind::Unsupported(PixelFormat::Other(7)),
                (3840, 2160),
                23976,
                &c
            ),
            None
        );
        assert_eq!(rate_name(50000), "50");
        assert_eq!(size_name(2560, 1440), "2560 \u{d7} 1440");
    }

    #[test]
    fn the_info_panel_says_what_comes_in_and_goes_out() {
        let key = |kind| Key {
            width: 3840,
            height: 2160,
            kind,
        };
        let lldv = Some(key(ycbcr(Transfer::Bt1886)));
        let s = input_section(lldv, None, false, Some(96), None);
        assert_eq!(
            (s.line.as_str(), s.detail.as_str()),
            ("Dolby Vision \u{b7} low latency", "3840 \u{d7} 2160")
        );
        assert_eq!(
            input_section(lldv, None, true, Some(96), None).line,
            "No signal"
        );
        assert_eq!(
            input_section(None, None, false, Some(96), None).line,
            "No signal"
        );
        // The brightest level: PQ codes in nits, against the assumed peak.
        let lldv_kind = ycbcr(Transfer::Bt1886);
        let at = |nits: f64| {
            (0..1024u16)
                .min_by(|&a, &b| {
                    (pq_nits(signal(a)) - nits)
                        .abs()
                        .total_cmp(&(pq_nits(signal(b)) - nits).abs())
                })
                .unwrap()
        };
        let at96 = peak_words(lldv_kind, at(96.0), Some(96));
        // The nearest code to 96 nits reads 95: within the assumed peak.
        assert!(at96.ends_with(" nits") && !at96.contains("above"), "{at96}");
        let over = peak_words(lldv_kind, at(200.0), Some(96));
        assert!(over.ends_with("nits, above the 96 assumed"), "{over}");
        // With input.lldv off the same signal is SDR: a share of white.
        assert!(peak_words(lldv_kind, at(96.0), None).ends_with("% of white"));
        let sdr = Kind::Ycbcr(YcbcrColor {
            matrix: Matrix::Bt709,
            primaries: Primaries::Bt709,
            transfer: Transfer::Bt1886,
        });
        assert_eq!(peak_words(sdr, 940, Some(96)), "brightest 100% of white");
        assert!((pq_nits(1.0) - 10_000.0).abs() < 1e-6);
        assert!((pq_nits(0.508) - 100.0).abs() < 2.0, "{}", pq_nits(0.508));
        // 4096x2160 at 23.976: 296,703 kHz over 5500 x 2250.
        let mode = cinefit_kms::ModeTiming {
            width: 4096,
            height: 2160,
            clock_khz: 296_703,
            htotal: 5500,
            vtotal: 2250,
            interlaced: false,
            preferred: false,
        };
        let out = output_section(lldv, false, true, follow::Signal::Sdr, mode, None, 96);
        assert_eq!(out.line, "SDR BT.2020 \u{b7} unchanged from the player");
        assert_eq!(out.detail, "4096 \u{d7} 2160 \u{b7} 23.976");
        let pq = Some(key(ycbcr(Transfer::Pq)));
        let mapped = output_section(pq, false, true, follow::Signal::Sdr, mode, None, 96);
        assert_eq!(mapped.line, "SDR BT.2020 \u{b7} tone-mapped by cinefit");
        let waiting = output_section(None, false, true, follow::Signal::Sdr, mode, None, 96);
        // What went out: SDR full scale is cinefit's white; 94 nits of it
        // is (94/96)^(1/2.4) of the scale.
        let shown = output_section(
            lldv,
            false,
            true,
            follow::Signal::Sdr,
            mode,
            Some((94.0f64 / 96.0).powf(1.0 / 2.4)),
            96,
        );
        assert!(
            shown.detail.ends_with("brightest 94 nits"),
            "{}",
            shown.detail
        );
        assert_eq!(waiting.line, "SDR BT.2020 \u{b7} black, waiting");
    }

    #[test]
    fn hlg_and_sdr() {
        let (h, out) = output_for(ycbcr(Transfer::Hlg), None, None);
        assert_eq!(h.map(|h| h.eotf), Some(cinefit_kms::Eotf::Hlg));
        assert_eq!(out.transfer, Transfer::Hlg);
        let (h, out) = output_for(ycbcr(Transfer::Bt1886), None, None);
        assert!(h.is_none());
        assert_eq!(
            (out.primaries, out.transfer),
            (Primaries::Bt709, Transfer::Bt1886)
        );
        // Plain RGB is signalled as SDR for now.
        assert!(
            output_for(Kind::Rgb(rgb::Format::R210), None, None)
                .0
                .is_none()
        );
    }
}
