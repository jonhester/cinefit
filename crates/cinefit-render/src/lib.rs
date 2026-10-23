//! Rendering with libplacebo on Vulkan, through a small C shim.
//!
//! libplacebo is the pinned build from `scripts/build-libplacebo.sh` (API 375,
//! or the shim fails to build), and rendering is Linux only; elsewhere
//! [`Placebo::new`] reports that.

pub mod color;
pub mod dm;
mod ffi;
#[cfg(target_os = "linux")]
pub mod power;
pub mod rgb;
pub mod tunnel;
pub mod v210;

pub use color::{
    Curve, DoviColor, Levels, Mapping, Matrix, Primaries, RgbColor, RgbSourceColor, Transfer,
    YcbcrColor,
};

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

/// Where a picture goes in its target, in the target's pixels: `x0, y0` to
/// `x1, y1`. A render with `None` fills the target.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Rect {
    /// The largest rectangle of a `width` x `height` picture's shape that fits
    /// a `target_w` x `target_h` target, centred: bars at the sides or at the
    /// top and bottom, never a stretch.
    pub fn fit(width: u32, height: u32, target_w: u32, target_h: u32) -> Self {
        let scale = f64::min(
            f64::from(target_w) / f64::from(width.max(1)),
            f64::from(target_h) / f64::from(height.max(1)),
        );
        let (w, h) = (f64::from(width) * scale, f64::from(height) * scale);
        let (x0, y0) = (
            (f64::from(target_w) - w) / 2.0,
            (f64::from(target_h) - h) / 2.0,
        );
        Self {
            x0: x0 as f32,
            y0: y0 as f32,
            x1: (x0 + w) as f32,
            y1: (y0 + h) as f32,
        }
    }

    #[cfg(target_os = "linux")]
    fn raw(place: Option<&Self>) -> Option<[f32; 4]> {
        place.map(|r| [r.x0, r.y0, r.x1, r.y1])
    }
}

/// All of a frame's geometry: which part of the source is drawn (`src`, in
/// source pixels; `None` for the whole frame) and where it lands in the
/// target (`dst`, in target pixels; `None` for the whole of it). Crop
/// rectangles and nothing else, since a geometry change that commits is a
/// bug.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct View {
    pub src: Option<Rect>,
    pub dst: Option<Rect>,
}

impl View {
    /// The whole frame, placed at `dst`.
    pub fn placed(dst: Option<Rect>) -> Self {
        View { src: None, dst }
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
/// [`v210::Planes`] for the layout. Each sample is stored as its code shifted
/// to the top of 16 bits: v210's 10 bits by 6, a tunnel's 12 by 4. Destroyed on drop; the borrow keeps it from outliving
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

    /// Reads the planes back as stored (`code << 6` for v210, `code << 4`
    /// for a tunnel), for checks.
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

/// Captured RGB on the GPU, one 16-bit texture with each code shifted to the
/// top bits. Destroyed on drop; the borrow keeps it from outliving its device.
#[cfg(target_os = "linux")]
pub struct GpuRgb<'p> {
    pl: &'p Placebo,
    raw: std::ptr::NonNull<ffi::Rgb>,
    width: usize,
    height: usize,
}

#[cfg(target_os = "linux")]
impl Drop for GpuRgb<'_> {
    fn drop(&mut self) {
        // SAFETY: the texture belongs to this live device and is freed once.
        unsafe { ffi::cf_pl_rgb_destroy(self.pl.handle.as_ptr(), self.raw.as_ptr()) }
    }
}

