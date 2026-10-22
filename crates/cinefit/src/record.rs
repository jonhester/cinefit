//! `cinefit capture --record DIR`: every captured frame, losslessly, as the
//! C++ version records clips. A clip is a directory:
//!
//! - `frames.zst`: every frame's raw bytes, back to back, through zstd
//! - `frames.csv`: one line per frame: where its bytes are in the
//!   decompressed stream and what the card said about it
//! - `README.txt`: what this is, and that it stays private
//!
//! Frames are copied out of the card's buffer into a fixed pool and written
//! from another thread, so a slow disk or compressor never holds a frame or
//! stalls capture. With every buffer in flight a frame is dropped and
//! counted, never silently lost: the CSV's frame numbers show where.
//!
//! Raw 2160p is about 530 MB/s as v210 and 800 as r210; zstd at its fastest
//! level on six threads kept up with v210 for the C++ version.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use cinefit_decklink::FrameInfo;

/// Frames that can be in flight between the card and the disk: about 530 MB
/// of r210 at 2160p, so a second of disk stall costs nothing.
const POOL: usize = 16;

const README: &str = "\
A clip captured by cinefit capture --record, for testing on this machine.
frames.zst is every frame's raw bytes, back to back, through zstd; frames.csv
says where each frame starts in the decompressed stream, how many bytes it is,
and what the card reported about it. Decompress with zstd -dc and split by
offset and size.

This is copyrighted content captured for private testing. Keep it on this
machine, outside any repository. Never commit or publish it.
";

struct Item {
    index: u64,
    at: Duration,
    info: FrameInfo,
    buf: Vec<u8>,
    len: usize,
}

/// Records frames to a clip directory. Create with [`ClipWriter::create`],
/// hand frames to [`ClipWriter::offer`] from the capture callback, and close
/// with [`ClipWriter::finish`].
pub struct ClipWriter {
    tx: Mutex<Option<SyncSender<Item>>>,
    free: Arc<Mutex<Vec<Vec<u8>>>>,
    next: AtomicU64,
    /// Frames offered so far, which the pool grows with.
    offered: AtomicU64,
    dropped: AtomicU64,
    thread: Mutex<Option<JoinHandle<std::io::Result<u64>>>>,
    dir: PathBuf,
}

/// What a finished recording wrote.
#[derive(Debug)]
pub struct Recorded {
    pub written: u64,
    pub dropped: u64,
}

