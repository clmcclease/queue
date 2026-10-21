//! The API key (N8, #32): kept in the OS credential store, never in a file (AGENTS.md, D-029).
//! The app only uses the [`Secrets`] trait; tests use [`Memory`] and never touch the real store.
use std::sync::Mutex;

/// The Tauri identifier, so the Keychain or Credential Manager entry says whose it is.
pub const SERVICE: &str = "dev.mcclease.queue";
/// The one key slot. Changing the base URL keeps it.
pub const USER: &str = "api-key";
/// What a store failure says. It never stops the app (D-029).
pub const UNREACHABLE: &str = "Couldn't reach your system's credential store.";

/// Where the API key lives. A missing key is `Ok(None)`, and removing it isn't an error.
pub trait Secrets: Send + Sync {
    fn get(&self) -> Result<Option<String>, String>;
    fn set(&self, key: &str) -> Result<(), String>;
    fn remove(&self) -> Result<(), String>;
}

/// The OS credential store: macOS Keychain, Windows Credential Manager, Linux Secret Service.
/// The key is cached after the first read, so the store isn't asked for every row (and an
/// unsigned dev build on macOS doesn't prompt again and again).
pub struct Keyring {
    user: String,
    cache: Mutex<Option<Option<String>>>,
}

impl Keyring {
    pub fn new() -> Self {
        Self::for_user(USER)
    }

    /// A store entry under another user name (the hand-run round-trip test uses its own).
    pub fn for_user(user: &str) -> Self {
        Keyring {
            user: user.to_owned(),
            cache: Mutex::new(None),
        }
    }

    fn entry(&self) -> Result<keyring::Entry, String> {
        keyring::Entry::new(SERVICE, &self.user).map_err(|_| UNREACHABLE.to_owned())
    }
}

impl Default for Keyring {
    fn default() -> Self {
        Self::new()
    }
}

impl Secrets for Keyring {
    fn get(&self) -> Result<Option<String>, String> {
        let mut cache = self.cache.lock().map_err(|_| UNREACHABLE.to_owned())?;
        if let Some(key) = &*cache {
            return Ok(key.clone());
        }
        let key = match self.entry()?.get_password() {
            Ok(key) => Some(key),
            Err(keyring::Error::NoEntry) => None,
            Err(_) => return Err(UNREACHABLE.to_owned()),
        };
        *cache = Some(key.clone());
        Ok(key)
    }

    fn set(&self, key: &str) -> Result<(), String> {
        let mut cache = self.cache.lock().map_err(|_| UNREACHABLE.to_owned())?;
        self.entry()?
            .set_password(key)
            .map_err(|_| UNREACHABLE.to_owned())?;
        *cache = Some(Some(key.to_owned()));
        Ok(())
    }

    fn remove(&self) -> Result<(), String> {
        let mut cache = self.cache.lock().map_err(|_| UNREACHABLE.to_owned())?;
        match self.entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(_) => return Err(UNREACHABLE.to_owned()),
        }
        *cache = Some(None);
        Ok(())
    }
}

/// An in-memory store for tests. `broken` makes every call fail like an unreachable store.
#[derive(Default)]
pub struct Memory {
    pub key: Mutex<Option<String>>,
    pub broken: bool,
}

impl Memory {
    pub fn with(key: &str) -> Self {
        Memory {
            key: Mutex::new(Some(key.to_owned())),
            broken: false,
        }
    }

    pub fn broken() -> Self {
        Memory {
            broken: true,
            ..Default::default()
        }
    }

    fn check(&self) -> Result<(), String> {
        if self.broken {
            Err(UNREACHABLE.to_owned())
        } else {
            Ok(())
        }
    }
}

impl Secrets for Memory {
    fn get(&self) -> Result<Option<String>, String> {
        self.check()?;
        Ok(self.key.lock().unwrap().clone())
    }

    fn set(&self, key: &str) -> Result<(), String> {
        self.check()?;
        *self.key.lock().unwrap() = Some(key.to_owned());
        Ok(())
    }

    fn remove(&self) -> Result<(), String> {
        self.check()?;
        *self.key.lock().unwrap() = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_memory_store_sets_reads_and_removes_one_key() {
        let store = Memory::default();
        assert_eq!(store.get(), Ok(None));
        store.set("sk-1").unwrap();
        store.set("sk-2").unwrap();
        assert_eq!(store.get(), Ok(Some("sk-2".into())), "one slot");
        store.remove().unwrap();
        store.remove().unwrap();
        assert_eq!(store.get(), Ok(None), "removing a missing key is fine");
        let broken = Memory::broken();
        assert_eq!(broken.get(), Err(UNREACHABLE.into()));
        assert_eq!(broken.set("k"), Err(UNREACHABLE.into()));
    }

    /// The real store, run by hand (D-029): `QUEUE_QA_REAL_KEYRING=1 cargo test --lib
    /// real_keyring -- --ignored`. It uses its own entry, never the app's key.
    #[test]
    #[ignore]
    fn real_keyring_round_trip() {
        if std::env::var_os("QUEUE_QA_REAL_KEYRING").is_none() {
            return;
        }
        let store = Keyring::for_user("api-key-test");
        store.remove().unwrap();
        assert_eq!(store.get(), Ok(None));
        store.set("sk-queue-test-ünïcødé").unwrap();
        let fresh = Keyring::for_user("api-key-test");
        assert_eq!(
            fresh.get(),
            Ok(Some("sk-queue-test-ünïcødé".into())),
            "read back from the store, not the cache"
        );
        fresh.remove().unwrap();
        assert_eq!(Keyring::for_user("api-key-test").get(), Ok(None));
    }
}
