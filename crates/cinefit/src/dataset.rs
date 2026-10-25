//! The detector's dataset, recorded from normal viewing (#91): for every frame
//! `play` shows, what the detector measured (its readings, in the readings
//! tier's format, #63) and a small fingerprint of the picture. A session is a
//! directory under `~/cinefit-clips/caught/`, with the private footage, never
//! in a repository:
//!
//! - `readings.csv`: the readings tier's CSV, so `cinefit replay --readings`
//!   re-runs the policy over a whole evening's viewing in milliseconds;
//! - `frames.csv`: one line a frame: its index, time, epoch, what the input
//!   was (`Content`), the crop shown, and whether it has a reading and a
//!   fingerprint;
//! - `thumbs.bin`: a 32 x 18 luma thumbnail (8-bit, rows in order) for each
//!   frame `frames.csv` marks, in the same order: enough to find a moment
//!   again and to align two viewings of a title, far too small to judge bars.
//!
//! Writing happens on its own thread; the frame loop only hands over a
//! record, and drops it (counted) rather than wait if the writer falls behind.
//!
//! Full frames only where they teach something (#91 part 2), chosen by a
//! [`Picker`]: a crop change and the two frames after it, a crop the policy
//! is considering but has not adopted (at most one every few seconds), and a
//! random sample, about one frame in five minutes, for an unbiased set. They
//! go into `frames/`, a clip as `cinefit capture --record` writes them (the
//! raw captured bytes through zstd, `frames.csv` its index), numbered as the
//! readings are, with `kept.csv` saying why each was kept. A disk budget
//! (`detect.frames_gb`) bounds the kept frames of every session together:
//! the oldest sessions' are deleted first, and a session that alone
//! reaches it keeps no more.

use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::thread::JoinHandle;
use std::time::Duration;

use log::{info, warn};

use crate::record::ClipWriter;

use cinefit_decklink::FrameInfo;
use cinefit_detect::Samples;
use cinefit_detect::bars::Observation;
use cinefit_detect::readings::{self, Reading};

/// The fingerprint's size: 32 x 18, the picture's shape at 16:9.
pub const THUMB_W: usize = 32;
pub const THUMB_H: usize = 18;
pub const THUMB: usize = THUMB_W * THUMB_H;

/// Records queued before the writer is considered behind: about four
/// seconds at 60 Hz.
const QUEUE: usize = 256;

/// One frame's record.
pub struct Record {
    pub index: u64,
    pub time: f64,
    pub epoch: u32,
    /// What the input was (`Content::tag`).
    pub content: &'static str,
    /// The crop shown on this frame: rows of bar, top and bottom.
    pub crop: (u32, u32),
    /// What the detector measured, and whether it saw a cut; `None` where it
    /// did not run (input it cannot read, or cropping off).
    pub reading: Option<(Observation, bool)>,
    pub thumb: Option<[u8; THUMB]>,
    /// A hard cut on this frame, from whichever detector read it: the
    /// tunnel's too, which has no reading (#109).
    pub cut: bool,
}

