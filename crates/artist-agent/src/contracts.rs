use artist_tool_api::{ArtistToolAnnotations as A, ToolCategory as C};

artist_tool_api::impl_text_tool_contract!(
    crate::ask_tool::AskTool,
    C::UserInteraction,
    A {
        read_only: false,
        destructive: false,
        idempotent: false,
        open_world: true
    },
    "The spawned ask session id."
);
artist_tool_api::impl_text_tool_contract!(
    crate::canvas::CanvasTool,
    C::Canvas,
    A {
        read_only: false,
        destructive: true,
        idempotent: false,
        open_world: false
    },
    "Canvas search results or the stable canvas session id that was opened/cloned."
);
artist_tool_api::impl_text_tool_contract!(
    crate::code_search::CodeSearchTool,
    C::Code,
    A::read_only(),
    "Semantic or lexical code-search hits."
);
artist_tool_api::impl_text_tool_contract!(
    crate::code_search::CodeRelatedTool,
    C::Code,
    A::read_only(),
    "Code objects related to the requested symbol or passage."
);
artist_tool_api::impl_text_tool_contract!(
    crate::delegate::Delegate,
    C::Agents,
    A {
        read_only: false,
        destructive: true,
        idempotent: false,
        open_world: true
    },
    "The spawned subagent's bare artist-name session id."
);
artist_tool_api::impl_text_tool_contract!(
    crate::handoff::HandoffTool,
    C::Agents,
    A {
        read_only: false,
        destructive: false,
        idempotent: false,
        open_world: false
    },
    "The accepted handoff target and continuation state."
);
artist_tool_api::impl_text_tool_contract!(
    crate::memory::MemoryTool,
    C::Memory,
    A {
        read_only: false,
        destructive: true,
        idempotent: false,
        open_world: false
    },
    "Memory retrieval, write, or maintenance results."
);

artist_tool_api::impl_text_tool_contract!(
    crate::resources::SkillTool,
    C::Files,
    A::read_only(),
    "Profile-scoped skill search results or the loaded skill instructions."
);
artist_tool_api::impl_text_tool_contract!(
    crate::todo::TodoTool,
    C::Administration,
    A {
        read_only: false,
        destructive: false,
        idempotent: true,
        open_world: false
    },
    "The durable todo list and completion summary."
);
artist_tool_api::impl_text_tool_contract!(
    crate::bash_tool::BashTool,
    C::Shell,
    A {
        read_only: false,
        destructive: false,
        idempotent: false,
        open_world: true
    },
    "The spawned bash session id."
);
artist_tool_api::impl_text_tool_contract!(
    crate::session_tools::PollTool,
    C::Agents,
    A::read_only(),
    "The current durable session snapshot or ordered snapshots."
);
artist_tool_api::impl_text_tool_contract!(
    crate::session_tools::AbortTool,
    C::Agents,
    A {
        read_only: false,
        destructive: true,
        idempotent: true,
        open_world: true
    },
    "The durable cancellation-request state for the targeted sessions."
);
artist_tool_api::impl_text_tool_contract!(
    crate::session_tools::SendTool,
    C::Agents,
    A {
        read_only: false,
        destructive: false,
        idempotent: false,
        open_world: true
    },
    "Durable delivery acknowledgements for the targeted sessions or artists."
);
artist_tool_api::impl_text_tool_contract!(
    crate::session_tools::ListTool,
    C::Agents,
    A::read_only(),
    "Live session summaries from the durable registry."
);

impl artist_tool_api::ArtistToolContract for crate::computer_tool::ComputerTool {
    fn category(&self) -> C {
        C::Computer
    }

    fn annotations(&self) -> A {
        A {
            read_only: false,
            destructive: true,
            idempotent: false,
            open_world: true,
        }
    }

    fn output_schema(&self) -> serde_json::Value {
        artist_tool_api::text_output_schema(
            <Self as rig_core::tool::PortableTool>::NAME,
            "Computer session identifiers, observations, interaction results, and images.",
        )
    }

    fn structured_output(
        &self,
        output: &<Self as rig_core::tool::PortableTool>::Output,
    ) -> Result<serde_json::Value, rig_core::tool::ToolExecutionError> {
        match output.as_json() {
            Some(serde_json::Value::Object(map)) => Ok(serde_json::Value::Object(map.clone())),
            Some(value) => Ok(serde_json::json!({"value": value})),
            None => Ok(serde_json::json!({"text": output.render()})),
        }
    }
}
