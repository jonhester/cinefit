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
//! `/api/logs/<name>` the run logs in `~/cinefit-logs`.

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

fn serve(s: &Shared, mut request: tiny_http::Request) {
    let url = request.url().split('?').next().unwrap_or("").to_string();
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
        "output.mode" => ("Output", "HDR10"),
        "output.wait_hz" => ("Output", "Rate while waiting"),
        "output.white_nits" => ("Output", "SDR white and low-latency Dolby Vision, nits"),
        "tonemap.curve" => ("Output", "Tone curve"),
        "tonemap.source_peak" => ("Output", "HDR10 source peak, nits (0: from the disc)"),
        "geometry.screen_ar" => ("Screen", "Screen ratio"),
        "output.width" => ("Screen", "Panel width"),
        "output.height" => ("Screen", "Panel height"),
        "slate.venue" => ("Screen", "Theater name"),
        "input.rgb_range" => ("Input", "RGB range"),
        "input.lldv" => ("Input", "Dolby Vision low latency"),
        "web.port" => ("Web page", "Port"),
        "avr.denon" => ("Receiver", "Denon AVR (hostname or IP)"),
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
        assert_eq!(set(&s, r#"{"key":"output.white_nits","value":96}"#).0, 200);
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
                "output.white_nits=96"
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
        // A free port from the system, then served on it.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let web = Web::start(port).expect("serves");
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
    }

    #[test]
    fn only_plain_log_names_are_read() {
        for bad in ["../x.log", "a/b.log", "", "x.txt", "..log", "x .log"] {
            assert!(log_text(bad).is_none(), "{bad}");
        }
    }
}
