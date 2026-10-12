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

/// Opaque `cf_pl_tex`.
#[cfg(target_os = "linux")]
#[repr(C)]
pub struct Tex {
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
    pub fn cf_pl_import_dmabuf(
        pl: *mut Handle,
        fd: c_int,
        w: c_int,
        h: c_int,
        fourcc: u32,
        modifier: u64,
        stride: u32,
        offset: u32,
    ) -> *mut Tex;
    pub fn cf_pl_upload_rgba8(pl: *mut Handle, w: c_int, h: c_int, rgba: *const u8) -> *mut Tex;
    pub fn cf_pl_tex_destroy(pl: *mut Handle, tex: *mut Tex);
    pub fn cf_pl_render(pl: *mut Handle, src: *mut Tex, dst: *mut Tex, shift_x: f32) -> c_int;
}
