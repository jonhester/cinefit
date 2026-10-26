//! Dolby's display mapping on the GPU against `dm::Frame::apply`, the
//! reference it was ported from, on a spread of BT.2020 PQ pixels and the
//! scenes that matter: 1000- and 4000-nit grades, with and without trims, at
//! 96 and 76 nits. Needs a Vulkan device; skipped, with a note, without one,
//! unless `CINEFIT_REQUIRE_GPU` is set, as on a machine that has one: there a
//! device that fails to start fails the test (#209).
//!
//!     cargo test -p cinefit-render --test dm_gpu -- --nocapture

#![cfg(target_os = "linux")]

use cinefit_render::Placebo;
use cinefit_render::dm::{Frame, Scene, Trim, ictcp_to_rgb, rgb_to_ictcp};

/// patentfit's colour difference, x720 in ICtCp with Ct halved: the measure
/// the model was fitted and scored with. A 12-bit PQ step in grey is about
/// 0.18, the model's error against Dolby about 5; the GPU's float32 PQ round
/// trip reaches two 16-bit codes in a bright channel (about 0.05).
const TOLERANCE: f64 = 0.1;

fn de(a: [f64; 3], b: [f64; 3]) -> f64 {
    720.0 * ((a[0] - b[0]).powi(2) + (0.5 * (a[1] - b[1])).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn scenes() -> Vec<(&'static str, Scene, f64)> {
    let trim = |t, c: [u16; 5]| Trim::from_codes(t, c);
    vec![
        (
            "1000-nit grade, trim at 100 nits, 96",
            Scene::from_codes(
                [62, 1200, 2950],
                [62, 3079],
                vec![trim(2081, [2100, 2000, 2060, 2048, 2100])],
            ),
            96.0,
        ),
        (
            "1000-nit grade, three trims, 76",
            Scene::from_codes(
                [0, 1360, 3079],
                [7, 3079],
                vec![
                    trim(2081, [1990, 2110, 2010, 2048, 1990]),
                    trim(2771, [2020, 2060, 2040, 2048, 2030]),
                    trim(3079, [2048; 5]),
                ],
            ),
            76.0,
        ),
        (
            "4000-nit grade, L1 max pinned, 96",
            Scene::from_codes(
                [0, 1100, 3079],
                [0, 3696],
                vec![trim(2081, [2160, 1950, 2100, 2010, 2200])],
            ),
            96.0,
        ),
        (
            "4000-nit grade, no trims, 76",
            Scene::from_codes([20, 1700, 3079], [0, 3696], vec![]),
            76.0,
        ),
    ]
}

/// Pixels across the PQ cube, saturated corners included, from a fixed
/// generator so a failure repeats.
fn pixels(n: usize) -> Vec<u16> {
    let mut s: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        (s >> 48) as u16
    };
    let mut v = Vec::with_capacity(n * 4);
    for i in 0..n {
        let rgb: [u16; 3] = match i % 4 {
            // Greys, where hue is undefined.
            0 => [next(); 3],
            // One channel near zero: saturated colour.
            1 => [next(), next() / 64, next()],
            _ => [next(), next(), next()],
        };
        v.extend_from_slice(&rgb);
        v.push(65535);
    }
    v
}

fn placebo() -> Option<Placebo> {
    match Placebo::new() {
        Ok(pl) => Some(pl),
        Err(e) if std::env::var_os("CINEFIT_REQUIRE_GPU").is_some() => {
            panic!("no Vulkan device, and CINEFIT_REQUIRE_GPU is set: {e}")
        }
        Err(e) => {
            eprintln!("skipped: no Vulkan device ({e})");
            None
        }
    }
}

#[test]
fn the_gpu_maps_as_the_reference_does() {
    let Some(pl) = placebo() else { return };
    let (w, h) = (64, 32);
    let input = pixels(w * h);
    for (name, scene, target) in scenes() {
        let frame = Frame::new(&scene, target);
        assert!(frame.lut.iter().all(|v| v.is_finite()), "{name}: table");
        let out = pl.dm_rgba16(w, h, &input, &frame).expect("GPU mapping");
        let mut worst = (0.0, String::new());
        for (px_in, px_out) in input.chunks(4).zip(out.chunks(4)) {
            let pq = |c: &[u16]| [0, 1, 2].map(|k| f64::from(c[k]) / 65535.0);
            // The model's output is checked before the conversion back to
            // RGB, whose PQ curve takes NaN to 0 (`max`), and the difference
            // after it: a NaN fails the comparison below and drops out unseen
            // (#209).
            let mapped = frame.apply(rgb_to_ictcp(pq(px_in)));
            assert!(
                mapped.iter().all(|v| v.is_finite()),
                "{name}: in {:?}, model {mapped:?}",
                &px_in[..3]
            );
            // The reference's output goes through the same 16-bit PQ
            // target, so both are compared after it.
            let want =
                ictcp_to_rgb(mapped).map(|v| (v.clamp(0.0, 1.0) * 65535.0).round() / 65535.0);
            let d = de(rgb_to_ictcp(want), rgb_to_ictcp(pq(px_out)));
            assert!(
                d.is_finite(),
                "{name}: in {:?}, reference {want:?}",
                &px_in[..3]
            );
            if d > worst.0 {
                worst = (
                    d,
                    format!(
                        "in {:?}: GPU {:?}, reference {want:?}",
                        &px_in[..3],
                        pq(px_out)
                    ),
                );
            }
        }
        eprintln!("{name}: worst difference {:.4}", worst.0);
        assert!(worst.0 < TOLERANCE, "{name}: {:.4}, {}", worst.0, worst.1);
    }
}

/// A v210 frame of `w x h` (w a multiple of 6): luma ramping across, chroma
/// varying down and across, all within limited range.
fn v210_frame(w: usize, h: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(w / 6 * 16 * h);
    for y in 0..h {
        let luma = |x: usize| (64 + x * 876 / w) as u32;
        let cb = |p: usize| (512 + (p * 13 + y * 7) % 400 - 200) as u32;
        let cr = |p: usize| (512 + (p * 29 + y * 11) % 300 - 150) as u32;
        for g in 0..w / 6 {
            let (x, p) = (g * 6, g * 3);
            let word = |a: u32, b: u32, c: u32| a | b << 10 | c << 20;
            for w in [
                word(cb(p), luma(x), cr(p)),
                word(luma(x + 1), cb(p + 1), luma(x + 2)),
                word(cr(p + 1), luma(x + 3), cb(p + 2)),
                word(luma(x + 4), cr(p + 2), luma(x + 5)),
            ] {
                out.extend_from_slice(&w.to_le_bytes());
            }
        }
    }
    out
}

/// HDR10 through the mapping (#141) is the frame converted to BT.2020 PQ RGB,
/// then the mapping pass, then placed: the same as those steps run apart.
#[test]
fn hdr10_through_the_mapping_is_the_conversion_then_the_mapping() {
    use cinefit_render::{Mapping, Matrix, Primaries, RgbColor, Transfer, View, YcbcrColor};
    let Some(pl) = placebo() else { return };
    let (w, h) = (192, 64);
    let mut planes = pl.planes(w, h).expect("planes");
    pl.unpack_v210(&mut planes, &v210_frame(w, h), w / 6 * 16)
        .expect("unpack");
    let hdr10 = YcbcrColor {
        matrix: Matrix::Bt2020Nc,
        primaries: Primaries::Bt2020,
        transfer: Transfer::Pq,
    };
    let pq = RgbColor {
        primaries: Primaries::Bt2020,
        transfer: Transfer::Pq,
    };
    let frame = Frame::new(
        &Scene::from_codes([62, 1200, 2950], [62, 3079], vec![]),
        96.0,
    );
    let flat = Mapping {
        src_peak: 1000.0,
        dst_peak: 1000.0,
        curve: cinefit_render::Curve::Bt2390,
        peak_detect: false,
        contrast_recovery: 0.0,
        peak_percentile: 0.0,
        knee_offset: 0.0,
    };

    let whole = pl.target_rgba16(w, h).expect("target");
    pl.render_ycbcr_dm(
        &planes,
        hdr10,
        &frame,
        whole.texture(),
        pq,
        View::default(),
        &flat,
    )
    .expect("render");
    let got = whole.download().expect("download");

    let apart = pl.target_rgba16(w, h).expect("target");
    pl.render_ycbcr_mapped(&planes, hdr10, apart.texture(), pq, View::default(), None)
        .expect("render");
    let rgb = apart.download().expect("download");
    let want = pl.dm_rgba16(w, h, &rgb, &frame).expect("mapping");

    let worst = got
        .chunks(4)
        .zip(want.chunks(4))
        .flat_map(|(a, b)| (0..3).map(move |c| a[c].abs_diff(b[c])))
        .max()
        .unwrap();
    // The mapping moves the picture a long way, so a match is no accident.
    let moved = rgb
        .chunks(4)
        .zip(want.chunks(4))
        .filter(|(a, b)| a[1].abs_diff(b[1]) > 1000)
        .count();
    assert!(
        moved > w * h / 4,
        "the mapping barely moved the frame: {moved}"
    );
    assert!(worst <= 4, "16-bit codes apart: {worst}");
}
