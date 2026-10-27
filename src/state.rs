use std::time::Instant;

#[derive(Default, Clone)]
pub struct ServerState {
    pub position_secs: f32,
    pub is_playing: bool,
    pub length_secs: f32,
    pub title_hash: u64,
    pub updated_at: Option<Instant>,
    pub has_media: bool,
}

pub struct InterpolationState {
    pub base_position: f32,
    pub base_time: Instant,
    pub last_server_update: Option<Instant>,
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

pub fn hash_string(s: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}
