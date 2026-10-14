//! What the overlay shows during `play`: the waiting slate (#76) while there
//! is no picture, and a banner (#77) when the picture type changes. They
//! never share the screen: a slate means there is no picture to label.
//!
//! `play` says what is happening ([`Ui::set_slate`], [`Ui::picture`],
//! [`Ui::resynced`]) and calls [`Ui::update`] before every flip and modeset,
//! so a change rides the commit that shows the frame it belongs with: the
//! picture's first frame takes the slate down. Drawing happens only when
//! the content changes (minutes apart on a slate, once a banner); a fade is
//! the plane's own opacity, a property in the commit.

use std::time::{Duration, Instant};

use cinefit_kms::Output;
use log::{info, warn};

use crate::osd::{self, Area, BannerText, Canvas, Corner, Encoding, Fonts, SlateText, Tone};

/// How long the slate stays in one corner, so nothing sits in one place on
/// the screen for hours.
const CORNER_EVERY: Duration = Duration::from_secs(5 * 60);
/// A banner's fade in, hold and fade out.
const FADE_IN: Duration = Duration::from_millis(300);
const HOLD: Duration = Duration::from_secs(4);
const FADE_OUT: Duration = Duration::from_millis(600);
/// After a resync, a banner waits this long, to land on a picture rather
/// than on the display finding the signal (2-6 s on the JVC).
const AFTER_RESYNC: Duration = Duration::from_secs(6);
/// A banner waiting for a resync that never comes (refused, or the source
/// moved on) shows anyway after this.
const RESYNC_WAIT: Duration = Duration::from_secs(10);

/// Why there is no picture.
#[derive(Debug, Clone, PartialEq)]
pub enum Slate {
    /// No source since the start.
    NoSignal,
    /// The source went away mid-run.
    SignalLost { since: Instant },
    /// A signal held back until it proves what it is (a possible tunnel).
    Identifying { detail: String },
    /// A format nothing renders, and what to change.
    CantShow { detail: String },
}

impl Slate {
    fn text(&self, venue: &str, now: Instant) -> SlateText {
        let (state, detail, tone) = match self {
            Slate::NoSignal => ("No signal", String::new(), Tone::Plain),
            Slate::SignalLost { since } => (
                "Signal lost",
                format!("{} \u{b7} waiting for the source", ago(now - *since)),
                Tone::Attention,
            ),
            Slate::Identifying { detail } => (
                "Identifying the signal\u{2026}",
                detail.clone(),
                Tone::Plain,
            ),
            Slate::CantShow { detail } => {
                ("Can\u{2019}t show this format", detail.clone(), Tone::Error)
            }
        };
        SlateText {
            venue: venue.to_string(),
            state: state.to_string(),
            detail,
            tone,
        }
    }
}

/// How long ago, to the minute. Not a clock time: the machine's clock is
/// not necessarily in the room's time zone.
fn ago(d: Duration) -> String {
    let m = d.as_secs() / 60;
    match m {
        0 => "just now".into(),
        1..60 => format!("{m} min ago"),
        _ => format!("{} h {} min ago", m / 60, m % 60),
    }
}

/// A banner waiting to be shown.
enum Waiting {
    /// From this moment.
    At(Instant),
    /// For the resync this change needs, since this moment.
    Resync(Instant),
}

/// What the overlay holds, to tell when it must be redrawn.
#[derive(Debug, Clone, PartialEq)]
enum Content {
    Slate(SlateText, Corner),
    Banner(BannerText),
}

pub struct Ui {
    fonts: Fonts,
    venue: String,
    banners: bool,
    raster: (usize, usize),
    /// The screen's band of the raster: top row and height.
    band: (usize, usize),
    /// Two overlay buffers, drawn alternately so a redraw never shows half
    /// done; `None` without an overlay plane, when nothing is shown.
    buffers: Option<[usize; 2]>,
    /// Where each buffer has been drawn, to clear before drawing again.
    drawn: [Option<Area>; 2],
    /// The buffer on screen, what it holds, and in which encoding.
    shown: Option<(usize, Content, Encoding)>,
    fades: bool,
    started: Instant,
    slate: Option<Slate>,
    /// The banner text last shown or queued: a banner is for a change.
    last_banner: Option<BannerText>,
    waiting: Option<(BannerText, Waiting)>,
    showing: Option<(BannerText, Instant)>,
    /// The last drawing logged: a slate redrawn each minute for its time
    /// logs once.
    logged: String,
}

