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

use crate::config::{self, Config};
use crate::content::*;
use crate::follow;
use crate::menu::Menu;
use crate::osd::InfoText;
use crate::runlog;
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
    pub device: usize,
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

/// Decodes the tunnel in an `r210` frame, if it carries one. `None` when no
/// packet passes its CRC (not a tunnel) or the metadata is unusable.
fn decode_tunnel(info: &FrameInfo, bytes: &[u8]) -> Option<Option<Tunnel>> {
    let parity = cinefit_dovi::r210_parity(bytes, info.width as usize, info.row_bytes);
    let tunnel = match cinefit_dovi::decode(parity) {
        Ok(t) => t,
        Err(cinefit_dovi::Error::NoPacket { packet: 0 }) => return None,
        // A tunnel whose packets did not fit: still a tunnel.
        Err(_) => return Some(None),
    };
    // No metadata this frame, or offsets on no known scale: a tunnel whose
    // last good conversion still applies.
    let m = tunnel.metadata?;
    Some(m.conversion().map(|c| Tunnel {
        color: DoviColor {
            ycc_to_lms: c.ycc_to_rgb,
            offset: c.offset,
            linear: c.rgb_to_lms,
        },
        source_min_pq: m.source_min_pq,
        source_max_pq: m.source_max_pq,
        scene_max_pq: m.level1.map(|l| l.max_pq),
        scene_avg_pq: m.level1.map(|l| l.avg_pq),
    }))
}

