//! Part 2 of the bar detector: observations over time to a crop.
//!
//! One asymmetry decides everything: zooming out wrongly shows a sliver of
//! bar, which is cheap; zooming in wrongly cuts the top and bottom off a menu
//! or an IMAX shot, which is the mistake people notice. So zoom out on the
//! first frame that shows picture beyond the crop, and zoom in only when
//! evidence (frame strength x persistence) crosses a threshold set by how
//! familiar the height is.
//!
//! Pure: part 1's [`Observation`] and a cut flag in, a [`Decision`] out. No
//! pixels, so a whole clip's reading sequence replays in milliseconds.

use crate::bars::{FrameClass, Observation, SideObs};

/// Rows of bar to crop. `{0, 0}` is the full frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Crop {
    pub top: u32,
    pub bottom: u32,
}

impl Crop {
    pub fn active(self, height: u32) -> u32 {
        height.saturating_sub(self.top + self.bottom)
    }
}

/// Rows are at 2160 and scale with frame height. Every value here is a
/// starting point: they were set by synthetic sequences and then by replaying
/// recorded clips through the C++ version.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    /// Rows a height must sit inside the crop to zoom in to; and a match to
    /// a standard ratio (2.39 and 2.40 are 7 apart).
    pub tol_ar: u32,
    /// Rows between edges for "the same height".
    pub tol_match: u32,
    /// Rows picture must show beyond the crop to zoom out.
    pub tol_out: u32,
    /// A letterbox this many rows taller than the crop, at most, widens it in
    /// place: under 2.39 vs 2.40's 7, the closest two standard heights.
    pub widen_max: u32,
    /// Picture at most this many rows past the crop on a side widens it
    /// rather than zooming out, and never further than this past the height
    /// zoomed in to. Its own limit, not `widen_max`: a side reading into its
    /// bar in a dark scene went up to 8-9 rows on a 2.00 show, and the next
    /// real height up is ~90 rows a side away (2.20 from 2.39).
    pub intrude_max: u32,
    /// A height on screen this recently returns at `recent`.
    pub recent_s: f64,
    /// An unconfirmed zoom-in drops its height to `known` this long.
    pub penalty_s: f64,
    /// Of nothing but EMPTY: a title change with no mode change.
    pub idle_reset_s: f64,
    /// No zoom-in for this long after UI was seen in a bar. A transport bar
    /// or menu stays up for seconds, and a frame where its density dips under
    /// the UI threshold must not let the crop snap back under it.
    pub ui_hold_s: f64,
    /// Evidence a candidate needs, in frames' worth at full strength. A recent
    /// height can come back on the first frame of its cut; a never-seen one
    /// never does on one frame, because a single frame corroborates nothing.
    pub threshold_recent: f64,
    pub threshold_known: f64,
    pub threshold_new: f64,
    /// Per frame of streak ...
    pub persistence_step: f64,
    /// ... up to this multiple.
    pub persistence_max: f64,
    /// Strength factor for a height near, not on, a standard or known one.
    pub inexact: f64,
    /// A height near no standard ratio and not known is adopted only once
    /// both edges have held still (within `tol_match`) through this many
    /// seconds of letterbox frames, across at least `nonstd_cuts` cuts. Across
    /// a cut the picture changes completely: real bars stay put, a dark
    /// scene's apparent edge moves with it, and a static menu has no cut at
    /// all. Apple TV trailers at 1.86 (48/48) and 2.08 (158/158) hold their
    /// edges to a row for 5-20 s across several cuts.
    pub nonstd_s: f64,
    pub nonstd_cuts: u32,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            tol_ar: 4,
            tol_match: 2,
            tol_out: 2,
            widen_max: 6,
            intrude_max: 12,
            recent_s: 15.0 * 60.0,
            penalty_s: 60.0,
            idle_reset_s: 30.0 * 60.0,
            ui_hold_s: 1.0,
            threshold_recent: 0.5,
            threshold_known: 2.0,
            threshold_new: 6.0,
            persistence_step: 0.25,
            persistence_max: 4.0,
            inexact: 0.8,
            nonstd_s: 2.0,
            nonstd_cuts: 1,
        }
    }
}

/// A crop the policy is considering, and its progress.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Candidate {
    pub crop: Crop,
    pub evidence: f32,
    pub threshold: f32,
}

/// What the policy shows of its reasoning, for the log and the on-screen
/// overlay. Everything here is diagnostic; only [`Decision::crop`] drives
/// geometry.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Diagnostics {
    /// LETTERBOX, WEAK, ...
    pub frame_class: &'static str,
    /// This frame's measured bar rows.
    pub edge_top: Option<u32>,
    pub edge_bottom: Option<u32>,
    pub candidate: Option<Candidate>,
    /// Why the crop last changed, or "".
    pub reason: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Decision {
    pub crop: Crop,
    /// The crop differs from the previous frame's.
    pub changed: bool,
    pub diag: Diagnostics,
}

/// Time spent in one adopted state, by its active rows (the frame height for
/// full frame). The display AR is width / rows, never a rounded constant.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Share {
    pub rows: u32,
    pub seconds: f64,
}

/// The standard ratios wider than 16:9. Anything 1.78 or narrower arrives
/// full height and needs no state of its own.
const STANDARD: [f64; 9] = [1.85, 1.90, 2.00, 2.20, 2.35, 2.39, 2.40, 2.55, 2.76];

struct Known {
    rows: Crop,
    last_on_screen: f64,
    penalised_until: f64,
}

#[derive(Clone, Copy, PartialEq)]
enum Prior {
    Recent,
    Known,
    New,
}

struct Cand {
    rows: Crop,
    evidence: f64,
    streak: u32,
}

/// The non-standard candidate: its own state, since a cut is part of its
/// evidence rather than a reason to drop it.
struct NonStd {
    rows: Crop,
    /// Seconds of letterbox frames at its rows.
    held: f64,
    cuts: u32,
}

