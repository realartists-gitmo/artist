//! Agent profiles.
//!
//! A profile is one object with three instantiation modes: the session root, a
//! delegated subagent, and a handoff target. It carries its own routing
//! (provider account, model, thinking configuration) directly — there is no
//! indirection layer between a profile and the model it runs on.
//!
//! Profiles are markdown files with YAML frontmatter; the body is the system
//! prompt. Discovery layers `$ARTIST_CONFIG_DIR/profiles/*.md` under
//! `.artist/profiles/*.md`, and both layer over the built-ins.

use globset::{Glob, GlobMatcher};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};
use tokio::sync::Semaphore;

use dashmap::DashMap;

/// Whether the model reasons before answering. Providers spell this
/// differently; the profile records intent and the request builder translates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingMode {
    #[default]
    On,
    Off,
}

/// How hard the model reasons. Normalized across providers: Anthropic maps
/// these onto `output_config.effort`, OpenAI-shaped providers onto
/// `reasoning.effort`. A provider without an equivalent ignores the level.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingLevel {
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl ThinkingLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }
}

/// Merged as a unit, never per-subfield: a child that inherits `mode` while
/// setting `level` can silently produce a combination that is invalid on the
/// resolved provider and visible in neither file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Thinking {
    #[serde(default)]
    pub mode: ThinkingMode,
    #[serde(default)]
    pub level: Option<ThinkingLevel>,
}

/// One routing target. `provider` names a saved provider account by id or
/// display name; `None` inherits the session's account.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Candidate {
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub thinking: Option<Thinking>,
}

impl Candidate {
    fn is_empty(&self) -> bool {
        self.provider.is_none() && self.model.is_none() && self.thinking.is_none()
    }

    /// Resolve this candidate to a concrete account. A candidate that names no
    /// provider inherits `parent` — that is what keeps an unconfigured profile
    /// running wherever the session already runs, while a named one can route
    /// to an entirely different account and provider.
    pub fn resolve<'a>(
        &self,
        set: &'a llm_provider::ProviderSet,
        parent: &'a llm_provider::SavedProvider,
    ) -> Result<&'a llm_provider::SavedProvider, String> {
        match self.provider.as_deref() {
            Some(reference) => set.resolve(reference),
            None => Ok(parent),
        }
    }

    /// The model to run on, preferring the candidate's own over whatever the
    /// resolved account defaults to.
    pub fn model_for(&self, resolved: &llm_provider::SavedProvider) -> Option<String> {
        self.model.clone().or_else(|| resolved.model.clone())
    }
}

/// A resolved, ready-to-instantiate profile.
#[derive(Clone, Debug)]
pub struct Profile {
    pub name: String,
    pub description: String,
    pub instructions: String,
    /// Ordered; index 0 is preferred while healthy. Never empty — a profile
    /// with no declared routing gets one fully-inheriting candidate.
    pub candidates: Vec<Candidate>,
    allow: Option<Vec<GlobMatcher>>,
    deny: Vec<GlobMatcher>,
    pub source: Option<PathBuf>,
}

impl Profile {
    /// Tool policy: `allow` intersects what the caller already has, then `deny`
    /// is subtracted. Deny wins. Patterns are globs over full tool names, so
    /// MCP and extension tools are addressable (`mcp:github/*`).
    ///
    /// This is a bloat and steering control, not a security boundary.
    ///
    /// `handoff` and `todo` are exempt from `allow`. They drive the harness
    /// rather than the world — neither can read, write, or run anything — and
    /// sweeping them up in a capability allow-list silently breaks the
    /// plan-then-hand-off flow profiles exist for. `deny` still removes them.
    pub fn permits(&self, tool: &str) -> bool {
        let allowed = HARNESS_TOOLS.contains(&tool)
            || self
                .allow
                .as_ref()
                .is_none_or(|patterns| patterns.iter().any(|pattern| pattern.is_match(tool)));
        allowed && !self.deny.iter().any(|pattern| pattern.is_match(tool))
    }
}

