//! Secret storage. Passwords never touch profile files; they live in the OS credential store.

use crate::{Error, Result};

/// Which side of a profile a secret belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretRole {
    Source,
    Target,
}

/// Key under which a profile's secret is stored: `mailbackup:<profile>:source|target`.
pub fn secret_key(profile: &str, role: SecretRole) -> String {
    let role = match role {
        SecretRole::Source => "source",
        SecretRole::Target => "target",
    };
    format!("mailbackup:{profile}:{role}")
}

pub trait SecretStore {
    fn set(&self, key: &str, secret: &str) -> Result<()>;
    fn get(&self, key: &str) -> Result<String>;
    /// Removes the secret; deleting a missing key is not an error.
    fn delete(&self, key: &str) -> Result<()>;
}

/// OS-native credential store (Windows Credential Manager, macOS Keychain, Secret Service).
pub struct KeyringSecrets;

const KEYRING_SERVICE: &str = "mailbackup";

fn entry(key: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, key).map_err(|e| map_err(key, "open", &e))
}

/// Maps a keyring error to ours. Only the key (not the secret) and the error description are
/// included; keyring errors never carry the secret value.
fn map_err(key: &str, op: &str, e: &keyring::Error) -> Error {
    match e {
        keyring::Error::NoEntry => Error::Auth(format!(
            "no password stored in the credential store for '{key}'"
        )),
        keyring::Error::NoStorageAccess(_) | keyring::Error::NoDefaultStore => Error::Config(
            format!("credential store is not accessible (could not {op} '{key}'): {e}"),
        ),
        _ => Error::Config(format!(
            "credential store error (could not {op} '{key}'): {e}"
        )),
    }
}

impl SecretStore for KeyringSecrets {
    fn set(&self, key: &str, secret: &str) -> Result<()> {
        entry(key)?
            .set_password(secret)
            .map_err(|e| map_err(key, "store", &e))
    }
    fn get(&self, key: &str) -> Result<String> {
        entry(key)?
            .get_password()
            .map_err(|e| map_err(key, "read", &e))
    }
    fn delete(&self, key: &str) -> Result<()> {
        match entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(map_err(key, "delete", &e)),
        }
    }
}

/// In-memory fake for tests.
#[cfg(any(test, feature = "testing"))]
#[derive(Default)]
pub struct MemorySecrets {
    map: std::sync::Mutex<std::collections::HashMap<String, String>>,
}

#[cfg(any(test, feature = "testing"))]
impl MemorySecrets {
    pub fn new() -> Self {
        Self::default()
    }
}

#[cfg(any(test, feature = "testing"))]
impl SecretStore for MemorySecrets {
    fn set(&self, key: &str, secret: &str) -> Result<()> {
        self.map.lock().unwrap().insert(key.into(), secret.into());
        Ok(())
    }
    fn get(&self, key: &str) -> Result<String> {
        self.map
            .lock()
            .unwrap()
            .get(key)
            .cloned()
            .ok_or_else(|| Error::Auth(format!("no password stored for '{key}'")))
    }
    fn delete(&self, key: &str) -> Result<()> {
        self.map.lock().unwrap().remove(key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_key_format() {
        assert_eq!(
            secret_key("work", SecretRole::Source),
            "mailbackup:work:source"
        );
        assert_eq!(
            secret_key("work", SecretRole::Target),
            "mailbackup:work:target"
        );
    }

    fn exercise(store: &dyn SecretStore) {
        let k = secret_key("p", SecretRole::Source);
        assert!(matches!(store.get(&k), Err(Error::Auth(_))));
        store.set(&k, "hunter2").unwrap();
        assert_eq!(store.get(&k).unwrap(), "hunter2");
        store.set(&k, "other").unwrap();
        assert_eq!(store.get(&k).unwrap(), "other");
        store.delete(&k).unwrap();
        store.delete(&k).unwrap();
        assert!(store.get(&k).is_err());
    }

    #[test]
    fn secret_store_trait_works_with_in_memory_fake() {
        exercise(&MemorySecrets::new());
    }

    #[test]
    fn missing_secret_error_does_not_leak_other_secrets() {
        let s = MemorySecrets::new();
        s.set("a", "topsecret").unwrap();
        let msg = s.get("b").unwrap_err().to_string();
        assert!(!msg.contains("topsecret"));
    }
}
