//! Settings: one TOML file, overridden from the command line.
//!
//! ```toml
//! [output]
//! width = 4096
//!
//! [input]
//! rgb_range = "limited"
//! ```
//!
//! A setting's key is `section.name` (`output.width`), which is also what
//! `--set` takes. Every setting is a row in one table ([`settings`]), with its
//! range, its help and what applying a change costs; anything that lists or
//! edits settings (`cinefit settings`, a menu later) works from that table, and
//! nothing outside this file needs to know the keys.
//!
//! Only things the owner sets once belong here. Switches for one experiment
//! (`--dry-run`, `--change-max-cll`, `--edid`) stay flags on purpose: in a
//! file they could be left on and forgotten.
//!
//! The file is `~/.config/cinefit-rs/cinefit.toml`, apart from the C++
//! version's `~/.config/cinefit/cinefit.toml` until this one replaces it.
//!
//! Pure: no hardware types, so it is tested anywhere.

use std::fmt;
use std::path::{Path, PathBuf};

use cinefit_render::{Curve, Levels};

/// Every setting's value.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// The panel: the output's size. The rate is not a setting: it follows
    /// the source.
    pub width: u16,
    pub height: u16,
    /// What plain RGB input's codes mean. The card does not report it, and
    /// HDMI's default for video formats is limited (#23). YCbCr is always
    /// limited, and a Dolby Vision tunnel states its own range.
    pub rgb_range: Levels,
    /// What the output sends: SDR in a BT.2020 container, tone-mapped where
    /// the source is brighter, or HDR10 passed through as it came. Dolby
    /// Vision is never passed through.
    pub mode: OutputMode,
    /// The brightness, in nits, that the SDR output's white stands for: what
    /// HDR is mapped down to, and the peak a player's low-latency Dolby
    /// Vision is taken to be mapped to, so that it passes one to one. One
    /// setting for both: two that must be equal could drift apart.
    pub white_nits: u16,
    /// The output's rate, in hertz, while there is no source to follow:
    /// the rate the sources start in saves a resync when one comes on.
    pub wait_hz: f64,
    /// Whether YCbCr input labelled SDR BT.2020 is a player's low-latency
    /// Dolby Vision: PQ already mapped by the player to `white_nits`.
    pub lldv: bool,
    /// The capture device by persistent ID (hex); empty for the first that
    /// can capture (#68).
    pub input_device: String,
    /// The tone curve for HDR into the SDR output.
    pub curve: Curve,
    /// The HDR10 source's peak for the curve, in nits, in place of its
    /// MaxCLL; 0 uses the metadata. A disc can declare far more than it
    /// holds: the test disc's 10,000-nit title says 10,000, its scenes peak
    /// at 2,700-4,000.
    pub source_peak: u16,
    /// Find the black bars and crop them out, placing the picture at
    /// constant image height; off, every frame fills the raster as it came.
    pub crop: bool,
    /// Record the detector's readings and a small fingerprint of every frame
    /// shown, for tuning the detector (#91).
    pub record: bool,
    /// The disk the dataset's kept full frames may use, every session
    /// together, in GB; 0 keeps none (#91).
    pub frames_gb: u16,
    /// Pairing passes (#109): keep each shot's first frame and every this
    /// many after it, and nothing else; 0 for the usual choice.
    pub pair_every: u32,
    /// The screen's aspect: the band of the raster that lands on it.
    pub screen_ar: Ratio,
    /// Content wider than this is trimmed at the sides to fill the screen's
    /// height rather than letterboxed inside it; 0 is off.
    pub fill_max_ar: Ratio,
    /// The theater's name, on the waiting slate; empty for none.
    pub venue: String,
    /// The Denon receiver: a hostname or an IP address; empty for none
    /// (#116).
    pub denon: String,
    /// The Apple TV: a hostname or an IP address; empty for none (#115).
    pub atv_host: String,
    /// The receiver input the Apple TV is on, by its name or code; empty
    /// for any.
    pub atv_input: String,
    /// A banner on each change of picture type.
    pub banners: bool,
    /// The web page's port on the home network; 0 for none.
    pub web_port: u16,
}

/// An aspect ratio as it was written: `"16:9"` keeps its two sides, so it
/// is exact and is written back the same way; a plain number is `n:1`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ratio {
    pub w: f64,
    pub h: f64,
}

impl Ratio {
    pub const fn of(n: f64) -> Self {
        Self { w: n, h: 1.0 }
    }

    pub fn value(self) -> f64 {
        self.w / self.h
    }

    /// `"16:9"` or `"1.85"`, `"2.39:1"`: anything else is `None`.
    pub fn parse(text: &str) -> Option<Self> {
        let num = |t: &str| t.trim().parse::<f64>().ok().filter(|v| v.is_finite());
        let r = match text.split_once(':') {
            Some((w, h)) => Self {
                w: num(w)?,
                h: num(h)?,
            },
            None => Self::of(num(text)?),
        };
        (r.h > 0.0 && r.w >= 0.0).then_some(r)
    }
}

impl fmt::Display for Ratio {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.h == 1.0 {
            write!(f, "{}", self.w)
        } else {
            write!(f, "{}:{}", self.w, self.h)
        }
    }
}

