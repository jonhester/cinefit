//! Send IR commands through a Logitech Harmony Hub over its local network API.
//! The Hub blasts IR at the devices it is set up for, so this is how cinefit
//! presses buttons on a player that has no usable network control of its own
//! (the UB820: it reports state over the network but refuses key presses
//! without a secret key; see `cinefit-ub820`). The same Hub can drive any
//! device it knows, the projector and receiver included.
//!
//! The Hub's local API needs no authentication. Two transports are involved:
//!
//! 1. a plain HTTP POST to port 8088 (`setup.account?getProvisionInfo`) returns
//!    the Hub's `activeRemoteId`, which the next step needs;
//! 2. a WebSocket on the same port carries everything else as JSON messages:
//!    the configuration (devices and their commands) and `holdAction` to press
//!    a key.
//!
//! Written from the local protocol as the open-source clients use it (aioharmony
//! and pyharmony, both under permissive licences). None is a dependency.
//!
//! A command's **label is not always what to send**: the config gives each
//! button a label and, inside its action, the real IR command string. The OK
//! button is labelled `Select` but its command is `OK`. Always send
//! [`Command::command`], not the label.

mod ws;

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

use log::debug;
use serde_json::{Value, json};

use ws::WebSocket;

/// The Hub's local API port (HTTP and WebSocket share it).
pub const PORT: u16 = 8088;

/// The Origin the Hub expects on both the HTTP and the WebSocket handshake.
const ORIGIN: &str = "http://sl.dhg.myharmony.com";
const DOMAIN: &str = "svcs.myharmony.com";
const TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    NoAddress,
    /// An HTTP step returned a status other than the one expected.
    Http(u16),
    /// The WebSocket upgrade was refused or malformed.
    Upgrade(String),
    /// A reply was not in the form this understands.
    Protocol(String),
    /// The Hub answered, but with a non-success code for the request.
    Refused(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::NoAddress => write!(f, "host did not resolve to an address"),
            Error::Http(c) => write!(f, "hub answered HTTP {c}"),
            Error::Upgrade(s) => write!(f, "websocket upgrade failed: {s}"),
            Error::Protocol(s) => write!(f, "{s}"),
            Error::Refused(s) => write!(f, "hub refused: {s}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

type Result<T> = std::result::Result<T, Error>;

/// One button on a device: what the Hub shows it as, and the IR command string
/// to actually send.
#[derive(Debug, Clone)]
pub struct Command {
    /// The Hub's label for the button, e.g. `Select`, `SkipForward`.
    pub label: String,
    /// The IR command string to send, e.g. `OK`, `SkipForward`. Often equal to
    /// the label, but not always.
    pub command: String,
}

/// A device the Hub can control.
#[derive(Debug, Clone)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub manufacturer: String,
    pub model: String,
    pub commands: Vec<Command>,
}

impl Device {
    /// The IR command string for a button, matched case-insensitively against
    /// the label first and then the command. So `device.command_for("ok")`
    /// finds the button labelled `Select` whose command is `OK`.
    pub fn command_for(&self, name: &str) -> Option<&str> {
        self.commands
            .iter()
            .find(|c| c.label.eq_ignore_ascii_case(name))
            .or_else(|| {
                self.commands
                    .iter()
                    .find(|c| c.command.eq_ignore_ascii_case(name))
            })
            .map(|c| c.command.as_str())
    }
}

/// A live connection to the Hub. Holds one WebSocket; cheap to open per task
/// and closed on drop.
pub struct Hub {
    ws: WebSocket<TcpStream>,
    remote_id: String,
    next_id: u32,
}

impl Hub {
    /// Opens a connection: fetches the Hub's remote id over HTTP, then upgrades
    /// a WebSocket to it.
    pub fn connect<A: ToSocketAddrs>(host: A) -> Result<Hub> {
        let addr = host.to_socket_addrs()?.next().ok_or(Error::NoAddress)?;
        let remote_id = provision_info(addr)?;
        debug!("harmony hub {addr}: remote id {remote_id}");
        let ws = ws_upgrade(addr, &remote_id)?;
        Ok(Hub {
            ws,
            remote_id,
            next_id: 0,
        })
    }

    fn request_id(&mut self) -> String {
        let id = self.next_id;
        self.next_id += 1;
        id.to_string()
    }

    /// Reads the Hub's configuration: every device it controls and that
    /// device's buttons.
    pub fn devices(&mut self) -> Result<Vec<Device>> {
        let id = self.request_id();
        let data = self.request(
            &id,
            "vnd.logitech.harmony/vnd.logitech.harmony.engine?config",
            json!({ "verb": "get" }),
        )?;
        let devices = data
            .get("device")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::Protocol("config had no device list".into()))?;
        Ok(devices.iter().map(parse_device).collect())
    }

    /// Finds a device by a substring of its name (case-insensitive).
    pub fn device_named(&mut self, needle: &str) -> Result<Option<Device>> {
        let needle = needle.to_ascii_lowercase();
        Ok(self
            .devices()?
            .into_iter()
            .find(|d| d.name.to_ascii_lowercase().contains(&needle)))
    }

    /// Presses one IR command on a device: a `press` then a `release`, which is
    /// how the Hub renders a single button tap. `command` is the IR command
    /// string ([`Command::command`]), not the label.
    pub fn press(&mut self, device_id: &str, command: &str) -> Result<()> {
        // The action is itself a JSON string carried in a field.
        let action = json!({
            "command": command,
            "type": "IRCommand",
            "deviceId": device_id,
        })
        .to_string();

        for (status, timestamp) in [("press", "0"), ("release", "150")] {
            let id = self.request_id();
            self.send(
                &id,
                "vnd.logitech.harmony/vnd.logitech.harmony.engine?holdAction",
                json!({
                    "status": status,
                    "timestamp": timestamp,
                    "verb": "render",
                    "action": action,
                }),
            )?;
        }
        // The Hub acks asynchronously; a failed render would come back as a
        // non-200 code, but a missing ack is not itself a failure to press.
        self.drain_acks()?;
        Ok(())
    }

    /// Sends a message and waits for the reply that carries our request id,
    /// returning its `data` object.
    fn request(&mut self, id: &str, cmd: &str, params: Value) -> Result<Value> {
        self.send(id, cmd, params)?;
        loop {
            let msg = self
                .ws
                .read_text()?
                .ok_or_else(|| Error::Protocol("hub closed before replying".into()))?;
            let v: Value = serde_json::from_str(&msg)
                .map_err(|e| Error::Protocol(format!("reply was not JSON: {e}")))?;
            // The Hub interleaves unsolicited state messages; keep only ours.
            if v.get("id").and_then(Value::as_str) != Some(id) {
                continue;
            }
            let code = v.get("code").and_then(Value::as_i64).unwrap_or(0);
            if !(200..300).contains(&code) {
                let msg = v.get("msg").and_then(Value::as_str).unwrap_or("");
                return Err(Error::Refused(format!("code {code} {msg}")));
            }
            return Ok(v.get("data").cloned().unwrap_or(Value::Null));
        }
    }

    fn send(&mut self, id: &str, cmd: &str, params: Value) -> Result<()> {
        let hub_id: u64 = self.remote_id.parse().unwrap_or(0);
        let message = json!({
            "hubId": hub_id,
            "timeout": 30,
            "hbus": { "cmd": cmd, "id": id, "params": params },
        });
        self.ws.send_text(&message.to_string())?;
        Ok(())
    }

    /// Reads and discards any immediately available ack frames after a press,
    /// so they do not pile up before the next request. Best effort: a read
    /// timeout just ends the drain.
    fn drain_acks(&mut self) -> Result<()> {
        // A short read timeout for the non-blocking drain, restored after.
        self.ws
            .get_ref()
            .set_read_timeout(Some(Duration::from_millis(400)))?;
        loop {
            match self.ws.read_text() {
                Ok(Some(_)) => continue,
                Ok(None) => break,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => break,
                Err(e) => return Err(e.into()),
            }
        }
        self.ws.get_ref().set_read_timeout(Some(TIMEOUT))?;
        Ok(())
    }
}

