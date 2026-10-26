//! Constant-image-height geometry. Pure arithmetic, no graphics types, so it
//! is tested anywhere. Ported from the C++ version's `geometry.cpp`, with its
//! tests, which were worked out by hand in its `docs/plan.md` before any code
//! existed: an oracle, not a restatement.
//!
//! The raster is 1.896:1 and the screen 2.35:1, so one fixed lens zoom cannot
//! match both. Zooming so the raster's height matches the screen would cap
//! the displayable aspect at 1.896, a dead end; so the lens is set so the
//! raster's width matches the screen's, and the screen is a centred
//! horizontal band of the raster. Rows above and below it land on velvet.
//! Screen size cancels out; only aspect matters.

/// A rectangle in whole pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    /// Width over height: the aspect to place a crop at. Never a rounded
    /// constant: 16:9 and 1.78 already land 4 pixels apart on screen.
    pub fn aspect(&self) -> f64 {
        if self.h == 0 {
            0.0
        } else {
            f64::from(self.w) / f64::from(self.h)
        }
    }
}

/// The band of the raster that lands on the screen, derived from the raster
/// and the screen's aspect. In practice it is calibrated against the screen
/// itself: where the lens and the screen actually landed is not arithmetic.
///
/// For 4096 at 2.35 the band is 1743.0 rows; rounding down to an even 1742
/// keeps it integer-symmetric, and the 0.06% aspect error is about 0.3 mm
/// across a 130" screen.
pub fn screen_window(raster_w: u32, raster_h: u32, screen_ar: f64) -> Rect {
    if screen_ar <= 0.0 || raster_h == 0 {
        return Rect {
            x: 0,
            y: 0,
            w: raster_w,
            h: raster_h,
        };
    }
    let h = ((f64::from(raster_w) / screen_ar) as u32).min(raster_h) & !1;
    Rect {
        x: 0,
        y: (raster_h - h) / 2,
        w: raster_w,
        h,
    }
}

/// Where content of the given aspect lands inside the window. Up to the
/// window's own aspect it fills the window's height and is pillarboxed; wider,
/// it fills the width and is letterboxed inside the window by a few rows.
/// That asymmetry is the whole point: every ratio up to the screen's own
/// shares the same top and bottom edges, so the image height never changes.
pub fn dest_rect(win: Rect, content_ar: f64) -> Rect {
    if content_ar <= 0.0 || win.h == 0 {
        return win;
    }
    let (w, h) = if content_ar <= win.aspect() {
        (
            ((f64::from(win.h) * content_ar + 0.5) as u32).min(win.w),
            win.h,
        )
    } else {
        (
            win.w,
            ((f64::from(win.w) / content_ar + 0.5) as u32).min(win.h),
        )
    };
    Rect {
        x: win.x + (win.w - w) / 2,
        y: win.y + (win.h - h) / 2,
        w,
        h,
    }
}

/// The active image inside a captured frame of the given aspect, assuming
/// symmetric bars. Cropping the bars is the other half of CIH: without it the
/// black is scaled up with the picture.
pub fn source_crop(frame_w: u32, frame_h: u32, content_ar: f64) -> Rect {
    let full = Rect {
        x: 0,
        y: 0,
        w: frame_w,
        h: frame_h,
    };
    if content_ar <= 0.0 || frame_w == 0 || frame_h == 0 {
        return full;
    }
    let (w, h) = if content_ar >= full.aspect() {
        (
            frame_w,
            ((f64::from(frame_w) / content_ar + 0.5) as u32).min(frame_h),
        )
    } else {
        (
            ((f64::from(frame_h) * content_ar + 0.5) as u32).min(frame_w),
            frame_h,
        )
    };
    Rect {
        x: (frame_w - w) / 2,
        y: (frame_h - h) / 2,
        w,
        h,
    }
}

/// The active image from measured bar rows, what the detector reports: the
/// rows as they are, no centring and no rounding (a centred crop of a
/// 1601-row picture would start at row 279.5, and a float crop taken
/// literally blends half a black row into the edge). Rows that leave no
/// picture give the full frame.
pub fn source_rows(frame_w: u32, frame_h: u32, top: u32, bottom: u32) -> Rect {
    if u64::from(top) + u64::from(bottom) >= u64::from(frame_h) {
        return Rect {
            x: 0,
            y: 0,
            w: frame_w,
            h: frame_h,
        };
    }
    Rect {
        x: 0,
        y: top,
        w: frame_w,
        h: frame_h - top - bottom,
    }
}

