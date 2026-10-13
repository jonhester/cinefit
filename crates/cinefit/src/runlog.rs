//! A record of every `play` run in `~/cinefit-logs`, so a run started by hand
//! over ssh leaves something to read afterwards: a question about what the
//! player sent, asked after the terminal is closed, had no answer before this
//! (2026-10-13).
//!
//! Each line goes to stdout as before and to the file, written as it is said
//! rather than buffered: Ctrl-C, or a second Ctrl-C that exits on the spot,
//! keeps everything said up to then. Piping through `tee` instead lost the
//! summary once, when Ctrl-C killed `tee` first. File lines carry the UTC wall
//! clock, to line up with "it jumped at about quarter past" from the screen.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

static FILE: Mutex<Option<File>> = Mutex::new(None);

/// Prints a line and records it in the run's log.
macro_rules! say {
    ($($arg:tt)*) => {
        $crate::runlog::line(&format!($($arg)*))
    };
}
pub(crate) use say;

/// The directory runs are logged in: `~/cinefit-logs`.
pub fn default_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join("cinefit-logs"))
}

/// Starts this run's log in `dir`, named for the UTC time it started, with
/// the command line as its first line. Lines said before this go to stdout
/// only.
pub fn open(dir: &Path, prefix: &str) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let (date, time) = stamp(unix_now());
    let path = dir.join(format!("{prefix}-{date}-{}.log", time.replace(':', "")));
    // create_new: two runs started in the same second must not share a file.
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    *FILE.lock().unwrap_or_else(PoisonError::into_inner) = Some(file);
    let args: Vec<String> = std::env::args().collect();
    record(&format!("{} (started {date} {time} UTC)", args.join(" ")));
    Ok(path)
}

/// Prints `text` and records it, one stamped line per line of it.
pub fn line(text: &str) {
    println!("{}", text.trim_end_matches('\n'));
    record(text);
}

/// Records `text` in the log only: what the terminal already shows another
/// way (an error main prints), or what would bury it (the settings).
pub fn record(text: &str) {
    let mut guard = FILE.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(file) = guard.as_mut() else {
        return;
    };
    let (_, time) = stamp(unix_now());
    let mut out = String::new();
    for line in text.trim_end_matches('\n').lines() {
        out.push_str(&time);
        out.push(' ');
        out.push_str(line);
        out.push('\n');
    }
    // A disk that fills mid-run must not stop the picture: stop logging.
    if file.write_all(out.as_bytes()).is_err() {
        *guard = None;
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Seconds since 1970 as a UTC date and time, `2026-10-13` and `22:05:07`.
/// Days to a civil date by Howard Hinnant's algorithm, which is exact for
/// every date the proleptic Gregorian calendar has.
fn stamp(secs: u64) -> (String, String) {
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (
        format!("{year:04}-{month:02}-{day:02}"),
        format!("{:02}:{:02}:{:02}", rem / 3_600, rem % 3_600 / 60, rem % 60),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps_are_utc_dates_and_times() {
        assert_eq!(stamp(0), ("1970-01-01".into(), "00:00:00".into()));
        // `date -u -d @1791929107`: Tue Oct 13 22:05:07 UTC 2026.
        assert_eq!(
            stamp(1_791_929_107),
            ("2026-10-13".into(), "22:05:07".into())
        );
        // The leap day and the day after it.
        assert_eq!(stamp(1_709_164_800).0, "2024-02-29");
        assert_eq!(stamp(1_709_251_200).0, "2024-03-01");
        assert_eq!(stamp(1_735_689_599).0, "2024-12-31");
    }
}
