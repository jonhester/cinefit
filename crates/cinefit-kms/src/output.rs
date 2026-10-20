//! One display output: a connector driven by a CRTC through its primary plane,
//! with the atomic API. Whatever this changes is put back when it drops.

use std::collections::HashMap;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::{AsFd, BorrowedFd};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use drm::buffer::DrmFourcc;
use drm::control::dumbbuffer::DumbBuffer;
use drm::control::{
    AtomicCommitFlags, Device as ControlDevice, Event, FbCmd2Flags, Mode, ModeFlags, ModeTypeFlags,
    PlaneType, RawResourceHandle, atomic::AtomicModeReq, connector, crtc, framebuffer, plane,
    property,
};
use drm::{ClientCapability, Device as BasicDevice};
use rustix::event::{PollFd, PollFlags, poll};

use crate::hdr::HdrStatic;
use crate::timing::ModeTiming;

/// How long [`Output::wait_flip`] waits before giving up: the display or
/// driver has stopped answering. A flip waits for the GPU to finish the frame
/// it shows, so a GPU stall makes one late; 250 ms, the first figure, ended a
/// run on 2026-10-15 six seconds in, with a 103 ms upload stall in the same
/// run, for a restart and a projector resync. A real hang is still caught,
/// long before systemd's 15 s stop timeout.
pub const FLIP_TIMEOUT: Duration = Duration::from_secs(2);
/// A flip this late is reported ([`Flip::waited`]): ten frames at 23.976.
pub const FLIP_LATE: Duration = Duration::from_millis(400);

/// An open DRM device node.
struct Card(File);

impl AsFd for Card {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

impl BasicDevice for Card {}
impl ControlDevice for Card {}

#[derive(Debug)]
pub enum Error {
    /// A DRM call failed.
    Io {
        what: &'static str,
        source: io::Error,
    },
    /// No `/dev/dri/card*` has a connected display.
    NoDisplay,
    NoCrtc,
    NoPlane,
    /// The primary plane takes none of the scanout formats used here.
    NoScanoutFormat,
    /// The CRTC has no overlay plane that takes ARGB8888.
    NoOverlay,
    /// The driver lacks a property every atomic driver should have.
    MissingProperty(&'static str),
    /// Another program holds the display (DRM master).
    Busy,
    /// The operation needs the other kind of frame: CPU-drawn ones are drawn,
    /// GBM ones exported and read back.
    NotScanout,
    /// GBM chose a layout of several planes (a compressed one, with its
    /// metadata beside it), which the export and the renderer's import do
    /// not handle: the layout, and how many planes.
    Planes(u64, u32),
    /// A queued flip did not complete in time: the display or the driver has
    /// stopped answering.
    NoFlip(Duration),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io { what, source } => write!(f, "{what}: {source}"),
            Error::NoDisplay => f.write_str("no DRM device has a connected display"),
            Error::NoCrtc => f.write_str("no CRTC can drive the connector"),
            Error::NoPlane => f.write_str("no primary plane for the CRTC"),
            Error::NoOverlay => f.write_str("no overlay plane taking ARGB8888 for the CRTC"),
            Error::NoScanoutFormat => {
                f.write_str("the primary plane takes no XRGB2101010, XBGR2101010 or XRGB8888")
            }
            Error::MissingProperty(p) => write!(f, "the driver has no {p} property"),
            Error::Busy => f.write_str("another program holds the display (DRM master)"),
            Error::NotScanout => f.write_str("wrong kind of frame for this operation"),
            Error::Planes(m, n) => write!(
                f,
                "GBM chose {}, a layout of {n} planes",
                crate::modifier_name(*m)
            ),
            Error::NoFlip(waited) => write!(
                f,
                "no page-flip event within {} ms: the display or driver stopped answering",
                waited.as_millis()
            ),
        }
    }
}

impl std::error::Error for Error {}

pub(crate) fn io(what: &'static str) -> impl FnOnce(io::Error) -> Error {
    move |source| Error::Io { what, source }
}

/// The ids of the properties this module sets, looked up by name.
struct Props {
    connector_crtc: property::Handle,
    mode_id: property::Handle,
    active: property::Handle,
    fb_id: property::Handle,
    plane_crtc: property::Handle,
    src: [property::Handle; 4],
    dst: [property::Handle; 4],
    /// HDR signalling: optional, since not every driver or connector has it.
    hdr_metadata: Option<property::Handle>,
    colorspace: Option<ColorspaceProp>,
    max_bpc: Option<property::Handle>,
}

/// The connector's `Colorspace` enum, with the two values used here.
struct ColorspaceProp {
    handle: property::Handle,
    default: Option<u64>,
    bt2020_rgb: Option<u64>,
}

/// The overlay plane: a second, transparent layer the display engine
/// composites over the video, for on-screen text. Drawn by the CPU when it
/// changes, never per frame; the video's buffers never see it.
struct Overlay {
    plane: plane::Handle,
    fb_id: property::Handle,
    crtc_id: property::Handle,
    src: [property::Handle; 4],
    dst: [property::Handle; 4],
    frames: Vec<(Frame, (u16, u16))>,
    /// A change for the next commit: `Some(None)` takes the layer down.
    pending: Option<Option<usize>>,
    on_screen: Option<usize>,
    /// The plane's own opacity (0-0xffff), for fades without redrawing:
    /// absent on planes without the property.
    alpha: Option<property::Handle>,
    alpha_pending: Option<u16>,
    /// What a commit of this run set it to; `None` until one has, since a
    /// run that ended without restoring may have left it anywhere.
    alpha_now: Option<u16>,
    /// Whether a commit of this run has changed the plane. Only then does
    /// `restore` take it down: an overlay nobody asked for is left alone.
    touched: bool,
}

/// An overlay's commit state: pending buffer change, buffer on screen,
/// pending opacity, opacity set.
type OverlayState = (
    Option<Option<usize>>,
    Option<usize>,
    Option<u16>,
    Option<u16>,
);

/// A CPU-drawn scanout buffer.
struct Frame {
    buffer: FrameBuffer,
    fb: framebuffer::Handle,
}

/// What backs a frame: memory the CPU draws into, or a GBM scanout buffer
/// the GPU renders into.
enum FrameBuffer {
    Dumb(DumbBuffer),
    Gbm(gbm::BufferObject<()>),
}

/// A scanout buffer shared as a dma-buf, for a GPU API to render into.
pub struct DmaBuf {
    /// Ours to close; importers duplicate it.
    pub fd: std::os::fd::OwnedFd,
    pub width: u32,
    pub height: u32,
    /// DRM fourcc, e.g. XRGB8888.
    pub fourcc: u32,
    pub modifier: u64,
    pub stride: u32,
    pub offset: u32,
}

/// How [`Output::restore`] left the display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Restored {
    /// Nothing had been changed.
    NothingChanged,
    /// The previous mode and plane state, with HDR cleared.
    Previous,
    /// The previous state no longer applied, so the output was turned off,
    /// with HDR cleared.
    TurnedOff,
}

