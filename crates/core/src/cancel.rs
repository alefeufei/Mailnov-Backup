//! Cooperative cancellation without touching the engines: decorators around a [`MailSource`] /
//! [`MailTarget`] that start failing as soon as a shared flag is set.
//!
//! The error is `Error::Connection("cancelled by user")`, which both engines already treat as a
//! systemic failure: `archive` stops at once (everything committed so far stays committed, the
//! next run resumes), `export` stops after its usual streak of connection failures (a message is
//! only recorded as exported after `put` succeeded, so nothing is lost or marked wrongly).
//! Cancellation therefore takes effect at the next call to the server, not in the middle of one.
use crate::{Error, FolderState, MailSource, MailTarget, RawMessage, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Reason carried by the error returned once cancellation was requested.
pub const CANCELLED: &str = "cancelled by user";

/// The error returned by the decorators after cancellation.
pub fn cancelled_error() -> Error {
    Error::Connection(CANCELLED.to_string())
}

/// Whether `e` (or its text, as reported in a failure event) is the cancellation error.
pub fn is_cancelled(e: &Error) -> bool {
    matches!(e, Error::Connection(m) if m == CANCELLED)
}

/// Whether a failure reason (`Error` Display text) comes from a cancellation.
pub fn is_cancelled_reason(reason: &str) -> bool {
    reason.ends_with(CANCELLED)
}

fn check(flag: &AtomicBool) -> Result<()> {
    if flag.load(Ordering::SeqCst) {
        Err(cancelled_error())
    } else {
        Ok(())
    }
}

/// A [`MailSource`] that fails every call made after `flag` was set.
pub struct CancellableSource<S> {
    inner: S,
    flag: Arc<AtomicBool>,
}

impl<S> CancellableSource<S> {
    pub fn new(inner: S, flag: Arc<AtomicBool>) -> Self {
        Self { inner, flag }
    }
    pub fn into_inner(self) -> S {
        self.inner
    }
}

impl<S: MailSource> MailSource for CancellableSource<S> {
    fn list_folders(&mut self) -> Result<Vec<String>> {
        check(&self.flag)?;
        self.inner.list_folders()
    }
    fn select(&mut self, folder: &str) -> Result<FolderState> {
        check(&self.flag)?;
        self.inner.select(folder)
    }
    fn uids_after(&mut self, last_uid: u32, since: Option<i64>) -> Result<Vec<u32>> {
        check(&self.flag)?;
        self.inner.uids_after(last_uid, since)
    }
    fn message_ids(&mut self, uids: &[u32]) -> Result<Vec<(u32, Option<String>)>> {
        check(&self.flag)?;
        self.inner.message_ids(uids)
    }
    fn fetch(&mut self, uids: &[u32]) -> Result<Vec<RawMessage>> {
        check(&self.flag)?;
        self.inner.fetch(uids)
    }
    fn flags(&mut self, uids: &[u32]) -> Result<Vec<(u32, Vec<String>)>> {
        check(&self.flag)?;
        self.inner.flags(uids)
    }
}

/// A [`MailTarget`] that fails every `put` made after `flag` was set.
///
/// `ensure_folder` deliberately does not fail after cancellation (it does nothing instead):
/// the export engine treats a folder that cannot be created as a failure of each of its pending
/// messages and moves on to the next folder, so failing it would turn one cancellation into a
/// failure per remaining message. Failing `put` instead stops the run after the engine's short
/// streak of connection errors, and nothing is ever recorded as exported without a real `put`.
pub struct CancellableTarget<T> {
    inner: T,
    flag: Arc<AtomicBool>,
}

impl<T> CancellableTarget<T> {
    pub fn new(inner: T, flag: Arc<AtomicBool>) -> Self {
        Self { inner, flag }
    }
    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<T: MailTarget> MailTarget for CancellableTarget<T> {
    fn id(&self) -> String {
        self.inner.id()
    }
    fn ensure_folder(&mut self, folder: &str) -> Result<()> {
        if self.flag.load(Ordering::SeqCst) {
            return Ok(());
        }
        self.inner.ensure_folder(folder)
    }
    fn put(
        &mut self,
        folder: &str,
        bytes: &[u8],
        flags: &[String],
        internal_date: Option<i64>,
    ) -> Result<()> {
        check(&self.flag)?;
        self.inner.put(folder, bytes, flags, internal_date)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeSource;

    #[derive(Default)]
    struct RecTarget {
        puts: usize,
        folders: usize,
    }
    impl MailTarget for RecTarget {
        fn id(&self) -> String {
            "rec".into()
        }
        fn ensure_folder(&mut self, _f: &str) -> Result<()> {
            self.folders += 1;
            Ok(())
        }
        fn put(&mut self, _f: &str, _b: &[u8], _fl: &[String], _d: Option<i64>) -> Result<()> {
            self.puts += 1;
            Ok(())
        }
    }

    fn fake() -> FakeSource {
        let mut s = FakeSource::new();
        s.add_folder("INBOX", 1);
        s.add_message("INBOX", 1, b"Subject: a\r\n\r\nx".to_vec(), &[]);
        s
    }

    #[test]
    fn source_passes_through_until_cancelled_then_fails_every_call() {
        let flag = Arc::new(AtomicBool::new(false));
        let mut s = CancellableSource::new(fake(), flag.clone());
        assert_eq!(s.list_folders().unwrap(), vec!["INBOX"]);
        s.select("INBOX").unwrap();
        assert_eq!(s.uids_after(0, None).unwrap(), vec![1]);
        assert_eq!(s.fetch(&[1]).unwrap().len(), 1);
        flag.store(true, Ordering::SeqCst);
        assert!(is_cancelled(&s.list_folders().unwrap_err()));
        assert!(is_cancelled(&s.select("INBOX").unwrap_err()));
        assert!(is_cancelled(&s.uids_after(0, None).unwrap_err()));
        assert!(is_cancelled(&s.message_ids(&[1]).unwrap_err()));
        assert!(is_cancelled(&s.flags(&[1]).unwrap_err()));
        let e = s.fetch(&[1]).unwrap_err();
        assert!(is_cancelled(&e));
        assert!(is_cancelled_reason(&e.to_string()));
        // the inner source was not called after cancellation
        assert_eq!(s.into_inner().fetch_calls, 1);
    }

    #[test]
    fn target_put_fails_after_cancel_and_ensure_folder_is_a_no_op() {
        let flag = Arc::new(AtomicBool::new(false));
        let mut t = CancellableTarget::new(RecTarget::default(), flag.clone());
        assert_eq!(t.id(), "rec");
        t.ensure_folder("INBOX").unwrap();
        t.put("INBOX", b"x", &[], None).unwrap();
        flag.store(true, Ordering::SeqCst);
        t.ensure_folder("Other").unwrap();
        assert!(is_cancelled(&t.put("INBOX", b"x", &[], None).unwrap_err()));
        let inner = t.into_inner();
        assert_eq!((inner.folders, inner.puts), (1, 1));
    }

    #[test]
    fn other_connection_errors_are_not_cancellation() {
        assert!(!is_cancelled(&Error::Connection("reset".into())));
        assert!(!is_cancelled(&Error::Imap(CANCELLED.into())));
    }
}