#[cfg(target_os = "linux")]
impl GpuRgb<'_> {
    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    /// Reads it back as stored (`code << (16 - depth)`), R, G, B a pixel,
    /// for checks.
    ///
    /// # Errors
    ///
    /// [`Error::Render`] if libplacebo fails.
    pub fn download(&self) -> Result<Vec<u16>, Error> {
        let mut rgba = vec![0u16; self.width * self.height * 4];
        // SAFETY: `rgba` holds the w * h * 4 values the shim writes; the
        // texture belongs to this live device.
        let rc = unsafe {
            ffi::cf_pl_rgb_download(
                self.pl.handle.as_ptr(),
                self.raw.as_ptr(),
                rgba.as_mut_ptr(),
            )
        };
        if rc != 0 {
            return Err(Error::Render);
        }
        Ok(rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect())
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

    /// Uploads a tunneled Dolby Vision frame, captured as `r210`, and unpacks
    /// the 12-bit 4:2:2 inside it into `planes` on the GPU: see [`tunnel`].
    ///
    /// # Errors
    ///
    /// [`Error::BadInput`] for an odd width, a stride that is not whole words
    /// covering it, or short `data`; [`Error::Render`] if libplacebo fails.
    pub fn unpack_tunnel(
        &self,
        planes: &mut GpuPlanes<'_>,
        data: &[u8],
        row_bytes: usize,
    ) -> Result<(), Error> {
        if !planes.width.is_multiple_of(2)
            || !row_bytes.is_multiple_of(4)
            || row_bytes < planes.width * 4
            || data.len() < row_bytes.saturating_mul(planes.height)
        {
            return Err(Error::BadInput);
        }
        // SAFETY: `data` holds row_bytes * height bytes, which the shim
        // uploads; the planes belong to this live device, and `&mut` keeps
        // anything else from using them meanwhile.
        let rc = unsafe {
            ffi::cf_pl_unpack_tunnel(
                self.handle.as_ptr(),
                planes.raw.as_ptr(),
                data.as_ptr(),
                row_bytes,
            )
        };
        if rc == 0 { Ok(()) } else { Err(Error::Render) }
    }

    /// Renders planes holding a tunnel through libplacebo's Dolby Vision
    /// conversion with `color`, the frame's own, to BT.2020 PQ and into an
    /// RGB target with `dst_color`, and waits for the GPU. Reshaping is the
    /// identity: the player has composed the picture.
    ///
    /// # Errors
    ///
    /// [`Error::Render`] if the planes hold no tunnel or libplacebo fails
    /// (see its log).
    pub fn render_dovi(
        &self,
        src: &GpuPlanes<'_>,
        color: &DoviColor,
        dst: &Texture<'_>,
        dst_color: RgbColor,
        place: Option<Rect>,
    ) -> Result<(), Error> {
        self.render_dovi_mapped(src, color, dst, dst_color, View::placed(place), None)
    }

    /// [`render_dovi`](Self::render_dovi), with a tone mapping into a target
    /// that cannot show the grade's range.
    ///
    /// # Errors
    ///
    /// As [`render_dovi`](Self::render_dovi).
    pub fn render_dovi_mapped(
        &self,
        src: &GpuPlanes<'_>,
        color: &DoviColor,
        dst: &Texture<'_>,
        dst_color: RgbColor,
        view: View,
        map: Option<&Mapping>,
    ) -> Result<(), Error> {
        let place = Rect::raw(view.dst.as_ref());
        let src_crop = Rect::raw(view.src.as_ref());
        let map = map.map(Mapping::raw);
        let raw = dovi_raw(color);
        // SAFETY: `raw` and `map` (null or not) are structs whose layout the
        // shim shares, and `place` null or four floats, all read during the
        // call; the planes and the texture belong to this live device.
        let rc = unsafe {
            ffi::cf_pl_render_dovi(
                self.handle.as_ptr(),
                src.raw.as_ptr(),
                &raw,
                dst.tex.as_ptr(),
                primaries_raw(dst_color.primaries),
                transfer_raw(dst_color.transfer),
                place.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()),
                map.as_ref().map_or(std::ptr::null(), std::ptr::from_ref),
                src_crop.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()),
            )
        };
        if rc == 0 { Ok(()) } else { Err(Error::Render) }
    }

    /// [`render_dovi`](Self::render_dovi) through Dolby's display mapping,
    /// modelled ([`dm`]), in place of a libplacebo curve: the tunnel to
    /// BT.2020 PQ, `frame`'s mapping on that, then into `dst` with `map`,
    /// which is one to one at the mapping's target, as for the player's
    /// low-latency rendering.
    ///
    /// # Errors
    ///
    /// As [`render_dovi`](Self::render_dovi).
    #[allow(clippy::too_many_arguments)]
    pub fn render_dovi_dm(
        &self,
        src: &GpuPlanes<'_>,
        color: &DoviColor,
        frame: &dm::Frame,
        dst: &Texture<'_>,
        dst_color: RgbColor,
        view: View,
        map: &Mapping,
    ) -> Result<(), Error> {
        let place = Rect::raw(view.dst.as_ref());
        let src_crop = Rect::raw(view.src.as_ref());
        let map = map.raw();
        let raw = dovi_raw(color);
        let dm = frame.raw();
        // SAFETY: `raw`, `dm` and `map` are structs whose layout the shim
        // shares, `place` and `src_crop` null or four floats, all read during
        // the call; the planes and the texture belong to this live device.
        let rc = unsafe {
            ffi::cf_pl_render_dovi_dm(
                self.handle.as_ptr(),
                src.raw.as_ptr(),
                &raw,
                &dm,
                dst.tex.as_ptr(),
                primaries_raw(dst_color.primaries),
                transfer_raw(dst_color.transfer),
                place.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()),
                &map,
                src_crop.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()),
            )
        };
        if rc == 0 { Ok(()) } else { Err(Error::Render) }
    }

    /// `frame`'s mapping alone on the GPU, over `width` x `height` BT.2020 PQ
    /// RGBA16 pixels (alpha ignored): what [`render_dovi_dm`](Self::render_dovi_dm)
    /// does between the tunnel and the target, for checking against
    /// [`dm::Frame::apply`].
    ///
    /// # Errors
    ///
    /// [`Error::BadInput`] if `rgba` is not `width` x `height` x 4 values;
    /// [`Error::Render`] if libplacebo fails.
    pub fn dm_rgba16(
        &self,
        width: usize,
        height: usize,
        rgba: &[u16],
        frame: &dm::Frame,
    ) -> Result<Vec<u16>, Error> {
        if rgba.len() != width * height * 4 {
            return Err(Error::BadInput);
        }
        let mut out = vec![0u16; rgba.len()];
        let dm = frame.raw();
        // SAFETY: `rgba` and `out` hold width x height x 4 values, as the shim
        // reads and writes; `dm` is a struct whose layout the shim shares.
        let rc = unsafe {
            ffi::cf_pl_dm_rgba16(
                self.handle.as_ptr(),
                c_int::try_from(width).map_err(|_| Error::BadInput)?,
                c_int::try_from(height).map_err(|_| Error::BadInput)?,
                rgba.as_ptr(),
                &dm,
                out.as_mut_ptr(),
            )
        };
        if rc == 0 { Ok(out) } else { Err(Error::Render) }
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
        place: Option<Rect>,
    ) -> Result<(), Error> {
        self.render_ycbcr_mapped(src, color, dst, dst_color, View::placed(place), None)
    }

    /// [`render_ycbcr`](Self::render_ycbcr), with a tone mapping into a
    /// target that cannot show the source's range.
    ///
    /// # Errors
    ///
    /// As [`render_ycbcr`](Self::render_ycbcr).
    pub fn render_ycbcr_mapped(
        &self,
        src: &GpuPlanes<'_>,
        color: YcbcrColor,
        dst: &Texture<'_>,
        dst_color: RgbColor,
        view: View,
        map: Option<&Mapping>,
    ) -> Result<(), Error> {
        let place = Rect::raw(view.dst.as_ref());
        let src_crop = Rect::raw(view.src.as_ref());
        let map = map.map(Mapping::raw);
        let matrix = match color.matrix {
            Matrix::Bt709 => ffi::MATRIX_BT709,
            Matrix::Bt2020Nc => ffi::MATRIX_BT2020_NC,
        };
        // SAFETY: the planes and the texture belong to this live device;
        // `place` is null or four floats and `map` null or a struct whose
        // layout the shim shares, both read during the call.
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
                place.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()),
                map.as_ref().map_or(std::ptr::null(), std::ptr::from_ref),
                src_crop.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()),
            )
        };
        if rc == 0 { Ok(()) } else { Err(Error::Render) }
    }

    /// A texture for `width` x `height` captured RGB.
    ///
    /// # Errors
    ///
    /// [`Error::BadInput`] for an empty or oversized picture, [`Error::Render`]
    /// if libplacebo cannot create it (see its log).
    pub fn rgb(&self, width: usize, height: usize) -> Result<GpuRgb<'_>, Error> {
        let dim = |v: usize| {
            c_int::try_from(v)
                .ok()
                .filter(|&v| v > 0)
                .ok_or(Error::BadInput)
        };
        // SAFETY: the handle is live.
        let raw = unsafe { ffi::cf_pl_rgb_create(self.handle.as_ptr(), dim(width)?, dim(height)?) };
        std::ptr::NonNull::new(raw)
            .map(|raw| GpuRgb {
                pl: self,
                raw,
                width,
                height,
            })
            .ok_or(Error::Render)
    }

    /// Uploads an RGB frame the size of `rgb` and unpacks it on the GPU.
    /// `row_bytes` is the card's stride.
    ///
    /// # Errors
    ///
    /// [`Error::BadInput`] if the stride is not whole words covering the
    /// width or `data` is short, [`Error::Render`] if libplacebo fails.
    pub fn unpack_rgb(
        &self,
        rgb: &mut GpuRgb<'_>,
        data: &[u8],
        row_bytes: usize,
        format: rgb::Format,
    ) -> Result<(), Error> {
        if !row_bytes.is_multiple_of(4)
            || row_bytes < rgb.width * 4
            || data.len() < row_bytes.saturating_mul(rgb.height)
        {
            return Err(Error::BadInput);
        }
        let format = match format {
            rgb::Format::R210 => ffi::R210,
            rgb::Format::Argb8 => ffi::ARGB8,
        };
        // SAFETY: `data` holds row_bytes * height bytes, which the shim
        // uploads; the texture belongs to this live device, and `&mut` keeps
        // anything else from using it meanwhile.
        let rc = unsafe {
            ffi::cf_pl_unpack_rgb(
                self.handle.as_ptr(),
                rgb.raw.as_ptr(),
                data.as_ptr(),
                row_bytes,
                format,
            )
        };
        if rc == 0 { Ok(()) } else { Err(Error::Render) }
    }

    /// Renders captured RGB over the whole of `dst`, as [`render_ycbcr`]
    /// does YCbCr, at the bit depth of its last unpack.
    ///
    /// [`render_ycbcr`]: Self::render_ycbcr
    ///
    /// # Errors
    ///
    /// [`Error::Render`] if libplacebo fails (see its log).
    pub fn render_rgb(
        &self,
        src: &GpuRgb<'_>,
        color: RgbSourceColor,
        dst: &Texture<'_>,
        dst_color: RgbColor,
        place: Option<Rect>,
    ) -> Result<(), Error> {
        self.render_rgb_view(src, color, dst, dst_color, View::placed(place))
    }

    /// [`render_rgb`](Self::render_rgb) with a source crop as well as a
    /// placement.
    ///
    /// # Errors
    ///
    /// As [`render_rgb`](Self::render_rgb).
    pub fn render_rgb_view(
        &self,
        src: &GpuRgb<'_>,
        color: RgbSourceColor,
        dst: &Texture<'_>,
        dst_color: RgbColor,
        view: View,
    ) -> Result<(), Error> {
        let place = Rect::raw(view.dst.as_ref());
        let src_crop = Rect::raw(view.src.as_ref());
        let levels = match color.levels {
            Levels::Full => ffi::LEVELS_FULL,
            Levels::Limited => ffi::LEVELS_LIMITED,
        };
        // SAFETY: the source and the texture belong to this live device;
        // `place` is null or four floats, read during the call.
        let rc = unsafe {
            ffi::cf_pl_render_rgb(
                self.handle.as_ptr(),
                src.raw.as_ptr(),
                levels,
                primaries_raw(color.primaries),
                transfer_raw(color.transfer),
                dst.tex.as_ptr(),
                primaries_raw(dst_color.primaries),
                transfer_raw(dst_color.transfer),
                place.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()),
                src_crop.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()),
            )
        };
        if rc == 0 { Ok(()) } else { Err(Error::Render) }
    }

    /// Fills `dst` with black: a frame with nothing to show yet.
    ///
    /// # Errors
    ///
    /// [`Error::Render`] if libplacebo fails.
    pub fn clear_black(&self, dst: &Texture<'_>) -> Result<(), Error> {
        // SAFETY: the texture belongs to this live device.
        let rc = unsafe { ffi::cf_pl_clear_black(self.handle.as_ptr(), dst.tex.as_ptr()) };
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

#[cfg(target_os = "linux")]
fn dovi_raw(color: &DoviColor) -> ffi::Dovi {
    let flat = |m: &[[f64; 3]; 3]| std::array::from_fn(|i| m[i / 3][i % 3] as f32);
    ffi::Dovi {
        ycc_to_lms: flat(&color.ycc_to_lms),
        offset: color.offset.map(|o| o as f32),
        linear: flat(&color.linear),
    }
}

#[cfg(target_os = "linux")]
impl Mapping {
    fn raw(&self) -> ffi::Map {
        ffi::Map {
            src_peak: self.src_peak,
            dst_peak: self.dst_peak,
            curve: match self.curve {
                Curve::Clip => 0,
                Curve::Hable => 1,
                Curve::Spline => 2,
                Curve::Bt2390 => 3,
                Curve::Bt2446a => 4,
                Curve::St2094_10 => 5,
                Curve::St2094_40 => 6,
                Curve::Mobius => 7,
                Curve::Reinhard => 8,
            },
            peak_detect: self.peak_detect.into(),
            contrast_recovery: self.contrast_recovery,
            peak_percentile: self.peak_percentile,
            knee_offset: self.knee_offset,
        }
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

    #[test]
    fn fit_keeps_the_shape_and_centres() {
        // UHD on a 17:9 panel: bars at the sides.
        let r = Rect::fit(3840, 2160, 4096, 2160);
        assert_eq!((r.x0, r.y0, r.x1, r.y1), (128.0, 0.0, 3968.0, 2160.0));
        // HD scales up to the full height.
        assert_eq!(Rect::fit(1920, 1080, 4096, 2160), r);
        // A wider picture gets bars at the top and bottom.
        let r = Rect::fit(4096, 1716, 3840, 2160);
        assert!((r.x1 - r.x0 - 3840.0).abs() < 1e-3 && r.y0 > 0.0);
    }
}
