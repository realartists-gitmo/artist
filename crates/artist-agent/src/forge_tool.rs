//! Read-only provider-neutral forge noun space.  The first transport is `gh`;
//! the path grammar is intentionally independent of that transport.

use std::{
    sync::OnceLock,
    time::{SystemTime, UNIX_EPOCH},
};

use dashmap::DashMap;
use rig_core::tool::{PortableTool, ToolExecutionError};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Clone)]
pub(crate) struct ForgeTool;

/// Only immutable SHA-addressed GitHub objects live here. Mutable resources
/// intentionally bypass it on every read, so this is not a freshness cache.
static IMMUTABLE_CACHE: OnceLock<DashMap<String, String>> = OnceLock::new();

struct ForgeRead {
    content: String,
    cache: Value,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ForgeArgs {
    path: String,
}

#[derive(thiserror::Error, Debug)]
pub(crate) enum ForgeError {
    #[error("invalid forge path: {0}")]
    Path(String),
    #[error("forge authentication failed; authenticate the GitHub CLI and retry")]
    Authentication,
    #[error("forge resource was not found: {0}")]
    NotFound(String),
    #[error("forge access is partial or forbidden: {0}")]
    Forbidden(String),
    #[error("failed to run GitHub CLI: {0}")]
    Transport(String),
    #[error("GitHub returned invalid JSON: {0}")]
    InvalidJson(String),
}

impl From<ForgeError> for ToolExecutionError {
    fn from(error: ForgeError) -> Self {
        let code = match error {
            ForgeError::Path(_) => "invalid_forge_path",
            ForgeError::Authentication => "forge_authentication",
            ForgeError::NotFound(_) => "forge_not_found",
            ForgeError::Forbidden(_) => "forge_forbidden",
            ForgeError::Transport(_) => "forge_transport",
            ForgeError::InvalidJson(_) => "forge_invalid_response",
        };
        ToolExecutionError::other(error.to_string()).with_code(code)
    }
}

impl PortableTool for ForgeTool {
    const NAME: &'static str = "forge";
    type Error = ForgeError;
    type Args = ForgeArgs;
    type Output = Value;

    fn description(&self) -> String {
        "Read canonical forge://github/<owner>/<repo>/ resources through the authenticated GitHub CLI. Forge is read-only; mutable provider resources are never served from a stale local cache.".into()
    }

    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false})
    }

    async fn call(&self, args: ForgeArgs) -> Result<Value, ForgeError> {
        let read = read_with_freshness(&args.path).await?;
        if args.path.contains("/diff/") {
            Ok(json!({
                "path": args.path,
                "contentType": "text/x-diff",
                "content": read.content,
                "freshness": read.cache,
            }))
        } else {
            let value: Value = serde_json::from_str(&read.content)
                .map_err(|error| ForgeError::InvalidJson(error.to_string()))?;
            Ok(json!({"path": args.path, "data": value, "freshness": read.cache}))
        }
    }
}

/// Fetch one canonical forge resource without retaining a mutable cache.
/// Both the tool and `read(forge://...)` use this resolver.
pub(crate) async fn read_path(path: &str) -> Result<String, ForgeError> {
    Ok(read_with_freshness(path).await?.content)
}

/// Resolve a forge object and return the cache/freshness semantics that were
/// actually applied. Mutable API nouns always contact the provider; SHA nouns
/// are immutable identities and may be served from the process cache.
async fn read_with_freshness(path: &str) -> Result<ForgeRead, ForgeError> {
    let endpoint = endpoint(path)?;
    let immutable = is_immutable_path(path);
    if immutable && let Some(cached) = IMMUTABLE_CACHE.get_or_init(DashMap::new).get(path) {
        return Ok(ForgeRead {
            content: cached.clone(),
            cache: freshness(path, "immutable-sha", "hit"),
        });
    }
    let mut command = tokio::process::Command::new("gh");
    command.args(["api", &endpoint]);
    // GitHub's default commit media type is JSON. A canonical `diff/` noun is
    // explicitly the textual patch representation the model can inspect.
    if path.contains("/diff/") {
        command.args(["-H", "Accept: application/vnd.github.diff"]);
    }
    let output = command
        .output()
        .await
        .map_err(|error| ForgeError::Transport(error.to_string()))?;
    if !output.status.success() {
        return Err(classify_failure(
            path,
            &String::from_utf8_lossy(&output.stderr),
        ));
    }
    let rendered = if path.contains("/diff/") {
        String::from_utf8(output.stdout).map_err(|error| ForgeError::InvalidJson(error.to_string()))
    } else {
        let value: Value = serde_json::from_slice(&output.stdout)
            .map_err(|error| ForgeError::InvalidJson(error.to_string()))?;
        serde_json::to_string_pretty(&value)
            .map_err(|error| ForgeError::InvalidJson(error.to_string()))
    }?;
    if immutable {
        IMMUTABLE_CACHE
            .get_or_init(DashMap::new)
            .insert(path.to_owned(), rendered.clone());
    }
    Ok(ForgeRead {
        content: rendered,
        cache: freshness(
            path,
            if immutable {
                "immutable-sha"
            } else {
                "provider-current"
            },
            if immutable { "miss" } else { "not-cached" },
        ),
    })
}

