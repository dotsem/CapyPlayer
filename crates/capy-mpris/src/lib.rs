//! capy-mpris - Pure zbus MPRIS client library
//!
//! Features:
//! - Single D-Bus connection (no memory leaks)
//! - Client-side time interpolation
//! - Multi-source support with favorites

pub mod client;
pub mod error;
pub mod sources;
pub mod types;

pub use client::MprisClient;
pub use error::MprisError;
pub use sources::{PlayerSource, SourcePreference};
pub use types::{MprisData, PlaybackStatus, PlayerCommand};
