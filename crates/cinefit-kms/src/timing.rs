//! Display timing without a display: choosing a mode by its exact rate, and
//! judging flip pacing. Pure arithmetic, so it is tested anywhere.

use std::fmt;
use std::time::Duration;

/// The timing of one display mode: enough to know its exact refresh rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModeTiming {
    pub width: u16,
    pub height: u16,
    /// Pixel clock in kHz, as KMS reports it.
    pub clock_khz: u32,
    pub htotal: u16,
    pub vtotal: u16,
    pub interlaced: bool,
    /// The sink's preferred mode, per its EDID.
    pub preferred: bool,
}

impl ModeTiming {
    /// Refreshes per second, exactly: `clock * 1000 / (htotal * vtotal)`.
    /// 23.976 is 296,703 kHz over 5500 x 2250, not 24 * 1000/1001.
    pub fn refresh_hz(&self) -> f64 {
        let pixels = f64::from(self.htotal) * f64::from(self.vtotal);
        if pixels == 0.0 {
            return 0.0;
        }
        f64::from(self.clock_khz) * 1000.0 / pixels
    }

    /// The deepest colour HDMI 2.0 carries at this mode, for `max bpc`: its
    /// TMDS character rate is the pixel clock times bpc / 8, at most 600 MHz.
    /// 4096x2160 at 23.976 (297 MHz) takes 10 bits; at 59.94 (593 MHz), 10
    /// bits would need 742, so 8. Asked for more than fits, amdgpu silently
    /// falls back to 4:2:0 rather than failing (the C++ version's finding).
    pub fn hdmi_bpc(&self) -> u8 {
        if u64::from(self.clock_khz) * 10 / 8 <= 600_000 {
            10
        } else {
            8
        }
    }

    /// One refresh period.
    pub fn period(&self) -> Duration {
        let hz = self.refresh_hz();
        if hz > 0.0 {
            Duration::from_secs_f64(1.0 / hz)
        } else {
            Duration::ZERO
        }
    }
}

impl fmt::Display for ModeTiming {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}x{}{} @ {:.4} Hz ({} kHz, {}x{} total)",
            self.width,
            self.height,
            if self.interlaced { "i" } else { "p" },
            self.refresh_hz(),
            self.clock_khz,
            self.htotal,
            self.vtotal
        )
    }
}

/// What mode to ask for: a size and a refresh rate, e.g. `3840x2160@23.976`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModeRequest {
    pub width: u16,
    pub height: u16,
    pub refresh_hz: f64,
}

impl std::str::FromStr for ModeRequest {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        let bad = || format!("expected WIDTHxHEIGHT@HZ, e.g. 3840x2160@23.976, not {s:?}");
        let (size, hz) = s.split_once('@').ok_or_else(bad)?;
        let (w, h) = size.split_once('x').ok_or_else(bad)?;
        Ok(Self {
            width: w.parse().map_err(|_| bad())?,
            height: h.parse().map_err(|_| bad())?,
            refresh_hz: hz.parse().map_err(|_| bad())?,
        })
    }
}

