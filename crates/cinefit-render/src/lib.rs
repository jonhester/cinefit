//! Rendering with libplacebo on Vulkan, through a small C shim.
//!
//! libplacebo is the pinned build from `scripts/build-libplacebo.sh` (API 375,
//! or the shim fails to build), and rendering is Linux only; elsewhere
//! [`Placebo::new`] reports that.

pub mod color;
mod ffi;
#[cfg(target_os = "linux")]
pub mod power;
pub mod v210;

pub use color::{Matrix, Primaries, RgbColor, Transfer, YcbcrColor};

use std::ffi::CStr;
#[cfg(target_os = "linux")]
use std::ffi::{c_char, c_int, c_void};
use std::fmt;

#[derive(Debug)]
pub enum Error {
    /// libplacebo or its Vulkan device could not be created.
    Create(String),
    /// A render failed; libplacebo's log says why.
    Render,
    /// The input does not match the size given.
    BadInput,
    /// Rendering is Linux only.
    Unsupported,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Create(why) => write!(f, "libplacebo: {why}"),
            Error::Render => f.write_str("libplacebo render failed (see its log)"),
            Error::BadInput => f.write_str("image size does not match its dimensions"),
            Error::Unsupported => f.write_str("rendering is only supported on Linux"),
        }
    }
}

impl std::error::Error for Error {}

/// What the GPU offers, as libplacebo sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    pub device_name: String,
    /// Vulkan API version as (major, minor, patch).
    pub vulkan_version: (u32, u32, u32),
    pub max_tex_2d_dim: u32,
    /// Whether textures can be imported from dma-bufs: what scanout buffers
    /// from GBM need.
    pub dma_buf_import: bool,
    /// The libplacebo API version the shim was built against.
    pub placebo_api: i32,
}

impl Info {
    // Used off Linux only by the tests.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    fn from_raw(raw: &ffi::Info, placebo_api: i32) -> Self {
        let v = raw.vulkan_api_version;
        Self {
            device_name: CStr::from_bytes_until_nul(&raw.device_name)
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            // VK_MAKE_API_VERSION: variant 3 bits, major 7, minor 10, patch 12.
            vulkan_version: ((v >> 22) & 0x7F, (v >> 12) & 0x3FF, v & 0xFFF),
            max_tex_2d_dim: raw.max_tex_2d_dim,
            dma_buf_import: raw.dma_buf_import != 0,
            placebo_api,
        }
    }
}

#[cfg(target_os = "linux")]
/// libplacebo's log, to stderr. Warnings and worse only; the shim sets that.
unsafe extern "C" fn log_to_stderr(_ctx: *mut c_void, level: c_int, msg: *const c_char) {
    // SAFETY: libplacebo passes a NUL-terminated message valid for the call.
    let msg = unsafe { CStr::from_ptr(msg) }.to_string_lossy();
    let level = match level {
        1 => "fatal",
        2 => "error",
        3 => "warning",
        _ => "info",
    };
    eprintln!("libplacebo {level}: {msg}");
}

/// libplacebo with its own Vulkan device and renderer. Freed on drop.
pub struct Placebo {
    #[cfg(target_os = "linux")]
    handle: std::ptr::NonNull<ffi::Handle>,
}

#[cfg(target_os = "linux")]
impl Placebo {
    /// Creates the Vulkan device and renderer.
    ///
    /// # Errors
    ///
    /// [`Error::Create`] if there is no usable Vulkan device.
    pub fn new() -> Result<Self, Error> {
        let mut err = [0u8; 512];
        // SAFETY: the callback has the shim's signature and ignores its
        // context; `err` is writable for its length.
        let handle = unsafe {
            ffi::cf_pl_create(
                log_to_stderr,
                std::ptr::null_mut(),
                err.as_mut_ptr().cast(),
                err.len(),
            )
        };
        std::ptr::NonNull::new(handle)
            .map(|handle| Self { handle })
            .ok_or_else(|| {
                Error::Create(
                    CStr::from_bytes_until_nul(&err)
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                )
            })
    }

