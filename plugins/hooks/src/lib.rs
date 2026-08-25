use artist_plugin_sdk::artist::plugin::types::{HookDecision, HookEvent, HookPhase};
use artist_plugin_sdk::serde_json::{Value, json};

struct Hooks;

fn observe(event: HookEvent) -> Result<HookDecision, String> {
    if event.phase != HookPhase::BeforeModelRequest {
        return Ok(HookDecision::Proceed);
    }
    let payload: Value = artist_plugin_sdk::serde_json::from_str(&event.payload)
        .map_err(|error| format!("invalid hook payload: {error}"))?;
    let context = payload
        .get("context")
        .and_then(Value::as_str)
        .ok_or_else(|| "before-model-request payload has no context".to_owned())?;
    Ok(HookDecision::Rewrite(
        json!({"context": format!("{context}\n\n[hooks-plugin]")}).to_string(),
    ))
}

artist_plugin_sdk::hooks_component!(Hooks, "artist.hooks", 0, observe);
