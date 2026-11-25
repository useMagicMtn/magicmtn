//! Customer visibility is Registry membership, independent of runtime enrollment.
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::LazyLock;

#[derive(Deserialize)]
struct Catalog {
    providers: Vec<Provider>,
}
#[derive(Deserialize)]
struct Provider {
    name: String,
}
static CATALOG: LazyLock<Catalog> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../registry/providers.json"))
        .expect("valid public Registry")
});
pub fn contains(name: &str) -> bool {
    CATALOG
        .providers
        .iter()
        .any(|provider| provider.name == name)
}
pub fn names() -> Vec<String> {
    CATALOG
        .providers
        .iter()
        .map(|provider| provider.name.clone())
        .collect()
}
pub fn require(name: &str) -> Result<(), (axum::http::StatusCode, String)> {
    if contains(name) {
        Ok(())
    } else {
        Err((
            axum::http::StatusCode::NOT_FOUND,
            "provider not found in the Registry".into(),
        ))
    }
}
pub fn library() -> Value {
    let mut value = crate::library::catalog();
    for key in ["inference_providers", "sandbox_providers"] {
        if let Some(providers) = value[key].as_object_mut() {
            providers.retain(|name, _| contains(name));
        }
    }
    value["sandbox_providers"] = json!(names());
    value
}
pub fn model_fields(id: uuid::Uuid, fields: &mut magicmtn_contracts::models::ModelFields) {
    if fields
        .model_configuration
        .provider
        .as_deref()
        .is_some_and(contains)
    {
        return;
    }
    fields.source = "magicmtn".into();
    fields.source_model_id = id.to_string();
    fields.description = "Model served by MagicMtn.".into();
    fields.connection_ref = None;
    fields.model_configuration = magicmtn_contracts::models::ModelConfiguration {
        model: id.to_string(),
        provider: Some("magicmtn".into()),
        version: None,
        parameters: json!({}),
    };
}
pub fn models(
    mut list: magicmtn_contracts::models::ModelList,
) -> magicmtn_contracts::models::ModelList {
    for model in &mut list.models {
        model_fields(model.id, &mut model.fields);
    }
    list
}
pub fn model_result(
    mut result: magicmtn_contracts::models::ModelResult,
) -> magicmtn_contracts::models::ModelResult {
    model_fields(result.model.id, &mut result.model.fields);
    result
}
/// Project structured runtime snapshots only; never run on user text or runner RPCs.
pub fn snapshot(value: &mut Value) {
    let Some(fields) = value.as_object_mut() else {
        return;
    };
    if let Some(provider) = fields
        .get("model_configuration")
        .and_then(|v| v.get("provider"))
        .and_then(Value::as_str)
    {
        if !contains(provider) {
            for key in ["source", "source_model_id", "connection_ref", "description"] {
                fields.remove(key);
            }
        }
    }
    let private = fields
        .get("provider")
        .or_else(|| fields.get("backend"))
        .and_then(Value::as_str)
        .is_some_and(|name| !contains(name));
    if private {
        for key in ["provider", "backend"] {
            if fields.contains_key(key) {
                fields.insert(key.into(), json!("magicmtn"));
            }
        }
        for key in [
            "binding_ref",
            "connection_ref",
            "configuration",
            "parameters",
            "provider_id",
            "version",
            "provider_version",
            "model",
            "source_model_id",
        ] {
            fields.remove(key);
        }
        if fields.contains_key("backend") && fields.contains_key("display_name") {
            fields.insert("display_name".into(), json!("MagicMtn Cloud"));
        }
    }
    for key in ["model_configuration", "model_snapshot", "compute_snapshot"] {
        if let Some(nested) = fields.get_mut(key) {
            snapshot(nested);
        }
    }
    fields.remove("provider_bindings");
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_registry_members_are_public() {
        assert!(contains("apple-container"));
        assert!(contains("aws"));
        assert!(contains("firecracker"));
        assert!(contains("modal"));
        for name in ["fireworks", "e2b", "future-private-provider"] {
            assert!(!contains(name));
        }
        assert!(crate::library::catalog()["inference_providers"]
            .get("fireworks")
            .is_some());
        assert!(library()["inference_providers"].get("fireworks").is_none());
    }
    #[test]
    fn public_model_preserves_selection_without_disclosing_adapter() {
        let mut fields = crate::library::default_model("fireworks").unwrap();
        let name = fields.display_name.clone();
        let id = uuid::Uuid::new_v4();
        model_fields(id, &mut fields);
        assert_eq!(fields.display_name, name);
        assert_eq!(
            fields.model_configuration.provider.as_deref(),
            Some("magicmtn")
        );
        assert!(!serde_json::to_string(&fields)
            .unwrap()
            .contains("fireworks"));
        fields.validate().unwrap();
    }
}
