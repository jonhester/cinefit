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

use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::thread::JoinHandle;

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
}

/// What a session wrote, at its end.
pub struct Summary {
    pub dir: PathBuf,
    pub frames: u64,
    pub dropped: u64,
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
            "index,time,epoch,content,crop_top,crop_bottom,reading,thumb"
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
        })
    }

    /// The session's directory.
    pub fn dir(&self) -> &Path {
        &self.dir
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
        Ok(Summary {
            dir: self.dir.clone(),
            frames,
            dropped: self.dropped,
        })
    }
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
            "{},{:.4},{},{},{},{},{},{}",
            r.index,
            r.time,
            r.epoch,
            r.content,
            r.crop.0,
            r.crop.1,
            u8::from(r.reading.is_some()),
            u8::from(r.thumb.is_some())
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
                .ends_with(",hdr10,276,277,0,1")
        );
        let thumbs = fs::read(dir.join("thumbs.bin")).unwrap();
        assert_eq!(thumbs.len(), 3 * THUMB);
        assert_eq!(thumbs[THUMB], 1);
        fs::remove_dir_all(&dir).unwrap();
    }
}
