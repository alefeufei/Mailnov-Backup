pub mod fs_sqlite;
pub mod paths;

use crate::{RawMessage, Result};

#[cfg(any(test, feature = "testing"))]
pub use fs_sqlite::FaultPoint;
pub use fs_sqlite::FsSqliteStore;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredFolder {
    pub uidvalidity: u32,
    pub last_uid: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredMessage {
    pub sha256: String,
    pub account: String,
    pub folder: String,
    pub uid: Option<u32>,
    pub flags: Vec<String>,
    pub internal_date: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BatchOutcome {
    pub added: usize,
    pub duplicates: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VerifyReport {
    pub checked: usize,
    pub missing: Vec<String>,
    pub corrupt: Vec<String>,
}

pub trait Store {
    fn folder_state(&self, account: &str, folder: &str) -> Result<Option<StoredFolder>>;
    fn commit_batch(
        &mut self,
        account: &str,
        folder: &str,
        uidvalidity: u32,
        batch: &[RawMessage],
        new_last_uid: u32,
    ) -> Result<BatchOutcome>;
    /// Whether a message with this Message-ID has a location in `folder` of `account` (with or
    /// without a UID: a location whose UID was detached by a UIDVALIDITY change still counts).
    fn has_message_id(&self, account: &str, folder: &str, message_id: &str) -> Result<bool>;
    fn update_flags(
        &mut self,
        account: &str,
        folder: &str,
        uid: u32,
        flags: &[String],
    ) -> Result<()>;
    fn messages(&self, account: &str) -> Result<Vec<StoredMessage>>;
    /// The archived bytes of a message, checked against `sha256`. A missing or altered file is
    /// `Error::Corrupt` (a problem of that one message), never `Error::Io`.
    fn read_eml(&self, sha256: &str) -> Result<Vec<u8>>;
    fn export_done(&self, target: &str, sha256: &str, folder: &str) -> Result<bool>;
    fn mark_exported(&mut self, target: &str, sha256: &str, folder: &str) -> Result<()>;
    fn verify(&self) -> Result<VerifyReport>;
    fn reindex(&mut self) -> Result<usize>;
}
