//! Declarations for `shim/shim.h`. Hand-written; keep the two in step.
//!
//! The types compile on every platform so the conversions built on them can be
//! tested anywhere; only the functions need the shim, and so Linux.

use std::mem::{offset_of, size_of};

pub const UNKNOWN: i64 = -1;

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
// the build instead of corrupting memory.
const _: () = assert!(size_of::<DeviceInfo>() == 288);
const _: () = assert!(offset_of!(DeviceInfo, persistent_id) == 256);
const _: () = assert!(offset_of!(DeviceInfo, input_format_detection) == 280);

#[cfg(target_os = "linux")]
pub use self::linux::*;

#[cfg(target_os = "linux")]
mod linux {
    use super::DeviceInfo;
    use std::ffi::{c_char, c_int};

    /// Opaque `cf_dl_iterator`.
    #[repr(C)]
    pub struct Iterator {
        _opaque: [u8; 0],
    }

    /// Opaque `cf_dl_device`.
    #[repr(C)]
    pub struct Device {
        _opaque: [u8; 0],
    }

    unsafe extern "C" {
        pub fn cf_dl_api_version(buf: *mut c_char, len: usize) -> c_int;
        pub fn cf_dl_iterator_create() -> *mut Iterator;
        pub fn cf_dl_iterator_next(it: *mut Iterator) -> *mut Device;
        pub fn cf_dl_iterator_release(it: *mut Iterator);
        pub fn cf_dl_device_get_info(dev: *mut Device, out: *mut DeviceInfo);
        pub fn cf_dl_device_release(dev: *mut Device);
    }
}