/// What the output sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    Sdr2020,
    Passthrough,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            width: 4096,
            height: 2160,
            rgb_range: Levels::Limited,
            // HDR10 to the display's own mapping: on 2026-10-13 the JVC's
            // Frame Adapt was judged as good as libplacebo's curves or
            // better. Dolby Vision still goes out as SDR BT.2020.
            mode: OutputMode::Passthrough,
            // The 96-nit low-latency Dolby Vision target the splitter's EDID
            // declares (2026-10-13), so that input maps one to one.
            white_nits: 96,
            wait_hz: 23.976,
            input_device: String::new(),
            lldv: true,
            // libplacebo's own default. Hable ranked closest to Dolby in the
            // C++ version's step zero, but on a stand-in source, and blew out
            // snow on the screen (2026-10-13); no curve has been chosen on
            // evidence yet.
            curve: Curve::Spline,
            source_peak: 0,
            crop: true,
            record: true,
            frames_gb: 20,
            pair_every: 0,
            screen_ar: Ratio::of(2.35),
            fill_max_ar: Ratio::of(2.40),
            venue: String::new(),
            denon: String::new(),
            atv_host: String::new(),
            atv_input: String::new(),
            banners: true,
            web_port: 8080,
        }
    }
}

/// What it costs to apply a change, inside the running process: a UI has to
/// show it, because a modeset blanks the projector for 2-6 s, and it is the
/// contract the code keeps: a `Live` setting that causes a commit is a bug.
/// cinefit never restarts itself for a setting: it cannot know how it is
/// run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Apply {
    /// The next frame uses it; no commit.
    Live,
    /// An atomic modeset in the same run: the projector resyncs.
    Modeset,
    /// Read when cinefit starts. Only the panel's size, which would mean
    /// rebuilding every output buffer: set once, at installation.
    Start,
}

impl fmt::Display for Apply {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Apply::Live => "live",
            Apply::Modeset => "modeset",
            Apply::Start => "start",
        })
    }
}

/// What values a setting takes.
#[derive(Debug, Clone, Copy)]
pub enum Kind {
    /// A whole number in `min..=max`.
    Int { min: i64, max: i64 },
    /// One of these names.
    Choice(&'static [&'static str]),
    /// A number in `min..=max`, with a fraction: a ratio.
    Float { min: f64, max: f64 },
    /// Free text, up to this many characters.
    Text { max: usize },
    /// An aspect ratio, `W:H` or a number, in `min..=max`; with `off`, 0
    /// too, meaning off.
    Ratio { min: f64, max: f64, off: bool },
}

/// One row of the table.
pub struct Setting {
    pub key: &'static str,
    pub kind: Kind,
    pub apply: Apply,
    pub help: &'static str,
    get: fn(&Config) -> Value,
    set: Set,
}

/// A value as a setting's kind has checked it.
enum Checked<'a> {
    Int(i64),
    Float(f64),
    Name(&'a str),
    Ratio(Ratio),
}

/// A setting's setter, taking the value as its kind has already checked
/// it: a whole number, a number, a name or a ratio. There is no loose
/// conversion that could turn a value of the wrong type into 0 or "" (#100).
#[derive(Clone, Copy)]
enum Set {
    Int(fn(&mut Config, i64)),
    Float(fn(&mut Config, f64)),
    Name(fn(&mut Config, &str)),
    Ratio(fn(&mut Config, Ratio)),
}

/// A setting's value, as the file and the command line hold it.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i64),
    Name(String),
    Float(f64),
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Int(v) => write!(f, "{v}"),
            Value::Name(n) => f.write_str(n),
            Value::Float(v) => write!(f, "{v}"),
        }
    }
}

/// A ratio's value as the file writes it: a number, or `"W:H"` as given.
fn ratio_value(r: Ratio) -> Value {
    if r.h == 1.0 {
        Value::Float(r.w)
    } else {
        Value::Name(r.to_string())
    }
}

