use crate::{FolderState, RawMessage, Result};

/// Abstraction over a remote mailbox (IMAP in production, `FakeSource` in tests).
///
/// Contract: `fetch`, `message_ids` and `flags` must return `Err` for transient or protocol
/// problems, never a silently truncated `Ok`. A UID may be absent from an `Ok` result only if
/// it no longer exists on the server; the engine double-checks with `uids_after` and treats
/// UIDs that are still listed as failed messages.
pub trait MailSource {
    fn list_folders(&mut self) -> Result<Vec<String>>;
    fn select(&mut self, folder: &str) -> Result<FolderState>;
    fn uids_after(&mut self, last_uid: u32, since: Option<i64>) -> Result<Vec<u32>>;
    fn message_ids(&mut self, uids: &[u32]) -> Result<Vec<(u32, Option<String>)>>;
    fn fetch(&mut self, uids: &[u32]) -> Result<Vec<RawMessage>>;
    fn flags(&mut self, uids: &[u32]) -> Result<Vec<(u32, Vec<String>)>>;
}
