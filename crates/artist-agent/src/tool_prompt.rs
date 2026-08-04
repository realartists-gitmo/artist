//! Erasure and filtering for the tools registered on a run.
//!
//! Tool definitions reach the model through the API's `tools` field, not the
//! system prompt: that is the provider-native channel, it is the single source
//! of truth for name, description, and schema, and it works identically for
//! built-in, MCP, and extension tools. Per-tool usage guidance therefore lives
//! in each tool's own `description`.

use artist_tool_api::{ArtistDynamicTool, ArtistToolContract, ArtistToolOutput};
use futures::FutureExt;
#[cfg(test)]
use rig_core::tool::PortableTool;
use rig_core::tool::ToolExecutionError;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
#[error("the {tool} tool panicked and produced no result: {detail}")]
struct Panicked {
    tool: String,
    detail: String,
}

/// Contain a panicking tool so it fails its own call rather than the session.
///
/// A panic anywhere in a tool used to abort the agent task, ending the turn
/// with the call unanswered — which then poisoned the provider's record of the
/// conversation. Reporting it as a tool error instead lets the model see what
/// broke and carry on, and keeps the call paired with a result.
///
/// The future is asserted unwind-safe: the tool may be left inconsistent, but
/// the alternative is losing the session outright, and the error says plainly
/// that the tool did not complete.
///
/// Also reports files that changed underneath the model, when `drift` is set.
///
/// This wrapper is where it belongs because it is the one thing every tool
/// passes through — built-in, MCP, extension, canvas, delegate — so no tool can
/// be added later that forgets to participate. It also means the check is
/// blind to *how* a file changed: `bash` running `sed`, the user saving in
/// their editor, another session in the same worktree, a formatter, a
/// checkout. All of them stale the model's view identically, and none of them
/// would be caught by instrumenting the harness's own write paths.
pub(crate) fn guard(
    tool: ArtistDynamicTool,
    drift: Option<artist_tools::DriftWatch>,
) -> ArtistDynamicTool {
    let name = tool.name().to_owned();
    let original = Arc::new(tool.clone());
    tool.with_context_callback(move |arguments, context| {
        let tool = Arc::clone(&original);
        let name = name.clone();
        let drift = drift.clone();
        Box::pin(async move {
            let outcome = match AssertUnwindSafe(tool.execute_with_context(arguments, context))
                .catch_unwind()
                .await
            {
                Ok(result) => result,
                Err(panic) => Err(ToolExecutionError::from_error(Panicked {
                    tool: name,
                    detail: panic_detail(&panic),
                })),
            };
            append_drift(outcome, drift).await
        })
    })
}

/// Splice a drift report onto a tool result.
///
/// Only onto a successful one. A failure already sends the model looking at
/// state, and appending a second unrelated report to an error message makes the
/// error harder to read for no gain — the drift will still be there, and
/// reported, on the next call that works.
async fn append_drift(
    outcome: Result<ArtistToolOutput, ToolExecutionError>,
    drift: Option<artist_tools::DriftWatch>,
) -> Result<ArtistToolOutput, ToolExecutionError> {
    let Ok(mut output) = outcome else {
        return outcome;
    };
    let Some(watch) = drift else {
        return Ok(output);
    };
    let Some(report) = watch.report().await else {
        return Ok(output);
    };

    use rig_core::completion::message::ToolResultContent;
    let mut items: Vec<ToolResultContent> =
        output.presentation.into_content().into_iter().collect();
    match items
        .iter()
        .rposition(|item| matches!(item, ToolResultContent::Text(_)))
    {
        Some(index) => {
            if let Some(ToolResultContent::Text(text)) = items.get_mut(index) {
                text.text.push_str(&report);
            }
        }
        None => items.push(ToolResultContent::text(report.clone())),
    }
    output.presentation = match rig_core::OneOrMany::many(items) {
        Ok(content) => rig_core::tool::ToolOutput::content(content),
        Err(_) => rig_core::tool::ToolOutput::text(report),
    };
    Ok(output)
}

fn panic_detail(panic: &Box<dyn std::any::Any + Send>) -> String {
    panic
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| panic.downcast_ref::<&str>().map(|text| (*text).to_owned()))
        .unwrap_or_else(|| "no panic message".to_owned())
}

/// Erase a typed portable tool into Rig's runtime-authored portable contract.
pub(crate) fn dynamic<T>(tool: T) -> ArtistDynamicTool
where
    T: ArtistToolContract + 'static,
{
    artist_tool_api::dynamic(tool)
}

