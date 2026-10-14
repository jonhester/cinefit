//! Cuts, for the AR policy: one frame and the current crop in, "was this a
//! hard cut" out. A cut is a shot boundary, not a scene change: a scene is
//! many shots, and its boundaries are often fades, which this ignores.
//!
//! A cut clears the policy's pending zoom-in candidate, so evidence from the
//! previous shot stops counting. That is its only effect. It is a hint, not a
//! gate: a false cut delays a zoom-in by a few frames, a missed one lets old
//! evidence count a little longer, and neither can crop picture. So this is
//! deliberately cheap: no confirmation frame, no cooldown, no near-black
//! events, all of which the reference detector needs because its cuts drive
//! decisions.
//!
//! The thresholds are the reference description's, in 10-bit codes, plus one
//! of the C++ version's own, `spike`, set against 352 labelled detections from
//! recorded clips: fast motion through texture passes every threshold on
//! every frame, and what tells a cut from it is that a cut is a spike after a
//! calm frame.

use crate::{RowScratch, Samples};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    /// Cells across; rows are chosen to keep them square.
    pub cols: u32,
    /// Per side: each cell is the mean of `points`² luma samples. Grain is a
    /// few codes against a 32-code threshold, so 1 may do; the clips decide.
    pub points: u32,
    /// Codes a cell must move to count as changed.
    pub changed_delta: u32,
    /// Mean |cell difference|, codes ...
    pub mean_min: f64,
    /// ... and share of cells changed, both needed.
    pub changed_min: f64,
    /// ... and the picture's make-up must have changed too: a histogram jump,
    /// or a mean so large it cannot be motion. A pan moves cells a long way
    /// while leaving the histogram alone; a fade moves the histogram a little
    /// every frame. Neither is a cut. Share of samples that changed 16-bin
    /// histogram bin.
    pub hist_min: f64,
    pub mean_alone: f64,
    /// ... and the mean must be at least this many times the previous frame's.
    /// Measured: real cuts were 3.1x or more, bar four cuts out of fast
    /// motion (1.1-1.7x), so 2 sits in the gap; it removed 134 of 160 false
    /// cuts. Cuts out of fast motion are missed, which costs nothing.
    pub spike: f64,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            cols: 32,
            points: 2,
            changed_delta: 32,
            mean_min: 44.0,
            changed_min: 0.45,
            hist_min: 0.055,
            mean_alone: 64.0,
            spike: 2.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CutObs {
    pub cut: bool,
    /// False on the first frame after a reset or a crop change.
    pub compared: bool,
    /// Cell rows over the crop.
    pub rows: u32,
    /// Codes.
    pub mean_diff: f64,
    /// The previous frame's `mean_diff`; 0 after a new baseline.
    pub prev_mean: f64,
    /// Share of cells.
    pub changed: f64,
    /// Share of samples.
    pub hist_dist: f64,
}

/// The previous frame, as the grid saw it.
struct Baseline {
    width: u32,
    height: u32,
    top: u32,
    bottom: u32,
    cells: Vec<u32>,
    hist: [u32; 16],
}

pub struct Cuts {
    p: Params,
    prev: Option<Baseline>,
    prev_mean: f64,
    cells: Vec<u32>,
    /// The grid's columns, and one row's luma at them: points, not whole
    /// rows, read from the frame.
    xs: Vec<u32>,
    row: Vec<u16>,
    scratch: RowScratch,
}

impl Default for Cuts {
    fn default() -> Self {
        Self::new(Params::default())
    }
}

impl Cuts {
    pub fn new(p: Params) -> Self {
        Cuts {
            p,
            prev: None,
            prev_mean: 0.0,
            cells: Vec::new(),
            xs: Vec::new(),
            row: Vec::new(),
            scratch: RowScratch::default(),
        }
    }

    pub fn params(&self) -> &Params {
        &self.p
    }

    /// Forget the previous frame: on an epoch change (new format or signal
    /// loss).
    pub fn reset(&mut self) {
        self.prev = None;
        self.prev_mean = 0.0;
    }

