//! Capture from a device's input.
//!
//! The shim owns the SDK callback and the restart on a format change; this
//! module decides which pixel format each detected input is captured as, and
//! owns every frame's reference.

use std::ffi::{CStr, c_char, c_void};
use std::fmt;
use std::ptr::{self, NonNull};

use crate::{ColorInfo, Device, Error, ffi};

// BMDDetectedVideoInputFormatFlags.
const DETECTED_RGB444: u32 = 1 << 1;
const DETECTED_12BIT: u32 = 1 << 3;
const DETECTED_10BIT: u32 = 1 << 4;
const DETECTED_8BIT: u32 = 1 << 5;

// BMDFrameFlags.
const FRAME_CONTAINS_DOLBY_VISION_METADATA: u32 = 1 << 4;
const FRAME_HAS_NO_INPUT_SOURCE: u32 = 1 << 31;

// E_ACCESSDENIED from the SDK's LinuxCOM.h.
const E_ACCESSDENIED: i32 = 0x8000_0009_u32 as i32;

/// How the input's colour is sampled, as the card detects it. The card converts
/// any other YCbCr sampling to 4:2:2 before reporting it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sampling {
    Ycbcr422,
    Rgb444,
}

/// A pixel format frames are captured in (the SDK's `BMDPixelFormat`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PixelFormat {
    /// `v210`: 10-bit YCbCr 4:2:2, the SDK's only 10-bit YUV format.
    V210,
    /// 8-bit ARGB, bit-exact for 8-bit RGB input.
    Argb8,
    /// `r210`: 10-bit RGB. Used only so deeper RGB input is detected and
    /// counted; the card's level handling for it is unverified.
    Rgb10,
    /// Anything else the card reports.
    Other(u32),
}

impl PixelFormat {
    const fn to_raw(self) -> u32 {
        match self {
            PixelFormat::V210 => 0x7632_3130,  // 'v210'
            PixelFormat::Argb8 => 32,          // bmdFormat8BitARGB
            PixelFormat::Rgb10 => 0x7232_3130, // 'r210'
            PixelFormat::Other(raw) => raw,
        }
    }

    const fn from_raw(raw: u32) -> Self {
        match raw {
            0x7632_3130 => PixelFormat::V210,
            32 => PixelFormat::Argb8,
            0x7232_3130 => PixelFormat::Rgb10,
            _ => PixelFormat::Other(raw),
        }
    }
}

impl fmt::Display for PixelFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PixelFormat::V210 => f.write_str("v210"),
            PixelFormat::Argb8 => f.write_str("8-bit ARGB"),
            PixelFormat::Rgb10 => f.write_str("r210"),
            PixelFormat::Other(raw) => write!(f, "pixel format {raw:#010x}"),
        }
    }
}

/// The input's format, as detected when the signal changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputFormat {
    pub width: u32,
    pub height: u32,
    /// The frame rate is exactly `time_scale / frame_duration` frames per
    /// second, e.g. 24000/1001. Kept as a fraction: 23.976 is not 24.
    pub time_scale: i64,
    pub frame_duration: i64,
    pub sampling: Sampling,
    /// 8, 10 or 12, or `None` if the card did not report it.
    pub bit_depth: Option<u8>,
    display_mode: u32,
    enabled_mode: u32,
    enabled_pixel_format: u32,
}

impl InputFormat {
    /// The pixel format this input is captured as.
    ///
    /// YCbCr goes to `v210` at any depth; 12-bit loses its two lowest bits,
    /// which the SDK offers no way to avoid. 8-bit RGB goes to 8-bit ARGB,
    /// which keeps every bit: tunneled Dolby Vision hides its metadata in pixel
    /// parity, which any conversion destroys. Deeper RGB is captured as `r210`
    /// only to be detected and counted.
    pub fn capture_as(&self) -> PixelFormat {
        match (self.sampling, self.bit_depth) {
            (Sampling::Ycbcr422, _) => PixelFormat::V210,
            (Sampling::Rgb444, Some(8)) => PixelFormat::Argb8,
            (Sampling::Rgb444, _) => PixelFormat::Rgb10,
        }
    }

