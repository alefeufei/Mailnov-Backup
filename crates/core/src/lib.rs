pub mod auth;
pub mod autoconfig;
pub mod cancel;
pub mod eml_target;
pub mod error;
mod imap_conn;
pub mod imap_source;
pub mod imap_target;
pub mod profile;
pub mod secrets;
pub mod service;
pub mod source;
pub mod store;
pub mod sync;
pub mod target;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod tls;
pub mod types;

pub use auth::{Authenticator, PasswordAuth};
pub use autoconfig::guess_imap;
pub use eml_target::EmlDirTarget;
pub use error::{Error, Result};
pub use imap_source::ImapSource;
pub use imap_target::ImapTarget;
pub use profile::{Endpoint, Profile, ProfileStore, TargetSpec};
#[cfg(any(test, feature = "testing"))]
pub use secrets::MemorySecrets;
pub use secrets::{secret_key, KeyringSecrets, SecretRole, SecretStore};
pub use source::MailSource;
pub use target::MailTarget;
pub use tls::TlsMode;
pub use types::*;