pub struct ArPolicy {
    p: Params,
    started: bool,
    epoch: u32,
    width: u32,
    height: u32,
    crop: Crop,
    known: Vec<Known>,
    candidate: Option<Cand>,
    /// The last zoom-in, until a frame after it confirms it or a zoom out
    /// ends it.
    pending_confirm: bool,
    zoomed_to: Crop,
    /// The height the last zoom-in adopted, which widening may not move a
    /// side more than `intrude_max` from; and whether the crop is a widened
    /// one now.
    adopted: Crop,
    widened: bool,
    nonstd: Option<NonStd>,
    prev_t: f64,
    /// Last non-EMPTY frame.
    last_seen: f64,
    /// Last frame with UI in a bar.
    last_ui: f64,
    last_t: f64,
    shares: Vec<Share>,
}

impl Default for ArPolicy {
    fn default() -> Self {
        Self::new(Params::default())
    }
}

impl ArPolicy {
    pub fn new(p: Params) -> Self {
        ArPolicy {
            p,
            started: false,
            epoch: 0,
            width: 0,
            height: 0,
            crop: Crop::default(),
            known: Vec::new(),
            candidate: None,
            pending_confirm: false,
            zoomed_to: Crop::default(),
            adopted: Crop::default(),
            widened: false,
            nonstd: None,
            prev_t: 0.0,
            last_seen: 0.0,
            last_ui: -1e9,
            last_t: 0.0,
            shares: Vec::new(),
        }
    }

    pub fn params(&self) -> &Params {
        &self.p
    }

    pub fn crop(&self) -> Crop {
        self.crop
    }

    /// Cumulative over the policy's life: a reset forgets heights, not
    /// history.
    pub fn shares(&self) -> &[Share] {
        &self.shares
    }

    fn tol(&self, rows_at_2160: u32) -> u32 {
        if self.height == 0 {
            rows_at_2160
        } else {
            ((u64::from(rows_at_2160) * u64::from(self.height) / 2160) as u32).max(1)
        }
    }

    /// Rows, not heights: two crops are the same only if both edges agree.
    fn same(&self, a: Crop, b: Crop) -> bool {
        let t = self.tol(self.p.tol_match);
        a.top.abs_diff(b.top) <= t && a.bottom.abs_diff(b.bottom) <= t
    }

    fn find_known(&self, rows: Crop) -> Option<usize> {
        self.known.iter().position(|k| self.same(k.rows, rows))
    }

    fn near_standard(&self, rows: Crop, within: u32) -> bool {
        let a = rows.active(self.height);
        STANDARD.iter().any(|&r| {
            let s = (f64::from(self.width) / r).round() as u32;
            s < self.height && a.abs_diff(s) <= within
        })
    }

    fn prior_of(&self, k: Option<usize>, t: f64) -> Prior {
        match k {
            None => Prior::New,
            Some(i) => {
                let k = &self.known[i];
                if t - k.last_on_screen <= self.p.recent_s && t >= k.penalised_until {
                    Prior::Recent
                } else {
                    Prior::Known
                }
            }
        }
    }

    fn threshold(&self, p: Prior) -> f64 {
        match p {
            Prior::Recent => self.p.threshold_recent,
            Prior::Known => self.p.threshold_known,
            Prior::New => self.p.threshold_new,
        }
    }

    /// Reset means full frame, no known heights, no candidate: starting tall
    /// is the safe direction, and re-learning costs one never-seen zoom-in.
    fn reset(&mut self, t: f64, epoch: u32) {
        self.epoch = epoch;
        self.crop = Crop::default();
        self.known.clear();
        self.candidate = None;
        self.nonstd = None;
        self.pending_confirm = false;
        self.adopted = Crop::default();
        self.widened = false;
        self.last_seen = t;
        self.last_ui = -1e9;
    }

    /// Time share: the interval since the last frame belongs to the crop held
    /// through it. Gaps longer than a second are signal loss or a stall, not
    /// viewing.
    fn tally(&mut self, t: f64) {
        let dt = t - self.last_t;
        self.last_t = t;
        if !(dt > 0.0 && dt <= 1.0) || self.height == 0 {
            return;
        }
        let rows = self.crop.active(self.height);
        match self.shares.iter_mut().find(|s| s.rows == rows) {
            Some(s) => s.seconds += dt,
            None => self.shares.push(Share { rows, seconds: dt }),
        }
    }