static SETTINGS: &[Setting] = &[
    Setting {
        key: "output.width",
        kind: Kind::Int {
            min: 640,
            max: 8192,
        },
        apply: Apply::Start,
        help: "The panel's width in pixels: the output's size. 4096 for a 17:9 4K projector.",
        get: |c| Value::Int(c.width.into()),
        set: Set::Int(|c, v| c.width = v as u16),
    },
    Setting {
        key: "output.height",
        kind: Kind::Int {
            min: 480,
            max: 4320,
        },
        apply: Apply::Start,
        help: "The panel's height in pixels.",
        get: |c| Value::Int(c.height.into()),
        set: Set::Int(|c, v| c.height = v as u16),
    },
    Setting {
        key: "output.mode",
        kind: Kind::Choice(&["sdr2020", "passthrough"]),
        apply: Apply::Modeset,
        help: "sdr2020: SDR in a BT.2020 container, HDR tone-mapped down to output.white_nits; set the display \
               to an SDR BT.2020 mode. passthrough: HDR10 goes out as HDR10, for the display to map. Dolby \
               Vision always goes out as SDR BT.2020.",
        get: |c| {
            Value::Name(
                match c.mode {
                    OutputMode::Sdr2020 => "sdr2020",
                    OutputMode::Passthrough => "passthrough",
                }
                .into(),
            )
        },
        set: Set::Name(|c, v| {
            c.mode = if v == "passthrough" {
                OutputMode::Passthrough
            } else {
                OutputMode::Sdr2020
            }
        }),
    },
    Setting {
        key: "output.white_nits",
        kind: Kind::Int { min: 48, max: 1000 },
        apply: Apply::Live,
        help: "The brightness, in nits, that SDR output's white stands for: HDR above it is tone-mapped. \
               Also the peak a player's low-latency Dolby Vision is mapped to, which passes one to one: set \
               it to what the EDID upstream declares (96 for the usual low-latency Dolby Vision EDIDs).",
        get: |c| Value::Int(c.white_nits.into()),
        set: Set::Int(|c, v| c.white_nits = v as u16),
    },
    Setting {
        key: "output.wait_hz",
        kind: Kind::Float {
            min: 23.0,
            max: 61.0,
        },
        apply: Apply::Modeset,
        help: "The output's rate while no source is playing, with black on screen. A source that starts at the same rate (and SDR) comes up without a resync: 23.976 for an Apple TV set to 23.976, 59.94 for a player whose menus are 59.94.",
        get: |c| Value::Float(c.wait_hz),
        set: Set::Float(|c, v| c.wait_hz = v),
    },
    Setting {
        key: "input.rgb_range",
        kind: Kind::Choice(&["limited", "full"]),
        apply: Apply::Live,
        help: "What plain RGB input's codes mean: limited (16-235, HDMI's default for video) or full (0-255). \
               The capture card does not report it. YCbCr and Dolby Vision are not affected.",
        get: |c| {
            Value::Name(
                match c.rgb_range {
                    Levels::Limited => "limited",
                    Levels::Full => "full",
                }
                .into(),
            )
        },
        set: Set::Name(|c, v| {
            c.rgb_range = if v == "full" {
                Levels::Full
            } else {
                Levels::Limited
            }
        }),
    },
    Setting {
        key: "input.lldv",
        kind: Kind::Choice(&["on", "off"]),
        apply: Apply::Live,
        help: "on: YCbCr input labelled SDR BT.2020 is a player's low-latency Dolby Vision, PQ already mapped \
               to output.white_nits. The capture card cannot tell the two apart, and SDR discs are BT.709.",
        get: |c| Value::Name(if c.lldv { "on" } else { "off" }.into()),
        set: Set::Name(|c, v| c.lldv = v == "on"),
    },
    Setting {
        key: "input.device",
        kind: Kind::Text { max: 20 },
        apply: Apply::Start,
        help: "The capture device, by the persistent ID `cinefit probe` prints (for example 0x1620413): on a \
               card with several inputs, which one. The SDK's list order can change across reboots and driver \
               updates, so not an index. An ID that is not present stops the run with the list. Empty: the \
               first device that can capture.",
        get: |c| Value::Name(c.input_device.clone()),
        set: Set::Name(|c, v| c.input_device = v.trim().to_string()),
    },
    Setting {
        key: "tonemap.curve",
        kind: Kind::Choice(CURVES),
        apply: Apply::Live,
        help: "The tone curve for HDR into SDR output: libplacebo's, by name.",
        get: |c| Value::Name(c.curve.name().into()),
        set: Set::Name(|c, v| c.curve = Curve::from_name(v).unwrap_or(Curve::Spline)),
    },
    Setting {
        key: "tonemap.source_peak",
        kind: Kind::Int { min: 0, max: 10000 },
        apply: Apply::Live,
        help: "The HDR10 source's peak, in nits, that the curve maps from, in place of its MaxCLL or \
               mastering peak; 0 uses the metadata.",
        get: |c| Value::Int(c.source_peak.into()),
        set: Set::Int(|c, v| c.source_peak = v as u16),
    },
    Setting {
        key: "detect.crop",
        kind: Kind::Choice(&["on", "off"]),
        apply: Apply::Live,
        help: "on: find the black bars and crop them out, the picture placed at constant image height in the \
               screen's band of the raster. off: every frame fills the raster as it came.",
        get: |c| Value::Name(if c.crop { "on" } else { "off" }.into()),
        set: Set::Name(|c, v| c.crop = v == "on"),
    },
    Setting {
        key: "detect.record",
        kind: Kind::Choice(&["on", "off"]),
        apply: Apply::Live,
        help: "on: record what the bar detector measured on every frame shown, and a small fingerprint of the \
               picture, under ~/cinefit-clips/caught, for tuning the detector (about 150 MB a film). Full frames \
               only where they teach something, within detect.frames_gb.",
        get: |c| Value::Name(if c.record { "on" } else { "off" }.into()),
        set: Set::Name(|c, v| c.record = v == "on"),
    },
    Setting {
        key: "detect.frames_gb",
        kind: Kind::Int { min: 0, max: 10000 },
        apply: Apply::Live,
        help: "While recording: the disk, in GB, that full frames may use, every session together. Frames are \
               kept where the detector changed the crop or hesitated, and a few at random; the oldest sessions' \
               are deleted first. 0: no full frames, only the readings.",
        get: |c| Value::Int(c.frames_gb.into()),
        set: Set::Int(|c, v| c.frames_gb = v as u16),
    },
    Setting {
        key: "detect.pair_every",
        kind: Kind::Int {
            min: 0,
            max: 100_000,
        },
        apply: Apply::Live,
        help: "For pairing two passes of a title (Dolby Vision tunnel, then low-latency): keep in full the first \
               frame of every shot and every this many frames after it, and no other frames, so both passes keep \
               the same frames of the film. 48 at 24p is one every two seconds; a tunnel frame is about 15 MB \
               kept, so mind detect.frames_gb. 0: off, frames kept as usual.",
        get: |c| Value::Int(c.pair_every.into()),
        set: Set::Int(|c, v| c.pair_every = v as u32),
    },
    Setting {
        key: "geometry.screen_ar",
        kind: Kind::Ratio {
            min: 1.0,
            max: 3.0,
            off: false,
        },
        apply: Apply::Live,
        help: "The screen's aspect ratio: with the lens set so the raster's width fills the screen, the band of \
               the raster that lands on it. 2.35 for a 2.35:1 screen; a ratio may be written \"16:9\".",
        get: |c| ratio_value(c.screen_ar),
        set: Set::Ratio(|c, v| c.screen_ar = v),
    },
    Setting {
        key: "geometry.fill_max_ar",
        kind: Kind::Ratio {
            min: 1.0,
            max: 3.0,
            off: true,
        },
        apply: Apply::Live,
        help: "Content wider than the screen but no wider than this fills the screen's height, losing a \
               sliver at each side (2.39 and 2.40 on a 2.35 screen give up about 1% a side and show no bars). \
               Wider content (2.55, 2.76) keeps its full width with thin bars inside the screen. 0 is off.",
        get: |c| ratio_value(c.fill_max_ar),
        set: Set::Ratio(|c, v| c.fill_max_ar = v),
    },
    Setting {
        key: "slate.venue",
        kind: Kind::Text { max: 40 },
        apply: Apply::Live,
        help: "The theater's name, shown on the waiting slate above what cinefit is waiting for. Empty: no \
               name, only the state.",
        get: |c| Value::Name(c.venue.clone()),
        set: Set::Name(|c, v| c.venue = v.trim().to_string()),
    },
    Setting {
        key: "osd.banners",
        kind: Kind::Choice(&["on", "off"]),
        apply: Apply::Live,
        help: "on: a short banner when the picture type changes (Dolby Vision, HDR10, SDR), saying what came in \
               and what happens to it; after the projector's resync when the change needs one.",
        get: |c| Value::Name(if c.banners { "on" } else { "off" }.into()),
        set: Set::Name(|c, v| c.banners = v == "on"),
    },
    Setting {
        key: "avr.denon",
        kind: Kind::Text { max: 253 },
        apply: Apply::Live,
        help: "The Denon receiver, by hostname or IP address (for example denon.local): cinefit reads which \
               input is on, by its name on the receiver, and whether the receiver is on, for the info panel, the \
               slate and the detector's dataset. Reads only. Empty: none.",
        get: |c| Value::Name(c.denon.clone()),
        set: Set::Name(|c, v| c.denon = v.trim().to_string()),
    },
    Setting {
        key: "atv.host",
        kind: Kind::Text { max: 253 },
        apply: Apply::Live,
        help: "The Apple TV, by hostname or IP address: cinefit reads what it is playing (the title, the app, \
               where in it), for the info panel and the detector's dataset. Reads only. It needs pairing once; \
               the pairing is kept in atv-credentials beside this file. Some apps say nothing of what they \
               play. Empty: none.",
        get: |c| Value::Name(c.atv_host.clone()),
        set: Set::Name(|c, v| c.atv_host = v.trim().to_string()),
    },
    Setting {
        key: "atv.input",
        kind: Kind::Text { max: 64 },
        apply: Apply::Live,
        help: "The Denon input the Apple TV is plugged into, by its name on the receiver or its code (for \
               example Apple TV, or MPLAY): what the Apple TV plays is shown, and labels the detector's \
               dataset, only while the receiver is on it, so a paused show does not linger over a disc. \
               Empty: always, as without a receiver.",
        get: |c| Value::Name(c.atv_input.clone()),
        set: Set::Name(|c, v| c.atv_input = v.trim().to_string()),
    },
    Setting {
        key: "web.port",
        kind: Kind::Int { min: 0, max: 65535 },
        apply: Apply::Start,
        help: "The port of the web page with cinefit's status and settings, open to the home network: \
               http://<this machine>:8080. 0: no web page. Above 1023, as it runs without root.",
        get: |c| Value::Int(c.web_port.into()),
        set: Set::Int(|c, v| c.web_port = v as u16),
    },
];

