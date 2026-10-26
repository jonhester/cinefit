//! `cinefit play`: capture to screen. The capture callback puts each frame
//! in a latest-wins mailbox; this thread takes the newest, unpacks and
//! renders it into a free scanout buffer ahead of the vblank, and flips.
//!
//! The output is chosen once, before the modeset, from a pre-roll of stable
//! input: the source's own frame rate, and its HDR signalling in the same
//! commit, so the display resyncs once. Changes of input format during the
//! run are counted, not followed yet.

use std::cell::{Cell, RefCell};
use std::error::Error;
use std::os::fd::AsFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::appletv;
use crate::avr;
use crate::config::{self, Config};
use crate::content::*;
use crate::dataset;
use crate::follow;
use crate::l1;
use crate::menu::Menu;
use crate::osd::InfoText;
use crate::pairfollow;
use crate::runlog;
use crate::snapshot;
use crate::status::{self, Window};
use crate::times::Times;
use crate::ui::{Slate, Ui};
use crate::web::Web;
use cinefit_decklink::{
    Colorspace, Eotf, Frame, FrameInfo, Handler, HdrMetadata, InputFormat, Mailbox, PixelFormat,
};
use cinefit_detect::{Crop, Detector, V210};
use cinefit_geometry as geo;
use cinefit_input::{Action, KeyReader};
use cinefit_render::{
    Curve, DoviColor, GpuPlanes, GpuRgb, Levels, Mapping, Matrix, Placebo, Primaries, Rect,
    RgbColor, RgbSourceColor, Texture, Transfer, View, YcbcrColor, rgb,
};
use log::{info, warn};

/// Frames of unchanged input before the output is chosen. The first frames
/// after a change can carry the wrong EOTF (the C++ version's finding), so
/// one frame is not enough.
const STABLE_FRAMES: usize = 12;
/// Frames of 10-bit RGB that does not decode as a Dolby Vision tunnel before
/// it is taken for plain RGB: two seconds at 24p. The player starts a Dolby
/// Vision title with RGB frames that carry no tunnel yet, and believing them
/// on [`STABLE_FRAMES`] cost a modeset to SDR and another back a second later
/// (2026-10-13, S&M montage). Plain 10-bit RGB is rare from this chain, so
/// waiting for it costs little.
const TUNNEL_GRACE: usize = 48;
/// How long to wait for stable input before starting without it. A source
/// that is on settles in well under a second (a tunnel, in two); past this
/// the display is opened at `output.wait_hz` with black, so it shows neither
/// the console nor a stale picture, and the source is followed when it comes.
const PRE_ROLL_WAIT: Duration = Duration::from_secs(4);
/// No frame for this long is no signal: black, not the last frame held. The
/// C++ version's figure; a frame at 24p is 42 ms, so two missed frames.
const NO_SIGNAL: Duration = Duration::from_millis(100);
/// Startup is excluded from the run's verdict: the first seconds always
/// include the resync.
const SETTLE: Duration = Duration::from_secs(5);
/// A line on how the run is going this often, so a log shows when trouble
/// started rather than only that it happened.
const STATUS_EVERY: Duration = Duration::from_secs(60);
/// Input changes logged one by one; past this many, only counted.
const LOGGED_CHANGES: u64 = 50;
/// Three buffers: one on screen, one queued, one to render into, so a render
/// that runs late still has a target.
const BUFFERS: usize = 3;

pub struct Options {
    /// How long to run; `None` until stopped (Ctrl-C, or SIGTERM from a
    /// service manager).
    pub seconds: Option<f64>,
    /// `-d`: the capture device's index for this run, over `input.device`.
    pub device: Option<usize>,
    pub card: Option<PathBuf>,
    /// The settings this run starts with: the file's, with any --set.
    pub config: crate::config::Config,
    /// The settings file and its settings as they are, without --set: a
    /// change made in the menu is applied to both and saved there.
    pub file: (PathBuf, crate::config::Config),
    pub pstate: bool,
    /// Everything up to the modeset (pre-roll, mode, signalling, the first
    /// render into a scanout buffer), then stop: the display is not touched.
    pub dry_run: bool,
    /// LINEAR scanout buffers, as before #101, for comparing.
    pub linear_scanout: bool,
    /// GPU time per pass, reported by situation at the end (#219).
    pub gpu_timers: bool,
}

/// One scanout buffer and the render target imported from it, together.
struct Scanout<'p> {
    /// The buffer, as the output numbers its frames.
    frame: usize,
    texture: cinefit_render::Texture<'p>,
}

/// The scanout buffers, each with its render target, their pixel format
/// and the layout GBM chose (one for all).
struct Scanouts<'p> {
    buffers: Vec<Scanout<'p>>,
    fourcc: u32,
    modifier: u64,
}

impl<'p> Scanouts<'p> {
    /// Buffer `b`'s frame, for the output.
    fn frame(&self, b: usize) -> usize {
        self.buffers[b].frame
    }

    /// Buffer `b`'s render target.
    fn texture(&self, b: usize) -> &cinefit_render::Texture<'p> {
        &self.buffers[b].texture
    }
}

/// Why a set of scanout buffers could not be made, and the layout to blame
/// when there is one.
struct Failed {
    layout: Option<u64>,
    error: Box<dyn Error>,
}

/// Renders a picture that places every pixel by position into `target`
/// (scanout buffer `frame`) and reads it back through GBM, which detiles: a
/// layout the renderer writes wrongly comes back moved or scrambled. Once at
/// start; about a tenth of a second at 4K.
pub(crate) fn renders_right(
    out: &cinefit_kms::Output,
    pl: &Placebo,
    frame: usize,
    target: &cinefit_render::Texture<'_>,
) -> Result<(), Box<dyn Error>> {
    let d = out.export(frame)?;
    let (w, h) = (d.width as usize, d.height as usize);
    // Red counts across, green down, blue the 256-pixel block: no two
    // pixels within a block alike, and blocks told apart.
    let px = |x: usize, y: usize| [x as u8, y as u8, ((x / 256) * 16 + y / 256) as u8, 255];
    let rgba: Vec<u8> = (0..h)
        .flat_map(|y| (0..w).flat_map(move |x| px(x, y)))
        .collect();
    let src = pl.upload_rgba8(w as u32, h as u32, &rgba)?;
    pl.render(&src, target, 0.0)?;
    let fourcc = d.fourcc;
    let wrong = out.read_scanout(frame, |bytes, stride| {
        (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                let o = y * stride + x * 4;
                let got = cinefit_kms::scanout::decode(
                    fourcc,
                    bytes[o..o + 4].try_into().expect("4 bytes"),
                )
                .map_or([0; 3], |c| c.map(|v| (v * 255.0).round() as u8));
                let want = px(x, y);
                (0..3).any(|c| got[c].abs_diff(want[c]) > 1)
            })
            .count()
    })?;
    if wrong > 0 {
        return Err(format!(
            "a test picture came back with {wrong} of {} pixels wrong",
            w * h
        )
        .into());
    }
    Ok(())
}

/// Makes [`BUFFERS`] scanout buffers for mode `index` in a layout from
/// `modifiers`, and imports each for the renderer.
fn scanouts<'p>(
    out: &mut cinefit_kms::Output,
    pl: &'p Placebo,
    index: usize,
    modifiers: &[u64],
) -> Result<Scanouts<'p>, Failed> {
    let unblamed = |e: Box<dyn Error>| Failed {
        layout: None,
        error: e,
    };
    let mut buffers = Vec::new();
    let (mut fourcc, mut modifier) = (0, 0);
    for i in 0..BUFFERS {
        let b = match out.create_scanout_in(index, modifiers) {
            Ok(b) => b,
            Err(cinefit_kms::Error::Planes(m, n)) => {
                return Err(Failed {
                    layout: Some(m),
                    error: format!("a layout of {n} planes").into(),
                });
            }
            Err(e) => return Err(unblamed(e.into())),
        };
        let d = out.export(b).map_err(|e| unblamed(e.into()))?;
        // One layout for all: the log names one, and the read-back for the
        // info panel takes one.
        if i > 0 && (d.fourcc, d.modifier) != (fourcc, modifier) {
            return Err(unblamed(
                format!(
                    "GBM chose {} for one buffer and {} for another",
                    cinefit_kms::modifier_name(modifier),
                    cinefit_kms::modifier_name(d.modifier)
                )
                .into(),
            ));
        }
        (fourcc, modifier) = (d.fourcc, d.modifier);
        let texture = pl
            .import_dmabuf(&cinefit_render::DmaBufTarget {
                fd: d.fd.as_fd(),
                width: d.width,
                height: d.height,
                fourcc: d.fourcc,
                modifier: d.modifier,
                stride: d.stride,
                offset: d.offset,
            })
            .map_err(|e| Failed {
                layout: Some(d.modifier),
                error: format!("the renderer will not import it: {e}").into(),
            })?;
        buffers.push(Scanout { frame: b, texture });
    }
    Ok(Scanouts {
        buffers,
        fourcc,
        modifier,
    })
}

/// Hands frames from the SDK's thread to the mailbox.
struct Feed {
    mailbox: Arc<Mailbox<Frame>>,
    format: Arc<Mutex<Option<InputFormat>>>,
    captured: Arc<AtomicU64>,
    no_input: Arc<AtomicU64>,
}

impl Handler for Feed {
    fn format_changed(&self, format: &InputFormat) {
        *self.format.lock().unwrap_or_else(PoisonError::into_inner) = Some(*format);
    }

    fn frame(&self, frame: Frame) {
        if frame.info().has_input {
            self.captured.fetch_add(1, Ordering::Relaxed);
            self.mailbox.put(frame);
        } else {
            self.no_input.fetch_add(1, Ordering::Relaxed);
        }
    }
}

fn ycbcr_color(info: &FrameInfo) -> YcbcrColor {
    let bt2020 = info.color.colorspace == Some(Colorspace::Rec2020);
    YcbcrColor {
        matrix: if bt2020 {
            Matrix::Bt2020Nc
        } else {
            Matrix::Bt709
        },
        primaries: if bt2020 {
            Primaries::Bt2020
        } else {
            Primaries::Bt709
        },
        transfer: match info.color.eotf {
            Some(Eotf::Pq) => Transfer::Pq,
            Some(Eotf::Hlg) => Transfer::Hlg,
            _ => Transfer::Bt1886,
        },
    }
}

/// What an `r210` frame's pixels say about the tunnel.
///
/// One per frame, moved straight into `last_tunnel`: boxing the update would
/// only add an allocation a frame.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
enum TunnelFrame {
    /// No packet passes its CRC: not a tunnel.
    Absent,
    /// A tunnel with nothing usable this frame: no metadata, packets that
    /// did not fit, or offsets on no known scale. The last good conversion
    /// still applies.
    Held,
    /// A tunnel with a conversion of its own.
    Update(Tunnel),
}

/// Decodes the tunnel in an `r210` frame, if it carries one.
fn decode_tunnel(info: &FrameInfo, bytes: &[u8]) -> TunnelFrame {
    let parity = cinefit_dovi::r210_parity(bytes, info.width as usize, info.row_bytes);
    tunnel_frame(cinefit_dovi::decode(parity))
}

fn tunnel_frame(decoded: Result<cinefit_dovi::Tunnel, cinefit_dovi::Error>) -> TunnelFrame {
    let tunnel = match decoded {
        Ok(t) => t,
        Err(cinefit_dovi::Error::NoPacket { packet: 0 }) => return TunnelFrame::Absent,
        // A tunnel whose packets did not fit: still a tunnel.
        Err(_) => return TunnelFrame::Held,
    };
    // A packet that passed its CRC is a tunnel, with metadata or without
    // (#194): without, it is no update, not plain RGB.
    let Some(m) = tunnel.metadata else {
        return TunnelFrame::Held;
    };
    let Some(c) = m.conversion() else {
        return TunnelFrame::Held;
    };
    TunnelFrame::Update(Tunnel {
        color: DoviColor {
            ycc_to_lms: c.ycc_to_rgb,
            offset: c.offset,
            linear: c.rgb_to_lms,
        },
        source_min_pq: m.source_min_pq,
        source_max_pq: m.source_max_pq,
        scene_max_pq: m.level1.map(|l| l.max_pq),
        scene_avg_pq: m.level1.map(|l| l.avg_pq),
        scene_min_pq: m.level1.map(|l| l.min_pq),
        trims: m
            .level2
            .iter()
            .map(|t| {
                cinefit_render::dm::Trim::from_codes(
                    t.target_max_pq,
                    [
                        t.trim_slope,
                        t.trim_offset,
                        t.trim_power,
                        t.trim_chroma_weight,
                        t.trim_saturation_gain,
                    ],
                )
            })
            .collect(),
    })
}

/// The kind of an `r210` frame, updating the last good tunnel conversion.
fn rgb10_kind(t: TunnelFrame, last_tunnel: &mut Option<Tunnel>) -> Kind {
    match t {
        TunnelFrame::Absent => Kind::Rgb(rgb::Format::R210),
        TunnelFrame::Held => Kind::Tunnel,
        TunnelFrame::Update(t) => {
            *last_tunnel = Some(t);
            Kind::Tunnel
        }
    }
}

/// Classifies a frame, updating the last good tunnel conversion.
fn classify(frame: &Frame, last_tunnel: &mut Option<Tunnel>) -> Key {
    let info = frame.info();
    let kind = match info.pixel_format {
        PixelFormat::V210 => Kind::Ycbcr(ycbcr_color(info)),
        PixelFormat::Argb8 => Kind::Rgb(rgb::Format::Argb8),
        PixelFormat::Rgb10 => {
            let t = frame
                .pixels()
                .map_or(TunnelFrame::Absent, |p| decode_tunnel(info, p.bytes()));
            rgb10_kind(t, last_tunnel)
        }
        other => Kind::Unsupported(other),
    };
    Key {
        width: info.width,
        height: info.height,
        kind,
    }
}

/// How many frames in a row an input must hold before it is believed.
fn frames_to_trust(key: Key) -> usize {
    match key.kind {
        Kind::Rgb(rgb::Format::R210) => TUNNEL_GRACE,
        _ => STABLE_FRAMES,
    }
}

/// The brightest channel (0-1) of a frame as it went out, from a grid of
/// every 16th row and 8th column: what the display is sent, whatever the
/// renderer was asked to do.
fn scanout_peak(px: &[u8], stride: usize, fourcc: u32, size: (u32, u32)) -> Option<f64> {
    let mut peak: Option<f64> = None;
    for y in (0..size.1 as usize).step_by(16) {
        for x in (0..size.0 as usize).step_by(8) {
            let o = y * stride + x * 4;
            let w = px.get(o..o + 4)?;
            let rgb = cinefit_kms::scanout::decode(fourcc, [w[0], w[1], w[2], w[3]])?;
            let v = rgb.iter().copied().fold(0.0, f64::max);
            peak = Some(peak.map_or(v, |p: f64| p.max(v)));
        }
    }
    peak
}

/// GPU textures for the source, made when its size first appears.
#[derive(Default)]
struct Sources<'p> {
    planes: Option<GpuPlanes<'p>>,
    rgb: Option<GpuRgb<'p>>,
}

/// The last tunnel mapping and the scene and target it was built for (see
/// [`dm_frame`]).
type DmCache = Option<(cinefit_render::dm::Scene, f32, cinefit_render::dm::Frame)>;

/// A frame uploaded and ready to draw: what each kind's draw needs, from the
/// textures in [`Sources`] and the tunnel's conversion.
enum Prepared<'a, 'p> {
    /// YCbCr: low-latency Dolby Vision (`lldv`) is PQ under its SDR label.
    Ycbcr {
        planes: &'a GpuPlanes<'p>,
        color: YcbcrColor,
        lldv: bool,
    },
    Rgb(&'a GpuRgb<'p>),
    Tunnel {
        planes: &'a GpuPlanes<'p>,
        tunnel: &'a Tunnel,
    },
}

/// The tunnel mapping for `scene` at `target` nits, built only when either
/// changed since the last frame: L1 and the trims change at cuts, tens of
/// times a film, and a table costs about 0.05 ms to build (#206).
/// With it, whether it was built just now: a scene change (#219).
fn dm_frame(
    cache: &mut DmCache,
    scene: cinefit_render::dm::Scene,
    target: f32,
) -> (&cinefit_render::dm::Frame, bool) {
    let built = !cache
        .as_ref()
        .is_some_and(|(s, t, _)| *s == scene && *t == target);
    if built {
        let frame = cinefit_render::dm::Frame::new(&scene, f64::from(target));
        *cache = Some((scene, target, frame));
    }
    (&cache.as_ref().expect("just set").2, built)
}

/// Per-stage times, milliseconds.
#[derive(Default)]
struct Stages {
    unpack: Times,
    render: Times,
    /// The bar detector, on its own thread beside the upload (#220), by what
    /// it read; and the two together, which is what the frame waits for.
    detect_tunnel: Times,
    detect_v210: Times,
    detect_and_upload: Times,
    /// GPU time per pass ([`cinefit_render::PASSES`]), with `--gpu-timers`,
    /// every result once; and the runs timing could not follow.
    gpu: [Times; 5],
    gpu_losses: cinefit_render::GpuLosses,
    /// The last frames drawn, by tag, and what was going on at each: a GPU
    /// result comes in a frame or two after its run, and is put down to the
    /// frame it measured, not the one drawn when it came in (#263).
    recent: std::collections::VecDeque<(u64, Situation)>,
    /// The same, by what was going on (#219), and the flips missed in each.
    by: [Split; 3],
}

/// What was going on at a frame: the times of the steady frames, of a
/// scene change's, and of those drawn with the menu or panel over them, kept
/// apart so a spike can be put down to one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Situation {
    Steady,
    /// A new Dolby mapping table, built for the frame (#206).
    NewTable,
    Overlay,
}

impl Situation {
    const ALL: [Situation; 3] = [Situation::Steady, Situation::NewTable, Situation::Overlay];

    fn of(new_table: bool, overlay: bool) -> Self {
        if overlay {
            Situation::Overlay
        } else if new_table {
            Situation::NewTable
        } else {
            Situation::Steady
        }
    }

    fn name(self) -> &'static str {
        match self {
            Situation::Steady => "steady",
            Situation::NewTable => "a new Dolby table (a scene change)",
            Situation::Overlay => "the menu or info panel open",
        }
    }
}

#[derive(Default)]
struct Split {
    frames: u64,
    missed: u64,
    unpack: Times,
    detect_and_upload: Times,
    render: Times,
    gpu: [Times; 5],
}

/// Frames whose situation is remembered for their GPU results to find: far
/// more than results lag, which is a frame or two.
const RECENT_FRAMES: usize = 64;

