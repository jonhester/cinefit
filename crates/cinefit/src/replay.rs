//! `cinefit replay`: recorded clips through the bar detector, with no capture
//! card or display. For each piece: every crop change with its frame number,
//! time and reason, how long each zoom-in took from the first frame that
//! showed the new height, the time spent at each height, and what the
//! detector cost per frame on real content, timed apart from reading and
//! decompressing the clip. With `--compare`, every frame's crop against the
//! C++ version's replay log of the same piece.
//!
//! Clips are the C++ version's (`frames.v210.zst`) or ours (`frames.zst`).
//! A trimmed piece, `<clip>.<start>-<end>`, keeps the original clip's frame
//! numbers and times. Each piece starts with a fresh detector, as the C++
//! replay does without `--join`, and a change of frame format inside a piece
//! is a new epoch, as in `play`.
//!
//! The readings tier: `--save-readings DIR` also writes what the bar reader
//! and the cut detector saw on each frame ([`readings`]), and checks that the
//! policy replaying them, as written, gives every frame the crop the pixels
//! did. `--readings` then replays those files through the policy alone, a
//! whole clip in milliseconds, with `--set KEY=VALUE` to try a policy
//! parameter. The C++ version's saved readings replay too.
//!
//! Clips and anything derived from them are copyrighted footage: they stay on
//! the machine, outside any repository. A pixel replay is disk- and
//! zstd-bound and takes minutes per clip; do not run one beside a recording
//! or a benchmark.

use std::collections::BTreeMap;
use std::error::Error;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use cinefit_detect::ar_policy::{ArPolicy, Params};
use cinefit_detect::readings::{self, Reading};
use cinefit_detect::{Crop, Decision, Detector, V210};

use crate::record;

pub struct Options {
    /// Clip directories, or with `readings` readings files.
    pub pieces: Vec<PathBuf>,
    /// Also write each piece's readings to `<dir>/<piece>.csv`.
    pub save_readings: Option<PathBuf>,
    /// The pieces are readings files: the policy alone, no pixels.
    pub readings: bool,
    /// Policy parameters for a readings replay, `KEY=VALUE`.
    pub sets: Vec<String>,
    /// A directory of the C++ version's replay logs, `<piece>.cinefit.csv`.
    pub compare: Option<PathBuf>,
    /// Write every frame's decision to `<dir>/<piece>.rust.csv`, in the C++
    /// log's columns.
    pub log: Option<PathBuf>,
    /// Summaries only, no list of crop changes.
    pub quiet: bool,
}

/// One crop change.
#[derive(Debug, Clone, PartialEq)]
struct Switch {
    frame: u64,
    time: f64,
    from: Crop,
    to: Crop,
    reason: &'static str,
    /// Zoom-ins only: frames from the first frame since the previous change
    /// whose measured edges matched the new crop (within 2 rows) to the
    /// change. `None` when no frame showed them.
    latency: Option<u64>,
}

/// Follows one detector's decisions over a piece.
#[derive(Default)]
struct Tracker {
    crop: Crop,
    switches: Vec<Switch>,
    /// Since the last change: (frame, measured top, measured bottom).
    edges: Vec<(u64, Option<u32>, Option<u32>)>,
}

impl Tracker {
    fn push(&mut self, frame: u64, time: f64, d: &Decision) {
        self.edges
            .push((frame, d.diag.edge_top, d.diag.edge_bottom));
        if d.crop == self.crop {
            return;
        }
        let zoom_in = d.crop.top + d.crop.bottom > self.crop.top + self.crop.bottom;
        let near = |e: Option<u32>, c: u32| e.is_some_and(|e| e.abs_diff(c) <= 2);
        let latency = zoom_in
            .then(|| {
                self.edges
                    .iter()
                    .find(|(_, t, b)| near(*t, d.crop.top) && near(*b, d.crop.bottom))
                    .map(|(n, _, _)| frame - n)
            })
            .flatten();
        self.switches.push(Switch {
            frame,
            time,
            from: self.crop,
            to: d.crop,
            reason: d.diag.reason,
            latency,
        });
        self.crop = d.crop;
        self.edges.clear();
    }
}

