//! The API keys (N8, #32): kept in the OS credential store, never in a file (AGENTS.md, D-029).
//! One key per service kind (#100, D-036), so switching kinds never sends one service's key to
//! another. The app only uses the [`Secrets`] trait; tests use [`Memory`] and never touch the
//! real store.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::ai::ApiKind;

/// The Tauri identifier, so the Keychain or Credential Manager entry says whose it is.
pub const SERVICE: &str = "dev.mcclease.queue";
/// The single slot used before #100. It's deleted, never read, the first time a key is used
/// (D-040): it doesn't say which service its key was for.
pub const LEGACY_USER: &str = "api-key";

/// Each service kind's slot. Changing the base URL within a kind keeps its key.
pub fn user(kind: ApiKind) -> &'static str {
    match kind {
        ApiKind::OpenaiCompatible => "api-key-openai-compatible",
        ApiKind::Anthropic => "api-key-anthropic",
    }
}
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
    /// A store entry under this user name: [`user`]'s per-kind slots, [`LEGACY_USER`], or the
    /// hand-run round-trip test's own.
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

/// The API keys, one slot per service kind (#100, D-036).
pub struct Keys {
    inner: Arc<Inner>,
    openai_compatible: KindKey,
    anthropic: KindKey,
}

struct Inner {
    legacy: Box<dyn Secrets>,
    openai_compatible: Box<dyn Secrets>,
    anthropic: Box<dyn Secrets>,
    /// Whether the pre-#100 entry is known to be gone (deleted, or never there).
    legacy_gone: AtomicBool,
}

/// One kind's key, as the rest of the app sees it. Nothing touches the credential store until a
/// key is read, saved or removed, so Local and Off naming never wait on it (Codex's review
/// 20261109-131744-review-19mt).
pub struct KindKey {
    inner: Arc<Inner>,
    kind: ApiKind,
}

impl Keys {
    /// The OS credential store's entries.
    pub fn keyring() -> Self {
        Keys::new(
            Box::new(Keyring::for_user(LEGACY_USER)),
            Box::new(Keyring::for_user(user(ApiKind::OpenaiCompatible))),
            Box::new(Keyring::for_user(user(ApiKind::Anthropic))),
        )
    }

    pub fn new(
        legacy: Box<dyn Secrets>,
        openai_compatible: Box<dyn Secrets>,
        anthropic: Box<dyn Secrets>,
    ) -> Self {
        let inner = Arc::new(Inner {
            legacy,
            openai_compatible,
            anthropic,
            legacy_gone: AtomicBool::new(false),
        });
        let view = |kind| KindKey {
            inner: inner.clone(),
            kind,
        };
        Keys {
            openai_compatible: view(ApiKind::OpenaiCompatible),
            anthropic: view(ApiKind::Anthropic),
            inner,
        }
    }

    /// `kind`'s key, which is always the kind saved in settings. Nothing is read yet.
    pub fn for_kind(&self, kind: ApiKind) -> &dyn Secrets {
        match kind {
            ApiKind::OpenaiCompatible => &self.openai_compatible,
            ApiKind::Anthropic => &self.anthropic,
        }
    }

    /// Whether the pre-#100 entry is known to be gone (tests).
    pub fn legacy_gone(&self) -> bool {
        self.inner.legacy_gone.load(Ordering::SeqCst)
    }
}

impl Inner {
    fn slot(&self, kind: ApiKind) -> &dyn Secrets {
        match kind {
            ApiKind::OpenaiCompatible => self.openai_compatible.as_ref(),
            ApiKind::Anthropic => self.anthropic.as_ref(),
        }
    }

    /// Deletes the pre-#100 entry, best effort and without reading it (D-040). A missing entry
    /// counts as done; a failed delete is tried again at the next key operation and changes
    /// nothing else.
    fn forget_legacy(&self) {
        if !self.legacy_gone.load(Ordering::SeqCst) && self.legacy.remove().is_ok() {
            self.legacy_gone.store(true, Ordering::SeqCst);
        }
    }
}

impl Secrets for KindKey {
    fn get(&self) -> Result<Option<String>, String> {
        self.inner.forget_legacy();
        self.inner.slot(self.kind).get()
    }

