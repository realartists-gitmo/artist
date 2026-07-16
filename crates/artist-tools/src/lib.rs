mod bash;
mod edit;
mod find;
mod grep;
mod output;
mod read;
mod workspace;
mod write;

pub use bash::BashTool;
pub use edit::EditTool;
pub use find::FindTool;
pub use grep::GrepTool;
pub use read::ReadTool;
pub use workspace::Workspace;
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
}
impl ToolBundle {
    pub fn new(workspace: Workspace) -> Self {
        Self {
            bash: BashTool::new(workspace.clone()),
            read: ReadTool(workspace.clone()),
            find: FindTool(workspace.clone()),
            grep: GrepTool(workspace.clone()),
            edit: EditTool(workspace.clone()),
            write: WriteTool(workspace),
        }
    }

    pub fn for_actor(&self, id: &str) -> anyhow::Result<Self> {
        Ok(Self::new(self.read.0.with_actor(id)?))
    }
}

pub const TOOL_POLICY: &str = r#"All paths are project-root-relative. Bash runs as if already cd'd into the project root.
Use find for fuzzy file/path discovery. Use grep for content search. Use read before editing a file.
Read returns mnemonic anchors for each line. Use edit for targeted changes with anchors from the latest read.
Use write only for new files or intentional full-file replacement. Use bash for tests, builds, diagnostics,
package commands, and persistent dev servers. Use delegate for focused subagent investigation; subagents
never receive delegate. Never use line numbers as edit targets. If an edit fails due to stale or unknown
anchors, read the file again and retry."#;
