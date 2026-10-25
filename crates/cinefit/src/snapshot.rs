//! Save a frame (#92): what is on the screen right now, kept for a bug
//! report, a colour question or a test, without a recording.
//!
//! Each save is a directory `~/cinefit-clips/frames/<date>-<time>/` (private
//! footage, never in a repository) holding the frame as it arrived, in
//! `capture --dump`'s form (`frame-000.bin` and `frame-000.json`), so the
//! dump tests and the renderer read it as they read any dump; and
//! `context.json`, everything that decided how it was shown: the tunnel's
//! Dolby Vision metadata decoded, HDR10's static metadata, what low-latency
//! Dolby Vision is taken to be, the settings, the crop and the sources.
//!
//! The frame loop only copies the bytes; the writing is another thread's.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use cinefit_decklink::{FrameInfo, HdrMetadata, PixelFormat};
use cinefit_dovi::DmMetadata;
use serde_json::{Value as Json, json};

use crate::capture::{Dump, write_dump};

/// Where saves go: `~/cinefit-clips/frames`.
pub fn default_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join("cinefit-clips").join("frames"))
}

/// One frame and what it was shown with, copied off the frame loop.
pub struct Save {
    pub info: FrameInfo,
    pub bytes: Vec<u8>,
    /// Seconds into the run.
    pub at: f64,
    /// Everything else, built by `play` (see [`context`]).
    pub context: Json,
}

/// Writes `save` into a new directory under `root`, named by the time;
/// returns it.
///
/// # Errors
///
/// If the directory or a file cannot be written.
pub fn write(root: &Path, save: &Save) -> io::Result<PathBuf> {
    let stamp = time::OffsetDateTime::now_utc()
        .format(time::macros::format_description!(
            "[year][month][day]-[hour][minute][second]"
        ))
        .map_err(io::Error::other)?;
    std::fs::create_dir_all(root)?;
    // Two saves in one second get their own directories.
    let mut dir = root.join(&stamp);
    let mut n = 1;
    while dir.exists() {
        n += 1;
        dir = root.join(format!("{stamp}-{n}"));
    }
    std::fs::create_dir(&dir)?;
    write_dump(
        &dir,
        &Dump {
            index: 0,
            at: Duration::from_secs_f64(save.at.max(0.0)),
            info: save.info,
            bytes: save.bytes.clone(),
        },
    )?;
    let text = serde_json::to_string_pretty(&save.context).map_err(io::Error::other)?;
    std::fs::write(dir.join("context.json"), text + "\n")?;
    Ok(dir)
}

/// The tunnel's metadata, decoded from the frame's own bytes: every field
/// the structure carries, levels by name, others raw. `None` for a frame
/// that is not a tunnel.
pub fn tunnel(info: &FrameInfo, bytes: &[u8]) -> Option<Json> {
    if info.pixel_format != PixelFormat::Rgb10 {
        return None;
    }
    let parity = cinefit_dovi::r210_parity(bytes, info.width as usize, info.row_bytes);
    match cinefit_dovi::decode(parity) {
        Ok(t) => Some(json!({
            "packet": {
                "copies_used": t.copies_used,
                "no_metadata": t.header.no_metadata,
                "end_of_stream": t.header.end_of_stream,
                "metadata_version": t.header.metadata_version,
            },
            "metadata": t.metadata.as_ref().map(dm),
        })),
        Err(cinefit_dovi::Error::NoPacket { packet: 0 }) => None,
        Err(e) => Some(json!({ "error": e.to_string() })),
    }
}

fn dm(m: &DmMetadata) -> Json {
    let nits = |pq: u16| cinefit_dovi::pq_to_nits(pq);
    json!({
        "scene_refresh": m.scene_refresh,
        "source_min_pq": m.source_min_pq,
        "source_max_pq": m.source_max_pq,
        "source_nits": [nits(m.source_min_pq), nits(m.source_max_pq)],
        "signal_bit_depth": m.signal_bit_depth,
        "signal_color_space": m.signal_color_space,
        "ycc_to_rgb": m.ycc_to_rgb,
        "ycc_to_rgb_offset": m.ycc_to_rgb_offset,
        "rgb_to_lms": m.rgb_to_lms,
        "level1": m.level1.map(|l| json!({
            "min_pq": l.min_pq, "avg_pq": l.avg_pq, "max_pq": l.max_pq,
            "nits": [nits(l.min_pq), nits(l.avg_pq), nits(l.max_pq)],
        })),
        "level2": m.level2.iter().map(|t| json!({
            "target_max_pq": t.target_max_pq,
            "target_nits": nits(t.target_max_pq),
            "trim_slope": t.trim_slope,
            "trim_offset": t.trim_offset,
            "trim_power": t.trim_power,
            "trim_chroma_weight": t.trim_chroma_weight,
            "trim_saturation_gain": t.trim_saturation_gain,
            "ms_weight": t.ms_weight,
        })).collect::<Vec<_>>(),
        "level4": m.level4.map(|l| json!({"anchor_pq": l.anchor_pq, "anchor_power": l.anchor_power})),
        "level5": m.level5.map(|l| json!({
            "left": l.active_area_left_offset, "right": l.active_area_right_offset,
            "top": l.active_area_top_offset, "bottom": l.active_area_bottom_offset,
        })),
        "other": m.other.iter().map(|(level, b)| json!({
            "level": level,
            "hex": b.iter().map(|x| format!("{x:02x}")).collect::<String>(),
        })).collect::<Vec<_>>(),
    })
}

