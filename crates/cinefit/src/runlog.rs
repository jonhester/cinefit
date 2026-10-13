//! A record of every `play` run in `~/cinefit-logs`, so a run started by hand
//! over ssh leaves something to read afterwards: a question about what the
//! player sent, asked after the terminal is closed, had no answer before this
//! (2026-10-13).
//!
//! Code logs through the `log` macros; `fern` sends `info` and above to the
//! terminal as bare messages, as `println!` printed them, and `debug` and
//! above (detail such as the settings a run read) to a file named for the UTC
//! time the run started, each line stamped with the UTC time, its level and
//! where it came from, to line up with "it jumped at about quarter past" from
//! the screen. fern flushes the file after every record: Ctrl-C, or a second
//! Ctrl-C that exits on the spot, keeps everything logged up to then. Piping
//! through `tee` instead lost the summary once, when Ctrl-C killed `tee`
//! first.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use log::LevelFilter;
use time::format_description::well_known::Rfc3339;

/// The target of a record the terminal already shows another way: the error
/// a run ends with, which main prints. Kept off the terminal by target, not
/// by level, because it is an error.
pub const FILE_ONLY: &str = "cinefit::file_only";

/// The directory runs are logged in: `~/cinefit-logs`.
pub fn default_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join("cinefit-logs"))
}

/// Logs to the terminal and to a new file in `dir`, named
/// `<prefix>-<date>-<time>.log`. Returns the file's path, or why there is
/// none: the terminal gets its lines either way.
pub fn init(dir: Option<&Path>, prefix: &str) -> Result<PathBuf, String> {
    let file = dir
        .ok_or_else(|| "HOME is not set".to_string())
        .and_then(|d| create(d, prefix).map_err(|e| format!("{}: {e}", d.display())));
    let (path, file) = match file {
        Ok((path, f)) => (Ok(path), Some(f)),
        Err(e) => (Err(e), None),
    };
    // One run per process, so no logger is set yet.
    let _ = dispatch(io::stdout(), file).apply();
    path
}

fn create(dir: &Path, prefix: &str) -> io::Result<(PathBuf, File)> {
    fs::create_dir_all(dir)?;
    let stamp = time::OffsetDateTime::now_utc()
        .format(time::macros::format_description!(
            "[year]-[month]-[day]-[hour][minute][second]"
        ))
        .map_err(io::Error::other)?;
    // create_new, and a numbered name if taken: two runs started in the same
    // second must neither share a file nor go unlogged (the second fails on
    // the capture card, and its error is worth keeping).
    let mut n = 1;
    loop {
        let name = match n {
            1 => format!("{prefix}-{stamp}.log"),
            _ => format!("{prefix}-{stamp}-{n}.log"),
        };
        let path = dir.join(name);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists && n < 100 => n += 1,
            Err(e) => return Err(e),
        }
    }
}

fn dispatch(
    terminal: impl Into<fern::Output>,
    file: Option<impl Into<fern::Output>>,
) -> fern::Dispatch {
    let terminal = fern::Dispatch::new()
        .level(LevelFilter::Info)
        .level_for(FILE_ONLY, LevelFilter::Off)
        .format(|out, message, _| out.finish(*message))
        .chain(terminal);
    let mut both = fern::Dispatch::new().chain(terminal);
    if let Some(file) = file {
        both = both.chain(
            fern::Dispatch::new()
                .level(LevelFilter::Debug)
                .format(|out, message, record| {
                    let now = time::OffsetDateTime::now_utc()
                        .format(&Rfc3339)
                        .unwrap_or_default();
                    out.finish(format_args!(
                        "{now} {:5} {}: {message}",
                        record.level(),
                        record.target()
                    ));
                })
                .chain(file),
        );
    }
    both
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Buffer(Arc<Mutex<Vec<u8>>>);

    impl Write for Buffer {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Buffer {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
        fn output(&self) -> fern::Output {
            (Box::new(self.clone()) as Box<dyn Write + Send>).into()
        }
    }

    fn log(logger: &dyn log::Log, level: log::Level, target: &str, message: &str) {
        logger.log(
            &log::Record::builder()
                .level(level)
                .target(target)
                .args(format_args!("{message}"))
                .build(),
        );
    }

    #[test]
    fn runs_in_the_same_second_get_their_own_files() {
        let dir = std::env::temp_dir().join(format!("cinefit-runlog-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let paths: Vec<PathBuf> = (0..3).map(|_| create(&dir, "play").unwrap().0).collect();
        let _ = fs::remove_dir_all(&dir);
        // Three in a row land in one second, or straddle two; either way no
        // two share a name.
        assert_ne!(paths[0], paths[1]);
        assert_ne!(paths[1], paths[2]);
        assert_ne!(paths[0], paths[2]);
    }

    #[test]
    fn terminal_gets_bare_messages_and_the_file_gets_stamped_lines() {
        let (term, file) = (Buffer::default(), Buffer::default());
        let (_, logger) = dispatch(term.output(), Some(file.output())).into_log();
        log(
            &*logger,
            log::Level::Info,
            "cinefit::play",
            "input: 3840x2160",
        );
        log(
            &*logger,
            log::Level::Debug,
            "cinefit::play",
            "settings: crop = on",
        );
        log(&*logger, log::Level::Error, FILE_ONLY, "no stable input");
        log(
            &*logger,
            log::Level::Trace,
            "cinefit::play",
            "a detector reading",
        );

        assert_eq!(term.text(), "input: 3840x2160\n");
        let lines: Vec<String> = file.text().lines().map(str::to_string).collect();
        assert_eq!(lines.len(), 3, "{lines:?}");
        // 2026-10-13T21:47:53.123456789Z INFO  cinefit::play: input: ...
        let stamp = lines[0].split_whitespace().next().unwrap();
        assert!(stamp.len() > 20 && stamp.ends_with('Z'), "{stamp}");
        assert!(
            lines[0].contains(" INFO ") && lines[0].ends_with("cinefit::play: input: 3840x2160")
        );
        assert!(lines[1].contains(" DEBUG ") && lines[1].ends_with("settings: crop = on"));
        assert!(lines[2].contains(" ERROR ") && lines[2].ends_with("no stable input"));
    }
}