    /// One frame. `cut` comes from [`crate::cuts::Cuts`] over the current
    /// [`crop`](Self::crop); `epoch` is bumped by the caller on a change of
    /// format or on signal loss.
    pub fn push(&mut self, o: &Observation, cut: bool, t: f64, epoch: u32) -> Decision {
        let mut d = Decision::default();
        let before = self.crop;
        let resized = o.width != 0 && (o.width != self.width || o.height != self.height);

        if self.started {
            self.tally(t);
        } else {
            self.last_t = t;
        }

        // C. Reset on a new epoch (format change, signal loss), a new frame
        // size, or a long stretch of nothing: the next title on an Apple TV
        // arrives with no mode change. Known heights do not otherwise expire.
        if !self.started
            || epoch != self.epoch
            || resized
            || t - self.last_seen > self.p.idle_reset_s
        {
            if self.started {
                d.diag.reason = "reset";
            }
            self.started = true;
            if o.width != 0 {
                self.width = o.width;
                self.height = o.height;
            }
            self.reset(t, epoch);
        }

        d.diag.frame_class = o.class.name();
        d.diag.edge_top = o.top.found.then_some(o.top.bar);
        d.diag.edge_bottom = o.bottom.found.then_some(o.bottom.bar);

        // A cut: evidence from the previous shot stops counting. A hint, never
        // a gate. For a non-standard candidate it is the opposite: bars that
        // hold through a cut are what tells them from a dark scene's edge.
        if cut {
            self.candidate = None;
            if let Some(n) = &mut self.nonstd {
                n.cuts += 1;
            }
        }
        let dt = t - self.prev_t;
        self.prev_t = t;

        if o.class != FrameClass::Empty {
            self.last_seen = t;
        }
        // Text in both bars is a menu (see A): it holds off zoom-ins as UI
        // does.
        if o.top.ui || o.bottom.ui || (o.top.islands > 0 && o.bottom.islands > 0) {
            self.last_ui = t;
        }

        let mut zoomed_out = false;

        // A. Zoom out on the first frame that shows picture beyond the crop:
        // on both sides, or on one side running unbroken into the picture
        // (dark sky above a scope crop). A subtitle is one-sided and detached,
        // so it cannot. UI in the bar (a dense island) can, even one-sided: a
        // cropped menu is unusable.
        if o.class != FrameClass::Empty {
            let out = self.tol(self.p.tol_out);
            let beyond = |s: &SideObs, edge: u32| s.found && s.bar + out < edge;
            let top_out = beyond(&o.top, self.crop.top);
            let bot_out = beyond(&o.bottom, self.crop.bottom);
            let top_joined = top_out && o.top.bar + o.top.run >= self.crop.top;
            let bot_joined = bot_out && o.bottom.bar + o.bottom.run >= self.crop.bottom;
            let top_ui = o.top.ui && o.top.ui_start + out < self.crop.top;
            let bot_ui = o.bottom.ui && o.bottom.ui_start + out < self.crop.bottom;
            // Sparse text beyond the crop. Alone in one bar it is a subtitle,
            // and stays cropped by decision; but a subtitle is one-sided, so
            // text (or picture) beyond the crop on *both* sides is a menu
            // drawn as bare text. Interstellar's UHD menu is one: 8 lines of
            // text in the top bar and a thin rule in the bottom one, no panel,
            // read as subtitles until this.
            let top_text = o.top.islands > 0 && o.top.island_start + out < self.crop.top;
            let bot_text = o.bottom.islands > 0 && o.bottom.island_start + out < self.crop.bottom;
            let both = (top_out || top_text) && (bot_out || bot_text);

            if both || top_joined || bot_joined || top_ui || bot_ui {
                let mut seen = self.crop;
                if top_out {
                    seen.top = o.top.bar;
                }
                if bot_out {
                    seen.bottom = o.bottom.bar;
                }
                if both && top_text {
                    seen.top = seen.top.min(o.top.island_start);
                }
                if both && bot_text {
                    seen.bottom = seen.bottom.min(o.bottom.island_start);
                }
                if top_ui {
                    seen.top = seen.top.min(o.top.ui_start);
                }
                if bot_ui {
                    seen.bottom = seen.bottom.min(o.bottom.ui_start);
                }

                // The tightest known height that contains everything seen,
                // else full frame. Containment is strict: a known height a row
                // short of what is lit would crop that row, and zooming out
                // too far is the cheap mistake: if the height was right it is
                // recent, and comes back on its next frame.
                let mut next = Crop::default();
                for k in &self.known {
                    if k.rows.top <= seen.top
                        && k.rows.bottom <= seen.bottom
                        && k.rows.top + k.rows.bottom > next.top + next.bottom
                    {
                        next = k.rows;
                    }
                }

                // A clean letterbox only slightly taller than the crop is the
                // same picture measured a few rows wider: a matte still
                // opening, or the crop adopted a row tight. Widen to its rows
                // rather than going to full frame and back: La La Land's
                // opening grows from 329/329 to 326/327 after 2.55 is adopted,
                // and the trip out and in two frames later was a visible
                // jump. The known height widens with it, so a return lands on
                // the wider rows.
                let rows = Crop {
                    top: o.top.bar,
                    bottom: o.bottom.bar,
                };
                let no_text_or_ui = !top_ui && !bot_ui && !top_text && !bot_text;
                let nudge = o.class == FrameClass::Letterbox
                    && no_text_or_ui
                    && self.crop != Crop::default()
                    && rows.top <= seen.top
                    && rows.bottom <= seen.bottom
                    && rows.active(self.height)
                        <= self.crop.active(self.height) + self.tol(self.p.widen_max);
                // Picture a few rows past the crop on a side (noise or a soft
                // edge in a dark scene, the bar's own edge wandering by a row
                // or three) is the same picture: widen to it rather than zoom
                // out. A 2.00 show in a dark scene read 3-6 rows into its
                // bars, one side at a time, and zoomed out and back every few
                // seconds; Interstellar's one false zoom out is a 6-row
                // intrusion. Never more than intrude_max past the height
                // zoomed in to, so it cannot creep: past that, a side is real
                // picture and zooms out.
                let wmax = self.tol(self.p.intrude_max);
                let mut wide = self.crop;
                if top_out {
                    wide.top = o.top.bar;
                }
                if bot_out {
                    wide.bottom = o.bottom.bar;
                }
                let small = !nudge
                    && self.crop != Crop::default()
                    && no_text_or_ui
                    && (top_out || bot_out)
                    && wide.top + wmax >= self.crop.top
                    && wide.bottom + wmax >= self.crop.bottom
                    && wide.top + wmax >= self.adopted.top
                    && wide.bottom + wmax >= self.adopted.bottom;
                if nudge || small {
                    // A wider clean letterbox is the picture itself grown (a
                    // matte opening): the known height and the reference move
                    // with it. A few noisy rows are not: the known height
                    // keeps its rows, so a return lands where the picture
                    // really is and still counts as recent; rewriting it made
                    // Interstellar's next return to scope a "new" height, 5
                    // frames late.
                    if nudge {
                        if let Some(i) = self.find_known(self.crop) {
                            self.known[i].rows = rows;
                        }
                        self.adopted = rows;
                    }
                    next = if nudge { rows } else { wide };
                    self.widened = true;
                } else {
                    self.widened = false;
                    // A known height zoomed out to bounds its own widening.
                    self.adopted = next;
                }

                // Zoomed out before any frame backed the zoom-in up: it was
                // wrong. Confirmation, not elapsed time: Interstellar's rapid
                // scope/IMAX intercutting undoes zoom-ins quickly and
                // correctly.
                if self.pending_confirm
                    && self.same(self.zoomed_to, self.crop)
                    && !nudge
                    && !small
                    && let Some(i) = self.find_known(self.crop)
                {
                    self.known[i].penalised_until = t + self.p.penalty_s;
                }

                self.crop = next;
                self.candidate = None;
                if !small {
                    self.pending_confirm = false;
                }
                zoomed_out = true;
                d.diag.reason = if nudge || small {
                    "widen"
                } else if top_ui || bot_ui {
                    "zoom out (UI)"
                } else if top_out && bot_out {
                    "zoom out"
                } else if both {
                    "zoom out (text)"
                } else {
                    "zoom out (one side)"
                };
            }
        }

        // B. Zoom in when evidence crosses the threshold for the candidate's
        // prior. Only LETTERBOX frames count, and only at a standard or known
        // height inside the crop. WEAK and EMPTY pause the streak: a fade or a
        // title card in the middle of a switch neither advances nor resets it.
        if !zoomed_out && o.class == FrameClass::Letterbox {
            let rows = Crop {
                top: o.top.bar,
                bottom: o.bottom.bar,
            };
            if self.pending_confirm && self.same(rows, self.crop) {
                self.pending_confirm = false;
            }

            let slack = self.tol(if self.widened {
                2 * self.p.intrude_max
            } else {
                self.p.tol_ar
            });
            // At the current crop or taller: nothing to do. After a widen, a
            // frame up to intrude_max a side inside the crop is the same
            // picture measured tight, not a new height to zoom in to.
            if rows.active(self.height) + slack >= self.crop.active(self.height) {
                self.candidate = None;
                self.nonstd = None;
            } else if t - self.last_ui < self.p.ui_hold_s {
                // UI in a bar: the film behind a transport bar or a pop-up
                // menu still reads as a clean letterbox, and zooming in to it
                // crops the UI being used. Seen on a disc while
                // fast-forwarding: zoom out (UI), zoom in two frames later, ~5
                // times a second.
                self.candidate = None;
                self.nonstd = None;
            } else {
                let k = self.find_known(rows);
                // A height near no standard ratio is held to a stricter test
                // (nonstd_s, nonstd_cuts): that is what rejects a menu whose
                // dark edges look letterboxed at an arbitrary height.
                if k.is_none() && !self.near_standard(rows, self.tol(self.p.tol_ar)) {
                    self.candidate = None;
                    let held = self
                        .nonstd
                        .as_ref()
                        .is_some_and(|n| self.same(n.rows, rows));
                    match &mut self.nonstd {
                        Some(n) if held => {
                            if dt > 0.0 && dt <= 1.0 {
                                n.held += dt;
                            }
                        }
                        _ => {
                            self.nonstd = Some(NonStd {
                                rows,
                                held: 0.0,
                                cuts: 0,
                            });
                        }
                    }
                    let n = self.nonstd.as_ref().expect("set above");
                    if n.held >= self.p.nonstd_s && n.cuts >= self.p.nonstd_cuts {
                        self.crop = rows;
                        self.known.push(Known {
                            rows: self.crop,
                            last_on_screen: t,
                            penalised_until: -1.0,
                        });
                        self.pending_confirm = true;
                        self.zoomed_to = self.crop;
                        self.adopted = self.crop;
                        self.widened = false;
                        self.nonstd = None;
                        d.diag.reason = "zoom in (non-standard)";
                    }
                } else {
                    self.nonstd = None;
                    let continuing = self
                        .candidate
                        .as_ref()
                        .is_some_and(|c| self.same(c.rows, rows));
                    match &mut self.candidate {
                        Some(c) if continuing => c.streak += 1,
                        _ => {
                            self.candidate = Some(Cand {
                                rows,
                                evidence: 0.0,
                                streak: 1,
                            });
                        }
                    }
                    // Persistence is what a dark scene cannot fake: real bars
                    // hold both edges on the same rows frame after frame,
                    // while dark content's apparent edges wander with the
                    // picture.
                    let exact = k.is_some() || self.near_standard(rows, self.tol(self.p.tol_match));
                    let prior = self.prior_of(k, t);
                    let threshold = self.threshold(prior);
                    let c = self.candidate.as_mut().expect("set above");
                    let persistence = self
                        .p
                        .persistence_max
                        .min(1.0 + self.p.persistence_step * f64::from(c.streak - 1));
                    c.evidence +=
                        o.strength * if exact { 1.0 } else { self.p.inexact } * persistence;

                    if c.evidence >= threshold {
                        // Rows, not ratios: the measured rows, or a known
                        // height's own rows so a return lands exactly where it
                        // was, but never tighter than this frame's picture. A
                        // known height matches within tol_match, and La La
                        // Land's opening grows from 329/329 to 326/327:
                        // reusing 329/329 on a 327/327 frame cut two rows, the
                        // next frame showed three past the crop and zoomed
                        // out, and it flapped until the picture was far enough
                        // off to count as new. So the known height widens to
                        // contain the frame, and keeps the wider rows: at
                        // worst a row of bar shows.
                        self.crop = match k {
                            Some(i) => {
                                let kr = &mut self.known[i].rows;
                                kr.top = kr.top.min(rows.top);
                                kr.bottom = kr.bottom.min(rows.bottom);
                                *kr
                            }
                            None => {
                                self.known.push(Known {
                                    rows,
                                    last_on_screen: t,
                                    penalised_until: -1.0,
                                });
                                rows
                            }
                        };
                        self.pending_confirm = true;
                        self.zoomed_to = self.crop;
                        self.adopted = self.crop;
                        self.widened = false;
                        self.candidate = None;
                        d.diag.reason = match prior {
                            Prior::Recent => "zoom in (recent)",
                            Prior::Known => "zoom in (known)",
                            Prior::New => "zoom in (new)",
                        };
                    }
                }
            }
        } else if !zoomed_out && o.class == FrameClass::Full {
            self.candidate = None;
            self.nonstd = None;
        }

        // On screen: the crop's height, or while widened, the height it
        // widened from, which is the one a return will look for.
        let on = if self.widened {
            self.adopted
        } else {
            self.crop
        };
        if on != Crop::default()
            && let Some(i) = self.find_known(on)
        {
            self.known[i].last_on_screen = t;
        }

        d.crop = self.crop;
        d.changed = self.crop != before;
        if let Some(c) = &self.candidate {
            let prior = self.prior_of(self.find_known(c.rows), t);
            d.diag.candidate = Some(Candidate {
                crop: c.rows,
                evidence: c.evidence as f32,
                threshold: self.threshold(prior) as f32,
            });
        }
        d
    }
}

