//! Holds the GPU at amdgpu's "standard" stable power state while alive.
//!
//! Under the default clock policy the GPU never ramps up for cinefit's load, a
//! few milliseconds of work every 41.7, and runs it at about half the speed it
//! manages back to back. Forcing the level through sysfs needs root and
//! outlives the process (an experiment's setting was once found still applied
//! hours later). So this asks for it the way the kernel offers it per process:
//! a stable pstate on an amdgpu context (`AMDGPU_CTX_OP_SET_STABLE_PSTATE`).
//!
//! That needs only the render node, no capability, and the kernel undoes it:
//! freeing the context, including the implicit free when a crashed or killed
//! process's file is closed, restores the level in force before. Nothing can
//! strand it.
//!
//! Only one context on the device may hold a stable pstate; another holder
//! makes [`StablePstate::acquire`] fail with [`PowerError::Busy`].

use std::ffi::{c_int, c_void};
use std::fmt;
use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::path::Path;

const AMDGPU_CTX_OP_SET_STABLE_PSTATE: u32 = 6;
const AMDGPU_CTX_STABLE_PSTATE_NONE: u32 = 0;
const AMDGPU_CTX_STABLE_PSTATE_STANDARD: u32 = 1;
const EBUSY: c_int = 16;

#[link(name = "drm_amdgpu")]
unsafe extern "C" {
    fn amdgpu_device_initialize(
        fd: c_int,
        major: *mut u32,
        minor: *mut u32,
        device: *mut *mut c_void,
    ) -> c_int;
    fn amdgpu_device_deinitialize(device: *mut c_void) -> c_int;
    fn amdgpu_cs_ctx_create(device: *mut c_void, context: *mut *mut c_void) -> c_int;
    fn amdgpu_cs_ctx_free(context: *mut c_void) -> c_int;
    fn amdgpu_cs_ctx_stable_pstate(
        context: *mut c_void,
        op: u32,
        flags: u32,
        out_flags: *mut u32,
    ) -> c_int;
}

#[derive(Debug)]
pub enum PowerError {
    /// No render node belongs to amdgpu, or it cannot be opened (is this user
    /// in the render group?).
    NoAmdgpu,
    /// The level already reads `profile_standard`: something else forced it,
    /// a sysfs write or another process. The kernel would accept the request
    /// while holding nothing, so it is refused rather than reported as held.
    AlreadyForced,
    /// Another process holds a stable pstate.
    Busy,
    /// A libdrm_amdgpu call failed with this errno.
    Call { what: &'static str, errno: c_int },
}

impl fmt::Display for PowerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PowerError::NoAmdgpu => {
                f.write_str("no amdgpu render node (is this user in the render group?)")
            }
            PowerError::AlreadyForced => f.write_str(
                "the clock level is already profile_standard, forced elsewhere; not taking it",
            ),
            PowerError::Busy => f.write_str("another process holds a stable pstate"),
            PowerError::Call { what, errno } => {
                write!(f, "{what}: {}", std::io::Error::from_raw_os_error(*errno))
            }
        }
    }
}

impl std::error::Error for PowerError {}

/// The held stable pstate. Dropping it asks for none, then frees the context.
pub struct StablePstate {
    // Kept open for the device's lifetime: libdrm_amdgpu uses this fd.
    _file: File,
    device: *mut c_void,
    context: *mut c_void,
    node: String,
}

impl StablePstate {
    /// Finds the amdgpu render node and holds "standard".
    ///
    /// # Errors
    ///
    /// See [`PowerError`]; on any failure nothing is left held.
    pub fn acquire() -> Result<Self, PowerError> {
        let node = amdgpu_render_node().ok_or(PowerError::NoAmdgpu)?;
        // Checked before taking ownership: the kernel's set returns early when
        // the level already matches, so it would succeed holding nothing, and
        // the release would then save "standard" as the level to restore.
        if level_of(&node) == "profile_standard" {
            return Err(PowerError::AlreadyForced);
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(Path::new("/dev/dri").join(&node))
            .map_err(|_| PowerError::NoAmdgpu)?;

        let mut held = Self {
            _file: file,
            device: std::ptr::null_mut(),
            context: std::ptr::null_mut(),
            node,
        };
        let (mut major, mut minor) = (0, 0);
        // SAFETY: the fd is open for the struct's lifetime; the out pointers
        // are valid. On failure `Drop` frees whatever was created.
        let r = unsafe {
            amdgpu_device_initialize(
                held._file.as_raw_fd(),
                &mut major,
                &mut minor,
                &mut held.device,
            )
        };
        check("amdgpu_device_initialize", r)?;
        // SAFETY: the device was just initialized.
        let r = unsafe { amdgpu_cs_ctx_create(held.device, &mut held.context) };
        check("amdgpu_cs_ctx_create", r)?;
        let mut out = 0;
        // SAFETY: the context was just created.
        let r = unsafe {
            amdgpu_cs_ctx_stable_pstate(
                held.context,
                AMDGPU_CTX_OP_SET_STABLE_PSTATE,
                AMDGPU_CTX_STABLE_PSTATE_STANDARD,
                &mut out,
            )
        };
        if r == -EBUSY {
            return Err(PowerError::Busy);
        }
        check("setting the stable pstate", r)?;
        Ok(held)
    }

    /// The render node, e.g. `renderD128`.
    pub fn node(&self) -> &str {
        &self.node
    }

    /// The device's clock level as sysfs reports it, read fresh: while held,
    /// `profile_standard`.
    pub fn level(&self) -> String {
        level_of(&self.node)
    }
}

impl Drop for StablePstate {
    fn drop(&mut self) {
        if !self.context.is_null() {
            // Freeing the context alone restores the level in force when it
            // took ownership; if that was a leftover "manual", the kernel reads
            // it as no pstate and sets auto anyway. Asking for none first makes
            // the intent explicit rather than incidental.
            let mut out = 0;
            // SAFETY: the context is live and freed exactly once, here.
            unsafe {
                amdgpu_cs_ctx_stable_pstate(
                    self.context,
                    AMDGPU_CTX_OP_SET_STABLE_PSTATE,
                    AMDGPU_CTX_STABLE_PSTATE_NONE,
                    &mut out,
                );
                amdgpu_cs_ctx_free(self.context);
            }
        }
        if !self.device.is_null() {
            // SAFETY: the device is live and deinitialized exactly once.
            unsafe { amdgpu_device_deinitialize(self.device) };
        }
    }
}

/// libdrm_amdgpu returns a negative errno on failure.
fn check(what: &'static str, r: c_int) -> Result<(), PowerError> {
    if r == 0 {
        Ok(())
    } else {
        Err(PowerError::Call { what, errno: -r })
    }
}

/// The first `/dev/dri/renderD*` whose kernel driver is amdgpu.
fn amdgpu_render_node() -> Option<String> {
    let mut nodes: Vec<String> = std::fs::read_dir("/dev/dri")
        .ok()?
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.starts_with("renderD"))
        .collect();
    nodes.sort();
    nodes.into_iter().find(|n| {
        std::fs::read_link(format!("/sys/class/drm/{n}/device/driver"))
            .ok()
            .and_then(|p| p.file_name().map(|f| f == "amdgpu"))
            .unwrap_or(false)
    })
}

fn level_of(node: &str) -> String {
    std::fs::read_to_string(format!(
        "/sys/class/drm/{node}/device/power_dpm_force_performance_level"
    ))
    .map(|s| s.trim().to_owned())
    .unwrap_or_default()
}