    /// `top` and `bottom` are the current crop's bar rows: the grid covers
    /// only the picture, or unchanging bar samples would dilute every
    /// statistic, by 22% at 2.39 and 33% at 2.76, making cuts in wide films
    /// harder to see. A changed crop or frame size starts a new baseline and
    /// is never a cut.
    pub fn observe(&mut self, f: &dyn Samples, top: u32, bottom: u32) -> CutObs {
        let (w, h) = (f.width(), f.height());
        let mut o = CutObs::default();
        if w == 0 || top + bottom >= h {
            self.reset();
            return o;
        }

        let active = h - top - bottom;
        let cols = self.p.cols.max(1);
        // Square cells: 18 rows at 16:9, 13 at 2.39, 12 at 2.76.
        let rows = ((f64::from(cols) * f64::from(active) / f64::from(w)).round() as u32).max(1);
        let points = self.p.points.max(1);
        o.rows = rows;

        // Points sit at the centres of a points x points subdivision of each
        // cell, so they never land on the cell's (and hence the crop's) edge
        // rows.
        let (cw, ch) = (
            f64::from(w) / f64::from(cols),
            f64::from(active) / f64::from(rows),
        );
        self.cells.clear();
        self.cells.resize((cols * rows) as usize, 0);
        // The columns, cell by cell, `points` to a cell.
        self.xs.clear();
        for c in 0..cols {
            for i in 0..points {
                let x = (((f64::from(c) + (f64::from(i) + 0.5) / f64::from(points)) * cw) as u32)
                    .min(w - 1);
                self.xs.push(x);
            }
        }
        self.row.resize(self.xs.len(), 0);
        let mut hist = [0u32; 16];
        for r in 0..rows {
            for j in 0..points {
                let y =
                    top + ((f64::from(r) + (f64::from(j) + 0.5) / f64::from(points)) * ch) as u32;
                let y = y.min(top + active - 1);
                f.luma_at(y, &self.xs, &mut self.row, &mut self.scratch);
                for c in 0..cols {
                    let cell = &mut self.cells[(r * cols + c) as usize];
                    for i in 0..points {
                        let v = u32::from(self.row[(c * points + i) as usize]) & 0x3FF;
                        *cell += v;
                        hist[(v >> 6) as usize] += 1;
                    }
                }
            }
        }
        for cell in &mut self.cells {
            *cell /= points * points;
        }

        let same_area = self
            .prev
            .as_ref()
            .is_some_and(|b| b.width == w && b.height == h && b.top == top && b.bottom == bottom);
        if same_area {
            let prev = self.prev.as_ref().expect("checked");
            let (mut sum, mut changed) = (0u64, 0u64);
            for (a, b) in self.cells.iter().zip(&prev.cells) {
                let d = a.abs_diff(*b);
                sum += u64::from(d);
                changed += u64::from(d > self.p.changed_delta);
            }
            // Histogram distance: half the L1 distance between the two bin
            // counts, as a share of samples: the fraction that would have to
            // change bin to turn one histogram into the other. The reference
            // description gives the threshold (55 per mille) but not the
            // measure; this is the C++ version's reading.
            let l1: u64 = hist
                .iter()
                .zip(&prev.hist)
                .map(|(a, b)| u64::from(a.abs_diff(*b)))
                .sum();
            let n = self.cells.len() as f64;
            let samples = n * f64::from(points * points);
            o.compared = true;
            o.mean_diff = sum as f64 / n;
            o.changed = changed as f64 / n;
            o.hist_dist = l1 as f64 / 2.0 / samples;
            o.prev_mean = self.prev_mean;
            o.cut = o.mean_diff >= self.p.mean_min
                && o.changed >= self.p.changed_min
                && (o.hist_dist >= self.p.hist_min || o.mean_diff >= self.p.mean_alone)
                && o.mean_diff >= self.p.spike * self.prev_mean;
            self.prev_mean = o.mean_diff;
        } else {
            // A new baseline has no previous change to compare with.
            self.prev_mean = 0.0;
        }

        let cells = std::mem::take(&mut self.cells);
        if let Some(b) = &mut self.prev {
            self.cells = std::mem::replace(&mut b.cells, cells);
            b.hist = hist;
            (b.width, b.height, b.top, b.bottom) = (w, h, top, bottom);
        } else {
            self.prev = Some(Baseline {
                width: w,
                height: h,
                top,
                bottom,
                cells,
                hist,
            });
        }
        o
    }
}

