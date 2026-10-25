//! Dolby's display mapping for a tunnel, modelled: what the player's
//! low-latency rendering does to a frame, worked out from the frame's own
//! metadata so cinefit can do it at a target the player will not (#141).
//!
//! The model was fitted offline on pairs of tunnel and low-latency frames of
//! the same disc frames (#109), in ICtCp. Per frame, the scene's L1 and the
//! colourist's L2 trims set a curve on intensity; per pixel, chroma follows
//! intensity, over-bright saturated colour is pulled in, and hue is turned a
//! little as Dolby's output turns it. Measured at 96 nits, brightness error
//! x720 per frame: 3.8 Top Gun, 4.3 Project Hail Mary (both in the fit), 6.4
//! A Quiet Place Part II and 4.3 Hitman (the 4000-nit terms fitted on Hitman),
//! against 9.4, 8.7, 5.0 and 10.8 for BT.2390 with peak detection. The
//! 4000-nit terms were refitted on Hitman at 96 and 155 together
//! (2026-10-25): held out, A Quiet Place Part II went from 5.1 to 4.2 at 96
//! and from 4.8 to 4.1 at 155. Fitted on A Quiet Place Part II instead, they
//! fail on Hitman, as its L1 is fixed on every frame: a third 4000-nit title
//! is still owed (#169).
//!
//! Every constant is a fitted value; none is a setting. This module is the
//! reference the GPU pass in the shim is checked against.

/// One trim, as the model reads L2: the target as PQ 0-1 and each field as
/// its 12-bit code / 4096 - 0.5, so 0 is neutral.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Trim {
    pub target: f64,
    pub slope: f64,
    pub offset: f64,
    pub power: f64,
    pub chroma: f64,
    pub saturation: f64,
}

impl Trim {
    /// From L2's 12-bit codes.
    pub fn from_codes(target_max_pq: u16, codes: [u16; 5]) -> Self {
        let q = |v: u16| f64::from(v) / 4096.0 - 0.5;
        Self {
            target: f64::from(target_max_pq) / 4095.0,
            slope: q(codes[0]),
            offset: q(codes[1]),
            power: q(codes[2]),
            chroma: q(codes[3]),
            saturation: q(codes[4]),
        }
    }

    fn neutral(target: f64) -> Self {
        Self {
            target,
            slope: 0.0,
            offset: 0.0,
            power: 0.0,
            chroma: 0.0,
            saturation: 0.0,
        }
    }
}

/// A frame's metadata, as PQ 0-1: L1 (crush the minimum, mid the average,
/// clip the maximum), the grade's declared range and the trims.
#[derive(Debug, Clone, PartialEq)]
pub struct Scene {
    pub crush: f64,
    pub mid: f64,
    pub clip: f64,
    pub source_min: f64,
    pub source_max: f64,
    pub trims: Vec<Trim>,
}

impl Scene {
    /// From the 12-bit PQ codes the metadata carries.
    pub fn from_codes(l1: [u16; 3], source: [u16; 2], trims: Vec<Trim>) -> Self {
        let pq = |v: u16| f64::from(v) / 4095.0;
        Self {
            crush: pq(l1[0]),
            mid: pq(l1[1]),
            clip: pq(l1[2]),
            source_min: pq(source[0]),
            source_max: pq(source[1]),
            trims,
        }
    }
}

/// Points in the intensity table, plus one: the curve is costly per pixel and
/// smooth, so it is tabulated per frame and interpolated.
pub const LUT_POINTS: usize = 1025;

/// Hue turn, radians: a fixed part on cos h, sin h, cos 2h, sin 2h, and a part
/// growing with compression on those and cos 3h, sin 3h.
const HUE_FIX: [f64; 4] = [-0.39, 0.61, -0.09, -0.49];
const HUE_CMP: [f64; 6] = [-18.9, -48.8, -18.2, 27.3, 11.2, -35.5];

