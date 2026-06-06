use crate::mpris::MprisData;
use std::sync::{Mutex, OnceLock};

type Callback = Box<dyn Fn(MprisData) + Send + Sync + 'static>;

static LISTENERS: OnceLock<Mutex<Vec<Callback>>> = OnceLock::new();
static LAST_DATA: OnceLock<Mutex<Option<MprisData>>> = OnceLock::new();

fn get_listeners() -> &'static Mutex<Vec<Callback>> {
    LISTENERS.get_or_init(|| Mutex::new(Vec::new()))
}

fn get_last_data() -> &'static Mutex<Option<MprisData>> {
    LAST_DATA.get_or_init(|| Mutex::new(None))
}

pub fn register_listener<F>(callback: F)
where
    F: Fn(MprisData) + Send + Sync + 'static,
{
    let mut listeners = get_listeners().lock().unwrap();
    listeners.push(Box::new(callback));
    
    // immediately send last cached data if it exists to prevent race conditions on startup
    if let Some(data) = get_last_data().lock().unwrap().as_ref() {
        (listeners.last().unwrap())(data.clone());
    }
}

pub fn send_mpris(data: MprisData) {
    *get_last_data().lock().unwrap() = Some(data.clone());
    let listeners = get_listeners().lock().unwrap();
    for cb in listeners.iter() {
        cb(data.clone());
    }
}