impl Ui {
    /// Makes the overlay buffers for mode `index`. Without an overlay plane,
    /// or if the buffers cannot be made, the run goes on without one.
    pub fn new(
        out: &mut Output,
        index: usize,
        raster: (u32, u32),
        band: (u32, u32),
        venue: &str,
        banners: bool,
    ) -> Self {
        let buffers = if out.has_overlay() {
            match (out.create_overlay(index), out.create_overlay(index)) {
                (Ok(a), Ok(b)) => Some([a, b]),
                (Err(e), _) | (_, Err(e)) => {
                    warn!("osd: no overlay buffers, so no slate or banners: {e}");
                    None
                }
            }
        } else {
            info!("osd: the output has no overlay plane, so no slate or banners");
            None
        };
        let fades = out.has_overlay_alpha();
        if buffers.is_some() && !fades {
            info!("osd: the overlay plane has no opacity property: banners appear without fades");
        }
        Self {
            fonts: Fonts::load(),
            venue: venue.to_string(),
            banners,
            raster: (raster.0 as usize, raster.1 as usize),
            band: (band.0 as usize, band.1 as usize),
            buffers,
            drawn: [None; 2],
            shown: None,
            fades,
            started: Instant::now(),
            slate: None,
            last_banner: None,
            waiting: None,
            showing: None,
            logged: String::new(),
        }
    }

    /// The slate, or `None` once there is a picture. A slate takes any
    /// banner down with it.
    pub fn set_slate(&mut self, slate: Option<Slate>) {
        if slate.is_some() {
            self.waiting = None;
            self.showing = None;
        }
        // Signal lost keeps its time while it lasts.
        if let (Some(Slate::SignalLost { .. }), Some(Slate::SignalLost { .. })) =
            (&self.slate, &slate)
        {
            return;
        }
        self.slate = slate;
    }

    /// The picture as it is coming in, on frames that are shown: a banner
    /// when it differs from the last one. `resync` says the output is to
    /// change mode for it, so the banner waits for that and then some.
    pub fn picture(&mut self, text: BannerText, resync: bool, now: Instant) {
        if !self.banners || self.last_banner.as_ref() == Some(&text) {
            return;
        }
        self.last_banner = Some(text.clone());
        self.showing = None;
        let when = if resync {
            Waiting::Resync(now)
        } else {
            Waiting::At(now)
        };
        self.waiting = Some((text, when));
    }

    /// The output has just changed mode: a banner waiting for it shows
    /// [`AFTER_RESYNC`] from now.
    pub fn resynced(&mut self, now: Instant) {
        if let Some((_, when)) = self.waiting.as_mut() {
            *when = Waiting::At(now + AFTER_RESYNC);
        }
    }

    /// Puts what should be on the overlay now into the next commit. Call
    /// before every flip and modeset, with the encoding that commit's output
    /// will have.
    pub fn update(&mut self, out: &mut Output, enc: Encoding, now: Instant) {
        let Some(buffers) = self.buffers else {
            return;
        };
        let (content, alpha) = self.want(now);
        let Some(content) = content else {
            if self.shown.take().is_some() {
                out.set_overlay(None);
            }
            return;
        };
        if self
            .shown
            .as_ref()
            .is_none_or(|(_, c, e)| *c != content || *e != enc)
        {
            // The buffer not on screen: the last commit has completed, so
            // nothing scans it out.
            let i = match &self.shown {
                Some((b, ..)) if *b == buffers[0] => 1,
                _ => 0,
            };
            if let Err(e) = self.draw(out, buffers[i], i, &content, enc) {
                warn!("osd: drawing failed, so no slate or banners from now: {e}");
                self.buffers = None;
                out.set_overlay(None);
                return;
            }
            out.set_overlay(Some(buffers[i]));
            self.shown = Some((buffers[i], content, enc));
        }
        out.set_overlay_alpha((alpha * 65535.0).round() as u16);
    }

