//! Export to a directory of `.eml` files (`<dir>/<folder components>/<date>-<sha256>.eml`).
use crate::store::paths::safe_component;
use crate::target::MailTarget;
use crate::{sha256_hex, Result};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

pub struct EmlDirTarget {
    dir: PathBuf,
}

impl EmlDirTarget {
    /// The directory is created if missing (best effort; `put` reports real failures) so that its
    /// canonical path, which is part of the target id, is the same on every run.
    pub fn new(dir: &Path) -> Self {
        let _ = fs::create_dir_all(dir);
        let dir = dir
            .canonicalize()
            .or_else(|_| std::path::absolute(dir))
            .unwrap_or_else(|_| dir.to_path_buf());
        Self { dir }
    }

    /// `<dir>/<safe_component(c1)>/<safe_component(c2)>/...` for a "/"-separated folder path.
    fn folder_dir(&self, folder: &str) -> PathBuf {
        let mut p = self.dir.clone();
        let mut any = false;
        for c in folder.split('/').filter(|c| !c.is_empty()) {
            p.push(safe_component(c));
            any = true;
        }
        if !any {
            p.push(safe_component(""));
        }
        p
    }
}

impl MailTarget for EmlDirTarget {
    fn id(&self) -> String {
        format!("eml-dir:{}", self.dir.display())
    }

    fn ensure_folder(&mut self, folder: &str) -> Result<()> {
        fs::create_dir_all(self.folder_dir(folder))?;
        Ok(())
    }

    /// Flags are not representable in a plain `.eml` file and are dropped; the internal date is
    /// part of the file name so the folder sorts chronologically. The file name also holds the
    /// full SHA-256, so distinct messages never collide and a re-run finds its earlier files.
    fn put(
        &mut self,
        folder: &str,
        bytes: &[u8],
        _flags: &[String],
        internal_date: Option<i64>,
    ) -> Result<()> {
        let dir = self.folder_dir(folder);
        fs::create_dir_all(&dir)?;
        let name = format!(
            "{:010}-{}.eml",
            internal_date.unwrap_or(0).max(0),
            sha256_hex(bytes)
        );
        let path = dir.join(&name);
        if path.exists() {
            return Ok(());
        }
        // Write next to the final name and rename, so a crash never leaves a partial `.eml`.
        let tmp = dir.join(format!(".{name}.tmp"));
        let written = (|| -> std::io::Result<()> {
            let mut f = File::create(&tmp)?;
            f.write_all(bytes)?;
            f.sync_all()?;
            fs::rename(&tmp, &path)
        })();
        if let Err(e) = written {
            let _ = fs::remove_file(&tmp);
            return Err(e.into());
        }
        Ok(())
    }
}
