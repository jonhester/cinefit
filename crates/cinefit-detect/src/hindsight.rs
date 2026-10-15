//! Hindsight (#91): with every frame's readings saved, a shot can be judged
//! whole. Live, the policy must decide within a few frames; afterwards, the
//! picture's true edge in a shot is what its frames showed at their widest,
//! since a shot's framing does not change. Comparing that with the crop shown
//! finds both kinds of mistake:
//!
//! - **cropped too much**: the crop was larger than the true bar, so picture
//!   rows were hidden. The serious one.
//! - **cropped too little**: the crop was smaller, so black rows were on
//!   screen. Often intended: the policy keeps a known height through a dark
//!   shot rather than chase it.
//!
//! The true edge is the smallest bar seen in at least a few frames of the
//! shot, so one stray reading cannot move it. A shot dark at its edges
//! throughout reads as having larger bars than it has; that only ever makes
//! "too little" look worse, never invents "too much".

use crate::bars::FrameClass;
use crate::readings::Reading;

/// What a shot was judged to be, and how the crop shown compared.
#[derive(Debug, Clone, PartialEq)]
pub struct Shot {
    /// Its first and last frame (reading indices) and their times.
    pub first: u64,
    pub last: u64,
    pub start: f64,
    pub end: f64,
    pub frames: usize,
    /// The picture's true edge, rows of bar top and bottom; `None` for a
    /// side no frame measured often enough.
    pub truth: (Option<u32>, Option<u32>),
    /// Frames cropped too much (picture hidden), and the most rows hidden
    /// on either side.
    pub over_frames: usize,
    pub over_rows: u32,
    /// Frames cropped too little (black rows shown), and the most rows.
    pub under_frames: usize,
    pub under_rows: u32,
}

impl Shot {
    pub fn verdict(&self) -> &'static str {
        match (self.over_frames > 0, self.under_frames > 0) {
            _ if self.truth == (None, None) => "unknown",
            (true, true) => "both",
            (true, false) => "too much",
            (false, true) => "too little",
            (false, false) => "right",
        }
    }
}

/// Rows within which the crop shown counts as matching the true edge: the
/// bar measurement moves by a row or two between frames of a still shot.
pub const TOLERANCE: u32 = 3;
/// A gap longer than this between frames ends a shot: signal loss or a
/// stall, not one shot.
const GAP_S: f64 = 1.0;

/// Judges `readings` against the crop shown on each, in the same order.
pub fn judge(readings: &[Reading], shown: &[(u32, u32)]) -> Vec<Shot> {
    assert_eq!(readings.len(), shown.len(), "one shown crop per reading");
    let mut shots = Vec::new();
    let mut start = 0;
    for i in 1..=readings.len() {
        let ends = i == readings.len() || {
            let (a, b) = (&readings[i - 1], &readings[i]);
            b.cut || b.epoch != a.epoch || b.time - a.time > GAP_S
        };
        if ends {
            shots.push(shot(&readings[start..i], &shown[start..i]));
            start = i;
        }
    }
    shots
}

fn shot(r: &[Reading], shown: &[(u32, u32)]) -> Shot {
    // Each side measured on its own: a frame the detector called weak for
    // asymmetry still measured each edge it found.
    let measured = |pick: fn(&Reading) -> &crate::bars::SideObs| -> Vec<u32> {
        r.iter()
            .filter(|x| x.obs.class != FrameClass::Empty)
            .map(pick)
            .filter(|s| s.found && !s.continues)
            .map(|s| s.bar)
            .collect()
    };
    let top = robust_min(measured(|x| &x.obs.top));
    let bottom = robust_min(measured(|x| &x.obs.bottom));
    let mut s = Shot {
        first: r[0].index,
        last: r[r.len() - 1].index,
        start: r[0].time,
        end: r[r.len() - 1].time,
        frames: r.len(),
        truth: (top, bottom),
        over_frames: 0,
        over_rows: 0,
        under_frames: 0,
        under_rows: 0,
    };
    for &(st, sb) in shown {
        let mut over = 0;
        let mut under = 0;
        for (shown, truth) in [(st, top), (sb, bottom)] {
            let Some(t) = truth else { continue };
            if shown > t + TOLERANCE {
                over = over.max(shown - t);
            } else if shown + TOLERANCE < t {
                under = under.max(t - shown);
            }
        }
        if over > 0 {
            s.over_frames += 1;
            s.over_rows = s.over_rows.max(over);
        } else if under > 0 {
            s.under_frames += 1;
            s.under_rows = s.under_rows.max(under);
        }
    }
    s
}