impl fmt::Display for Restored {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Restored::NothingChanged => "nothing to restore",
            Restored::Previous => "restored the previous state",
            Restored::TurnedOff => "previous state no longer applied: turned the output off",
        })
    }
}

/// A completed flip: the vblank it landed on and when.
#[derive(Debug, Clone, Copy)]
pub struct Flip {
    pub sequence: u32,
    /// CLOCK_MONOTONIC, to the microsecond.
    pub at: Duration,
    /// How long [`Output::wait_flip`] waited for it.
    pub waited: Duration,
}

pub struct Output {
    card: Card,
    path: PathBuf,
    connector: connector::Handle,
    connector_name: String,
    crtc: crtc::Handle,
    plane: plane::Handle,
    modes: Vec<Mode>,
    props: Props,
    /// Every property this module changes, with the value it had before.
    saved: Vec<(RawResourceHandle, property::Handle, property::RawValue)>,
    mode_blob: Option<u64>,
    modeset: bool,
    frames: Vec<Frame>,
    /// Created on first use, on the card's own open file, so the buffers it
    /// allocates belong to the same DRM client as the framebuffers.
    gbm: Option<gbm::Device<File>>,
    /// HDR metadata for the next modeset; `None` signals SDR.
    hdr: Option<HdrStatic>,
    /// SDR in a BT.2020 container: `Colorspace` BT2020_RGB without HDR
    /// metadata, for a display set to an SDR BT.2020 mode.
    sdr_bt2020: bool,
    /// The blob on the connector now, and one queued for the next flip.
    hdr_blob: Option<u64>,
    pending_hdr_blob: Option<u64>,
    /// Whether the lit output signals HDR. `None` while nothing is
    /// committed.
    hdr_on_screen: Option<bool>,
    /// Modesets that went through dark first, into or out of HDR, and how
    /// long the last one left the output dark.
    relights: u64,
    last_dark: Option<Duration>,
    /// `None` when the CRTC has no overlay plane that takes ARGB8888.
    overlay: Option<Overlay>,
}

