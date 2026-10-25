// Unsafe code lives in the wrapper crates (cinefit-decklink), behind safe APIs.
#![forbid(unsafe_code)]

#[cfg(target_os = "linux")]
mod appletv;
mod avr;
#[cfg(target_os = "linux")]
mod bench;
mod capture;
mod compare;
mod config;
#[cfg(target_os = "linux")]
mod content;
#[cfg(target_os = "linux")]
mod dataset;
mod device;
mod dovi_meta;
mod follow;
mod hindsight;
#[cfg(target_os = "linux")]
mod menu;
#[cfg(target_os = "linux")]
mod osd;
#[cfg(target_os = "linux")]
mod pairfollow;
mod pairs;
#[cfg(target_os = "linux")]
mod play;
mod record;
mod replay;
#[cfg(target_os = "linux")]
mod runlog;
mod snapshot;
mod splitter;
#[cfg(target_os = "linux")]
mod status;
#[cfg(target_os = "linux")]
mod times;
#[cfg(target_os = "linux")]
mod tunnel_check;
mod ub820;
#[cfg(target_os = "linux")]
mod ui;
#[cfg(target_os = "linux")]
mod unpack_check;
#[cfg(target_os = "linux")]
mod web;

/// An error and its causes, as one line ("what: cause: its cause"): what a
/// person reads, where `{e}` would show only the outermost.
pub(crate) struct Chain<'a>(&'a (dyn std::error::Error + 'a));

pub(crate) fn chain<'a>(e: &'a (dyn std::error::Error + 'a)) -> Chain<'a> {
    Chain(e)
}

impl std::fmt::Display for Chain<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)?;
        let mut cause = self.0.source();
        while let Some(c) = cause {
            write!(f, ": {c}")?;
            cause = c.source();
        }
        Ok(())
    }
}

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};

/// This build: its version, commit and the commit's date (`build.rs`).
pub const VERSION: &str = env!("CINEFIT_VERSION_LINE");

