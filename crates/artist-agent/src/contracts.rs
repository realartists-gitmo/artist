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
    "The user's answer to the structured question."
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
    "Canvas lifecycle, rendering, state, or interaction results."
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
    "The delegated agent's completed response or background task handle."
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
artist_tool_api::impl_serialized_tool_contract!(
    crate::message_tools::MessageTools,
    C::Agents,
    A {
        read_only: false,
        destructive: false,
        idempotent: false,
        open_world: true
    }
);
artist_tool_api::impl_serialized_tool_contract!(
    crate::message_tools::QueryTool,
    C::Agents,
    A {
        read_only: false,
        destructive: false,
        idempotent: false,
        open_world: true
    }
);
artist_tool_api::impl_serialized_tool_contract!(
    crate::message_tools::ReplyTool,
    C::Agents,
    A {
        read_only: false,
        destructive: false,
        idempotent: false,
        open_world: true
    }
);
artist_tool_api::impl_serialized_tool_contract!(
    crate::message_tools::GroupTool,
    C::Agents,
    A {
        read_only: false,
        destructive: false,
        idempotent: false,
        open_world: true
    }
);
artist_tool_api::impl_text_tool_contract!(
    crate::resources::SkillTool,
    C::Files,
    A::read_only(),
    "The selected skill instructions and supporting context."
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
