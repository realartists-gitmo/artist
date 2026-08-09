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
mod short_id;
pub mod skeleton;
mod workspace;
mod write;

pub use bash::{BashResult, BashStatus, BashTool};
pub use code::{
    AstQueryTool, AstRewriteTool, CodeCallsTool, CodeCyclesTool, CodeDepsTool, CodeImpactTool,
    CodeImplementsTool, CodeMapTool, CodeShowTool, CodeSurfaceTool, CodeTraceTool,
};
pub use drift::{DRIFT_BUDGET, DriftWatch, report as drift_report};
pub use edit::EditTool;
pub use find::FindTool;
pub use grep::GrepTool;
pub use locate::{Located, Locator};
pub use read::ReadTool;
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
}
impl From<Box<dyn std::error::Error + Send + Sync>> for ToolError {
    fn from(value: Box<dyn std::error::Error + Send + Sync>) -> Self {
        Self::Pty(value.to_string())
    }
}

#[derive(Clone)]
pub struct ToolBundle {
    pub bash: BashTool,
    pub read: ReadTool,
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