    /// What should show now, and at what opacity.
    fn want(&mut self, now: Instant) -> (Option<Content>, f32) {
        if let Some(slate) = &self.slate {
            let turns = (now - self.started).as_secs() / CORNER_EVERY.as_secs();
            let corner = [
                Corner::BottomLeft,
                Corner::BottomRight,
                Corner::TopRight,
                Corner::TopLeft,
            ][(turns % 4) as usize];
            return (
                Some(Content::Slate(slate.text(&self.venue, now), corner)),
                1.0,
            );
        }
        let due = match &self.waiting {
            Some((_, Waiting::At(at))) => now >= *at,
            Some((_, Waiting::Resync(since))) => now - *since >= RESYNC_WAIT,
            None => false,
        };
        if due && let Some((text, _)) = self.waiting.take() {
            self.showing = Some((text, now));
        }
        let Some((text, from)) = &self.showing else {
            return (None, 0.0);
        };
        let t = now - *from;
        let alpha = if !self.fades {
            1.0
        } else if t < FADE_IN {
            t.as_secs_f32() / FADE_IN.as_secs_f32()
        } else if t < FADE_IN + HOLD {
            1.0
        } else {
            1.0 - (t - FADE_IN - HOLD).as_secs_f32() / FADE_OUT.as_secs_f32()
        };
        if t >= FADE_IN + HOLD + FADE_OUT {
            self.showing = None;
            return (None, 0.0);
        }
        (Some(Content::Banner(text.clone())), alpha.clamp(0.0, 1.0))
    }

