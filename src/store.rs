//! In-memory key-value store.
//!
//! Every operation takes the current time as an argument so expiry is
//! deterministic in tests. Callers pass [`Instant::now`], a monotonic clock,
//! so wall-clock changes never affect leases. An expired entry is invisible to
//! every operation until the background sweep frees its memory.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use bytes::Bytes;

/// Maximum key length in bytes.
pub const MAX_KEY_LEN: usize = 32;

/// A stored value and its metadata.
#[derive(Debug)]
struct Entry {
    /// Raw value bytes.
    value: Bytes,
    /// Starts at 1 on create and increments on every overwrite.
    version: u64,
    /// Instant at which the entry becomes invisible; `None` never expires.
    expires_at: Option<Instant>,
}

impl Entry {
    /// Whether the entry is expired at `now`.
    fn is_expired(&self, now: Instant) -> bool {
        self.expires_at.is_some_and(|at| at <= now)
    }

    /// Snapshot of the entry as seen at `now`.
    fn current(&self, now: Instant) -> Current {
        Current {
            value: self.value.clone(),
            version: self.version,
            ttl_remaining: self.expires_at.map(|at| at.saturating_duration_since(now)),
        }
    }
}

/// A snapshot of a live entry.
#[derive(Debug, PartialEq, Eq)]
pub struct Current {
    /// Raw value bytes.
    pub value: Bytes,
    /// Version of the entry.
    pub version: u64,
    /// Time until the entry expires; `None` if it has no TTL.
    pub ttl_remaining: Option<Duration>,
}

/// Precondition a set must satisfy, checked atomically with the write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Condition<'a> {
    /// Write unconditionally.
    Always,
    /// Write only if the key is absent (set-if-absent).
    IfAbsent,
    /// Write only if the current value equals these bytes (compare-and-set).
    IfValue(&'a [u8]),
}

/// Result of [`Store::set`].
#[derive(Debug, PartialEq, Eq)]
pub enum SetOutcome {
    /// The value was written; carries the new version.
    Written(u64),
    /// The precondition failed; carries the entry that blocked the write.
    Conflict(Current),
    /// `IfValue` was requested but the key is absent.
    NotFound,
}

/// Result of [`Store::delete`].
#[derive(Debug, PartialEq, Eq)]
pub enum DeleteOutcome {
    /// The entry was removed.
    Deleted,
    /// The `if_value` check failed; carries the entry that blocked the delete.
    Conflict(Current),
    /// The key is absent.
    NotFound,
}

/// Thread-safe map from key to a stored value with its metadata.
///
/// One lock serializes every operation, so all operations are linearizable
/// and each conditional check and its write happen in a single step. The lock
/// is never held across an `.await`, which makes `std::sync::Mutex`
/// sufficient.
#[derive(Debug, Default)]
pub struct Store {
    /// All entries, guarded by the single store lock.
    entries: Mutex<HashMap<String, Entry>>,
}

impl Store {
    /// Returns the live entry for `key`. O(1) on average.
    pub fn get(&self, key: &str, now: Instant) -> Option<Current> {
        let entries = self.lock();
        entries
            .get(key)
            .filter(|entry| !entry.is_expired(now))
            .map(|entry| entry.current(now))
    }

    /// Stores `value` under `key` if `condition` holds. O(1) on average.
    ///
    /// `ttl` sets the expiry; `None` clears any existing TTL. An expired
    /// entry counts as absent, so overwriting it restarts the version at 1.
    pub fn set(
        &self,
        key: String,
        value: Bytes,
        ttl: Option<Duration>,
        condition: Condition<'_>,
        now: Instant,
    ) -> SetOutcome {
        let mut entries = self.lock();
        let live = entries.get(&key).filter(|entry| !entry.is_expired(now));
        match (condition, live) {
            (Condition::IfAbsent, Some(entry)) => {
                return SetOutcome::Conflict(entry.current(now));
            }
            (Condition::IfValue(_), None) => return SetOutcome::NotFound,
            (Condition::IfValue(expected), Some(entry)) if entry.value.as_ref() != expected => {
                return SetOutcome::Conflict(entry.current(now));
            }
            _ => {}
        }
        let version = live.map_or(1, |entry| entry.version + 1);
        entries.insert(
            key,
            Entry {
                value,
                version,
                expires_at: ttl.map(|ttl| now + ttl),
            },
        );
        SetOutcome::Written(version)
    }