/// A small luma thumbnail of `f`: each cell the mean of a grid of samples in
/// its block, as an 8-bit level (limited range: code 64 is 0, 940 is 255).
pub fn thumbnail(f: &dyn Samples) -> [u8; THUMB] {
    let (w, h) = (f.width() as usize, f.height() as usize);
    let mut out = [0u8; THUMB];
    if w < THUMB_W || h < THUMB_H {
        return out;
    }
    let mut row = vec![0u16; w];
    // Four rows a cell and every eighth pixel across: about 9,000 samples
    // for a 4K frame, a fraction of a millisecond on the detector's thread.
    const ROWS: usize = 4;
    for cy in 0..THUMB_H {
        let mut sums = [0u32; THUMB_W];
        let mut counts = [0u32; THUMB_W];
        for r in 0..ROWS {
            let y = (cy * h + (2 * r + 1) * h / (2 * ROWS)) / THUMB_H;
            f.luma_row(y as u32, &mut row);
            for (x, &v) in row.iter().enumerate().step_by(8) {
                let cx = x * THUMB_W / w;
                sums[cx] += u32::from(v);
                counts[cx] += 1;
            }
        }
        for cx in 0..THUMB_W {
            let mean = f64::from(sums[cx]) / f64::from(counts[cx].max(1));
            out[cy * THUMB_W + cx] =
                ((mean - 64.0) * 255.0 / 876.0).round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

/// Adds a line to `session.txt` in session `dir`, from any thread.
pub fn note_in(dir: &Path, line: &str) {
    if let Ok(mut f) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("session.txt"))
    {
        let _ = writeln!(f, "{line}");
    }
}

/// The thumbnails of frames `lo..=hi` of session `dir`, from its
/// `frames.csv` and `thumbs.bin`, in order: `None` for a frame without one or
/// not on disk yet. The files are written through buffers, so the newest
/// frames are missing and the last line may be cut short; it is ignored.
pub fn read_thumbs(dir: &Path, lo: u64, hi: u64) -> io::Result<Vec<Option<[u8; THUMB]>>> {
    let csv = fs::read_to_string(dir.join("frames.csv"))?;
    // Each frame's place in thumbs.bin: the frames marked before it.
    let mut want = Vec::new();
    let mut n = 0u64;
    for line in csv.lines().skip(1) {
        let f: Vec<&str> = line.split(',').collect();
        let (Some(index), 9) = (f[0].parse::<u64>().ok(), f.len()) else {
            break;
        };
        if index > hi {
            break;
        }
        if f[7] == "1" {
            if index >= lo {
                want.push(((index - lo) as usize, n));
            }
            n += 1;
        }
    }
    let mut out = vec![None; (hi - lo + 1) as usize];
    let (Some(&(_, first)), Some(&(_, last))) = (want.first(), want.last()) else {
        return Ok(out);
    };
    let mut f = File::open(dir.join("thumbs.bin"))?;
    f.seek(io::SeekFrom::Start(first * THUMB as u64))?;
    let mut bytes = Vec::new();
    f.take((last - first + 1) * THUMB as u64)
        .read_to_end(&mut bytes)?;
    for (slot, k) in want {
        let at = (k - first) as usize * THUMB;
        if let Some(t) = bytes.get(at..at + THUMB) {
            out[slot] = Some(t.try_into().expect("THUMB bytes"));
        }
    }
    Ok(out)
}

/// The session directory for a run starting now: `caught/<UTC date-time>`
/// under `root`.
pub fn session_dir(root: &Path) -> PathBuf {
    let stamp = time::OffsetDateTime::now_utc()
        .format(time::macros::format_description!(
            "[year][month][day]-[hour][minute][second]"
        ))
        .unwrap_or_else(|_| "unknown".into());
    root.join("caught").join(stamp)
}

/// Where sessions go by default: `~/cinefit-clips`.
pub fn default_root() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join("cinefit-clips"))
}

/// A session being written.
pub struct Recorder {
    tx: Option<SyncSender<Record>>,
    writer: Option<JoinHandle<io::Result<u64>>>,
    dir: PathBuf,
    dropped: u64,
    /// The full frames kept, once there is one.
    keeper: Option<Keeper>,
}

/// The session's kept frames: the clip, and why each was kept.
struct Keeper {
    clip: ClipWriter,
    why: BufWriter<File>,
    kept: u64,
    /// The budget was reached: no more this session.
    full: bool,
}

/// What a session wrote, at its end.
pub struct Summary {
    pub dir: PathBuf,
    pub frames: u64,
    pub dropped: u64,
    /// Full frames kept, and kept frames the clip writer had to drop.
    pub kept: u64,
    pub kept_dropped: u64,
}

impl Recorder {
    /// Creates `dir` and starts the writer.
    ///
    /// # Errors
    ///
    /// If the directory or its files cannot be created.
    pub fn start(dir: PathBuf) -> io::Result<Self> {
        fs::create_dir_all(&dir)?;
        let mut readings_file = BufWriter::new(File::create(dir.join("readings.csv"))?);
        writeln!(readings_file, "{}", readings::header())?;
        let mut frames_file = BufWriter::new(File::create(dir.join("frames.csv"))?);
        writeln!(
            frames_file,
            "index,time,epoch,content,crop_top,crop_bottom,reading,thumb,cut"
        )?;
        let thumbs_file = BufWriter::new(File::create(dir.join("thumbs.bin"))?);
        let (tx, rx) = sync_channel(QUEUE);
        let writer = std::thread::Builder::new()
            .name("dataset".into())
            .spawn(move || write_all(&rx, readings_file, frames_file, thumbs_file))?;
        Ok(Self {
            tx: Some(tx),
            writer: Some(writer),
            dir,
            dropped: 0,
            keeper: None,
        })
    }

