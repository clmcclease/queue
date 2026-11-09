//! Independent N8 / #100 part 3 QA. Fake credential stores only; no real keys or network.
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use queue_lib::ai::{self, ApiKind, LocalModel, Provider, Unavailable};
use queue_lib::secrets::{Keys, Secrets, UNREACHABLE};
use queue_lib::settings::Settings;

#[derive(Default)]
struct Store {
    value: Mutex<Option<String>>,
    fail_reads: AtomicUsize,
    fail_writes: AtomicUsize,
    fail_deletes: AtomicUsize,
}

impl Store {
    fn with(value: &str) -> Arc<Self> {
        Arc::new(Self {
            value: Mutex::new(Some(value.into())),
            ..Self::default()
        })
    }

    fn fail(counter: &AtomicUsize) -> Result<(), String> {
        if counter
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
        {
            Err(UNREACHABLE.into())
        } else {
            Ok(())
        }
    }
}

#[derive(Clone)]
struct Shared(Arc<Store>);

impl Secrets for Shared {
    fn get(&self) -> Result<Option<String>, String> {
        Store::fail(&self.0.fail_reads)?;
        Ok(self.0.value.lock().unwrap().clone())
    }

    fn set(&self, key: &str) -> Result<(), String> {
        Store::fail(&self.0.fail_writes)?;
        *self.0.value.lock().unwrap() = Some(key.into());
        Ok(())
    }

    fn remove(&self) -> Result<(), String> {
        Store::fail(&self.0.fail_deletes)?;
        *self.0.value.lock().unwrap() = None;
        Ok(())
    }
}

fn keys(stores: &[Arc<Store>; 3]) -> Keys {
    Keys::new(
        Box::new(Shared(stores[0].clone())),
        Box::new(Shared(stores[1].clone())),
        Box::new(Shared(stores[2].clone())),
    )
}

fn stores(legacy: Option<&str>) -> [Arc<Store>; 3] {
    [
        legacy.map(Store::with).unwrap_or_default(),
        Arc::default(),
        Arc::default(),
    ]
}

fn settings(kind: ApiKind) -> Settings {
    Settings {
        naming_provider: Provider::Api,
        api_kind: kind,
        api_base_url: "https://service.example.test/v1".into(),
        api_model: "qa-model".into(),
        ..Default::default()
    }
}

// D-040 replaced the pre-#100 key's migration with deleting it unread, so the five migration
// tests from Codex's test run 20261109-124827-test-lZ7l are rewritten to the new rule: the
// isolation assertions stay, and "the first kind gets the old key" becomes "no kind does".
#[test]
fn a_legacy_key_is_never_carried_over_in_either_direction_even_after_restart() {
    for (first, other) in [
        (ApiKind::OpenaiCompatible, ApiKind::Anthropic),
        (ApiKind::Anthropic, ApiKind::OpenaiCompatible),
    ] {
        let stores = stores(Some("qa-old-key"));
        let vault = keys(&stores);
        assert_eq!(vault.for_kind(first).get().unwrap(), None);
        assert_eq!(
            *stores[0].value.lock().unwrap(),
            None,
            "the old entry is deleted at the first key operation"
        );
        drop(vault);
        let restarted = keys(&stores);
        assert_eq!(restarted.for_kind(other).get().unwrap(), None);
        assert_eq!(restarted.for_kind(first).get().unwrap(), None);
    }
}

#[test]
fn saved_kind_selects_its_own_endpoint_key_and_removal_does_not_touch_the_other_kind() {
    let stores = stores(None);
    let vault = keys(&stores);
    vault
        .for_kind(ApiKind::OpenaiCompatible)
        .set("qa-openai")
        .unwrap();
    vault
        .for_kind(ApiKind::Anthropic)
        .set("qa-anthropic")
        .unwrap();
    for (kind, expected) in [
        (ApiKind::OpenaiCompatible, "qa-openai"),
        (ApiKind::Anthropic, "qa-anthropic"),
        (ApiKind::OpenaiCompatible, "qa-openai"),
    ] {
        let endpoint =
            ai::endpoint(&settings(kind), vault.for_kind(kind), LocalModel::Missing).unwrap();
        assert_eq!(endpoint.kind, kind);
        assert_eq!(endpoint.key.as_deref(), Some(expected));
    }
    vault.for_kind(ApiKind::Anthropic).remove().unwrap();
    assert!(matches!(
        ai::endpoint(
            &settings(ApiKind::Anthropic),
            vault.for_kind(ApiKind::Anthropic),
            LocalModel::Missing
        ),
        Err(Unavailable::NotSetUp)
    ));
    assert_eq!(
        vault
            .for_kind(ApiKind::OpenaiCompatible)
            .get()
            .unwrap()
            .as_deref(),
        Some("qa-openai")
    );
}

