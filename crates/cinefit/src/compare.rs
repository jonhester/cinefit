//! `cinefit compare`: the same content recorded twice, as a Dolby Vision
//! tunnel and as HDR10, rendered through each path and compared (#24). Where
//! the decode is right the two agree, apart from what an enhancement layer
//! adds; a range or scale error shows as a gain or black-level offset, a chroma
//! or matrix error as a hue shift on everything coloured.
//!
//! The recordings start at slightly different moments, so they are aligned
//! first: each frame gets a small luma fingerprint, normalised so the two
//! encodings (the tunnel's ICtCp intensity, HDR10's Y') compare by shape, and
//! the frame offset that best matches the whole stretch wins.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::path::PathBuf;

use crate::record::{ClipFrame, for_each_frame, read_index};

/// Fingerprint grid: 48 x 27 cells, the frame's shape.
const GRID_W: usize = 48;
const GRID_H: usize = 27;

pub struct Options {
    pub tunnel: PathBuf,
    pub hdr10: PathBuf,
    /// Frame pairs to compare.
    pub pairs: usize,
    /// The largest start offset to try, in frames either way.
    pub max_shift: i64,
}

/// A frame's luma, sampled into the grid and normalised to zero mean and unit
/// variance; `None` for a frame with no fingerprint (unknown format, or flat,
/// which matches anything).
type Print = Option<Vec<f32>>;

/// Luma at (x, y), on the format's own scale.
fn luma(format: &str, bytes: &[u8], row_bytes: usize, x: usize, y: usize) -> Option<f32> {
    match format {
        // The tunnel: 12-bit intensity, G = Y[11:4], B's low nibble Y[3:0].
        "r210" => {
            let o = y * row_bytes + x * 4;
            let w = u32::from_be_bytes(bytes.get(o..o + 4)?.try_into().ok()?);
            let (g, b) = ((w >> 12) & 0xFF, (w >> 2) & 0xFF);
            Some((g << 4 | (b & 0xF)) as f32)
        }
        // v210: the first luma of the pixel's group of six is close enough
        // for a fingerprint.
        "v210" => {
            let o = y * row_bytes + (x / 6) * 16;
            let w = u32::from_le_bytes(bytes.get(o..o + 4)?.try_into().ok()?);
            Some(((w >> 10) & 0x3FF) as f32)
        }
        _ => None,
    }
}

fn fingerprint(f: &ClipFrame, bytes: &[u8]) -> Print {
    let (cw, ch) = (f.width / GRID_W, f.height / GRID_H);
    if cw == 0 || ch == 0 {
        return None;
    }
    let mut cells = Vec::with_capacity(GRID_W * GRID_H);
    for gy in 0..GRID_H {
        for gx in 0..GRID_W {
            let (mut sum, mut n) = (0f32, 0f32);
            for y in (gy * ch..(gy + 1) * ch).step_by(8) {
                for x in (gx * cw..(gx + 1) * cw).step_by(8) {
                    sum += luma(&f.pixel_format, bytes, f.row_bytes, x, y)?;
                    n += 1.0;
                }
            }
            cells.push(sum / n.max(1.0));
        }
    }
    normalise(cells)
}

fn normalise(mut v: Vec<f32>) -> Print {
    let n = v.len() as f32;
    let mean = v.iter().sum::<f32>() / n;
    let var = v.iter().map(|x| (x - mean) * (x - mean)).sum::<f32>() / n;
    if var < 1e-6 {
        return None;
    }
    let sd = var.sqrt();
    for x in &mut v {
        *x = (*x - mean) / sd;
    }
    Some(v)
}

/// Normalised cross-correlation of two fingerprints: 1 for the same shape.
fn ncc(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>() / a.len() as f32
}

