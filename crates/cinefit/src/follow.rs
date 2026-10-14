//! Following the source mid-run: what the output should do when the input's
//! rate or colour changes. Pure: `play` asks every frame and carries out the
//! answer against the display.
//!
//! Two kinds of change, two costs:
//!
//! - a different frame rate, or a different EOTF (SDR, PQ, HLG), needs a
//!   modeset, which is a projector resync of 2-6 s. Colour rides in the same
//!   commit, so film to menu is one resync, not two. Debounced: the change must
//!   hold for [`STABLE_FRAMES`], because the first frames after a source
//!   change can carry the wrong EOTF (the C++ version's finding).
//! - the same EOTF with other metadata values (MaxCLL, mastering) is not a
//!   modeset on amdgpu and the JVC applies it without a resync (measured
//!   2026-10-12). It rides the next flip, after [`VALUE_FRAMES`]: a disc
//!   correcting its placeholder metadata, or Dolby Vision's per-scene MaxCLL.
//!
//! The target is what the source wants on this frame, re-evaluated every
//! frame, never queued: a queued change lost to a signal blip was Void Video's
//! worst bug. A refused modeset is retried after [`RETRY`], not every frame.

use std::time::Duration;

use cinefit_kms::{Eotf, HdrStatic};

/// Frames a modeset-class change must hold: half a second at 24p.
pub const STABLE_FRAMES: u32 = 12;
/// Frames a metadata-only change must hold.
pub const VALUE_FRAMES: u32 = 3;
/// How long a refused modeset waits before it is tried again.
pub const RETRY: Duration = Duration::from_secs(5);

/// What the output signals: SDR, or HDR with its static metadata.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Signal {
    Sdr,
    Hdr(HdrStatic),
}

impl Signal {
    fn eotf(&self) -> Eotf {
        match self {
            Signal::Sdr => Eotf::Sdr,
            Signal::Hdr(h) => h.eotf,
        }
    }
}

/// What the output should be for a source: its frame rate, in millihertz,
/// and its signalling.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Want {
    pub rate_mhz: u32,
    pub signal: Signal,
}

impl Want {
    /// The part that takes a modeset to change.
    pub fn mode_key(&self) -> (u32, Eotf) {
        (self.rate_mhz, self.signal.eotf())
    }
}

/// What to do on this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    /// A modeset to this rate, with this signalling in the same commit.
    Modeset(Want),
    /// New HDR metadata values on the next flip; no modeset.
    Metadata(HdrStatic),
}

pub struct Policy {
    /// What the output is committed to now.
    out: Want,
    /// A modeset-class target seen on consecutive frames, and how many.
    mode_candidate: Option<((u32, Eotf), u32)>,
    /// A metadata target seen on consecutive frames, and how many.
    value_candidate: Option<(HdrStatic, u32)>,
    /// The last refused modeset target, and when.
    refused: Option<((u32, Eotf), Duration)>,
}

impl Policy {
    /// Starts from what the output was committed to at startup.
    pub fn new(out: Want) -> Self {
        Self {
            out,
            mode_candidate: None,
            value_candidate: None,
            refused: None,
        }
    }

    /// What the output is committed to now.
    pub fn committed(&self) -> Want {
        self.out
    }

    /// Called once a frame with what the source wants now; `now` is any
    /// steady clock. At most one action a frame; carry it out and report it
    /// with [`Policy::done`].
    pub fn next(&mut self, want: &Want, now: Duration) -> Option<Action> {
        let key = want.mode_key();
        if key != self.out.mode_key() {
            self.value_candidate = None;
            let count = match self.mode_candidate {
                Some((k, n)) if k == key => n + 1,
                _ => 1,
            };
            self.mode_candidate = Some((key, count));
            let held_off = self
                .refused
                .is_some_and(|(k, at)| k == key && now.saturating_sub(at) < RETRY);
            return (count >= STABLE_FRAMES && !held_off).then_some(Action::Modeset(*want));
        }
        // Back to what is running: whatever was building up is dropped.
        self.mode_candidate = None;
        match (want.signal, self.out.signal) {
            (Signal::Hdr(w), Signal::Hdr(o)) if w != o => {
                let count = match self.value_candidate {
                    Some((v, n)) if v == w => n + 1,
                    _ => 1,
                };
                self.value_candidate = Some((w, count));
                (count >= VALUE_FRAMES).then_some(Action::Metadata(w))
            }
            _ => {
                self.value_candidate = None;
                None
            }
        }
    }

