//! The OSD menu (#75): what it holds and what the remote does in it. Pure:
//! the settings and a key in, a change to make and a picture to draw out;
//! `play` applies the change and `ui` draws the picture.
//!
//! Four pages over the settings table. ←→ on a row steps through a short
//! list of values and applies the new one at once; comparing two values is
//! pressing back and forth. Back keeps what is set and goes up a level, and
//! Back on the pages, or Menu, closes it. Values come from lists, never
//! typed: ratios by their usual names, rates and the panel's size from the
//! display's own modes. A setting with no list here (the tone curve, SDR
//! white) is set in the file or on the web page.
//!
//! The last page is the test patterns, which are not settings: choosing one
//! shows it at once under the menu, nothing is saved, and it stays up after
//! the menu closes until Back takes it down (←→ step between patterns
//! meanwhile). It goes by itself after [`PATTERN_IDLE`] without a key.

use std::time::{Duration, Instant};

use cinefit_input::Action;

use crate::config::{self, Apply, Config, OutputMode, Ratio, Value};
use crate::osd::Encoding;
use crate::patterns::{self, KINDS, Kind, SIZES, Spec};

/// Closes by itself after this long without a key, so it is never left over
/// a film.
pub const IDLE: Duration = Duration::from_secs(60);

/// A test pattern goes by itself after this long without a key: long, as
/// focusing is done with the projector's own remote, which cinefit does not
/// see, but never left over a film.
pub const PATTERN_IDLE: Duration = Duration::from_secs(30 * 60);

const PAGES: [&str; 5] = ["Picture", "Output", "Screen", "Input", "Patterns"];

/// The screen shapes offered: the usual names, 16:9 by its own.
const SCREEN: [(f64, f64, &str); 8] = [
    (16.0, 9.0, "16:9"),
    (1.85, 1.0, "1.85 : 1"),
    (2.00, 1.0, "2.00 : 1"),
    (2.20, 1.0, "2.20 : 1"),
    (2.35, 1.0, "2.35 : 1"),
    (2.37, 1.0, "2.37 : 1"),
    (2.39, 1.0, "2.39 : 1"),
    (2.40, 1.0, "2.40 : 1"),
];

/// The widest content that still fills the screen's height: off, or a
/// standard ratio wider than the screen.
const FILL: [f64; 5] = [2.37, 2.39, 2.40, 2.55, 2.76];

/// One row: what it shows, and the setting behind it.
#[derive(Clone, Copy)]
enum Row {
    /// A setting stepped through a list.
    Setting {
        label: &'static str,
        key: &'static str,
    },
    /// Shown, not changed here: where it is set instead.
    Fixed {
        label: &'static str,
        what: fn(&Config) -> String,
        cost: &'static str,
    },
    /// A test pattern's choice: shown at once, never saved.
    Pattern { label: &'static str, what: Pick },
}

/// What a pattern row picks.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pick {
    Kind,
    Size,
    Level,
}

fn rows(page: usize) -> &'static [Row] {
    const PICTURE: &[Row] = &[
        Row::Setting {
            label: "Crop black bars",
            key: "detect.crop",
        },
        Row::Setting {
            label: "Fill the screen up to",
            key: "geometry.fill_max_ar",
        },
        Row::Setting {
            label: "Banners",
            key: "osd.banners",
        },
    ];
    const OUTPUT: &[Row] = &[
        Row::Setting {
            label: "HDR10",
            key: "output.mode",
        },
        Row::Setting {
            label: "Rate while waiting",
            key: "output.wait_hz",
        },
    ];
    const SCREEN_ROWS: &[Row] = &[
        Row::Setting {
            label: "Screen ratio",
            key: "geometry.screen_ar",
        },
        Row::Fixed {
            label: "Panel size",
            what: |c| format!("{} \u{d7} {}", c.width, c.height),
            cost: "Set at installation, in the settings file",
        },
        Row::Fixed {
            label: "Theater name",
            what: |c| {
                if c.venue.is_empty() {
                    "\u{2014}".into()
                } else {
                    c.venue.clone()
                }
            },
            cost: "Set on the web page or in the settings file",
        },
    ];
    const INPUT: &[Row] = &[
        Row::Setting {
            label: "RGB range",
            key: "input.rgb_range",
        },
        Row::Setting {
            label: "Dolby Vision low latency",
            key: "input.lldv",
        },
    ];
    const PATTERNS: &[Row] = &[
        Row::Pattern {
            label: "Pattern",
            what: Pick::Kind,
        },
        Row::Pattern {
            label: "Window size",
            what: Pick::Size,
        },
        Row::Pattern {
            label: "Window level",
            what: Pick::Level,
        },
    ];
    [PICTURE, OUTPUT, SCREEN_ROWS, INPUT, PATTERNS][page]
}

