//! GPU time per pass (#219): with timing on, each pass a frame goes through
//! reports times, and only those; with it off, nothing does. The upload is
//! not required: libplacebo reports a GPU time for some uploads and not
//! others (on an RTX 3060 a 1080p v210 frame's reports none, a tunnel
//! frame's does), and the CPU stage times it either way. Needs a Vulkan
//! device; skipped, with a note, without one, unless `CINEFIT_REQUIRE_GPU`
//! is set, as on a machine that has one.

#![cfg(target_os = "linux")]

use cinefit_render::dm::{Frame, Scene};
use cinefit_render::{
    Curve, DoviColor, Mapping, Matrix, PASSES, Placebo, Primaries, RgbColor, Transfer, View,
    YcbcrColor,
};

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
fn each_pass_a_frame_takes_is_timed() {
    let Some(pl) = placebo() else { return };
    let (w, h) = (1920, 1080);
    let mut planes = pl.planes(w, h).expect("planes");
    let tgt = pl.target_rgba16(w, h).expect("target");
    let sdr = RgbColor {
        primaries: Primaries::Bt2020,
        transfer: Transfer::Bt1886,
    };
    let ycbcr = YcbcrColor {
        matrix: Matrix::Bt2020Nc,
        primaries: Primaries::Bt2020,
        transfer: Transfer::Bt1886,
    };
    let v210 = vec![0x40u8; w.div_ceil(6) * 16 * h];
    let tunnel = vec![0x55u8; w * 4 * h];
    let ycbcr_frame = |pl: &Placebo, planes: &mut _| {
        pl.unpack_v210(planes, &v210, w.div_ceil(6) * 16)
            .expect("unpack");
        pl.render_ycbcr(planes, ycbcr, tgt.texture(), sdr, None)
            .expect("render");
    };

    // Off: nothing is timed.
    ycbcr_frame(&pl, &mut planes);
    assert_eq!(pl.gpu_times().count, [0; 5], "timing off");

    if !pl.set_gpu_timing(true) {
        eprintln!("skipped: the GPU has no timers");
        return;
    }
    // Results come in a frame or two later: a few frames each way.
    for _ in 0..8 {
        ycbcr_frame(&pl, &mut planes);
    }
    let t = pl.gpu_times();
    let timed = |t: &cinefit_render::GpuTimes| -> Vec<&str> {
        (0..5)
            .filter(|&i| t.count[i] > 0)
            .map(|i| PASSES[i])
            .collect()
    };
    let passes: Vec<&str> = timed(&t).into_iter().filter(|&p| p != "upload").collect();
    assert_eq!(passes, ["unpack", "output"], "{t:?}");
    assert!(t.ms[4] > 0.0 && t.ms[4] < 1000.0, "{t:?}");

    let ictcp = [
        [1.0, 0.009, 0.111],
        [1.0, -0.009, -0.111],
        [1.0, 0.56, -0.32],
    ];
    let id = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let color = DoviColor {
        ycc_to_lms: ictcp,
        offset: [0.0, 0.5, 0.5],
        linear: id,
    };
    let frame = Frame::new(
        &Scene::from_codes([62, 1200, 2950], [62, 3079], vec![]),
        76.0,
    );
    let map = Mapping {
        src_peak: 76.0,
        dst_peak: 76.0,
        curve: Curve::Bt2390,
        peak_detect: false,
        contrast_recovery: 0.0,
        peak_percentile: 0.0,
        knee_offset: 0.0,
    };
    for _ in 0..8 {
        pl.unpack_tunnel(&mut planes, &tunnel, w * 4)
            .expect("unpack");
        pl.render_dovi_dm(
            &planes,
            &color,
            &frame,
            tgt.texture(),
            sdr,
            View::default(),
            &map,
        )
        .expect("render");
    }
    let d = pl.gpu_times();
    for (i, pass) in PASSES.iter().enumerate().skip(1) {
        assert!(
            d.count[i] > t.count[i],
            "{pass} not timed on the Dolby path: {d:?}"
        );
    }
    eprintln!(
        "upload timed: {}",
        if d.count[0] > 0 {
            "yes"
        } else {
            "no (no transfer-queue timers)"
        }
    );

    // Off again: nothing more.
    assert!(!pl.set_gpu_timing(false));
    ycbcr_frame(&pl, &mut planes);
    assert_eq!(pl.gpu_times().count, d.count, "timing off again");
}