#[derive(Parser)]
#[command(version = VERSION, about = "Constant image height video processing")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List DeckLink devices and what they report. Opens no input, so it is
    /// safe while another program is capturing.
    Probe {
        /// Also read what the card's input EDID advertises. Taking and
        /// releasing the card's EDID interface may be a hotplug upstream,
        /// which the Denon in this chain does not reliably recover from
        /// (black picture until its input is cycled), so it is opt-in.
        #[arg(long)]
        edid: bool,
    },
    /// Capture from a device's input for a while and summarise what arrived.
    Capture {
        /// How long to capture, in seconds.
        #[arg(short = 't', long, default_value_t = 10.0)]
        seconds: f64,
        /// Which device, by its index in `cinefit probe`; by default the
        /// first that can capture.
        #[arg(short, long)]
        device: Option<usize>,
        /// Save raw frames, with a JSON description of each, into this
        /// directory.
        #[arg(long)]
        dump: Option<PathBuf>,
        /// Seconds between saved frames.
        #[arg(long, default_value_t = 1.0, requires = "dump")]
        dump_every: f64,
        /// Record every frame, losslessly through zstd, into this directory
        /// (frames.zst, frames.csv), as the C++ version records clips.
        /// Captured content: keep it outside any repository.
        #[arg(long, conflicts_with = "dump")]
        record: Option<PathBuf>,
    },
    /// Check the GPU path: libplacebo on Vulkan, dma-buf import support, and
    /// an identity render read back and compared. Touches no display.
    Gpu,
    /// The Dolby Vision metadata of a session's kept tunnel frames (#109):
    /// the grade's range, each shot's brightness and the colourist's trims.
    /// Writes dovi.csv into the session.
    Dovi {
        /// The session directory (under ~/cinefit-clips/caught).
        session: PathBuf,
    },
    /// Pair the frames two passes of a title both kept (#109): a Dolby
    /// Vision tunnel pass and a low-latency one, each recorded with
    /// detect.pair_every. Writes pairs.csv into the first session.
    Pairs {
        /// The first pass's session directory (under ~/cinefit-clips/caught).
        a: PathBuf,
        /// The second pass's.
        b: PathBuf,
        /// Only the first session's frames of this content (tunnel, lldv,
        /// sdr, hdr10): for a session holding several passes.
        #[arg(long)]
        a_content: Option<String>,
        /// The same for the second session.
        #[arg(long)]
        b_content: Option<String>,
    },
    /// Time a 4K render into a scanout buffer in each layout the display
    /// plane takes, and check each renders the same picture as LINEAR, with
    /// a negative control (#101). Off-screen; for clean numbers, stop the
    /// cinefit service first.
    Bench {
        /// Renders timed per layout, after a warm-up.
        #[arg(short = 'n', long, default_value_t = 300)]
        frames: usize,
        /// The DRM device; by default the first with a connected display.
        #[arg(long)]
        card: Option<PathBuf>,
    },
    /// List what each display device offers: connectors and their EDID's
    /// HDR and colorimetry, the HDR properties, and every plane's formats and
    /// modifiers. Reads only, without taking the display, so it is safe while
    /// cinefit or anything else is showing a picture.
    Display {
        /// A DRM device such as /dev/dri/card1; default: every card.
        #[arg(long)]
        card: Option<PathBuf>,
    },
    /// Judge a dataset session's crops shot by shot (#91): the picture's
    /// true edge in each shot, from its readings, against the crop shown.
    /// Prints a summary and the worst shots, and writes hindsight.csv in the
    /// session's directory. Reads files only.
    Hindsight {
        /// Session directories, under ~/cinefit-clips/caught.
        #[arg(required = true)]
        sessions: Vec<PathBuf>,
        /// How many of the worst shots of each kind to list.
        #[arg(long, default_value_t = 10)]
        worst: usize,
    },
    /// Show a test pattern on the display and measure flip pacing. Needs the
    /// display to itself: nothing else may be driving it.
    Pattern {
        /// How long to run, in seconds.
        #[arg(short = 't', long, default_value_t = 30.0)]
        seconds: f64,
        /// The mode, as WIDTHxHEIGHT@HZ. The default is a 17:9 panel's full
        /// width at film rate.
        #[arg(short, long, default_value = "4096x2160@23.976")]
        mode: cinefit_kms::ModeRequest,
        /// The DRM device; by default the first with a connected display.
        #[arg(long)]
        card: Option<PathBuf>,
        /// Signal HDR10 (PQ, BT.2020 primaries, D65) instead of SDR.
        #[arg(long)]
        hdr: bool,
        /// MaxCLL to declare, in nits.
        #[arg(long, default_value_t = 1000.0, requires = "hdr")]
        max_cll: f64,
        /// MaxFALL to declare, in nits.
        #[arg(long, default_value_t = 400.0, requires = "hdr")]
        max_fall: f64,
        /// Mastering display peak to declare, in nits.
        #[arg(long, default_value_t = 1000.0, requires = "hdr")]
        max_mastering: f64,
        /// Halfway through, change MaxCLL to this, with no modeset allowed.
        #[arg(long, requires = "hdr")]
        change_max_cll: Option<f64>,
        /// Render each frame with libplacebo on the GPU, into scanout buffers,
        /// scrolling, instead of drawing with the CPU.
        #[arg(long)]
        gpu: bool,
        /// With --gpu: hold the GPU at the standard stable power state instead
        /// of leaving its clocks to the driver. For A/B timing.
        #[arg(long, requires = "gpu")]
        pstate: bool,
        /// Also show an OSD test card on the overlay plane (slate, banner and
        /// grey steps), encoded for SDR or, with --hdr, PQ. It comes down a
        /// third of the way through and back at two thirds: watch for a resync.
        #[arg(long)]
        slate: bool,
    },
    /// Replay recorded clips through the bar detector: every crop change, the
    /// time at each height, what the detector costs per frame, and with
    /// --compare, every frame against the C++ version's replay log. Touches no
    /// capture card or display.
    Replay {
        /// Clip directories: ours or the C++ version's, whole or trimmed; with
        /// --readings, readings files.
        #[arg(required = true)]
        clips: Vec<PathBuf>,
        /// A directory of the C++ version's replay logs (<clip>.cinefit.csv)
        /// to compare every frame's crop with.
        #[arg(long)]
        compare: Option<PathBuf>,
        /// Write every frame's decision to DIR/<clip>.rust.csv.
        #[arg(long, value_name = "DIR")]
        log: Option<PathBuf>,
        /// Summaries only, without the list of crop changes.
        #[arg(short, long)]
        quiet: bool,
        /// Also save what the bar reader and cut detector saw on every frame
        /// to DIR/<clip>.csv, and check that the policy replaying them gives
        /// the same crops.
        #[arg(long, value_name = "DIR", conflicts_with = "readings")]
        save_readings: Option<PathBuf>,
        /// The arguments are saved readings (ours or the C++ version's): the
        /// policy alone over them, a whole clip in milliseconds.
        #[arg(long)]
        readings: bool,
        /// A policy parameter for a --readings replay, e.g.
        /// threshold_new=4; repeatable.
        #[arg(long = "set", value_name = "KEY=VALUE", requires = "readings")]
        sets: Vec<String>,
    },
    /// Compare the same content recorded twice with `capture --record`, as a
    /// Dolby Vision tunnel and as HDR10: align the recordings, render matched
    /// frames through each path, and report how far apart they are (#24).
    /// Uses the GPU; touches no display or capture card.
    Compare {
        /// The tunnel recording (r210).
        tunnel: PathBuf,
        /// The HDR10 recording (v210).
        hdr10: PathBuf,
        /// Frame pairs to compare, spread through the recordings.
        #[arg(long, default_value_t = 12)]
        pairs: usize,
        /// The largest start offset to try between the recordings, in frames.
        #[arg(long, default_value_t = 240)]
        max_shift: i64,
    },
    /// Capture to screen: show the input on the display, at the source's
    /// frame rate, with its HDR signalling, and measure what arrived and what
    /// was shown. Needs the input and the display to itself.
    Play {
        /// How long to run, in seconds; without it, until stopped (Ctrl-C, or
        /// SIGTERM from a service manager).
        #[arg(short = 't', long)]
        seconds: Option<f64>,
        /// Which capture device, by its index in `cinefit probe`, for this
        /// run; by default the `input.device` setting (a persistent ID), else
        /// the first that can capture.
        #[arg(short, long)]
        device: Option<usize>,
        /// The DRM device; by default the first with a connected display.
        #[arg(long)]
        card: Option<PathBuf>,
        /// The settings file; by default ~/.config/cinefit-rs/cinefit.toml.
        #[arg(long)]
        config: Option<PathBuf>,
        /// Override one setting for this run, as KEY=VALUE (repeatable;
        /// `cinefit settings` lists them).
        #[arg(long = "set", value_name = "KEY=VALUE")]
        sets: Vec<String>,
        /// Hold the GPU at the standard stable power state instead of leaving
        /// its clocks to the driver (held while there is a signal, and given
        /// back after a minute without one). For a GPU whose clocks do not
        /// rise on their own for a few milliseconds of work a frame, as the
        /// RX 6400's did not.
        #[arg(long)]
        pstate: bool,
        /// Stop before the modeset: pre-roll, mode, signalling and the first
        /// render, with the display untouched.
        #[arg(long)]
        dry_run: bool,
        /// Scan out of LINEAR buffers, as before #101, instead of the tiled
        /// layout the display and the GPU agree on: for comparing the two.
        /// (A tiled layout that fails to render, or renders wrongly, is
        /// caught at start and LINEAR used without it.)
        #[arg(long)]
        linear_scanout: bool,
        /// Time each GPU pass with libplacebo's timers (upload, unpack, the
        /// tunnel's decode, the Dolby mapping, the output), and report them
        /// by situation at the end of the run: for one run, measuring.
        #[arg(long)]
        gpu_timers: bool,
    },
    /// Show what the remote's and keyboards' buttons mean to cinefit, as
    /// they are pressed. Takes the keyboards from anything else while it
    /// runs, a running `play` included.
    Keys {
        /// How long to listen, in seconds.
        #[arg(short = 't', long, default_value_t = 30.0)]
        seconds: f64,
    },
    /// Read a Panasonic UB820's play state and position over the network
    /// (needs Voice Control on in the player). Reads only; presses nothing.
    Ub820 {
        /// The player's IP address or hostname.
        host: String,
        /// Keep reading, once a second, until interrupted.
        #[arg(long)]
        watch: bool,
    },
    /// List every setting: its value, range, what a change costs, and help.
    Settings {
        /// The settings file; by default ~/.config/cinefit-rs/cinefit.toml.
        #[arg(long)]
        config: Option<PathBuf>,
        /// Print the settings as a complete, commented settings file instead.
        #[arg(long)]
        toml: bool,
    },
}

