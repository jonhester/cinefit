//! The Denon receiver (#116): which input is on, by the owner's names for
//! them, and whether it is on at all. Reads only; never sends anything that
//! changes the receiver.
//!
//! Two of its interfaces, as the C++ version measured them (cinefit-cpp
//! `docs/findings.md`, *Denon control protocol*):
//!
//! - the web status, `GET :8080/goform/formMainZone_MainZoneXmlStatusLite.xml`:
//!   power and the input, polled. It holds no connection;
//! - the text protocol on port 23, CR-terminated, for the input names
//!   (`SSFUN ?`), read once: it allows one session at a time, so it is
//!   opened, asked and closed, never held.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// How often the status is read.
const POLL: Duration = Duration::from_secs(3);
/// Connecting and each read: a receiver that is off the network should not
/// hold anything up.
const TIMEOUT: Duration = Duration::from_secs(2);

/// What the receiver last said.
#[derive(Debug, Clone, PartialEq)]
pub struct Seen {
    pub on: bool,
    /// The input's code (`MPLAY`), and the owner's name for it, if known.
    pub input: String,
    pub name: Option<String>,
}

impl Seen {
    /// The input as people say it: the owner's name, else the code.
    pub fn source(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.input)
    }
}

/// Power and the input from the web status. `None` if it is not that page.
pub fn parse_status(xml: &str) -> Option<(bool, String)> {
    let value = |tag: &str| -> Option<String> {
        let start = xml.find(&format!("<{tag}>"))?;
        let rest = &xml[start..];
        let v = rest.find("<value>")? + "<value>".len();
        let end = rest[v..].find("</value>")?;
        Some(rest[v..v + end].trim().to_string())
    };
    let power = value("Power")?;
    let input = value("InputFuncSelect")?;
    Some((power.eq_ignore_ascii_case("ON"), input))
}

/// The input names from the text protocol's `SSFUN` replies: code, then the
/// name padded with spaces.
pub fn parse_names(text: &str) -> HashMap<String, String> {
    text.split(['\r', '\n'])
        .filter_map(|l| l.strip_prefix("SSFUN"))
        .filter_map(|l| {
            let (code, name) = l.split_once(' ')?;
            let name = name.trim();
            (!code.is_empty() && !name.is_empty() && code != "END")
                .then(|| (code.to_string(), name.to_string()))
        })
        .collect()
}

/// A background reader of one receiver; follows the address as it changes.
pub struct Receiver {
    shared: Arc<Mutex<Shared>>,
}

#[derive(Default)]
struct Shared {
    host: String,
    seen: Option<Seen>,
}

impl Receiver {
    /// Starts reading `host` (a hostname or an IP address; empty for none).
    pub fn start(host: &str) -> Self {
        let shared = Arc::new(Mutex::new(Shared {
            host: host.to_string(),
            seen: None,
        }));
        let s = Arc::clone(&shared);
        let _ = std::thread::Builder::new()
            .name("avr".into())
            .spawn(move || poll(&s));
        Self { shared }
    }

    /// A new address (the `avr.denon` setting, changed live).
    pub fn set_host(&self, host: &str) {
        let mut s = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        if s.host != host {
            s.host = host.to_string();
            s.seen = None;
        }
    }

    /// What the receiver last said; `None` with no address, or while it
    /// does not answer.
    pub fn seen(&self) -> Option<Seen> {
        self.shared
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .seen
            .clone()
    }
}

fn poll(shared: &Mutex<Shared>) {
    let mut names: HashMap<String, String> = HashMap::new();
    let mut names_for = String::new();
    let mut names_tried: Option<Instant> = None;
    loop {
        let host = shared
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .host
            .clone();
        let seen = if host.is_empty() {
            None
        } else {
            if names_for != host {
                names.clear();
                names_for.clone_from(&host);
                names_tried = None;
            }
            status(&host).map(|(on, input)| {
                // The names once, and again for an input with none, at most
                // once a minute: the text protocol is one session at a time.
                if !names.contains_key(&input)
                    && names_tried.is_none_or(|t| t.elapsed() > Duration::from_secs(60))
                {
                    names_tried = Some(Instant::now());
                    if let Some(n) = input_names(&host) {
                        names = n;
                    }
                }
                Seen {
                    on,
                    name: names.get(&input).cloned(),
                    input,
                }
            })
        };
        {
            let mut s = shared.lock().unwrap_or_else(PoisonError::into_inner);
            if s.host == host {
                s.seen = seen;
            }
        }
        std::thread::sleep(POLL);
    }
}

