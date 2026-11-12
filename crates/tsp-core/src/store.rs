//! Saving profile files: atomic writes, the one-time `.v1.bak` backup, and
//! change detection (another program edited the file since we loaded or
//! saved it). File access goes through [`Fs`] so the rules are tested with a
//! fake file system.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::io;
use std::path::{Path, PathBuf};

/// What we last saw on disk: modification time, size and a content hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identity {
    pub mtime_ms: u128,
    pub size: u64,
    pub hash: u64,
}

pub trait Fs {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>>;
    /// Modification time in ms since the epoch.
    fn mtime_ms(&self, path: &Path) -> io::Result<u128>;
    fn exists(&self, path: &Path) -> bool;
    /// Writes `bytes` to a temp file beside `path`, then renames it over `path`.
    fn write_atomic(&mut self, path: &Path, bytes: &[u8]) -> io::Result<()>;
    fn copy(&mut self, from: &Path, to: &Path) -> io::Result<()>;
}

fn hash(bytes: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

pub fn identity(fs: &dyn Fs, path: &Path) -> io::Result<Identity> {
    let bytes = fs.read(path)?;
    Ok(Identity { mtime_ms: fs.mtime_ms(path)?, size: bytes.len() as u64, hash: hash(&bytes) })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveError {
    /// The file changed on disk since `expected` (or vanished).
    ChangedOnDisk,
    BackupFailed { backup: PathBuf, error: String },
    WriteFailed { error: String },
}

impl SaveError {
    pub fn code(&self) -> &'static str {
        match self {
            SaveError::ChangedOnDisk => "profile-changed-on-disk",
            SaveError::BackupFailed { .. } => "profile-backup-failed",
            SaveError::WriteFailed { .. } => "profile-save-failed",
        }
    }

    pub fn message(&self, path: &Path) -> String {
        match self {
            SaveError::ChangedOnDisk => "profile-changed-on-disk: the pad file changed on disk. Keep my edits (overwrite file) or Load file.".into(),
            SaveError::BackupFailed { backup, error } => format!(
                "profile-backup-failed: couldn't copy {} to {} ({error}). Nothing changed. Make the folder writable, or use New pad.",
                path.display(),
                backup.display()
            ),
            SaveError::WriteFailed { error } => format!(
                "profile-save-failed: couldn't save {} ({error}). Your edits are kept in memory; the file is unchanged.",
                path.display()
            ),
        }
    }
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// Where the v1 original goes before its first rewrite, or `None` when an
/// identical backup already exists. A differing `.v1.bak` is never
/// overwritten: the next free `.v1.<n>.bak` is used instead.
pub fn backup_path(fs: &dyn Fs, path: &Path) -> io::Result<Option<PathBuf>> {
    let original = fs.read(path)?;
    let candidates = std::iter::once(with_suffix(path, ".v1.bak")).chain((1..).map(|n| with_suffix(path, &format!(".v1.{n}.bak"))));
    for c in candidates {
        if !fs.exists(&c) {
            return Ok(Some(c));
        }
        if fs.read(&c)? == original {
            return Ok(None);
        }
    }
    unreachable!("candidates are unbounded")
}

/// Saves `text` to `path`.
/// - `expected`: the identity from the last load or save; a mismatch (or a
///   vanished file) refuses with `ChangedOnDisk` and writes nothing. `None`
///   force-writes ("Keep my edits").
/// - `backup_v1`: copy the original to its `.v1.bak` first (first rewrite of
///   a v1 file); a failed copy writes nothing.
///
/// Returns the identity of the written file.
pub fn save(fs: &mut dyn Fs, path: &Path, text: &str, expected: Option<&Identity>, backup_v1: bool) -> Result<Identity, SaveError> {
    if let Some(want) = expected {
        match identity(fs, path) {
            Ok(now) if now == *want => {}
            _ => return Err(SaveError::ChangedOnDisk),
        }
    }
    if backup_v1 && fs.exists(path) {
        let bak = backup_path(fs, path).map_err(|e| SaveError::BackupFailed { backup: with_suffix(path, ".v1.bak"), error: e.to_string() })?;
        if let Some(bak) = bak {
            fs.copy(path, &bak).map_err(|e| SaveError::BackupFailed { backup: bak.clone(), error: e.to_string() })?;
        }
    }
    fs.write_atomic(path, text.as_bytes()).map_err(|e| SaveError::WriteFailed { error: e.to_string() })?;
    identity(fs, path).map_err(|e| SaveError::WriteFailed { error: e.to_string() })
}

/// The real file system.
pub struct RealFs;

impl Fs for RealFs {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        std::fs::read(path)
    }

    fn mtime_ms(&self, path: &Path) -> io::Result<u128> {
        let m = std::fs::metadata(path)?.modified()?;
        Ok(m.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0))
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn write_atomic(&mut self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        let tmp = with_suffix(path, &format!(".tmp-{}", std::process::id()));
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, path).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
    }

    fn copy(&mut self, from: &Path, to: &Path) -> io::Result<()> {
        std::fs::copy(from, to).map(|_| ())
    }
}

