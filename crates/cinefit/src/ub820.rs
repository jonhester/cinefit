//! Play state and position of a Panasonic DP-UB820 Blu-ray player, read over
//! its network control interface. Reads only: this module sends no key presses
//! (the player refuses those without a secret key Panasonic issues to control
//! partners). Pressing keys is a separate path, over IR; see `cinefit-harmony`.
//!
//! The interface is the one Panasonic's own remote app and Home Assistant's
//! `panasonic_bluray` integration use, undocumented by Panasonic: a form POST
//! to `/WAN/dvdr/dvdr_ctrl.cgi`. The player must have **Voice Control** turned
//! on (Setup > Network); with it off the control port is closed.
//!
//! `cCMD_PST` (the position) is the one command that answers without
//! authentication. Its reply, measured on firmware 1.85 (2026-10-17), is two
//! lines:
//!
//! ```text
//! 00,"",1
//! 1,2077,0,00000000,,0
//! ```
//!
//! a result line (`00` ok), then `state,position,...` with the position in
//! seconds within the current title. State 0 is stopped, 1 playing, 2 paused.
//! The position is per title, not per disc: a menu is its own title with its
//! own clock, so moving from a menu into the feature resets it to 0. That
//! reset is the one signal that distinguishes a menu from the film over this
//! interface; the richer status (`REVIEW`, which names a menu state) needs the
//! secret key.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

/// The default HTTP port the control interface listens on.
pub const PORT: u16 = 80;

/// Panacotta sends this; the player is said to ignore requests without it.
const USER_AGENT: &str = "MEI-LAN-REMOTE-CALL";
const CONTROL_PATH: &str = "/WAN/dvdr/dvdr_ctrl.cgi";
const TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    /// Could not resolve the host to an address.
    NoAddress,
    /// It answered, but not in a form this understands. The raw body is kept so
    /// a caller can log what the player actually said.
    Protocol {
        what: &'static str,
        raw: String,
    },
    /// An HTTP status other than 200 (e.g. the control port refusing because
    /// Voice Control is off).
    Http(u16),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::NoAddress => write!(f, "host did not resolve to an address"),
            Error::Protocol { what, raw } => write!(f, "{what} (player said {raw:?})"),
            Error::Http(code) => write!(
                f,
                "player answered HTTP {code} (is Voice Control on in its network settings?)"
            ),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

/// Whether the player is stopped, playing or paused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Stopped,
    Playing,
    Paused,
}

impl State {
    fn from_code(c: &str) -> Option<State> {
        match c {
            "0" => Some(State::Stopped),
            "1" => Some(State::Playing),
            "2" => Some(State::Paused),
            _ => None,
        }
    }
}

/// One reading of the player's transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    pub state: State,
    /// Seconds into the current title. A title change (menu to feature, or the
    /// reverse) resets this to 0, which is how a menu is told from the film.
    pub position: u32,
}

