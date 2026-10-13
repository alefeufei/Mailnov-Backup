//! !!! TEST ONLY !!!  This module is compiled only with the `test-secrets` cargo feature.
//!
//! `FileSecrets` stores passwords as PLAINTEXT files under `<config-dir>/test-secrets/`. It exists
//! solely so integration tests can run the real binary without touching the OS credential store.
//! It must never be enabled in a build that handles real mail accounts.
use mailbackup_core::{Error, Result, SecretStore};
use std::path::PathBuf;

pub struct FileSecrets {
    dir: PathBuf,
}

impl FileSecrets {
    pub fn new(config_dir: &std::path::Path) -> Self {
        Self {
            dir: config_dir.join("test-secrets"),
        }
    }

    /// One file per key; the key is hex-encoded so it is a valid file name on every platform.
    fn path(&self, key: &str) -> PathBuf {
        let hex: String = key.bytes().map(|b| format!("{b:02x}")).collect();
        self.dir.join(hex)
    }
}

impl SecretStore for FileSecrets {
    fn set(&self, key: &str, secret: &str) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        std::fs::write(self.path(key), secret)?;
        Ok(())
    }
    fn get(&self, key: &str) -> Result<String> {
        match std::fs::read_to_string(self.path(key)) {
            Ok(s) => Ok(s),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(Error::Auth(format!(
                "no password stored in the credential store for '{key}'"
            ))),
            Err(e) => Err(Error::Io(e)),
        }
    }
    fn delete(&self, key: &str) -> Result<()> {
        match std::fs::remove_file(self.path(key)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Error::Io(e)),
        }
    }
}