/// The `data` of a device entry, into a [`Device`].
fn parse_device(d: &Value) -> Device {
    let s = |k: &str| d.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let mut commands = Vec::new();
    if let Some(groups) = d.get("controlGroup").and_then(Value::as_array) {
        for g in groups {
            let Some(fns) = g.get("function").and_then(Value::as_array) else {
                continue;
            };
            for f in fns {
                let label = f.get("name").and_then(Value::as_str).unwrap_or("");
                // The action is a JSON string; its "command" is what to send.
                let command = f
                    .get("action")
                    .and_then(Value::as_str)
                    .and_then(|a| serde_json::from_str::<Value>(a).ok())
                    .and_then(|a| a.get("command").and_then(Value::as_str).map(str::to_owned))
                    .unwrap_or_else(|| label.to_string());
                if !label.is_empty() {
                    commands.push(Command {
                        label: label.to_string(),
                        command,
                    });
                }
            }
        }
    }
    Device {
        id: s("id"),
        name: s("label"),
        manufacturer: s("manufacturer"),
        model: s("model"),
        commands,
    }
}

/// HTTP POST to the Hub asking for its account provisioning, which carries the
/// `activeRemoteId` the WebSocket needs.
fn provision_info(addr: SocketAddr) -> Result<String> {
    let body = r#"{"id":1,"cmd":"setup.account?getProvisionInfo","params":{}}"#;
    let request = format!(
        "POST / HTTP/1.1\r\n\
         Host: {host}\r\n\
         Origin: {ORIGIN}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {len}\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        host = addr,
        len = body.len(),
    );
    let mut stream = TcpStream::connect_timeout(&addr, TIMEOUT)?;
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.write_all(request.as_bytes())?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;
    let text = String::from_utf8_lossy(&raw);
    let (status, body) = split_http(&text)?;
    if status != 200 {
        return Err(Error::Http(status));
    }
    let v: Value = serde_json::from_str(body)
        .map_err(|e| Error::Protocol(format!("provision reply was not JSON: {e}")))?;
    remote_id_from_provision(&v)
        .ok_or_else(|| Error::Protocol("provision reply had no activeRemoteId".into()))
}