    /// The pixel format to re-enable the input with, or `None` when it already
    /// runs in the right mode and format. Restarting anyway is not harmless:
    /// the card reports the change again, and the restarts never end.
    fn restart_with(&self) -> Option<PixelFormat> {
        let want = self.capture_as();
        let running =
            self.display_mode == self.enabled_mode && want.to_raw() == self.enabled_pixel_format;
        (!running).then_some(want)
    }

    /// The frame rate as a float, for display only.
    pub fn frames_per_second(&self) -> f64 {
        if self.frame_duration > 0 {
            self.time_scale as f64 / self.frame_duration as f64
        } else {
            0.0
        }
    }
}

impl From<&ffi::Format> for InputFormat {
    fn from(raw: &ffi::Format) -> Self {
        let flags = raw.detected_flags;
        let bit_depth = if flags & DETECTED_12BIT != 0 {
            Some(12)
        } else if flags & DETECTED_10BIT != 0 {
            Some(10)
        } else if flags & DETECTED_8BIT != 0 {
            Some(8)
        } else {
            None
        };
        Self {
            width: u32::try_from(raw.width).unwrap_or(0),
            height: u32::try_from(raw.height).unwrap_or(0),
            time_scale: raw.time_scale,
            frame_duration: raw.frame_duration,
            sampling: if flags & DETECTED_RGB444 != 0 {
                Sampling::Rgb444
            } else {
                Sampling::Ycbcr422
            },
            bit_depth,
            display_mode: raw.display_mode,
            enabled_mode: raw.enabled_mode,
            enabled_pixel_format: raw.enabled_pixel_format,
        }
    }
}

impl fmt::Display for InputFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sampling = match self.sampling {
            Sampling::Ycbcr422 => "YCbCr 4:2:2",
            Sampling::Rgb444 => "RGB 4:4:4",
        };
        write!(
            f,
            "{}x{} {:.3} fps ({}/{}), {sampling}",
            self.width,
            self.height,
            self.frames_per_second(),
            self.time_scale,
            self.frame_duration
        )?;
        match self.bit_depth {
            Some(d) => write!(f, " {d}-bit"),
            None => f.write_str(", depth unreported"),
        }
    }
}

/// What the shim reads off a frame as it arrives.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameInfo {
    pub width: u32,
    pub height: u32,
    /// The SDK's row stride. Never compute it from the width.
    pub row_bytes: usize,
    pub pixel_format: PixelFormat,
    /// When the frame was captured, and how long it lasts, in the current
    /// format's `time_scale`. `None` if the card did not say.
    pub stream_time: Option<i64>,
    pub duration: Option<i64>,
    /// False for frames the card delivers while nothing is on its input.
    pub has_input: bool,
    /// Colorspace, transfer function and HDR static metadata.
    pub color: ColorInfo,
    /// The card's own Dolby Vision flag. The SDK documents Dolby Vision
    /// metadata for output only; this records whether input ever sets it.
    pub dolby_vision_flagged: bool,
}

impl From<&ffi::FrameInfo> for FrameInfo {
    fn from(raw: &ffi::FrameInfo) -> Self {
        let known = |v: i64| (v != ffi::UNKNOWN).then_some(v);
        Self {
            width: u32::try_from(raw.width).unwrap_or(0),
            height: u32::try_from(raw.height).unwrap_or(0),
            row_bytes: usize::try_from(raw.row_bytes).unwrap_or(0),
            pixel_format: PixelFormat::from_raw(raw.pixel_format),
            stream_time: known(raw.stream_time),
            duration: known(raw.stream_duration),
            has_input: raw.flags & FRAME_HAS_NO_INPUT_SOURCE == 0,
            color: ColorInfo::from(raw),
            dolby_vision_flagged: raw.flags & FRAME_CONTAINS_DOLBY_VISION_METADATA != 0,
        }
    }
}

/// One captured frame. Holds an SDK reference, released on drop.
///
/// Every frame held is a buffer the card cannot reuse, and its pool is small:
/// when it runs out it drops incoming frames without reporting an error. Drop
/// frames as soon as they are done with.
#[derive(Debug)]
pub struct Frame {
    handle: NonNull<ffi::FrameHandle>,
    info: FrameInfo,
}

// SAFETY: the SDK's reference counts are atomic, as COM's are, so the frame's
// reference may be released from any thread. Nothing else is reachable
// through the handle yet.
unsafe impl Send for Frame {}

