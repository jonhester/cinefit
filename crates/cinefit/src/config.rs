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
    /// The peak, in nits, a player's low-latency Dolby Vision is mapped to:
    /// what the EDID upstream declares, a fact of the input. It passes one
    /// to one, the SDR output's white standing for this.
    pub lldv_nits: u16,
    /// The screen's measured peak white, in nits: the output's scale. What
    /// HDR is mapped into when `target_nits` is not set.
    pub peak_nits: u16,
    /// What HDR10 and the Dolby Vision tunnel are mapped into, in nits, and
    /// so what the SDR output's white stands for with them; 0 is
    /// `peak_nits`. Above the peak it is madVR's "target nits": the curve
    /// keeps more highlight separation and the screen shows the whole
    /// picture at peak / target of the mapped levels.
    pub target_nits: u16,
    /// The output's rate, in hertz, while there is no source to follow:
    /// the rate the sources start in saves a resync when one comes on.
    pub wait_hz: f64,
    /// Whether YCbCr input labelled SDR BT.2020 is a player's low-latency
    /// Dolby Vision: PQ already mapped by the player to `lldv_nits`.
    pub lldv: bool,
    /// The capture device by persistent ID (hex); empty for the first that
    /// can capture (#68).
    pub input_device: String,
    /// The tone curve for HDR into the SDR output.
    pub curve: Curve,
    /// Whether that curve follows each frame's measured peak and average
    /// (libplacebo's peak detection) or the title's static peak alone.
    pub peak_detect: bool,
    /// libplacebo's contrast recovery for that curve, 0 (off) to 1.
    pub contrast_recovery: f64,
    /// Where peak detection reads each frame's peak, as a percentile of its
    /// brightness; 0 or 100 is the brightest pixel (libplacebo's default).
    pub peak_percentile: f64,
    /// BT.2390's knee offset; 0 leaves libplacebo's default (1.0).
    pub knee_offset: f64,
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
    /// Pairing passes (#138): `SESSION:FIRST-LAST`, the tunnel pass this one
    /// follows, as frames of the open dataset session; empty for none.
    pub pair_follow: String,
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
            lldv_nits: 96,
            // SDR's reference white: an unmeasured screen is taken to be a
            // standard one. A measured screen sets its own in the file.
            peak_nits: 100,
            target_nits: 0,
            wait_hz: 23.976,
            input_device: String::new(),
            lldv: true,
            // Judged at the screen on 2026-10-20/21 with the settings below
            // (the clouds), and the best HDR10 result against Dolby's
            // low-latency renditions on the validation titles
            // (cinefit-research docs/log.md). Spline was libplacebo's default.
            curve: Curve::Bt2390,
            // libplacebo's own default, and what its curves are designed
            // around: from a static peak alone, one curve serves a whole film.
            peak_detect: true,
            // libplacebo's 0.3: judged at the screen (2026-10-21), it brings
            // back the cloud texture the curve flattens.
            contrast_recovery: 0.3,
            peak_percentile: PEAK_PERCENTILE_DEFAULT,
            knee_offset: KNEE_OFFSET_DEFAULT,
            source_peak: 0,
            crop: true,
            record: true,
            frames_gb: 20,
            pair_every: 0,
            pair_follow: String::new(),
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
        help: "sdr2020: SDR in a BT.2020 container, HDR tone-mapped into tonemap.target_nits; set the display \
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
               to input.lldv_nits. The capture card cannot tell the two apart, and SDR discs are BT.709.",
        get: |c| Value::Name(if c.lldv { "on" } else { "off" }.into()),
        set: Set::Name(|c, v| c.lldv = v == "on"),
    },
    Setting {
        key: "input.lldv_nits",
        kind: Kind::Int { min: 48, max: 1000 },
        apply: Apply::Live,
        help: "The peak, in nits, a player's low-latency Dolby Vision is mapped to: what the EDID upstream \
               declares (96 for the usual low-latency Dolby Vision EDIDs). It passes one to one, the SDR \
               output's white standing for this; the tone-mapping settings do not touch it.",
        get: |c| Value::Int(c.lldv_nits.into()),
        set: Set::Int(|c, v| c.lldv_nits = v as u16),
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
        key: "tonemap.peak_detect",
        kind: Kind::Choice(&["on", "off"]),
        apply: Apply::Live,
        help: "on: the curve follows each frame's measured peak and average, scene by scene. off: one curve \
               for the whole title, from its declared peak (or tonemap.source_peak). Only where HDR is \
               tone-mapped; SDR input, low-latency Dolby Vision and passthrough are never measured.",
        get: |c| Value::Name(if c.peak_detect { "on" } else { "off" }.into()),
        set: Set::Name(|c, v| c.peak_detect = v == "on"),
    },
    Setting {
        key: "tonemap.contrast_recovery",
        kind: Kind::Float { min: 0.0, max: 1.0 },
        apply: Apply::Live,
        help: "Detail the tone curve flattens in bright areas (cloud texture, lamps, skies), added back by \
               sharpening there: 0 off, libplacebo's own default 0.3. Higher can halo bright edges and sharpen \
               grain. Only where HDR is tone-mapped.",
        get: |c| Value::Float(c.contrast_recovery),
        set: Set::Float(|c, v| c.contrast_recovery = v),
    },
    Setting {
        key: "tonemap.peak_percentile",
        kind: Kind::Float {
            min: 0.0,
            max: 100.0,
        },
        apply: Apply::Live,
        help: "Where peak detection reads each frame's peak, as a percentile of its brightness: 100 (or 0) is \
               the brightest pixel, libplacebo's own default. A few stray bright pixels then set the peak, and \
               the curve darkens the whole scene to make room for them; a little below 100 (99.9) follows the \
               scene's real highlights. 99.9 suits spline; bt2390, already brighter than Dolby, is better at 100. \
               Only with tonemap.peak_detect on.",
        get: |c| Value::Float(c.peak_percentile),
        set: Set::Float(|c, v| c.peak_percentile = v),
    },
    Setting {
        key: "tonemap.knee_offset",
        kind: Kind::Float { min: 0.0, max: 2.0 },
        apply: Apply::Live,
        help: "BT.2390's knee offset: where its roll-off starts, relative to the spec's own placement (0.5); \
               higher keeps more of the picture below the knee untouched. 0 leaves libplacebo's default (1.0). \
               Only for tonemap.curve bt2390; the other curves ignore it.",
        get: |c| Value::Float(c.knee_offset),
        set: Set::Float(|c, v| c.knee_offset = v),
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
        key: "tonemap.peak_nits",
        kind: Kind::Int { min: 10, max: 1000 },
        apply: Apply::Live,
        help: "The screen's measured peak white, in nits, in the lamp mode in use. HDR is mapped into it \
               unless tonemap.target_nits is set. 100, SDR's reference white, until it is measured.",
        get: |c| Value::Int(c.peak_nits.into()),
        set: Set::Int(|c, v| c.peak_nits = v as u16),
    },
    Setting {
        key: "tonemap.target_nits",
        kind: Kind::Int { min: 0, max: 2000 },
        apply: Apply::Live,
        help: "What HDR10 and Dolby Vision are mapped into, in nits; 0 is tonemap.peak_nits. Higher darkens \
               the picture and keeps more highlight separation, lower brightens it and compresses more: the \
               screen shows the mapped picture at peak / target, its peak staying the brightest code either \
               way. Low-latency Dolby Vision is not affected (input.lldv_nits).",
        get: |c| Value::Int(c.target_nits.into()),
        set: Set::Int(|c, v| c.target_nits = v as u16),
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
               the same frames of the film. 48 at 24p is one every two seconds; a tunnel frame is about 14 MB \
               kept, so mind detect.frames_gb. 0: off, frames kept as usual.",
        get: |c| Value::Int(c.pair_every.into()),
        set: Set::Int(|c, v| c.pair_every = v as u32),
    },
    Setting {
        key: "detect.pair_follow",
        kind: Kind::Text { max: 40 },
        apply: Apply::Live,
        help: "For pairing two passes of a title: SESSION:FIRST-LAST names the tunnel pass of this chapter, as \
               frames of the open dataset session. The pass now playing then keeps the frames whose pictures match \
               the ones that pass kept, found by thumbnail, in place of detect.pair_every's own cuts. Ignored in any \
               other session. Set by the pass runner for each low-latency pass. Empty: off.",
        get: |c| Value::Name(c.pair_follow.clone()),
        set: Set::Name(|c, v| c.pair_follow = v.trim().to_string()),
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
        help: "The Denon receiver, by hostname or IP address (for example 10.42.1.172): cinefit reads which \
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

// Chosen against Dolby's low-latency renditions on the development titles
// (cinefit-research docs/log.md, 2026-10-20). Spline with the peak read at
// 99.9 % and BT.2390 with knee offset 2 scored the same there (12.6 dE,
// libplacebo's defaults 14.9 and 15.1); BT.2390 at 100 % held up best on the
// validation titles and at the screen, so it is the default. BT.2390 is
// better at 100 than 99.9, as it is already brighter than Dolby. Knee offset
// 2 only touches BT.2390.
const PEAK_PERCENTILE_DEFAULT: f64 = 100.0;
const KNEE_OFFSET_DEFAULT: f64 = 2.0;

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

impl Config {
    /// What HDR10 and the Dolby Vision tunnel are mapped into, in nits:
    /// the target if one is set, else the screen's peak.
    pub fn mapped_nits(&self) -> u16 {
        if self.target_nits > 0 {
            self.target_nits
        } else {
            self.peak_nits
        }
    }
}

/// The table, in a stable order.
pub fn settings() -> &'static [Setting] {
    SETTINGS
}

