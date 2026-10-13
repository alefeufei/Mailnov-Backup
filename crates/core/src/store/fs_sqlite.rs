use super::paths::eml_relpath;
use super::*;
use crate::{sha256_hex, Error, RawMessage};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const SCHEMA: &str = include_str!("schema.sql");
const USER_VERSION: i64 = 1;

/// Failure-injection points for `commit_batch` (tests only).
#[cfg(any(test, feature = "testing"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultPoint {
    AfterEmlWrite,
    BeforeCommit,
}

pub struct FsSqliteStore {
    root: PathBuf,
    conn: Connection,
    #[cfg(any(test, feature = "testing"))]
    fault: Option<FaultPoint>,
}

#[derive(Default)]
struct Meta {
    message_id: Option<String>,
    date: Option<i64>,
    sender: Option<String>,
    recipients: Option<String>,
    subject: Option<String>,
}

fn parse_meta(bytes: &[u8]) -> Meta {
    let Some(msg) = mail_parser::MessageParser::default().parse(bytes) else {
        return Meta::default();
    };
    let addrs = |a: Option<&mail_parser::Address>| -> Option<String> {
        let list: Vec<String> = a?
            .iter()
            .filter_map(|x| x.address().map(|s| s.to_string()))
            .collect();
        if list.is_empty() {
            None
        } else {
            Some(list.join(", "))
        }
    };
    Meta {
        message_id: msg.message_id().map(str::to_string),
        date: msg.date().map(|d| d.to_timestamp()),
        sender: msg
            .from()
            .and_then(|a| a.first())
            .and_then(|a| a.address())
            .map(str::to_string),
        recipients: addrs(msg.to()),
        subject: msg.subject().map(str::to_string),
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn rel_to_string(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

fn account_id(tx: &Transaction, name: &str) -> Result<i64> {
    tx.execute("INSERT OR IGNORE INTO account(name) VALUES (?1)", [name])?;
    Ok(tx.query_row("SELECT id FROM account WHERE name = ?1", [name], |r| r.get(0))?)
}

/// Write `bytes` to `full` atomically (tmp + fsync + rename); reuse an identical file already there.
fn write_eml(full: &Path, sha: &str, bytes: &[u8]) -> Result<()> {
    if let Ok(existing) = fs::read(full) {
        if sha256_hex(&existing) == sha {
            return Ok(());
        }
    }
    let parent = full.parent().expect("eml path has a parent");
    fs::create_dir_all(parent)?;
    let tmp = parent.join(format!("{sha}.tmp"));
    {
        let mut f = File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    if full.exists() {
        fs::remove_file(full)?;
    }
    fs::rename(&tmp, full)?;
    Ok(())
}

impl FsSqliteStore {
    pub fn open(root: &Path) -> Result<FsSqliteStore> {
        fs::create_dir_all(root)?;
        let toml = root.join("archive.toml");
        if !toml.exists() {
            let id = sha256_hex(
                format!(
                    "{}-{}",
                    std::process::id(),
                    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
                )
                .as_bytes(),
            );
            fs::write(
                &toml,
                format!("format_version = {USER_VERSION}\narchive_id = \"{}\"\n", &id[..32]),
            )?;
        }
        let conn = Connection::open(root.join("index.db"))?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let _mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version > USER_VERSION {
            return Err(Error::Config(format!(
                "archive index version {version} is newer than supported {USER_VERSION}"
            )));
        }
        conn.execute_batch(SCHEMA)?;
        if version < USER_VERSION {
            conn.pragma_update(None, "user_version", USER_VERSION)?;
        }
        Ok(FsSqliteStore {
            root: root.to_path_buf(),
            conn,
            #[cfg(any(test, feature = "testing"))]
            fault: None,
        })
    }

    pub fn user_version(&self) -> Result<i64> {
        Ok(self.conn.query_row("PRAGMA user_version", [], |r| r.get(0))?)
    }

    /// Arm a one-shot failure for the next `commit_batch`.
    #[cfg(any(test, feature = "testing"))]
    pub fn inject_fault(&mut self, point: FaultPoint) {
        self.fault = Some(point);
    }
}

#[cfg(any(test, feature = "testing"))]
macro_rules! fault_check {
    ($f:expr, $p:ident) => {
        if $f == Some(FaultPoint::$p) {
            return Err(Error::Db("injected fault".into()));
        }
    };
}
#[cfg(not(any(test, feature = "testing")))]
macro_rules! fault_check {
    ($f:expr, $p:ident) => {};
}

impl Store for FsSqliteStore {
    fn folder_state(&self, account: &str, folder: &str) -> Result<Option<StoredFolder>> {
        Ok(self
            .conn
            .query_row(
                "SELECT f.uidvalidity, f.last_uid FROM folder f JOIN account a ON a.id = f.account_id
                 WHERE a.name = ?1 AND f.path = ?2",
                params![account, folder],
                |r| Ok(StoredFolder { uidvalidity: r.get(0)?, last_uid: r.get(1)? }),
            )
            .optional()?)
    }

    fn commit_batch(
        &mut self,
        account: &str,
        folder: &str,
        uidvalidity: u32,
        batch: &[RawMessage],
        new_last_uid: u32,
    ) -> Result<BatchOutcome> {
        #[cfg(any(test, feature = "testing"))]
        let fault = self.fault.take();
        let root = self.root.clone();
        let tx = self.conn.transaction()?;
        let acc_id = account_id(&tx, account)?;
        let prev_validity: Option<u32> = tx
            .query_row(
                "SELECT uidvalidity FROM folder WHERE account_id = ?1 AND path = ?2",
                params![acc_id, folder],
                |r| r.get(0),
            )
            .optional()?;
        tx.execute(
            "INSERT INTO folder(account_id, path, uidvalidity, last_uid) VALUES (?1, ?2, ?3, 0)
             ON CONFLICT(account_id, path) DO UPDATE SET uidvalidity = excluded.uidvalidity",
            params![acc_id, folder, uidvalidity],
        )?;
        let folder_id: i64 = tx.query_row(
            "SELECT id FROM folder WHERE account_id = ?1 AND path = ?2",
            params![acc_id, folder],
            |r| r.get(0),
        )?;
        if matches!(prev_validity, Some(v) if v != 0 && v != uidvalidity) {
            // UIDs were renumbered: keep old locations, but detach them from their stale UIDs.
            tx.execute(
                "UPDATE message_location SET uid = NULL WHERE folder_id = ?1",
                [folder_id],
            )?;
        }
        let mut out = BatchOutcome::default();
        for m in batch {
            let sha = sha256_hex(&m.bytes);
            let existing: Option<i64> = tx
                .query_row("SELECT id FROM message WHERE sha256 = ?1", [&sha], |r| r.get(0))
                .optional()?;
            let message_id = match existing {
                Some(id) => {
                    out.duplicates += 1;
                    id
                }
                None => {
                    let rel = eml_relpath(account, folder, &sha);
                    write_eml(&root.join(&rel), &sha, &m.bytes)?;
                    let meta = parse_meta(&m.bytes);
                    tx.execute(
                        "INSERT INTO message(sha256, size, date, sender, recipients, subject, message_id, eml_path, archived_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                        params![
                            sha,
                            m.bytes.len() as i64,
                            meta.date,
                            meta.sender,
                            meta.recipients,
                            meta.subject,
                            meta.message_id,
                            rel_to_string(&rel),
                            now()
                        ],
                    )?;
                    out.added += 1;
                    tx.last_insert_rowid()
                }
            };
            let same: Option<i64> = tx
                .query_row(
                    "SELECT message_id FROM message_location WHERE folder_id = ?1 AND uid = ?2",
                    params![folder_id, m.uid],
                    |r| r.get(0),
                )
                .optional()?;
            match same {
                Some(mid) if mid == message_id => {
                    tx.execute(
                        "UPDATE message_location SET flags = ?1, internal_date = ?2 WHERE folder_id = ?3 AND uid = ?4",
                        params![m.flags.join(" "), m.internal_date, folder_id, m.uid],
                    )?;
                }
                other => {
                    if other.is_some() {
                        // server inconsistency: keep the old row, detach its uid
                        tx.execute(
                            "UPDATE message_location SET uid = NULL WHERE folder_id = ?1 AND uid = ?2",
                            params![folder_id, m.uid],
                        )?;
                    }
                    tx.execute(
                        "INSERT INTO message_location(message_id, folder_id, uid, flags, internal_date)
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![message_id, folder_id, m.uid, m.flags.join(" "), m.internal_date],
                    )?;
                }
            }
        }
        fault_check!(fault, AfterEmlWrite);
        tx.execute(
            "UPDATE folder SET last_uid = ?1, uidvalidity = ?2 WHERE id = ?3",
            params![new_last_uid, uidvalidity, folder_id],
        )?;
        fault_check!(fault, BeforeCommit);
        tx.commit()?;
        Ok(out)
    }

    fn has_message_id(&self, account: &str, message_id: &str) -> Result<bool> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM message m
             JOIN message_location l ON l.message_id = m.id
             JOIN folder f ON f.id = l.folder_id
             JOIN account a ON a.id = f.account_id
             WHERE a.name = ?1 AND m.message_id = ?2",
            params![account, message_id],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    fn update_flags(&mut self, account: &str, folder: &str, uid: u32, flags: &[String]) -> Result<()> {
        self.conn.execute(
            "UPDATE message_location SET flags = ?1
             WHERE uid = ?2 AND folder_id = (
                SELECT f.id FROM folder f JOIN account a ON a.id = f.account_id
                WHERE a.name = ?3 AND f.path = ?4)",
            params![flags.join(" "), uid, account, folder],
        )?;
        Ok(())
    }

    fn messages(&self, account: &str) -> Result<Vec<StoredMessage>> {
        let mut st = self.conn.prepare(
            "SELECT m.sha256, a.name, f.path, l.uid, l.flags, l.internal_date
             FROM message_location l
             JOIN message m ON m.id = l.message_id
             JOIN folder f ON f.id = l.folder_id
             JOIN account a ON a.id = f.account_id
             WHERE a.name = ?1 ORDER BY l.id",
        )?;
        let rows = st.query_map([account], |r| {
            let flags: String = r.get(4)?;
            Ok(StoredMessage {
                sha256: r.get(0)?,
                account: r.get(1)?,
                folder: r.get(2)?,
                uid: r.get(3)?,
                flags: flags.split_whitespace().map(str::to_string).collect(),
                internal_date: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    fn read_eml(&self, sha256: &str) -> Result<Vec<u8>> {
        let rel: Option<String> = self
            .conn
            .query_row("SELECT eml_path FROM message WHERE sha256 = ?1", [sha256], |r| r.get(0))
            .optional()?;
        let rel = rel.ok_or_else(|| Error::Db(format!("unknown message {sha256}")))?;
        Ok(fs::read(self.root.join(rel))?)
    }

    fn export_done(&self, target: &str, sha256: &str, folder: &str) -> Result<bool> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM export_state WHERE target = ?1 AND sha256 = ?2 AND folder = ?3 AND status = 'done'",
            params![target, sha256, folder],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    fn mark_exported(&mut self, target: &str, sha256: &str, folder: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO export_state(target, sha256, folder, status) VALUES (?1, ?2, ?3, 'done')",
            params![target, sha256, folder],
        )?;
        Ok(())
    }

    fn verify(&self) -> Result<VerifyReport> {
        let mut st = self.conn.prepare("SELECT sha256, eml_path FROM message ORDER BY id")?;
        let rows = st
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut rep = VerifyReport::default();
        for (sha, rel) in rows {
            rep.checked += 1;
            match fs::read(self.root.join(&rel)) {
                Ok(bytes) if sha256_hex(&bytes) == sha => {}
                Ok(_) => rep.corrupt.push(sha),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => rep.missing.push(sha),
                Err(e) => return Err(e.into()),
            }
        }
        Ok(rep)
    }

    fn reindex(&mut self) -> Result<usize> {
        let root = self.root.clone();
        let mut files: Vec<(String, String, PathBuf)> = Vec::new();
        let mail = root.join("mail");
        let subdirs = |p: &Path| -> Result<Vec<PathBuf>> {
            let mut v = Vec::new();
            if let Ok(rd) = fs::read_dir(p) {
                for e in rd {
                    let p = e?.path();
                    if p.is_dir() {
                        v.push(p);
                    }
                }
            }
            v.sort();
            Ok(v)
        };
        let name = |p: &Path| p.file_name().unwrap_or_default().to_string_lossy().to_string();
        for acc in subdirs(&mail)? {
            for fol in subdirs(&acc)? {
                for pre in subdirs(&fol)? {
                    for e in fs::read_dir(&pre)? {
                        let p = e?.path();
                        if p.extension().is_some_and(|x| x == "eml") {
                            files.push((name(&acc), name(&fol), p));
                        }
                    }
                }
            }
        }
        let tx = self.conn.transaction()?;
        for (acc, fol, path) in &files {
            let bytes = fs::read(path)?;
            let sha = sha256_hex(&bytes);
            let acc_id = account_id(&tx, acc)?;
            tx.execute(
                "INSERT OR IGNORE INTO folder(account_id, path, uidvalidity, last_uid) VALUES (?1, ?2, 0, 0)",
                params![acc_id, fol],
            )?;
            let folder_id: i64 = tx.query_row(
                "SELECT id FROM folder WHERE account_id = ?1 AND path = ?2",
                params![acc_id, fol],
                |r| r.get(0),
            )?;
            let existing: Option<i64> = tx
                .query_row("SELECT id FROM message WHERE sha256 = ?1", [&sha], |r| r.get(0))
                .optional()?;
            let mid = match existing {
                Some(id) => id,
                None => {
                    let meta = parse_meta(&bytes);
                    let rel = path.strip_prefix(&root).unwrap_or(path);
                    tx.execute(
                        "INSERT INTO message(sha256, size, date, sender, recipients, subject, message_id, eml_path, archived_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                        params![
                            sha,
                            bytes.len() as i64,
                            meta.date,
                            meta.sender,
                            meta.recipients,
                            meta.subject,
                            meta.message_id,
                            rel_to_string(rel),
                            now()
                        ],
                    )?;
                    tx.last_insert_rowid()
                }
            };
            let has_loc: i64 = tx.query_row(
                "SELECT COUNT(*) FROM message_location WHERE message_id = ?1 AND folder_id = ?2",
                params![mid, folder_id],
                |r| r.get(0),
            )?;
            if has_loc == 0 {
                tx.execute(
                    "INSERT INTO message_location(message_id, folder_id, uid, flags) VALUES (?1, ?2, NULL, '')",
                    params![mid, folder_id],
                )?;
            }
        }
        tx.commit()?;
        Ok(files.len())
    }
}

#[cfg(test)]
#[path = "fs_tests.rs"]
mod tests;
