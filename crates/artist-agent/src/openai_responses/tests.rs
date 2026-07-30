use super::*;
use rig_core::{
    OneOrMany,
    completion::{self, ToolDefinition},
};
use serde_json::json;

#[test]
fn request_serializes_artist_extensions_and_rig_messages_tools() {
    let core = completion::CompletionRequest {
        model: None,
        preamble: Some("be concise".into()),
        chat_history: OneOrMany::one(completion::Message::user("hello")),
        documents: vec![],
        tools: vec![ToolDefinition {
            name: "lookup".into(),
            description: "look up".into(),
            parameters: json!({"type":"object"}),
        }],
        temperature: Some(0.2),
        max_tokens: Some(64),
        tool_choice: None,
        additional_params: None,
        output_schema: None,
        record_telemetry_content: false,
    };
    let mut request = Request::try_from(("gpt-test".into(), core)).unwrap();
    request.store = Some(false);
    request.include = Some(vec!["reasoning.encrypted_content".into()]);
    request.reasoning = Some(Reasoning {
        context: Some(json!({"encrypted_content":"cipher"})),
        ..Default::default()
    });
    request.prompt_cache_key = Some("thread-1".into());
    request.context_management = vec![ContextManagement {
        kind: "compaction".into(),
        extra: serde_json::from_value(json!({"compact_threshold":12000})).unwrap(),
    }];

    let wire = serde_json::to_value(request).unwrap();
    assert_eq!(wire["store"], false);
    assert_eq!(wire["reasoning"]["context"]["encrypted_content"], "cipher");
    assert_eq!(wire["context_management"][0]["type"], "compaction");
    assert_eq!(wire["input"][0]["content"][0]["text"], "hello");
    assert_eq!(wire["tools"][0]["name"], "lookup");
}

#[test]
fn output_items_roundtrip_losslessly_with_phase_compaction_and_unknown() {
    let fixture = json!([
        {"type":"message","id":"m1","role":"assistant","phase":"commentary","content":[{"type":"output_text","text":"hi","annotations":[]}]},
        {"type":"compaction","id":"c1","encrypted_content":"opaque","future_field":{"x":1}},
        {"type":"future_item","id":"z1","payload":[1,2,3]}
    ]);
    let items: Vec<OutputItem> = serde_json::from_value(fixture.clone()).unwrap();
    assert_eq!(items[0].phase(), Some("commentary"));
    assert!(matches!(items[1], OutputItem::Compaction(_)));
    assert!(matches!(items[2], OutputItem::Unknown(_)));
    assert_eq!(serde_json::to_value(items).unwrap(), fixture);
}

#[test]
fn input_fixture_preserves_encrypted_reasoning_and_tool_ids() {
    let fixture = json!({"model":"gpt-test","input":[
        {"type":"reasoning","id":"rs_1","summary":[],"encrypted_content":"cipher"},
        {"type":"function_call","id":"fc_1","call_id":"call_1","name":"lookup","arguments":"{}"},
        {"type":"function_call_output","call_id":"call_1","output":"ok","status":"completed"}
    ],"store":true});
    let request: Request = serde_json::from_value(fixture.clone()).unwrap();
    assert_eq!(serde_json::to_value(request).unwrap(), fixture);
}