/// The resolved profile set for a project, plus the delegation concurrency
/// limit and any configuration diagnostics.
#[derive(Clone)]
pub struct Profiles {
    profiles: Arc<BTreeMap<String, Profile>>,
    pub semaphore: Arc<Semaphore>,
    diagnostics: Arc<Vec<String>>,
}

#[derive(Default, Deserialize)]
struct SettingsFile {
    settings: Option<Settings>,
}

#[derive(Default, Deserialize)]
struct Settings {
    max_concurrent: Option<usize>,
}

/// Frontmatter as written. Unknown keys are tolerated, matching the rule-file
/// convention in `artist-rules`.
#[derive(Default, Deserialize)]
struct Frontmatter {
    name: Option<String>,
    description: Option<String>,
    extends: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    thinking: Option<Thinking>,
    candidates: Option<Vec<Candidate>>,
    tools: Option<Tools>,
}

#[derive(Default, Deserialize)]
struct Tools {
    allow: Option<Vec<String>>,
    #[serde(default)]
    deny: Vec<String>,
}

/// A parsed but unresolved definition: inheritance has not been applied.
#[derive(Clone)]
struct Raw {
    name: String,
    description: Option<String>,
    extends: Option<String>,
    candidates: Option<Vec<Candidate>>,
    allow: Option<Vec<String>>,
    deny: Vec<String>,
    instructions: String,
    source: Option<PathBuf>,
}

const DEFAULT_MAX_CONCURRENT: usize = 4;

/// Session-control tools, governed by `deny` but never narrowed away by
/// `allow`. `subagent` is deliberately not here: delegating expands the
/// capability surface, so it stays something a profile opts into.
const HARNESS_TOOLS: [&str; 2] = ["handoff", "todo"];

impl Profiles {
    pub fn discover(project: &Path) -> Self {
        let root = crate::prompt_config::config_root();
        Self::discover_from(project, root.as_deref())
    }

    pub(crate) fn discover_from(project: &Path, global: Option<&Path>) -> Self {
        let mut diagnostics = Vec::new();
        let mut raw: BTreeMap<String, Raw> = builtins()
            .into_iter()
            .map(|profile| (profile.name.clone(), profile))
            .collect();
        let mut max_concurrent = None;

        let project_root = project.join(".artist");
        for root in global
            .into_iter()
            .chain(std::iter::once(project_root.as_path()))
        {
            if let Some(value) = load_settings(root, &mut diagnostics) {
                max_concurrent = Some(value.max(1));
            }
            for (name, definition) in load_dir(&root.join("profiles"), &mut diagnostics) {
                raw.insert(name, definition);
            }
        }

        let mut profiles = BTreeMap::new();
        for name in raw.keys().cloned().collect::<Vec<_>>() {
            match resolve(&name, &raw, &mut BTreeSet::new()) {
                Ok(profile) => {
                    profiles.insert(name, profile);
                }
                Err(error) => diagnostics.push(error),
            }
        }

        Self {
            profiles: Arc::new(profiles),
            semaphore: project_semaphore(project, max_concurrent.unwrap_or(DEFAULT_MAX_CONCURRENT)),
            diagnostics: Arc::new(diagnostics),
        }
    }

    pub fn get(&self, name: &str) -> Result<Profile, String> {
        self.profiles.get(name).cloned().ok_or_else(|| {
            format!(
                "unknown profile: {name}; available profiles: {}",
                self.profiles.keys().cloned().collect::<Vec<_>>().join(", ")
            )
        })
    }

    pub fn names(&self) -> Vec<String> {
        self.profiles.keys().cloned().collect()
    }

    pub fn diagnostics(&self) -> &[String] {
        &self.diagnostics
    }

