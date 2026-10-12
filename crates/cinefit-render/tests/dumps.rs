//! Unpacks frames saved with `cinefit capture --dump` on the GPU and compares
//! them with the CPU's unpack, bit for bit, whatever format they were
//! captured in. They are captured content, so they never enter the
//! repository: point `CINEFIT_RENDER_DUMPS` at a dump directory to run this,
//! and it is skipped anywhere else. Needs the GPU.
//!
//!     CINEFIT_RENDER_DUMPS=DIR cargo test -p cinefit-render --test dumps -- --nocapture

#![cfg(target_os = "linux")]

use std::path::PathBuf;

use cinefit_render::{Placebo, rgb, v210};

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
            differ(
                &want.rgb,
                &gpu.download().expect("download"),
                16 - f.depth(),
            )
        };
        assert_eq!(wrong, 0, "{}: {wrong} components differ", bin.display());
    }
    println!(
        "{} {} frames unpacked bit-exact",
        frames.len(),
        dir.display()
    );
}
