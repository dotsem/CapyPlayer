//! Error types for capy-mpris

/// MPRIS client errors
#[derive(Debug, thiserror::Error)]
pub enum MprisError {
    #[error("D-Bus error: {0}")]
    DBus(#[from] zbus::Error),

    #[error("D-Bus fdo error: {0}")]
    Fdo(#[from] zbus::fdo::Error),

    #[error("No MPRIS player found")]
    NoPlayer,

    #[error("Player disconnected")]
    Disconnected,

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Config error: {0}")]
    Config(String),
}