impl Output {
    /// Opens `path`, or else the first `/dev/dri/card*` with a connected
    /// display, and takes its first connected connector.
    ///
    /// # Errors
    ///
    /// If no device has a connected display, or the device lacks what an
    /// atomic driver needs.
    pub fn open(path: Option<&Path>) -> Result<Self, Error> {
        let candidates: Vec<PathBuf> = match path {
            Some(p) => vec![p.to_owned()],
            None => {
                let mut v: Vec<_> = std::fs::read_dir("/dev/dri")
                    .map_err(io("listing /dev/dri"))?
                    .filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| {
                        p.file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|n| n.starts_with("card"))
                    })
                    .collect();
                v.sort();
                v
            }
        };
        // A device named explicitly must work. While searching, one that
        // fails (a basic framebuffer device without the atomic API, say) is
        // skipped, and its error only reported if nothing else works.
        let explicit = path.is_some();
        let mut last_error = None;
        for p in candidates {
            let opened = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&p)
                .map_err(io("opening the DRM device"))
                .and_then(|file| Self::from_card(Card(file), p));
            match opened {
                Ok(Some(out)) => return Ok(out),
                Ok(None) => {}
                Err(e) if explicit => return Err(e),
                Err(e) => last_error = Some(e),
            }
        }
        Err(last_error.unwrap_or(Error::NoDisplay))
    }

    fn from_card(card: Card, path: PathBuf) -> Result<Option<Self>, Error> {
        card.set_client_capability(ClientCapability::UniversalPlanes, true)
            .map_err(io("enabling universal planes"))?;
        card.set_client_capability(ClientCapability::Atomic, true)
            .map_err(io("enabling the atomic API"))?;
        let res = card.resource_handles().map_err(io("reading resources"))?;

        let Some(conn) = res
            .connectors()
            .iter()
            .filter_map(|&c| card.get_connector(c, true).ok())
            .find(|c| {
                c.state() == connector::State::Connected
                    && c.interface() != connector::Interface::Writeback
            })
        else {
            return Ok(None);
        };

        // Prefer the CRTC already driving the connector, else any it can use.
        let current = conn
            .current_encoder()
            .and_then(|e| card.get_encoder(e).ok())
            .and_then(|e| e.crtc());
        let crtc = match current {
            Some(c) => c,
            None => {
                // Never take a CRTC that is driving another display.
                let busy: Vec<crtc::Handle> = res
                    .connectors()
                    .iter()
                    .filter(|&&c| c != conn.handle())
                    .filter_map(|&c| card.get_connector(c, false).ok())
                    .filter_map(|c| c.current_encoder())
                    .filter_map(|e| card.get_encoder(e).ok())
                    .filter_map(|e| e.crtc())
                    .collect();
                conn.encoders()
                    .iter()
                    .filter_map(|&e| card.get_encoder(e).ok())
                    .flat_map(|e| res.filter_crtcs(e.possible_crtcs()))
                    .find(|c| !busy.contains(c))
                    .ok_or(Error::NoCrtc)?
            }
        };

        let plane = card
            .plane_handles()
            .map_err(io("listing planes"))?
            .into_iter()
            .find(|&p| {
                card.get_plane(p)
                    .is_ok_and(|info| res.filter_crtcs(info.possible_crtcs()).contains(&crtc))
                    && plane_type(&card, p) == Some(PlaneType::Primary)
            })
            .ok_or(Error::NoPlane)?;

        let overlay = find_overlay(&card, &res, crtc);

        let conn_props = props_by_name(&card, conn.handle())?;
        let crtc_props = props_by_name(&card, crtc)?;
        let plane_props = props_by_name(&card, plane)?;
        let get = |m: &HashMap<String, property::Handle>, name: &'static str| {
            m.get(name).copied().ok_or(Error::MissingProperty(name))
        };
        let props = Props {
            connector_crtc: get(&conn_props, "CRTC_ID")?,
            mode_id: get(&crtc_props, "MODE_ID")?,
            active: get(&crtc_props, "ACTIVE")?,
            fb_id: get(&plane_props, "FB_ID")?,
            plane_crtc: get(&plane_props, "CRTC_ID")?,
            src: [
                get(&plane_props, "SRC_X")?,
                get(&plane_props, "SRC_Y")?,
                get(&plane_props, "SRC_W")?,
                get(&plane_props, "SRC_H")?,
            ],
            dst: [
                get(&plane_props, "CRTC_X")?,
                get(&plane_props, "CRTC_Y")?,
                get(&plane_props, "CRTC_W")?,
                get(&plane_props, "CRTC_H")?,
            ],
            hdr_metadata: conn_props.get("HDR_OUTPUT_METADATA").copied(),
            colorspace: conn_props
                .get("Colorspace")
                .map(|&handle| colorspace_prop(&card, handle)),
            max_bpc: conn_props.get("max bpc").copied(),
        };

        let connector_name = format!("{}-{}", conn.interface().as_str(), conn.interface_id());
        Ok(Some(Self {
            card,
            path,
            connector: conn.handle(),
            connector_name,
            crtc,
            plane,
            modes: conn.modes().to_vec(),
            props,
            saved: Vec::new(),
            mode_blob: None,
            modeset: false,
            frames: Vec::new(),
            gbm: None,
            hdr: None,
            sdr_bt2020: false,
            hdr_blob: None,
            pending_hdr_blob: None,
            hdr_on_screen: None,
            relights: 0,
            last_dark: None,
            overlay,
        }))
    }

    /// The device path and connector name, e.g. `/dev/dri/card1 HDMI-A-1`.
    pub fn describe(&self) -> String {
        format!("{} {}", self.path.display(), self.connector_name)
    }

    /// The connector's modes, in the order the driver lists them.
    pub fn modes(&self) -> Vec<ModeTiming> {
        self.modes.iter().map(timing).collect()
    }

    /// Asks the driver to probe the connector again (its EDID, so a display
    /// that came on behind a receiver is seen) and takes the modes it now
    /// offers, which later mode indices refer to. Whether it is still
    /// connected, and the modes. Only before the first modeset: indices
    /// already handed out would point into the old list.
    ///
    /// The probe, not the cached list: a hotplug pulse updates the
    /// connector's state, but the mode list is refreshed only when someone
    /// probes, which a desktop does on the uevent and cinefit does here.
    ///
    /// # Errors
    ///
    /// If the connector cannot be read at all.
    pub fn probe_modes(&mut self) -> Result<(bool, Vec<ModeTiming>), Error> {
        let conn = self
            .card
            .get_connector(self.connector, true)
            .map_err(io("probing the connector"))?;
        let connected = conn.state() == connector::State::Connected;
        if !self.modeset {
            self.modes = conn.modes().to_vec();
        }
        Ok((connected, self.modes()))
    }

    /// A buffer the size of mode `index`, in XRGB8888, for the CPU to draw.
    ///
    /// # Errors
    ///
    /// If the driver cannot allocate or register it.
    pub fn create_frame(&mut self, index: usize) -> Result<usize, Error> {
        let (w, h) = self.modes[index].size();
        let mut buffer = self
            .card
            .create_dumb_buffer((w.into(), h.into()), DrmFourcc::Xrgb8888, 32)
            .map_err(io("allocating a dumb buffer"))?;
        let fb = match self.card.add_framebuffer(&buffer, 24, 32) {
            Ok(fb) => fb,
            Err(e) => {
                let _ = self.card.destroy_dumb_buffer(buffer);
                return Err(io("registering a framebuffer")(e));
            }
        };
        // Zero it, so an undrawn frame is black rather than stale memory.
        if let Ok(mut map) = self.card.map_dumb_buffer(&mut buffer) {
            map.as_mut().fill(0);
        }
        self.frames.push(Frame {
            buffer: FrameBuffer::Dumb(buffer),
            fb,
        });
        Ok(self.frames.len() - 1)
    }

    /// A scanout buffer the size of mode `index`, for the GPU to render into:
    /// the deepest of [`scanout::PREFERRED`] the plane takes, linear. Linear
    /// because both the display engine and the GPU always accept it, which
    /// the C++ version relied on for the same card; tiled layouts can come
    /// later if rendering speed calls for them.
    ///
    /// [`scanout::PREFERRED`]: crate::scanout::PREFERRED
    ///
    /// # Errors
    ///
    /// If GBM cannot allocate it or the driver cannot register it.
    /// The format [`Self::create_scanout`] allocates: the first of
    /// [`scanout::PREFERRED`] the primary plane lists.
    ///
    /// [`scanout::PREFERRED`]: crate::scanout::PREFERRED
    ///
    /// # Errors
    ///
    /// If the plane cannot be read or lists none of them.
    pub fn scanout_format(&self) -> Result<gbm::Format, Error> {
        let info = self
            .card
            .get_plane(self.plane)
            .map_err(io("reading the plane's formats"))?;
        let formats = info.formats();
        crate::scanout::PREFERRED
            .iter()
            .find(|f| formats.contains(f))
            .and_then(|&f| gbm::Format::try_from(f).ok())
            .ok_or(Error::NoScanoutFormat)
    }

    pub fn create_scanout(&mut self, index: usize) -> Result<usize, Error> {
        self.create_scanout_as(index, 0)
    }

    /// The layouts (DRM format modifiers) the primary plane scans the
    /// scanout format out in, from its `IN_FORMATS`; LINEAR alone if the
    /// plane does not list them. For `cinefit bench` (#101).
    ///
    /// # Errors
    ///
    /// If the plane's formats or properties cannot be read.
    pub fn scanout_modifiers(&self) -> Result<Vec<u64>, Error> {
        let format = self.scanout_format()? as u32;
        let props = props_by_name(&self.card, self.plane)?;
        let Some(&prop) = props.get("IN_FORMATS") else {
            return Ok(vec![0]);
        };
        let values = self
            .card
            .get_properties(self.plane)
            .map_err(io("reading the plane's properties"))?;
        let (ids, vals) = values.as_props_and_values();
        let Some(blob) = ids
            .iter()
            .zip(vals)
            .find(|(id, _)| **id == prop)
            .map(|(_, &v)| v)
            .filter(|&v| v != 0)
            .and_then(|v| self.card.get_property_blob(v).ok())
        else {
            return Ok(vec![0]);
        };
        Ok(crate::display::in_formats(&blob)
            .and_then(|list| list.into_iter().find(|(f, _)| *f == format))
            .map_or_else(|| vec![0], |(_, mods)| mods))
    }

    /// A scanout buffer the size of mode `index` in layout `modifier`, as
    /// [`Output::create_scanout`] makes in LINEAR: registered as a
    /// framebuffer, so the plane is known to take it.
    ///
    /// # Errors
    ///
    /// If GBM cannot allocate it or the driver will not register it.
    pub fn create_scanout_as(&mut self, index: usize, modifier: u64) -> Result<usize, Error> {
        self.create_scanout_in(index, &[modifier])
    }

    /// A scanout buffer the size of mode `index` in whichever of `modifiers`
    /// GBM thinks best for the plane (#101): given the plane's list, a tiled
    /// layout where the GPU has one, which it renders into faster than
    /// LINEAR. A layout of more than one plane (a compressed one, with its
    /// metadata beside it) is refused: the export and the renderer's import
    /// handle one.
    ///
    /// # Errors
    ///
    /// If GBM cannot allocate any of them, it chose a layout of several
    /// planes, or the driver will not register it.
    pub fn create_scanout_in(&mut self, index: usize, modifiers: &[u64]) -> Result<usize, Error> {
        let (w, h) = self.modes[index].size();
        if self.gbm.is_none() {
            // A duplicate of the card's fd: the same open file, so GEM handles
            // GBM creates are valid for this client's framebuffers.
            let file = self
                .card
                .0
                .try_clone()
                .map_err(io("duplicating the DRM fd"))?;
            self.gbm = Some(gbm::Device::new(file).map_err(io("creating the GBM device"))?);
        }
        let format = self.scanout_format()?;
        let gbm = self.gbm.as_ref().expect("created above");
        let alloc = |flags| {
            gbm.create_buffer_object_with_modifiers2::<()>(
                w.into(),
                h.into(),
                format,
                modifiers.iter().map(|&m| gbm::Modifier::from(m)),
                flags,
            )
        };
        // NVIDIA's GBM refuses LINEAR with RENDERING (EINVAL, every format)
        // but allocates it for SCANOUT alone. RENDERING is only a hint here:
        // the renderer writes through Vulkan's dma-buf import, not GBM.
        let bo = alloc(gbm::BufferObjectFlags::SCANOUT | gbm::BufferObjectFlags::RENDERING)
            .or_else(|e| {
                if modifiers == [0] {
                    alloc(gbm::BufferObjectFlags::SCANOUT)
                } else {
                    Err(e)
                }
            })
            .map_err(io("allocating a GBM scanout buffer"))?;
        if bo.plane_count() != 1 {
            return Err(Error::Planes(bo.modifier().into(), bo.plane_count()));
        }
        let fb = self
            .card
            .add_planar_framebuffer(&bo, FbCmd2Flags::MODIFIERS)
            .map_err(io("registering a framebuffer"))?;
        self.frames.push(Frame {
            buffer: FrameBuffer::Gbm(bo),
            fb,
        });
        Ok(self.frames.len() - 1)
    }

    /// How many scanout buffers have been made.
    pub fn scanout_count(&self) -> usize {
        self.frames.len()
    }

    /// Releases the scanout buffers made after the first `keep`: a set that
    /// cannot be used is given back before another is made. Only those not
    /// on screen: call it before the first modeset.
    pub fn discard_scanouts(&mut self, keep: usize) {
        for f in self.frames.drain(keep.min(self.frames.len())..) {
            let _ = self.card.destroy_framebuffer(f.fb);
            if let FrameBuffer::Dumb(b) = f.buffer {
                let _ = self.card.destroy_dumb_buffer(b);
            }
        }
    }

    /// Whether this output has an overlay plane for on-screen text.
    pub fn has_overlay(&self) -> bool {
        self.overlay.is_some()
    }

    /// A transparent ARGB8888 buffer the size of mode `index` for the overlay
    /// plane, premultiplied alpha (the plane's default blend). Make two and
    /// alternate, so a redraw never shows half-drawn.
    ///
    /// # Errors
    ///
    /// [`Error::NoOverlay`] without an overlay plane; else if the driver
    /// cannot allocate or register it.
    pub fn create_overlay(&mut self, index: usize) -> Result<usize, Error> {
        let (w, h) = self.modes[index].size();
        let Some(ov) = self.overlay.as_mut() else {
            return Err(Error::NoOverlay);
        };
        let mut buffer = self
            .card
            .create_dumb_buffer((w.into(), h.into()), DrmFourcc::Argb8888, 32)
            .map_err(io("allocating an overlay buffer"))?;
        // Depth 32 at 32 bpp is ARGB8888 to the legacy call.
        let fb = match self.card.add_framebuffer(&buffer, 32, 32) {
            Ok(fb) => fb,
            Err(e) => {
                let _ = self.card.destroy_dumb_buffer(buffer);
                return Err(io("registering an overlay framebuffer")(e));
            }
        };
        if let Ok(mut map) = self.card.map_dumb_buffer(&mut buffer) {
            map.as_mut().fill(0); // transparent
        }
        ov.frames.push((
            Frame {
                buffer: FrameBuffer::Dumb(buffer),
                fb,
            },
            (w, h),
        ));
        Ok(ov.frames.len() - 1)
    }

    /// Draws into overlay buffer `i`: `draw` gets its bytes (B, G, R, A per
    /// pixel, premultiplied) and pitch.
    ///
    /// # Errors
    ///
    /// [`Error::NoOverlay`], or if the buffer cannot be mapped.
    pub fn draw_overlay(
        &mut self,
        i: usize,
        draw: impl FnOnce(&mut [u8], usize),
    ) -> Result<(), Error> {
        let ov = self.overlay.as_mut().ok_or(Error::NoOverlay)?;
        let FrameBuffer::Dumb(buffer) = &mut ov.frames[i].0.buffer else {
            return Err(Error::NotScanout);
        };
        let pitch = drm::buffer::Buffer::pitch(buffer) as usize;
        let mut map = self
            .card
            .map_dumb_buffer(buffer)
            .map_err(io("mapping an overlay buffer"))?;
        draw(map.as_mut(), pitch);
        Ok(())
    }

    /// Shows overlay buffer `i`, or with `None` takes the layer down, with
    /// the next flip or modeset: no commit of its own, and no modeset.
    pub fn set_overlay(&mut self, i: Option<usize>) {
        if let Some(ov) = self.overlay.as_mut()
            && ov.on_screen != i
        {
            ov.pending = Some(i);
        }
    }

    /// Whether the overlay plane has its own opacity, for
    /// [`Self::set_overlay_alpha`].
    pub fn has_overlay_alpha(&self) -> bool {
        self.overlay.as_ref().is_some_and(|ov| ov.alpha.is_some())
    }

    /// The whole overlay's opacity, 0 to 0xffff, with the next flip or
    /// modeset, as [`Self::set_overlay`]: a fade costs a property in the
    /// commit, not a redraw. Does nothing on a plane without the property.
    pub fn set_overlay_alpha(&mut self, alpha: u16) {
        if let Some(ov) = self.overlay.as_mut()
            && ov.alpha.is_some()
        {
            ov.alpha_pending = (ov.alpha_now != Some(alpha)).then_some(alpha);
        }
    }

    /// The overlay's pending changes, as properties for a commit.
    fn overlay_props(&mut self) -> Vec<(RawResourceHandle, property::Handle, u64)> {
        let crtc = u64::from(u32::from(RawResourceHandle::from(self.crtc)));
        let Some(ov) = self.overlay.as_mut() else {
            return Vec::new();
        };
        let plane: RawResourceHandle = ov.plane.into();
        let mut set = Vec::new();
        if let (Some(a), Some(prop)) = (ov.alpha_pending.take(), ov.alpha) {
            ov.alpha_now = Some(a);
            ov.touched = true;
            set.push((plane, prop, u64::from(a)));
        }
        let Some(change) = ov.pending.take() else {
            return set;
        };
        ov.on_screen = change;
        ov.touched = true;
        set.extend(match change {
            None => vec![(plane, ov.fb_id, 0), (plane, ov.crtc_id, 0)],
            Some(i) => {
                let (frame, (w, h)) = &ov.frames[i];
                let fb = u64::from(u32::from(RawResourceHandle::from(frame.fb)));
                let mut set = vec![(plane, ov.fb_id, fb), (plane, ov.crtc_id, crtc)];
                for (prop, v) in ov
                    .src
                    .iter()
                    .zip([0, 0, u64::from(*w) << 16, u64::from(*h) << 16])
                {
                    set.push((plane, *prop, v));
                }
                for (prop, v) in ov.dst.iter().zip([0, 0, u64::from(*w), u64::from(*h)]) {
                    set.push((plane, *prop, v));
                }
                set
            }
        });
        set
    }

    /// The overlay's commit state, to put back if a commit is refused:
    /// nothing changed, so its changes are still to come.
    fn overlay_state(&self) -> Option<OverlayState> {
        self.overlay
            .as_ref()
            .map(|o| (o.pending, o.on_screen, o.alpha_pending, o.alpha_now))
    }

    fn put_back_overlay(&mut self, state: Option<OverlayState>) {
        if let (Some(ov), Some(s)) = (self.overlay.as_mut(), state) {
            (ov.pending, ov.on_screen, ov.alpha_pending, ov.alpha_now) = s;
        }
    }

    /// Shares scanout frame `frame` as a dma-buf.
    ///
    /// # Errors
    ///
    /// If the frame is not a GBM scanout buffer, or cannot be exported.
    pub fn export(&self, frame: usize) -> Result<DmaBuf, Error> {
        let FrameBuffer::Gbm(bo) = &self.frames[frame].buffer else {
            return Err(Error::NotScanout);
        };
        let fd = bo.fd_for_plane(0).map_err(|_| Error::Io {
            what: "exporting a dma-buf",
            source: io::Error::other("GBM returned no fd"),
        })?;
        Ok(DmaBuf {
            fd,
            width: bo.width(),
            height: bo.height(),
            fourcc: bo.format() as u32,
            modifier: bo.modifier().into(),
            stride: bo.stride_for_plane(0),
            offset: bo.offset(0),
        })
    }

    /// Reads scanout frame `frame` back through the CPU, with
    /// `read(pixels, stride)`. For checks, not for every frame.
    ///
    /// # Errors
    ///
    /// If the frame is not a GBM scanout buffer, or cannot be mapped.
    pub fn read_scanout<T>(
        &self,
        frame: usize,
        read: impl FnOnce(&[u8], usize) -> T,
    ) -> Result<T, Error> {
        let FrameBuffer::Gbm(bo) = &self.frames[frame].buffer else {
            return Err(Error::NotScanout);
        };
        bo.map(0, 0, bo.width(), bo.height(), |m| {
            read(m.buffer(), m.stride() as usize)
        })
        .map_err(io("mapping a scanout buffer"))
    }

    /// Draws into frame `frame` with `draw(pixels, pitch)`.
    ///
    /// # Errors
    ///
    /// If the buffer cannot be mapped.
    pub fn draw(&mut self, frame: usize, draw: impl FnOnce(&mut [u8], usize)) -> Result<(), Error> {
        let FrameBuffer::Dumb(buffer) = &mut self.frames[frame].buffer else {
            return Err(Error::NotScanout);
        };
        let pitch = drm::buffer::Buffer::pitch(buffer) as usize;
        let mut map = self
            .card
            .map_dumb_buffer(buffer)
            .map_err(io("mapping a dumb buffer"))?;
        draw(map.as_mut(), pitch);
        Ok(())
    }

    /// Sets mode `index` and shows `frame`, full screen.
    ///
    /// # Errors
    ///
    /// [`Error::Busy`] if another program holds the display, else the
    /// driver's refusal.
    pub fn modeset(&mut self, index: usize, frame: usize) -> Result<(), Error> {
        let mode = self.modes[index];
        let (w, h) = mode.size();
        let blob = blob_id(
            self.card
                .create_property_blob(&mode)
                .map_err(io("creating the mode blob"))?,
        );

        let p = &self.props;
        let conn: RawResourceHandle = self.connector.into();
        let crtc: RawResourceHandle = self.crtc.into();
        let plane: RawResourceHandle = self.plane.into();
        let crtc_id = u64::from(u32::from(crtc));
        let mut set = vec![
            (conn, p.connector_crtc, crtc_id),
            (crtc, p.mode_id, blob),
            (crtc, p.active, 1),
            (
                plane,
                p.fb_id,
                u64::from(u32::from(RawResourceHandle::from(self.frames[frame].fb))),
            ),
            (plane, p.plane_crtc, crtc_id),
        ];
        // Source rectangle in 16.16 fixed point, destination in pixels.
        for (prop, v) in p
            .src
            .iter()
            .zip([0, 0, u64::from(w) << 16, u64::from(h) << 16])
        {
            set.push((plane, *prop, v));
        }
        for (prop, v) in p.dst.iter().zip([0, 0, u64::from(w), u64::from(h)]) {
            set.push((plane, *prop, v));
        }

        // Colour signalling goes in the same commit as the mode: a separate
        // commit would be a second modeset, and a second resync on the display.
        let mut hdr_blob = None;
        match &self.hdr {
            Some(hdr) => {
                let prop = p
                    .hdr_metadata
                    .ok_or(Error::MissingProperty("HDR_OUTPUT_METADATA"))?;
                let id = blob_id(
                    self.card
                        .create_property_blob(&hdr.blob())
                        .map_err(io("creating the HDR metadata blob"))?,
                );
                hdr_blob = Some(id);
                set.push((conn, prop, id));
                if let Some(cs) = &p.colorspace
                    && let Some(v) = cs.bt2020_rgb
                {
                    set.push((conn, cs.handle, v));
                }
                // 10 bits on the wire, as HDR10 is defined, where the link
                // carries them; 8 where it does not (`hdmi_bpc`).
                if let Some(bpc) = p.max_bpc {
                    set.push((conn, bpc, u64::from(timing(&mode).hdmi_bpc())));
                }
            }
            None => {
                // Say SDR explicitly, so nothing another program left on the
                // connector survives the modeset.
                if let Some(prop) = p.hdr_metadata {
                    set.push((conn, prop, 0));
                }
                let colorspace = p.colorspace.as_ref().and_then(|cs| {
                    if self.sdr_bt2020 {
                        cs.bt2020_rgb
                    } else {
                        cs.default
                    }
                    .map(|v| (cs.handle, v))
                });
                if let Some((handle, v)) = colorspace {
                    set.push((conn, handle, v));
                }
                // SDR BT.2020 is rendered at 10 bits too; left at the
                // kernel's 16, amdgpu picks the deepest format the sink
                // takes, which landed on YCbCr (docs/findings.md in the C++
                // version).
                if self.sdr_bt2020
                    && let Some(bpc) = p.max_bpc
                {
                    set.push((conn, bpc, u64::from(timing(&mode).hdmi_bpc())));
                }
            }
        }

        if !self.modeset {
            self.saved = self.current_values(&set);
        }
        // The overlay rides in the same commit, outside what is saved: it
        // was off before, and restore always takes it down.
        let overlay_before = self.overlay_state();
        set.extend(self.overlay_props());
        let req = request(&set);
        // Into or out of HDR the output goes dark first and comes up with the
        // new signalling, as from off, and as a player does when it switches.
        // At an unchanged mode amdgpu only rewrites the HDR InfoFrame on the
        // running signal ("Mode change not required"), and a JVC that never
        // relocks never reads it: on 2026-10-14 one stayed in SDR through
        // three live switches into HDR10, and in HDR after a live switch
        // back, while a start from off was followed every time (#103). The
        // display resyncs on these switches anyway. Other colour changes
        // (Colorspace alone) stay live.
        let hdr_now = self.hdr.is_some();
        let relight = goes_dark_first(self.modeset, self.hdr_on_screen, hdr_now);
        let dark_from = Instant::now();
        // A test commit first: if another program holds the display, or the
        // new state would be refused, nothing on screen changes (or goes
        // dark) before we find out.
        let result = commit(
            &self.card,
            AtomicCommitFlags::TEST_ONLY | AtomicCommitFlags::ALLOW_MODESET,
            req.clone(),
        )
        .and_then(|()| {
            if relight {
                // Inactive, planes left bound: the next commit lights it.
                commit(
                    &self.card,
                    AtomicCommitFlags::ALLOW_MODESET,
                    request(&[(crtc, self.props.active, 0)]),
                )
            } else {
                Ok(())
            }
        })
        .and_then(|()| commit(&self.card, AtomicCommitFlags::ALLOW_MODESET, req));
        self.last_dark = None;
        if relight && result.is_ok() {
            self.relights += 1;
            self.last_dark = Some(dark_from.elapsed());
        }
        if let Err(e) = result {
            self.put_back_overlay(overlay_before);
            for id in [Some(blob), hdr_blob].into_iter().flatten() {
                let _ = self.card.destroy_property_blob(id);
            }
            return Err(e);
        }

        if let Some(old) = self.mode_blob.replace(blob) {
            let _ = self.card.destroy_property_blob(old);
        }
        if let Some(old) = std::mem::replace(&mut self.hdr_blob, hdr_blob) {
            let _ = self.card.destroy_property_blob(old);
        }
        self.modeset = true;
        self.hdr_on_screen = Some(hdr_now);
        Ok(())
    }

    /// Modesets that took the output dark first, into or out of HDR (see
    /// [`Self::modeset`]).
    pub fn relights(&self) -> u64 {
        self.relights
    }

    /// How long the last modeset left the output dark, if it went through
    /// dark: from the inactive commit to the output lit again.
    pub fn last_dark(&self) -> Option<Duration> {
        self.last_dark
    }

    /// HDR metadata for the next [`Self::modeset`], or `None` for SDR.
    pub fn set_hdr(&mut self, hdr: Option<HdrStatic>) {
        self.hdr = hdr;
    }

    /// Whether SDR from the next [`Self::modeset`] on is signalled as BT.2020
    /// (`Colorspace` BT2020_RGB) rather than the default. HDR is always
    /// BT.2020.
    pub fn set_sdr_bt2020(&mut self, on: bool) {
        self.sdr_bt2020 = on;
    }

    /// Changes the HDR metadata while already in HDR, carried by the next
    /// [`Self::flip`]. That flip is not allowed to modeset, so if the driver
    /// would need one for this change, the flip fails instead of the display
    /// resyncing.
    ///
    /// # Errors
    ///
    /// If the connector has no HDR property, or the blob cannot be created.
    pub fn update_hdr(&mut self, hdr: &HdrStatic) -> Result<(), Error> {
        if self.props.hdr_metadata.is_none() {
            return Err(Error::MissingProperty("HDR_OUTPUT_METADATA"));
        }
        let id = blob_id(
            self.card
                .create_property_blob(&hdr.blob())
                .map_err(io("creating the HDR metadata blob"))?,
        );
        if let Some(old) = self.pending_hdr_blob.replace(id) {
            let _ = self.card.destroy_property_blob(old);
        }
        self.hdr = Some(*hdr);
        Ok(())
    }

    /// The connector's HDR signalling as it stands, read back from the
    /// driver: whether a metadata blob is set, the colorspace, and max bpc.
    pub fn hdr_state(&self) -> String {
        let Ok(values) = self.card.get_properties(self.connector) else {
            return "unreadable".into();
        };
        let (ids, vals) = values.as_props_and_values();
        let value = |h: Option<property::Handle>| {
            h.and_then(|h| ids.iter().position(|&id| id == h).map(|i| vals[i]))
        };
        let metadata = match value(self.props.hdr_metadata) {
            None => "no HDR property".to_string(),
            Some(0) => "no HDR metadata".to_string(),
            Some(_) => "HDR metadata set".to_string(),
        };
        let colorspace = self
            .props
            .colorspace
            .as_ref()
            .and_then(|cs| {
                let v = value(Some(cs.handle))?;
                enum_name(&self.card, cs.handle, v)
            })
            .unwrap_or_else(|| "none".into());
        let bpc = value(self.props.max_bpc).map_or("none".into(), |v| v.to_string());
        format!("{metadata}, Colorspace {colorspace}, max bpc {bpc}")
    }

    /// Queues `frame` for the next vblank. Wait for it with [`Self::wait_flip`]
    /// before queueing another.
    ///
    /// # Errors
    ///
    /// If the driver refuses the flip.
    pub fn flip(&mut self, frame: usize) -> Result<(), Error> {
        let fb = u64::from(u32::from(RawResourceHandle::from(self.frames[frame].fb)));
        let mut set = vec![(self.plane.into(), self.props.fb_id, fb)];
        let overlay_before = self.overlay_state();
        set.extend(self.overlay_props());
        let pending = self.pending_hdr_blob.take();
        if let (Some(id), Some(prop)) = (pending, self.props.hdr_metadata) {
            set.push((self.connector.into(), prop, id));
        }
        if let Err(e) = commit(
            &self.card,
            AtomicCommitFlags::PAGE_FLIP_EVENT | AtomicCommitFlags::NONBLOCK,
            request(&set),
        ) {
            // Refused: nothing changed, so the overlay change is still to come.
            self.put_back_overlay(overlay_before);
            self.pending_hdr_blob = pending;
            return Err(e);
        }
        if let Some(id) = pending
            && let Some(old) = self.hdr_blob.replace(id)
        {
            let _ = self.card.destroy_property_blob(old);
        }
        Ok(())
    }

    /// Blocks until the queued flip completes, or [`FLIP_TIMEOUT`] passes.
    ///
    /// # Errors
    ///
    /// If reading events from the device fails, or no flip event arrives in
    /// time ([`Error::NoFlip`]). Without the deadline a lost event would hang
    /// the frame loop where nothing reads the stop flag, so a SIGTERM would
    /// end in a kill and the display would never be put back.
    pub fn wait_flip(&self) -> Result<Flip, Error> {
        let started = Instant::now();
        let flip = wait_readable(self.card.as_fd(), FLIP_TIMEOUT, || {
            Ok(self.card.receive_events()?.find_map(|event| match event {
                Event::PageFlip(e) => Some(Flip {
                    sequence: e.frame,
                    at: e.duration,
                    waited: started.elapsed(),
                }),
                _ => None,
            }))
        })
        .map_err(io("reading DRM events"))?;
        flip.ok_or(Error::NoFlip(FLIP_TIMEOUT))
    }

    /// Puts back every property the modeset changed, except HDR: leaving
    /// always clears the HDR metadata and sets `Colorspace` to `Default`, and
    /// never re-applies an earlier program's metadata, which may be stale. If
    /// the old values no longer apply (another program's mode blob is gone,
    /// say), turns the output off instead, still clearing HDR and putting
    /// `max bpc` back.
    ///
    /// # Errors
    ///
    /// If neither the old state nor "off" can be committed. The output then
    /// still counts as changed, so a later call (or the drop) tries again.
    pub fn restore(&mut self) -> Result<Restored, Error> {
        if !self.modeset {
            return Ok(Restored::NothingChanged);
        }
        let p = &self.props;
        let conn: RawResourceHandle = self.connector.into();
        let clear_hdr = |prop: property::Handle, value: u64| {
            if Some(prop) == p.hdr_metadata {
                Some(0)
            } else if let Some(cs) = &p.colorspace
                && prop == cs.handle
            {
                Some(cs.default.unwrap_or(value))
            } else {
                None
            }
        };

        // The overlay goes down either way if this run put anything on it:
        // it was free or ours when found (`find_overlay`).
        let overlay_off: Vec<_> = self
            .overlay
            .as_ref()
            .filter(|ov| ov.touched)
            .map(|ov| {
                let plane: RawResourceHandle = ov.plane.into();
                let mut off = vec![(plane, ov.fb_id, 0), (plane, ov.crtc_id, 0)];
                // Opaque again, its default, for whoever uses the plane next.
                if let Some(a) = ov.alpha {
                    off.push((plane, a, 0xffff));
                }
                off
            })
            .unwrap_or_default();
        let mut previous: Vec<_> = self
            .saved
            .iter()
            .map(|&(obj, prop, value)| (obj, prop, clear_hdr(prop, value).unwrap_or(value)))
            .collect();
        previous.extend(overlay_off.iter().copied());
        let outcome = commit(
            &self.card,
            AtomicCommitFlags::ALLOW_MODESET,
            request(&previous),
        )
        .map(|()| Restored::Previous)
        .or_else(|_| {
            let mut off = vec![
                (conn, p.connector_crtc, 0),
                (self.crtc.into(), p.active, 0),
                (self.crtc.into(), p.mode_id, 0),
                (self.plane.into(), p.fb_id, 0),
                (self.plane.into(), p.plane_crtc, 0),
            ];
            off.extend(overlay_off.iter().copied());
            // Off is not enough on its own: HDR signalling and max bpc
            // belong to the connector and outlive the mode.
            for &(obj, prop, value) in &self.saved {
                if let Some(v) = clear_hdr(prop, value) {
                    off.push((obj, prop, v));
                } else if Some(prop) == p.max_bpc {
                    off.push((obj, prop, value));
                }
            }
            commit(&self.card, AtomicCommitFlags::ALLOW_MODESET, request(&off))
                .map(|()| Restored::TurnedOff)
        });
        let outcome = outcome?;
        self.modeset = false;
        self.hdr_on_screen = None;
        if let Some(ov) = self.overlay.as_mut() {
            ov.on_screen = None;
            ov.pending = None;
            ov.alpha_pending = None;
            ov.alpha_now = None;
            ov.touched = false;
        }
        for blob in [
            self.mode_blob.take(),
            self.hdr_blob.take(),
            self.pending_hdr_blob.take(),
        ]
        .into_iter()
        .flatten()
        {
            let _ = self.card.destroy_property_blob(blob);
        }
        Ok(outcome)
    }

    fn current_values(
        &self,
        set: &[(RawResourceHandle, property::Handle, u64)],
    ) -> Vec<(RawResourceHandle, property::Handle, property::RawValue)> {
        let connector = self.card.get_properties(self.connector).ok();
        let crtc = self.card.get_properties(self.crtc).ok();
        let plane = self.card.get_properties(self.plane).ok();
        set.iter()
            .filter_map(|&(obj, prop, _)| {
                let values = if obj == self.connector.into() {
                    connector.as_ref()
                } else if obj == self.crtc.into() {
                    crtc.as_ref()
                } else {
                    plane.as_ref()
                }?;
                let (ids, vals) = values.as_props_and_values();
                let i = ids.iter().position(|&id| id == prop)?;
                Some((obj, prop, vals[i]))
            })
            .collect()
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        let _ = self.restore();
        // Destroyed even if restoring failed: removing a framebuffer that is
        // on screen makes the kernel turn the output off, and closing the
        // device below releases everything anyway. Keeping them gains nothing.
        let overlay_frames = self
            .overlay
            .as_mut()
            .map(|ov| std::mem::take(&mut ov.frames))
            .unwrap_or_default();
        for f in std::mem::take(&mut self.frames)
            .into_iter()
            .chain(overlay_frames.into_iter().map(|(f, _)| f))
        {
            let _ = self.card.destroy_framebuffer(f.fb);
            if let FrameBuffer::Dumb(b) = f.buffer {
                let _ = self.card.destroy_dumb_buffer(b);
            }
            // A GBM buffer object frees itself when dropped.
        }
    }
}

