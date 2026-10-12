//! `cinefit capture`: run the input for a while and report what arrived.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use cinefit_decklink::{Frame, FrameInfo, Handler, InputFormat, PixelFormat};

/// What a capture run saw. Counting only: frames are dropped as soon as they
/// are counted, so the card's buffers are never held.
#[derive(Debug, Default)]
pub struct Stats {
    /// Every format change, with when it happened.
    pub format_changes: Vec<(Duration, InputFormat)>,
    /// Frames with a signal, by the pixel format they arrived in.
    pub frames: BTreeMap<PixelFormat, u64>,
    /// Frames the card delivered with nothing on its input.
    pub no_input: u64,
    /// Breaks in stream time between consecutive frames.
    pub gaps: u64,
    /// Frames those breaks skipped, as far as stream time can tell.
    pub missed: u64,
    /// Frames that carried no stream time, so could not be checked for gaps.
    pub untimed: u64,
    last_stream_time: Option<i64>,
}

impl Stats {
    pub fn on_format(&mut self, at: Duration, format: &InputFormat) {
        self.format_changes.push((at, *format));
        // Stream time restarts in the new mode's units.
        self.last_stream_time = None;
    }

    pub fn on_frame(&mut self, info: &FrameInfo) {
        if !info.has_input {
            self.no_input += 1;
            self.last_stream_time = None;
            return;
        }
        *self.frames.entry(info.pixel_format).or_default() += 1;

        let (Some(t), Some(d)) = (info.stream_time, info.duration) else {
            self.untimed += 1;
            self.last_stream_time = None;
            return;
        };
        if let Some(last) = self.last_stream_time
            && d > 0
        {
            let delta = t - last;
            // One tick of slop: stream time is in the mode's own units, but a
            // driver could still round.
            if (delta - d).abs() > 1 {
                self.gaps += 1;
                if delta > d {
                    // Round to whole frames; the frame itself is not missed.
                    let frames = (delta + d / 2) / d;
                    self.missed += u64::try_from(frames - 1).unwrap_or(0);
                }
            }
        }
        self.last_stream_time = Some(t);
    }

    pub fn total_frames(&self) -> u64 {
        self.frames.values().sum()
    }

    /// The human-readable summary printed at the end of a run.
    pub fn summary(&self, ran: Duration, failed_restarts: u64) -> String {
        let mut s = String::new();
        let secs = ran.as_secs_f64();
        let _ = writeln!(s, "ran {secs:.1} s");

        let _ = writeln!(s, "format changes: {}", self.format_changes.len());
        for (at, f) in &self.format_changes {
            let _ = writeln!(s, "  {:6.2} s  {f} -> {}", at.as_secs_f64(), f.capture_as());
        }

        let total = self.total_frames();
        let _ = write!(s, "frames: {total}");
        // Only meaningful when one format held from its detection to the stop.
        if let [(at, f)] = self.format_changes.as_slice()
            && f.frame_duration > 0
            && ran > *at
        {
            let expected = (ran - *at).as_secs_f64() * f.frames_per_second();
            let _ = write!(s, " (about {expected:.0} expected since detection)");
        }
        let _ = writeln!(s);
        for (pf, n) in &self.frames {
            let _ = writeln!(s, "  {pf}: {n}");
        }
        let _ = writeln!(s, "no input source: {}", self.no_input);
        let _ = writeln!(
            s,
            "stream gaps: {} (about {} frames missed)",
            self.gaps, self.missed
        );
        if self.untimed > 0 {
            let _ = writeln!(s, "frames without stream time: {}", self.untimed);
        }
        let _ = writeln!(s, "failed restarts: {failed_restarts}");
        s
    }
}

/// Counts what the input delivers. Runs on the SDK's thread.
pub struct Recorder {
    started: Instant,
    stats: Arc<Mutex<Stats>>,
}

impl Recorder {
    pub fn new(stats: Arc<Mutex<Stats>>) -> Self {
        Self {
            started: Instant::now(),
            stats,
        }
    }
}

impl Handler for Recorder {
    fn format_changed(&self, format: &InputFormat) {
        let at = self.started.elapsed();
        lock(&self.stats).on_format(at, format);
    }

    fn frame(&self, frame: Frame) {
        lock(&self.stats).on_frame(frame.info());
        // `frame` drops here, handing its buffer straight back to the card.
    }
}

/// A poisoned lock only means a panic elsewhere, which aborts across the SDK
/// boundary anyway; the counts are still usable.
pub fn lock(stats: &Mutex<Stats>) -> std::sync::MutexGuard<'_, Stats> {
    stats.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(t: i64) -> FrameInfo {
        FrameInfo {
            width: 3840,
            height: 2160,
            row_bytes: 10240,
            pixel_format: PixelFormat::V210,
            stream_time: Some(t),
            duration: Some(1001),
            has_input: true,
        }
    }

    #[test]
    fn steady_stream_has_no_gaps() {
        let mut s = Stats::default();
        for i in 0..100 {
            s.on_frame(&frame(i * 1001));
        }
        assert_eq!(s.total_frames(), 100);
        assert_eq!((s.gaps, s.missed), (0, 0));
    }

    #[test]
    fn a_skipped_stretch_is_one_gap_counting_its_frames() {
        let mut s = Stats::default();
        s.on_frame(&frame(0));
        s.on_frame(&frame(1001));
        s.on_frame(&frame(4 * 1001)); // frames 2 and 3 never arrived
        assert_eq!((s.gaps, s.missed), (1, 2));
    }

    #[test]
    fn one_tick_of_jitter_is_not_a_gap() {
        let mut s = Stats::default();
        s.on_frame(&frame(0));
        s.on_frame(&frame(1002));
        s.on_frame(&frame(2002));
        assert_eq!(s.gaps, 0);
    }

    #[test]
    fn no_input_frames_are_counted_apart_and_reset_timing() {
        let mut s = Stats::default();
        s.on_frame(&frame(0));
        s.on_frame(&FrameInfo {
            has_input: false,
            ..frame(1001)
        });
        s.on_frame(&frame(50 * 1001)); // after a signal loss: not a gap
        assert_eq!(s.no_input, 1);
        assert_eq!(s.total_frames(), 2);
        assert_eq!(s.gaps, 0);
    }

    #[test]
    fn frames_without_stream_time_are_counted_as_untimed() {
        let mut s = Stats::default();
        s.on_frame(&FrameInfo {
            stream_time: None,
            ..frame(0)
        });
        assert_eq!(s.untimed, 1);
        assert_eq!(s.total_frames(), 1);
    }

    #[test]
    fn frames_are_counted_by_pixel_format() {
        let mut s = Stats::default();
        s.on_frame(&frame(0));
        s.on_frame(&FrameInfo {
            pixel_format: PixelFormat::Argb8,
            ..frame(1001)
        });
        assert_eq!(s.frames.get(&PixelFormat::V210), Some(&1));
        assert_eq!(s.frames.get(&PixelFormat::Argb8), Some(&1));
    }
}
