//! Reference domain plugin: durable notes state through scoped resources plus
//! generic durable events — no `provider-state`, no kernel feature variants.

use artist_plugin_sdk::artist::plugin::types::{InvocationScope, ToolEffect};
use artist_plugin_sdk::schemars::JsonSchema;
use artist_plugin_sdk::serde::Deserialize;
use artist_plugin_sdk::serde_json::{Value, json};

const STATE_SCOPE: &str = "global";
const STATE_KEY: &str = "notes-state";
const SCHEMA_ID: &str = "notes-added-v1";
const EVENT_TYPE: &str = "artist.notes.added";

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NotesArgs {
    /// `add`, `list`, or `remove`.
    op: String,
    /// Note text for `add`; note id for `remove`.
    value: Option<String>,
}

fn load_state() -> Result<Vec<Value>, String> {
    match artist_plugin_sdk::durable_get(STATE_SCOPE, STATE_KEY)? {
        Some(text) => {
            let parsed: Vec<Value> = artist_plugin_sdk::serde_json::from_str(&text)
                .map_err(|error| format!("stored notes are not a list: {error}"))?;
            Ok(parsed)
        }
        None => Ok(Vec::new()),
    }
}

fn save_state(notes: &[Value]) -> Result<(), String> {
    let text = artist_plugin_sdk::serde_json::to_string(notes)
        .map_err(|error| format!("notes are not serializable: {error}"))?;
    artist_plugin_sdk::durable_put(STATE_SCOPE, STATE_KEY, &text)
}

fn register_schema() -> Result<String, String> {
    artist_plugin_sdk::register_event_schema(
        SCHEMA_ID,
        EVENT_TYPE,
        "1",
        json!({
            "type": "object",
            "required": ["id", "text"],
            "properties": {
                "id": {"type": "string"},
                "text": {"type": "string"}
            }
        }),
        json!({"type": "object"}),
        json!({
            "artist.notes.label": "note added",
            "render": {"style": "line"}
        }),
    )
}

/// The host overrides the guest-supplied scope with the active invocation
/// scope, so this placeholder only needs to be well-formed.
fn placeholder_scope() -> InvocationScope {
    InvocationScope {
        session_id: "unknown".into(),
        run_id: None,
        call_id: None,
        correlation_id: "artist.notes".into(),
        parent_correlation_id: None,
    }
}

fn emit_added(id: &str, text: &str, digest: &str) -> Result<(), String> {
    artist_plugin_sdk::emit_event(artist_plugin_sdk::EventEmission {
        schema_id: SCHEMA_ID.into(),
        event_type: EVENT_TYPE.into(),
        schema_version: "1".into(),
        // The registration above returned this canonical digest.
        schema_digest: digest.to_string(),
        scope: placeholder_scope(),
        payload: json!({"id": id, "text": text}),
        presentation: json!({}),
        durable: true,
    })
}

fn invoke(args: NotesArgs) -> Result<String, String> {
    match args.op.as_str() {
        "add" => {
            let text = args
                .value
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| "`add` requires non-empty `value`".to_string())?;
            let mut notes = load_state()?;
            let id = format!("n{}", notes.len() + 1);
            notes.push(json!({"id": id, "text": text}));
            save_state(&notes)?;
            let digest = register_schema()?;
            emit_added(&id, &text, &digest)?;
            Ok(json!({"added": id, "total": notes.len()}).to_string())
        }
        "list" => {
            let notes = load_state()?;
            Ok(json!({"notes": notes, "total": notes.len()}).to_string())
        }
        "remove" => {
            let id = args
                .value
                .ok_or_else(|| "`remove` requires the note id in `value`".to_string())?;
            let mut notes = load_state()?;
            let before = notes.len();
            notes.retain(|note| note.get("id").and_then(Value::as_str) != Some(id.as_str()));
            if notes.len() == before {
                return Err(format!("no note with id `{id}`"));
            }
            save_state(&notes)?;
            Ok(json!({"removed": id, "total": notes.len()}).to_string())
        }
        other => Err(format!(
            "unknown op `{other}`; expected add, list, or remove"
        )),
    }
}

struct NotesTool;

artist_plugin_sdk::tool_component!(
    NotesTool,
    "artist.notes",
    NotesArgs,
    "notes",
    "Persist durable session-relevant notes; emits a generic notes.added event on add.",
    vec![ToolEffect::Mutate],
    invoke
);
