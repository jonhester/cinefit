//! DeckLink capture, through a small C++ shim over Blackmagic's SDK.
//!
//! Talking to the card is Linux only: the SDK loads `libDeckLinkAPI.so`, which
//! ships with the Desktop Video driver, at run time. The types and conversions
//! here build everywhere so they can be tested on any machine.

mod ffi;

use std::ffi::CStr;
use std::fmt;

// Bits from the SDK's BMDVideoIOSupport and BMDVideoConnection.
const DEVICE_SUPPORTS_CAPTURE: i64 = 1 << 0;
const VIDEO_CONNECTION_HDMI: i64 = 1 << 1;

#[derive(Debug)]
pub enum Error {
    /// `libDeckLinkAPI.so` could not be loaded: the driver is not installed.
    DriverNotLoaded,
    /// The driver loaded but did not report its version, so it cannot be
    /// checked against [`ApiVersion::REQUIRED`].
    VersionUnreported,
    /// The driver is older than the SDK headers this crate is built with, and
    /// lacks the entry points they call.
    DriverTooOld { found: ApiVersion },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::DriverNotLoaded => {
                f.write_str("DeckLink driver not loaded (is Desktop Video installed?)")
            }
            Error::VersionUnreported => f.write_str("DeckLink driver did not report its version"),
            Error::DriverTooOld { found } => write!(
                f,
                "DeckLink driver {found} is too old: Desktop Video {} or newer is required",
                ApiVersion::REQUIRED
            ),
        }
    }
}

impl std::error::Error for Error {}

/// A driver API version, e.g. 16.4.
///
/// Fields are in significance order, so the derived ordering compares versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ApiVersion {
    pub major: u8,
    pub minor: u8,
    pub patch: u8,
}

impl ApiVersion {
    /// The oldest driver the vendored SDK headers work with.
    pub const REQUIRED: Self = Self::from_raw(ffi::REQUIRED_API_VERSION);

    /// Decodes the SDK's 0xMMmmpp00 encoding.
    const fn from_raw(raw: i64) -> Self {
        Self {
            major: (raw >> 24) as u8,
            minor: (raw >> 16) as u8,
            patch: (raw >> 8) as u8,
        }
    }
}

impl fmt::Display for ApiVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)?;
        if self.patch != 0 {
            write!(f, ".{}", self.patch)?;
        }
        Ok(())
    }
}

/// What a device says about itself. `None` means the device did not report the
/// attribute, which is not the same as reporting `false`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub display_name: String,
    pub model_name: String,
    pub persistent_id: Option<i64>,
    pub supports_capture: Option<bool>,
    pub hdmi_input: Option<bool>,
    pub input_format_detection: Option<bool>,
    pub hdr_metadata: Option<bool>,
}

impl From<&ffi::DeviceInfo> for DeviceInfo {
    fn from(raw: &ffi::DeviceInfo) -> Self {
        let int = |v: i64| (v != ffi::UNKNOWN).then_some(v);
        let flag = |v: i32| (i64::from(v) != ffi::UNKNOWN).then_some(v != 0);
        Self {
            display_name: c_string(&raw.display_name),
            model_name: c_string(&raw.model_name),
            persistent_id: int(raw.persistent_id),
            supports_capture: int(raw.video_io_support).map(|b| b & DEVICE_SUPPORTS_CAPTURE != 0),
            hdmi_input: int(raw.input_connections).map(|b| b & VIDEO_CONNECTION_HDMI != 0),
            input_format_detection: flag(raw.input_format_detection),
            hdr_metadata: flag(raw.hdr_metadata),
        }
    }
}

/// The shim always NUL-terminates; a buffer without one reads as empty.
fn c_string(buf: &[u8]) -> String {
    CStr::from_bytes_until_nul(buf)
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(target_os = "linux")]
pub use self::linux::*;

#[cfg(target_os = "linux")]
mod linux {
    use super::{ApiVersion, DeviceInfo, Error, ffi};
    use std::ptr::NonNull;

    /// The installed driver's API version.
    ///
    /// # Errors
    ///
    /// [`Error::DriverNotLoaded`] if the Desktop Video driver is not installed,
    /// or [`Error::VersionUnreported`] if it does not say which version it is.
    pub fn api_version() -> Result<ApiVersion, Error> {
        let mut raw = ffi::UNKNOWN;
        // SAFETY: the shim writes one `i64` through the pointer, which is live.
        let rc = unsafe { ffi::cf_dl_api_version(&mut raw) };
        if rc != 0 {
            return Err(Error::DriverNotLoaded);
        }
        if raw == ffi::UNKNOWN {
            return Err(Error::VersionUnreported);
        }
        Ok(ApiVersion::from_raw(raw))
    }