/// Whether a modeset goes dark first (see [`Output::modeset`]): only on a lit
/// output, and only into or out of HDR. `lit` is whether the output is lit,
/// `on_screen` whether it signals HDR (`None` before anything is committed),
/// and `hdr_now` whether the new state does. Metadata changes within HDR,
/// colorimetry-only changes and the first modeset stay as they were.
fn goes_dark_first(lit: bool, on_screen: Option<bool>, hdr_now: bool) -> bool {
    lit && on_screen.is_some_and(|h| h != hdr_now)
}

/// Calls `read` each time `fd` becomes readable until it returns a value, or
/// `timeout` passes (`Ok(None)`). `read` must not block once `fd` polls
/// readable.
fn wait_readable<T>(
    fd: BorrowedFd<'_>,
    timeout: Duration,
    mut read: impl FnMut() -> io::Result<Option<T>>,
) -> io::Result<Option<T>> {
    let deadline = Instant::now() + timeout;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(None);
        }
        // Rounded up, so a wait never ends a hair before the deadline and
        // spins on a zero timeout.
        let ms = i32::try_from(left.as_micros().div_ceil(1000)).unwrap_or(i32::MAX);
        let mut fds = [PollFd::new(&fd, PollFlags::IN)];
        match poll(&mut fds, ms) {
            Ok(0) | Err(rustix::io::Errno::INTR) => continue,
            Ok(_) => {}
            Err(e) => return Err(e.into()),
        }
        // An error or hang-up alone would make `read` block on a device
        // with nothing to read.
        if !fds[0].revents().contains(PollFlags::IN) {
            return Err(io::Error::other(format!(
                "poll reported {:?}",
                fds[0].revents()
            )));
        }
        if let Some(v) = read()? {
            return Ok(Some(v));
        }
    }
}

