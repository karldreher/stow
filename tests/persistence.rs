//! Tests for save-file round trips through the public persistence API.

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use stow::persist::{self, SavedEntry};
use stow::store::{Condition, Store};

/// A fresh path under the system temp directory, unique per test.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("stow-persistence-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    dir
}

/// Milliseconds since the UNIX epoch.
fn unix_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis()
        .try_into()
        .unwrap()
}

#[test]
fn round_trip_of_100k_entries_preserves_keys_values_and_versions() {
    let dir = temp_dir("round-trip");
    let path = dir.join("state.bin");
    let (source, now) = (Store::default(), Instant::now());
    for i in 0..100_000_usize {
        let value = Bytes::from(vec![(i % 251) as u8; i % 64]);
        let ttl = (i % 5 == 0).then(|| Duration::from_secs(3600));
        source.set(
            format!("key/{i}"),
            value.clone(),
            ttl,
            Condition::Always,
            now,
        );
        if i % 3 == 0 {
            source.set(format!("key/{i}"), value, ttl, Condition::Always, now);
        }
    }

    assert_eq!(persist::save(&path, &source, now).unwrap(), 100_000);
    let restored = Store::default();
    assert_eq!(persist::load(&path, &restored, now).unwrap(), Some(100_000));

    let mut expected = source.snapshot(now);
    let mut actual = restored.snapshot(now);
    expected.sort_by(|a, b| a.0.cmp(&b.0));
    actual.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(expected.len(), actual.len());
    for ((ek, e), (ak, a)) in expected.iter().zip(&actual) {
        assert_eq!((ek, &e.value, e.version), (ak, &a.value, a.version));
        assert_eq!(e.ttl_remaining.is_some(), a.ttl_remaining.is_some(), "{ek}");
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn unexpired_ttl_survives_with_remaining_time_preserved() {
    let dir = temp_dir("ttl");
    let path = dir.join("state.bin");
    let (source, now) = (Store::default(), Instant::now());
    let ttl = Some(Duration::from_secs(60));
    source.set(
        "k".into(),
        Bytes::from_static(b"v"),
        ttl,
        Condition::Always,
        now,
    );

    persist::save(&path, &source, now).unwrap();
    let restored = Store::default();
    persist::load(&path, &restored, now).unwrap();

    let remaining = restored.get("k", now).unwrap().ttl_remaining.unwrap();
    assert!(
        remaining.abs_diff(Duration::from_secs(60)) <= Duration::from_secs(1),
        "{remaining:?}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn entries_that_expired_while_stopped_are_dropped_on_load() {
    let dir = temp_dir("expired");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("state.bin");
    let entry = |key: &str, expires_at_unix_ms| SavedEntry {
        key: key.into(),
        value: b"v".to_vec(),
        version: 4,
        expires_at_unix_ms,
    };
    let entries = [
        entry("gone", Some(unix_now_ms() - 5_000)),
        entry("kept", Some(unix_now_ms() + 60_000)),
        entry("forever", None),
    ];
    fs::write(&path, persist::encode(&entries)).unwrap();

    let store = Store::default();
    let now = Instant::now();
    assert_eq!(persist::load(&path, &store, now).unwrap(), Some(2));
    assert!(store.get("gone", now).is_none());
    assert_eq!(store.get("kept", now).unwrap().version, 4);
    assert!(store.get("forever", now).is_some());
    let _ = fs::remove_dir_all(&dir);
}
