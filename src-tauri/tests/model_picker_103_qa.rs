//! Independent #103 / N7 QA: hardware boundaries and compatibility of saved model choices.
use queue_lib::{ai::Provider, models, settings::Settings};
use serde_json::json;

#[test]
fn recommendations_cover_zero_unknown_extreme_and_every_threshold_neighbor() {
    let threshold = 7 * 1024_u64.pow(3) + 512 * 1024_u64.pow(2);
    let mut values = vec![None, Some(0), Some(1), Some(u64::MAX)];
    for offset in 0..=1024 {
        values.push(Some(threshold - offset));
        values.push(Some(threshold + offset));
    }
    for memory in values {
        assert_eq!(models::suggested(memory, false), None, "CPU: {memory:?}");
        let expected = match memory {
            Some(bytes) if bytes >= threshold => Some(models::ModelId::Qwen3_4bInstruct2507),
            _ => None,
        };
        assert_eq!(models::suggested(memory, true), expected, "GPU: {memory:?}");
    }
}

#[test]
fn absent_unknown_or_malformed_saved_models_keep_local_as_the_provider() {
    for value in [
        json!({}),
        json!({"localModel": null}),
        json!({"localModel": "qwen3_8b"}),
        json!({"localModel": "qwen3_5_2b\u{200b}"}),
        json!({"localModel": ["qwen3_5_2b"]}),
        json!({"localModel": {"name": "CON"}}),
    ] {
        let settings: Settings = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(settings.local_model, None, "{value}");
        assert_eq!(settings.naming_provider, Provider::Local, "{value}");
        assert_eq!(
            models::ModelId::default(),
            models::ModelId::Qwen3_4bInstruct2507
        );
        let saved = serde_json::to_value(settings).unwrap();
        assert!(
            saved.get("localModel").is_none(),
            "no implicit saved choice: {saved}"
        );
    }
}

#[test]
fn both_saved_model_choices_round_trip_for_all_providers() {
    for (wire, model) in [
        (
            "qwen3_4b_instruct_2507",
            models::ModelId::Qwen3_4bInstruct2507,
        ),
        ("qwen3_5_2b", models::ModelId::Qwen3_5_2b),
    ] {
        for (provider_wire, provider) in [
            ("local", Provider::Local),
            ("api", Provider::Api),
            ("off", Provider::Off),
        ] {
            let settings: Settings = serde_json::from_value(json!({
                "localModel": wire,
                "namingProvider": provider_wire,
                "downloadDir": "C:\\Users\\Zoë 日本語\\Downloads"
            }))
            .unwrap();
            assert_eq!(settings.local_model, Some(model));
            assert_eq!(settings.naming_provider, provider);
            let serialized = serde_json::to_value(&settings).unwrap();
            assert_eq!(serialized["localModel"], wire);
            assert_eq!(serialized["namingProvider"], provider_wire);
            assert_eq!(
                serde_json::from_value::<Settings>(serialized).unwrap(),
                settings
            );
        }
    }
}