fn blob_id(value: property::Value<'static>) -> u64 {
    match value {
        property::Value::Blob(id) => id,
        _ => unreachable!("create_property_blob returns a blob"),
    }
}

fn colorspace_prop(card: &Card, handle: property::Handle) -> ColorspaceProp {
    let value = |name: &str| {
        let info = card.get_property(handle).ok()?;
        let property::ValueType::Enum(e) = info.value_type() else {
            return None;
        };
        let (_, enums) = e.values();
        enums
            .iter()
            .find(|v| v.name().to_str() == Ok(name))
            .map(|v| v.value())
    };
    ColorspaceProp {
        handle,
        default: value("Default"),
        bt2020_rgb: value("BT2020_RGB"),
    }
}

fn enum_name(card: &Card, handle: property::Handle, value: u64) -> Option<String> {
    let info = card.get_property(handle).ok()?;
    let property::ValueType::Enum(e) = info.value_type() else {
        return None;
    };
    let (_, enums) = e.values();
    enums
        .iter()
        .find(|v| v.value() == value)
        .map(|v| v.name().to_string_lossy().into_owned())
}

fn request(set: &[(RawResourceHandle, property::Handle, u64)]) -> AtomicModeReq {
    let mut req = AtomicModeReq::new();
    for &(obj, prop, value) in set {
        req.add_raw_property(obj, prop, value);
    }
    req
}