#[cfg(test)]
pub mod fake {
    use super::*;
    use std::collections::HashMap;

    /// In-memory file system with a ticking clock and injectable failures.
    #[derive(Default)]
    pub struct FakeFs {
        pub files: HashMap<PathBuf, (Vec<u8>, u128)>,
        pub clock: u128,
        pub fail_write: bool,
        pub fail_copy: bool,
    }

    impl FakeFs {
        pub fn put(&mut self, path: &str, text: &str) {
            self.clock += 1;
            self.files.insert(PathBuf::from(path), (text.as_bytes().to_vec(), self.clock));
        }
        pub fn text(&self, path: &str) -> Option<String> {
            self.files.get(Path::new(path)).map(|(b, _)| String::from_utf8_lossy(b).into_owned())
        }
    }

    fn missing() -> io::Error {
        io::Error::new(io::ErrorKind::NotFound, "not found")
    }

    impl Fs for FakeFs {
        fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
            self.files.get(path).map(|(b, _)| b.clone()).ok_or_else(missing)
        }
        fn mtime_ms(&self, path: &Path) -> io::Result<u128> {
            self.files.get(path).map(|(_, t)| *t).ok_or_else(missing)
        }
        fn exists(&self, path: &Path) -> bool {
            self.files.contains_key(path)
        }
        fn write_atomic(&mut self, path: &Path, bytes: &[u8]) -> io::Result<()> {
            if self.fail_write {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, "read-only file"));
            }
            self.clock += 1;
            self.files.insert(path.to_path_buf(), (bytes.to_vec(), self.clock));
            Ok(())
        }
        fn copy(&mut self, from: &Path, to: &Path) -> io::Result<()> {
            if self.fail_copy {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, "folder not writable"));
            }
            let bytes = self.read(from)?;
            self.clock += 1;
            self.files.insert(to.to_path_buf(), (bytes, self.clock));
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeFs;
    use super::*;

    const P: &str = "/cfg/pad.tsp.json";

    fn loaded(fs: &FakeFs) -> Identity {
        identity(fs, Path::new(P)).unwrap()
    }

    #[test]
    fn save_writes_and_returns_the_new_identity() {
        let mut fs = FakeFs::default();
        fs.put(P, "v1");
        let id = loaded(&fs);
        let next = save(&mut fs, Path::new(P), "v2", Some(&id), false).unwrap();
        assert_eq!(fs.text(P).unwrap(), "v2");
        assert_eq!(next, loaded(&fs));
        // A second save with the returned identity succeeds (own writes are not conflicts).
        save(&mut fs, Path::new(P), "v3", Some(&next), false).unwrap();
    }

    #[test]
    fn a_hand_edit_between_writes_is_a_conflict_and_nothing_is_written() {
        let mut fs = FakeFs::default();
        fs.put(P, "mine");
        let id = loaded(&fs);
        fs.put(P, "edited in VS Code");
        assert_eq!(save(&mut fs, Path::new(P), "editor", Some(&id), false), Err(SaveError::ChangedOnDisk));
        assert_eq!(fs.text(P).unwrap(), "edited in VS Code");
        // Keep my edits: force-write without an expected identity.
        save(&mut fs, Path::new(P), "editor", None, false).unwrap();
        assert_eq!(fs.text(P).unwrap(), "editor");
    }

    #[test]
    fn same_size_same_mtime_but_different_content_is_still_a_conflict() {
        let mut fs = FakeFs::default();
        fs.put(P, "aaaa");
        let id = loaded(&fs);
        let t = fs.files[Path::new(P)].1;
        fs.files.insert(PathBuf::from(P), (b"bbbb".to_vec(), t));
        assert_eq!(save(&mut fs, Path::new(P), "x", Some(&id), false), Err(SaveError::ChangedOnDisk));
    }

    #[test]
    fn a_deleted_file_is_a_conflict() {
        let mut fs = FakeFs::default();
        fs.put(P, "x");
        let id = loaded(&fs);
        fs.files.clear();
        assert_eq!(save(&mut fs, Path::new(P), "y", Some(&id), false), Err(SaveError::ChangedOnDisk));
    }

    #[test]
    fn first_v1_rewrite_backs_up_the_original_once() {
        let mut fs = FakeFs::default();
        fs.put(P, "original v1");
        let id = loaded(&fs);
        let id = save(&mut fs, Path::new(P), "v2 a", Some(&id), true).unwrap();
        assert_eq!(fs.text("/cfg/pad.tsp.json.v1.bak").unwrap(), "original v1");
        // A later rewrite with backup requested again keeps the existing backup.
        save(&mut fs, Path::new(P), "v2 b", Some(&id), true).unwrap();
        assert!(fs.text("/cfg/pad.tsp.json.v1.1.bak").is_some(), "file differs from .v1.bak now, so a new numbered backup");
        assert_eq!(fs.text("/cfg/pad.tsp.json.v1.bak").unwrap(), "original v1", "never overwritten");
    }

    #[test]
    fn an_identical_existing_backup_is_reused() {
        let mut fs = FakeFs::default();
        fs.put(P, "original");
        fs.put("/cfg/pad.tsp.json.v1.bak", "original");
        let id = loaded(&fs);
        save(&mut fs, Path::new(P), "new", Some(&id), true).unwrap();
        assert!(fs.text("/cfg/pad.tsp.json.v1.1.bak").is_none());
    }

    #[test]
    fn a_differing_backup_leads_to_a_numbered_one() {
        let mut fs = FakeFs::default();
        fs.put(P, "original");
        fs.put("/cfg/pad.tsp.json.v1.bak", "someone else's");
        let id = loaded(&fs);
        save(&mut fs, Path::new(P), "new", Some(&id), true).unwrap();
        assert_eq!(fs.text("/cfg/pad.tsp.json.v1.1.bak").unwrap(), "original");
        assert_eq!(fs.text("/cfg/pad.tsp.json.v1.bak").unwrap(), "someone else's");
    }

    #[test]
    fn a_failed_backup_writes_nothing() {
        let mut fs = FakeFs { fail_copy: true, ..Default::default() };
        fs.put(P, "original");
        let id = loaded(&fs);
        let e = save(&mut fs, Path::new(P), "new", Some(&id), true).unwrap_err();
        assert_eq!(e.code(), "profile-backup-failed");
        assert!(e.message(Path::new(P)).contains("Nothing changed"));
        assert_eq!(fs.text(P).unwrap(), "original");
    }

    #[test]
    fn a_failed_write_reports_save_failed_and_keeps_the_file() {
        let mut fs = FakeFs { fail_write: true, ..Default::default() };
        fs.put(P, "original");
        let id = loaded(&fs);
        let e = save(&mut fs, Path::new(P), "new", Some(&id), false).unwrap_err();
        assert_eq!(e.code(), "profile-save-failed");
        assert!(e.message(Path::new(P)).contains("kept in memory"));
        assert_eq!(fs.text(P).unwrap(), "original");
    }

    #[test]
    fn real_fs_writes_atomically_in_a_temp_dir() {
        let dir = std::env::temp_dir().join(format!("tsp-store-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pad.tsp.json");
        std::fs::write(&path, "old").unwrap();
        let mut fs = RealFs;
        let id = identity(&fs, &path).unwrap();
        save(&mut fs, &path, "new", Some(&id), true).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(dir.join("pad.tsp.json.v1.bak")).unwrap(), "old");
        let leftovers: Vec<_> = std::fs::read_dir(&dir).unwrap().filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().contains(".tmp-")).collect();
        assert!(leftovers.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
