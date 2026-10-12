//! `cinefit capture`: run the input for a while and report what arrived.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use cinefit_decklink::{
    Colorspace, Eotf, Frame, FrameInfo, Handler, HdrMetadata, InputFormat, PixelFormat,
};

/// What a capture run saw. Counting only: frames are dropped as soon as they
/// are counted, so the card's buffers are never held.
#[derive(Debug, Default)]
pub struct Stats {
    /// Every format change, with when it happened.
    pub format_changes: Vec<(Duration, InputFormat)>,
    /// Frames with a signal, by the pixel format they arrived in.
    pub frames: BTreeMap<PixelFormat, u64>,
    /// Frames the card delivered with nothing on its input.
    pub no_input: u64,
    /// Breaks in stream time between consecutive frames.
    pub gaps: u64,
    /// Frames those breaks skipped, as far as stream time can tell.
    pub missed: u64,
    /// Frames that carried no stream time, so could not be checked for gaps.
    pub untimed: u64,
    /// Frames by transfer function and colorspace; `None` is unreported.
    pub signal: BTreeMap<(Option<Eotf>, Option<Colorspace>), u64>,
    /// HDR static metadata over the run, one entry per change: when it
    /// started, the value (`None` for frames without any), and how many
    /// consecutive frames carried it.
    pub hdr: Vec<(Duration, Option<HdrMetadata>, u64)>,
    /// Frames the card flagged as carrying Dolby Vision metadata.
    pub dolby_vision_flagged: u64,
    /// Frames whose pixels could not be read.
    pub pixel_read_failures: u64,
    /// Liveness: consecutive frame pairs whose middle row changed, and the
    /// pairs compared. A valid signal is not a picture; this is.
    pub middle_row_changed: u64,
    pub middle_row_compared: u64,
    /// `r210` frames checked for 8-bit content padded to 10 bits, and how
    /// many had any of the low two bits set in the rows checked.
    pub r210_checked: u64,
    pub r210_low_bits_set: u64,
    /// Tunneled Dolby Vision, looked for in every RGB frame.
    pub tunnel: TunnelStats,
    last_stream_time: Option<i64>,
    last_middle_row: Vec<u8>,
}

/// What the tunnel decoder found in the RGB frames of a run.
#[derive(Debug, Default)]
pub struct TunnelStats {
    /// RGB frames looked at.
    pub checked: u64,
    pub decoded: u64,
    /// Frames whose first packet failed its CRC in all three copies: no tunnel.
    pub not_tunnel: u64,
    /// Frames whose packets passed their CRCs but did not decode, by reason.
    pub errors: BTreeMap<String, u64>,
    /// Decoded frames that needed a second or third copy of some packet.
    pub fallback: u64,
    /// Decoded frames saying "no metadata in this picture".
    pub no_metadata: u64,
    /// Times L1 changed between decoded frames: roughly, shot changes.
    pub l1_changes: u64,
    /// Every L2 target seen, as 12-bit PQ.
    pub l2_targets: BTreeSet<u16>,
    /// Decoded frames carrying L5, the active area.
    pub l5: u64,
    last_l1: Option<cinefit_dovi::Level1>,
}

impl TunnelStats {
    pub fn on_frame(&mut self, result: &Result<cinefit_dovi::Tunnel, cinefit_dovi::Error>) {
        self.checked += 1;
        let tunnel = match result {
            Ok(t) => t,
            Err(cinefit_dovi::Error::NoPacket { packet: 0 }) => {
                self.not_tunnel += 1;
                return;
            }
            Err(e) => {
                *self.errors.entry(e.to_string()).or_default() += 1;
                return;
            }
        };
        self.decoded += 1;
        if tunnel.copies_used.iter().any(|&c| c != 0) {
            self.fallback += 1;
        }
        let Some(m) = &tunnel.metadata else {
            self.no_metadata += 1;
            return;
        };
        if m.level1 != self.last_l1 && self.last_l1.is_some() {
            self.l1_changes += 1;
        }
        self.last_l1 = m.level1;
        self.l2_targets
            .extend(m.level2.iter().map(|b| b.target_max_pq));
        if m.level5.is_some() {
            self.l5 += 1;
        }
    }