/// What the Apple TV reported playing, every field; `on_screen` whether
/// the receiver was on its input, so it was what the frame shows.
pub fn apple_tv(np: &cinefit_atv::NowPlaying, on_screen: bool) -> Json {
    let unix = |t: std::time::SystemTime| {
        t.duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|d| d.as_secs_f64())
    };
    json!({
        "on_screen": on_screen,
        "app": np.app,
        "app_name": np.app_name,
        "player": np.player,
        "state": np.state.map(|s| format!("{s:?}")),
        "title": np.title,
        "artist": np.artist,
        "series": np.series,
        "season": np.season,
        "episode": np.episode,
        "duration": np.duration,
        "position": np.position_at(std::time::SystemTime::now()),
        "elapsed": np.elapsed,
        "elapsed_at_unix": np.elapsed_at.and_then(unix),
        "rate": np.rate,
        "content_id": np.content_id,
        "store_id": np.store_id,
        "media_type": np.media_type,
    })
}

/// The UB820's transport, read now over its network interface: off the
/// frame loop, since a player that does not answer takes its timeout.
pub fn ub820(host: &str) -> Json {
    match crate::ub820::read_status((host, crate::ub820::PORT)) {
        Ok(s) => json!({
            "host": host,
            "state": format!("{:?}", s.state),
            "position_seconds": s.position,
        }),
        Err(e) => json!({ "host": host, "error": e.to_string() }),
    }
}

/// HDR10's static metadata as the card reported it; unreported values are
/// null, never zero.
pub fn hdr(h: &HdrMetadata) -> Json {
    let xy = |c: Option<cinefit_decklink::Chromaticity>| c.map(|c| [c.x, c.y]);
    json!({
        "red": xy(h.red), "green": xy(h.green), "blue": xy(h.blue), "white": xy(h.white),
        "max_mastering": h.max_mastering,
        "min_mastering": h.min_mastering,
        "max_cll": h.max_cll,
        "max_fall": h.max_fall,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(pixel_format: PixelFormat) -> FrameInfo {
        FrameInfo {
            width: 8,
            height: 2,
            row_bytes: 32,
            pixel_format,
            ..crate::record::tests::info(32)
        }
    }

    #[test]
    fn a_save_is_a_dump_with_its_context() {
        let root = std::env::temp_dir().join(format!("cinefit-save-{}", std::process::id()));
        let save = Save {
            info: info(PixelFormat::V210),
            bytes: vec![7u8; 64],
            at: 12.5,
            context: json!({"content": "HDR10", "settings": {"tonemap.target_nits": 96}}),
        };
        let a = write(&root, &save).unwrap();
        let b = write(&root, &save).unwrap();
        assert_ne!(a, b, "two saves in one second keep apart");
        assert_eq!(std::fs::read(a.join("frame-000.bin")).unwrap(), save.bytes);
        let dump = std::fs::read_to_string(a.join("frame-000.json")).unwrap();
        assert!(dump.contains("\"pixel_format\": \"v210\""), "{dump}");
        let ctx: Json =
            serde_json::from_str(&std::fs::read_to_string(a.join("context.json")).unwrap())
                .unwrap();
        assert_eq!(ctx["settings"]["tonemap.target_nits"], 96);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn unreported_hdr_values_are_null() {
        let h = HdrMetadata {
            red: None,
            green: None,
            blue: None,
            white: None,
            max_mastering: None,
            min_mastering: None,
            max_cll: Some(617.0),
            max_fall: None,
        };
        let j = hdr(&h);
        assert_eq!(j["max_cll"], 617.0);
        assert!(j["max_fall"].is_null());
        assert!(j["red"].is_null());
    }

    #[test]
    fn only_rgb_is_read_for_a_tunnel() {
        assert!(tunnel(&info(PixelFormat::V210), &[0u8; 64]).is_none());
        // RGB without a packet is plain RGB, not a tunnel.
        assert!(tunnel(&info(PixelFormat::Rgb10), &[0u8; 64]).is_none());
    }
}
