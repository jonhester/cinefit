//! The Apple TV (#115): what it is playing, for the info panel, the run log
//! and the detector's dataset. A thread holds the reader's connection
//! (`cinefit-atv`, after pyatv), follows the address as it changes, and
//! reconnects after a failure. Reads only.
//!
//! The pairing's credentials live in a file of their own beside the
//! settings, `atv-credentials`, readable only by its owner: the settings
//! file is shown on the web page, and the credentials include a secret key.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use cinefit_atv::{AtvCredentials, NowPlaying, State};
use log::{info, warn};

/// Between attempts after a failure: an Apple TV asleep or off the network
/// is tried again, not hammered.
const RETRY: Duration = Duration::from_secs(30);

/// Where the pairing's credentials are kept.
pub fn credentials_path() -> PathBuf {
    crate::config::default_path().with_file_name("atv-credentials")
}

/// Keeps a new pairing: written whole and readable only by its owner, then
/// moved into place, so the reader never sees half of one.
pub fn save_credentials(c: &AtvCredentials) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let path = credentials_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("new");
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    f.write_all(c.to_text().as_bytes())?;
    f.sync_all()?;
    std::fs::rename(tmp, path)
}

fn credentials() -> Option<AtvCredentials> {
    let text = std::fs::read_to_string(credentials_path()).ok()?;
    AtvCredentials::parse(&text)
}

#[derive(Default)]
struct Shared {
    host: String,
    now: Option<NowPlaying>,
    /// Ends the connection in hand, for a new address.
    stop: Option<Arc<AtomicBool>>,
}

pub struct AppleTv {
    shared: Arc<Mutex<Shared>>,
}

impl AppleTv {
    /// Starts reading `host` (a hostname or an IP address; empty for none).
    pub fn start(host: &str) -> Self {
        let shared = Arc::new(Mutex::new(Shared {
            host: host.to_string(),
            ..Shared::default()
        }));
        let s = Arc::clone(&shared);
        let _ = std::thread::Builder::new()
            .name("appletv".into())
            .spawn(move || run(&s));
        Self { shared }
    }

    /// A new address (the `atv.host` setting, changed live).
    pub fn set_host(&self, host: &str) {
        let mut s = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        if s.host != host {
            s.host = host.to_string();
            s.now = None;
            if let Some(stop) = &s.stop {
                stop.store(true, Ordering::Relaxed);
            }
        }
    }

    /// What it last said is playing; `None` while not connected.
    pub fn now(&self) -> Option<NowPlaying> {
        self.shared
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .now
            .clone()
    }
}

fn run(shared: &Mutex<Shared>) {
    let lock = || shared.lock().unwrap_or_else(PoisonError::into_inner);
    let mut last_error = String::new();
    loop {
        let host = lock().host.clone();
        // Read each time: a pairing made while running is picked up.
        let creds = credentials();
        let (Some(creds), false) = (creds, host.is_empty()) else {
            std::thread::sleep(Duration::from_secs(5));
            continue;
        };
        let stop = Arc::new(AtomicBool::new(false));
        lock().stop = Some(Arc::clone(&stop));
        info!("appletv: connecting to {host}");
        let result = cinefit_atv::watch(&host, &creds, &stop, |np| {
            let mut s = lock();
            if s.host == host {
                s.now = Some(np.clone());
            }
        });
        {
            let mut s = lock();
            s.stop = None;
            if s.host == host {
                s.now = None;
            }
        }
        if stop.load(Ordering::Relaxed) {
            continue;
        }
        match result {
            Ok(()) => info!("appletv: the connection ended"),
            // Once per kind of failure, not every 30 s.
            Err(e) if e.to_string() != last_error => {
                warn!("appletv: {e}; trying again every {} s", RETRY.as_secs());
                last_error = e.to_string();
            }
            Err(_) => {}
        }
        std::thread::sleep(RETRY);
    }
}

/// Whether the Apple TV is what is on screen: with `input` set (the
/// `atv.input` setting), only while the receiver is on and on that input,
/// matched by its name or its code. Without a setting, or without word
/// from the receiver, it cannot be told, and is taken to be.
pub fn on_screen(input: &str, seen: Option<&crate::avr::Seen>) -> bool {
    if input.is_empty() {
        return true;
    }
    match seen {
        None => true,
        Some(s) => {
            s.on && (s.input.eq_ignore_ascii_case(input)
                || s.name
                    .as_deref()
                    .is_some_and(|n| n.eq_ignore_ascii_case(input)))
        }
    }
}

/// What is playing as one line, for the log and the dataset; `None` when the
/// app says nothing of it.
pub fn label(np: &NowPlaying) -> Option<String> {
    let title = np.title.as_deref().filter(|t| !t.is_empty())?;
    let mut parts = Vec::new();
    if let Some(a) = np.app_name.as_deref().or(np.app.as_deref()) {
        parts.push(a.to_string());
    }
    if let Some(series) = np.series.as_deref().filter(|s| !s.is_empty()) {
        parts.push(series.to_string());
    }
    match (np.season, np.episode) {
        (Some(s), Some(e)) => parts.push(format!("S{s} E{e}")),
        (None, Some(e)) => parts.push(format!("E{e}")),
        _ => {}
    }
    parts.push(title.to_string());
    if let Some(a) = np.artist.as_deref().filter(|a| !a.is_empty()) {
        parts.push(a.to_string());
    }
    Some(parts.join(" \u{b7} "))
}