impl Stages {
    /// A frame drawn, under what was going on: its stage times (the latest
    /// pushed, this frame's), and the GPU results new since the last, each put
    /// down to the frame its run was tagged with. A result without a tag (the
    /// render passes, timed inside libplacebo) counts only in the totals.
    fn frame_done(
        &mut self,
        tag: u64,
        situation: Situation,
        gpu: &[cinefit_render::GpuResult],
        losses: cinefit_render::GpuLosses,
    ) {
        if self.recent.len() == RECENT_FRAMES {
            self.recent.pop_front();
        }
        self.recent.push_back((tag, situation));
        let s = &mut self.by[situation as usize];
        s.frames += 1;
        for (from, to) in [
            (&self.unpack, &mut s.unpack),
            (&self.detect_and_upload, &mut s.detect_and_upload),
            (&self.render, &mut s.render),
        ] {
            if let Some(v) = from.last() {
                to.push(v);
            }
        }
        for r in gpu {
            self.gpu[r.pass].push(r.ms);
            let of = r
                .tag
                .and_then(|t| self.recent.iter().find(|(tag, _)| *tag == t));
            if let Some(&(_, at)) = of {
                self.by[at as usize].gpu[r.pass].push(r.ms);
            }
        }
        self.gpu_losses = losses;
    }

    /// The end-of-run lines for the GPU passes and each situation (#219).
    fn split_lines(&self, gpu: bool) -> Vec<String> {
        let mut lines = Vec::new();
        let passes = |times: &[Times; 5], lines: &mut Vec<String>, indent: &str| {
            for (i, t) in times.iter().enumerate() {
                if t.count() > 0 {
                    let name = format!("{indent}GPU {}", cinefit_render::PASSES[i]);
                    lines.push(t.line(&name).trim_end().to_string());
                }
            }
        };
        if gpu {
            passes(&self.gpu, &mut lines, "");
            let l = self.gpu_losses;
            for (i, pass) in cinefit_render::PASSES.iter().enumerate() {
                let (d, u, a) = (l.dropped[i], l.untimed[i], l.unanswered[i]);
                if d + u + a > 0 {
                    lines.push(format!(
                        "GPU {pass} runs not followed: {d} results dropped, {u} untimed (no free timer), {a} timers unanswered"
                    ));
                }
            }
        }
        for situation in Situation::ALL {
            let s = &self.by[situation as usize];
            if s.frames == 0 {
                continue;
            }
            lines.push(format!(
                "{}: {} frames, {} flips missed",
                situation.name(),
                s.frames,
                s.missed
            ));
            for (name, t) in [
                ("unpack, with upload", &s.unpack),
                (
                    "detection beside the upload, both done",
                    &s.detect_and_upload,
                ),
                ("render, with the wait for the GPU", &s.render),
            ] {
                lines.push(t.line(&format!("  {name}")).trim_end().to_string());
            }
            if gpu {
                passes(&s.gpu, &mut lines, "  ");
            }
        }
        lines
    }

    /// The detector's time on a frame, under what it read; nothing where it
    /// did not read one (cropping off, a kind it does not read).
    fn detected(&mut self, ms: Option<f64>, kind: Kind) {
        match (ms, kind) {
            (Some(d), Kind::Tunnel) => self.detect_tunnel.push(d),
            (Some(d), Kind::Ycbcr(_)) => self.detect_v210.push(d),
            _ => {}
        }
    }
}

/// Where a frame is rendered: a scanout buffer of the output's size, in the
/// output's colour.
struct Target<'a, 'p> {
    tex: &'a Texture<'p>,
    color: RgbColor,
    /// What plain RGB input's codes mean (the `input.rgb_range` setting).
    rgb_range: Levels,
    /// What SDR white stands for: low-latency Dolby Vision's own peak, which
    /// passes one to one, and what HDR10 and the tunnel are mapped into
    /// (the settings), with the curve.
    lldv_nits: f32,
    mapped_nits: f32,
    curve: Curve,
    /// The `tonemap.peak_detect`, `tonemap.contrast_recovery`,
    /// `tonemap.peak_percentile` and `tonemap.knee_offset` settings.
    peak_detect: bool,
    contrast_recovery: f32,
    peak_percentile: f32,
    knee_offset: f32,
    /// The `tonemap.source_peak` override for HDR10, if set.
    source_peak: Option<f32>,
    /// Which part of the frame is drawn and where it lands.
    view: View,
}

/// How often to probe the connector again while its modes lack the one
/// wanted. Each probe reads the display's EDID, so not every frame; a display
/// coming on is followed within this.
const MODE_PROBE_EVERY: Duration = Duration::from_secs(5);

/// The output, waiting for a connected display when there is none yet (#177):
/// at boot the service can start before the display's connector reads
/// connected. Exiting and letting the service restart came back by itself,
/// as for a missing mode (#143), but reopened the capture card and wrote a run
/// log every time. Logged once, probed again every few seconds. Any other
/// error, and a dry run, still end the run.
fn wait_for_display(
    card: Option<&std::path::Path>,
    dry_run: bool,
) -> Result<cinefit_kms::Output, Box<dyn Error>> {
    let out = retry_no_display(
        || cinefit_kms::Output::open(card),
        dry_run,
        MODE_PROBE_EVERY,
    )?;
    Ok(out)
}

/// `open` again every `every` while it says no display is connected; any
/// other answer, or the first one in a dry run, is the result.
fn retry_no_display<T>(
    mut open: impl FnMut() -> Result<T, cinefit_kms::Error>,
    dry_run: bool,
    every: Duration,
) -> Result<T, cinefit_kms::Error> {
    let mut waiting_since: Option<Instant> = None;
    loop {
        match open() {
            Err(e @ cinefit_kms::Error::NoDisplay) if !dry_run => {
                if waiting_since.is_none() {
                    info!("{e}: waiting for one (probing every {} s)", every.as_secs());
                    waiting_since = Some(Instant::now());
                }
            }
            result => {
                if let (Ok(_), Some(t)) = (&result, waiting_since) {
                    info!(
                        "display connected after {:.0} s of waiting",
                        t.elapsed().as_secs_f64()
                    );
                }
                return result;
            }
        }
        std::thread::sleep(every);
    }
}

/// The wanted mode, from the modes the output offers, waiting for it when it
/// is not there yet (#143): a receiver offers only its own 1080p fallback
/// while the projector behind it is off. Exiting and letting the service
/// restart did come back by itself, but every attempt reopened the capture
/// card and wrote a run log (440 in an hour). Instead the list is logged once,
/// probed again every few seconds, and logged again only when it changes. A
/// connector that stops being connected still ends the run, so the service
/// recovers from a display that is gone. A dry run does not wait.
fn wait_for_mode(
    out: &mut cinefit_kms::Output,
    want: &cinefit_kms::ModeRequest,
    dry_run: bool,
) -> Result<(Vec<cinefit_kms::ModeTiming>, usize), Box<dyn Error>> {
    let mut watch = cinefit_kms::ModeWatch::default();
    let mut modes = out.modes();
    let mut connected = true;
    let mut waiting_since: Option<Instant> = None;
    loop {
        match watch.observe(connected, &modes, want) {
            cinefit_kms::Watch::Ready(index) => {
                if let Some(t) = waiting_since {
                    info!(
                        "mode offered after {:.0} s of waiting",
                        t.elapsed().as_secs_f64()
                    );
                }
                return Ok((modes, index));
            }
            cinefit_kms::Watch::Gone => {
                return Err("the display's connector is no longer connected".into());
            }
            cinefit_kms::Watch::Changed => {
                info!("progressive modes offered:");
                for m in modes.iter().filter(|m| !m.interlaced) {
                    info!("  {m}{}", if m.preferred { " (preferred)" } else { "" });
                }
                let missing = format!(
                    "no {}x{} mode within 0.01 Hz of {:.3}",
                    want.width, want.height, want.refresh_hz
                );
                if dry_run {
                    return Err(missing.into());
                }
                if waiting_since.is_none() {
                    info!(
                        "{missing}: waiting for the display to offer it (probing every {} s)",
                        MODE_PROBE_EVERY.as_secs()
                    );
                    waiting_since = Some(Instant::now());
                }
            }
            cinefit_kms::Watch::Unchanged => {}
        }
        std::thread::sleep(MODE_PROBE_EVERY);
        (connected, modes) = out.probe_modes()?;
    }
}

/// The render's rectangles for a frame: the detector's rows placed at constant
/// image height in the screen's band of the raster, or the whole frame fitted
/// to the raster when cropping is off. Geometry stays in these crop rects; a
/// geometry change never commits.
fn view_for(
    frame: (u32, u32),
    out: (u32, u32),
    win: geo::Rect,
    source: geo::Source,
    fill_max_ar: f64,
) -> View {
    if source == geo::Source::Whole {
        return View::placed(Some(Rect::fit(frame.0, frame.1, out.0, out.1)));
    }
    let p = geo::place_frame(frame, out, win, source, fill_max_ar);
    let rect = |r: geo::Rect| Rect {
        x0: r.x as f32,
        y0: r.y as f32,
        x1: (r.x + r.w) as f32,
        y1: (r.y + r.h) as f32,
    };
    View {
        src: Some(rect(p.src)),
        dst: Some(rect(p.dst)),
    }
}

/// What a frame renders into, by the settings as they are now: they can
/// change mid-run, from the menu.
fn target<'a, 'p>(tex: &'a Texture<'p>, color: RgbColor, view: View, c: &Config) -> Target<'a, 'p> {
    Target {
        tex,
        color,
        rgb_range: c.rgb_range,
        lldv_nits: f32::from(c.lldv_nits),
        mapped_nits: f32::from(c.mapped_nits()),
        curve: c.curve,
        peak_detect: c.peak_detect,
        contrast_recovery: c.contrast_recovery as f32,
        peak_percentile: c.peak_percentile as f32,
        knee_offset: c.knee_offset as f32,
        source_peak: (c.source_peak > 0).then_some(f32::from(c.source_peak)),
        view,
    }
}

/// The bar detector as `play` runs it: on a worker thread, beside the
/// upload of the same frame.
/// The detector's epoch: a new one on each change of input format, each
/// return of the signal and each change of the input's frame rate (#222),
/// on which the detector forgets what it learned. Frame rate is not in a
/// frame's key, so it is followed here from what the card reports.
#[derive(Default)]
struct Epochs {
    n: u32,
    rate_mhz: Option<u32>,
}

impl Epochs {
    /// The epoch now; the first is 1.
    fn now(&self) -> u32 {
        self.n + 1
    }

    fn next(&mut self) {
        self.n += 1;
    }

    /// The input's frame rate as the card reports it: a change is a new
    /// epoch; the first report is not.
    fn rate(&mut self, mhz: u32) {
        if self.rate_mhz.is_some_and(|r| r != mhz) {
            self.next();
        }
        self.rate_mhz = Some(mhz);
    }
}

struct CropFinder {
    det: Detector,
    /// The `detect.crop` setting.
    on: bool,
    crops: u64,
    /// The last frame was one the detector does not read, and the crop went
    /// to full frame for it.
    unread: bool,
    /// Make each frame's fingerprint for the dataset (`detect.record`, #91).
    thumbs: bool,
    /// Cuts in tunneled Dolby Vision, which the detector does not read:
    /// for the dataset, so a tunnel pass can be paired with a low-latency one
    /// by its shots (#109). The epoch it last saw, to start again on a new one.
    tunnel_cuts: cinefit_detect::cuts::Cuts,
    tunnel_epoch: u32,
    /// Measure HDR10 frames' L1 for Dolby's mapping (`output.mode` dolby,
    /// #141), over the rows the crop keeps.
    measure: bool,
}

/// What the detector made of one frame.
struct Found {
    source: geo::Source,
    /// Its time on this frame, when it ran.
    ms: Option<f64>,
    /// The crop it changed to on this frame, and why.
    change: Option<(Crop, &'static str)>,
    /// The frame's brightest luma code (10-bit), YCbCr frames only.
    peak: Option<u16>,
    /// What the detector measured, and whether it saw a cut, when it ran:
    /// the dataset's reading (#91).
    reading: Option<(cinefit_detect::bars::Observation, bool)>,
    /// The frame's fingerprint for the dataset, YCbCr frames only.
    thumb: Option<[u8; dataset::THUMB]>,
    /// The policy holds a crop it has not adopted: the detector is unsure,
    /// a moment the dataset keeps a frame of (#91).
    candidate: bool,
    /// A hard cut on this frame: from the detector on YCbCr, from a cut
    /// detector of its own on the tunnel (#109).
    cut: bool,
    /// HDR10 only, while measuring: the frame's L1 over the picture (#141).
    l1: Option<l1::FrameL1>,
}

/// The brightest luma code in a frame, from every 16th row: enough to see
/// where a player's highlights go, for about half a millisecond on the
/// worker thread.
fn peak_luma(v: &V210<'_>) -> u16 {
    use cinefit_detect::Samples;
    let mut row = vec![0u16; v.width() as usize];
    let mut peak = 0;
    for y in (0..v.height()).step_by(16) {
        v.luma_row(y, &mut row);
        peak = peak.max(row.iter().copied().max().unwrap_or(0));
    }
    peak
}

/// The run's dataset session (#91), opened on the first frame recorded.
#[derive(Default)]
struct Dataset {
    recorder: Option<dataset::Recorder>,
    /// Opening it failed once: not tried again this run.
    failed: bool,
    next: u64,
    /// The readings since the last judging, with the crop shown on each:
    /// judged in hindsight (#91) when the input goes dark, at the end of the
    /// run, or every [`JUDGE_EVERY`] frames at a cut.
    pending: Vec<(cinefit_detect::readings::Reading, (u32, u32))>,
    /// Judging and closing sessions, off the play loop (#208).
    worker: Worker,
    /// The receiver's input by name, noted in each session (#116).
    source: Option<String>,
    /// What the Apple TV plays, by name, noted in each session (#115).
    playing: Option<String>,
    /// The input's HDR static metadata, noted in each session: a title's
    /// MaxCLL, MaxFALL and mastering peak, which its frames do not show.
    hdr: Option<HdrMetadata>,
    /// Which frames to keep in full (#91), from the first frame recorded.
    picker: Option<dataset::Picker>,
    /// While pairing (#138): the session's recent frames with their
    /// thumbnails, and every frame it kept, so a later pass can follow an
    /// earlier one from memory; the files on disk are written through
    /// buffers and lag.
    history: std::collections::VecDeque<(u64, Option<[u8; dataset::THUMB]>)>,
    kept: std::collections::BTreeSet<u64>,
    /// The `detect.pair_follow` in force and its follower.
    follow: Option<(String, Following)>,
}

/// Frames of history held while pairing: 45 minutes at 24p, about 38 MB.
/// Older frames are read back from the session's files (#160): a clips run
/// of 23 chapters and four passes plays its last pass more than an hour after
/// the tunnel's.
const HISTORY: usize = 1 << 16;

/// A follower, or the thumbnails it needs still being read from disk.
enum Following {
    /// `None` when there is nothing to follow (another session, frames not
    /// recorded), said why when it was made.
    Ready(Option<Box<pairfollow::Follower>>),
    Reading(std::sync::mpsc::Receiver<Option<Box<pairfollow::Follower>>>),
}

/// The signal gone this long ends a viewing: the dataset's session closes,
/// and the next frame opens a new one. Short drops (a player changing HDR or
/// rate at a title's start, 1-3 s) stay within one.
const VIEWING_GAP: Duration = Duration::from_secs(10);

/// The signal gone this long gives the GPU's clocks back to the driver's
/// default policy, which idles them: the slate and the menu are light work,
/// and the stable pstate otherwise holds the clocks up all night in standby.
/// Long past a player's HDR or rate change, so a viewing keeps its clocks;
/// the first frame back takes them again before it is rendered.
const CLOCKS_IDLE: Duration = Duration::from_secs(60);

/// About half an hour at 24p: a film with no break in its signal is judged
/// in pieces, each ending at a cut, so memory stays bounded.
const JUDGE_EVERY: usize = 24 * 60 * 30;

impl Dataset {
    /// Records one frame, when `on` (the `detect.record` setting).
    fn record(&mut self, on: bool, found: &Found, t: f64, epoch: u32, content: Content) {
        if !on || self.failed {
            return;
        }
        if self.recorder.is_none() {
            match dataset::default_root()
                .ok_or_else(|| std::io::Error::other("no HOME"))
                .and_then(|r| dataset::Recorder::start(dataset::session_dir(&r)))
            {
                Ok(r) => {
                    info!("dataset: recording to {}", r.dir().display());
                    // The splitter's EDID, which decides what the player
                    // sends: a pairing pass's label (#109). Up to two seconds
                    // over USB, so on a thread of its own.
                    let dir = r.dir().to_path_buf();
                    let _ = std::thread::Builder::new()
                        .name("splitter".into())
                        .spawn(move || {
                            if let Some(line) = crate::splitter::note() {
                                info!("dataset: {line}");
                                dataset::note_in(&dir, &line);
                            }
                        });
                    if let Some(s) = &self.source {
                        r.note(&format!("source: {s}"));
                    }
                    if let Some(p) = &self.playing {
                        r.note(&format!("playing: {p}"));
                    }
                    if let Some(h) = &self.hdr {
                        r.note(&hdr_line(h));
                    }
                    self.recorder = Some(r);
                }
                Err(e) => {
                    warn!("dataset: not recorded: {e}");
                    self.failed = true;
                    return;
                }
            }
        }
        let crop = match found.source {
            geo::Source::Rows { top, bottom } => (top, bottom),
            _ => (0, 0),
        };
        if let Some((obs, cut)) = found.reading {
            self.pending.push((
                cinefit_detect::readings::Reading {
                    index: self.next,
                    time: t,
                    epoch,
                    obs,
                    cut,
                },
                crop,
            ));
            if self.pending.len() >= JUDGE_EVERY {
                self.judge(false);
            }
        }
        if let Some(r) = self.recorder.as_mut() {
            r.record(dataset::Record {
                index: self.next,
                time: t,
                epoch,
                content: content.tag(),
                crop,
                reading: found.reading,
                thumb: found.thumb,
                cut: found.cut,
            });
            self.next += 1;
        }
    }
}

impl Dataset {
    /// Keeps the frame just recorded in full, when the picker says it
    /// teaches something (#91): the raw bytes, copied, written elsewhere.
    fn keep(&mut self, cfg: &Config, found: &Found, t: f64, frame: &Frame) {
        if !cfg.record || cfg.frames_gb == 0 || self.next == 0 {
            return;
        }
        if self.recorder.is_none() {
            return;
        }
        let index = self.next - 1;
        let pairing = cfg.pair_every > 0 || !cfg.pair_follow.is_empty();
        if pairing && self.history.back().is_none_or(|h| h.0 < index) {
            self.history.push_back((index, found.thumb));
            if self.history.len() > HISTORY {
                self.history.pop_front();
            }
        }
        let why = if cfg.pair_follow.is_empty() {
            self.follow = None;
            let picker = self.picker.get_or_insert_with(|| {
                let seed = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(1, |d| d.as_nanos() as u64);
                dataset::Picker::new(seed, t)
            });
            if cfg.pair_every > 0 {
                picker.pick_pairs(found.cut, cfg.pair_every)
            } else {
                picker.pick(t, found.change.is_some(), found.candidate)
            }
        } else {
            self.follow_step(&cfg.pair_follow, index, found.thumb.as_ref())
        };
        let Some(why) = why else {
            return;
        };
        if let (Some(r), Some(px)) = (self.recorder.as_mut(), frame.pixels()) {
            let budget = u64::from(cfg.frames_gb) << 30;
            r.keep(index, t, frame.info(), px.bytes(), why, budget);
            if pairing {
                self.kept.insert(index);
            }
        }
    }