    /// The catalog injected into a parent agent's prompt so it knows what it
    /// can delegate to and hand off to.
    pub fn catalog(&self) -> String {
        let entries: String = self
            .profiles
            .values()
            .map(|profile| {
                format!(
                    "<profile><name>{}</name><description>{}</description></profile>",
                    escape(&profile.name),
                    escape(&profile.description)
                )
            })
            .collect();
        let diagnostics: String = self
            .diagnostics
            .iter()
            .map(|item| format!("<diagnostic>{}</diagnostic>", escape(item)))
            .collect();
        format!("{entries}{diagnostics}")
    }
}

fn load_settings(root: &Path, diagnostics: &mut Vec<String>) -> Option<usize> {
    let path = root.join("profiles.toml");
    if !path.exists() {
        return None;
    }
    match std::fs::read_to_string(&path)
        .map_err(|error| error.to_string())
        .and_then(|text| toml::from_str::<SettingsFile>(&text).map_err(|error| error.to_string()))
    {
        Ok(file) => file.settings.and_then(|settings| settings.max_concurrent),
        Err(error) => {
            diagnostics.push(format!("{}: invalid settings: {error}", path.display()));
            None
        }
    }
}

fn load_dir(dir: &Path, diagnostics: &mut Vec<String>) -> BTreeMap<String, Raw> {
    let mut found = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };
    let mut paths = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .collect::<Vec<_>>();
    paths.sort();
    for path in paths {
        match parse(&path) {
            Ok(profile) => {
                found.insert(profile.name.clone(), profile);
            }
            Err(error) => diagnostics.push(format!("{}: {error}", path.display())),
        }
    }
    found
}

fn parse(path: &Path) -> Result<Raw, String> {
    let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    let (yaml, body) =
        artist_rules::declarative::frontmatter(&text).map_err(|error| error.to_string())?;
    let front: Frontmatter =
        serde_yaml::from_str(yaml).map_err(|error| format!("invalid frontmatter: {error}"))?;
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .to_owned();
    let name = front.name.unwrap_or(stem);
    if name.is_empty() {
        return Err("profile has no name".into());
    }
    let inline = Candidate {
        provider: front.provider,
        model: front.model,
        thinking: front.thinking,
    };
    let candidates = match (front.candidates, inline.is_empty()) {
        (Some(_), false) => {
            return Err(format!(
                "profile {name} sets both `candidates` and inline provider/model/thinking"
            ));
        }
        (Some(list), true) if list.is_empty() => {
            return Err(format!("profile {name} has an empty `candidates` list"));
        }
        (Some(list), true) => Some(list),
        (None, false) => Some(vec![inline]),
        (None, true) => None,
    };
    let tools = front.tools.unwrap_or_default();
    Ok(Raw {
        name,
        description: front.description,
        extends: front.extends,
        candidates,
        allow: tools.allow,
        deny: tools.deny,
        instructions: body.trim().to_owned(),
        source: Some(path.to_owned()),
    })
}

/// Whole-field merge along the `extends` chain: a field present in the child
/// replaces the parent's entirely, a field absent is inherited.
fn resolve(
    name: &str,
    raw: &BTreeMap<String, Raw>,
    seen: &mut BTreeSet<String>,
) -> Result<Profile, String> {
    if !seen.insert(name.to_owned()) {
        return Err(format!("profile {name} has a circular `extends` chain"));
    }
    let definition = raw
        .get(name)
        .ok_or_else(|| format!("profile {name} extends unknown profile"))?;

    let parent = match &definition.extends {
        Some(parent) => Some(resolve(parent, raw, seen)?),
        None => None,
    };

    let description = definition
        .description
        .clone()
        .or_else(|| parent.as_ref().map(|p| p.description.clone()))
        .ok_or_else(|| format!("profile {name} has no description"))?;

    let candidates = definition
        .candidates
        .clone()
        .or_else(|| parent.as_ref().map(|p| p.candidates.clone()))
        .unwrap_or_else(|| vec![Candidate::default()]);

    let allow = match (&definition.allow, parent.as_ref()) {
        (Some(patterns), _) => Some(compile(patterns, name)?),
        (None, Some(parent)) => parent.allow.clone(),
        (None, None) => None,
    };
    let deny = if definition.deny.is_empty() {
        parent.as_ref().map(|p| p.deny.clone()).unwrap_or_default()
    } else {
        compile(&definition.deny, name)?
    };

    let instructions = if definition.instructions.is_empty() {
        parent
            .as_ref()
            .map(|p| p.instructions.clone())
            .unwrap_or_default()
    } else {
        definition.instructions.clone()
    };

    Ok(Profile {
        name: definition.name.clone(),
        description,
        instructions,
        candidates,
        allow,
        deny,
        source: definition.source.clone(),
    })
}

