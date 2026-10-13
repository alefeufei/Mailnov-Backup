#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("database error: {0}")]
    Db(String),
    #[error("imap error: {0}")]
    Imap(String),
    #[error("connection error: {0}")]
    Connection(String),
    #[error("authentication error: {0}")]
    Auth(String),
    #[error("config error: {0}")]
    Config(String),
    #[error("corrupt data: {0}")]
    Corrupt(String),
    /// The folder being read cannot be used any more in this run (it vanished or its UIDVALIDITY
    /// changed, typically noticed when re-opening it after a reconnect). Ends that folder only.
    #[error("folder unavailable: {0}")]
    FolderGone(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Error::Db(e.to_string())
    }
}