/// Content a little wider than the screen (2.39 or 2.40 on a 2.35 screen)
/// fills the window's width and leaves a few rows of bar inside the screen.
/// With `fill_max_ar` set, content no wider than it fills the window's height
/// instead and loses a sliver at each side: a 2.40 film gives up 1% of its
/// width a side and shows no bars. Wider content (2.55, 2.76) keeps its full
/// width and its thin bars. Narrows `src` symmetrically in whole pixels and
/// returns whether it applied; place the result at the window itself, which
/// it then fills. 0 is off.
///
/// A measured crop's ratio wobbles by a row around the nominal one (2.40
/// measures 2.3985 or 2.4015), so "no wider than" allows 0.5%.
pub fn trim_sides(win: Rect, fill_max_ar: f64, src: &mut Rect) -> bool {
    if fill_max_ar <= 0.0 || src.h == 0 || win.h == 0 {
        return false;
    }
    let (ar, win_ar) = (src.aspect(), win.aspect());
    if ar <= win_ar || ar > fill_max_ar * 1.005 {
        return false;
    }
    // Rounded up: a hair wider than the window, so it fills it exactly
    // rather than landing a pixel short. Content less than a column wider
    // than the window (544 rows of 1280) has nothing to trim but still fills
    // it, rather than being letterboxed by a single row.
    let w = ((f64::from(src.h) * win_ar).ceil() as u32).min(src.w);
    src.x += (src.w - w) / 2;
    src.w = w;
    true
}

/// The share of the raster's rows outside the screen window: the panel's
/// light thrown onto velvet, the real argument for an anamorphic lens.
pub fn wasted_light_fraction(raster_h: u32, win: Rect) -> f64 {
    if raster_h == 0 {
        0.0
    } else {
        f64::from(raster_h - win.h) / f64::from(raster_h)
    }
}

/// How the active picture is known, for [`place_frame`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Source {
    /// Not at all: the whole frame fills the whole raster, unprocessed.
    Whole,
    /// Bar rows measured by the detector, top and bottom.
    Rows { top: u32, bottom: u32 },
    /// A stated aspect, bars assumed symmetric.
    Aspect(f64),
}

/// Where one frame is read from (`src`, in the frame) and where it lands
/// (`dst`, in the raster): all of a frame's geometry, as crop rectangles and
/// nothing else, since a geometry change that commits is a bug.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub src: Rect,
    pub dst: Rect,
}

