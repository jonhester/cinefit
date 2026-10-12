//! Declarations for `shim/placebo.h`. Hand-written; keep the two in step.

#[cfg(target_os = "linux")]
use std::ffi::{c_char, c_int, c_void};
use std::mem::{offset_of, size_of};

#[repr(C)]
pub struct Info {
    pub device_name: [u8; 256],
    pub vulkan_api_version: u32,
    pub max_tex_2d_dim: u32,
    pub dma_buf_import: i32,
}

const _: () = assert!(size_of::<Info>() == 268);
const _: () = assert!(offset_of!(Info, vulkan_api_version) == 256);
const _: () = assert!(offset_of!(Info, max_tex_2d_dim) == 260);
const _: () = assert!(offset_of!(Info, dma_buf_import) == 264);

/// Opaque `cf_pl`.
#[cfg(target_os = "linux")]
#[repr(C)]
pub struct Handle {
    _opaque: [u8; 0],
}

#[cfg(target_os = "linux")]
pub type LogCallback = unsafe extern "C" fn(ctx: *mut c_void, level: c_int, msg: *const c_char);

#[cfg(target_os = "linux")]
unsafe extern "C" {
    pub fn cf_pl_api_version() -> c_int;
    pub fn cf_pl_create(
        log_cb: LogCallback,
        ctx: *mut c_void,
        err: *mut c_char,
        err_len: usize,
    ) -> *mut Handle;
    pub fn cf_pl_destroy(pl: *mut Handle);
    pub fn cf_pl_get_info(pl: *mut Handle, out: *mut Info);
    pub fn cf_pl_render_rgba8(
        pl: *mut Handle,
        w: c_int,
        h: c_int,
        input: *const u8,
        out: *mut u8,
    ) -> c_int;
}