const CURVES: &[&str] = &[
    "spline",
    "hable",
    "bt2390",
    "bt2446a",
    "st2094-10",
    "st2094-40",
    "mobius",
    "reinhard",
    "clip",
];

/// The table, in a stable order.
pub fn settings() -> &'static [Setting] {
    SETTINGS
}

/// Settings that were removed, and what took their place: a file still
/// holding one is told what to do, not just that the key is unknown.
const RETIRED: &[(&str, &str)] = &[(
    "input.lldv_nits",
    "merged into output.white_nits (2026-10-14), which low-latency Dolby Vision now passes through at; \
     remove the line, and set output.white_nits if it is not 96",
)];

pub fn find(key: &str) -> Option<&'static Setting> {
    SETTINGS.iter().find(|s| s.key == key)
}

impl Setting {
    pub fn get(&self, c: &Config) -> Value {
        (self.get)(c)
    }

    /// The range or choices, for listing.
    pub fn range(&self) -> String {
        match self.kind {
            Kind::Int { min, max } => format!("{min}-{max}"),
            Kind::Choice(names) => names.join(" | "),
            Kind::Float { min, max } => format!("{min}-{max}"),
            Kind::Text { max } => format!("text, up to {max} characters"),
            Kind::Ratio { min, max, off } => {
                format!("{}{min}-{max}, or W:H", if off { "0 (off), " } else { "" })
            }
        }
    }

