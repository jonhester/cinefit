//! Display output through Linux KMS, with the atomic API.
//!
//! Talking to the display is Linux only; mode timing and the test pattern are
//! plain arithmetic, so they build and test everywhere.

pub mod hdr;
pub mod pattern;
pub mod timing;

#[cfg(target_os = "linux")]
mod output;

pub use hdr::{Eotf, HdrStatic};
#[cfg(target_os = "linux")]
pub use output::{Error, Flip, Output, Restored};
pub use timing::{ModeRequest, ModeTiming, Pacing, choose_mode};
