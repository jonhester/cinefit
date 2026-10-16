//! Where cinefit keeps its files: the settings, the run logs and the
//! detector's recordings.
//!
//! Installed as a package, cinefit runs as a system user whose unit declares
//! `ConfigurationDirectory=`, `StateDirectory=` and `LogsDirectory=`: systemd
//! creates `/etc/cinefit`, `/var/lib/cinefit` and `/var/log/cinefit`, owned by
//! that user, and hands their paths over in `$CONFIGURATION_DIRECTORY`,
//! `$STATE_DIRECTORY` and `$LOGS_DIRECTORY`. Anywhere else (a run by hand, a
//! service started from `~/cinefit-app`) those are unset and everything stays
//! in the home directory, as before.
//!
//! One exception: on a machine with the package installed, a command typed
//! by hand (`cinefit settings`) reads the service's settings in
//! `/etc/cinefit`, not a file of its own in the home directory that the
//! service never sees.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The package's settings directory, which `ConfigurationDirectory=cinefit`
/// makes.
const SYSTEM_CONFIG: &str = "/etc/cinefit";

/// The settings file.
pub fn config_file() -> PathBuf {
    config_file_from(&var, Path::new(SYSTEM_CONFIG).is_dir())
}

/// The directory run logs go in.
pub fn log_dir() -> Option<PathBuf> {
    log_dir_from(&var)
}

/// The directory the detector's sessions go under (in `caught/`).
pub fn clips_root() -> Option<PathBuf> {
    clips_root_from(&var)
}

fn var(name: &str) -> Option<OsString> {
    std::env::var_os(name)
}

/// A directory systemd handed over in `name`. A unit that declares several
/// directories of one kind gets them colon-separated; cinefit declares one,
/// and takes the first.
fn systemd_dir(var: &dyn Fn(&str) -> Option<OsString>, name: &str) -> Option<PathBuf> {
    let v = var(name)?;
    let first = v.to_str().map_or(v.clone(), |s| {
        OsString::from(s.split(':').next().unwrap_or_default())
    });
    (!first.is_empty()).then(|| PathBuf::from(first))
}

fn home(var: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    var("HOME").filter(|h| !h.is_empty()).map(PathBuf::from)
}

fn config_file_from(var: &dyn Fn(&str) -> Option<OsString>, system: bool) -> PathBuf {
    if let Some(dir) = systemd_dir(var, "CONFIGURATION_DIRECTORY") {
        return dir.join("cinefit.toml");
    }
    if system {
        return Path::new(SYSTEM_CONFIG).join("cinefit.toml");
    }
    var("XDG_CONFIG_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| home(var).map(|h| h.join(".config")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("cinefit-rs")
        .join("cinefit.toml")
}

fn log_dir_from(var: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    systemd_dir(var, "LOGS_DIRECTORY").or_else(|| home(var).map(|h| h.join("cinefit-logs")))
}

fn clips_root_from(var: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    systemd_dir(var, "STATE_DIRECTORY").or_else(|| home(var).map(|h| h.join("cinefit-clips")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<OsString> {
        move |name| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| OsString::from(v))
        }
    }

    #[test]
    fn by_hand_everything_is_in_the_home_directory() {
        let e = env(&[("HOME", "/home/u")]);
        assert_eq!(
            config_file_from(&e, false),
            PathBuf::from("/home/u/.config/cinefit-rs/cinefit.toml")
        );
        assert_eq!(
            log_dir_from(&e),
            Some(PathBuf::from("/home/u/cinefit-logs"))
        );
        assert_eq!(
            clips_root_from(&e),
            Some(PathBuf::from("/home/u/cinefit-clips"))
        );
    }

    #[test]
    fn xdg_config_home_still_moves_the_settings() {
        let e = env(&[("HOME", "/home/u"), ("XDG_CONFIG_HOME", "/cfg")]);
        assert_eq!(
            config_file_from(&e, false),
            PathBuf::from("/cfg/cinefit-rs/cinefit.toml")
        );
    }

    #[test]
    fn under_the_package_unit_systemd_says_where() {
        let e = env(&[
            ("HOME", "/"),
            ("CONFIGURATION_DIRECTORY", "/etc/cinefit"),
            ("STATE_DIRECTORY", "/var/lib/cinefit"),
            ("LOGS_DIRECTORY", "/var/log/cinefit"),
        ]);
        assert_eq!(
            config_file_from(&e, true),
            PathBuf::from("/etc/cinefit/cinefit.toml")
        );
        assert_eq!(log_dir_from(&e), Some(PathBuf::from("/var/log/cinefit")));
        assert_eq!(clips_root_from(&e), Some(PathBuf::from("/var/lib/cinefit")));
    }

    #[test]
    fn several_directories_of_a_kind_take_the_first() {
        let e = env(&[("STATE_DIRECTORY", "/var/lib/cinefit:/var/lib/other")]);
        assert_eq!(clips_root_from(&e), Some(PathBuf::from("/var/lib/cinefit")));
    }

    /// With the package installed, a command typed by a user reads the
    /// service's settings, not a home file the service never sees.
    #[test]
    fn by_hand_on_a_packaged_machine_the_settings_are_the_services() {
        let e = env(&[("HOME", "/home/u")]);
        assert_eq!(
            config_file_from(&e, true),
            PathBuf::from("/etc/cinefit/cinefit.toml")
        );
        // Logs and recordings of a run by hand stay the user's own.
        assert_eq!(
            log_dir_from(&e),
            Some(PathBuf::from("/home/u/cinefit-logs"))
        );
    }

    #[test]
    fn empty_values_count_as_unset() {
        let e = env(&[
            ("HOME", "/home/u"),
            ("LOGS_DIRECTORY", ""),
            ("XDG_CONFIG_HOME", ""),
        ]);
        assert_eq!(
            log_dir_from(&e),
            Some(PathBuf::from("/home/u/cinefit-logs"))
        );
        assert_eq!(
            config_file_from(&e, false),
            PathBuf::from("/home/u/.config/cinefit-rs/cinefit.toml")
        );
        assert_eq!(log_dir_from(&env(&[])), None);
    }
}
