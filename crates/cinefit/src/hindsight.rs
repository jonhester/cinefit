//! `cinefit hindsight`: judges a dataset session's crops shot by shot, from
//! its readings and the crop shown on each frame (#91). Reads files only, so
//! it runs anywhere; the judging itself is `cinefit_detect::hindsight`.

use std::error::Error;
use std::fmt::Write as _;
use std::path::Path;

use cinefit_detect::hindsight::{self, Shot};
use cinefit_detect::readings::{self, Reading};

/// Judges one session directory; prints a summary and the worst shots, and
/// writes `hindsight.csv` there, one line a shot.
pub fn run(session: &Path, worst: usize) -> Result<(), Box<dyn Error>> {
    let text = std::fs::read_to_string(session.join("readings.csv"))?;
    let readings: Vec<Reading> = text.lines().filter_map(readings::parse).collect();
    let shown = shown_crops(&std::fs::read_to_string(session.join("frames.csv"))?)?;
    // The readings with a frame line, in order. A session still being
    // written can have its newest readings flushed before their lines.
    let readings: Vec<Reading> = readings
        .into_iter()
        .filter(|r| shown.contains_key(&r.index))
        .collect();
    let crops: Vec<(u32, u32)> = readings.iter().map(|r| shown[&r.index]).collect();
    let shots = hindsight::judge(&readings, &crops);
    print!("{}", report(session, &shots, worst));
    let mut csv = format!("{HEADER}\n");
    for s in &shots {
        let _ = writeln!(csv, "{}", line(s));
    }
    std::fs::write(session.join("hindsight.csv"), csv)?;
    Ok(())
}

/// `hindsight.csv`'s header.
pub const HEADER: &str = "first,last,start,end,frames,truth_top,truth_bottom,verdict,over_frames,over_rows,under_frames,under_rows";

/// One shot as a `hindsight.csv` line.
pub fn line(s: &Shot) -> String {
    let opt = |v: Option<u32>| v.map_or(String::new(), |v| v.to_string());
    format!(
        "{},{},{:.3},{:.3},{},{},{},{},{},{},{},{}",
        s.first,
        s.last,
        s.start,
        s.end,
        s.frames,
        opt(s.truth.0),
        opt(s.truth.1),
        s.verdict(),
        s.over_frames,
        s.over_rows,
        s.under_frames,
        s.under_rows
    )
}

/// Judges `batch` (readings and the crop shown on each) and appends its
/// shots to `session`'s `hindsight.csv`, with the header if the file is new.
/// What `play` does at each break in the input. Returns a one-line summary.
///
/// # Errors
///
/// If the file cannot be written.
pub fn append(session: &Path, batch: &[(Reading, (u32, u32))]) -> std::io::Result<String> {
    use std::io::Write as _;
    let readings: Vec<Reading> = batch.iter().map(|(r, _)| *r).collect();
    let crops: Vec<(u32, u32)> = batch.iter().map(|(_, c)| *c).collect();
    let shots = hindsight::judge(&readings, &crops);
    let path = session.join("hindsight.csv");
    let new = !path.exists();
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    let mut text = String::new();
    if new {
        let _ = writeln!(text, "{HEADER}");
    }
    for s in &shots {
        let _ = writeln!(text, "{}", line(s));
    }
    f.write_all(text.as_bytes())?;
    Ok(summary(&shots))
}

/// A batch's shots in one line: counts, and the worst picture hidden.
pub fn summary(shots: &[Shot]) -> String {
    let count = |v: &str| shots.iter().filter(|s| s.verdict() == v).count();
    let worst = shots.iter().max_by_key(|s| (s.over_rows, s.over_frames));
    let mut out = format!(
        "{} shots: {} right, {} cropped too much ({} beyond {MINOR_ROWS} rows), {} too little ({} beyond)",
        shots.len(),
        count("right"),
        count("too much") + count("both"),
        shots.iter().filter(|s| s.over_rows > MINOR_ROWS).count(),
        count("too little") + count("both"),
        shots.iter().filter(|s| s.under_rows > MINOR_ROWS).count(),
    );
    if let Some(w) = worst.filter(|w| w.over_rows > 0) {
        let _ = write!(
            out,
            "; most hidden {} rows at {:.1}-{:.1} s",
            w.over_rows, w.start, w.end
        );
    }
    out
}

/// `frames.csv`'s crop shown, by frame index, for the frames with a reading.
fn shown_crops(text: &str) -> Result<std::collections::HashMap<u64, (u32, u32)>, Box<dyn Error>> {
    let mut out = std::collections::HashMap::new();
    let lines: Vec<&str> = text.lines().skip(1).collect();
    for (i, line) in lines.iter().enumerate() {
        let f: Vec<&str> = line.split(',').collect();
        if f.len() < 8 {
            // A session still being written ends mid-line.
            if i + 1 == lines.len() {
                break;
            }
            return Err(format!("a frames.csv line with {} fields", f.len()).into());
        }
        if f[6] == "1" {
            out.insert(f[0].parse()?, (f[4].parse()?, f[5].parse()?));
        }
    }
    Ok(out)
}

