//! Part 1 of the detector: one frame in, one observation out: where each
//! side's bar ends and how much the edge looks like a matte. Ported from the
//! C++ version's `detect/bars.cpp`; the algorithm is its `docs/detector.md`,
//! "cinefit / Part 1", and its reasons its `docs/plan.md` M5.
//!
//! Constants marked [measured] come from real discs (Interstellar's bars are
//! flat at 64, 99.99% within 64-67, with at most one row of edge ringing);
//! the rest are starting points set against recorded clips.

use crate::{RowScratch, Samples};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    /// [measured] Luma lit margin above the bar level. Set by the rows *next
    /// to* the picture, not the bar's flat interior: compression ringing
    /// spreads a few rows out from the edge (rows 1-6 outside Interstellar's
    /// have 0.2-3.9% of samples above bar + 4, under 0.1% above bar + 16). A
    /// margin of 4 moved edges out by up to 8 rows. Dark picture still clears
    /// 16: 0.01 nits is about code 83.
    pub delta: u32,
    /// Chroma lit margin from neutral (bars sit within 3).
    pub delta_c: u32,
    /// A sample lit by chroma alone needs a run of this many chroma-lit pairs
    /// (C++ #106): compression rings in the chroma of the black rows next to
    /// the picture at isolated pairs, while dark coloured picture lights
    /// stretches of a row.
    pub chroma_run: u32,
    /// Lit samples for a row to be lit: specks and ringing cannot end a bar.
    pub min_lit: u32,
    /// Of the height, per side: 2.76's bars are 384 rows of 2160.
    pub band: f64,
    /// Of the height: the longest lit run that can be an island.
    pub island_max: f64,
    /// Rows at 2160 of black that must follow an island.
    pub island_gap: u32,
    /// [measure] Lit fraction above which an island is UI, not a subtitle.
    pub ui_density: f64,
    /// Rows inside an edge for coverage and strength.
    pub edge_rows: u32,
    /// Rows at 2160 inside an edge for fill and zones.
    pub window: u32,
    /// [measured] Rows at 2160 between the top and bottom bars.
    pub tol_sym: u32,
    /// Hard gate on the lit fraction of rows just inside the edge.
    pub fill_floor: f64,
    /// Codes of mean luma excess that count as full strength.
    pub strength_span: f64,
    /// Rows at 2160 from the frame edge that must agree on the bar level for
    /// it to be measured rather than nominal black.
    pub level_rows: u32,
    /// Codes the level rows' medians may spread: bars dither over 62-66.
    pub level_tol: u32,
    /// [measured] The highest level a bar is taken to have. Film bars sit at
    /// 62-66, but Top Gun: Maverick's featurette "Cleared for Take Off" has
    /// grey bars, flat at 76, around its film clips; a flat edge brighter than
    /// this is picture (a sky, a menu's grey panel at 300). At nominal black's
    /// lit threshold: a bar refused for it would be lit at nominal black too.
    pub level_max: u32,
    /// [measured] Rows at 2160 at the frame edge that may be a line of their
    /// own, outside the bar: Blade Runner 2049's rating card has one flat
    /// row at 95 on each side, over bar at 64.
    pub edge_lines: u32,
    /// [measured] Rows at 2160 of black line that sharpness looks past, inside
    /// the edge. Top Gun: Maverick's featurette puts two rows at 64 between
    /// its grey bars (76, ending on a row of ringing) and each film clip.
    pub black_line: u32,
}

impl Params {
    /// The names [`Params::set`] takes: the fields'.
    pub const KEYS: &[&str] = &[
        "delta",
        "delta_c",
        "chroma_run",
        "min_lit",
        "band",
        "island_max",
        "island_gap",
        "ui_density",
        "edge_rows",
        "window",
        "tol_sym",
        "fill_floor",
        "strength_span",
        "level_rows",
        "level_tol",
        "level_max",
        "edge_lines",
        "black_line",
    ];

    /// One parameter by name, for trying a bar reader in replay. Before
    /// #224 the level was the outermost row's alone, without a ceiling:
    /// `level_rows=1,edge_lines=0,level_max=1023` reads as that did.
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        let bad = || format!("{key}: {value} is not a number of the right kind");
        let int = || value.parse::<u32>().map_err(|_| bad());
        let real = || {
            value
                .parse::<f64>()
                .ok()
                .filter(|v| v.is_finite())
                .ok_or_else(bad)
        };
        match key {
            "delta" => self.delta = int()?,
            "delta_c" => self.delta_c = int()?,
            "chroma_run" => self.chroma_run = int()?,
            "min_lit" => self.min_lit = int()?,
            "band" => self.band = real()?,
            "island_max" => self.island_max = real()?,
            "island_gap" => self.island_gap = int()?,
            "ui_density" => self.ui_density = real()?,
            "edge_rows" => self.edge_rows = int()?,
            "window" => self.window = int()?,
            "tol_sym" => self.tol_sym = int()?,
            "fill_floor" => self.fill_floor = real()?,
            "strength_span" => self.strength_span = real()?,
            "level_rows" => self.level_rows = int()?,
            "level_tol" => self.level_tol = int()?,
            "level_max" => self.level_max = int()?,
            "edge_lines" => self.edge_lines = int()?,
            "black_line" => self.black_line = int()?,
            _ => {
                return Err(format!(
                    "no bar reader parameter {key}; one of {}",
                    Self::KEYS.join(", ")
                ));
            }
        }
        Ok(())
    }
}

