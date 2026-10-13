pub mod auth;
pub mod error;
mod imap_conn;
pub mod imap_source;
pub mod source;
pub mod store;
pub mod sync;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod tls;
pub mod types;

pub use error::{Error, Result};
pub use auth::{Authenticator, PasswordAuth};
pub use imap_source::ImapSource;
pub use source::MailSource;
pub use tls::TlsMode;
pub use types::*;
