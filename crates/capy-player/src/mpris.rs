//! MPRIS media player monitoring service.
//!
//! Uses capy-mpris for D-Bus communication (single connection, no memory leaks).
//! Handles album art processing and event bus integration.

use crate::events;
use capy_mpris::{MprisClient, MprisData as ClientMprisData, PlayerCommand, PlayerSource};
use image::imageops::FilterType;
use log::{error, info, warn};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::RwLock;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use tokio::sync::mpsc;

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
}

const CACHE_DIR: &str = ".cache/CapyShell/thumbs";
const CONFIG_DIR: &str = ".config/capyshell";

// Generation counter to handle race conditions for async image loading
static GENERATION: AtomicU64 = AtomicU64::new(0);

// Global command sender for UI callbacks
static COMMAND_SENDER: OnceLock<mpsc::Sender<PlayerCommand>> = OnceLock::new();

/// Get the command sender for sending playback commands
pub fn get_command_sender() -> Option<&'static mpsc::Sender<PlayerCommand>> {
    COMMAND_SENDER.get()
}

static ALBUM_SIZE: AtomicU32 = AtomicU32::new(256);
static BLUR_ALBUM_SIZE: AtomicU32 = AtomicU32::new(128);

/// Set album art size
/// Album art is assumed to be a square.
pub fn set_album_size(size: u32) {
    ALBUM_SIZE.store(size, Ordering::Relaxed)
}

/// Get album art size
/// Album art is assumed to be a square.
pub fn get_album_size() -> u32 {
    ALBUM_SIZE.load(Ordering::Relaxed)
}

/// Set blurred album art size
/// Blurred album art is assumed to be a square.
pub fn set_blur_album_size(size: u32) {
    BLUR_ALBUM_SIZE.store(size, Ordering::Relaxed)
}

/// Get blurred album art size
/// Blurred album art is assumed to be a square.
pub fn get_blur_album_size() -> u32 {
    BLUR_ALBUM_SIZE.load(Ordering::Relaxed)
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
    let cached_art_paths = Arc::new(RwLock::new((String::new(), String::new()))); // (art_path, blur_path)

    let on_update = move |data: ClientMprisData| {
        let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;

        // Detect track change by comparing art URL
        let is_track_change = {
            let last = last_art_url.read().unwrap();
            *last != data.art_url
        };

        // Update last art URL on track change
        if is_track_change {
            *last_art_url.write().unwrap() = data.art_url.clone();
            // Clear cached paths on track change
            *cached_art_paths.write().unwrap() = (String::new(), String::new());
        }

        info!(
            "MPRIS update: title='{}', playing={}, pos={:.1}s, track_change={}",
            data.title,
            data.status.is_playing(),
            data.position_us as f64 / 1_000_000.0,
            is_track_change
        );

        // Get cached art paths if available
        let (cached_art, cached_blur) = {
            let paths = cached_art_paths.read().unwrap();
            (paths.0.clone(), paths.1.clone())
        };

        // Send update with current art paths (may be empty on first update)
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
        };
        events::send_mpris(immediate_data);

        // Process album art asynchronously if:
        // 1. Track changed OR
        // 2. We don't have cached art but art URL is available
        let should_process_art =
            !data.art_url.is_empty() && (is_track_change || cached_art.is_empty());

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
            let cached_art_paths_clone = cached_art_paths.clone();

            tokio::task::spawn_blocking(move || {
                if GENERATION.load(Ordering::SeqCst) != generation {
                    return;
                }

                if let Some((art_path, blur_path)) = process_album_art(&art_url, &cache_dir_clone) {
                    if GENERATION.load(Ordering::SeqCst) != generation {
                        return;
                    }

                    // Cache the processed paths
                    *cached_art_paths_clone.write().unwrap() =
                        (art_path.clone(), blur_path.clone());

                    let data_with_art = MprisData {
                        title,
                        artist,
                        album,
                        album_art_path: art_path,
                        blurred_art_path: blur_path,
                        length_secs,
                        position_secs,
                        is_playing,
                        has_media: true,
                        is_track_change: false,
                        position_timestamp_ms,
                        source_name,
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
                // Store the sender globally for UI callbacks
                let _ = COMMAND_SENDER.set(sender);
                info!("MPRIS client started, command sender available");

                // Keep running - the client loop runs in a spawned task
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                }
            }
            Err(e) => {
                warn!("Failed to start MPRIS client: {}. Retrying in 2s...", e);
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        }

        // Clear state when client exits
        GENERATION.fetch_add(1, Ordering::SeqCst);
        events::send_mpris(MprisData::default());
    }
}

fn process_album_art(url: &str, cache_dir: &Path) -> Option<(String, String)> {
    let start_time = std::time::Instant::now();
    info!("Processing album art for URL: {}", url);
    let mut hasher = Sha256::new();
    hasher.update(url.as_bytes());
    let hash = hex::encode(hasher.finalize());

    let original_path = cache_dir.join(format!("{}.png", hash));
    let blur_path = cache_dir.join(format!("{}_blur.png", hash));

    // Check if already cached on disk
    if original_path.exists() && blur_path.exists() {
        info!(
            "Album art cache hit on disk: {} (took {:?})",
            url,
            start_time.elapsed()
        );
        return Some((
            original_path.to_string_lossy().to_string(),
            blur_path.to_string_lossy().to_string(),
        ));
    }

    info!("Downloading album art: {}", url);
    let download_start = std::time::Instant::now();
    let img_data = if url.starts_with("file://") {
        let path = url.strip_prefix("file://").unwrap();
        fs::read(path).ok()?
    } else if url.starts_with("http") {
        match ureq::get(url).call() {
            Ok(response) => {
                let mut bytes = Vec::new();
                if response
                    .into_body()
                    .as_reader()
                    .read_to_end(&mut bytes)
                    .is_err()
                {
                    warn!("Failed to read downloaded body for: {}", url);
                    return None;
                }
                bytes
            }
            Err(e) => {
                warn!("Failed to download album art from {}: {}", url, e);
                return None;
            }
        }
    } else {
        warn!("Unsupported album art URL scheme: {}", url);
        return None;
    };
    info!(
        "Downloaded/read album art bytes in {:?}",
        download_start.elapsed()
    );

    let process_start = std::time::Instant::now();
    let img = image::load_from_memory(&img_data).ok()?;

    let album_size = get_album_size();
    let blur_album_size = get_blur_album_size();

    let resized = img.resize_to_fill(album_size, album_size, FilterType::CatmullRom);
    resized.save(&original_path).ok()?;

    let blur_base = img.resize_to_fill(blur_album_size, blur_album_size, FilterType::Triangle);
    let blurred = blur_base.blur(4.0);
    blurred.save(&blur_path).ok()?;

    info!(
        "Processed, blurred and saved album art in {:?}",
        process_start.elapsed()
    );
    info!(
        "Total album art processing time: {:?}",
        start_time.elapsed()
    );

    Some((
        original_path.to_string_lossy().to_string(),
        blur_path.to_string_lossy().to_string(),
    ))
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
