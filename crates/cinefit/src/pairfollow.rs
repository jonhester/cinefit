//! `detect.pair_follow`: a low-latency pass keeps the frames the tunnel pass
//! of the same chapter kept, found by its pictures rather than by its own
//! cuts.
//!
//! With `detect.pair_every`, each pass keeps every Nth frame after a cut it
//! sees itself, and the tunnel's cut detector and the YCbCr one disagree on
//! cuts between dark shots: on one disc, 232 of 530 tunnel frames had no
//! low-latency partner on disk. Here the pass follows the tunnel instead:
//!
//! - **The lock:** each incoming thumbnail, normalised (the two renditions
//!   differ in level, not in what is where), against every thumbnail of the
//!   tunnel pass; the offset whose diagonal agrees best over the last
//!   [`LOCK_WINDOW`] frames, every one of them inside the tunnel pass, at
//!   least [`LOCK_MIN`] and better by [`UNIQUE`] than any offset [`FAR`] or
//!   more frames away. A still shot agrees with every offset inside it, so it
//!   does not stand out and does not lock; nor does a window half outside the
//!   tunnel pass, where a still could match an edge perfectly; nor an offset
//!   within [`FAR`] of the end of those the window can be scored at yet, where
//!   the right one may be just past it.
//! - **Following:** the offset may move by [`DRIFT`] (the capture dropping
//!   or repeating a frame), judged over the last [`TRACK_WINDOW`] frames; a
//!   move must beat staying by [`TIE`], so slow or dark pictures, where the
//!   neighbouring offsets score within noise of each other, keep theirs.
//!   Below [`LOST`] the lock is let go, the window emptied, and found again.
//! - **Repeats count for nothing:** a frame the same as the one before it, on
//!   either side, is left out of every score. A clip ends with the player
//!   paused, one frame sent over and over, and the two passes pause at
//!   different film frames; their repeats matched each other at the wrong
//!   offsets and pulled the lock 13 frames off on the hardware. Nor is a
//!   repeat ever kept: once this pass's player stops, the frame it repeats is
//!   not the one the offset predicts.
//! - **What is kept:** the frame whose partner the tunnel pass kept. Nothing
//!   before the lock, and the lock needs a window of the tunnel's own frames
//!   and [`FAR`] more, so a frame the tunnel kept in its first three seconds
//!   cannot be followed however early this pass starts.
//!
//! The windows and margins were chosen on the 26 recorded passes of one
//! title's 13 chapters (2026-10-19): every pass locked, 2.2 s in (median; 7.3 s
//! at most), and 432 of the tunnel's 530 kept frames would have had their
//! partner kept; 4 were off their chapter's median offset, all in the chapter
//! whose passes were recorded twice, where they correlate as well as the
//! median would. A first version judged the lock on frames that differ from
//! the one before, and real film at 32x18 hardly does: it locked on 19 of 26.
//! On the hardware, a lock at the first window wholly inside the tunnel pass
//! went 42 frames off, hence the margin from the ends.
//!
//! Pure: tested without a card, a GPU or a disk.

use std::collections::{BTreeSet, VecDeque};

use crate::dataset::THUMB;

/// Frames the lock is judged over: two seconds at 24p.
pub const LOCK_WINDOW: usize = 48;
/// The least mean correlation to lock on.
pub const LOCK_MIN: f32 = 0.8;
/// How much better than any offset [`FAR`] or more frames away the lock must
/// be.
pub const UNIQUE: f32 = 0.05;
pub const FAR: i64 = 24;
/// How far the offset may move from one frame to the next.
pub const DRIFT: i64 = 3;
/// Frames a move of the offset is judged over.
pub const TRACK_WINDOW: usize = 48;
/// How much better a moved offset must be than the one held: with 0.002 the
/// recorded passes moved back and forth by three frames in dark stretches.
pub const TIE: f32 = 0.01;
/// Below this, over [`TRACK_WINDOW`], the pictures no longer match (the
/// player went somewhere else): the lock is let go.
pub const LOST: f32 = 0.5;
/// The most tunnel frames one follower takes: every frame of this pass is
/// compared with every one of them on the frame loop. Four minutes at 24p,
/// for passes of a chapter's half minute.
pub const MAX_PASS: usize = 24 * 60 * 4;

