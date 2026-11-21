//! Byte-identical uninstall needs the original settings, which may hold
//! secrets. Store them as private files under WHY's data directory.
//!
//! Both files are scoped by instance, harness, config path and the hash of
//! the written config. Staging a new restore pair before changing the config
//! leaves the previous pair usable if publication fails or WHY exits early.

use std::fs;
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use why_wire::Instance;

use super::{HookError, Target, config_file, sha256_hex};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    version: u32,
    original: SavedOriginal,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum SavedOriginal {
    /// WHY created the config, so uninstall may delete it again.
    Absent,
    Sha256(String),
}

pub(super) enum Original {
    Absent,
    File(Vec<u8>),
}

pub(super) struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(data_dir: &Path, instance: Instance, target: Target, config: &Path) -> Self {
        let instance = match instance {
            Instance::Installed => "installed".to_owned(),
            Instance::Dev { port } => format!("dev-{port}"),
        };
        let path_hash = sha256_hex(config.as_os_str().as_encoded_bytes());
        Self {
            dir: data_dir
                .join("hook-installs")
                .join(instance)
                .join(target.hook_arg())
                .join(digest(&path_hash)),
        }
    }

    fn record_path(&self, written: &str) -> PathBuf {
        self.dir.join(digest(written)).with_extension("json")
    }

    /// Where the private copy of the original lands for a config WHY will
    /// write as `written` (shown in the install preview).
    pub fn original_copy_path(&self, written: &str) -> PathBuf {
        self.original_path(written)
    }

    fn original_path(&self, written: &str) -> PathBuf {
        self.dir.join(digest(written)).with_extension("orig")
    }

    /// Missing or damaged data falls back to removing only this instance's
    /// entries. Never restore a snapshot for a different set of config bytes.
    pub fn load(&self, written: &str) -> Option<Original> {
        let text = fs::read_to_string(self.record_path(written)).ok()?;
        let record: Record = serde_json::from_str(&text).ok()?;
        if record.version != 1 {
            return None;
        }
        let SavedOriginal::Sha256(hash) = record.original else {
            return Some(Original::Absent);
        };
        let bytes = fs::read(self.original_path(written)).ok()?;
        (sha256_hex(&bytes) == hash).then_some(Original::File(bytes))
    }

    pub fn save(&self, written: &str, original: Option<&[u8]>) -> Result<(), HookError> {
        config_file::reject_worktree(&self.dir)?;
        create_private_dir(&self.dir)?;
        if let Some(bytes) = original {
            config_file::write_backup(&self.original_path(written), bytes)?;
        }
        let record = Record {
            version: 1,
            original: original.map_or(SavedOriginal::Absent, |bytes| {
                SavedOriginal::Sha256(sha256_hex(bytes))
            }),
        };
        let json = serde_json::to_vec(&record).map_err(io::Error::other)?;
        config_file::write_backup(&self.record_path(written), &json)
    }

    pub fn discard(&self, written: &str) {
        if config_file::reject_worktree(&self.dir).is_ok() {
            let _ = fs::remove_file(self.record_path(written));
            let _ = fs::remove_file(self.original_path(written));
        }
    }

    /// After a successful edit, remove any older or abandoned restore pairs
    /// for this config. Leave unrelated files in the directory alone.
    pub fn clear_except(&self, keep: Option<&str>) {
        if config_file::reject_worktree(&self.dir).is_err() {
            return;
        }
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return;
        };
        let keep = keep.map(digest);
        for entry in entries.flatten() {
            let path = entry.path();
            if matches!(
                path.extension().and_then(|ext| ext.to_str()),
                Some("json" | "orig")
            ) && let Some(stem) = path.file_stem().and_then(|stem| stem.to_str())
                && Some(stem) != keep
                && stem.len() == 64
                && stem.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                let _ = fs::remove_file(path);
            }
        }
    }
}

fn digest(hash: &str) -> &str {
    // Internal input: every caller supplies a fingerprint from sha256_hex.
    hash.strip_prefix("sha256:").unwrap_or(hash)
}

fn create_private_dir(dir: &Path) -> io::Result<()> {
    match fs::metadata(dir) {
        Ok(meta) if meta.is_dir() => return Ok(()),
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                "restore path is not a directory",
            ));
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let parent = dir
        .parent()
        .ok_or_else(|| io::Error::other("restore directory has no parent"))?;
    create_private_dir(parent)?;
    match fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists && dir.is_dir() => {}
        Err(e) => return Err(e),
    }
    // Persist each new directory entry as well as the restore files.
    config_file::sync_dir(parent)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::MetadataExt;

    use tempfile::TempDir;

    use super::*;

    const CONFIG: &str = "/h/.claude/settings.json";

    #[test]
    fn restore_pairs_are_private_and_scoped_to_the_written_generation() {
        let t = TempDir::new().unwrap();
        let store = Store::new(
            t.path(),
            Instance::Dev { port: 1520 },
            Target::ClaudeCode,
            Path::new(CONFIG),
        );
        let written = sha256_hex(b"written");
        let next = sha256_hex(b"next");
        assert!(store.load(&written).is_none());

        store.save(&written, Some(b"orig")).unwrap();
        store.save(&next, None).unwrap();
        assert!(matches!(store.load(&written), Some(Original::File(bytes)) if bytes == b"orig"));
        assert!(matches!(store.load(&next), Some(Original::Absent)));
        let mode = |p: &Path| fs::metadata(p).unwrap().mode() & 0o777;
        assert_eq!(mode(&store.dir), 0o700);
        assert_eq!(mode(&store.record_path(&written)), 0o600);
        assert_eq!(mode(&store.original_path(&written)), 0o600);

        store.discard(&next);
        assert!(store.load(&next).is_none());
        assert!(store.load(&written).is_some());
        store.clear_except(None);
        assert!(store.load(&written).is_none());
    }

    #[test]
    fn a_tampered_copy_is_not_trusted() {
        let t = TempDir::new().unwrap();
        let store = Store::new(
            t.path(),
            Instance::Installed,
            Target::ClaudeCode,
            Path::new(CONFIG),
        );
        let written = sha256_hex(b"written");
        store.save(&written, Some(b"orig")).unwrap();
        fs::write(store.original_path(&written), b"tampered").unwrap();
        assert!(store.load(&written).is_none());
        fs::write(store.record_path(&written), br#"{"version":1}"#).unwrap();
        assert!(store.load(&written).is_none());
    }
}
