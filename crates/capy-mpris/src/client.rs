//! MPRIS client implementation
//!
//! Single D-Bus connection with proper cleanup:
//! - Listen to property signals as triggers
//! - When triggered, fetch ALL properties fresh
//! - Explicit stream cleanup on session exit

use crate::error::MprisError;
use crate::sources::{PlayerSource, SourcePreference};
use crate::types::{MprisData, PlaybackStatus, PlayerCommand};
use futures_util::StreamExt;
use log::{debug, info, warn};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use zbus::Connection;
use zbus::zvariant::OwnedValue;

const MPRIS_PREFIX: &str = "org.mpris.MediaPlayer2.";

/// D-Bus proxy for MPRIS player interface
#[zbus::proxy(
    interface = "org.mpris.MediaPlayer2.Player",
    default_path = "/org/mpris/MediaPlayer2"
)]
trait MprisPlayer {
    // Methods
    fn play_pause(&self) -> zbus::Result<()>;
    fn next(&self) -> zbus::Result<()>;
    fn previous(&self) -> zbus::Result<()>;
    fn seek(&self, offset: i64) -> zbus::Result<()>;
    fn set_position(
        &self,
        track_id: &zbus::zvariant::ObjectPath<'_>,
        position: i64,
    ) -> zbus::Result<()>;

    // Properties
    #[zbus(property)]
    fn metadata(&self) -> zbus::Result<HashMap<String, OwnedValue>>;

    #[zbus(property)]
    fn playback_status(&self) -> zbus::Result<String>;

    #[zbus(property(emits_changed_signal = "false"))]
    fn position(&self) -> zbus::Result<i64>;

    #[zbus(property)]
    fn can_seek(&self) -> zbus::Result<bool>;

    // Signal for property changes - we use this as a TRIGGER only
    #[zbus(signal)]
    fn seeked(&self, position: i64) -> zbus::Result<()>;
}

/// D-Bus proxy for MPRIS root interface
#[zbus::proxy(
    interface = "org.mpris.MediaPlayer2",
    default_path = "/org/mpris/MediaPlayer2"
)]
trait MprisRoot {
    #[zbus(property)]
    fn identity(&self) -> zbus::Result<String>;
}

/// MPRIS client
pub struct MprisClient;

impl MprisClient {
    /// Start the MPRIS client.
    pub async fn start<F, G>(
        on_update: F,
        on_sources_changed: G,
        config_path: Option<PathBuf>,
    ) -> Result<mpsc::Sender<PlayerCommand>, MprisError>
    where
        F: Fn(MprisData) + Send + Sync + 'static,
        G: Fn(Vec<PlayerSource>, Option<String>) + Send + Sync + 'static,
    {
        let connection = Connection::session().await?;
        let (cmd_tx, cmd_rx) = mpsc::channel::<PlayerCommand>(32);

        // Load preferences
        let preference = config_path
            .as_ref()
            .map(|p| SourcePreference::load(p))
            .unwrap_or_default();

        // Find initial player
        let sources = discover_sources(&connection).await?;
        let active_bus = preference
            .select_source(&sources)
            .map(|s| s.bus_name.clone());

        on_sources_changed(sources.clone(), active_bus.clone());

        // Spawn the main loop
        let on_update = Arc::new(on_update);
        let on_sources_changed = Arc::new(on_sources_changed);

        tokio::spawn(async move {
            run_loop(
                connection,
                active_bus,
                cmd_rx,
                preference,
                config_path,
                on_update,
                on_sources_changed,
            )
            .await;
        });

        Ok(cmd_tx)
    }
}

