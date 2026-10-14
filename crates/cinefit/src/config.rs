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
#[derive(Debug, Clone, Copy, PartialEq)]
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
    /// HDR is mapped down to.
    pub white_nits: u16,
    /// The output's rate, in hertz, while there is no source to follow:
    /// the rate the sources start in saves a resync when one comes on.
    pub wait_hz: f64,
    /// Whether YCbCr input labelled SDR BT.2020 is a player's low-latency
    /// Dolby Vision: PQ already mapped by the player to `lldv_nits`.
    pub lldv: bool,
    pub lldv_nits: u16,
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
    /// The screen's aspect: the band of the raster that lands on it.
    pub screen_ar: f64,
    /// Content wider than this is trimmed at the sides to fill the screen's
    /// height rather than letterboxed inside it.
    pub fill_max_ar: f64,
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
            lldv: true,
            lldv_nits: 96,
            // libplacebo's own default. Hable ranked closest to Dolby in the
            // C++ version's step zero, but on a stand-in source, and blew out
            // snow on the screen (2026-10-13); no curve has been chosen on
            // evidence yet.
            curve: Curve::Spline,
            source_peak: 0,
            crop: true,
            screen_ar: 2.35,
            fill_max_ar: 2.40,
        }
    }
}

/// What it costs to apply a change: a UI has to show it, because a modeset
/// blanks the projector for 2-6 s, and it is the contract the code keeps: a
/// `Live` setting that causes a commit is a bug.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Apply {
    /// Renderer state only; no commit.
    Live,
    /// An atomic modeset: the projector resyncs.
    #[expect(
        dead_code,
        reason = "no setting costs a modeset yet; the cost belongs in the vocabulary"
    )]
    Modeset,
    /// Read only at startup.
    Restart,
}

impl fmt::Display for Apply {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Apply::Live => "live",
            Apply::Modeset => "modeset",
            Apply::Restart => "restart",
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
}

/// One row of the table.
pub struct Setting {
    pub key: &'static str,
    pub kind: Kind,
    pub apply: Apply,
    pub help: &'static str,
    get: fn(&Config) -> Value,
    set: fn(&mut Config, &Value),
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

fn int(v: &Value) -> i64 {
    match v {
        Value::Int(v) => *v,
        Value::Float(v) => *v as i64,
        Value::Name(_) => 0,
    }
}

fn name(v: &Value) -> &str {
    match v {
        Value::Name(n) => n,
        Value::Int(_) | Value::Float(_) => "",
    }
}

fn float(v: &Value) -> f64 {
    match v {
        Value::Float(v) => *v,
        Value::Int(v) => *v as f64,
        Value::Name(_) => 0.0,
    }
}

static SETTINGS: &[Setting] = &[
    Setting {
        key: "output.width",
        kind: Kind::Int {
            min: 640,
            max: 8192,
        },
        apply: Apply::Restart,
        help: "The panel's width in pixels: the output's size. 4096 for a 17:9 4K projector.",
        get: |c| Value::Int(c.width.into()),
        set: |c, v| c.width = int(v) as u16,
    },
    Setting {
        key: "output.height",
        kind: Kind::Int {
            min: 480,
            max: 4320,
        },
        apply: Apply::Restart,
        help: "The panel's height in pixels.",
        get: |c| Value::Int(c.height.into()),
        set: |c, v| c.height = int(v) as u16,
    },
    Setting {
        key: "output.mode",
        kind: Kind::Choice(&["sdr2020", "passthrough"]),
        apply: Apply::Restart,
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
        set: |c, v| {
            c.mode = if name(v) == "passthrough" {
                OutputMode::Passthrough
            } else {
                OutputMode::Sdr2020
            }
        },
    },
    Setting {
        key: "output.white_nits",
        kind: Kind::Int { min: 48, max: 1000 },
        apply: Apply::Restart,
        help: "The brightness, in nits, that SDR output's white stands for: HDR above it is tone-mapped. \
               Match input.lldv_nits so a player's low-latency Dolby Vision passes one to one.",
        get: |c| Value::Int(c.white_nits.into()),
        set: |c, v| c.white_nits = int(v) as u16,
    },
    Setting {
        key: "output.wait_hz",
        kind: Kind::Float {
            min: 23.0,
            max: 61.0,
        },
        apply: Apply::Restart,
        help: "The output's rate while no source is playing, with black on screen. A source that starts at the same rate (and SDR) comes up without a resync: 23.976 for an Apple TV set to 23.976, 59.94 for a player whose menus are 59.94.",
        get: |c| Value::Float(c.wait_hz),
        set: |c, v| c.wait_hz = float(v),
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
        set: |c, v| {
            c.rgb_range = if name(v) == "full" {
                Levels::Full
            } else {
                Levels::Limited
            }
        },
    },
    Setting {
        key: "input.lldv",
        kind: Kind::Choice(&["on", "off"]),
        apply: Apply::Restart,
        help: "on: YCbCr input labelled SDR BT.2020 is a player's low-latency Dolby Vision, PQ already mapped \
               to input.lldv_nits. The capture card cannot tell the two apart, and SDR discs are BT.709.",
        get: |c| Value::Name(if c.lldv { "on" } else { "off" }.into()),
        set: |c, v| c.lldv = name(v) == "on",
    },
    Setting {
        key: "input.lldv_nits",
        kind: Kind::Int {
            min: 48,
            max: 10000,
        },
        apply: Apply::Restart,
        help: "The peak, in nits, that low-latency Dolby Vision is mapped to: what the EDID upstream declares.",
        get: |c| Value::Int(c.lldv_nits.into()),
        set: |c, v| c.lldv_nits = int(v) as u16,
    },
    Setting {
        key: "tonemap.curve",
        kind: Kind::Choice(CURVES),
        apply: Apply::Restart,
        help: "The tone curve for HDR into SDR output: libplacebo's, by name.",
        get: |c| Value::Name(c.curve.name().into()),
        set: |c, v| c.curve = Curve::from_name(name(v)).unwrap_or(Curve::Spline),
    },
    Setting {
        key: "tonemap.source_peak",
        kind: Kind::Int { min: 0, max: 10000 },
        apply: Apply::Restart,
        help: "The HDR10 source's peak, in nits, that the curve maps from, in place of its MaxCLL or \
               mastering peak; 0 uses the metadata.",
        get: |c| Value::Int(c.source_peak.into()),
        set: |c, v| c.source_peak = int(v) as u16,
    },
    Setting {
        key: "detect.crop",
        kind: Kind::Choice(&["on", "off"]),
        apply: Apply::Restart,
        help: "on: find the black bars and crop them out, the picture placed at constant image height in the \
               screen's band of the raster. off: every frame fills the raster as it came.",
        get: |c| Value::Name(if c.crop { "on" } else { "off" }.into()),
        set: |c, v| c.crop = name(v) == "on",
    },
    Setting {
        key: "geometry.screen_ar",
        kind: Kind::Float { min: 1.0, max: 3.0 },
        apply: Apply::Restart,
        help: "The screen's aspect ratio: with the lens set so the raster's width fills the screen, the band of \
               the raster that lands on it. 2.35 for a 2.35:1 screen.",
        get: |c| Value::Float(c.screen_ar),
        set: |c, v| c.screen_ar = float(v),
    },
    Setting {
        key: "geometry.fill_max_ar",
        kind: Kind::Float { min: 1.0, max: 3.0 },
        apply: Apply::Restart,
        help: "Content wider than the screen but no wider than this fills the screen's height, losing a \
               sliver at each side (2.39 and 2.40 on a 2.35 screen give up about 1% a side and show no bars). \
               Wider content (2.55, 2.76) keeps its full width with thin bars inside the screen. 0 is off.",
        get: |c| Value::Float(c.fill_max_ar),
        set: |c, v| c.fill_max_ar = float(v),
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
        }
    }

