//! Every safe method that hands a texture, planes or RGB source to libplacebo
//! refuses one another `Placebo` made, before the call (#199), and takes its
//! own. Needs a Vulkan device; skipped, with a note, without one, unless
//! `CINEFIT_REQUIRE_GPU` is set, as on a machine that has one.
//!
//!     cargo test -p cinefit-render --test foreign

#![cfg(target_os = "linux")]

use cinefit_render::{
    Curve, DoviColor, Error, Levels, Mapping, Matrix, Placebo, Primaries, RgbColor, RgbSourceColor,
    Transfer, View, YcbcrColor, dm, rgb,
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

fn foreign<T: std::fmt::Debug>(what: &str, r: Result<T, Error>) {
    assert!(matches!(r, Err(Error::Foreign)), "{what}: {r:?}");
}

#[test]
fn another_instances_resources_are_refused() {
    let (Some(a), Some(b)) = (placebo(), placebo()) else {
        return;
    };
    // Six pixels by two: whole v210 groups and an even tunnel width.
    let (w, h) = (6, 2);
    let target = a.target_rgba16(w, h).expect("target");
    let theirs = target.texture();
    let ours = b.target_rgba16(w, h).expect("target");
    let ours = ours.texture();
    let mut planes = a.planes(w, h).expect("planes");
    let mut src_rgb = a.rgb(w, h).expect("rgb");
    let out = RgbColor {
        primaries: Primaries::Bt2020,
        transfer: Transfer::Bt1886,
    };
    let ycbcr = YcbcrColor {
        matrix: Matrix::Bt2020Nc,
        primaries: Primaries::Bt2020,
        transfer: Transfer::Bt1886,
    };
    let rgb_color = RgbSourceColor {
        levels: Levels::Full,
        primaries: Primaries::Bt2020,
        transfer: Transfer::Bt1886,
    };
    let identity = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let dovi = DoviColor {
        ycc_to_lms: identity,
        offset: [0.0, 0.5, 0.5],
        linear: identity,
    };
    let map = Mapping {
        src_peak: 1000.0,
        dst_peak: 96.0,
        curve: Curve::Bt2390,
        peak_detect: false,
        contrast_recovery: 0.0,
        peak_percentile: 0.0,
        knee_offset: 0.0,
    };
    let frame = dm::Frame::new(
        &dm::Scene::from_codes([62, 1200, 2950], [62, 3079], vec![]),
        96.0,
    );
    let v210 = vec![0u8; 16 * h];
    let words = vec![0u8; 4 * w * h];

    foreign("clear_black", b.clear_black(theirs));
    foreign("render, source", b.render(theirs, ours, 0.0));
    foreign("render, target", b.render(ours, theirs, 0.0));
    foreign("unpack_v210", b.unpack_v210(&mut planes, &v210, 16));
    foreign(
        "unpack_v210_biased",
        b.unpack_v210_biased(&mut planes, &v210, 16, Some([0.5, 0.5])),
    );
    foreign("unpack_tunnel", b.unpack_tunnel(&mut planes, &words, 4 * w));
    foreign(
        "render_ycbcr",
        b.render_ycbcr(&planes, ycbcr, ours, out, None),
    );
    foreign(
        "render_dovi",
        b.render_dovi(&planes, &dovi, ours, out, None),
    );
    foreign(
        "render_dovi_dm",
        b.render_dovi_dm(&planes, &dovi, &frame, ours, out, View::default(), &map),
    );
    foreign(
        "unpack_rgb",
        b.unpack_rgb(&mut src_rgb, &words, 4 * w, rgb::Format::R210),
    );
    foreign(
        "render_rgb",
        b.render_rgb(&src_rgb, rgb_color, ours, out, None),
    );

    // Each instance takes what it made.
    a.clear_black(theirs).expect("own texture");
    a.unpack_v210(&mut planes, &v210, 16).expect("own planes");
    a.render_ycbcr(&planes, ycbcr, theirs, out, None)
        .expect("own planes into own texture");
    a.unpack_rgb(&mut src_rgb, &words, 4 * w, rgb::Format::R210)
        .expect("own rgb");
    a.render_rgb(&src_rgb, rgb_color, theirs, out, None)
        .expect("own rgb into own texture");
}
