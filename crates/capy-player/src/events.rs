use crate::mpris::MprisData;
use std::sync::{Mutex, OnceLock};

type MprisCallback = Box<dyn Fn(MprisData) + Send + Sync + 'static>;
type VisualizerCallback = Box<dyn Fn(Vec<f32>) + Send + Sync + 'static>;

static MPRIS_LISTENERS: OnceLock<Mutex<Vec<MprisCallback>>> = OnceLock::new();
static VIS_LISTENERS: OnceLock<Mutex<Vec<VisualizerCallback>>> = OnceLock::new();
static LAST_MPRIS: OnceLock<Mutex<Option<MprisData>>> = OnceLock::new();

/// Registers a callback for MPRIS event data. The callback is called immediately with the last cached data if it exists.
pub fn register_mpris_listener<F: Fn(MprisData) + Send + Sync + 'static>(cb: F) {
    let mut list = MPRIS_LISTENERS
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .unwrap();
    list.push(Box::new(cb));
    if let Some(data) = LAST_MPRIS
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap()
        .as_ref()
    {
        (list.last().unwrap())(data.clone());
    }
}

/// Registers a callback for visualizer event data.
pub fn register_visualizer_listener<F: Fn(Vec<f32>) + Send + Sync + 'static>(cb: F) {
    let mut list = VIS_LISTENERS
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .unwrap();
    list.push(Box::new(cb));
}

pub fn send_mpris(data: MprisData) {
    *LAST_MPRIS.get_or_init(|| Mutex::new(None)).lock().unwrap() = Some(data.clone());
    if let Some(list) = MPRIS_LISTENERS.get() {
        for cb in list.lock().unwrap().iter() {
            cb(data.clone());
        }
    }
}

pub fn send_visualizer(bars: Vec<f32>) {
    if let Some(list) = VIS_LISTENERS.get() {
        for cb in list.lock().unwrap().iter() {
            cb(bars.clone());
        }
    }
}
