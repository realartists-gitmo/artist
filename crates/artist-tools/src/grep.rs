use crate::{ToolError, Workspace, output};
use fff_search::{GrepMode, GrepSearchOptions, parse_grep_query};
use globset::Glob;
use rig_core::tool::PortableTool;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::Path;

#[derive(Clone)]
pub struct GrepTool(pub Workspace);
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrepArgs {
    pub query: String,
    pub path: Option<String>,
    pub glob: Option<String>,
    #[serde(rename = "match")]
    pub match_mode: Option<String>,
    pub case: Option<String>,
    pub context: Option<usize>,
    pub limit: Option<usize>,
}
impl PortableTool for GrepTool {
    const NAME: &'static str = "grep";
    type Error = ToolError;
    type Args = GrepArgs;
    type Output = String;
    fn description(&self) -> String {
        "FFF-backed ranked content search over project-relative or absolute paths. Use read before editing matched files."
            .into()
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"query":{"type":"string"},"path":{"type":"string","description":"Optional project-relative or absolute search scope."},"glob":{"type":"string"},"match":{"enum":["auto","literal","regex","fuzzy"],"default":"auto","description":"auto tries literal matching first and falls back to FFF fuzzy matching only when there is no literal result."},"case":{"enum":["smart","sensitive","insensitive"]},"context":{"type":"integer","minimum":0,"maximum":5},"limit":{"type":"integer","minimum":1,"maximum":100}},"required":["query"],"additionalProperties":false})
    }
    async fn call(&self, args: GrepArgs) -> Result<String, ToolError> {
        let limit = args.limit.unwrap_or(20).min(100);
        let context = args.context.unwrap_or(1).min(5);
        let scope = self.0.search_scope(args.path.as_deref(), true).await?;
        let glob = compile_glob(args.glob.as_deref())?;
        let mode = match args.match_mode.as_deref().unwrap_or("smart") {
            "regex" => GrepMode::Regex,
            "literal" => GrepMode::PlainText,
            "fuzzy" => GrepMode::Fuzzy,
            // `smart` was the pre-path-first spelling. Keep it as an input
            // compatibility alias, but never infer regex syntax: auto must
            // first preserve the caller's bytes as a literal query.
            "auto" | "smart" => GrepMode::PlainText,
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
        // Grep has its own parser configuration. In particular it keeps code
        // punctuation such as `.*` as search text rather than accidentally
        // treating it as a filename/search constraint.
        let query = parse_grep_query(&search_query);
        let picker = scope
            .index
            .read()
            .map_err(|error| ToolError::Message(error.to_string()))?;
        let picker = picker
            .as_ref()
            .ok_or_else(|| ToolError::Message("FFF index is unavailable".into()))?;
        let search = |mode| {
            picker.grep(
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
            )
        };
        let auto = matches!(
            args.match_mode.as_deref().unwrap_or("auto"),
            "auto" | "smart"
        );
        let mut result = search(mode);
        // Auto has a deliberately strict ordering: literal evidence wins, and
        // fuzzy is considered only when no literal hit survives scope and glob
        // filtering. This is not regex guessing.
        if auto
            && !result.matches.iter().any(|found| {
                let file = result.files[found.file_index];
                let relative = file.relative_path(picker).replace('\\', "/");
                scope.matches(&relative) && matches_glob(&relative, glob.as_ref())
            })
        {
            result = search(GrepMode::Fuzzy);
        }
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
