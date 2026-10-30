//! Save-file persistence: load the store at startup, write it at shutdown.
//!
//! The file is a fixed header followed by a postcard payload:
//!
//! ```text
//! magic    b"STOW"   (4 bytes)
//! format   u16 little-endian (currently 1)
//! payload  postcard-encoded Vec<SavedEntry>
//! ```
//!
//! `Instant` cannot be serialized, so TTLs are stored as absolute UNIX
//! milliseconds and converted back to `Instant` deadlines on load. Save and
//! load are both O(n) in entries and bytes.

use std::fmt;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::store::Store;

/// First four bytes of every save file.
const MAGIC: &[u8; 4] = b"STOW";

/// Format version written by this build and the only one it can read.
const FORMAT_VERSION: u16 = 1;

/// Size in bytes of the magic plus the format version.
const HEADER_LEN: usize = MAGIC.len() + size_of::<u16>();

/// One store entry as written to the save file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedEntry {
    /// Entry key.
    pub key: String,
    /// Raw value bytes.
    pub value: Vec<u8>,
    /// Entry version at save time.
    pub version: u64,
    /// Absolute expiry in milliseconds since the UNIX epoch; `None` never
    /// expires.
    pub expires_at_unix_ms: Option<u64>,
}

/// Failure to read or write a save file. Every variant names the file.
#[derive(Debug)]
pub enum PersistError {
    /// Reading, writing, or syncing the file or its directory failed.
    Io {
        /// The save file the operation targeted.
        path: PathBuf,
        /// The underlying I/O error.
        source: io::Error,
    },
    /// The file does not start with the `STOW` magic bytes.
    BadMagic {
        /// The save file that was read.
        path: PathBuf,
    },
    /// The file has a format version this build cannot read.
    UnsupportedFormat {
        /// The save file that was read.
        path: PathBuf,
        /// The format version found in the header.
        found: u16,
    },
    /// The payload is truncated or corrupt.
    Decode {
        /// The save file that was read.
        path: PathBuf,
        /// The underlying decode error.
        source: postcard::Error,
    },
}