    /// The session's directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Adds a line to the session's `session.txt`: what it was (the source,
    /// later the title, #115). Rare, so written at once.
    pub fn note(&self, line: &str) {
        note_in(&self.dir, line);
    }

    /// Hands a record to the writer; never waits. A full queue drops it.
    pub fn record(&mut self, r: Record) {
        if let Some(tx) = &self.tx {
            match tx.try_send(r) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => self.dropped += 1,
                // The writer stopped on an error: stop sending.
                Err(TrySendError::Disconnected(_)) => self.tx = None,
            }
        }
    }

    /// Keeps one full frame, the raw bytes as captured, as frame `index` of
    /// the readings, for `why`. Copies the bytes and returns; a slow disk
    /// drops it (counted) rather than wait. Within `budget` bytes for the
    /// kept frames of every session: the first keep of a session makes room
    /// by deleting the oldest sessions' (on a thread of its own), and a
    /// session that alone reaches it keeps no more.
    pub fn keep(
        &mut self,
        index: u64,
        time: f64,
        info: &FrameInfo,
        bytes: &[u8],
        why: &str,
        budget: u64,
    ) {
        if budget == 0 {
            return;
        }
        if self.keeper.is_none() {
            let dir = self.dir.join("frames");
            let made = ClipWriter::create(&dir).and_then(|clip| {
                let mut why = BufWriter::new(File::create(dir.join("kept.csv"))?);
                writeln!(why, "index,time,why")?;
                Ok(Keeper {
                    clip,
                    why,
                    kept: 0,
                    full: false,
                })
            });
            match made {
                Ok(k) => self.keeper = Some(k),
                Err(e) => {
                    warn!("dataset: no full frames this session: {e}");
                    return;
                }
            }
            if let Some(caught) = self.dir.parent().map(Path::to_path_buf) {
                let current = self.dir.clone();
                let _ = std::thread::Builder::new()
                    .name("dataset-budget".into())
                    .spawn(move || match prune(&caught, &current, budget) {
                        Ok(0) => {}
                        Ok(n) => {
                            info!("dataset: deleted {n} older sessions' kept frames for the budget")
                        }
                        Err(e) => warn!("dataset: the budget could not be kept: {e}"),
                    });
            }
        }
        let Some(k) = self.keeper.as_mut() else {
            return;
        };
        if k.full {
            return;
        }
        // The clip's size as written so far: a cheap look, before each keep.
        // What zstd still holds is not on disk yet, so the budget can be
        // overshot by a few frames' worth, against many GB.
        let size = fs::metadata(k.clip.dir().join("frames.zst")).map_or(0, |m| m.len());
        if size + bytes.len() as u64 > budget {
            k.full = true;
            info!("dataset: this session's kept frames reached the budget; no more kept");
            return;
        }
        k.clip
            .feed()
            .offer_as(index, Duration::from_secs_f64(time.max(0.0)), info, bytes);
        // At once: a few lines a minute, and a look at a pass still running
        // (cinefit pairs) should see them.
        let _ = writeln!(k.why, "{index},{time:.3},{why}").and_then(|()| k.why.flush());
        k.kept += 1;
    }

    /// Ends the session: the writer drains its queue and closes the files.
    ///
    /// # Errors
    ///
    /// If writing failed at any point.
    pub fn finish(mut self) -> io::Result<Summary> {
        self.tx = None;
        let frames = match self.writer.take().map(JoinHandle::join) {
            Some(Ok(r)) => r?,
            Some(Err(_)) => return Err(io::Error::other("the dataset writer panicked")),
            None => 0,
        };
        let (mut kept, mut kept_dropped) = (0, 0);
        if let Some(mut k) = self.keeper.take() {
            k.why.flush()?;
            let r = k.clip.finish()?;
            (kept, kept_dropped) = (k.kept.saturating_sub(r.dropped), r.dropped);
        }
        Ok(Summary {
            dir: self.dir.clone(),
            frames,
            dropped: self.dropped,
            kept,
            kept_dropped,
        })
    }
}

/// After a crop change, the frames kept with it: the change's own and the
/// next two, to see what the policy saw just after.
const AFTER_CHANGE: u8 = 2;
/// At most one kept for a candidate in this many seconds: a policy
/// hesitating flips its candidate often.
const CANDIDATE_GAP: f64 = 5.0;
/// The random sample's mean interval, in seconds.
const SAMPLE_MEAN: f64 = 300.0;