fn connect(host: &str, port: u16) -> Option<TcpStream> {
    let addr = (host, port).to_socket_addrs().ok()?.next()?;
    let s = TcpStream::connect_timeout(&addr, TIMEOUT).ok()?;
    s.set_read_timeout(Some(TIMEOUT)).ok()?;
    s.set_write_timeout(Some(TIMEOUT)).ok()?;
    Some(s)
}

fn status(host: &str) -> Option<(bool, String)> {
    let mut s = connect(host, 8080)?;
    write!(
        s,
        "GET /goform/formMainZone_MainZoneXmlStatusLite.xml HTTP/1.0\r\nHost: {host}\r\n\r\n"
    )
    .ok()?;
    let mut reply = String::new();
    s.read_to_string(&mut reply).ok()?;
    parse_status(&reply)
}

fn input_names(host: &str) -> Option<HashMap<String, String>> {
    let mut s = connect(host, 23)?;
    s.write_all(b"SSFUN ?\r").ok()?;
    // Read until the list ends or the receiver goes quiet; it also sends
    // unrelated status lines, which parse_names skips.
    let mut text = String::new();
    let mut buf = [0u8; 1024];
    let started = Instant::now();
    while started.elapsed() < TIMEOUT && !text.contains("SSFUN END") {
        match s.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => text.push_str(&String::from_utf8_lossy(&buf[..n])),
        }
    }
    let names = parse_names(&text);
    (!names.is_empty()).then_some(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_status_page_gives_power_and_input() {
        // As the AVR-X3700H answered on 2026-10-15.
        let xml = "<?xml version=\"1.0\" encoding=\"utf-8\" ?>\n<item>\n\
                   <Power><value>ON</value></Power>\n\
                   <InputFuncSelect><value>MPLAY</value></InputFuncSelect>\n\
                   <VolumeDisplay><value>Relative</value></VolumeDisplay>\n\
                   <MasterVolume><value>-44.0</value></MasterVolume>\n</item>\n";
        assert_eq!(parse_status(xml), Some((true, "MPLAY".to_string())));
        let off = xml.replace("<value>ON</value>", "<value>STANDBY</value>");
        assert_eq!(parse_status(&off), Some((false, "MPLAY".to_string())));
        assert_eq!(parse_status("<html>301 Moved Permanently</html>"), None);
    }

    #[test]
    fn the_names_come_from_ssfun() {
        // As the receiver answered, CR-terminated, names padded, with an
        // unrelated status line mixed in.
        let text = "SSFUNDVD DVD             \rSSFUNBD Panasonic BD    \rMVMAX 98\r\
                    SSFUNMPLAY Apple TV        \rSSFUNSAT/CBL CBL/SAT         \rSSFUN END\r";
        let n = parse_names(text);
        assert_eq!(n.get("MPLAY").map(String::as_str), Some("Apple TV"));
        assert_eq!(n.get("BD").map(String::as_str), Some("Panasonic BD"));
        assert_eq!(n.get("SAT/CBL").map(String::as_str), Some("CBL/SAT"));
        assert_eq!(n.len(), 4);
    }

    #[test]
    fn a_source_is_its_name_when_known() {
        let s = Seen {
            on: true,
            input: "MPLAY".into(),
            name: Some("Apple TV".into()),
        };
        assert_eq!(s.source(), "Apple TV");
        let unnamed = Seen { name: None, ..s };
        assert_eq!(unnamed.source(), "MPLAY");
    }

    /// Against a real receiver, read-only: `CINEFIT_DENON=<host> cargo test
    /// -p cinefit avr -- --ignored`.
    #[test]
    #[ignore = "needs a receiver on the network"]
    fn a_real_receiver_answers() {
        let host = std::env::var("CINEFIT_DENON").expect("CINEFIT_DENON");
        let r = Receiver::start(&host);
        let started = Instant::now();
        while r.seen().is_none() && started.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(200));
        }
        let seen = r.seen().expect("an answer within 10 s");
        eprintln!("{seen:?}: source {}", seen.source());
        assert!(!seen.input.is_empty());
    }

    #[test]
    fn no_address_reads_nothing() {
        let r = Receiver::start("");
        assert_eq!(r.seen(), None);
    }
}