/// Classifies a frame, updating the last good tunnel conversion.
fn classify(frame: &Frame, last_tunnel: &mut Option<Tunnel>) -> Key {
    let info = frame.info();
    let kind = match info.pixel_format {
        PixelFormat::V210 => Kind::Ycbcr(ycbcr_color(info)),
        PixelFormat::Argb8 => Kind::Rgb(rgb::Format::Argb8),
        PixelFormat::Rgb10 => {
            let decoded = frame.pixels().and_then(|p| decode_tunnel(info, p.bytes()));
            match decoded {
                Some(t) => {
                    if let Some(t) = t {
                        *last_tunnel = Some(t);
                    }
                    Kind::Tunnel
                }
                None => Kind::Rgb(rgb::Format::R210),
            }
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
    /// The HDR10 source's peak, held between frames: the card attaches the
    /// metadata only to frames it flags, and a peak that came and went would
    /// make the tone mapping flicker.
    pq_peak: Option<f32>,
}

/// Per-stage times, milliseconds.
#[derive(Default)]
struct Stages {
    unpack: Times,
    render: Times,
}

/// Where a frame is rendered: a scanout buffer of the output's size, in the
/// output's colour.
struct Target<'a, 'p> {
    tex: &'a Texture<'p>,
    color: RgbColor,
    /// What plain RGB input's codes mean (the `input.rgb_range` setting).
    rgb_range: Levels,
    /// HDR into SDR: what SDR white stands for, which is also the peak
    /// low-latency Dolby Vision is mapped to, and the curve (the settings).
    white_nits: f32,
    curve: Curve,
    /// The `tonemap.source_peak` override for HDR10, if set.
    source_peak: Option<f32>,
    /// Which part of the frame is drawn and where it lands.
    view: View,
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
fn target<'a, 'p>(
    targets: &'a [Texture<'p>],
    b: usize,
    color: RgbColor,
    view: View,
    c: &Config,
) -> Target<'a, 'p> {
    Target {
        tex: &targets[b],
        color,
        rgb_range: c.rgb_range,
        white_nits: f32::from(c.white_nits),
        curve: c.curve,
        source_peak: (c.source_peak > 0).then_some(f32::from(c.source_peak)),
        view,
    }
}

/// The bar detector as `play` runs it: on a worker thread, beside the
/// upload of the same frame.
struct CropFinder {
    det: Detector,
    /// The `detect.crop` setting.
    on: bool,
    crops: u64,
    /// The last frame was one the detector does not read, and the crop went
    /// to full frame for it.
    unread: bool,
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

impl CropFinder {
    fn find(
        &mut self,
        info: &FrameInfo,
        bytes: Option<&[u8]>,
        kind: Kind,
        t: f64,
        epoch: u32,
    ) -> Found {
        let peak = bytes
            .filter(|_| matches!(kind, Kind::Ycbcr(_)))
            .and_then(|b| V210::new(b, info.width, info.height, info.row_bytes))
            .map(|v| peak_luma(&v));
        if !self.on {
            return Found {
                source: geo::Source::Whole,
                ms: None,
                change: None,
                peak,
            };
        }
        // A crop belongs to the picture it was found on: held over RGB or the
        // tunnel, a scope title's crop would trim whatever followed it. Those
        // have no adapter yet (#69), so they get the full frame. The detector
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
            };
        }
        self.unread = false;
        let (mut took, mut change) = (None, None);
        if let Some(b) = bytes
            && let Some(v) = V210::new(b, info.width, info.height, info.row_bytes)
        {
            let t0 = Instant::now();
            let d = self.det.push(&v, t, epoch);
            took = Some(ms(t0));
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
        }
    }
}

/// A frame's detection and upload, side by side: the detector reads the
/// captured bytes on a worker thread while this thread copies the same bytes
/// to the GPU and unpacks them. Both only read the frame, and only the draw
/// that follows needs the crop, so it still applies to this same frame:
/// nothing waits a frame, and the frame costs the longer of the two instead
/// of their sum. `at` is the frame's time and the detector's epoch. Returns
/// what the detector found and whether the frame was uploaded.
#[allow(clippy::too_many_arguments)]
fn detect_and_upload<'p>(
    pl: &'p Placebo,
    finder: &mut CropFinder,
    frame: &Frame,
    kind: Kind,
    tunnel: Option<&Tunnel>,
    sources: &mut Sources<'p>,
    stages: &mut Stages,
    at: (f64, u32),
) -> Result<(Found, bool), Box<dyn Error>> {
    let info = *frame.info();
    let Some(pixels) = frame.pixels() else {
        return Ok((finder.find(&info, None, kind, at.0, at.1), false));
    };
    let bytes = pixels.bytes();
    std::thread::scope(|s| {
        // A thread a frame: starting one costs tens of microseconds, against
        // the milliseconds the detector takes.
        let worker = s.spawn(|| finder.find(&info, Some(bytes), kind, at.0, at.1));
        let uploaded = upload(pl, &info, bytes, kind, tunnel, sources, stages);
        let found = worker
            .join()
            .unwrap_or_else(|e| std::panic::resume_unwind(e));
        Ok((found, uploaded?))
    })
}

/// The first half of a frame: its bytes into the GPU, unpacked into planes
/// (copy and unpack). Needs nothing from the detector, so it runs while the
/// detector reads the same bytes on another thread. `false` for what nothing
/// renders.
fn upload<'p>(
    pl: &'p Placebo,
    info: &FrameInfo,
    bytes: &[u8],
    kind: Kind,
    tunnel: Option<&Tunnel>,
    sources: &mut Sources<'p>,
    stages: &mut Stages,
) -> Result<bool, Box<dyn Error>> {
    let (w, h) = (info.width as usize, info.height as usize);
    let t = Instant::now();
    match kind {
        Kind::Ycbcr(_) | Kind::Tunnel => {
            if kind == Kind::Tunnel && tunnel.is_none() {
                return Ok(false);
            }
            if sources
                .planes
                .as_ref()
                .is_none_or(|p| (p.width(), p.height()) != (w, h))
            {
                sources.planes = Some(pl.planes(w, h)?);
            }
            let p = sources.planes.as_mut().expect("made above");
            if kind == Kind::Tunnel {
                pl.unpack_tunnel(p, bytes, info.row_bytes)?;
            } else {
                pl.unpack_v210(p, bytes, info.row_bytes)?;
            }
        }
        Kind::Rgb(format) => {
            if sources
                .rgb
                .as_ref()
                .is_none_or(|r| (r.width(), r.height()) != (w, h))
            {
                sources.rgb = Some(pl.rgb(w, h)?);
            }
            let r = sources.rgb.as_mut().expect("made above");
            pl.unpack_rgb(r, bytes, info.row_bytes, format)?;
        }
        Kind::Unsupported(_) => return Ok(false),
    }
    stages.unpack.push(ms(t));
    Ok(true)
}

