use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawMessage {
    pub uid: u32,
    pub flags: Vec<String>,
    pub internal_date: Option<i64>,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FolderState {
    pub uidvalidity: u32,
    pub uidnext: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    FolderStarted { folder: String, total: usize },
    MessageArchived { folder: String, uid: u32 },
    MessageFailed { folder: String, uid: u32, reason: String },
    Finished(RunSummary),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunSummary {
    pub added: usize,
    pub duplicates: usize,
    pub failed: usize,
}

/// Lowercase hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_hex_matches_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