impl ClipWriter {
    /// Creates `dir` (which must not hold a clip already) and starts zstd.
    ///
    /// # Errors
    ///
    /// If the directory or its files cannot be created, `dir` already has a
    /// `frames.zst`, or `zstd` cannot be started.
    pub fn create(dir: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let frames = dir.join("frames.zst");
        if frames.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("{} already exists", frames.display()),
            ));
        }
        std::fs::write(dir.join("README.txt"), README)?;
        let mut csv = BufWriter::new(File::create(dir.join("frames.csv"))?);
        writeln!(
            csv,
            "index,offset,size,at_seconds,stream_time,duration,width,height,row_bytes,pixel_format,eotf,colorspace,max_cll,max_fall,max_mastering,min_mastering"
        )?;
        let zstd = spawn_zstd(&frames)?;

        let (tx, rx) = sync_channel::<Item>(POOL);
        let free = Arc::new(Mutex::new(Vec::with_capacity(POOL)));
        let thread = {
            let free = Arc::clone(&free);
            std::thread::spawn(move || write_loop(rx, zstd, csv, free))
        };
        Ok(Self {
            tx: Mutex::new(Some(tx)),
            free,
            next: AtomicU64::new(0),
            offered: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            thread: Mutex::new(Some(thread)),
            dir: dir.to_owned(),
        })
    }

    /// Copies one frame for writing. On the capture callback's thread: it
    /// never blocks, and drops the frame (counted) when the pool is in use.
    pub fn offer(&self, at: Duration, info: &FrameInfo, bytes: &[u8]) {
        let index = self.next.fetch_add(1, Ordering::Relaxed);
        self.offer_as(index, at, info, bytes);
    }

    /// As [`ClipWriter::offer`], under a frame number of the caller's: the
    /// dataset keeps a few frames of many, numbered as its readings are
    /// (#91).
    pub fn offer_as(&self, index: u64, at: Duration, info: &FrameInfo, bytes: &[u8]) {
        let offered = self.offered.fetch_add(1, Ordering::Relaxed);
        let in_flight = POOL as u64;
        let mut buf = {
            let mut free = self.free.lock().unwrap_or_else(PoisonError::into_inner);
            match free.pop() {
                Some(b) => b,
                // Grow the pool to its size once; after that, a frame with no
                // buffer free is dropped.
                None if offered < in_flight => Vec::new(),
                None => {
                    self.dropped.fetch_add(1, Ordering::Relaxed);
                    return;
                }
            }
        };
        buf.clear();
        buf.extend_from_slice(bytes);
        let item = Item {
            index,
            at,
            info: *info,
            len: bytes.len(),
            buf,
        };
        let tx = self.tx.lock().unwrap_or_else(PoisonError::into_inner);
        let sent = tx.as_ref().is_some_and(|tx| tx.try_send(item).is_ok());
        if !sent {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Writes everything queued, closes zstd and reports what was written.
    ///
    /// # Errors
    ///
    /// If writing failed or zstd did not exit cleanly: the clip is then
    /// incomplete, and says so rather than looking whole.
    pub fn finish(&self) -> std::io::Result<Recorded> {
        drop(
            self.tx
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take(),
        );
        let thread = self
            .thread
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let written = match thread {
            Some(t) => t
                .join()
                .map_err(|_| std::io::Error::other("the clip writer panicked"))??,
            None => 0,
        };
        Ok(Recorded {
            written,
            dropped: self.dropped.load(Ordering::Relaxed),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

/// zstd at its fastest level on six threads, in its own process group: a
/// Ctrl-C at the terminal goes to cinefit, which closes the clip, and not to
/// zstd, which would cut the stream short (the C++ version lost a clip's
/// frames that way, 2026-10-08). With SIGTERM ignored, for the same reason:
/// stopping the service sends it to every process in the unit at once, and
/// zstd died before cinefit could close its input (#142). It ends when its
/// input does, or by the SIGKILL that follows a stop that hangs. Started
/// through sh, whose ignored signals exec keeps, so nothing unsafe runs
/// between fork and exec; zstd sets a handler for SIGINT alone.
fn spawn_zstd(out: &Path) -> std::io::Result<Child> {
    use std::os::unix::process::CommandExt;
    // Through sh a missing zstd would show only as a broken pipe later.
    Command::new("zstd")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    Command::new("sh")
        .args(["-c", "trap '' TERM; exec zstd -q -1 -T6 -o \"$1\"", "sh"])
        .arg(out)
        .stdin(Stdio::piped())
        .process_group(0)
        .spawn()
}

fn write_loop(
    rx: Receiver<Item>,
    mut zstd: Child,
    mut csv: BufWriter<File>,
    free: Arc<Mutex<Vec<Vec<u8>>>>,
) -> std::io::Result<u64> {
    let mut stdin: ChildStdin = zstd.stdin.take().expect("piped");
    let (mut offset, mut written) = (0u64, 0u64);
    let result = (|| {
        for item in rx {
            stdin.write_all(&item.buf[..item.len])?;
            writeln!(csv, "{}", csv_line(&item, offset))?;
            offset += item.len as u64;
            written += 1;
            free.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(item.buf);
        }
        csv.flush()
    })();
    // Closing stdin ends the stream; zstd then finishes the file and exits.
    drop(stdin);
    let status = zstd.wait()?;
    result?;
    if !status.success() {
        return Err(std::io::Error::other(format!("zstd exited with {status}")));
    }
    Ok(written)
}

fn csv_line(item: &Item, offset: u64) -> String {
    let i = &item.info;
    let opt_i = |v: Option<i64>| v.map_or_else(String::new, |v| v.to_string());
    let opt_f = |v: Option<f64>| v.map_or_else(String::new, |v| v.to_string());
    let hdr = i.color.hdr;
    format!(
        "{},{offset},{},{:.6},{},{},{},{},{},{},{},{},{},{},{},{}",
        item.index,
        item.len,
        item.at.as_secs_f64(),
        opt_i(i.stream_time),
        opt_i(i.duration),
        i.width,
        i.height,
        i.row_bytes,
        i.pixel_format,
        i.color.eotf.map_or_else(String::new, |e| e.to_string()),
        i.color
            .colorspace
            .map_or_else(String::new, |c| c.to_string()),
        opt_f(hdr.and_then(|h| h.max_cll)),
        opt_f(hdr.and_then(|h| h.max_fall)),
        opt_f(hdr.and_then(|h| h.max_mastering)),
        opt_f(hdr.and_then(|h| h.min_mastering)),
    )
}

/// One frame of a recorded clip, from its `frames.csv`.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipFrame {
    pub index: u64,
    pub offset: u64,
    pub size: usize,
    pub width: usize,
    pub height: usize,
    pub row_bytes: usize,
    /// As the card reported it: "v210", "r210", "8-bit ARGB".
    pub pixel_format: String,
    /// The frame's place in the source's stream, from the card's stream
    /// time: unlike `index`, it skips the frames a recording dropped.
    pub frame_number: u64,
    /// Seconds: the source's stream time for a C++ clip, which trimming
    /// keeps, so a trimmed piece's times are the original clip's; the
    /// capture time for one of ours, whose CSV has no time scale.
    pub time: f64,
}

/// Reads a clip's `frames.csv`, ours or the C++ version's (whose clips are
/// the recorded library in `~/cinefit-clips`).
///
/// # Errors
///
/// If the file cannot be read or a line does not parse.
pub fn read_index(dir: &Path) -> std::io::Result<Vec<ClipFrame>> {
    let text = std::fs::read_to_string(dir.join("frames.csv"))?;
    if text.lines().next().and_then(|h| h.split(',').nth(2)) == Some("sequence") {
        return read_cpp_index(&text);
    }
    let bad = |n: usize| std::io::Error::other(format!("frames.csv line {n} does not parse"));
    let mut frames = Vec::new();
    for (n, line) in text.lines().enumerate().skip(1) {
        let f: Vec<&str> = line.split(',').collect();
        if f.len() < 10 {
            return Err(bad(n + 1));
        }
        let num = |i: usize| f[i].parse::<u64>().map_err(|_| bad(n + 1));
        frames.push(ClipFrame {
            index: num(0)?,
            offset: num(1)?,
            size: num(2)? as usize,
            width: num(6)? as usize,
            height: num(7)? as usize,
            row_bytes: num(8)? as usize,
            pixel_format: f[9].to_string(),
            frame_number: match (num(4)?, num(5)?) {
                (_, 0) => num(0)?,
                (time, duration) => time / duration,
            },
            time: f[3].parse().map_err(|_| bad(n + 1))?,
        });
    }
    Ok(frames)
}

/// A C++ clip's index: `index,offset,sequence,stream_time,rate_scale,
/// rate_duration,width,height,row_bytes,...`, every frame v210 and
/// `height * row_bytes` long.
fn read_cpp_index(text: &str) -> std::io::Result<Vec<ClipFrame>> {
    let bad = |n: usize| std::io::Error::other(format!("frames.csv line {n} does not parse"));
    let mut frames = Vec::new();
    for (n, line) in text.lines().enumerate().skip(1) {
        let f: Vec<&str> = line.split(',').collect();
        if f.len() < 9 {
            return Err(bad(n + 1));
        }
        let num = |i: usize| f[i].parse::<u64>().map_err(|_| bad(n + 1));
        let stream_time = f[3].parse::<i64>().map_err(|_| bad(n + 1))?;
        let (scale, duration) = (num(4)?, num(5)?);
        let (height, row_bytes) = (num(7)? as usize, num(8)? as usize);
        frames.push(ClipFrame {
            index: num(0)?,
            offset: num(1)?,
            size: height * row_bytes,
            width: num(6)? as usize,
            height,
            row_bytes,
            pixel_format: "v210".into(),
            frame_number: match duration {
                0 => num(0)?,
                d => stream_time.max(0) as u64 / d,
            },
            // In rate_scale ticks; nanoseconds when the card gave no scale,
            // as the C++ capture source records it.
            time: stream_time as f64 / if scale == 0 { 1e9 } else { scale as f64 },
        });
    }
    Ok(frames)
}

/// A clip's compressed frames: ours, or the C++ version's.
fn frames_path(dir: &Path) -> std::path::PathBuf {
    let ours = dir.join("frames.zst");
    let cpp = dir.join("frames.v210.zst");
    if !ours.exists() && cpp.exists() {
        cpp
    } else {
        ours
    }
}

/// Streams a clip's frames through `zstd -dc` in order, calling `each` with
/// every frame for which `want` says yes. Frames are read one at a time, so a
/// clip of any length needs one frame of memory. A stream cut short (zstd
/// killed while writing it, #142) gives every frame before the cut; the
/// result is how many of the index's frames it lacked, 0 for a whole clip.
///
/// # Errors
///
/// If zstd cannot run, the stream and the index disagree, or `each` fails.
pub fn for_each_frame<E: From<std::io::Error>>(
    dir: &Path,
    index: &[ClipFrame],
    mut want: impl FnMut(&ClipFrame) -> bool,
    mut each: impl FnMut(&ClipFrame, &[u8]) -> Result<(), E>,
) -> Result<usize, E> {
    use std::io::Read;
    let mut child = Command::new("zstd")
        .arg("-dc")
        .arg(frames_path(dir))
        .stdout(Stdio::piped())
        .spawn()?;
    let mut out = std::io::BufReader::with_capacity(1 << 24, child.stdout.take().expect("piped"));
    let mut buf = Vec::new();
    let mut at = 0u64;
    let result = (|| {
        for (n, f) in index.iter().enumerate() {
            if f.offset != at {
                return Err(std::io::Error::other(format!(
                    "frame {} is at {} in the stream, not {at}",
                    f.index, f.offset
                ))
                .into());
            }
            buf.resize(f.size, 0);
            match out.read_exact(&mut buf) {
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    return Ok(index.len() - n);
                }
                r => r?,
            }
            at += f.size as u64;
            if want(f) {
                each(f, &buf)?;
            }
        }
        Ok(0)
    })();
    drop(out);
    let _ = child.kill();
    let _ = child.wait();
    result
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use cinefit_decklink::{ColorInfo, PixelFormat};

    pub(crate) fn info(row_bytes: usize) -> FrameInfo {
        FrameInfo {
            width: 6,
            height: 2,
            row_bytes,
            pixel_format: PixelFormat::V210,
            stream_time: Some(1001),
            duration: Some(1001),
            has_input: true,
            color: ColorInfo {
                colorspace: None,
                eotf: None,
                hdr: None,
            },
            dolby_vision_flagged: false,
            dolby_vision_bytes: None,
        }
    }

    #[test]
    fn frames_come_back_byte_for_byte_with_their_offsets() {
        if Command::new("zstd").arg("--version").output().is_err() {
            eprintln!("zstd not installed: skipped");
            return;
        }
        let dir = std::env::temp_dir().join(format!("cinefit-record-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let w = ClipWriter::create(&dir).expect("create");
        let frames: Vec<Vec<u8>> = (0..5u8)
            .map(|i| (0..32).map(|b| b ^ i.wrapping_mul(37)).collect())
            .collect();
        for (i, f) in frames.iter().enumerate() {
            w.offer(Duration::from_millis(42 * i as u64), &info(16), f);
        }
        let r = w.finish().expect("finish");
        assert_eq!((r.written, r.dropped), (5, 0));

        let out = Command::new("zstd")
            .arg("-dc")
            .arg(dir.join("frames.zst"))
            .output()
            .expect("zstd -dc");
        assert_eq!(out.stdout, frames.concat());
        let csv = std::fs::read_to_string(dir.join("frames.csv")).expect("csv");
        let offsets: Vec<&str> = csv
            .lines()
            .skip(1)
            .map(|l| l.split(',').nth(1).expect("offset"))
            .collect();
        assert_eq!(offsets, ["0", "32", "64", "96", "128"]);
        assert!(dir.join("README.txt").exists());
        // And back through the reader: the index matches, and every frame
        // comes out whole.
        let index = read_index(&dir).expect("index");
        assert_eq!(index.len(), 5);
        assert_eq!(
            (index[2].offset, index[2].size, index[2].row_bytes),
            (64, 32, 16)
        );
        assert_eq!(index[0].pixel_format, "v210");
        let mut got = Vec::new();
        for_each_frame::<std::io::Error>(
            &dir,
            &index,
            |f| f.index != 3,
            |f, b| {
                got.push((f.index, b.to_vec()));
                Ok(())
            },
        )
        .expect("read");
        assert_eq!(got.len(), 4);
        assert_eq!(got[3], (4, frames[4].clone()));
        // A second recording into the same directory is refused, not mixed in.
        assert!(ClipWriter::create(&dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn zstd_outlives_a_sigterm_and_finishes_the_stream() {
        if Command::new("zstd").arg("--version").output().is_err() {
            eprintln!("zstd not installed: skipped");
            return;
        }
        let dir = std::env::temp_dir().join(format!("cinefit-term-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let out = dir.join("frames.zst");
        let mut z = spawn_zstd(&out).expect("zstd");
        let mut stdin = z.stdin.take().expect("piped");
        stdin.write_all(&[7u8; 1 << 16]).expect("write");
        // What a service stop does to every process in the unit.
        let killed = Command::new("kill")
            .args(["-TERM", &z.id().to_string()])
            .status()
            .expect("kill");
        assert!(killed.success());
        std::thread::sleep(Duration::from_millis(200));
        stdin
            .write_all(&[9u8; 1 << 16])
            .expect("still taking input");
        drop(stdin);
        assert!(z.wait().expect("wait").success());
        let back = Command::new("zstd")
            .arg("-dc")
            .arg(&out)
            .output()
            .expect("zstd -dc");
        assert!(back.status.success());
        assert_eq!(back.stdout.len(), 1 << 17);
        assert_eq!(back.stdout[(1 << 17) - 1], 9);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stream_cut_short_gives_the_frames_before_the_cut() {
        if Command::new("zstd").arg("--version").output().is_err() {
            eprintln!("zstd not installed: skipped");
            return;
        }
        let dir = std::env::temp_dir().join(format!("cinefit-cut-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let w = ClipWriter::create(&dir).expect("create");
        // Noise, so the stream is about as long as the frames and a cut
        // partway through lands among them; 256 KB each spans several of
        // zstd's 128 KB blocks.
        let mut x = 0x9E37_79B9_7F4A_7C15u64;
        let frames: Vec<Vec<u8>> = (0..6)
            .map(|_| {
                (0..1 << 18)
                    .map(|_| {
                        x ^= x << 13;
                        x ^= x >> 7;
                        x ^= x << 17;
                        x as u8
                    })
                    .collect()
            })
            .collect();
        for (i, f) in frames.iter().enumerate() {
            w.offer(Duration::from_millis(42 * i as u64), &info(1 << 17), f);
        }
        w.finish().expect("finish");
        // As a SIGTERM to zstd leaves it: the end of the stream missing.
        let zst = dir.join("frames.zst");
        let len = std::fs::metadata(&zst).expect("zst").len();
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&zst)
            .expect("open");
        file.set_len(len * 6 / 10).expect("truncate");

        let index = read_index(&dir).expect("index");
        let mut got = Vec::new();
        let lost = for_each_frame::<std::io::Error>(
            &dir,
            &index,
            |_| true,
            |f, b| {
                got.push((f.index, b.to_vec()));
                Ok(())
            },
        )
        .expect("the complete frames, not an error");
        assert!((1..6).contains(&lost), "{lost} lost");
        assert_eq!(got.len() + lost, 6);
        for (i, (n, b)) in got.iter().enumerate() {
            assert_eq!((*n, b), (i as u64, &frames[i]));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cpp_clip_index_reads_with_stream_times_and_v210_sizes() {
        let text = "index,offset,sequence,stream_time,rate_scale,rate_duration,width,height,row_bytes,eotf,hdr_valid,max_cll,max_fall,max_mastering,min_mastering
0,0,299,311311,24000,1001,3840,2160,10240,2,1,1242,436,4000,0.0050
1,22118400,300,312312,24000,1001,3840,2160,10240,2,1,1242,436,4000,0.0050
";
        let index = read_cpp_index(text).expect("parses");
        assert_eq!(index.len(), 2);
        assert_eq!((index[1].offset, index[1].size), (22_118_400, 2160 * 10240));
        assert_eq!(index[0].pixel_format, "v210");
        // 311311 / 1001 and / 24000: the original clip's frame and time.
        assert_eq!(index[0].frame_number, 311);
        assert!((index[0].time - 12.971_292).abs() < 1e-6);
    }
}