#[cfg(test)]
mod tests {
    //! The C++ version's `cuts_test`: synthetic shots, what must read as a cut
    //! (two different shots back to back) and what must not (grain, pans,
    //! fades, cross-fades, anything in the bars). The shots are smooth fields
    //! plus +-3 codes of grain rather than the palette's rows, whose per-pixel
    //! random texture changes every cell on every frame and so makes every
    //! frame a cut.
    use super::*;
    use crate::V210;
    use crate::synth::{Row, hash, row_bytes};

    #[derive(Clone, Copy, PartialEq)]
    enum Shot {
        A,
        B,
        AInverted,
        Black,
        Texture,
    }

    /// The shots, scrolled `pan` pixels left, in 10-bit legal-range codes.
    /// Every term is a column function times a row function, so a frame
    /// costs a few sines per column and per row rather than per pixel.
    struct Columns {
        ua: Vec<f64>,
        sa: Vec<f64>,
        ui: Vec<f64>,
        si: Vec<f64>,
        sb: Vec<f64>,
        tx: Vec<f64>,
    }

    impl Columns {
        fn new(w: u32, pan: f64) -> Self {
            let wf = f64::from(w);
            let mut c = Columns {
                ua: vec![0.0; w as usize],
                sa: vec![0.0; w as usize],
                ui: vec![0.0; w as usize],
                si: vec![0.0; w as usize],
                sb: vec![0.0; w as usize],
                tx: vec![0.0; w as usize],
            };
            for x in 0..w as usize {
                let xf = x as f64;
                let u = (xf + pan) / wf;
                let u_inv = (wf - 1.0 - xf - pan) / wf;
                c.ua[x] = u;
                c.sa[x] = (6.0 * u).sin();
                c.sb[x] = (9.0 * u).sin();
                c.ui[x] = u_inv;
                c.si[x] = (6.0 * u_inv).sin();
                c.tx[x] = (2.0 * std::f64::consts::PI * (xf + pan) / 240.0).sin();
            }
            c
        }
    }

    struct RowTerms {
        v: f64,
        ga: f64,
        ca: f64,
        gi: f64,
        ci: f64,
        ty: f64,
    }

    impl RowTerms {
        fn new(y: u32, h: u32) -> Self {
            let (yf, hf) = (f64::from(y), f64::from(h));
            let v = yf / hf;
            let v_inv = (hf - 1.0 - yf) / hf;
            RowTerms {
                v,
                ga: 0.6 + 0.4 * v,
                ca: (4.0 * v).cos(),
                gi: 0.6 + 0.4 * v_inv,
                ci: (4.0 * v_inv).cos(),
                ty: 0.6 + 0.4 * (2.0 * std::f64::consts::PI * yf / 300.0).cos(),
            }
        }
    }

    fn shot(s: Shot, c: &Columns, r: &RowTerms, x: usize) -> f64 {
        match s {
            // Daylight: broad gradient with soft blobs.
            Shot::A => 220.0 + 380.0 * c.ua[x] * r.ga + 120.0 * c.sa[x] * r.ca,
            // The same make-up, mirrored: a cut the histogram cannot see.
            Shot::AInverted => 220.0 + 380.0 * c.ui[x] * r.gi + 120.0 * c.si[x] * r.ci,
            // Bright sky over dark ground.
            Shot::B => {
                if r.v < 0.45 {
                    760.0 - 200.0 * r.v
                } else {
                    140.0 + 60.0 * c.sb[x]
                }
            }
            Shot::Black => 64.0,
            // Broad bands sweeping past: corn from inside it, a whip pan.
            Shot::Texture => 420.0 + 300.0 * c.tx[x] * r.ty,
        }
    }

    struct Frame {
        w: u32,
        h: u32,
        bytes: Vec<u8>,
        luma: Vec<u16>,
        chroma: Vec<u16>,
    }

    impl Frame {
        fn new(w: u32, h: u32) -> Self {
            Frame {
                w,
                h,
                bytes: Vec::with_capacity(row_bytes(w) * h as usize),
                luma: vec![0; w as usize],
                chroma: vec![512; w as usize],
            }
        }