fn remote_id_from_provision(v: &Value) -> Option<String> {
    let id = v.get("data")?.get("activeRemoteId")?;
    match id {
        Value::Number(n) => Some(n.to_string()),
        Value::String(s) => Some(s.clone()),
        _ => None,
    }
}

/// Opens the WebSocket to the Hub: the HTTP upgrade, then a [`WebSocket`] over
/// the same stream.
fn ws_upgrade(addr: SocketAddr, remote_id: &str) -> Result<WebSocket<TcpStream>> {
    let mut stream = TcpStream::connect_timeout(&addr, TIMEOUT)?;
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;

    // A client key: 16 bytes, base64. It need not be cryptographic; the Hub
    // does not challenge it.
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let key = ws::base64(&nonce.to_le_bytes());

    let request = format!(
        "GET /?domain={DOMAIN}&hubId={remote_id} HTTP/1.1\r\n\
         Host: {addr}\r\n\
         Origin: {ORIGIN}\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: {key}\r\n\
         Sec-WebSocket-Version: 13\r\n\
         \r\n",
    );
    stream.write_all(request.as_bytes())?;

    // Read response headers byte by byte so nothing past the blank line (the
    // first WebSocket frames) is consumed.
    let headers = read_http_headers(&mut stream)?;
    let status = headers
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse::<u16>().ok())
        .ok_or_else(|| Error::Upgrade("no status line".into()))?;
    if status != 101 {
        return Err(Error::Upgrade(format!("expected 101, got {status}")));
    }
    Ok(WebSocket::new(stream))
}

/// Reads up to and including the header-terminating blank line.
fn read_http_headers(stream: &mut TcpStream) -> Result<String> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    while stream.read_exact(&mut byte).is_ok() {
        buf.push(byte[0]);
        if buf.ends_with(b"\r\n\r\n") {
            break;
        }
        if buf.len() > 8192 {
            return Err(Error::Upgrade("headers too long".into()));
        }
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn split_http(text: &str) -> Result<(u16, &str)> {
    let status = text
        .split_once(' ')
        .and_then(|(_, rest)| rest.split_whitespace().next())
        .and_then(|c| c.parse::<u16>().ok())
        .ok_or_else(|| Error::Protocol("no HTTP status line".into()))?;
    let body = text
        .split_once("\r\n\r\n")
        .or_else(|| text.split_once("\n\n"))
        .map(|(_, b)| b)
        .unwrap_or("");
    Ok((status, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = r#"{
        "id":"0","code":200,"msg":"OK",
        "data":{"device":[{
            "id":"10000001","label":"Panasonic DVD/Blu-ray Player",
            "manufacturer":"Panasonic","model":"DP-UB820",
            "controlGroup":[{"name":"NavigationBasic","function":[
                {"name":"Select","label":"Select","action":"{\"command\":\"OK\",\"type\":\"IRCommand\",\"deviceId\":\"10000001\"}"},
                {"name":"SkipForward","label":"SkipForward","action":"{\"command\":\"SkipForward\",\"type\":\"IRCommand\",\"deviceId\":\"10000001\"}"}
            ]}]
        }]}
    }"#;

    #[test]
    fn parses_device_and_remaps_label_to_command() {
        let v: Value = serde_json::from_str(CONFIG).unwrap();
        let dev = v
            .get("data")
            .unwrap()
            .get("device")
            .unwrap()
            .as_array()
            .unwrap();
        let d = parse_device(&dev[0]);
        assert_eq!(d.name, "Panasonic DVD/Blu-ray Player");
        assert_eq!(d.model, "DP-UB820");
        // The OK button is labelled Select; its command is OK.
        assert_eq!(d.command_for("Select"), Some("OK"));
        assert_eq!(d.command_for("ok"), Some("OK"));
        assert_eq!(d.command_for("SkipForward"), Some("SkipForward"));
        assert_eq!(d.command_for("nope"), None);
    }

    #[test]
    fn reads_remote_id_as_number_or_string() {
        let n: Value = serde_json::from_str(r#"{"data":{"activeRemoteId":12345678}}"#).unwrap();
        assert_eq!(remote_id_from_provision(&n).as_deref(), Some("12345678"));
        let s: Value = serde_json::from_str(r#"{"data":{"activeRemoteId":"42"}}"#).unwrap();
        assert_eq!(remote_id_from_provision(&s).as_deref(), Some("42"));
        let none: Value = serde_json::from_str(r#"{"data":{}}"#).unwrap();
        assert_eq!(remote_id_from_provision(&none), None);
    }

    #[test]
    fn splits_http_body() {
        let (code, body) = split_http("HTTP/1.1 200 OK\r\nX: 1\r\n\r\n{\"a\":1}").unwrap();
        assert_eq!(code, 200);
        assert_eq!(body, "{\"a\":1}");
    }
}