#[cfg(test)]
mod tests {
    //! The C++ version's `ar_policy_test`: synthetic observation sequences,
    //! every row of its expected-behaviour table plus the rules behind them
    //! (priors, the unconfirmed zoom-in penalty, cuts, resets). No pixels: the
    //! observations are built directly, the way Bars would report each
    //! situation.
    use super::*;
    use crate::synth::hash;

    const W: u32 = 3840;
    const H: u32 = 2160;
    const FRAME: f64 = 1.0 / 24.0;
    const FULL: Crop = Crop { top: 0, bottom: 0 };
    /// 2.39: 1607 rows, the odd row at the bottom.
    const SCOPE: Crop = Crop {
        top: 276,
        bottom: 277,
    };
    /// 1.90: 2021 rows.
    const K190: Crop = Crop {
        top: 69,
        bottom: 70,
    };

    fn crop(top: u32, bottom: u32) -> Crop {
        Crop { top, bottom }
    }

    fn side(bar: u32, run: u32) -> SideObs {
        SideObs {
            found: true,
            bar,
            run,
            zones: 4,
            fill: 1.0,
            ..SideObs::default()
        }
    }

    fn empty_at(w: u32, h: u32) -> Observation {
        Observation {
            width: w,
            height: h,
            ..Observation::default()
        }
    }