        /// `mix` of shot b over shot a (0 = all a), times `gain` (a fade),
        /// with grain. Rows `0..top` and `h - bottom..h` are bar at `bar`.
        #[allow(clippy::too_many_arguments)]
        fn draw(
            &mut self,
            a: Shot,
            b: Shot,
            mix: f64,
            gain: f64,
            pan: f64,
            n: u32,
            top: u32,
            bottom: u32,
            bar: u32,
        ) -> V210<'_> {
            let cols = Columns::new(self.w, pan);
            self.bytes.clear();
            for y in 0..self.h {
                let r = RowTerms::new(y, self.h);
                for x in 0..self.w as usize {
                    let c = if y < top || y >= self.h - bottom {
                        f64::from(bar)
                    } else {
                        let mut c =
                            (1.0 - mix) * shot(a, &cols, &r, x) + mix * shot(b, &cols, &r, x);
                        c = 64.0 + (c - 64.0) * gain;
                        c + f64::from((hash(x as u32, y, n) % 7) as i32 - 3)
                    };
                    self.luma[x] = c.clamp(4.0, 1019.0) as u16;
                }
                self.bytes
                    .extend_from_slice(Row::new(self.w, &self.luma, &self.chroma).bytes());
            }
            V210::new(&self.bytes, self.w, self.h, row_bytes(self.w)).expect("whole rows")
        }

