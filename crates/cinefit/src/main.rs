// Unsafe code lives in the wrapper crates (cinefit-decklink), behind safe APIs.
#![forbid(unsafe_code)]

use std::process::ExitCode;

fn main() -> ExitCode {
    let cmd = std::env::args().nth(1);
    match cmd.as_deref() {
        Some("probe") => probe(),
        _ => {
            eprintln!("usage: cinefit probe");
            ExitCode::FAILURE
        }
    }
}

/// Lists DeckLink devices and what they report. Discovery only: it opens no
/// input, so it is safe while another program is capturing.
#[cfg(target_os = "linux")]
fn probe() -> ExitCode {
    let version = match cinefit_decklink::api_version() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    println!("DeckLink API {version}");

    let devices = match cinefit_decklink::devices() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
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
    ExitCode::SUCCESS
}

#[cfg(not(target_os = "linux"))]
fn probe() -> ExitCode {
    eprintln!("DeckLink capture is only supported on Linux");
    ExitCode::FAILURE
}