    pub fn info(&self) -> Info {
        let mut raw = ffi::Info {
            device_name: [0; 256],
            vulkan_api_version: 0,
            max_tex_2d_dim: 0,
            dma_buf_import: 0,
        };
        // SAFETY: the handle is live and `raw` is a writable struct whose
        // layout the shim shares.
        let api = unsafe {
            ffi::cf_pl_get_info(self.handle.as_ptr(), &mut raw);
            ffi::cf_pl_api_version()
        };
        Info::from_raw(&raw, api)
    }

    /// Renders a `w` x `h` RGBA8 image, rows packed, through libplacebo's
    /// renderer into a target of the same size and format, without dithering,
    /// and returns what it wrote. An identity render: the output should match
    /// the input, which is what makes it a check of the whole GPU path.
    ///
    /// # Errors
    ///
    /// [`Error::BadInput`] if `rgba` is not `w * h * 4` bytes, or
    /// [`Error::Render`] if libplacebo fails.
    pub fn render_rgba8(&self, w: u32, h: u32, rgba: &[u8]) -> Result<Vec<u8>, Error> {
        let len = (w as usize) * (h as usize) * 4;
        if rgba.len() != len || w == 0 || h == 0 {
            return Err(Error::BadInput);
        }
        let mut out = vec![0u8; len];
        let (wi, hi) = (
            c_int::try_from(w).map_err(|_| Error::BadInput)?,
            c_int::try_from(h).map_err(|_| Error::BadInput)?,
        );
        // SAFETY: both buffers hold w * h * 4 bytes, as the shim reads and
        // writes; the handle is live.
        let rc = unsafe {
            ffi::cf_pl_render_rgba8(
                self.handle.as_ptr(),
                wi,
                hi,
                rgba.as_ptr(),
                out.as_mut_ptr(),
            )
        };
        if rc != 0 {
            return Err(Error::Render);
        }
        Ok(out)
    }
}

/// A dma-buf to render into, as a scanout buffer describes it.
#[derive(Debug, Clone, Copy)]
pub struct DmaBufTarget<'a> {
    pub fd: std::os::fd::BorrowedFd<'a>,
    pub width: u32,
    pub height: u32,
    /// DRM fourcc, e.g. XRGB8888.
    pub fourcc: u32,
    pub modifier: u64,
    pub stride: u32,
    pub offset: u32,
}

/// A texture owned by a [`Placebo`], destroyed on drop. The borrow keeps it
/// from outliving the device it belongs to.
#[cfg(target_os = "linux")]
pub struct Texture<'p> {
    pl: &'p Placebo,
    tex: std::ptr::NonNull<ffi::Tex>,
}

#[cfg(target_os = "linux")]
impl Drop for Texture<'_> {
    fn drop(&mut self) {
        // SAFETY: the texture belongs to this live device and is freed once.
        unsafe { ffi::cf_pl_tex_destroy(self.pl.handle.as_ptr(), self.tex.as_ptr()) }
    }
}

