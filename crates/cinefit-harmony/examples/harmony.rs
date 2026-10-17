//! List a Harmony Hub's devices and commands, or press one button.
//!
//!     cargo run -p cinefit-harmony --example harmony -- <hub-ip> list [filter]
//!     cargo run -p cinefit-harmony --example harmony -- <hub-ip> press <device-id> <command>
//!
//! `list` reads only. `press` fires IR and changes device state, so confirm
//! the effect on the device itself (for the UB820, its network status).

use cinefit_harmony::Hub;

fn main() {
    let mut args = std::env::args().skip(1);
    let host = args.next();
    let action = args.next();
    let (host, action) = match (host, action) {
        (Some(h), Some(a)) => (h, a),
        _ => {
            eprintln!("usage: harmony <hub-ip> list [filter] | press <device-id> <command>");
            std::process::exit(2);
        }
    };
    let addr = format!("{host}:{}", cinefit_harmony::PORT);

    let mut hub = match Hub::connect(&addr) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("connect failed: {e}");
            std::process::exit(1);
        }
    };

    match action.as_str() {
        "list" => {
            let filter = args.next().unwrap_or_default().to_ascii_lowercase();
            let devices = hub.devices().unwrap_or_else(|e| {
                eprintln!("devices: {e}");
                std::process::exit(1);
            });
            for d in devices {
                if !filter.is_empty() && !d.name.to_ascii_lowercase().contains(&filter) {
                    continue;
                }
                println!("\n{} [id {}]  {} {}", d.name, d.id, d.manufacturer, d.model);
                for c in &d.commands {
                    if c.label == c.command {
                        println!("    {}", c.label);
                    } else {
                        println!("    {}  (send: {})", c.label, c.command);
                    }
                }
            }
        }
        "press" => {
            let (Some(device_id), Some(command)) = (args.next(), args.next()) else {
                eprintln!("usage: harmony <hub-ip> press <device-id> <command>");
                std::process::exit(2);
            };
            match hub.press(&device_id, &command) {
                Ok(()) => println!("sent {command} to device {device_id}"),
                Err(e) => {
                    eprintln!("press: {e}");
                    std::process::exit(1);
                }
            }
        }
        other => {
            eprintln!("unknown action {other:?}");
            std::process::exit(2);
        }
    }
}