fn freshness(path: &str, policy: &str, status: &str) -> Value {
    json!({
        "policy": policy,
        "cacheStatus": status,
        // A SHA-bearing noun is itself the immutable version. Mutable nouns
        // deliberately expose no synthetic version or stale validator.
        "version": is_immutable_path(path).then(|| path.rsplit('/').next().unwrap_or_default()),
        "etag": Value::Null,
        "checkedAtMs": SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64,
    })
}

fn is_immutable_path(path: &str) -> bool {
    let Ok(artist_tools::resource_path::ResourcePath::Virtual { scheme, segments }) =
        artist_tools::resource_path::ResourcePath::parse(path)
    else {
        return false;
    };
    scheme == artist_tools::resource_path::ResourceScheme::Forge
        && segments.len() == 5
        && segments[0] == "github"
        && matches!(segments[3].as_str(), "commits" | "blobs")
        && is_git_object_id(&segments[4])
}

fn is_git_object_id(value: &str) -> bool {
    (7..=64).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Map stable Artist forge paths to GitHub's read API.  No path contains a
/// pagination cursor: each object retains its provider identity regardless of
/// list ordering/pagination.
pub(crate) fn endpoint(path: &str) -> Result<String, ForgeError> {
    let parsed = artist_tools::resource_path::ResourcePath::parse(path)
        .map_err(|error| ForgeError::Path(error.to_string()))?;
    let artist_tools::resource_path::ResourcePath::Virtual { scheme, segments } = parsed else {
        return Err(ForgeError::Path("a forge:// path is required".into()));
    };
    if scheme != artist_tools::resource_path::ResourceScheme::Forge || segments.len() < 3 {
        return Err(ForgeError::Path("use forge://github/<owner>/<repo>[/commits/<sha>|/blobs/<sha>|/trees/<sha>|/pulls/<number>|/issues/<number>|/comments/<id>|/checks/<ref>|/contents/<path>]".into()));
    }
    if segments[0] != "github" {
        return Err(ForgeError::Path(format!(
            "provider {} is not installed; GitHub is currently available",
            segments[0]
        )));
    }
    let base = format!("repos/{}/{}", segments[1], segments[2]);
    let tail = &segments[3..];
    if tail.is_empty() {
        return Ok(base);
    }
    let endpoint_tail = match tail {
        [kind, reference] if kind == "checks" => format!("commits/{reference}/check-runs"),
        [kind, value, ..]
            if matches!(
                kind.as_str(),
                "commits"
                    | "blobs"
                    | "trees"
                    | "pulls"
                    | "issues"
                    | "comments"
                    | "contents"
                    | "checks"
            ) && !value.is_empty() =>
        {
            tail.join("/")
        }
        [kind, target] if kind == "diff" => format!("commits/{target}"),
        _ => {
            return Err(ForgeError::Path(
                "unsupported forge resource; use an explicit repository object path".into(),
            ));
        }
    };
    Ok(format!("{base}/{endpoint_tail}"))
}

fn classify_failure(path: &str, stderr: &str) -> ForgeError {
    let message = stderr.trim();
    if message.contains("401") || message.to_ascii_lowercase().contains("authentication") {
        ForgeError::Authentication
    } else if message.contains("404") {
        ForgeError::NotFound(path.to_owned())
    } else if message.contains("403") || message.to_ascii_lowercase().contains("forbidden") {
        ForgeError::Forbidden(message.to_owned())
    } else {
        ForgeError::Transport(message.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn maps_canonical_paths() {
        assert_eq!(
            endpoint("forge://github/a/r/pulls/7").unwrap(),
            "repos/a/r/pulls/7"
        );
        assert_eq!(
            endpoint("forge://github/a/r/trees/abc").unwrap(),
            "repos/a/r/trees/abc"
        );
        assert_eq!(
            endpoint("forge://github/a/r/diff/abc").unwrap(),
            "repos/a/r/commits/abc"
        );
        assert_eq!(
            endpoint("forge://github/a/r/checks/abc").unwrap(),
            "repos/a/r/commits/abc/check-runs"
        );
        assert!(endpoint("forge://gitlab/a/r").is_err());
    }

    #[test]
    fn only_sha_addressed_objects_are_immutable() {
        assert!(is_immutable_path(
            "forge://github/a/r/commits/0123456789abcdef"
        ));
        assert!(is_immutable_path(
            "forge://github/a/r/blobs/0123456789abcdef"
        ));
        assert!(!is_immutable_path("forge://github/a/r/commits/main"));
        assert!(!is_immutable_path("forge://github/a/r/pulls/7"));
    }

    #[test]
    fn freshness_is_explicit_and_never_claims_a_mutable_cache_validator() {
        let immutable = freshness(
            "forge://github/a/r/commits/0123456789abcdef",
            "immutable-sha",
            "hit",
        );
        assert_eq!(immutable["policy"], "immutable-sha");
        assert_eq!(immutable["cacheStatus"], "hit");
        assert_eq!(immutable["version"], "0123456789abcdef");
        assert!(immutable["etag"].is_null());

        let mutable = freshness(
            "forge://github/a/r/pulls/7",
            "provider-current",
            "not-cached",
        );
        assert_eq!(mutable["policy"], "provider-current");
        assert_eq!(mutable["cacheStatus"], "not-cached");
        assert!(mutable["version"].is_null());
        assert!(mutable["etag"].is_null());
    }
}