impl Frame {
    pub fn info(&self) -> &FrameInfo {
        &self.info
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        // SAFETY: the shim handed this wrapper exactly one reference.
        unsafe { ffi::cf_dl_frame_release(self.handle.as_ptr()) }
    }
}

/// Receives what the input delivers.
///
/// Both methods run on the SDK's own thread and must return quickly: stalling
/// them makes the card drop frames. A panic in either aborts the process, since
/// it cannot unwind through the SDK.
pub trait Handler: Send + Sync + 'static {
    /// The input's format changed. From here on, frames arrive as
    /// [`InputFormat::capture_as`].
    fn format_changed(&self, format: &InputFormat) {
        let _ = format;
    }

    /// A frame arrived. Dropping it returns its buffer to the card.
    fn frame(&self, frame: Frame);
}

/// A running capture. Dropping it stops the input; once the drop returns, no
/// handler method is running or will run.
pub struct Capture {
    input: NonNull<ffi::InputHandle>,
    // Boxed twice so the shim's context is a thin pointer. Declared after
    // `input`, and freed only after `drop` has stopped the callbacks.
    _handler: Box<Box<dyn Handler>>,
}

impl Capture {
    /// How many format-change restarts failed to enable the new mode. Any
    /// failure leaves the input without frames until the next change.
    pub fn failed_restarts(&self) -> u64 {
        // SAFETY: the input is live for as long as `self`.
        unsafe { ffi::cf_dl_input_failed_restarts(self.input.as_ptr()) }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        // SAFETY: we own the input. Releasing it stops it first, and the shim
        // guarantees no callback runs once that returns, so the handler is
        // unreachable before its box is freed.
        unsafe { ffi::cf_dl_input_release(self.input.as_ptr()) }
    }
}

impl Device {
    /// Starts capturing from this device's input with format detection on.
    /// The input starts in 720p50, a mode no source here sends, so the real
    /// format is always reported once through [`Handler::format_changed`].
    ///
    /// # Errors
    ///
    /// [`Error::NoInput`] if the device cannot capture, [`Error::InputBusy`] if
    /// another program holds the input, or [`Error::Sdk`] for any other
    /// failure to start it.
    pub fn start_capture(&self, handler: impl Handler) -> Result<Capture, Error> {
        // SAFETY: the device is live; the shim takes its own reference.
        let input = NonNull::new(unsafe { ffi::cf_dl_input_open(self.0.as_ptr()) })
            .ok_or(Error::NoInput)?;

        let handler: Box<Box<dyn Handler>> = Box::new(Box::new(handler));
        let ctx: *const Box<dyn Handler> = &raw const *handler;
        let mut failed_call: *const c_char = ptr::null();
        // SAFETY: the input is live; `ctx` points into a heap box that the
        // returned `Capture` keeps alive until the callbacks have stopped.
        let hr = unsafe {
            ffi::cf_dl_input_start(
                input.as_ptr(),
                on_format,
                on_frame,
                ctx.cast_mut().cast(),
                &mut failed_call,
            )
        };
        if hr != 0 {
            // SAFETY: the start failed, so no callback is armed; this frees
            // the input before the handler box is dropped.
            unsafe { ffi::cf_dl_input_release(input.as_ptr()) };
            // SAFETY: on failure the shim points `failed_call` at a static,
            // NUL-terminated name.
            let call = unsafe { CStr::from_ptr(failed_call) }.to_string_lossy();
            return Err(if call == "EnableVideoInput" && hr == E_ACCESSDENIED {
                Error::InputBusy
            } else {
                Error::Sdk {
                    call: call.into_owned(),
                    hresult: hr,
                }
            });
        }
        Ok(Capture {
            input,
            _handler: handler,
        })
    }
}

unsafe extern "C" fn on_format(ctx: *mut c_void, raw: *const ffi::Format) -> u32 {
    // SAFETY: `ctx` is the handler box of a `Capture`, which outlives every
    // callback; `raw` is valid for this call.
    let (handler, raw) = unsafe { (&*ctx.cast::<Box<dyn Handler>>(), &*raw) };
    let format = InputFormat::from(raw);
    handler.format_changed(&format);
    format.restart_with().map_or(0, PixelFormat::to_raw)
}

