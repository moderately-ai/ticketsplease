//! Sortable, unique ids: `<epoch_nanos>-<rand>` (nanos zero-padded to 19 digits).
//!
//! The nanosecond prefix makes ids lexically sortable in chronological order (one
//! machine, one clock — see the comment/event topologies). Nanosecond resolution
//! is what makes the event-log `--since` cursor reliable: any two events from
//! separate process invocations land on distinct, ordered timestamps. The random
//! suffix only breaks ties between writers that share the same instant, and needs
//! no extra dependency — `RandomState` is seeded from OS entropy.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::time::{SystemTime, UNIX_EPOCH};

/// A fresh sortable-unique id.
#[must_use]
pub fn new_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    // 19 digits holds epoch-nanos until ~year 2286, keeping the width fixed so the
    // lexical sort stays chronological.
    format!("{nanos:019}-{:08x}", random_u32())
}

/// Current time in epoch seconds (0 if the clock predates the epoch).
#[must_use]
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn random_u32() -> u32 {
    let mut h = RandomState::new().build_hasher();
    h.write_u8(0);
    h.finish() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_sortable() {
        let a = new_id();
        let b = new_id();
        assert_ne!(a, b);
        assert_eq!(
            a.len(),
            b.len(),
            "fixed width keeps lexical sort chronological"
        );
    }
}
