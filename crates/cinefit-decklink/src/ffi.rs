//! Declarations for `shim/shim.h`. Hand-written; keep the two in step.
//!
//! The types compile on every platform so the conversions built on them can be
//! tested anywhere; only the functions need the shim, and so Linux.

use std::mem::{offset_of, size_of};

pub const UNKNOWN: i64 = -1;

/// The oldest driver API version the vendored headers work with, as the SDK
/// encodes it (0xMMmmpp00). shim.cpp asserts it against the headers.
pub const REQUIRED_API_VERSION: i64 = 0x1000_0000;

#[repr(C)]
pub struct DeviceInfo {
    pub display_name: [u8; 128],
    pub model_name: [u8; 128],
    pub persistent_id: i64,
    pub video_io_support: i64,
    pub input_connections: i64,
    pub input_format_detection: i32,
    pub hdr_metadata: i32,
}

// shim.cpp asserts the same numbers, so a field changed on one side only fails
// the build instead of corrupting memory. Every field is checked: two fields of
// the same type swapped on one side keep the size but read each other's values.
const _: () = assert!(size_of::<DeviceInfo>() == 288);
const _: () = assert!(offset_of!(DeviceInfo, display_name) == 0);
const _: () = assert!(offset_of!(DeviceInfo, model_name) == 128);
const _: () = assert!(offset_of!(DeviceInfo, persistent_id) == 256);
const _: () = assert!(offset_of!(DeviceInfo, video_io_support) == 264);
const _: () = assert!(offset_of!(DeviceInfo, input_connections) == 272);
const _: () = assert!(offset_of!(DeviceInfo, input_format_detection) == 280);
const _: () = assert!(offset_of!(DeviceInfo, hdr_metadata) == 284);

#[cfg(target_os = "linux")]
pub use self::linux::*;

#[cfg(target_os = "linux")]
mod linux {
    use super::{DeviceInfo, UNKNOWN};
    use std::ffi::c_int;

    impl DeviceInfo {
        /// Nothing reported: a safe starting value for the shim to fill. If the
        /// shim ever skips a field, it reads as unreported, not as "no".
        pub const UNREPORTED: Self = Self {
            display_name: [0; 128],
            model_name: [0; 128],
            persistent_id: UNKNOWN,
            video_io_support: UNKNOWN,
            input_connections: UNKNOWN,
            input_format_detection: UNKNOWN as i32,
            hdr_metadata: UNKNOWN as i32,
        };
    }

    /// Opaque `cf_dl_iterator`.
    #[repr(C)]
    pub struct IteratorHandle {
        _opaque: [u8; 0],
    }

    /// Opaque `cf_dl_device`.
    #[repr(C)]
    pub struct DeviceHandle {
        _opaque: [u8; 0],
    }

    unsafe extern "C" {
        pub fn cf_dl_api_version(version: *mut i64) -> c_int;
        pub fn cf_dl_iterator_create() -> *mut IteratorHandle;
        pub fn cf_dl_iterator_next(it: *mut IteratorHandle) -> *mut DeviceHandle;
        pub fn cf_dl_iterator_release(it: *mut IteratorHandle);
        pub fn cf_dl_device_get_info(dev: *mut DeviceHandle, out: *mut DeviceInfo);
        pub fn cf_dl_device_release(dev: *mut DeviceHandle);
    }
}