fn commit(card: &Card, flags: AtomicCommitFlags, req: AtomicModeReq) -> Result<(), Error> {
    card.atomic_commit(flags, req)
        .map_err(|e| match e.raw_os_error() {
            // EACCES / EPERM: not DRM master.
            Some(13) | Some(1) => Error::Busy,
            _ => Error::Io {
                what: "atomic commit",
                source: e,
            },
        })
}

fn props_by_name<T: drm::control::ResourceHandle>(
    card: &Card,
    handle: T,
) -> Result<HashMap<String, property::Handle>, Error> {
    let map = card
        .get_properties(handle)
        .map_err(io("reading properties"))?
        .as_hashmap(card)
        .map_err(io("reading property names"))?;
    Ok(map.into_iter().map(|(k, v)| (k, v.handle())).collect())
}

/// The first overlay plane that can sit on `crtc` and takes ARGB8888, with
/// the properties a commit needs. A plane on another CRTC is skipped:
/// putting it on ours would take it off that output. One already on ours is
/// taken, since a run that ended without restoring leaves it there.
fn find_overlay(
    card: &Card,
    res: &drm::control::ResourceHandles,
    crtc: crtc::Handle,
) -> Option<Overlay> {
    let plane = card.plane_handles().ok()?.into_iter().find(|&p| {
        card.get_plane(p).is_ok_and(|info| {
            info.crtc().is_none_or(|c| c == crtc)
                && res.filter_crtcs(info.possible_crtcs()).contains(&crtc)
                && info.formats().contains(&(DrmFourcc::Argb8888 as u32))
        }) && plane_type(card, p) == Some(PlaneType::Overlay)
    })?;
    let props = props_by_name(card, plane).ok()?;
    let get = |name: &str| props.get(name).copied();
    Some(Overlay {
        plane,
        fb_id: get("FB_ID")?,
        crtc_id: get("CRTC_ID")?,
        src: [get("SRC_X")?, get("SRC_Y")?, get("SRC_W")?, get("SRC_H")?],
        dst: [
            get("CRTC_X")?,
            get("CRTC_Y")?,
            get("CRTC_W")?,
            get("CRTC_H")?,
        ],
        frames: Vec::new(),
        pending: None,
        on_screen: None,
        alpha: get("alpha"),
        alpha_pending: None,
        alpha_now: None,
        touched: false,
    })
}