/// A mistake of at most this many rows is minor: 0.5% of a 2160-row frame,
/// within what filling the screen trims anyway.
const MINOR_ROWS: u32 = 11;

fn report(session: &Path, shots: &[Shot], worst: usize) -> String {
    let count = |v: &str| shots.iter().filter(|s| s.verdict() == v).count();
    let frames: usize = shots.iter().map(|s| s.frames).sum();
    let noticeable_over = shots.iter().filter(|s| s.over_rows > MINOR_ROWS).count();
    let noticeable_under = shots.iter().filter(|s| s.under_rows > MINOR_ROWS).count();
    let mut out = format!(
        "{}: {} shots, {} frames: {} right, {} cropped too much, {} too little, {} both, {} unknown\n  \
         beyond {MINOR_ROWS} rows (0.5% of 2160): {noticeable_over} too much, {noticeable_under} too little\n",
        session.display(),
        shots.len(),
        frames,
        count("right"),
        count("too much"),
        count("too little"),
        count("both"),
        count("unknown")
    );
    // Worst first: picture hidden, by rows times frames.
    let mut flagged: Vec<&Shot> = shots.iter().filter(|s| s.over_frames > 0).collect();
    flagged.sort_by_key(|s| std::cmp::Reverse(u64::from(s.over_rows) * s.over_frames as u64));
    for s in flagged.iter().take(worst) {
        let _ = writeln!(
            out,
            "  too much: frames {}-{} ({:.1}-{:.1} s): up to {} rows hidden on {} of {} frames; edge {:?}/{:?}",
            s.first,
            s.last,
            s.start,
            s.end,
            s.over_rows,
            s.over_frames,
            s.frames,
            s.truth.0,
            s.truth.1
        );
    }
    let mut under: Vec<&Shot> = shots.iter().filter(|s| s.under_frames > 0).collect();
    under.sort_by_key(|s| std::cmp::Reverse(u64::from(s.under_rows) * s.under_frames as u64));
    for s in under.iter().take(worst) {
        let _ = writeln!(
            out,
            "  too little: frames {}-{} ({:.1}-{:.1} s): up to {} rows of black on {} of {} frames; edge {:?}/{:?}",
            s.first,
            s.last,
            s.start,
            s.end,
            s.under_rows,
            s.under_frames,
            s.frames,
            s.truth.0,
            s.truth.1
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batches_append_to_one_file_with_one_header() {
        use cinefit_detect::bars::{FrameClass, Observation, SideObs};
        let dir =
            std::env::temp_dir().join(format!("cinefit-hindsight-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let side = |bar| SideObs {
            found: true,
            bar,
            ..SideObs::default()
        };
        let batch = |first: u64, shown: (u32, u32)| -> Vec<(Reading, (u32, u32))> {
            (0..24)
                .map(|i| {
                    (
                        Reading {
                            index: first + i,
                            time: (first + i) as f64 / 24.0,
                            epoch: 1,
                            obs: Observation {
                                class: FrameClass::Letterbox,
                                width: 3840,
                                height: 2160,
                                top: side(276),
                                bottom: side(276),
                                ..Observation::default()
                            },
                            cut: i == 0,
                        },
                        shown,
                    )
                })
                .collect()
        };
        let first = append(&dir, &batch(0, (276, 276))).unwrap();
        assert!(first.starts_with("1 shots: 1 right"), "{first}");
        let second = append(&dir, &batch(24, (300, 300))).unwrap();
        assert!(
            second.contains("1 cropped too much (1 beyond 11 rows)"),
            "{second}"
        );
        assert!(
            second.ends_with("most hidden 24 rows at 1.0-2.0 s"),
            "{second}"
        );
        let text = std::fs::read_to_string(dir.join("hindsight.csv")).unwrap();
        assert_eq!(text.lines().count(), 3);
        assert_eq!(text.lines().next(), Some(HEADER));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn frames_csv_gives_the_crop_shown_for_frames_with_a_reading() {
        let text = "index,time,epoch,content,crop_top,crop_bottom,reading,thumb\n\
                    0,0.0000,1,sdr,0,0,1,1\n\
                    1,0.0417,1,sdr,118,120,0,1\n\
                    2,0.0834,1,sdr,118,120,1,1\n";
        let m = shown_crops(text).unwrap();
        assert_eq!(m.len(), 2);
        assert_eq!(m[&2], (118, 120));
        // A half-written last line is a session in progress: skipped. One
        // anywhere else is a broken file.
        assert_eq!(
            shown_crops("h\n0,0.0,1,sdr,1,2,1,1\n1,2\n").unwrap().len(),
            1
        );
        assert!(shown_crops("h\n1,2\n0,0.0,1,sdr,1,2,1,1\n").is_err());
    }
}