    fn summary(&self, s: &mut String) {
        if self.checked == 0 {
            return;
        }
        let _ = writeln!(
            s,
            "Dolby Vision tunnel: decoded in {} of {} RGB frames",
            self.decoded, self.checked
        );
        if self.decoded == 0 {
            return;
        }
        let nits: Vec<_> = self
            .l2_targets
            .iter()
            .map(|&pq| format!("{:.0}", cinefit_dovi::pq_to_nits(pq)))
            .collect();
        let _ = writeln!(
            s,
            "  L1 changes: {}, L2 targets: {} nits, L5 in {} frames",
            self.l1_changes,
            if nits.is_empty() {
                "none".into()
            } else {
                nits.join(", ")
            },
            self.l5
        );
        let _ = writeln!(
            s,
            "  not a tunnel: {}, needed a fallback copy: {}, no metadata: {}",
            self.not_tunnel, self.fallback, self.no_metadata
        );
        for (why, n) in &self.errors {
            let _ = writeln!(s, "  failed: {why}: {n}");
        }
    }
}

impl Stats {
    pub fn on_format(&mut self, at: Duration, format: &InputFormat) {
        self.format_changes.push((at, *format));
        // Stream time restarts in the new mode's units.
        self.last_stream_time = None;
    }

    pub fn on_frame(&mut self, at: Duration, info: &FrameInfo) {
        if !info.has_input {
            self.no_input += 1;
            self.last_stream_time = None;
            return;
        }
        *self.frames.entry(info.pixel_format).or_default() += 1;

        let color = &info.color;
        *self
            .signal
            .entry((color.eotf, color.colorspace))
            .or_default() += 1;
        match self.hdr.last_mut() {
            Some((_, value, n)) if *value == color.hdr => *n += 1,
            _ => self.hdr.push((at, color.hdr, 1)),
        }
        if info.dolby_vision_flagged {
            self.dolby_vision_flagged += 1;
        }

        let (Some(t), Some(d)) = (info.stream_time, info.duration) else {
            self.untimed += 1;
            self.last_stream_time = None;
            return;
        };
        if let Some(last) = self.last_stream_time
            && d > 0
        {
            let delta = t - last;
            // One tick of slop: stream time is in the mode's own units, but a
            // driver could still round.
            if (delta - d).abs() > 1 {
                self.gaps += 1;
                if delta > d {
                    // Round to whole frames; the frame itself is not missed.
                    let frames = (delta + d / 2) / d;
                    self.missed += u64::try_from(frames - 1).unwrap_or(0);
                }
            }
        }
        self.last_stream_time = Some(t);
    }

    /// Checks one frame's pixels: the middle row for liveness, and for
    /// `r210` the first and middle rows for padded 8-bit content.
    pub fn on_pixels(&mut self, info: &FrameInfo, first_row: &[u8], middle_row: &[u8]) {
        // Rows of a new size belong to a new format: nothing to compare yet.
        if self.last_middle_row.len() == middle_row.len() {
            self.middle_row_compared += 1;
            if self.last_middle_row != middle_row {
                self.middle_row_changed += 1;
            }
        }
        self.last_middle_row.clear();
        self.last_middle_row.extend_from_slice(middle_row);

        if info.pixel_format == PixelFormat::Rgb10 {
            let width = info.width as usize;
            self.r210_checked += 1;
            if r210_low_bits_set(first_row, width) || r210_low_bits_set(middle_row, width) {
                self.r210_low_bits_set += 1;
            }
        }
    }

    pub fn total_frames(&self) -> u64 {
        self.frames.values().sum()
    }