/// Which frames to keep in full, frame by frame. Pure: tested without a
/// card, a GPU or a disk.
pub struct Picker {
    /// Frames since the last cut, once there has been one: the pair mode's
    /// anchor (#109).
    since_cut: Option<u32>,
    after_change: u8,
    candidate: bool,
    last_candidate: Option<f64>,
    next_sample: f64,
    rng: u64,
}

impl Picker {
    /// `seed` drives the sample's random times (any value but 0).
    pub fn new(seed: u64, t: f64) -> Self {
        let mut p = Self {
            since_cut: None,
            after_change: 0,
            candidate: false,
            last_candidate: None,
            next_sample: 0.0,
            rng: seed.max(1),
        };
        p.next_sample = t + p.interval();
        p
    }

    /// The next random interval: exponential about [`SAMPLE_MEAN`], so the
    /// times sampled do not line up with anything in the films.
    fn interval(&mut self) -> f64 {
        // xorshift64: enough randomness for picking moments.
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        let u = (self.rng >> 11) as f64 / (1u64 << 53) as f64;
        -SAMPLE_MEAN * (1.0 - u).ln()
    }

    /// The pair mode (#109): the frame a cut starts a shot on, and every
    /// `every`th frame of the shot after it. A hard cut is the film's own, so
    /// two passes of the same title (tunnel, then low-latency) keep the same
    /// frames without knowing of each other, where keeping by time would
    /// not. Before the first cut, nothing: there is no anchor yet.
    pub fn pick_pairs(&mut self, cut: bool, every: u32) -> Option<&'static str> {
        if cut {
            self.since_cut = Some(0);
            return Some("cut");
        }
        let n = self.since_cut.as_mut()?;
        *n += 1;
        (every > 0 && (*n).is_multiple_of(every)).then_some("in shot")
    }

    /// Whether to keep the frame at `t`, and why: `changed` if the crop
    /// changed on it, `candidate` if the policy holds one it has not
    /// adopted.
    pub fn pick(&mut self, t: f64, changed: bool, candidate: bool) -> Option<&'static str> {
        let new_candidate = candidate && !self.candidate;
        self.candidate = candidate;
        if changed {
            self.after_change = AFTER_CHANGE;
            return Some("crop change");
        }
        if self.after_change > 0 {
            self.after_change -= 1;
            return Some("after a crop change");
        }
        if new_candidate && self.last_candidate.is_none_or(|l| t - l >= CANDIDATE_GAP) {
            self.last_candidate = Some(t);
            return Some("candidate");
        }
        if t >= self.next_sample {
            self.next_sample = t + self.interval();
            return Some("sample");
        }
        None
    }
}

/// Deletes the oldest sessions' kept frames under `caught` until those of
/// every session together fit `budget` bytes, never `current`'s. Returns
/// how many sessions' it deleted.
fn prune(caught: &Path, current: &Path, budget: u64) -> io::Result<usize> {
    let mut sessions: Vec<(PathBuf, u64)> = Vec::new();
    for entry in fs::read_dir(caught)? {
        let frames = entry?.path().join("frames");
        if frames.is_dir() {
            let size = fs::read_dir(&frames)?
                .filter_map(Result::ok)
                .filter_map(|e| e.metadata().ok())
                .map(|m| m.len())
                .sum();
            sessions.push((frames, size));
        }
    }
    // Session directories are named by their UTC start: by name is by age.
    sessions.sort();
    let mut total: u64 = sessions.iter().map(|(_, s)| s).sum();
    let mut deleted = 0;
    for (frames, size) in sessions {
        if total <= budget {
            break;
        }
        if frames.parent() == Some(current) {
            continue;
        }
        fs::remove_dir_all(&frames)?;
        total -= size;
        deleted += 1;
    }
    Ok(deleted)
}

