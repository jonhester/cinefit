//! Watches an Apple TV and prints what is playing on every change, for
//! checking the reader against pyatv by hand:
//!
//!     CINEFIT_ATV_CREDENTIALS=<pyatv's AirPlay credentials> \
//!         cargo run -p cinefit-atv --example now_playing -- <host> [-v]

use std::sync::atomic::AtomicBool;
use std::time::SystemTime;

struct Stderr(log::LevelFilter);

impl log::Log for Stderr {
    fn enabled(&self, m: &log::Metadata<'_>) -> bool {
        m.level() <= self.0
    }
    fn log(&self, r: &log::Record<'_>) {
        if self.enabled(r.metadata()) {
            eprintln!("{} {}", r.level(), r.args());
        }
    }
    fn flush(&self) {}
}

fn main() {
    let mut args = std::env::args().skip(1);
    let host = args.next().expect("usage: now_playing <host> [-v]");
    let level = if args.next().as_deref() == Some("-v") {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    };
    log::set_logger(Box::leak(Box::new(Stderr(level)))).expect("one logger");
    log::set_max_level(level);
    let creds = std::env::var("CINEFIT_ATV_CREDENTIALS").expect("CINEFIT_ATV_CREDENTIALS");
    let creds = cinefit_atv::AtvCredentials::parse(&creds).expect("pyatv's four-field form");
    let stop = AtomicBool::new(false);
    let result = cinefit_atv::watch(&host, &creds, &stop, |np| {
        let pos = np.position_at(SystemTime::now()).map(|p| format!("{p:.3}"));
        println!(
            "{} [{}] | {:?} | {:?} by {:?} | series {:?} s{:?}e{:?} | {:?} of {:?} s (measured {:?}, rate {:?}) | ids {:?} {:?} | media {:?}",
            np.app_name.as_deref().or(np.app.as_deref()).unwrap_or("?"),
            np.player.as_deref().unwrap_or("?"),
            np.state,
            np.title,
            np.artist,
            np.series,
            np.season,
            np.episode,
            pos,
            np.duration,
            np.elapsed,
            np.rate,
            np.content_id,
            np.store_id,
            np.media_type,
        );
    });
    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