#[test]
fn a_failed_slot_read_never_exposes_the_legacy_key_after_a_kind_switch() {
    assert_failed_slot_access_never_exposes_the_legacy_key(false);
}

#[test]
fn a_failed_slot_write_never_exposes_the_legacy_key_after_a_kind_switch() {
    assert_failed_slot_access_never_exposes_the_legacy_key(true);
}

fn assert_failed_slot_access_never_exposes_the_legacy_key(fail_write: bool) {
    let stores = stores(Some("qa-openai-legacy"));
    let counter = if fail_write {
        &stores[1].fail_writes
    } else {
        &stores[1].fail_reads
    };
    counter.store(1, Ordering::SeqCst);
    let vault = keys(&stores);
    // The saved kind on upgrade is OpenAI-compatible; a transient credential-store error hits
    // its slot. The user then saves Anthropic as the service.
    let first = if fail_write {
        vault.for_kind(ApiKind::OpenaiCompatible).set("qa-new")
    } else {
        vault.for_kind(ApiKind::OpenaiCompatible).get().map(|_| ())
    };
    assert!(first.is_err());
    let new_service = ai::endpoint(
        &settings(ApiKind::Anthropic),
        vault.for_kind(ApiKind::Anthropic),
        LocalModel::Missing,
    );
    assert!(
        matches!(new_service, Err(Unavailable::NotSetUp)),
        "the OpenAI key must never become an Anthropic request credential (write failure: {fail_write})"
    );
    assert_eq!(
        vault.for_kind(ApiKind::OpenaiCompatible).get().unwrap(),
        None
    );
}

#[test]
fn a_failed_legacy_delete_never_puts_the_key_in_any_slot() {
    let stores = stores(Some("qa-openai-legacy"));
    stores[0].fail_deletes.store(1, Ordering::SeqCst);
    let vault = keys(&stores);
    assert_eq!(
        vault.for_kind(ApiKind::OpenaiCompatible).get().unwrap(),
        None
    );
    assert_eq!(
        stores[0].value.lock().unwrap().as_deref(),
        Some("qa-openai-legacy"),
        "the delete failed once"
    );
    assert_eq!(
        vault.for_kind(ApiKind::Anthropic).get().unwrap(),
        None,
        "never imported into Anthropic"
    );
    assert_eq!(
        *stores[0].value.lock().unwrap(),
        None,
        "retried and deleted"
    );
}

#[test]
fn an_existing_destination_key_survives_and_the_legacy_key_never_comes_back() {
    let stores = stores(Some("qa-old-openai"));
    Shared(stores[1].clone()).set("qa-new-openai").unwrap();
    let vault = keys(&stores);
    assert_eq!(
        vault
            .for_kind(ApiKind::OpenaiCompatible)
            .get()
            .unwrap()
            .as_deref(),
        Some("qa-new-openai")
    );
    assert_eq!(vault.for_kind(ApiKind::Anthropic).get().unwrap(), None);
    drop(vault);
    let restarted = keys(&stores);
    assert_eq!(
        restarted.for_kind(ApiKind::Anthropic).get().unwrap(),
        None,
        "the obsolete OpenAI credential must not be resurrected for Anthropic on restart"
    );
    assert_eq!(
        restarted
            .for_kind(ApiKind::OpenaiCompatible)
            .get()
            .unwrap()
            .as_deref(),
        Some("qa-new-openai")
    );
}

#[test]
fn key_destination_handles_ipv6_unicode_and_refuses_embedded_credentials() {
    for (base, expected) in [
        ("http://[::1]:11434/v1", Some("[::1]:11434")),
        ("http://[::1]:80/v1", Some("[::1]")),
        ("https://例え.テスト/v1", Some("xn--r8jz45g.xn--zckzah")),
        (
            "https://service.test:8443/v1/chat/completions/",
            Some("service.test:8443"),
        ),
        ("https://user:qa-secret@service.test/v1", None),
        ("https://service.test/v1?key=qa-secret", None),
        ("https://service.test/v1#qa-secret", None),
    ] {
        assert_eq!(ai::key_host(base).as_deref(), expected, "{base}");
    }
}