    fn empty() -> Observation {
        empty_at(W, H)
    }

    fn full() -> Observation {
        Observation {
            class: FrameClass::Full,
            top: side(0, 300),
            bottom: side(0, 300),
            ..empty()
        }
    }

    fn letterbox_at(c: Crop, strength: f64, w: u32, h: u32) -> Observation {
        Observation {
            class: FrameClass::Letterbox,
            top: side(c.top, 300),
            bottom: side(c.bottom, 300),
            strength,
            ..empty_at(w, h)
        }
    }

    fn letterbox(c: Crop) -> Observation {
        letterbox_at(c, 1.0, W, H)
    }

    fn letterbox_s(c: Crop, strength: f64) -> Observation {
        letterbox_at(c, strength, W, H)
    }

    fn weak(top: Option<u32>, bottom: Option<u32>, run: u32) -> Observation {
        let mut o = empty();
        o.class = FrameClass::Weak;
        if let Some(t) = top {
            o.top = side(t, run);
        }
        if let Some(b) = bottom {
            o.bottom = side(b, run);
        }
        o
    }

    /// A policy with a clock. `feed` pushes one observation n times and
    /// returns the frame index (from 1) of the first crop change, or 0 if
    /// none.
    struct Run {
        p: ArPolicy,
        t: f64,
        epoch: u32,
        last: Decision,
        changes: u32,
    }

    impl Run {
        fn new() -> Self {
            Run {
                p: ArPolicy::default(),
                t: 0.0,
                epoch: 1,
                last: Decision::default(),
                changes: 0,
            }
        }

        fn push_cut(&mut self, o: &Observation, cut: bool) -> Decision {
            self.last = self.p.push(o, cut, self.t, self.epoch);
            self.t += FRAME;
            if self.last.changed {
                self.changes += 1;
            }
            self.last
        }

        fn push(&mut self, o: &Observation) -> Decision {
            self.push_cut(o, false)
        }

        fn feed(&mut self, o: &Observation, n: u32) -> u32 {
            let mut first = 0;
            for i in 1..=n {
                if self.push(o).changed && first == 0 {
                    first = i;
                }
            }
            first
        }

        /// Adopt a height from full frame the slow way, as at the start of a
        /// film. Switches are counted from here on.
        fn learn(&mut self, c: Crop) {
            self.feed(&letterbox(c), 20);
            self.changes = 0;
        }
    }

    #[test]
    fn a_scope_feature_is_one_zoom_in() {
        let mut r = Run::new();
        let at = r.feed(&letterbox(SCOPE), 24 * 60);
        assert_eq!(r.p.crop(), SCOPE, "adopts 276 / 277, the measured rows");
        assert!(
            at > 1 && at <= 12,
            "never-seen height: several frames, under 0.5 s bright ({at})"
        );
        assert_eq!(r.changes, 1, "one switch in a minute");
    }

    #[test]
    fn a_never_seen_height_is_never_adopted_on_one_frame() {
        let mut r = Run::new();
        r.push(&letterbox(SCOPE));
        assert_eq!(r.p.crop(), FULL);
    }

    #[test]
    fn menu_film_menu() {
        let mut r = Run::new();
        r.learn(SCOPE);
        r.feed(&letterbox(SCOPE), 48);
        assert!(
            r.feed(&full(), 1) == 1 && r.p.crop() == FULL,
            "menu: zoom out on its first frame"
        );
        r.feed(&full(), 240);
        assert!(
            r.feed(&letterbox(SCOPE), 1) == 1 && r.p.crop() == SCOPE,
            "film again: zoom in on its first frame (recent)"
        );
        assert_eq!(r.last.diag.reason, "zoom in (recent)");
    }

    #[test]
    fn rapid_scope_imax_intercutting_switches_on_frame_one() {
        let mut r = Run::new();
        r.learn(SCOPE);
        for shot in 0..40 {
            let imax = shot % 2 == 0;
            let n = if shot == 7 { 2 } else { 12 };
            let at = r.feed(&if imax { full() } else { letterbox(SCOPE) }, n);
            assert_eq!(at, 1, "shot {shot}");
            assert_eq!(r.p.crop(), if imax { FULL } else { SCOPE });
        }
    }

    #[test]
    fn three_ar_title() {
        let mut r = Run::new();
        let first_scope = r.feed(&letterbox(SCOPE), 48);
        assert!(
            r.feed(&letterbox(K190), 1) == 1 && r.p.crop() == FULL,
            "1.90 after scope, unknown: zoom out to full frame"
        );
        let first_190 = r.feed(&letterbox(K190), 48);
        assert!(
            first_scope > 1 && first_190 > 1 && r.p.crop() == K190,
            "1.90 adopted at the new threshold"
        );
        assert!(
            r.feed(&letterbox(SCOPE), 1) == 1 && r.p.crop() == SCOPE,
            "back to scope: instant"
        );
        assert!(
            r.feed(&letterbox(K190), 1) == 1 && r.p.crop() == K190,
            "back to 1.90: zoom out to the known 1.90, not full frame"
        );
        assert!(
            r.feed(&full(), 1) == 1 && r.p.crop() == FULL,
            "16:9: full frame"
        );
    }

