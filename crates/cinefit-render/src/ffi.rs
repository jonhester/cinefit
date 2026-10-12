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

/// `cf_pl_dovi`.
#[repr(C)]
pub struct Dovi {
    pub ycc_to_lms: [f32; 9],
    pub offset: [f32; 3],
    pub linear: [f32; 9],
}

const _: () = assert!(size_of::<Dovi>() == 84);
const _: () = assert!(offset_of!(Dovi, offset) == 36);
const _: () = assert!(offset_of!(Dovi, linear) == 48);

/// Opaque `cf_pl`.
#[cfg(target_os = "linux")]
#[repr(C)]
pub struct Handle {
    _opaque: [u8; 0],
}

/// Opaque `cf_pl_planes`.
#[cfg(target_os = "linux")]
#[repr(C)]
pub struct Planes {
    _opaque: [u8; 0],
}

/// Opaque `cf_pl_rgb`.
#[cfg(target_os = "linux")]
#[repr(C)]
pub struct Rgb {
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
    pub fn cf_pl_planes_create(pl: *mut Handle, w: c_int, h: c_int) -> *mut Planes;
    pub fn cf_pl_planes_destroy(pl: *mut Handle, planes: *mut Planes);
    pub fn cf_pl_unpack_v210(
        pl: *mut Handle,
        planes: *mut Planes,
        data: *const u8,
        row_bytes: usize,
    ) -> c_int;
    pub fn cf_pl_planes_download(
        pl: *mut Handle,
        planes: *mut Planes,
        luma: *mut u16,
        chroma: *mut u16,
    ) -> c_int;
    pub fn cf_pl_render_ycbcr(
        pl: *mut Handle,
        src: *mut Planes,
        matrix: c_int,
        primaries: c_int,
        trc: c_int,
        dst: *mut Tex,
        dst_primaries: c_int,
        dst_trc: c_int,
        place: *const f32,
    ) -> c_int;
    pub fn cf_pl_unpack_tunnel(
        pl: *mut Handle,
        planes: *mut Planes,
        data: *const u8,
        row_bytes: usize,
    ) -> c_int;
    pub fn cf_pl_render_dovi(
        pl: *mut Handle,
        src: *mut Planes,
        dovi: *const Dovi,
        dst: *mut Tex,
        dst_primaries: c_int,
        dst_trc: c_int,
        place: *const f32,
    ) -> c_int;
    pub fn cf_pl_rgb_create(pl: *mut Handle, w: c_int, h: c_int) -> *mut Rgb;
    pub fn cf_pl_rgb_destroy(pl: *mut Handle, rgb: *mut Rgb);
    pub fn cf_pl_unpack_rgb(
        pl: *mut Handle,
        rgb: *mut Rgb,
        data: *const u8,
        row_bytes: usize,
        format: c_int,
    ) -> c_int;
    pub fn cf_pl_rgb_download(pl: *mut Handle, rgb: *mut Rgb, out: *mut u16) -> c_int;
    pub fn cf_pl_render_rgb(
        pl: *mut Handle,
        src: *mut Rgb,
        levels: c_int,
        primaries: c_int,
        trc: c_int,
        dst: *mut Tex,
        dst_primaries: c_int,
        dst_trc: c_int,
        place: *const f32,
    ) -> c_int;
    pub fn cf_pl_target_rgba16(pl: *mut Handle, w: c_int, h: c_int) -> *mut Tex;
    pub fn cf_pl_download_rgba16(pl: *mut Handle, tex: *mut Tex, out: *mut u16) -> c_int;
}

// The colour constants in `shim/placebo.h`.
#[cfg(target_os = "linux")]
mod color {
    pub const MATRIX_BT709: i32 = 0;
    pub const MATRIX_BT2020_NC: i32 = 1;
    pub const PRIM_BT709: i32 = 0;
    pub const PRIM_BT2020: i32 = 1;
    pub const TRC_BT1886: i32 = 0;
    pub const TRC_PQ: i32 = 1;
    pub const TRC_HLG: i32 = 2;
    pub const R210: i32 = 0;
    pub const ARGB8: i32 = 1;
    pub const LEVELS_FULL: i32 = 0;
    pub const LEVELS_LIMITED: i32 = 1;
}
#[cfg(target_os = "linux")]
pub use color::*;
