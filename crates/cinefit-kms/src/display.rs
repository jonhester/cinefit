//! `cinefit display`: what each DRM device offers, read without taking the
//! display. Opening a card, reading its resources, properties and blobs needs
//! no DRM master, so this is safe while the service is showing a film. It
//! answers the questions a new GPU raises before anything is committed: are
//! the HDR properties there, which planes exist, and which formats and
//! modifiers each plane scans out.

use std::fmt::Write as _;
use std::fs::OpenOptions;
use std::os::fd::{AsFd, BorrowedFd};
use std::path::{Path, PathBuf};

use drm::control::{Device as ControlDevice, connector, plane, property};
use drm::{ClientCapability, Device as BasicDevice, DriverCapability};

use crate::edid;
use crate::output::{Error, io};

struct Card(std::fs::File);

impl AsFd for Card {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}
impl BasicDevice for Card {}
impl ControlDevice for Card {}

/// The connector properties cinefit sets or reads, in the order printed.
/// Their names use spaces: `max bpc`, not `max_bpc`.
const CONNECTOR_PROPS: [&str; 6] = [
    "HDR_OUTPUT_METADATA",
    "Colorspace",
    "max bpc",
    "content type",
    "Broadcast RGB",
    "link-status",
];

/// Describes `path`, or every `/dev/dri/card*`.
///
/// # Errors
///
/// If no device can be opened at all.
pub fn describe(path: Option<&Path>) -> Result<String, Error> {
    let cards: Vec<PathBuf> = match path {
        Some(p) => vec![p.to_owned()],
        None => {
            let mut v: Vec<PathBuf> = std::fs::read_dir("/dev/dri")
                .map_err(io("listing /dev/dri"))?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("card"))
                })
                .collect();
            v.sort();
            v
        }
    };
    let mut out = String::new();
    let mut opened = 0;
    for p in &cards {
        match OpenOptions::new().read(true).write(true).open(p) {
            Ok(f) => {
                opened += 1;
                card(&mut out, p, &Card(f));
            }
            Err(e) => {
                let _ = writeln!(out, "{}: cannot open: {e}", p.display());
            }
        }
    }
    if opened == 0 {
        return Err(Error::NoDisplay);
    }
    Ok(out)
}

fn card(out: &mut String, path: &Path, card: &Card) {
    let driver = card.get_driver().map_or_else(
        |e| format!("? ({e})"),
        |d| {
            format!(
                "{} ({})",
                d.name().to_string_lossy(),
                d.description().to_string_lossy()
            )
        },
    );
    let _ = writeln!(out, "{}: driver {driver}", path.display());
    // Without these the kernel hides the primary and overlay planes and
    // the atomic properties. Setting them needs no master.
    let universal = card.set_client_capability(ClientCapability::UniversalPlanes, true);
    let atomic = card.set_client_capability(ClientCapability::Atomic, true);
    let cap = |c| card.get_driver_capability(c).ok();
    let _ = writeln!(
        out,
        "  atomic {}, universal planes {}, framebuffer modifiers {}",
        yes_no(atomic.is_ok()),
        yes_no(universal.is_ok()),
        cap(DriverCapability::AddFB2Modifiers).map_or("?", |v| yes_no(v != 0)),
    );
    let Ok(res) = card.resource_handles() else {
        let _ = writeln!(out, "  no display resources (a render-only device)");
        return;
    };
    for &c in res.connectors() {
        connector_info(out, card, c);
    }
    let _ = writeln!(out, "  CRTCs: {}", res.crtcs().len());
    match card.plane_handles() {
        Ok(planes) => {
            for p in planes {
                plane_info(out, card, &res, p);
            }
        }
        Err(e) => {
            let _ = writeln!(out, "  planes: cannot list ({e})");
        }
    }
}