/// The second half: the uploaded frame drawn into the target, with the crop
/// the detector found for this same frame.
fn draw<'p>(
    pl: &'p Placebo,
    info: &FrameInfo,
    content: Content,
    tunnel: Option<&Tunnel>,
    sources: &mut Sources<'p>,
    dst: &Target<'_, '_>,
    stages: &mut Stages,
) -> Result<(), Box<dyn Error>> {
    let view = dst.view;
    let out = dst.color;
    let t = Instant::now();
    match content {
        Content::Sdr(color)
        | Content::Hdr10(color)
        | Content::Hlg(color)
        | Content::Lldv(color) => {
            // Low-latency Dolby Vision is PQ under its SDR label, already
            // mapped by the player to its peak.
            let (color, peak) = match content {
                Content::Lldv(_) => (
                    YcbcrColor {
                        transfer: Transfer::Pq,
                        ..color
                    },
                    Some(dst.white_nits),
                ),
                _ => {
                    if color.transfer == Transfer::Pq
                        && let Some(h) = info.color.hdr.as_ref()
                    {
                        // MaxCLL, or the mastering display's peak without it.
                        let p = h
                            .max_cll
                            .filter(|&v| v > 0.0)
                            .or(h.max_mastering.filter(|&v| v > 0.0));
                        if let Some(p) = p {
                            sources.pq_peak = Some(p as f32);
                        }
                    }
                    (color, dst.source_peak.or(sources.pq_peak))
                }
            };
            let p = sources.planes.as_mut().expect("uploaded");
            // HDR into SDR is tone-mapped; anything else only changes space.
            let map =
                (color.transfer == Transfer::Pq && out.transfer == Transfer::Bt1886).then(|| {
                    Mapping {
                        // An HDR10 disc without MaxCLL or mastering: 1000 nits,
                        // the common grade.
                        src_peak: peak.unwrap_or(1000.0),
                        dst_peak: dst.white_nits,
                        curve: dst.curve,
                    }
                });
            pl.render_ycbcr_mapped(p, color, dst.tex, out, view, map.as_ref())?;
        }
        Content::Tunnel => {
            let tunnel = tunnel.expect("uploaded only with one");
            let p = sources.planes.as_mut().expect("uploaded");
            // Into SDR from the grade's own peak, the whole title on one
            // curve: the scene-by-scene metadata is not used yet.
            let map = (out.transfer == Transfer::Bt1886).then(|| Mapping {
                src_peak: cinefit_dovi::pq_to_nits(tunnel.source_max_pq) as f32,
                dst_peak: dst.white_nits,
                curve: dst.curve,
            });
            pl.render_dovi_mapped(p, &tunnel.color, dst.tex, out, view, map.as_ref())?;
        }
        Content::Rgb(_) => {
            let r = sources.rgb.as_mut().expect("uploaded");
            // The card does not report RGB's range: the setting says (#23).
            let color = RgbSourceColor {
                levels: dst.rgb_range,
                primaries: ycbcr_color(info).primaries,
                transfer: ycbcr_color(info).transfer,
            };
            pl.render_rgb_view(r, color, dst.tex, out, view)?;
        }
        Content::Unsupported(_) => unreachable!("never uploaded"),
    }
    stages.render.push(ms(t));
    Ok(())
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
        log::error!(target: runlog::FILE_ONLY, "{e}");
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
    let devices = cinefit_decklink::devices()?;
    let device = devices
        .get(opts.device)
        .ok_or_else(|| format!("no device {}: {} found", opts.device, devices.len()))?;
    info!(
        "capture: device {}: {}",
        opts.device,
        device.info().display_name
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
    let mut out = cinefit_kms::Output::open(opts.card.as_deref())?;
    info!("output: {}", out.describe());
    let modes = out.modes();
    let want = cinefit_kms::ModeRequest {
        width: cfg.width,
        height: cfg.height,
        refresh_hz: start
            .as_ref()
            .map_or(cfg.wait_hz, |s| s.2.frames_per_second()),
    };
    let Some(index) = cinefit_kms::choose_mode(&modes, &want) else {
        info!("progressive modes offered:");
        for m in modes.iter().filter(|m| !m.interlaced) {
            info!("  {m}{}", if m.preferred { " (preferred)" } else { "" });
        }
        return Err(format!(
            "no {}x{} mode within 0.01 Hz of {:.3}",
            want.width, want.height, want.refresh_hz
        )
        .into());
    };
    let mode = modes[index];
    info!("mode: {mode}");
    // What the first input is, resolved once (#99).
    let start_content = start.as_ref().map(|s| Content::of(s.0.kind, cfg.lldv));
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
                    cfg.white_nits
                ),
                Some(Content::Tunnel | Content::Hdr10(_)) => format!(
                    ": HDR tone-mapped ({}) to white at {} nits",
                    cfg.curve.name(),
                    cfg.white_nits
                ),
                _ => String::new(),
            }
        ),
    }
    let size = (u32::from(mode.width), u32::from(mode.height));

    let _clocks = if opts.pstate {
        match cinefit_render::power::StablePstate::acquire() {
            Ok(p) => {
                info!("gpu clocks: {} (stable pstate, held until exit)", p.level());
                Some(p)
            }
            Err(e) => {
                info!("gpu clocks: not held: {e}");
                None
            }
        }
    } else {
        info!("gpu clocks: default policy (--no-pstate)");
        None
    };

    let pl = Placebo::new()?;
    let mut buffers = Vec::new();
    // The scanout buffers' pixel format, to read one back for the info panel.
    let mut scan_fourcc = 0;
    let mut targets = Vec::new();
    for _ in 0..BUFFERS {
        let b = out.create_scanout(index)?;
        let d = out.export(b)?;
        scan_fourcc = d.fourcc;
        targets.push(pl.import_dmabuf(&cinefit_render::DmaBufTarget {
            fd: d.fd.as_fd(),
            width: d.width,
            height: d.height,
            fourcc: d.fourcc,
            modifier: d.modifier,
            stride: d.stride,
            offset: d.offset,
        })?);
        buffers.push(b);
    }

    // The first picture is rendered before the modeset, so the display's
    // first frame is the source's.
    let mut sources = Sources::default();
    let mut stages = Stages::default();

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
    };
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
            let (found, uploaded) = detect_and_upload(
                &pl,
                &mut finder,
                first,
                key.kind,
                last_tunnel.as_ref(),
                &mut sources,
                &mut stages,
                (0.0, 1),
            )?;
            noted(&found, &first_info, 0.0);
            let source = found.source;
            let view = view_for(
                (first_info.width, first_info.height),
                size,
                win,
                source,
                cfg.fill_max_ar.value(),
            );
            if !uploaded {
                return Err("the first frame could not be rendered".into());
            }
            draw(
                &pl,
                &first_info,
                Content::of(key.kind, cfg.lldv),
                last_tunnel.as_ref(),
                &mut sources,
                &target(&targets, 0, out_color, view, &cfg),
                &mut stages,
            )?;
            let place = view
                .dst
                .unwrap_or_else(|| Rect::fit(first_info.width, first_info.height, size.0, size.1));
            (source, place)
        }
        None => {
            pl.clear_black(&targets[0])?;
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
        let fourcc = out.export(buffers[0])?.fourcc;
        let report = out.read_scanout(buffers[0], |px, stride| {
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
    out.modeset(index, buffers[0])?;
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
    // Pacing per output mode: a modeset changes the period it is judged by.
    let mut finished: Vec<(String, Duration, cinefit_kms::Pacing)> = Vec::new();
    let mut mode_name = mode.to_string();
    let mut latest_want: Option<follow::Want> = None;
    let (mut front, mut pending) = (0usize, 0usize);
    ui.update(&mut out, enc, Instant::now());
    out.flip(buffers[front])?;
    let mut ready: Option<usize> = None;
    let mut counts = Counts::default();
    // The input as last seen, and for how many frames in a row.
    let (mut last_key, mut held) = (start_key, start_key.map_or(0, frames_to_trust));
    let mut settled: Option<(Counts, u64)> = None;
    // What the card last reported, to log when it reports something else.
    let mut last_format = start_input;
    // Black is on screen for want of a signal, and since when frames last
    // came. Each return of the signal is a new epoch for the detector.
    let (mut dark, mut last_frame_at, mut returns) = (start_key.is_none(), Instant::now(), 0u32);
    let mut next_status = STATUS_EVERY;
    let mut last_source = first_source;
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
                        .read_scanout(buffers[front], |px, stride| {
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
                                cfg.lldv.then_some(cfg.white_nits)
                            ),
                            shown
                                .and_then(|v| out_nits(v, signal, cfg.white_nits))
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
                    _ => {}
                }
            }
            if let Some(w) = &web {
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
                    "detect.crop" => finder.on = cfg.crop,
                    "geometry.screen_ar" => {
                        win = geo::screen_window(size.0, size.1, cfg.screen_ar.value());
                        ui.set_band((win.y, win.h));
                    }
                    "osd.banners" => ui.set_banners(cfg.banners),
                    "slate.venue" => ui.set_venue(&cfg.venue),
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
                // Only while the panel is open: mapping the output buffer
                // copies all of it off the GPU, ~13 ms.
                let shown = if ui.info_open() {
                    out.read_scanout(buffers[front], |px, stride| {
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
                let text = InfoText {
                    sections: vec![
                        input_section(
                            picture.map(|(c, k)| (c, k.width, k.height)),
                            last_format,
                            cfg.lldv.then_some(cfg.white_nits),
                            brightest,
                        ),
                        output_section(
                            picture.map(|(c, _)| c),
                            policy.committed().signal,
                            cur_mode,
                            shown,
                            cfg.white_nits,
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
                if let Some(w) = &web {
                    let pill = match last_key.filter(|_| !dark).map(|k| k.kind) {
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
                        }),
                        &cfg,
                    );
                }
                ui.set_info(text);
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
                pl.clear_black(&targets[free])?;
                ready = Some(free);
                dark = true;
                counts.signal_losses += 1;
                ui.set_slate(Some(Slate::SignalLost {
                    since: last_frame_at,
                }));
                info!(
                    "{:.1} s: no signal for {} ms: black",
                    run_started.elapsed().as_secs_f64(),
                    last_frame_at.elapsed().as_millis()
                );
            }
            if let Some(frame) = taken {
                if dark {
                    returns += 1;
                    info!("{:.1} s: signal back", run_started.elapsed().as_secs_f64());
                    dark = false;
                    wait_changed = false;
                }
                last_frame_at = Instant::now();
                let k = classify(&frame, &mut last_tunnel);
                // What this frame is, resolved once for everything below.
                let content = Content::of(k.kind, cfg.lldv);
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
                    pl.clear_black(&targets[free])?;
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
                    // A PQ frame without its metadata block is not a change of
                    // metadata: the card attaches it only to frames it flags, and
                    // treating its absence as zeros would flap the output.
                    let pq_without_metadata =
                        matches!(content, Content::Hdr10(_)) && frame.info().color.hdr.is_none();
                    if !matches!(k.kind, Kind::Unsupported(_))
                        && !pq_without_metadata
                        && let Some(f) = *format.lock().unwrap_or_else(PoisonError::into_inner)
                    {
                        let (hdr, _) = output_in(
                            cfg.mode,
                            content,
                            frame.info().color.hdr.as_ref(),
                            last_tunnel.as_ref(),
                        );
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
                    let free = (0..BUFFERS)
                        .find(|&b| b != front && b != pending)
                        .expect("three buffers, two in use");
                    // Every change of input format is a new epoch for the
                    // detector, which forgets what it learned.
                    let started = Instant::now();
                    let t = run_started.elapsed().as_secs_f64();
                    let frame_info = *frame.info();
                    let (found, uploaded) = detect_and_upload(
                        &pl,
                        &mut finder,
                        &frame,
                        k.kind,
                        last_tunnel.as_ref(),
                        &mut sources,
                        &mut stages,
                        (t, counts.format_changes as u32 + returns + 1),
                    )?;
                    noted(&found, &frame_info, t);
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
                    if uploaded {
                        draw(
                            &pl,
                            &frame_info,
                            content,
                            last_tunnel.as_ref(),
                            &mut sources,
                            &target(&targets, free, out_color, view, &cfg),
                            &mut stages,
                        )?;
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
                        pl.clear_black(&targets[free])?;
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
            let flip = out.wait_flip()?;
            if flip.waited > cinefit_kms::FLIP_LATE {
                warn!(
                    "{:.1} s: a flip came {:.0} ms after it was waited for",
                    run_started.elapsed().as_secs_f64(),
                    flip.waited.as_secs_f64() * 1000.0
                );
            }
            pacing.on_flip(flip.sequence, flip.at);
            front = pending;
            // Nothing is in flight now: the moment for a modeset or new metadata.
            let now = run_started.elapsed();
            match latest_want.take().and_then(|w| policy.next(&w, now)) {
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
                            let show = ready.unwrap_or(front);
                            out.modeset(i, buffers[show])
                                .map(|()| (i, show))
                                .map_err(|e| e.to_string())
                        }
                    };
                    match result {
                        Ok((i, show)) => {
                            policy.done(&a, true, now);
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
                            out.flip(buffers[pending])?;
                            continue;
                        }
                        Err(e) => {
                            policy.done(&a, false, now);
                            counts.refused += 1;
                            info!(
                                "{:.1} s: could not follow the source: {e}",
                                now.as_secs_f64()
                            );
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
            match ready.take() {
                Some(b) => {
                    counts.shown += 1;
                    pending = b;
                    window.borrow_mut().flip(Instant::now(), true);
                }
                None => {
                    counts.repeats += 1;
                    pending = front;
                    window.borrow_mut().flip(Instant::now(), false);
                }
            }
            ui.update(&mut out, enc, Instant::now());
            out.flip(buffers[pending])?;
        }
        Ok(())
    };
    let looped = run_loop();
    let ran = run_started.elapsed();
    // After a failed flip no flip event may come: do not wait for one.
    if looped.is_ok() {
        let _ = out.wait_flip();
    }
    match out.restore() {
        Ok(restored) => info!("{restored}; now: {}", out.hdr_state()),
        Err(e) => warn!("could not restore the display: {e}"),
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
    info!(
        "{}",
        stages
            .render
            .line("render, with the wait for the GPU")
            .trim_end()
    );
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

#[cfg(test)]
mod tests {
    use super::*;

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

        // RGB at a new size, then the tunnel: full frame, and one change
        // logged for it, not one a frame.
        let rgb_info = FrameInfo {
            width: 3840,
            height: 2160,
            ..info
        };
        let mut changes = Vec::new();
        for (i, kind) in [
            Kind::Rgb(rgb::Format::R210),
            Kind::Rgb(rgb::Format::R210),
            Kind::Tunnel,
        ]
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