/// What one frame did to the lock, for the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// Locked on: this pass's frame minus the tunnel's, for the same picture.
    Locked(i64),
    /// The offset moved by a frame or a few.
    Moved(i64),
    Lost,
}

/// One frame's result: keep it in full, and what happened to the lock.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    pub keep: bool,
    pub event: Option<Event>,
}

pub struct Follower {
    /// The tunnel pass's first frame, and its thumbnails from there,
    /// normalised, one row of [`THUMB`] each; flat or missing ones zero.
    lo: u64,
    refs: Vec<f32>,
    valid: Vec<bool>,
    kept: BTreeSet<u64>,
    /// Each recent frame of this pass with its correlation against every
    /// reference (`None` for a flat or missing thumbnail).
    rows: VecDeque<(u64, Option<Vec<f32>>)>,
    /// The last thumbnail seen, as sent, to tell a repeat.
    last: Option<[u8; THUMB]>,
    offset: Option<i64>,
}

/// A thumbnail normalised to zero mean and unit length; `None` for a flat
/// one, which correlates with nothing.
fn norm(t: &[u8; THUMB]) -> Option<[f32; THUMB]> {
    let mean = t.iter().map(|&v| f32::from(v)).sum::<f32>() / THUMB as f32;
    let mut v = [0f32; THUMB];
    for (o, &x) in v.iter_mut().zip(t) {
        *o = f32::from(x) - mean;
    }
    let len = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    (len > 1e-3).then(|| v.map(|x| x / len))
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

impl Follower {
    /// From the tunnel pass: its frames from `lo` on, in order, each with its
    /// thumbnail where it has one, and the frames it kept.
    pub fn new(lo: u64, thumbs: &[Option<[u8; THUMB]>], kept: BTreeSet<u64>) -> Self {
        let mut refs = vec![0f32; thumbs.len() * THUMB];
        let mut valid = vec![false; thumbs.len()];
        for (k, t) in thumbs.iter().enumerate() {
            if k > 0 && t.is_some() && thumbs[k - 1] == *t {
                continue;
            }
            if let Some(n) = t.as_ref().and_then(norm) {
                refs[k * THUMB..(k + 1) * THUMB].copy_from_slice(&n);
                valid[k] = true;
            }
        }
        Self {
            lo,
            refs,
            valid,
            kept,
            rows: VecDeque::with_capacity(LOCK_WINDOW.max(TRACK_WINDOW) + 1),
            last: None,
            offset: None,
        }
    }

    /// How many tunnel frames it follows, and how many of them were kept.
    pub fn size(&self) -> (usize, usize) {
        (self.valid.len(), self.kept.len())
    }

    /// The mean correlation along offset `d` over the last `w` frames, when
    /// at least half of them have a reference to compare with; with `whole`,
    /// only when every one of the `w` falls inside the tunnel pass.
    fn score(&self, d: i64, w: usize, whole: bool) -> Option<f32> {
        if whole && self.rows.len() < w {
            return None;
        }
        let (mut sum, mut n) = (0f32, 0usize);
        let hi = self.lo as i64 + self.valid.len() as i64;
        for (j, corr) in self.rows.iter().rev().take(w) {
            let a = *j as i64 - d;
            if a < self.lo as i64 || a >= hi {
                if whole {
                    return None;
                }
                continue;
            }
            let k = (a - self.lo as i64) as usize;
            if let Some(c) = corr
                && self.valid[k]
            {
                sum += c[k];
                n += 1;
            }
        }
        (n * 2 >= w).then(|| sum / n as f32)
    }

    /// The lock from scratch: the best offset over [`LOCK_WINDOW`], if it is
    /// good enough and stands out, and not within [`FAR`] of either end of
    /// the offsets the window can be scored at yet. At an end the offsets just
    /// past it, the right one among them, cannot be scored, so standing out
    /// there proves nothing: a shot at the tunnel pass's start that repeats
    /// one this pass showed earlier locked 42 frames off on the hardware.
    fn search(&self) -> Option<i64> {
        let (j, n) = (self.rows.back()?.0 as i64, self.valid.len() as i64);
        let first = j - (self.lo as i64 + n - 1);
        let scores: Vec<(i64, f32)> = (first..first + n)
            .filter_map(|d| self.score(d, LOCK_WINDOW, true).map(|s| (d, s)))
            .collect();
        let (lowest, highest) = (scores.first()?.0, scores.last()?.0);
        let &(best, s) = scores.iter().max_by(|x, y| x.1.total_cmp(&y.1))?;
        let rival = scores
            .iter()
            .filter(|x| (x.0 - best).abs() >= FAR)
            .map(|x| x.1)
            .fold(-1f32, f32::max);
        let inside = best - lowest >= FAR && highest - best >= FAR;
        (s >= LOCK_MIN && s - rival >= UNIQUE && inside).then_some(best)
    }

    /// Follows the offset held by a frame or a few, or lets it go.
    fn track(&mut self, d: i64) -> Option<Event> {
        let held = self.score(d, TRACK_WINDOW, false);
        let best = (-DRIFT..=DRIFT)
            .filter_map(|e| self.score(d + e, TRACK_WINDOW, false).map(|s| (d + e, s)))
            .max_by(|x, y| x.1.total_cmp(&y.1));
        // A move first: after a dropped frame the held offset falls below
        // LOST over the window at about the moment the moved one wins.
        if let (Some(h), Some((m, s))) = (held, best)
            && m != d
            && s > h + TIE
            && s >= LOST
        {
            self.offset = Some(m);
            return Some(Event::Moved(m - d));
        }
        if held.is_some_and(|h| h < LOST) {
            // The window holds the old pictures: emptied, or the search would
            // lock straight back on to them.
            self.offset = None;
            self.rows.clear();
            return Some(Event::Lost);
        }
        None
    }

    /// This pass's frame `index` and its thumbnail: whether to keep it.
    pub fn step(&mut self, index: u64, thumb: Option<&[u8; THUMB]>) -> Step {
        let repeat = thumb.is_some() && thumb == self.last.as_ref();
        self.last = thumb.copied();
        let corr = thumb.filter(|_| !repeat).and_then(norm).map(|b| {
            self.refs
                .as_chunks::<THUMB>()
                .0
                .iter()
                .map(|r| dot(r, &b))
                .collect()
        });
        self.rows.push_back((index, corr));
        if self.rows.len() > LOCK_WINDOW.max(TRACK_WINDOW) {
            self.rows.pop_front();
        }
        let mut event = self.offset.and_then(|d| self.track(d));
        if self.offset.is_none()
            && let Some(d) = self.search()
        {
            self.offset = Some(d);
            event = Some(Event::Locked(d));
        }
        // A repeat is not the film frame the offset predicts: the player has
        // stopped and the tunnel pass may not have.
        let keep = !repeat
            && self.offset.is_some_and(|d| {
                let a = index as i64 - d;
                a >= 0 && self.kept.contains(&(a as u64))
            });
        Step { keep, event }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A film's frames as thumbnails: a fresh random picture each frame,
    /// except a still from `still.0` for `still.1` frames.
    fn film(frames: usize, seed: u64, still: (usize, usize)) -> Vec<[u8; THUMB]> {
        let mut rng = seed;
        let mut out: Vec<[u8; THUMB]> = Vec::with_capacity(frames);
        for f in 0..frames {
            if f > still.0 && f < still.0 + still.1 {
                out.push(out[f - 1]);
                continue;
            }
            out.push(std::array::from_fn(|_| {
                rng ^= rng << 13;
                rng ^= rng >> 7;
                rng ^= rng << 17;
                (rng >> 56) as u8
            }));
        }
        out
    }

    /// Another rendition: compressed and lifted, as a tone mapping does.
    fn mapped(t: &[u8; THUMB]) -> [u8; THUMB] {
        t.map(|v| (f64::from(v) * 0.6 + 20.0) as u8)
    }

    /// The tunnel pass: film frames `from..to` as session frames from `lo`,
    /// keeping every 48th from the 10th.
    fn tunnel(f: &[[u8; THUMB]], from: usize, to: usize, lo: u64) -> Follower {
        let thumbs: Vec<_> = f[from..to].iter().map(|t| Some(*t)).collect();
        let kept = (0..(to - from) as u64)
            .filter(|k| k % 48 == 10)
            .map(|k| lo + k)
            .collect();
        Follower::new(lo, &thumbs, kept)
    }

    /// Runs this pass: film frames `from..to` as session frames from `first`,
    /// skipping the film frames in `dropped`. The session frames kept, each
    /// with its film frame, and the events.
    fn follow(
        fl: &mut Follower,
        f: &[[u8; THUMB]],
        from: usize,
        to: usize,
        first: u64,
        dropped: &[usize],
    ) -> (Vec<(u64, usize)>, Vec<Event>) {
        let (mut kept, mut events, mut index) = (Vec::new(), Vec::new(), first);
        for (film_frame, t) in f.iter().enumerate().take(to).skip(from) {
            if dropped.contains(&film_frame) {
                continue;
            }
            let s = fl.step(index, Some(&mapped(t)));
            if s.keep {
                kept.push((index, film_frame));
            }
            events.extend(s.event);
            index += 1;
        }
        (kept, events)
    }

    /// The film frames the tunnel kept that can be followed: tunnel session
    /// frame `lo + k` is film frame `from + k`, and the lock needs a whole
    /// window of the tunnel's frames, and [`FAR`] more, first.
    fn followable(fl: &Follower, from: usize, lo: u64) -> Vec<usize> {
        fl.kept
            .iter()
            .map(|&a| from + (a - lo) as usize)
            .filter(|&f| f + 1 >= from + LOCK_WINDOW + FAR as usize)
            .collect()
    }

    #[test]
    fn keeps_the_partner_of_every_followable_tunnel_frame_and_nothing_else() {
        let f = film(1400, 1, (0, 0));
        // The tunnel pass: film 300..1200 as session frames from 937; this
        // one starts 240 frames (10 s) earlier in the film, at 11471.
        let mut fl = tunnel(&f, 300, 1200, 937);
        let (kept, events) = follow(&mut fl, &f, 60, 1300, 11471, &[]);
        assert_eq!(events, vec![Event::Locked(11471 + 240 - 937)]);
        let got: Vec<usize> = kept.iter().map(|&(_, film)| film).collect();
        assert_eq!(
            got,
            followable(&fl, 300, 937),
            "the same film frames, all of them"
        );
        assert_eq!(got.len(), 17, "of the tunnel's {}", fl.size().1);
    }

    #[test]
    fn nothing_is_kept_before_the_lock() {
        let f = film(1400, 2, (0, 0));
        let mut fl = tunnel(&f, 300, 1200, 0);
        // Starts on the tunnel's first frame: the first kept ones fall before
        // a lock window has filled.
        let (kept, _) = follow(&mut fl, &f, 300, 1200, 5000, &[]);
        assert!(!kept.is_empty());
        assert!(
            kept.iter()
                .all(|&(i, _)| i >= 5000 + LOCK_WINDOW as u64 - 1)
        );
        assert!(kept.iter().all(|&(_, film)| (film - 300) % 48 == 10));
    }

    #[test]
    fn a_dropped_frame_is_followed() {
        let f = film(1400, 3, (0, 0));
        let mut fl = tunnel(&f, 300, 1200, 0);
        // This pass loses film frame 700: from there on its numbering is one
        // behind, and the offset must move by one.
        let (kept, events) = follow(&mut fl, &f, 60, 1300, 0, &[700]);
        assert_eq!(events, vec![Event::Locked(240), Event::Moved(-1)]);
        // Every frame kept is a film frame the tunnel kept; the move is
        // judged over a short window, so one kept right after the drop may
        // be missed, never mispaired.
        let want = followable(&fl, 300, 0);
        let got: Vec<usize> = kept.iter().map(|&(_, film)| film).collect();
        assert!(got.iter().all(|g| want.contains(g)), "{got:?}");
        assert!(
            got.len() + 1 >= want.len(),
            "{} of {}",
            got.len(),
            want.len()
        );
    }

    #[test]
    fn a_still_shot_does_not_lock_on_the_wrong_frame() {
        // The pass opens on a still of 300 frames, and the tunnel pass starts
        // inside it: there every offset agrees, and at the tunnel's first
        // frame a window half outside it matches the still perfectly. The lock
        // must wait for the picture to move, and then be right.
        let f = film(1400, 4, (40, 300));
        let mut fl = tunnel(&f, 200, 1100, 0);
        let (kept, events) = follow(&mut fl, &f, 0, 1300, 0, &[]);
        assert_eq!(events, vec![Event::Locked(200)]);
        let want = followable(&fl, 200, 0);
        let got: Vec<usize> = kept.iter().map(|&(_, film)| film).collect();
        assert!(!got.is_empty());
        assert!(got.iter().all(|g| want.contains(g)), "{got:?}");
    }

    /// Recorded passes: `CINEFIT_PAIRFOLLOW=SESSION:A_FIRST-A_LAST:B_FIRST-B_LAST`,
    /// a tunnel pass and the low-latency pass of the same chapter in one
    /// dataset session. Prints the lock and the frames this pass would keep;
    /// skipped when unset.
    #[test]
    fn recorded_passes() {
        let Ok(spec) = std::env::var("CINEFIT_PAIRFOLLOW") else {
            return;
        };
        let range = |r: &str| -> (u64, u64) {
            let (lo, hi) = r.split_once('-').expect("FIRST-LAST");
            (lo.parse().expect("FIRST"), hi.parse().expect("LAST"))
        };
        let mut parts = spec.rsplitn(3, ':');
        let (b, a, dir) = (
            range(parts.next().expect("B")),
            range(parts.next().expect("A")),
            parts.next().expect("SESSION"),
        );
        let pass = crate::pairs::load(std::path::Path::new(dir), None).expect("the session");
        let thumbs: Vec<_> = (a.0..=a.1).map(|i| pass.thumbs.get(&i).copied()).collect();
        let kept = pass.kept.range(a.0..=a.1).copied().collect();
        let mut fl = Follower::new(a.0, &thumbs, kept);
        let (mut keep, mut events, mut d) = (Vec::new(), Vec::new(), 0i64);
        let started = std::time::Instant::now();
        for j in b.0..=b.1 {
            let s = fl.step(j, pass.thumbs.get(&j));
            match s.event {
                Some(Event::Locked(x)) => d = x,
                Some(Event::Moved(e)) => d += e,
                _ => {}
            }
            if s.keep {
                // This pass's frame and the tunnel frame it partners.
                keep.push((j, j as i64 - d));
            }
            events.extend(s.event.map(|e| (j, e)));
        }
        println!(
            "tunnel {}-{}: {} kept; events {events:?}; would keep {} ({} kept by this pass already)",
            a.0,
            a.1,
            fl.size().1,
            keep.len(),
            keep.iter().filter(|k| pass.kept.contains(&k.0)).count()
        );
        println!(
            "step: {:.0} us a frame against {} tunnel frames",
            started.elapsed().as_secs_f64() * 1e6 / (b.1 - b.0 + 1) as f64,
            fl.size().0
        );
        println!("would keep: {keep:?}");
        assert!(
            events.iter().any(|e| matches!(e.1, Event::Locked(_))),
            "never locked"
        );
    }

    #[test]
    fn a_paused_end_does_not_pull_the_offset() {
        // Each clip ends with the player paused, one frame repeated, and the
        // two passes pause at different film frames: here the tunnel plays on
        // to film 1079 and this pass stops at 1064. Its repeats match the
        // tunnel's one 1064 at ever larger offsets (seen on the hardware,
        // 2026-10-19: walked 13 frames and kept the wrong partner).
        let f = film(1400, 10, (0, 0));
        let tunnel_films: Vec<usize> = (300..1080).chain([1079; 40]).collect();
        let thumbs: Vec<_> = tunnel_films.iter().map(|&x| Some(f[x])).collect();
        let kept: BTreeSet<u64> = (0..tunnel_films.len() as u64)
            .filter(|k| k % 48 == 10)
            .collect();
        let kept_films: Vec<usize> = kept.iter().map(|&k| tunnel_films[k as usize]).collect();
        let mut fl = Follower::new(0, &thumbs, kept);
        let pass_films: Vec<usize> = (60..1065).chain([1064; 60]).collect();
        let (mut events, mut got) = (Vec::new(), Vec::new());
        for (j, &x) in pass_films.iter().enumerate() {
            let s = fl.step(j as u64, Some(&mapped(&f[x])));
            events.extend(s.event);
            if s.keep {
                got.push(x);
            }
        }
        assert_eq!(events, vec![Event::Locked(240)]);
        assert!(got.iter().all(|g| kept_films.contains(g)), "{got:?}");
    }

    #[test]
    fn a_repeated_shot_at_the_tunnel_start_does_not_lock_early() {
        // The tunnel pass opens on a shot that repeats one this pass, started
        // earlier, has just shown: film 300-359 is 240-299 again. The first
        // window wholly inside the tunnel pass can only be scored at the edge
        // offset, and there the repeat matches perfectly and stands out (seen
        // on the hardware, 2026-10-19: locked 42 frames off, then walked).
        let mut f = film(1400, 9, (0, 0));
        for k in 0..60 {
            f[300 + k] = f[240 + k];
        }
        let mut fl = tunnel(&f, 300, 1200, 0);
        let (kept, events) = follow(&mut fl, &f, 60, 1300, 0, &[]);
        assert_eq!(events, vec![Event::Locked(240)]);
        let want = followable(&fl, 300, 0);
        let got: Vec<usize> = kept.iter().map(|&(_, film)| film).collect();
        assert!(got.iter().all(|g| want.contains(g)), "{got:?}");
    }

    #[test]
    fn another_film_locks_on_nothing_and_keeps_nothing() {
        let a = film(1400, 5, (0, 0));
        let b = film(1400, 6, (0, 0));
        let mut fl = tunnel(&a, 300, 1200, 0);
        let (kept, events) = follow(&mut fl, &b, 0, 1400, 0, &[]);
        assert!(kept.is_empty());
        assert!(events.is_empty(), "{events:?}");
    }

    #[test]
    fn the_player_going_elsewhere_lets_the_lock_go() {
        // Locked on the right chapter, then the picture becomes another film.
        let a = film(1400, 7, (0, 0));
        let b = film(1400, 8, (0, 0));
        let mut fl = tunnel(&a, 300, 1200, 0);
        let (_, first) = follow(&mut fl, &a, 60, 600, 0, &[]);
        assert_eq!(first, vec![Event::Locked(240)]);
        let (kept, then) = follow(&mut fl, &b, 0, 400, 540, &[]);
        assert_eq!(then, vec![Event::Lost]);
        // Nothing of the other film kept once lost (a frame or two may go
        // before the short window notices).
        assert!(kept.len() <= 1, "{kept:?}");
    }
}
