//! Source for the default replaceable prompt-composition component.

wit_bindgen::generate!({
    path: "../../../crates/artist-kernel/wasm/composition/wit",
    world: "composition-extension",
});

use artist::composition::types::{CompositionUpdate, Contribution, Error, SessionInput, Snapshot};

struct Component;

impl exports::artist::composition::extension::Guest for Component {
    fn initial(input: SessionInput) -> Result<Snapshot, Error> {
        let mut contributions = Vec::new();
        if let Some(content) = input.system {
            contributions.push(Contribution { id: "system".into(), source: Some("prompt://SYSTEM.md".into()), slot: "system".into(), order: 0, revision: 1, content });
        }
        if let Some(content) = input.agent_instructions {
            contributions.push(Contribution { id: "agent-instructions".into(), source: Some("prompt://AGENTS.md".into()), slot: "agent_instructions".into(), order: 0, revision: 1, content });
        }
        if !input.tool_events.is_empty() {
            contributions.push(Contribution { id: "tools".into(), source: None, slot: "tools".into(), order: 0, revision: 1, content: input.tool_events.join("\n") });
        }
        if let Some(content) = input.profile_content {
            contributions.push(Contribution { id: "profile".into(), source: input.profile.map(|value| format!("profile://{value}/PROFILE.md")), slot: "profile".into(), order: 0, revision: 1, content });
        }
        contributions.push(Contribution { id: "identity".into(), source: None, slot: "identity".into(), order: 0, revision: 1, content: format!("You are {}.", input.identity) });
        Ok(Snapshot { contributions })
    }

    fn update(_input: SessionInput) -> Result<Vec<CompositionUpdate>, Error> { Ok(Vec::new()) }
}

export!(Component);
