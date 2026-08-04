use artist_tool_api::{ArtistToolAnnotations as A, ToolCategory as C};

impl artist_tool_api::ArtistToolContract for crate::BashTool {
    fn category(&self) -> C {
        C::Shell
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
        artist_tool_api::schema_for::<crate::bash::BashResult>()
    }

    fn structured_output(
        &self,
        output: &<Self as rig_core::tool::PortableTool>::Output,
    ) -> Result<serde_json::Value, rig_core::tool::ToolExecutionError> {
        serde_json::to_value(crate::bash::BashResult::parse(output))
            .map_err(rig_core::tool::ToolExecutionError::from_error)
    }
}
artist_tool_api::impl_tool_output_contract!(
    crate::ReadTool,
    C::Files,
    A::read_only(),
    "File, directory, or image content selected by the read request."
);
artist_tool_api::impl_text_tool_contract!(
    crate::FindTool,
    C::Files,
    A::read_only(),
    "Paths matching the requested name or glob search."
);
artist_tool_api::impl_text_tool_contract!(
    crate::GrepTool,
    C::Files,
    A::read_only(),
    "Text matches with paths and source locations."
);
artist_tool_api::impl_text_tool_contract!(
    crate::EditTool,
    C::Files,
    A::mutating(),
    "Confirmation and diagnostics for an anchored edit."
);
artist_tool_api::impl_text_tool_contract!(
    crate::WriteTool,
    C::Files,
    A::mutating(),
    "Confirmation and diagnostics for a file write."
);
artist_tool_api::impl_text_tool_contract!(
    crate::CodeMapTool,
    C::Code,
    A::read_only(),
    "A structural outline of the requested source file."
);
artist_tool_api::impl_text_tool_contract!(
    crate::CodeShowTool,
    C::Code,
    A::read_only(),
    "The requested symbol definition with anchored source."
);
artist_tool_api::impl_text_tool_contract!(
    crate::CodeSurfaceTool,
    C::Code,
    A::read_only(),
    "The public code surface and re-export chains."
);
artist_tool_api::impl_text_tool_contract!(
    crate::CodeImplementsTool,
    C::Code,
    A::read_only(),
    "Implementations associated with the requested type or trait."
);
artist_tool_api::impl_text_tool_contract!(
    crate::CodeDepsTool,
    C::Code,
    A::read_only(),
    "Dependency relationships around the requested source file."
);
artist_tool_api::impl_text_tool_contract!(
    crate::CodeCyclesTool,
    C::Code,
    A::read_only(),
    "Import cycles found in the requested scope."
);
artist_tool_api::impl_text_tool_contract!(
    crate::CodeCallsTool,
    C::Code,
    A::read_only(),
    "Callers or callees around the requested code symbol."
);
artist_tool_api::impl_text_tool_contract!(
    crate::CodeTraceTool,
    C::Code,
    A::read_only(),
    "A static path between two code symbols."
);
artist_tool_api::impl_text_tool_contract!(
    crate::CodeImpactTool,
    C::Code,
    A::read_only(),
    "Callers, dependencies, tests, and other code affected by a symbol."
);
artist_tool_api::impl_text_tool_contract!(
    crate::AstQueryTool,
    C::Code,
    A::read_only(),
    "Structural syntax matches for the AST query."
);
artist_tool_api::impl_text_tool_contract!(
    crate::AstRewriteTool,
    C::Code,
    A::mutating(),
    "Preview or application result for a structural rewrite."
);
