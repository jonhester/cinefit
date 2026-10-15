//! Display output through Linux KMS, with the atomic API.
//!
//! Talking to the display is Linux only; mode timing and the test pattern are
//! plain arithmetic, so they build and test everywhere.

#[cfg(target_os = "linux")]
mod display;
pub mod edid;
pub mod hdr;
pub mod pattern;
pub mod scanout;
pub mod timing;

#[cfg(target_os = "linux")]
mod output;

#[cfg(target_os = "linux")]
pub use display::describe as describe_displays;
pub use hdr::{Eotf, HdrStatic};
#[cfg(target_os = "linux")]
pub use output::{DmaBuf, Error, FLIP_LATE, FLIP_TIMEOUT, Flip, Output, Restored};
pub use timing::{ModeRequest, ModeTiming, Pacing, choose_mode};
