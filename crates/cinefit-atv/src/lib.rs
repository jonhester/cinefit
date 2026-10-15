//! What an Apple TV is playing (#115): the app, the title, where in it, and
//! whether it is playing, read over the remote-control channel AirPlay 2
//! tunnels. Reads only; it sends nothing that changes what the Apple TV
//! does.
//!
//! Written from pyatv (MIT, <https://github.com/postlund/pyatv>; see the
//! crate's README for what came from where and its licence), which
//! stays a development tool to check this against, never a dependency. The
//! steps, as pyatv takes them (`protocols/airplay/ap2_session.py`):
//!
//! 1. pair-verify over HTTP on the AirPlay port with credentials from one
//!    pairing ([`hap`]), after which that connection is encrypted;
//! 2. RTSP on it: `SETUP` an event channel (which the Apple TV talks on and
//!    is answered `200`), `RECORD`, `SETUP` a remote-control data stream;
//! 3. on the data stream, protobuf messages ([`proto`], [`stream`]): our
//!    device information first, then the Apple TV sends its state;
//! 4. `POST /feedback` on the control connection every 2 s keeps it all up.

pub mod conn;
pub mod hap;
pub mod proto;
pub mod stream;
pub mod tlv8;

use std::collections::HashMap;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use log::{debug, info, warn};

use conn::{Conn, Message};
use hap::{Credentials, PairVerify, Session};
use stream::Frame;

pub use hap::Credentials as AtvCredentials;

