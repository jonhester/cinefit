//! Readings: what [`Bars`](crate::bars::Bars) and [`Cuts`](crate::cuts::Cuts)
//! saw on each frame, saved so that [`ArPolicy`](crate::ar_policy::ArPolicy)
//! alone can replay a whole clip in milliseconds instead of minutes of
//! decompressing pixels. A policy threshold is then tried over every clip at
//! once.
//!
//! One CSV line per frame in the C++ version's columns (its
//! `detect/readings.cpp`), so its saved readings replay here too, including
//! those from before `island_start` existed (13 fields a side).

use crate::bars::{FrameClass, Observation, SideObs};

/// One frame's reading.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Reading {
    pub index: u64,
    pub time: f64,
    pub epoch: u32,
    pub obs: Observation,
    pub cut: bool,
}

const SIDE: [&str; 14] = [
    "found",
    "bar",
    "run",
    "level",
    "coverage",
    "strength",
    "sharpness",
    "fill",
    "zones",
    "continues",
    "islands",
    "ui",
    "ui_start",
    "island_start",
];
/// Readings saved before `island_start` existed.
const OLD_SIDE: usize = 13;
const HEAD: usize = 9;

/// The CSV header line.
pub fn header() -> String {
    let mut h = "index,time,epoch,width,height,class,strength,why,cut".to_string();
    for p in ["t_", "b_"] {
        for f in SIDE {
            h.push(',');
            h.push_str(p);
            h.push_str(f);
        }
    }
    h
}

fn side(out: &mut String, s: &SideObs) {
    out.push_str(&format!(
        ",{},{},{},{},{:.4},{:.4},{:.4},{:.4},{},{},{},{},{},{}",
        u8::from(s.found),
        s.bar,
        s.run,
        s.level,
        s.coverage,
        s.strength,
        s.sharpness,
        s.fill,
        s.zones,
        u8::from(s.continues),
        s.islands,
        u8::from(s.ui),
        s.ui_start,
        s.island_start
    ));
}

/// One reading as a CSV line, without the newline. Fractions are kept to 4
/// places, as the C++ version writes them: replaying them must still give
/// the pixel run's crops, which the replay checks.
pub fn format(r: &Reading) -> String {
    let o = &r.obs;
    let mut out = format!(
        "{},{:.6},{},{},{},{},{:.4},{},{}",
        r.index,
        r.time,
        r.epoch,
        o.width,
        o.height,
        o.class.name(),
        o.strength,
        o.why,
        u8::from(r.cut)
    );
    side(&mut out, &o.top);
    side(&mut out, &o.bottom);
    out
}

fn parse_side(f: &[&str], n: usize) -> Option<SideObs> {
    let u = |i: usize| f.get(i)?.parse::<u32>().ok();
    let x = |i: usize| f.get(i)?.parse::<f64>().ok();
    let b = |i: usize| u(i).map(|v| v != 0);
    Some(SideObs {
        found: b(0)?,
        bar: u(1)?,
        run: u(2)?,
        level: u(3)?,
        coverage: x(4)?,
        strength: x(5)?,
        sharpness: x(6)?,
        fill: x(7)?,
        zones: u(8)?,
        continues: b(9)?,
        islands: u(10)?,
        ui: b(11)?,
        ui_start: u(12)?,
        // No position known: past any crop, so the policy never takes such
        // an island for text beyond it (as the C++ version reads them).
        island_start: if n > OLD_SIDE { u(13)? } else { u32::MAX / 2 },
    })
}

/// `Observation::why` is one of a fixed set of the bar reader's phrases;
/// anything else reads as none. It is diagnostic only: the policy never
/// reads it.
fn why(s: &str) -> &'static str {
    [
        "no edge on one side",
        "full height on one side",
        "asymmetric",
        "picture continues across the edge",
        "low fill",
        "picture missing from a zone",
    ]
    .into_iter()
    .find(|w| *w == s)
    .unwrap_or("")
}

