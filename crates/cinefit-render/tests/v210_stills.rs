//! Unpacks real captured `v210` frames on the GPU and compares them with the
//! CPU's unpack, bit for bit. They are captured content, so they never enter
//! the repository: point `CINEFIT_V210_STILLS` at a directory of them to run
//! this, and it is skipped anywhere else. Needs the GPU.
//!
//! The directory holds zstd-compressed frames and a `stills.csv` with at least
//! `file`, `width`, `height` and `row_bytes` columns; `zstd` must be on the path.
//!
//!     CINEFIT_V210_STILLS=DIR cargo test -p cinefit-render --test v210_stills -- --nocapture

#![cfg(target_os = "linux")]

use std::path::PathBuf;
use std::process::Command;

use cinefit_render::{Placebo, v210};

#[test]
fn every_still_unpacks_bit_exact() {
    let Some(dir) = std::env::var_os("CINEFIT_V210_STILLS").map(PathBuf::from) else {
        eprintln!("CINEFIT_V210_STILLS not set: skipped");
        return;
    };
    let csv = std::fs::read_to_string(dir.join("stills.csv")).expect("stills.csv");
    let mut lines = csv.lines();
    let header: Vec<&str> = lines.next().expect("header").split(',').collect();
    let col = |name: &str| header.iter().position(|&h| h == name).expect(name);
    let (file, width, height, row_bytes) =
        (col("file"), col("width"), col("height"), col("row_bytes"));

    let pl = Placebo::new().expect("libplacebo");
    let mut planes = None;
    let mut checked = 0;
    for line in lines.filter(|l| !l.is_empty()) {
        let f: Vec<&str> = line.split(',').collect();
        let num = |i: usize| f[i].parse::<usize>().expect("number");
        let (w, h, stride) = (num(width), num(height), num(row_bytes));
        let path = dir.join(f[file]);
        let out = Command::new("zstd")
            .arg("-dc")
            .arg(&path)
            .output()
            .expect("zstd");
        assert!(out.status.success(), "{}: zstd failed", path.display());
        let bytes = out.stdout;
        assert_eq!(bytes.len(), stride * h, "{}: size", path.display());

        let want = v210::unpack(&bytes, w, h, stride).expect("CPU unpack");
        // Reused while the size holds: each frame differs from the last, so a
        // stale plane cannot pass.
        if planes
            .as_ref()
            .is_none_or(|p: &cinefit_render::GpuPlanes<'_>| (p.width(), p.height()) != (w, h))
        {
            planes = Some(pl.planes(w, h).expect("planes"));
        }
        let p = planes.as_mut().expect("planes");
        pl.unpack_v210(p, &bytes, stride).expect("GPU unpack");
        let got = p.download().expect("download");

        let differ = |want: &[u16], got: &[u16]| {
            want.iter().zip(got).filter(|&(&w, &g)| w << 6 != g).count()
                + want.len().abs_diff(got.len())
        };
        let wrong = differ(&want.luma, &got.luma) + differ(&want.chroma, &got.chroma);
        assert_eq!(wrong, 0, "{}: {wrong} samples differ", path.display());
        checked += 1;
    }
    assert!(checked > 0, "no stills listed in {}", dir.display());
    println!("{checked} stills unpacked bit-exact");
}