    /// For the web page's state (#138): the open session and the index the
    /// next frame recorded gets, which does not move while recording is off.
    /// A pass runner reads its passes' frames here; the session's files are
    /// written through buffers and lag by seconds.
    fn state(&self) -> serde_json::Value {
        let session = self
            .recorder
            .as_ref()
            .and_then(|r| r.dir().file_name())
            .map(|n| n.to_string_lossy().into_owned());
        serde_json::json!({ "session": session, "next": self.next })
    }

    /// `detect.pair_follow` (#138) on this frame: the follower for `spec`,
    /// made when it changes, told the frame.
    fn follow_step(
        &mut self,
        spec: &str,
        index: u64,
        thumb: Option<&[u8; dataset::THUMB]>,
    ) -> Option<&'static str> {
        if self.follow.as_ref().is_none_or(|(s, _)| s != spec) {
            let f = self.follower(spec);
            self.follow = Some((spec.to_string(), f));
        }
        let following = &mut self.follow.as_mut()?.1;
        if let Following::Reading(rx) = following {
            // Frames go by unfollowed while the thumbnails are read, tens of
            // milliseconds: a runner sets the follow seconds before the
            // tunnel's first kept frame.
            match rx.try_recv() {
                Ok(f) => *following = Following::Ready(f),
                Err(std::sync::mpsc::TryRecvError::Empty) => return None,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    *following = Following::Ready(None)
                }
            }
        }
        let Following::Ready(Some(fl)) = following else {
            return None;
        };
        let s = fl.step(index, thumb);
        match s.event {
            Some(pairfollow::Event::Locked(d)) => {
                info!(
                    "pair follow: locked at frame {index}: this pass's frames are the tunnel pass's + {d}"
                );
            }
            Some(pairfollow::Event::Moved(e)) => {
                info!("pair follow: offset moved by {e} at frame {index}")
            }
            Some(pairfollow::Event::Lost) => {
                warn!("pair follow: lost at frame {index}: the pictures no longer match")
            }
            None => {}
        }
        s.keep.then_some("follow")
    }

    /// The follower `spec` asks for: from the history, or for frames older
    /// than it from the session's files, on a thread of its own so the frame
    /// loop does not wait on the disk.
    fn follower(&self, spec: &str) -> Following {
        let Some((lo, hi)) = self.follow_range(spec) else {
            return Following::Ready(None);
        };
        let kept = self.kept.range(lo..=hi).copied().collect();
        let dir = self.recorder.as_ref().map(|r| r.dir().to_path_buf());
        match (self.history.front(), dir) {
            (Some(&(front, _)), _) if lo >= front => {
                let held = |i: u64| {
                    self.history
                        .binary_search_by_key(&i, |h| h.0)
                        .ok()
                        .and_then(|k| self.history[k].1)
                };
                let thumbs: Vec<_> = (lo..=hi).map(held).collect();
                Following::Ready(make_follower(lo, hi, &thumbs, kept, "held").map(Box::new))
            }
            (_, Some(dir)) => {
                let (tx, rx) = std::sync::mpsc::sync_channel(1);
                let spawned = std::thread::Builder::new()
                    .name("pair-follow".into())
                    .spawn(move || {
                        let f = match dataset::read_thumbs(&dir, lo, hi) {
                            Ok(thumbs) => make_follower(lo, hi, &thumbs, kept, "on disk").map(Box::new),
                            Err(e) => {
                                warn!("pair follow: frames {lo}-{hi}: reading thumbnails: {e}: keeping nothing");
                                None
                            }
                        };
                        let _ = tx.send(f);
                    });
                match spawned {
                    Ok(_) => Following::Reading(rx),
                    Err(e) => {
                        warn!("pair follow: no thread to read thumbnails: {e}: keeping nothing");
                        Following::Ready(None)
                    }
                }
            }
            (_, None) => Following::Ready(None),
        }
    }

    /// The frames `spec` names, when it is well formed and names the open
    /// session: `None`, said why, otherwise.
    fn follow_range(&self, spec: &str) -> Option<(u64, u64)> {
        let open = self
            .recorder
            .as_ref()
            .and_then(|r| r.dir().file_name())
            .map(|n| n.to_string_lossy().into_owned());
        let range = spec.rsplit_once(':').and_then(|(session, range)| {
            let (lo, hi) = range.split_once('-')?;
            Some((
                session,
                lo.trim().parse::<u64>().ok()?,
                hi.trim().parse::<u64>().ok()?,
            ))
        });
        let Some((session, lo, hi)) = range.filter(|r| r.1 <= r.2) else {
            warn!("pair follow: {spec:?} is not SESSION:FIRST-LAST: keeping nothing");
            return None;
        };
        if hi - lo >= pairfollow::MAX_PASS as u64 {
            warn!(
                "pair follow: {spec}: {} frames, more than a pass ({} at most): keeping nothing",
                hi - lo + 1,
                pairfollow::MAX_PASS
            );
            return None;
        }
        if open.as_deref() != Some(session) {
            warn!(
                "pair follow: {spec} is not this session ({}): keeping nothing",
                open.as_deref().unwrap_or("none open")
            );
            return None;
        }
        Some((lo, hi))
    }

    /// The receiver's input changed: noted in the open session, and in
    /// every session opened after.
    fn set_source(&mut self, source: Option<String>, t: f64) {
        if source == self.source {
            return;
        }
        if let (Some(r), Some(s)) = (&self.recorder, &source) {
            r.note(&format!("at {t:.1} s, source: {s}"));
        }
        self.source = source;
    }

    /// The input's HDR metadata changed: noted in the open session, and in
    /// every session opened after.
    fn set_hdr(&mut self, hdr: Option<HdrMetadata>, t: f64) {
        if hdr == self.hdr {
            return;
        }
        if let (Some(r), Some(h)) = (&self.recorder, &hdr) {
            r.note(&format!("at {t:.1} s, {}", hdr_line(h)));
        }
        self.hdr = hdr;
    }

    /// What the Apple TV plays changed: noted in the open session, and in
    /// every session opened after.
    fn set_playing(&mut self, playing: Option<String>, t: f64) {
        if playing == self.playing {
            return;
        }
        if let (Some(r), Some(p)) = (&self.recorder, &playing) {
            r.note(&format!("at {t:.1} s, playing: {p}"));
        }
        self.playing = playing;
    }

    /// Judges the readings so far, on a thread of its own, appending to the
    /// session's `hindsight.csv` and logging a line. `all` false keeps the
    /// shot still going (from its cut) for next time.
    fn judge(&mut self, all: bool) {
        let Some(dir) = self.recorder.as_ref().map(|r| r.dir().to_path_buf()) else {
            self.pending.clear();
            return;
        };
        let keep_from = if all {
            self.pending.len()
        } else {
            self.pending.iter().rposition(|(r, _)| r.cut).unwrap_or(0)
        };
        if keep_from == 0 {
            return;
        }
        let rest = self.pending.split_off(keep_from);
        let batch = std::mem::replace(&mut self.pending, rest);
        // On the one worker, in order, so the file's shots stay in order.
        self.worker.run(Box::new(move || {
            match crate::hindsight::append(&dir, &batch) {
                Ok(summary) => info!("hindsight: {summary}"),
                Err(e) => warn!("hindsight: not written: {e}"),
            }
        }));
    }

    /// Ends the viewing: the last judging, then its session closed. The next
    /// frame recorded opens a new one.
    fn end_session(&mut self) {
        self.judge(true);
        self.next = 0;
        self.picker = None;
        // Frame numbers start again in the next session.
        self.history.clear();
        self.kept.clear();
        self.follow = None;
        // Closed after its last judging, on the same worker: flushing and
        // closing zstd takes as long as it takes, off the loop.
        if let Some(r) = self.recorder.take() {
            self.worker.run(Box::new(move || match r.finish() {
                Ok(s) => info!(
                    "dataset: {} frames recorded to {}, {} dropped; {} full frames kept, {} dropped",
                    s.frames,
                    s.dir.display(),
                    s.dropped,
                    s.kept,
                    s.kept_dropped
                ),
                Err(e) => warn!("dataset: writing failed: {e}"),
            }));
        }
    }
}

type Job = Box<dyn FnOnce() + Send>;

/// The dataset's background work on one thread, in the order it was handed
/// over (#208): judging a session's readings, then closing it. The play loop
/// hands each job over and goes on; only the end of the run waits, for what
/// is queued. Started with the first job, so a run without research capture
/// has no thread.
#[derive(Default)]
struct Worker {
    tx: Option<std::sync::mpsc::Sender<Job>>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// The thread could not be started: jobs run where they are handed over.
    inline: bool,
}

impl Worker {
    fn run(&mut self, job: Job) {
        if self.tx.is_none() && !self.inline {
            let (tx, rx) = std::sync::mpsc::channel::<Job>();
            match std::thread::Builder::new()
                .name("dataset".into())
                .spawn(move || rx.into_iter().for_each(caught))
            {
                Ok(t) => (self.tx, self.thread) = (Some(tx), Some(t)),
                Err(e) => {
                    warn!("dataset: no worker thread ({e}): judging and closing on the play loop");
                    self.inline = true;
                }
            }
        }
        match &self.tx {
            Some(tx) => {
                if let Err(std::sync::mpsc::SendError(job)) = tx.send(job) {
                    // The worker is gone: done here rather than lost.
                    caught(job);
                }
            }
            None => caught(job),
        }
    }
}

/// Runs a job, saying so if it panics: the next one still runs, and a job
/// run on the play loop does not take it down.
fn caught(job: Job) {
    if let Err(p) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)) {
        warn!("dataset: a job panicked: {}", panic_text(&*p));
    }
}

impl Drop for Worker {
    /// The end of the run: what is queued is finished first.
    fn drop(&mut self) {
        drop(self.tx.take());
        if let Some(t) = self.thread.take()
            && let Err(p) = t.join()
        {
            warn!("dataset: the worker panicked: {}", panic_text(&*p));
        }
    }
}

fn panic_text(p: &(dyn std::any::Any + Send)) -> &str {
    p.downcast_ref::<&str>()
        .copied()
        .or_else(|| p.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("(no message)")
}

/// A follower of frames `lo..=hi` from their thumbnails (`whence`, for the
/// log): `None`, said why, when none of them has one.
fn make_follower(
    lo: u64,
    hi: u64,
    thumbs: &[Option<[u8; dataset::THUMB]>],
    kept: std::collections::BTreeSet<u64>,
    whence: &str,
) -> Option<pairfollow::Follower> {
    let have = thumbs.iter().filter(|t| t.is_some()).count();
    if have == 0 {
        warn!("pair follow: no thumbnails {whence} for frames {lo}-{hi}: keeping nothing");
        return None;
    }
    let fl = pairfollow::Follower::new(lo, thumbs, kept);
    info!(
        "pair follow: following frames {lo}-{hi}: {have} thumbnails {whence} of {}, {} kept",
        thumbs.len(),
        fl.size().1
    );
    Some(fl)
}

impl CropFinder {
    /// The live `detect.crop` setting. Off, the detector sees no frames, so
    /// turned back on it starts over (#223): the crop and confidence it had
    /// belong to whatever was on before, not to what is on now.
    fn set_on(&mut self, on: bool) {
        if on && !self.on {
            self.det = Detector::default();
        }
        self.on = on;
    }

    fn find(
        &mut self,
        info: &FrameInfo,
        bytes: Option<&[u8]>,
        kind: Kind,
        t: f64,
        epoch: u32,
    ) -> Found {
        let v210 = bytes
            .filter(|_| matches!(kind, Kind::Ycbcr(_)))
            .and_then(|b| V210::new(b, info.width, info.height, info.row_bytes));
        let peak = v210.as_ref().map(peak_luma);
        let pq = matches!(kind, Kind::Ycbcr(c) if c.transfer == Transfer::Pq);
        let measure = |top, bottom| {
            v210.as_ref()
                .filter(|_| self.measure && pq)
                .and_then(|v| l1::measure(v, top, bottom))
        };
        // The tunnel: its bars for the detector, in v210's scale (#69), and
        // for the dataset its fingerprint and cuts, while recording (#109).
        let bars = bytes
            .filter(|_| matches!(kind, Kind::Tunnel))
            .and_then(|b| cinefit_detect::Tunnel::new(b, info.width, info.height, info.row_bytes))
            .map(cinefit_detect::TunnelBars);
        let tunnel = bytes
            .filter(|_| self.thumbs && matches!(kind, Kind::Tunnel))
            .and_then(|b| cinefit_detect::Tunnel::new(b, info.width, info.height, info.row_bytes));
        let thumb = match (&v210, &tunnel) {
            (Some(v), _) if self.thumbs => Some(dataset::thumbnail(v)),
            (_, Some(t)) => Some(dataset::thumbnail(t)),
            _ => None,
        };
        let tunnel_cut = tunnel.as_ref().is_some_and(|t| {
            if epoch != self.tunnel_epoch {
                self.tunnel_cuts.reset();
                self.tunnel_epoch = epoch;
            }
            self.tunnel_cuts.observe(t, 0, 0).cut
        });
        if !self.on {
            return Found {
                source: geo::Source::Whole,
                ms: None,
                change: None,
                peak,
                reading: None,
                thumb,
                candidate: false,
                cut: tunnel_cut,
                l1: measure(0, 0),
            };
        }
        // A crop belongs to the picture it was found on: held over RGB, a
        // scope title's crop would trim whatever followed it. RGB has no
        // adapter yet (#69), so it gets the full frame. The detector
        // itself starts again on the next YCbCr frame: a format change is a
        // new epoch, which resets it.
        if let Some(why) = unread_by_detector(kind) {
            let change = (!self.unread && self.det.crop() != Crop::default())
                .then_some((Crop::default(), why));
            if change.is_some() {
                self.crops += 1;
            }
            self.unread = true;
            return Found {
                source: geo::Source::Rows { top: 0, bottom: 0 },
                ms: None,
                change,
                peak,
                reading: None,
                thumb,
                candidate: false,
                cut: tunnel_cut,
                l1: None,
            };
        }
        self.unread = false;
        let (mut took, mut change, mut reading, mut candidate) = (None, None, None, false);
        let read: Option<&dyn cinefit_detect::Samples> = match (&v210, &bars) {
            (Some(v), _) => Some(v),
            (_, Some(b)) => Some(b),
            _ => None,
        };
        if let Some(v) = read {
            let t0 = Instant::now();
            let (d, obs, cut) = self.det.push_reading(v, t, epoch);
            took = Some(ms(t0));
            reading = Some((obs, cut));
            candidate = d.diag.candidate.is_some();
            if d.changed {
                self.crops += 1;
                change = Some((d.crop, d.diag.reason));
            }
        }
        let c = self.det.crop();
        Found {
            source: geo::Source::Rows {
                top: c.top,
                bottom: c.bottom,
            },
            ms: took,
            change,
            peak,
            cut: reading.is_some_and(|(_, cut)| cut),
            reading,
            thumb,
            candidate,
            l1: measure(c.top, c.bottom),
        }
    }
}

/// A frame's detection and upload, side by side: the detector reads the
/// captured bytes on a worker thread while this thread copies the same bytes
/// to the GPU and unpacks them. Both only read the frame, and only the draw
/// that follows needs the crop, so it still applies to this same frame:
/// nothing waits a frame, and the frame costs the longer of the two instead
/// of their sum. `at` is the frame's time and the detector's epoch. Returns
/// what the detector found and the frame ready to draw, if it was uploaded.
#[allow(clippy::too_many_arguments)]
fn detect_and_upload<'a, 'p>(
    pl: &'p Placebo,
    finder: &mut CropFinder,
    frame: &Frame,
    kind: Kind,
    lldv: bool,
    tunnel: Option<&'a Tunnel>,
    sources: &'a mut Sources<'p>,
    stages: &mut Stages,
    at: (f64, u32),
) -> Result<(Found, Option<Prepared<'a, 'p>>), Box<dyn Error>> {
    let info = *frame.info();
    let Some(pixels) = frame.pixels() else {
        return Ok((finder.find(&info, None, kind, at.0, at.1), None));
    };
    let bytes = pixels.bytes();
    let t = Instant::now();
    let (found, uploaded) = std::thread::scope(|s| {
        // A thread a frame: starting one costs tens of microseconds, against
        // the milliseconds the detector takes.
        let worker = s.spawn(|| finder.find(&info, Some(bytes), kind, at.0, at.1));
        let uploaded = upload(pl, &info, bytes, kind, lldv, tunnel, &mut *sources, stages);
        let found = worker
            .join()
            .unwrap_or_else(|e| std::panic::resume_unwind(e));
        (found, uploaded)
    });
    stages.detect_and_upload.push(ms(t));
    stages.detected(found.ms, kind);
    Ok((found, uploaded?))
}

/// The capture chain's chroma bias on low-latency Dolby Vision, in 10-bit
/// codes added back to Cb and Cr in the unpack: they arrive about 0.625 and
/// 0.40 of a code low on every film and target measured, against
/// independent decodes of the same frames (#146). Not Dolby's mapping, which
/// would vary with the target; the mechanism is not proven. Luma shows no
/// bias that can be told apart from Dolby's mapping, so it is left alone.
const LLDV_CHROMA_BIAS: [f32; 2] = [0.625, 0.40];

/// A GPU resource with a picture's size.
trait GpuSize {
    fn size(&self) -> (usize, usize);
}

impl GpuSize for GpuPlanes<'_> {
    fn size(&self) -> (usize, usize) {
        (self.width(), self.height())
    }
}

impl GpuSize for GpuRgb<'_> {
    fn size(&self) -> (usize, usize) {
        (self.width(), self.height())
    }
}

/// The resource in `slot`, made again (by `make`) when there is none or it
/// is another size: kept from frame to frame otherwise.
fn sized<T: GpuSize>(
    slot: &mut Option<T>,
    w: usize,
    h: usize,
    make: impl FnOnce() -> Result<T, cinefit_render::Error>,
) -> Result<&mut T, cinefit_render::Error> {
    if slot.as_ref().is_some_and(|r| r.size() != (w, h)) {
        *slot = None;
    }
    match slot {
        Some(r) => Ok(r),
        None => Ok(slot.insert(make()?)),
    }
}

