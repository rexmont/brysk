//! DirectLook's platform pieces on macOS, mirroring `rf-win::look`.
//!
//! The viewers are the same Rust code on both platforms; what differs is how the selection to
//! preview is found (Explorer vs Finder) and how the window is placed. Finder integration and
//! the Quick Look key hook come with the DirectLook milestone; until then DirectLook works
//! from its own window and from paths it is given.

pub mod decode;
pub mod hook;
pub mod keys;
pub mod pdf;
pub mod target;
pub mod win;