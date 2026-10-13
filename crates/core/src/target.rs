use crate::Result;

/// Destination of an export: a directory of `.eml` files or another IMAP account.
///
/// Folder paths use the archive's canonical "/" separator; each target maps them to its own
/// naming rules. `put` must not report success unless the message is durably stored, because
/// the export engine records it as exported afterwards and never sends it again.
pub trait MailTarget {
    /// Stable identity of the destination, used as the `export_state` key.
    fn id(&self) -> String;
    /// Make sure `folder` exists. Idempotent; may be called again for a folder already created.
    fn ensure_folder(&mut self, folder: &str) -> Result<()>;
    /// Store one message in `folder` (flags as stored in the archive; the target decides which
    /// it can and should keep).
    fn put(
        &mut self,
        folder: &str,
        bytes: &[u8],
        flags: &[String],
        internal_date: Option<i64>,
    ) -> Result<()>;
}