/// A CSV line back into a reading; `None` for the header or a line that is
/// not one.
pub fn parse(line: &str) -> Option<Reading> {
    let f: Vec<&str> = line.trim_end().split(',').collect();
    let n = match f.len() {
        l if l == HEAD + 2 * SIDE.len() => SIDE.len(),
        l if l == HEAD + 2 * OLD_SIDE => OLD_SIDE,
        _ => return None,
    };
    let class = match f[5] {
        "EMPTY" => FrameClass::Empty,
        "FULL" => FrameClass::Full,
        "LETTERBOX" => FrameClass::Letterbox,
        "WEAK" => FrameClass::Weak,
        _ => return None,
    };
    Some(Reading {
        index: f[0].parse().ok()?,
        time: f[1].parse().ok()?,
        epoch: f[2].parse().ok()?,
        obs: Observation {
            class,
            width: f[3].parse().ok()?,
            height: f[4].parse().ok()?,
            strength: f[6].parse().ok()?,
            why: why(f[7]),
            top: parse_side(&f[HEAD..HEAD + n], n)?,
            bottom: parse_side(&f[HEAD + n..], n)?,
        },
        cut: f[8].parse::<u8>().ok()? != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Reading {
        let side = |bar| SideObs {
            found: true,
            bar,
            run: bar + 3,
            level: 70,
            coverage: 0.875,
            strength: 0.5,
            sharpness: 1.25,
            fill: 0.9375,
            zones: 4,
            continues: false,
            islands: 1,
            island_start: 300,
            ui: false,
            ui_start: 0,
        };
        Reading {
            index: 1234,
            time: 51.468_125,
            epoch: 2,
            obs: Observation {
                class: FrameClass::Letterbox,
                width: 3840,
                height: 2160,
                top: side(276),
                bottom: side(277),
                strength: 0.5,
                why: "",
            },
            cut: true,
        }
    }

    #[test]
    fn a_reading_reads_back_as_written() {
        let r = sample();
        let line = format(&r);
        assert_eq!(line.split(',').count(), header().split(',').count());
        assert_eq!(parse(&line), Some(r));
        assert_eq!(parse(&header()), None);
        let weak = Reading {
            obs: Observation {
                class: FrameClass::Weak,
                why: "asymmetric",
                ..r.obs
            },
            ..r
        };
        assert_eq!(parse(&format(&weak)), Some(weak));
    }

    #[test]
    fn the_policy_over_saved_readings_gives_the_pixels_crops() {
        // Synthetic frames: full, then scope, then full again, each with a
        // little motion, through the whole detector; the policy alone over
        // what was saved must decide every frame the same.
        use crate::ar_policy::ArPolicy;
        use crate::detector::Detector;
        use crate::synth::{Frame, Palette};
        let (w, h) = (3840, 2160);
        let pal = Palette::new(w);
        let mut f = Frame::new(&pal, h);
        let mut det = Detector::default();
        let mut policy = ArPolicy::default();
        let mut changes = 0;
        for i in 0..72u32 {
            let t = f64::from(i) / 24.0;
            f.fill(0, h, &pal.bar);
            let (top, bottom) = if (16..48).contains(&i) {
                (276, 277)
            } else {
                (0, 0)
            };
            // A cut every 8 frames: new content, as a film's shots change.
            f.content(top, bottom, &pal.picture, i / 8);
            let (d, obs, cut) = det.push_reading(&f.pack(), t, 1);
            changes += u32::from(d.changed);
            let line = format(&Reading {
                index: u64::from(i),
                time: t,
                epoch: 1,
                obs,
                cut,
            });
            let back = parse(&line).expect("reads back");
            assert_eq!(
                policy.push(&back.obs, back.cut, back.time, back.epoch).crop,
                d.crop,
                "frame {i}"
            );
        }
        assert!(
            changes >= 2,
            "the sequence should zoom in and out: {changes}"
        );
    }

    #[test]
    fn the_cpp_columns_and_its_older_files_read() {
        // A line as the C++ version writes it, and one from before
        // island_start: 13 fields a side.
        let new = "7,0.291958,1,3840,2160,LETTERBOX,0.6123,,0,1,276,280,64,0.9100,0.6123,1.0400,0.9500,4,0,0,0,0,1080,1,277,281,64,0.9000,0.6200,1.0100,0.9600,4,0,0,0,0,1080";
        let r = parse(new).expect("current columns");
        assert_eq!((r.index, r.obs.top.bar, r.obs.bottom.bar), (7, 276, 277));
        assert_eq!(r.obs.top.island_start, 1080);
        let old = "7,0.291958,1,3840,2160,EMPTY,0.0000,,1,0,0,0,64,0.0000,0.0000,0.0000,0.0000,0,0,0,0,0,0,0,0,64,0.0000,0.0000,0.0000,0.0000,0,0,0,0,0";
        let r = parse(old).expect("old columns");
        assert!(r.cut && r.obs.class == FrameClass::Empty);
        assert_eq!(r.obs.top.island_start, u32::MAX / 2);
        assert_eq!(parse("7,0.29,1,3840,2160,ODD,0,,0"), None);
    }
}
