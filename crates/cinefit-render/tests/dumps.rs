//! Unpacks frames saved with `cinefit capture --dump` on the GPU and compares
//! them with the CPU's unpack, bit for bit, whatever format they were
//! captured in. An `r210` frame that carries a Dolby Vision tunnel is also
//! unpacked as one and rendered with its own metadata, against the CPU's
//! conversion on every pixel that sits on a chroma sample. They are captured content, so they never enter the
//! repository: point `CINEFIT_RENDER_DUMPS` at a dump directory to run this,
//! and it is skipped anywhere else. Needs the GPU.
//!
//!     CINEFIT_RENDER_DUMPS=DIR cargo test -p cinefit-render --test dumps -- --nocapture

#![cfg(target_os = "linux")]

use std::path::PathBuf;

use cinefit_render::color::{dovi12_to_pq_rgb, pq_eotf};
use cinefit_render::{DoviColor, Placebo, Primaries, RgbColor, Transfer, rgb, tunnel, v210};

/// The number after `"key": ` in a dump's JSON description.
fn json_number(json: &str, key: &str) -> usize {
    let at = json.find(&format!("\"{key}\": ")).expect(key) + key.len() + 4;
    json[at..]
        .split(|c: char| !c.is_ascii_digit())
        .next()
        .and_then(|n| n.parse().ok())
        .expect(key)
}

/// Components that differ between the CPU's codes and the GPU's stored
/// values (`code << shift`).
fn differ(want: &[u16], got: &[u16], shift: u8) -> usize {
    want.iter()
        .zip(got)
        .filter(|&(&w, &g)| w << shift != g)
        .count()
        + want.len().abs_diff(got.len())
}

/// How far a tunnel frame's render is from the CPU's conversion, in two
/// measures that each exaggerate a different harmless case. PQ is nearly
/// vertical at 0, so in a bright pixel whose colour all but empties one
/// channel, the GPU's float rounding (about 1e-8 of full scale in light)
/// shows as up to 5e-3 in PQ. In light relative to the pixel's brightest
/// channel, near-black pixels show the 16-bit target's rounding as 1e-3. A
/// wiring error is large in both, so a channel fails only when it is off by
/// more than [`PQ_TOLERANCE`] and [`LIGHT_TOLERANCE`] at once.
struct Render {
    pq: f64,
    light: f64,
    /// Channels off by both tolerances, and the first of them.
    failed: usize,
    example: String,
}

const PQ_TOLERANCE: f64 = 3e-4;
const LIGHT_TOLERANCE: f64 = 1e-4;

/// Unpacks and renders a tunnel frame; returns samples that differ from the
/// CPU's unpack, and how far the render is from the CPU's conversion.
fn tunnel_frame(
    pl: &Placebo,
    bytes: &[u8],
    w: usize,
    h: usize,
    row_bytes: usize,
) -> Option<(usize, Render)> {
    let parity = cinefit_dovi::r210_parity(bytes, w, row_bytes);
    let tunnel = cinefit_dovi::decode(parity).ok()?;
    let c = tunnel
        .metadata?
        .conversion()
        .expect("offsets on a known scale");
    let color = DoviColor {
        ycc_to_lms: c.ycc_to_rgb,
        offset: c.offset,
        linear: c.rgb_to_lms,
    };

    let want = tunnel::unpack_r210(bytes, w, h, row_bytes).expect("CPU unpack");
    let mut planes = pl.planes(w, h).expect("planes");
    pl.unpack_tunnel(&mut planes, bytes, row_bytes)
        .expect("GPU unpack");
    let got = planes.download().expect("download");
    let wrong = differ(&want.luma, &got.luma, 4) + differ(&want.chroma, &got.chroma, 4);

    let out = RgbColor {
        primaries: Primaries::Bt2020,
        transfer: Transfer::Pq,
    };
    let target = pl.target_rgba16(w, h).expect("target");
    pl.render_dovi(&planes, &color, target.texture(), out, None)
        .expect("render");
    let rgba = target.download().expect("download");
    let mut r = Render {
        pq: 0.0,
        light: 0.0,
        failed: 0,
        example: String::new(),
    };
    // Even pixels hold the Cb of their pair, odd ones the Cr; a co-sited
    // render takes both exactly at the even pixel.
    for y in 0..h {
        for x in (0..w).step_by(2) {
            let i = y * w + x;
            let codes = [want.luma[i], want.chroma[i], want.chroma[i + 1]];
            let cpu = dovi12_to_pq_rgb(
                f64::from(codes[0]),
                f64::from(codes[1]),
                f64::from(codes[2]),
                &color,
            );
            if cpu.iter().any(|&v| !(0.0..=1.0).contains(&v)) {
                continue;
            }
            let gpu = [0, 1, 2].map(|c| f64::from(rgba[i * 4 + c]) / 65535.0);
            let (cl, gl) = (cpu.map(pq_eotf), gpu.map(pq_eotf));
            let peak = cl.iter().fold(1e-9, |m: f64, &v| m.max(v));
            for c in 0..3 {
                let pq = (cpu[c] - gpu[c]).abs();
                let light = (cl[c] - gl[c]).abs() / peak;
                r.pq = r.pq.max(pq);
                r.light = r.light.max(light);
                if pq > PQ_TOLERANCE && light > LIGHT_TOLERANCE {
                    r.failed += 1;
                    if r.example.is_empty() {
                        r.example = format!(
                            "; first: pixel {x},{y} ICtCp {codes:?}, CPU {cpu:.5?}, GPU {gpu:.5?}"
                        );
                    }
                }
            }
        }
    }
    Some((wrong, r))
}