fn plane_type(card: &Card, plane: plane::Handle) -> Option<PlaneType> {
    let props = card.get_properties(plane).ok()?;
    let (ids, vals) = props.as_props_and_values();
    ids.iter().zip(vals).find_map(|(&id, &val)| {
        let info = card.get_property(id).ok()?;
        (info.name().to_str() == Ok("type")).then_some(match val {
            v if v == PlaneType::Primary as u64 => PlaneType::Primary,
            v if v == PlaneType::Cursor as u64 => PlaneType::Cursor,
            _ => PlaneType::Overlay,
        })
    })
}

fn timing(m: &Mode) -> ModeTiming {
    let (width, height) = m.size();
    ModeTiming {
        width,
        height,
        clock_khz: m.clock(),
        htotal: m.hsync().2,
        vtotal: m.vsync().2,
        interlaced: m.flags().contains(ModeFlags::INTERLACE),
        preferred: m.mode_type().contains(ModeTypeFlags::PREFERRED),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;

    #[test]
    fn only_a_switch_into_or_out_of_hdr_goes_dark_first() {
        // Into and out of HDR on a lit output: dark first (#103).
        assert!(goes_dark_first(true, Some(false), true));
        assert!(goes_dark_first(true, Some(true), false));
        // Staying in HDR (a metadata change) or in SDR (Colorspace alone,
        // or a rate change): no dark step.
        assert!(!goes_dark_first(true, Some(true), true));
        assert!(!goes_dark_first(true, Some(false), false));
        // The first modeset, and one after the output was restored: it
        // comes up from off anyway.
        assert!(!goes_dark_first(false, None, true));
        assert!(!goes_dark_first(false, Some(false), true));
        assert!(!goes_dark_first(true, None, true));
    }

    #[test]
    fn a_source_that_never_delivers_times_out() {
        let (quiet, _other_end) = UnixStream::pair().unwrap();
        let started = Instant::now();
        let got = wait_readable(quiet.as_fd(), Duration::from_millis(50), || {
            panic!("read called with nothing to read")
        })
        .unwrap();
        let waited = started.elapsed();
        assert_eq!(got, None::<()>);
        assert!(waited >= Duration::from_millis(50), "{waited:?}");
        assert!(waited < Duration::from_secs(1), "{waited:?}");
    }

    #[test]
    fn events_that_are_not_the_one_wanted_still_time_out() {
        // A vblank-only stream: readable over and over, never a flip.
        let (quiet, mut other_end) = UnixStream::pair().unwrap();
        let mut reader = quiet.try_clone().unwrap();
        other_end.write_all(&[0; 64]).unwrap();
        let mut reads = 0;
        let got = wait_readable(quiet.as_fd(), Duration::from_millis(50), || {
            reads += 1;
            let mut b = [0];
            reader.read_exact(&mut b)?;
            Ok(None::<()>)
        })
        .unwrap();
        assert_eq!(got, None);
        assert_eq!(reads, 64);
    }

    #[test]
    fn a_delivered_event_returns_at_once() {
        let (quiet, mut other_end) = UnixStream::pair().unwrap();
        let mut reader = quiet.try_clone().unwrap();
        other_end.write_all(&[7]).unwrap();
        let started = Instant::now();
        let got = wait_readable(quiet.as_fd(), Duration::from_secs(5), || {
            let mut b = [0];
            reader.read_exact(&mut b)?;
            Ok(Some(b[0]))
        })
        .unwrap();
        assert_eq!(got, Some(7));
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