    /// Checks `v` and sets it.
    pub fn put(&self, c: &mut Config, v: &Value) -> Result<(), Error> {
        let bad = || Error::Value {
            key: self.key.to_string(),
            value: v.to_string(),
            range: self.range(),
        };
        // The value as its kind takes it, once checked; `None` if it is not.
        let checked = match (self.kind, v) {
            (Kind::Int { min, max }, Value::Int(n)) if (min..=max).contains(n) => {
                Some(Checked::Int(*n))
            }
            (Kind::Choice(names), Value::Name(n)) if names.contains(&n.as_str()) => {
                Some(Checked::Name(n))
            }
            (Kind::Float { min, max }, Value::Float(x)) if (min..=max).contains(x) => {
                Some(Checked::Float(*x))
            }
            // A whole number where a number goes: 2 for 2.0.
            (Kind::Float { min, max }, Value::Int(n)) if (min..=max).contains(&(*n as f64)) => {
                Some(Checked::Float(*n as f64))
            }
            // Control characters would draw as nothing, or break the file.
            (Kind::Text { max }, Value::Name(t))
                if t.chars().count() <= max && !t.chars().any(char::is_control) =>
            {
                Some(Checked::Name(t))
            }
            (Kind::Ratio { min, max, off }, v) => match v {
                Value::Name(t) => Ratio::parse(t),
                Value::Float(x) => Some(Ratio::of(*x)),
                Value::Int(n) => Some(Ratio::of(*n as f64)),
            }
            .filter(|r| (min..=max).contains(&r.value()) || (off && r.value() == 0.0))
            .map(Checked::Ratio),
            _ => None,
        };
        // A setter of another type than its kind would be a mistake in the
        // table: refused like a bad value, never defaulted (and a test puts
        // every setting's own value through, so it cannot ship).
        match (self.set, checked.ok_or_else(bad)?) {
            (Set::Int(f), Checked::Int(n)) => f(c, n),
            (Set::Float(f), Checked::Float(x)) => f(c, x),
            (Set::Name(f), Checked::Name(n)) => f(c, n),
            (Set::Ratio(f), Checked::Ratio(r)) => f(c, r),
            _ => return Err(bad()),
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum Error {
    Read(PathBuf, std::io::Error),
    Parse(PathBuf, String),
    UnknownKey(String),
    Value {
        key: String,
        value: String,
        range: String,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Read(p, e) => write!(f, "{}: {e}", p.display()),
            Error::Parse(p, e) => write!(f, "{}: {e}", p.display()),
            Error::UnknownKey(k) => match RETIRED.iter().find(|(old, _)| old == k) {
                Some((_, now)) => write!(f, "{k} is no longer a setting: {now}"),
                None => write!(f, "no setting {k} (cinefit settings lists them)"),
            },
            Error::Value { key, value, range } => write!(f, "{key} = {value}: must be {range}"),
        }
    }
}

impl std::error::Error for Error {}

/// The settings file's usual place.
pub fn default_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("cinefit-rs").join("cinefit.toml")
}

/// Reads settings from TOML text over the defaults. Unknown keys are errors:
/// a misspelt key silently ignored is a setting that never applies.
pub fn parse(text: &str, path: &Path) -> Result<Config, Error> {
    let table: toml::Table = text
        .parse()
        .map_err(|e: toml::de::Error| Error::Parse(path.to_owned(), e.to_string()))?;
    let mut c = Config::default();
    for (section, body) in &table {
        let Some(body) = body.as_table() else {
            return Err(Error::UnknownKey(section.clone()));
        };
        for (k, v) in body {
            let key = format!("{section}.{k}");
            let s = find(&key).ok_or_else(|| Error::UnknownKey(key.clone()))?;
            let value = match v {
                toml::Value::Integer(n) => Value::Int(*n),
                toml::Value::String(n) => Value::Name(n.clone()),
                toml::Value::Float(x) => Value::Float(*x),
                // A bool or table fits no setting: refused by put.
                other => Value::Name(format!("a {}", other.type_str())),
            };
            s.put(&mut c, &value)?;
        }
    }
    Ok(c)
}

/// Reads the settings file, or the defaults if there is none.
pub fn load(path: &Path) -> Result<Config, Error> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text, path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(Error::Read(path.to_owned(), e)),
    }
}

/// Applies one `key=value` from the command line.
pub fn set(c: &mut Config, assignment: &str) -> Result<(), Error> {
    let (key, text) = assignment
        .split_once('=')
        .ok_or_else(|| Error::UnknownKey(assignment.to_string()))?;
    let s = find(key.trim()).ok_or_else(|| Error::UnknownKey(key.trim().to_string()))?;
    let value = s.value_of(text)?;
    s.put(c, &value)
}

impl Setting {
    /// A value as typed (on the command line, on the web page), read as
    /// this setting's kind. Not checked against the range: `put` does that.
    pub fn value_of(&self, text: &str) -> Result<Value, Error> {
        let s = self;
        let text = text.trim();
        Ok(match s.kind {
            Kind::Int { .. } => Value::Int(text.parse().map_err(|_| Error::Value {
                key: s.key.to_string(),
                value: text.to_string(),
                range: s.range(),
            })?),
            Kind::Choice(_) | Kind::Text { .. } | Kind::Ratio { .. } => {
                Value::Name(text.to_string())
            }
            Kind::Float { .. } => Value::Float(text.parse().map_err(|_| Error::Value {
                key: s.key.to_string(),
                value: text.to_string(),
                range: s.range(),
            })?),
        })
    }
}