/// The fitted constants that depend on neither the frame nor the target.
const TMIN_LOG: f64 = -3.19;
const KSHIFT: f64 = 0.991;
const K1: f64 = 1.379;
const NEUTRAL: f64 = 0.97;
const TRIM_SCALE: f64 = 0.876;
const TRIM_POWER: f64 = 1.138;
const TRIM_SAT: f64 = 0.746;
const CS_G: f64 = 0.577;
const CS_H: f64 = 1.562;
const CS_B: f64 = 2.06;
const CS_S0: f64 = 0.179;
const HUE_S1: f64 = 0.35;
const EXPAND: f64 = 0.8;
/// Grades declared above this many nits get the 4000-nit terms; every
/// 1000-nit grade (1001 in the metadata) stays below it. Fitted on one
/// 4000-nit title so far (Hitman, at 96 and 155), held out on another.
const GRADE_KNEE_NITS: f64 = 1200.0;
const GRADE_PEAK: f64 = 0.227;
const GRADE_ROLLOFF: f64 = -0.379;
const GRADE_SHIFT: f64 = -2.372;
/// The share of the scene's minimum above the target's black that the curve's
/// low end keeps (#235). Fitted on Hitman at 96 and 155; A Quiet Place Part II
/// held out.
const DARK_KEEP: f64 = 0.75;
/// The least spread, PQ, of a scene's L1 and of its curve's ends for the
/// curve to be fitted (16 of the metadata's 12-bit codes); the mid point
/// keeps 1e-3 from either end.
const MIN_SPAN: f64 = 4e-3;

pub fn pq_eotf(e: f64) -> f64 {
    let (m1, m2, c1, c2, c3) = (0.1593017578125, 78.84375, 0.8359375, 18.8515625, 18.6875);
    let p = e.max(0.0).powf(1.0 / m2);
    10000.0 * ((p - c1).max(0.0) / (c2 - c3 * p)).powf(1.0 / m1)
}

pub fn pq_oetf(nits: f64) -> f64 {
    let (m1, m2, c1, c2, c3) = (0.1593017578125, 78.84375, 0.8359375, 18.8515625, 18.6875);
    let y = (nits / 10000.0).max(0.0).powf(m1);
    ((c1 + c2 * y) / (1.0 + c3 * y)).powf(m2)
}

/// One frame's mapping: the intensity table and the per-pixel constants, all
/// the GPU pass needs.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub lut: [f32; LUT_POINTS],
    /// Chroma follows intensity: k1, then the trim's chroma weight and
    /// saturation gain.
    pub k1: f32,
    pub chroma_weight: f32,
    pub saturation_gain: f32,
    /// How far the grade's top sits above the target (PQ), which scales the
    /// pull on over-bright saturated colour and the compression's hue turn.
    pub compression: f32,
    /// The target's peak, PQ.
    pub tmax: f32,
}