#[cfg(target_os = "linux")]
impl Placebo {
    /// Imports a dma-buf as a texture to render into. libplacebo duplicates
    /// the fd, so the caller keeps its own.
    ///
    /// # Errors
    ///
    /// [`Error::Render`] if libplacebo cannot import it (see its log).
    pub fn import_dmabuf(&self, t: &DmaBufTarget<'_>) -> Result<Texture<'_>, Error> {
        use std::os::fd::AsRawFd;
        let dim = |v: u32| c_int::try_from(v).map_err(|_| Error::BadInput);
        // SAFETY: the fd is valid for the call, and libplacebo duplicates it
        // rather than taking ownership; the handle is live.
        let tex = unsafe {
            ffi::cf_pl_import_dmabuf(
                self.handle.as_ptr(),
                t.fd.as_raw_fd(),
                dim(t.width)?,
                dim(t.height)?,
                t.fourcc,
                t.modifier,
                t.stride,
                t.offset,
            )
        };
        std::ptr::NonNull::new(tex)
            .map(|tex| Texture { pl: self, tex })
            .ok_or(Error::Render)
    }

    /// Uploads a `w` x `h` RGBA8 image, rows packed, as a texture to render
    /// from.
    ///
    /// # Errors
    ///
    /// [`Error::BadInput`] if the size is wrong, [`Error::Render`] if
    /// libplacebo fails.
    pub fn upload_rgba8(&self, w: u32, h: u32, rgba: &[u8]) -> Result<Texture<'_>, Error> {
        if rgba.len() != (w as usize) * (h as usize) * 4 || w == 0 || h == 0 {
            return Err(Error::BadInput);
        }
        let dim = |v: u32| c_int::try_from(v).map_err(|_| Error::BadInput);
        // SAFETY: `rgba` holds w * h * 4 bytes, which libplacebo copies.
        let tex = unsafe {
            ffi::cf_pl_upload_rgba8(self.handle.as_ptr(), dim(w)?, dim(h)?, rgba.as_ptr())
        };
        std::ptr::NonNull::new(tex)
            .map(|tex| Texture { pl: self, tex })
            .ok_or(Error::Render)
    }

    /// Renders `src` over the whole of `dst` and waits for the GPU to finish,
    /// so `dst` can go to the display. `shift_x` scrolls the source left by
    /// that many pixels, wrapping around; whole pixels keep it exact.
    ///
    /// # Errors
    ///
    /// [`Error::Render`] if libplacebo fails (see its log).
    pub fn render(&self, src: &Texture<'_>, dst: &Texture<'_>, shift_x: f32) -> Result<(), Error> {
        // SAFETY: both textures belong to this live device.
        let rc = unsafe {
            ffi::cf_pl_render(
                self.handle.as_ptr(),
                src.tex.as_ptr(),
                dst.tex.as_ptr(),
                shift_x,
            )
        };
        if rc == 0 { Ok(()) } else { Err(Error::Render) }
    }
}

/// Captured 4:2:2 YCbCr on the GPU, as planes libplacebo renders from: see
/// [`v210::Planes`] for the layout. Each sample is stored as its 10-bit code
/// shifted left by 6. Destroyed on drop; the borrow keeps it from outliving
/// its device.
#[cfg(target_os = "linux")]
pub struct GpuPlanes<'p> {
    pl: &'p Placebo,
    raw: std::ptr::NonNull<ffi::Planes>,
    width: usize,
    height: usize,
}

#[cfg(target_os = "linux")]
impl Drop for GpuPlanes<'_> {
    fn drop(&mut self) {
        // SAFETY: the planes belong to this live device and are freed once.
        unsafe { ffi::cf_pl_planes_destroy(self.pl.handle.as_ptr(), self.raw.as_ptr()) }
    }
}

#[cfg(target_os = "linux")]
impl GpuPlanes<'_> {
    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    /// Reads the planes back as stored (`code << 6`), for checks.
    ///
    /// # Errors
    ///
    /// [`Error::Render`] if libplacebo fails.
    pub fn download(&self) -> Result<v210::Planes, Error> {
        let mut p = v210::Planes {
            width: self.width,
            height: self.height,
            luma: vec![0; self.width * self.height],
            chroma: Vec::new(),
        };
        p.chroma = vec![0; p.chroma_width() * 2 * self.height];
        // SAFETY: the buffers hold the w * h luma and ceil(w/2) * h pairs the
        // shim writes; the planes belong to this live device.
        let rc = unsafe {
            ffi::cf_pl_planes_download(
                self.pl.handle.as_ptr(),
                self.raw.as_ptr(),
                p.luma.as_mut_ptr(),
                p.chroma.as_mut_ptr(),
            )
        };
        if rc == 0 { Ok(p) } else { Err(Error::Render) }
    }
}