    /// The human-readable summary printed at the end of a run.
    pub fn summary(&self, ran: Duration, failed_restarts: u64) -> String {
        let mut s = String::new();
        let secs = ran.as_secs_f64();
        let _ = writeln!(s, "ran {secs:.1} s");

        let _ = writeln!(s, "format changes: {}", self.format_changes.len());
        for (at, f) in &self.format_changes {
            let _ = writeln!(s, "  {:6.2} s  {f} -> {}", at.as_secs_f64(), f.capture_as());
        }

        let total = self.total_frames();
        let _ = write!(s, "frames: {total}");
        // Only meaningful when one format held from its detection to the stop.
        if let [(at, f)] = self.format_changes.as_slice()
            && f.frame_duration > 0
            && ran > *at
        {
            let expected = (ran - *at).as_secs_f64() * f.frames_per_second();
            let _ = write!(s, " (about {expected:.0} expected since detection)");
        }
        let _ = writeln!(s);
        for (pf, n) in &self.frames {
            let _ = writeln!(s, "  {pf}: {n}");
        }
        let _ = writeln!(s, "no input source: {}", self.no_input);

        let _ = writeln!(s, "signal:");
        let or_unreported = |v: Option<String>| v.unwrap_or_else(|| "unreported".into());
        for ((eotf, cs), n) in &self.signal {
            let _ = writeln!(
                s,
                "  {}, {}: {n}",
                or_unreported(eotf.map(|e| e.to_string())),
                or_unreported(cs.map(|c| c.to_string())),
            );
        }
        // A changing value is the point of the check, but a value that flips
        // every frame would flood the summary.
        const SHOWN: usize = 20;
        let _ = writeln!(
            s,
            "HDR metadata: {} change(s)",
            self.hdr.len().saturating_sub(1)
        );
        for (at, value, n) in self.hdr.iter().take(SHOWN) {
            let value = value.map_or_else(|| "none".to_string(), |m| m.to_string());
            let _ = writeln!(s, "  {:6.2} s  {value} ({n} frames)", at.as_secs_f64());
        }
        if self.hdr.len() > SHOWN {
            let _ = writeln!(s, "  ... {} more", self.hdr.len() - SHOWN);
        }
        let _ = writeln!(s, "Dolby Vision flag: {} frames", self.dolby_vision_flagged);
        let _ = writeln!(
            s,
            "stream gaps: {} (about {} frames missed)",
            self.gaps, self.missed
        );
        if self.untimed > 0 {
            let _ = writeln!(s, "frames without stream time: {}", self.untimed);
        }
        let _ = writeln!(
            s,
            "pixels: middle row changed in {} of {} frame pairs",
            self.middle_row_changed, self.middle_row_compared
        );
        if self.pixel_read_failures > 0 {
            let _ = writeln!(s, "pixel reads failed: {}", self.pixel_read_failures);
        }
        if self.r210_checked > 0 {
            let _ = writeln!(
                s,
                "r210 low two bits: set in {} of {} frames (first and middle rows)",
                self.r210_low_bits_set, self.r210_checked
            );
        }
        self.tunnel.summary(&mut s);
        let _ = writeln!(s, "failed restarts: {failed_restarts}");
        s
    }
}

/// Whether any of the first `width` pixels of an `r210` row has a low bit set
/// in any component. Each pixel is a big-endian word: 2 unused bits, then
/// red, green and blue at 10 bits each. 8-bit content padded to 10 bits has
/// the bottom two bits of every component clear.
pub fn r210_low_bits_set(row: &[u8], width: usize) -> bool {
    const LOW_BITS: u32 = 0b11 << 20 | 0b11 << 10 | 0b11;
    row.as_chunks::<4>()
        .0
        .iter()
        .take(width)
        .any(|w| u32::from_be_bytes(*w) & LOW_BITS != 0)
}

/// Counts what the input delivers. Runs on the SDK's thread.
pub struct Recorder {
    started: Instant,
    stats: Arc<Mutex<Stats>>,
    dump: Option<Dumper>,
}

impl Recorder {
    pub fn new(stats: Arc<Mutex<Stats>>, dump: Option<Dumper>) -> Self {
        Self {
            started: Instant::now(),
            stats,
            dump,
        }
    }
}

/// One frame saved for offline analysis: its bytes exactly as captured.
pub struct Dump {
    pub index: usize,
    pub at: Duration,
    pub info: FrameInfo,
    pub bytes: Vec<u8>,
}

/// Hands a copy of one frame every `every` to a writer thread. The SDK thread
/// only copies; the disk write happens elsewhere, so it cannot stall capture.
pub struct Dumper {
    every: Duration,
    state: Mutex<(Sender<Dump>, Duration, usize)>,
}

impl Dumper {
    pub fn new(every: Duration, tx: Sender<Dump>) -> Self {
        Self {
            every,
            state: Mutex::new((tx, Duration::ZERO, 0)),
        }
    }

