use slint::{ComponentHandle, SharedString};
use spell_framework::{
    self, cast_spell,
    layer_properties::{LayerAnchor, LayerType, WindowConf},
};
use std::{cell::RefCell, error::Error, rc::Rc, sync::mpsc, time::Instant};

slint::include_modules!();
spell_framework::generate_widgets![CapySpellPlayer];

mod animation;
mod config;
mod wayle_cava;

const WINDOW_WIDTH: u32 = 200;
const WINDOW_HEIGHT: u32 = 200;

#[derive(Default, Clone)]
struct ServerState {
    position_secs: f32,
    is_playing: bool,
    length_secs: f32,
    title_hash: u64,
    updated_at: Option<Instant>,
    has_media: bool,
}

struct InterpolationState {
    base_position: f32,
    base_time: Instant,
    last_server_update: Option<Instant>,
}

impl Default for InterpolationState {
    fn default() -> Self {
        Self {
            base_position: 0.0,
            base_time: Instant::now(),
            last_server_update: None,
        }
    }
}

fn hash_string(s: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

thread_local! {
    static ART_CACHE: RefCell<(String, slint::Image)> = RefCell::new((String::new(), slint::Image::default()));
    static BLUR_CACHE: RefCell<(String, slint::Image)> = RefCell::new((String::new(), slint::Image::default()));
}

fn load_image_cached(path: &str, has_media: bool, is_blur: bool) -> slint::Image {
    if !has_media || path.is_empty() {
        return slint::Image::default();
    }

    let slot = if is_blur { &BLUR_CACHE } else { &ART_CACHE };

    slot.with(|cell| {
        let cached = cell.borrow();
        if cached.0 == path {
            log::debug!("Image cache hit for: {} (is_blur={})", path, is_blur);
            return cached.1.clone();
        }
        drop(cached);

        let start = std::time::Instant::now();
        log::info!(
            "Image cache miss. Loading from disk: {} (is_blur={})",
            path,
            is_blur
        );
        let img = slint::Image::load_from_path(std::path::Path::new(path)).unwrap_or_default();
        log::info!("Loaded image in {:?}", start.elapsed());

        *cell.borrow_mut() = (path.to_string(), img.clone());
        img
    })
}

fn main() -> Result<(), Box<dyn Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    log::info!("Initializing capy-spell-player...");
    log::info!("Slint backend: {:?}", std::env::var("SLINT_BACKEND"));

    let window_conf = WindowConf::builder()
        .width(WINDOW_WIDTH)
        .height(WINDOW_HEIGHT)
        .anchor_1(LayerAnchor::BOTTOM)
        .anchor_2(LayerAnchor::RIGHT)
        .margins(5, 0, 0, 10)
        .layer_type(LayerType::Bottom)
        .build()
        .unwrap();

    let ui = CapySpellPlayerSpell::invoke_spell("capy-player", window_conf);

    ui.set_skin(config::get_skin().into());

    wayle_cava::start(ui.as_weak());

    let skin_cfg = ui.get_current_skin();
    let scale = config::get_scale();
    let width = skin_cfg.width * scale;
    let height = skin_cfg.height * scale;
    log::info!(
        "Skin: {}, Scale: {}, Width: {}, Height: {}",
        config::get_skin(),
        scale,
        width,
        height
    );
    ui.window().set_size(slint::LogicalSize::new(width, height));

    let max_dim = (width.max(height)).round() as u32;
    capy_player::mpris::set_album_size(max_dim);
    capy_player::mpris::set_blur_album_size(max_dim);

    capy_player::mpris::start();

    let server_state = Rc::new(RefCell::new(ServerState::default()));

    let (tx, rx) = mpsc::channel();

    let tx_clone = tx.clone();
    capy_player::events::register_listener(move |data| {
        let _ = tx_clone.send(data);
    });

    let interp_state = Rc::new(RefCell::new(InterpolationState::default()));
    let last_rendered_position = Rc::new(RefCell::new(0.0f32));
    let playback_motion = Rc::new(RefCell::new(animation::PlaybackMotion::default()));

    let ui_weak_timer = ui.as_weak();
    let interp_clone = interp_state.clone();
    let last_pos_clone = last_rendered_position.clone();
    let server_state_for_timer = server_state.clone();
    let motion_clone = playback_motion.clone();

    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(25),
        move || {
            if let Some(ui) = ui_weak_timer.upgrade() {
                let mut latest_data = None;
                while let Ok(data) = rx.try_recv() {
                    latest_data = Some(data);
                }

                if let Some(data) = latest_data {
                    log::info!(
                        "Processing update for: title='{}', artist='{}'",
                        data.title,
                        data.artist
                    );
                    let title_hash = hash_string(data.title.as_str());

                    {
                        let mut state = server_state_for_timer.borrow_mut();
                        state.position_secs = data.position_secs;
                        state.is_playing = data.is_playing;
                        state.length_secs = data.length_secs;
                        state.title_hash = title_hash;
                        state.updated_at = Some(Instant::now());
                        state.has_media = data.has_media;
                    }

                    let album_art =
                        load_image_cached(data.album_art_path.as_str(), data.has_media, false);
                    let blurred_art =
                        load_image_cached(data.blurred_art_path.as_str(), data.has_media, true);

                    let media_data = MediaData {
                        title: SharedString::from(data.title),
                        artist: SharedString::from(data.artist),
                        album: SharedString::from(data.album),
                        album_art,
                        blurred_art,
                        length_secs: data.length_secs,
                        position_secs: data.position_secs,
                        is_playing: data.is_playing,
                        has_media: data.has_media,
                        text_color: slint::Color::from_rgb_u8(255, 255, 255),
                    };
                    ui.set_media_data(media_data);
                    ui.set_position_secs(data.position_secs);
                }

                let server = server_state_for_timer.borrow().clone();
                let motion = motion_clone
                    .borrow_mut()
                    .update(server.has_media && server.is_playing);
                ui.set_vinyl_angle(motion.angle_deg);
                ui.set_animation_phase(motion.phase);

                if !server.has_media {
                    return;
                }

                let server_updated = server.updated_at;
                let needs_resync = {
                    let interp = interp_clone.borrow();
                    server_updated != interp.last_server_update
                };

                if needs_resync {
                    let mut interp = interp_clone.borrow_mut();
                    interp.base_position = server.position_secs;
                    interp.base_time = Instant::now();
                    interp.last_server_update = server_updated;
                }

                let new_position = {
                    let interp = interp_clone.borrow();
                    if server.is_playing {
                        let elapsed = interp.base_time.elapsed().as_secs_f32();
                        (interp.base_position + elapsed)
                            .min(server.length_secs)
                            .max(0.0)
                    } else {
                        server.position_secs
                    }
                };

                let last_pos = *last_pos_clone.borrow();
                let position_changed = (new_position - last_pos).abs() > 0.01;

                if position_changed {
                    *last_pos_clone.borrow_mut() = new_position;
                    ui.set_position_secs(new_position);
                }
            }
        },
    );
    // why: prevent timer from being dropped at end of main
    std::mem::forget(timer);

    ui.on_play_pause(|| {
        capy_player::mpris::play_pause();
    });

    ui.on_next(|| {
        capy_player::mpris::next();
    });

    ui.on_prev(|| {
        capy_player::mpris::prev();
    });

    ui.on_seek({
        let server_state_clone = server_state.clone();
        move |percent| {
            let length_secs = server_state_clone.borrow().length_secs;
            let position_us = (length_secs * percent * 1_000_000.0) as i64;
            capy_player::mpris::seek(position_us);
        }
    });

    cast_spell!(ui)
}
