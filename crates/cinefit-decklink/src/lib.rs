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
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::DriverNotLoaded => {
                f.write_str("DeckLink driver not loaded (is Desktop Video installed?)")
            }
        }
    }
}

impl std::error::Error for Error {}

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
    use super::{DeviceInfo, Error, c_string, ffi};
    use std::mem::MaybeUninit;
    use std::ptr::NonNull;

    /// The installed driver's API version, e.g. "16.4".
    ///
    /// # Errors
    ///
    /// [`Error::DriverNotLoaded`] if the Desktop Video driver is not installed.
    pub fn api_version() -> Result<String, Error> {
        let mut buf = [0u8; 64];
        // SAFETY: the shim writes at most `buf.len()` bytes, NUL-terminated.
        let rc = unsafe { ffi::cf_dl_api_version(buf.as_mut_ptr().cast(), buf.len()) };
        if rc != 0 {
            return Err(Error::DriverNotLoaded);
        }
        Ok(c_string(&buf))
    }

    /// Every DeckLink device the driver can see, in the driver's order.
    ///
    /// # Errors
    ///
    /// [`Error::DriverNotLoaded`] if the Desktop Video driver is not installed.
    /// No devices is not an error: the result is empty.
    pub fn devices() -> Result<Vec<Device>, Error> {
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

    struct Iter(NonNull<ffi::Iterator>);

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
    pub struct Device(NonNull<ffi::Device>);

    impl Device {
        /// What the device reports about itself; see [`DeviceInfo`].
        pub fn info(&self) -> DeviceInfo {
            let mut raw = MaybeUninit::<ffi::DeviceInfo>::uninit();
            // SAFETY: the device is live, and the shim initialises every byte
            // of `raw` (it zeroes the struct before filling it).
            let raw = unsafe {
                ffi::cf_dl_device_get_info(self.0.as_ptr(), raw.as_mut_ptr());
                raw.assume_init()
            };
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
    fn unterminated_string_reads_as_empty() {
        assert_eq!(c_string(&[b'x'; 4]), "");
    }
}
