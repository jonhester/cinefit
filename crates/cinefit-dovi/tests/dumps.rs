//! Decodes real frames saved with `cinefit capture --dump`. They are captured
//! content, so they never enter the repository: point `CINEFIT_DOVI_DUMPS` at
//! a dump directory to run this, and it is skipped anywhere else.
//!
//!     CINEFIT_DOVI_DUMPS=~/cinefit-dumps/dv-fel cargo test -p cinefit-dovi --test dumps -- --nocapture

use std::path::PathBuf;

/// The number after `"key": ` in a dump's JSON description.
fn json_number(json: &str, key: &str) -> usize {
    let at = json.find(&format!("\"{key}\": ")).expect(key) + key.len() + 4;
    json[at..]
        .split(|c: char| !c.is_ascii_digit())
        .next()
        .and_then(|n| n.parse().ok())
        .expect(key)
}

#[test]
fn every_dumped_frame_decodes() {
    let Some(dir) = std::env::var_os("CINEFIT_DOVI_DUMPS").map(PathBuf::from) else {
        eprintln!("CINEFIT_DOVI_DUMPS not set: skipped");
        return;
    };
    let mut frames: Vec<_> = std::fs::read_dir(&dir)
        .expect("dump directory")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "bin"))
        .collect();
    frames.sort();
    assert!(!frames.is_empty(), "no frames in {}", dir.display());

    for bin in &frames {
        let json = std::fs::read_to_string(bin.with_extension("json")).expect("json");
        assert!(
            json.contains("\"pixel_format\": \"r210\""),
            "{}: not r210",
            bin.display()
        );
        let width = json_number(&json, "width");
        let row_bytes = json_number(&json, "row_bytes");
        let bytes = std::fs::read(bin).expect("frame");

        let tunnel = cinefit_dovi::decode(cinefit_dovi::r210_parity(&bytes, width, row_bytes))
            .unwrap_or_else(|e| panic!("{}: {e}", bin.display()));
        let m = tunnel.metadata.expect("metadata");
        let l1 = m.level1.as_ref().expect("L1");
        let targets: Vec<_> = m.level2.iter().map(|b| b.target_max_pq).collect();
        println!(
            "{}: copies {:?}, source PQ {}-{}, L1 {}/{}/{}, L2 targets {:?}, L4 {}, L5 {}, other {}",
            bin.file_name().unwrap().to_string_lossy(),
            tunnel.copies_used,
            m.source_min_pq,
            m.source_max_pq,
            l1.min_pq,
            l1.max_pq,
            l1.avg_pq,
            targets,
            m.level4.is_some(),
            m.level5.is_some(),
            m.other.len(),
        );
        assert_eq!(m.signal_color_space, 2, "tunnels carry IPT");
    }
}
