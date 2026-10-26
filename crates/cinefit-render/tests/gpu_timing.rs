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
    Curve, DoviColor, GpuResult, Mapping, Matrix, PASSES, Placebo, Primaries, RgbColor, Transfer,
    View, YcbcrColor,
};

/// Every result taken so far.
fn take(pl: &Placebo) -> Vec<GpuResult> {
    let mut out = Vec::new();
    pl.take_gpu_results(&mut out);
    out
}

fn of(results: &[GpuResult], pass: usize) -> Vec<GpuResult> {
    results.iter().filter(|r| r.pass == pass).copied().collect()
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
    assert!(take(&pl).is_empty(), "timing off");

    if !pl.set_gpu_timing(true) {
        eprintln!("skipped: the GPU has no timers");
        return;
    }
    // Results come in a frame or two later: a few frames each way, each
    // tagged, the results taken after each as play takes them.
    let mut t = Vec::new();
    for tag in 1..=8 {
        pl.set_gpu_tag(tag);
        ycbcr_frame(&pl, &mut planes);
        t.extend(take(&pl));
    }
    let mut passes: Vec<&str> = t.iter().map(|r| PASSES[r.pass]).collect();
    passes.sort_unstable();
    passes.dedup();
    passes.retain(|&p| p != "upload");
    assert_eq!(passes, ["output", "unpack"], "{t:?}");
    assert!(t.iter().all(|r| r.ms > 0.0 && r.ms < 1000.0), "{t:?}");
    // Each unpack once, under its own frame's tag; the output untagged.
    let mut tags: Vec<u64> = of(&t, 1).iter().map(|r| r.tag.expect("tagged")).collect();
    let n = tags.len();
    tags.dedup();
    assert_eq!(tags.len(), n, "an unpack result given twice: {t:?}");
    assert!(
        n >= 6 && tags.iter().all(|&g| (1..=8).contains(&g)),
        "{t:?}"
    );
    assert!(of(&t, 4).iter().all(|r| r.tag.is_none()), "{t:?}");

    // Runs whose results come in together are each kept (#263): four unpacks
    // before any result is read, then a render that waits for the GPU.
    for tag in 100..104 {
        pl.set_gpu_tag(tag);
        pl.unpack_v210(&mut planes, &v210, w.div_ceil(6) * 16)
            .expect("unpack");
    }
    pl.set_gpu_tag(104);
    pl.render_ycbcr(&planes, ycbcr, tgt.texture(), sdr, None)
        .expect("render");
    let together: Vec<u64> = of(&take(&pl), 1).iter().filter_map(|r| r.tag).collect();
    assert_eq!(together, [100, 101, 102, 103], "{together:?}");

    // libplacebo's history of a pass is 256 long: past it, still each new
    // result once (a few may still be on their way at the end).
    take(&pl);
    let mut outputs = 0;
    for tag in 200..500 {
        pl.set_gpu_tag(tag);
        ycbcr_frame(&pl, &mut planes);
        outputs += of(&take(&pl), 4).len();
    }
    assert!(
        (297..=300).contains(&outputs),
        "300 renders, {outputs} output results"
    );
    // Passes that answer lose nothing; an upload with no GPU work, as here
    // on some GPUs, shows as unanswered rather than as a result.
    let losses = pl.take_gpu_results(&mut Vec::new());
    for i in [1, 4] {
        assert_eq!(
            (losses.dropped[i], losses.untimed[i], losses.unanswered[i]),
            (0, 0, 0),
            "{}: {losses:?}",
            PASSES[i]
        );
    }

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
    let mut d = Vec::new();
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
        d.extend(take(&pl));
    }
    for (i, pass) in PASSES.iter().enumerate().skip(1) {
        assert!(
            !of(&d, i).is_empty(),
            "{pass} not timed on the Dolby path: {d:?}"
        );
    }
    eprintln!(
        "upload timed: {}",
        if of(&d, 0).is_empty() {
            "no (no transfer-queue timers)"
        } else {
            "yes"
        }
    );

    // Off again: nothing more.
    assert!(!pl.set_gpu_timing(false));
    ycbcr_frame(&pl, &mut planes);
    assert!(take(&pl).is_empty(), "timing off again");
}