    /// Every DeckLink device the driver can see, in the driver's order.
    ///
    /// # Errors
    ///
    /// Any error from [`api_version`], or [`Error::DriverTooOld`] if the driver
    /// predates [`ApiVersion::REQUIRED`]. No devices is not an error: the
    /// result is empty.
    pub fn devices() -> Result<Vec<Device>, Error> {
        // An old driver lacks the iterator entry point, and the SDK reports
        // that the same way as a missing driver, so check first.
        let found = api_version()?;
        if found < ApiVersion::REQUIRED {
            return Err(Error::DriverTooOld { found });
        }

        // SAFETY: no preconditions; null means the driver is not loaded.
        let it =
            NonNull::new(unsafe { ffi::cf_dl_iterator_create() }).ok_or(Error::DriverNotLoaded)?;
        let it = Iter(it);

        let mut out = Vec::new();
        // SAFETY: `it` is a live iterator; each non-null result carries a
        // reference that `Device` takes ownership of.
        while let Some(dev) = NonNull::new(unsafe { ffi::cf_dl_iterator_next(it.0.as_ptr()) }) {
            out.push(Device(dev));
        }
        Ok(out)
    }

    struct Iter(NonNull<ffi::IteratorHandle>);

    impl Drop for Iter {
        fn drop(&mut self) {
            // SAFETY: we hold the only reference this wrapper took.
            unsafe { ffi::cf_dl_iterator_release(self.0.as_ptr()) }
        }
    }

    /// One DeckLink device, from [`devices`]. Holds an SDK reference, released
    /// on drop.
    ///
    /// Not `Send` or `Sync` for now: nothing needs a device on another thread
    /// yet, and the SDK's threading rules for it are not established here.
    #[derive(Debug)]
    pub struct Device(NonNull<ffi::DeviceHandle>);

    impl Device {
        /// What the device reports about itself; see [`DeviceInfo`].
        pub fn info(&self) -> DeviceInfo {
            let mut raw = ffi::DeviceInfo::UNREPORTED;
            // SAFETY: the device is live, and `raw` is a valid, writable struct
            // whose layout the shim shares.
            unsafe { ffi::cf_dl_device_get_info(self.0.as_ptr(), &mut raw) };
            DeviceInfo::from(&raw)
        }
    }

    impl Drop for Device {
        fn drop(&mut self) {
            // SAFETY: we hold the only reference this wrapper took.
            unsafe { ffi::cf_dl_device_release(self.0.as_ptr()) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(name: &str) -> ffi::DeviceInfo {
        let mut display_name = [0u8; 128];
        display_name[..name.len()].copy_from_slice(name.as_bytes());
        ffi::DeviceInfo {
            display_name,
            model_name: [0; 128],
            persistent_id: ffi::UNKNOWN,
            video_io_support: ffi::UNKNOWN,
            input_connections: ffi::UNKNOWN,
            input_format_detection: ffi::UNKNOWN as i32,
            hdr_metadata: ffi::UNKNOWN as i32,
        }
    }

    #[test]
    fn unreported_attributes_are_none_not_false() {
        let info = DeviceInfo::from(&raw("card"));
        assert_eq!(info.display_name, "card");
        assert_eq!(info.persistent_id, None);
        assert_eq!(info.supports_capture, None);
        assert_eq!(info.hdmi_input, None);
        assert_eq!(info.input_format_detection, None);
        assert_eq!(info.hdr_metadata, None);
    }

    #[test]
    fn reported_attributes_decode() {
        let mut r = raw("card");
        r.persistent_id = 0x1620413;
        r.video_io_support = DEVICE_SUPPORTS_CAPTURE;
        r.input_connections = VIDEO_CONNECTION_HDMI;
        r.input_format_detection = 1;
        r.hdr_metadata = 0;
        let info = DeviceInfo::from(&r);
        assert_eq!(info.persistent_id, Some(0x1620413));
        assert_eq!(info.supports_capture, Some(true));
        assert_eq!(info.hdmi_input, Some(true));
        assert_eq!(info.input_format_detection, Some(true));
        assert_eq!(info.hdr_metadata, Some(false));
    }

    #[test]
    fn missing_bits_read_as_false() {
        let mut r = raw("card");
        r.video_io_support = 0;
        r.input_connections = 1; // SDI only
        let info = DeviceInfo::from(&r);
        assert_eq!(info.supports_capture, Some(false));
        assert_eq!(info.hdmi_input, Some(false));
    }

    #[test]
    fn api_version_decodes_sdk_encoding() {
        // What Desktop Video 16.4 reports.
        let v = ApiVersion::from_raw(0x1004_0000);
        assert_eq!((v.major, v.minor, v.patch), (16, 4, 0));
        assert_eq!(v.to_string(), "16.4");
        assert_eq!(ApiVersion::from_raw(0x0f03_0100).to_string(), "15.3.1");
        assert_eq!(ApiVersion::REQUIRED.to_string(), "16.0");
    }

    #[test]
    fn api_versions_order_by_significance() {
        let v = |major, minor, patch| ApiVersion {
            major,
            minor,
            patch,
        };
        assert!(v(15, 3, 1) < ApiVersion::REQUIRED);
        assert!(v(15, 9, 9) < v(16, 0, 0));
        assert!(v(16, 0, 1) > ApiVersion::REQUIRED);
        assert!(v(16, 4, 0) > v(16, 0, 9));
    }

    #[test]
    fn unterminated_string_reads_as_empty() {
        assert_eq!(c_string(&[b'x'; 4]), "");
    }
}
