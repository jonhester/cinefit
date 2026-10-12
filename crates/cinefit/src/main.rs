// Unsafe code lives in the wrapper crates (cinefit-decklink), behind safe APIs.
#![forbid(unsafe_code)]

mod capture;

use std::process::ExitCode;
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
    },
}

fn main() -> ExitCode {
    let result = match Cli::parse().command {
        Command::Probe => probe(),
        Command::Capture { seconds, device } => capture(seconds, device),
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
        match d.persistent_id {
            Some(id) => println!("  persistent ID:    {id:#x}"),
            None => println!("  persistent ID:    unreported"),
        }
    }
    Ok(())
}

fn capture(seconds: f64, index: usize) -> Result<(), Box<dyn std::error::Error>> {
    let run_for = Duration::try_from_secs_f64(seconds)
        .ok()
        .filter(|d| !d.is_zero())
        .ok_or("--seconds must be a positive number")?;

    let devices = cinefit_decklink::devices()?;
    let device = devices
        .get(index)
        .ok_or_else(|| format!("no device {index}: {} found", devices.len()))?;
    println!("device {index}: {}", device.info().display_name);

    let stats = Arc::new(Mutex::new(capture::Stats::default()));
    let started = Instant::now();
    let running = device.start_capture(capture::Recorder::new(Arc::clone(&stats)))?;
    std::thread::sleep(run_for);
    // Measured before stopping: frames stop counting here, not after the stop
    // has finished.
    let ran = started.elapsed();
    let failed_restarts = running.failed_restarts();
    // Stops the input; no callback runs after this.
    drop(running);

    print!("{}", capture::lock(&stats).summary(ran, failed_restarts));
    Ok(())
}
