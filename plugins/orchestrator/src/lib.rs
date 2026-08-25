//! Orchestration plugin: creates, drives, and awaits related sessions
//! entirely through the host-sessions service.

use artist_plugin_sdk::artist::plugin::types::ToolEffect;
use artist_plugin_sdk::schemars::JsonSchema;
use artist_plugin_sdk::serde::Deserialize;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SpawnArgs {
    /// Profile for the child session.
    profile: String,
    /// Attached children are cancelled with the parent; detached survive it.
    attached: bool,
    /// `remain-interrupted`, `resume-queued-work`, or `plugin-resolved`.
    recovery: String,
}

fn invoke(args: SpawnArgs) -> Result<String, String> {
    // Caller-supplied IDs keep retries idempotent; the timestamp gives
    // deterministic uniqueness per spawn.
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let summary = artist_plugin_sdk::spawn_child(artist_plugin_sdk::ChildSpec {
        request_id: format!("orch-{nonce}"),
        session_id: format!("child-{nonce}"),
        profile: args.profile,
        attached: args.attached,
        recovery: args.recovery.clone(),
        relationship: "subtask".into(),
        input: "run the subtask".into(),
    })?;
    Ok(summary)
}

struct OrchestratorTool;

artist_plugin_sdk::tool_component!(
    OrchestratorTool,
    "artist.orchestrator",
    SpawnArgs,
    "spawn-child",
    "Spawn a related child session through the host service and await its outcome.",
    vec![ToolEffect::SessionControl],
    invoke
);
