//! L1 measured from an HDR10 picture, for Dolby's display mapping (#141):
//! the minimum, mean and maximum of max(R,G,B) in PQ, as Dolby's analysis
//! gives them, over the picture without its bars.
//!
//! Against Dolby's own L1 on Netflix's masters (#184): min is the true
//! minimum, max the top of the picture, and a running minimum and maximum
//! over the shot match Dolby's per-shot values within 10-15 codes of 4095.
//! Mid is the mean on the 2020 master. Each frame is read on a grid of every
//! 8th row and pixel, the 480 x 270 the model was scored with.

use cinefit_detect::Samples;

/// Rows and pixels between samples.
const STEP: u32 = 8;

/// One frame's statistics of max(R,G,B), PQ 0-1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameL1 {
    pub min: f64,
    pub mean: f64,
    pub max: f64,
}

/// max(R,G,B) in PQ of one limited-range 10-bit BT.2020 Y'CbCr sample:
/// R'G'B' are the PQ values, so the largest is the brightest channel's.
fn max_rgb(y: u16, cb: u16, cr: u16) -> f64 {
    let y = (f64::from(y) - 64.0) / 876.0;
    let cb = (f64::from(cb) - 512.0) / 896.0;
    let cr = (f64::from(cr) - 512.0) / 896.0;
    let r = y + 1.4746 * cr;
    let b = y + 1.8814 * cb;
    let g = (y - 0.2627 * r - 0.0593 * b) / 0.6780;
    r.max(g).max(b).clamp(0.0, 1.0)
}

/// The frame's L1 over rows `top .. height - bottom`, or `None` when no row
/// is left between the bars.
pub fn measure(f: &dyn Samples, top: u32, bottom: u32) -> Option<FrameL1> {
    let (w, h) = (f.width() as usize, f.height());
    let end = h.saturating_sub(bottom);
    if top >= end || w < 2 {
        return None;
    }
    let mut luma = vec![0u16; w];
    let mut chroma = vec![[0u16; 2]; w.div_ceil(2)];
    let (mut min, mut max, mut sum, mut n) = (1.0f64, 0.0f64, 0.0, 0u64);
    // Starting half a step in, so the grid sits inside the picture.
    for y in (top + STEP / 2..end).step_by(STEP as usize) {
        f.luma_row(y, &mut luma);
        f.chroma_row(y, &mut chroma);
        // Even pixels: each is co-sited with its chroma pair.
        for x in (0..w).step_by(STEP as usize) {
            let [cb, cr] = chroma[x / 2];
            let v = max_rgb(luma[x], cb, cr);
            min = min.min(v);
            max = max.max(v);
            sum += v;
            n += 1;
        }
    }
    (n > 0).then(|| FrameL1 {
        min,
        mean: sum / n as f64,
        max,
    })
}

/// [measured] How far, in PQ, a frame's mean may sit from the shot's before
/// the frame starts a new shot without a cut. Fades and dissolves have no cut
/// frame, and Dolby gives them per-frame L1 (a blend of the shots either side,
/// #184); carried across one, the shot's minimum and mean would keep the
/// last shot's. On Netflix's masters with cinefit's cuts, 0.10 brought Sol
/// Levante's mid from 413 codes of 4095 off Dolby's to 59 and its max from
/// 103 to 33, about what Dolby's own shot list gives, and changed nothing on
/// Sparks but its credits' and fades' worst max (p90 1522 to 146).
const DEPART: f64 = 0.10;

/// L1 over the shot so far: the lowest minimum and highest maximum of its
/// frames, and the mean of their means. A cut starts it again, as does a
/// frame far from the shot's mean (a fade or a dissolve). Causal, and steady
/// within a shot: the curve only follows the shot as it shows more.
#[derive(Debug, Default)]
pub struct Shot {
    l1: Option<FrameL1>,
    frames: u32,
}

impl Shot {
    /// A frame of the shot, or the first of a new one after a cut. Returns
    /// the shot's L1 so far as 12-bit PQ codes, min, mid and max.
    pub fn push(&mut self, frame: FrameL1, cut: bool) -> [u16; 3] {
        let departs = self
            .l1
            .is_some_and(|s| (frame.mean - s.mean).abs() > DEPART);
        if cut || departs {
            self.l1 = None;
            self.frames = 0;
        }
        self.frames += 1;
        let k = f64::from(self.frames);
        let l1 = match self.l1 {
            None => frame,
            Some(s) => FrameL1 {
                min: s.min.min(frame.min),
                mean: s.mean + (frame.mean - s.mean) / k,
                max: s.max.max(frame.max),
            },
        };
        self.l1 = Some(l1);
        let code = |v: f64| (v * 4095.0).round() as u16;
        [code(l1.min), code(l1.mean), code(l1.max)]
    }

    /// Forgets the shot: the next frame starts one, as after a cut.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame of `w x h` whose rows are given by `px(x, y) -> (Y, Cb, Cr)`.
    struct Picture {
        w: u32,
        h: u32,
        px: fn(u32, u32) -> (u16, u16, u16),
    }

    impl Samples for Picture {
        fn width(&self) -> u32 {
            self.w
        }
        fn height(&self) -> u32 {
            self.h
        }
        fn luma_row(&self, y: u32, out: &mut [u16]) {
            for (x, o) in out.iter_mut().enumerate() {
                *o = (self.px)(x as u32, y).0;
            }
        }
        fn chroma_row(&self, y: u32, out: &mut [[u16; 2]]) {
            for (p, o) in out.iter_mut().enumerate() {
                let (_, cb, cr) = (self.px)(2 * p as u32, y);
                *o = [cb, cr];
            }
        }
    }

