use std::{collections::HashMap, sync::{Arc, Mutex}};

use once_cell::sync::Lazy;

#[derive(Clone, Debug)]
pub struct CustomPaintTexture {
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<Vec<u8>>,
}

pub trait CustomPaintSource: Send + Sync {
    fn frame(&self, width: u32, height: u32, scale: f64) -> Option<CustomPaintTexture>;
}

static NEXT_ID: Lazy<std::sync::atomic::AtomicU64> =
    Lazy::new(|| std::sync::atomic::AtomicU64::new(1));
static SOURCES: Lazy<Mutex<HashMap<u64, Arc<dyn CustomPaintSource>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

pub fn register_custom_paint_source(source: Arc<dyn CustomPaintSource>) -> u64 {
    let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    SOURCES.lock().unwrap().insert(id, source);
    id
}

pub fn unregister_custom_paint_source(id: u64) {
    SOURCES.lock().unwrap().remove(&id);
}

pub fn get_custom_paint_source(id: u64) -> Option<Arc<dyn CustomPaintSource>> {
    SOURCES.lock().unwrap().get(&id).cloned()
}
