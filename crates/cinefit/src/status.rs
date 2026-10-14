//! The run's live state in words, for the info panel (#72): what is coming
//! in, what goes out, the crop, and how the last ten seconds went. Built from
//! what `play` already tracks, about once a second while the panel is open.
//! The web page (#74) will read the same.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::osd::InfoSection;

/// How far back the panel's timings look.
const SPAN: Duration = Duration::from_secs(10);

/// The last [`SPAN`] of flips and stage timings.
#[derive(Default)]
pub struct Window {
    /// Each vblank: whether a new frame went out, or the last one again.
    flips: VecDeque<(Instant, bool)>,
    render: VecDeque<(Instant, f64)>,
    detect: VecDeque<(Instant, f64)>,
    /// Each rendered frame's whole work on the frame thread: detect, unpack
    /// and render, which run one after the other.
    work: VecDeque<(Instant, f64)>,
    /// Each frame's brightest luma code.
    peaks: VecDeque<(Instant, u16)>,
}

fn prune<T>(q: &mut VecDeque<(Instant, T)>, now: Instant) {
    while q
        .front()
        .is_some_and(|(t, _)| now.duration_since(*t) > SPAN)
    {
        q.pop_front();
    }
}

fn median(q: &VecDeque<(Instant, f64)>) -> Option<f64> {
    let mut v: Vec<f64> = q.iter().map(|(_, ms)| *ms).collect();
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    Some(v[v.len() / 2])
}

impl Window {
    pub fn flip(&mut self, now: Instant, new_frame: bool) {
        self.flips.push_back((now, new_frame));
        prune(&mut self.flips, now);
    }

    /// A rendered frame: its render time, and its whole work.
    pub fn render(&mut self, now: Instant, ms: f64, work_ms: f64) {
        self.render.push_back((now, ms));
        self.work.push_back((now, work_ms));
        prune(&mut self.render, now);
        prune(&mut self.work, now);
    }

    pub fn peak(&mut self, now: Instant, code: u16) {
        self.peaks.push_back((now, code));
        prune(&mut self.peaks, now);
    }

    /// The brightest luma code of the last ten seconds.
    pub fn brightest(&mut self, now: Instant) -> Option<u16> {
        prune(&mut self.peaks, now);
        self.peaks.iter().map(|(_, c)| *c).max()
    }

    pub fn detect(&mut self, now: Instant, ms: f64) {
        self.detect.push_back((now, ms));
        prune(&mut self.detect, now);
    }

    /// LAST 10 S: frames shown and repeated, and the median frame's whole
    /// work against the frame period, with render's and detect's part.
    pub fn section(&mut self, now: Instant, period: Duration) -> InfoSection {
        prune(&mut self.flips, now);
        prune(&mut self.render, now);
        prune(&mut self.detect, now);
        prune(&mut self.work, now);
        let shown = self.flips.iter().filter(|(_, n)| *n).count();
        let repeated = self.flips.len() - shown;
        let ms = |v: Option<f64>| v.map_or_else(|| "-".to_string(), |v| format!("{v:.1}"));
        InfoSection {
            label: "LAST 10 S".into(),
            line: format!("{shown} shown \u{b7} {repeated} repeated"),
            detail: format!(
                "frame work {} ms of {:.1} (render {}, detect {})",
                ms(median(&self.work)),
                period.as_secs_f64() * 1000.0,
                ms(median(&self.render)),
                ms(median(&self.detect))
            ),
        }
    }
}

/// How long ago, roughly: seconds, then minutes, then hours.
pub fn ago(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..60 => format!("{s} s ago"),
        60..3600 => format!("{} min ago", s / 60),
        _ => format!("{} h {} min ago", s / 3600, (s % 3600) / 60),
    }
}

/// CROP: the picture's shape and rows of bar, and the last change. `source`
/// is the input's size; `rows` the crop (top, bottom), `None` with the crop
/// off; `unread` why the detector does not read this input, if it does not.
pub fn crop_section(
    source: Option<(u32, u32)>,
    rows: Option<(u32, u32)>,
    last: Option<(Duration, &str)>,
    unread: Option<&str>,
) -> InfoSection {
    let label = "CROP".to_string();
    let Some(rows) = rows else {
        return InfoSection {
            label,
            line: "Off".into(),
            detail: "detect.crop is off".into(),
        };
    };
    let Some((w, h)) = source else {
        return InfoSection {
            label,
            line: "\u{2014}".into(),
            detail: "no picture".into(),
        };
    };
    let active = h.saturating_sub(rows.0 + rows.1).max(1);
    let ar = f64::from(w) / f64::from(active);
    let line = if rows == (0, 0) {
        format!("Full frame \u{b7} {ar:.2} : 1")
    } else {
        format!("{ar:.2} : 1 \u{b7} {} / {} rows", rows.0, rows.1)
    };
    InfoSection {
        label,
        line,
        detail: match (unread, last) {
            // Said whether or not a crop was dropped for it: an input that
            // was never cropped is still not being looked at.
            (Some(why), _) => why.into(),
            (None, Some((d, reason))) => format!("{reason}, {}", ago(d)),
            (None, None) => "no change yet".into(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_counts_the_last_ten_seconds() {
        let mut w = Window::default();
        let t0 = Instant::now();
        // 24 new frames a second for 12 s, one repeat a second, at the end.
        for i in 0..12 * 24u32 {
            let t = t0 + Duration::from_millis(u64::from(i) * 1000 / 24);
            w.flip(t, i % 24 != 0);
            w.render(t, 5.0 + f64::from(i % 3), 7.0 + f64::from(i % 3));
            w.detect(t, 1.25);
        }
        let s = w.section(t0 + Duration::from_secs(12), Duration::from_micros(41_708));
        // Ten seconds of 24: 240 flips, 10 of them repeats.
        assert_eq!(s.line, "230 shown \u{b7} 10 repeated", "{s:?}");
        assert_eq!(
            s.detail,
            "frame work 8.0 ms of 41.7 (render 6.0, detect 1.2)"
        );
        assert_eq!(
            Window::default()
                .section(t0, Duration::from_millis(40))
                .detail,
            "frame work - ms of 40.0 (render -, detect -)"
        );
    }

    #[test]
    fn the_crop_reads_as_a_shape() {
        let s = crop_section(
            Some((3840, 2160)),
            Some((280, 279)),
            Some((Duration::from_secs(12), "zoom in (recent)")),
            None,
        );
        assert_eq!(s.line, "2.40 : 1 \u{b7} 280 / 279 rows");
        assert_eq!(s.detail, "zoom in (recent), 12 s ago");
        let full = crop_section(Some((1920, 1080)), Some((0, 0)), None, None);
        assert_eq!(
            (full.line.as_str(), full.detail.as_str()),
            ("Full frame \u{b7} 1.78 : 1", "no change yet")
        );
        assert_eq!(
            crop_section(Some((1920, 1080)), None, None, None).line,
            "Off"
        );
        assert_eq!(
            crop_section(None, Some((0, 0)), None, None).detail,
            "no picture"
        );
        // RGB input: full frame, and why, even with no crop dropped for it.
        let rgb = crop_section(
            Some((3840, 2160)),
            Some((0, 0)),
            None,
            Some("the detector does not read RGB"),
        );
        assert_eq!(
            (rgb.line.as_str(), rgb.detail.as_str()),
            (
                "Full frame \u{b7} 1.78 : 1",
                "the detector does not read RGB"
            )
        );
        assert_eq!(ago(Duration::from_secs(185)), "3 min ago");
        assert_eq!(ago(Duration::from_secs(3 * 3600 + 120)), "3 h 2 min ago");
    }
}