/// A value a row can take, with what the menu calls it.
struct Choice {
    value: Value,
    label: String,
}

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

fn rate_label(hz: f64) -> String {
    let s = format!("{hz:.3}");
    format!("{} Hz", s.trim_end_matches('0').trim_end_matches('.'))
}

fn ratio_label(r: Ratio) -> String {
    SCREEN
        .iter()
        .find(|(w, h, _)| (w / h - r.value()).abs() < 1e-9)
        .map_or_else(|| format!("{:.2} : 1", r.value()), |(_, _, l)| (*l).into())
}

/// What a setting row offers, in order, and the index of the value set now
/// if it is one of them.
fn choices(key: &str, c: &Config, rates: &[f64]) -> (Vec<Choice>, Option<usize>) {
    let named = |pairs: &[(&str, &str)], now: &str| {
        let list: Vec<Choice> = pairs
            .iter()
            .map(|(v, l)| Choice {
                value: Value::Name((*v).into()),
                label: (*l).into(),
            })
            .collect();
        let at = pairs.iter().position(|(v, _)| *v == now);
        (list, at)
    };
    match key {
        "detect.crop" => named(&[("off", "Off"), ("on", "On")], on_off(c.crop)),
        "osd.banners" => named(&[("off", "Off"), ("on", "On")], on_off(c.banners)),
        "input.lldv" => named(&[("off", "Off"), ("on", "On")], on_off(c.lldv)),
        "input.rgb_range" => named(
            &[("limited", "Limited"), ("full", "Full")],
            match c.rgb_range {
                cinefit_render::Levels::Limited => "limited",
                cinefit_render::Levels::Full => "full",
            },
        ),
        "output.mode" => named(
            &[
                ("passthrough", "To the projector"),
                ("sdr2020", "Tone-mapped to SDR BT.2020"),
            ],
            match c.mode {
                OutputMode::Passthrough => "passthrough",
                OutputMode::Sdr2020 => "sdr2020",
            },
        ),
        "geometry.screen_ar" => {
            let list: Vec<Choice> = SCREEN
                .iter()
                .map(|&(w, h, l)| Choice {
                    value: Value::Name(Ratio { w, h }.to_string()),
                    label: l.into(),
                })
                .collect();
            let at = SCREEN
                .iter()
                .position(|(w, h, _)| (w / h - c.screen_ar.value()).abs() < 1e-9);
            (list, at)
        }
        "geometry.fill_max_ar" => {
            let screen = c.screen_ar.value();
            let mut values = vec![0.0];
            values.extend(FILL.iter().copied().filter(|&f| f > screen + 1e-9));
            let list = values
                .iter()
                .map(|&f| Choice {
                    value: Value::Float(f),
                    label: if f == 0.0 {
                        "Off".into()
                    } else {
                        format!("{f:.2} : 1")
                    },
                })
                .collect();
            let at = values
                .iter()
                .position(|&f| (f - c.fill_max_ar.value()).abs() < 1e-9);
            (list, at)
        }
        "output.wait_hz" => {
            let list = rates
                .iter()
                .map(|&hz| Choice {
                    value: Value::Float(hz),
                    label: rate_label(hz),
                })
                .collect();
            let at = rates.iter().position(|&hz| (hz - c.wait_hz).abs() < 0.01);
            (list, at)
        }
        _ => (Vec::new(), None),
    }
}