impl Default for Params {
    fn default() -> Self {
        Self {
            delta: 16,
            delta_c: 16,
            chroma_run: 3,
            min_lit: 8,
            band: 3.0 / 16.0,
            island_max: 0.10,
            island_gap: 8,
            ui_density: 0.5,
            edge_rows: 4,
            window: 64,
            tol_sym: 2,
            fill_floor: 0.5,
            strength_span: 48.0,
            level_rows: 8,
            level_tol: 4,
            level_max: 80,
            edge_lines: 2,
            black_line: 2,
        }
    }
}

/// What one side shows.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SideObs {
    /// A picture edge within the band.
    pub found: bool,
    /// Rows of bar from the frame edge to the picture.
    pub bar: u32,
    /// Consecutive lit rows from the edge, capped at the band.
    pub run: u32,
    /// The bar level used for this side.
    pub level: u32,
    /// Of the level rows, the share that support `level`: flat, neutral and
    /// within the level tolerance of it. 1 when every row agrees; `None` from
    /// readings saved before it existed.
    pub level_confidence: Option<f64>,
    /// Mean lit fraction of the rows just inside the edge.
    pub coverage: f64,
    /// Mean luma excess there over the strength span, capped at 1.
    pub strength: f64,
    /// The first picture row's excess over a few rows in: a step is about 1.
    pub sharpness: f64,
    /// Lit rows over window rows, just inside the edge.
    pub fill: f64,
    /// Horizontal quarters with picture just inside the edge.
    pub zones: u32,
    /// Picture crosses the edge: scenery, not a bar.
    pub continues: bool,
    /// Sparse islands skipped (subtitles), and the outermost one's first row.
    pub islands: u32,
    pub island_start: u32,
    /// A dense island (a menu, a transport bar) was seen, at this row.
    pub ui: bool,
    pub ui_start: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FrameClass {
    #[default]
    Empty,
    Full,
    Letterbox,
    Weak,
}

impl FrameClass {
    pub fn name(self) -> &'static str {
        match self {
            FrameClass::Empty => "EMPTY",
            FrameClass::Full => "FULL",
            FrameClass::Letterbox => "LETTERBOX",
            FrameClass::Weak => "WEAK",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Observation {
    pub class: FrameClass,
    pub width: u32,
    pub height: u32,
    pub top: SideObs,
    pub bottom: SideObs,
    /// LETTERBOX only: the weakest of the sides' signals.
    pub strength: f64,
    /// WEAK only: the first hard gate that failed.
    pub why: &'static str,
}

fn scaled(rows_at_2160: u32, h: u32) -> u32 {
    ((u64::from(rows_at_2160) * u64::from(h) / 2160) as u32).max(1)
}

/// Pair flags kept only inside a run of at least `run` lit pairs, in place:
/// each run of lit pairs measured once, end to end, rather than a window
/// tested at every pair. `run` is at least 1.
fn in_runs(flags: &mut [u8], run: usize) {
    let mut p = 0;
    while p < flags.len() {
        let len = flags[p..].iter().take_while(|&&f| f != 0).count();
        let keep = u8::from(len >= run);
        for f in &mut flags[p..p + len] {
            *f &= keep;
        }
        p += len.max(1);
    }
}

/// Measures one frame. Keeps its buffers between frames.
#[derive(Default)]
pub struct Bars {
    p: Params,
    buf: Buffers,
}

/// One side's band of rows, kept between frames so a frame allocates
/// nothing: rows read through [`Samples`], and what was learned from them.
#[derive(Default)]
struct Buffers {
    luma: Vec<u16>,
    chroma: Vec<[u16; 2]>,
    have_luma: Vec<bool>,
    have_chroma: Vec<bool>,
    /// Per band row: lit samples and the quarters they are in.
    lit: Vec<Option<(u32, u8)>>,
    /// One row's chroma-lit flags per pair.
    flags: Vec<u8>,
    /// The continuation probes' rows of lit flags.
    probes: [Vec<bool>; 3],
    /// The level rows' medians, for those flat and neutral.
    levels: Vec<Option<u32>>,
    /// For the counting methods of a format without its own.
    scratch: RowScratch,
}

/// One side's band of rows, read lazily from the frame edge inward.
struct Side<'a> {
    f: &'a dyn Samples,
    p: &'a Params,
    b: &'a mut Buffers,
    w: usize,
    pairs: usize,
    h: u32,
    from_bottom: bool,
    band: u32,
    level: u32,
}

impl Side<'_> {
    fn y(&self, i: u32) -> u32 {
        if self.from_bottom { self.h - 1 - i } else { i }
    }