/// Median, 95th percentile and largest of a sample, sorting it.
fn spread(v: &mut [f64]) -> Option<(f64, f64, f64)> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    let at = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize];
    Some((at(0.5), at(0.95), v[v.len() - 1]))
}

/// The C++ version's decisions for a piece, by time in microseconds.
fn cpp_log(path: &Path) -> Result<BTreeMap<i64, (Crop, String)>, Box<dyn Error>> {
    let text = std::fs::read_to_string(path)?;
    let mut lines = text.lines();
    let header: Vec<&str> = lines.next().unwrap_or("").split(',').collect();
    let col = |name: &str| {
        header
            .iter()
            .position(|h| *h == name)
            .ok_or_else(|| format!("{}: no {name} column", path.display()))
    };
    let (time, top, bottom, reason) = (
        col("time")?,
        col("crop_top")?,
        col("crop_bottom")?,
        col("reason")?,
    );
    let mut out = BTreeMap::new();
    for line in lines.filter(|l| !l.is_empty()) {
        let f: Vec<&str> = line.split(',').collect();
        let get = |i: usize| f.get(i).copied().unwrap_or("");
        let t: f64 = get(time).parse()?;
        out.insert(
            micros(t),
            (
                Crop {
                    top: get(top).parse()?,
                    bottom: get(bottom).parse()?,
                },
                get(reason).to_string(),
            ),
        );
    }
    Ok(out)
}

fn micros(t: f64) -> i64 {
    (t * 1e6).round() as i64
}

/// What one piece came to.
#[derive(Default)]
struct Piece {
    frames: u64,
    skipped: u64,
    compared: u64,
    differ: u64,
    switches: usize,
    /// The detector's time per frame, milliseconds.
    detect_ms: Vec<f64>,
    seconds: f64,
    /// Frames where the policy over the saved readings, as written, gave
    /// another crop than the pixels did.
    unfaithful: u64,
}

pub fn run(o: Options) -> Result<(), Box<dyn Error>> {
    if o.pieces.is_empty() {
        return Err("no clips given".into());
    }
    if !o.sets.is_empty() && !o.readings {
        return Err(
            "--set applies to --readings replays only: the pixel tier runs the policy as built"
                .into(),
        );
    }
    if o.readings && o.save_readings.is_some() {
        return Err("--save-readings needs clips, not readings".into());
    }
    let mut params = Params::default();
    for kv in &o.sets {
        let (k, v) = kv
            .split_once('=')
            .ok_or_else(|| format!("--set {kv}: KEY=VALUE"))?;
        params.set(k.trim(), v.trim())?;
    }
    for dir in [&o.log, &o.save_readings].into_iter().flatten() {
        std::fs::create_dir_all(dir)?;
    }
    let mut all = Piece::default();
    for dir in &o.pieces {
        let p = if o.readings {
            // A file with no readings (a header alone) is said and passed
            // over, as the C++ version does: one bad file in a glob should
            // not stop the rest.
            match replay_readings(dir, &o, params)? {
                Some(p) => p,
                None => {
                    println!("{}: no readings, skipped", dir.display());
                    continue;
                }
            }
        } else {
            piece(dir, &o)?
        };
        all.unfaithful += p.unfaithful;
        all.frames += p.frames;
        all.skipped += p.skipped;
        all.compared += p.compared;
        all.differ += p.differ;
        all.switches += p.switches;
        all.seconds += p.seconds;
        all.detect_ms.extend(p.detect_ms);
    }
    if o.pieces.len() > 1 {
        println!(
            "\nall {} pieces: {} frames, {} crop changes{}",
            o.pieces.len(),
            all.frames,
            all.switches,
            if all.skipped > 0 {
                format!(", {} frames not v210 skipped", all.skipped)
            } else {
                String::new()
            }
        );
        if let Some((m, p95, max)) = spread(&mut all.detect_ms) {
            println!("detector: median {m:.2} ms, p95 {p95:.2}, max {max:.2} per frame");
        }
        if o.compare.is_some() {
            println!(
                "against C++: {} of {} frames differ",
                all.differ, all.compared
            );
        }
        if o.save_readings.is_some() {
            println!(
                "readings replay: {} frames differ from the pixel run{}",
                all.unfaithful,
                if all.unfaithful > 0 {
                    "  -- NOT FAITHFUL"
                } else {
                    ""
                }
            );
        }
    }
    if all.unfaithful > 0 {
        return Err("the saved readings do not reproduce the pixel run".into());
    }
    Ok(())
}

