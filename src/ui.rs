use crate::CapySpellPlayer;
use crate::config::Settings;
use crate::state::ServerState;
use slint::ComponentHandle;
use std::{cell::RefCell, rc::Rc};

pub fn apply_skin_and_layout(ui: &CapySpellPlayer, settings: &Settings) {
    ui.set_skin(settings.skin.as_str().into());

    let skin_cfg = ui.get_current_skin();
    let width = skin_cfg.width * settings.scale;
    let height = skin_cfg.height * settings.scale;
    log::info!(
        "Applying skin: {}, Scale: {}, Width: {}, Height: {}",
        settings.skin,
        settings.scale,
        width,
        height
    );
    ui.window().set_size(slint::LogicalSize::new(width, height));

    let max_dim = (width.max(height)).round() as u32;
    capy_player::mpris::set_album_size(max_dim);
    capy_player::mpris::set_blur_album_size(max_dim);
}

pub fn setup_callbacks(ui: &CapySpellPlayer, server_state: Rc<RefCell<ServerState>>) {
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
}