    fn offer(&self, at: Duration, info: &FrameInfo, bytes: &[u8]) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let (tx, next, index) = &mut *state;
        if at < *next {
            return;
        }
        *next = at + self.every;
        // A closed channel means the writer stopped; nothing to do about it here.
        let _ = tx.send(Dump {
            index: *index,
            at,
            info: *info,
            bytes: bytes.to_vec(),
        });
        *index += 1;
    }
}

/// Writes `frame-NNN.bin` (the raw bytes) and `frame-NNN.json` (what they are).
pub fn write_dump(dir: &Path, d: &Dump) -> std::io::Result<()> {
    let i = &d.info;
    let opt = |v: Option<String>| v.map_or_else(|| "null".to_string(), |v| format!("{v:?}"));
    let json = format!(
        "{{\n  \"at_seconds\": {:.3},\n  \"width\": {},\n  \"height\": {},\n  \"row_bytes\": {},\n  \"pixel_format\": \"{}\",\n  \"stream_time\": {},\n  \"eotf\": {},\n  \"colorspace\": {},\n  \"hdr\": {}\n}}\n",
        d.at.as_secs_f64(),
        i.width,
        i.height,
        i.row_bytes,
        i.pixel_format,
        i.stream_time
            .map_or_else(|| "null".to_string(), |t| t.to_string()),
        opt(i.color.eotf.map(|e| e.to_string())),
        opt(i.color.colorspace.map(|c| c.to_string())),
        opt(i.color.hdr.map(|h| h.to_string())),
    );
    let base = dir.join(format!("frame-{:03}", d.index));
    std::fs::write(base.with_extension("bin"), &d.bytes)?;
    std::fs::write(base.with_extension("json"), json)
}

impl Handler for Recorder {
    fn format_changed(&self, format: &InputFormat) {
        let at = self.started.elapsed();
        lock(&self.stats).on_format(at, format);
    }

    fn frame(&self, frame: Frame) {
        let at = self.started.elapsed();
        let info = *frame.info();
        let mut stats = lock(&self.stats);
        stats.on_frame(at, &info);
        if !info.has_input {
            return;
        }
        let rows =
            frame.pixels().and_then(|p| {
                if let Some(dump) = &self.dump {
                    dump.offer(at, &info, p.bytes());
                }
                // Only RGB can carry the tunnel. Decoding reads at most a few
                // thousand pixels, cheap enough for the SDK's thread.
                let (w, stride) = (info.width as usize, info.row_bytes);
                let tunnel =
                    match info.pixel_format {
                        PixelFormat::Rgb10 => Some(cinefit_dovi::decode(
                            cinefit_dovi::r210_parity(p.bytes(), w, stride),
                        )),
                        PixelFormat::Argb8 => Some(cinefit_dovi::decode(
                            cinefit_dovi::argb8_parity(p.bytes(), w, stride),
                        )),
                        _ => None,
                    };
                let first = p.row(0)?.to_vec();
                let middle = p.row(info.height as usize / 2)?.to_vec();
                Some((first, middle, tunnel))
            });
        match rows {
            Some((first, middle, tunnel)) => {
                stats.on_pixels(&info, &first, &middle);
                if let Some(t) = &tunnel {
                    stats.tunnel.on_frame(t);
                }
            }
            None => stats.pixel_read_failures += 1,
        }
        // `frame` drops here, handing its buffer straight back to the card.
    }
}