    /// The result of an action as carried out. A refused modeset is held off
    /// for [`RETRY`]; refused metadata is simply tried again next frame.
    pub fn done(&mut self, action: &Action, ok: bool, now: Duration) {
        match (action, ok) {
            (Action::Modeset(w), true) => {
                self.out = *w;
                self.mode_candidate = None;
                self.value_candidate = None;
                self.refused = None;
            }
            (Action::Modeset(w), false) => {
                self.refused = Some((w.mode_key(), now));
                self.mode_candidate = None;
            }
            (Action::Metadata(h), true) => {
                self.out.signal = Signal::Hdr(*h);
                self.value_candidate = None;
            }
            (Action::Metadata(_), false) => self.value_candidate = None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILM: u32 = 23_976;
    const MENU: u32 = 59_940;

    fn hdr(max_cll: f64) -> Signal {
        Signal::Hdr(HdrStatic {
            max_cll,
            max_fall: 0.0,
            ..HdrStatic::BT2020_1000
        })
    }

    fn want(rate_mhz: u32, signal: Signal) -> Want {
        Want { rate_mhz, signal }
    }

    /// Feeds `want` for `frames` frames from `t0`, 42 ms apart, carrying out
    /// every action successfully; returns the actions in order.
    fn run(p: &mut Policy, w: Want, frames: u32, t0: Duration) -> Vec<Action> {
        let mut acts = Vec::new();
        for i in 0..frames {
            let now = t0 + Duration::from_millis(42 * u64::from(i));
            if let Some(a) = p.next(&w, now) {
                p.done(&a, true, now);
                acts.push(a);
            }
        }
        acts
    }

    #[test]
    fn a_steady_source_needs_nothing() {
        let mut p = Policy::new(want(FILM, hdr(1000.0)));
        assert!(run(&mut p, want(FILM, hdr(1000.0)), 500, Duration::ZERO).is_empty());
    }

    #[test]
    fn film_to_menu_is_one_modeset_with_colour_after_the_debounce() {
        let mut p = Policy::new(want(FILM, hdr(1000.0)));
        let menu = want(MENU, Signal::Sdr);
        let mut acts = Vec::new();
        for i in 0..STABLE_FRAMES {
            let a = p.next(&menu, Duration::from_millis(17 * u64::from(i)));
            if i + 1 < STABLE_FRAMES {
                assert_eq!(a, None, "frame {i}");
            }
            acts.extend(a);
        }
        assert_eq!(acts, [Action::Modeset(menu)]);
        p.done(&acts[0], true, Duration::ZERO);
        assert_eq!(p.committed(), menu);
    }

    #[test]
    fn a_blip_shorter_than_the_debounce_changes_nothing() {
        let mut p = Policy::new(want(FILM, hdr(1000.0)));
        let blip = run(
            &mut p,
            want(FILM, Signal::Sdr),
            STABLE_FRAMES - 1,
            Duration::ZERO,
        );
        let back = run(&mut p, want(FILM, hdr(1000.0)), 50, Duration::from_secs(1));
        assert!(blip.is_empty() && back.is_empty());
        // And the count starts over: another short blip is short too.
        assert!(
            run(
                &mut p,
                want(FILM, Signal::Sdr),
                STABLE_FRAMES - 1,
                Duration::from_secs(3)
            )
            .is_empty()
        );
    }

    #[test]
    fn metadata_values_ride_a_flip_without_a_modeset() {
        let mut p = Policy::new(want(FILM, hdr(0.0)));
        let acts = run(&mut p, want(FILM, hdr(637.0)), 10, Duration::ZERO);
        let Signal::Hdr(h) = hdr(637.0) else {
            unreachable!()
        };
        assert_eq!(acts, [Action::Metadata(h)]);
        // Per-scene values: each new scene's value, once.
        let acts = run(&mut p, want(FILM, hdr(181.0)), 10, Duration::from_secs(1));
        assert_eq!(acts.len(), 1);
    }

    #[test]
    fn a_target_that_changes_mid_debounce_starts_its_count_over() {
        let mut p = Policy::new(want(FILM, Signal::Sdr));
        assert!(
            run(
                &mut p,
                want(FILM, hdr(1000.0)),
                STABLE_FRAMES - 1,
                Duration::ZERO
            )
            .is_empty()
        );
        // Now the rate changes too: a new target, counted from 1.
        let to = want(MENU, hdr(1000.0));
        let acts = run(&mut p, to, STABLE_FRAMES, Duration::from_secs(1));
        assert_eq!(acts, [Action::Modeset(to)]);
    }

    #[test]
    fn a_refused_modeset_waits_before_trying_again() {
        let mut p = Policy::new(want(FILM, hdr(1000.0)));
        let menu = want(MENU, Signal::Sdr);
        let mut tries = Vec::new();
        for i in 0..400u64 {
            let now = Duration::from_millis(17 * i);
            if let Some(a) = p.next(&menu, now) {
                p.done(&a, false, now);
                tries.push(now);
            }
        }
        // Once after the debounce, then again only after RETRY and a fresh
        // debounce: two tries in 6.8 s, not hundreds.
        assert_eq!(tries.len(), 2, "{tries:?}");
        assert!(tries[1] - tries[0] >= RETRY);
        assert_eq!(p.committed(), want(FILM, hdr(1000.0)));
    }
}
