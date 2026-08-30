use std::cell::RefCell;

thread_local! {
    static ART_CACHE: RefCell<(String, slint::Image)> = RefCell::new((String::new(), slint::Image::default()));
    static BLUR_CACHE: RefCell<(String, slint::Image)> = RefCell::new((String::new(), slint::Image::default()));
}

pub fn load_image_cached(path: &str, has_media: bool, is_blur: bool) -> slint::Image {
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