pub(crate) fn retain_enabled(tools: &mut Vec<ArtistDynamicTool>, disabled: &[String]) {
    tools.retain(|tool| !disabled.iter().any(|name| name == tool.name()));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Clone)]
    struct Stub(&'static str);
    #[derive(Debug, thiserror::Error)]
    #[error("stub")]
    struct Error;
    impl PortableTool for Stub {
        const NAME: &'static str = "read";
        type Args = serde_json::Value;
        type Output = String;
        type Error = Error;
        fn description(&self) -> String {
            self.0.into()
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type":"object"})
        }
        async fn call(&self, _: Self::Args) -> Result<String, Error> {
            Ok(String::new())
        }
    }
    artist_tool_api::impl_text_tool_contract!(
        Stub,
        artist_tool_api::ToolCategory::Files,
        artist_tool_api::ArtistToolAnnotations::read_only(),
        "Stub output."
    );

    #[test]
    fn disabled_tools_are_dropped() {
        let mut tools = vec![dynamic(Stub("Inspect files"))];
        assert_eq!(tools.len(), 1);
        retain_enabled(&mut tools, &["read".into()]);
        assert!(tools.is_empty());
    }

    /// A real workspace over a temp project, with the real tools.
    fn workspace() -> (artist_tools::Workspace, tempfile::TempDir) {
        let project = tempfile::tempdir().expect("project dir");
        let state = project.path().join(".state");
        let workspace =
            artist_tools::Workspace::open(project.path(), &state, "test").expect("open workspace");
        (workspace, project)
    }

    fn text_of(output: &artist_tool_api::ArtistToolOutput) -> String {
        output.presentation.render()
    }

    /// The whole path, without a model: read a file so the session holds
    /// anchors for it, change it through `bash` — which knows nothing about
    /// anchors — and confirm the drift lands on that tool's own result.
    ///
    /// The unit tests each prove a layer. This proves they are connected: the
    /// `guard` wrapper, the watch handle, the coordinator's three-phase check,
    /// reconciliation, and the rendering.
    #[tokio::test]
    async fn a_bash_edit_reports_drift_on_its_own_result() {
        let (workspace, project) = workspace();
        let file = project.path().join("subject.txt");
        std::fs::write(&file, "keep me\nchange me\n").expect("seed");
        let path = file.to_string_lossy().into_owned();

        let watch = Some(workspace.drift_watch());
        let read = guard(
            dynamic(artist_tools::ReadTool(workspace.clone())),
            watch.clone(),
        );
        let bash = guard(
            dynamic(artist_tools::BashTool::new(workspace.clone())),
            watch,
        );

        // The model reads the file, so it now holds anchors into it.
        let first = read
            .execute(serde_json::json!({"path": path}))
            .await
            .expect("read");
        let rendered = text_of(&first);
        assert!(rendered.contains("change me"), "{rendered}");
        // Nothing has moved yet, so nothing should be appended.
        assert!(
            !rendered.contains("files changed since you read them"),
            "reported drift on a quiet read: {rendered}"
        );

        // Something outside the harness edits it. `bash` does not participate
        // in the anchor system at all — that is the point.
        let bashed = bash
            .execute(serde_json::json!({
                "command": format!("printf 'keep me\\nCHANGED\\n' > {path}")
            }))
            .await
            .expect("bash");
        let rendered = text_of(&bashed);

        assert!(
            rendered.contains("files changed since you read them"),
            "no drift reported after an out-of-band edit: {rendered}"
        );
        assert!(rendered.contains("subject.txt"), "{rendered}");
        // Both sides, because the removal row carries the anchor the model is
        // still holding.
        assert!(rendered.contains("-change me"), "{rendered}");
        assert!(rendered.contains("+CHANGED"), "{rendered}");
    }

    /// The counterpart, and the one that would make this feature intolerable if
    /// it broke: an ordinary command that touches nothing must append nothing.
    #[tokio::test]
    async fn a_bash_command_that_changes_nothing_appends_nothing() {
        let (workspace, project) = workspace();
        let file = project.path().join("subject.txt");
        std::fs::write(&file, "quiet\n").expect("seed");
        let path = file.to_string_lossy().into_owned();

        let watch = Some(workspace.drift_watch());
        let read = guard(
            dynamic(artist_tools::ReadTool(workspace.clone())),
            watch.clone(),
        );
        let bash = guard(
            dynamic(artist_tools::BashTool::new(workspace.clone())),
            watch,
        );

        read.execute(serde_json::json!({"path": path}))
            .await
            .expect("read");
        let bashed = bash
            .execute(serde_json::json!({"command": "echo hello"}))
            .await
            .expect("bash");

        let rendered = text_of(&bashed);
        assert!(
            !rendered.contains("files changed since you read them"),
            "reported drift for a command that wrote nothing: {rendered}"
        );
    }

    /// The harness's own edits must stay silent, or every write would be
    /// followed by a report of itself.
    #[tokio::test]
    async fn our_own_write_tool_does_not_report_drift() {
        let (workspace, project) = workspace();
        let file = project.path().join("subject.txt");
        std::fs::write(&file, "before\n").expect("seed");
        let path = file.to_string_lossy().into_owned();

        let watch = Some(workspace.drift_watch());
        let read = guard(
            dynamic(artist_tools::ReadTool(workspace.clone())),
            watch.clone(),
        );
        let write = guard(dynamic(artist_tools::WriteTool(workspace.clone())), watch);

        read.execute(serde_json::json!({"path": path}))
            .await
            .expect("read");
        let written = write
            .execute(serde_json::json!({"path": path, "content": "after\n"}))
            .await
            .expect("write");

        let rendered = text_of(&written);
        assert!(
            !rendered.contains("files changed since you read them"),
            "the write tool reported its own write as drift: {rendered}"
        );
    }
}