/// Main event loop - simplified like the Dart version
async fn run_loop<F, G>(
    connection: Connection,
    mut active_bus: Option<String>,
    mut cmd_rx: mpsc::Receiver<PlayerCommand>,
    mut preference: SourcePreference,
    config_path: Option<PathBuf>,
    on_update: Arc<F>,
    on_sources_changed: Arc<G>,
) where
    F: Fn(MprisData) + Send + Sync + 'static,
    G: Fn(Vec<PlayerSource>, Option<String>) + Send + Sync + 'static,
{
    loop {
        // Clone the bus name first to avoid borrow conflicts
        let current_bus = active_bus.clone();

        if let Some(bus_name) = current_bus {
            info!("Connecting to player: {}", bus_name);

            match run_player_session(
                &connection,
                &bus_name,
                &mut cmd_rx,
                &mut preference,
                &config_path,
                &on_update,
                &on_sources_changed,
                &mut active_bus,
            )
            .await
            {
                Ok(()) => {
                    info!("Player session ended normally");
                }
                Err(e) => {
                    warn!("Player session error: {}", e);
                }
            }

            // Player disconnected - try to find new one
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            if let Ok(sources) = discover_sources(&connection).await {
                active_bus = preference
                    .select_source(&sources)
                    .map(|s| s.bus_name.clone());
                on_sources_changed(sources, active_bus.clone());
            }
        } else {
            // No player - wait and poll
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            if let Ok(sources) = discover_sources(&connection).await {
                active_bus = preference
                    .select_source(&sources)
                    .map(|s| s.bus_name.clone());
                if active_bus.is_some() {
                    on_sources_changed(sources, active_bus.clone());
                }
            }
        }
    }
}

/// Run a session connected to a specific player
async fn run_player_session<F, G>(
    connection: &Connection,
    bus_name: &str,
    cmd_rx: &mut mpsc::Receiver<PlayerCommand>,
    preference: &mut SourcePreference,
    config_path: &Option<PathBuf>,
    on_update: &Arc<F>,
    on_sources_changed: &Arc<G>,
    active_bus: &mut Option<String>,
) -> Result<(), MprisError>
where
    F: Fn(MprisData) + Send + Sync + 'static,
    G: Fn(Vec<PlayerSource>, Option<String>) + Send + Sync + 'static,
{
    // Create proxy for this player
    let proxy = MprisPlayerProxy::builder(connection)
        .destination(bus_name)?
        .build()
        .await?;

    // Subscribe to property change signals - zbus generates these from #[zbus(property)]
    let mut status_stream = proxy.receive_playback_status_changed().await;
    let mut metadata_stream = proxy.receive_metadata_changed().await;
    let mut seeked_stream = proxy.receive_seeked().await?;

    // Wait a bit for UI to initialize and subscribe to event bus
    // This fixes race condition where initial state is sent before UI subscribes
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // Send initial state
    fetch_and_send_state(bus_name, &proxy, on_update).await;

    // Send again after a short delay to catch any late-subscribing UIs
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    fetch_and_send_state(bus_name, &proxy, on_update).await;

    let result = run_session_loop(
        connection,
        bus_name,
        &proxy,
        &mut status_stream,
        &mut metadata_stream,
        &mut seeked_stream,
        cmd_rx,
        preference,
        config_path,
        on_update,
        on_sources_changed,
        active_bus,
    )
    .await;

    // CRITICAL: Explicit cleanup to prevent subscription accumulation
    // Drop streams explicitly before returning
    drop(status_stream);
    drop(metadata_stream);
    drop(seeked_stream);
    // Yield to allow async cleanup tasks to run
    tokio::task::yield_now().await;

    result
}

