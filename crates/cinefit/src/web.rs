//! The web page (#74): cinefit's status and every setting, in a browser on
//! the home network, open (no PIN, as decided). A small HTTP server on a
//! thread of its own: nothing here runs on the frame path.
//!
//! `play` publishes, about once a second, a snapshot of the status and the
//! settings ([`Web::publish`]); the page reads it as JSON and asks for it
//! again a second later. A change posted from the page is checked against
//! the settings table here, so a bad value is refused at once, and queued
//! for `play`, which applies and saves it exactly as a change from the menu
//! ([`Web::take_changes`]). No picture of the screen: a preview is being
//! worked out on its own branch (`preview-720`).
//!
//! Endpoints: `/` the page; `/api/state` the snapshot;
//! `POST /api/set` with `{"key": …, "value": …}`; `/api/logs` and
//! `/api/logs/<name>` the run logs in `~/cinefit-logs`; `/api/atv` whether
//! the Apple TV is paired, and `POST /api/atv/pair/start` and
//! `/api/atv/pair/finish` with `{"pin": …}` to pair it (#115); `POST
//! /api/save` saves the frame on screen (#92).
//!
//! Open, but only to the home network itself (#268): a request whose Host
//! is a public name is refused, and so is a POST from another site's page.

use std::io::Read;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use log::{info, warn};
use serde_json::{Value as Json, json};
use tiny_http::{Header, Method, Response, Server};

use crate::config::{self, Apply, Config, Kind};

/// The page, with its style and script, in the binary.
const PAGE: &str = include_str!("../assets/web/index.html");

/// What `play` and the server share.
#[derive(Default)]
struct Shared {
    /// The last snapshot, as JSON.
    state: Mutex<String>,
    /// The settings as they are, to check a change against.
    config: Mutex<Option<Config>>,
    /// Changes posted from the page, for `play` to apply: key and value as
    /// typed, already checked.
    changes: Mutex<Vec<(String, String)>>,
    /// An Apple TV pairing begun, waiting for the PIN it shows.
    pairing: Mutex<Option<cinefit_atv::pair::PairSetup>>,
    /// Save frame presses not yet taken by `play` (#92).
    saves: Mutex<u32>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

pub struct Web {
    shared: Arc<Shared>,
}

impl Web {
    /// Serves on `port` on every address. `None` if the port is 0 or taken:
    /// the run goes on without a web page, and says so.
    pub fn start(port: u16) -> Option<Self> {
        if port == 0 {
            info!("web: off (web.port = 0)");
            return None;
        }
        let server = match Server::http(("0.0.0.0", port)) {
            Ok(s) => s,
            Err(e) => {
                warn!("web: no web page, port {port} not available: {e}");
                return None;
            }
        };
        info!("web: serving the page on port {port}");
        let shared = Arc::new(Shared::default());
        let s = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("web".into())
            .spawn(move || {
                for request in server.incoming_requests() {
                    serve(&s, request);
                }
            })
            .ok()?;
        Some(Self { shared })
    }

    /// The status and settings as they are now.
    pub fn publish(&self, state: Json, config: &Config) {
        *lock(&self.shared.state) = state.to_string();
        *lock(&self.shared.config) = Some(config.clone());
    }

    /// Changes posted since the last call, oldest first.
    pub fn take_changes(&self) -> Vec<(String, String)> {
        std::mem::take(&mut *lock(&self.shared.changes))
    }

    /// Whether Save frame was pressed since the last call (#92); presses in
    /// between count as one.
    pub fn take_save(&self) -> bool {
        std::mem::take(&mut *lock(&self.shared.saves)) > 0
    }
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("a valid header")
}

fn json_response(code: u16, body: &Json) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string(body.to_string())
        .with_status_code(code)
        .with_header(header("Content-Type", "application/json"))
        .with_header(header("Cache-Control", "no-store"))
}

/// The value of a request header, if it was sent.
fn header_value<'a>(request: &'a tiny_http::Request, name: &'static str) -> Option<&'a str> {
    request
        .headers()
        .iter()
        .find(|h| h.field.equiv(name))
        .map(|h| h.value.as_str())
}

