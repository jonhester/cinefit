//! One display output: a connector driven by a CRTC through its primary plane,
//! with the atomic API. Whatever this changes is put back when it drops.

use std::collections::HashMap;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::{AsFd, BorrowedFd};
use std::path::{Path, PathBuf};
use std::time::Duration;

use drm::buffer::DrmFourcc;
use drm::control::dumbbuffer::DumbBuffer;
use drm::control::{
    AtomicCommitFlags, Device as ControlDevice, Event, Mode, ModeFlags, ModeTypeFlags, PlaneType,
    RawResourceHandle, atomic::AtomicModeReq, connector, crtc, framebuffer, plane, property,
};
use drm::{ClientCapability, Device as BasicDevice};

use crate::hdr::HdrStatic;
use crate::timing::ModeTiming;

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
    /// The driver lacks a property every atomic driver should have.
    MissingProperty(&'static str),
    /// Another program holds the display (DRM master).
    Busy,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io { what, source } => write!(f, "{what}: {source}"),
            Error::NoDisplay => f.write_str("no DRM device has a connected display"),
            Error::NoCrtc => f.write_str("no CRTC can drive the connector"),
            Error::NoPlane => f.write_str("no primary plane for the CRTC"),
            Error::MissingProperty(p) => write!(f, "the driver has no {p} property"),
            Error::Busy => f.write_str("another program holds the display (DRM master)"),
        }
    }
}

impl std::error::Error for Error {}

fn io(what: &'static str) -> impl FnOnce(io::Error) -> Error {
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

/// A CPU-drawn scanout buffer.
pub struct DumbFrame {
    buffer: DumbBuffer,
    fb: framebuffer::Handle,
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
    frames: Vec<DumbFrame>,
    /// HDR metadata for the next modeset; `None` signals SDR.
    hdr: Option<HdrStatic>,
    /// The blob on the connector now, and one queued for the next flip.
    hdr_blob: Option<u64>,
    pending_hdr_blob: Option<u64>,
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
            hdr: None,
            hdr_blob: None,
            pending_hdr_blob: None,
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
        self.frames.push(DumbFrame { buffer, fb });
        Ok(self.frames.len() - 1)
    }

    /// Draws into frame `frame` with `draw(pixels, pitch)`.
    ///
    /// # Errors
    ///
    /// If the buffer cannot be mapped.
    pub fn draw(&mut self, frame: usize, draw: impl FnOnce(&mut [u8], usize)) -> Result<(), Error> {
        let f = &mut self.frames[frame];
        let pitch = drm::buffer::Buffer::pitch(&f.buffer) as usize;
        let mut map = self
            .card
            .map_dumb_buffer(&mut f.buffer)
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
                // 10 bits on the wire, as HDR10 is defined.
                if let Some(bpc) = p.max_bpc {
                    set.push((conn, bpc, 10));
                }
            }
            None => {
                // Say SDR explicitly, so nothing another program left on the
                // connector survives the modeset.
                if let Some(prop) = p.hdr_metadata {
                    set.push((conn, prop, 0));
                }
                if let Some(cs) = &p.colorspace
                    && let Some(v) = cs.default
                {
                    set.push((conn, cs.handle, v));
                }
            }
        }

        if !self.modeset {
            self.saved = self.current_values(&set);
        }
        let req = request(&set);
        // A test commit first: if another program holds the display, nothing
        // on screen changes before we find out.
        let result = commit(
            &self.card,
            AtomicCommitFlags::TEST_ONLY | AtomicCommitFlags::ALLOW_MODESET,
            req.clone(),
        )
        .and_then(|()| commit(&self.card, AtomicCommitFlags::ALLOW_MODESET, req));
        if let Err(e) = result {
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
        Ok(())
    }

    /// HDR metadata for the next [`Self::modeset`], or `None` for SDR.
    pub fn set_hdr(&mut self, hdr: Option<HdrStatic>) {
        self.hdr = hdr;
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
        let pending = self.pending_hdr_blob.take();
        if let (Some(id), Some(prop)) = (pending, self.props.hdr_metadata) {
            set.push((self.connector.into(), prop, id));
        }
        commit(
            &self.card,
            AtomicCommitFlags::PAGE_FLIP_EVENT | AtomicCommitFlags::NONBLOCK,
            request(&set),
        )?;
        if let Some(id) = pending
            && let Some(old) = self.hdr_blob.replace(id)
        {
            let _ = self.card.destroy_property_blob(old);
        }
        Ok(())
    }

    /// Blocks until the queued flip completes.
    ///
    /// # Errors
    ///
    /// If reading events from the device fails.
    pub fn wait_flip(&self) -> Result<Flip, Error> {
        loop {
            for event in self
                .card
                .receive_events()
                .map_err(io("reading DRM events"))?
            {
                if let Event::PageFlip(e) = event {
                    return Ok(Flip {
                        sequence: e.frame,
                        at: e.duration,
                    });
                }
            }
        }
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

        let previous: Vec<_> = self
            .saved
            .iter()
            .map(|&(obj, prop, value)| (obj, prop, clear_hdr(prop, value).unwrap_or(value)))
            .collect();
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
        for f in std::mem::take(&mut self.frames) {
            let _ = self.card.destroy_framebuffer(f.fb);
            let _ = self.card.destroy_dumb_buffer(f.buffer);
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
