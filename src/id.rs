use std::sync::atomic::AtomicU64;

use lum_log::warn;

static ID_COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn get_unique_id() -> u64 {
    let id = ID_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if id == u64::MAX {
        warn!(
            "Unique ID counter has reached the maximum value of u64::MAX. From now on, IDs may be reused."
        );
    }

    id
}