/// Places one frame: the source from measured rows or a stated aspect,
/// trimmed at the sides when [`trim_sides`] applies and then filling the
/// window, otherwise placed in it by [`dest_rect`]. Rows, not ratios:
/// measured rows are placed at width over rows, never rounded to a named
/// ratio.
pub fn place_frame(
    frame: (u32, u32),
    out: (u32, u32),
    win: Rect,
    source: Source,
    fill_max_ar: f64,
) -> Placement {
    let whole = Placement {
        src: Rect {
            x: 0,
            y: 0,
            w: frame.0,
            h: frame.1,
        },
        dst: Rect {
            x: 0,
            y: 0,
            w: out.0,
            h: out.1,
        },
    };
    let (mut src, by_rows, content_ar) = match source {
        Source::Whole => return whole,
        Source::Aspect(ar) if ar <= 0.0 => return whole,
        Source::Rows { top, bottom } => (source_rows(frame.0, frame.1, top, bottom), true, 0.0),
        Source::Aspect(ar) => (source_crop(frame.0, frame.1, ar), false, ar),
    };
    // A trimmed source is placed at the window itself, not at its own aspect:
    // the trim was chosen to fill it, and its rounded-up column is enough at
    // 1080 or 720 rows to put the aspect past the window's and land a row
    // short. Filling it stretches by at most that one column.
    let dst = if trim_sides(win, fill_max_ar, &mut src) {
        win
    } else {
        dest_rect(win, if by_rows { src.aspect() } else { content_ar })
    };
    Placement { src, dst }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: u32, y: u32, w: u32, h: u32) -> Rect {
        Rect { x, y, w, h }
    }

    fn win() -> Rect {
        screen_window(4096, 2160, 2.35)
    }

    #[test]
    fn the_window_for_4096x2160_at_2_35() {
        assert_eq!(win(), r(0, 209, 4096, 1742));
    }

    #[test]
    fn the_plans_hand_worked_table() {
        // Width and height exact; position within a pixel, since centring an
        // odd leftover is arbitrary (the plan rounded up, the code truncates).
        for (ar, want) in [
            (16.0 / 9.0, r(500, 209, 3097, 1742)),
            (1.85, r(436, 209, 3223, 1742)),
            (2.00, r(306, 209, 3484, 1742)),
            (2.39, r(0, 223, 4096, 1714)),
        ] {
            let got = dest_rect(win(), ar);
            assert_eq!((got.w, got.h, got.y), (want.w, want.h, want.y), "{ar}");
            assert!(got.x.abs_diff(want.x) <= 1, "{ar}: {got:?}");
        }
    }

    #[test]
    fn sixteen_nine_and_1_78_differ_by_4_px() {
        // A detector reporting a rounded ratio would land content 4 px off:
        // "nearly right" geometry is harder to spot than badly wrong.
        assert_eq!(dest_rect(win(), 16.0 / 9.0).w, 3097);
        assert_eq!(dest_rect(win(), 1.78).w, 3101);
    }

    #[test]
    fn content_at_the_screens_ratio_fills_the_window() {
        let got = dest_rect(win(), 2.35);
        assert_eq!(got.h, 1742);
        assert!((4094..=4096).contains(&got.w));
    }

    #[test]
    fn every_ratio_up_to_the_screens_keeps_height_and_top_edge() {
        // The property the whole design exists for.
        let mut ar = 1.33;
        while ar <= 2.35 {
            let got = dest_rect(win(), ar);
            assert_eq!((got.h, got.y), (1742, 209), "{ar}");
            ar += 0.01;
        }
    }

    #[test]
    fn wider_content_is_inset_symmetrically_and_all_centred() {
        let w = win();
        let got = dest_rect(w, 2.39);
        assert!(got.y > w.y);
        assert_eq!(got.y - w.y, w.y + w.h - got.y - got.h);
        for ar in [1.33, 1.78, 1.85, 2.0, 2.2] {
            let got = dest_rect(w, ar);
            let (left, right) = (got.x - w.x, w.x + w.w - got.x - got.w);
            assert!(left.abs_diff(right) <= 1, "{ar}");
        }
    }

    #[test]
    fn degenerate_inputs_degrade_safely() {
        assert_eq!(dest_rect(win(), 0.0).w, 4096);
        assert_eq!(screen_window(4096, 2160, 0.0).h, 2160);
        // A screen squarer than the raster cannot exceed it.
        assert_eq!(screen_window(4096, 2160, 1.0).h, 2160);
    }

    #[test]
    fn crops_from_measured_rows() {
        let full = source_rows(3840, 2160, 0, 0);
        assert_eq!(full, r(0, 0, 3840, 2160));
        assert_eq!(
            dest_rect(win(), full.aspect()),
            dest_rect(win(), 16.0 / 9.0)
        );
        // Interstellar's 280/279: rows as measured, no centring.
        let scope = source_rows(3840, 2160, 280, 279);
        assert_eq!(scope, r(0, 280, 3840, 1601));
        assert_eq!(dest_rect(win(), scope.aspect()), r(0, 226, 4096, 1708));
        assert_eq!(source_rows(3840, 2160, 1080, 1080), full);
        assert_eq!(source_rows(3840, 2160, 2000, 500), full);
        assert_eq!(source_rows(1920, 1080, 138, 139), r(0, 138, 1920, 803));
    }

    #[test]
    fn filling_the_height_for_content_a_little_wider() {
        let w = win();
        let mut s240 = source_rows(3840, 2160, 280, 279); // Interstellar, 2.3985
        assert!(trim_sides(w, 2.40, &mut s240));
        assert_eq!((s240.y, s240.h, s240.w, s240.x), (280, 1601, 3765, 37));
        assert_eq!(dest_rect(w, s240.aspect()), r(0, 209, 4096, 1742));
        let mut s241 = source_rows(3840, 2160, 280, 281); // 1599 rows, 2.4015
        assert!(
            trim_sides(w, 2.40, &mut s241),
            "a row narrower still counts as 2.40"
        );
        let mut s255 = source_rows(3840, 2160, 327, 327); // La La Land, 2.55
        let before = s255;
        assert!(!trim_sides(w, 2.40, &mut s255) && s255 == before);
        let mut s178 = source_rows(3840, 2160, 0, 0);
        assert!(!trim_sides(w, 2.40, &mut s178));
        let mut off = source_rows(3840, 2160, 280, 279);
        assert!(!trim_sides(w, 0.0, &mut off), "fill 0: off");
    }

    #[test]
    fn trimmed_content_fills_the_window_at_every_source_size() {
        // At 1080 and 720 rows the trim's extra column is a bigger share of
        // the width than at 2160, enough to tip dest_rect into its width-bound
        // branch and land a row short (1741 of 1742); and a crop less than a
        // column wider than the window was not trimmed at all, so landed
        // there too.
        let w = win();
        for (fw, fh) in [(1920, 1080), (1280, 720), (3840, 2160)] {
            let mut filled = 0;
            for h in 1..=fh {
                let ar = f64::from(fw) / f64::from(h);
                if ar <= w.aspect() || ar > 2.40 * 1.005 {
                    continue;
                }
                let (top, bottom) = ((fh - h) / 2, fh - h - (fh - h) / 2);
                let p = place_frame(
                    (fw, fh),
                    (4096, 2160),
                    w,
                    Source::Rows { top, bottom },
                    2.40,
                );
                assert_eq!(p.dst, w, "{fw}x{fh}, {h} rows");
                assert_eq!((p.src.y, p.src.h), (top, h), "{fw}x{fh}, {h} rows");
                assert!(p.src.x + p.src.w <= fw);
                let off_centre = 2 * i64::from(p.src.x) + i64::from(p.src.w) - i64::from(fw);
                assert!(off_centre.abs() <= 1, "{fw}x{fh}, {h} rows");
                // Stretched to the window by at most the one column rounded up.
                assert!(p.src.aspect() >= w.aspect());
                assert!(f64::from(p.src.w) - f64::from(h) * w.aspect() < 1.0);
                filled += 1;
            }
            assert!(filled > 0, "{fw}x{fh}: the fill range holds crops");
        }
    }

    #[test]
    fn place_frame_from_hand_worked_numbers() {
        let w = win();
        let (raster, frame) = (r(0, 0, 4096, 2160), r(0, 0, 3840, 2160));
        let pf = |s, fill| place_frame((3840, 2160), (4096, 2160), w, s, fill);
        let p = pf(Source::Whole, 0.0);
        assert_eq!(
            (p.src, p.dst),
            (frame, raster),
            "unprocessed fills the raster 1:1"
        );
        // 1600 rows of 3840 is 2.40, wider than the 2.351 window: width-bound,
        // 4096 / 2.40 = 1706.7 -> 1707 rows at 209 + (1742 - 1707) / 2.
        let rows = Source::Rows {
            top: 280,
            bottom: 280,
        };
        let p = pf(rows, 0.0);
        assert_eq!(
            (p.src, p.dst),
            (r(0, 280, 3840, 1600), r(0, 226, 4096, 1707))
        );
        // With fill 2.40 the sides give way and the window is filled, centred
        // to a pixel (77 columns split 38/39).
        let p = pf(rows, 2.40);
        assert_eq!(p.dst, r(0, 209, 4096, 1742));
        assert_eq!((p.src.y, p.src.h), (280, 1600));
        assert!(p.src.w < 3840);
        assert!((2 * i64::from(p.src.x) + i64::from(p.src.w) - 3840).abs() <= 1);
        // A full-height 16:9 frame by rows: pillarboxed, 3097 wide.
        let p = pf(Source::Rows { top: 0, bottom: 0 }, 0.0);
        assert_eq!(p.src, frame);
        assert_eq!((p.dst.w, p.dst.h, p.dst.y), (3097, 1742, 209));
        assert!(p.dst.x.abs_diff(500) <= 1);
        // No detector, a stated 1.85: 3223 wide, the source cropped to it.
        let p = pf(Source::Aspect(1.85), 0.0);
        assert_eq!((p.dst.w, p.dst.h), (3223, 1742));
        assert!(p.dst.x.abs_diff(436) <= 1);
        assert_eq!(p.src.w, 3840);
        assert!((2075..=2076).contains(&p.src.h));
    }

    #[test]
    fn about_19_percent_of_the_rows_land_on_velvet() {
        let waste = wasted_light_fraction(2160, win());
        assert!(waste > 0.18 && waste < 0.20);
    }

    #[test]
    fn the_screens_aspect_decides_the_window() {
        // Screen size cancels: a 2.40 screen of the same width is 1706 rows.
        assert_eq!(screen_window(4096, 2160, 2.40), r(0, 227, 4096, 1706));
    }
}
