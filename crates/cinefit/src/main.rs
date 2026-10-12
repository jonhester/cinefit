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
}

fn main() -> ExitCode {
    let result = match Cli::parse().command {
        Command::Probe => probe(),
        Command::Capture {
            seconds,
            device,
            dump,
            dump_every,
        } => capture(seconds, device, dump, dump_every),
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
