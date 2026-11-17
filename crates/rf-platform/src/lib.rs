//! The platform layer of rustyfiles, by target.
//!
//! * Windows: re-exports `rf-win` (Win32, Shell COM, the file-operation engine).
//! * macOS: re-exports `rf-mac` (POSIX/AppKit; see docs/port-macos.md).
//!
//! The interface calls `rf_platform::<module>`; the module names and signatures are the same on
//! both platforms, so a feature missing on one is a typed stub returning empty/`None`, never a
//! `cfg` in `rf-app`.

#[cfg(windows)]
pub use rf_win::*;

#[cfg(target_os = "macos")]
pub use rf_mac::*;