    #[test]
    fn a_dark_shot_in_a_16_9_section_never_zooms_in() {
        let mut r = Run::new();
        for i in 0..24 * 30 {
            let h = hash(i, 11, 0);
            if h.is_multiple_of(3) {
                r.push(&weak(None, Some(h % 300), 300));
            } else {
                r.push(&letterbox_s(
                    crop(200 + (h >> 4) % 200, 200 + (h >> 12) % 200),
                    0.15,
                ));
            }
        }
        assert_eq!(r.changes, 0);
    }

    #[test]
    fn a_dark_shot_in_a_190_section_never_zooms_in() {
        let mut r = Run::new();
        r.learn(K190);
        for i in 0..24 * 30 {
            let h = hash(i, 12, 0);
            r.push(&letterbox_s(crop(276 + h % 40, 277 + (h >> 8) % 40), 0.15));
        }
        assert_eq!(r.p.crop(), K190);
    }

    #[test]
    fn a_dark_switch_to_scope_zooms_in_later_than_bright() {
        let (mut bright, mut dark) = (Run::new(), Run::new());
        let b = bright.feed(&letterbox_s(SCOPE, 1.0), 240);
        let d = dark.feed(&letterbox_s(SCOPE, 0.2), 240);
        assert!(
            d > b && dark.p.crop() == SCOPE,
            "bright frame {b}, dark frame {d}"
        );
    }

    #[test]
    fn subtitles_in_the_lower_bar_do_not_zoom_out() {
        let mut r = Run::new();
        r.learn(SCOPE);
        let mut o = letterbox(SCOPE);
        o.bottom.islands = 1;
        r.feed(&o, 48);
        r.feed(&weak(Some(276), None, 300), 48);
        r.feed(&weak(None, None, 300), 24);
        assert_eq!(r.changes, 0);
    }

    #[test]
    fn full_height_rolling_credits_zoom_out() {
        let mut r = Run::new();
        r.learn(SCOPE);
        assert!(r.feed(&weak(Some(40), Some(90), 20), 1) == 1 && r.p.crop() == FULL);
    }

    #[test]
    fn title_cards_on_black_hold_and_pause_the_streak() {
        let mut r = Run::new();
        r.learn(SCOPE);
        r.feed(&weak(Some(350), None, 300), 48);
        r.feed(&empty(), 48);
        assert!(
            r.changes == 0 && r.p.crop() == SCOPE,
            "title card on black: hold the crop"
        );
        // ... and in the middle of a switch they pause the streak, not reset
        // it.
        let (mut a, mut b) = (Run::new(), Run::new());
        let straight = a.feed(&letterbox(SCOPE), 48);
        let (mut paused, mut frames) = (0, 0);
        for i in 0..48 {
            if paused != 0 {
                break;
            }
            let card = (2..30).contains(&i);
            if !card {
                frames += 1;
            }
            let o = if card {
                weak(Some(350), None, 300)
            } else {
                letterbox(SCOPE)
            };
            if b.push(&o).changed {
                paused = frames;
            }
        }
        assert_eq!(
            paused, straight,
            "WEAK frames pause the streak, neither advance nor reset it"
        );
    }

    #[test]
    fn a_fade_to_black_holds_up_to_the_idle_reset() {
        let mut r = Run::new();
        r.learn(SCOPE);
        r.feed(&empty(), 24 * 120);
        assert_eq!(r.changes, 0, "fade to black for two minutes: hold");
        assert_eq!(
            r.feed(&letterbox(SCOPE), 24),
            0,
            "picture returns at scope: no switch"
        );
    }

    #[test]
    fn pillarboxed_content() {
        let mut r = Run::new();
        r.feed(&weak(Some(0), Some(0), 300), 240);
        assert_eq!(r.changes, 0, "pillarboxed from full frame: no state change");
        r.learn(SCOPE);
        assert!(
            r.feed(&weak(Some(0), Some(0), 300), 1) == 1 && r.p.crop() == FULL,
            "pillarboxed after scope: zoom out"
        );
    }

    #[test]
    fn a_dark_edged_menu_at_a_non_standard_height_is_never_adopted() {
        let mut r = Run::new();
        r.feed(&letterbox(crop(150, 150)), 24 * 30);
        assert_eq!(r.changes, 0);
    }

    #[test]
    fn one_side_running_into_the_picture_zooms_out_a_detached_one_holds() {
        let mut r = Run::new();
        r.learn(SCOPE);
        assert!(r.feed(&weak(Some(100), Some(277), 400), 1) == 1 && r.p.crop() == FULL);
        assert_eq!(r.last.diag.reason, "zoom out (one side)");
        let mut s = Run::new();
        s.learn(SCOPE);
        assert_eq!(
            s.feed(&weak(Some(100), Some(277), 40), 24),
            0,
            "one side detached from the picture: hold"
        );
    }

    #[test]
    fn ui_in_the_bar_zooms_out_even_one_sided() {
        let mut r = Run::new();
        r.learn(SCOPE);
        let mut o = letterbox(SCOPE);
        o.bottom.ui = true;
        o.bottom.ui_start = 120;
        assert!(r.feed(&o, 1) == 1 && r.p.crop() == FULL);
        assert_eq!(r.last.diag.reason, "zoom out (UI)");
    }

    #[test]
    fn a_menu_drawn_as_bare_text() {
        let mut r = Run::new();
        r.learn(SCOPE);
        let mut m = weak(None, Some(29), 7);
        m.top.islands = 8;
        m.top.island_start = 60;
        m.bottom.islands = 1;
        m.bottom.island_start = 120;
        assert!(
            r.feed(&m, 1) == 1 && r.p.crop() == FULL,
            "text in both bars (a menu): zoom out"
        );
        assert_eq!(r.last.diag.reason, "zoom out (text)");
        r.feed(&m, 24 * 5);
        assert_eq!(r.changes, 1, "and stays out while the menu is up");
        // Text in both bars over a letterboxed menu background: no zoom-in.
        let mut s = Run::new();
        s.learn(SCOPE);
        s.feed(&full(), 1);
        let mut lb = letterbox(SCOPE);
        lb.top.islands = 3;
        lb.top.island_start = 100;
        lb.bottom.islands = 1;
        lb.bottom.island_start = 120;
        assert_eq!(
            s.feed(&lb, 48),
            0,
            "letterboxed menu with text in both bars: no zoom-in"
        );
        // A subtitle alone is still cropped: one bar only.
        let mut u = Run::new();
        u.learn(SCOPE);
        let mut sub = letterbox(SCOPE);
        sub.bottom.islands = 1;
        sub.bottom.island_start = 150;
        assert!(
            u.feed(&sub, 48) == 0 && u.p.crop() == SCOPE,
            "a subtitle in one bar: still cropped"
        );
    }