    /// Draws `content` into overlay buffer `buffer` (slot `slot`): off the
    /// overlay's memory, which is slow to read, then copied in, clearing
    /// what was drawn there before.
    fn draw(
        &mut self,
        out: &mut Output,
        buffer: usize,
        slot: usize,
        content: &Content,
        enc: Encoding,
    ) -> Result<(), cinefit_kms::Error> {
        let started = Instant::now();
        let area = match content {
            Content::Slate(_, corner) => osd::slate_area(self.raster, self.band, *corner),
            Content::Banner(text) => osd::banner_area(self.raster, self.band.0, &self.fonts, text),
        };
        let mut px = vec![0u8; area.w * 4 * area.h];
        let mut c = Canvas::window(&mut px, area.w * 4, area, self.raster, enc);
        match content {
            Content::Slate(text, corner) => osd::slate(&mut c, &self.fonts, area, *corner, text),
            Content::Banner(text) => osd::banner(&mut c, &self.fonts, area, text),
        }
        let old = self.drawn[slot].replace(area);
        out.draw_overlay(buffer, |dst, pitch| {
            if let Some(o) = old {
                for row in dst.chunks_mut(pitch).skip(o.y).take(o.h) {
                    row[o.x * 4..(o.x + o.w) * 4].fill(0);
                }
            }
            for (row, src) in dst
                .chunks_mut(pitch)
                .skip(area.y)
                .take(area.h)
                .zip(px.chunks(area.w * 4))
            {
                row[area.x * 4..(area.x + area.w) * 4].copy_from_slice(src);
            }
        })?;
        let what = match content {
            Content::Slate(t, _) => format!("slate \"{}\"", t.state),
            Content::Banner(t) => format!("banner \"{} / {}\"", t.title, t.detail),
        };
        if what != self.logged {
            info!(
                "osd: {what} drawn in {:.1} ms",
                started.elapsed().as_secs_f64() * 1000.0
            );
            self.logged = what;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A UI with no output, to test what it wants shown when.
    fn bare(banners: bool) -> Ui {
        Ui {
            fonts: Fonts::load(),
            venue: "The Screening Room".into(),
            banners,
            raster: (4096, 2160),
            band: (209, 1742),
            buffers: None,
            drawn: [None; 2],
            shown: None,
            fades: true,
            started: Instant::now(),
            slate: None,
            last_banner: None,
            waiting: None,
            showing: None,
            logged: String::new(),
        }
    }

    fn text(title: &str) -> BannerText {
        BannerText {
            title: title.into(),
            detail: "4K 23.976".into(),
        }
    }

    fn is_banner(c: &(Option<Content>, f32), title: &str) -> bool {
        matches!(&c.0, Some(Content::Banner(t)) if t.title == title)
    }

    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn a_banner_without_a_resync_shows_at_once_fades_and_goes() {
        let mut ui = bare(true);
        let t0 = Instant::now();
        ui.picture(text("HDR10"), false, t0);
        let first = ui.want(t0);
        assert!(is_banner(&first, "HDR10") && first.1 == 0.0, "{first:?}");
        assert!((ui.want(t0 + 150 * MS).1 - 0.5).abs() < 0.01);
        assert_eq!(ui.want(t0 + Duration::from_secs(2)).1, 1.0);
        let fading = ui.want(t0 + FADE_IN + HOLD + 300 * MS);
        assert!(is_banner(&fading, "HDR10") && (fading.1 - 0.5).abs() < 0.01);
        assert!(ui.want(t0 + FADE_IN + HOLD + FADE_OUT).0.is_none());
        // The same picture again is no change: no second banner.
        ui.picture(text("HDR10"), false, t0 + Duration::from_secs(10));
        assert!(ui.want(t0 + Duration::from_secs(10)).0.is_none());
    }

    #[test]
    fn a_banner_waits_for_its_resync_and_then_some() {
        let mut ui = bare(true);
        let t0 = Instant::now();
        ui.picture(text("SDR"), true, t0);
        assert!(ui.want(t0 + Duration::from_secs(1)).0.is_none());
        let at = t0 + Duration::from_secs(2);
        ui.resynced(at);
        assert!(ui.want(at + AFTER_RESYNC - MS).0.is_none());
        assert!(is_banner(&ui.want(at + AFTER_RESYNC), "SDR"));
        // A resync that never comes: shown anyway.
        let mut ui = bare(true);
        ui.picture(text("SDR"), true, t0);
        assert!(ui.want(t0 + RESYNC_WAIT - MS).0.is_none());
        assert!(is_banner(&ui.want(t0 + RESYNC_WAIT), "SDR"));
    }

    #[test]
    fn the_slate_takes_the_screen_and_any_banner_with_it() {
        let mut ui = bare(true);
        let t0 = Instant::now();
        ui.picture(text("Dolby Vision"), false, t0);
        assert!(is_banner(&ui.want(t0 + MS), "Dolby Vision"));
        ui.set_slate(Some(Slate::SignalLost { since: t0 }));
        let Some(Content::Slate(s, Corner::BottomLeft)) = ui.want(t0 + MS).0 else {
            panic!("no slate");
        };
        assert_eq!(
            (s.state.as_str(), s.detail.as_str(), s.tone),
            (
                "Signal lost",
                "just now \u{b7} waiting for the source",
                Tone::Attention
            )
        );
        // Still lost later: the time it was lost is kept.
        ui.set_slate(Some(Slate::SignalLost {
            since: t0 + Duration::from_secs(300),
        }));
        let later = t0 + Duration::from_secs(5 * 60 + 1);
        let Some(Content::Slate(s, corner)) = ui.want(later).0 else {
            panic!("no slate");
        };
        assert_eq!(s.detail, "5 min ago \u{b7} waiting for the source");
        // And it has moved to the next corner.
        assert_eq!(corner, Corner::BottomRight);
        // The picture back, of the same type: no banner, nothing shown.
        ui.set_slate(None);
        ui.picture(text("Dolby Vision"), false, later);
        assert!(ui.want(later).0.is_none());
    }

    #[test]
    fn banners_off_shows_none() {
        let mut ui = bare(false);
        let t0 = Instant::now();
        ui.picture(text("HDR10"), false, t0);
        assert!(ui.want(t0 + Duration::from_secs(1)).0.is_none());
    }

    #[test]
    fn minutes_ago_reads_plainly() {
        assert_eq!(ago(Duration::from_secs(20)), "just now");
        assert_eq!(ago(Duration::from_secs(60 * 12 + 5)), "12 min ago");
        assert_eq!(ago(Duration::from_secs(3600 * 3 + 60 * 4)), "3 h 4 min ago");
    }
}