/// A poisoned lock only means a panic elsewhere, which aborts across the SDK
/// boundary anyway; the counts are still usable.
pub fn lock(stats: &Mutex<Stats>) -> std::sync::MutexGuard<'_, Stats> {
    stats.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cinefit_decklink::ColorInfo;

    const NO_COLOR: ColorInfo = ColorInfo {
        colorspace: None,
        eotf: None,
        hdr: None,
    };

    fn frame(t: i64) -> FrameInfo {
        FrameInfo {
            width: 3840,
            height: 2160,
            row_bytes: 10240,
            pixel_format: PixelFormat::V210,
            stream_time: Some(t),
            duration: Some(1001),
            has_input: true,
            color: NO_COLOR,
            dolby_vision_flagged: false,
        }
    }

    fn hdr(max_cll: f64) -> HdrMetadata {
        HdrMetadata {
            red: None,
            green: None,
            blue: None,
            white: None,
            max_mastering: Some(1000.0),
            min_mastering: Some(0.005),
            max_cll: Some(max_cll),
            max_fall: Some(400.0),
        }
    }

    #[test]
    fn steady_stream_has_no_gaps() {
        let mut s = Stats::default();
        for i in 0..100 {
            s.on_frame(Duration::ZERO, &frame(i * 1001));
        }
        assert_eq!(s.total_frames(), 100);
        assert_eq!((s.gaps, s.missed), (0, 0));
    }

    #[test]
    fn a_skipped_stretch_is_one_gap_counting_its_frames() {
        let mut s = Stats::default();
        s.on_frame(Duration::ZERO, &frame(0));
        s.on_frame(Duration::ZERO, &frame(1001));
        s.on_frame(Duration::ZERO, &frame(4 * 1001)); // frames 2 and 3 never arrived
        assert_eq!((s.gaps, s.missed), (1, 2));
    }

    #[test]
    fn one_tick_of_jitter_is_not_a_gap() {
        let mut s = Stats::default();
        s.on_frame(Duration::ZERO, &frame(0));
        s.on_frame(Duration::ZERO, &frame(1002));
        s.on_frame(Duration::ZERO, &frame(2002));
        assert_eq!(s.gaps, 0);
    }

    #[test]
    fn no_input_frames_are_counted_apart_and_reset_timing() {
        let mut s = Stats::default();
        s.on_frame(Duration::ZERO, &frame(0));
        s.on_frame(
            Duration::ZERO,
            &FrameInfo {
                has_input: false,
                ..frame(1001)
            },
        );
        s.on_frame(Duration::ZERO, &frame(50 * 1001)); // after a signal loss: not a gap
        assert_eq!(s.no_input, 1);
        assert_eq!(s.total_frames(), 2);
        assert_eq!(s.gaps, 0);
    }

    #[test]
    fn frames_without_stream_time_are_counted_as_untimed() {
        let mut s = Stats::default();
        s.on_frame(
            Duration::ZERO,
            &FrameInfo {
                stream_time: None,
                ..frame(0)
            },
        );
        assert_eq!(s.untimed, 1);
        assert_eq!(s.total_frames(), 1);
    }

    #[test]
    fn frames_are_counted_by_pixel_format() {
        let mut s = Stats::default();
        s.on_frame(Duration::ZERO, &frame(0));
        s.on_frame(
            Duration::ZERO,
            &FrameInfo {
                pixel_format: PixelFormat::Argb8,
                ..frame(1001)
            },
        );
        assert_eq!(s.frames.get(&PixelFormat::V210), Some(&1));
        assert_eq!(s.frames.get(&PixelFormat::Argb8), Some(&1));
    }

    #[test]
    fn hdr_metadata_is_kept_as_a_timeline_of_changes() {
        let mut s = Stats::default();
        let with = |m: Option<HdrMetadata>, t| FrameInfo {
            color: ColorInfo { hdr: m, ..NO_COLOR },
            ..frame(t)
        };
        s.on_frame(Duration::ZERO, &with(None, 0));
        s.on_frame(Duration::from_secs(1), &with(Some(hdr(637.0)), 1001));
        s.on_frame(Duration::from_secs(2), &with(Some(hdr(637.0)), 2002));
        s.on_frame(Duration::from_secs(3), &with(Some(hdr(1000.0)), 3003));
        let runs: Vec<_> = s
            .hdr
            .iter()
            .map(|(at, m, n)| (at.as_secs(), m.and_then(|m| m.max_cll), *n))
            .collect();
        assert_eq!(
            runs,
            [(0, None, 1), (1, Some(637.0), 2), (3, Some(1000.0), 1)]
        );
    }

    #[test]
    fn signal_is_counted_by_eotf_and_colorspace() {
        let mut s = Stats::default();
        let sdr = |cs| FrameInfo {
            color: ColorInfo {
                eotf: Some(Eotf::Sdr),
                colorspace: Some(cs),
                hdr: None,
            },
            ..frame(0)
        };
        s.on_frame(Duration::ZERO, &sdr(Colorspace::Rec709));
        s.on_frame(Duration::ZERO, &sdr(Colorspace::Rec2020));
        s.on_frame(Duration::ZERO, &sdr(Colorspace::Rec2020));
        assert_eq!(
            s.signal.get(&(Some(Eotf::Sdr), Some(Colorspace::Rec709))),
            Some(&1)
        );
        assert_eq!(
            s.signal.get(&(Some(Eotf::Sdr), Some(Colorspace::Rec2020))),
            Some(&2)
        );
    }

    fn r210(r: u32, g: u32, b: u32) -> [u8; 4] {
        (r << 20 | g << 10 | b).to_be_bytes()
    }

    #[test]
    fn padded_8_bit_r210_has_clear_low_bits() {
        let row: Vec<u8> = [r210(200 << 2, 17 << 2, 255 << 2), r210(0, 0, 0)].concat();
        assert!(!r210_low_bits_set(&row, 2));
    }

    #[test]
    fn any_low_bit_in_any_component_is_found() {
        for px in [r210(801, 0, 0), r210(0, 2, 0), r210(0, 0, 3)] {
            assert!(r210_low_bits_set(&px, 1));
        }
    }

    #[test]
    fn only_the_first_width_pixels_are_checked() {
        // Row padding past the picture is not picture.
        let row: Vec<u8> = [r210(4, 4, 4), r210(1, 1, 1)].concat();
        assert!(!r210_low_bits_set(&row, 1));
    }

    #[test]
    fn middle_row_liveness_counts_changes_between_frames() {
        let mut s = Stats::default();
        let info = frame(0);
        s.on_pixels(&info, &[], &[1, 2, 3]);
        s.on_pixels(&info, &[], &[1, 2, 3]);
        s.on_pixels(&info, &[], &[9, 2, 3]);
        s.on_pixels(&info, &[], &[1, 2]); // new size: not compared
        assert_eq!((s.middle_row_changed, s.middle_row_compared), (1, 2));
    }

    fn tunnel(l1_max: u16, targets: &[u16]) -> cinefit_dovi::Tunnel {
        use cinefit_dovi::{DmMetadata, Header, Level1, Level2, packet::PacketType};
        cinefit_dovi::Tunnel {
            header: Header {
                packet_type: PacketType::First,
                metadata_type: 0,
                affected_metadata_id: 0,
                metadata_version: 0,
                current_metadata_id: 0,
                no_metadata: false,
                end_of_stream: false,
            },
            copies_used: vec![0, 0],
            metadata: Some(DmMetadata {
                scene_refresh: false,
                ycc_to_rgb: [0; 9],
                ycc_to_rgb_offset: [0; 3],
                rgb_to_lms: [0; 9],
                signal_bit_depth: 12,
                signal_color_space: 2,
                source_min_pq: 62,
                source_max_pq: 3696,
                level1: Some(Level1 {
                    min_pq: 62,
                    max_pq: l1_max,
                    avg_pq: 819,
                }),
                level2: targets
                    .iter()
                    .map(|&t| Level2 {
                        target_max_pq: t,
                        trim_slope: 2048,
                        trim_offset: 2048,
                        trim_power: 2048,
                        trim_chroma_weight: 2048,
                        trim_saturation_gain: 2048,
                        ms_weight: 2048,
                    })
                    .collect(),
                level4: None,
                level5: None,
                other: Vec::new(),
            }),
        }
    }

    #[test]
    fn tunnel_stats_count_shots_targets_and_misses() {
        let mut t = TunnelStats::default();
        t.on_frame(&Ok(tunnel(3686, &[2081, 2851])));
        t.on_frame(&Ok(tunnel(3686, &[2081, 2851])));
        t.on_frame(&Ok(tunnel(3465, &[2081, 2851, 3388])));
        t.on_frame(&Err(cinefit_dovi::Error::NoPacket { packet: 0 }));
        t.on_frame(&Err(cinefit_dovi::Error::NoPacket { packet: 1 }));
        assert_eq!((t.checked, t.decoded, t.not_tunnel), (5, 3, 1));
        assert_eq!(t.l1_changes, 1);
        assert_eq!(
            t.l2_targets.iter().copied().collect::<Vec<_>>(),
            [2081, 2851, 3388]
        );
        assert_eq!(t.errors.values().sum::<u64>(), 1);
    }
}