unsafe extern "C" fn on_frame(
    ctx: *mut c_void,
    frame: *mut ffi::FrameHandle,
    raw: *const ffi::FrameInfo,
) {
    // SAFETY: as in `on_format`.
    let (handler, raw) = unsafe { (&*ctx.cast::<Box<dyn Handler>>(), &*raw) };
    if let Some(handle) = NonNull::new(frame) {
        handler.frame(Frame {
            handle,
            info: FrameInfo::from(raw),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODE_2160P2398: u32 = 0x346B_3233; // '4k23'
    const MODE_1080P2398: u32 = 0x3233_7073; // '23ps'

    fn raw_format(detected_flags: u32) -> ffi::Format {
        ffi::Format {
            frame_duration: 1001,
            time_scale: 24000,
            width: 3840,
            height: 2160,
            display_mode: MODE_2160P2398,
            detected_flags,
            enabled_mode: MODE_2160P2398,
            enabled_pixel_format: PixelFormat::V210.to_raw(),
        }
    }

    #[test]
    fn ycbcr_at_any_depth_is_captured_as_v210() {
        for depth in [DETECTED_8BIT, DETECTED_10BIT, DETECTED_12BIT] {
            let f = InputFormat::from(&raw_format(depth));
            assert_eq!(f.sampling, Sampling::Ycbcr422);
            assert_eq!(f.capture_as(), PixelFormat::V210);
        }
    }

    #[test]
    fn eight_bit_rgb_is_captured_bit_exact() {
        let f = InputFormat::from(&raw_format(DETECTED_RGB444 | DETECTED_8BIT));
        assert_eq!(f.sampling, Sampling::Rgb444);
        assert_eq!(f.bit_depth, Some(8));
        assert_eq!(f.capture_as(), PixelFormat::Argb8);
    }

    #[test]
    fn deeper_or_unreported_rgb_is_only_detected() {
        for depth in [DETECTED_10BIT, DETECTED_12BIT, 0] {
            let f = InputFormat::from(&raw_format(DETECTED_RGB444 | depth));
            assert_eq!(f.capture_as(), PixelFormat::Rgb10);
        }
    }

    #[test]
    fn no_restart_when_mode_and_format_already_run() {
        let f = InputFormat::from(&raw_format(DETECTED_10BIT));
        assert_eq!(f.restart_with(), None);
    }

    #[test]
    fn restart_on_new_mode() {
        let mut raw = raw_format(DETECTED_10BIT);
        raw.display_mode = MODE_1080P2398;
        let f = InputFormat::from(&raw);
        assert_eq!(f.restart_with(), Some(PixelFormat::V210));
    }

    #[test]
    fn restart_on_new_pixel_format_in_the_same_mode() {
        let f = InputFormat::from(&raw_format(DETECTED_RGB444 | DETECTED_8BIT));
        assert_eq!(f.restart_with(), Some(PixelFormat::Argb8));
    }

    #[test]
    fn rate_stays_a_fraction() {
        let f = InputFormat::from(&raw_format(DETECTED_10BIT));
        assert_eq!((f.time_scale, f.frame_duration), (24000, 1001));
        assert_eq!(
            f.to_string(),
            "3840x2160 23.976 fps (24000/1001), YCbCr 4:2:2 10-bit"
        );
    }

    #[test]
    fn pixel_formats_round_trip() {
        for pf in [
            PixelFormat::V210,
            PixelFormat::Argb8,
            PixelFormat::Rgb10,
            PixelFormat::Other(0x5231_3242),
        ] {
            assert_eq!(PixelFormat::from_raw(pf.to_raw()), pf);
        }
    }

    #[test]
    fn frame_info_decodes_unknowns_and_no_input() {
        let raw = ffi::FrameInfo {
            row_bytes: 10240,
            width: 3840,
            height: 2160,
            pixel_format: PixelFormat::V210.to_raw(),
            flags: FRAME_HAS_NO_INPUT_SOURCE,
            ..crate::metadata::tests::unreported()
        };
        let info = FrameInfo::from(&raw);
        assert_eq!(info.stream_time, None);
        assert_eq!(info.duration, None);
        assert_eq!(info.row_bytes, 10240);
        assert_eq!(info.pixel_format, PixelFormat::V210);
        assert!(!info.has_input);
        assert!(!info.dolby_vision_flagged);
        assert_eq!(info.color.hdr, None);
    }
}
