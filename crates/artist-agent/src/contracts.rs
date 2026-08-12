use artist_tool_api::{ArtistToolAnnotations as A, ToolCategory as C};

artist_tool_api::impl_tool_output_contract!(
    crate::virtual_read::VirtualReadTool,
    C::Files,
    A::read_only(),
    "File content or typed virtual-resource snapshots selected by the read request."
);
impl artist_tool_api::ArtistToolContract for artist_tools::ReadManyTool {
    fn category(&self) -> C {
        C::Files
    }
    fn annotations(&self) -> A {
        A::read_only()
    }
    fn output_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type":"object",
            "required":["reads","snapshot"],
            "properties": {
                "snapshot":{"const":"artist-coordinator"},
                "reads":{"type":"array","items":{"type":"object"}}
            }
        })
    }
    fn structured_output(
        &self,
        output: &<Self as rig_core::tool::PortableTool>::Output,
    ) -> Result<serde_json::Value, rig_core::tool::ToolExecutionError> {
        Ok(output.clone())
    }
}
artist_tool_api::impl_text_tool_contract!(
    crate::virtual_read::VirtualFindTool,
    C::Files,
    A::read_only(),
    "Ranked real-file or typed virtual-resource paths selected by the find request."
);
artist_tool_api::impl_text_tool_contract!(
    crate::virtual_read::VirtualGrepTool,
    C::Files,
    A::read_only(),
    "Text matches from real files or canonical typed virtual-resource snapshots."
);

artist_tool_api::impl_text_tool_contract!(
    crate::ask_tool::AskTool,
    C::UserInteraction,
    A {
        read_only: false,
        destructive: false,
        idempotent: false,
        open_world: true
    },
    "The spawned canonical ask:// session path."
);
impl artist_tool_api::ArtistToolContract for crate::relationships::RelationshipTool {
    fn category(&self) -> C {
        C::Administration
    }
    fn annotations(&self) -> A {
        A {
            read_only: false,
            destructive: true,
            idempotent: false,
            open_world: false,
        }
    }
    fn output_schema(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","description":"Structured relationship edge mutation or traversal result."})
    }
    fn structured_output(
        &self,
        output: &<Self as rig_core::tool::PortableTool>::Output,
    ) -> Result<serde_json::Value, rig_core::tool::ToolExecutionError> {
        Ok(output.clone())
    }
}
impl artist_tool_api::ArtistToolContract for crate::eval_tool::EvalTool {
    fn category(&self) -> C {
        C::Shell
    }
    fn annotations(&self) -> A {
        A {
            read_only: false,
            destructive: false,
            idempotent: false,
            open_world: true,
        }
    }
    fn output_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "description": "Persistent Python scratchpad result with canonical eval path and ordered policy-mediated callback provenance."
        })
    }
    fn structured_output(
        &self,
        output: &<Self as rig_core::tool::PortableTool>::Output,
    ) -> Result<serde_json::Value, rig_core::tool::ToolExecutionError> {
        Ok(output.clone())
    }
}
impl artist_tool_api::ArtistToolContract for crate::debug_tool::DebugTool {
    fn category(&self) -> C {
        C::Shell
    }

    fn annotations(&self) -> A {
        A {
            read_only: false,
            destructive: false,
            idempotent: false,
            open_world: true,
        }
    }

    fn output_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "description": "Structured DAP result with canonical debug path and resolved adapter provenance."
        })
    }

    fn structured_output(
        &self,
        output: &<Self as rig_core::tool::PortableTool>::Output,
    ) -> Result<serde_json::Value, rig_core::tool::ToolExecutionError> {
        Ok(output.clone())
    }
}
impl artist_tool_api::ArtistToolContract for crate::lsp_tool::LspTool {
    fn category(&self) -> C {
        C::Code
    }

    fn annotations(&self) -> A {
        A::read_only()
    }

    fn output_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "description": "Structured LSP result plus server provenance or shared-client status."
        })
    }

    fn structured_output(
        &self,
        output: &<Self as rig_core::tool::PortableTool>::Output,
    ) -> Result<serde_json::Value, rig_core::tool::ToolExecutionError> {
        Ok(output.clone())
    }
}
impl artist_tool_api::ArtistToolContract for crate::forge_tool::ForgeTool {
    fn category(&self) -> C {
        C::Files
    }
    fn annotations(&self) -> A {
        A::read_only()
    }
    fn output_schema(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","description":"Current structured read-only forge resource or diff."})
    }
    fn structured_output(
        &self,
        output: &<Self as rig_core::tool::PortableTool>::Output,
    ) -> Result<serde_json::Value, rig_core::tool::ToolExecutionError> {
        Ok(output.clone())
    }
}
artist_tool_api::impl_text_tool_contract!(
    crate::canvas::CanvasTool,
    C::Canvas,
    A {
        read_only: false,
        destructive: true,
        idempotent: false,
        open_world: false
    },
    "Canvas search results or the stable canonical canvas:// session path that was opened/cloned."
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
    "The spawned subagent's canonical agent:// session path."
);
artist_tool_api::impl_text_tool_contract!(
    crate::delegate::AgentCreation,
    C::Agents,
    A {
        read_only: false,
        destructive: true,
        idempotent: false,
        open_world: true
    },
    "The retained Artist's canonical agent:// session path."
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
    "The spawned canonical bash:// session path."
);
artist_tool_api::impl_text_tool_contract!(
    crate::run_tool::RunTool,
    C::Shell,
    A {
        read_only: false,
        destructive: false,
        idempotent: false,
        open_world: true
    },
    "The spawned canonical bash://, process://, or canvas:// path for the requested executable."
);
artist_tool_api::impl_text_tool_contract!(
    crate::session_tools::PollTool,
    C::Agents,
    A::read_only(),
    "The current durable session snapshot or ordered snapshots."
);
artist_tool_api::impl_text_tool_contract!(
    crate::session_tools::StopTool,
    C::Agents,
    A {
        read_only: false,
        destructive: true,
        idempotent: true,
        open_world: true
    },
    "Durable graceful-cancellation state for the targeted sessions."
);
artist_tool_api::impl_text_tool_contract!(
    crate::session_tools::DeleteTool,
    C::Agents,
    A {
        read_only: false,
        destructive: true,
        idempotent: false,
        open_world: true
    },
    "Permanent session-resource deletion confirmations or recovery directions."
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
