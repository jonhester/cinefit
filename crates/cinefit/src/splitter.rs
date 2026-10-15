//! The EZCOO EZ-SP12H2 splitter in front of the capture card (#109): which
//! EDID it is presenting upstream, which decides what the player sends
//! (Dolby Vision tunnel, low-latency at some target, or the card's own). A
//! pairing pass is only as good as its label, so each dataset session notes
//! it.
//!
//! The splitter's USB port is a vendor HID device (28e9:028a, 64-byte
//! reports, no report IDs) taking EZCOO's text commands, as
//! cinefit-cpp's `tools/ezcoo/ezcoo.py` found; `EZG IN1 EDID` answers
//! `IN1 EDID <n>`. Read through `/dev/hidrawN`, which a udev rule opens to
//! the owner. Reads only: nothing here changes the splitter.

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, poll};

const ID: &str = "000028E9:0000028A";

/// The splitter's hidraw node, if it is plugged in.
fn node() -> Option<std::path::PathBuf> {
    let mut dirs: Vec<_> = std::fs::read_dir("/sys/class/hidraw")
        .ok()?
        .flatten()
        .collect();
    dirs.sort_by_key(|d| d.file_name());
    dirs.into_iter().find_map(|d| {
        let uevent = std::fs::read_to_string(d.path().join("device/uevent")).ok()?;
        uevent
            .to_uppercase()
            .contains(ID)
            .then(|| std::path::Path::new("/dev").join(d.file_name()))
    })
}

/// Sends one command and returns what comes back within `wait`.
fn command(text: &str, wait: Duration) -> Option<String> {
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(node()?)
        .ok()?;
    let data = format!("{}\r\n", text.trim()).into_bytes();
    for chunk in data.chunks(64) {
        // The first byte is the report ID, 0 for a device with none.
        let mut report = [0u8; 65];
        report[1..=chunk.len()].copy_from_slice(chunk);
        f.write_all(&report).ok()?;
    }
    let mut out = Vec::new();
    let mut deadline = Instant::now() + wait;
    let mut buf = [0u8; 64];
    while Instant::now() < deadline {
        let mut fds = [PollFd::new(&f, PollFlags::IN)];
        if matches!(poll(&mut fds, 200), Ok(n) if n > 0) {
            // An error or hang-up alone (unplugged) would leave a read
            // waiting on a device with nothing to say.
            if !fds[0].revents().contains(PollFlags::IN) {
                return None;
            }
            let n = f.read(&mut buf).ok()?;
            out.extend_from_slice(&buf[..n]);
            // Keep reading while it is talking.
            deadline = Instant::now() + Duration::from_millis(400);
        }
    }
    let text: String = String::from_utf8_lossy(&out).replace('\0', "");
    Some(text)
}

/// `IN1 EDID 28` as 28.
pub fn parse(reply: &str) -> Option<u32> {
    reply
        .lines()
        .find_map(|l| l.trim().strip_prefix("IN1 EDID"))
        .and_then(|n| n.trim().parse().ok())
}

/// What the switch position means, where it is known: the rig's own
/// positions, as the owner set them up (2026-10-13).
pub fn name(index: u32) -> Option<&'static str> {
    match index {
        32 => Some("copy: the capture card's own EDID, no Dolby Vision"),
        _ => None,
    }
}

/// The splitter's EDID index now; `None` without the splitter, or without
/// an answer. Up to about two seconds: never on the frame loop.
pub fn edid_index() -> Option<u32> {
    parse(&command("EZG IN1 EDID", Duration::from_millis(1500))?)
}

/// The line a session notes, or `None` without the splitter.
pub fn note() -> Option<String> {
    let i = edid_index()?;
    Some(match name(i) {
        Some(n) => format!("splitter: EDID {i} ({n})"),
        None => format!("splitter: EDID {i}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reply_gives_the_index() {
        assert_eq!(parse("IN1 EDID 28\r\n"), Some(28));
        assert_eq!(parse("\r\nIN1 EDID 32"), Some(32));
        assert_eq!(parse("Unknown command"), None);
    }

    /// Against the real splitter, read-only: `cargo test -p cinefit splitter
    /// -- --ignored`.
    #[test]
    #[ignore = "needs the splitter on USB"]
    fn the_real_splitter_answers() {
        let i = edid_index().expect("an answer");
        eprintln!("{}", note().unwrap_or_default());
        assert!(i > 0);
    }
}
