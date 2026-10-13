#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("database error: {0}")]
    Db(String),
    #[error("imap error: {0}")]
    Imap(String),
    #[error("authentication error: {0}")]
    Auth(String),
    #[error("config error: {0}")]
    Config(String),
    #[error("corrupt data: {0}")]
    Corrupt(String),
}

pub type Result<T> = std::result::Result<T, Error>;