/// The offset `d` (tunnel frame `i` against HDR10 frame `i + d`) whose
/// fingerprints agree best on average, with that average. Every `step`th
/// tunnel frame is used; an offset needs at least 20 comparable frames.
fn best_offset(t: &[Print], h: &[Print], max_shift: i64, step: usize) -> Option<(i64, f32)> {
    let mut best: Option<(i64, f32)> = None;
    for d in -max_shift..=max_shift {
        let (mut sum, mut n) = (0f32, 0usize);
        for i in (0..t.len()).step_by(step.max(1)) {
            let j = i as i64 + d;
            if j < 0 || j as usize >= h.len() {
                continue;
            }
            if let (Some(a), Some(b)) = (&t[i], &h[j as usize]) {
                sum += ncc(a, b);
                n += 1;
            }
        }
        if n >= 20 {
            let score = sum / n as f32;
            if best.is_none_or(|(_, s)| score > s) {
                best = Some((d, score));
            }
        }
    }
    best
}

/// A candidate pair: tunnel frame `t`, HDR10 frame `h`, how well they match,
/// and how much the tunnel moves around frame `t` (0 for a still).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Pair {
    t: usize,
    h: usize,
    matched: f32,
    motion: f32,
}

/// Pairs at the offset, with their match and motion.
fn pairs_at(t: &[Print], h: &[Print], d: i64) -> Vec<Pair> {
    let mut out = Vec::new();
    for i in 1..t.len().saturating_sub(1) {
        let j = i as i64 + d;
        if j < 0 || j as usize >= h.len() {
            continue;
        }
        let (Some(a), Some(b)) = (&t[i], &h[j as usize]) else {
            continue;
        };
        // Motion: how far the frame's neighbours have moved from it.
        let near = |k: usize| t[k].as_ref().map_or(0.0, |c| 1.0 - ncc(a, c));
        out.push(Pair {
            t: i,
            h: j as usize,
            matched: ncc(a, b),
            motion: near(i - 1).max(near(i + 1)),
        });
    }
    out
}

/// `n` pairs spread through the recording: in each of `n` equal stretches,
/// the stillest pair that matches at least `min_match`.
fn pick(pairs: &[Pair], n: usize, min_match: f32) -> Vec<Pair> {
    let (Some(first), Some(last)) = (pairs.first(), pairs.last()) else {
        return Vec::new();
    };
    let span = (last.t - first.t + 1).max(1);
    let mut chosen = Vec::new();
    for k in 0..n {
        let (lo, hi) = (first.t + span * k / n, first.t + span * (k + 1) / n);
        let best = pairs
            .iter()
            .filter(|p| p.t >= lo && p.t < hi && p.matched >= min_match)
            .min_by(|a, b| a.motion.total_cmp(&b.motion));
        if let Some(p) = best {
            chosen.push(*p);
        }
    }
    chosen
}

/// BT.2100 ICtCp from linear BT.2020 RGB (0-1 of 10 000 nits).
fn ictcp(rgb: [f64; 3]) -> [f64; 3] {
    use cinefit_render::color::pq_oetf;
    let [r, g, b] = rgb;
    let l = pq_oetf((1688.0 * r + 2146.0 * g + 262.0 * b) / 4096.0);
    let m = pq_oetf((683.0 * r + 2951.0 * g + 462.0 * b) / 4096.0);
    let s = pq_oetf((99.0 * r + 309.0 * g + 3688.0 * b) / 4096.0);
    [
        0.5 * l + 0.5 * m,
        (6610.0 * l - 13613.0 * m + 7003.0 * s) / 4096.0,
        (17933.0 * l - 17390.0 * m - 543.0 * s) / 4096.0,
    ]
}

/// The comparison of one rendered pair, both in BT.2020 PQ, 16 bits.
#[derive(Debug, Default)]
struct Diff {
    /// Per-channel PQ difference, in 10-bit codes: median and 99th percentile.
    pq_median: f64,
    pq_p99: f64,
    /// Median of log2(tunnel / HDR10) luminance over pixels between 1 and
    /// 1000 nits: 0 for no gain error, 0.1 for 7 % brighter.
    gain_stops: f64,
    /// Mean PQ of near-black pixels (HDR10 below 0.1 nits), tunnel minus
    /// HDR10, in 10-bit codes.
    black_codes: f64,
    /// On pixels with real colour: median hue difference in degrees, and the
    /// median ratio of colourfulness (tunnel / HDR10).
    hue_degrees: f64,
    chroma_ratio: f64,
    coloured: usize,
}

fn median(v: &mut [f64]) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

