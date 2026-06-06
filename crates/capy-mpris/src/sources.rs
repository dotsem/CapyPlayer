//! Multi-source tracking and favorite management

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

/// A discovered MPRIS player source
#[derive(Clone, Debug)]
pub struct PlayerSource {
    /// Full D-Bus name, e.g. "org.mpris.MediaPlayer2.spotify"
    pub bus_name: String,
    /// Identity from MPRIS, e.g. "Spotify"
    pub identity: String,
    /// Short name extracted from bus name, e.g. "spotify"
    pub short_name: String,
    pub can_play: bool,
    pub can_pause: bool,
    pub can_seek: bool,
}

impl PlayerSource {
    /// Extract short name from full bus name
    /// "org.mpris.MediaPlayer2.spotify" -> "spotify"
    /// "org.mpris.MediaPlayer2.firefox.instance_1_234" -> "firefox"
    pub fn extract_short_name(bus_name: &str) -> String {
        bus_name
            .strip_prefix("org.mpris.MediaPlayer2.")
            .unwrap_or(bus_name)
            .split('.')
            .next()
            .unwrap_or(bus_name)
            .to_string()
    }
}

/// User preference for source selection
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SourcePreference {
    /// Short name of favorite source (e.g. "spotify")
    pub favorite: Option<String>,
}

impl SourcePreference {
    /// Load from config file, or return default if not found
    pub fn load(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Save to config file
    pub fn save(&self, path: &Path) -> Result<(), std::io::Error> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        fs::write(path, json)
    }

    /// Select the best source from available list
    pub fn select_source<'a>(&self, sources: &'a [PlayerSource]) -> Option<&'a PlayerSource> {
        // If favorite exists and is available, use it
        if let Some(fav) = &self.favorite {
            if let Some(src) = sources.iter().find(|s| &s.short_name == fav) {
                return Some(src);
            }
        }
        // Otherwise use first available
        sources.first()
    }

    /// Set favorite source
    pub fn set_favorite(&mut self, short_name: String) {
        self.favorite = Some(short_name);
    }

    /// Clear favorite
    pub fn clear_favorite(&mut self) {
        self.favorite = None;
    }
}