/// Inner session loop - separated for cleaner stream cleanup
async fn run_session_loop<F, G>(
    connection: &Connection,
    bus_name: &str,
    proxy: &MprisPlayerProxy<'_>,
    status_stream: &mut zbus::PropertyStream<'_, String>,
    metadata_stream: &mut zbus::PropertyStream<'_, HashMap<String, OwnedValue>>,
    seeked_stream: &mut SeekedStream<'_>,
    cmd_rx: &mut mpsc::Receiver<PlayerCommand>,
    preference: &mut SourcePreference,
    config_path: &Option<PathBuf>,
    on_update: &Arc<F>,
    on_sources_changed: &Arc<G>,
    active_bus: &mut Option<String>,
) -> Result<(), MprisError>
where
    F: Fn(MprisData) + Send + Sync + 'static,
    G: Fn(Vec<PlayerSource>, Option<String>) + Send + Sync + 'static,
{
    loop {
        tokio::select! {
            // PlaybackStatus changed
            Some(_) = status_stream.next() => {
                debug!("PlaybackStatus changed signal received");
                fetch_and_send_state(bus_name, proxy, on_update).await;
            }

            // Metadata changed (track change)
            Some(_) = metadata_stream.next() => {
                debug!("Metadata changed signal received");
                fetch_and_send_state(bus_name, proxy, on_update).await;
            }

            // Seeked signal
            Some(_) = seeked_stream.next() => {
                debug!("Seeked signal received");
                fetch_and_send_state(bus_name, proxy, on_update).await;
            }

            // Commands from UI
            Some(cmd) = cmd_rx.recv() => {
                match cmd {
                    PlayerCommand::PlayPause => {
                        debug!("Sending PlayPause command");
                        let _ = proxy.play_pause().await;
                        // Signal should trigger update, but add small delay + poll as backup
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        fetch_and_send_state(bus_name, proxy, on_update).await;
                    }
                    PlayerCommand::Next => {
                        debug!("Sending Next command");
                        let _ = proxy.next().await;
                        // Spotify may not emit signal when paused - poll after delay
                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                        fetch_and_send_state(bus_name, proxy, on_update).await;
                    }
                    PlayerCommand::Previous => {
                        debug!("Sending Previous command");
                        let _ = proxy.previous().await;
                        // Spotify may not emit signal when paused - poll after delay
                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                        fetch_and_send_state(bus_name, proxy, on_update).await;
                    }
                    PlayerCommand::Seek(offset) => {
                        debug!("Sending Seek command: offset={}us", offset);
                        let _ = proxy.seek(offset).await;
                        // Seeked signal should fire, but poll as backup
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        fetch_and_send_state(bus_name, proxy, on_update).await;
                    }
                    PlayerCommand::SetPosition(position) => {
                        debug!("Sending SetPosition command: position={}us", position);
                        if let Ok(metadata) = proxy.metadata().await {
                            if let Some(track_id) = extract_track_id(&metadata) {
                                if let Ok(path) = zbus::zvariant::ObjectPath::try_from(track_id.as_str()) {
                                    let _ = proxy.set_position(&path, position).await;
                                    // Seeked signal should fire, but poll as backup
                                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                                    fetch_and_send_state(bus_name, proxy, on_update).await;
                                }
                            }
                        }
                    }
                    PlayerCommand::SwitchSource(short_name) => {
                        if let Ok(sources) = discover_sources(connection).await {
                            if let Some(src) = sources.iter().find(|s| s.short_name == short_name) {
                                *active_bus = Some(src.bus_name.clone());
                                on_sources_changed(sources, active_bus.clone());
                                return Ok(()); // Exit to reconnect to new player
                            }
                        }
                    }
                    PlayerCommand::SetFavorite(short_name) => {
                        preference.set_favorite(short_name);
                        if let Some(path) = config_path {
                            let _ = preference.save(path);
                        }
                    }
                    PlayerCommand::ClearFavorite => {
                        preference.clear_favorite();
                        if let Some(path) = config_path {
                            let _ = preference.save(path);
                        }
                    }
                }
            }

            // Timeout - check if player still exists (increased to 30s to reduce churn)
            _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => {
                // Ping the player to check if still alive
                if proxy.playback_status().await.is_err() {
                    warn!("Player {} no longer responding", bus_name);
                    return Err(MprisError::Disconnected);
                }
            }
        }
    }
}

