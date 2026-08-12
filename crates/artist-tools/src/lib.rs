mod annotate;
mod bash;
mod code;
mod contracts;
mod drift;
mod edit;
mod find;
mod grep;
mod locate;
mod outline;
// Public because the truncation contract is shared: `artist-computer` cuts an
// adapter's output the same way every other oversized output in the harness is
// cut, rather than inventing a second rule that drifts.
pub mod output;
mod read;
pub mod resource_path;
mod short_id;
pub mod skeleton;
mod workspace;
mod write;

pub use bash::{BashResult, BashStatus, BashTool, WEZTERM_TERM_REVISION};
pub use code::{
    AstQueryTool, AstRewriteTool, CodeCallsTool, CodeCyclesTool, CodeDepsTool, CodeImpactTool,
    CodeImplementsTool, CodeMapTool, CodeShowTool, CodeSurfaceTool, CodeTraceTool,
};
pub use drift::{DRIFT_BUDGET, DriftWatch, report as drift_report};
pub use edit::EditTool;
pub use find::{FindArgs, FindTool};
pub use grep::{GrepArgs, GrepTool};
pub use locate::{Located, Locator};
pub use read::{ReadArgs, ReadManyTool, ReadTool};
pub use short_id::short_id;
pub use workspace::{Workspace, forget_conversation};
pub use write::WriteTool;

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    Anyhow(#[from] anyhow::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Hashline(#[from] hashline_tools::HashlineError),
    #[error("PTY error: {0}")]
    Pty(String),
    #[error("stale_revision: {path}; refresh it with read(path=\"{path}\")")]
    StaleRevision {
        path: String,
        expected_revision: Option<String>,
        actual_revision: Option<String>,
    },
}
impl From<Box<dyn std::error::Error + Send + Sync>> for ToolError {
    fn from(value: Box<dyn std::error::Error + Send + Sync>) -> Self {
        Self::Pty(value.to_string())
    }
}

impl ToolError {
    pub fn stale_revision(
        path: impl Into<String>,
        expected_revision: Option<String>,
        actual_revision: Option<String>,
    ) -> Self {
        Self::StaleRevision {
            path: path.into(),
            expected_revision,
            actual_revision,
        }
    }

    /// Preserve path/revision facts across the portable tool boundary instead
    /// of asking each transport to parse an explanatory string.
    pub fn into_execution_error(self) -> rig_core::tool::ToolExecutionError {
        match self {
            Self::StaleRevision {
                path,
                expected_revision,
                actual_revision,
            } => {
                let message = format!(
                    "{path} changed or its continuation revision is unavailable; refresh it with read(path=\"{path}\")"
                );
                artist_tool_api::structured_failure_error(
                    artist_tool_api::ArtistFailure {
                        code: "stale_revision".into(),
                        message,
                        retryable: false,
                        retry_after_ms: None,
                        field_errors: Vec::new(),
                        partial_data: None,
                        path: Some(path.clone()),
                        expected_revision,
                        actual_revision,
                    },
                    vec![artist_tool_api::NextAction::RetryWith {
                        tool: "read".into(),
                        arguments: serde_json::json!({"path": path}),
                        reason: "Refresh the resource and use its returned revision.".into(),
                    }],
                )
            }
            error => rig_core::tool::ToolExecutionError::from_error(error),
        }
    }
}

#[derive(Clone)]
pub struct ToolBundle {
    pub bash: BashTool,
    pub read: ReadTool,
    pub read_many: ReadManyTool,
    pub find: FindTool,
    pub grep: GrepTool,
    pub edit: EditTool,
    pub write: WriteTool,
    // Structural navigation, backed by artist-ast.
    pub code_map: CodeMapTool,
    pub code_show: CodeShowTool,
    pub code_surface: CodeSurfaceTool,
    pub code_implements: CodeImplementsTool,
    pub code_deps: CodeDepsTool,
    pub code_cycles: CodeCyclesTool,
    pub code_calls: CodeCallsTool,
    pub code_trace: CodeTraceTool,
    pub code_impact: CodeImpactTool,
    pub ast_query: AstQueryTool,
    pub ast_rewrite: AstRewriteTool,
}
impl ToolBundle {
    pub fn new(workspace: Workspace) -> Self {
        Self {
            bash: BashTool::new(workspace.clone()),
            read: ReadTool(workspace.clone()),
            read_many: ReadManyTool(workspace.clone()),
            find: FindTool(workspace.clone()),
            grep: GrepTool(workspace.clone()),
            edit: EditTool(workspace.clone()),
            write: WriteTool(workspace.clone()),
            code_map: CodeMapTool(workspace.clone()),
            code_show: CodeShowTool(workspace.clone()),
            code_surface: CodeSurfaceTool(workspace.clone()),
            code_implements: CodeImplementsTool(workspace.clone()),
            code_deps: CodeDepsTool(workspace.clone()),
            code_cycles: CodeCyclesTool(workspace.clone()),
            code_calls: CodeCallsTool(workspace.clone()),
            code_trace: CodeTraceTool(workspace.clone()),
            code_impact: CodeImpactTool(workspace.clone()),
            ast_query: AstQueryTool(workspace.clone()),
            ast_rewrite: AstRewriteTool(workspace),
        }
    }

    pub fn project_root(&self) -> &std::path::Path {
        self.read.0.root()
    }

    pub fn for_actor(&self, id: &str) -> anyhow::Result<Self> {
        Ok(Self::new(self.read.0.with_actor(id)?))
    }
}