    fn luma(&mut self, i: u32) -> &[u16] {
        let (w, k) = (self.w, i as usize);
        if !self.b.have_luma[k] {
            let y = self.y(i);
            self.f.luma_row(y, &mut self.b.luma[k * w..(k + 1) * w]);
            self.b.have_luma[k] = true;
        }
        &self.b.luma[k * w..(k + 1) * w]
    }

    fn chroma(&mut self, i: u32) -> &[[u16; 2]] {
        let (n, k) = (self.pairs, i as usize);
        if !self.b.have_chroma[k] {
            let y = self.y(i);
            self.f.chroma_row(y, &mut self.b.chroma[k * n..(k + 1) * n]);
            self.b.have_chroma[k] = true;
        }
        &self.b.chroma[k * n..(k + 1) * n]
    }

    /// Row `i`'s chroma-lit pair flags into `b.flags`, kept only inside a
    /// run of at least `chroma_run` pairs.
    fn run_flags(&mut self, i: u32) {
        let y = self.y(i);
        let mut flags = std::mem::take(&mut self.b.flags);
        self.f
            .chroma_flags(y, self.p.delta_c, &mut flags, &mut self.b.scratch);
        let run = self.p.chroma_run as usize;
        if run > 1 && flags.contains(&1) {
            in_runs(&mut flags, run);
        }
        self.b.flags = flags;
    }

    /// Row `i`'s lit samples and the quarters with one, counted in place by
    /// the frame's format: the scan's whole cost.
    fn lit_count(&mut self, i: u32) -> (u32, u8) {
        if let Some(c) = self.b.lit[i as usize] {
            return c;
        }
        let (y, thr) = (self.y(i), (self.level + self.p.delta) as u16);
        let (mut n, mut mask, chroma_only) =
            self.f
                .count_lit(y, thr, self.p.delta_c, &mut self.b.scratch);
        // The run rule can only lower the count of a row with samples lit by
        // chroma alone; flat bar rows and picture lit by luma skip it.
        if self.p.chroma_run > 1 && chroma_only > 0 {
            self.run_flags(i);
            (n, mask) = self
                .f
                .count_lit_flags(y, thr, &self.b.flags, &mut self.b.scratch);
        }
        self.b.lit[i as usize] = Some((n, mask));
        (n, mask)
    }

    fn row_lit(&mut self, i: u32) -> bool {
        self.lit_count(i).0 >= self.p.min_lit
    }

    /// Every sample's lit flag, by the run rule, into probe row `k`: for the
    /// continuation probes.
    fn lit_pixels(&mut self, i: u32, k: usize) {
        let thr = self.level + self.p.delta;
        self.run_flags(i);
        let (flags, mut out) = (
            std::mem::take(&mut self.b.flags),
            std::mem::take(&mut self.b.probes[k]),
        );
        out.clear();
        out.extend(
            self.luma(i)
                .iter()
                .enumerate()
                .map(|(x, &l)| u32::from(l) > thr || flags[x / 2] != 0),
        );
        (self.b.flags, self.b.probes[k]) = (flags, out);
    }

    /// Row `i`'s median, if the row is flat and neutral, i.e. could be bar.
    /// The 10th, 50th and 90th percentiles by histogram, as a sort would pick.
    fn row_level(&mut self, i: u32) -> Option<u32> {
        let (w, p) = (self.w, self.p);
        let mut hist = [0u32; 1024];
        for &l in self.luma(i) {
            hist[usize::from(l.min(1023))] += 1;
        }
        let nth = |k: usize| {
            let mut seen = 0usize;
            for (c, &n) in hist.iter().enumerate() {
                seen += n as usize;
                if seen > k {
                    return c as u32;
                }
            }
            1023
        };
        let (p10, p50, p90) = (nth(w / 10), nth(w / 2), nth(w * 9 / 10));
        let cmax = self
            .chroma(i)
            .iter()
            .flat_map(|c| c.iter())
            .map(|&c| u32::from(c).abs_diff(512))
            .max()
            .unwrap_or(0);
        (p90 - p10 <= 2 * p.delta && cmax <= 4 * p.delta_c && p50 >= 32).then_some(p50)
    }

    /// Mean luma above the bar level in row `i`, at least 0.
    fn excess(&mut self, i: u32) -> f64 {
        let i = i.min(self.band - 1);
        let level = f64::from(self.level);
        let luma = self.luma(i);
        let sum: u64 = luma.iter().map(|&l| u64::from(l)).sum();
        (sum as f64 / luma.len() as f64 - level).max(0.0)
    }
}

impl Bars {
    pub fn new(p: Params) -> Self {
        Self {
            p,
            buf: Buffers::default(),
        }
    }

    pub fn params(&self) -> &Params {
        &self.p
    }