/// The settings as TOML, every row with its help as a comment: what `save`
/// writes, and what a person reads.
pub fn render(c: &Config) -> String {
    let mut out = String::new();
    let mut section = "";
    for s in SETTINGS {
        let (sec, name) = s.key.split_once('.').expect("keys are section.name");
        if sec != section {
            if !section.is_empty() {
                out.push('\n');
            }
            out.push_str(&format!("[{sec}]\n"));
            section = sec;
        }
        out.push_str(&format!(
            "# {} ({}; applies: {})\n",
            s.help,
            s.range(),
            s.apply
        ));
        match s.get(c) {
            Value::Int(v) => out.push_str(&format!("{name} = {v}\n")),
            Value::Name(n) => out.push_str(&format!("{name} = \"{}\"\n", toml_escape(&n))),
            // Always with a fraction, so it reads back as a float.
            Value::Float(v) => out.push_str(&format!("{name} = {v:?}\n")),
        }
    }
    out
}

/// The settings that differ from the defaults, as TOML, each with its help:
/// what a change made while running saves, so the file holds only what was
/// chosen and stays short enough to read.
pub fn render_changed(c: &Config) -> String {
    let d = Config::default();
    let mut out = String::from(
        "# cinefit's settings that differ from the defaults; `cinefit settings` lists them all.\n",
    );
    let mut section = "";
    for s in SETTINGS.iter().filter(|s| s.get(c) != s.get(&d)) {
        let (sec, name) = s.key.split_once('.').expect("keys are section.name");
        if sec != section {
            out.push_str(&format!("\n[{sec}]\n"));
            section = sec;
        }
        out.push_str(&format!("# {}\n", s.help));
        match s.get(c) {
            Value::Int(v) => out.push_str(&format!("{name} = {v}\n")),
            Value::Name(n) => out.push_str(&format!("{name} = \"{}\"\n", toml_escape(&n))),
            Value::Float(v) => out.push_str(&format!("{name} = {v:?}\n")),
        }
    }
    out
}

/// The user guide's settings page, as Markdown: every row with its help,
/// values, default and cost, grouped and named as the web page shows them,
/// and marked where the on-screen menu offers it too. The guide is generated
/// from the table so that it cannot describe a setting the binary does not
/// have; a test fails when the committed page differs
/// (`CINEFIT_UPDATE_GUIDE=1 cargo test -p cinefit config` rewrites it).
#[cfg(test)]
fn render_markdown() -> String {
    // The detector's own settings, which most people leave alone, go last
    // under a heading of their own rather than among the picture's.
    const TUNING: &[&str] = &["detect.record", "detect.frames_gb", "detect.pair_every"];
    const GROUPS: &[(&str, &str)] = &[
        ("Picture", ""),
        ("Output", ""),
        ("Screen", ""),
        ("Input", ""),
        (
            "Receiver",
            "For the [receiver and Apple TV](/integrations/) integrations.",
        ),
        ("Web page", ""),
        (
            "Detector tuning",
            "For improving cinefit's bar detector; most people leave these alone. Recording \
             is on by default and keeps what the detector measured, a little each film, and some \
             frames within a disk limit; turn it off with the first setting here.",
        ),
    ];
    let d = Config::default();
    let mut out = String::from(
        "---\ntitle: \"Settings\"\ndescription: \"Every cinefit setting: what it does, its \
         values, its default and what changing it costs.\"\n---\n\n\
         <!-- Generated from crates/cinefit/src/config.rs: edit the table there, then run \
         CINEFIT_UPDATE_GUIDE=1 cargo test -p cinefit config -->\n\n\
         # Settings\n\n\
         Every setting, grouped as on the [web page](/web-page/), where all of them can be \
         changed. Those marked **in the menu** are on the remote's [menu](/using/#menu) too. \
         Both save to the settings file, `/etc/cinefit/cinefit.toml`, which holds only what you \
         have changed; `cinefit settings` lists every setting with its current value.\n\n\
         A change costs one of three things:\n\n\
         - **applies now**: the next frame uses it.\n\
         - **resyncs the display**: the display blanks for a few seconds while it locks on.\n\
         - **applies at the next start**: restart the service (`sudo systemctl restart \
         cinefit`).\n",
    );
    let quote = |v: &str| format!("`{v}`");
    for (group, intro) in GROUPS {
        let rows: Vec<&Setting> = SETTINGS
            .iter()
            .filter(|s| {
                let tuning = TUNING.contains(&s.key);
                if *group == "Detector tuning" {
                    tuning
                } else {
                    !tuning && crate::web::place(s.key).0 == *group
                }
            })
            .collect();
        assert!(!rows.is_empty(), "no settings in {group}");
        out.push_str(&format!("\n## {group}\n"));
        if !intro.is_empty() {
            out.push_str(&format!("\n{intro}\n"));
        }
        for s in rows {
            let label = crate::web::place(s.key).1;
            let values = match s.kind {
                Kind::Int { min, max } => format!("{min} to {max}"),
                Kind::Float { min, max } => format!("{min} to {max}"),
                Kind::Choice(names) => match names.split_last() {
                    Some((last, [])) => quote(last),
                    Some((last, rest)) => format!(
                        "{} or {}",
                        rest.iter().map(|n| quote(n)).collect::<Vec<_>>().join(", "),
                        quote(last)
                    ),
                    None => String::new(),
                },
                Kind::Text { max } => format!("text, up to {max} characters"),
                Kind::Ratio { min, max, off } => format!(
                    "{min} to {max}, as a number or W:H{}",
                    if off { ", or 0 for off" } else { "" }
                ),
            };
            let default = match s.get(&d) {
                Value::Name(n) if n.is_empty() => "empty".to_string(),
                v => quote(&v.to_string()),
            };
            let applies = match s.apply {
                Apply::Live => "applies now",
                Apply::Modeset => "resyncs the display",
                Apply::Start => "applies at the next start",
            };
            let menu = if crate::menu::offers(s.key) {
                " \u{b7} **in the menu**"
            } else {
                ""
            };
            // `<` opens a tag in Markdown: help text may say `<this machine>`.
            let help = s.help.replace('<', "&lt;");
            // The key is the anchor, `#output-width`, so links survive a
            // label or a group changing.
            out.push_str(&format!(
                "\n### {label} {{#{}}}\n\n{help}\n\n\
                 `{}` \u{b7} {values} \u{b7} default {default} \u{b7} {applies}{menu}\n",
                s.key.replace(['.', '_'], "-"),
                s.key,
            ));
        }
    }
    out
}

