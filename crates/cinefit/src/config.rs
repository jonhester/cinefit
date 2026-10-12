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

use cinefit_render::Levels;

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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            width: 4096,
            height: 2160,
            rgb_range: Levels::Limited,
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
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Int(v) => write!(f, "{v}"),
            Value::Name(n) => f.write_str(n),
        }
    }
}

fn int(v: &Value) -> i64 {
    match v {
        Value::Int(v) => *v,
        Value::Name(_) => 0,
    }
}

fn name(v: &Value) -> &str {
    match v {
        Value::Name(n) => n,
        Value::Int(_) => "",
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
                // A float, bool or table fits no setting yet: refused by put.
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
    fn what_is_written_reads_back_the_same() {
        let mut c = Config::default();
        set(&mut c, "output.width=3840").unwrap();
        set(&mut c, "input.rgb_range=full").unwrap();
        let text = render(&c);
        assert!(text.contains("# The panel's width"));
        assert_eq!(parse(&text, p()).unwrap(), c);
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