/// The first half of a frame: its bytes into the GPU, unpacked into planes
/// (copy and unpack). Needs nothing from the detector, so it runs while the
/// detector reads the same bytes on another thread. `None` for what nothing
/// renders: a format with no path, or a tunnel before its first conversion.
/// `lldv` is the `input.lldv` setting.
#[allow(clippy::too_many_arguments)]
fn upload<'a, 'p>(
    pl: &'p Placebo,
    info: &FrameInfo,
    bytes: &[u8],
    kind: Kind,
    lldv: bool,
    tunnel: Option<&'a Tunnel>,
    sources: &'a mut Sources<'p>,
    stages: &mut Stages,
) -> Result<Option<Prepared<'a, 'p>>, Box<dyn Error>> {
    let (w, h) = (info.width as usize, info.height as usize);
    let t = Instant::now();
    let prepared = match kind {
        Kind::Tunnel => {
            let Some(tunnel) = tunnel else {
                return Ok(None);
            };
            let planes = sized(&mut sources.planes, w, h, || pl.planes(w, h))?;
            pl.unpack_tunnel(planes, bytes, info.row_bytes)?;
            Prepared::Tunnel { planes, tunnel }
        }
        Kind::Ycbcr(color) => {
            let lldv = matches!(Content::of(kind, lldv), Content::Lldv(_));
            let planes = sized(&mut sources.planes, w, h, || pl.planes(w, h))?;
            // The chain's bias on low-latency Dolby Vision only: the rest
            // has not been measured to carry it.
            let bias = lldv.then_some(LLDV_CHROMA_BIAS);
            pl.unpack_v210_biased(planes, bytes, info.row_bytes, bias)?;
            Prepared::Ycbcr {
                planes,
                color,
                lldv,
            }
        }
        Kind::Rgb(format) => {
            let rgb = sized(&mut sources.rgb, w, h, || pl.rgb(w, h))?;
            pl.unpack_rgb(rgb, bytes, info.row_bytes, format)?;
            Prepared::Rgb(rgb)
        }
        Kind::Unsupported(_) => return Ok(None),
    };
    stages.unpack.push(ms(t));
    Ok(Some(prepared))
}

/// The HDR10 source's peak for tone mapping: MaxCLL, or the mastering
/// display's peak without it. From the metadata in force for the frame
/// ([`carried_hdr`]), so it holds across frames without a block (a peak that
/// came and went would make the tone mapping flicker) and is forgotten with
/// it, so a new title is not mapped from the last one's peak (#195).
fn pq_peak(hdr: Option<&HdrMetadata>) -> Option<f32> {
    let h = hdr?;
    let p = h
        .max_cll
        .filter(|&v| v > 0.0)
        .or(h.max_mastering.filter(|&v| v > 0.0))?;
    Some(p as f32)
}

/// An HDR10 title's source range for Dolby's mapping, 12-bit PQ codes: its
/// mastering display's, or MaxCLL for the top; without either the common
/// 1000-nit grade, and 0.0001 nits at the bottom (#141).
fn hdr10_source(hdr: Option<&HdrMetadata>) -> [u16; 2] {
    let nits = |v: Option<f64>| v.filter(|&n| n > 0.0);
    let top = hdr
        .and_then(|h| nits(h.max_mastering).or(nits(h.max_cll)))
        .unwrap_or(1000.0);
    let bottom = hdr.and_then(|h| nits(h.min_mastering)).unwrap_or(0.0001);
    let code = |n: f64| (cinefit_render::dm::pq_oetf(n) * 4095.0).round() as u16;
    [code(bottom), code(top)]
}

/// The final render after Dolby's mapping: one to one at the mapped peak, as
/// the player's low-latency rendering is.
fn one_to_one(dst: &Target<'_, '_>) -> Mapping {
    Mapping {
        src_peak: dst.mapped_nits,
        dst_peak: dst.mapped_nits,
        curve: dst.curve,
        peak_detect: false,
        contrast_recovery: 0.0,
        peak_percentile: dst.peak_percentile,
        knee_offset: 0.0,
    }
}

/// The second half: the uploaded frame drawn into the target, with the crop
/// the detector found for this same frame. `hdr` is the HDR10 metadata in
/// force for it, carried over frames without a block; `hdr10_scene` the
/// shot's measured L1 for Dolby's mapping, with `output.mode` dolby; `dm` the
/// last mapping table, kept between frames.
#[allow(clippy::too_many_arguments)]
fn draw(
    pl: &Placebo,
    info: &FrameInfo,
    hdr: Option<&HdrMetadata>,
    frame: Prepared<'_, '_>,
    hdr10_scene: Option<cinefit_render::dm::Scene>,
    dm: &mut DmCache,
    dst: &Target<'_, '_>,
    stages: &mut Stages,
) -> Result<bool, Box<dyn Error>> {
    let view = dst.view;
    let out = dst.color;
    let t = Instant::now();
    // Whether the Dolby mapping built a new table for this frame.
    let mut new_table = false;
    match frame {
        Prepared::Ycbcr {
            planes: p,
            color,
            lldv,
        } => {
            // Low-latency Dolby Vision is PQ under its SDR label, already
            // mapped by the player to its peak.
            let (color, peak, white) = match lldv {
                true => (
                    YcbcrColor {
                        transfer: Transfer::Pq,
                        ..color
                    },
                    Some(dst.lldv_nits),
                    dst.lldv_nits,
                ),
                false => (
                    color,
                    dst.source_peak.or_else(|| {
                        (color.transfer == Transfer::Pq)
                            .then(|| pq_peak(hdr))
                            .flatten()
                    }),
                    dst.mapped_nits,
                ),
            };
            // HDR10 into SDR through Dolby's mapping, on the shot's measured
            // L1 and no trims, then one to one (#141).
            if let (false, Transfer::Pq, Transfer::Bt1886, Some(scene)) =
                (lldv, color.transfer, out.transfer, hdr10_scene)
            {
                let (frame, built) = dm_frame(dm, scene, dst.mapped_nits);
                new_table = built;
                pl.render_ycbcr_dm(p, color, frame, dst.tex, out, view, &one_to_one(dst))?;
                stages.render.push(ms(t));
                return Ok(new_table);
            }
            // HDR into SDR is tone-mapped; anything else only changes space.
            let map =
                (color.transfer == Transfer::Pq && out.transfer == Transfer::Bt1886).then(|| {
                    Mapping {
                        // An HDR10 disc without MaxCLL or mastering: 1000 nits,
                        // the common grade.
                        src_peak: peak.unwrap_or(1000.0),
                        dst_peak: white,
                        curve: dst.curve,
                        // Never low-latency Dolby Vision: the player has mapped
                        // it already and it passes one to one, which a measured
                        // peak below white_nits would turn into a mapping.
                        peak_detect: dst.peak_detect && !lldv,
                        // Likewise: one to one adds nothing back.
                        contrast_recovery: if lldv { 0.0 } else { dst.contrast_recovery },
                        peak_percentile: dst.peak_percentile,
                        knee_offset: if lldv { 0.0 } else { dst.knee_offset },
                    }
                });
            pl.render_ycbcr_mapped(p, color, dst.tex, out, view, map.as_ref())?;
        }
        Prepared::Tunnel { planes: p, tunnel } => {
            // Into SDR: Dolby's display mapping, modelled, from the frame's own
            // metadata, onto the target, then one to one into SDR as the
            // player's low-latency rendering is.
            if let (Transfer::Bt1886, Some(scene)) = (out.transfer, tunnel.dm_scene()) {
                let (frame, built) = dm_frame(dm, scene, dst.mapped_nits);
                new_table = built;
                let map = one_to_one(dst);
                pl.render_dovi_dm(p, &tunnel.color, frame, dst.tex, out, view, &map)?;
            } else {
                // No level 1: from the grade's own peak, with peak detection
                // to follow the scene.
                let map = (out.transfer == Transfer::Bt1886).then(|| Mapping {
                    src_peak: cinefit_dovi::pq_to_nits(tunnel.source_max_pq) as f32,
                    dst_peak: dst.mapped_nits,
                    curve: dst.curve,
                    peak_detect: dst.peak_detect,
                    contrast_recovery: dst.contrast_recovery,
                    peak_percentile: dst.peak_percentile,
                    knee_offset: dst.knee_offset,
                });
                pl.render_dovi_mapped(p, &tunnel.color, dst.tex, out, view, map.as_ref())?;
            }
        }
        Prepared::Rgb(r) => {
            // The card does not report RGB's range: the setting says (#23).
            let color = RgbSourceColor {
                levels: dst.rgb_range,
                primaries: ycbcr_color(info).primaries,
                transfer: ycbcr_color(info).transfer,
            };
            pl.render_rgb_view(r, color, dst.tex, out, view)?;
        }
    }
    stages.render.push(ms(t));
    Ok(new_table)
}

/// What the SDR output's white stands for with this content: low-latency
/// Dolby Vision's own peak, which passes one to one, else what HDR is mapped
/// into. A readout of the output in nits uses it.
fn white_for(content: Content, c: &Config) -> u16 {
    match content {
        Content::Lldv(_) => c.lldv_nits,
        _ => c.mapped_nits(),
    }
}

fn ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

/// Runs `play`, recorded in `~/cinefit-logs` (see [`runlog`]). A log that
/// cannot be opened is reported and the run goes on without one.
pub fn run(opts: Options) -> Result<(), Box<dyn Error>> {
    match runlog::init(runlog::default_dir().as_deref(), "play") {
        Ok(path) => info!("log: {}", path.display()),
        Err(e) => warn!("log: not written: {e}"),
    }
    info!("cinefit {}", crate::VERSION);
    // args_os: clap takes a path that is not UTF-8, which args() would panic on.
    let args: Vec<String> = std::env::args_os()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    log::debug!("{}", args.join(" "));
    // The settings as this run read them: a question about a run is often
    // which setting was on.
    log::debug!(
        "settings:\n{}",
        crate::config::render(&opts.config).trim_end()
    );
    let result = play(opts);
    if let Err(e) = &result {
        // main prints it to the terminal.
        log::error!(target: runlog::FILE_ONLY, "{}", crate::chain(&**e));
    }
    result
}