/// Compares two renders (RGBA, 16 bits, rows packed), every `step`th pixel.
fn diff(tunnel: &[u16], hdr10: &[u16], step: usize) -> Diff {
    use cinefit_render::color::pq_eotf;
    let px = |v: &[u16], i: usize| [0, 1, 2].map(|c| f64::from(v[i * 4 + c]) / 65535.0);
    let (mut pq, mut gain, mut black, mut hue, mut chroma) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for i in (0..tunnel.len().min(hdr10.len()) / 4).step_by(step.max(1)) {
        let (a, b) = (px(tunnel, i), px(hdr10, i));
        for c in 0..3 {
            pq.push((a[c] - b[c]).abs() * 1023.0);
        }
        let (la, lb) = (a.map(pq_eotf), b.map(pq_eotf));
        let y = |l: [f64; 3]| 0.2627 * l[0] + 0.6780 * l[1] + 0.0593 * l[2];
        let (ya, yb) = (y(la), y(lb));
        // 0-1 of 10 000 nits: 1 nit is 1e-4.
        if (1e-4..=0.1).contains(&yb) && ya > 0.0 {
            gain.push((ya / yb).log2());
        }
        if yb < 1e-5 {
            let m = |v: [f64; 3]| (v[0] + v[1] + v[2]) / 3.0;
            black.push((m(a) - m(b)) * 1023.0);
        }
        let (ia, ib) = (ictcp(la), ictcp(lb));
        let cb = ib[1].hypot(ib[2]);
        if cb > 0.03 && ib[0] > 0.1 {
            let ca = ia[1].hypot(ia[2]);
            let d = (ia[2].atan2(ia[1]) - ib[2].atan2(ib[1])).to_degrees();
            // Wrapped to -180..180, then its size.
            hue.push(((d + 540.0) % 360.0 - 180.0).abs());
            chroma.push(ca / cb);
        }
    }
    let p99 = |v: &mut Vec<f64>| {
        v.sort_by(f64::total_cmp);
        v.get(v.len() * 99 / 100).copied().unwrap_or(f64::NAN)
    };
    Diff {
        pq_median: median(&mut pq.clone()),
        pq_p99: p99(&mut pq),
        gain_stops: median(&mut gain),
        black_codes: if black.is_empty() {
            f64::NAN
        } else {
            black.iter().sum::<f64>() / black.len() as f64
        },
        coloured: hue.len(),
        hue_degrees: median(&mut hue),
        chroma_ratio: median(&mut chroma),
    }
}

/// Fingerprints every frame of a clip.
fn fingerprints(dir: &std::path::Path, index: &[ClipFrame]) -> Result<Vec<Print>, Box<dyn Error>> {
    let mut prints = Vec::with_capacity(index.len());
    for_each_frame::<Box<dyn Error>>(
        dir,
        index,
        |_| true,
        |f, b| {
            prints.push(fingerprint(f, b));
            Ok(())
        },
    )?;
    Ok(prints)
}

/// The bytes of the chosen frames of a clip.
fn frames(
    dir: &std::path::Path,
    index: &[ClipFrame],
    wanted: &BTreeSet<usize>,
) -> Result<BTreeMap<usize, Vec<u8>>, Box<dyn Error>> {
    let mut out = BTreeMap::new();
    let mut k = 0usize;
    for_each_frame::<Box<dyn Error>>(
        dir,
        index,
        |_| {
            let w = wanted.contains(&k);
            k += 1;
            w
        },
        |f, b| {
            let pos = index.iter().position(|g| g.index == f.index).unwrap_or(0);
            out.insert(pos, b.to_vec());
            Ok(())
        },
    )?;
    Ok(out)
}