/// Whether a request's Host is one the home network itself gives: an IP
/// address, `localhost`, a bare hostname, or a name under a suffix no one
/// can register in public DNS (`.local` from mDNS, `.lan`, `.home.arpa`,
/// `.internal`). DNS rebinding needs a name its attacker serves, which is
/// a public dotted one, so those are what is refused; the owner's ways in
/// (the IP, the hostname, `name.local`) all pass. No Host at all is a
/// client that is not a browser (HTTP/1.0, curl by hand): let through.
fn host_allowed(host: Option<&str>) -> bool {
    let Some(host) = host else { return true };
    let host = host.trim();
    // A bracketed IPv6 address, with or without a port.
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split_once(']').is_some_and(|(ip, port)| {
            ip.parse::<std::net::Ipv6Addr>().is_ok()
                && (port.is_empty() || port.strip_prefix(':').is_some_and(is_port))
        });
    }
    let name = match host.rsplit_once(':') {
        Some((name, port)) if is_port(port) => name,
        Some(_) => return false,
        None => host,
    };
    // `name.` is the same name as an absolute one.
    let name = name.strip_suffix('.').unwrap_or(name).to_ascii_lowercase();
    let hostname = |n: &str| {
        !n.is_empty()
            && n.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.'))
    };
    if !hostname(&name) {
        return false;
    }
    if name.parse::<std::net::Ipv4Addr>().is_ok() || !name.contains('.') {
        return true;
    }
    [".local", ".lan", ".home.arpa", ".internal", ".localhost"]
        .iter()
        .any(|suffix| name.ends_with(suffix))
}

fn is_port(p: &str) -> bool {
    !p.is_empty() && p.parse::<u16>().is_ok()
}

/// Whether a POST may change anything: a browser names the page that sent
/// it in Origin, and only the page served here (same host and port as the
/// request was sent to) may. Without an Origin it is not a browser page
/// (curl, a script on the network), which is allowed as before. A text/plain
/// POST from another site needs no preflight, so this is the check that
/// stops it; Origin `null` (a sandboxed or local file page) is refused.
fn origin_allowed(origin: Option<&str>, host: Option<&str>) -> bool {
    let Some(origin) = origin else { return true };
    let Some(host) = host else { return false };
    origin
        .trim()
        .strip_prefix("http://")
        .is_some_and(|o| o.eq_ignore_ascii_case(host.trim()))
}

fn serve(s: &Shared, mut request: tiny_http::Request) {
    let url = request.url().split('?').next().unwrap_or("").to_string();
    let host = header_value(&request, "Host");
    let refused = if !host_allowed(host) {
        Some("this page is served only by its address on the home network")
    } else if *request.method() == Method::Post
        && !origin_allowed(header_value(&request, "Origin"), host)
    {
        Some("changes are taken only from cinefit's own page")
    } else {
        None
    };
    if let Some(why) = refused {
        warn!(
            "web: refused {} {url} (Host {:?}, Origin {:?})",
            request.method(),
            host.unwrap_or(""),
            header_value(&request, "Origin").unwrap_or("")
        );
        let _ = request.respond(json_response(403, &json!({"error": why})));
        return;
    }
    let reply = match (request.method(), url.as_str()) {
        (Method::Get, "/") => Response::from_string(PAGE)
            .with_header(header("Content-Type", "text/html; charset=utf-8"))
            .boxed(),
        (Method::Get, "/api/state") => {
            let body = lock(&s.state).clone();
            Response::from_string(if body.is_empty() { "{}".into() } else { body })
                .with_header(header("Content-Type", "application/json"))
                .with_header(header("Cache-Control", "no-store"))
                .boxed()
        }
        (Method::Post, "/api/set") => {
            let mut body = String::new();
            let _ = request.as_reader().take(4096).read_to_string(&mut body);
            let (code, reply) = set(s, &body);
            json_response(code, &reply).boxed()
        }
        (Method::Post, "/api/save") => {
            *lock(&s.saves) += 1;
            json_response(200, &json!({"ok": true})).boxed()
        }
        (Method::Get, "/api/atv") => json_response(
            200,
            &json!({"paired": crate::appletv::credentials_path().exists()}),
        )
        .boxed(),
        (Method::Post, "/api/atv/pair/start") => {
            let (code, reply) = pair_start(s);
            json_response(code, &reply).boxed()
        }
        (Method::Post, "/api/atv/pair/finish") => {
            let mut body = String::new();
            let _ = request.as_reader().take(4096).read_to_string(&mut body);
            let (code, reply) = pair_finish(s, &body);
            json_response(code, &reply).boxed()
        }
        (Method::Get, "/api/logs") => json_response(200, &logs()).boxed(),
        (Method::Get, path) if path.starts_with("/api/logs/") => {
            match log_text(&path["/api/logs/".len()..]) {
                Some(text) => Response::from_string(text)
                    .with_header(header("Content-Type", "text/plain; charset=utf-8"))
                    .boxed(),
                None => Response::empty(404).boxed(),
            }
        }
        _ => Response::empty(404).boxed(),
    };
    let _ = request.respond(reply);
}

