use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{OnceLock, RwLock};

use hex;
use image::imageops::FilterType;
use log::{info, warn};
use material_colors::image::ImageReader;
use material_colors::theme::ThemeBuilder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

static ALBUM_SIZE: AtomicU32 = AtomicU32::new(256);
static BLUR_ALBUM_SIZE: AtomicU32 = AtomicU32::new(128);

/// In memory cache for palettes for fast access
static PALETTE_CACHE: OnceLock<RwLock<HashMap<String, ArtPalette>>> = OnceLock::new();

fn get_palette_cache() -> &'static RwLock<HashMap<String, ArtPalette>> {
    PALETTE_CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtPalette {
    pub dominant: (u8, u8, u8),
    pub on_dominant: (u8, u8, u8),
    pub is_dark: bool,
    pub primary: (u8, u8, u8),
    pub on_primary: (u8, u8, u8),
    pub surface: (u8, u8, u8),
    pub on_surface: (u8, u8, u8),
}

#[derive(Clone, Debug, Default)]
pub struct ProcessedArt {
    pub art_path: String,
    pub blur_path: String,
    pub palette: ArtPalette,
}

pub fn set_album_size(size: u32) {
    ALBUM_SIZE.store(size, Ordering::Relaxed)
}

pub fn get_album_size() -> u32 {
    ALBUM_SIZE.load(Ordering::Relaxed)
}

pub fn set_blur_album_size(size: u32) {
    BLUR_ALBUM_SIZE.store(size, Ordering::Relaxed)
}

pub fn get_blur_album_size() -> u32 {
    BLUR_ALBUM_SIZE.load(Ordering::Relaxed)
}

pub fn process_album_art(url: &str, cache_dir: &Path) -> Option<ProcessedArt> {
    let start_time = std::time::Instant::now();
    info!("Processing album art for URL: {}", url);

    let mut hasher = Sha256::new();
    hasher.update(url.as_bytes());
    let hash = hex::encode(hasher.finalize());

    let original_path = cache_dir.join(format!("{}.png", hash));
    let blur_path = cache_dir.join(format!("{}_blur.png", hash));
    let palette_path = cache_dir.join(format!("{}.json", hash));

    if original_path.exists() && blur_path.exists() {
        if let Some(palette) = get_cached_palette(&hash, &palette_path) {
            info!(
                "Album art & palette cache hit on disk: {} (took {:?})",
                url,
                start_time.elapsed()
            );
            return Some(ProcessedArt {
                art_path: original_path.to_string_lossy().to_string(),
                blur_path: blur_path.to_string_lossy().to_string(),
                palette,
            });
        }
    }

    info!("Downloading/reading album art: {}", url);
    let download_start = std::time::Instant::now();
    let img_data = fetch_image_bytes(url)?;
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

    let palette = extract_and_cache_palette(&hash, &img, &palette_path);

    info!(
        "Processed, blurred, and extracted palette in {:?}",
        process_start.elapsed()
    );
    info!(
        "Total album art processing time: {:?}",
        start_time.elapsed()
    );

    Some(ProcessedArt {
        art_path: original_path.to_string_lossy().to_string(),
        blur_path: blur_path.to_string_lossy().to_string(),
        palette,
    })
}

fn fetch_image_bytes(url: &str) -> Option<Vec<u8>> {
    if url.starts_with("file://") {
        let path = url.strip_prefix("file://")?;
        fs::read(path).ok()
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
                Some(bytes)
            }
            Err(e) => {
                warn!("Failed to download album art from {}: {}", url, e);
                None
            }
        }
    } else {
        warn!("Unsupported album art URL scheme: {}", url);
        None
    }
}

fn get_cached_palette(hash: &str, palette_path: &Path) -> Option<ArtPalette> {
    if let Ok(guard) = get_palette_cache().read() {
        if let Some(palette) = guard.get(hash) {
            return Some(palette.clone());
        }
    }

    if palette_path.exists() {
        if let Ok(content) = fs::read_to_string(palette_path) {
            if let Ok(palette) = serde_json::from_str::<ArtPalette>(&content) {
                if let Ok(mut guard) = get_palette_cache().write() {
                    guard.insert(hash.to_string(), palette.clone());
                }
                return Some(palette);
            }
        }
    }

    None
}

fn extract_and_cache_palette(
    hash: &str,
    img: &image::DynamicImage,
    palette_path: &Path,
) -> ArtPalette {
    if let Some(palette) = get_cached_palette(hash, palette_path) {
        return palette;
    }

    let palette = extract_palette_from_image(img).unwrap_or_else(default_palette);

    if let Ok(json_str) = serde_json::to_string_pretty(&palette) {
        let _ = fs::write(palette_path, json_str);
    }

    if let Ok(mut guard) = get_palette_cache().write() {
        guard.insert(hash.to_string(), palette.clone());
    }

    palette
}

fn extract_palette_from_image(img: &image::DynamicImage) -> Option<ArtPalette> {
    // why: fast downsample to 64x64 via nearest filter to avoid software Lanczos convolution on large images
    let small = img.resize_exact(64, 64, FilterType::Nearest);
    let mut small_bytes = Vec::new();
    small
        .write_to(
            &mut std::io::Cursor::new(&mut small_bytes),
            image::ImageFormat::Png,
        )
        .ok()?;

    let data = ImageReader::read(small_bytes).ok()?;
    let seed_color = ImageReader::extract_color(&data);
    let theme = ThemeBuilder::with_source(seed_color).build();

    let dominant = (seed_color.red, seed_color.green, seed_color.blue);
    let is_dark = is_dark_color(dominant.0, dominant.1, dominant.2);
    let on_dominant = if is_dark {
        (255, 255, 255)
    } else {
        (18, 18, 18)
    };

    let scheme = &theme.schemes.dark;
    let light_scheme = &theme.schemes.light;

    Some(ArtPalette {
        dominant,
        on_dominant,
        is_dark,
        primary: (
            scheme.primary.red,
            scheme.primary.green,
            scheme.primary.blue,
        ),
        on_primary: (
            scheme.on_primary.red,
            scheme.on_primary.green,
            scheme.on_primary.blue,
        ),
        surface: (
            light_scheme.surface.red,
            light_scheme.surface.green,
            light_scheme.surface.blue,
        ),
        on_surface: (
            light_scheme.on_surface.red,
            light_scheme.on_surface.green,
            light_scheme.on_surface.blue,
        ),
    })
}

fn is_dark_color(r: u8, g: u8, b: u8) -> bool {
    let to_linear = |v: u8| -> f32 {
        let c = v as f32 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let lum = 0.2126 * to_linear(r) + 0.7152 * to_linear(g) + 0.0722 * to_linear(b);
    lum <= 0.179
}

fn default_palette() -> ArtPalette {
    ArtPalette {
        dominant: (40, 40, 40),
        on_dominant: (255, 255, 255),
        is_dark: true,
        primary: (103, 80, 164),
        on_primary: (255, 255, 255),
        surface: (255, 255, 255),
        on_surface: (28, 27, 31),
    }
}