/// Renders a frame to BT.2020 PQ RGBA16 through its own path: a tunnel with
/// its own metadata, HDR10 as YCbCr. `None` for a frame that is neither.
#[cfg(target_os = "linux")]
fn render(
    pl: &cinefit_render::Placebo,
    f: &ClipFrame,
    bytes: &[u8],
) -> Result<Option<Vec<u16>>, Box<dyn Error>> {
    use cinefit_render::{DoviColor, Matrix, Primaries, RgbColor, Transfer, YcbcrColor};
    let out = RgbColor {
        primaries: Primaries::Bt2020,
        transfer: Transfer::Pq,
    };
    let target = pl.target_rgba16(f.width, f.height)?;
    let mut planes = pl.planes(f.width, f.height)?;
    match f.pixel_format.as_str() {
        "r210" => {
            let parity = cinefit_dovi::r210_parity(bytes, f.width, f.row_bytes);
            let Ok(tunnel) = cinefit_dovi::decode(parity) else {
                return Ok(None);
            };
            let Some(c) = tunnel.metadata.and_then(|m| m.conversion()) else {
                return Ok(None);
            };
            let color = DoviColor {
                ycc_to_lms: c.ycc_to_rgb,
                offset: c.offset,
                linear: c.rgb_to_lms,
            };
            pl.unpack_tunnel(&mut planes, bytes, f.row_bytes)?;
            pl.render_dovi(&planes, &color, target.texture(), out, None)?;
        }
        "v210" => {
            pl.unpack_v210(&mut planes, bytes, f.row_bytes)?;
            let color = YcbcrColor {
                matrix: Matrix::Bt2020Nc,
                primaries: Primaries::Bt2020,
                transfer: Transfer::Pq,
            };
            pl.render_ycbcr(&planes, color, target.texture(), out, None)?;
        }
        _ => return Ok(None),
    }
    Ok(Some(target.download()?))
}