fn main() -> ExitCode {
    let result = match Cli::parse().command {
        Command::Probe { edid } => probe(edid),
        Command::Display { card } => display(card.as_deref()),
        Command::Hindsight { sessions, worst } => {
            sessions.iter().try_for_each(|s| hindsight::run(s, worst))
        }
        Command::Gpu => gpu(),
        Command::Ub820 { host, watch } => ub820::run(&host, watch),
        Command::Dovi { session } => dovi_meta::run(&session),
        Command::Pairs {
            a,
            b,
            a_content,
            b_content,
        } => pairs::run((&a, a_content.as_deref()), (&b, b_content.as_deref())),
        #[cfg(target_os = "linux")]
        Command::Bench { frames, card } => bench::run(frames, card.as_deref()),
        #[cfg(not(target_os = "linux"))]
        Command::Bench { .. } => Err("bench needs Linux: DRM, GBM and Vulkan".into()),
        Command::Replay {
            clips,
            compare,
            log,
            quiet,
            save_readings,
            readings,
            sets,
        } => replay::run(replay::Options {
            pieces: clips,
            save_readings,
            readings,
            sets,
            compare,
            log,
            quiet,
        }),
        Command::Compare {
            tunnel,
            hdr10,
            pairs,
            max_shift,
        } => compare_run(compare::Options {
            tunnel,
            hdr10,
            pairs,
            max_shift,
        }),
        Command::Capture {
            seconds,
            device,
            dump,
            dump_every,
            record,
        } => capture(seconds, device, dump, dump_every, record),
        Command::Pattern {
            seconds,
            mode,
            card,
            hdr,
            max_cll,
            max_fall,
            max_mastering,
            change_max_cll,
            gpu,
            pstate,
            slate,
        } => {
            let hdr = hdr.then_some(cinefit_kms::HdrStatic {
                max_cll,
                max_fall,
                max_mastering,
                ..cinefit_kms::HdrStatic::BT2020_1000
            });
            pattern(PatternRun {
                seconds,
                want: mode,
                card,
                hdr,
                change_max_cll,
                gpu,
                pstate,
                slate,
            })
        }
        Command::Play {
            seconds,
            device,
            card,
            config,
            sets,
            pstate,
            dry_run,
            linear_scanout,
            gpu_timers,
        } => {
            let file = config.clone().unwrap_or_else(config::default_path);
            load_config(config.as_deref(), &sets).and_then(|config| {
                // The file as it is, without --set: what a change made in
                // the menu is saved over, so a one-run override never is.
                let saved = config::load(&file)?;
                play(play_options(
                    seconds,
                    device,
                    card,
                    config,
                    (file, saved),
                    Switches {
                        pstate,
                        dry_run,
                        linear_scanout,
                        gpu_timers,
                    },
                ))
            })
        }
        Command::Keys { seconds } => keys(seconds),
        Command::Settings { config, toml } => settings(config.as_deref(), toml),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{}", chain(&*e));
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn display(_card: Option<&std::path::Path>) -> Result<(), Box<dyn std::error::Error>> {
    Err("display output is Linux only".into())
}

#[cfg(target_os = "linux")]
fn display(card: Option<&std::path::Path>) -> Result<(), Box<dyn std::error::Error>> {
    print!("{}", cinefit_kms::describe_displays(card)?);
    Ok(())
}

fn probe(edid: bool) -> Result<(), Box<dyn std::error::Error>> {
    println!("DeckLink API {}", cinefit_decklink::api_version()?);

    let devices = cinefit_decklink::devices()?;
    if devices.is_empty() {
        println!("no devices");
    }
    let yn = |v: Option<bool>| match v {
        Some(true) => "yes",
        Some(false) => "no",
        None => "unreported",
    };
    for (i, dev) in devices.iter().enumerate() {
        let d = dev.info();
        println!("device {i}: {} ({})", d.display_name, d.model_name);
        println!("  capture:          {}", yn(d.supports_capture));
        println!("  HDMI input:       {}", yn(d.hdmi_input));
        println!("  format detection: {}", yn(d.input_format_detection));
        println!("  HDR metadata:     {}", yn(d.hdr_metadata));
        if edid {
            match dev.read_input_edid() {
                Some(e) => println!("  input EDID:       {e}"),
                None => println!("  input EDID:       unreported"),
            }
        }
        match d.persistent_id {
            Some(id) => println!("  persistent ID:    {id:#x}"),
            None => println!("  persistent ID:    unreported"),
        }
    }
    Ok(())
}

fn capture(
    seconds: f64,
    index: Option<usize>,
    dump_dir: Option<PathBuf>,
    dump_every: f64,
    record_dir: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let run_for = Duration::try_from_secs_f64(seconds)
        .ok()
        .filter(|d| !d.is_zero())
        .ok_or("--seconds must be a positive number")?;

    let devices = cinefit_decklink::devices()?;
    let infos: Vec<_> = devices.iter().map(|d| d.info()).collect();
    let want = index.map_or(crate::device::Want::First, crate::device::Want::Index);
    let index = crate::device::choose(&infos, want)?;
    let device = &devices[index];
    println!("device {index}: {}", device.info().display_name);

    // Frames to dump go to a writer thread, so disk writes never stall the
    // SDK's thread.
    let mut writer = None;
    let dumper = match &dump_dir {
        Some(dir) => {
            let every = Duration::try_from_secs_f64(dump_every)
                .ok()
                .filter(|d| !d.is_zero())
                .ok_or("--dump-every must be a positive number")?;
            std::fs::create_dir_all(dir)?;
            let (tx, rx) = mpsc::channel::<capture::Dump>();
            let dir = dir.clone();
            writer = Some(std::thread::spawn(move || {
                let mut written = 0usize;
                for d in rx {
                    match capture::write_dump(&dir, &d) {
                        Ok(()) => written += 1,
                        Err(e) => eprintln!("dump frame {}: {e}", d.index),
                    }
                }
                written
            }));
            Some(capture::Dumper::new(every, tx))
        }
        None => None,
    };

    let stats = Arc::new(Mutex::new(capture::Stats::default()));
    let started = Instant::now();
    let clip = match &record_dir {
        Some(dir) => Some(record::ClipWriter::create(dir)?),
        None => None,
    };
    let running = device.start_capture(capture::Recorder::new(
        Arc::clone(&stats),
        dumper,
        clip.as_ref().map(|c| c.feed().clone()),
    ))?;
    // Ctrl-C ends the run early and still prints the summary; a second one
    // exits at once, in case stopping itself hangs.
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let mut presses = 0;
    ctrlc::set_handler(move || {
        presses += 1;
        if presses > 1 {
            std::process::exit(130);
        }
        let _ = stop_tx.send(());
    })?;
    let interrupted = stop_rx.recv_timeout(run_for).is_ok();
    // Measured before stopping: frames stop counting here, not after the stop
    // has finished.
    let ran = started.elapsed();
    let failed_restarts = running.failed_restarts();
    let saved_hdmi = running.saved_input_was_hdmi();
    // Stops the input; no callback runs after this.
    drop(running);

    if interrupted {
        println!("stopped by Ctrl-C or SIGTERM");
    }
    print!("{}", capture::lock(&stats).summary(ran, failed_restarts));
    if let Some(clip) = clip {
        let dir = clip.dir().to_owned();
        let r = clip.finish()?;
        println!(
            "recorded {} frames to {} ({} dropped: no buffer free)",
            r.written,
            dir.display(),
            r.dropped
        );
    }
    // A saved input other than HDMI means the card reverts to it as this run
    // ends: a hotplug on the HDMI port, after which the Denon upstream may
    // send a black picture until its input is cycled.
    println!(
        "card's saved input: {}",
        match saved_hdmi {
            Some(true) => "HDMI",
            Some(false) =>
                "not HDMI: HDMI was selected for this run only, and the card reverts as it ends (a hotplug)",
            None => "unreadable",
        }
    );
    // Dropping the capture dropped the handler and its sender, so the writer
    // finishes once the queue is written.
    if let (Some(writer), Some(dir)) = (writer, &dump_dir) {
        let written = writer.join().map_err(|_| "dump writer panicked")?;
        println!("dumped {written} frames to {}", dir.display());
    }
    Ok(())
}

/// What `cinefit pattern` was asked for.
struct PatternRun {
    seconds: f64,
    want: cinefit_kms::ModeRequest,
    card: Option<PathBuf>,
    hdr: Option<cinefit_kms::HdrStatic>,
    change_max_cll: Option<f64>,
    gpu: bool,
    pstate: bool,
    slate: bool,
}

#[cfg(not(target_os = "linux"))]
fn pattern(_run: PatternRun) -> Result<(), Box<dyn std::error::Error>> {
    Err("display output is only supported on Linux".into())
}

#[cfg(target_os = "linux")]
fn pattern(run: PatternRun) -> Result<(), Box<dyn std::error::Error>> {
    use cinefit_kms::pattern;
    use std::sync::atomic::{AtomicBool, Ordering};
    let PatternRun {
        seconds,
        want,
        card,
        hdr,
        change_max_cll,
        gpu,
        pstate,
        slate,
    } = run;

    let run_for = Duration::try_from_secs_f64(seconds)
        .ok()
        .filter(|d| !d.is_zero())
        .ok_or("--seconds must be a positive number")?;

    let mut out = cinefit_kms::Output::open(card.as_deref())?;
    println!("output: {}", out.describe());
    let modes = out.modes();
    let Some(index) = cinefit_kms::choose_mode(&modes, &want) else {
        eprintln!("progressive modes offered:");
        for m in modes.iter().filter(|m| !m.interlaced) {
            eprintln!("  {m}{}", if m.preferred { " (preferred)" } else { "" });
        }
        return Err(format!(
            "no {}x{} mode within 0.01 Hz of {}",
            want.width, want.height, want.refresh_hz
        )
        .into());
    };
    let mode = modes[index];
    println!("mode: {mode}");
    let (w, h) = (usize::from(mode.width), usize::from(mode.height));

    // With --pstate, held for the whole run; released, by the kernel if need
    // be, when this ends.
    let _clocks = if gpu && pstate {
        match cinefit_render::power::StablePstate::acquire() {
            Ok(p) => {
                println!("gpu clocks: {} (stable pstate, held until exit)", p.level());
                Some(p)
            }
            Err(e) => {
                println!("gpu clocks: not held: {e}");
                None
            }
        }
    } else {
        if gpu {
            println!("gpu clocks: the driver's default policy");
        }
        None
    };

    // Two buffers: one on screen while the other is drawn or rendered.
    let placebo = if gpu {
        Some(cinefit_render::Placebo::new()?)
    } else {
        None
    };
    let frames = if gpu {
        [out.create_scanout(index)?, out.create_scanout(index)?]
    } else {
        [out.create_frame(index)?, out.create_frame(index)?]
    };
    // On the GPU path: the pattern uploaded once, and both scanout buffers
    // imported as render targets.
    let gpu_state = match &placebo {
        Some(pl) => {
            use std::os::fd::AsFd;
            let mut targets = Vec::new();
            for &f in &frames {
                let b = out.export(f)?;
                targets.push(pl.import_dmabuf(&cinefit_render::DmaBufTarget {
                    fd: b.fd.as_fd(),
                    width: b.width,
                    height: b.height,
                    fourcc: b.fourcc,
                    modifier: b.modifier,
                    stride: b.stride,
                    offset: b.offset,
                })?);
            }
            let src = pl.upload_rgba8(w as u32, h as u32, &pattern_rgba(w, h))?;
            pl.render(&src, &targets[0], 0.0)?;
            Some((pl, src, targets))
        }
        None => None,
    };
    let mut bar_at: [Option<usize>; 2] = [None, None];
    if gpu_state.is_none() {
        for &f in &frames {
            out.draw(f, |px, pitch| pattern::draw_background(px, pitch, w, h))?;
        }
        let x0 = pattern::bar_x(0, w);
        out.draw(frames[0], |px, pitch| {
            pattern::move_bar(px, pitch, w, h, None, x0)
        })?;
        bar_at[0] = Some(x0);
    }
    let mut render_ms: Vec<f64> = Vec::new();
    if let Some(h) = &hdr {
        if !h.is_coherent() {
            return Err(
                "incoherent HDR metadata: MaxFALL above MaxCLL, or MaxCLL above the mastering peak"
                    .into(),
            );
        }
        println!(
            "HDR10: MaxCLL {}, MaxFALL {}, mastering {}-{} nits",
            h.max_cll, h.max_fall, h.min_mastering, h.max_mastering
        );
    }
    // The OSD test card goes up with the modeset, in the same commit.
    let card_frame = if slate {
        if !out.has_overlay() {
            return Err("--slate: this output has no overlay plane taking ARGB8888".into());
        }
        let i = out.create_overlay(index)?;
        let enc = if hdr.is_some() {
            osd::Encoding::Pq
        } else {
            osd::Encoding::Sdr
        };
        let fonts = osd::Fonts::load();
        let t = Instant::now();
        // Drawn in memory and copied in, as play's OSD is: drawing blends, so
        // it reads, and a mapped buffer can be uncached video memory (drawn
        // in place on NVIDIA, the card took 9.5 s).
        let mut card = vec![0u8; w * h * 4];
        osd::test_card(
            &mut osd::Canvas::new(&mut card, w * 4, w, h, enc),
            &fonts,
            enc,
        );
        let drawn = t.elapsed();
        out.draw_overlay(i, |px, pitch| {
            for (row, src) in px.chunks_mut(pitch).zip(card.chunks(w * 4)) {
                row[..w * 4].copy_from_slice(src);
            }
        })?;
        println!(
            "overlay: test card drawn in {:.0} ms, copied in {:.0} ms, encoded for {enc:?}",
            drawn.as_secs_f64() * 1000.0,
            (t.elapsed() - drawn).as_secs_f64() * 1000.0
        );
        out.set_overlay(Some(i));
        Some(i)
    } else {
        None
    };
    out.set_hdr(hdr);
    out.modeset(index, frames[0])?;
    println!("after modeset: {}", out.hdr_state());
    let mut overlay_steps =
        card_frame.map(|i| vec![(run_for * 2 / 3, Some(i)), (run_for / 3, None)]);

    // Ctrl-C ends the run and still restores the display and prints the
    // summary; a second one exits at once.
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

    let started = Instant::now();
    let mut pacing = cinefit_kms::Pacing::default();
    let (mut shown, mut frame) = (0usize, 0u64);
    let mut change = hdr.zip(change_max_cll);
    while started.elapsed() < run_for && !stop.load(Ordering::Relaxed) {
        frame += 1;
        if started.elapsed() >= run_for / 2
            && let Some((h, cll)) = change.take()
        {
            let changed = cinefit_kms::HdrStatic { max_cll: cll, ..h };
            out.update_hdr(&changed)?;
            println!(
                "{:.1} s: MaxCLL {} -> {cll}, sent with the next flip (no modeset allowed)",
                started.elapsed().as_secs_f64(),
                h.max_cll
            );
        }
        if let Some(steps) = overlay_steps.as_mut()
            && steps.last().is_some_and(|&(at, _)| started.elapsed() >= at)
        {
            let (_, change) = steps.pop().expect("checked");
            out.set_overlay(change);
            println!(
                "{:.1} s: overlay {} with the next flip (no modeset)",
                started.elapsed().as_secs_f64(),
                if change.is_some() {
                    "back up"
                } else {
                    "taken down"
                }
            );
        }
        let next = 1 - shown;
        if let Some((pl, src, targets)) = &gpu_state {
            // Whole pixels, so the scroll is exact rather than resampled.
            let shift = (frame as usize * pattern::BAR_STEP % w) as f32;
            let t = Instant::now();
            pl.render(src, &targets[next], shift)?;
            render_ms.push(t.elapsed().as_secs_f64() * 1000.0);
        } else {
            let x = pattern::bar_x(frame, w);
            let old = bar_at[next];
            out.draw(frames[next], |px, pitch| {
                pattern::move_bar(px, pitch, w, h, old, x)
            })?;
            bar_at[next] = Some(x);
        }
        out.flip(frames[next])?;
        let flip = out.wait_flip()?;
        pacing.on_flip(flip.sequence, flip.at);
        shown = next;
    }
    let restored = out.restore()?;
    println!("{restored}; now: {}", out.hdr_state());

    if stop.load(Ordering::Relaxed) {
        println!("stopped by Ctrl-C or SIGTERM");
    }
    println!("ran {:.1} s", started.elapsed().as_secs_f64());
    print!("{}", pacing.summary(mode.period()));
    if !render_ms.is_empty() {
        render_ms.sort_by(f64::total_cmp);
        let at = |q: f64| render_ms[((render_ms.len() - 1) as f64 * q) as usize];
        println!(
            "GPU render per frame, including the wait for it: median {:.2} ms, p95 {:.2}, max {:.2}",
            at(0.5),
            at(0.95),
            at(1.0)
        );
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn play_options(
    seconds: Option<f64>,
    device: Option<usize>,
    card: Option<PathBuf>,
    config: config::Config,
    file: (PathBuf, config::Config),
    switches: Switches,
) -> play::Options {
    play::Options {
        seconds,
        device,
        card,
        config,
        file,
        pstate: switches.pstate,
        dry_run: switches.dry_run,
        linear_scanout: switches.linear_scanout,
        gpu_timers: switches.gpu_timers,
    }
}

/// `play`'s one-run switches, as flags so none is left on in a file.
struct Switches {
    pstate: bool,
    dry_run: bool,
    linear_scanout: bool,
    gpu_timers: bool,
}

#[cfg(target_os = "linux")]
fn play(opts: play::Options) -> Result<(), Box<dyn std::error::Error>> {
    play::run(opts)
}

#[cfg(not(target_os = "linux"))]
fn play_options(
    _seconds: Option<f64>,
    _device: Option<usize>,
    _card: Option<PathBuf>,
    _config: config::Config,
    _file: (PathBuf, config::Config),
    _switches: Switches,
) {
}

#[cfg(not(target_os = "linux"))]
fn play(_opts: ()) -> Result<(), Box<dyn std::error::Error>> {
    Err("playing is only supported on Linux".into())
}

#[cfg(target_os = "linux")]
fn compare_run(opts: compare::Options) -> Result<(), Box<dyn std::error::Error>> {
    compare::run(opts)
}

#[cfg(not(target_os = "linux"))]
fn compare_run(_opts: compare::Options) -> Result<(), Box<dyn std::error::Error>> {
    Err("comparing renders is only supported on Linux".into())
}

/// The settings file (or the defaults), then each `--set` over it.
#[cfg(target_os = "linux")]
fn keys(seconds: f64) -> Result<(), Box<dyn std::error::Error>> {
    let until = Instant::now()
        + Duration::try_from_secs_f64(seconds)
            .map_err(|_| "--seconds must be a positive number")?;
    let reader = cinefit_input::KeyReader::start(std::path::Path::new("/dev/input"), |line| {
        println!("{line}")
    });
    println!("listening for {seconds:.0} s: press the remote's buttons");
    while Instant::now() < until {
        for a in reader.drain() {
            println!("{}", a.name());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn keys(_seconds: f64) -> Result<(), Box<dyn std::error::Error>> {
    Err("keys: Linux only".into())
}

fn load_config(
    path: Option<&std::path::Path>,
    sets: &[String],
) -> Result<config::Config, Box<dyn std::error::Error>> {
    let path = path.map_or_else(config::default_path, std::path::Path::to_path_buf);
    let mut c = config::load(&path)?;
    for s in sets {
        config::set(&mut c, s)?;
    }
    Ok(c)
}

fn settings(path: Option<&std::path::Path>, toml: bool) -> Result<(), Box<dyn std::error::Error>> {
    let file = path.map_or_else(config::default_path, std::path::Path::to_path_buf);
    let c = config::load(&file)?;
    if toml {
        print!("{}", config::render(&c));
        return Ok(());
    }
    println!(
        "{} ({})",
        file.display(),
        if file.exists() {
            "read"
        } else {
            "not there: defaults"
        }
    );
    for s in config::settings() {
        println!(
            "{} = {}  ({}; applies: {})",
            s.key,
            s.get(&c),
            s.range(),
            s.apply
        );
        println!("    {}", s.help);
    }
    Ok(())
}

fn gpu() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "linux")]
    let clocks = {
        let p = cinefit_render::power::StablePstate::acquire();
        match &p {
            Ok(p) => println!(
                "gpu clocks: {} on {} (stable pstate held)",
                p.level(),
                p.node()
            ),
            Err(e) => println!("gpu clocks: not held: {e}"),
        }
        p.ok()
    };
    let pl = cinefit_render::Placebo::new()?;
    let info = pl.info();
    let (major, minor, patch) = info.vulkan_version;
    println!("device: {}", info.device_name);
    println!(
        "Vulkan {major}.{minor}.{patch}, libplacebo API {}",
        info.placebo_api
    );
    println!("max texture: {}", info.max_tex_2d_dim);
    println!(
        "dma-buf import: {}",
        if info.dma_buf_import { "yes" } else { "no" }
    );

    // The test pattern at full panel size, rendered unchanged and read back.
    let (w, h) = (4096usize, 2160usize);
    let rgba = pattern_rgba(w, h);
    let started = Instant::now();
    let out = pl.render_rgba8(w as u32, h as u32, &rgba)?;
    let took = started.elapsed();
    let max_diff = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(out.as_chunks::<4>().0)
        .flat_map(|(a, b)| (0..3).map(move |c| a[c].abs_diff(b[c])))
        .max()
        .unwrap_or(0);
    println!(
        "identity render {w}x{h}: largest difference {max_diff} (of 255), {:.1} ms with upload and readback",
        took.as_secs_f64() * 1000.0
    );
    if max_diff > 1 {
        return Err("identity render changed the image".into());
    }
    #[cfg(target_os = "linux")]
    unpack_check::run(&pl)?;
    #[cfg(target_os = "linux")]
    tunnel_check::run(&pl)?;
    scanout_check(&pl, w, h, &rgba)?;

    // The hold must end with the program: check the level goes back.
    #[cfg(target_os = "linux")]
    if let Some(p) = clocks {
        let node = p.node().to_owned();
        drop(p);
        let level = std::fs::read_to_string(format!(
            "/sys/class/drm/{node}/device/power_dpm_force_performance_level"
        ))
        .unwrap_or_default();
        println!("gpu clocks after release: {}", level.trim());
    }
    Ok(())
}

/// Renders the pattern into a real GBM scanout buffer through a dma-buf
/// import, reads the buffer back through the CPU and compares. Allocates on
/// the display device but shows nothing.
#[cfg(target_os = "linux")]
fn scanout_check(
    pl: &cinefit_render::Placebo,
    w: usize,
    h: usize,
    rgba: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    use std::os::fd::AsFd;

    let mut out = cinefit_kms::Output::open(None)?;
    let modes = out.modes();
    let index = modes
        .iter()
        .position(|m| usize::from(m.width) == w && usize::from(m.height) == h)
        .ok_or_else(|| format!("the display offers no {w}x{h} mode"))?;
    let frame = out.create_scanout(index)?;
    let buf = out.export(frame)?;
    let target = pl.import_dmabuf(&cinefit_render::DmaBufTarget {
        fd: buf.fd.as_fd(),
        width: buf.width,
        height: buf.height,
        fourcc: buf.fourcc,
        modifier: buf.modifier,
        stride: buf.stride,
        offset: buf.offset,
    })?;
    let src = pl.upload_rgba8(w as u32, h as u32, rgba)?;
    let started = Instant::now();
    pl.render(&src, &target, 0.0)?;
    let took = started.elapsed();

    // Whatever the scanout format, read back as 8-bit RGB to compare.
    let fourcc = buf.fourcc;
    let (max_diff, first_bad, samples) = out.read_scanout(frame, |px, stride| {
        let mut worst = 0u8;
        let mut first_bad = None;
        for y in 0..h {
            for x in 0..w {
                let o = y * stride + x * 4;
                let i = (y * w + x) * 4;
                let word = [px[o], px[o + 1], px[o + 2], px[o + 3]];
                let got = cinefit_kms::scanout::decode(fourcc, word)
                    .map_or([0; 3], |c| c.map(|v| (v * 255.0).round() as u8));
                let d = (0..3)
                    .map(|c| got[c].abs_diff(rgba[i + c]))
                    .max()
                    .unwrap_or(0);
                if d > 1 && first_bad.is_none() {
                    first_bad = Some((x, y, px[o..o + 4].to_vec(), rgba[i..i + 4].to_vec()));
                }
                worst = worst.max(d);
            }
        }
        // Raw bytes at a few known points: a corner (border), bars, the ramp.
        let samples: Vec<_> = [
            (0, 0),
            (w / 16, h / 3),
            (w * 3 / 16, h / 3),
            (w / 2, h * 5 / 6),
        ]
        .iter()
        .map(|&(x, y)| {
            let o = y * stride + x * 4;
            let i = (y * w + x) * 4;
            (x, y, px[o..o + 4].to_vec(), rgba[i..i + 4].to_vec())
        })
        .collect::<Vec<_>>();
        (worst, first_bad, samples)
    })?;
    // Raw bytes only when something is wrong: they tell a channel swap, a
    // stride mix-up and missing writes apart at a glance.
    if max_diff > 1 {
        for (x, y, got, want) in samples.iter().chain(first_bad.iter()) {
            println!("  ({x}, {y}): buffer bytes {got:02x?}, expected RGBA {want:02x?}");
        }
    }
    println!(
        "scanout render {w}x{h} (GBM, {}, modifier {:#x}, stride {}): largest difference {max_diff} (of 255), {:.1} ms",
        String::from_utf8_lossy(&fourcc.to_le_bytes()),
        buf.modifier,
        buf.stride,
        took.as_secs_f64() * 1000.0
    );
    if max_diff > 1 {
        return Err("render into the scanout buffer changed the image".into());
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn scanout_check(
    _pl: &cinefit_render::Placebo,
    _w: usize,
    _h: usize,
    _rgba: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    Ok(())
}

/// The static test pattern as RGBA8, rows packed.
fn pattern_rgba(w: usize, h: usize) -> Vec<u8> {
    let mut rgba = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let rgb = cinefit_kms::pattern::background(x, y, w, h);
            let o = (y * w + x) * 4;
            rgba[o..o + 4].copy_from_slice(&[(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8, 255]);
        }
    }
    rgba
}

#[cfg(test)]
mod tests {
    // A KMS error reads as it did before its cause became the source: what
    // failed, then why (#228).
    #[test]
    fn a_chain_reads_as_one_line() {
        let e = cinefit_kms::Error::Io {
            what: "allocating a dumb buffer",
            source: std::io::Error::from_raw_os_error(12),
        };
        assert_eq!(
            super::chain(&e).to_string(),
            format!(
                "allocating a dumb buffer: {}",
                std::io::Error::from_raw_os_error(12)
            )
        );
        let plain: Box<dyn std::error::Error> = "no source".into();
        assert_eq!(super::chain(&*plain).to_string(), "no source");
    }
}