fn compile(patterns: &[String], profile: &str) -> Result<Vec<GlobMatcher>, String> {
    patterns
        .iter()
        .map(|pattern| {
            Glob::new(pattern)
                .map(|glob| glob.compile_matcher())
                .map_err(|error| format!("profile {profile} has an invalid tool pattern: {error}"))
        })
        .collect()
}

fn project_semaphore(project: &Path, permits: usize) -> Arc<Semaphore> {
    static SEMAPHORES: OnceLock<DashMap<PathBuf, Arc<Semaphore>>> = OnceLock::new();
    SEMAPHORES
        .get_or_init(DashMap::new)
        .entry(project.to_owned())
        .or_insert_with(|| Arc::new(Semaphore::new(permits)))
        .clone()
}

/// The profiles that ship in the binary.
pub fn builtin_names() -> &'static [&'static str] {
    &BUILTIN_NAMES
}

/// Render a built-in as the file that would override it.
///
/// Nothing is scaffolded to disk, so this is how a built-in becomes editable:
/// write it into `.artist/profiles/<name>.md` and change what you want. Returns
/// `None` for a name that is not built in.
pub fn builtin_source(name: &str) -> Option<String> {
    if !BUILTIN_NAMES.contains(&name) {
        return None;
    }
    let (allow, deny) = builtin_tools(name);
    let mut tools = String::new();
    if allow.is_some() || !deny.is_empty() {
        tools.push_str("tools:\n");
        if let Some(allow) = allow {
            tools.push_str(&format!("  allow: [{}]\n", allow.join(", ")));
        }
        if !deny.is_empty() {
            tools.push_str(&format!("  deny: [{}]\n", deny.join(", ")));
        }
    }
    Some(format!(
        "---\ndescription: {}\n{tools}---\n\n{}",
        crate::prompt_config::profile_description(name),
        crate::prompt_config::profile_prompt(name)
    ))
}

pub(crate) const BUILTIN_NAMES: [&str; 5] =
    ["default", "worker", "explorer", "planner", "reviewer"];

/// Built-in profiles, replaced wholesale by a same-named file at either layer.
fn builtins() -> Vec<Raw> {
    BUILTIN_NAMES
        .iter()
        .map(|name| {
            let (allow, deny) = builtin_tools(name);
            Raw {
                name: (*name).to_owned(),
                description: Some(crate::prompt_config::profile_description(name).to_owned()),
                extends: None,
                candidates: None,
                allow,
                deny,
                instructions: crate::prompt_config::profile_prompt(name).trim().to_owned(),
                source: None,
            }
        })
        .collect()
}