fn play(opts: Options) -> Result<(), Box<dyn Error>> {
    // The settings as they are now: the menu changes them mid-run, and what
    // reads them reads this, each time.
    let mut cfg = opts.config.clone();
    let (file_path, mut file_cfg) = opts.file.clone();
    let run_for = match opts.seconds {
        None => None,
        Some(s) => Some(
            Duration::try_from_secs_f64(s)
                .ok()
                .filter(|d| !d.is_zero())
                .ok_or("--seconds must be a positive number")?,
        ),
    };

    // Capture first: the output is chosen from what arrives.
    // The input by its persistent ID (#68), so a card with several inputs
    // cannot quietly swap one for another.
    let devices = cinefit_decklink::devices()?;
    let infos: Vec<_> = devices.iter().map(|d| d.info()).collect();
    let index = crate::device::choose(
        &infos,
        crate::device::want(opts.device, &opts.config.input_device)?,
    )?;
    let device = &devices[index];
    let d = &infos[index];
    info!(
        "capture: device {index}: {} ({}), persistent ID {}",
        d.display_name,
        d.model_name,
        d.persistent_id
            .map_or_else(|| "unreported".into(), |id| format!("{id:#x}"))
    );
    let mailbox = Arc::new(Mailbox::new());
    let format = Arc::new(Mutex::new(None));
    let (captured, no_input) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)));
    let capture = device.start_capture(Feed {
        mailbox: Arc::clone(&mailbox),
        format: Arc::clone(&format),
        captured: Arc::clone(&captured),
        no_input: Arc::clone(&no_input),
    })?;

    // Pre-roll: wait for the input to hold still.
    let mut last_tunnel = None;
    let started = Instant::now();
    let (mut key, mut stable, mut first) = (None, 0usize, None);
    // The format report can trail the first frames, so it is waited for too.
    let has_format = || {
        format
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    };
    while stable < key.map_or(STABLE_FRAMES, frames_to_trust) || !has_format() {
        if started.elapsed() > PRE_ROLL_WAIT {
            break;
        }
        let Some(frame) = mailbox.take_wait(Duration::from_millis(100)) else {
            continue;
        };
        let k = classify(&frame, &mut last_tunnel);
        if Some(k) == key {
            stable += 1;
        } else {
            key = Some(k);
            stable = 1;
        }
        first = Some(frame);
    }
    let reported = *format.lock().unwrap_or_else(PoisonError::into_inner);
    // The source to start from, if one held still: without one the output
    // starts at output.wait_hz with black, and follows the source when it comes.
    let start = match (key, first, reported) {
        (Some(key), Some(first), Some(input))
            if stable >= frames_to_trust(key) && !matches!(key.kind, Kind::Unsupported(_)) =>
        {
            info!(
                "input: {input}, as {}; stable after {:.1} s",
                describe(key.kind),
                started.elapsed().as_secs_f64()
            );
            Some((key, first, input))
        }
        (key, _, reported) => {
            let what = match (key, reported) {
                (
                    Some(Key {
                        kind: Kind::Unsupported(f),
                        ..
                    }),
                    _,
                ) => format!("{f}, which nothing renders"),
                (Some(k), Some(f)) => format!("{f}, as {}, not yet steady", describe(k.kind)),
                _ => format!(
                    "no signal ({} frames with one, {} without)",
                    captured.load(Ordering::Relaxed),
                    no_input.load(Ordering::Relaxed)
                ),
            };
            if opts.dry_run {
                return Err(format!(
                    "no stable input after {} s: {what}",
                    PRE_ROLL_WAIT.as_secs()
                )
                .into());
            }
            info!(
                "input after {} s: {what}; starting with black, to follow the source when it comes",
                PRE_ROLL_WAIT.as_secs()
            );
            None
        }
    };

    // The output: the panel at the source's own rate, signalling chosen from
    // the source, in one commit.
    let mut out = wait_for_display(opts.card.as_deref(), opts.dry_run)?;
    info!("output: {}", out.describe());
    let want = cinefit_kms::ModeRequest {
        width: cfg.width,
        height: cfg.height,
        refresh_hz: start
            .as_ref()
            .map_or(cfg.wait_hz, |s| s.2.frames_per_second()),
    };
    let (modes, index) = wait_for_mode(&mut out, &want, opts.dry_run)?;
    let mode = modes[index];
    info!("mode: {mode}");
    // What the first input is, resolved once (#99).
    let start_content = start.as_ref().map(|s| Content::of(s.0.kind, cfg.lldv));
    let start_hdr = start.as_ref().and_then(|s| s.1.info().color.hdr);
    let (hdr, mut out_color) = match (&start, start_content) {
        (Some((_, first, _)), Some(content)) => output_in(
            cfg.mode,
            content,
            first.info().color.hdr.as_ref(),
            last_tunnel.as_ref(),
        ),
        _ => (None, SDR_BT2020),
    };
    match &hdr {
        Some(h) => info!(
            "signalling {:?}: MaxCLL {}, MaxFALL {}, mastering {:.4}-{:.0} nits",
            h.eotf, h.max_cll, h.max_fall, h.min_mastering, h.max_mastering
        ),
        None => info!(
            "signalling SDR BT.2020{}",
            match start_content {
                None => ", no source".to_string(),
                Some(Content::Lldv(_)) => format!(
                    ": low-latency Dolby Vision, passed through at {} nits",
                    cfg.lldv_nits
                ),
                Some(Content::Tunnel | Content::Hdr10(_)) => format!(
                    ": HDR tone-mapped ({}) to white at {} nits (screen peak {})",
                    cfg.curve.name(),
                    cfg.mapped_nits(),
                    cfg.peak_nits
                ),
                _ => String::new(),
            }
        ),
    }
    let size = (u32::from(mode.width), u32::from(mode.height));

    let mut clocks = if opts.pstate {
        match cinefit_render::power::StablePstate::acquire() {
            Ok(p) => {
                info!(
                    "gpu clocks: {} (stable pstate, held while there is a signal)",
                    p.level()
                );
                Some(p)
            }
            Err(e) => {
                info!("gpu clocks: not held: {e}");
                None
            }
        }
    } else {
        info!("gpu clocks: the driver's default policy");
        None
    };
    // Only a hold taken at startup is given back and taken again: one refused
    // then (another holder), or not asked for, stays as it is for the run.
    let holds_clocks = clocks.is_some();

    let pl = Placebo::new()?;
    let gpu_timers = opts.gpu_timers && pl.set_gpu_timing(true);
    if opts.gpu_timers {
        info!(
            "gpu timers: {}",
            if gpu_timers {
                "on"
            } else {
                "not available on this GPU"
            }
        );
    }
    // The scanout buffers in the layout GBM picks from the plane's list
    // (#101): tiled where the GPU has one, which `cinefit bench` measured
    // 20% faster to render into than LINEAR on the RX 6400 and the same
    // picture to the bit. A layout the renderer will not import, or any other
    // failure, falls back to LINEAR, as before.
    let tiled = if opts.linear_scanout {
        None
    } else {
        out.scanout_modifiers()
            .ok()
            .filter(|m| m.iter().any(|&m| m != 0))
    };
    // GBM picks from the list; a layout that cannot be used (several
    // planes, refused by the renderer, or rendering wrongly) is taken out
    // and GBM asked again, down to LINEAR. A failure no layout can be blamed
    // for goes straight to LINEAR.
    let before = out.scanout_count();
    let mut mods = tiled.unwrap_or_else(|| vec![0]);
    let made = loop {
        let attempt = scanouts(&mut out, &pl, index, &mods).and_then(|s| {
            // Imported is not proof it renders: a test picture through each
            // buffer, read back, must come out right. On a GPU where a tiled
            // render failed or scrambled, play would otherwise fail at every
            // restart, or show a scrambled picture.
            for b in &s.buffers {
                renders_right(&out, &pl, b.frame, &b.texture).map_err(|e| Failed {
                    layout: Some(s.modifier),
                    error: e,
                })?;
            }
            Ok(s)
        });
        match attempt {
            Ok(s) => break Ok(s),
            Err(f) if mods == [0] => break Err(f.error),
            Err(f) => {
                out.discard_scanouts(before);
                match f.layout.filter(|&m| m != 0) {
                    Some(m) => {
                        info!(
                            "scanout: {} not usable ({}); asking for another",
                            cinefit_kms::modifier_name(m),
                            crate::chain(&*f.error)
                        );
                        mods.retain(|&x| x != m);
                        if mods.iter().all(|&x| x == 0) {
                            mods = vec![0];
                        }
                    }
                    None => {
                        warn!(
                            "scanout: the tiled layouts failed ({}); LINEAR, as before",
                            crate::chain(&*f.error)
                        );
                        mods = vec![0];
                    }
                }
            }
        }
    };
    let scan = made?;
    let (scan_fourcc, modifier) = (scan.fourcc, scan.modifier);
    info!(
        "scanout: {} {}{}",
        String::from_utf8_lossy(&scan_fourcc.to_le_bytes()),
        cinefit_kms::modifier_name(modifier),
        if opts.linear_scanout {
            " (--linear-scanout)"
        } else {
            ""
        }
    );

    // The first picture is rendered before the modeset, so the display's
    // first frame is the source's.
    let mut sources = Sources::default();
    let mut dm_cache: DmCache = None;
    // HDR10 through Dolby's mapping (#141): the shot's L1 so far, and the
    // epoch it belongs to.
    let mut shot_l1 = l1::Shot::default();
    let mut shot_epoch = 0;
    let mut stages = Stages::default();
    // Each frame's tag for its GPU runs, and the results taken after it.
    let mut frame_tag = 0u64;
    let mut gpu_results = Vec::new();

    // The bar detector reads each YCbCr frame on this thread (about 8 ms at
    // 2160p) and its crop goes to the geometry: the picture at constant
    // image height in the screen's band of the raster. Other formats, which
    // have no adapter yet, get the full frame.
    let mut win = geo::screen_window(size.0, size.1, cfg.screen_ar.value());
    if cfg.crop {
        info!(
            "crop: on; screen {:.2}:1 is rows {}-{} of the raster, fill up to {:.2}:1",
            cfg.screen_ar,
            win.y,
            win.y + win.h,
            cfg.fill_max_ar
        );
    }
    let mut finder = CropFinder {
        det: Detector::default(),
        on: cfg.crop,
        crops: 0,
        unread: false,
        thumbs: cfg.record,
        tunnel_cuts: Default::default(),
        tunnel_epoch: 0,
        measure: cfg.mode == config::OutputMode::Dolby,
    };
    // The detector's dataset (#91): a session directory, opened on the first
    // frame recorded, so a run with nothing coming in leaves none.
    let mut dataset = Dataset::default();
    // For the info panel: the last ten seconds' timings, and when and why
    // the crop last changed.
    let window = RefCell::new(Window::default());
    let crop_change: Cell<Option<(Instant, &'static str)>> = Cell::new(None);
    // What the detector found on a frame: its time into the panel's window,
    // and a crop change into the log.
    let noted = |found: &Found, info: &FrameInfo, t: f64| {
        if let Some(d) = found.ms {
            window.borrow_mut().detect(Instant::now(), d);
        }
        if let Some(p) = found.peak {
            window.borrow_mut().peak(Instant::now(), p);
        }
        if let Some((c, reason)) = found.change {
            crop_change.set(Some((Instant::now(), reason)));
            info!(
                "{t:.1} s: crop {}/{} ({reason}): picture {} x {} rows, {:.3}:1",
                c.top,
                c.bottom,
                info.width,
                c.active(info.height),
                f64::from(info.width) / f64::from(c.active(info.height).max(1))
            );
        }
    };
    let (first_source, first_place) = match &start {
        Some((key, first, _)) => {
            let first_info = *first.info();
            let (found, prepared) = detect_and_upload(
                &pl,
                &mut finder,
                first,
                key.kind,
                cfg.lldv,
                last_tunnel.as_ref(),
                &mut sources,
                &mut stages,
                (0.0, 1),
            )?;
            noted(&found, &first_info, 0.0);
            // Recorded too: the policy saw it, so a replay of the readings
            // must see it as well to reach the same crops.
            if let Some(content) = start_content {
                dataset.record(cfg.record, &found, 0.0, 1, content);
            }
            let source = found.source;
            let view = view_for(
                (first_info.width, first_info.height),
                size,
                win,
                source,
                cfg.fill_max_ar.value(),
            );
            let Some(prepared) = prepared else {
                return Err("the first frame could not be rendered".into());
            };
            // The first frame alone is its shot so far.
            let scene = found.l1.map(|f| {
                cinefit_render::dm::Scene::from_codes(
                    l1::Shot::default().push(f, true),
                    hdr10_source(first_info.color.hdr.as_ref()),
                    vec![],
                )
            });
            draw(
                &pl,
                &first_info,
                first_info.color.hdr.as_ref(),
                prepared,
                scene,
                &mut dm_cache,
                &target(scan.texture(0), out_color, view, &cfg),
                &mut stages,
            )?;
            let place = view
                .dst
                .unwrap_or_else(|| Rect::fit(first_info.width, first_info.height, size.0, size.1));
            (source, place)
        }
        None => {
            pl.clear_black(scan.texture(0))?;
            let none = if cfg.crop {
                geo::Source::Rows { top: 0, bottom: 0 }
            } else {
                geo::Source::Whole
            };
            (none, Rect::fit(size.0, size.1, size.0, size.1))
        }
    };
    let (start_key, start_input) = match start {
        Some((key, first, input)) => {
            drop(first);
            (Some(key), Some(input))
        }
        None => (None, None),
    };
    if opts.dry_run {
        info!(
            "dry run: first frame rendered into a scanout buffer ({:.2} ms unpack, {:.2} ms render)",
            stages.unpack.first().unwrap_or(0.0),
            stages.render.first().unwrap_or(0.0)
        );
        // A signal is not a picture: look at the pixels.
        let fourcc = out.export(scan.frame(0))?.fourcc;
        let report = out.read_scanout(scan.frame(0), |px, stride| {
            look(px, stride, fourcc, size, first_place)
        })?;
        info!("{report}");
        info!("stopping before the modeset");
        return Ok(());
    }
    // The slate and banners on the overlay plane: up with the modeset, so
    // a run without a source opens on the slate.
    let mut ui = Ui::new(
        &mut out,
        index,
        size,
        (win.y, win.h),
        &cfg.venue,
        cfg.banners,
    );
    let mut enc = encoding_for(signal_of(hdr));
    // The remote and keyboards, read on their own thread.
    let keys = KeyReader::start(Path::new("/dev/input"), |line| info!("{line}"));
    // The menu, with the rates the display offers at the panel's size for
    // the rate while waiting.
    let mut rates: Vec<f64> = modes
        .iter()
        .filter(|m| !m.interlaced && (m.width, m.height) == (cfg.width, cfg.height))
        .map(|m| (m.refresh_hz() * 1000.0).round() / 1000.0)
        .collect();
    rates.sort_by(f64::total_cmp);
    rates.dedup();
    // The web page: its own thread, the port from the settings.
    let web = Web::start(cfg.web_port);
    // The receiver (#116): which input is on, and whether it is on at all.
    let receiver = avr::Receiver::start(&cfg.denon);
    // The Apple TV (#115): what it is playing, by name.
    let apple_tv = appletv::AppleTv::start(&cfg.atv_host);
    let mut last_playing: Option<String> = None;
    let mut last_seen: Option<avr::Seen> = None;
    let mut menu = Menu::new(rates, crate::VERSION.split(' ').next().unwrap_or("?"));
    // A rate-while-waiting change made while waiting: the output follows it
    // (a modeset) until a source comes.
    let mut wait_changed = false;
    let mut info_at: Option<Instant> = None;
    let mut read_back_logged = false;
    let mut cur_mode = mode;
    let now = Instant::now();
    match (&start_key, &start_input) {
        (Some(key), Some(input)) => {
            if let Some(b) = banner_for(
                Content::of(key.kind, opts.config.lldv),
                (key.width, key.height),
                rate_mhz(input),
                size.0,
                &opts.config,
            ) {
                ui.picture(b, true, now);
            }
            ui.resynced(now);
        }
        _ => ui.set_slate(Some(match key {
            Some(Key {
                kind: Kind::Unsupported(f),
                ..
            }) => Slate::CantShow {
                detail: cant_show(f),
            },
            _ => Slate::NoSignal,
        })),
    }
    ui.update(&mut out, enc, now);
    out.set_hdr(hdr);
    out.set_sdr_bt2020(true);
    out.modeset(index, scan.frame(0))?;
    info!("after modeset: {}", out.hdr_state());
    let mut policy = follow::Policy::new(follow::Want {
        rate_mhz: start_input
            .as_ref()
            .map_or_else(|| (mode.refresh_hz() * 1000.0).round() as u32, rate_mhz),
        signal: signal_of(hdr),
    });

    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let mut presses = 0;
    ctrlc::set_handler(move || {
        presses += 1;
        // A second press is the way out of a run that hangs, so it exits on
        // the spot; the log says so, rather than just stopping.
        if presses > 1 {
            warn!("second Ctrl-C or SIGTERM: exiting at once, without the summary");
            std::process::exit(130);
        }
        flag.store(true, Ordering::Relaxed);
    })?;

    // The loop: with nothing rendered yet, wait a little for a frame and
    // render it into a free buffer; then wait for the vblank and queue
    // whatever is ready, or the same buffer again so the vblanks keep coming.
    let mut period = mode.period();
    let mut take_timeout = period.mul_f64(0.8).min(Duration::from_millis(35));
    let mut pacing = cinefit_kms::Pacing::default();
    // What was going on at the frame ready to show, and at the one flipped.
    let (mut ready_situation, mut flip_situation) = (Situation::Steady, Situation::Steady);
    // Pacing per output mode: a modeset changes the period it is judged by.
    let mut finished: Vec<(String, Duration, cinefit_kms::Pacing)> = Vec::new();
    let mut mode_name = mode.to_string();
    let mut latest_want: Option<follow::Want> = None;
    // The HDR10 metadata carried over frames that come without it (#61),
    // from the first frame on: its block may be the last for a while, and
    // the output and its tone mapping were chosen from it.
    let mut pq_meta = None;
    if let Some(content) = start_content {
        carried_hdr(content, start_hdr, &mut pq_meta);
    }
    let (mut front, mut pending) = (0usize, 0usize);
    ui.update(&mut out, enc, Instant::now());
    out.flip(scan.frame(front))?;
    let mut ready: Option<usize> = None;
    let mut counts = Counts::default();
    // The input as last seen, and for how many frames in a row.
    let (mut last_key, mut held) = (start_key, start_key.map_or(0, frames_to_trust));
    let mut settled: Option<(Counts, u64)> = None;
    // What the card last reported, to log when it reports something else.
    let mut last_format = start_input;
    // Black is on screen for want of a signal, and since when frames last
    // came. Each return of the signal is a new epoch for the detector.
    let (mut dark, mut last_frame_at) = (start_key.is_none(), Instant::now());
    // From the startup frame's rate: a change before the loop's first frame
    // is a change, not the first report.
    let mut epochs = Epochs::default();
    if let Some(f) = start_input {
        epochs.rate(rate_mhz(&f));
    }
    // The output stopped for want of a signal (`output.idle_off`, #144), and
    // since when; when it last came on again, which starts the wait over (the
    // menu wakes it with no source); whether stopping it was refused, not to
    // be tried again until the signal has come and gone.
    let (mut asleep, mut woke_at, mut sleep_refused) = (None::<Instant>, None::<Instant>, false);
    // Since when the receiver has said it is in standby (#191). A source can
    // still come through it (the Denon's HDMI pass-through), but with no sound
    // nobody watches, so for output.idle_off it counts as no signal.
    let mut standby_since: Option<Instant> = None;
    let mut next_status = STATUS_EVERY;
    let mut last_source = first_source;
    // Save frame (#92): asked for, and the writer's answers, written off the
    // frame loop.
    let mut save_wanted = false;
    let (saved_tx, saved_rx) = std::sync::mpsc::channel::<Result<PathBuf, String>>();
    let mut last_saved: Option<String> = None;
    let run_started = Instant::now();
    // The loop's errors are held until the summary is written: a run that
    // fails at minute 40 needs its summary more than one that ends on time.
    let mut run_loop = || -> Result<(), Box<dyn Error>> {
        while run_for.is_none_or(|d| run_started.elapsed() < d) && !stop.load(Ordering::Relaxed) {
            if settled.is_none() && run_started.elapsed() >= SETTLE {
                settled = Some((counts, mailbox.displaced()));
            }
            if run_started.elapsed() >= next_status {
                next_status += STATUS_EVERY;
                // The input's brightest of the last ten seconds against what
                // went out on this frame: whether the renderer passes it
                // through or maps it down, without the panel open.
                let levels = {
                    let brightest = window.borrow_mut().brightest(Instant::now());
                    let shown = out
                        .read_scanout(scan.frame(front), |px, stride| {
                            scanout_peak(px, stride, scan_fourcc, size)
                        })
                        .ok()
                        .flatten();
                    let signal = policy.committed().signal;
                    match (last_key.filter(|_| !dark), brightest) {
                        (Some(k), Some(b)) => format!(
                            "; in {}, out {}",
                            peak_words(
                                Content::of(k.kind, cfg.lldv),
                                b,
                                cfg.lldv.then_some(cfg.lldv_nits)
                            ),
                            shown
                                .and_then(|v| {
                                    out_nits(
                                        v,
                                        signal,
                                        white_for(Content::of(k.kind, cfg.lldv), &cfg),
                                    )
                                })
                                .map_or_else(|| "-".into(), |n| format!("brightest {n:.0} nits"))
                        ),
                        _ => String::new(),
                    }
                };
                info!(
                    "{:.0} s: shown {}, vblanks repeated {}, replaced unseen {}, unrendered {}; crop {}{levels}",
                    run_started.elapsed().as_secs_f64(),
                    counts.shown,
                    counts.repeats,
                    mailbox.displaced(),
                    counts.unrendered,
                    match last_source {
                        geo::Source::Rows { top, bottom } => format!("{top}/{bottom}"),
                        _ => "off".into(),
                    }
                );
            }
            // Setting changes from the menu and the web page, applied below
            // the same way.
            let mut changes: Vec<(&'static str, config::Value, &str)> = Vec::new();
            for a in keys.drain() {
                info!(
                    "{:.1} s: key {}",
                    run_started.elapsed().as_secs_f64(),
                    a.name()
                );
                // While the menu is open it takes every key but Info.
                if a == Action::Menu || (menu.is_open() && a != Action::Info) {
                    if let Some((key, value)) = menu.key(a, &cfg, Instant::now()) {
                        changes.push((key, value, "menu"));
                    }
                    continue;
                }
                match a {
                    Action::Info => ui.toggle_info(Instant::now()),
                    Action::Back => ui.close_info(),
                    Action::Save => save_wanted = true,
                    _ => {}
                }
            }
            if let Some(w) = &web {
                save_wanted |= w.take_save();
                for (key, text) in w.take_changes() {
                    // Checked by the server already; a race with another
                    // change can still make one fail, which is logged.
                    match config::find(&key).map(|s| (s.key, s.value_of(&text))) {
                        Some((k, Ok(v))) => changes.push((k, v, "web")),
                        _ => warn!("web: {key} = {text} not understood"),
                    }
                }
            }
            for (key, value, from) in changes {
                let setting = config::find(key).expect("changes name settings");
                if let Err(e) = setting.put(&mut cfg, &value) {
                    warn!("{from}: {key} = {value} refused: {e}");
                    continue;
                }
                // Saved over the file as it is, without --set.
                let _ = setting.put(&mut file_cfg, &value);
                let saved = match config::save(&file_cfg, &file_path) {
                    Ok(()) => "saved".to_string(),
                    Err(e) => format!("not saved: {e}"),
                };
                info!(
                    "{:.1} s: {from}: {key} = {value} ({}, {saved})",
                    run_started.elapsed().as_secs_f64(),
                    setting.apply
                );
                // What reads it each frame needs nothing; the rest is told
                // here.
                match key {
                    "detect.crop" => finder.set_on(cfg.crop),
                    "detect.record" => finder.thumbs = cfg.record,
                    "output.mode" => finder.measure = cfg.mode == config::OutputMode::Dolby,
                    "geometry.screen_ar" => {
                        win = geo::screen_window(size.0, size.1, cfg.screen_ar.value());
                        ui.set_band((win.y, win.h));
                    }
                    "osd.banners" => ui.set_banners(cfg.banners),
                    "slate.venue" => ui.set_venue(&cfg.venue),
                    "avr.denon" => receiver.set_host(&cfg.denon),
                    "atv.host" => apple_tv.set_host(&cfg.atv_host),
                    "output.wait_hz" => wait_changed = dark,
                    _ => {}
                }
            }
            ui.set_menu(menu.view(&cfg, Instant::now()));
            // Waiting, with the rate changed: ask for it on every pass, as
            // the policy wants a change held before it modesets.
            if wait_changed && dark {
                latest_want = Some(follow::Want {
                    rate_mhz: (cfg.wait_hz * 1000.0).round() as u32,
                    signal: follow::Signal::Sdr,
                });
            }
            if !ui.info_open() && web.is_none() {
                info_at = None;
            } else if info_at.is_none_or(|t| t.elapsed() >= Duration::from_secs(1)) {
                let now = Instant::now();
                info_at = Some(now);
                let rows = cfg.crop.then_some(match last_source {
                    geo::Source::Rows { top, bottom } => (top, bottom),
                    _ => (0, 0),
                });
                // Read first: the window is borrowed again for LAST 10 S
                // below, and two borrows in one statement panic.
                let brightest = window.borrow_mut().brightest(now);
                // The frame on screen, read back: what the projector is
                // sent. A mapping of the buffer, so timed and logged once.
                let read_at = Instant::now();
                // Only while the panel is open: mapping the output buffer and
                // reading its grid takes about 2 ms on the RX 9060 XT (5.5 to
                // 9.7 ms on the RX 6400), as logged below (#203).
                let shown = if ui.info_open() {
                    out.read_scanout(scan.frame(front), |px, stride| {
                        scanout_peak(px, stride, scan_fourcc, size)
                    })
                    .ok()
                    .flatten()
                } else {
                    None
                };
                if ui.info_open() && !read_back_logged {
                    read_back_logged = true;
                    info!(
                        "osd: the frame on screen read back for the panel in {:.1} ms",
                        ms(read_at)
                    );
                }
                // What is coming in, resolved once for every section.
                let picture = last_key
                    .filter(|_| !dark)
                    .map(|k| (Content::of(k.kind, cfg.lldv), k));
                let mut text = InfoText {
                    sections: vec![
                        input_section(
                            picture.map(|(c, k)| (c, k.width, k.height)),
                            last_format,
                            cfg.lldv.then_some(cfg.lldv_nits),
                            brightest,
                        ),
                        output_section(
                            picture.map(|(c, _)| c),
                            policy.committed().signal,
                            cur_mode,
                            shown,
                            picture.map_or(cfg.mapped_nits(), |(c, _)| white_for(c, &cfg)),
                        ),
                        status::crop_section(
                            last_key.filter(|_| !dark).map(|k| (k.width, k.height)),
                            rows,
                            crop_change.get().map(|(at, why)| (now - at, why)),
                            last_key
                                .filter(|_| !dark)
                                .and_then(|k| unread_by_detector(k.kind)),
                        ),
                        window.borrow_mut().section(now, period),
                    ],
                };
                // What the Apple TV plays, under what comes in, when its
                // app says.
                if let Some(sec) = apple_tv
                    .now()
                    .filter(|_| appletv::on_screen(&cfg.atv_input, last_seen.as_ref()))
                    .and_then(|np| appletv::section(&np, std::time::SystemTime::now()))
                {
                    text.sections.insert(1, sec);
                }
                // Stopped (#144): seen on the web page only, as nothing
                // reaches the screen.
                if asleep.is_some()
                    && let Some(o) = text.sections.iter_mut().find(|s| s.label == "OUT")
                {
                    o.line = "stopped, for the display to sleep".into();
                    o.detail = match (dark, standby_since) {
                        (false, Some(t)) => {
                            format!(
                                "the receiver in standby for {} min",
                                t.elapsed().as_secs() / 60
                            )
                        }
                        _ => format!(
                            "no signal for {} min",
                            last_frame_at.elapsed().as_secs() / 60
                        ),
                    };
                }
                // The receiver's input, first on the IN line's detail.
                if let Some(s) = &last_seen {
                    let first = &mut text.sections[0];
                    first.detail = if s.on {
                        format!("{} \u{b7} {}", s.source(), first.detail)
                    } else {
                        "the receiver is off".into()
                    };
                }
                if let Some(w) = &web {
                    let pill = match last_key.filter(|_| !dark).map(|k| k.kind) {
                        // Stopped with a source still coming through the
                        // receiver in standby, as well as with no signal.
                        _ if asleep.is_some() => "Output stopped",
                        None => "Waiting",
                        Some(Kind::Unsupported(_)) => "Can't show this",
                        Some(_) => "Playing",
                    };
                    let now_rows: Vec<serde_json::Value> = text
                        .sections
                        .iter()
                        .map(|s| serde_json::json!({"label": s.label, "line": s.line, "detail": s.detail}))
                        .collect();
                    w.publish(
                        serde_json::json!({
                            "version": crate::VERSION,
                            "pill": pill,
                            "now": now_rows,
                            "settings": crate::web::settings_json(&cfg),
                            "dataset": dataset.state(),
                            "saved": last_saved,
                        }),
                        &cfg,
                    );
                }
                ui.set_info(text);
            }
            for r in saved_rx.try_iter() {
                let t = run_started.elapsed().as_secs_f64();
                let said = match r {
                    Ok(dir) => {
                        info!("{t:.1} s: frame saved to {}", dir.display());
                        ui.picture(
                            crate::osd::BannerText {
                                title: "Frame saved".into(),
                                detail: dir
                                    .file_name()
                                    .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
                            },
                            false,
                            Instant::now(),
                        );
                        format!("Saved to {}", dir.display())
                    }
                    Err(e) => {
                        warn!("{t:.1} s: frame not saved: {e}");
                        format!("Not saved: {e}")
                    }
                };
                last_saved = Some(said);
            }
            let taken = if ready.is_none() {
                mailbox.take_wait(take_timeout)
            } else {
                None
            };
            if taken.is_none() && ready.is_none() && !dark && last_frame_at.elapsed() >= NO_SIGNAL {
                // The signal is gone: black, in the mode as it stands. A blip
                // costs no resync; the source is followed when it is back.
                let free = (0..BUFFERS)
                    .find(|&b| b != front && b != pending)
                    .expect("three buffers, two in use");
                pl.clear_black(scan.texture(free))?;
                ready = Some(free);
                dark = true;
                // The next title after a break may send its block late: it
                // must not go out with the last one's values meanwhile (#61).
                pq_meta = None;
                // Nor a tunnel with the last title's conversion and mapping:
                // until its first good packet it goes out black (#213).
                last_tunnel = None;
                counts.signal_losses += 1;
                receiver.nudge();
                // A break in the input: the viewing so far is judged.
                dataset.judge(true);
                ui.set_slate(Some(if last_seen.as_ref().is_some_and(|s| !s.on) {
                    Slate::ReceiverOff
                } else {
                    Slate::SignalLost {
                        since: last_frame_at,
                    }
                }));
                info!(
                    "{:.1} s: no signal for {} ms: black",
                    run_started.elapsed().as_secs_f64(),
                    last_frame_at.elapsed().as_millis()
                );
            }
            // What the receiver says, when it changes: logged, noted in the
            // dataset, and on the slate while there is no picture.
            // What the Apple TV plays, when it changes: logged, and noted
            // in the dataset; while the receiver is on another input,
            // nothing is.
            let playing = apple_tv
                .now()
                .filter(|_| appletv::on_screen(&cfg.atv_input, last_seen.as_ref()))
                .as_ref()
                .and_then(appletv::label);
            if playing != last_playing {
                let t = run_started.elapsed().as_secs_f64();
                match &playing {
                    Some(p) => info!("{t:.1} s: Apple TV playing: {p}"),
                    None => info!("{t:.1} s: Apple TV: nothing named playing, or not on screen"),
                }
                dataset.set_playing(playing.clone(), t);
                last_playing = playing;
            }
            let seen = receiver.seen();
            if seen != last_seen {
                let t = run_started.elapsed().as_secs_f64();
                match &seen {
                    Some(s) if s.on => info!("{t:.1} s: receiver on, input {}", s.source()),
                    Some(_) => info!("{t:.1} s: receiver in standby"),
                    None if !cfg.denon.is_empty() => info!("{t:.1} s: receiver not answering"),
                    None => {}
                }
                let source = seen
                    .as_ref()
                    .filter(|s| s.on)
                    .map(|s| s.source().to_string());
                ui.set_source(source.clone());
                dataset.set_source(source, t);
                if dark {
                    ui.set_slate(Some(if seen.as_ref().is_some_and(|s| !s.on) {
                        Slate::ReceiverOff
                    } else {
                        Slate::SignalLost {
                            since: last_frame_at,
                        }
                    }));
                }
                standby_since = match &seen {
                    Some(s) if !s.on => standby_since.or_else(|| Some(Instant::now())),
                    _ => None,
                };
                // The receiver to standby: the viewing is over, now.
                if seen.as_ref().is_some_and(|s| !s.on) && dataset.recorder.is_some() {
                    info!("{t:.1} s: the receiver is off: the viewing ends");
                    dataset.end_session();
                }
                last_seen = seen;
            }
            if dark && dataset.recorder.is_some() && last_frame_at.elapsed() >= VIEWING_GAP {
                info!(
                    "{:.1} s: no signal for {} s: the viewing ends",
                    run_started.elapsed().as_secs_f64(),
                    VIEWING_GAP.as_secs()
                );
                dataset.end_session();
            }
            if taken.is_none() && dark && clocks.is_some() && last_frame_at.elapsed() >= CLOCKS_IDLE
            {
                let node = clocks.take().map(|p| p.node().to_owned());
                info!(
                    "{:.1} s: no signal for {} s: gpu clocks released ({})",
                    run_started.elapsed().as_secs_f64(),
                    CLOCKS_IDLE.as_secs(),
                    node.map(|n| cinefit_render::power::level_of(&n))
                        .unwrap_or_default()
                );
            }
            if let Some(frame) = taken {
                let came_back = dark;
                if dark {
                    epochs.next();
                    info!("{:.1} s: signal back", run_started.elapsed().as_secs_f64());
                    dark = false;
                    wait_changed = false;
                }
                if came_back && holds_clocks && clocks.is_none() {
                    // A failure here (another process took the pstate while
                    // it was free) is logged and the run goes on at the
                    // default policy; the next return tries again, not the
                    // next frame, which would retry and log 24 times a second.
                    match cinefit_render::power::StablePstate::acquire() {
                        Ok(p) => {
                            info!(
                                "{:.1} s: gpu clocks: {} again",
                                run_started.elapsed().as_secs_f64(),
                                p.level()
                            );
                            clocks = Some(p);
                        }
                        Err(e) => info!(
                            "{:.1} s: gpu clocks: not held again: {e}",
                            run_started.elapsed().as_secs_f64()
                        ),
                    }
                }
                last_frame_at = Instant::now();
                let k = classify(&frame, &mut last_tunnel);
                // What this frame is, resolved once for everything below.
                let content = Content::of(k.kind, cfg.lldv);
                if save_wanted && let Some(p) = frame.pixels() {
                    save_wanted = false;
                    let info = *frame.info();
                    let at = run_started.elapsed().as_secs_f64();
                    let save = snapshot::Save {
                        info,
                        // The one copy on this thread; the decoding and the
                        // writing are the writer's.
                        bytes: p.bytes().to_vec(),
                        at,
                        context: serde_json::json!({
                            "version": crate::VERSION,
                            "run_seconds": at,
                            "content": format!("{content:?}"),
                            "card": {
                                "eotf": info.color.eotf.map(|e| e.to_string()),
                                "colorspace": info.color.colorspace.map(|c| c.to_string()),
                                "dolby_vision_flagged": info.dolby_vision_flagged,
                                "dolby_vision_bytes": info.dolby_vision_bytes,
                            },
                            "hdr10": info.color.hdr.as_ref().map(snapshot::hdr),
                            // A frame without its own block is signalled
                            // with the last one carried over (#61): that,
                            // or null where nothing is carried.
                            "hdr10_carried": pq_meta
                                .as_ref()
                                .filter(|_| {
                                    matches!(content, Content::Hdr10(_))
                                        && info.color.hdr.is_none()
                                })
                                .map(snapshot::hdr),
                            // What low-latency Dolby Vision is taken to be,
                            // and what HDR10 and the tunnel are mapped into.
                            "lldv": {
                                "labelled_lldv": matches!(content, Content::Lldv(_)),
                                "lldv_nits": cfg.lldv_nits,
                            },
                            "mapped_nits": cfg.mapped_nits(),
                            "crop": format!("{last_source:?}"),
                            "receiver": last_seen.as_ref().map(|s| serde_json::json!({
                                "on": s.on,
                                "input": s.input,
                                "name": s.name,
                            })),
                            "apple_tv": apple_tv.now().map(|np| {
                                snapshot::apple_tv(
                                    &np,
                                    appletv::on_screen(&cfg.atv_input, last_seen.as_ref()),
                                )
                            }),
                            "apple_tv_label": last_playing,
                            "settings": crate::web::settings_json(&cfg),
                        }),
                    };
                    let tx = saved_tx.clone();
                    let player = cfg.ub820.clone();
                    std::thread::spawn(move || {
                        let mut save = save;
                        if !player.is_empty() {
                            save.context["ub820"] = snapshot::ub820(&player);
                        }
                        if let Some(t) = snapshot::tunnel(&save.info, &save.bytes) {
                            save.context["tunnel"] = t;
                        }
                        let r = snapshot::default_dir()
                            .ok_or_else(|| "HOME is not set".to_string())
                            .and_then(|root| {
                                snapshot::write(&root, &save).map_err(|e| e.to_string())
                            });
                        let _ = tx.send(r);
                    });
                }
                {
                    let info = frame.info();
                    let first = counts.dv_flagged + counts.dv_bytes == 0;
                    counts.dv_flagged += u64::from(info.dolby_vision_flagged);
                    counts.dv_bytes += u64::from(info.dolby_vision_bytes.is_some());
                    counts.lldv_labelled += u64::from(matches!(content, Content::Lldv(_)));
                    if first && (info.dolby_vision_flagged || info.dolby_vision_bytes.is_some()) {
                        info!(
                            "{:.1} s: the card reports Dolby Vision on input: flag {}, metadata {}",
                            run_started.elapsed().as_secs_f64(),
                            if info.dolby_vision_flagged {
                                "set"
                            } else {
                                "not set"
                            },
                            info.dolby_vision_bytes
                                .map_or_else(|| "none".to_string(), |n| format!("{n} bytes"))
                        );
                    }
                }
                if Some(k) != last_key {
                    counts.format_changes += 1;
                    epochs.next();
                    receiver.nudge();
                    // Capped: a source that flaps would bury everything else.
                    if counts.format_changes <= LOGGED_CHANGES {
                        info!(
                            "{:.1} s: input now {} x {}, {}{}",
                            run_started.elapsed().as_secs_f64(),
                            k.width,
                            k.height,
                            describe(k.kind),
                            hdr_note(frame.info().color.hdr.as_ref())
                        );
                    } else if counts.format_changes == LOGGED_CHANGES + 1 {
                        info!("further input changes are counted, not logged");
                    }
                    last_key = Some(k);
                    held = 0;
                }
                let reported = *format.lock().unwrap_or_else(PoisonError::into_inner);
                if let Some(f) = reported {
                    epochs.rate(rate_mhz(&f));
                }
                if reported != last_format {
                    if let Some(f) = reported {
                        info!(
                            "{:.1} s: the card reports {f}",
                            run_started.elapsed().as_secs_f64()
                        );
                    }
                    last_format = reported;
                }
                held += 1;
                // RGB that may yet turn out to be a tunnel does not move the
                // output, and goes out as black: drawn as plain RGB, tunnel bytes
                // are colour noise, and holding the last frame instead read as a
                // frozen start to the film (2026-10-13). Black keeps time with
                // the sound, which does not pass through here.
                if held < frames_to_trust(k) && frames_to_trust(k) > STABLE_FRAMES {
                    counts.unproven += 1;
                    let free = (0..BUFFERS)
                        .find(|&b| b != front && b != pending)
                        .expect("three buffers, two in use");
                    pl.clear_black(scan.texture(free))?;
                    ready = Some(free);
                    ui.set_slate(Some(Slate::Identifying {
                        detail: format!(
                            "{} \u{b7} {}",
                            size_name(k.width, k.height),
                            reported
                                .map_or_else(|| "rate unknown".into(), |f| rate_name(rate_mhz(&f)))
                        ),
                    }));
                } else {
                    // What the output should be for this frame, for the policy after
                    // the next flip. The rate is the input's, as the card reported it.
                    // An HDR10 frame without its metadata block carries the last
                    // one on, so every frame's rate reaches the policy (#61).
                    let hdr_meta = carried_hdr(content, frame.info().color.hdr, &mut pq_meta);
                    if !matches!(k.kind, Kind::Unsupported(_))
                        && let Some(f) = *format.lock().unwrap_or_else(PoisonError::into_inner)
                    {
                        let (hdr, _) =
                            output_in(cfg.mode, content, hdr_meta.as_ref(), last_tunnel.as_ref());
                        let want = follow::Want {
                            rate_mhz: rate_mhz(&f),
                            signal: signal_of(hdr),
                        };
                        latest_want = Some(want);
                        // Once the input has held still, as the policy
                        // judges it: a banner if the picture type changed,
                        // after the resync if the output is to follow it.
                        if held == frames_to_trust(k).max(STABLE_FRAMES)
                            && let Some(b) = banner_for(
                                content,
                                (k.width, k.height),
                                want.rate_mhz,
                                size.0,
                                &opts.config,
                            )
                        {
                            let resync = want.mode_key() != policy.committed().mode_key();
                            ui.picture(b, resync, Instant::now());
                        }
                    }
                    if asleep.is_some() {
                        // Stopped: nothing goes out, so nothing is uploaded,
                        // detected or drawn. The GPU's clocks can then rest
                        // (14.2 W against 14.8 W rendering black, with each
                        // frame still uploaded). The frame has said what it
                        // wants above, which is what wakes the output.
                        ready = None;
                    } else {
                        let free = (0..BUFFERS)
                            .find(|&b| b != front && b != pending)
                            .expect("three buffers, two in use");
                        // Every change of input format or frame rate, and
                        // each return of the signal, is a new epoch for the
                        // detector, which forgets what it learned.
                        let started = Instant::now();
                        frame_tag += 1;
                        if gpu_timers {
                            pl.set_gpu_tag(frame_tag);
                        }
                        let t = run_started.elapsed().as_secs_f64();
                        let frame_info = *frame.info();
                        let epoch = epochs.now();
                        let (found, prepared) = detect_and_upload(
                            &pl,
                            &mut finder,
                            &frame,
                            k.kind,
                            cfg.lldv,
                            last_tunnel.as_ref(),
                            &mut sources,
                            &mut stages,
                            (t, epoch),
                        )?;
                        noted(&found, &frame_info, t);
                        let hdr10_scene = found.l1.map(|frame_l1| {
                            if epoch != shot_epoch {
                                shot_l1.reset();
                                shot_epoch = epoch;
                            }
                            let codes = shot_l1.push(frame_l1, found.cut);
                            cinefit_render::dm::Scene::from_codes(
                                codes,
                                hdr10_source(hdr_meta.as_ref()),
                                vec![],
                            )
                        });
                        dataset.set_hdr(frame_info.color.hdr, t);
                        dataset.record(cfg.record, &found, t, epoch, content);
                        dataset.keep(&cfg, &found, t, &frame);
                        let source = found.source;
                        last_source = source;
                        let view = view_for(
                            (k.width, k.height),
                            size,
                            win,
                            source,
                            cfg.fill_max_ar.value(),
                        );
                        // Rendered into the output as it stands, which libplacebo maps
                        // to, until the policy moves the output.
                        if let Some(prepared) = prepared {
                            let new_table = draw(
                                &pl,
                                &frame_info,
                                hdr_meta.as_ref(),
                                prepared,
                                hdr10_scene,
                                &mut dm_cache,
                                &target(scan.texture(free), out_color, view, &cfg),
                                &mut stages,
                            )?;
                            ready_situation =
                                Situation::of(new_table, menu.is_open() || ui.info_open());
                            let losses = if gpu_timers {
                                pl.take_gpu_results(&mut gpu_results)
                            } else {
                                cinefit_render::GpuLosses::default()
                            };
                            stages.frame_done(frame_tag, ready_situation, &gpu_results, losses);
                            gpu_results.clear();
                            ready = Some(free);
                            ui.set_slate(None);
                            if let Some(r) = stages.render.last() {
                                // The frame's whole work, as it ran: detect beside
                                // the upload, then the draw.
                                window.borrow_mut().render(Instant::now(), r, ms(started));
                            }
                        } else {
                            // Nothing renders this format: black, not the last
                            // picture held over it.
                            counts.unrendered += 1;
                            pl.clear_black(scan.texture(free))?;
                            ready = Some(free);
                            ui.set_slate(Some(Slate::CantShow {
                                detail: match k.kind {
                                    Kind::Unsupported(f) => cant_show(f),
                                    other => describe(other),
                                },
                            }));
                        }
                    }
                }
            }
            let now;
            let action = if asleep.is_some() {
                // Stopped (#144): no flip is in flight and none comes to pace
                // the loop; the wait for a frame above does. A source that
                // says what it wants lights the output in that. The waiting
                // mode lights it otherwise: for the menu, so it can be seen;
                // for a source nothing can show, so its slate can; and when
                // the timeout is set to never, as it now says.
                now = run_started.elapsed();
                // A source through a receiver in standby does not light it:
                // the receiver coming on does, as its frames then ask.
                let quiet = dark || standby_since.is_some();
                let want = if quiet {
                    latest_want = None;
                    None
                } else {
                    latest_want.take()
                };
                let unshowable =
                    !quiet && matches!(last_key.map(|k| k.kind), Some(Kind::Unsupported(_)));
                let want = want.or_else(|| {
                    (menu.is_open() || cfg.idle_off == 0 || unshowable).then(|| follow::Want {
                        rate_mhz: (cfg.wait_hz * 1000.0).round() as u32,
                        signal: follow::Signal::Sdr,
                    })
                });
                let Some(a) = want.and_then(|w| policy.wake(&w, now)) else {
                    ready = None;
                    continue;
                };
                Some(a)
            } else {
                let flip = out.wait_flip()?;
                if flip.waited > cinefit_kms::FLIP_LATE {
                    warn!(
                        "{:.1} s: a flip came {:.0} ms after it was waited for",
                        run_started.elapsed().as_secs_f64(),
                        flip.waited.as_secs_f64() * 1000.0
                    );
                }
                let missed = pacing.missed;
                pacing.on_flip(flip.sequence, flip.at);
                // Put down to what was going on at the frame it showed.
                stages.by[flip_situation as usize].missed += pacing.missed - missed;
                front = pending;
                // Nothing is in flight now: the moment for a modeset, new
                // metadata, or stopping.
                now = run_started.elapsed();
                // Idle since the last frame, or since the receiver went to
                // standby with a source still coming through it.
                let idle_since = if dark {
                    Some(last_frame_at)
                } else {
                    standby_since
                };
                if idle_since.is_none() {
                    sleep_refused = false;
                }
                let idle = Duration::from_secs(60 * u64::from(cfg.idle_off));
                if cfg.idle_off > 0
                    && !sleep_refused
                    && !menu.is_open()
                    && idle_since.is_some_and(|t| t.elapsed() >= idle)
                    && woke_at.is_none_or(|t| t.elapsed() >= idle)
                {
                    match out.sleep() {
                        Ok(()) => {
                            asleep = Some(Instant::now());
                            ready = None;
                            info!(
                                "{:.1} s: {} for {} min: output stopped, for the display to sleep",
                                now.as_secs_f64(),
                                if dark {
                                    "no signal"
                                } else {
                                    "the receiver in standby"
                                },
                                cfg.idle_off
                            );
                            continue;
                        }
                        Err(e) => {
                            sleep_refused = true;
                            warn!(
                                "{:.1} s: could not stop the output: {}",
                                now.as_secs_f64(),
                                crate::chain(&e)
                            );
                        }
                    }
                }
                latest_want.take().and_then(|w| policy.next(&w, now))
            };
            match action {
                Some(a @ follow::Action::Modeset(w)) => {
                    let request = cinefit_kms::ModeRequest {
                        width: cfg.width,
                        height: cfg.height,
                        refresh_hz: f64::from(w.rate_mhz) / 1000.0,
                    };
                    let hdr = match w.signal {
                        follow::Signal::Hdr(h) => Some(h),
                        follow::Signal::Sdr => None,
                    };
                    let result = match cinefit_kms::choose_mode(&modes, &request) {
                        None => Err(format!(
                            "no {}x{} mode at {:.3} Hz",
                            request.width, request.height, request.refresh_hz
                        )),
                        Some(i) => {
                            // The overlay in the new signalling, in the same
                            // commit.
                            ui.update(&mut out, encoding_for(w.signal), Instant::now());
                            out.set_hdr(hdr);
                            // Into or out of HDR what is rendered is encoded
                            // for the old output: the new signalling lights
                            // with black, which is black in both (#197). The
                            // display resyncs on these switches anyway.
                            let show = if color_for(w.signal) == out_color {
                                Ok(ready.unwrap_or(front))
                            } else {
                                let free = (0..BUFFERS)
                                    .find(|&b| b != front && b != pending)
                                    .expect("three buffers, two in use");
                                ready = None;
                                pl.clear_black(scan.texture(free))
                                    .map(|()| free)
                                    .map_err(|e| e.to_string())
                            };
                            show.and_then(|show| {
                                out.modeset(i, scan.frame(show))
                                    .map(|()| (i, show))
                                    .map_err(|e| crate::chain(&e).to_string())
                            })
                        }
                    };
                    match result {
                        Ok((i, show)) => {
                            policy.done(&a, true, now);
                            if let Some(since) = asleep.take() {
                                woke_at = Some(Instant::now());
                                info!(
                                    "{:.1} s: output on again after {:.0} min stopped",
                                    now.as_secs_f64(),
                                    since.elapsed().as_secs_f64() / 60.0
                                );
                            }
                            counts.modesets += 1;
                            if ready.take().is_some() {
                                counts.shown += 1;
                            }
                            info!(
                                "{:.1} s: followed the source: {} with {}{}",
                                now.as_secs_f64(),
                                modes[i],
                                match w.signal {
                                    follow::Signal::Sdr => "SDR".to_string(),
                                    follow::Signal::Hdr(h) =>
                                        format!("{:?}, MaxCLL {}", h.eotf, h.max_cll),
                                },
                                match out.last_dark() {
                                    Some(d) => format!(
                                        " (through dark, into or out of HDR: dark {:.0} ms)",
                                        d.as_secs_f64() * 1000.0
                                    ),
                                    None => String::new(),
                                }
                            );
                            finished.push((
                                std::mem::replace(&mut mode_name, modes[i].to_string()),
                                period,
                                std::mem::take(&mut pacing),
                            ));
                            period = modes[i].period();
                            cur_mode = modes[i];
                            take_timeout = period.mul_f64(0.8).min(Duration::from_millis(35));
                            out_color = color_for(w.signal);
                            enc = encoding_for(w.signal);
                            ui.resynced(Instant::now());
                            (front, pending) = (show, show);
                            // A modeset sends no flip event; queue one so the
                            // vblanks keep coming.
                            ui.update(&mut out, enc, Instant::now());
                            out.flip(scan.frame(pending))?;
                            continue;
                        }
                        Err(e) => {
                            policy.done(&a, false, now);
                            counts.refused += 1;
                            info!(
                                "{:.1} s: could not follow the source: {e}",
                                now.as_secs_f64()
                            );
                            // Refused after going dark: the output is off,
                            // not on the old signal. Stopped, nothing flips
                            // onto it, and the next want lights it as from
                            // off, as a wake does (#212).
                            if asleep.is_none() && out.stopped() {
                                asleep = Some(Instant::now());
                                warn!(
                                    "{:.1} s: the output is dark after the refused switch: stopped until it can be lit",
                                    now.as_secs_f64()
                                );
                            }
                        }
                    }
                }
                Some(a @ follow::Action::Metadata(h)) => {
                    let ok = out.update_hdr(&h).is_ok();
                    policy.done(&a, ok, now);
                    if ok {
                        counts.metadata += 1;
                    }
                    info!(
                        "{:.1} s: HDR metadata {}: MaxCLL {}, MaxFALL {}, mastering {:.4}-{:.0} nits",
                        now.as_secs_f64(),
                        if ok { "sent" } else { "refused" },
                        h.max_cll,
                        h.max_fall,
                        h.min_mastering,
                        h.max_mastering
                    );
                }
                None => {}
            }
            // A wake that was refused: still stopped, nothing to flip.
            if asleep.is_some() {
                ready = None;
                continue;
            }
            match ready.take() {
                Some(b) => {
                    counts.shown += 1;
                    pending = b;
                    flip_situation = ready_situation;
                    window.borrow_mut().flip(Instant::now(), true);
                }
                None => {
                    counts.repeats += 1;
                    pending = front;
                    window.borrow_mut().flip(Instant::now(), false);
                }
            }
            ui.update(&mut out, enc, Instant::now());
            out.flip(scan.frame(pending))?;
        }
        Ok(())
    };
    let looped = run_loop();
    let ran = run_started.elapsed();
    // After a failed flip no flip event may come, and none is in flight
    // while the output is stopped: do not wait for one.
    if looped.is_ok() && asleep.is_none() {
        let _ = out.wait_flip();
    }
    match out.restore() {
        Ok(restored) => info!("{restored}; now: {}", out.hdr_state()),
        Err(e) => warn!("could not restore the display: {}", crate::chain(&e)),
    }
    let failed_restarts = capture.failed_restarts();
    drop(capture);

    // The handler cannot tell which signal it was: the service stops with
    // SIGTERM, and the journal should not claim someone pressed Ctrl-C.
    if stop.load(Ordering::Relaxed) {
        info!("stopped by Ctrl-C or SIGTERM");
    }
    if looped.is_err() {
        info!("stopped early by an error, reported after the summary");
    }
    info!(
        "ran {:.1} s: captured {} frames with a signal ({} without), shown {}, replaced unseen {}, vblanks repeated {}, unrendered {}, input format changes {}, RGB held back as a possible tunnel {}, signal lost {} times, failed restarts {failed_restarts}",
        ran.as_secs_f64(),
        captured.load(Ordering::Relaxed),
        no_input.load(Ordering::Relaxed),
        counts.shown,
        mailbox.displaced(),
        counts.repeats,
        counts.unrendered,
        counts.format_changes,
        counts.unproven,
        counts.signal_losses,
    );
    info!(
        "Dolby Vision from the card: flag on {} frames, metadata on {} (input labelled as low-latency Dolby Vision: {} frames)",
        counts.dv_flagged, counts.dv_bytes, counts.lldv_labelled
    );
    match settled {
        Some((at, displaced)) => {
            let repeats = counts.repeats - at.repeats;
            let lost = mailbox.displaced() - displaced;
            info!(
                "after the first {} s: replaced unseen {lost}, vblanks repeated {repeats}",
                SETTLE.as_secs()
            );
        }
        None => info!("shorter than the {} s settle: no verdict", SETTLE.as_secs()),
    }
    info!(
        "followed the source: {} modesets ({} through dark, into or out of HDR), {} metadata updates without one, {} refused",
        counts.modesets,
        out.relights(),
        counts.metadata,
        counts.refused
    );
    dataset.end_session();
    if cfg.crop {
        let shares: Vec<String> = finder
            .det
            .policy()
            .shares()
            .iter()
            .map(|s| format!("{} rows {:.1} s", s.rows, s.seconds))
            .collect();
        info!(
            "crop: {} changes; time at each height: {}",
            finder.crops,
            shares.join(", ")
        );
    }
    finished.push((mode_name, period, pacing));
    for (name, period, pacing) in &finished {
        info!("{name}: {}", pacing.summary(*period).trim_end());
    }
    info!("{}", stages.unpack.line("unpack, with upload").trim_end());
    // Beside the upload, on its own thread: the longer of the two is what
    // the frame waits for (#220).
    info!(
        "{}",
        stages
            .detect_tunnel
            .line("bar detection, tunnel")
            .trim_end()
    );
    info!(
        "{}",
        stages.detect_v210.line("bar detection, v210").trim_end()
    );
    info!(
        "{}",
        stages
            .detect_and_upload
            .line("detection beside the upload, both done")
            .trim_end()
    );
    info!(
        "{}",
        stages
            .render
            .line("render, with the wait for the GPU")
            .trim_end()
    );
    for line in stages.split_lines(gpu_timers) {
        info!("{line}");
    }
    looped
}

/// What a rendered scanout buffer holds, from every eighth pixel: how much
/// of the picture area has something in it, its brightest value, and the
/// brightest value in the bars, which should be black.
fn look(px: &[u8], stride: usize, fourcc: u32, size: (u32, u32), place: Rect) -> String {
    let (mut lit, mut total, mut peak, mut bars) = (0usize, 0usize, 0f64, 0f64);
    for y in (0..size.1 as usize).step_by(8) {
        for x in (0..size.0 as usize).step_by(8) {
            let o = y * stride + x * 4;
            let Some(word) = px.get(o..o + 4) else {
                continue;
            };
            let word = [word[0], word[1], word[2], word[3]];
            let Some(rgb) = cinefit_kms::scanout::decode(fourcc, word) else {
                return "scanout format unknown: pixels not read".into();
            };
            let v = rgb.iter().copied().fold(0.0, f64::max);
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            if fx >= place.x0 && fx < place.x1 && fy >= place.y0 && fy < place.y1 {
                total += 1;
                lit += usize::from(v > 0.02);
                peak = peak.max(v);
            } else {
                bars = bars.max(v);
            }
        }
    }
    format!(
        "pixels ({}): {:.1}% of the picture area above 2%, brightest {peak:.3}; brightest in the bars {bars:.3}",
        String::from_utf8_lossy(&fourcc.to_le_bytes()),
        100.0 * lit as f64 / total.max(1) as f64
    )
}

#[derive(Debug, Default, Clone, Copy)]
struct Counts {
    shown: u64,
    repeats: u64,
    unrendered: u64,
    format_changes: u64,
    /// Frames of RGB held back while it might still be a tunnel.
    unproven: u64,
    /// Times the signal went away for longer than [`NO_SIGNAL`].
    signal_losses: u64,
    modesets: u64,
    metadata: u64,
    refused: u64,
    /// Frames the card flagged as carrying Dolby Vision metadata, and frames
    /// it attached any to: whether the chain ever delivers it, which would
    /// tell low-latency Dolby Vision from SDR BT.2020 for certain. None on
    /// the owner's chain so far (2026-10-14: ~4,000 frames read as low-latency
    /// Dolby Vision, none flagged). Either the HDCP splitter drops the
    /// player's Dolby Vision InfoFrame or the card does not pass it on; this
    /// cannot tell which.
    dv_flagged: u64,
    dv_bytes: u64,
    /// Frames that `input.lldv` reads as low-latency Dolby Vision: what the
    /// two above are to be compared with.
    lldv_labelled: u64,
}

/// An input's HDR metadata, for a log line: empty when it carries none.
fn hdr_note(hdr: Option<&HdrMetadata>) -> String {
    let Some(h) = hdr else {
        return String::new();
    };
    let n = |v: Option<f64>| v.map_or("unreported".to_string(), |v| format!("{v:.0}"));
    format!(
        "; HDR metadata MaxCLL {}, MaxFALL {}, mastering peak {}",
        n(h.max_cll),
        n(h.max_fall),
        n(h.max_mastering)
    )
}

/// HDR metadata for a session's notes, every value or "unreported".
fn hdr_line(h: &HdrMetadata) -> String {
    let n = |v: Option<f64>, d: usize| v.map_or("unreported".to_string(), |v| format!("{v:.d$}"));
    format!(
        "HDR metadata: MaxCLL {}, MaxFALL {}, mastering {}-{} nits",
        n(h.max_cll, 0),
        n(h.max_fall, 0),
        n(h.min_mastering, 4),
        n(h.max_mastering, 0)
    )
}

#[cfg(test)]
mod tests {
    fn dovi_tunnel(offsets: [u32; 3], l1_max: Option<u16>) -> cinefit_dovi::Tunnel {
        use cinefit_dovi::{DmMetadata, Header, Level1, packet::PacketType};
        cinefit_dovi::Tunnel {
            header: Header {
                packet_type: PacketType::First,
                metadata_type: 0,
                affected_metadata_id: 0,
                metadata_version: 0,
                current_metadata_id: 0,
                no_metadata: l1_max.is_none(),
                end_of_stream: false,
            },
            copies_used: vec![0],
            metadata: l1_max.map(|max_pq| DmMetadata {
                scene_refresh: false,
                ycc_to_rgb: [0; 9],
                ycc_to_rgb_offset: offsets,
                rgb_to_lms: [0; 9],
                signal_bit_depth: 12,
                signal_color_space: 2,
                source_min_pq: 62,
                source_max_pq: 3696,
                level1: Some(Level1 {
                    min_pq: 62,
                    max_pq,
                    avg_pq: 819,
                }),
                level2: Vec::new(),
                level4: None,
                level5: None,
                other: Vec::new(),
            }),
        }
    }

    const UB820: [u32; 3] = [0, 0x0800_0000, 0x0800_0000];

    #[test]
    fn a_tunnel_frame_is_absent_held_or_an_update() {
        use super::{TunnelFrame, tunnel_frame};
        use cinefit_dovi::Error;
        let absent = tunnel_frame(Err(Error::NoPacket { packet: 0 }));
        assert!(matches!(absent, TunnelFrame::Absent));
        // Packets that did not fit, a packet without metadata (#194), and
        // offsets on no known scale: a tunnel, nothing new.
        for t in [
            Err(Error::NoPacket { packet: 1 }),
            Ok(dovi_tunnel(UB820, None)),
            Ok(dovi_tunnel([0, 1, 1], Some(3079))),
        ] {
            let held = tunnel_frame(t);
            assert!(matches!(held, TunnelFrame::Held), "{held:?}");
        }
        let update = tunnel_frame(Ok(dovi_tunnel(UB820, Some(3079))));
        assert!(matches!(update, TunnelFrame::Update(ref t) if t.scene_max_pq == Some(3079)));
    }

    // Metadata, a frame without, metadata: a tunnel throughout, the first
    // conversion kept over the frame without (#194).
    #[test]
    fn a_tunnel_frame_without_metadata_keeps_the_last_conversion() {
        use super::{Kind, rgb10_kind, tunnel_frame};
        use cinefit_dovi::Error;
        let rgb = Kind::Rgb(cinefit_render::rgb::Format::R210);
        let mut last = None;
        for (t, kind, max) in [
            (Ok(dovi_tunnel(UB820, Some(3079))), Kind::Tunnel, 3079),
            (Ok(dovi_tunnel(UB820, None)), Kind::Tunnel, 3079),
            (Ok(dovi_tunnel(UB820, Some(3465))), Kind::Tunnel, 3465),
            (Err(Error::NoPacket { packet: 0 }), rgb, 3465),
        ] {
            assert_eq!(rgb10_kind(tunnel_frame(t), &mut last), kind);
            assert_eq!(last.as_ref().and_then(|t| t.scene_max_pq), Some(max));
        }
    }

    // The cached table is the one Frame::new gives for every frame's scene
    // and target: kept over repeats, rebuilt when L1, a trim or the target
    // changes, and when a scene comes back (#206).
    #[test]
    fn the_tunnel_mapping_is_rebuilt_on_any_change() {
        use cinefit_render::dm::{Frame, Scene, Trim};
        let scene = |max, slope| {
            Scene::from_codes(
                [62, 1200, max],
                [62, 3079],
                vec![Trim::from_codes(2081, [slope, 2048, 2048, 2048, 2048])],
            )
        };
        let mut cache = None;
        assert!(
            super::dm_frame(&mut cache, scene(2950, 2048), 96.0).1,
            "built"
        );
        assert!(
            !super::dm_frame(&mut cache, scene(2950, 2048), 96.0).1,
            "kept"
        );
        for (s, target) in [
            (scene(2950, 2048), 96.0),
            (scene(2950, 2048), 96.0),
            (scene(3079, 2048), 96.0),
            (scene(3079, 2100), 96.0),
            (scene(3079, 2100), 155.0),
            (scene(2950, 2048), 96.0),
        ] {
            let want = Frame::new(&s, f64::from(target));
            assert_eq!(*super::dm_frame(&mut cache, s, target).0, want);
        }
        // A repeat of the last inputs returns the kept table, not a new one:
        // a mark set on it survives the call.
        cache.as_mut().expect("cached").2.k1 = -1.0;
        assert_eq!(
            super::dm_frame(&mut cache, scene(2950, 2048), 96.0).0.k1,
            -1.0
        );
    }

    #[test]
    fn no_display_is_waited_for() {
        use cinefit_kms::Error as Kms;
        // Two refusals, then a display: the third answer is the result.
        let mut answers = vec![Ok(7), Err(Kms::NoDisplay), Err(Kms::NoDisplay)];
        let got = retry_no_display(|| answers.pop().unwrap(), false, Duration::ZERO);
        assert_eq!(got.ok(), Some(7));
        assert!(answers.is_empty());
        // A dry run does not wait.
        let mut answers = vec![Ok(7), Err(Kms::NoDisplay)];
        let got = retry_no_display(|| answers.pop().unwrap(), true, Duration::ZERO);
        assert!(matches!(got, Err(Kms::NoDisplay)));
        // Any other error ends it at once.
        let mut answers = vec![Ok(7), Err(Kms::NoCrtc)];
        let got = retry_no_display(|| answers.pop().unwrap(), false, Duration::ZERO);
        assert!(matches!(got, Err(Kms::NoCrtc)));
    }

    use super::*;

    /// An HDR10 title's source range: its mastering display, else MaxCLL for
    /// the top; without metadata 1000 nits over 0.0001 (#141).
    #[test]
    fn an_hdr10_source_range_comes_from_its_metadata() {
        let code = |n: f64| (cinefit_render::dm::pq_oetf(n) * 4095.0).round() as u16;
        assert_eq!(hdr10_source(None), [code(0.0001), code(1000.0)]);
        let mut h = HdrMetadata {
            red: None,
            green: None,
            blue: None,
            white: None,
            max_mastering: None,
            min_mastering: None,
            max_cll: Some(488.0),
            max_fall: None,
        };
        assert_eq!(hdr10_source(Some(&h)), [code(0.0001), code(488.0)]);
        h.max_mastering = Some(4000.0);
        h.min_mastering = Some(0.005);
        assert_eq!(hdr10_source(Some(&h)), [code(0.005), code(4000.0)]);
        // Zero is unreported, not black.
        h.min_mastering = Some(0.0);
        assert_eq!(hdr10_source(Some(&h))[0], code(0.0001));
    }

    // Frames are put down to what was going on, a GPU result is taken once,
    // and the summary has a block for each situation seen (#219).
    #[test]
    fn times_are_split_by_situation() {
        assert_eq!(Situation::of(false, false), Situation::Steady);
        assert_eq!(Situation::of(true, false), Situation::NewTable);
        assert_eq!(Situation::of(true, true), Situation::Overlay);
        use cinefit_render::{GpuLosses, GpuResult};
        let unpack = |tag: u64, ms: f64| GpuResult {
            pass: 1,
            ms,
            tag: Some(tag),
        };
        let output = |ms: f64| GpuResult {
            pass: 4,
            ms,
            tag: None,
        };
        let none = GpuLosses::default();
        let mut stages = Stages::default();
        stages.render.push(4.0);
        stages.frame_done(1, Situation::Steady, &[], none);
        stages.frame_done(2, Situation::Steady, &[unpack(1, 0.5), output(2.0)], none);
        stages.render.push(9.0);
        // Frame 2's slow unpack comes in at the scene change: it is frame 2's,
        // and so steady's, not the scene change's (#263).
        stages.frame_done(3, Situation::NewTable, &[unpack(2, 7.0)], none);
        stages.by[Situation::NewTable as usize].missed += 1;
        // Results that came in together after a stall are each kept: the
        // spike among them too.
        stages.frame_done(
            4,
            Situation::Steady,
            &[unpack(3, 0.6), output(2.0), output(30.0), unpack(4, 0.5)],
            GpuLosses {
                untimed: [1, 0, 0, 0, 0],
                ..none
            },
        );
        let steady = &stages.by[Situation::Steady as usize];
        assert_eq!(steady.frames, 3);
        assert_eq!(steady.gpu[1].count(), 3, "frames 1, 2 and 4's unpacks");
        let table = &stages.by[Situation::NewTable as usize];
        assert_eq!(table.gpu[1].count(), 1, "frame 3's unpack");
        assert_eq!(stages.gpu[1].count(), 4);
        // Untagged render results: in the totals only, every one of them.
        assert_eq!(stages.gpu[4].count(), 3);
        assert!(
            Situation::ALL
                .iter()
                .all(|&s| stages.by[s as usize].gpu[4].count() == 0)
        );
        let lines = stages.split_lines(true);
        let text = lines.join("\n");
        assert!(text.contains("steady: 3 frames, 0 flips missed"), "{text}");
        assert!(
            text.contains("a new Dolby table (a scene change): 1 frames, 1 flips missed"),
            "{text}"
        );
        assert!(text.contains("GPU output: median"), "{text}");
        assert!(
            text.contains("GPU upload runs not followed: 0 results dropped, 1 untimed"),
            "{text}"
        );
        assert!(!text.contains("menu or info panel"), "{text}");
        let off = stages.split_lines(false).join("\n");
        assert!(
            !off.contains("GPU output") && !off.contains("GPU unpack"),
            "no timers, no GPU pass lines: {off}"
        );
    }

    /// A result whose frame has left the recent ones still counts in the
    /// totals, under no situation.
    #[test]
    fn a_result_too_late_for_its_frame_counts_in_the_totals() {
        let mut stages = Stages::default();
        let none = cinefit_render::GpuLosses::default();
        for tag in 1..=RECENT_FRAMES as u64 + 1 {
            stages.frame_done(tag, Situation::Steady, &[], none);
        }
        let late = cinefit_render::GpuResult {
            pass: 0,
            ms: 1.0,
            tag: Some(1),
        };
        stages.frame_done(100, Situation::Steady, &[late], none);
        assert_eq!(stages.gpu[0].count(), 1);
        assert_eq!(stages.by[Situation::Steady as usize].gpu[0].count(), 0);
    }

    // Detection is timed by what it read, apart from the upload (#220).
    #[test]
    fn detection_is_timed_by_kind() {
        let mut stages = Stages::default();
        let v210 = Kind::Ycbcr(YcbcrColor {
            matrix: Matrix::Bt2020Nc,
            primaries: Primaries::Bt2020,
            transfer: Transfer::Pq,
        });
        stages.detected(Some(9.0), Kind::Tunnel);
        stages.detected(Some(6.0), v210);
        stages.detected(None, Kind::Tunnel);
        stages.detected(Some(1.0), Kind::Rgb(rgb::Format::R210));
        assert_eq!(stages.detect_tunnel.last(), Some(9.0));
        assert_eq!(stages.detect_v210.last(), Some(6.0));
        assert_eq!(
            stages.detect_tunnel.quantile(0.5),
            Some(9.0),
            "one tunnel time"
        );
        assert!(stages.detect_and_upload.last().is_none());
        assert!(stages.unpack.last().is_none());
    }

    // The dataset's background work does not hold up the play loop: a slow
    // job is handed over at once, jobs run in order (a panic in one does not
    // stop the next), and the end of the run waits for what is queued (#208).
    #[test]
    fn the_dataset_worker_runs_in_order_off_the_loop() {
        let done = Arc::new(Mutex::new(Vec::new()));
        let mut worker = Worker::default();
        let started = Instant::now();
        let d = Arc::clone(&done);
        worker.run(Box::new(move || {
            std::thread::sleep(Duration::from_millis(300));
            d.lock().unwrap().push(1);
        }));
        worker.run(Box::new(|| panic!("a bad batch")));
        let d = Arc::clone(&done);
        worker.run(Box::new(move || d.lock().unwrap().push(2)));
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "handing over waited {:?}",
            started.elapsed()
        );
        drop(worker);
        assert!(
            started.elapsed() >= Duration::from_millis(300),
            "the end waits"
        );
        assert_eq!(*done.lock().unwrap(), [1, 2]);
    }

    /// The tone mapping's peak goes with the HDR10 metadata in force: held
    /// over frames without a block, forgotten when HDR10 ends (#195).
    #[test]
    fn the_hdr10_peak_does_not_outlive_its_title() {
        let meta = |cll: Option<f64>, mastering: Option<f64>| HdrMetadata {
            red: None,
            green: None,
            blue: None,
            white: None,
            max_mastering: mastering,
            min_mastering: Some(0.005),
            max_cll: cll,
            max_fall: None,
        };
        let ycbcr = |transfer| YcbcrColor {
            matrix: Matrix::Bt2020Nc,
            primaries: Primaries::Bt2020,
            transfer,
        };
        let hdr10 = Content::Hdr10(ycbcr(Transfer::Pq));
        let menu = Content::Sdr(ycbcr(Transfer::Bt1886));
        let mut kept = None;
        let peak =
            |content, frame, kept: &mut _| pq_peak(carried_hdr(content, frame, kept).as_ref());
        // A 4000-nit grade, with and between its blocks.
        assert_eq!(
            peak(hdr10, Some(meta(None, Some(4000.0))), &mut kept),
            Some(4000.0)
        );
        assert_eq!(peak(hdr10, None, &mut kept), Some(4000.0));
        // MaxCLL first; a zero is no value.
        assert_eq!(
            peak(hdr10, Some(meta(Some(1445.0), Some(4000.0))), &mut kept),
            Some(1445.0)
        );
        assert_eq!(
            peak(hdr10, Some(meta(Some(0.0), Some(4000.0))), &mut kept),
            Some(4000.0)
        );
        // A menu, then a title with neither: the 1000-nit fallback, not 4000.
        assert_eq!(peak(menu, None, &mut kept), None);
        assert_eq!(peak(hdr10, None, &mut kept), None);
        assert_eq!(peak(hdr10, Some(meta(Some(0.0), None)), &mut kept), None);
    }

    /// A session's HDR note keeps an unreported value apart from zero.
    #[test]
    fn the_hdr_note_says_what_was_not_reported() {
        let h = HdrMetadata {
            red: None,
            green: None,
            blue: None,
            white: None,
            max_mastering: Some(1000.0),
            min_mastering: Some(0.0001),
            max_cll: Some(1445.0),
            max_fall: None,
        };
        assert_eq!(
            hdr_line(&h),
            "HDR metadata: MaxCLL 1445, MaxFALL unreported, mastering 0.0001-1000 nits"
        );
    }

    /// A v210 frame, 10-bit luma by row and column, neutral chroma: what the
    /// detector reads, without the capture card.
    fn v210(width: u32, height: u32, luma: impl Fn(u32, u32) -> u32) -> (FrameInfo, Vec<u8>) {
        let row_bytes = (width as usize).div_ceil(6) * 16;
        let mut bytes = vec![0u8; row_bytes * height as usize];
        let word = |a: u32, b: u32, c: u32| (a | b << 10 | c << 20).to_le_bytes();
        for (y, row) in bytes.chunks_mut(row_bytes).enumerate() {
            let l = |x: u32| if x < width { luma(x, y as u32) } else { 64 };
            for (g, group) in row.chunks_mut(16).enumerate() {
                let x = g as u32 * 6;
                group[0..4].copy_from_slice(&word(512, l(x), 512));
                group[4..8].copy_from_slice(&word(l(x + 1), 512, l(x + 2)));
                group[8..12].copy_from_slice(&word(512, l(x + 3), 512));
                group[12..16].copy_from_slice(&word(l(x + 4), 512, l(x + 5)));
            }
        }
        let info = FrameInfo {
            width,
            height,
            row_bytes,
            pixel_format: PixelFormat::V210,
            stream_time: None,
            duration: None,
            has_input: true,
            color: cinefit_decklink::ColorInfo {
                colorspace: None,
                eotf: None,
                hdr: None,
            },
            dolby_vision_flagged: false,
            dolby_vision_bytes: None,
        };
        (info, bytes)
    }

    #[test]
    fn a_crop_is_not_held_over_input_the_detector_cannot_read() {
        // A still 2.40 picture in a 1080p frame: 140 rows of bar each side.
        let (w, h, bar) = (1920, 1080, 140);
        let (info, scope) = v210(w, h, |x, y| {
            let hash = (x.wrapping_mul(2_654_435_761) ^ y.wrapping_mul(40_503)) >> 7;
            if y < bar || y >= h - bar {
                64
            } else {
                180 + hash % 620
            }
        });
        let mut finder = CropFinder {
            det: Detector::default(),
            on: true,
            crops: 0,
            unread: false,
            thumbs: false,
            tunnel_cuts: Default::default(),
            tunnel_epoch: 0,
            measure: false,
        };
        let sdr = Kind::Ycbcr(YcbcrColor {
            matrix: Matrix::Bt2020Nc,
            primaries: Primaries::Bt2020,
            transfer: Transfer::Bt1886,
        });
        let mut source = geo::Source::Whole;
        for i in 0..24u32 {
            source = finder
                .find(&info, Some(&scope), sdr, f64::from(i) / 24.0, 1)
                .source;
        }
        let geo::Source::Rows { top, bottom } = source else {
            panic!("no rows: {source:?}");
        };
        assert!(top > 0 && bottom > 0, "scope frames never cropped");

        // RGB at a new size: full frame, and one change logged for it, not
        // one a frame.
        let rgb_info = FrameInfo {
            width: 3840,
            height: 2160,
            ..info
        };
        let mut changes = Vec::new();
        for (i, kind) in [Kind::Rgb(rgb::Format::R210), Kind::Rgb(rgb::Format::R210)]
            .into_iter()
            .enumerate()
        {
            let found = finder.find(&rgb_info, None, kind, 1.0 + i as f64 / 24.0, 2);
            assert_eq!(found.source, geo::Source::Rows { top: 0, bottom: 0 });
            assert!(found.ms.is_none());
            changes.extend(found.change);
        }
        assert_eq!(
            changes,
            [(Crop::default(), "the detector does not read RGB")]
        );

        // YCbCr full-frame picture again, a new epoch: the old crop does not
        // come back with it.
        let (_, full) = v210(w, h, |x, y| {
            180 + ((x.wrapping_mul(2_654_435_761) ^ y.wrapping_mul(40_503)) >> 7) % 620
        });
        let found = finder.find(&info, Some(&full), sdr, 2.0, 3);
        assert_eq!(found.source, geo::Source::Rows { top: 0, bottom: 0 });
    }

    /// A 1080p tunnel frame with `bar` black rows top and bottom and a
    /// textured picture between, packed as HDMI packs 12-bit 4:2:2.
    fn tunnel_with_bars(bar: u32) -> (FrameInfo, Vec<u8>) {
        let (w, h) = (1920u32, 1080u32);
        let row_bytes = w as usize * 4;
        let mut bytes = vec![0u8; row_bytes * h as usize];
        for y in 0..h {
            for x in 0..w {
                let hash = (x.wrapping_mul(2_654_435_761) ^ y.wrapping_mul(40_503)) >> 7;
                let l = if y < bar || y >= h - bar {
                    0
                } else {
                    (600 + hash % 2400) as u16
                };
                let c = 2048u16;
                let (r, g, b) = (c >> 4, l >> 4, (c & 0xF) << 4 | (l & 0xF));
                let word = u32::from(r) << 22 | u32::from(g) << 12 | u32::from(b) << 2;
                let o = y as usize * row_bytes + 4 * x as usize;
                bytes[o..o + 4].copy_from_slice(&word.to_be_bytes());
            }
        }
        let info = FrameInfo {
            width: w,
            height: h,
            row_bytes,
            pixel_format: PixelFormat::Rgb10,
            ..v210(2, 2, |_, _| 64).0
        };
        (info, bytes)
    }

    fn finder() -> CropFinder {
        CropFinder {
            det: Detector::default(),
            on: true,
            crops: 0,
            unread: false,
            thumbs: false,
            tunnel_cuts: Default::default(),
            tunnel_epoch: 0,
            measure: false,
        }
    }

    #[test]
    fn a_tunnel_frame_is_cropped_as_its_bars_say() {
        // A 2.40 picture in a 1080p tunnel frame: full-range luma, bars at 0
        // and chroma at neutral 2048, packed as HDMI packs 12-bit 4:2:2.
        let bar = 140u32;
        let (info, bytes) = tunnel_with_bars(bar);
        let mut finder = finder();
        let mut found = None;
        for i in 0..24u32 {
            found = Some(finder.find(&info, Some(&bytes), Kind::Tunnel, f64::from(i) / 24.0, 1));
        }
        let found = found.expect("frames");
        let geo::Source::Rows { top, bottom } = found.source else {
            panic!("no rows: {:?}", found.source);
        };
        assert!(
            top.abs_diff(bar) <= 2 && bottom.abs_diff(bar) <= 2,
            "{top}/{bottom}"
        );
        assert!(found.ms.is_some(), "the detector read it");
    }

    // Cropping off on a scope title, a full-frame one after, cropping on
    // over a black opening: with nothing to read in it, a fresh detector
    // shows the full frame, where the old one held the scope crop from
    // before it was turned off (#223). A picture in the bars would undo a
    // stale crop at once; a dark start would not.
    #[test]
    fn turning_cropping_back_on_starts_the_detector_over() {
        let (scope_info, scope) = tunnel_with_bars(140);
        let (full_info, full) = tunnel_with_bars(0);
        let (black_info, black) = tunnel_with_bars(540);
        let mut finder = finder();
        let rows = |f: &Found| match f.source {
            geo::Source::Rows { top, bottom } => (top, bottom),
            ref other => panic!("no rows: {other:?}"),
        };
        let mut t = 0.0;
        let mut step = |finder: &mut CropFinder, info: &FrameInfo, bytes: &[u8]| {
            let f = finder.find(info, Some(bytes), Kind::Tunnel, t, 1);
            t += 1.0 / 24.0;
            f
        };
        let mut last = None;
        for _ in 0..24 {
            last = Some(step(&mut finder, &scope_info, &scope));
        }
        let (top, _) = rows(&last.expect("frames"));
        assert!(top.abs_diff(140) <= 2, "the scope crop first: {top}");
        finder.set_on(false);
        for _ in 0..24 {
            assert_eq!(
                step(&mut finder, &full_info, &full).source,
                geo::Source::Whole
            );
        }
        finder.set_on(true);
        for _ in 0..24 {
            assert_eq!(rows(&step(&mut finder, &black_info, &black)), (0, 0));
        }
        // On while on already is not a new start.
        for _ in 0..24 {
            step(&mut finder, &scope_info, &scope);
        }
        finder.set_on(true);
        let (top, _) = rows(&step(&mut finder, &scope_info, &scope));
        assert!(top.abs_diff(140) <= 2, "kept: {top}");
    }

    // A change of frame rate alone (the same size and format) is a new epoch:
    // the detector starts over, so the first frame after it is read as the
    // full frame, not with the crop the last stream built (#222).
    #[test]
    fn a_frame_rate_change_starts_the_detector_over() {
        let (info, bytes) = tunnel_with_bars(140);
        let mut finder = finder();
        let mut epochs = Epochs::default();
        let rows = |f: &Found| match f.source {
            geo::Source::Rows { top, bottom } => (top, bottom),
            ref other => panic!("no rows: {other:?}"),
        };
        epochs.rate(23_976);
        let mut t = 0.0;
        let mut last = None;
        for _ in 0..24 {
            epochs.rate(23_976);
            last = Some(finder.find(&info, Some(&bytes), Kind::Tunnel, t, epochs.now()));
            t += 1.0 / 24.0;
        }
        let (top, _) = rows(&last.expect("frames"));
        assert!(top.abs_diff(140) <= 2, "the scope crop first: {top}");
        let before = epochs.now();
        epochs.rate(59_940);
        assert_eq!(epochs.now(), before + 1, "a new epoch");
        let first = finder.find(&info, Some(&bytes), Kind::Tunnel, t, epochs.now());
        assert_eq!(rows(&first), (0, 0), "a fresh detector");
        // The same rate again is not.
        epochs.rate(59_940);
        assert_eq!(epochs.now(), before + 1);
    }

    #[test]
    fn look_tells_picture_from_bars() {
        // 64x16, XRGB2101010: the middle 32 columns lit at half, the rest black.
        let (w, h) = (64usize, 16usize);
        let mut px = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 16..48 {
                let word = 512u32 << 20 | 512 << 10 | 512;
                px[(y * w + x) * 4..][..4].copy_from_slice(&word.to_le_bytes());
            }
        }
        let place = Rect {
            x0: 16.0,
            y0: 0.0,
            x1: 48.0,
            y1: 16.0,
        };
        let report = look(
            &px,
            w * 4,
            cinefit_kms::scanout::XRGB2101010,
            (64, 16),
            place,
        );
        assert!(report.contains("100.0% of the picture area"), "{report}");
        assert!(report.contains("brightest 0.500"), "{report}");
        assert!(report.contains("in the bars 0.000"), "{report}");
        // Black everywhere reads as such.
        let black = look(
            &vec![0; w * h * 4],
            w * 4,
            cinefit_kms::scanout::XRGB2101010,
            (64, 16),
            place,
        );
        assert!(black.contains("0.0% of the picture area"), "{black}");
    }
}
