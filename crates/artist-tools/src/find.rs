use crate::{ToolError, Workspace, output};
use fff_search::{FuzzySearchOptions, PaginationArgs, QueryParser};
use globset::Glob;
use rig_core::tool::Tool;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::Path;

#[derive(Clone)]
pub struct FindTool(pub Workspace);
#[derive(Deserialize)]
pub struct FindArgs {
    query: String,
    path: Option<String>,
    glob: Option<String>,
    limit: Option<usize>,
}
impl Tool for FindTool {
    const NAME: &'static str = "find";
    type Error = ToolError;
    type Args = FindArgs;
    type Output = String;
    fn description(&self) -> String {
        "FFF-backed ranked fuzzy file and path discovery within the project.".into()
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"query":{"type":"string"},"path":{"type":"string"},"glob":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":100}},"required":["query"],"additionalProperties":false})
    }
    async fn call(&self, args: FindArgs) -> Result<String, ToolError> {
        let limit = args.limit.unwrap_or(20).min(100);
        let scope = validate_scope(&self.0, args.path.as_deref())?;
        let glob = compile_glob(args.glob.as_deref())?;
        let query = QueryParser::default().parse(&args.query);
        let picker = self.0.index.lock();
        let result = picker.fuzzy_search(
            &query,
            None,
            FuzzySearchOptions {
                pagination: PaginationArgs {
                    offset: 0,
                    limit: 1000,
                },
                ..Default::default()
            },
        );
        let mut output = Vec::new();
        for (item, score) in result.items.iter().zip(&result.scores) {
            let relative = item.relative_path(&*picker).replace('\\', "/");
            if !matches_filters(&relative, scope.as_deref(), glob.as_ref()) {
                continue;
            }
            output.push(format!("{}\t(score {})", relative, score.total));
            if output.len() == limit {
                break;
            }
        }
        let truncated = result.total_matched > output.len();
        if truncated {
            output.push(format!("[truncated: showing at most {limit} results]"));
        }
        Ok(if output.is_empty() {
            "No files found.".into()
        } else {
            output::head(output.join("\n"), output::OUTPUT_CAP)
        })
    }
}

fn validate_scope(workspace: &Workspace, scope: Option<&str>) -> Result<Option<String>, ToolError> {
    scope
        .map(|path| {
            workspace
                .resolve_existing(path)
                .map(|p| workspace.display(&p))
                .map_err(Into::into)
        })
        .transpose()
}
fn compile_glob(value: Option<&str>) -> Result<Option<globset::GlobMatcher>, ToolError> {
    value
        .map(|pattern| {
            Glob::new(pattern)
                .map(|g| g.compile_matcher())
                .map_err(|e| ToolError::Message(format!("invalid glob: {e}")))
        })
        .transpose()
}
fn matches_filters(path: &str, scope: Option<&str>, glob: Option<&globset::GlobMatcher>) -> bool {
    scope.is_none_or(|s| path == s || path.starts_with(&format!("{s}/")))
        && glob.is_none_or(|g| g.is_match(Path::new(path)))
}