    #[test]
    fn a_transport_bar_over_scope() {
        let mut r = Run::new();
        r.learn(SCOPE);
        let mut o = letterbox(SCOPE);
        o.bottom.ui = true;
        o.bottom.ui_start = 120;
        r.feed(&o, 24 * 5);
        assert!(
            r.changes == 1 && r.p.crop() == FULL,
            "transport bar over scope: one zoom out, no flapping"
        );
        // Its density flickering under the UI threshold must not let it snap
        // back.
        let mut f = Run::new();
        f.learn(SCOPE);
        for i in 0..24 * 5 {
            f.push(&if i % 3 != 0 { o } else { letterbox(SCOPE) });
        }
        assert!(
            f.changes == 1 && f.p.crop() == FULL,
            "nor when the UI is seen only every third frame"
        );
        // Gone: back in once the hold has passed, as a recent height.
        let at = r.feed(&letterbox(SCOPE), 48);
        assert!(
            at > 20 && at <= 25 && r.p.crop() == SCOPE,
            "bar gone: zoom in after the 1 s hold ({at})"
        );
    }

    #[test]
    fn an_unconfirmed_zoom_in_is_penalised() {
        let mut r = Run::new();
        r.learn(SCOPE);
        r.feed(&full(), 48);
        assert_eq!(
            r.feed(&letterbox(SCOPE), 1),
            1,
            "false scope frame while recent: zooms in"
        );
        assert_eq!(r.feed(&full(), 1), 1, "next frame: zoom out");
        assert_eq!(
            r.feed(&letterbox(SCOPE), 1),
            0,
            "second false frame: penalised, no zoom-in"
        );
        r.feed(&full(), 1);
        r.t += 61.0;
        assert_eq!(
            r.feed(&letterbox(SCOPE), 1),
            1,
            "after the penalty: recent again"
        );
    }

    #[test]
    fn a_confirmed_zoom_in_undone_costs_nothing() {
        let mut r = Run::new();
        r.learn(SCOPE);
        r.feed(&full(), 48);
        r.feed(&letterbox(SCOPE), 2);
        r.feed(&full(), 1);
        assert_eq!(r.feed(&letterbox(SCOPE), 1), 1);
    }

    #[test]
    fn known_but_not_recent_takes_a_few_frames() {
        let mut r = Run::new();
        r.learn(SCOPE);
        r.feed(&full(), 1);
        r.t += 16.0 * 60.0;
        r.push(&letterbox(SCOPE));
        assert_eq!(r.p.crop(), FULL, "known, not recent: not on one frame");
        r.push(&letterbox(SCOPE));
        assert!(
            r.p.crop() == SCOPE && r.last.diag.reason == "zoom in (known)",
            "but on the second bright one"
        );
    }

    #[test]
    fn a_cut_clears_the_candidate() {
        let (mut a, mut b) = (Run::new(), Run::new());
        let straight = a.feed(&letterbox(SCOPE), 48);
        b.feed(&letterbox(SCOPE), straight - 1);
        b.push_cut(&letterbox(SCOPE), true);
        assert_eq!(
            b.p.crop(),
            FULL,
            "a cut on the frame that would have zoomed in: no zoom-in"
        );
        assert_eq!(
            b.feed(&letterbox(SCOPE), 48),
            straight - 1,
            "evidence restarts from that frame"
        );
    }

    #[test]
    fn a_known_heights_rows_are_reused_or_widened() {
        let mut r = Run::new();
        r.learn(SCOPE);
        r.feed(&full(), 1);
        r.feed(&letterbox(crop(277, 278)), 2); // the second frame confirms it
        assert_eq!(
            r.p.crop(),
            SCOPE,
            "return lands on the known rows when the frame's are tighter"
        );
        r.feed(&full(), 1);
        r.feed(&letterbox(crop(275, 277)), 1);
        assert_eq!(
            r.p.crop(),
            crop(275, 277),
            "and widens when the frame shows more picture"
        );
    }

    #[test]
    fn a_picture_still_opening_up_settles_without_flapping() {
        let mut r = Run::new();
        r.feed(&letterbox(crop(329, 329)), 24);
        r.feed(&letterbox(crop(328, 328)), 12);
        r.feed(&letterbox(crop(327, 327)), 12);
        r.changes = 0;
        for i in 0..48 {
            r.push(&letterbox(if i % 2 == 1 {
                crop(326, 327)
            } else {
                crop(327, 327)
            }));
        }
        r.feed(&letterbox(crop(326, 327)), 48);
        assert!(
            r.changes <= 2 && r.p.crop().top <= 327 && r.p.crop().bottom <= 327,
            "settles on its widest rows, no flapping ({} changes, {:?})",
            r.changes,
            r.p.crop()
        );
        // Adopted a few rows tight, then a frame 3 rows wider: widen in place,
        // never a trip to full frame and back.
        let mut w = Run::new();
        w.feed(&letterbox(crop(329, 329)), 24);
        let at = w.feed(&letterbox(crop(326, 327)), 24);
        assert!(
            at == 1
                && w.changes == 2
                && w.p.crop() == crop(326, 327)
                && w.last.diag.reason.is_empty(),
            "a crop a few rows tight widens in place, in one step"
        );
    }

    #[test]
    fn a_taller_picture_is_still_a_zoom_out() {
        let mut r = Run::new();
        r.learn(SCOPE);
        r.learn(K190);
        r.feed(&letterbox(SCOPE), 2);
        r.feed(&letterbox(K190), 1);
        assert_eq!(r.p.crop(), K190);
    }