/// What a row's value reads as now, set from a list or not.
fn value_label(key: &str, c: &Config, rates: &[f64]) -> String {
    let (list, at) = choices(key, c, rates);
    if let Some(i) = at {
        return list[i].label.clone();
    }
    // Set by hand to something the list does not hold: shown as it is.
    match key {
        "geometry.screen_ar" => ratio_label(c.screen_ar),
        "geometry.fill_max_ar" => format!("{:.2} : 1", c.fill_max_ar.value()),
        "output.wait_hz" => rate_label(c.wait_hz),
        _ => String::new(),
    }
}

/// The step from the value set now: the next in the list (`up`) or the one
/// before. A value set by hand that the list does not hold steps to its
/// nearest neighbour that way.
fn step(key: &str, c: &Config, rates: &[f64], up: bool) -> Option<Value> {
    let (list, at) = choices(key, c, rates);
    let i = match at {
        Some(i) if up => (i + 1 < list.len()).then_some(i + 1)?,
        Some(i) => i.checked_sub(1)?,
        None => {
            let now = match key {
                "geometry.screen_ar" => c.screen_ar.value(),
                "geometry.fill_max_ar" => c.fill_max_ar.value(),
                "output.wait_hz" => c.wait_hz,
                _ => return list.first().map(|x| x.value.clone()),
            };
            let num = |v: &Value| match v {
                Value::Float(x) => *x,
                Value::Name(t) => Ratio::parse(t).map_or(0.0, Ratio::value),
                Value::Int(n) => *n as f64,
            };
            if up {
                list.iter().position(|x| num(&x.value) > now)?
            } else {
                list.iter().rposition(|x| num(&x.value) < now)?
            }
        }
    };
    Some(list[i].value.clone())
}

fn cost(key: &str) -> &'static str {
    match config::find(key).map(|s| s.apply) {
        Some(Apply::Live) => "Applies now, no resync",
        Some(Apply::Modeset) => "Resyncs the projector",
        _ => "Set in the settings file",
    }
}

/// What the menu shows, for the OSD to draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuText {
    pub pages: Vec<String>,
    pub page: usize,
    /// Whether the rows have the focus, not the pages.
    pub in_rows: bool,
    /// Label, value, whether it can be changed here.
    pub rows: Vec<(String, String, bool)>,
    pub row: usize,
    pub cost: String,
    pub version: String,
}

/// The menu's state.
pub struct Menu {
    /// The test pattern up, if any, and its window's size and level.
    pattern: Option<Kind>,
    size: usize,
    level: usize,
    pattern_key: Instant,
    /// The output's encoding as last drawn: it names the window's levels.
    enc: Encoding,
    open: bool,
    page: usize,
    row: usize,
    in_rows: bool,
    last_key: Instant,
    /// The rates the display offers at the panel's size, for the rate while
    /// waiting.
    rates: Vec<f64>,
    version: String,
}