/// An RGBA 16-bit texture to render into and read back, for checks.
#[cfg(target_os = "linux")]
pub struct Rgba16Target<'p> {
    tex: Texture<'p>,
    width: usize,
    height: usize,
}

#[cfg(target_os = "linux")]
impl<'p> Rgba16Target<'p> {
    pub fn texture(&self) -> &Texture<'p> {
        &self.tex
    }

    /// Reads it back: `width * height` RGBA values, rows packed.
    ///
    /// # Errors
    ///
    /// [`Error::Render`] if libplacebo fails.
    pub fn download(&self) -> Result<Vec<u16>, Error> {
        let mut out = vec![0u16; self.width * self.height * 4];
        // SAFETY: `out` holds the w * h * 4 values the shim writes; the
        // texture belongs to this live device.
        let rc = unsafe {
            ffi::cf_pl_download_rgba16(
                self.tex.pl.handle.as_ptr(),
                self.tex.tex.as_ptr(),
                out.as_mut_ptr(),
            )
        };
        if rc == 0 { Ok(out) } else { Err(Error::Render) }
    }
}

#[cfg(target_os = "linux")]
fn primaries_raw(p: Primaries) -> c_int {
    match p {
        Primaries::Bt709 => ffi::PRIM_BT709,
        Primaries::Bt2020 => ffi::PRIM_BT2020,
    }
}

#[cfg(target_os = "linux")]
fn transfer_raw(t: Transfer) -> c_int {
    match t {
        Transfer::Bt1886 => ffi::TRC_BT1886,
        Transfer::Pq => ffi::TRC_PQ,
        Transfer::Hlg => ffi::TRC_HLG,
    }
}