    fn set(&self, key: &str) -> Result<(), String> {
        self.inner.forget_legacy();
        self.inner.slot(self.kind).set(key)
    }

    fn remove(&self) -> Result<(), String> {
        self.inner.forget_legacy();
        self.inner.slot(self.kind).remove()
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

    fn keys(legacy: Option<&str>) -> (Keys, [std::sync::Arc<Memory>; 3]) {
        use std::sync::Arc;
        let stores = [
            Arc::new(legacy.map(Memory::with).unwrap_or_default()),
            Arc::new(Memory::default()),
            Arc::new(Memory::default()),
        ];
        let boxed = |m: &Arc<Memory>| -> Box<dyn Secrets> { Box::new(Shared(m.clone())) };
        let keys = Keys::new(boxed(&stores[0]), boxed(&stores[1]), boxed(&stores[2]));
        (keys, stores)
    }

    /// A test store the test can still look into after `Keys` owns it.
    struct Shared(std::sync::Arc<Memory>);
    impl Secrets for Shared {
        fn get(&self) -> Result<Option<String>, String> {
            self.0.get()
        }
        fn set(&self, key: &str) -> Result<(), String> {
            self.0.set(key)
        }
        fn remove(&self) -> Result<(), String> {
            self.0.remove()
        }
    }

    #[test]
    fn each_kind_has_its_own_slot() {
        let (keys, _) = keys(None);
        keys.for_kind(ApiKind::Anthropic).set("sk-ant").unwrap();
        assert_eq!(keys.for_kind(ApiKind::OpenaiCompatible).get(), Ok(None));
        keys.for_kind(ApiKind::OpenaiCompatible)
            .set("sk-oai")
            .unwrap();
        assert_eq!(
            keys.for_kind(ApiKind::Anthropic).get(),
            Ok(Some("sk-ant".into()))
        );
        keys.for_kind(ApiKind::Anthropic).remove().unwrap();
        assert_eq!(
            keys.for_kind(ApiKind::OpenaiCompatible).get(),
            Ok(Some("sk-oai".into()))
        );
        assert_ne!(user(ApiKind::Anthropic), user(ApiKind::OpenaiCompatible));
        assert_ne!(user(ApiKind::Anthropic), LEGACY_USER);
        assert_ne!(user(ApiKind::OpenaiCompatible), LEGACY_USER);
    }

    /// The pre-#100 entry, counting deletes and panicking if its value is ever read (D-040).
    struct Legacy {
        present: AtomicBool,
        deletable: AtomicBool,
        deletes: std::sync::atomic::AtomicUsize,
    }
    impl Legacy {
        fn new(present: bool, deletable: bool) -> std::sync::Arc<Self> {
            std::sync::Arc::new(Legacy {
                present: AtomicBool::new(present),
                deletable: AtomicBool::new(deletable),
                deletes: Default::default(),
            })
        }
    }
    struct LegacyStore(std::sync::Arc<Legacy>);
    impl Secrets for LegacyStore {
        fn get(&self) -> Result<Option<String>, String> {
            panic!("the pre-#100 key was read")
        }
        fn set(&self, _: &str) -> Result<(), String> {
            panic!("the pre-#100 entry was written")
        }
        fn remove(&self) -> Result<(), String> {
            self.0.deletes.fetch_add(1, Ordering::SeqCst);
            if !self.0.deletable.load(Ordering::SeqCst) {
                return Err(UNREACHABLE.to_owned());
            }
            self.0.present.store(false, Ordering::SeqCst);
            Ok(())
        }
    }

    fn launch(
        legacy: &std::sync::Arc<Legacy>,
        openai: &std::sync::Arc<Memory>,
        anthropic: &std::sync::Arc<Memory>,
    ) -> Keys {
        Keys::new(
            Box::new(LegacyStore(legacy.clone())),
            Box::new(Shared(openai.clone())),
            Box::new(Shared(anthropic.clone())),
        )
    }

    #[test]
    fn an_old_key_is_never_carried_over_before_or_after_a_restart() {
        let legacy = Legacy::new(true, true);
        let (openai, anthropic) = Default::default();
        for _ in 0..2 {
            let keys = launch(&legacy, &openai, &anthropic);
            for kind in [ApiKind::OpenaiCompatible, ApiKind::Anthropic] {
                assert_eq!(keys.for_kind(kind).get(), Ok(None), "{kind:?}");
            }
        }
    }

    #[test]
    fn the_first_key_operation_deletes_the_old_entry() {
        for op in ["get", "set", "remove"] {
            let legacy = Legacy::new(true, true);
            let (openai, anthropic) = Default::default();
            let keys = launch(&legacy, &openai, &anthropic);
            let key = keys.for_kind(ApiKind::Anthropic);
            match op {
                "get" => drop(key.get()),
                "set" => key.set("sk-ant").unwrap(),
                _ => key.remove().unwrap(),
            }
            assert!(!legacy.present.load(Ordering::SeqCst), "{op}");
            assert!(keys.legacy_gone());
            // Later operations and a later launch don't try again.
            keys.for_kind(ApiKind::OpenaiCompatible).get().unwrap();
            assert_eq!(legacy.deletes.load(Ordering::SeqCst), 1, "{op}");
            let later = launch(&legacy, &openai, &anthropic);
            later.for_kind(ApiKind::Anthropic).get().unwrap();
            assert!(!legacy.present.load(Ordering::SeqCst));
        }
    }

    #[test]
    fn an_undeletable_old_entry_changes_no_answer_and_is_retried() {
        let legacy = Legacy::new(true, false);
        let (openai, anthropic) = Default::default();
        let keys = launch(&legacy, &openai, &anthropic);
        let key = keys.for_kind(ApiKind::OpenaiCompatible);
        assert_eq!(key.get(), Ok(None));
        assert_eq!(key.set("sk-new"), Ok(()));
        assert_eq!(key.get(), Ok(Some("sk-new".into())));
        assert_eq!(key.remove(), Ok(()));
        assert_eq!(key.get(), Ok(None));
        assert_eq!(keys.for_kind(ApiKind::Anthropic).get(), Ok(None));
        assert_eq!(
            legacy.deletes.load(Ordering::SeqCst),
            6,
            "tried at every operation"
        );
        assert!(!keys.legacy_gone());
        legacy.deletable.store(true, Ordering::SeqCst);
        key.get().unwrap();
        assert!(keys.legacy_gone());
        assert!(!legacy.present.load(Ordering::SeqCst));
    }

    #[test]
    fn handing_out_a_key_touches_no_store() {
        let legacy = Legacy::new(true, true);
        let (openai, anthropic) = Default::default();
        let keys = launch(&legacy, &openai, &anthropic);
        let _ = keys.for_kind(ApiKind::OpenaiCompatible);
        let _ = keys.for_kind(ApiKind::Anthropic);
        assert_eq!(legacy.deletes.load(Ordering::SeqCst), 0);
    }

    /// A store that panics if touched: Local and Off naming must never reach the store.
    struct Untouchable;
    impl Secrets for Untouchable {
        fn get(&self) -> Result<Option<String>, String> {
            panic!("the credential store was read")
        }
        fn set(&self, _: &str) -> Result<(), String> {
            panic!("the credential store was written")
        }
        fn remove(&self) -> Result<(), String> {
            panic!("the credential store was written")
        }
    }

    #[test]
    fn local_and_off_naming_never_touch_the_store() {
        use crate::ai::{self, LocalModel, Provider};
        use crate::settings::Settings;
        let keys = Keys::new(
            Box::new(Untouchable),
            Box::new(Untouchable),
            Box::new(Untouchable),
        );
        for provider in [Provider::Local, Provider::Off] {
            let settings = Settings {
                naming_provider: provider,
                api_base_url: "https://api.example.test/v1".into(),
                api_model: "m".into(),
                ..Settings::default()
            };
            let key = keys.for_kind(settings.api_kind);
            assert!(ai::endpoint(&settings, key, LocalModel::Missing).is_err());
            assert!(
                ai::unavailable(&settings, key, LocalModel::Downloaded).is_none()
                    || provider == Provider::Off
            );
            assert!(!ai::store_unreachable(&settings, key));
        }
    }

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
