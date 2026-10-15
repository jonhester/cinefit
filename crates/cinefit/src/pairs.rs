//! `cinefit pairs A B`: the frames two passes of a title both kept, paired
//! frame for frame (#109). Typically A is a Dolby Vision tunnel pass and B
//! a low-latency one of the same chapter, both recorded with
//! `detect.pair_every`, so both kept each shot's first frame and every Nth
//! after it.
//!
//! - **The offset between the passes,** from their cuts: each pass's shots
//!   as a run of lengths, and the offset most pairs of consecutive shots
//!   agree on. Then cut by cut, following the offset where it moves (a frame
//!   the capture dropped shifts everything after it), within
//!   [`DRIFT`] frames.
//! - **The pairs:** a kept frame of A, `k` frames into a shot whose cut has
//!   a partner in B, pairs with B's frame `k` frames into that shot, when B
//!   kept it. A cut one pass saw and the other did not leaves its shot out.
//! - **A check on each:** the passes' thumbnails, normalised (the tunnel and
//!   the low-latency picture differ in level, not in what is where), must
//!   correlate at least [`MIN_CORR`] for the pair, and no less (by more than
//!   [`NEIGHBOUR_MARGIN`]) than for either neighbour of B's frame, or the
//!   pair is dropped as possibly a frame off. A still shot, where neighbours
//!   are alike, passes: its frames are the same picture anyway.

use std::collections::{BTreeSet, HashMap};
use std::error::Error;
use std::fmt::Write as _;
use std::path::Path;

use crate::dataset::THUMB;

/// How far the offset may move between consecutive matched cuts: the
/// capture dropping a frame or two.
pub const DRIFT: i64 = 3;
/// The least correlation of a pair's thumbnails.
pub const MIN_CORR: f64 = 0.9;
/// How much better a neighbour of B's frame must correlate to make the pair
/// doubtful. In a still or slow shot the neighbours are as alike as the
/// frame itself, and a strict "no neighbour better" dropped about half of
/// them on the first real passes (2026-10-15); a frame truly one off, in a
/// shot that moves, loses by far more than this.
pub const NEIGHBOUR_MARGIN: f64 = 0.01;

