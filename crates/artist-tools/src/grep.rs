use crate::{ToolError, Workspace, output};
use fff_search::{GrepMode, GrepSearchOptions, QueryParser};
use globset::Glob;
use rig_core::tool::Tool;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::Path;

#[derive(Clone)]
pub struct GrepTool(pub Workspace);
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrepArgs {
    query: String,
    path: Option<String>,
    glob: Option<String>,
    #[serde(rename = "match")]
    match_mode: Option<String>,
    case: Option<String>,
    context: Option<usize>,
    limit: Option<usize>,
}
impl Tool for GrepTool {
    const NAME: &'static str = "grep";
    type Error = ToolError;
    type Args = GrepArgs;
    type Output = String;
    fn description(&self) -> String {
        "FFF-backed ranked content search over project-relative or absolute paths. Use read before editing matched files."
            .into()
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"query":{"type":"string"},"path":{"type":"string","description":"Optional project-relative or absolute search scope."},"glob":{"type":"string"},"match":{"enum":["smart","literal","regex"]},"case":{"enum":["smart","sensitive","insensitive"]},"context":{"type":"integer","minimum":0,"maximum":5},"limit":{"type":"integer","minimum":1,"maximum":100}},"required":["query"],"additionalProperties":false})
    }
    async fn call(&self, args: GrepArgs) -> Result<String, ToolError> {
        let limit = args.limit.unwrap_or(20).min(100);
        let context = args.context.unwrap_or(1).min(5);
        let scope = self.0.search_scope(args.path.as_deref(), true).await?;
        let glob = compile_glob(args.glob.as_deref())?;
        let mode = match args.match_mode.as_deref().unwrap_or("smart") {
            "regex" => GrepMode::Regex,
            "literal" => GrepMode::PlainText,
            "smart" => {
                if looks_regex(&args.query) {
                    GrepMode::Regex
                } else {
                    GrepMode::PlainText
                }
            }
            other => return Err(ToolError::Message(format!("invalid match mode: {other}"))),
        };
        if mode == GrepMode::Regex {
            regex::Regex::new(&args.query)
                .map_err(|e| ToolError::Message(format!("invalid regex: {e}")))?;
        }
        let (search_query, smart_case) = match args.case.as_deref().unwrap_or("smart") {
            "smart" => (args.query.clone(), true),
            "insensitive" => (args.query.to_lowercase(), true),
            "sensitive" => (args.query.clone(), false),
            other => return Err(ToolError::Message(format!("invalid case mode: {other}"))),
        };
        let query = QueryParser::default().parse(&search_query);
        let picker = scope
            .index
            .read()
            .map_err(|error| ToolError::Message(error.to_string()))?;
        let picker = picker
            .as_ref()
            .ok_or_else(|| ToolError::Message("FFF index is unavailable".into()))?;
        let result = picker.grep(
            &query,
            &GrepSearchOptions {
                page_limit: 1000,
                max_matches_per_file: 1000,
                mode,
                smart_case,
                before_context: context,
                after_context: context,
                time_budget_ms: 10_000,
                ..Default::default()
            },
        );
        if let Some(error) = &result.regex_fallback_error {
            return Err(ToolError::Message(error.clone()));
        }
        let mut output = Vec::new();
        let mut filtered_matches = 0;
        for found in &result.matches {
            let file = result.files[found.file_index];
            let relative = file.relative_path(picker).replace('\\', "/");
            if !scope.matches(&relative) || !matches_glob(&relative, glob.as_ref()) {
                continue;
            }
            filtered_matches += 1;
            if filtered_matches > limit {
                continue;
            }
            let displayed = scope.display(&relative);
            let before_start = found
                .line_number
                .saturating_sub(found.context_before.len() as u64);
            for (index, line) in found.context_before.iter().enumerate() {
                output.push(format!(
                    "{displayed}-{}- {line}",
                    before_start + index as u64
                ));
            }
            output.push(format!(
                "{displayed}:{}:{}: {}",
                found.line_number,
                found.col + 1,
                found.line_content
            ));
            for (index, line) in found.context_after.iter().enumerate() {
                output.push(format!(
                    "{displayed}-{}- {line}",
                    found.line_number + index as u64 + 1
                ));
            }
        }
        if filtered_matches > limit {
            output.push(format!("[truncated: showing at most {limit} matches]"));
        }

        Ok(if output.is_empty() {
            "No matches found.".into()
        } else {
            output::head(output.join("\n"), output::OUTPUT_CAP)
        })
    }
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
fn matches_glob(path: &str, glob: Option<&globset::GlobMatcher>) -> bool {
    glob.is_none_or(|matcher| matcher.is_match(Path::new(path)))
}
fn looks_regex(query: &str) -> bool {
    query.chars().any(|c| "[](){}.*+?|^$\\".contains(c))
}
