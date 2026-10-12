//! Declarations for `shim/shim.h`. Hand-written; keep the two in step.

use std::ffi::{c_char, c_int, c_void};
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

/// `cf_dl_format`: the input's format, as detected when the signal changes.
#[repr(C)]
pub struct Format {
    pub frame_duration: i64,
    pub time_scale: i64,
    pub width: i32,
    pub height: i32,
    pub display_mode: u32,
    pub detected_flags: u32,
    pub enabled_mode: u32,
    pub enabled_pixel_format: u32,
}

const _: () = assert!(size_of::<Format>() == 40);
const _: () = assert!(offset_of!(Format, frame_duration) == 0);
const _: () = assert!(offset_of!(Format, time_scale) == 8);
const _: () = assert!(offset_of!(Format, width) == 16);
const _: () = assert!(offset_of!(Format, height) == 20);
const _: () = assert!(offset_of!(Format, display_mode) == 24);
const _: () = assert!(offset_of!(Format, detected_flags) == 28);
const _: () = assert!(offset_of!(Format, enabled_mode) == 32);
const _: () = assert!(offset_of!(Format, enabled_pixel_format) == 36);

/// `cf_dl_frame_info`: what the shim reads off a frame as it arrives.
#[repr(C)]
pub struct FrameInfo {
    pub stream_time: i64,
    pub stream_duration: i64,
    pub row_bytes: i64,
    pub width: i32,
    pub height: i32,
    pub pixel_format: u32,
    pub flags: u32,
    pub colorspace: i64,
    pub eotf: i64,
    pub red_x: f64,
    pub red_y: f64,
    pub green_x: f64,
    pub green_y: f64,
    pub blue_x: f64,
    pub blue_y: f64,
    pub white_x: f64,
    pub white_y: f64,
    pub max_mastering: f64,
    pub min_mastering: f64,
    pub max_cll: f64,
    pub max_fall: f64,
}

const _: () = assert!(size_of::<FrameInfo>() == 152);
const _: () = assert!(offset_of!(FrameInfo, stream_time) == 0);
const _: () = assert!(offset_of!(FrameInfo, stream_duration) == 8);
const _: () = assert!(offset_of!(FrameInfo, row_bytes) == 16);
const _: () = assert!(offset_of!(FrameInfo, width) == 24);
const _: () = assert!(offset_of!(FrameInfo, height) == 28);
const _: () = assert!(offset_of!(FrameInfo, pixel_format) == 32);
const _: () = assert!(offset_of!(FrameInfo, flags) == 36);
const _: () = assert!(offset_of!(FrameInfo, colorspace) == 40);
const _: () = assert!(offset_of!(FrameInfo, eotf) == 48);
const _: () = assert!(offset_of!(FrameInfo, red_x) == 56);
const _: () = assert!(offset_of!(FrameInfo, red_y) == 64);
const _: () = assert!(offset_of!(FrameInfo, green_x) == 72);
const _: () = assert!(offset_of!(FrameInfo, green_y) == 80);
const _: () = assert!(offset_of!(FrameInfo, blue_x) == 88);
const _: () = assert!(offset_of!(FrameInfo, blue_y) == 96);
const _: () = assert!(offset_of!(FrameInfo, white_x) == 104);
const _: () = assert!(offset_of!(FrameInfo, white_y) == 112);
const _: () = assert!(offset_of!(FrameInfo, max_mastering) == 120);
const _: () = assert!(offset_of!(FrameInfo, min_mastering) == 128);
const _: () = assert!(offset_of!(FrameInfo, max_cll) == 136);
const _: () = assert!(offset_of!(FrameInfo, max_fall) == 144);

/// Opaque `cf_dl_input`.
#[repr(C)]
pub struct InputHandle {
    _opaque: [u8; 0],
}

/// Opaque `cf_dl_frame`.
#[repr(C)]
pub struct FrameHandle {
    _opaque: [u8; 0],
}

/// Opaque `cf_dl_buffer`.
#[repr(C)]
pub struct BufferHandle {
    _opaque: [u8; 0],
}

/// `cf_dl_format_cb`.
pub type FormatCallback = unsafe extern "C" fn(ctx: *mut c_void, format: *const Format) -> u32;
/// `cf_dl_frame_cb`.
pub type FrameCallback =
    unsafe extern "C" fn(ctx: *mut c_void, frame: *mut FrameHandle, info: *const FrameInfo);

unsafe extern "C" {
    pub fn cf_dl_api_version(version: *mut i64, err: *mut c_char, err_len: usize) -> c_int;
    pub fn cf_dl_iterator_create() -> *mut IteratorHandle;
    pub fn cf_dl_iterator_next(it: *mut IteratorHandle) -> *mut DeviceHandle;
    pub fn cf_dl_iterator_release(it: *mut IteratorHandle);
    pub fn cf_dl_device_get_info(dev: *mut DeviceHandle, out: *mut DeviceInfo);
    pub fn cf_dl_device_release(dev: *mut DeviceHandle);

    pub fn cf_dl_input_open(dev: *mut DeviceHandle) -> *mut InputHandle;
    pub fn cf_dl_input_start(
        input: *mut InputHandle,
        on_format: FormatCallback,
        on_frame: FrameCallback,
        ctx: *mut c_void,
        failed_call: *mut *const c_char,
    ) -> i32;
    pub fn cf_dl_input_failed_restarts(input: *mut InputHandle) -> u64;
    pub fn cf_dl_input_release(input: *mut InputHandle);
    pub fn cf_dl_frame_release(frame: *mut FrameHandle);
    pub fn cf_dl_frame_read(
        frame: *mut FrameHandle,
        data: *mut *const u8,
        size: *mut u64,
    ) -> *mut BufferHandle;
    pub fn cf_dl_buffer_end(buf: *mut BufferHandle);
}