    /// Removes `key`, optionally only if its value equals `if_value`.
    /// O(1) on average.
    pub fn delete(&self, key: &str, if_value: Option<&[u8]>, now: Instant) -> DeleteOutcome {
        let mut entries = self.lock();
        let Some(entry) = entries.get(key).filter(|entry| !entry.is_expired(now)) else {
            return DeleteOutcome::NotFound;
        };
        if if_value.is_some_and(|expected| entry.value.as_ref() != expected) {
            return DeleteOutcome::Conflict(entry.current(now));
        }
        entries.remove(key);
        DeleteOutcome::Deleted
    }

    /// Returns the sorted live keys that start with `prefix` (all when `None`).
    ///
    /// O(n) to scan plus O(k log k) to sort the k matches, so output is stable.
    pub fn keys(&self, prefix: Option<&str>, now: Instant) -> Vec<String> {
        let entries = self.lock();
        let mut keys: Vec<String> = entries
            .iter()
            .filter(|(key, entry)| {
                !entry.is_expired(now) && prefix.is_none_or(|prefix| key.starts_with(prefix))
            })
            .map(|(key, _)| key.clone())
            .collect();
        keys.sort_unstable();
        keys
    }

    /// Frees expired entries and returns how many were removed. O(n).
    pub fn sweep(&self, now: Instant) -> usize {
        let mut entries = self.lock();
        let before = entries.len();
        entries.retain(|_, entry| !entry.is_expired(now));
        before - entries.len()
    }

    /// Number of stored entries, including expired ones not yet swept.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// Whether the store holds no entries, including expired ones not yet swept.
    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    /// Acquires the store lock.
    ///
    /// # Panics
    ///
    /// Panics if the lock is poisoned, which only happens after another
    /// thread panicked while holding it.
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        self.entries.lock().expect("store lock poisoned")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes `value` under `key` unconditionally with no TTL.
    fn put(store: &Store, key: &str, value: &'static [u8], now: Instant) -> SetOutcome {
        store.set(
            key.into(),
            Bytes::from_static(value),
            None,
            Condition::Always,
            now,
        )
    }

    #[test]
    fn set_starts_at_version_one_and_increments_on_overwrite() {
        let (store, now) = (Store::default(), Instant::now());
        assert_eq!(put(&store, "a", b"1", now), SetOutcome::Written(1));
        assert_eq!(put(&store, "a", b"2", now), SetOutcome::Written(2));
        let current = store.get("a", now).unwrap();
        assert_eq!(
            (current.value, current.version),
            (Bytes::from_static(b"2"), 2)
        );
    }

    #[test]
    fn get_missing_key_is_none() {
        assert_eq!(Store::default().get("missing", Instant::now()), None);
    }

    #[test]
    fn delete_reports_whether_key_existed() {
        let (store, now) = (Store::default(), Instant::now());
        put(&store, "a", b"", now);
        assert_eq!(store.delete("a", None, now), DeleteOutcome::Deleted);
        assert_eq!(store.delete("a", None, now), DeleteOutcome::NotFound);
        assert_eq!(store.get("a", now), None);
    }

    #[test]
    fn recreating_a_deleted_key_restarts_at_version_one() {
        let (store, now) = (Store::default(), Instant::now());
        put(&store, "a", b"", now);
        put(&store, "a", b"", now);
        store.delete("a", None, now);
        assert_eq!(put(&store, "a", b"", now), SetOutcome::Written(1));
    }

    #[test]
    fn keys_filters_by_prefix_and_sorts() {
        let (store, now) = (Store::default(), Instant::now());
        for key in ["lock/b", "lock/a", "other"] {
            put(&store, key, b"", now);
        }
        assert_eq!(store.keys(None, now), ["lock/a", "lock/b", "other"]);
        assert_eq!(store.keys(Some("lock/"), now), ["lock/a", "lock/b"]);
        assert!(store.keys(Some("nope"), now).is_empty());
    }

