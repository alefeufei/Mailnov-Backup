//! In-memory fakes for tests (`cfg(test)` or feature `testing`).
use crate::{Error, FolderState, MailSource, RawMessage, Result};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Kind of error injected by the per-folder failure hooks of [`FakeSource`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Injected {
    /// A server `NO`/`BAD` for that folder (`Error::Imap`).
    Imap,
    /// The connection is gone (`Error::Connection`).
    Connection,
}

impl Injected {
    fn error(self, what: &str, folder: &str) -> Error {
        match self {
            Injected::Imap => Error::Imap(format!("injected: {what} failed for {folder}")),
            Injected::Connection => Error::Connection(format!(
                "injected: connection lost during {what} of {folder}"
            )),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FakeMsg {
    pub uid: u32,
    pub flags: Vec<String>,
    pub internal_date: Option<i64>,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Default)]
pub struct FakeFolder {
    pub uidvalidity: u32,
    pub messages: Vec<FakeMsg>,
}

#[derive(Debug, Default)]
pub struct FakeSource {
    pub folders: BTreeMap<String, FakeFolder>,
    /// Any `fetch` containing one of these UIDs fails with `Error::Imap` (a bad message).
    pub fail_uids: HashSet<u32>,
    /// Any `fetch` containing one of these UIDs fails with `Error::Connection` (checked first).
    pub fail_uids_connection: HashSet<u32>,
    /// After this many successful `fetch` calls, every `fetch` fails with `Error::Connection`.
    pub fail_after_fetches: Option<usize>,
    /// Number of `fetch` calls made (successful or not).
    pub fetch_calls: usize,
    /// Every UID requested through `fetch`, in order.
    pub fetched_uids: Vec<u32>,
    /// UIDs silently omitted from `fetch` results while still listed (truncated response).
    pub drop_from_fetch: HashSet<u32>,
    /// UIDs expunged from the folder the moment they are fetched (gone between listing and fetch).
    pub expunge_on_fetch: HashSet<u32>,
    /// `select` of these folders fails with the given kind of error.
    pub fail_select: HashMap<String, Injected>,
    /// `uids_after` fails with the given kind of error while these folders are selected.
    pub fail_uids_after: HashMap<String, Injected>,
    successful_fetches: usize,
    selected: Option<String>,
}

impl FakeSource {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_folder(&mut self, name: &str, uidvalidity: u32) {
        self.folders
            .entry(name.to_string())
            .or_default()
            .uidvalidity = uidvalidity;
    }

    pub fn add_message(&mut self, folder: &str, uid: u32, bytes: Vec<u8>, flags: &[&str]) {
        self.add_message_dated(folder, uid, bytes, flags, Some(1_000));
    }

    pub fn add_message_dated(
        &mut self,
        folder: &str,
        uid: u32,
        bytes: Vec<u8>,
        flags: &[&str],
        internal_date: Option<i64>,
    ) {
        self.folders
            .entry(folder.to_string())
            .or_default()
            .messages
            .push(FakeMsg {
                uid,
                flags: flags.iter().map(|s| s.to_string()).collect(),
                internal_date,
                bytes,
            });
    }

    pub fn remove_message(&mut self, folder: &str, uid: u32) {
        if let Some(f) = self.folders.get_mut(folder) {
            f.messages.retain(|m| m.uid != uid);
        }
    }

    pub fn set_flags(&mut self, folder: &str, uid: u32, flags: &[&str]) {
        if let Some(m) = self
            .folders
            .get_mut(folder)
            .and_then(|f| f.messages.iter_mut().find(|m| m.uid == uid))
        {
            m.flags = flags.iter().map(|s| s.to_string()).collect();
        }
    }

    /// Change UIDVALIDITY and renumber every UID as `old + offset`.
    pub fn renumber(&mut self, folder: &str, new_uidvalidity: u32, offset: u32) {
        if let Some(f) = self.folders.get_mut(folder) {
            f.uidvalidity = new_uidvalidity;
            for m in &mut f.messages {
                m.uid += offset;
            }
        }
    }

    pub fn clear_failures(&mut self) {
        self.fail_uids.clear();
        self.fail_uids_connection.clear();
        self.fail_after_fetches = None;
        self.drop_from_fetch.clear();
        self.expunge_on_fetch.clear();
        self.fail_select.clear();
        self.fail_uids_after.clear();
    }

    fn current(&self) -> Result<&FakeFolder> {
        let name = self
            .selected
            .as_ref()
            .ok_or_else(|| Error::Imap("no folder selected".into()))?;
        self.folders
            .get(name)
            .ok_or_else(|| Error::Imap(format!("no such folder {name}")))
    }
}

fn header_message_id(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    for line in text.lines() {
        if line.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case("message-id") {
                let v = v.trim().trim_start_matches('<').trim_end_matches('>');
                return if v.is_empty() {
                    None
                } else {
                    Some(v.to_string())
                };
            }
        }
    }
    None
}

impl MailSource for FakeSource {
    fn list_folders(&mut self) -> Result<Vec<String>> {
        Ok(self.folders.keys().cloned().collect())
    }

    fn select(&mut self, folder: &str) -> Result<FolderState> {
        if let Some(kind) = self.fail_select.get(folder) {
            return Err(kind.error("select", folder));
        }
        let f = self
            .folders
            .get(folder)
            .ok_or_else(|| Error::Imap(format!("no such folder {folder}")))?;
        let uidnext = f.messages.iter().map(|m| m.uid).max().map_or(1, |u| u + 1);
        let st = FolderState {
            uidvalidity: f.uidvalidity,
            uidnext,
        };
        self.selected = Some(folder.to_string());
        Ok(st)
    }

    fn uids_after(&mut self, last_uid: u32, since: Option<i64>) -> Result<Vec<u32>> {
        if let Some(name) = &self.selected {
            if let Some(kind) = self.fail_uids_after.get(name) {
                return Err(kind.error("uids_after", name));
            }
        }
        let mut v: Vec<u32> = self
            .current()?
            .messages
            .iter()
            .filter(|m| m.uid > last_uid)
            .filter(|m| match (since, m.internal_date) {
                (Some(s), Some(d)) => d >= s,
                _ => true,
            })
            .map(|m| m.uid)
            .collect();
        v.sort_unstable();
        Ok(v)
    }

    fn message_ids(&mut self, uids: &[u32]) -> Result<Vec<(u32, Option<String>)>> {
        let f = self.current()?;
        Ok(uids
            .iter()
            .filter_map(|u| f.messages.iter().find(|m| m.uid == *u))
            .map(|m| (m.uid, header_message_id(&m.bytes)))
            .collect())
    }

    fn fetch(&mut self, uids: &[u32]) -> Result<Vec<RawMessage>> {
        self.fetch_calls += 1;
        self.fetched_uids.extend_from_slice(uids);
        if let Some(n) = self.fail_after_fetches {
            if self.successful_fetches >= n {
                return Err(Error::Connection("injected: connection lost".into()));
            }
        }
        if let Some(u) = uids.iter().find(|u| self.fail_uids_connection.contains(u)) {
            return Err(Error::Connection(format!(
                "injected: connection lost fetching uid {u}"
            )));
        }
        if let Some(u) = uids.iter().find(|u| self.fail_uids.contains(u)) {
            return Err(Error::Imap(format!("injected: bad message uid {u}")));
        }
        if let Some(name) = self.selected.clone() {
            if let Some(f) = self.folders.get_mut(&name) {
                let gone = &self.expunge_on_fetch;
                f.messages
                    .retain(|m| !(gone.contains(&m.uid) && uids.contains(&m.uid)));
            }
        }
        let f = self.current()?;
        let out = uids
            .iter()
            .filter(|u| !self.drop_from_fetch.contains(u))
            .filter_map(|u| f.messages.iter().find(|m| m.uid == *u))
            .map(|m| RawMessage {
                uid: m.uid,
                flags: m.flags.clone(),
                internal_date: m.internal_date,
                bytes: m.bytes.clone(),
            })
            .collect();
        self.successful_fetches += 1;
        Ok(out)
    }

    fn flags(&mut self, uids: &[u32]) -> Result<Vec<(u32, Vec<String>)>> {
        let f = self.current()?;
        Ok(uids
            .iter()
            .filter_map(|u| f.messages.iter().find(|m| m.uid == *u))
            .map(|m| (m.uid, m.flags.clone()))
            .collect())
    }
}