/// A change from the page: checked against the table now, queued for
/// `play`.
fn set(s: &Shared, body: &str) -> (u16, Json) {
    let parsed: Option<(String, String)> = serde_json::from_str::<Json>(body).ok().and_then(|j| {
        let key = j.get("key")?.as_str()?.to_string();
        let value = match j.get("value")? {
            Json::String(v) => v.clone(),
            other => other.to_string(),
        };
        Some((key, value))
    });
    let Some((key, value)) = parsed else {
        return (400, json!({"error": "expected {\"key\": …, \"value\": …}"}));
    };
    let Some(mut c) = lock(&s.config).clone() else {
        return (503, json!({"error": "not ready"}));
    };
    if let Err(e) = config::set(&mut c, &format!("{key}={value}")) {
        return (400, json!({"error": e.to_string()}));
    }
    lock(&s.changes).push((key, value));
    (200, json!({"ok": true}))
}

/// Asks the Apple TV at `atv.host` to show a PIN.
fn pair_start(s: &Shared) -> (u16, Json) {
    let host = lock(&s.config)
        .as_ref()
        .map(|c| c.atv_host.clone())
        .unwrap_or_default();
    if host.is_empty() {
        return (400, json!({"error": "set the Apple TV's address first"}));
    }
    match cinefit_atv::pair::start(&host) {
        Ok(p) => {
            info!("web: Apple TV pairing begun with {host}");
            *lock(&s.pairing) = Some(p);
            (200, json!({"ok": true}))
        }
        Err(e) => (502, json!({"error": format!("the Apple TV: {e}")})),
    }
}

/// The PIN typed in: the pairing finished and kept. A wrong PIN ends it;
/// the Apple TV shows a new one for the next try.
fn pair_finish(s: &Shared, body: &str) -> (u16, Json) {
    let pin = serde_json::from_str::<Json>(body)
        .ok()
        .and_then(|j| j.get("pin")?.as_str().map(str::to_string));
    let Some(pin) = pin.filter(|p| !p.trim().is_empty()) else {
        return (400, json!({"error": "expected {\"pin\": …}"}));
    };
    let Some(pairing) = lock(&s.pairing).take() else {
        return (409, json!({"error": "press Pair first"}));
    };
    match pairing
        .finish(&pin)
        .map_err(|e| e.to_string())
        .and_then(|c| crate::appletv::save_credentials(&c).map_err(|e| e.to_string()))
    {
        Ok(()) => {
            info!("web: Apple TV paired");
            (200, json!({"ok": true}))
        }
        Err(e) => {
            warn!("web: Apple TV pairing failed: {e}");
            (
                400,
                json!({"error": format!("{e}; press Pair for a new PIN")}),
            )
        }
    }
}

fn log_dir() -> Option<PathBuf> {
    crate::runlog::default_dir()
}