impl Frame {
    /// The mapping of a frame with `scene` onto a display of `target_nits`.
    pub fn new(scene: &Scene, target_nits: f64) -> Self {
        // The grade enters through how far its declared top sits above the
        // target; past a 1000-nit grade, through three more slopes.
        let grade = scene.source_max;
        let c = grade - pq_oetf(target_nits);
        let h = (grade - pq_oetf(GRADE_KNEE_NITS)).max(0.0);
        let tmax_nits = target_nits * (0.877 + 0.330 * c + GRADE_PEAK * h);
        let rolloff = 0.314 + 0.123 * c + GRADE_ROLLOFF * h;
        let kshift = KSHIFT * (1.0 + GRADE_SHIFT * h);

        let tmax = pq_oetf(tmax_nits);
        let tmin = pq_oetf(10f64.powf(TMIN_LOG));
        let smin = scene.source_min;
        // L1 as the curve needs it: in range and in order, whatever the
        // metadata carried.
        let (crush, clip) = {
            let (a, b) = (scene.crush.clamp(0.0, 1.0), scene.clip.clamp(0.0, 1.0));
            (a.min(b), a.max(b))
        };
        // The curve spans the scene's own top, not the grade's.
        let smax = clip;
        let tr = trim_at(scene, tmax);
        let (ts, to, tp) = (
            tr.slope * TRIM_SCALE,
            tr.offset * TRIM_SCALE,
            tr.power * TRIM_SCALE * TRIM_POWER,
        );
        let sop = |u: f64| ((1.0 + ts) * u + to).clamp(0.0, 1.0).powf(1.0 + tp);
        let s2t = ((tmax - tmin) / (smax - smin).max(1e-6)).sqrt().min(1.0);
        let slope = (1.0 / s2t).sqrt();
        let smid = scene.mid.max(crush + 1e-3).min(clip - 1e-3);
        let key = (smid - crush) / (clip - crush).max(1e-6);
        let shift = kshift * smid * (1.0 - s2t) * (2.0 * key);
        let r = rolloff.clamp(0.05, 2.0);
        let hi = (clip - shift).min(tmax);
        // The low end is held above the target's black by a share of how far
        // the scene's minimum sits above it: on grades whose L1 minimum is a
        // pinned value (both 4000-nit titles, 0.0151 on every frame), the
        // shift alone takes it to black and everything below the minimum
        // with it, where Dolby keeps the separation (#235). Never so far up
        // that it meets the top: a scene with nothing dark in it would lose
        // its curve.
        let lo = (crush - shift)
            .max(tmin)
            .max((tmin + DARK_KEEP * (crush - tmin)).min(hi - MIN_SPAN));
        let n = slope / r;
        let (x1, x2, x3) = (crush.max(1e-6).powf(n), smid.powf(n), clip.powf(n));
        let (y1, y2, y3) = (
            lo.max(1e-9).powf(1.0 / r),
            (smid - shift).max(1e-6).powf(1.0 / r),
            hi.powf(1.0 / r),
        );
        // The three-point rational curve through (crush, lo), (mid, mid
        // shifted) and (clip, hi), in powers of n and 1/r.
        let temp = x3 * y3 * (x1 - x2) + x2 * y2 * (x3 - x1) + x1 * y1 * (x2 - x3);
        let c1 =
            (x2 * x3 * (y2 - y3) * y1 - x1 * x3 * (y1 - y3) * y2 + x1 * x2 * (y1 - y2) * y3) / temp;
        let c2 = (-(x2 * y2 - x3 * y3) * y1 + (x1 * y1 - x3 * y3) * y2 - (x1 * y1 - x2 * y2) * y3)
            / temp;
        let c3 = ((x3 - x2) * y1 - (x3 - x1) * y2 + (x2 - x1) * y3) / temp;
        // A scene with no spread (black, one level, or a top at or below the
        // target's black) has no three distinct points to fit, and its bounds
        // can cross: it passes one to one within the target's range instead
        // (#193).
        let fitted = clip - crush >= MIN_SPAN
            && hi - lo >= MIN_SPAN
            && [c1, c2, c3].iter().all(|c| c.is_finite());
        let curve = |i0: f64| {
            let mut i = if fitted {
                let u = ((i0 - smin) / (smax - smin).max(1e-6)).clamp(0.0, 1.0);
                let x = (smin + u * (smax - smin)).clamp(crush, clip).powf(n);
                ((c1 + c2 * x) / (1.0 + c3 * x))
                    .max(0.0)
                    .powf(r)
                    .clamp(lo, hi)
            } else {
                i0.clamp(tmin, tmax)
            };
            // Below the scene's minimum, a straight line down to the
            // target's black. Not for the one to one: it already ends there.
            if fitted && i0 < crush {
                i = tmin + (i - tmin) * (i0.max(0.0) / crush.max(1e-6));
            }
            // A scene below the target is lifted towards it.
            if tmax > scene.source_max {
                i = (i * (1.0 + EXPAND * (tmax - scene.source_max))).min(tmax);
            }
            sop(i.clamp(0.0, 1.0)).clamp(tmin, tmax)
        };
        let n_lut = (LUT_POINTS - 1) as f64;
        Self {
            lut: std::array::from_fn(|k| curve(k as f64 / n_lut) as f32),
            k1: K1 as f32,
            chroma_weight: tr.chroma as f32,
            saturation_gain: (tr.saturation * TRIM_SAT) as f32,
            compression: (smax - tmax).max(0.0) as f32,
            tmax: tmax as f32,
        }
    }

