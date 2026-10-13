//! `cinefit play`: capture to screen. The capture callback puts each frame
//! in a latest-wins mailbox; this thread takes the newest, unpacks and
//! renders it into a free scanout buffer ahead of the vblank, and flips.
//!
//! The output is chosen once, before the modeset, from a pre-roll of stable
//! input: the source's own frame rate, and its HDR signalling in the same
//! commit, so the display resyncs once. Changes of input format during the
//! run are counted, not followed yet.

use std::error::Error;
use std::os::fd::AsFd;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::config::OutputMode;
use crate::follow;
use crate::runlog::{self, say};
use cinefit_decklink::{
    Colorspace, Eotf, Frame, FrameInfo, Handler, HdrMetadata, InputFormat, Mailbox, PixelFormat,
};
use cinefit_detect::{Detector, V210};
use cinefit_geometry as geo;
use cinefit_render::{
    Curve, DoviColor, GpuPlanes, GpuRgb, Levels, Mapping, Matrix, Placebo, Primaries, Rect,
    RgbColor, RgbSourceColor, Texture, Transfer, View, YcbcrColor, rgb,
};

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
/// Give up waiting for stable input after this long.
const PRE_ROLL_LIMIT: Duration = Duration::from_secs(15);
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
    pub seconds: f64,
    pub device: usize,
    pub card: Option<PathBuf>,
    /// The settings: the panel's size (the output runs at it, at the
    /// source's rate) and the RGB input range.
    pub config: crate::config::Config,
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

/// What a frame is, as far as rendering goes.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Ycbcr(YcbcrColor),
    Rgb(rgb::Format),
    Tunnel,
    /// A capture format nothing renders.
    Unsupported(PixelFormat),
}

/// The input as the output is chosen from: two frames with equal keys need
/// the same output.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Key {
    width: u32,
    height: u32,
    kind: Kind,
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