    #[test]
    fn if_absent_writes_once_then_conflicts_with_the_winner() {
        let (store, now) = (Store::default(), Instant::now());
        let set = |value: &'static [u8]| {
            store.set(
                "k".into(),
                Bytes::from_static(value),
                None,
                Condition::IfAbsent,
                now,
            )
        };
        assert_eq!(set(b"first"), SetOutcome::Written(1));
        let SetOutcome::Conflict(current) = set(b"second") else {
            panic!("expected conflict");
        };
        assert_eq!(current.value, Bytes::from_static(b"first"));
        assert_eq!(current.version, 1);
    }

    #[test]
    fn if_value_requires_an_exact_match() {
        let (store, now) = (Store::default(), Instant::now());
        let cas = |expected: &[u8], value: &'static [u8]| {
            store.set(
                "k".into(),
                Bytes::from_static(value),
                None,
                Condition::IfValue(expected),
                now,
            )
        };
        assert_eq!(cas(b"a", b"b"), SetOutcome::NotFound);
        put(&store, "k", b"a", now);
        assert!(matches!(cas(b"x", b"b"), SetOutcome::Conflict(_)));
        assert_eq!(cas(b"a", b"b"), SetOutcome::Written(2));
    }

    #[test]
    fn if_value_delete_only_removes_a_matching_entry() {
        let (store, now) = (Store::default(), Instant::now());
        assert_eq!(store.delete("k", Some(b"a"), now), DeleteOutcome::NotFound);
        put(&store, "k", b"a", now);
        assert!(matches!(
            store.delete("k", Some(b"x"), now),
            DeleteOutcome::Conflict(_)
        ));
        assert!(store.get("k", now).is_some());
        assert_eq!(store.delete("k", Some(b"a"), now), DeleteOutcome::Deleted);
    }

    #[test]
    fn expired_entries_are_invisible_to_every_operation() {
        let (store, now) = (Store::default(), Instant::now());
        let ttl = Duration::from_secs(5);
        store.set(
            "k".into(),
            Bytes::from_static(b"a"),
            Some(ttl),
            Condition::Always,
            now,
        );
        let before = now + Duration::from_secs(4);
        assert_eq!(
            store.get("k", before).unwrap().ttl_remaining,
            Some(Duration::from_secs(1))
        );

        let after = now + ttl;
        assert_eq!(store.get("k", after), None);
        assert!(store.keys(None, after).is_empty());
        assert_eq!(store.delete("k", None, after), DeleteOutcome::NotFound);
        let cas = store.set(
            "k".into(),
            Bytes::new(),
            None,
            Condition::IfValue(b"a"),
            after,
        );
        assert_eq!(cas, SetOutcome::NotFound);
        let acquire = store.set(
            "k".into(),
            Bytes::from_static(b"b"),
            None,
            Condition::IfAbsent,
            after,
        );
        assert_eq!(acquire, SetOutcome::Written(1));
    }

    #[test]
    fn set_without_ttl_clears_an_existing_ttl() {
        let (store, now) = (Store::default(), Instant::now());
        let ttl = Some(Duration::from_secs(5));
        store.set("k".into(), Bytes::new(), ttl, Condition::Always, now);
        put(&store, "k", b"", now);
        assert_eq!(store.get("k", now).unwrap().ttl_remaining, None);
        assert!(store.get("k", now + Duration::from_secs(60)).is_some());
    }

    #[test]
    fn sweep_frees_only_expired_entries() {
        let (store, now) = (Store::default(), Instant::now());
        let ttl = Some(Duration::from_secs(1));
        store.set("short".into(), Bytes::new(), ttl, Condition::Always, now);
        put(&store, "forever", b"", now);
        assert_eq!(store.sweep(now), 0);
        assert_eq!(store.len(), 2);
        assert_eq!(store.sweep(now + Duration::from_secs(1)), 1);
        assert_eq!(store.len(), 1);
    }
}
