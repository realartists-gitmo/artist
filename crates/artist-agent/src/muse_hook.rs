//! Non-interrupting injection of applicable, source-linked Muse findings.
//!
//! Finding selection is already decided by exact formal applicability in
//! `artist-session`; this hook only renders newly applicable conclusions into
//! the standard request-patch context path.

use artist_session::MuseFormalFinding;
use rig_agent::agent::{
    AgentHook, CompletionCallAction, CompletionCallEvent, HookContext, RequestPatch, StepEventKind,
};
use rig_core::completion::Document;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

const FINDING_LIMIT: usize = 4;
const MESSAGE_LIMIT: usize = 480;

#[derive(Clone)]
pub(crate) struct MuseFindingHook {
    project: PathBuf,
    injected: Arc<Mutex<BTreeSet<String>>>,
}

impl MuseFindingHook {
    pub(crate) fn new(project: PathBuf) -> Self {
        Self {
            project,
            injected: Arc::new(Mutex::new(BTreeSet::new())),
        }
    }

    fn take_new(&self) -> Vec<MuseFormalFinding> {
        let directory = artist_session::muse_findings_dir(&self.project);
        let Ok(entries) = std::fs::read_dir(directory) else {
            return Vec::new();
        };
        let mut paths = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_file()
                    && path
                        .extension()
                        .is_some_and(|extension| extension == "json")
            })
            .collect::<Vec<_>>();
        paths.sort();
        let mut injected = self
            .injected
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut findings = Vec::new();
        for path in paths {
            if findings.len() == FINDING_LIMIT {
                break;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let Ok(finding) = serde_json::from_slice::<MuseFormalFinding>(&bytes) else {
                continue;
            };
            if injected.insert(finding.id.clone()) {
                findings.push(finding);
            }
        }
        findings
    }
}

fn render(findings: &[MuseFormalFinding]) -> String {
    let mut output = String::from("<artist_muse_formal_findings>\n");
    for finding in findings {
        let message = truncate(&finding.message, MESSAGE_LIMIT);
        output.push_str(&format!(
            "- [formal rule {}] {} (source: memory://findings/{})\n",
            finding.rule, message, finding.id
        ));
        for document in &finding.supporting_documents {
            output.push_str(&format!("  evidence: memory://documents/{document}\n"));
        }
    }
    output.push_str("</artist_muse_formal_findings>");
    output
}

fn truncate(value: &str, limit: usize) -> &str {
    if value.len() <= limit {
        return value;
    }
    let mut index = limit;
    while !value.is_char_boundary(index) {
        index -= 1;
    }
    &value[..index]
}

impl AgentHook for MuseFindingHook {
    fn observes(&self, kind: StepEventKind) -> bool {
        kind == StepEventKind::CompletionCall
    }

    async fn on_completion_call(
        &self,
        _context: &HookContext,
        _event: CompletionCallEvent<'_>,
    ) -> CompletionCallAction {
        let findings = self.take_new();
        if findings.is_empty() {
            return CompletionCallAction::continue_run();
        }
        CompletionCallAction::patch(RequestPatch::new().extra_context([Document {
            id: "artist-muse-formal-findings".to_owned(),
            text: render(&findings),
            additional_props: Default::default(),
        }]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use artist_session::MuseFormalFinding;

    #[test]
    fn findings_render_as_source_linked_rule_conclusions() {
        let rendered = render(&[MuseFormalFinding {
            id: "finding-1".into(),
            rule: "rule-1".into(),
            message: "Do not ship this configuration.".into(),
            supporting_documents: BTreeSet::from(["artist-event-document:1".into()]),
        }]);
        assert!(rendered.contains("formal rule rule-1"));
        assert!(rendered.contains("memory://findings/finding-1"));
        assert!(rendered.contains("memory://documents/artist-event-document:1"));
    }
}
