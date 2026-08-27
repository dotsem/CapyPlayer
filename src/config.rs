use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const CONFIG_PATH: &str = ".config/capy-player/config.toml";

pub fn get_config_path() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(CONFIG_PATH)
    } else {
        PathBuf::from(CONFIG_PATH)
    }
}

#[macro_export]
macro_rules! register_settings {
    (
        $(
            $(#[$meta:meta])*
            $name:ident : $ty:ty = $default:expr
        ),* $(,)?
    ) => {
        #[allow(dead_code)]
        #[derive(Debug, Clone, Serialize, Deserialize)]
        #[serde(default)]
        pub struct Settings {
            $(
                $(#[$meta])*
                pub $name: $ty,
            )*
        }

        impl Default for Settings {
            fn default() -> Self {
                Self {
                    $(
                        $name: $default,
                    )*
                }
            }
        }

        #[allow(dead_code)]
        impl Settings {
            pub fn load() -> Self {
                let path = $crate::config::get_config_path();
                Self::load_from(&path).unwrap_or_else(|err| {
                    log::warn!("Failed to load config from {}: {err}. Using defaults.", path.display());
                    let default_settings = Self::default();
                    let _ = default_settings.save();
                    default_settings
                })
            }

            pub fn load_from(path: &std::path::Path) -> Result<Self, Box<dyn std::error::Error>> {
                if !path.exists() {
                    let settings = Self::default();
                    settings.save_to(path)?;
                    return Ok(settings);
                }

                let content = std::fs::read_to_string(path)?;
                let settings: Self = toml::from_str(&content)?;
                Ok(settings)
            }

            pub fn save(&self) -> Result<(), Box<dyn std::error::Error>> {
                self.save_to(&$crate::config::get_config_path())
            }

            pub fn save_to(&self, path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let toml_str = toml::to_string_pretty(self)?;
                std::fs::write(path, toml_str)?;
                Ok(())
            }

            $(
                paste::paste! {
                    pub fn [<get_ $name>](&self) -> &$ty {
                        &self.$name
                    }

                    pub fn [<set_ $name>](&mut self, val: $ty) {
                        self.$name = val;
                    }

                    pub fn [<set_ $name _and_save>](&mut self, val: $ty) -> Result<(), Box<dyn std::error::Error>> {
                        self.$name = val;
                        self.save()
                    }
                }
            )*
        }

        static GLOBAL_SETTINGS: std::sync::OnceLock<std::sync::RwLock<Settings>> = std::sync::OnceLock::new();

        #[allow(dead_code)]
        pub fn global() -> &'static std::sync::RwLock<Settings> {
            GLOBAL_SETTINGS.get_or_init(|| std::sync::RwLock::new(Settings::load()))
        }

        #[allow(dead_code)]
        pub fn get_settings() -> Settings {
            global().read().unwrap().clone()
        }

        #[allow(dead_code)]
        pub fn update<F, R>(f: F) -> Result<R, Box<dyn std::error::Error>>
        where
            F: FnOnce(&mut Settings) -> R,
        {
            let mut guard = global().write().unwrap();
            let result = f(&mut guard);
            guard.save()?;
            Ok(result)
        }

        $(
            paste::paste! {
                #[allow(dead_code)]
                pub fn [<get_ $name>]() -> $ty {
                    global().read().unwrap().$name.clone()
                }

                #[allow(dead_code)]
                pub fn [<set_ $name>](val: $ty) -> Result<(), Box<dyn std::error::Error>> {
                    update(|s| s.$name = val)
                }
            }
        )*
    };
}

register_settings! {
    skin: String = "Vinyl".to_string(),
    scale: f32 = 1.0,
}