fn write_all(
    rx: &Receiver<Record>,
    mut readings_file: BufWriter<File>,
    mut frames_file: BufWriter<File>,
    mut thumbs_file: BufWriter<File>,
) -> io::Result<u64> {
    let mut frames = 0;
    for r in rx {
        if let Some((obs, cut)) = r.reading {
            let line = readings::format(&Reading {
                index: r.index,
                time: r.time,
                epoch: r.epoch,
                obs,
                cut,
            });
            writeln!(readings_file, "{line}")?;
        }
        writeln!(
            frames_file,
            "{},{:.4},{},{},{},{},{},{},{}",
            r.index,
            r.time,
            r.epoch,
            r.content,
            r.crop.0,
            r.crop.1,
            u8::from(r.reading.is_some()),
            u8::from(r.thumb.is_some()),
            u8::from(r.cut)
        )?;
        if let Some(t) = &r.thumb {
            thumbs_file.write_all(t)?;
        }
        frames += 1;
    }
    readings_file.flush()?;
    frames_file.flush()?;
    thumbs_file.flush()?;
    Ok(frames)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The detector's tunnel reader agrees with the renderer's packing,
    /// which the GPU unpack is checked against: the same frame, both ways.
    #[test]
    fn the_tunnel_reader_agrees_with_the_renderers_packing() {
        use cinefit_render::tunnel::pack_r210;
        use cinefit_render::v210::Planes;
        let (w, h) = (8usize, 3usize);
        let p = Planes {
            width: w,
            height: h,
            luma: (0..w * h).map(|i| (64 + 151 * i % 4000) as u16).collect(),
            chroma: (0..w * h).map(|i| (2048 + 97 * i % 1900) as u16).collect(),
        };
        let row_bytes = w * 4 + 16;
        let bytes = pack_r210(&p, row_bytes).unwrap();
        let t = cinefit_detect::Tunnel::new(&bytes, w as u32, h as u32, row_bytes).unwrap();
        let (mut luma, mut chroma) = (vec![0u16; w], vec![[0u16; 2]; w / 2]);
        for y in 0..h {
            t.luma_row(y as u32, &mut luma);
            t.chroma_row(y as u32, &mut chroma);
            let row = &p.luma[y * w..][..w];
            assert_eq!(
                luma,
                row.iter().map(|v| v >> 2).collect::<Vec<_>>(),
                "row {y}"
            );
            let c = &p.chroma[y * w..][..w];
            let want: Vec<[u16; 2]> = c.chunks(2).map(|k| [k[0] >> 2, k[1] >> 2]).collect();
            assert_eq!(chroma, want, "row {y}");
        }
    }

    /// Two passes starting at different moments, with the same cuts at the
    /// same frames of the film, keep the same frames of it.
    #[test]
    fn the_pair_mode_keeps_the_same_frames_of_the_film_in_any_pass() {
        let cuts = [100u64, 160, 400];
        let kept = |start: u64| {
            let mut p = Picker::new(1, 0.0);
            (start..600)
                .filter(|&f| p.pick_pairs(cuts.contains(&f), 48).is_some())
                .collect::<Vec<_>>()
        };
        // The cut frames, then every 48th of each shot after its cut.
        assert_eq!(
            kept(0),
            [100, 148, 160, 208, 256, 304, 352, 400, 448, 496, 544, 592]
        );
        // A pass started later, after the first cut: from the next cut on,
        // the same frames.
        assert_eq!(
            kept(120),
            [160, 208, 256, 304, 352, 400, 448, 496, 544, 592]
        );
        // Before any cut there is no anchor.
        let mut p = Picker::new(1, 0.0);
        assert!((0..99).all(|_| p.pick_pairs(false, 1).is_none()));
    }

    #[test]
    fn a_crop_change_keeps_itself_and_the_next_two() {
        let mut p = Picker::new(1, 0.0);
        p.next_sample = f64::INFINITY;
        assert_eq!(p.pick(10.0, true, false), Some("crop change"));
        assert_eq!(p.pick(10.04, false, false), Some("after a crop change"));
        assert_eq!(p.pick(10.08, false, false), Some("after a crop change"));
        assert_eq!(p.pick(10.12, false, false), None);
    }

    #[test]
    fn a_candidate_is_kept_when_it_appears_at_most_every_few_seconds() {
        let mut p = Picker::new(1, 0.0);
        p.next_sample = f64::INFINITY;
        assert_eq!(p.pick(1.0, false, true), Some("candidate"));
        // Held: not again.
        assert_eq!(p.pick(1.04, false, true), None);
        // Gone and back within the gap: not again either.
        assert_eq!(p.pick(2.0, false, false), None);
        assert_eq!(p.pick(3.0, false, true), None);
        // Gone and back after it.
        assert_eq!(p.pick(4.0, false, false), None);
        assert_eq!(p.pick(6.5, false, true), Some("candidate"));
    }

    #[test]
    fn the_sample_averages_one_in_five_minutes() {
        let mut p = Picker::new(0x9e37_79b9_7f4a_7c15, 0.0);
        // Ten hours at 24 frames a second.
        let samples = (0..24 * 3600 * 10)
            .filter(|&i| p.pick(f64::from(i) / 24.0, false, false) == Some("sample"))
            .count();
        // 120 expected; a Poisson count is within 3 sigma (about 33) of it.
        assert!((87..=153).contains(&samples), "{samples}");
    }

    /// Kept frames come back from the session's clip whole, under the
    /// readings' frame numbers, with why; a frame over the budget is not
    /// kept, and none after it.
    #[test]
    fn kept_frames_read_back_under_their_frame_numbers() {
        if std::process::Command::new("zstd")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("zstd not installed: skipped");
            return;
        }
        let dir = std::env::temp_dir()
            .join(format!("cinefit-keep-{}", std::process::id()))
            .join("20261015-030000");
        let _ = fs::remove_dir_all(&dir);
        let mut r = Recorder::start(dir.clone()).unwrap();
        let info = crate::record::tests::info(16);
        let (a, b) = (vec![1u8; 32], vec![2u8; 32]);
        r.keep(7, 0.29, &info, &a, "crop change", 1 << 30);
        r.keep(42, 1.75, &info, &b, "sample", 1 << 30);
        // A budget the next frame alone would cross: not kept, and the
        // session keeps no more even under a larger one. (What zstd has not
        // yet written is not counted, so a budget is overshot by at most a
        // few frames; here nothing is on disk yet, so smaller than a frame.)
        r.keep(43, 1.79, &info, &b, "sample", 31);
        r.keep(44, 1.83, &info, &b, "sample", 1 << 30);
        let s = r.finish().unwrap();
        assert_eq!((s.kept, s.kept_dropped), (2, 0));

        let frames = dir.join("frames");
        let index = crate::record::read_index(&frames).unwrap();
        assert_eq!(index.iter().map(|f| f.index).collect::<Vec<_>>(), [7, 42]);
        let mut got = Vec::new();
        crate::record::for_each_frame::<io::Error>(
            &frames,
            &index,
            |_| true,
            |f, bytes| {
                got.push((f.index, bytes.to_vec()));
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(got, [(7, a), (42, b)]);
        let why = fs::read_to_string(frames.join("kept.csv")).unwrap();
        assert_eq!(
            why,
            "index,time,why\n7,0.290,crop change\n42,1.750,sample\n"
        );
        let _ = fs::remove_dir_all(dir.parent().unwrap());
    }

    #[test]
    fn the_budget_deletes_the_oldest_sessions_frames_and_never_the_current() {
        let root = std::env::temp_dir().join(format!("cinefit-prune-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let mk = |name: &str, kb: usize| {
            let d = root.join(name).join("frames");
            fs::create_dir_all(&d).unwrap();
            fs::write(d.join("frames.zst"), vec![0u8; kb * 1024]).unwrap();
            fs::write(root.join(name).join("readings.csv"), "kept\n").unwrap();
        };
        mk("20261013-200000", 400);
        mk("20261014-200000", 400);
        mk("20261015-200000", 400);
        // Another session with no kept frames is left alone.
        fs::create_dir_all(root.join("20261012-200000")).unwrap();
        let current = root.join("20261015-200000");
        // 1200 KB of kept frames against a 700 KB budget: the two oldest go,
        // the readings stay.
        assert_eq!(prune(&root, &current, 700 * 1024).unwrap(), 2);
        assert!(!root.join("20261013-200000/frames").exists());
        assert!(!root.join("20261014-200000/frames").exists());
        assert!(root.join("20261014-200000/readings.csv").exists());
        assert!(current.join("frames").exists());
        // The current session alone over the budget is not deleted.
        assert_eq!(prune(&root, &current, 1).unwrap(), 0);
        assert!(current.join("frames").exists());
        let _ = fs::remove_dir_all(&root);
    }

    /// A frame of luma by position, for the thumbnail.
    struct Luma {
        w: u32,
        h: u32,
        f: fn(u32, u32) -> u16,
    }

    impl Samples for Luma {
        fn width(&self) -> u32 {
            self.w
        }
        fn height(&self) -> u32 {
            self.h
        }
        fn luma_row(&self, y: u32, out: &mut [u16]) {
            for (x, o) in out.iter_mut().enumerate() {
                *o = (self.f)(x as u32, y);
            }
        }
        fn chroma_row(&self, _y: u32, out: &mut [[u16; 2]]) {
            out.fill([512, 512]);
        }
    }

    #[test]
    fn a_thumbnail_keeps_the_pictures_layout() {
        // Black bars of 280 rows top and bottom around white: the bar rows
        // of the thumbnail read 0, the picture's 255.
        let f = Luma {
            w: 3840,
            h: 2160,
            f: |_, y| if (280..1880).contains(&y) { 940 } else { 64 },
        };
        let t = thumbnail(&f);
        assert!(t[..THUMB_W].iter().all(|&v| v == 0), "{:?}", &t[..THUMB_W]);
        assert!(t[THUMB - THUMB_W..].iter().all(|&v| v == 0));
        let mid = THUMB_H / 2 * THUMB_W;
        assert!(t[mid..mid + THUMB_W].iter().all(|&v| v == 255));
        // A left-to-right ramp stays a ramp.
        let ramp = Luma {
            w: 1920,
            h: 1080,
            f: |x, _| 64 + (x * 876 / 1919) as u16,
        };
        let t = thumbnail(&ramp);
        let row = &t[..THUMB_W];
        assert!(row.windows(2).all(|p| p[0] <= p[1]), "{row:?}");
        assert!(row[0] < 10 && row[THUMB_W - 1] > 245, "{row:?}");
    }

    #[test]
    fn a_session_writes_what_replay_reads_back() {
        let dir = std::env::temp_dir().join(format!("cinefit-dataset-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut rec = Recorder::start(dir.clone()).unwrap();
        for i in 0..3u64 {
            rec.record(Record {
                index: i,
                time: i as f64 / 24.0,
                epoch: 1,
                content: "hdr10",
                crop: (276, 277),
                // The middle frame without a reading: the detector did not run.
                reading: (i != 1).then(|| (Observation::default(), i == 2)),
                thumb: Some([i as u8; THUMB]),
                cut: false,
            });
        }
        let s = rec.finish().unwrap();
        assert_eq!((s.frames, s.dropped), (3, 0));
        let readings_text = fs::read_to_string(dir.join("readings.csv")).unwrap();
        let back: Vec<Reading> = readings_text.lines().filter_map(readings::parse).collect();
        assert_eq!(back.len(), 2);
        assert_eq!((back[0].index, back[1].index), (0, 2));
        assert!(back[1].cut && !back[0].cut);
        let frames = fs::read_to_string(dir.join("frames.csv")).unwrap();
        assert_eq!(frames.lines().count(), 4);
        assert!(
            frames
                .lines()
                .nth(2)
                .unwrap()
                .ends_with(",hdr10,276,277,0,1,0")
        );
        let thumbs = fs::read(dir.join("thumbs.bin")).unwrap();
        assert_eq!(thumbs.len(), 3 * THUMB);
        assert_eq!(thumbs[THUMB], 1);
        fs::remove_dir_all(&dir).unwrap();
    }

    /// A follower's thumbnails read back from the files: frames without one
    /// counted out of thumbs.bin's order, and a line cut short ignored.
    #[test]
    fn thumbnails_read_back_from_the_files_by_frame() {
        let dir = std::env::temp_dir().join(format!("cinefit-thumbs-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut rec = Recorder::start(dir.clone()).unwrap();
        for i in 0..8u64 {
            rec.record(Record {
                index: i,
                time: i as f64 / 24.0,
                epoch: 1,
                content: "tunnel",
                crop: (0, 0),
                reading: None,
                thumb: (i % 3 != 1).then_some([i as u8 * 10; THUMB]),
                cut: false,
            });
        }
        rec.finish().unwrap();
        let mut csv = fs::OpenOptions::new()
            .append(true)
            .open(dir.join("frames.csv"))
            .unwrap();
        write!(csv, "8,0.33").unwrap();
        let got = read_thumbs(&dir, 2, 9).unwrap();
        let first: Vec<Option<u8>> = got.iter().map(|t| t.map(|t| t[0])).collect();
        assert_eq!(
            first,
            [
                Some(20),
                Some(30),
                None,
                Some(50),
                Some(60),
                None,
                None,
                None
            ]
        );
        fs::remove_dir_all(&dir).unwrap();
    }
}