    pub fn observe(&mut self, f: &dyn Samples) -> Observation {
        let (w, h) = (f.width(), f.height());
        let mut o = Observation {
            width: w,
            height: h,
            ..Default::default()
        };
        if w < 64 || h < 64 {
            return o;
        }
        o.top = self.measure_side(f, false);
        o.bottom = self.measure_side(f, true);
        let (t, b) = (o.top, o.bottom);

        if !t.found && !b.found {
            o.class = FrameClass::Empty;
            return o;
        }
        if t.found && b.found && t.bar == 0 && b.bar == 0 && t.zones == 4 && b.zones == 4 {
            o.class = FrameClass::Full;
            return o;
        }
        // Hard gates, each meaning "this is not a bar edge"; the first to fail
        // names the frame.
        o.class = FrameClass::Weak;
        let why = if !t.found || !b.found {
            Some("no edge on one side")
        } else if t.bar == 0 || b.bar == 0 {
            Some("full height on one side")
        } else if t.bar.abs_diff(b.bar) > scaled(self.p.tol_sym, h) {
            Some("asymmetric")
        } else if t.continues || b.continues {
            Some("picture continues across the edge")
        } else if t.fill.min(b.fill) < self.p.fill_floor {
            Some("low fill")
        } else if t.zones.min(b.zones) < 4 {
            Some("picture missing from a zone")
        } else {
            None
        };
        if let Some(why) = why {
            o.why = why;
            return o;
        }
        // Weakest link: a frame is only as trustworthy as its worst signal.
        let weakest = |s: &SideObs| s.coverage.min(s.strength).min(s.sharpness);
        o.class = FrameClass::Letterbox;
        o.strength = weakest(&t).min(weakest(&b));
        o
    }

    fn measure_side(&mut self, f: &dyn Samples, from_bottom: bool) -> SideObs {
        let p = &self.p;
        let (w, h) = (f.width() as usize, f.height());
        // Rows indexed from this side's frame edge inward, read only when
        // first needed: the scan stops a window past the edge.
        let band = (h / 2).min(((f64::from(h) * p.band) as u32).max(32));
        let (rows, pairs) = (band as usize, w.div_ceil(2));
        let b = &mut self.buf;
        b.luma.resize(rows * w, 0);
        b.chroma.resize(rows * pairs, [512, 512]);
        b.have_luma.clear();
        b.have_luma.resize(rows, false);
        b.have_chroma.clear();
        b.have_chroma.resize(rows, false);
        b.lit.clear();
        b.lit.resize(rows, None);
        let mut s = Side {
            f,
            p,
            b,
            w,
            pairs,
            h,
            from_bottom,
            band,
            level: 64,
        };
        let mut obs = SideObs::default();

        // Bar level: the median of the outer rows' medians, if every one of
        // those rows could be bar and they agree, and the result is dark
        // enough to be bar; otherwise nominal black. One row decides nothing,
        // and a flat edge is not bar for being flat: Blade Runner 2049's menus
        // have a grey panel at 300 there, which is UI. A line of its own at
        // the very edge, flat and neutral, with agreeing bar behind it, is
        // passed over, rows and scan both: as content it would read as UI at
        // row 0 and hold the picture uncropped.
        let n = scaled(p.level_rows, h).min(band);
        let lines = if p.edge_lines == 0 {
            0
        } else {
            scaled(p.edge_lines, h).min(n / 2)
        };
        let mut levels = std::mem::take(&mut s.b.levels);
        levels.clear();
        levels.extend((0..n).map(|i| s.row_level(i)));
        let mut skip = 0;
        for k in 0..=lines as usize {
            if levels[..k].iter().any(Option::is_none) {
                break;
            }
            let (lo, hi) = levels[k..]
                .iter()
                .fold((u32::MAX, 0), |(lo, hi), l| match l {
                    Some(l) => (lo.min(*l), hi.max(*l)),
                    None => (0, u32::MAX),
                });
            if hi - lo > p.level_tol {
                continue;
            }
            // All measured and within the tolerance: the middle one. Sorting
            // the rows in place changes nothing below, which only counts them.
            levels[k..].sort_unstable();
            let level = levels[k + (n as usize - k) / 2].unwrap_or(64);
            if level <= p.level_max {
                s.level = level;
                skip = k as u32;
            }
            break;
        }
        let support = levels
            .iter()
            .flatten()
            .filter(|&&l| l.abs_diff(s.level) <= p.level_tol)
            .count();
        obs.level_confidence = Some(support as f64 / f64::from(n));
        s.b.levels = levels;
        obs.level = s.level;

        let island_max = (f64::from(h) * p.island_max) as u32;
        let gap = scaled(p.island_gap, h);

        // Scan inward. A short lit run followed by black is an island: a dense
        // one is UI (kept as content: it must not be cropped), a sparse one a
        // subtitle (skipped: the crop cuts it, by decision).
        let (mut r, mut edge) = (skip, band);
        while r < band {
            if !s.row_lit(r) {
                r += 1;
                continue;
            }
            let mut run = 0;
            while r + run < band && s.row_lit(r + run) {
                run += 1;
            }
            let mut isolated = run <= island_max && r + run + gap <= band;
            let mut g = 0;
            while isolated && g < gap {
                if s.row_lit(r + run + g) {
                    isolated = false;
                }
                g += 1;
            }
            if isolated {
                let n: u64 = (r..r + run).map(|i| u64::from(s.lit_count(i).0)).sum();
                let density = n as f64 / (f64::from(run) * w as f64);
                if density >= p.ui_density {
                    if !obs.ui {
                        obs.ui = true;
                        obs.ui_start = r;
                    }
                } else {
                    if obs.islands == 0 {
                        obs.island_start = r;
                    }
                    obs.islands += 1;
                }
                r += run + gap;
                continue;
            }
            edge = r;
            obs.run = run;
            break;
        }
        if edge >= band {
            return obs; // nothing but black and islands in the band
        }
        obs.found = true;
        obs.bar = edge;
        let last = band - 1;

        // Coverage and strength just inside the edge.
        let (mut cov, mut ex) = (0.0, 0.0);
        for k in 0..p.edge_rows {
            let i = (edge + k).min(last);
            cov += f64::from(s.lit_count(i).0) / w as f64;
            ex += s.excess(i);
        }
        obs.coverage = cov / f64::from(p.edge_rows);
        obs.strength = (ex / f64::from(p.edge_rows) / p.strength_span).min(1.0);

        // Sharpness: a matte edge reaches full level within a row or two, a
        // dark scene ramps up over many. Row edge+1 skips the ringing row; the
        // reference is 12 rows in, deep enough that a ramp is still climbing
        // where a real edge has long been flat. A black line between bar and
        // picture (rows inside the edge no brighter than the bar, which a ramp
        // never has) moves both past it, where the picture starts (#258).
        let line = if p.black_line == 0 {
            0
        } else {
            (1..=scaled(p.black_line, h))
                .take_while(|&k| s.excess(edge + k) < 1.0)
                .count() as u32
        };
        let start = edge + line + u32::from(line > 0);
        let deep = s.excess(start + scaled(12, h));
        obs.sharpness = if deep > 1.0 {
            (s.excess(start + 1) / deep).min(1.0)
        } else {
            0.0
        };

        // Fill and zones over a window just inside the edge.
        let win = scaled(p.window, h).min(band - edge);
        let (mut lit_rows, mut zone) = (0u32, 0u8);
        for k in 0..win {
            let i = edge + k;
            if s.row_lit(i) {
                lit_rows += 1;
            }
            zone |= s.lit_count(i).1;
        }
        obs.fill = if win > 0 {
            f64::from(lit_rows) / f64::from(win)
        } else {
            0.0
        };
        obs.zones = zone.count_ones();

        // Continuation: picture in the same columns inside, outside and
        // further outside the edge is scenery crossing it, not a bar. The
        // probes skip a step either side of the edge, because the row just
        // outside carries compression ringing that would match by chance
        // across thousands of columns; two adjacent columns are required.
        let step = (h / 540).max(2);
        if edge > 2 * step {
            s.lit_pixels((edge + step).min(last), 0);
            s.lit_pixels(edge - 1 - step, 1);
            s.lit_pixels(edge - 1 - 2 * step, 2);
            let [inside, outside, farther] = &s.b.probes;
            let mut prev = false;
            for x in 0..w {
                let conn = outside[x] && farther[x] && inside[x];
                if conn && prev {
                    obs.continues = true;
                    break;
                }
                prev = conn;
            }
        }
        obs
    }
}