/// A decoded tunnel: its conversion, and the grade's range for the output's
/// static metadata.
#[derive(Debug, Clone, Copy)]
struct Tunnel {
    color: DoviColor,
    source_min_pq: u16,
    source_max_pq: u16,
    /// This scene's brightest maxRGB, as a 12-bit PQ code (level 1
    /// `max_PQ`): the same measure as MaxCLL, per scene.
    scene_max_pq: Option<u16>,
    /// This scene's average maxRGB, as a 12-bit PQ code (level 1 `avg_PQ`),
    /// sent as MaxFALL.
    scene_avg_pq: Option<u16>,
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

/// What the output signals and renders into for an input.
fn output_for(
    kind: Kind,
    hdr: Option<&HdrMetadata>,
    tunnel: Option<&Tunnel>,
) -> (Option<cinefit_kms::HdrStatic>, RgbColor) {
    const BT2020: [(f64, f64); 3] = [(0.708, 0.292), (0.170, 0.797), (0.131, 0.046)];
    const D65: (f64, f64) = (0.3127, 0.3290);
    let pq_out = RgbColor {
        primaries: Primaries::Bt2020,
        transfer: Transfer::Pq,
    };
    let base = cinefit_kms::HdrStatic {
        eotf: cinefit_kms::Eotf::Pq,
        primaries: BT2020,
        white: D65,
        max_mastering: 0.0,
        min_mastering: 0.0,
        max_cll: 0.0,
        max_fall: 0.0,
    };
    match kind {
        // The grade's range as mastering, and this scene's peak as MaxCLL:
        // the JVC re-derives its curve from a MaxCLL change mid-stream with
        // no resync (measured 2026-10-12), so the curve follows the film
        // scene by scene. Capped at the grade's peak. With no level 1 yet,
        // the grade's peak too: 0 is "unknown", and was sent at the start of
        // a title until the first scene arrived (2026-10-13).
        Kind::Tunnel => {
            let mastering = tunnel.map_or(0.0, |t| cinefit_dovi::pq_to_nits(t.source_max_pq));
            let scene = tunnel
                .and_then(|t| t.scene_max_pq)
                .map_or(mastering, cinefit_dovi::pq_to_nits);
            (
                Some(cinefit_kms::HdrStatic {
                    max_mastering: mastering,
                    min_mastering: tunnel
                        .map_or(0.0, |t| cinefit_dovi::pq_to_nits(t.source_min_pq)),
                    // Whole nits: the blob carries no finer, and a value that
                    // rounds the same is no change to send.
                    max_cll: if mastering > 0.0 {
                        scene.min(mastering)
                    } else {
                        scene
                    }
                    .round(),
                    // An average taken in PQ, so below MaxFALL's linear one;
                    // the nearest measure the metadata has. 0, "unknown",
                    // with no level 1. Whether the JVC reads MaxFALL at all is
                    // not yet measured.
                    max_fall: tunnel
                        .and_then(|t| t.scene_avg_pq)
                        .map_or(0.0, cinefit_dovi::pq_to_nits)
                        .round(),
                    ..base
                }),
                pq_out,
            )
        }
        Kind::Ycbcr(c) if c.transfer == Transfer::Pq => {
            // The source's own values; any it does not send stay unknown (0)
            // or default to BT.2020 D65.
            let xy = |c: Option<cinefit_decklink::Chromaticity>, d: (f64, f64)| {
                c.map_or(d, |c| (c.x, c.y))
            };
            let field = |f: fn(&HdrMetadata) -> Option<f64>| hdr.and_then(f).unwrap_or(0.0);
            (
                Some(cinefit_kms::HdrStatic {
                    primaries: [
                        xy(hdr.and_then(|h| h.red), BT2020[0]),
                        xy(hdr.and_then(|h| h.green), BT2020[1]),
                        xy(hdr.and_then(|h| h.blue), BT2020[2]),
                    ],
                    white: xy(hdr.and_then(|h| h.white), D65),
                    max_mastering: field(|h| h.max_mastering),
                    min_mastering: field(|h| h.min_mastering),
                    max_cll: field(|h| h.max_cll),
                    max_fall: field(|h| h.max_fall),
                    ..base
                }),
                pq_out,
            )
        }
        Kind::Ycbcr(c) if c.transfer == Transfer::Hlg => (
            Some(cinefit_kms::HdrStatic {
                eotf: cinefit_kms::Eotf::Hlg,
                ..base
            }),
            RgbColor {
                primaries: Primaries::Bt2020,
                transfer: Transfer::Hlg,
            },
        ),
        _ => (
            None,
            RgbColor {
                primaries: Primaries::Bt709,
                transfer: Transfer::Bt1886,
            },
        ),
    }
}

/// SDR in a BT.2020 container: what the output sends unless HDR10 is passed
/// through.
const SDR_BT2020: RgbColor = RgbColor {
    primaries: Primaries::Bt2020,
    transfer: Transfer::Bt1886,
};

/// Whether a frame is a player's low-latency Dolby Vision: YCbCr labelled SDR
/// BT.2020, when the `input.lldv` setting says that label means it.
fn is_lldv(kind: Kind, lldv: bool) -> bool {
    matches!(kind, Kind::Ycbcr(c) if lldv && c.primaries == Primaries::Bt2020 && c.transfer == Transfer::Bt1886)
}

/// The output for an input under the `output.mode` setting. Dolby Vision, as
/// a tunnel or already mapped by the player, always goes out as SDR BT.2020:
/// passed through it would be HDR10 with derived metadata, which the display
/// maps by its own rules. Passthrough is for HDR10 (and HLG) only.
fn output_in(
    mode: OutputMode,
    lldv: bool,
    kind: Kind,
    hdr: Option<&HdrMetadata>,
    tunnel: Option<&Tunnel>,
) -> (Option<cinefit_kms::HdrStatic>, RgbColor) {
    let sdr = (None, SDR_BT2020);
    if kind == Kind::Tunnel || is_lldv(kind, lldv) || mode == OutputMode::Sdr2020 {
        return sdr;
    }
    match output_for(kind, hdr, tunnel) {
        (Some(h), color) => (Some(h), color),
        (None, _) => sdr,
    }
}

/// The policy's view of an output's signalling.
fn signal_of(hdr: Option<cinefit_kms::HdrStatic>) -> follow::Signal {
    hdr.map_or(follow::Signal::Sdr, follow::Signal::Hdr)
}

/// What a frame renders into for a signalling: BT.2020 PQ or HLG for HDR,
/// SDR BT.2020 otherwise.
fn color_for(signal: follow::Signal) -> RgbColor {
    match signal {
        follow::Signal::Hdr(h) if h.eotf == cinefit_kms::Eotf::Hlg => RgbColor {
            primaries: Primaries::Bt2020,
            transfer: Transfer::Hlg,
        },
        follow::Signal::Hdr(_) => RgbColor {
            primaries: Primaries::Bt2020,
            transfer: Transfer::Pq,
        },
        follow::Signal::Sdr => RgbColor {
            primaries: Primaries::Bt2020,
            transfer: Transfer::Bt1886,
        },
    }
}

/// A source's frame rate in millihertz, from its exact fraction.
fn rate_mhz(f: &InputFormat) -> u32 {
    if f.frame_duration <= 0 {
        return 0;
    }
    ((f.time_scale as f64 * 1000.0) / f.frame_duration as f64).round() as u32
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
    unpack: Vec<f64>,
    render: Vec<f64>,
}

impl Stages {
    fn line(name: &str, v: &mut [f64]) -> String {
        if v.is_empty() {
            return format!("{name}: none\n");
        }
        v.sort_by(f64::total_cmp);
        let at = |q: f64| v[((v.len() - 1) as f64 * q) as usize];
        format!(
            "{name}: median {:.2} ms, p95 {:.2}, max {:.2}\n",
            at(0.5),
            at(0.95),
            at(1.0)
        )
    }
}

/// Where a frame is rendered: a scanout buffer of the output's size, in the
/// output's colour.
struct Target<'a, 'p> {
    tex: &'a Texture<'p>,
    color: RgbColor,
    /// What plain RGB input's codes mean (the `input.rgb_range` setting).
    rgb_range: Levels,
    /// HDR into SDR: what SDR white stands for, the curve, and the peak of
    /// low-latency Dolby Vision when that input is on (the settings).
    white_nits: f32,
    curve: Curve,
    lldv_nits: Option<f32>,
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

/// Unpacks `frame` and renders it, its shape kept, into `dst`. Returns false
/// for a frame that cannot be rendered (an unsupported format, a tunnel with
/// no conversion yet), leaving `dst` as it was.
fn render<'p>(
    pl: &'p Placebo,
    frame: &Frame,
    kind: Kind,
    tunnel: Option<&Tunnel>,
    sources: &mut Sources<'p>,
    dst: &Target<'_, '_>,
    stages: &mut Stages,
) -> Result<bool, Box<dyn Error>> {
    let info = frame.info();
    let (w, h) = (info.width as usize, info.height as usize);
    let Some(pixels) = frame.pixels() else {
        return Ok(false);
    };
    let view = dst.view;
    let out = dst.color;
    let planes = |s: &mut Sources<'p>| -> Result<(), Box<dyn Error>> {
        if s.planes
            .as_ref()
            .is_none_or(|p| (p.width(), p.height()) != (w, h))
        {
            s.planes = Some(pl.planes(w, h)?);
        }
        Ok(())
    };
    let t = Instant::now();
    match kind {
        Kind::Ycbcr(color) => {
            // Low-latency Dolby Vision is PQ under its SDR label, already
            // mapped by the player to its peak.
            let (color, peak) = match dst.lldv_nits {
                Some(nits) if is_lldv(kind, true) => (
                    YcbcrColor {
                        transfer: Transfer::Pq,
                        ..color
                    },
                    Some(nits),
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
            planes(sources)?;
            let p = sources.planes.as_mut().expect("made above");
            pl.unpack_v210(p, pixels.bytes(), info.row_bytes)?;
            stages.unpack.push(ms(t));
            let t = Instant::now();
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
            stages.render.push(ms(t));
        }
        Kind::Tunnel => {
            let Some(tunnel) = tunnel else {
                return Ok(false);
            };
            planes(sources)?;
            let p = sources.planes.as_mut().expect("made above");
            pl.unpack_tunnel(p, pixels.bytes(), info.row_bytes)?;
            stages.unpack.push(ms(t));
            let t = Instant::now();
            // Into SDR from the grade's own peak, the whole title on one
            // curve: the scene-by-scene metadata is not used yet.
            let map = (out.transfer == Transfer::Bt1886).then(|| Mapping {
                src_peak: cinefit_dovi::pq_to_nits(tunnel.source_max_pq) as f32,
                dst_peak: dst.white_nits,
                curve: dst.curve,
            });
            pl.render_dovi_mapped(p, &tunnel.color, dst.tex, out, view, map.as_ref())?;
            stages.render.push(ms(t));
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
            pl.unpack_rgb(r, pixels.bytes(), info.row_bytes, format)?;
            stages.unpack.push(ms(t));
            let t = Instant::now();
            // The card does not report RGB's range: the setting says (#23).
            let color = RgbSourceColor {
                levels: dst.rgb_range,
                primaries: ycbcr_color(info).primaries,
                transfer: ycbcr_color(info).transfer,
            };
            pl.render_rgb_view(r, color, dst.tex, out, view)?;
            stages.render.push(ms(t));
        }
        Kind::Unsupported(_) => return Ok(false),
    }
    Ok(true)
}

fn ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

/// Runs `play`, recorded in `~/cinefit-logs` (see [`runlog`]). A log that
/// cannot be opened is reported and the run goes on without one.
pub fn run(opts: Options) -> Result<(), Box<dyn Error>> {
    match runlog::default_dir()
        .ok_or_else(|| "HOME is not set".into())
        .and_then(|d| runlog::open(&d, "play").map_err(|e| format!("{}: {e}", d.display())))
    {
        Ok(path) => println!("log: {}", path.display()),
        Err(e) => eprintln!("log: not written: {e}"),
    }
    // The settings as this run read them, in the log only: a question about
    // a run is often which setting was on.
    runlog::record(&format!(
        "settings:\n{}",
        crate::config::render(&opts.config)
    ));
    let result = play(opts);
    if let Err(e) = &result {
        // main prints it to the terminal.
        runlog::record(&format!("error: {e}"));
    }
    result
}

fn play(opts: Options) -> Result<(), Box<dyn Error>> {
    let run_for = Duration::try_from_secs_f64(opts.seconds)
        .ok()
        .filter(|d| !d.is_zero())
        .ok_or("--seconds must be a positive number")?;

    // Capture first: the output is chosen from what arrives.
    let devices = cinefit_decklink::devices()?;
    let device = devices
        .get(opts.device)
        .ok_or_else(|| format!("no device {}: {} found", opts.device, devices.len()))?;
    say!(
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
        if started.elapsed() > PRE_ROLL_LIMIT {
            return Err(format!(
                "no stable input after {} s: {} frames with a signal, {} without",
                PRE_ROLL_LIMIT.as_secs(),
                captured.load(Ordering::Relaxed),
                no_input.load(Ordering::Relaxed)
            )
            .into());
        }
        let Some(frame) = mailbox.take_wait(Duration::from_millis(500)) else {
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
    let key = key.expect("pre-roll ends with a key");
    let first = first.expect("pre-roll ends with a frame");
    let input = format
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .expect("pre-roll waits for the format");
    say!(
        "input: {input}, as {}; stable after {:.1} s",
        describe(key.kind),
        started.elapsed().as_secs_f64()
    );
    if let Kind::Unsupported(f) = key.kind {
        return Err(format!("{f}: no render path").into());
    }

    // The output: the panel at the source's own rate, signalling chosen from
    // the source, in one commit.
    let mut out = cinefit_kms::Output::open(opts.card.as_deref())?;
    say!("output: {}", out.describe());
    let modes = out.modes();
    let want = cinefit_kms::ModeRequest {
        width: opts.config.width,
        height: opts.config.height,
        refresh_hz: input.frames_per_second(),
    };
    let Some(index) = cinefit_kms::choose_mode(&modes, &want) else {
        say!("progressive modes offered:");
        for m in modes.iter().filter(|m| !m.interlaced) {
            say!("  {m}{}", if m.preferred { " (preferred)" } else { "" });
        }
        return Err(format!(
            "no {}x{} mode within 0.01 Hz of the source's {:.3}",
            want.width, want.height, want.refresh_hz
        )
        .into());
    };
    let mode = modes[index];
    say!("mode: {mode}");
    let (hdr, mut out_color) = output_in(
        opts.config.mode,
        opts.config.lldv,
        key.kind,
        first.info().color.hdr.as_ref(),
        last_tunnel.as_ref(),
    );
    match &hdr {
        Some(h) => say!(
            "signalling {:?}: MaxCLL {}, MaxFALL {}, mastering {:.4}-{:.0} nits",
            h.eotf,
            h.max_cll,
            h.max_fall,
            h.min_mastering,
            h.max_mastering
        ),
        None => say!(
            "signalling SDR BT.2020{}",
            match key.kind {
                k if is_lldv(k, opts.config.lldv) => format!(
                    ": low-latency Dolby Vision, {} nits to white at {} nits",
                    opts.config.lldv_nits, opts.config.white_nits
                ),
                Kind::Tunnel
                | Kind::Ycbcr(YcbcrColor {
                    transfer: Transfer::Pq,
                    ..
                }) => format!(
                    ": HDR tone-mapped ({}) to white at {} nits",
                    opts.config.curve.name(),
                    opts.config.white_nits
                ),
                _ => String::new(),
            }
        ),
    }
    let size = (u32::from(mode.width), u32::from(mode.height));

    let _clocks = if opts.pstate {
        match cinefit_render::power::StablePstate::acquire() {
            Ok(p) => {
                say!("gpu clocks: {} (stable pstate, held until exit)", p.level());
                Some(p)
            }
            Err(e) => {
                say!("gpu clocks: not held: {e}");
                None
            }
        }
    } else {
        say!("gpu clocks: default policy (--no-pstate)");
        None
    };

    let pl = Placebo::new()?;
    let mut buffers = Vec::new();
    let mut targets = Vec::new();
    for _ in 0..BUFFERS {
        let b = out.create_scanout(index)?;
        let d = out.export(b)?;
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
    let target = |b: usize, color: RgbColor, view: View| Target {
        tex: &targets[b],
        color,
        rgb_range: opts.config.rgb_range,
        white_nits: f32::from(opts.config.white_nits),
        curve: opts.config.curve,
        lldv_nits: opts.config.lldv.then_some(f32::from(opts.config.lldv_nits)),
        source_peak: (opts.config.source_peak > 0).then_some(f32::from(opts.config.source_peak)),
        view,
    };

    // The bar detector reads each YCbCr frame on this thread (about 8 ms at
    // 2160p) and its crop goes to the geometry: the picture at constant
    // image height in the screen's band of the raster. Other formats, which
    // have no adapter yet, keep the last crop.
    let win = geo::screen_window(size.0, size.1, opts.config.screen_ar);
    if opts.config.crop {
        say!(
            "crop: on; screen {:.2}:1 is rows {}-{} of the raster, fill up to {:.2}:1",
            opts.config.screen_ar,
            win.y,
            win.y + win.h,
            opts.config.fill_max_ar
        );
    }
    let mut det = Detector::default();
    let mut crops = 0u64;
    let mut detect = |frame: &Frame, kind: Kind, t: f64, epoch: u32| -> geo::Source {
        if !opts.config.crop {
            return geo::Source::Whole;
        }
        let info = frame.info();
        if matches!(kind, Kind::Ycbcr(_))
            && let Some(px) = frame.pixels()
            && let Some(v) = V210::new(px.bytes(), info.width, info.height, info.row_bytes)
        {
            let d = det.push(&v, t, epoch);
            if d.changed {
                crops += 1;
                let c = d.crop;
                say!(
                    "{t:.1} s: crop {}/{} ({}): picture {} x {} rows, {:.3}:1",
                    c.top,
                    c.bottom,
                    d.diag.reason,
                    info.width,
                    c.active(info.height),
                    f64::from(info.width) / f64::from(c.active(info.height).max(1))
                );
            }
        }
        let c = det.crop();
        geo::Source::Rows {
            top: c.top,
            bottom: c.bottom,
        }
    };
    let first_info = *first.info();
    let first_source = detect(&first, key.kind, 0.0, 1);
    let first_view = view_for(
        (first_info.width, first_info.height),
        size,
        win,
        first_source,
        opts.config.fill_max_ar,
    );
    if !render(
        &pl,
        &first,
        key.kind,
        last_tunnel.as_ref(),
        &mut sources,
        &target(0, out_color, first_view),
        &mut stages,
    )? {
        return Err("the first frame could not be rendered".into());
    }
    drop(first);
    if opts.dry_run {
        say!(
            "dry run: first frame rendered into a scanout buffer ({:.2} ms unpack, {:.2} ms render)",
            stages.unpack.first().copied().unwrap_or(0.0),
            stages.render.first().copied().unwrap_or(0.0)
        );
        // A signal is not a picture: look at the pixels.
        let fourcc = out.export(buffers[0])?.fourcc;
        let place = first_view
            .dst
            .unwrap_or_else(|| Rect::fit(first_info.width, first_info.height, size.0, size.1));
        let report = out.read_scanout(buffers[0], |px, stride| {
            look(px, stride, fourcc, size, place)
        })?;
        say!("{report}");
        say!("stopping before the modeset");
        return Ok(());
    }
    out.set_hdr(hdr);
    out.set_sdr_bt2020(true);
    out.modeset(index, buffers[0])?;
    say!("after modeset: {}", out.hdr_state());
    let mut policy = follow::Policy::new(follow::Want {
        rate_mhz: rate_mhz(&input),
        signal: signal_of(hdr),
    });

    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let mut presses = 0;
    ctrlc::set_handler(move || {
        presses += 1;
        if presses > 1 {
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
    out.flip(buffers[front])?;
    let mut ready: Option<usize> = None;
    let mut counts = Counts::default();
    // The input as last seen, and for how many frames in a row.
    let (mut last_key, mut held) = (key, frames_to_trust(key));
    let mut settled: Option<(Counts, u64)> = None;
    // What the card last reported, to log when it reports something else.
    let mut last_format = Some(input);
    let mut next_status = STATUS_EVERY;
    let mut last_source = first_source;
    let run_started = Instant::now();
    while run_started.elapsed() < run_for && !stop.load(Ordering::Relaxed) {
        if settled.is_none() && run_started.elapsed() >= SETTLE {
            settled = Some((counts, mailbox.displaced()));
        }
        if run_started.elapsed() >= next_status {
            next_status += STATUS_EVERY;
            say!(
                "{:.0} s: shown {}, vblanks repeated {}, replaced unseen {}, unrendered {}; crop {}",
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
        if ready.is_none()
            && let Some(frame) = mailbox.take_wait(take_timeout)
        {
            let k = classify(&frame, &mut last_tunnel);
            if k != last_key {
                counts.format_changes += 1;
                // Capped: a source that flaps would bury everything else.
                if counts.format_changes <= LOGGED_CHANGES {
                    say!(
                        "{:.1} s: input now {} x {}, {}{}",
                        run_started.elapsed().as_secs_f64(),
                        k.width,
                        k.height,
                        describe(k.kind),
                        hdr_note(frame.info().color.hdr.as_ref())
                    );
                } else if counts.format_changes == LOGGED_CHANGES + 1 {
                    say!("further input changes are counted, not logged");
                }
                last_key = k;
                held = 0;
            }
            let reported = *format.lock().unwrap_or_else(PoisonError::into_inner);
            if reported != last_format {
                if let Some(f) = reported {
                    say!(
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
            } else {
                // What the output should be for this frame, for the policy after
                // the next flip. The rate is the input's, as the card reported it.
                // A PQ frame without its metadata block is not a change of
                // metadata: the card attaches it only to frames it flags, and
                // treating its absence as zeros would flap the output.
                let pq_without_metadata = matches!(k.kind, Kind::Ycbcr(c) if c.transfer == Transfer::Pq)
                    && frame.info().color.hdr.is_none();
                if !matches!(k.kind, Kind::Unsupported(_))
                    && !pq_without_metadata
                    && let Some(f) = *format.lock().unwrap_or_else(PoisonError::into_inner)
                {
                    let (hdr, _) = output_in(
                        opts.config.mode,
                        opts.config.lldv,
                        k.kind,
                        frame.info().color.hdr.as_ref(),
                        last_tunnel.as_ref(),
                    );
                    latest_want = Some(follow::Want {
                        rate_mhz: rate_mhz(&f),
                        signal: signal_of(hdr),
                    });
                }
                let free = (0..BUFFERS)
                    .find(|&b| b != front && b != pending)
                    .expect("three buffers, two in use");
                // Every change of input format is a new epoch for the
                // detector, which forgets what it learned.
                let source = detect(
                    &frame,
                    k.kind,
                    run_started.elapsed().as_secs_f64(),
                    counts.format_changes as u32 + 1,
                );
                last_source = source;
                let view = view_for(
                    (k.width, k.height),
                    size,
                    win,
                    source,
                    opts.config.fill_max_ar,
                );
                // Rendered into the output as it stands, which libplacebo maps
                // to, until the policy moves the output.
                if render(
                    &pl,
                    &frame,
                    k.kind,
                    last_tunnel.as_ref(),
                    &mut sources,
                    &target(free, out_color, view),
                    &mut stages,
                )? {
                    ready = Some(free);
                } else {
                    counts.unrendered += 1;
                }
            }
        }
        let flip = out.wait_flip()?;
        pacing.on_flip(flip.sequence, flip.at);
        front = pending;
        // Nothing is in flight now: the moment for a modeset or new metadata.
        let now = run_started.elapsed();
        match latest_want.take().and_then(|w| policy.next(&w, now)) {
            Some(a @ follow::Action::Modeset(w)) => {
                let request = cinefit_kms::ModeRequest {
                    width: opts.config.width,
                    height: opts.config.height,
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
                        say!(
                            "{:.1} s: followed the source: {} with {}",
                            now.as_secs_f64(),
                            modes[i],
                            match w.signal {
                                follow::Signal::Sdr => "SDR".to_string(),
                                follow::Signal::Hdr(h) =>
                                    format!("{:?}, MaxCLL {}", h.eotf, h.max_cll),
                            }
                        );
                        finished.push((
                            std::mem::replace(&mut mode_name, modes[i].to_string()),
                            period,
                            std::mem::take(&mut pacing),
                        ));
                        period = modes[i].period();
                        take_timeout = period.mul_f64(0.8).min(Duration::from_millis(35));
                        out_color = color_for(w.signal);
                        (front, pending) = (show, show);
                        // A modeset sends no flip event; queue one so the
                        // vblanks keep coming.
                        out.flip(buffers[pending])?;
                        continue;
                    }
                    Err(e) => {
                        policy.done(&a, false, now);
                        counts.refused += 1;
                        say!(
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
                say!(
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
            }
            None => {
                counts.repeats += 1;
                pending = front;
            }
        }
        out.flip(buffers[pending])?;
    }
    let ran = run_started.elapsed();
    let _ = out.wait_flip();
    let restored = out.restore()?;
    say!("{restored}; now: {}", out.hdr_state());
    let failed_restarts = capture.failed_restarts();
    drop(capture);

    if stop.load(Ordering::Relaxed) {
        say!("stopped early by Ctrl-C");
    }
    say!(
        "ran {:.1} s: captured {} frames with a signal ({} without), shown {}, replaced unseen {}, vblanks repeated {}, unrendered {}, input format changes {}, RGB held back as a possible tunnel {}, failed restarts {failed_restarts}",
        ran.as_secs_f64(),
        captured.load(Ordering::Relaxed),
        no_input.load(Ordering::Relaxed),
        counts.shown,
        mailbox.displaced(),
        counts.repeats,
        counts.unrendered,
        counts.format_changes,
        counts.unproven,
    );
    match settled {
        Some((at, displaced)) => {
            let repeats = counts.repeats - at.repeats;
            let lost = mailbox.displaced() - displaced;
            say!(
                "after the first {} s: replaced unseen {lost}, vblanks repeated {repeats}",
                SETTLE.as_secs()
            );
        }
        None => say!("shorter than the {} s settle: no verdict", SETTLE.as_secs()),
    }
    say!(
        "followed the source: {} modesets, {} metadata updates without one, {} refused",
        counts.modesets,
        counts.metadata,
        counts.refused
    );
    if opts.config.crop {
        let shares: Vec<String> = det
            .policy()
            .shares()
            .iter()
            .map(|s| format!("{} rows {:.1} s", s.rows, s.seconds))
            .collect();
        say!(
            "crop: {crops} changes; time at each height: {}",
            shares.join(", ")
        );
    }
    finished.push((mode_name, period, pacing));
    for (name, period, pacing) in &finished {
        say!("{name}: {}", pacing.summary(*period));
    }
    runlog::line(&Stages::line("unpack, with upload", &mut stages.unpack));
    runlog::line(&Stages::line(
        "render, with the wait for the GPU",
        &mut stages.render,
    ));
    Ok(())
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
    modesets: u64,
    metadata: u64,
    refused: u64,
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

fn describe(kind: Kind) -> String {
    match kind {
        Kind::Ycbcr(c) => format!("YCbCr {:?} {:?} {:?}", c.matrix, c.primaries, c.transfer),
        Kind::Rgb(f) => format!("RGB {f:?}, range from input.rgb_range"),
        Kind::Tunnel => "Dolby Vision tunnel".into(),
        Kind::Unsupported(f) => format!("{f}, unsupported"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ycbcr(transfer: Transfer) -> Kind {
        Kind::Ycbcr(YcbcrColor {
            matrix: Matrix::Bt2020Nc,
            primaries: Primaries::Bt2020,
            transfer,
        })
    }

    #[test]
    fn dolby_vision_is_never_passed_through() {
        use OutputMode::{Passthrough, Sdr2020};
        let lldv = ycbcr(Transfer::Bt1886);
        let hdr10 = ycbcr(Transfer::Pq);
        for mode in [Sdr2020, Passthrough] {
            assert_eq!(
                output_in(mode, true, Kind::Tunnel, None, None),
                (None, SDR_BT2020)
            );
            assert_eq!(output_in(mode, true, lldv, None, None), (None, SDR_BT2020));
        }
        // HDR10 is passed through only when asked.
        assert_eq!(
            output_in(Sdr2020, true, hdr10, None, None),
            (None, SDR_BT2020)
        );
        let (h, out) = output_in(Passthrough, true, hdr10, None, None);
        assert_eq!(h.map(|h| h.eotf), Some(cinefit_kms::Eotf::Pq));
        assert_eq!(out.transfer, Transfer::Pq);
        // With the LLDV reading off, SDR BT.2020 is just SDR.
        assert!(!is_lldv(lldv, false));
        assert_eq!(
            output_in(Passthrough, false, lldv, None, None),
            (None, SDR_BT2020)
        );
    }

    #[test]
    fn hdr10_passes_the_sources_values_through() {
        let hdr = HdrMetadata {
            red: Some(cinefit_decklink::Chromaticity { x: 0.68, y: 0.32 }),
            green: None,
            blue: None,
            white: None,
            max_mastering: Some(1000.0),
            min_mastering: Some(0.005),
            max_cll: Some(637.0),
            max_fall: Some(148.0),
        };
        let (h, out) = output_for(ycbcr(Transfer::Pq), Some(&hdr), None);
        let h = h.expect("HDR");
        assert_eq!(h.eotf, cinefit_kms::Eotf::Pq);
        assert_eq!((h.max_cll, h.max_fall), (637.0, 148.0));
        assert_eq!((h.max_mastering, h.min_mastering), (1000.0, 0.005));
        // Sent where given, BT.2020 where not.
        assert_eq!(h.primaries[0], (0.68, 0.32));
        assert_eq!(h.primaries[1], (0.170, 0.797));
        assert_eq!(out.transfer, Transfer::Pq);
    }

    #[test]
    fn hdr10_without_metadata_signals_unknowns() {
        let (h, _) = output_for(ycbcr(Transfer::Pq), None, None);
        let h = h.expect("HDR");
        assert_eq!((h.max_cll, h.max_fall, h.max_mastering), (0.0, 0.0, 0.0));
    }

    #[test]
    fn a_tunnel_signals_pq_with_its_grade_as_mastering_and_its_scene_as_maxcll() {
        let t = Tunnel {
            color: DoviColor {
                ycc_to_lms: [[0.0; 3]; 3],
                offset: [0.0; 3],
                linear: [[0.0; 3]; 3],
            },
            source_min_pq: 62,
            source_max_pq: 3696,
            scene_max_pq: None,
            scene_avg_pq: None,
        };
        let (h, out) = output_for(Kind::Tunnel, None, Some(&t));
        let h = h.expect("HDR");
        assert_eq!(h.eotf, cinefit_kms::Eotf::Pq);
        assert!(
            (h.max_mastering - 4000.0).abs() < 10.0,
            "{}",
            h.max_mastering
        );
        assert!(h.min_mastering < 0.01);
        // No level 1: the grade's peak, never 0 ("unknown").
        assert!((h.max_cll - 4000.0).abs() < 10.0, "{}", h.max_cll);
        // No level 1, no average: MaxFALL unknown.
        assert_eq!(h.max_fall, 0.0);
        // A scene peaking at PQ 3079 (about 1000 nits) sends that as MaxCLL;
        // one brighter than the grade is capped at the grade's peak.
        let scene = |pq| Tunnel {
            scene_max_pq: Some(pq),
            ..t
        };
        let (h, _) = output_for(Kind::Tunnel, None, Some(&scene(3079)));
        assert!((h.expect("HDR").max_cll - 1001.0).abs() < 2.0);
        let (h, _) = output_for(Kind::Tunnel, None, Some(&scene(4000)));
        assert!((h.expect("HDR").max_cll - 4000.0).abs() < 10.0);
        // The scene's average, PQ 2081 (about 100 nits), goes out as MaxFALL.
        let avg = Tunnel {
            scene_avg_pq: Some(2081),
            ..scene(3079)
        };
        let (h, _) = output_for(Kind::Tunnel, None, Some(&avg));
        assert!((h.expect("HDR").max_fall - 100.0).abs() < 1.5);
        assert_eq!(
            (out.primaries, out.transfer),
            (Primaries::Bt2020, Transfer::Pq)
        );
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

    #[test]
    fn hlg_and_sdr() {
        let (h, out) = output_for(ycbcr(Transfer::Hlg), None, None);
        assert_eq!(h.map(|h| h.eotf), Some(cinefit_kms::Eotf::Hlg));
        assert_eq!(out.transfer, Transfer::Hlg);
        let (h, out) = output_for(ycbcr(Transfer::Bt1886), None, None);
        assert!(h.is_none());
        assert_eq!(
            (out.primaries, out.transfer),
            (Primaries::Bt709, Transfer::Bt1886)
        );
        // Plain RGB is signalled as SDR for now.
        assert!(
            output_for(Kind::Rgb(rgb::Format::R210), None, None)
                .0
                .is_none()
        );
    }
}
