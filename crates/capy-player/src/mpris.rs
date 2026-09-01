//! MPRIS media player monitoring service.
//!
//! Uses capy-mpris for D-Bus communication (single connection, no memory leaks).
//! Handles album art processing and event bus integration.

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};

use capy_mpris::{MprisClient, MprisData as ClientMprisData, PlayerCommand, PlayerSource};
use log::{error, info, warn};
use tokio::sync::mpsc;

use crate::art::{self, ArtPalette};
use crate::events;

pub use crate::art::{get_album_size, get_blur_album_size, set_album_size, set_blur_album_size};

/// Data sent to UI for display
#[derive(Clone, Debug, Default)]
pub struct MprisData {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_art_path: String,
    pub blurred_art_path: String,
    pub length_secs: f32,
    pub position_secs: f32,
    pub is_playing: bool,
    pub has_media: bool,
    pub is_track_change: bool,
    /// Timestamp when position was fetched (for client-side interpolation)
    pub position_timestamp_ms: u64,
    /// Current source short name
    pub source_name: String,
    pub palette: ArtPalette,
}

const CACHE_DIR: &str = ".cache/CapyShell/thumbs";
const CONFIG_DIR: &str = ".config/capyshell";

// Global command sender for UI callbacks
static COMMAND_SENDER: OnceLock<mpsc::Sender<PlayerCommand>> = OnceLock::new();

/// Get the command sender for sending playback commands
pub fn get_command_sender() -> Option<&'static mpsc::Sender<PlayerCommand>> {
    COMMAND_SENDER.get()
}

/// Send a command to the MPRIS client
pub fn send_command(cmd: PlayerCommand) {
    if let Some(sender) = COMMAND_SENDER.get() {
        let _ = sender.try_send(cmd);
    } else {
        warn!("MPRIS command sender not initialized");
    }
}

pub fn start() {
    std::thread::Builder::new()
        .name("mpris-monitor".to_string())
        .spawn(|| {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    error!("Failed to create tokio runtime for MPRIS: {}", e);
                    return;
                }
            };
            rt.block_on(run_mpris_loop());
        })
        .expect("Failed to spawn mpris thread");
}

async fn run_mpris_loop() {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let cache_dir = PathBuf::from(&home).join(CACHE_DIR);
    let config_path = PathBuf::from(&home).join(CONFIG_DIR).join("mpris.json");

    if let Err(e) = fs::create_dir_all(&cache_dir) {
        error!("Failed to create cache dir: {}", e);
        return;
    }

    info!("Starting MPRIS D-Bus client with capy-mpris");

    // Shared state for tracking art processing
    let cache_dir_for_update = cache_dir.clone();
    let last_art_url = Arc::new(RwLock::new(String::new()));
    let cached_art_data = Arc::new(RwLock::new((
        String::new(),
        String::new(),
        ArtPalette::default(),
    )));

    let on_update = move |data: ClientMprisData| {
        let is_track_change = {
            let mut last = last_art_url.write().unwrap();
            if *last != data.art_url {
                *last = data.art_url.clone();
                true
            } else {
                false
            }
        };

        if is_track_change {
            *cached_art_data.write().unwrap() =
                (String::new(), String::new(), ArtPalette::default());
        }

        info!(
            "MPRIS update: title='{}', playing={}, pos={:.1}s, track_change={}",
            data.title,
            data.status.is_playing(),
            data.position_us as f64 / 1_000_000.0,
            is_track_change
        );

        // Get cached art data if available
        let (cached_art, cached_blur, cached_palette) = {
            let paths = cached_art_data.read().unwrap();
            (paths.0.clone(), paths.1.clone(), paths.2.clone())
        };

        // Send update with current art data (may be empty on first update)
        let immediate_data = MprisData {
            title: data.title.clone(),
            artist: data.artist.clone(),
            album: data.album.clone(),
            album_art_path: cached_art.clone(),
            blurred_art_path: cached_blur.clone(),
            length_secs: data.length_secs(),
            position_secs: data.position_us as f32 / 1_000_000.0,
            is_playing: data.status.is_playing(),
            has_media: true,
            is_track_change,
            position_timestamp_ms: data.position_timestamp_ms,
            source_name: data.source_name.clone(),
            palette: cached_palette,
        };
        events::send_mpris(immediate_data);

        let should_process_art = !data.art_url.is_empty() && is_track_change;

        if should_process_art {
            let art_url = data.art_url.clone();
            let title = data.title.clone();
            let artist = data.artist.clone();
            let album = data.album.clone();
            let length_secs = data.length_secs();
            let position_secs = data.position_us as f32 / 1_000_000.0;
            let is_playing = data.status.is_playing();
            let position_timestamp_ms = data.position_timestamp_ms;
            let source_name = data.source_name.clone();
            let cache_dir_clone = cache_dir_for_update.clone();
            let cached_art_data_clone = cached_art_data.clone();
            let last_art_url_clone = last_art_url.clone();

            tokio::task::spawn_blocking(move || {
                if *last_art_url_clone.read().unwrap() != art_url {
                    return;
                }

                if let Some(processed) = art::process_album_art(&art_url, &cache_dir_clone) {
                    if *last_art_url_clone.read().unwrap() != art_url {
                        return;
                    }

                    // Cache the processed art paths and palette
                    *cached_art_data_clone.write().unwrap() = (
                        processed.art_path.clone(),
                        processed.blur_path.clone(),
                        processed.palette.clone(),
                    );

                    let data_with_art = MprisData {
                        title,
                        artist,
                        album,
                        album_art_path: processed.art_path,
                        blurred_art_path: processed.blur_path,
                        length_secs,
                        position_secs,
                        is_playing,
                        has_media: true,
                        is_track_change: false,
                        position_timestamp_ms,
                        source_name,
                        palette: processed.palette,
                    };
                    events::send_mpris(data_with_art);
                }
            });
        }
    };

    let on_sources_changed = |sources: Vec<PlayerSource>, active: Option<String>| {
        info!(
            "MPRIS sources: {:?}, active: {:?}",
            sources.iter().map(|s| &s.short_name).collect::<Vec<_>>(),
            active
        );
    };

    loop {
        match MprisClient::start(
            on_update.clone(),
            on_sources_changed.clone(),
            Some(config_path.clone()),
        )
        .await
        {
            Ok(sender) => {
                let _ = COMMAND_SENDER.set(sender);
                info!("MPRIS client started, command sender available");

                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                }
            }
            Err(e) => {
                warn!("Failed to start MPRIS client: {}. Retrying in 2s...", e);
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        }

        events::send_mpris(MprisData::default());
    }
}

pub fn play_pause() {
    send_command(PlayerCommand::PlayPause);
}

pub fn next() {
    send_command(PlayerCommand::Next);
}

pub fn prev() {
    send_command(PlayerCommand::Previous);
}

pub fn seek(position_us: i64) {
    send_command(PlayerCommand::SetPosition(position_us));
}