/// Settings that were removed, and what took their place: a file still
/// holding one is told what to do, not just that the key is unknown.
const RETIRED: &[(&str, &str)] = &[(
    "output.white_nits",
    "split (2026-10-16): low-latency Dolby Vision's peak is input.lldv_nits, the screen's peak \
     tonemap.peak_nits and what HDR is mapped into tonemap.target_nits; remove the line and set those",
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
    fn hdr_maps_into_the_target_else_the_screen_peak() {
        let mut c = Config::default();
        // Unmeasured: a standard screen.
        assert_eq!((c.peak_nits, c.mapped_nits()), (100, 100));
        set(&mut c, "tonemap.peak_nits=76").unwrap();
        assert_eq!(c.mapped_nits(), 76);
        set(&mut c, "tonemap.target_nits=150").unwrap();
        assert_eq!(c.mapped_nits(), 150);
        // Low-latency Dolby Vision keeps its own peak whatever HDR maps into.
        assert_eq!(c.lldv_nits, 96);
    }

    #[test]
    fn a_retired_setting_says_what_replaced_it() {
        let e = parse("[output]\nwhite_nits = 96\n", p()).unwrap_err();
        assert!(e.to_string().contains("tonemap.target_nits"), "{e}");
        assert!(set(&mut Config::default(), "output.white_nits=96").is_err());
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
}
