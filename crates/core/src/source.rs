use crate::{FolderState, RawMessage, Result};

/// Abstraction over a remote mailbox (IMAP in production, `FakeSource` in tests).
pub trait MailSource {
    fn list_folders(&mut self) -> Result<Vec<String>>;
    fn select(&mut self, folder: &str) -> Result<FolderState>;
    fn uids_after(&mut self, last_uid: u32, since: Option<i64>) -> Result<Vec<u32>>;
    fn message_ids(&mut self, uids: &[u32]) -> Result<Vec<(u32, Option<String>)>>;
    fn fetch(&mut self, uids: &[u32]) -> Result<Vec<RawMessage>>;
    fn flags(&mut self, uids: &[u32]) -> Result<Vec<(u32, Vec<String>)>>;
}