/// The newest run logs: name, size and when.
fn logs() -> Json {
    let Some(dir) = log_dir() else {
        return json!([]);
    };
    let mut entries: Vec<(String, u64, std::time::SystemTime)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            let name = e.file_name().to_string_lossy().into_owned();
            name.ends_with(".log")
                .then_some((name, m.len(), m.modified().ok()?))
        })
        .collect();
    entries.sort_by_key(|e| std::cmp::Reverse(e.2));
    Json::Array(
        entries
            .into_iter()
            .take(30)
            .map(|(name, size, _)| json!({"name": name, "size": size}))
            .collect(),
    )
}

/// A log's text, its last 256 KiB. Names are checked: only a plain file
/// in the logs directory.
fn log_text(name: &str) -> Option<String> {
    let ok = !name.is_empty()
        && name.ends_with(".log")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !name.contains("..");
    if !ok {
        return None;
    }
    let bytes = std::fs::read(log_dir()?.join(name)).ok()?;
    let tail = &bytes[bytes.len().saturating_sub(256 * 1024)..];
    Some(String::from_utf8_lossy(tail).into_owned())
}

/// Where each setting appears on the page, under which label: the menu's
/// pages, plus the settings only the page offers.
fn place(key: &str) -> (&'static str, &'static str) {
    match key {
        "detect.crop" => ("Picture", "Crop black bars"),
        "geometry.fill_max_ar" => ("Picture", "Fill the screen up to"),
        "osd.banners" => ("Picture", "Banners"),
        "detect.record" => ("Picture", "Record for detector tuning"),
        "detect.frames_gb" => ("Picture", "Disk for kept frames, GB (0: none)"),
        "detect.pair_every" => (
            "Picture",
            "Pairing passes: keep every Nth frame of each shot (0: off)",
        ),
        "detect.pair_follow" => (
            "Picture",
            "Pairing passes: follow the tunnel pass (SESSION:FIRST-LAST)",
        ),
        "output.mode" => ("Output", "HDR10"),
        "output.wait_hz" => ("Output", "Rate while waiting"),
        "output.idle_off" => (
            "Output",
            "Stop the output without a signal, minutes (0: never)",
        ),
        "tonemap.curve" => ("Output", "Tone curve"),
        "tonemap.peak_detect" => ("Output", "Tone curve follows each scene"),
        "tonemap.contrast_recovery" => ("Output", "Contrast recovery (0: off)"),
        "tonemap.peak_percentile" => ("Output", "Peak detection percentile (100: brightest pixel)"),
        "tonemap.knee_offset" => ("Output", "BT.2390 knee offset (0: libplacebo's)"),
        "tonemap.source_peak" => ("Output", "HDR10 source peak, nits (0: from the disc)"),
        "tonemap.peak_nits" => ("Output", "Screen peak, nits"),
        "tonemap.target_nits" => ("Output", "Map HDR into, nits (0: the screen peak)"),
        "geometry.screen_ar" => ("Screen", "Screen ratio"),
        "output.width" => ("Screen", "Panel width"),
        "output.height" => ("Screen", "Panel height"),
        "slate.venue" => ("Screen", "Theater name"),
        "input.rgb_range" => ("Input", "RGB range"),
        "input.lldv" => ("Input", "Dolby Vision low latency"),
        "input.lldv_nits" => ("Input", "Low-latency Dolby Vision peak, nits"),
        "input.device" => ("Input", "Capture device (persistent ID)"),
        "web.port" => ("Web page", "Port"),
        "avr.denon" => ("Receiver", "Denon AVR (hostname or IP)"),
        "atv.host" => ("Receiver", "Apple TV (hostname or IP)"),
        "atv.input" => ("Receiver", "Apple TV's receiver input"),
        "player.ub820" => ("Receiver", "Panasonic UB820 (hostname or IP)"),
        _ => ("Other", ""),
    }
}