    /// One pixel, ICtCp in (Ct as defined, not halved) and out. What the GPU
    /// pass computes, in f64 for the checks.
    pub fn apply(&self, [i0, ct, cp]: [f64; 3]) -> [f64; 3] {
        let lut = |k: usize| f64::from(self.lut[k]);
        let n = (LUT_POINTS - 1) as f64;
        let t = i0.clamp(0.0, 1.0) * n;
        let k = (t as usize).min(LUT_POINTS - 2);
        let i = lut(k) + (t - k as f64) * (lut(k + 1) - lut(k));
        let k1 = f64::from(self.k1);
        let kk = if i0 > 1e-4 && i > 1e-4 {
            (i * (k1 * i0 + 1.0)) / (i0 * (k1 * i + 1.0))
        } else {
            1.0
        };
        let kk = (1.0 + (1.0 + f64::from(self.chroma_weight)) * (kk - 1.0))
            * (1.0 + f64::from(self.saturation_gain));
        let mut out = [i.max(0.0), ct * kk, cp * kk];
        // Near black, hue and saturation mean little: left as they are.
        if i0 <= 0.02 {
            return out;
        }
        let comp = f64::from(self.compression);
        let sat = ((0.5 * ct).powi(2) + cp.powi(2)).sqrt() / i0;
        let w = comp
            * (sat - CS_S0).max(0.0)
            * (out[0] / f64::from(self.tmax))
                .clamp(0.0, 1.5)
                .powf(CS_B.clamp(0.0, 8.0));
        out[0] = (out[0] - CS_G * w).max(0.0);
        let kc = (1.0 - CS_H * w).clamp(0.0, 2.0);
        out[1] *= kc;
        out[2] *= kc;
        let h = cp.atan2(0.5 * ct);
        let b = [
            h.cos(),
            h.sin(),
            (2.0 * h).cos(),
            (2.0 * h).sin(),
            (3.0 * h).cos(),
            (3.0 * h).sin(),
        ];
        let wh = comp * (sat - HUE_S1).max(0.0);
        let fix: f64 = (0..4).map(|j| b[j] * HUE_FIX[j].to_radians()).sum();
        let cmp: f64 = (0..6).map(|j| b[j] * HUE_CMP[j].to_radians()).sum();
        let ang = fix + wh * cmp;
        let (oc, op) = (0.5 * out[1], out[2]);
        out[1] = 2.0 * (oc * ang.cos() - op * ang.sin());
        out[2] = oc * ang.sin() + op * ang.cos();
        out
    }
}

impl Frame {
    /// As the shim takes it, with the model's fixed constants.
    pub(crate) fn raw(&self) -> crate::ffi::Dm {
        let rad = |d: f64| d.to_radians() as f32;
        crate::ffi::Dm {
            lut: self.lut,
            k1: self.k1,
            chroma_weight: self.chroma_weight,
            saturation_gain: self.saturation_gain,
            compression: self.compression,
            tmax: self.tmax,
            cs: [CS_G, CS_H, CS_B, CS_S0].map(|v| v as f32),
            hue_s1: HUE_S1 as f32,
            hue_fix: HUE_FIX.map(rad),
            hue_cmp: HUE_CMP.map(rad),
        }
    }
}

/// The trim for a target between the colourist's: interpolated, and neutral
/// towards the grade's own top above the highest.
fn trim_at(s: &Scene, t: f64) -> Trim {
    let mut v = s.trims.clone();
    v.sort_by(|a, b| a.target.total_cmp(&b.target));
    let Some(&top) = v.last() else {
        return Trim::neutral(t);
    };
    if s.source_max > top.target {
        let at = (top.target + NEUTRAL.clamp(0.02, 1.5) * (s.source_max - top.target))
            .max(top.target + 1e-4);
        v.push(Trim::neutral(at));
    }
    if t <= v[0].target {
        return v[0];
    }
    for w in v.windows(2) {
        let (a, b) = (w[0], w[1]);
        if t <= b.target {
            let f = (t - a.target) / (b.target - a.target);
            let l = |x: f64, y: f64| x + f * (y - x);
            return Trim {
                target: t,
                slope: l(a.slope, b.slope),
                offset: l(a.offset, b.offset),
                power: l(a.power, b.power),
                chroma: l(a.chroma, b.chroma),
                saturation: l(a.saturation, b.saturation),
            };
        }
    }
    v[v.len() - 1]
}