#[cfg(test)]
mod tests {
    //! The C++ version's `bars_test`, one synthetic frame per situation the
    //! detector must tell apart; bars are built as real ones measured, flat 64.
    use super::*;
    use crate::synth::{Frame, Palette, Row, hash};

    const W: u32 = 3840;
    const H: u32 = 2160;

    fn observe(f: &mut Frame<'_>) -> Observation {
        Bars::default().observe(&f.pack())
    }

    /// The run rule measured run by run is the rule as first written: a
    /// pair is kept when some window of `run` lit pairs covers it.
    #[test]
    fn runs_are_kept_as_the_window_rule_keeps_them() {
        let window = |flags: &[u8], run: usize| -> Vec<u8> {
            let n = flags.len();
            let start: Vec<bool> = (0..n)
                .map(|p| p + run <= n && flags[p..p + run].iter().all(|&f| f != 0))
                .collect();
            (0..n)
                .map(|p| flags[p] & u8::from((0..run).any(|k| p >= k && start[p - k])))
                .collect()
        };
        let mut x = 7u64;
        for n in [0usize, 1, 2, 3, 5, 64, 1920] {
            for density in [1u64, 3, 5, 7, 9] {
                let flags: Vec<u8> = (0..n)
                    .map(|_| {
                        x = x
                            .wrapping_mul(6_364_136_223_846_793_005)
                            .wrapping_add(1_442_695_040_888_963_407);
                        u8::from((x >> 33) % 10 < density)
                    })
                    .collect();
                for run in [2usize, 3, 4, 7] {
                    let mut got = flags.clone();
                    in_runs(&mut got, run);
                    assert_eq!(
                        got,
                        window(&flags, run),
                        "n {n} density {density} run {run}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_2_40_letterbox_reads_letterbox_at_exact_rows() {
        // Interstellar's measured 2.40: bars 280 / 279.
        let pal = Palette::new(W);
        let mut f = Frame::new(&pal, H);
        f.content(280, 279, &pal.picture, 0);
        let o = observe(&mut f);
        assert_eq!(o.class, FrameClass::Letterbox, "{o:?}");
        assert_eq!((o.top.bar, o.bottom.bar), (280, 279));
        assert!(
            o.strength > 0.5,
            "clean bright edge is strong: {}",
            o.strength
        );
    }

    #[test]
    fn dithered_bars_are_still_bars() {
        let pal = Palette::new(W);
        let mut f = Frame::new(&pal, H);
        f.fill(0, 280, &pal.bar_noisy);
        f.fill(H - 279, H, &pal.bar_noisy);
        f.content(280, 279, &pal.picture, 0);
        assert_eq!(observe(&mut f).class, FrameClass::Letterbox);
    }

    #[test]
    fn full_picture_and_black_frames() {
        let pal = Palette::new(W);
        let mut f = Frame::new(&pal, H);
        f.content(0, 0, &pal.picture, 0);
        assert_eq!(observe(&mut f).class, FrameClass::Full);
        let mut f = Frame::new(&pal, H);
        assert_eq!(observe(&mut f).class, FrameClass::Empty);
    }

    #[test]
    fn dark_sky_on_16_9_is_weak_not_a_letterbox() {
        let pal = Palette::new(W);
        let mut f = Frame::new(&pal, H);
        f.content(300, 0, &pal.picture, 0);
        let o = observe(&mut f);
        assert_eq!(
            (o.class, o.why),
            (FrameClass::Weak, "full height on one side")
        );
    }

    #[test]
    fn a_subtitle_in_the_bar_is_skipped_as_an_island() {
        let pal = Palette::new(W);
        let mut f = Frame::new(&pal, H);
        f.content(280, 279, &pal.picture, 0);
        f.fill(H - 150, H - 110, &pal.text[3]);
        let o = observe(&mut f);
        assert_eq!(o.class, FrameClass::Letterbox);
        assert_eq!((o.bottom.islands, o.bottom.ui), (1, false));
    }

    #[test]
    fn a_pop_up_panel_in_the_bar_is_reported_as_ui_at_its_row() {
        let pal = Palette::new(W);
        let panel = Row::luma(W, |x| if x > 200 && x < 3640 { 300 } else { 64 });
        let mut f = Frame::new(&pal, H);
        f.content(280, 279, &pal.picture, 0);
        f.fill(H - 200, H - 120, &panel);
        let o = observe(&mut f);
        assert!(o.bottom.ui && o.bottom.ui_start == 120, "{:?}", o.bottom);
    }

    #[test]
    fn a_starfield_with_a_planet_is_never_a_letterbox() {
        let pal = Palette::new(W);
        let stars: Vec<Row> = (0..16)
            .map(|v| {
                Row::luma(W, |x| {
                    if hash(x, v, 9).is_multiple_of(1500) {
                        800
                    } else {
                        64
                    }
                })
            })
            .collect();
        let planet = Row::luma(W, |x| if x > 1500 && x < 2300 { 500 } else { 64 });
        let mut f = Frame::new(&pal, H);
        for y in 0..H {
            f.set(y, &stars[(hash(y, 0, 0) % 16) as usize]);
        }
        f.fill(900, 1260, &planet);
        assert_ne!(observe(&mut f).class, FrameClass::Letterbox);
    }

    #[test]
    fn a_title_card_on_black_is_empty() {
        let pal = Palette::new(W);
        let mut f = Frame::new(&pal, H);
        f.fill(1000, 1040, &pal.text[1]);
        f.fill(1100, 1140, &pal.text[2]);
        assert_eq!(observe(&mut f).class, FrameClass::Empty);
    }

    #[test]
    fn rolling_credits_are_never_a_letterbox() {
        let pal = Palette::new(W);
        let mut f = Frame::new(&pal, H);
        for y in 300..H - 300 {
            if (y - 300) % 60 < 20 || (H - 301 - y) % 60 < 20 {
                f.set(y, &pal.text[(y % 16) as usize]);
            }
        }
        assert_ne!(observe(&mut f).class, FrameClass::Letterbox);
    }

    #[test]
    fn picture_continuing_across_the_edge_is_weak() {
        // A 2-pixel bright stripe crossing the edge: too thin to light a bar
        // row, but it continues across.
        let pal = Palette::new(W);
        let stripe = Row::luma(W, |x| if x == 2000 || x == 2001 { 900 } else { 64 });
        let mut f = Frame::new(&pal, H);
        f.content(280, 279, &pal.picture, 0);
        for y in 200..280 {
            f.set(y, &stripe);
        }
        let o = observe(&mut f);
        assert_eq!(
            (o.top.bar, o.top.continues, o.class),
            (280, true, FrameClass::Weak)
        );
    }

    #[test]
    fn chroma_ringing_in_isolated_pairs_does_not_move_the_edge() {
        // Four black rows with chroma 25 off neutral at isolated pairs, about
        // 1 in 50: the old rule read them as picture and moved the edge up.
        let pal = Palette::new(W);
        let luma = vec![64u16; W as usize];
        let mut chroma = vec![512u16; W as usize];
        for x in (0..W as usize - 1).step_by(100) {
            chroma[x] = 537;
            chroma[x + 1] = 487;
        }
        let ring = Row::new(W, &luma, &chroma);
        let mut f = Frame::new(&pal, H);
        f.content(280, 279, &pal.picture, 0);
        for y in 276..280 {
            f.set(y, &ring);
        }
        let o = observe(&mut f);
        assert_eq!((o.class, o.top.bar), (FrameClass::Letterbox, 280));
        let old = Bars::new(Params {
            chroma_run: 1,
            ..Params::default()
        })
        .observe(&f.pack());
        assert_eq!(old.top.bar, 276, "the old rule read it as picture");
    }

    #[test]
    fn dark_teal_at_the_top_is_picture() {
        let pal = Palette::new(W);
        let mut f = Frame::new(&pal, H);
        f.fill(0, 280, &pal.teal);
        f.content(280, 279, &pal.picture, 0);
        assert_eq!(observe(&mut f).top.bar, 0);
    }

    #[test]
    fn a_soft_ramp_is_a_letterbox_with_low_sharpness() {
        let pal = Palette::new(W);
        let ramp: Vec<Row> = (0..40)
            .map(|k| Row::luma(W, |x| 66 + k * 3 + hash(x, k, 0) % 4))
            .collect();
        let mut f = Frame::new(&pal, H);
        f.content(280, 279, &pal.picture, 0);
        for k in 0..40 {
            f.set(280 + k, &ramp[k as usize]);
            f.set(H - 280 - k, &ramp[k as usize]);
        }
        let o = observe(&mut f);
        assert_eq!(o.class, FrameClass::Letterbox, "{o:?}");
        assert!(o.top.sharpness < 0.5, "{}", o.top.sharpness);
    }

    #[test]
    fn every_parameter_is_settable_by_name() {
        let mut p = Params::default();
        for k in Params::KEYS {
            p.set(k, "3").unwrap();
        }
        assert_eq!((p.delta, p.band, p.edge_lines), (3, 3.0, 3));
        assert!(p.set("nonsense", "1").is_err() && p.set("delta", "x").is_err());
    }

    #[test]
    fn the_old_level_rule_is_a_setting() {
        // The outermost row alone, no ceiling: a rating card's line at 95
        // sets the level, as it did before #224.
        let pal = Palette::new(W);
        let line = Row::luma(W, |_| 95);
        let mut f = Frame::new(&pal, H);
        f.content(280, 279, &pal.picture, 0);
        f.set(0, &line);
        let mut p = Params::default();
        for (k, v) in [
            ("level_rows", "1"),
            ("edge_lines", "0"),
            ("level_max", "1023"),
        ] {
            p.set(k, v).unwrap();
        }
        let o = Bars::new(p).observe(&f.pack());
        assert_eq!((o.top.level, o.top.bar), (95, 280));
        assert_eq!(o.top.level_confidence, Some(1.0));
    }

    #[test]
    fn a_line_at_the_edge_neither_sets_the_level_nor_reads_as_ui() {
        // Blade Runner 2049's rating card: one flat row at 95 on each side,
        // bar at 63-66 behind it. Dim picture inside the edge (100, under the
        // lit threshold the line's level would give) still ends the bar.
        let pal = Palette::new(W);
        let line = Row::luma(W, |_| 95);
        let dim = Row::luma(W, |x| 100 + hash(x, 9, 0) % 4);
        let mut f = Frame::new(&pal, H);
        f.fill(0, 280, &pal.bar_noisy);
        f.fill(H - 279, H, &pal.bar_noisy);
        f.content(280, 279, &pal.picture, 0);
        f.fill(280, 300, &dim);
        f.fill(H - 299, H - 279, &dim);
        f.set(0, &line);
        f.set(H - 1, &line);
        let o = observe(&mut f);
        assert_eq!(o.class, FrameClass::Letterbox, "{o:?}");
        assert_eq!((o.top.bar, o.bottom.bar), (280, 279));
        assert!(!o.top.ui && !o.bottom.ui, "{o:?}");
        assert!(o.top.level.abs_diff(64) <= 1, "{}", o.top.level);
        assert_eq!(o.top.level_confidence, Some(0.875));
    }

    #[test]
    fn a_menu_panel_over_the_bar_is_content_not_bar() {
        // Blade Runner 2049's pop-up menu: a flat grey panel at 300 over the
        // whole bottom bar. The level stays black, so the panel is lit from
        // the frame edge and nothing there is cropped.
        let pal = Palette::new(W);
        let panel = Row::luma(W, |_| 300);
        let mut f = Frame::new(&pal, H);
        f.content(280, 279, &pal.picture, 0);
        f.fill(H - 279, H, &panel);
        let o = observe(&mut f);
        assert_eq!(o.bottom.level, 64);
        assert_eq!(o.bottom.level_confidence, Some(0.0));
        assert_eq!(o.bottom.bar, 0, "{:?}", o.bottom);
        assert_eq!((o.top.bar, o.top.level_confidence), (280, Some(1.0)));
    }

    #[test]
    fn raised_bars_that_agree_set_the_level() {
        let pal = Palette::new(W);
        let raised = Row::luma(W, |x| 69 + hash(x, 11, 0) % 3);
        let dim = Row::luma(W, |x| 82 + hash(x, 12, 0) % 4);
        let mut f = Frame::new(&pal, H);
        f.fill(0, 280, &raised);
        f.fill(H - 279, H, &raised);
        f.content(280, 279, &pal.picture, 0);
        f.fill(280, 300, &dim);
        let o = observe(&mut f);
        assert_eq!((o.top.level, o.top.level_confidence), (70, Some(1.0)));
        // Picture at 82-85 is lit over black but not over the raised level
        // (86): the edge is where the bright picture starts.
        assert_eq!((o.top.bar, o.bottom.bar), (300, 279));
    }

    #[test]
    fn grey_bars_set_the_level() {
        // Top Gun: Maverick's featurette: bars flat at 76, a black line at 64
        // between them and each film clip.
        let pal = Palette::new(W);
        let grey = Row::luma(W, |_| 76);
        let mut f = Frame::new(&pal, H);
        f.fill(0, 273, &grey);
        f.fill(H - 273, H, &grey);
        f.content(275, 275, &pal.picture, 0);
        let o = observe(&mut f);
        assert_eq!((o.top.level, o.top.level_confidence), (76, Some(1.0)));
        assert_eq!((o.top.bar, o.bottom.bar), (275, 275), "{o:?}");
    }

    #[test]
    fn a_black_line_inside_the_edge_keeps_it_sharp() {
        // Top Gun: Maverick's featurette: grey bars at 76, a row of ringing
        // the scan ends the bar on, two rows of black, then the picture.
        let pal = Palette::new(W);
        let grey = Row::luma(W, |_| 76);
        let ring = Row::luma(W, |x| if x % 16 == 0 { 100 } else { 84 });
        let mut f = Frame::new(&pal, H);
        f.content(275, 275, &pal.picture, 0);
        for (y0, step) in [(0u32, 1i64), (H - 1, -1)] {
            let at = |k: i64| (i64::from(y0) + step * k) as u32;
            f.fill(at(0).min(at(271)), at(0).max(at(271)) + 1, &grey);
            f.set(at(272), &ring);
            f.set(at(273), &pal.bar);
            f.set(at(274), &pal.bar);
        }
        let o = observe(&mut f);
        assert_eq!((o.top.bar, o.bottom.bar), (272, 272), "{o:?}");
        assert!(o.top.sharpness > 0.5, "{}", o.top.sharpness);
        assert_eq!(o.class, FrameClass::Letterbox, "{o:?}");
        assert!(o.strength > 0.1, "{}", o.strength);
    }

    #[test]
    fn a_bright_flat_edge_is_picture_however_flat() {
        // The cornfield's sky: flat and neutral at 580 across the top of a
        // full-height shot. It is not a bar level, so the sky is lit picture.
        let pal = Palette::new(W);
        let sky = Row::luma(W, |x| 580 + hash(x, 13, 0) % 3);
        let mut f = Frame::new(&pal, H);
        f.content(0, 0, &pal.picture, 0);
        f.fill(0, 300, &sky);
        let o = observe(&mut f);
        assert_eq!((o.top.level, o.top.level_confidence), (64, Some(0.0)));
        assert_eq!(o.top.bar, 0);
    }

    #[test]
    fn a_1080p_2_40_letterbox() {
        let pal = Palette::new(1920);
        let mut f = Frame::new(&pal, 1080);
        f.content(140, 139, &pal.picture, 0);
        let o = observe(&mut f);
        assert_eq!(
            (o.class, o.top.bar, o.bottom.bar),
            (FrameClass::Letterbox, 140, 139)
        );
    }

    #[test]
    fn cost_of_a_scope_frame() {
        // What the render thread pays per frame; printed, not asserted (run
        // with --release --nocapture to read it).
        let pal = Palette::new(W);
        let mut f = Frame::new(&pal, H);
        f.content(280, 279, &pal.picture, 0);
        let v = f.pack();
        let mut bars = Bars::default();
        let t = std::time::Instant::now();
        for _ in 0..20 {
            bars.observe(&v);
        }
        eprintln!(
            "cost: {:.2} ms per 2160p scope frame (CPU, one thread)",
            t.elapsed().as_secs_f64() * 1000.0 / 20.0
        );
    }
}