/// The AirPlay port.
pub const PORT: u16 = 7000;
/// Connecting and each setup step.
const TIMEOUT: Duration = Duration::from_secs(10);
/// How often the control connection is told we are still here.
const FEEDBACK: Duration = Duration::from_secs(2);
/// The name the Apple TV knows this reader by.
const NAME: &str = "cinefit";

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    /// The other end closed the connection.
    Closed,
    /// It answered, but not in a way this understands.
    Protocol(&'static str),
    /// It answered no: unknown credentials, another device, a refused step.
    Refused(String),
    Crypto(&'static str),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::Closed => write!(f, "the Apple TV closed the connection"),
            Error::Protocol(s) | Error::Crypto(s) => write!(f, "{s}"),
            Error::Refused(s) => write!(f, "{s}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

/// An upper-case random UUID, the form Apple's messages carry.
pub(crate) fn uuid() -> String {
    let mut b = [0u8; 16];
    getrandom::getrandom(&mut b).expect("random bytes");
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02X}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

fn random_u64() -> u64 {
    let mut b = [0u8; 8];
    getrandom::getrandom(&mut b).expect("random bytes");
    u64::from_le_bytes(b)
}

/// Playing, paused and so on, as the app reports it. The Apple TV app has
/// been seen to say playing while paused (2026-10-15): a hint, not a fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Playing,
    Paused,
    Stopped,
    Interrupted,
    Seeking,
}

impl State {
    fn from_wire(v: i32) -> Option<Self> {
        Some(match v {
            1 => State::Playing,
            2 => State::Paused,
            3 => State::Stopped,
            4 => State::Interrupted,
            5 => State::Seeking,
            _ => return None,
        })
    }
}

/// What is playing, as far as the app says. Every field may be missing:
/// some apps publish nothing but their state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NowPlaying {
    /// The app's bundle identifier (`com.disney.disneyplus`).
    pub app: Option<String>,
    pub app_name: Option<String>,
    pub state: Option<State>,
    pub title: Option<String>,
    pub series: Option<String>,
    pub season: Option<i32>,
    pub episode: Option<i32>,
    /// Seconds.
    pub duration: Option<f64>,
    /// The position in seconds when it was measured, and when that was.
    pub elapsed: Option<f64>,
    pub elapsed_at: Option<SystemTime>,
    pub rate: Option<f32>,
    pub content_id: Option<String>,
    pub store_id: Option<i64>,
    /// 1 audio, 2 video.
    pub media_type: Option<i32>,
    /// The queue item these fields describe: a new one starts afresh.
    pub item: Option<String>,
}

impl NowPlaying {
    /// The position at `now`, carried on from the last measurement at the
    /// reported rate. The rate is the app's word, which the Apple TV app
    /// has left at 0 while playing: the caller knows better from the
    /// picture whether it moves.
    pub fn position_at(&self, now: SystemTime) -> Option<f64> {
        let elapsed = self.elapsed?;
        let since = self
            .elapsed_at
            .and_then(|t| now.duration_since(t).ok())
            .map_or(0.0, |d| d.as_secs_f64());
        Some(elapsed + since * f64::from(self.rate.unwrap_or(0.0)))
    }

    fn merge(&mut self, m: &proto::ContentItemMetadata) {
        fn set<T: Clone>(to: &mut Option<T>, from: &Option<T>) {
            if from.is_some() {
                to.clone_from(from);
            }
        }
        set(&mut self.title, &m.title);
        set(&mut self.series, &m.series_name);
        set(&mut self.season, &m.season_number);
        set(&mut self.episode, &m.episode_number);
        set(&mut self.duration, &m.duration);
        set(&mut self.elapsed, &m.elapsed_time);
        set(&mut self.rate, &m.playback_rate);
        set(&mut self.content_id, &m.content_identifier);
        set(&mut self.store_id, &m.itunes_store_identifier);
        set(&mut self.media_type, &m.media_type);
        if let Some(t) = m.elapsed_time_timestamp {
            self.elapsed_at = Some(cocoa(t));
        }
    }
}

/// Apple's time: seconds since 2001-01-01 UTC.
pub fn cocoa(secs: f64) -> SystemTime {
    const EPOCH_2001: u64 = 978_307_200;
    UNIX_EPOCH + Duration::from_secs(EPOCH_2001) + Duration::from_secs_f64(secs.max(0.0))
}

/// Each app's state, kept apart: the Apple TV reports for every app that
/// has a player, and says which one is in front.
///
/// Simplified from pyatv's `protocols/mrp/player_state.py` (MIT): one
/// player per app, where pyatv keeps each app's players apart.
#[derive(Debug, Default)]
pub struct Tracker {
    front: Option<String>,
    apps: HashMap<String, (Option<String>, NowPlaying)>,
}

impl Tracker {
    /// What the app in front is playing.
    pub fn now(&self) -> Option<NowPlaying> {
        let front = self.front.as_ref()?;
        let mut np = self
            .apps
            .get(front)
            .map(|(_, np)| np.clone())
            .unwrap_or_default();
        np.app = Some(front.clone());
        Some(np)
    }

    fn app(&mut self, client: Option<&proto::NowPlayingClient>) -> Option<&mut NowPlaying> {
        let client = client?;
        let id = client.bundle_identifier.clone()?;
        let entry = self.apps.entry(id.clone()).or_default();
        entry.1.app = Some(id);
        if client.display_name.is_some() {
            entry.1.app_name.clone_from(&client.display_name);
        }
        Some(&mut entry.1)
    }

    /// Takes one message; true if what is in front may have changed.
    pub fn apply(&mut self, m: &proto::ProtocolMessage) -> bool {
        if let Some(c) = m
            .set_now_playing_client
            .as_ref()
            .and_then(|s| s.client.as_ref())
        {
            self.front.clone_from(&c.bundle_identifier);
            self.app(Some(c));
            return true;
        }
        if let Some(s) = &m.set_state {
            let Some(np) = self.app(s.player_path.as_ref().and_then(|p| p.client.as_ref())) else {
                return false;
            };
            if let Some(st) = s.playback_state {
                np.state = State::from_wire(st);
            }
            if let Some(q) = &s.playback_queue {
                let at = q
                    .location
                    .and_then(|l| usize::try_from(l).ok())
                    .unwrap_or(0);
                if let Some(item) = q.content_items.get(at).or(q.content_items.first()) {
                    // A different item starts afresh; the same one is updated.
                    if item.identifier != np.item {
                        let (app, name, state) = (np.app.take(), np.app_name.take(), np.state);
                        *np = NowPlaying {
                            app,
                            app_name: name,
                            state,
                            ..Default::default()
                        };
                        np.item.clone_from(&item.identifier);
                    }
                    if let Some(meta) = &item.metadata {
                        np.merge(meta);
                    }
                }
            }
            if let Some(info) = &s.now_playing_info {
                // The older summary, for what the queue did not say.
                np.title = np.title.take().or(info.title.clone());
                np.duration = np.duration.or(info.duration);
                if np.elapsed.is_none() {
                    np.elapsed = info.elapsed_time;
                    np.rate = info.playback_rate;
                    np.elapsed_at = info.timestamp.map(cocoa);
                }
            }
            return true;
        }
        if let Some(u) = &m.update_content_item {
            let Some(np) = self.app(u.player_path.as_ref().and_then(|p| p.client.as_ref())) else {
                return false;
            };
            for item in &u.content_items {
                if item.identifier == np.item
                    && let Some(meta) = &item.metadata
                {
                    np.merge(meta);
                }
            }
            return true;
        }
        false
    }
}

/// Reads one Apple TV until the connection ends, calling `update` with what
/// the app in front is playing whenever a message may have changed it.
/// `stop` ends it at the next message or keep-alive.
pub fn watch(
    host: &str,
    creds: &Credentials,
    stop: &AtomicBool,
    mut update: impl FnMut(&NowPlaying),
) -> Result<(), Error> {
    let addr = (host, PORT)
        .to_socket_addrs()?
        .next()
        .ok_or(Error::Protocol("the host has no address"))?;
    let mut control = Conn::new(connect(addr)?);
    let pv = verify(&mut control, creds)?;
    let rtsp = Rtsp::new(&control)?;

    // The event channel: the Apple TV's side talks here, and is answered.
    let resp = rtsp.setup(&mut control, setup_events())?;
    let port = int(&resp, "eventPort").ok_or(Error::Protocol("SETUP gave no event port"))?;
    let mut events = Conn::new(connect(SocketAddr::new(addr.ip(), port))?);
    events.encrypt(Session::new(pv.keys(
        "Events-Salt",
        "Events-Read-Encryption-Key",
        "Events-Write-Encryption-Key",
    )));
    std::thread::Builder::new()
        .name("atv-events".into())
        .spawn(move || answer_events(events))?;

    rtsp.request(&mut control, "RECORD", None)?;

    let seed = random_u64() >> 1;
    let resp = rtsp.setup(&mut control, setup_data(seed))?;
    let port = resp
        .get("streams")
        .and_then(plist::Value::as_array)
        .and_then(|s| s.first())
        .and_then(|s| s.as_dictionary())
        .and_then(|s| s.get("dataPort"))
        .and_then(plist::Value::as_unsigned_integer)
        .and_then(|p| u16::try_from(p).ok())
        .ok_or(Error::Protocol("SETUP gave no data port"))?;
    let data_stream = connect(SocketAddr::new(addr.ip(), port))?;
    data_stream.set_read_timeout(None)?;
    let mut data = Conn::new(data_stream.try_clone()?);
    data.encrypt(Session::new(pv.keys(
        &format!("DataStream-Salt{seed}"),
        "DataStream-Output-Encryption-Key",
        "DataStream-Input-Encryption-Key",
    )));

    // The keep-alive. When it fails, or `stop` is set, the data stream is
    // shut, which ends the read below.
    let alive = Arc::new(AtomicBool::new(true));
    let (alive2, closer) = (Arc::clone(&alive), data_stream);
    let feedback = std::thread::Builder::new()
        .name("atv-feedback".into())
        .spawn(move || {
            while alive2.load(Ordering::Relaxed) {
                std::thread::sleep(FEEDBACK);
                if let Err(e) = rtsp.request(&mut control, "POST /feedback", None) {
                    debug!("atv: feedback: {e}");
                    break;
                }
            }
            let _ = closer.shutdown(std::net::Shutdown::Both);
        })?;

    let result = read_data(&mut data, creds, stop, &alive, &mut update);
    alive.store(false, Ordering::Relaxed);
    let _ = data.stream.shutdown(std::net::Shutdown::Both);
    let _ = feedback.join();
    result
}

fn read_data(
    data: &mut Conn,
    creds: &Credentials,
    stop: &AtomicBool,
    alive: &AtomicBool,
    update: &mut impl FnMut(&NowPlaying),
) -> Result<(), Error> {
    // pyatv's sequence number for what it sends: one random value, kept.
    let seqno = 0x1_0000_0000 + (random_u64() & 0xffff_ffff);
    let client_id = String::from_utf8_lossy(&creds.client_id).into_owned();
    let send =
        |data: &mut Conn, m: &proto::ProtocolMessage| data.send(&Frame::message(seqno, m).encode());
    // Our device information must come first, and be answered, before the
    // Apple TV says anything else.
    send(data, &proto::device_info(&client_id, NAME))?;
    let mut tracker = Tracker::default();
    let mut ready = false;
    let mut last: Option<NowPlaying> = None;
    while alive.load(Ordering::Relaxed) && !stop.load(Ordering::Relaxed) {
        let frame = match data.next(Frame::decode) {
            Ok(f) => f,
            Err(e) if !alive.load(Ordering::Relaxed) || stop.load(Ordering::Relaxed) => {
                debug!("atv: data stream ended: {e}");
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        if frame.is_sync() {
            data.send(&Frame::reply(frame.seqno).encode())?;
        }
        for m in frame.messages()? {
            debug!("atv: message type {:?}", m.r#type);
            if !ready && m.r#type == Some(proto::DEVICE_INFO) {
                ready = true;
                info!("atv: connected");
                send(data, &proto::set_connection_state())?;
                send(data, &proto::client_updates_config())?;
                send(data, &proto::get_keyboard_session())?;
            }
            if tracker.apply(&m) {
                let now = tracker.now();
                if now != last {
                    if let Some(np) = &now {
                        update(np);
                    }
                    last = now;
                }
            }
        }
    }
    Ok(())
}

fn connect(addr: SocketAddr) -> Result<TcpStream, Error> {
    let s = TcpStream::connect_timeout(&addr, TIMEOUT)?;
    s.set_read_timeout(Some(TIMEOUT))?;
    s.set_write_timeout(Some(TIMEOUT))?;
    s.set_nodelay(true)?;
    Ok(s)
}

/// Pair-verify on the control connection, then encrypt it with the
/// control keys.
fn verify(control: &mut Conn, creds: &Credentials) -> Result<PairVerify, Error> {
    let post = |body: Vec<u8>| Message {
        first: "POST /pair-verify HTTP/1.1".into(),
        headers: vec![
            ("User-Agent".into(), "AirPlay/320.20".into()),
            ("Connection".into(), "keep-alive".into()),
            ("X-Apple-HKP".into(), "3".into()),
            ("Content-Type".into(), "application/octet-stream".into()),
        ],
        body,
    };
    let (mut pv, m1) = PairVerify::start()?;
    let m2 = control.exchange(&post(m1))?;
    let m3 = pv.answer(creds, &m2.body)?;
    control.exchange(&post(m3))?;
    control.encrypt(Session::new(pv.keys(
        "Control-Salt",
        "Control-Write-Encryption-Key",
        "Control-Read-Encryption-Key",
    )));
    info!("atv: pair-verify done");
    Ok(pv)
}

/// RTSP on the control connection: the session's identity and a counter.
struct Rtsp {
    uri: String,
    dacp_id: String,
    active_remote: u32,
    cseq: std::cell::Cell<u32>,
}

impl Rtsp {
    fn new(control: &Conn) -> Result<Self, Error> {
        let local = control.stream.local_addr()?.ip();
        Ok(Self {
            uri: format!("rtsp://{local}/{}", random_u64() as u32),
            dacp_id: format!("{:X}", random_u64()),
            active_remote: random_u64() as u32,
            cseq: std::cell::Cell::new(0),
        })
    }

    /// `method` is the method, or the method and a path in place of the
    /// session's URI.
    fn request(
        &self,
        control: &mut Conn,
        method: &str,
        body: Option<plist::Value>,
    ) -> Result<Message, Error> {
        let cseq = self.cseq.get();
        self.cseq.set(cseq + 1);
        let first = match method.split_once(' ') {
            Some((m, path)) => format!("{m} {path} RTSP/1.0"),
            None => format!("{method} {} RTSP/1.0", self.uri),
        };
        let mut headers = vec![
            ("User-Agent".into(), "AirPlay/550.10".into()),
            ("CSeq".into(), cseq.to_string()),
            ("DACP-ID".into(), self.dacp_id.clone()),
            ("Active-Remote".into(), self.active_remote.to_string()),
            ("Client-Instance".into(), self.dacp_id.clone()),
        ];
        let mut bytes = Vec::new();
        if let Some(body) = body {
            headers.push((
                "Content-Type".into(),
                "application/x-apple-binary-plist".into(),
            ));
            body.to_writer_binary(&mut bytes)
                .map_err(|_| Error::Protocol("a plist did not encode"))?;
        }
        control.exchange(&Message {
            first,
            headers,
            body: bytes,
        })
    }

    fn setup(&self, control: &mut Conn, body: plist::Value) -> Result<plist::Dictionary, Error> {
        let resp = self.request(control, "SETUP", Some(body))?;
        plist::Value::from_reader(std::io::Cursor::new(&resp.body))
            .ok()
            .and_then(plist::Value::into_dictionary)
            .ok_or(Error::Protocol("SETUP's answer is not a plist"))
    }
}

fn int(d: &plist::Dictionary, key: &str) -> Option<u16> {
    d.get(key)?
        .as_unsigned_integer()
        .and_then(|p| u16::try_from(p).ok())
}

fn dict<const N: usize>(items: [(&str, plist::Value); N]) -> plist::Value {
    plist::Value::Dictionary(items.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

/// As pyatv describes itself, a phone's remote app, but by our own name
/// and a locally administered address of our own.
fn setup_events() -> plist::Value {
    use plist::Value::{Boolean, String as S};
    dict([
        ("isRemoteControlOnly", Boolean(true)),
        ("osName", S("iPhone OS".into())),
        ("sourceVersion", S("550.10".into())),
        ("timingProtocol", S("None".into())),
        ("model", S("iPhone10,6".into())),
        ("deviceID", S("FF:63:69:6E:65:66".into())),
        ("osVersion", S("14.7.1".into())),
        ("osBuildVersion", S("18G82".into())),
        ("macAddress", S("02:63:69:6E:65:66".into())),
        ("sessionUUID", S(uuid())),
        ("name", S(NAME.into())),
    ])
}

fn setup_data(seed: u64) -> plist::Value {
    use plist::Value::{Boolean, Integer, String as S};
    let stream = dict([
        ("controlType", Integer(2.into())),
        ("channelID", S(uuid())),
        ("seed", Integer(seed.into())),
        ("clientUUID", S(uuid())),
        ("type", Integer(130.into())),
        ("wantsDedicatedSocket", Boolean(true)),
        (
            "clientTypeUUID",
            S("1910A70F-DBC0-4242-AF95-115DB30604E1".into()),
        ),
    ]);
    dict([("streams", plist::Value::Array(vec![stream]))])
}

/// The event channel: every request the Apple TV makes is answered 200,
/// as pyatv does. Ends when the connection does.
fn answer_events(mut events: Conn) {
    let _ = events.stream.set_read_timeout(None);
    loop {
        let req = match events.next(Message::decode) {
            Ok(r) => r,
            Err(e) => {
                debug!("atv: event channel ended: {e}");
                return;
            }
        };
        debug!("atv: event {}", req.first);
        let proto = req.first.rsplit(' ').next().unwrap_or("RTSP/1.0");
        let mut headers = vec![
            ("Content-Length".into(), "0".into()),
            ("Audio-Latency".into(), "0".into()),
        ];
        for h in ["Server", "CSeq"] {
            if let Some(v) = req.header(h) {
                headers.push((h.into(), v.into()));
            }
        }
        let resp = Message {
            first: format!("{proto} 200 OK"),
            headers,
            body: Vec::new(),
        };
        if let Err(e) = events.send(&resp.encode()) {
            warn!("atv: event channel: {e}");
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proto::*;

    fn client(id: &str) -> Option<PlayerPath> {
        Some(PlayerPath {
            client: Some(NowPlayingClient {
                bundle_identifier: Some(id.into()),
                display_name: None,
            }),
        })
    }

    fn item(id: &str, meta: ContentItemMetadata) -> ContentItem {
        ContentItem {
            identifier: Some(id.into()),
            metadata: Some(meta),
        }
    }

    #[test]
    fn the_app_in_front_and_its_item_are_followed() {
        let mut t = Tracker::default();
        // State for an app not yet in front is kept, not shown.
        let set = ProtocolMessage {
            set_state: Some(SetState {
                player_path: client("com.apple.TVWatchList"),
                playback_state: Some(1),
                playback_queue: Some(PlaybackQueue {
                    location: Some(0),
                    content_items: vec![item(
                        "a",
                        ContentItemMetadata {
                            title: Some("Episode".into()),
                            duration: Some(1882.2),
                            elapsed_time: Some(825.3),
                            elapsed_time_timestamp: Some(813_733_151.4),
                            playback_rate: Some(0.0),
                            ..Default::default()
                        },
                    )],
                }),
                ..Default::default()
            }),
            ..message(SET_STATE)
        };
        assert!(t.apply(&set));
        assert_eq!(t.now(), None);
        let front = ProtocolMessage {
            set_now_playing_client: Some(SetNowPlayingClient {
                client: Some(NowPlayingClient {
                    bundle_identifier: Some("com.apple.TVWatchList".into()),
                    display_name: Some("TV".into()),
                }),
            }),
            ..message(SET_NOW_PLAYING_CLIENT)
        };
        assert!(t.apply(&front));
        let np = t.now().unwrap();
        assert_eq!(np.app_name.as_deref(), Some("TV"));
        assert_eq!(np.state, Some(State::Playing));
        assert_eq!(np.title.as_deref(), Some("Episode"));
        // 2026-10-15 04:59:11 UTC, as the Apple TV app sent it.
        let at = np.elapsed_at.unwrap().duration_since(UNIX_EPOCH).unwrap();
        assert_eq!(at.as_secs(), 1_792_040_351);
        // At rate 0 the position stays where it was measured.
        assert_eq!(np.position_at(SystemTime::now()), Some(825.3));

        // An update to the same item changes only what it carries.
        let update = ProtocolMessage {
            update_content_item: Some(UpdateContentItem {
                content_items: vec![item(
                    "a",
                    ContentItemMetadata {
                        elapsed_time: Some(895.8),
                        playback_rate: Some(1.0),
                        ..Default::default()
                    },
                )],
                player_path: client("com.apple.TVWatchList"),
            }),
            ..message(UPDATE_CONTENT_ITEM)
        };
        t.apply(&update);
        let np = t.now().unwrap();
        assert_eq!((np.elapsed, np.rate), (Some(895.8), Some(1.0)));
        assert_eq!(np.title.as_deref(), Some("Episode"));
        let later = np.elapsed_at.unwrap() + Duration::from_secs(10);
        assert_eq!(np.position_at(later), Some(905.8));

        // A new item starts afresh: nothing of the old one carries over.
        let next = ProtocolMessage {
            set_state: Some(SetState {
                player_path: client("com.apple.TVWatchList"),
                playback_queue: Some(PlaybackQueue {
                    location: Some(0),
                    content_items: vec![item(
                        "b",
                        ContentItemMetadata {
                            title: Some("Next".into()),
                            ..Default::default()
                        },
                    )],
                }),
                ..Default::default()
            }),
            ..message(SET_STATE)
        };
        t.apply(&next);
        let np = t.now().unwrap();
        assert_eq!(np.title.as_deref(), Some("Next"));
        assert_eq!((np.elapsed, np.duration), (None, None));
        assert_eq!(np.app_name.as_deref(), Some("TV"));
    }
}