#[cfg(target_os = "linux")]
impl Placebo {
    /// Planes for `width` x `height` captured YCbCr.
    ///
    /// # Errors
    ///
    /// [`Error::BadInput`] for an empty or oversized picture, [`Error::Render`]
    /// if libplacebo cannot create them (see its log).
    pub fn planes(&self, width: usize, height: usize) -> Result<GpuPlanes<'_>, Error> {
        let dim = |v: usize| {
            c_int::try_from(v)
                .ok()
                .filter(|&v| v > 0)
                .ok_or(Error::BadInput)
        };
        // SAFETY: the handle is live.
        let raw =
            unsafe { ffi::cf_pl_planes_create(self.handle.as_ptr(), dim(width)?, dim(height)?) };
        std::ptr::NonNull::new(raw)
            .map(|raw| GpuPlanes {
                pl: self,
                raw,
                width,
                height,
            })
            .ok_or(Error::Render)
    }

    /// Uploads a `v210` frame the size of `planes` and unpacks it into them on
    /// the GPU. `row_bytes` is the card's stride.
    ///
    /// # Errors
    ///
    /// [`Error::BadInput`] if the stride is not whole 16-byte groups covering
    /// the width or `data` is short, [`Error::Render`] if libplacebo fails.
    pub fn unpack_v210(
        &self,
        planes: &mut GpuPlanes<'_>,
        data: &[u8],
        row_bytes: usize,
    ) -> Result<(), Error> {
        if !row_bytes.is_multiple_of(16)
            || row_bytes < v210::min_row_bytes(planes.width)
            || data.len() < row_bytes.saturating_mul(planes.height)
        {
            return Err(Error::BadInput);
        }
        // SAFETY: `data` holds row_bytes * height bytes, which the shim
        // uploads; the planes belong to this live device, and `&mut` keeps
        // anything else from using them meanwhile.
        let rc = unsafe {
            ffi::cf_pl_unpack_v210(
                self.handle.as_ptr(),
                planes.raw.as_ptr(),
                data.as_ptr(),
                row_bytes,
            )
        };
        if rc == 0 { Ok(()) } else { Err(Error::Render) }
    }

    /// Renders the planes over the whole of `dst` with their colour, into an
    /// RGB target with its own, and waits for the GPU. Chroma is co-sited, as
    /// HDMI 4:2:2 is. Between identical primaries and transfers this changes
    /// only the representation: no tone or gamut mapping.
    ///
    /// # Errors
    ///
    /// [`Error::Render`] if libplacebo fails (see its log).
    pub fn render_ycbcr(
        &self,
        src: &GpuPlanes<'_>,
        color: YcbcrColor,
        dst: &Texture<'_>,
        dst_color: RgbColor,
    ) -> Result<(), Error> {
        let matrix = match color.matrix {
            Matrix::Bt709 => ffi::MATRIX_BT709,
            Matrix::Bt2020Nc => ffi::MATRIX_BT2020_NC,
        };
        // SAFETY: the planes and the texture belong to this live device.
        let rc = unsafe {
            ffi::cf_pl_render_ycbcr(
                self.handle.as_ptr(),
                src.raw.as_ptr(),
                matrix,
                primaries_raw(color.primaries),
                transfer_raw(color.transfer),
                dst.tex.as_ptr(),
                primaries_raw(dst_color.primaries),
                transfer_raw(dst_color.transfer),
            )
        };
        if rc == 0 { Ok(()) } else { Err(Error::Render) }
    }

    /// A `width` x `height` RGBA 16-bit target that can be read back.
    ///
    /// # Errors
    ///
    /// [`Error::BadInput`] for an empty or oversized size, [`Error::Render`]
    /// if libplacebo fails.
    pub fn target_rgba16(&self, width: usize, height: usize) -> Result<Rgba16Target<'_>, Error> {
        let dim = |v: usize| {
            c_int::try_from(v)
                .ok()
                .filter(|&v| v > 0)
                .ok_or(Error::BadInput)
        };
        // SAFETY: the handle is live.
        let tex =
            unsafe { ffi::cf_pl_target_rgba16(self.handle.as_ptr(), dim(width)?, dim(height)?) };
        std::ptr::NonNull::new(tex)
            .map(|tex| Rgba16Target {
                tex: Texture { pl: self, tex },
                width,
                height,
            })
            .ok_or(Error::Render)
    }
}

#[cfg(target_os = "linux")]
impl Drop for Placebo {
    fn drop(&mut self) {
        // SAFETY: we own the handle and free it once.
        unsafe { ffi::cf_pl_destroy(self.handle.as_ptr()) }
    }
}

#[cfg(not(target_os = "linux"))]
impl Placebo {
    /// # Errors
    ///
    /// Always [`Error::Unsupported`]: rendering is Linux only.
    pub fn new() -> Result<Self, Error> {
        Err(Error::Unsupported)
    }

    pub fn info(&self) -> Info {
        unreachable!("no Placebo exists off Linux")
    }

    /// # Errors
    ///
    /// Never reached: no `Placebo` exists off Linux.
    pub fn render_rgba8(&self, _w: u32, _h: u32, _rgba: &[u8]) -> Result<Vec<u8>, Error> {
        Err(Error::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vulkan_version_decodes() {
        let mut name = [0u8; 256];
        name[..18].copy_from_slice(b"AMD Radeon RX 6400");
        let raw = ffi::Info {
            device_name: name,
            // 1.4.335 in VK_MAKE_API_VERSION's encoding.
            vulkan_api_version: (1 << 22) | (4 << 12) | 335,
            max_tex_2d_dim: 16384,
            dma_buf_import: 1,
        };
        let info = Info::from_raw(&raw, 360);
        assert_eq!(info.device_name, "AMD Radeon RX 6400");
        assert_eq!(info.vulkan_version, (1, 4, 335));
        assert!(info.dma_buf_import);
    }
}
