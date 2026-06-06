//! Core types for capy-mpris

use std::time::{SystemTime, UNIX_EPOCH};

/// Playback status from MPRIS player
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum PlaybackStatus {
    Playing,
    Paused,
    #[default]
    Stopped,
}

impl PlaybackStatus {
    pub fn from_str(s: &str) -> Self {
        match s {
            "Playing" => PlaybackStatus::Playing,
            "Paused" => PlaybackStatus::Paused,
            _ => PlaybackStatus::Stopped,
        }
    }

    pub fn is_playing(&self) -> bool {
        matches!(self, PlaybackStatus::Playing)
    }
}

/// Commands that can be sent to the media player
#[derive(Clone, Debug)]
pub enum PlayerCommand {
    PlayPause,
    Next,
    Previous,
    Seek(i64),            // Offset in microseconds
    SetPosition(i64),     // Absolute position in microseconds
    SwitchSource(String), // Switch to named source (short name)
    SetFavorite(String),  // Set favorite source (short name)
    ClearFavorite,        // Clear favorite
}

/// Media player state snapshot with interpolation info
#[derive(Clone, Debug, Default)]
pub struct MprisData {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub art_url: String,
    pub length_us: i64,
    pub status: PlaybackStatus,
    pub track_id: Option<String>,

    // For client-side interpolation
    /// Last known position from D-Bus (microseconds)
    pub position_us: i64,
    /// When position was fetched (unix millis)
    pub position_timestamp_ms: u64,

    // Source info
    /// Short name, e.g. "spotify", "firefox"
    pub source_name: String,
    /// Full D-Bus name, e.g. "org.mpris.MediaPlayer2.spotify"
    pub source_bus_name: String,
}

impl MprisData {
    /// Get current interpolated position in microseconds
    pub fn interpolated_position_us(&self) -> i64 {
        if !self.status.is_playing() {
            return self.position_us;
        }

        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(self.position_timestamp_ms);

        let elapsed_ms = now_ms.saturating_sub(self.position_timestamp_ms);
        let elapsed_us = (elapsed_ms * 1000) as i64;

        (self.position_us + elapsed_us).min(self.length_us).max(0)
    }

    /// Get current interpolated position in seconds
    pub fn interpolated_position_secs(&self) -> f32 {
        self.interpolated_position_us() as f32 / 1_000_000.0
    }

    /// Get length in seconds
    pub fn length_secs(&self) -> f32 {
        self.length_us as f32 / 1_000_000.0
    }
}