    #[test]
    fn grey_is_its_luma_and_colour_its_brightest_channel() {
        // Neutral: every channel is the luma.
        assert!((max_rgb(64 + 438, 512, 512) - 0.5).abs() < 1e-9);
        // Black and white.
        assert_eq!(max_rgb(64, 512, 512), 0.0);
        assert!((max_rgb(940, 512, 512) - 1.0).abs() < 1e-9);
        // Saturated red is brighter in R than its luma says.
        let (y, cr) = (64 + 300, 512 + 300);
        assert!(max_rgb(y, 512, cr) > (f64::from(y) - 64.0) / 876.0 + 0.3);
    }

    #[test]
    fn the_bars_are_left_out() {
        // A 2.40 picture: bars at black, picture at half PQ, a highlight
        // at three quarters in its middle.
        let pic = Picture {
            w: 64,
            h: 64,
            px: |x, y| match y {
                0..8 | 56.. => (64, 512, 512),
                _ if (28..36).contains(&y) && (24..40).contains(&x) => (64 + 657, 512, 512),
                _ => (64 + 438, 512, 512),
            },
        };
        let whole = measure(&pic, 0, 0).unwrap();
        assert_eq!(whole.min, 0.0, "the bars read black");
        let picture = measure(&pic, 8, 8).unwrap();
        assert!((picture.min - 0.5).abs() < 1e-9, "{picture:?}");
        assert!((picture.max - 657.0 / 876.0).abs() < 1e-9, "{picture:?}");
        assert!(picture.mean > 0.5 && picture.mean < 0.6, "{picture:?}");
        assert_eq!(measure(&pic, 40, 30), None, "nothing between the bars");
    }

    /// A recording of HDR10 frames (our format, v210), named by
    /// `CINEFIT_L1_CLIP`: each frame's L1 and the time it took, as CSV on
    /// stdout, to set beside an offline measurement of the same frames
    /// (#184). `CINEFIT_L1_FRAMES` frames, 240 by default. Skipped without it.
    ///
    ///     CINEFIT_L1_CLIP=DIR cargo test --release -p cinefit l1:: -- --nocapture
    #[test]
    fn a_recordings_frames_measure() {
        let Some(dir) = std::env::var_os("CINEFIT_L1_CLIP") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        let frames: u64 = std::env::var("CINEFIT_L1_FRAMES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(240);
        let index = crate::record::read_index(&dir).expect("index");
        println!("frame,min,mean,max,ms");
        let mut times = Vec::new();
        crate::record::for_each_frame::<std::io::Error>(
            &dir,
            &index,
            |f| f.index < frames,
            |f, bytes| {
                let v =
                    cinefit_detect::V210::new(bytes, f.width as u32, f.height as u32, f.row_bytes)
                        .expect("a v210 frame");
                let t = std::time::Instant::now();
                let l1 = measure(&v, 0, 0).expect("rows");
                let ms = t.elapsed().as_secs_f64() * 1000.0;
                times.push(ms);
                println!(
                    "{},{:.6},{:.6},{:.6},{ms:.3}",
                    f.index, l1.min, l1.mean, l1.max
                );
                Ok(())
            },
        )
        .expect("frames");
        times.sort_by(f64::total_cmp);
        if !times.is_empty() {
            eprintln!(
                "measure: median {:.2} ms, max {:.2} ms, {} frames",
                times[times.len() / 2],
                times[times.len() - 1],
                times.len()
            );
        }
    }

    #[test]
    fn a_shot_widens_until_a_cut_starts_it_again() {
        let f = |min, mean, max| FrameL1 { min, mean, max };
        let mut shot = Shot::default();
        assert_eq!(shot.push(f(0.1, 0.4, 0.6), false), [410, 1638, 2457]);
        // Darker, brighter and a higher mean: min and max widen, mid averages.
        assert_eq!(shot.push(f(0.05, 0.45, 0.7), false), [205, 1740, 2867]);
        // A frame inside the range moves only the mean.
        assert_eq!(shot.push(f(0.2, 0.4, 0.5), false), [205, 1706, 2867]);
        // A cut: the new shot's first frame alone.
        assert_eq!(shot.push(f(0.3, 0.35, 0.4), true), [1229, 1433, 1638]);
        shot.reset();
        assert_eq!(shot.push(f(0.0, 0.1, 0.2), false), [0, 410, 819]);
    }

    #[test]
    fn a_fade_without_a_cut_starts_a_shot_where_the_picture_departs() {
        let f = |mean: f64| FrameL1 {
            min: mean / 4.0,
            mean,
            max: mean * 1.5,
        };
        let mut shot = Shot::default();
        shot.push(f(0.02), false);
        shot.push(f(0.05), false);
        // Fading up from black: once a frame is more than DEPART above the
        // shot's mean, it is a new shot, not more of the dark one.
        let [min, mid, _] = shot.push(f(0.2), false);
        assert_eq!((min, mid), [205, 819].into());
        // Small moves stay in the shot: the mean averages them.
        assert_eq!(shot.push(f(0.24), false)[1], 901);
    }
}
