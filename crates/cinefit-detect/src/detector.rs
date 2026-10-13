//! The whole detector: frames in, a crop out. [`Bars`] reads the frame,
//! [`Cuts`] compares it with the last one over the crop now on screen, and
//! [`ArPolicy`] decides.
//!
//! A detector decides only how many source rows of bar to crop top and
//! bottom. It never touches the image path: capture, unpack, render and
//! scanout are the same whatever it decides, so a wrong decision changes the
//! scaling and nothing else.

use crate::Samples;
use crate::ar_policy::{ArPolicy, Crop, Decision};
use crate::bars::Bars;
use crate::cuts::Cuts;

#[derive(Default)]
pub struct Detector {
    bars: Bars,
    cuts: Cuts,
    policy: ArPolicy,
    epoch: u32,
}

impl Detector {
    /// One frame, at `time_s` (presentation time, monotonic). `epoch` is
    /// bumped by the caller on any change of resolution, frame rate or EOTF
    /// and on signal loss; the detector resets whatever it has learned when
    /// it changes.
    pub fn push(&mut self, f: &dyn Samples, time_s: f64, epoch: u32) -> Decision {
        if epoch != self.epoch {
            self.cuts.reset();
            self.epoch = epoch;
        }
        let o = self.bars.observe(f);
        // The grid covers the crop held going into this frame, which is what
        // the previous frame was compared over; a crop change re-baselines it.
        let c = self.policy.crop();
        let cut = self.cuts.observe(f, c.top, c.bottom).cut;
        self.policy.push(&o, cut, time_s, epoch)
    }

    pub fn crop(&self) -> Crop {
        self.policy.crop()
    }

    pub fn policy(&self) -> &ArPolicy {
        &self.policy
    }
}

#[cfg(test)]
mod tests {
    //! The end of the C++ version's `ar_policy_test`: the whole detector over
    //! synthetic frames.
    use super::*;
    use crate::synth::{Frame, Palette};

    const W: u32 = 3840;
    const H: u32 = 2160;
    const SCOPE: Crop = Crop {
        top: 276,
        bottom: 277,
    };

    #[test]
    fn synthetic_scope_frames_zoom_in_and_out() {
        let pal = Palette::new(W);
        let mut f = Frame::new(&pal, H);
        let mut det = Detector::default();
        let mut switched = 0;
        let mut crop = Crop::default();
        for i in 0..24u32 {
            let scope = (4..16).contains(&i);
            f.fill(0, H, &pal.bar);
            // A still picture: palette texture changes every cell when it
            // moves, which would read as a cut on every frame.
            if scope {
                f.content(SCOPE.top, SCOPE.bottom, &pal.picture, 0);
            } else {
                f.content(0, 0, &pal.picture, 0);
            }
            let d = det.push(&f.pack(), f64::from(i) / 24.0, 1);
            if d.changed {
                switched += 1;
            }
            crop = d.crop;
            if i == 15 {
                assert_eq!(
                    crop, SCOPE,
                    "synthetic scope frames: zoomed in to 276 / 277"
                );
            }
        }
        assert!(
            crop == Crop::default() && switched == 2,
            "16:9 after it: zoomed out again, two switches"
        );

        // Cost, steady state: what the render thread pays per frame.
        f.fill(0, H, &pal.bar);
        f.content(SCOPE.top, SCOPE.bottom, &pal.picture, 0);
        let v = f.pack();
        let t0 = std::time::Instant::now();
        for i in 0..50u32 {
            det.push(&v, f64::from(24 + i) / 24.0, 1);
        }
        eprintln!(
            "detector: {:.2} ms per 2160p scope frame, whole detector (CPU, one thread)",
            t0.elapsed().as_secs_f64() * 1000.0 / 50.0
        );
    }
}