fn builtin_tools(name: &str) -> (Option<Vec<String>>, Vec<String>) {
    match name {
        "explorer" | "planner" | "reviewer" => (
            Some(
                ["read", "find", "grep", "skill"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
            ),
            Vec::new(),
        ),
        _ => (None, Vec::new()),
    }
}

#[cfg(test)]
fn catalog_contains(profiles: &Profiles, needle: &str) -> bool {
    profiles.catalog().contains(needle)
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, contents: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    /// The headline workflow is "plan, then hand to a worker". An allow-list
    /// that omits `handoff` and `todo` silently makes that impossible.
    #[test]
    fn read_only_builtins_can_still_hand_off_and_track_work() {
        let dir = tempfile::tempdir().unwrap();
        let profiles = Profiles::discover_from(dir.path(), None);
        for name in ["explorer", "planner", "reviewer"] {
            let profile = profiles.get(name).unwrap();
            assert!(profile.permits("handoff"), "{name} cannot hand off");
            assert!(profile.permits("todo"), "{name} cannot track work");
            assert!(!profile.permits("write"), "{name} must stay read-only");
            assert!(!profile.permits("bash"), "{name} must stay read-only");
            // Delegating expands the capability surface, so it stays opt-in.
            assert!(!profile.permits("subagent"), "{name} should not delegate");
        }
    }

    #[test]
    fn builtins_are_available_without_configuration() {
        let dir = tempfile::tempdir().unwrap();
        let profiles = Profiles::discover_from(dir.path(), None);
        for name in BUILTIN_NAMES {
            assert!(profiles.get(name).is_ok(), "missing built-in {name}");
        }
        assert!(!profiles.get("planner").unwrap().permits("write"));
        assert!(profiles.get("planner").unwrap().permits("read"));
        assert!(profiles.get("worker").unwrap().permits("bash"));
    }

    #[test]
    fn project_replaces_global_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let global = dir.path().join("global");
        write(
            &global.join("profiles/worker.md"),
            "---\ndescription: global\n---\nglobal prompt\n",
        );
        write(
            &dir.path().join(".artist/profiles/worker.md"),
            "---\ndescription: project\n---\nproject prompt\n",
        );
        let profiles = Profiles::discover_from(dir.path(), Some(&global));
        let worker = profiles.get("worker").unwrap();
        assert_eq!(worker.description, "project");
        assert_eq!(worker.instructions, "project prompt");
    }

    #[test]
    fn routing_is_fully_differentiated() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join(".artist/profiles/review.md"),
            "---\ndescription: r\nprovider: anthropic-work\nmodel: claude-opus-5\nthinking:\n  mode: on\n  level: xhigh\n---\nprompt\n",
        );
        let profiles = Profiles::discover_from(dir.path(), None);
        let candidate = &profiles.get("review").unwrap().candidates[0];
        assert_eq!(candidate.provider.as_deref(), Some("anthropic-work"));
        assert_eq!(candidate.model.as_deref(), Some("claude-opus-5"));
        assert_eq!(
            candidate.thinking,
            Some(Thinking {
                mode: ThinkingMode::On,
                level: Some(ThinkingLevel::Xhigh)
            })
        );
    }

    #[test]
    fn candidate_lists_are_ordered_and_may_cross_providers() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join(".artist/profiles/resilient.md"),
            "---\ndescription: r\ncandidates:\n  - provider: chatgpt-personal\n    model: gpt-5\n  - provider: anthropic-work\n    model: claude-opus-5\n  - provider: ollama-local\n    model: qwen3-coder\n    thinking: {mode: off}\n---\nprompt\n",
        );
        let profiles = Profiles::discover_from(dir.path(), None);
        let candidates = profiles.get("resilient").unwrap().candidates;
        assert_eq!(candidates.len(), 3);
        assert_eq!(candidates[0].model.as_deref(), Some("gpt-5"));
        assert_eq!(candidates[2].provider.as_deref(), Some("ollama-local"));
        assert_eq!(candidates[2].thinking.unwrap().mode, ThinkingMode::Off);
    }

    #[test]
    fn inheritance_is_whole_field() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join(".artist/profiles/base.md"),
            "---\ndescription: base\nmodel: gpt-5\nthinking:\n  mode: off\n---\nbase prompt\n",
        );
        // The child re-declares thinking; it must not inherit `mode: off`.
        write(
            &dir.path().join(".artist/profiles/child.md"),
            "---\ndescription: child\nextends: base\nthinking:\n  level: high\n---\nchild prompt\n",
        );
        let profiles = Profiles::discover_from(dir.path(), None);
        let child = profiles.get("child").unwrap();
        let thinking = child.candidates[0].thinking.unwrap();
        assert_eq!(
            thinking.mode,
            ThinkingMode::On,
            "mode must not be inherited"
        );
        assert_eq!(thinking.level, Some(ThinkingLevel::High));
        // The child declared its own routing, so it does not inherit the model.
        assert_eq!(child.candidates[0].model, None);
    }

    #[test]
    fn absent_fields_are_inherited() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join(".artist/profiles/base.md"),
            "---\ndescription: base\nmodel: gpt-5\ntools:\n  allow: [read, grep]\n---\nbase prompt\n",
        );
        write(
            &dir.path().join(".artist/profiles/child.md"),
            "---\ndescription: child\nextends: base\n---\nchild prompt\n",
        );
        let profiles = Profiles::discover_from(dir.path(), None);
        let child = profiles.get("child").unwrap();
        assert_eq!(child.candidates[0].model.as_deref(), Some("gpt-5"));
        assert!(child.permits("read"));
        assert!(!child.permits("bash"));
        assert_eq!(child.instructions, "child prompt");
    }

    /// `deny` still reaches the harness tools; only `allow` skips them.
    #[test]
    fn deny_still_removes_a_harness_tool() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join(".artist/profiles/terminal.md"),
            "---\ndescription: t\ntools:\n  deny: [handoff]\n---\nprompt\n",
        );
        let profiles = Profiles::discover_from(dir.path(), None);
        let profile = profiles.get("terminal").unwrap();
        assert!(!profile.permits("handoff"));
        assert!(profile.permits("todo"));
    }

    #[test]
    fn tool_patterns_are_globs_over_full_names() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join(".artist/profiles/gh.md"),
            "---\ndescription: gh\ntools:\n  allow: [read, \"mcp:github/*\"]\n  deny: [\"mcp:github/create_*\"]\n---\nprompt\n",
        );
        let profiles = Profiles::discover_from(dir.path(), None);
        let profile = profiles.get("gh").unwrap();
        assert!(profile.permits("read"));
        assert!(profile.permits("mcp:github/list_issues"));
        assert!(!profile.permits("mcp:github/create_issue"), "deny wins");
        assert!(!profile.permits("bash"));
    }

    #[test]
    fn circular_inheritance_is_a_diagnostic() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join(".artist/profiles/a.md"),
            "---\ndescription: a\nextends: b\n---\na\n",
        );
        write(
            &dir.path().join(".artist/profiles/b.md"),
            "---\ndescription: b\nextends: a\n---\nb\n",
        );
        let profiles = Profiles::discover_from(dir.path(), None);
        assert!(profiles.get("a").is_err());
        assert!(profiles.catalog().contains("circular"));
        // A broken definition never takes out the built-ins.
        assert!(profiles.get("default").is_ok());
    }

    #[test]
    fn invalid_definitions_are_isolated() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join(".artist/profiles/nodesc.md"),
            "---\nmodel: gpt-5\n---\nprompt\n",
        );
        write(
            &dir.path().join(".artist/profiles/both.md"),
            "---\ndescription: both\nmodel: gpt-5\ncandidates:\n  - model: gpt-4\n---\nprompt\n",
        );
        write(
            &dir.path().join(".artist/profiles/fine.md"),
            "---\ndescription: fine\n---\nprompt\n",
        );
        let profiles = Profiles::discover_from(dir.path(), None);
        assert!(profiles.get("nodesc").is_err());
        assert!(profiles.get("both").is_err());
        assert!(profiles.get("fine").is_ok());
        assert!(catalog_contains(&profiles, "both `candidates`"));
    }

    fn account(
        id: &str,
        name: &str,
        kind: llm_provider::ProviderKind,
        model: &str,
    ) -> llm_provider::SavedProvider {
        let mut provider = llm_provider::SavedProvider::chatgpt(
            llm_provider::ProviderId::new(id).unwrap(),
            name,
            llm_provider::Auth {
                access_token: llm_provider::Secret::new("access"),
                refresh_token: llm_provider::Secret::new("refresh"),
                account_id: "acct".into(),
                email: None,
                expires_at: None,
            },
        );
        provider.provider = kind;
        provider.model = Some(model.to_owned());
        provider
    }

    /// The point of naming an account rather than a model: a delegated profile
    /// can run on a different provider entirely from the session that spawned
    /// it. Before profiles this was impossible — the dispatch read the parent's
    /// account and only substituted the model string.
    #[test]
    fn a_candidate_resolves_to_an_account_other_than_the_parent() {
        let parent = account(
            "chatgpt",
            "ChatGPT",
            llm_provider::ProviderKind::Chatgpt,
            "gpt-5",
        );
        let set = llm_provider::ProviderSet::new(vec![
            parent.clone(),
            account(
                "anthropic-work",
                "Anthropic Work",
                llm_provider::ProviderKind::Anthropic,
                "claude-opus-5",
            ),
        ]);

        let crossing = Candidate {
            provider: Some("anthropic-work".into()),
            model: None,
            thinking: None,
        };
        let resolved = crossing.resolve(&set, &parent).unwrap();
        assert_eq!(resolved.provider, llm_provider::ProviderKind::Anthropic);
        assert_eq!(
            crossing.model_for(resolved).as_deref(),
            Some("claude-opus-5"),
            "an unnamed model falls back to the resolved account's own"
        );
    }

    #[test]
    fn an_unnamed_provider_inherits_the_parent_account() {
        let parent = account(
            "chatgpt",
            "ChatGPT",
            llm_provider::ProviderKind::Chatgpt,
            "gpt-5",
        );
        let set = llm_provider::ProviderSet::new(vec![parent.clone()]);
        let candidate = Candidate {
            provider: None,
            model: Some("gpt-5-mini".into()),
            thinking: None,
        };
        let resolved = candidate.resolve(&set, &parent).unwrap();
        assert_eq!(resolved.id.as_str(), "chatgpt");
        assert_eq!(
            candidate.model_for(resolved).as_deref(),
            Some("gpt-5-mini"),
            "the candidate's own model wins over the account default"
        );
    }

    #[test]
    fn an_unknown_provider_reference_is_an_error_not_a_silent_fallback() {
        let parent = account(
            "chatgpt",
            "ChatGPT",
            llm_provider::ProviderKind::Chatgpt,
            "gpt-5",
        );
        let set = llm_provider::ProviderSet::new(vec![parent.clone()]);
        let candidate = Candidate {
            provider: Some("not-configured".into()),
            ..Candidate::default()
        };
        assert!(candidate.resolve(&set, &parent).is_err());
    }

    /// An empty body means "the shared prompt is the whole of it", which is
    /// how `default` is expressed.
    #[test]
    fn an_empty_body_is_valid_and_contributes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join(".artist/profiles/plain.md"),
            "---\ndescription: plain\n---\n\n",
        );
        let profiles = Profiles::discover_from(dir.path(), None);
        assert_eq!(profiles.get("plain").unwrap().instructions, "");
    }

    #[test]
    fn max_concurrent_is_read_from_settings() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join(".artist/profiles.toml"),
            "[settings]\nmax_concurrent = 2\n",
        );
        let profiles = Profiles::discover_from(dir.path(), None);
        assert_eq!(profiles.semaphore.available_permits(), 2);
    }

    #[test]
    fn a_profile_without_routing_gets_one_inheriting_candidate() {
        let dir = tempfile::tempdir().unwrap();
        let profiles = Profiles::discover_from(dir.path(), None);
        let default = profiles.get("default").unwrap();
        assert_eq!(default.candidates.len(), 1);
        assert!(default.candidates[0].is_empty());
    }
}
