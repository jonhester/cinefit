//! Dolby's display mapping on the GPU against `dm::Frame::apply`, the
//! reference it was ported from, on a spread of BT.2020 PQ pixels and the
//! scenes that matter: 1000- and 4000-nit grades, with and without trims, at
//! 96 and 76 nits. Needs a Vulkan device; skipped, with a note, without one.
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

#[test]
fn the_gpu_maps_as_the_reference_does() {
    let pl = match Placebo::new() {
        Ok(pl) => pl,
        Err(e) => {
            eprintln!("skipped: no Vulkan device ({e})");
            return;
        }
    };
    let (w, h) = (64, 32);
    let input = pixels(w * h);
    for (name, scene, target) in scenes() {
        let frame = Frame::new(&scene, target);
        let out = pl.dm_rgba16(w, h, &input, &frame).expect("GPU mapping");
        let mut worst = (0.0, String::new());
        for (px_in, px_out) in input.chunks(4).zip(out.chunks(4)) {
            let pq = |c: &[u16]| [0, 1, 2].map(|k| f64::from(c[k]) / 65535.0);
            // The reference's output goes through the same 16-bit PQ
            // target, so both are compared after it.
            let want = ictcp_to_rgb(frame.apply(rgb_to_ictcp(pq(px_in))))
                .map(|v| (v.clamp(0.0, 1.0) * 65535.0).round() / 65535.0);
            let d = de(rgb_to_ictcp(want), rgb_to_ictcp(pq(px_out)));
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