/// A change the menu made: a setting and its new value.
pub type Change = (&'static str, Value);

impl Menu {
    pub fn new(rates: Vec<f64>, version: &str) -> Self {
        Self {
            pattern: None,
            // A quarter of the screen: big enough for a meter, small
            // enough to keep the projector's light control out of it.
            size: 1,
            level: 0,
            pattern_key: Instant::now(),
            enc: Encoding::Sdr,
            open: false,
            page: 0,
            row: 0,
            in_rows: false,
            last_key: Instant::now(),
            rates,
            version: version.into(),
        }
    }

    #[cfg(test)]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Whether the menu takes key `a`: every key while open, Menu to open
    /// it, and Back and ←→ while a pattern is up.
    pub fn takes(&self, a: Action) -> bool {
        self.open
            || a == Action::Menu
            || (self.pattern.is_some() && matches!(a, Action::Back | Action::Left | Action::Right))
    }

    /// The test pattern to show now, if any; it goes after
    /// [`PATTERN_IDLE`] without a key.
    pub fn pattern(&mut self, now: Instant) -> Option<Spec> {
        if self.pattern.is_some() && now.duration_since(self.pattern_key) >= PATTERN_IDLE {
            self.pattern = None;
        }
        self.pattern.map(|kind| Spec {
            kind,
            size: self.size,
            level: self.level.min(patterns::level_count(self.enc) - 1),
        })
    }

    /// Steps a pattern row: off and the patterns, the sizes, or the levels
    /// of the encoding up now. The ends stop.
    fn step_pattern(&mut self, what: Pick, up: bool) {
        let step = |i: usize, n: usize| {
            if up {
                (i + 1).min(n - 1)
            } else {
                i.saturating_sub(1)
            }
        };
        match what {
            Pick::Kind => {
                // 0 is off, then the patterns in order.
                let i = self
                    .pattern
                    .and_then(|k| KINDS.iter().position(|&x| x == k))
                    .map_or(0, |i| i + 1);
                let i = step(i, KINDS.len() + 1);
                self.pattern = i.checked_sub(1).map(|i| KINDS[i]);
            }
            Pick::Size => self.size = step(self.size, SIZES.len()),
            Pick::Level => {
                let n = patterns::level_count(self.enc);
                self.level = step(self.level.min(n - 1), n);
            }
        }
    }

    fn pattern_label(&self, what: Pick) -> String {
        match what {
            Pick::Kind => self.pattern.map_or("Off", Kind::label).into(),
            Pick::Size => format!("{:.0}% of the screen", SIZES[self.size] * 100.0),
            Pick::Level => patterns::level_label(self.level, self.enc),
        }
    }

    /// One key. Returns a setting to change, if the key changed one. Keys
    /// the menu does not take while open (Info) are for the caller.
    pub fn key(&mut self, a: Action, c: &Config, now: Instant) -> Option<Change> {
        self.last_key = now;
        self.pattern_key = now;
        if !self.open {
            match a {
                Action::Menu => {
                    self.open = true;
                    self.in_rows = false;
                }
                Action::Back => self.pattern = None,
                // Between the patterns, never to off: that is Back.
                Action::Left | Action::Right if self.pattern.is_some() => {
                    self.step_pattern(Pick::Kind, a == Action::Right);
                    if self.pattern.is_none() {
                        self.pattern = Some(KINDS[0]);
                    }
                }
                _ => {}
            }
            return None;
        }
        let n_rows = rows(self.page).len();
        match (a, self.in_rows) {
            (Action::Menu, _) | (Action::Back, false) => self.open = false,
            (Action::Back, true) => self.in_rows = false,
            (Action::Up, false) => self.page = self.page.saturating_sub(1),
            (Action::Down, false) => self.page = (self.page + 1).min(PAGES.len() - 1),
            (Action::Right | Action::Select, false) => {
                self.in_rows = true;
                self.row = 0;
            }
            (Action::Up, true) => self.row = self.row.saturating_sub(1),
            (Action::Down, true) => self.row = (self.row + 1).min(n_rows - 1),
            (Action::Left | Action::Right, true) => match rows(self.page)[self.row] {
                Row::Setting { key, .. } => {
                    return step(key, c, &self.rates, a == Action::Right).map(|v| (key, v));
                }
                Row::Pattern { what, .. } => self.step_pattern(what, a == Action::Right),
                Row::Fixed { .. } => {}
            },
            _ => {}
        }
        None
    }

    /// What to draw, while open, for an output in `enc`; closes after
    /// [`IDLE`] without a key.
    pub fn view(&mut self, c: &Config, enc: Encoding, now: Instant) -> Option<MenuText> {
        self.enc = enc;
        if self.open && now.duration_since(self.last_key) >= IDLE {
            self.open = false;
        }
        if !self.open {
            return None;
        }
        let rows_now: Vec<(String, String, bool)> = rows(self.page)
            .iter()
            .map(|r| match *r {
                Row::Setting { label, key } => {
                    (label.into(), value_label(key, c, &self.rates), true)
                }
                Row::Fixed { label, what, .. } => (label.into(), what(c), false),
                Row::Pattern { label, what } => (label.into(), self.pattern_label(what), true),
            })
            .collect();
        let cost = if self.in_rows {
            match rows(self.page)[self.row] {
                Row::Setting { key, .. } => cost(key).into(),
                Row::Fixed { cost, .. } => cost.into(),
                Row::Pattern {
                    what: Pick::Level, ..
                } => match self.enc {
                    Encoding::Sdr => "SDR now: 100% white is the projector's peak".into(),
                    Encoding::Pq => "HDR10 now: where the projector maps each level".into(),
                },
                Row::Pattern { .. } => "Shown now, never saved \u{b7} Back takes it down".into(),
            }
        } else {
            String::new()
        };
        Some(MenuText {
            pages: PAGES.iter().map(|p| (*p).into()).collect(),
            page: self.page,
            in_rows: self.in_rows,
            rows: rows_now,
            row: self.row,
            cost,
            version: self.version.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATES: [f64; 4] = [23.976, 24.0, 59.94, 60.0];

    fn menu() -> Menu {
        Menu::new(RATES.to_vec(), "0.1.0")
    }

    /// Presses keys, applying each change to `c` as `play` does.
    fn press(m: &mut Menu, c: &mut Config, keys: &[Action]) -> Vec<String> {
        let mut changed = Vec::new();
        for &k in keys {
            if let Some((key, v)) = m.key(k, c, Instant::now()) {
                config::find(key).unwrap().put(c, &v).unwrap();
                changed.push(format!("{key}={v}"));
            }
        }
        changed
    }

    #[test]
    fn menu_opens_moves_between_pages_and_closes() {
        let (mut m, mut c) = (menu(), Config::default());
        assert!(m.view(&c, Encoding::Sdr, Instant::now()).is_none());
        press(&mut m, &mut c, &[Action::Menu]);
        let v = m.view(&c, Encoding::Sdr, Instant::now()).unwrap();
        assert_eq!((v.page, v.in_rows, v.cost.as_str()), (0, false, ""));
        press(
            &mut m,
            &mut c,
            &[
                Action::Down,
                Action::Down,
                Action::Down,
                Action::Down,
                Action::Down,
            ],
        );
        assert_eq!(
            m.view(&c, Encoding::Sdr, Instant::now()).unwrap().page,
            4,
            "stops at the last page"
        );
        press(&mut m, &mut c, &[Action::Back]);
        assert!(!m.is_open());
        press(&mut m, &mut c, &[Action::Menu, Action::Menu]);
        assert!(!m.is_open(), "Menu again closes it");
    }

    #[test]
    fn arrows_step_a_setting_and_apply_it() {
        let (mut m, mut c) = (menu(), Config::default());
        // Picture › Crop black bars: on, then off with ←, on again with →.
        let changed = press(
            &mut m,
            &mut c,
            &[Action::Menu, Action::Right, Action::Left, Action::Right],
        );
        assert_eq!(changed, ["detect.crop=off", "detect.crop=on"]);
        assert!(c.crop);
        // Down to the fill limit: 2.40 on a 2.35 screen, → to 2.55.
        let changed = press(&mut m, &mut c, &[Action::Down, Action::Right]);
        assert_eq!(changed, ["geometry.fill_max_ar=2.55"]);
        let v = m.view(&c, Encoding::Sdr, Instant::now()).unwrap();
        assert_eq!(v.rows[1].1, "2.55 : 1");
        assert_eq!(v.cost, "Applies now, no resync");
        // Back keeps it and goes to the pages; Back again closes.
        press(&mut m, &mut c, &[Action::Back, Action::Back]);
        assert!(!m.is_open());
        assert_eq!(c.fill_max_ar.value(), 2.55);
    }

    #[test]
    fn lists_are_the_usual_values_and_the_ends_stop() {
        let mut c = Config::default();
        // The fill limit offers off and the ratios wider than the screen.
        let (list, at) = choices("geometry.fill_max_ar", &c, &RATES);
        let labels: Vec<&str> = list.iter().map(|x| x.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Off", "2.37 : 1", "2.39 : 1", "2.40 : 1", "2.55 : 1", "2.76 : 1"
            ]
        );
        assert_eq!(at, Some(3));
        // 16:9 by its name, stored exactly.
        config::set(&mut c, "geometry.screen_ar=16:9").unwrap();
        assert_eq!(value_label("geometry.screen_ar", &c, &RATES), "16:9");
        assert_eq!(
            step("geometry.screen_ar", &c, &RATES, false),
            None,
            "the first stops"
        );
        assert_eq!(
            step("geometry.screen_ar", &c, &RATES, true),
            Some(Value::Name("1.85".into()))
        );
        // Rates from the display, by name.
        assert_eq!(value_label("output.wait_hz", &c, &RATES), "23.976 Hz");
        assert_eq!(
            step("output.wait_hz", &c, &RATES, true),
            Some(Value::Float(24.0))
        );
        assert_eq!(cost("output.wait_hz"), "Resyncs the projector");
    }

    #[test]
    fn a_hand_set_value_shows_as_it_is_and_steps_to_its_neighbours() {
        let mut c = Config::default();
        config::set(&mut c, "geometry.screen_ar=2.36").unwrap();
        assert_eq!(value_label("geometry.screen_ar", &c, &RATES), "2.36 : 1");
        assert_eq!(
            step("geometry.screen_ar", &c, &RATES, true),
            Some(Value::Name("2.37".into()))
        );
        assert_eq!(
            step("geometry.screen_ar", &c, &RATES, false),
            Some(Value::Name("2.35".into()))
        );
    }

    #[test]
    fn fixed_rows_show_and_say_where_they_are_set() {
        let (mut m, mut c) = (menu(), Config::default());
        // Screen: down to panel size, ←→ change nothing.
        let changed = press(
            &mut m,
            &mut c,
            &[
                Action::Menu,
                Action::Down,
                Action::Down,
                Action::Right,
                Action::Down,
                Action::Right,
                Action::Left,
            ],
        );
        assert!(changed.is_empty());
        let v = m.view(&c, Encoding::Sdr, Instant::now()).unwrap();
        assert_eq!(
            v.rows[1],
            ("Panel size".into(), "4096 \u{d7} 2160".into(), false)
        );
        assert_eq!(v.cost, "Set at installation, in the settings file");
        assert_eq!(v.rows[2].1, "\u{2014}", "no theater name");
    }

    /// Opens the menu on the Patterns page, its rows in focus.
    fn to_patterns(m: &mut Menu, c: &mut Config) {
        let mut keys = vec![Action::Menu];
        keys.extend([Action::Down; 4]);
        keys.push(Action::Right);
        assert!(press(m, c, &keys).is_empty());
    }

    #[test]
    fn a_pattern_shows_at_once_stays_after_the_menu_and_is_never_saved() {
        let (mut m, mut c) = (menu(), Config::default());
        let now = Instant::now();
        to_patterns(&mut m, &mut c);
        assert_eq!(m.pattern(now), None);
        // → on Pattern: alignment, at once, and no setting changed.
        assert!(press(&mut m, &mut c, &[Action::Right]).is_empty());
        let v = m.view(&c, Encoding::Sdr, now).unwrap();
        assert_eq!(v.pages[4], "Patterns");
        assert_eq!(v.rows[0].1, "Alignment");
        assert_eq!(v.cost, "Shown now, never saved \u{b7} Back takes it down");
        assert_eq!(m.pattern(now).map(|p| p.kind), Some(Kind::Alignment));
        // Menu closes the menu; the pattern stays.
        press(&mut m, &mut c, &[Action::Menu]);
        assert!(!m.is_open());
        assert_eq!(m.pattern(now).map(|p| p.kind), Some(Kind::Alignment));
        // ←→ step between the patterns with the menu closed, never to off.
        assert!(m.takes(Action::Right) && !m.takes(Action::Up));
        press(&mut m, &mut c, &[Action::Right, Action::Right]);
        assert_eq!(m.pattern(now).map(|p| p.kind), Some(Kind::FocusGreen));
        press(&mut m, &mut c, &[Action::Left, Action::Left, Action::Left]);
        assert_eq!(m.pattern(now).map(|p| p.kind), Some(Kind::Alignment));
        // Back takes it down, and then the menu takes no more keys.
        press(&mut m, &mut c, &[Action::Back]);
        assert_eq!(m.pattern(now), None);
        assert!(!m.takes(Action::Back) && !m.takes(Action::Right));
        assert_eq!(c, Config::default(), "nothing set");
    }

    #[test]
    fn the_window_steps_its_size_and_its_level_in_the_output_encoding() {
        let (mut m, mut c) = (menu(), Config::default());
        let now = Instant::now();
        to_patterns(&mut m, &mut c);
        // Pattern: → four times to the window, the last; a fifth stops.
        press(&mut m, &mut c, &[Action::Right; 5]);
        assert_eq!(m.pattern(now).map(|p| p.kind), Some(Kind::Window));
        // Size: a quarter to start, → to half.
        press(&mut m, &mut c, &[Action::Down, Action::Right]);
        let v = m.view(&c, Encoding::Pq, now).unwrap();
        assert_eq!(v.rows[1].1, "50% of the screen");
        // Level, in nits under HDR10, down from 10,000 to 1,000.
        assert_eq!(v.rows[2].1, "10000 nits");
        press(
            &mut m,
            &mut c,
            &[Action::Down, Action::Right, Action::Right],
        );
        let v = m.view(&c, Encoding::Pq, now).unwrap();
        assert_eq!(v.rows[2].1, "1000 nits");
        assert!(v.cost.starts_with("HDR10 now"));
        assert_eq!(m.pattern(now).unwrap().level, 2);
        // Under SDR, as a share of white; the index held past its end is
        // the list's last until stepped.
        press(&mut m, &mut c, &[Action::Right; 3]);
        assert_eq!(m.pattern(now).unwrap().level, 5);
        let v = m.view(&c, Encoding::Sdr, now).unwrap();
        assert_eq!(v.rows[2].1, "10% white");
        assert_eq!(m.pattern(now).unwrap().level, 4);
        press(&mut m, &mut c, &[Action::Left]);
        assert_eq!(m.pattern(now).unwrap().level, 3);
    }

    #[test]
    fn a_pattern_goes_by_itself_after_a_long_while() {
        let (mut m, mut c) = (menu(), Config::default());
        let t0 = Instant::now();
        m.key(Action::Menu, &c, t0);
        for _ in 0..4 {
            m.key(Action::Down, &c, t0);
        }
        m.key(Action::Right, &c, t0);
        m.key(Action::Right, &c, t0);
        m.key(Action::Menu, &c, t0);
        assert!(
            m.pattern(t0 + PATTERN_IDLE - Duration::from_secs(1))
                .is_some()
        );
        assert!(m.pattern(t0 + PATTERN_IDLE).is_none());
        let _ = &mut c;
    }

    #[test]
    fn it_closes_itself_when_left_alone() {
        let (mut m, mut c) = (menu(), Config::default());
        let t0 = Instant::now();
        m.key(Action::Menu, &c, t0);
        assert!(
            m.view(&c, Encoding::Sdr, t0 + IDLE - Duration::from_millis(1))
                .is_some()
        );
        assert!(m.view(&c, Encoding::Sdr, t0 + IDLE).is_none());
        let _ = &mut c;
    }
}