/// Writes [`render_changed`] to `path`, through a file beside it renamed
/// over the old one: a crash mid-write leaves the old file, never half of
/// one that would stop cinefit at its next start.
pub fn save(c: &Config, path: &Path) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("toml.new");
    std::fs::write(&tmp, render_changed(c))?;
    std::fs::rename(&tmp, path)
}

/// `text` inside a TOML basic string: quotes and backslashes escaped.
/// Control characters never get here: settings refuse them.
fn toml_escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p() -> &'static Path {
        Path::new("test.toml")
    }

    #[test]
    fn an_empty_file_is_the_defaults() {
        assert_eq!(parse("", p()).unwrap(), Config::default());
        assert_eq!(Config::default().width, 4096);
        assert_eq!(Config::default().rgb_range, Levels::Limited);
    }

    #[test]
    fn values_are_read_by_section_and_name() {
        let c = parse(
            "[output]\nwidth = 3840\n[input]\nrgb_range = \"full\"\n",
            p(),
        )
        .unwrap();
        assert_eq!((c.width, c.height, c.rgb_range), (3840, 2160, Levels::Full));
    }

    #[test]
    fn a_misspelt_key_or_an_out_of_range_value_is_an_error_not_ignored() {
        assert!(matches!(
            parse("[output]\nwidht = 3840\n", p()),
            Err(Error::UnknownKey(k)) if k == "output.widht"
        ));
        assert!(matches!(
            parse("[output]\nwidth = 99999\n", p()),
            Err(Error::Value { .. })
        ));
        assert!(matches!(
            parse("[input]\nrgb_range = \"tv\"\n", p()),
            Err(Error::Value { .. })
        ));
    }

    #[test]
    fn the_command_line_overrides_one_setting() {
        let mut c = Config::default();
        set(&mut c, "input.rgb_range=full").unwrap();
        set(&mut c, "output.height = 1080").unwrap();
        assert_eq!((c.rgb_range, c.height), (Levels::Full, 1080));
        assert!(set(&mut c, "output.height=tall").is_err());
        assert!(set(&mut c, "nonsense").is_err());
    }

    #[test]
    fn ratios_are_floats_with_a_range() {
        let mut c = Config::default();
        set(&mut c, "geometry.screen_ar=2.4").unwrap();
        set(&mut c, "geometry.fill_max_ar = 2").unwrap();
        assert_eq!((c.screen_ar.value(), c.fill_max_ar.value()), (2.4, 2.0));
        assert!(set(&mut c, "geometry.screen_ar=0.5").is_err());
        assert!(set(&mut c, "geometry.screen_ar=wide").is_err());
        let text = render(&c);
        assert!(
            text.contains("screen_ar = 2.4\n") && text.contains("fill_max_ar = 2.0\n"),
            "{text}"
        );
        assert_eq!(parse(&text, p()).unwrap(), c);
        assert_eq!(
            parse("[geometry]\nscreen_ar = 2\n", p())
                .unwrap()
                .screen_ar
                .value(),
            2.0
        );
    }

    #[test]
    fn a_ratio_is_kept_as_written() {
        let c = parse("[geometry]\nscreen_ar = \"16:9\"\n", p()).unwrap();
        assert_eq!(c.screen_ar, Ratio { w: 16.0, h: 9.0 });
        assert_eq!(c.screen_ar.value(), 16.0 / 9.0);
        // Written back as given, and read back the same.
        let text = render(&c);
        assert!(text.contains("screen_ar = \"16:9\"\n"), "{text}");
        assert_eq!(parse(&text, p()).unwrap(), c);
        let mut c = Config::default();
        set(&mut c, "geometry.screen_ar=2.39:1").unwrap();
        assert_eq!(c.screen_ar.value(), 2.39);
        // The fill limit can be off; the screen ratio cannot.
        set(&mut c, "geometry.fill_max_ar=0").unwrap();
        assert_eq!(c.fill_max_ar.value(), 0.0);
        assert!(set(&mut c, "geometry.screen_ar=0").is_err());
        assert!(set(&mut c, "geometry.screen_ar=16:0").is_err());
        assert!(set(&mut c, "geometry.screen_ar=4:9").is_err());
    }

    #[test]
    fn what_is_saved_is_only_what_differs_and_reads_back() {
        assert!(!render_changed(&Config::default()).contains(" = "));
        let mut c = Config::default();
        set(&mut c, "slate.venue=The Back Room").unwrap();
        set(&mut c, "geometry.screen_ar=16:9").unwrap();
        let text = render_changed(&c);
        assert_eq!(text.matches(" = ").count(), 2, "{text}");
        assert_eq!(parse(&text, p()).unwrap(), c);
        let dir = std::env::temp_dir().join(format!("cinefit-save-{}", std::process::id()));
        let path = dir.join("cinefit.toml");
        save(&c, &path).unwrap();
        assert_eq!(load(&path).unwrap(), c);
        assert!(!path.with_extension("toml.new").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn what_is_written_reads_back_the_same() {
        let mut c = Config::default();
        set(&mut c, "output.width=3840").unwrap();
        set(&mut c, "input.rgb_range=full").unwrap();
        let text = render(&c);
        assert!(text.contains("# The panel's width"));
        assert_eq!(parse(&text, p()).unwrap(), c);
    }

    #[test]
    fn the_venue_is_text_that_survives_the_file() {
        let mut c = Config::default();
        assert_eq!(c.venue, "");
        set(&mut c, r#"slate.venue = The "Back" Room \ 2"#).unwrap();
        assert_eq!(c.venue, r#"The "Back" Room \ 2"#);
        let text = render(&c);
        assert_eq!(parse(&text, p()).unwrap(), c);
        assert!(set(&mut c, &format!("slate.venue={}", "x".repeat(41))).is_err());
        assert!(parse("[slate]\nvenue = \"a\\nb\"\n", p()).is_err());
        assert!(parse("[slate]\nvenue = 3\n", p()).is_err());
    }

    #[test]
    fn a_retired_setting_says_what_replaced_it() {
        let e = parse("[input]\nlldv_nits = 96\n", p()).unwrap_err();
        assert!(
            e.to_string().contains("merged into output.white_nits"),
            "{e}"
        );
        assert!(set(&mut Config::default(), "input.lldv_nits=96").is_err());
    }

    #[test]
    fn each_section_is_one_run_of_rows() {
        // The file is written in table order, a header per change of section;
        // a section that comes back is a duplicate key in TOML.
        let mut done = std::collections::BTreeSet::new();
        let mut current = "";
        for s in settings() {
            let section = s.key.split('.').next().unwrap_or("");
            if section != current {
                assert!(done.insert(section), "{section} rows are split");
                current = section;
            }
        }
    }

    /// Every setter takes the type its kind checks (#100): each setting's
    /// own value, and a changed one, go through `put` and read back. A
    /// setter of the wrong type would be refused here instead of writing 0.
    #[test]
    fn every_setting_takes_its_own_value_and_reads_back() {
        let mut c = Config::default();
        for s in settings() {
            let own = (s.get)(&c);
            s.put(&mut c, &own)
                .unwrap_or_else(|e| panic!("{}: its own value {own:?} refused: {e}", s.key));
            assert_eq!((s.get)(&c), own, "{}", s.key);
            // Another value of the kind, where there is one.
            let other = match s.kind {
                Kind::Int { min, .. } => Some(Value::Int(min)),
                Kind::Float { max, .. } => Some(Value::Float(max)),
                Kind::Choice(names) => names.last().map(|n| Value::Name((*n).into())),
                Kind::Text { .. } => Some(Value::Name("x".into())),
                Kind::Ratio { min, .. } => Some(Value::Float(min)),
            };
            if let Some(v) = other {
                s.put(&mut c, &v)
                    .unwrap_or_else(|e| panic!("{}: {v:?} refused: {e}", s.key));
                assert_eq!((s.get)(&c), v, "{}", s.key);
            }
        }
    }

    /// A value of the wrong type is an error, not a zero or an empty name.
    #[test]
    fn a_value_of_the_wrong_type_is_refused() {
        let mut c = Config::default();
        let find = |k: &str| settings().iter().find(|s| s.key == k).unwrap();
        assert!(
            find("output.width")
                .put(&mut c, &Value::Name("wide".into()))
                .is_err()
        );
        assert!(find("slate.venue").put(&mut c, &Value::Int(3)).is_err());
        assert!(
            find("output.wait_hz")
                .put(&mut c, &Value::Name("24".into()))
                .is_err()
        );
        assert_eq!(c, Config::default(), "nothing changed");
    }

    #[test]
    fn every_key_is_section_dot_name_and_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for s in settings() {
            assert_eq!(s.key.split('.').count(), 2, "{}", s.key);
            assert!(seen.insert(s.key), "{} twice", s.key);
            assert!(!s.help.is_empty());
        }
    }

    /// The guide's settings page is the table's: rewritten with
    /// `CINEFIT_UPDATE_GUIDE=1`, and a failure here when a row changed
    /// without it.
    #[test]
    fn the_guide_lists_the_settings_as_the_table_has_them() {
        let page = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/guide/docs/settings.md");
        let want = render_markdown();
        if std::env::var_os("CINEFIT_UPDATE_GUIDE").is_some() {
            std::fs::write(&page, &want).unwrap();
        }
        let have = std::fs::read_to_string(&page).unwrap_or_default();
        assert!(
            have == want,
            "{} is out of date: run CINEFIT_UPDATE_GUIDE=1 cargo test -p cinefit config",
            page.display()
        );
    }
}