/// One pass, as its session recorded it.
#[derive(Debug, Default, Clone)]
pub struct Pass {
    /// Frames a cut started a shot on, in order.
    pub cuts: Vec<u64>,
    /// Frames kept in full.
    pub kept: BTreeSet<u64>,
    /// Each frame's thumbnail, where it has one.
    pub thumbs: HashMap<u64, [u8; THUMB]>,
    /// Each frame's content tag (`tunnel`, `lldv`, ...).
    pub content: HashMap<u64, String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pair {
    pub a: u64,
    pub b: u64,
    /// The cut in A whose shot it is in, and how far into the shot.
    pub shot: u64,
    pub k: u64,
    /// The thumbnails' correlation.
    pub corr: f64,
}

/// What pairing found, for the summary.
#[derive(Debug, Default, PartialEq)]
pub struct Found {
    pub pairs: Vec<Pair>,
    /// Cuts in A, and how many had a partner in B.
    pub cuts: usize,
    pub matched: usize,
    /// Pairs dropped by the thumbnail check.
    pub doubtful: usize,
    /// The offset (B's frame minus A's) at the first matched cut.
    pub offset: Option<i64>,
}

/// The offset most pairs of consecutive shots agree on: a shot length pair
/// in A matched by the same lengths (to a frame) in B.
fn offset(a: &[u64], b: &[u64]) -> Option<i64> {
    let lens = |c: &[u64]| -> Vec<(u64, u64, u64)> {
        c.windows(3)
            .map(|w| (w[0], w[1] - w[0], w[2] - w[1]))
            .collect()
    };
    let (la, lb) = (lens(a), lens(b));
    let mut votes: HashMap<i64, usize> = HashMap::new();
    for &(ca, a1, a2) in &la {
        for &(cb, b1, b2) in &lb {
            if a1.abs_diff(b1) <= 1 && a2.abs_diff(b2) <= 1 {
                *votes.entry(cb as i64 - ca as i64).or_default() += 1;
            }
        }
    }
    votes
        .into_iter()
        .max_by_key(|&(d, n)| (n, std::cmp::Reverse(d.abs())))
        .map(|(d, _)| d)
}

/// A thumbnail normalised: zero mean, unit length; `None` for a flat one,
/// which correlates with nothing.
fn norm(t: &[u8; THUMB]) -> Option<Vec<f64>> {
    let mean = t.iter().map(|&v| f64::from(v)).sum::<f64>() / THUMB as f64;
    let v: Vec<f64> = t.iter().map(|&x| f64::from(x) - mean).collect();
    let len = v.iter().map(|x| x * x).sum::<f64>().sqrt();
    (len > 1e-9).then(|| v.into_iter().map(|x| x / len).collect())
}

fn thumb(p: &Pass, f: u64) -> Option<&[u8; THUMB]> {
    p.thumbs.get(&f)
}

fn corr(a: &[u8; THUMB], b: &[u8; THUMB]) -> Option<f64> {
    let (a, b) = (norm(a)?, norm(b)?);
    Some(a.iter().zip(&b).map(|(x, y)| x * y).sum())
}

/// Pairs the frames A and B both kept.
pub fn align(a: &Pass, b: &Pass) -> Found {
    let mut found = Found {
        cuts: a.cuts.len(),
        ..Found::default()
    };
    let Some(mut d) = offset(&a.cuts, &b.cuts) else {
        return found;
    };
    let b_cuts: BTreeSet<u64> = b.cuts.iter().copied().collect();
    // A's shots: each cut and where the next begins.
    let ends: Vec<u64> = a.cuts.iter().skip(1).copied().chain([u64::MAX]).collect();
    for (&ca, &end) in a.cuts.iter().zip(&ends) {
        // B's cut nearest where the offset puts it, within the drift.
        let want = ca as i64 + d;
        let Some(cb) = (-DRIFT..=DRIFT)
            .map(|e| want + e)
            .filter(|&c| c >= 0 && b_cuts.contains(&(c as u64)))
            .min_by_key(|c| c.abs_diff(want))
        else {
            continue;
        };
        d = cb - ca as i64;
        found.matched += 1;
        found.offset.get_or_insert(d);
        let next_b = b_cuts
            .range(cb as u64 + 1..)
            .next()
            .copied()
            .unwrap_or(u64::MAX);
        for &fa in a.kept.range(ca..end) {
            let k = fa - ca;
            let fb = cb as u64 + k;
            // B's shot must be as long: a cut B saw and A did not ends it.
            if fb >= next_b || !b.kept.contains(&fb) {
                continue;
            }
            let (Some(ta), Some(tb)) = (thumb(a, fa), thumb(b, fb)) else {
                continue;
            };
            let Some(c) = corr(ta, tb) else {
                continue;
            };
            let neighbours = [fb.wrapping_sub(1), fb + 1]
                .iter()
                .filter_map(|&n| thumb(b, n).and_then(|tn| corr(ta, tn)))
                .fold(f64::NEG_INFINITY, f64::max);
            if c < MIN_CORR || c + NEIGHBOUR_MARGIN < neighbours {
                found.doubtful += 1;
                continue;
            }
            found.pairs.push(Pair {
                a: fa,
                b: fb,
                shot: ca,
                k,
                corr: c,
            });
        }
    }
    found
}

/// Reads a session's pass: `frames.csv` (cuts, content, which frames have a
/// thumbnail), `thumbs.bin`, and `frames/kept.csv`. With `content`, only the
/// frames of that content (`tunnel`, `lldv`, `sdr`, ...): one session can
/// hold several passes of different kinds, played back to back.
pub fn load(session: &Path, content: Option<&str>) -> Result<Pass, Box<dyn Error>> {
    let frames = std::fs::read_to_string(session.join("frames.csv"))?;
    let thumbs = std::fs::read(session.join("thumbs.bin"))?;
    let mut pass = Pass::default();
    let mut next_thumb = 0;
    for line in frames.lines().skip(1) {
        let f: Vec<&str> = line.split(',').collect();
        // A session still being written ends mid-line; one from before the
        // cut column has no cuts to pair by.
        if f.len() < 9 {
            if f.len() < 8 {
                break;
            }
            return Err(format!(
                "{}: no cut column in frames.csv (recorded before #109)",
                session.display()
            )
            .into());
        }
        let index: u64 = f[0].parse()?;
        // Read in order whatever the content, to stay in step with
        // thumbs.bin.
        let thumb = if f[7] == "1" {
            let Some(t) = thumbs.get(next_thumb * THUMB..(next_thumb + 1) * THUMB) else {
                break;
            };
            next_thumb += 1;
            Some(<[u8; THUMB]>::try_from(t).expect("THUMB bytes"))
        } else {
            None
        };
        if content.is_some_and(|c| c != f[3]) {
            continue;
        }
        pass.content.insert(index, f[3].to_string());
        if let Some(t) = thumb {
            pass.thumbs.insert(index, t);
        }
        if f[8] == "1" {
            pass.cuts.push(index);
        }
    }
    let kept = std::fs::read_to_string(session.join("frames").join("kept.csv")).unwrap_or_default();
    for line in kept.lines().skip(1) {
        if let Some(i) = line.split(',').next().and_then(|i| i.parse().ok())
            && pass.content.contains_key(&i)
        {
            pass.kept.insert(i);
        }
    }
    Ok(pass)
}

/// `cinefit pairs A B`: pairs the two sessions' kept frames, each limited to
/// a content if given, and writes `pairs[-<a>-<b>].csv` into A.
pub fn run(
    (a_dir, a_content): (&Path, Option<&str>),
    (b_dir, b_content): (&Path, Option<&str>),
) -> Result<(), Box<dyn Error>> {
    let (a, b) = (load(a_dir, a_content)?, load(b_dir, b_content)?);
    let found = align(&a, &b);
    let content = |p: &Pass, f: u64| p.content.get(&f).cloned().unwrap_or_default();
    let mut csv = format!(
        "# A {} {}\n# B {} {}\n",
        a_dir.display(),
        a_content.unwrap_or("(all)"),
        b_dir.display(),
        b_content.unwrap_or("(all)")
    );
    csv.push_str("a,b,shot,k,corr,a_content,b_content\n");
    for p in &found.pairs {
        let _ = writeln!(
            csv,
            "{},{},{},{},{:.4},{},{}",
            p.a,
            p.b,
            p.shot,
            p.k,
            p.corr,
            content(&a, p.a),
            content(&b, p.b)
        );
    }
    // Named for both sides, B's session included: two renditions in other
    // sessions with one content tag (SDR BT.709, SDR BT.2020) must not
    // overwrite each other.
    let tag = |c: Option<&str>| c.map_or_else(String::new, |c| format!("-{c}"));
    let b_name = b_dir
        .file_name()
        .map_or_else(String::new, |n| format!("-{}", n.to_string_lossy()));
    let out = a_dir.join(format!(
        "pairs{}{b_name}{}.csv",
        tag(a_content),
        tag(b_content)
    ));
    std::fs::write(&out, csv)?;
    println!(
        "A: {} cuts, {} frames kept; B: {} cuts, {} frames kept",
        a.cuts.len(),
        a.kept.len(),
        b.cuts.len(),
        b.kept.len()
    );
    match found.offset {
        None => println!("no offset found: the passes share no run of shot lengths"),
        Some(d) => println!(
            "offset {d} frames; {} of {} cuts matched; {} pairs, {} dropped as doubtful",
            found.matched,
            found.cuts,
            found.pairs.len(),
            found.doubtful
        ),
    }
    println!("written: {}", out.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A film's frames, each a random picture; a cut every so often.
    struct Film {
        thumbs: Vec<[u8; THUMB]>,
        cuts: Vec<u64>,
    }

    fn film(frames: usize, cuts: &[u64]) -> Film {
        film_seeded(frames, cuts, 0x2545_f491_4f6c_dd1d)
    }

    fn film_seeded(frames: usize, cuts: &[u64], seed: u64) -> Film {
        let mut rng = seed;
        let thumbs = (0..frames)
            .map(|_| {
                std::array::from_fn(|_| {
                    rng ^= rng << 13;
                    rng ^= rng >> 7;
                    rng ^= rng << 17;
                    (rng >> 56) as u8
                })
            })
            .collect();
        Film {
            thumbs,
            cuts: cuts.to_vec(),
        }
    }

    /// A pass of `film` from frame `start`, recorded with `pair_every`, its
    /// pictures through `tone` (another rendition), its frames numbered
    /// from 0, skipping `dropped` frames of the film and with `missed` cuts
    /// unseen and `extra` ones seen.
    fn pass(
        f: &Film,
        start: u64,
        tone: fn(u8) -> u8,
        dropped: &[u64],
        missed: &[u64],
        extra: &[u64],
    ) -> Pass {
        let mut p = Pass::default();
        let mut picker = crate::dataset::Picker::new(1, 0.0);
        let mut index = 0u64;
        for film_frame in start..f.thumbs.len() as u64 {
            if dropped.contains(&film_frame) {
                continue;
            }
            let cut = (f.cuts.contains(&film_frame) && !missed.contains(&film_frame))
                || extra.contains(&film_frame);
            if cut {
                p.cuts.push(index);
            }
            p.thumbs
                .insert(index, f.thumbs[film_frame as usize].map(tone));
            if picker.pick_pairs(cut, 24).is_some() {
                p.kept.insert(index);
            }
            index += 1;
        }
        p
    }

    fn same(v: u8) -> u8 {
        v
    }

    /// Another rendition: compressed and lifted, as a tone mapping does.
    fn mapped(v: u8) -> u8 {
        (f64::from(v) * 0.6 + 20.0) as u8
    }

    const CUTS: [u64; 8] = [50, 130, 170, 260, 300, 410, 470, 560];

    #[test]
    fn two_passes_pair_frame_for_frame_through_another_rendition() {
        let f = film(640, &CUTS);
        let (a, b) = (
            pass(&f, 10, same, &[], &[], &[]),
            pass(&f, 33, mapped, &[], &[], &[]),
        );
        let found = align(&a, &b);
        assert_eq!(found.offset, Some(-23));
        assert_eq!(found.matched, CUTS.len());
        assert_eq!(found.doubtful, 0);
        // Every kept frame of A paired, each with the same frame of the film.
        assert_eq!(found.pairs.len(), a.kept.len());
        assert!(
            found
                .pairs
                .iter()
                .all(|p| p.b as i64 == p.a as i64 - 23 && p.corr > 0.95)
        );
    }

    #[test]
    fn a_dropped_frame_moves_the_offset_and_it_is_followed() {
        let f = film(640, &CUTS);
        let a = pass(&f, 0, same, &[], &[], &[]);
        // B loses a frame of the film at 280: from the shot at 300 on, its
        // numbering is one behind.
        let b = pass(&f, 0, mapped, &[280], &[], &[]);
        let found = align(&a, &b);
        let after: Vec<_> = found.pairs.iter().filter(|p| p.a >= 300).collect();
        assert!(!after.is_empty());
        assert!(after.iter().all(|p| p.b + 1 == p.a), "{after:?}");
        // The shot the drop is in (260-299) is shorter in B: its frames after
        // the drop are paired a frame off, and the check must drop them.
        assert!(
            found
                .pairs
                .iter()
                .all(|p| f.thumbs[p.a as usize].map(mapped) == b.thumbs[&p.b]),
            "every pair is the same frame of the film"
        );
        // ... and it did drop one: A's frame 284 against B's 284, which is the
        // film's 285.
        assert_eq!(found.doubtful, 1);
    }

    #[test]
    fn a_cut_one_pass_missed_or_invented_leaves_its_shot_out_not_mispaired() {
        let f = film(640, &CUTS);
        let a = pass(&f, 0, same, &[], &[], &[]);
        let b = pass(&f, 0, mapped, &[], &[300], &[200]);
        let found = align(&a, &b);
        assert!(found.matched >= CUTS.len() - 1);
        // Whatever paired is the same frame of the film.
        assert!(found.pairs.iter().all(|p| p.a == p.b));
        assert!(!found.pairs.is_empty());
    }

    #[test]
    fn passes_of_different_films_pair_nothing() {
        let a = pass(&film(640, &CUTS), 0, same, &[], &[], &[]);
        let other = film_seeded(640, &[40, 90, 200, 330, 380, 500], 0x9e37_79b9_7f4a_7c15);
        let b = pass(&other, 0, same, &[], &[], &[]);
        assert!(align(&a, &b).pairs.is_empty());
    }
}
