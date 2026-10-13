use crate::Result;
use std::fmt;

/// Source of IMAP login credentials. Object safe; the password is requested on every (re)connect
/// so implementations may fetch it lazily (e.g. from the OS keyring).
pub trait Authenticator {
    fn user(&self) -> &str;
    fn password(&self) -> Result<String>;
}

/// Plain user/password credentials. `Debug` never prints the password.
#[derive(Clone)]
pub struct PasswordAuth {
    pub user: String,
    pub password: String,
}

impl PasswordAuth {
    pub fn new(user: impl Into<String>, password: impl Into<String>) -> Self {
        Self { user: user.into(), password: password.into() }
    }
}

impl fmt::Debug for PasswordAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PasswordAuth")
            .field("user", &self.user)
            .field("password", &"<redacted>")
            .finish()
    }
}

impl Authenticator for PasswordAuth {
    fn user(&self) -> &str {
        &self.user
    }
    fn password(&self) -> Result<String> {
        Ok(self.password.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_redacts_password() {
        let a = PasswordAuth::new("alice", "s3cr3t-pw");
        let d = format!("{a:?}");
        assert!(d.contains("alice"));
        assert!(!d.contains("s3cr3t-pw"));
    }

    #[test]
    fn usable_as_trait_object() {
        let a = PasswordAuth::new("u", "p");
        let o: &dyn Authenticator = &a;
        assert_eq!(o.user(), "u");
        assert_eq!(o.password().unwrap(), "p");
    }
}
