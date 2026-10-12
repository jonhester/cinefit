//! DeckLink capture, through a small C++ shim over Blackmagic's SDK.
//!
//! The SDK loads `libDeckLinkAPI.so`, which ships with the Desktop Video driver,
//! at run time, so everything builds and tests on any machine; without the
//! driver, calls fail with [`Error::DriverNotLoaded`].

mod capture;
mod ffi;
mod metadata;

pub use capture::{Capture, Frame, FrameInfo, Handler, InputFormat, PixelFormat, Pixels, Sampling};
pub use metadata::{Chromaticity, ColorInfo, Colorspace, Eotf, HdrMetadata};

use std::ffi::CStr;
use std::fmt;
use std::ptr::NonNull;

// Bits from the SDK's BMDVideoIOSupport, BMDVideoConnection and
// BMDDynamicRange (where SDR is 0: always implied).
const DEVICE_SUPPORTS_CAPTURE: i64 = 1 << 0;
const VIDEO_CONNECTION_HDMI: i64 = 1 << 1;
const DYNAMIC_RANGE_PQ: i64 = 1 << 29;
const DYNAMIC_RANGE_HLG: i64 = 1 << 30;

/// What the card's HDMI input EDID tells sources it accepts. SDR is always
/// accepted. Sources only send what the EDID they see allows, though a
/// splitter in between may present its own EDID instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EdidDynamicRange {
    pub pq: bool,
    pub hlg: bool,
}

impl fmt::Display for EdidDynamicRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SDR")?;
        if self.pq {
            f.write_str(", PQ")?;
        }
        if self.hlg {
            f.write_str(", HLG")?;
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum Error {
    /// `libDeckLinkAPI.so` could not be loaded, usually because the Desktop
    /// Video driver is not installed. `reason` is the loader's own message.
    DriverNotLoaded { reason: String },
    /// The driver loaded but did not report its version, so it cannot be
    /// checked against [`ApiVersion::REQUIRED`].
    VersionUnreported,
    /// The driver is older than the SDK headers this crate is built with, and
    /// lacks the entry points they call.
    DriverTooOld { found: ApiVersion },
    /// The device has no capture input.
    NoInput,
    /// Another program holds the capture input.
    InputBusy,
    /// An SDK call failed; `hresult` is its result code.
    Sdk { call: String, hresult: i32 },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::DriverNotLoaded { reason } => {
                write!(f, "DeckLink driver not loaded: {reason}")
            }
            Error::VersionUnreported => f.write_str("DeckLink driver did not report its version"),
            Error::DriverTooOld { found } => write!(
                f,
                "DeckLink driver {found} is too old: Desktop Video {} or newer is required",
                ApiVersion::REQUIRED
            ),
            Error::NoInput => f.write_str("DeckLink device has no capture input"),
            Error::InputBusy => f.write_str("capture input is in use by another program"),
            Error::Sdk { call, hresult } => {
                write!(
                    f,
                    "DeckLink {call} failed (HRESULT {:#010x})",
                    *hresult as u32
                )
            }
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
    pub input_edid: Option<EdidDynamicRange>,
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
            input_edid: int(raw.edid_dynamic_range).map(|b| EdidDynamicRange {
                pq: b & DYNAMIC_RANGE_PQ != 0,
                hlg: b & DYNAMIC_RANGE_HLG != 0,
            }),
        }
    }
}

/// The shim always NUL-terminates; a buffer without one reads as empty.
fn c_string(buf: &[u8]) -> String {
    CStr::from_bytes_until_nul(buf)
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The installed driver's API version.
///
/// # Errors
///
/// [`Error::DriverNotLoaded`] if the Desktop Video driver is not installed,
/// or [`Error::VersionUnreported`] if it does not say which version it is.
pub fn api_version() -> Result<ApiVersion, Error> {
    let mut raw = ffi::UNKNOWN;
    // Room for the loader's message, which on macOS lists every path tried.
    let mut err = [0u8; 2048];
    // SAFETY: both pointers are live; the shim writes one `i64` through the
    // first and at most `err.len()` bytes, NUL-terminated, through the second.
    let rc = unsafe { ffi::cf_dl_api_version(&mut raw, err.as_mut_ptr().cast(), err.len()) };
    if rc != 0 {
        return Err(Error::DriverNotLoaded {
            reason: c_string(&err),
        });
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

    // SAFETY: no preconditions. The driver loaded and is new enough, so null
    // means it lacks the entry point despite its version.
    let it = NonNull::new(unsafe { ffi::cf_dl_iterator_create() }).ok_or_else(|| {
        Error::DriverNotLoaded {
            reason: "libDeckLinkAPI.so has no device iterator entry point".into(),
        }
    })?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(name: &str) -> ffi::DeviceInfo {
        let mut display_name = [0u8; 128];
        display_name[..name.len()].copy_from_slice(name.as_bytes());
        ffi::DeviceInfo {
            display_name,
            ..ffi::DeviceInfo::UNREPORTED
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
        assert_eq!(info.input_edid, None);
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

    #[test]
    fn edid_dynamic_range_decodes() {
        let mut r = raw("card");
        r.edid_dynamic_range = DYNAMIC_RANGE_PQ;
        let e = DeviceInfo::from(&r).input_edid.expect("reported");
        assert_eq!(
            e,
            EdidDynamicRange {
                pq: true,
                hlg: false
            }
        );
        assert_eq!(e.to_string(), "SDR, PQ");
        r.edid_dynamic_range = 0;
        assert_eq!(
            DeviceInfo::from(&r)
                .input_edid
                .expect("reported")
                .to_string(),
            "SDR"
        );
        r.edid_dynamic_range = DYNAMIC_RANGE_PQ | DYNAMIC_RANGE_HLG;
        assert_eq!(
            DeviceInfo::from(&r)
                .input_edid
                .expect("reported")
                .to_string(),
            "SDR, PQ, HLG"
        );
    }
}