/// Fetch all properties and send update - like Dart's getPlayerData()
#[inline]
async fn fetch_and_send_state<F>(bus_name: &str, proxy: &MprisPlayerProxy<'_>, on_update: &Arc<F>)
where
    F: Fn(MprisData) + Send + Sync + 'static,
{
    // Fetch all properties fresh - this is the key insight from Dart
    let metadata = proxy.metadata().await.unwrap_or_default();
    let status_str = proxy.playback_status().await.unwrap_or_default();
    let position_us = proxy.position().await.unwrap_or(0);

    debug!(
        "Fetched state: status='{}', position={}us",
        status_str, position_us
    );

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let data = MprisData {
        title: extract_string(&metadata, "xesam:title").unwrap_or_default(),
        artist: extract_str_array(&metadata, "xesam:artist").unwrap_or_default(),
        album: extract_string(&metadata, "xesam:album").unwrap_or_default(),
        art_url: extract_string(&metadata, "mpris:artUrl").unwrap_or_default(),
        length_us: extract_i64(&metadata, "mpris:length").unwrap_or(0),
        status: PlaybackStatus::from_str(&status_str),
        track_id: extract_track_id(&metadata),
        position_us,
        position_timestamp_ms: now_ms,
        source_name: PlayerSource::extract_short_name(bus_name),
        source_bus_name: bus_name.to_string(),
    };

    on_update(data);
}

/// Discover all MPRIS players
async fn discover_sources(connection: &Connection) -> Result<Vec<PlayerSource>, MprisError> {
    let dbus_proxy = zbus::fdo::DBusProxy::new(connection).await?;
    let names = dbus_proxy.list_names().await?;

    let mut sources = Vec::new();

    for name in names.iter().filter(|n| n.starts_with(MPRIS_PREFIX)) {
        let bus_name = name.to_string();
        let short_name = PlayerSource::extract_short_name(&bus_name);

        // Try to get identity
        let identity = match MprisRootProxy::builder(connection)
            .destination(bus_name.as_str())?
            .build()
            .await
        {
            Ok(proxy) => proxy
                .identity()
                .await
                .unwrap_or_else(|_| short_name.clone()),
            Err(_) => short_name.clone(),
        };

        // Try to get capabilities
        let can_seek = match MprisPlayerProxy::builder(connection)
            .destination(bus_name.as_str())?
            .build()
            .await
        {
            Ok(proxy) => proxy.can_seek().await.unwrap_or(false),
            Err(_) => false,
        };

        sources.push(PlayerSource {
            bus_name,
            identity,
            short_name,
            can_play: true,
            can_pause: true,
            can_seek,
        });
    }

    // Sort: prefer spotify
    sources.sort_by(|a, b| {
        let a_spotify = a.short_name == "spotify";
        let b_spotify = b.short_name == "spotify";
        match (a_spotify, b_spotify) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.short_name.cmp(&b.short_name),
        }
    });

    Ok(sources)
}

// ============ Metadata extraction helpers ============

#[inline]
fn extract_string(map: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    use std::ops::Deref;
    use zbus::zvariant::Value;

    map.get(key).and_then(|v| match v.deref() {
        Value::Str(s) => Some(s.to_string()),
        _ => None,
    })
}

#[inline]
fn extract_i64(map: &HashMap<String, OwnedValue>, key: &str) -> Option<i64> {
    use std::ops::Deref;
    use zbus::zvariant::Value;

    map.get(key).and_then(|v| match v.deref() {
        Value::I64(i) => Some(*i),
        Value::U64(u) => Some(*u as i64),
        Value::I32(i) => Some(*i as i64),
        Value::U32(u) => Some(*u as i64),
        _ => None,
    })
}

#[inline]
fn extract_str_array(map: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    use std::ops::Deref;
    use zbus::zvariant::Value;

    map.get(key).and_then(|v| match v.deref() {
        Value::Array(arr) => {
            let strings: Vec<String> = arr
                .iter()
                .filter_map(|item| match item {
                    Value::Str(s) => Some(s.to_string()),
                    _ => None,
                })
                .collect();
            if strings.is_empty() {
                None
            } else {
                Some(strings.join(", "))
            }
        }
        _ => None,
    })
}

#[inline]
fn extract_track_id(map: &HashMap<String, OwnedValue>) -> Option<String> {
    use std::ops::Deref;
    use zbus::zvariant::Value;

    map.get("mpris:trackid").and_then(|v| match v.deref() {
        Value::ObjectPath(p) => Some(p.to_string()),
        Value::Str(s) => Some(s.to_string()),
        _ => None,
    })
}