#[cfg(target_os = "linux")]
pub fn run(opts: Options) -> Result<(), Box<dyn Error>> {
    let (ti, hi) = (read_index(&opts.tunnel)?, read_index(&opts.hdr10)?);
    println!("tunnel: {} frames, HDR10: {} frames", ti.len(), hi.len());
    let (tp, hp) = (
        fingerprints(&opts.tunnel, &ti)?,
        fingerprints(&opts.hdr10, &hi)?,
    );
    // Every frame for a short clip, about 500 of them for a long one.
    let step = (tp.len() / 500).max(1);
    let Some((d, score)) = best_offset(&tp, &hp, opts.max_shift, step) else {
        return Err("no offset has enough comparable frames".into());
    };
    println!("aligned: tunnel frame i is HDR10 frame i{d:+}, mean match {score:.3}");
    let chosen = pick(&pairs_at(&tp, &hp, d), opts.pairs, 0.95);
    if chosen.is_empty() {
        return Err("no pair matches well enough (0.95)".into());
    }
    let tw: BTreeSet<usize> = chosen.iter().map(|p| p.t).collect();
    let hw: BTreeSet<usize> = chosen.iter().map(|p| p.h).collect();
    let (tb, hb) = (
        frames(&opts.tunnel, &ti, &tw)?,
        frames(&opts.hdr10, &hi, &hw)?,
    );

    let pl = cinefit_render::Placebo::new()?;
    println!(
        "{:>7} {:>7} {:>6} {:>6} | {:>9} {:>7} | {:>8} {:>8} | {:>6} {:>6} {:>8}",
        "tunnel",
        "hdr10",
        "match",
        "motion",
        "PQ med",
        "p99",
        "gain %",
        "black",
        "hue°",
        "chroma",
        "coloured"
    );
    for p in &chosen {
        let (Some(a), Some(b)) = (tb.get(&p.t), hb.get(&p.h)) else {
            continue;
        };
        let (Some(ra), Some(rb)) = (render(&pl, &ti[p.t], a)?, render(&pl, &hi[p.h], b)?) else {
            println!(
                "{:>7} {:>7}  not a tunnel and HDR10 pair: skipped",
                p.t, p.h
            );
            continue;
        };
        let x = diff(&ra, &rb, 3);
        println!(
            "{:>7} {:>7} {:>6.3} {:>6.3} | {:>9.2} {:>7.2} | {:>+8.1} {:>+8.2} | {:>6.1} {:>6.2} {:>8}",
            p.t,
            p.h,
            p.matched,
            p.motion,
            x.pq_median,
            x.pq_p99,
            (x.gain_stops.exp2() - 1.0) * 100.0,
            x.black_codes,
            x.hue_degrees,
            x.chroma_ratio,
            x.coloured
        );
    }
    println!(
        "PQ in 10-bit codes; gain is tunnel against HDR10 luminance (1-1000 nits); black is near-black PQ, tunnel minus HDR10; hue and chroma on coloured pixels"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reproducible noise, no dependency.
    fn noise(seed: &mut u64) -> f32 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        (*seed >> 40) as f32 / (1u64 << 24) as f32
    }

    fn sequence(n: usize, seed: u64) -> Vec<Print> {
        let mut s = seed;
        (0..n)
            .map(|_| normalise((0..GRID_W * GRID_H).map(|_| noise(&mut s)).collect()))
            .collect()
    }

    #[test]
    fn finds_the_offset_between_two_recordings() {
        let t = sequence(300, 0x1234_5678);
        // The HDR10 recording started 17 frames earlier: its frame i + 17 is
        // the tunnel's frame i. A gain and an offset on the values must not
        // matter: fingerprints compare by shape.
        let mut h = sequence(17, 0x9999);
        h.extend(t.iter().map(|p| {
            p.as_ref()
                .map(|v| v.iter().map(|x| 3.0 * x + 7.0).collect())
        }));
        let h: Vec<Print> = h.into_iter().map(|p| p.and_then(normalise)).collect();
        let (d, score) = best_offset(&t, &h, 40, 2).expect("offset");
        assert_eq!(d, 17);
        assert!(score > 0.99, "{score}");
    }

    #[test]
    fn picks_the_stillest_good_match_in_each_stretch() {
        let pairs: Vec<Pair> = (0..100)
            .map(|i| Pair {
                t: i,
                h: i + 5,
                matched: if i == 10 { 0.5 } else { 0.99 },
                motion: if i % 25 == 10 { 0.001 } else { 0.1 },
            })
            .collect();
        let chosen = pick(&pairs, 4, 0.95);
        // Frame 10 is the stillest of its stretch but matches badly.
        assert_eq!(
            chosen.iter().map(|p| p.t).collect::<Vec<_>>(),
            [0, 35, 60, 85]
        );
    }

    #[test]
    fn ictcp_puts_grey_on_the_axis_and_tells_red_from_blue() {
        let grey = ictcp([0.01, 0.01, 0.01]);
        assert!(grey[1].abs() < 1e-3 && grey[2].abs() < 1e-3, "{grey:?}");
        let (red, blue) = (ictcp([0.01, 0.0, 0.0]), ictcp([0.0, 0.0, 0.01]));
        let hue = |c: [f64; 3]| c[2].atan2(c[1]).to_degrees();
        assert!((hue(red) - hue(blue)).abs() > 60.0);
    }

    #[test]
    fn a_colour_error_shows_as_hue_and_a_level_error_as_gain() {
        let img: Vec<u16> = (0..1000u32)
            .flat_map(|i| {
                let v = 20000 + (i * 40) as u16;
                [v, 30000, 60000 - v / 2, 65535]
            })
            .collect();
        // Red and blue swapped: what a chroma or matrix mistake does.
        let swapped: Vec<u16> = img
            .chunks(4)
            .flat_map(|p| [p[2], p[1], p[0], p[3]])
            .collect();
        let d = diff(&swapped, &img, 1);
        assert!(d.hue_degrees > 30.0, "{d:?}");
        // Every channel about 2 % lower in PQ: a level error, little hue.
        let dimmer: Vec<u16> = img.iter().map(|&v| (f64::from(v) * 0.98) as u16).collect();
        let d = diff(&dimmer, &img, 1);
        assert!(d.gain_stops < -0.05, "{d:?}");
        assert!(d.hue_degrees < 5.0, "{d:?}");
    }

    #[test]
    fn identical_renders_differ_by_nothing() {
        // A ramp of colours, the same on both sides.
        let img: Vec<u16> = (0..1000u32)
            .flat_map(|i| {
                let v = (i * 60) as u16;
                [v, 30000, 60000 - v, 65535]
            })
            .collect();
        let d = diff(&img, &img, 1);
        assert_eq!((d.pq_median, d.pq_p99), (0.0, 0.0));
        assert!(d.gain_stops.abs() < 1e-12);
        assert!(d.coloured > 0 && d.hue_degrees < 1e-9);
        assert!((d.chroma_ratio - 1.0).abs() < 1e-12);
    }
}