/// The progressive mode of the requested size whose rate is nearest the
/// request, if one is within 0.01 Hz. 23.976 and 24 differ by 0.024, so the
/// tolerance tells them apart while forgiving a rounded request like 59.94.
pub fn choose_mode(modes: &[ModeTiming], want: &ModeRequest) -> Option<usize> {
    modes
        .iter()
        .enumerate()
        .filter(|(_, m)| !m.interlaced && m.width == want.width && m.height == want.height)
        .map(|(i, m)| (i, (m.refresh_hz() - want.refresh_hz).abs()))
        .filter(|&(_, err)| err < 0.01)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// Flip pacing over a run, from each completed flip's vblank sequence number
/// and timestamp.
#[derive(Debug, Default, Clone)]
pub struct Pacing {
    pub flips: u64,
    /// Flips that landed more than one vblank after the previous one: each
    /// skipped vblank showed the previous frame again.
    pub missed: u64,
    pub vblanks_skipped: u64,
    intervals: Vec<Duration>,
    last: Option<(u32, Duration)>,
}

impl Pacing {
    pub fn on_flip(&mut self, sequence: u32, at: Duration) {
        self.flips += 1;
        if let Some((seq, t)) = self.last {
            let vblanks = sequence.wrapping_sub(seq);
            if vblanks > 1 {
                self.missed += 1;
                self.vblanks_skipped += u64::from(vblanks - 1);
            }
            // Pacing is judged on flips that landed on consecutive vblanks;
            // a skipped vblank is counted above, not averaged in here.
            if vblanks == 1 {
                self.intervals.push(at.saturating_sub(t));
            }
        }
        self.last = Some((sequence, at));
    }

    pub fn summary(&self, period: Duration) -> String {
        let n = self.intervals.len();
        let mut s = format!(
            "flips: {}, missed: {} ({} vblanks skipped)\n",
            self.flips, self.missed, self.vblanks_skipped
        );
        if n == 0 {
            return s;
        }
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        let mean = self.intervals.iter().map(|d| ms(*d)).sum::<f64>() / n as f64;
        let min = self.intervals.iter().min().map_or(0.0, |d| ms(*d));
        let max = self.intervals.iter().max().map_or(0.0, |d| ms(*d));
        let target = ms(period);
        let within = self
            .intervals
            .iter()
            .filter(|d| (ms(**d) - target).abs() <= 0.5)
            .count();
        s.push_str(&format!(
            "flip interval: mean {mean:.4} ms, min {min:.4}, max {max:.4}, target {target:.4}\n\
             within 0.5 ms of target: {within} of {n} ({:.4}%)\n",
            100.0 * within as f64 / n as f64
        ));
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(width: u16, height: u16, clock_khz: u32, htotal: u16, vtotal: u16) -> ModeTiming {
        ModeTiming {
            width,
            height,
            clock_khz,
            htotal,
            vtotal,
            interlaced: false,
            preferred: false,
        }
    }

    // CTA-861 timings.
    fn uhd_23976() -> ModeTiming {
        mode(3840, 2160, 296_703, 5500, 2250)
    }
    fn uhd_24() -> ModeTiming {
        mode(3840, 2160, 297_000, 5500, 2250)
    }
    fn uhd_60() -> ModeTiming {
        mode(3840, 2160, 594_000, 4400, 2250)
    }

    #[test]
    fn exact_rates() {
        assert!((uhd_23976().refresh_hz() - 23.976).abs() < 0.0001);
        assert!((uhd_24().refresh_hz() - 24.0).abs() < 1e-9);
        // The C++ version measured 41.7084 ms flips at 23.976.
        assert!((uhd_23976().period().as_secs_f64() * 1000.0 - 41.7084).abs() < 0.0001);
    }

    #[test]
    fn mode_requests_parse() {
        let r: ModeRequest = "3840x2160@23.976".parse().unwrap();
        assert_eq!((r.width, r.height), (3840, 2160));
        assert!("3840x2160".parse::<ModeRequest>().is_err());
        assert!("ax2160@24".parse::<ModeRequest>().is_err());
    }

    #[test]
    fn rates_23_976_and_24_are_told_apart() {
        let modes = [uhd_60(), uhd_24(), uhd_23976()];
        let pick = |s: &str| choose_mode(&modes, &s.parse().unwrap());
        assert_eq!(pick("3840x2160@23.976"), Some(2));
        assert_eq!(pick("3840x2160@24"), Some(1));
        assert_eq!(pick("3840x2160@60"), Some(0));
        assert_eq!(pick("3840x2160@30"), None);
        assert_eq!(pick("1920x1080@24"), None);
    }

    #[test]
    fn interlaced_modes_are_never_chosen() {
        let mut i = mode(1920, 1080, 74_250, 2200, 1125);
        i.interlaced = true;
        assert_eq!(choose_mode(&[i], &"1920x1080@60".parse().unwrap()), None);
    }

    #[test]
    fn steady_flips_have_no_misses() {
        let period = uhd_23976().period();
        let mut p = Pacing::default();
        for k in 0..100u32 {
            p.on_flip(1000 + k, period * k);
        }
        assert_eq!((p.flips, p.missed), (100, 0));
        assert!(
            p.summary(period)
                .contains("within 0.5 ms of target: 99 of 99")
        );
    }

    #[test]
    fn a_skipped_vblank_is_a_miss_not_a_long_interval() {
        let period = uhd_23976().period();
        let mut p = Pacing::default();
        p.on_flip(10, Duration::ZERO);
        p.on_flip(11, period);
        p.on_flip(13, period * 3); // vblank 12 showed frame 11 again
        p.on_flip(14, period * 4);
        assert_eq!((p.missed, p.vblanks_skipped), (1, 1));
        assert_eq!(p.intervals.len(), 2);
    }

    #[test]
    fn sequence_wraps() {
        let mut p = Pacing::default();
        p.on_flip(u32::MAX, Duration::ZERO);
        p.on_flip(0, Duration::from_millis(42));
        assert_eq!(p.missed, 0);
    }

    #[test]
    fn deep_colour_only_where_the_link_carries_it() {
        // 4096x2160: 23.976 and 59.94 Hz, as the JVC offers them.
        assert_eq!(mode(4096, 2160, 296_703, 5500, 2250).hdmi_bpc(), 10);
        assert_eq!(mode(4096, 2160, 593_407, 4400, 2250).hdmi_bpc(), 8);
        // 600 MHz exactly fits.
        assert_eq!(mode(4096, 2160, 480_000, 4400, 2250).hdmi_bpc(), 10);
    }
}