fn name_of(dir: &Path) -> String {
    dir.file_name().map_or_else(
        || dir.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// One piece's replay, whatever feeds it decisions: the comparison with the
/// C++ version's log, the per-frame log, the tracker and the counts.
struct Run {
    name: String,
    cpp: Option<BTreeMap<i64, (Crop, String)>>,
    log: Option<BufWriter<File>>,
    tracker: Tracker,
    p: Piece,
    first_diff: Option<(u64, Crop, String, Crop, &'static str)>,
}

impl Run {
    fn new(name: String, o: &Options) -> Result<Self, Box<dyn Error>> {
        let cpp = match &o.compare {
            Some(d) => {
                let path = d.join(format!("{name}.cinefit.csv"));
                if path.exists() {
                    Some(cpp_log(&path)?)
                } else {
                    println!("{name}: no C++ log at {}", path.display());
                    None
                }
            }
            None => None,
        };
        let log = match &o.log {
            Some(d) => {
                let mut w = BufWriter::new(File::create(d.join(format!("{name}.rust.csv")))?);
                writeln!(
                    w,
                    "index,time,class,edge_top,edge_bottom,crop_top,crop_bottom,candidate_top,candidate_bottom,evidence,threshold,reason"
                )?;
                Some(w)
            }
            None => None,
        };
        Ok(Self {
            name,
            cpp,
            log,
            tracker: Tracker::default(),
            p: Piece::default(),
            first_diff: None,
        })
    }

    /// One frame's decision.
    fn step(&mut self, frame: u64, time: f64, d: &Decision) -> Result<(), Box<dyn Error>> {
        self.p.frames += 1;
        self.tracker.push(frame, time, d);
        if let Some(cpp) = &self.cpp
            && let Some((c, reason)) = cpp.get(&micros(time))
        {
            self.p.compared += 1;
            if *c != d.crop {
                self.p.differ += 1;
                if self.first_diff.is_none() {
                    self.first_diff = Some((frame, *c, reason.clone(), d.crop, d.diag.reason));
                }
            }
        }
        if let Some(w) = self.log.as_mut() {
            let opt = |v: Option<u32>| v.map_or(-1, i64::from);
            let c = d.diag.candidate;
            writeln!(
                w,
                "{},{:.6},{},{},{},{},{},{},{},{:.3},{:.3},{}",
                frame,
                time,
                d.diag.frame_class,
                opt(d.diag.edge_top),
                opt(d.diag.edge_bottom),
                d.crop.top,
                d.crop.bottom,
                c.map_or(-1, |c| i64::from(c.crop.top)),
                c.map_or(-1, |c| i64::from(c.crop.bottom)),
                c.map_or(0.0, |c| c.evidence),
                c.map_or(0.0, |c| c.threshold),
                if d.changed { d.diag.reason } else { "" }
            )?;
        }
        Ok(())
    }

    /// The report. `span` is the piece's first and last time, `size` its
    /// frame size, `head` what follows the frame count on the first line.
    fn finish(
        mut self,
        policy: &ArPolicy,
        span: (f64, f64),
        size: (usize, usize),
        head: &str,
        quiet: bool,
    ) -> Result<Piece, Box<dyn Error>> {
        if let Some(mut w) = self.log.take() {
            w.flush()?;
        }
        let p = &mut self.p;
        p.seconds = span.1 - span.0;
        p.switches = self.tracker.switches.len();
        println!(
            "{}: {} frames, {:.1} s of content ({:.2}-{:.2} s){head}",
            self.name, p.frames, p.seconds, span.0, span.1
        );
        let (w, h) = size;
        if !quiet {
            for s in &self.tracker.switches {
                println!(
                    "  frame {} at {:.3} s: {}/{} -> {}/{} ({}), {:.3}:1{}",
                    s.frame,
                    s.time,
                    s.from.top,
                    s.from.bottom,
                    s.to.top,
                    s.to.bottom,
                    s.reason,
                    w as f64 / f64::from(s.to.active(h as u32).max(1)),
                    s.latency.map_or_else(String::new, |l| format!(
                        ", {l} frames after the height first showed"
                    ))
                );
            }
        }
        let mut latencies: Vec<f64> = self
            .tracker
            .switches
            .iter()
            .filter_map(|s| s.latency.map(|l| l as f64))
            .collect();
        let zoom_ins = latencies.len();
        let mut line = format!("  {} crop changes", p.switches);
        if let Some((m, _, max)) = spread(&mut latencies) {
            line += &format!(
                "; {zoom_ins} zoom-ins, from the height first showing: median {m:.0}, max {max:.0} frames"
            );
        }
        println!("{line}");
        let shares: Vec<String> = policy
            .shares()
            .iter()
            .map(|s| format!("{} rows {:.1} s", s.rows, s.seconds))
            .collect();
        if !shares.is_empty() {
            println!("  time at each height: {}", shares.join(", "));
        }
        if self.cpp.is_some() {
            print!(
                "  against C++: {} of {} frames differ",
                p.differ, p.compared
            );
            if let Some((n, c, cr, r, rr)) = &self.first_diff {
                print!(
                    "; first at frame {n}: C++ {}/{} ({cr}), Rust {}/{} ({rr})",
                    c.top, c.bottom, r.top, r.bottom
                );
            }
            println!();
        }
        Ok(self.p)
    }
}

/// A piece's pixels through the whole detector.
fn piece(dir: &Path, o: &Options) -> Result<Piece, Box<dyn Error>> {
    let name = name_of(dir);
    let index = record::read_index(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut run = Run::new(name.clone(), o)?;
    let mut saved = match &o.save_readings {
        Some(d) => {
            let mut w = BufWriter::new(File::create(d.join(format!("{name}.csv")))?);
            writeln!(w, "{}", readings::header())?;
            Some(w)
        }
        None => None,
    };
    // The policy alone over each reading as written, in step with the
    // pixels: a reading that loses what the policy needs (to rounding, say)
    // shows here as a frame whose crop differs.
    let mut lockstep = ArPolicy::default();

    let mut det = Detector::default();
    let (mut skipped, mut detect_ms, mut unfaithful) = (0u64, Vec::new(), 0u64);
    let (mut epoch, mut format) = (1u32, None);
    let started = Instant::now();
    let lost = record::for_each_frame::<Box<dyn Error>>(
        dir,
        &index,
        |_| true,
        |f, bytes| {
            let this = (f.width, f.height, f.pixel_format.clone());
            if format.as_ref().is_some_and(|x| *x != this) {
                epoch += 1;
            }
            format = Some(this);
            let v = (f.pixel_format == "v210")
                .then(|| V210::new(bytes, f.width as u32, f.height as u32, f.row_bytes))
                .flatten();
            let Some(v) = v else {
                skipped += 1;
                return Ok(());
            };
            let t = Instant::now();
            let (d, obs, cut) = det.push_reading(&v, f.time, epoch);
            detect_ms.push(t.elapsed().as_secs_f64() * 1000.0);
            run.step(f.frame_number, f.time, &d)?;
            if let Some(w) = saved.as_mut() {
                let line = readings::format(&Reading {
                    index: f.frame_number,
                    time: f.time,
                    epoch,
                    obs,
                    cut,
                });
                writeln!(w, "{line}")?;
                let back = readings::parse(&line).ok_or("a reading that does not read back")?;
                if lockstep
                    .push(&back.obs, back.cut, back.time, back.epoch)
                    .crop
                    != d.crop
                {
                    unfaithful += 1;
                }
            }
            Ok(())
        },
    )?;
    let wall = started.elapsed().as_secs_f64();
    if let Some(mut w) = saved {
        w.flush()?;
    }
    let span = (
        index.first().map_or(0.0, |f| f.time),
        index.last().map_or(0.0, |f| f.time),
    );
    let size = index.first().map_or((3840, 2160), |f| (f.width, f.height));
    let mut head = if skipped > 0 {
        format!(", {skipped} frames not v210 skipped (no adapter yet)")
    } else {
        String::new()
    };
    if lost > 0 {
        head += &format!(", the stream cut short {lost} frames before the index ends");
    }
    let frames = run.p.frames;
    let mut p = run.finish(det.policy(), span, size, &head, o.quiet)?;
    p.skipped = skipped;
    p.unfaithful = unfaithful;
    let detect_total: f64 = detect_ms.iter().sum::<f64>() / 1000.0;
    if let Some((m, p95, max)) = spread(&mut detect_ms.clone()) {
        println!(
            "  detector: median {m:.2} ms, p95 {p95:.2}, max {max:.2} per frame; reading the clip {:.0} frames/s",
            frames as f64 / (wall - detect_total).max(1e-9)
        );
    }
    p.detect_ms = detect_ms;
    if o.save_readings.is_some() {
        println!(
            "  readings replay: {unfaithful} frames differ from the pixel run{}",
            if unfaithful > 0 {
                "  -- NOT FAITHFUL"
            } else {
                ""
            }
        );
    }
    Ok(p)
}

/// A readings file through the policy alone.
fn replay_readings(
    path: &Path,
    o: &Options,
    params: Params,
) -> Result<Option<Piece>, Box<dyn Error>> {
    let name = path.file_stem().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let rs: Vec<Reading> = text.lines().filter_map(readings::parse).collect();
    let (Some(first), Some(last)) = (rs.first(), rs.last()) else {
        return Ok(None);
    };
    let mut run = Run::new(name, o)?;
    let mut policy = ArPolicy::new(params);
    let started = Instant::now();
    for r in &rs {
        let d = policy.push(&r.obs, r.cut, r.time, r.epoch);
        run.step(r.index, r.time, &d)?;
    }
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    run.finish(
        &policy,
        (first.time, last.time),
        (last.obs.width as usize, last.obs.height as usize),
        &format!(", policy alone in {ms:.1} ms"),
        o.quiet,
    )
    .map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cinefit_detect::Diagnostics;

    fn decision(crop: (u32, u32), edges: Option<(u32, u32)>, reason: &'static str) -> Decision {
        Decision {
            crop: Crop {
                top: crop.0,
                bottom: crop.1,
            },
            changed: false,
            diag: Diagnostics {
                edge_top: edges.map(|e| e.0),
                edge_bottom: edges.map(|e| e.1),
                reason,
                ..Diagnostics::default()
            },
        }
    }

    #[test]
    fn a_zoom_in_counts_frames_from_the_height_first_showing() {
        let mut t = Tracker::default();
        let push = |t: &mut Tracker, n: u64, d: &Decision| t.push(n, n as f64 / 24.0, d);
        push(&mut t, 0, &decision((0, 0), None, ""));
        push(&mut t, 1, &decision((0, 0), Some((140, 140)), ""));
        // The scope height shows from frame 2, within 2 rows.
        push(&mut t, 2, &decision((0, 0), Some((279, 281)), ""));
        push(&mut t, 3, &decision((0, 0), Some((280, 279)), ""));
        push(
            &mut t,
            7,
            &decision((280, 279), Some((280, 279)), "zoom in (new)"),
        );
        // Zooming out has no latency.
        push(&mut t, 9, &decision((0, 0), Some((0, 0)), "zoom out"));
        assert_eq!(t.switches.len(), 2);
        assert_eq!(t.switches[0].latency, Some(5));
        assert_eq!(t.switches[0].reason, "zoom in (new)");
        assert_eq!(t.switches[1].latency, None);
        assert_eq!(
            t.switches[1].from,
            Crop {
                top: 280,
                bottom: 279
            }
        );
    }

    #[test]
    fn spread_is_median_p95_and_max() {
        let mut v: Vec<f64> = (1..=100).map(f64::from).rev().collect();
        assert_eq!(spread(&mut v), Some((51.0, 95.0, 100.0)));
        assert_eq!(spread(&mut []), None);
    }
}
