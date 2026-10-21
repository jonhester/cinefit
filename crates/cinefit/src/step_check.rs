//! `cinefit gpu`'s check of Dolby's step (`input.lldv_step`): a grey ramp of
//! low-latency Dolby Vision rendered one to one and stepped down to a dimmer
//! screen, each read back against the curve it should follow.

use std::error::Error;

use cinefit_render::color::{dolby_step, pq_eotf, pq_oetf};
use cinefit_render::{
    Curve, Mapping, Matrix, Placebo, Primaries, RgbColor, Transfer, View, YcbcrColor, v210,
};

/// The player's target and the screen's peak, as on the rig.
const LLDV: f32 = 96.0;
const SCREEN: f32 = 76.0;

/// The ramp's width: grey levels from black to the player's target.
const W: usize = 96;

pub fn run(pl: &Placebo) -> Result<(), Box<dyn Error>> {
    let top = pq_oetf(f64::from(LLDV) / 1e4);
    // Limited-range codes, one level a pixel; the last at the target's peak.
    let codes: Vec<u16> = (0..W)
        .map(|i| (64.0 + 876.0 * top * i as f64 / (W - 1) as f64).round() as u16)
        .collect();
    let planes = v210::Planes {
        width: W,
        height: 2,
        luma: [codes.as_slice(), codes.as_slice()].concat(),
        chroma: vec![512; W * 2],
    };
    let row = v210::min_row_bytes(W);
    let bytes = v210::pack(&planes, row).ok_or("ramp does not pack")?;
    let mut gpu = pl.planes(W, 2)?;
    pl.unpack_v210(&mut gpu, &bytes, row)?;
    let target = pl.target_rgba16(W, 2)?;

    let step = dolby_step(LLDV, SCREEN);
    let v = |u: f64| {
        let x = u.clamp(0.0, 1.0) * 16.0;
        let k = (x as usize).min(15);
        f64::from(step[k]) + (x - k as f64) * f64::from(step[k + 1] - step[k])
    };
    let pq = |n: f64| pq_oetf(n / 1e4);
    for (name, dst, stepped) in [("one to one", LLDV, false), ("stepped", SCREEN, true)] {
        pl.render_ycbcr_mapped(
            &gpu,
            YcbcrColor {
                matrix: Matrix::Bt2020Nc,
                primaries: Primaries::Bt2020,
                transfer: Transfer::Pq,
            },
            target.texture(),
            RgbColor {
                primaries: Primaries::Bt2020,
                transfer: Transfer::Bt1886,
            },
            View::placed(None),
            Some(&Mapping {
                src_peak: LLDV,
                dst_peak: dst,
                curve: Curve::Spline,
                peak_detect: false,
                contrast_recovery: 0.0,
                peak_percentile: 0.0,
                knee_offset: 0.0,
                step: stepped,
            }),
        )?;
        let px = target.download()?;
        // Light on the screen, in nits, read back and expected: SDR white is
        // the screen's peak either way.
        let mut worst = (0.0f64, 0usize);
        let mut at = Vec::new();
        for (i, &c) in codes.iter().enumerate() {
            let e = (f64::from(c) - 64.0) / 876.0;
            let input = pq_eotf(e) * 1e4;
            let want = if stepped {
                pq_eotf(v(e / top) * pq(f64::from(SCREEN))) * 1e4
            } else {
                input / f64::from(LLDV) * f64::from(SCREEN)
            };
            let got = (f64::from(px[i * 4]) / 65535.0).powf(2.4) * f64::from(SCREEN);
            // In PQ, as the eye sees it, and as 10-bit PQ codes.
            let err = (pq(got) - pq(want)).abs() * 1023.0;
            if err > worst.0 {
                worst = (err, i);
            }
            if [W / 8, W / 3, W / 2, W - 1].contains(&i) {
                at.push(format!("{input:.1} -> {got:.2} ({want:.2})"));
            }
        }
        println!(
            "lldv {name}, {LLDV} -> {SCREEN} nits on the screen: largest error {:.2} PQ codes at {}; \
             nits in -> out (expected): {}",
            worst.0,
            worst.1,
            at.join(", ")
        );
        if worst.0 > 2.0 {
            return Err(format!("lldv {name} render is off its curve").into());
        }
    }
    Ok(())
}
