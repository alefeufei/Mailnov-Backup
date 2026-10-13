pub mod error;
pub mod source;
pub mod store;
pub mod sync;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod types;

pub use error::{Error, Result};
pub use source::MailSource;
pub use types::*;
