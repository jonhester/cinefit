//! A record of every `play` run in `~/cinefit-logs`, so a run started by hand
//! over ssh leaves something to read afterwards: a question about what the
//! player sent, asked after the terminal is closed, had no answer before this
//! (2026-10-13).
//!
//! `tracing` events at `info` and above go to the terminal as bare messages,
//! as `println!` printed them; at `debug` and above (detail such as the
//! settings a run read) to a file named for the UTC time the run started, each line stamped with the UTC time, its level and
//! where it came from, to line up with "it jumped at about quarter past" from
//! the screen. The file is written event by event, not through a buffer or a
//! background thread: Ctrl-C, or a second Ctrl-C that exits on the spot,
//! keeps everything logged up to then. Piping through `tee` instead lost the
//! summary once, when Ctrl-C killed `tee` first.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tracing::Subscriber;
use tracing_subscriber::filter::{LevelFilter, Targets};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::fmt::time::UtcTime;
use tracing_subscriber::prelude::*;

/// The target of an event the terminal already shows another way: the error
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
    let (path, writer) = match file {
        Ok((path, f)) => (Ok(path), Some(Mutex::new(f))),
        Err(e) => (Err(e), None),
    };
    // One run per process, so nothing has set it before.
    let _ = tracing::subscriber::set_global_default(subscriber(io::stdout, writer));
    path
}

fn create(dir: &Path, prefix: &str) -> io::Result<(PathBuf, File)> {
    fs::create_dir_all(dir)?;
    let stamp = time::OffsetDateTime::now_utc()
        .format(time::macros::format_description!(
            "[year]-[month]-[day]-[hour][minute][second]"
        ))
        .map_err(io::Error::other)?;
    let path = dir.join(format!("{prefix}-{stamp}.log"));
    // create_new: two runs started in the same second must not share a file.
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    Ok((path, file))
}

fn subscriber<T, F>(terminal: T, file: Option<F>) -> impl Subscriber + Send + Sync
where
    T: for<'w> MakeWriter<'w> + Send + Sync + 'static,
    F: for<'w> MakeWriter<'w> + Send + Sync + 'static,
{
    let terminal = tracing_subscriber::fmt::layer()
        .with_writer(terminal)
        .without_time()
        .with_level(false)
        .with_target(false)
        .with_filter(
            Targets::new()
                .with_default(LevelFilter::INFO)
                .with_target(FILE_ONLY, LevelFilter::OFF),
        );
    let file = file.map(|w| {
        tracing_subscriber::fmt::layer()
            .with_writer(w)
            .with_timer(UtcTime::rfc_3339())
            .with_filter(LevelFilter::DEBUG)
    });
    tracing_subscriber::registry().with(terminal).with(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::Arc;

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
    }

    #[test]
    fn terminal_gets_bare_messages_and_the_file_gets_stamped_lines() {
        let (term, file) = (Buffer::default(), Buffer::default());
        let (t, f) = (term.clone(), file.clone());
        let sub = subscriber(move || t.clone(), Some(move || f.clone()));
        tracing::subscriber::with_default(sub, || {
            tracing::info!("input: 3840x2160");
            tracing::debug!("settings: crop = on");
            tracing::error!(target: FILE_ONLY, "no stable input");
            tracing::trace!("a detector reading");
        });
        assert_eq!(term.text(), "input: 3840x2160\n");
        let lines: Vec<String> = file.text().lines().map(str::to_string).collect();
        assert_eq!(lines.len(), 3, "{lines:?}");
        // 2026-10-13T21:47:53.123456789Z  INFO cinefit::runlog::tests: input: ...
        let stamp = lines[0].split_whitespace().next().unwrap();
        assert!(stamp.len() > 20 && stamp.ends_with('Z'), "{stamp}");
        assert!(lines[0].contains(" INFO ") && lines[0].ends_with("input: 3840x2160"));
        assert!(lines[1].contains("DEBUG") && lines[1].ends_with("settings: crop = on"));
        assert!(lines[2].contains("ERROR") && lines[2].ends_with("no stable input"));
    }
}