/// BT.2020 RGB to LMS, and LMS' to ICtCp (BT.2100).
const RGB_LMS: [[f64; 3]; 3] = [
    [1688.0 / 4096.0, 2146.0 / 4096.0, 262.0 / 4096.0],
    [683.0 / 4096.0, 2951.0 / 4096.0, 462.0 / 4096.0],
    [99.0 / 4096.0, 309.0 / 4096.0, 3688.0 / 4096.0],
];
const LMS_ICTCP: [[f64; 3]; 3] = [
    [0.5, 0.5, 0.0],
    [6610.0 / 4096.0, -13613.0 / 4096.0, 7003.0 / 4096.0],
    [17933.0 / 4096.0, -17390.0 / 4096.0, -543.0 / 4096.0],
];

fn mul(m: &[[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|r| m[r][0] * v[0] + m[r][1] * v[1] + m[r][2] * v[2])
}

fn inverse(m: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let c = |a: usize, b: usize, cc: usize, dd: usize| m[a][b] * m[cc][dd] - m[a][dd] * m[cc][b];
    let d = m[0][0] * c(1, 1, 2, 2) - m[0][1] * c(1, 0, 2, 2) + m[0][2] * c(1, 0, 2, 1);
    [
        [c(1, 1, 2, 2) / d, -c(0, 1, 2, 2) / d, c(0, 1, 1, 2) / d],
        [-c(1, 0, 2, 2) / d, c(0, 0, 2, 2) / d, -c(0, 0, 1, 2) / d],
        [c(1, 0, 2, 1) / d, -c(0, 0, 2, 1) / d, c(0, 0, 1, 1) / d],
    ]
}

/// The two inverses the GPU pass needs, row-major: ICtCp to LMS', and LMS to
/// BT.2020 RGB.
pub fn inverses() -> ([[f64; 3]; 3], [[f64; 3]; 3]) {
    (inverse(LMS_ICTCP), inverse(RGB_LMS))
}

/// BT.2020 PQ RGB (0-1) to ICtCp.
pub fn rgb_to_ictcp(pq: [f64; 3]) -> [f64; 3] {
    let lms = mul(&RGB_LMS, pq.map(pq_eotf)).map(pq_oetf);
    mul(&LMS_ICTCP, lms)
}

/// ICtCp to BT.2020 PQ RGB (0-1); out-of-gamut channels clamp at 0.
pub fn ictcp_to_rgb(x: [f64; 3]) -> [f64; 3] {
    let (to_lms, to_rgb) = inverses();
    mul(&to_rgb, mul(&to_lms, x).map(pq_eotf)).map(pq_oetf)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Values from the offline model (dmv3b.py, Hitman's 4000-nit terms) that
    // the measurements above were made with.
    #[test]
    fn matches_the_offline_model() {
        for (scene, target, pixels) in golden::CASES {
            let f = Frame::new(&scene(), *target);
            for (input, want) in pixels.iter() {
                let got = f.apply(*input);
                for c in 0..3 {
                    assert!(
                        (got[c] - want[c]).abs() < 2e-6,
                        "target {target}, pixel {input:?}: got {got:?}, want {want:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn ictcp_round_trips() {
        for rgb in [[0.1, 0.5, 0.3], [0.58, 0.58, 0.58], [0.9, 0.2, 0.05]] {
            let back = ictcp_to_rgb(rgb_to_ictcp(rgb));
            for c in 0..3 {
                assert!((back[c] - rgb[c]).abs() < 1e-9, "{rgb:?} -> {back:?}");
            }
        }
    }

    #[test]
    fn a_neutral_trim_list_is_neutral() {
        let s = Scene::from_codes([62, 1500, 3079], [62, 3079], vec![]);
        let t = trim_at(&s, 0.5);
        assert_eq!(t, Trim::neutral(0.5));
    }

    // L1 the metadata can carry but no grade makes: black, one level, a top
    // below the target's black, a top below the minimum, codes past 12 bits.
    // Each must give a finite, rising table inside the target (#193).
    #[test]
    fn degenerate_l1_maps_without_panic() {
        let cases: [[u16; 3]; 9] = [
            [0, 0, 0],
            [0, 0, 1],
            [62, 62, 62],
            [2000, 2000, 2000],
            [100, 101, 103],
            [3000, 1000, 500],
            [1500, 1500, 1499],
            [0, 4095, 0],
            [65535, 65535, 65535],
        ];
        for l1 in cases {
            for target in [48.0, 96.0, 155.0, 1000.0] {
                let f = Frame::new(&Scene::from_codes(l1, [62, 3696], vec![]), target);
                let (tmin, tmax) = (pq_oetf(10f64.powf(TMIN_LOG)), f64::from(f.tmax));
                for w in f.lut.windows(2) {
                    let (a, b) = (f64::from(w[0]), f64::from(w[1]));
                    assert!(a.is_finite() && b.is_finite(), "{l1:?} at {target}");
                    assert!(b >= a - 1e-6, "{l1:?} at {target}: {a} then {b}");
                    assert!(a >= tmin - 1e-6 && b <= tmax + 1e-6, "{l1:?} at {target}");
                }
            }
        }
    }

    // Black stays black, and a scene of one level passes one to one inside
    // the target, above that level and below it.
    #[test]
    fn a_scene_without_spread_passes_one_to_one() {
        let s = |l1| Scene::from_codes(l1, [62, 3696], vec![]);
        let f = Frame::new(&s([0, 0, 0]), 96.0);
        let tmin = pq_oetf(10f64.powf(TMIN_LOG));
        assert!((f64::from(f.lut[0]) - tmin).abs() < 1e-6);
        let k = (LUT_POINTS - 1) / 4;
        let i0 = k as f64 / (LUT_POINTS - 1) as f64;
        for l1 in [[1000, 1000, 1000], [2000, 2000, 2000]] {
            let got = f64::from(Frame::new(&s(l1), 1000.0).lut[k]);
            assert!((got - i0).abs() < 1e-3, "{l1:?}: {got} vs {i0}");
        }
    }

    // A pinned L1 minimum above the target's black (both 4000-nit titles carry
    // 0.0151 on every frame) keeps the dark end rising below it, not flat at
    // black (#235).
    #[test]
    fn the_dark_end_below_a_pinned_minimum_keeps_its_steps() {
        let tmin = pq_oetf(10f64.powf(TMIN_LOG));
        let crush = (0.0151f64 * 4095.0).round() as u16;
        for target in [96.0, 155.0] {
            let f = Frame::new(
                &Scene::from_codes([crush, 760, 3079], [62, 3696], vec![]),
                target,
            );
            let below = (f64::from(crush) / 4095.0 * (LUT_POINTS - 1) as f64) as usize;
            assert!(
                f64::from(f.lut[below]) > tmin + 1e-3,
                "at {target}: {}",
                f.lut[below]
            );
            for w in f.lut[..=below].windows(2) {
                assert!(w[1] > w[0], "at {target}: flat below the minimum");
            }
        }
        // A scene with nothing dark in it keeps its fitted curve: the floor
        // stays below the top, rather than taking it to the one to one.
        let bright = Scene::from_codes([3039, 3075, 3079], [0, 3079], vec![]);
        let f = Frame::new(&bright, 96.0);
        let mid = (3060.0 / 4095.0 * (LUT_POINTS - 1) as f64) as usize;
        assert!(
            f64::from(f.lut[mid]) < f64::from(f.tmax) - 1e-3,
            "one to one at {} (tmax {})",
            f.lut[mid],
            f.tmax
        );
    }

    // Reversed L1 maps as the same L1 in order.
    #[test]
    fn reversed_l1_maps_as_ordered() {
        let s = |l1| Scene::from_codes(l1, [62, 3696], vec![]);
        for target in [96.0, 155.0] {
            let reversed = Frame::new(&s([3000, 1000, 500]), target);
            assert_eq!(reversed, Frame::new(&s([500, 1000, 3000]), target));
        }
    }

    mod golden {
        include!("dm_golden.rs");
    }
}