    /// Checks `v` and sets it.
    fn put(&self, c: &mut Config, v: &Value) -> Result<(), Error> {
        let bad = || Error::Value {
            key: self.key.to_string(),
            value: v.to_string(),
            range: self.range(),
        };
        match (self.kind, v) {
            (Kind::Int { min, max }, Value::Int(n)) if (min..=max).contains(n) => {}
            (Kind::Choice(names), Value::Name(n)) if names.contains(&n.as_str()) => {}
            (Kind::Float { min, max }, Value::Float(x)) if (min..=max).contains(x) => {}
            // A whole number where a ratio goes: 2 for 2.0.
            (Kind::Float { min, max }, Value::Int(n)) if (min..=max).contains(&(*n as f64)) => {}
            _ => return Err(bad()),
        }
        (self.set)(c, v);
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
            Error::UnknownKey(k) => write!(f, "no setting {k} (cinefit settings lists them)"),
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
    let text = text.trim();
    let value = match s.kind {
        Kind::Int { .. } => Value::Int(text.parse().map_err(|_| Error::Value {
            key: s.key.to_string(),
            value: text.to_string(),
            range: s.range(),
        })?),
        Kind::Choice(_) => Value::Name(text.to_string()),
        Kind::Float { .. } => Value::Float(text.parse().map_err(|_| Error::Value {
            key: s.key.to_string(),
            value: text.to_string(),
            range: s.range(),
        })?),
    };
    s.put(c, &value)
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
            Value::Name(n) => out.push_str(&format!("{name} = \"{n}\"\n")),
            // Always with a fraction, so it reads back as a float.
            Value::Float(v) => out.push_str(&format!("{name} = {v:?}\n")),
        }
    }
    out
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
        assert_eq!((c.screen_ar, c.fill_max_ar), (2.4, 2.0));
        assert!(set(&mut c, "geometry.screen_ar=0.5").is_err());
        assert!(set(&mut c, "geometry.screen_ar=wide").is_err());
        let text = render(&c);
        assert!(
            text.contains("screen_ar = 2.4\n") && text.contains("fill_max_ar = 2.0\n"),
            "{text}"
        );
        assert_eq!(parse(&text, p()).unwrap(), c);
        assert_eq!(
            parse("[geometry]\nscreen_ar = 2\n", p()).unwrap().screen_ar,
            2.0
        );
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

    #[test]
    fn every_key_is_section_dot_name_and_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for s in settings() {
            assert_eq!(s.key.split('.').count(), 2, "{}", s.key);
            assert!(seen.insert(s.key), "{} twice", s.key);
            assert!(!s.help.is_empty());
        }
    }
}
