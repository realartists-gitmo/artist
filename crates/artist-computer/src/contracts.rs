use artist_tool_api::{ArtistToolAnnotations as A, ToolCategory as C};

artist_tool_api::impl_tool_output_contract!(
    crate::ComputerTool,
    C::Computer,
    A {
        read_only: false,
        destructive: true,
        idempotent: false,
        open_world: true
    },
    "Computer surface state, observations, extracted values, or action results."
);
