//! Which capture device to use (#68): by its persistent ID, from the
//! `input.device` setting, or the first that can capture. A card with several
//! inputs (a DeckLink Quad HDMI Recorder) is several devices to the SDK, and
//! the order of its list is not promised across driver updates or reboots, so
//! an index in a settings file could quietly point at another input. An ID
//! that is not present is an error with the list, never a fall back to
//! another input.

use cinefit_decklink::DeviceInfo;

/// What was asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Want {
    /// `-d N`: the index in `cinefit probe`, for one run.
    Index(usize),
    /// `input.device`: a persistent ID.
    Id(i64),
    /// Neither: the first device that can capture.
    First,
}

/// A persistent ID as `cinefit probe` prints it, `0x1620413`, or the same
/// hex without the `0x`.
pub fn parse_id(s: &str) -> Option<i64> {
    let s = s.trim();
    let hex = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s);
    if hex.is_empty() {
        return None;
    }
    i64::from_str_radix(hex, 16).ok()
}

/// The device wanted, from a one-run index and the setting: the index wins.
pub fn want(index: Option<usize>, setting: &str) -> Result<Want, String> {
    if let Some(i) = index {
        return Ok(Want::Index(i));
    }
    if setting.trim().is_empty() {
        return Ok(Want::First);
    }
    parse_id(setting).map(Want::Id).ok_or_else(|| {
        format!(
            "input.device = {setting:?}: not a persistent ID (hex, as `cinefit probe` prints it)"
        )
    })
}

/// The devices as `cinefit probe` names them, for an error.
pub fn list(infos: &[DeviceInfo]) -> String {
    if infos.is_empty() {
        return "no devices".into();
    }
    infos
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let id = d
                .persistent_id
                .map_or_else(|| "unreported".into(), |id| format!("{id:#x}"));
            format!(
                "device {i}: {} ({}), persistent ID {id}",
                d.display_name, d.model_name
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// The index of the device wanted.
pub fn choose(infos: &[DeviceInfo], want: Want) -> Result<usize, String> {
    match want {
        Want::Index(i) if i < infos.len() => Ok(i),
        Want::Index(i) => Err(format!("no device {i}: {}", list(infos))),
        Want::Id(id) => infos
            .iter()
            .position(|d| d.persistent_id == Some(id))
            .ok_or_else(|| {
                format!(
                    "no device with persistent ID {id:#x} (input.device): {}",
                    list(infos)
                )
            }),
        // A device that does not say it cannot capture is tried.
        Want::First => infos
            .iter()
            .position(|d| d.supports_capture != Some(false))
            .ok_or_else(|| format!("no device that can capture: {}", list(infos))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(name: &str, id: Option<i64>, capture: Option<bool>) -> DeviceInfo {
        DeviceInfo {
            display_name: name.into(),
            model_name: "DeckLink Quad HDMI Recorder".into(),
            persistent_id: id,
            supports_capture: capture,
            hdmi_input: Some(true),
            input_format_detection: Some(true),
            hdr_metadata: Some(true),
        }
    }

    /// A Quad's four inputs, whose order is not promised.
    fn quad() -> Vec<DeviceInfo> {
        vec![
            dev("Quad (1)", Some(0x10), Some(true)),
            dev("Quad (2)", Some(0x11), Some(true)),
            dev("Quad (3)", Some(0x12), Some(true)),
            dev("Quad (4)", None, Some(true)),
        ]
    }

    #[test]
    fn an_id_finds_its_input_wherever_it_is_listed() {
        let mut q = quad();
        assert_eq!(choose(&q, Want::Id(0x12)), Ok(2));
        q.reverse();
        assert_eq!(choose(&q, Want::Id(0x12)), Ok(1));
    }

    #[test]
    fn an_id_not_present_is_an_error_with_the_list_not_another_input() {
        let e = choose(&quad(), Want::Id(0x99)).unwrap_err();
        assert!(e.contains("0x99"), "{e}");
        assert!(
            e.contains("device 0: Quad (1)") && e.contains("persistent ID 0x11"),
            "{e}"
        );
        assert!(e.contains("persistent ID unreported"), "{e}");
    }

    #[test]
    fn without_an_id_the_first_that_can_capture() {
        let mut d = quad();
        d[0].supports_capture = Some(false);
        assert_eq!(choose(&d, Want::First), Ok(1));
        // One that does not say is tried.
        d[1].supports_capture = None;
        assert_eq!(choose(&d, Want::First), Ok(1));
        assert!(choose(&[], Want::First).unwrap_err().contains("no devices"));
    }

    #[test]
    fn the_one_run_index_wins_over_the_setting() {
        assert_eq!(want(Some(3), "0x10"), Ok(Want::Index(3)));
        assert_eq!(want(None, "0x10"), Ok(Want::Id(0x10)));
        assert_eq!(want(None, "1620413"), Ok(Want::Id(0x1620413)));
        assert_eq!(want(None, "  "), Ok(Want::First));
        assert!(want(None, "Quad 2").is_err());
        assert!(
            choose(&quad(), Want::Index(4))
                .unwrap_err()
                .contains("no device 4")
        );
    }
}