#[test]
fn every_dumped_frame_unpacks_bit_exact() {
    let Some(dir) = std::env::var_os("CINEFIT_RENDER_DUMPS").map(PathBuf::from) else {
        eprintln!("CINEFIT_RENDER_DUMPS not set: skipped");
        return;
    };
    let mut frames: Vec<_> = std::fs::read_dir(&dir)
        .expect("dump directory")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "bin"))
        .collect();
    frames.sort();
    assert!(!frames.is_empty(), "no frames in {}", dir.display());

    let pl = Placebo::new().expect("libplacebo");
    let mut tunnels = 0;
    for bin in &frames {
        let json = std::fs::read_to_string(bin.with_extension("json")).expect("json");
        let (w, h) = (json_number(&json, "width"), json_number(&json, "height"));
        let row_bytes = json_number(&json, "row_bytes");
        let bytes = std::fs::read(bin).expect("frame");
        let format = ["v210", "r210", "8-bit ARGB"]
            .into_iter()
            .find(|f| json.contains(&format!("\"pixel_format\": \"{f}\"")))
            .unwrap_or_else(|| panic!("{}: unknown pixel format", bin.display()));

        let wrong = if format == "v210" {
            let want = v210::unpack(&bytes, w, h, row_bytes).expect("CPU unpack");
            let mut planes = pl.planes(w, h).expect("planes");
            pl.unpack_v210(&mut planes, &bytes, row_bytes)
                .expect("GPU unpack");
            let got = planes.download().expect("download");
            differ(&want.luma, &got.luma, 6) + differ(&want.chroma, &got.chroma, 6)
        } else {
            let f = if format == "r210" {
                rgb::Format::R210
            } else {
                rgb::Format::Argb8
            };
            let want = rgb::unpack(&bytes, w, h, row_bytes, f).expect("CPU unpack");
            let mut gpu = pl.rgb(w, h).expect("texture");
            pl.unpack_rgb(&mut gpu, &bytes, row_bytes, f)
                .expect("GPU unpack");
            let wrong = differ(
                &want.rgb,
                &gpu.download().expect("download"),
                16 - f.depth(),
            );
            if f == rgb::Format::R210
                && let Some((tunnel_wrong, r)) = tunnel_frame(&pl, &bytes, w, h, row_bytes)
            {
                println!(
                    "{}: tunnel, {tunnel_wrong} samples differ; render off by up to {:.1e} in PQ and {:.1e} in light, {} channels by both{}",
                    bin.file_name().unwrap().to_string_lossy(),
                    r.pq,
                    r.light,
                    r.failed,
                    r.example
                );
                assert_eq!(tunnel_wrong, 0, "{}: tunnel unpack", bin.display());
                assert_eq!(r.failed, 0, "{}: tunnel render", bin.display());
                tunnels += 1;
            }
            wrong
        };
        assert_eq!(wrong, 0, "{}: {wrong} components differ", bin.display());
    }
    println!(
        "{} {} frames unpacked bit-exact, {tunnels} of them tunnels rendered",
        frames.len(),
        dir.display()
    );
}