/// The `cinefit ub820 <ip>` diagnostic: print the player's state and position,
/// once or on a watch (once a second). Reads only; presses nothing. A watch is
/// handy to see a disc move from its menu into the feature, where the position
/// resets to 0.
pub fn run(host: &str, watch: bool) -> Result<(), Box<dyn std::error::Error>> {
    let addr = format!("{host}:{PORT}");
    let show = |s: Status| {
        println!(
            "{:?} @ {}:{:02}:{:02}",
            s.state,
            s.position / 3600,
            s.position / 60 % 60,
            s.position % 60
        );
    };
    if !watch {
        show(read_status(&addr)?);
        return Ok(());
    }
    loop {
        match read_status(&addr) {
            Ok(s) => show(s),
            Err(e) => println!("error: {e}"),
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

/// Reads the player's current transport state and position with one `PST`
/// query. Reads only; presses nothing.
pub fn read_status<A: ToSocketAddrs>(host: A) -> Result<Status, Error> {
    let body = post(host, "cCMD_PST")?;
    parse_pst(&body).ok_or(Error::Protocol {
        what: "PST reply not in the expected 'result / state,position' form",
        raw: body,
    })
}

/// Sends one command form-POST and returns the response body. Lower level than
/// [`read_status`]; exposed so a caller can issue other read-only commands and
/// parse the raw reply itself.
pub fn post<A: ToSocketAddrs>(host: A, command: &str) -> Result<String, Error> {
    let addr = host
        .to_socket_addrs()?
        .next()
        .ok_or(Error::NoAddress)?;
    // The command is x/y form fields, as if an image-map button were clicked.
    let form = format!("{command}.x=100&{command}.y=100");
    let request = format!(
        "POST {CONTROL_PATH} HTTP/1.1\r\n\
         Host: {host}\r\n\
         User-Agent: {USER_AGENT}\r\n\
         Content-Type: application/x-www-form-urlencoded\r\n\
         Content-Length: {len}\r\n\
         Connection: close\r\n\
         \r\n\
         {form}",
        host = addr.ip(),
        len = form.len(),
    );

    let mut stream = TcpStream::connect_timeout(&addr, TIMEOUT)?;
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    stream.write_all(request.as_bytes())?;

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (status, body) = split_http(&text)?;
    if status != 200 {
        return Err(Error::Http(status));
    }
    Ok(body.to_string())
}

/// Splits an HTTP/1.x response into its status code and body. The player speaks
/// a plain enough HTTP that a hand split is safer than pulling in a client.
fn split_http(text: &str) -> Result<(u16, &str), Error> {
    let status = text
        .split_once(' ')
        .and_then(|(_, rest)| rest.split_whitespace().next())
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or(Error::Protocol {
            what: "no HTTP status line",
            raw: text.chars().take(80).collect(),
        })?;
    // Headers end at the first blank line; the body is whatever follows.
    let body = text
        .split_once("\r\n\r\n")
        .or_else(|| text.split_once("\n\n"))
        .map(|(_, b)| b)
        .unwrap_or("");
    Ok((status, body))
}

/// Parses a `PST` body into a [`Status`]. Returns `None` if it is not a result
/// line (`00...`) followed by a `state,position,...` line with a known state.
fn parse_pst(body: &str) -> Option<Status> {
    let mut lines = body.lines().map(str::trim).filter(|l| !l.is_empty());
    let result = lines.next()?;
    if !result.starts_with("00") {
        return None;
    }
    let data = lines.next()?;
    let mut fields = data.split(',').map(|f| f.trim().trim_matches('"'));
    let state = State::from_code(fields.next()?)?;
    let position = fields.next()?.parse::<u32>().ok()?;
    Some(Status { state, position })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_playing_reply() {
        let s = parse_pst("00,\"\",1\r\n1,2077,0,00000000,,0\r\n").unwrap();
        assert_eq!(s.state, State::Playing);
        assert_eq!(s.position, 2077);
    }

    #[test]
    fn parses_stopped_and_paused() {
        assert_eq!(
            parse_pst("00,\"\",1\n0,0,0,00000000,,0").unwrap().state,
            State::Stopped
        );
        assert_eq!(
            parse_pst("00,\"\",1\n2,20,0,00000000,,0").unwrap().state,
            State::Paused
        );
    }

    #[test]
    fn rejects_a_non_ok_result() {
        // The error reply seen when an unauthenticated key press is refused.
        assert!(parse_pst("52,\"garbage\"\r\n").is_none());
    }

    #[test]
    fn rejects_a_short_reply() {
        assert!(parse_pst("00,\"\",1\r\n").is_none());
        assert!(parse_pst("").is_none());
    }

    #[test]
    fn rejects_an_unknown_state() {
        assert!(parse_pst("00,\"\",1\r\n9,0,0").is_none());
    }

    #[test]
    fn splits_http_status_and_body() {
        let (code, body) =
            split_http("HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabc").unwrap();
        assert_eq!(code, 200);
        assert_eq!(body, "abc");
    }

    #[test]
    fn reports_a_refusing_status() {
        let (code, _) = split_http("HTTP/1.1 403 Forbidden\r\n\r\n").unwrap();
        assert_eq!(code, 403);
    }
}
