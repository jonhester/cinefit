//! The bar measurement against real frames: every still in the stills set,
//! read by this port and compared with the C++ version's baseline
//! (`golden.csv`: each still's class and top and bottom rows, as the C++
//! `Bars` read them). A port is faithful when nothing differs.
//!
//! The stills are copyrighted frames, never in the repository: point
//! `CINEFIT_STILLS` at the directory (it holds `stills.csv`, `golden.csv` and
//! the zstd-compressed v210 frames); skipped anywhere else. Needs `zstd`.
//!
//!     CINEFIT_STILLS=DIR cargo test --release -p cinefit-detect --test stills -- --nocapture

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

use cinefit_detect::bars::Bars;
use cinefit_detect::{Detector, V210};

fn csv(path: &std::path::Path) -> Vec<BTreeMap<String, String>> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut lines = text.lines();
    let header: Vec<String> = lines
        .next()
        .expect("header")
        .split(',')
        .map(String::from)
        .collect();
    lines
        .filter(|l| !l.is_empty())
        .map(|l| {
            header
                .iter()
                .cloned()
                .zip(l.split(',').map(String::from))
                .collect()
        })
        .collect()
}

#[test]
fn every_still_reads_as_the_cpp_version_read_it() {
    let Some(dir) = std::env::var_os("CINEFIT_STILLS").map(PathBuf::from) else {
        eprintln!("CINEFIT_STILLS not set: skipped");
        return;
    };
    let golden: BTreeMap<String, (String, u32, u32)> = csv(&dir.join("golden.csv"))
        .into_iter()
        .map(|r| {
            let n = |k: &str| r[k].parse::<u32>().expect(k);
            (
                r["file"].clone(),
                (r["class"].clone(), n("top"), n("bottom")),
            )
        })
        .collect();
    let mut bars = Bars::default();
    let (mut checked, mut differ) = (0, Vec::new());
    let started = std::time::Instant::now();
    for s in csv(&dir.join("stills.csv")) {
        let n = |k: &str| s[k].parse::<u32>().expect(k);
        let (w, h, row_bytes) = (n("width"), n("height"), n("row_bytes") as usize);
        let file = &s["file"];
        let out = Command::new("zstd")
            .arg("-dc")
            .arg(dir.join(file))
            .output()
            .expect("zstd");
        assert!(out.status.success(), "{file}: zstd failed");
        let frame = V210::new(&out.stdout, w, h, row_bytes).expect("whole frame");
        let o = bars.observe(&frame);
        let got = (o.class.name().to_string(), o.top.bar, o.bottom.bar);
        match golden.get(file) {
            Some(want) if *want == got => {}
            want => differ.push(format!("{file}: C++ {want:?}, Rust {got:?}")),
        }
        checked += 1;
    }
    println!(
        "{checked} stills in {:.1} s, {} differ from the C++ baseline",
        started.elapsed().as_secs_f64(),
        differ.len()
    );
    for d in differ.iter().take(20) {
        println!("  {d}");
    }
    assert!(checked > 0, "no stills listed");
    // A still missing from golden.csv already shows up in `differ`; this
    // catches the other direction, rows lost from stills.csv.
    assert_eq!(
        checked,
        golden.len(),
        "stills.csv and golden.csv list different numbers of stills"
    );
    assert!(differ.is_empty(), "{} stills differ", differ.len());
}

/// The whole detector's per-frame reading, a fresh detector per still, must
/// be Bars' reading: it reads through Bars, so its diagnostics name the same
/// class and edges.
#[test]
fn the_whole_detector_reads_every_still_as_bars_does() {
    let Some(dir) = std::env::var_os("CINEFIT_STILLS").map(PathBuf::from) else {
        eprintln!("CINEFIT_STILLS not set: skipped");
        return;
    };
    let golden: BTreeMap<String, (String, u32, u32)> = csv(&dir.join("golden.csv"))
        .into_iter()
        .map(|r| {
            let n = |k: &str| r[k].parse::<u32>().expect(k);
            (
                r["file"].clone(),
                (r["class"].clone(), n("top"), n("bottom")),
            )
        })
        .collect();
    let (mut checked, mut differ, mut ms) = (0, Vec::new(), 0.0);
    for s in csv(&dir.join("stills.csv")) {
        let n = |k: &str| s[k].parse::<u32>().expect(k);
        let (w, h, row_bytes) = (n("width"), n("height"), n("row_bytes") as usize);
        let file = &s["file"];
        let out = Command::new("zstd")
            .arg("-dc")
            .arg(dir.join(file))
            .output()
            .expect("zstd");
        assert!(out.status.success(), "{file}: zstd failed");
        let frame = V210::new(&out.stdout, w, h, row_bytes).expect("whole frame");
        let mut det = Detector::default();
        let t0 = std::time::Instant::now();
        let g = det.push(&frame, 0.0, 1).diag;
        ms += t0.elapsed().as_secs_f64() * 1000.0;
        let got = (
            g.frame_class.to_string(),
            g.edge_top.unwrap_or(0),
            g.edge_bottom.unwrap_or(0),
        );
        match golden.get(file) {
            Some(want) if *want == got => {}
            want => differ.push(format!("{file}: Bars {want:?}, detector {got:?}")),
        }
        checked += 1;
    }
    println!(
        "{checked} stills through the whole detector, {:.2} ms each, {} differ from Bars",
        ms / checked.max(1) as f64,
        differ.len()
    );
    for d in differ.iter().take(20) {
        println!("  {d}");
    }
    assert_eq!(checked, golden.len());
    assert!(differ.is_empty(), "{} stills differ", differ.len());
}