/// A time as people read it: 1:12:40, 4:05.
fn clock(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// The info panel's section: the title, then the app, where in it and
/// whether it plays. `None` when the app says nothing of what it plays.
pub fn section(np: &NowPlaying, now: SystemTime) -> Option<crate::osd::InfoSection> {
    let title = np.title.as_deref().filter(|t| !t.is_empty())?;
    let line = match (
        np.series.as_deref().filter(|s| !s.is_empty()),
        np.artist.as_deref(),
    ) {
        (Some(series), _) => format!("{series} \u{b7} {title}"),
        (None, Some(artist)) if !artist.is_empty() => format!("{title} \u{b7} {artist}"),
        _ => title.to_string(),
    };
    let mut detail: Vec<String> = Vec::new();
    if let Some(a) = np.app_name.as_deref().or(np.app.as_deref()) {
        detail.push(a.to_string());
    }
    if let (Some(s), Some(e)) = (np.season, np.episode) {
        detail.push(format!("season {s}, episode {e}"));
    }
    match (np.position_at(now), np.duration) {
        (Some(p), Some(d)) if d > 0.0 => {
            detail.push(format!("{} of {}", clock(p.min(d)), clock(d)))
        }
        (Some(p), _) => detail.push(clock(p)),
        _ => {}
    }
    match np.state {
        Some(State::Paused) => detail.push("paused".into()),
        Some(State::Seeking) => detail.push("seeking".into()),
        Some(State::Stopped) => detail.push("stopped".into()),
        _ => {}
    }
    Some(crate::osd::InfoSection {
        label: "PLAYING".into(),
        line,
        detail: detail.join(" \u{b7} "),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn np() -> NowPlaying {
        NowPlaying {
            app: Some("com.disney.disneyplus".into()),
            app_name: Some("Disney+".into()),
            state: Some(State::Playing),
            title: Some("A Film".into()),
            duration: Some(8222.0),
            elapsed: Some(4360.0),
            elapsed_at: Some(SystemTime::UNIX_EPOCH),
            rate: Some(1.0),
            ..Default::default()
        }
    }

    #[test]
    fn a_film_shows_its_title_app_and_place() {
        let s = section(&np(), SystemTime::UNIX_EPOCH).unwrap();
        assert_eq!((s.label.as_str(), s.line.as_str()), ("PLAYING", "A Film"));
        assert_eq!(s.detail, "Disney+ \u{b7} 1:12:40 of 2:17:02");
        assert_eq!(label(&np()).as_deref(), Some("Disney+ \u{b7} A Film"));
        // Paused, it says so; the position holds.
        let paused = NowPlaying {
            state: Some(State::Paused),
            rate: Some(0.0),
            ..np()
        };
        let later = SystemTime::UNIX_EPOCH + Duration::from_secs(60);
        assert_eq!(
            section(&paused, later).unwrap().detail,
            "Disney+ \u{b7} 1:12:40 of 2:17:02 \u{b7} paused"
        );
    }

    #[test]
    fn an_episode_and_a_song_read_as_they_should() {
        let ep = NowPlaying {
            series: Some("A Show".into()),
            season: Some(1),
            episode: Some(2),
            title: Some("Brothers".into()),
            duration: Some(1882.0),
            elapsed: Some(825.0),
            ..np()
        };
        let s = section(&ep, SystemTime::UNIX_EPOCH).unwrap();
        assert_eq!(s.line, "A Show \u{b7} Brothers");
        assert_eq!(
            s.detail,
            "Disney+ \u{b7} season 1, episode 2 \u{b7} 13:45 of 31:22"
        );
        assert_eq!(
            label(&ep).as_deref(),
            Some("Disney+ \u{b7} A Show \u{b7} S1 E2 \u{b7} Brothers")
        );
        let song = NowPlaying {
            app_name: Some("Music".into()),
            title: Some("A Song".into()),
            artist: Some("A Band".into()),
            duration: Some(236.0),
            elapsed: Some(15.7),
            ..np()
        };
        assert_eq!(
            section(&song, SystemTime::UNIX_EPOCH).unwrap().line,
            "A Song \u{b7} A Band"
        );
    }

    #[test]
    fn it_is_on_screen_only_on_its_input() {
        use crate::avr::Seen;
        let on = |input: &str, name: Option<&str>| Seen {
            on: true,
            input: input.into(),
            name: name.map(Into::into),
        };
        let atv = on("MPLAY", Some("Apple TV"));
        let disc = on("BD", Some("Panasonic BD"));
        // By name or by code, in any case.
        assert!(on_screen("Apple TV", Some(&atv)));
        assert!(on_screen("mplay", Some(&atv)));
        assert!(!on_screen("Apple TV", Some(&disc)));
        // The receiver in standby: nothing is.
        let off = Seen {
            on: false,
            ..atv.clone()
        };
        assert!(!on_screen("Apple TV", Some(&off)));
        // No setting, or no word from the receiver: it cannot be told.
        assert!(on_screen("", Some(&disc)));
        assert!(on_screen("Apple TV", None));
    }

    #[test]
    fn an_app_that_says_nothing_has_no_section() {
        let netflix = NowPlaying {
            app_name: Some("Netflix".into()),
            state: Some(State::Playing),
            ..Default::default()
        };
        assert!(section(&netflix, SystemTime::UNIX_EPOCH).is_none());
        assert!(label(&netflix).is_none());
    }
}