        fn plain(&mut self, a: Shot, n: u32) -> V210<'_> {
            self.draw(a, a, 0.0, 1.0, 0.0, n, 0, 0, 64)
        }
    }

    /// Bar rows for a ratio in a frame: the odd remainder goes to the bottom.
    fn bars_for(w: u32, h: u32, ar: f64) -> (u32, u32) {
        let active = ((f64::from(w) / ar + 0.5) as u32).min(h);
        let top = (h - active) / 2;
        (top, h - active - top)
    }

    /// Runs a sequence and counts cuts from frame `from` on.
    fn cuts_in(
        c: &mut Cuts,
        frames: u32,
        mut draw: impl FnMut(u32) -> CutObsInput,
        from: u32,
    ) -> u32 {
        c.reset();
        let mut cuts = 0;
        for n in 0..frames {
            let (f, top, bottom) = draw(n);
            let o = c.observe(&f, top, bottom);
            if o.cut && n >= from {
                cuts += 1;
            }
        }
        cuts
    }

    type CutObsInput = (OwnedV210, u32, u32);

    /// A packed frame that owns its bytes, so a closure can hand one out.
    struct OwnedV210 {
        bytes: Vec<u8>,
        w: u32,
        h: u32,
    }

    impl OwnedV210 {
        fn of(v: V210<'_>) -> Self {
            OwnedV210 {
                bytes: v.bytes().to_vec(),
                w: v.width(),
                h: v.height(),
            }
        }

        /// The frame as `play` hands it over, with v210's own reads.
        fn v210(&self) -> V210<'_> {
            V210::new(&self.bytes, self.w, self.h, row_bytes(self.w)).expect("whole rows")
        }
    }

    impl Samples for OwnedV210 {
        fn width(&self) -> u32 {
            self.w
        }
        fn height(&self) -> u32 {
            self.h
        }
        fn luma_row(&self, y: u32, out: &mut [u16]) {
            V210::new(&self.bytes, self.w, self.h, row_bytes(self.w))
                .expect("whole rows")
                .luma_row(y, out)
        }
        fn chroma_row(&self, y: u32, out: &mut [[u16; 2]]) {
            V210::new(&self.bytes, self.w, self.h, row_bytes(self.w))
                .expect("whole rows")
                .chroma_row(y, out)
        }
    }

    // Sequences run at 1080p, where the grid is the same 32 x 18 cells as at
    // 2160p and a frame costs a quarter as much to draw; crops, baselines and
    // cost use 2160p.
    const W: u32 = 3840;
    const H: u32 = 2160;

    #[test]
    fn first_frame_has_nothing_to_compare_with() {
        let mut s = Frame::new(1920, 1080);
        let mut cuts = Cuts::default();
        let o = cuts.observe(&s.plain(Shot::A, 0), 0, 0);
        assert!(!o.compared && !o.cut);
        assert_eq!(o.rows, 18, "1080p full frame: 32 x 18 cells");
    }

    #[test]
    fn within_a_shot_nothing_is_a_cut() {
        let mut s = Frame::new(1920, 1080);
        let mut cuts = Cuts::default();
        let mut seq = |frames: u32, f: &mut dyn FnMut(&mut Frame, u32) -> V210<'_>| {
            cuts_in(
                &mut cuts,
                frames,
                |n| (OwnedV210::of(f(&mut s, n)), 0, 0),
                0,
            )
        };
        assert_eq!(
            seq(24, &mut |s, n| s.plain(Shot::A, n)),
            0,
            "static shot with grain"
        );
        assert_eq!(
            seq(24, &mut |s, n| s.draw(
                Shot::A,
                Shot::A,
                0.0,
                1.0,
                8.0 * f64::from(n),
                n,
                0,
                0,
                64
            )),
            0,
            "slow pan"
        );
        assert_eq!(
            seq(24, &mut |s, n| s.draw(
                Shot::A,
                Shot::A,
                0.0,
                1.0,
                48.0 * f64::from(n),
                n,
                0,
                0,
                64
            )),
            0,
            "fast pan (a screen width in 1.7 s)"
        );
        assert_eq!(
            seq(24, &mut |s, n| s.draw(
                Shot::A,
                Shot::A,
                0.0,
                1.0 - f64::from(n) / 23.0,
                0.0,
                n,
                0,
                0,
                64
            )),
            0,
            "fade to black over 1 s"
        );
        assert_eq!(
            seq(12, &mut |s, n| s.draw(
                Shot::B,
                Shot::B,
                0.0,
                f64::from(n) / 11.0,
                0.0,
                n,
                0,
                0,
                64
            )),
            0,
            "fade up from black over 0.5 s"
        );
        assert_eq!(
            seq(12, &mut |s, n| s.draw(
                Shot::A,
                Shot::B,
                f64::from(n) / 11.0,
                1.0,
                0.0,
                n,
                0,
                0,
                64
            )),
            0,
            "cross-fade over 0.5 s"
        );
    }

    /// The case the recorded clips found: a camera rushing through corn
    /// changes most cells by 50-80 codes on every frame and passes every
    /// threshold. Without the spike rule this reads as a cut per frame, which
    /// is what makes it a test that can fail. The first moving frame has no
    /// previous change to be a spike against, so counting starts at the
    /// second.
    #[test]
    fn fast_motion_through_texture_is_not_a_cut_because_of_the_spike_rule() {
        let mut s = Frame::new(1920, 1080);
        let pan = |s: &mut Frame, n: u32| {
            OwnedV210::of(s.draw(
                Shot::Texture,
                Shot::Texture,
                0.0,
                1.0,
                40.0 * f64::from(n),
                n,
                0,
                0,
                64,
            ))
        };
        let mut no_spike = Cuts::new(Params {
            spike: 0.0,
            ..Params::default()
        });
        assert!(
            cuts_in(&mut no_spike, 24, |n| (pan(&mut s, n), 0, 0), 2) >= 20,
            "fast motion through busy detail clears the base thresholds"
        );
        let mut cuts = Cuts::default();
        assert_eq!(
            cuts_in(&mut cuts, 24, |n| (pan(&mut s, n), 0, 0), 2),
            0,
            "... and the spike rule leaves no cut in it"
        );
        assert_eq!(
            cuts_in(
                &mut cuts,
                24,
                |n| {
                    let f = if n < 8 {
                        OwnedV210::of(s.plain(Shot::A, n))
                    } else {
                        pan(&mut s, n)
                    };
                    (f, 0, 0)
                },
                0
            ),
            1,
            "a cut from a still shot into fast motion: one cut, not one per frame"
        );
    }

    #[test]
    fn between_shots_is_a_cut() {
        let mut s = Frame::new(1920, 1080);
        let mut cuts = Cuts::default();
        let mut pair = |a: Shot, b: Shot| {
            cuts.reset();
            let first = OwnedV210::of(s.plain(a, 0));
            cuts.observe(&first, 0, 0);
            cuts.observe(&s.plain(b, 1), 0, 0).cut
        };
        assert!(pair(Shot::A, Shot::B), "hard cut A -> B");
        assert!(pair(Shot::B, Shot::A), "hard cut B -> A");
        assert!(
            pair(Shot::A, Shot::AInverted),
            "cut to a shot with the same histogram (mean alone)"
        );
        assert!(pair(Shot::B, Shot::Black), "cut to black");
    }

    #[test]
    fn the_grid_covers_the_crop() {
        let mut f = Frame::new(W, H);
        let mut cuts = Cuts::default();
        let (t, b) = bars_for(W, H, 2.39);
        cuts.reset();
        assert_eq!(
            cuts.observe(&f.draw(Shot::A, Shot::A, 0.0, 1.0, 0.0, 0, t, b, 64), t, b)
                .rows,
            13,
            "2.39 crop: 13 cell rows"
        );
        let (t2, b2) = bars_for(W, H, 2.76);
        cuts.reset();
        assert_eq!(
            cuts.observe(
                &f.draw(Shot::A, Shot::A, 0.0, 1.0, 0.0, 0, t2, b2, 64),
                t2,
                b2
            )
            .rows,
            12,
            "2.76 crop: 12 cell rows"
        );

        // Whatever happens in the bars is outside the grid: a subtitle or a
        // menu appearing there changes nothing.
        cuts.reset();
        let base = OwnedV210::of(f.draw(Shot::A, Shot::A, 0.0, 1.0, 0.0, 0, t, b, 64));
        cuts.observe(&base, t, b);
        assert!(
            !cuts
                .observe(&f.draw(Shot::A, Shot::A, 0.0, 1.0, 0.0, 1, t, b, 900), t, b)
                .cut,
            "bars changing under a 2.39 crop: no cut"
        );
        assert!(
            cuts.observe(&f.draw(Shot::B, Shot::B, 0.0, 1.0, 0.0, 2, t, b, 900), t, b)
                .cut,
            "the picture changing under a 2.39 crop: cut"
        );
    }

    #[test]
    fn baselines() {
        let mut f = Frame::new(W, H);
        let mut cuts = Cuts::default();
        let (t, b) = bars_for(W, H, 2.39);
        cuts.reset();
        let full = OwnedV210::of(f.plain(Shot::A, 0));
        cuts.observe(&full, 0, 0);
        let o = cuts.observe(&f.draw(Shot::B, Shot::B, 0.0, 1.0, 0.0, 1, t, b, 64), t, b);
        assert!(
            !o.compared && !o.cut,
            "a crop change starts a new baseline, never a cut"
        );
        let o = cuts.observe(&f.draw(Shot::A, Shot::A, 0.0, 1.0, 0.0, 2, t, b, 64), t, b);
        assert!(
            o.compared && o.cut,
            "... and the next frame at that crop compares again"
        );
        cuts.reset();
        let o = cuts.observe(&f.draw(Shot::B, Shot::B, 0.0, 1.0, 0.0, 3, t, b, 64), t, b);
        assert!(!o.compared && !o.cut, "reset() forgets the previous frame");
        assert!(
            !cuts.observe(&f.plain(Shot::A, 4), H, 0).compared,
            "a crop covering the whole frame is refused"
        );
    }

    #[test]
    fn other_sizes() {
        let mut f = Frame::new(W, H);
        let mut c = Cuts::default();
        let first = OwnedV210::of(f.plain(Shot::A, 0));
        c.observe(&first, 0, 0);
        let o = c.observe(&f.plain(Shot::B, 1), 0, 0);
        assert!(o.rows == 18 && o.cut, "2160p: 32 x 18 cells, cut detected");

        // A width that is not a multiple of six ends on a partial group.
        let mut k = Frame::new(1918, 1080);
        c.reset();
        let first = OwnedV210::of(k.plain(Shot::A, 0));
        c.observe(&first, 0, 0);
        let p = c.observe(&k.plain(Shot::A, 1), 0, 0);
        assert!(
            p.compared && !p.cut,
            "partial trailing group: reads, no cut within a shot"
        );
    }

    #[test]
    fn cost_is_well_under_a_millisecond() {
        let mut f = Frame::new(W, H);
        let a = OwnedV210::of(f.plain(Shot::A, 0));
        let mut cuts = Cuts::default();
        let n = 50;
        let t0 = std::time::Instant::now();
        for _ in 0..n {
            cuts.observe(&a.v210(), 0, 0);
        }
        let us = t0.elapsed().as_secs_f64() * 1e6 / f64::from(n);
        eprintln!("cuts: {us:.1} us per frame at 2160p");
        if !cfg!(debug_assertions) {
            assert!(us < 500.0, "{us} us");
        }
    }
}
