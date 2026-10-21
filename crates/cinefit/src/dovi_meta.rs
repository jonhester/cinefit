//! `cinefit dovi SESSION`: the Dolby Vision metadata of a session's kept
//! tunnel frames (#109), so a pairing pass's frames can be fitted with what
//! the player's engine had to work with: the grade's range, each shot's
//! brightness (level 1), the colourist's trims for each target display
//! (level 2), and the rest. Writes `dovi.csv` into the session, one line a
//! frame, and prints a summary. Reads the kept frames one at a time.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::Write as _;
use std::path::Path;

use cinefit_dovi::{pq_to_nits, r210_parity};

/// A level-2 trim value that changes nothing: the 12-bit codes' centre.
const NEUTRAL: u16 = 2048;

fn nits(code: u16) -> String {
    format!("{:.2}", pq_to_nits(code))
}

pub fn run(session: &Path) -> Result<(), Box<dyn Error>> {
    let frames = session.join("frames");
    let index = crate::record::read_index(&frames)?;
    let mut csv = String::from(
        "index,scene_refresh,source_min_nits,source_max_nits,l1_min_nits,l1_avg_nits,l1_max_nits,\
         l2 (target_nits:slope/offset/power/chroma/saturation/ms_weight; ...),l4 (anchor_pq/power),other_levels\n",
    );
    let (mut read, mut decoded) = (0usize, 0usize);
    // Each trim target, how many frames carried it, and how many of those
    // moved any trim from neutral.
    let mut targets: BTreeMap<u16, (usize, usize)> = BTreeMap::new();
    let mut ranges: BTreeMap<(u16, u16), usize> = BTreeMap::new();
    let mut other: BTreeMap<u8, usize> = BTreeMap::new();
    let mut l1_max = (u16::MAX, 0u16);
    let lost = crate::record::for_each_frame::<std::io::Error>(
        &frames,
        &index,
        |f| f.pixel_format == "r210",
        |f, bytes| {
            read += 1;
            let Ok(t) = cinefit_dovi::decode(r210_parity(bytes, f.width, f.row_bytes)) else {
                return Ok(());
            };
            let Some(m) = t.metadata else {
                return Ok(());
            };
            decoded += 1;
            *ranges
                .entry((m.source_min_pq, m.source_max_pq))
                .or_default() += 1;
            let l1 = m.level1.as_ref().map_or_else(
                || ",,".to_string(),
                |l| {
                    l1_max = (l1_max.0.min(l.max_pq), l1_max.1.max(l.max_pq));
                    format!("{},{},{}", nits(l.min_pq), nits(l.avg_pq), nits(l.max_pq))
                },
            );
            let l2: Vec<String> = m
                .level2
                .iter()
                .map(|b| {
                    let moved = [
                        b.trim_slope,
                        b.trim_offset,
                        b.trim_power,
                        b.trim_chroma_weight,
                        b.trim_saturation_gain,
                    ]
                    .iter()
                    .any(|&v| v != NEUTRAL);
                    let e = targets.entry(b.target_max_pq).or_default();
                    e.0 += 1;
                    e.1 += usize::from(moved);
                    format!(
                        "{}:{}/{}/{}/{}/{}/{}",
                        nits(b.target_max_pq),
                        b.trim_slope,
                        b.trim_offset,
                        b.trim_power,
                        b.trim_chroma_weight,
                        b.trim_saturation_gain,
                        b.ms_weight
                    )
                })
                .collect();
            let l4 = m.level4.as_ref().map_or_else(String::new, |l| {
                format!("{}/{}", l.anchor_pq, l.anchor_power)
            });
            for (level, _) in &m.other {
                *other.entry(*level).or_default() += 1;
            }
            let levels: Vec<String> = m
                .other
                .iter()
                .map(|(l, p)| format!("L{l}:{}B", p.len()))
                .collect();
            let _ = writeln!(
                csv,
                "{},{},{},{},{l1},{},{l4},{}",
                f.index,
                u8::from(m.scene_refresh),
                nits(m.source_min_pq),
                nits(m.source_max_pq),
                l2.join("; "),
                levels.join(" ")
            );
            Ok(())
        },
    )?;
    let out = session.join("dovi.csv");
    std::fs::write(&out, csv)?;

    println!("{read} tunnel frames read, {decoded} with metadata");
    if lost > 0 {
        println!("the stream was cut short: the last {lost} frames of the index are missing");
    }
    for ((lo, hi), n) in &ranges {
        println!(
            "grade's range {:.4}-{:.0} nits on {n} frames",
            pq_to_nits(*lo),
            pq_to_nits(*hi)
        );
    }
    if l1_max.1 > 0 {
        println!(
            "level 1 shot peaks from {:.1} to {:.0} nits",
            pq_to_nits(l1_max.0),
            pq_to_nits(l1_max.1)
        );
    }
    if targets.is_empty() {
        println!("no level 2 trims");
    }
    for (t, (n, moved)) in &targets {
        println!(
            "level 2 trims for {:.0} nits on {n} frames, moved from neutral on {moved}",
            pq_to_nits(*t)
        );
    }
    for (l, n) in &other {
        println!("level {l} on {n} frames (kept raw)");
    }
    println!("written: {}", out.display());
    Ok(())
}