    #[test]
    fn an_epoch_change_resets_everything() {
        let mut r = Run::new();
        r.learn(SCOPE);
        r.epoch += 1;
        assert!(
            r.feed(&letterbox(SCOPE), 1) == 1 && r.p.crop() == FULL,
            "epoch change: reset to full frame"
        );
        assert_eq!(r.last.diag.reason, "reset");
        assert_eq!(r.feed(&letterbox(SCOPE), 1), 0, "scope is never-seen again");
    }

    #[test]
    fn thirty_minutes_of_nothing_resets() {
        let mut r = Run::new();
        r.learn(SCOPE);
        let at = r.feed(&empty(), 24 * 60 * 31);
        assert!(at > 24 * 60 * 29 && r.p.crop() == FULL);
    }

    #[test]
    fn tolerances_scale_to_1080p() {
        let mut r = Run::new();
        r.feed(&letterbox_at(crop(138, 139), 1.0, 1920, 1080), 24);
        assert_eq!(r.p.crop(), crop(138, 139));
    }

    #[test]
    fn time_share_per_adopted_state() {
        let mut r = Run::new();
        r.feed(&full(), 240);
        r.feed(&letterbox(SCOPE), 480);
        let (mut full_s, mut scope_s) = (0.0, 0.0);
        for s in r.p.shares() {
            if s.rows == H {
                full_s = s.seconds;
            }
            if s.rows == H - SCOPE.top - SCOPE.bottom {
                scope_s = s.seconds;
            }
        }
        assert!(
            full_s > 10.0 && full_s < 10.5 && scope_s > 19.5 && scope_s < 20.0,
            "{full_s} {scope_s}"
        );
    }

    #[test]
    fn diagnostics_show_a_candidates_progress() {
        let mut r = Run::new();
        r.feed(&letterbox(SCOPE), 2);
        let g = r.last.diag;
        let c = g.candidate.expect("a candidate");
        assert!(
            c.crop == SCOPE
                && c.evidence > 0.0
                && c.evidence < c.threshold
                && g.frame_class == "LETTERBOX"
                && g.edge_top == Some(276)
                && g.edge_bottom == Some(277)
        );
    }

    #[test]
    fn small_intrusions_widen_rather_than_zoom_out() {
        // A 2.00 show in a dark scene: bars 117-120 rows, and frame by frame
        // one side reading 3-6 rows into its bar. It zoomed out and back
        // every few seconds.
        let k200 = crop(119, 120);
        let mut r = Run::new();
        r.learn(k200);
        let mut ever_full = false;
        for i in 0..24 * 60 {
            let o = match i % 23 {
                5 => weak(None, Some(114), 300),
                11 => weak(Some(116), None, 300),
                17 => weak(None, Some(115), 300),
                _ => letterbox(crop(117 + i % 3, 118 + i % 3)),
            };
            r.push(&o);
            ever_full |= r.p.crop() == FULL;
        }
        assert!(!ever_full, "a 2.00 show's noisy edges never zoom it out");
        assert!(
            r.changes <= 4,
            "and widen it a few rows a handful of times, not every few seconds ({})",
            r.changes
        );
        assert!(
            r.p.crop().top >= 107 && r.p.crop().bottom >= 108,
            "by no more than 12 rows past the height adopted"
        );
    }

    #[test]
    fn widening_cannot_creep() {
        let mut r = Run::new();
        r.learn(SCOPE);
        r.feed(&weak(None, Some(274), 300), 1);
        assert_eq!(
            r.p.crop(),
            crop(276, 274),
            "a 3-row intrusion widens that side to it"
        );
        r.feed(&letterbox(SCOPE), 24);
        assert_eq!(
            r.p.crop(),
            crop(276, 274),
            "and the original height does not narrow it back"
        );
        r.feed(&weak(None, Some(268), 300), 1);
        assert_eq!(
            r.p.crop(),
            crop(276, 268),
            "9 rows past the adopted height: still widens"
        );
        r.feed(&weak(None, Some(265), 300), 1);
        assert_eq!(r.p.crop(), crop(276, 265), "12 rows past it: still widens");
        r.feed(&weak(None, Some(261), 300), 1);
        assert_eq!(r.p.crop(), FULL, "16 rows past it: real picture, zooms out");
    }

    #[test]
    fn a_real_taller_picture_zooms_out_at_once() {
        let mut r = Run::new();
        r.learn(SCOPE);
        assert!(r.feed(&weak(None, Some(150), 300), 1) == 1 && r.p.crop() == FULL);
    }

    #[test]
    fn a_non_standard_ratio_holding_across_a_cut_is_adopted() {
        // An Apple TV trailer at 1.86: bars 48/48, a cut every second.
        let k186 = crop(48, 48);
        let mut r = Run::new();
        let mut at = 0;
        for i in 1..=24 * 4 {
            if at != 0 {
                break;
            }
            if r.push_cut(&letterbox(k186), i % 24 == 0).changed {
                at = i;
            }
        }
        assert!(
            (48..=50).contains(&at) && r.p.crop() == k186,
            "adopted after 2 s ({at})"
        );
        r.feed(&full(), 24);
        assert!(
            r.feed(&letterbox(k186), 24) > 0
                && r.p.crop() == k186
                && !r.last.diag.reason.contains("non-standard"),
            "and once known it returns as fast as a standard height"
        );
    }

    #[test]
    fn a_static_menu_at_an_arbitrary_height_is_never_adopted() {
        let mut r = Run::new();
        assert!(r.feed(&letterbox(crop(48, 48)), 24 * 20) == 0 && r.p.crop() == FULL);
    }

    #[test]
    fn edges_that_move_with_each_cut_are_never_adopted() {
        let mut r = Run::new();
        let mut adopted = false;
        for i in 0..24 * 20 {
            let e = 48 + (i / 24 % 4) * 9; // a new height each second
            adopted |= r.push_cut(&letterbox(crop(e, e)), i % 24 == 0).changed;
        }
        assert!(!adopted);
    }

    #[test]
    fn ui_in_a_bar_stops_a_non_standard_adoption() {
        let mut r = Run::new();
        let mut o = letterbox(crop(48, 48));
        o.bottom.ui = true;
        let mut adopted = false;
        for i in 1..=24 * 6 {
            adopted |= r.push_cut(&o, i % 24 == 0).changed;
        }
        assert!(!adopted);
    }
}