/// Every setting as the page shows it: page, label, value, what it takes,
/// what a change costs, and its help.
pub fn settings_json(c: &Config) -> Json {
    let rows: Vec<Json> = config::settings()
        .iter()
        .map(|s| {
            let (page, label) = place(s.key);
            let kind = match s.kind {
                Kind::Int { min, max } => json!({"type": "int", "min": min, "max": max}),
                Kind::Float { min, max } => json!({"type": "float", "min": min, "max": max}),
                Kind::Choice(names) => json!({"type": "choice", "choices": names}),
                Kind::Text { max } => json!({"type": "text", "max": max}),
                Kind::Ratio { min, max, off } => {
                    json!({"type": "ratio", "min": min, "max": max, "off": off})
                }
            };
            json!({
                "key": s.key,
                "page": page,
                "label": if label.is_empty() { s.key } else { label },
                "value": s.get(c).to_string(),
                "kind": kind,
                "cost": match s.apply {
                    Apply::Live => "Applies now, no resync",
                    Apply::Modeset => "Resyncs the projector",
                    Apply::Start => "Applies when cinefit next starts",
                },
                "help": s.help,
            })
        })
        .collect();
    Json::Array(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_setting_has_a_place_on_the_page() {
        for s in config::settings() {
            assert_ne!(place(s.key).0, "Other", "{} has no page", s.key);
        }
        let rows = settings_json(&Config::default());
        let crop = rows
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["key"] == "detect.crop")
            .unwrap();
        assert_eq!(crop["value"], "on");
        assert_eq!(crop["kind"]["type"], "choice");
        assert_eq!(crop["cost"], "Applies now, no resync");
    }

    /// The page draws only the sections it lists: a setting placed in one
    /// it does not list never shows (the Receiver section, 2026-10-15).
    #[test]
    fn every_section_is_drawn_by_the_page() {
        let listed = PAGE
            .lines()
            .find(|l| l.starts_with("const PAGES"))
            .expect("the page lists its sections");
        for s in config::settings() {
            let page = place(s.key).0;
            assert!(
                listed.contains(&format!("\"{page}\"")),
                "{page} ({}) is not drawn",
                s.key
            );
        }
    }

    #[test]
    fn pairing_needs_an_address_and_a_start() {
        let s = Shared::default();
        *lock(&s.config) = Some(Config::default());
        assert_eq!(pair_start(&s).0, 400, "no address");
        assert_eq!(pair_finish(&s, r#"{"pin":"1234"}"#).0, 409, "not begun");
        assert_eq!(pair_finish(&s, "{}").0, 400, "no PIN");
    }

    #[test]
    fn a_change_is_checked_before_it_is_queued() {
        let s = Shared::default();
        assert_eq!(
            set(&s, r#"{"key":"detect.crop","value":"off"}"#).0,
            503,
            "no settings yet"
        );
        *lock(&s.config) = Some(Config::default());
        assert_eq!(set(&s, r#"{"key":"detect.crop","value":"off"}"#).0, 200);
        assert_eq!(
            set(&s, r#"{"key":"geometry.screen_ar","value":"16:9"}"#).0,
            200
        );
        assert_eq!(
            set(&s, r#"{"key":"tonemap.target_nits","value":150}"#).0,
            200
        );
        let (code, e) = set(&s, r#"{"key":"detect.crop","value":"maybe"}"#);
        assert_eq!(code, 400);
        assert!(e["error"].as_str().unwrap().contains("on | off"), "{e}");
        assert_eq!(set(&s, r#"{"key":"no.such","value":"1"}"#).0, 400);
        assert_eq!(set(&s, "not json").0, 400);
        let queued: Vec<String> = lock(&s.changes)
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        assert_eq!(
            queued,
            [
                "detect.crop=off",
                "geometry.screen_ar=16:9",
                "tonemap.target_nits=150"
            ]
        );
    }

    /// One HTTP request over a socket, the reply as text.
    fn http(port: u16, req: &str) -> String {
        use std::io::Write;
        let mut c = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        c.write_all(req.as_bytes()).unwrap();
        let mut out = String::new();
        c.read_to_string(&mut out).unwrap();
        out
    }

    #[test]
    fn the_server_serves_the_page_the_state_and_takes_a_change() {
        // A free port from the system, then served on it. Released in
        // between, so a test running beside this one can take it first:
        // tried a few times (it failed once that way, 2026-10-15).
        let (port, web) = (0..5)
            .find_map(|_| {
                let port = std::net::TcpListener::bind("127.0.0.1:0")
                    .ok()?
                    .local_addr()
                    .ok()?
                    .port();
                Web::start(port).map(|w| (port, w))
            })
            .expect("serves");
        web.publish(json!({"pill": "Playing"}), &Config::default());
        let page = http(port, "GET / HTTP/1.0\r\n\r\n");
        assert!(
            page.starts_with("HTTP/1.0 200") || page.starts_with("HTTP/1.1 200"),
            "{page:.80}"
        );
        assert!(page.contains("<title>cinefit</title>"));
        let state = http(port, "GET /api/state HTTP/1.0\r\n\r\n");
        assert!(state.contains(r#""pill":"Playing""#), "{state}");
        let body = r#"{"key":"detect.crop","value":"off"}"#;
        let reply = http(
            port,
            &format!(
                "POST /api/set HTTP/1.0\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            ),
        );
        assert!(reply.contains(r#"{"ok":true}"#), "{reply}");
        assert_eq!(
            web.take_changes(),
            [("detect.crop".to_string(), "off".to_string())]
        );
        assert!(http(port, "GET /nothing HTTP/1.0\r\n\r\n").contains(" 404 "));
        // Another site's page, and a rebound name: refused, nothing queued.
        let cross = http(
            port,
            &format!(
                "POST /api/set HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://evil.example.com\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            ),
        );
        assert!(cross.contains(" 403 "), "{cross:.80}");
        let rebound = http(
            port,
            "GET /api/state HTTP/1.1\r\nHost: evil.example.com:8080\r\nConnection: close\r\n\r\n",
        );
        assert!(rebound.contains(" 403 "), "{rebound:.80}");
        assert!(web.take_changes().is_empty());
    }

    #[test]
    fn only_home_network_hosts_are_served() {
        for ok in [
            None,
            Some("192.168.1.20:8080"),
            Some("192.168.1.20"),
            Some("[::1]:8080"),
            Some("[fe80::1]"),
            Some("localhost:8080"),
            Some("cinefit"),
            Some("Cinefit:8080"),
            Some("cinefit.local:8080"),
            Some("cinefit.lan"),
            Some("cinefit.home.arpa:8080"),
            Some("cinefit.local."),
        ] {
            assert!(host_allowed(ok), "{ok:?}");
        }
        for bad in [
            "evil.example.com:8080",
            "evil.com",
            "evil.com.",
            "192.168.1.20.nip.io",
            "local.evil.com",
            "cinefit:port",
            "cinefit:99999",
            "[::1",
            "[not-v6]:8080",
            "::1",
            ":8080",
            "",
        ] {
            assert!(!host_allowed(Some(bad)), "{bad}");
        }
    }

    #[test]
    fn only_the_page_itself_may_post() {
        let host = Some("192.168.1.20:8080");
        assert!(origin_allowed(None, host), "not a browser");
        assert!(origin_allowed(Some("http://192.168.1.20:8080"), host));
        assert!(origin_allowed(
            Some("http://Cinefit.local:8080"),
            Some("cinefit.local:8080")
        ));
        for bad in [
            "https://evil.example.com",
            "http://evil.example.com",
            "http://192.168.1.20",
            "http://192.168.1.20:8081",
            "null",
            "",
        ] {
            assert!(!origin_allowed(Some(bad), host), "{bad}");
        }
        assert!(!origin_allowed(Some("http://192.168.1.20:8080"), None));
    }

    #[test]
    fn only_plain_log_names_are_read() {
        for bad in ["../x.log", "a/b.log", "", "x.txt", "..log", "x .log"] {
            assert!(log_text(bad).is_none(), "{bad}");
        }
    }
}
