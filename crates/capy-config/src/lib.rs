use log::{info, warn};
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{de::DeserializeOwned, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{mpsc, Arc, Mutex, RwLock},
    time::Duration,
};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Serialization error: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("Deserialization error: {0}")]
    Deserialize(#[from] toml::de::Error),
    #[error("Filesystem watcher error: {0}")]
    Watcher(#[from] notify::Error),
    #[error("Unable to resolve system config directory")]
    NoConfigDir,
}

type ChangeCallback<T> = Box<dyn Fn(T) + Send + Sync + 'static>;

#[derive(Clone)]
pub struct ConfigStore<T> {
    path: PathBuf,
    data: Arc<RwLock<T>>,
    listeners: Arc<RwLock<Vec<ChangeCallback<T>>>>,
    _watcher: Arc<Mutex<Option<RecommendedWatcher>>>,
}

impl<T> ConfigStore<T>
where
    T: Serialize + DeserializeOwned + Default + Clone + Send + Sync + 'static,
{
    pub fn open(app_name: &str) -> Result<Self, ConfigError> {
        let base_dir = dirs::config_dir().ok_or(ConfigError::NoConfigDir)?;
        let path = base_dir.join(app_name).join("config.toml");
        Self::open_at(path)
    }

    pub fn open_at(path: PathBuf) -> Result<Self, ConfigError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let initial_data = if path.exists() {
            let content = fs::read_to_string(&path)?;
            toml::from_str(&content)?
        } else {
            let default_val = T::default();
            let toml_str = toml::to_string_pretty(&default_val)?;
            fs::write(&path, toml_str)?;
            default_val
        };

        let data = Arc::new(RwLock::new(initial_data));
        let listeners: Arc<RwLock<Vec<ChangeCallback<T>>>> = Arc::new(RwLock::new(Vec::new()));

        let watch_dir = path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));

        let (tx, rx) = mpsc::channel();
        let watcher_res = RecommendedWatcher::new(
            move |res: Result<Event, notify::Error>| {
                if let Ok(event) = res {
                    let _ = tx.send(event);
                }
            },
            Config::default(),
        );

        let watcher_holder = match watcher_res {
            Ok(mut watcher) => {
                if let Err(err) = watcher.watch(&watch_dir, RecursiveMode::NonRecursive) {
                    warn!("failed to watch config dir {}: {err}", watch_dir.display());
                    None
                } else {
                    let path_clone = path.clone();
                    let data_clone = Arc::clone(&data);
                    let listeners_clone = Arc::clone(&listeners);

                    std::thread::spawn(move || {
                        while let Ok(event) = rx.recv() {
                            let is_target = event
                                .paths
                                .iter()
                                .any(|p| p.ends_with("config.toml") || p == &path_clone);
                            if !is_target {
                                continue;
                            }

                            // why: debounce intermediate writes (e.g. vim swap/truncate)
                            std::thread::sleep(Duration::from_millis(150));
                            while rx.try_recv().is_ok() {}

                            match fs::read_to_string(&path_clone) {
                                Ok(content) => match toml::from_str::<T>(&content) {
                                    Ok(new_val) => {
                                        info!("Configuration reloaded from disk");
                                        if let Ok(mut lock) = data_clone.write() {
                                            *lock = new_val.clone();
                                        }
                                        if let Ok(cbs) = listeners_clone.read() {
                                            for cb in cbs.iter() {
                                                cb(new_val.clone());
                                            }
                                        }
                                    }
                                    Err(err) => {
                                        // why: incomplete writes shouldn't overwrite valid state
                                        warn!("Ignoring invalid config syntax: {err}");
                                    }
                                },
                                Err(err) => {
                                    warn!("Failed to read config file during reload: {err}")
                                }
                            }
                        }
                    });
                    Some(watcher)
                }
            }
            Err(err) => {
                warn!("failed to create filesystem watcher: {err}");
                None
            }
        };

        Ok(Self {
            path,
            data,
            listeners,
            _watcher: Arc::new(Mutex::new(watcher_holder)),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn get(&self) -> T {
        self.data.read().unwrap().clone()
    }

    pub fn update<R>(&self, f: impl FnOnce(&mut T) -> R) -> Result<R, ConfigError> {
        let mut guard = self.data.write().unwrap();
        let result = f(&mut guard);
        let toml_str = toml::to_string_pretty(&*guard)?;
        fs::write(&self.path, toml_str)?;

        if let Ok(cbs) = self.listeners.read() {
            for cb in cbs.iter() {
                cb(guard.clone());
            }
        }

        Ok(result)
    }

    pub fn on_change<F>(&self, callback: F)
    where
        F: Fn(T) + Send + Sync + 'static,
    {
        if let Ok(mut cbs) = self.listeners.write() {
            cbs.push(Box::new(callback));
        }
    }
}
