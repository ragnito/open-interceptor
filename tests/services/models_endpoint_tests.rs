use open_interceptor::services::models::{ModelEntry, ModelsResponse};

#[test]
fn models_response_serializes_anthropic_shape() {
    let resp = ModelsResponse {
        data: vec![ModelEntry {
            entry_type: "model".into(),
            id: "claude-opus-4-7".into(),
            display_name: "Claude Opus 4.7".into(),
            context_window: None,
            max_output_tokens: None,
        }],
        first_id: None,
        last_id: None,
        has_more: false,
    };
    let json = serde_json::to_string(&resp).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["data"][0]["id"], "claude-opus-4-7");
    assert_eq!(parsed["data"][0]["type"], "model");
    assert_eq!(parsed["has_more"], false);
    // max_input_tokens / max_tokens omitted when None
    assert!(parsed["data"][0]["max_input_tokens"].is_null());
    assert!(parsed["data"][0]["max_tokens"].is_null());
}

#[test]
fn model_entry_emits_anthropic_field_names_when_set() {
    let resp = ModelsResponse {
        data: vec![ModelEntry {
            entry_type: "model".into(),
            id: "deepseek-v4-pro".into(),
            display_name: "deepseek-v4-pro".into(),
            context_window: Some(128000),
            max_output_tokens: Some(8192),
        }],
        first_id: None,
        last_id: None,
        has_more: false,
    };
    let json = serde_json::to_string(&resp).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    // Real Anthropic /v1/models uses `max_input_tokens` and `max_tokens`,
    // not `context_window` / `max_output_tokens`. Claude Code reads
    // those exact names — see docs/anthropic-models-schema.
    assert_eq!(parsed["data"][0]["max_input_tokens"], 128000);
    assert_eq!(parsed["data"][0]["max_tokens"], 8192);
    assert!(parsed["data"][0]["context_window"].is_null());
    assert!(parsed["data"][0]["max_output_tokens"].is_null());
}
