use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub skin: String,
    pub scale: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            skin: "Vinyl".to_string(),
            scale: 1.0,
        }
    }
}
