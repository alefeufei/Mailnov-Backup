pub mod auth;
pub mod eml_target;
pub mod error;
mod imap_conn;
pub mod imap_source;
pub mod imap_target;
pub mod source;
pub mod store;
pub mod sync;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod target;
pub mod tls;
pub mod types;

pub use error::{Error, Result};
pub use auth::{Authenticator, PasswordAuth};
pub use eml_target::EmlDirTarget;
pub use imap_source::ImapSource;
pub use imap_target::ImapTarget;
pub use target::MailTarget;
pub use source::MailSource;
pub use tls::TlsMode;
pub use types::*;