impl fmt::Display for PersistError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(f, "save file {}: {source}", path.display())
            }
            Self::BadMagic { path } => {
                write!(
                    f,
                    "save file {} is not a stow save file (bad magic bytes)",
                    path.display()
                )
            }
            Self::UnsupportedFormat { path, found } => {
                write!(
                    f,
                    "save file {} has unsupported format version {found}; this build reads version {FORMAT_VERSION}",
                    path.display()
                )
            }
            Self::Decode { path, source } => {
                write!(f, "save file {} is corrupt: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for PersistError {}

/// Serializes `entries` into the save-file byte layout.
///
/// # Panics
///
/// Panics if postcard fails to serialize, which cannot happen for
/// [`SavedEntry`] because it contains only plain data.
pub fn encode(entries: &[SavedEntry]) -> Vec<u8> {
    let mut header = Vec::with_capacity(HEADER_LEN);
    header.extend_from_slice(MAGIC);
    header.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    postcard::to_extend(entries, header).expect("SavedEntry is plain data and always serializes")
}

/// Parses the save-file byte layout read from `path`.
///
/// # Errors
///
/// Returns [`PersistError::BadMagic`], [`PersistError::UnsupportedFormat`],
/// or [`PersistError::Decode`] if the header or payload is invalid.
fn decode(path: &Path, bytes: &[u8]) -> Result<Vec<SavedEntry>, PersistError> {
    let bad_magic = || PersistError::BadMagic {
        path: path.to_path_buf(),
    };
    let (header, payload) = bytes.split_at_checked(HEADER_LEN).ok_or_else(bad_magic)?;
    let (magic, format) = header.split_at(MAGIC.len());
    if magic != MAGIC {
        return Err(bad_magic());
    }
    let found = u16::from_le_bytes([format[0], format[1]]);
    if found != FORMAT_VERSION {
        return Err(PersistError::UnsupportedFormat {
            path: path.to_path_buf(),
            found,
        });
    }
    postcard::from_bytes(payload).map_err(|source| PersistError::Decode {
        path: path.to_path_buf(),
        source,
    })
}

/// Milliseconds since the UNIX epoch, saturating at `u64::MAX`.
fn unix_now_ms() -> u64 {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    millis(since_epoch)
}

/// Whole milliseconds in `duration`, saturating at `u64::MAX`.
fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Loads the save file at `path` into `store`.
///
/// Returns `Ok(None)` if the file does not exist, otherwise the number of
/// entries restored. Entries whose TTL ran out while stopped are dropped.
/// Remaining TTLs become `Instant` deadlines relative to `now`.
///
/// # Errors
///
/// Returns a [`PersistError`] if the file exists but cannot be read or
/// decoded. The caller must not continue to a later [`save`], so an
/// unreadable file is never overwritten.
pub fn load(path: &Path, store: &Store, now: Instant) -> Result<Option<usize>, PersistError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(PersistError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    let unix_now = unix_now_ms();
    let live: Vec<_> = decode(path, &bytes)?
        .into_iter()
        .filter_map(|saved| {
            let ttl = match saved.expires_at_unix_ms {
                None => None,
                Some(at) => {
                    let remaining = at.checked_sub(unix_now).filter(|&ms| ms > 0)?;
                    Some(Duration::from_millis(remaining))
                }
            };
            Some((saved.key, Bytes::from(saved.value), saved.version, ttl))
        })
        .collect();
    let restored = live.len();
    store.restore(live, now);
    Ok(Some(restored))
}

/// Writes the live entries of `store` to `path` and returns how many were
/// written.
///
/// The write is atomic: a temp file is written and synced, renamed over
/// `path`, and the parent directory is synced. A crash mid-save leaves the
/// previous file intact. Missing parent directories are created.
///
/// # Errors
///
/// Returns [`PersistError::Io`] if any filesystem step fails.
pub fn save(path: &Path, store: &Store, now: Instant) -> Result<usize, PersistError> {
    let unix_now = unix_now_ms();
    let entries: Vec<SavedEntry> = store
        .snapshot(now)
        .into_iter()
        .map(|(key, current)| SavedEntry {
            key,
            value: current.value.to_vec(),
            version: current.version,
            expires_at_unix_ms: current
                .ttl_remaining
                .map(|ttl| unix_now.saturating_add(millis(ttl))),
        })
        .collect();
    write_atomic(path, &encode(&entries)).map_err(|source| PersistError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(entries.len())
}

/// Replaces `path` with `bytes` via temp file, `fsync`, and rename.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    if let Some(parent) = parent {
        fs::create_dir_all(parent)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let mut file = File::create(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&tmp, path)?;
    // Windows cannot open a directory as a file, so only unix syncs the
    // parent; without it the rename itself may not survive a power loss.
    #[cfg(unix)]
    File::open(parent.unwrap_or(Path::new(".")))?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Condition;

    /// A fresh path under the system temp directory, unique per test.
    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("stow-persist-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    /// A single saved entry with no TTL.
    fn entry(key: &str) -> SavedEntry {
        SavedEntry {
            key: key.into(),
            value: b"v".to_vec(),
            version: 3,
            expires_at_unix_ms: None,
        }
    }

    #[test]
    fn encode_then_decode_round_trips() {
        let entries = vec![entry("a"), entry("b")];
        let decoded = decode(Path::new("f"), &encode(&entries)).unwrap();
        assert_eq!(decoded, entries);
    }

    #[test]
    fn decode_rejects_bad_magic() {
        let mut bytes = encode(&[entry("a")]);
        bytes[0] = b'X';
        let err = decode(Path::new("f"), &bytes).unwrap_err();
        assert!(matches!(err, PersistError::BadMagic { .. }));
    }

    #[test]
    fn decode_rejects_a_file_shorter_than_the_header() {
        let err = decode(Path::new("f"), b"STO").unwrap_err();
        assert!(matches!(err, PersistError::BadMagic { .. }));
    }

    #[test]
    fn decode_rejects_unknown_format_version() {
        let mut bytes = encode(&[entry("a")]);
        bytes[4] = 2;
        let err = decode(Path::new("f"), &bytes).unwrap_err();
        assert!(matches!(
            err,
            PersistError::UnsupportedFormat { found: 2, .. }
        ));
    }

    #[test]
    fn decode_rejects_a_truncated_payload() {
        let mut bytes = encode(&[entry("a")]);
        bytes.pop();
        let err = decode(Path::new("f"), &bytes).unwrap_err();
        assert!(matches!(err, PersistError::Decode { .. }));
    }

    #[test]
    fn load_of_a_missing_file_is_none() {
        let path = temp_path("missing").join("absent.bin");
        assert!(
            load(&path, &Store::default(), Instant::now())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn save_creates_parent_directories_and_leaves_no_temp_file() {
        let dir = temp_path("save");
        let path = dir.join("nested/state.bin");
        let (store, now) = (Store::default(), Instant::now());
        store.set(
            "k".into(),
            Bytes::from_static(b"v"),
            None,
            Condition::Always,
            now,
        );
        assert_eq!(save(&path, &store, now).unwrap(), 1);
        assert!(path.is_file());
        assert!(!dir.join("nested/state.bin.tmp").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