/// The smallest value seen in at least `max(2, 2%)` of `v`: the widest the
/// picture really was, without trusting a single frame. `None` with fewer
/// than three measurements.
fn robust_min(mut v: Vec<u32>) -> Option<u32> {
    if v.len() < 3 {
        return None;
    }
    v.sort_unstable();
    let k = (v.len() / 50).max(2);
    Some(v[k - 1])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bars::{Observation, SideObs};

    fn side(bar: Option<u32>) -> SideObs {
        SideObs {
            found: bar.is_some(),
            bar: bar.unwrap_or(0),
            ..SideObs::default()
        }
    }

    /// A shot of `n` frames whose bars are `bars(i)`, starting at `first`
    /// with a cut.
    fn frames(
        first: u64,
        n: u64,
        bars: impl Fn(u64) -> (Option<u32>, Option<u32>),
    ) -> Vec<Reading> {
        (0..n)
            .map(|i| {
                let (t, b) = bars(i);
                Reading {
                    index: first + i,
                    time: (first + i) as f64 / 24.0,
                    epoch: 1,
                    obs: Observation {
                        class: FrameClass::Letterbox,
                        width: 3840,
                        height: 2160,
                        top: side(t),
                        bottom: side(b),
                        ..Observation::default()
                    },
                    cut: i == 0,
                }
            })
            .collect()
    }

    #[test]
    fn a_crop_matching_the_bars_is_right() {
        let r = frames(0, 48, |_| (Some(276), Some(277)));
        let shots = judge(&r, &vec![(276, 277); 48]);
        assert_eq!(shots.len(), 1);
        assert_eq!(shots[0].truth, (Some(276), Some(277)));
        assert_eq!(shots[0].verdict(), "right");
    }

    #[test]
    fn picture_opening_wider_later_in_a_shot_was_cropped_too_much() {
        // An IMAX shot whose first frames are dark at the edges: the bars
        // read 276 until frame 30, then the picture fills the frame. The
        // crop shown stayed at 276 throughout.
        let r = frames(0, 60, |i| {
            if i < 30 {
                (Some(276), Some(276))
            } else {
                (Some(0), Some(0))
            }
        });
        let shots = judge(&r, &vec![(276, 276); 60]);
        assert_eq!(shots[0].truth, (Some(0), Some(0)));
        assert_eq!(shots[0].verdict(), "too much");
        assert_eq!((shots[0].over_frames, shots[0].over_rows), (60, 276));
    }

    #[test]
    fn bars_left_on_screen_are_too_little() {
        let r = frames(0, 48, |_| (Some(276), Some(277)));
        let shots = judge(&r, &vec![(0, 0); 48]);
        assert_eq!(shots[0].verdict(), "too little");
        assert_eq!(shots[0].under_rows, 277);
    }

    #[test]
    fn one_stray_reading_does_not_move_the_truth() {
        // Frame 10 reads a bright object as picture at row 200: one frame
        // of 48 is not enough to say the picture reaches it.
        let r = frames(0, 48, |i| {
            if i == 10 {
                (Some(200), Some(276))
            } else {
                (Some(276), Some(276))
            }
        });
        let shots = judge(&r, &vec![(276, 276); 48]);
        assert_eq!(shots[0].truth, (Some(276), Some(276)));
        assert_eq!(shots[0].verdict(), "right");
    }

    #[test]
    fn within_the_tolerance_is_right() {
        let r = frames(0, 48, |_| (Some(276), Some(277)));
        let shots = judge(&r, &vec![(276 + TOLERANCE, 277 - TOLERANCE); 48]);
        assert_eq!(shots[0].verdict(), "right");
    }

    #[test]
    fn cuts_epochs_and_gaps_split_shots() {
        let mut r = frames(0, 24, |_| (Some(276), Some(276)));
        r.extend(frames(24, 24, |_| (Some(0), Some(0))));
        // A new epoch without a cut, and a gap.
        let mut later = frames(48, 24, |_| (Some(140), Some(140)));
        later[0].cut = false;
        for x in &mut later {
            x.epoch = 2;
        }
        r.extend(later);
        let mut gapped = frames(72, 24, |_| (Some(140), Some(140)));
        gapped[0].cut = false;
        for x in &mut gapped {
            x.epoch = 2;
            x.time += 5.0;
        }
        r.extend(gapped);
        let shown = vec![(276, 276); r.len()];
        let shots = judge(&r, &shown);
        assert_eq!(shots.len(), 4);
        let v: Vec<&str> = shots.iter().map(Shot::verdict).collect();
        assert_eq!(v, ["right", "too much", "too much", "too much"]);
    }

    #[test]
    fn too_few_measurements_are_unknown() {
        let r = frames(0, 2, |_| (Some(276), Some(276)));
        assert_eq!(judge(&r, &[(276, 276); 2])[0].verdict(), "unknown");
        let dark = frames(0, 24, |_| (None, None));
        assert_eq!(judge(&dark, &[(0, 0); 24])[0].verdict(), "unknown");
    }
}