fn yes_no(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

fn connector_info(out: &mut String, card: &Card, handle: connector::Handle) {
    // `false`: report the state as the kernel last saw it; a forced probe
    // re-reads the EDID, which some chains answer with a blackout.
    let Ok(info) = card.get_connector(handle, false) else {
        return;
    };
    let _ = writeln!(
        out,
        "  connector {}-{}: {:?}, {} modes",
        info.interface().as_str(),
        info.interface_id(),
        info.state(),
        info.modes().len()
    );
    if info.state() != connector::State::Connected {
        return;
    }
    let mut sizes: Vec<String> = info
        .modes()
        .iter()
        .map(|m| {
            // The exact rate: `vrefresh` is an integer, so 23.976 reads 24.
            let (w, h) = m.size();
            let (_, _, ht) = m.hsync();
            let (_, _, vt) = m.vsync();
            let hz = f64::from(m.clock()) * 1000.0 / (f64::from(ht) * f64::from(vt));
            let hz = format!("{hz:.3}");
            let hz = hz.trim_end_matches('0').trim_end_matches('.');
            let i = if m.flags().contains(drm::control::ModeFlags::INTERLACE) {
                "i"
            } else {
                ""
            };
            format!("{w}x{h}{i}@{hz}")
        })
        .collect();
    let mut seen = std::collections::HashSet::new();
    sizes.retain(|m| seen.insert(m.clone()));
    let _ = writeln!(out, "    modes: {}", sizes.join(" "));
    let Ok(props) = card.get_properties(handle) else {
        return;
    };
    let (ids, values) = props.as_props_and_values();
    let mut found = Vec::new();
    for (&id, &value) in ids.iter().zip(values) {
        let Ok(p) = card.get_property(id) else {
            continue;
        };
        let name = p.name().to_string_lossy().to_string();
        if name == "EDID" {
            edid_info(out, card, value);
        } else if CONNECTOR_PROPS.contains(&name.as_str()) {
            found.push((name, describe_value(card, &p, value)));
        }
    }
    for want in CONNECTOR_PROPS {
        match found.iter().find(|(n, _)| n == want) {
            Some((n, v)) => {
                let _ = writeln!(out, "    {n}: {v}");
            }
            None => {
                let _ = writeln!(out, "    {want}: absent");
            }
        }
    }
}

fn edid_info(out: &mut String, card: &Card, blob: u64) {
    let bytes = if blob == 0 {
        Vec::new()
    } else {
        card.get_property_blob(blob).unwrap_or_default()
    };
    match edid::summary(&bytes) {
        None => {
            let _ = writeln!(out, "    EDID: none readable ({} bytes)", bytes.len());
        }
        Some(s) => {
            let _ = writeln!(
                out,
                "    EDID: {} {} ({} bytes); HDR: {}; colorimetry: {}",
                s.manufacturer,
                s.name.as_deref().unwrap_or("(no name)"),
                bytes.len(),
                s.eotfs.map_or("no HDR block".into(), |e| e.join(", ")),
                if s.colorimetry.is_empty() {
                    "no block".into()
                } else {
                    s.colorimetry.join(", ")
                }
            );
        }
    }
}

/// A property's value in words, with its choices or range.
fn describe_value(card: &Card, p: &property::Info, value: u64) -> String {
    match p.value_type() {
        property::ValueType::Enum(e) => {
            let (vals, names) = e.values();
            let name_of = |v: u64| {
                vals.iter()
                    .zip(names)
                    .find(|(x, _)| **x == v)
                    .map(|(_, n)| n.name().to_string_lossy().to_string())
            };
            let all: Vec<String> = names
                .iter()
                .map(|n| n.name().to_string_lossy().to_string())
                .collect();
            format!(
                "{} (of {})",
                name_of(value).unwrap_or_else(|| value.to_string()),
                all.join(", ")
            )
        }
        property::ValueType::UnsignedRange(lo, hi) => format!("{value} (range {lo}-{hi})"),
        property::ValueType::Blob => {
            if value == 0 {
                "none set (blob)".into()
            } else {
                let len = card.get_property_blob(value).map_or(0, |b| b.len());
                format!("set ({len}-byte blob)")
            }
        }
        _ => value.to_string(),
    }
}

fn plane_info(
    out: &mut String,
    card: &Card,
    res: &drm::control::ResourceHandles,
    handle: plane::Handle,
) {
    let Ok(info) = card.get_plane(handle) else {
        return;
    };
    let mut kind = "?".to_string();
    let mut in_formats = None;
    if let Ok(props) = card.get_properties(handle) {
        let (ids, values) = props.as_props_and_values();
        for (&id, &value) in ids.iter().zip(values) {
            let Ok(p) = card.get_property(id) else {
                continue;
            };
            match p.name().to_bytes() {
                b"type" => kind = describe_value(card, &p, value),
                b"IN_FORMATS" if value != 0 => in_formats = card.get_property_blob(value).ok(),
                _ => {}
            }
        }
    }
    let kind = kind.split(" (of").next().unwrap_or("?").to_string();
    let crtcs: Vec<String> = res
        .filter_crtcs(info.possible_crtcs())
        .iter()
        .map(|c| u32::from(*c).to_string())
        .collect();
    let _ = writeln!(
        out,
        "  plane {} ({kind}, CRTCs {}){}",
        u32::from(handle),
        crtcs.join(","),
        if info.crtc().is_some() {
            ", in use"
        } else {
            ""
        }
    );
    match in_formats.as_deref().and_then(format_modifiers) {
        Some(list) => {
            for (format, mods) in list {
                let _ = writeln!(out, "    {}: {}", fourcc(format), mods.join(" "));
            }
        }
        None => {
            let names: Vec<String> = info.formats().iter().map(|&f| fourcc(f)).collect();
            let _ = writeln!(out, "    formats (no IN_FORMATS): {}", names.join(" "));
        }
    }
}

/// The plane's `IN_FORMATS` blob as (format, modifier names) pairs.
fn format_modifiers(blob: &[u8]) -> Option<Vec<(u32, Vec<String>)>> {
    let u32_at = |o: usize| {
        blob.get(o..o + 4)
            .map(|b| u32::from_ne_bytes(b.try_into().unwrap()))
    };
    let u64_at = |o: usize| {
        blob.get(o..o + 8)
            .map(|b| u64::from_ne_bytes(b.try_into().unwrap()))
    };
    let count_formats = u32_at(8)? as usize;
    let formats_offset = u32_at(12)? as usize;
    let count_modifiers = u32_at(16)? as usize;
    let modifiers_offset = u32_at(20)? as usize;
    let formats: Vec<u32> = (0..count_formats)
        .map(|i| u32_at(formats_offset + 4 * i))
        .collect::<Option<_>>()?;
    // Each entry: a 64-bit mask of formats, from `offset` in the list, then
    // the modifier.
    let mut list: Vec<(u32, Vec<String>)> = formats.iter().map(|&f| (f, Vec::new())).collect();
    for m in 0..count_modifiers {
        let at = modifiers_offset + 24 * m;
        let mask = u64_at(at)?;
        let offset = u32_at(at + 8)? as usize;
        let modifier = u64_at(at + 16)?;
        for bit in 0..64 {
            if mask & (1 << bit) != 0
                && let Some(entry) = list.get_mut(offset + bit)
            {
                entry.1.push(modifier_name(modifier));
            }
        }
    }
    Some(list)
}

fn modifier_name(m: u64) -> String {
    match m {
        0 => "LINEAR".into(),
        0x00ff_ffff_ffff_ffff => "INVALID".into(),
        _ => format!("{m:#018x}"),
    }
}

fn fourcc(f: u32) -> String {
    f.to_le_bytes().iter().map(|&b| char::from(b)).collect()
}
