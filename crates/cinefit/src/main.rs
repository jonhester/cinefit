// Unsafe code lives in the wrapper crates (cinefit-decklink), behind safe APIs.
#![forbid(unsafe_code)]

mod capture;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(version, about = "Constant image height video processing")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List DeckLink devices and what they report. Opens no input, so it is
    /// safe while another program is capturing.
    Probe,
    /// Capture from a device's input for a while and summarise what arrived.
    Capture {
        /// How long to capture, in seconds.
        #[arg(short = 't', long, default_value_t = 10.0)]
        seconds: f64,
        /// Which device, by its index in `cinefit probe`.
        #[arg(short, long, default_value_t = 0)]
        device: usize,
        /// Save raw frames, with a JSON description of each, into this
        /// directory.
        #[arg(long)]
        dump: Option<PathBuf>,
        /// Seconds between saved frames.
        #[arg(long, default_value_t = 1.0, requires = "dump")]
        dump_every: f64,
    },
    /// Check the GPU path: libplacebo on Vulkan, dma-buf import support, and
    /// an identity render read back and compared. Touches no display.
    Gpu,
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
    },
}

fn main() -> ExitCode {
    let result = match Cli::parse().command {
        Command::Probe => probe(),
        Command::Gpu => gpu(),
        Command::Capture {
            seconds,
            device,
            dump,
            dump_every,
        } => capture(seconds, device, dump, dump_every),
        Command::Pattern {
            seconds,
            mode,
            card,
            hdr,
            max_cll,
            max_fall,
            max_mastering,
            change_max_cll,
        } => {
            let hdr = hdr.then_some(cinefit_kms::HdrStatic {
                max_cll,
                max_fall,
                max_mastering,
                ..cinefit_kms::HdrStatic::BT2020_1000
            });
            pattern(seconds, mode, card, hdr, change_max_cll)
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn probe() -> Result<(), Box<dyn std::error::Error>> {
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
        match d.input_edid {
            Some(e) => println!("  input EDID:       {e}"),
            None => println!("  input EDID:       unreported"),
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
    index: usize,
    dump_dir: Option<PathBuf>,
    dump_every: f64,
) -> Result<(), Box<dyn std::error::Error>> {
    let run_for = Duration::try_from_secs_f64(seconds)
        .ok()
        .filter(|d| !d.is_zero())
        .ok_or("--seconds must be a positive number")?;

    let devices = cinefit_decklink::devices()?;
    let device = devices
        .get(index)
        .ok_or_else(|| format!("no device {index}: {} found", devices.len()))?;
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
    let running = device.start_capture(capture::Recorder::new(Arc::clone(&stats), dumper))?;
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
    // Stops the input; no callback runs after this.
    drop(running);

    if interrupted {
        println!("stopped early by Ctrl-C");
    }
    print!("{}", capture::lock(&stats).summary(ran, failed_restarts));
    // Dropping the capture dropped the handler and its sender, so the writer
    // finishes once the queue is written.
    if let (Some(writer), Some(dir)) = (writer, &dump_dir) {
        let written = writer.join().map_err(|_| "dump writer panicked")?;
        println!("dumped {written} frames to {}", dir.display());
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn pattern(
    _seconds: f64,
    _mode: cinefit_kms::ModeRequest,
    _card: Option<PathBuf>,
    _hdr: Option<cinefit_kms::HdrStatic>,
    _change_max_cll: Option<f64>,
) -> Result<(), Box<dyn std::error::Error>> {
    Err("display output is only supported on Linux".into())
}

#[cfg(target_os = "linux")]
fn pattern(
    seconds: f64,
    want: cinefit_kms::ModeRequest,
    card: Option<PathBuf>,
    hdr: Option<cinefit_kms::HdrStatic>,
    change_max_cll: Option<f64>,
) -> Result<(), Box<dyn std::error::Error>> {
    use cinefit_kms::pattern;
    use std::sync::atomic::{AtomicBool, Ordering};

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

    // Two buffers: one on screen while the other is drawn.
    let frames = [out.create_frame(index)?, out.create_frame(index)?];
    for &f in &frames {
        out.draw(f, |px, pitch| pattern::draw_background(px, pitch, w, h))?;
    }
    let mut bar_at: [Option<usize>; 2] = [None, None];
    let x0 = pattern::bar_x(0, w);
    out.draw(frames[0], |px, pitch| {
        pattern::move_bar(px, pitch, w, h, None, x0)
    })?;
    bar_at[0] = Some(x0);
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
    out.set_hdr(hdr);
    out.modeset(index, frames[0])?;
    println!("after modeset: {}", out.hdr_state());

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
        let next = 1 - shown;
        let x = pattern::bar_x(frame, w);
        let old = bar_at[next];
        out.draw(frames[next], |px, pitch| {
            pattern::move_bar(px, pitch, w, h, old, x)
        })?;
        bar_at[next] = Some(x);
        out.flip(frames[next])?;
        let flip = out.wait_flip()?;
        pacing.on_flip(flip.sequence, flip.at);
        shown = next;
    }
    let restored = out.restore()?;
    println!("{restored}; now: {}", out.hdr_state());

    if stop.load(Ordering::Relaxed) {
        println!("stopped early by Ctrl-C");
    }
    println!("ran {:.1} s", started.elapsed().as_secs_f64());
    print!("{}", pacing.summary(mode.period()));
    Ok(())
}

fn gpu() -> Result<(), Box<dyn std::error::Error>> {
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
    let mut rgba = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let rgb = cinefit_kms::pattern::background(x, y, w, h);
            let o = (y * w + x) * 4;
            rgba[o..o + 4].copy_from_slice(&[(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8, 255]);
        }
    }
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
    Ok(())
}
