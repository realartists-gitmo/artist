//! `read` over the path-first Artist noun space.
//!
//! Real paths retain the proven `artist-tools` implementation. Virtual session
//! roots are resolved through the durable registry, never by guessing from a
//! scheme-shaped string on the host filesystem.

use artist_tools::{
    FindArgs, FindTool, GrepArgs, GrepTool, ReadArgs, ReadTool, ToolError,
    resource_path::{ResourcePath, ResourceScheme},
};
use rig_core::{completion::message::ToolResultContent, tool::PortableTool};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::session_tools::SessionHub;

#[derive(Clone)]
pub(crate) struct VirtualReadTool {
    real: ReadTool,
    sessions: SessionHub,
    todos: crate::todo::TodoStore,
    resources: crate::resources::Resources,
    profiles: crate::profiles::Profiles,
    profile: crate::profiles::Profile,
    tools_root: Option<PathBuf>,
    extension_manager: Option<Arc<artist_extensions::Manager>>,
    pages: crate::pagination::PageStore,
    dictionary: Option<crate::Dictionary>,
    relationships: crate::relationships::RelationshipStore,
}

impl VirtualReadTool {
    pub fn new(
        real: ReadTool,
        sessions: SessionHub,
        todos: crate::todo::TodoStore,
        resources: crate::resources::Resources,
        profiles: crate::profiles::Profiles,
        profile: crate::profiles::Profile,
        pages: crate::pagination::PageStore,
        relationships: crate::relationships::RelationshipStore,
    ) -> Self {
        Self {
            real,
            sessions,
            todos,
            resources,
            profiles,
            profile,
            tools_root: artist_extensions::default_root().ok(),
            extension_manager: None,
            pages,
            dictionary: crate::Dictionary::global().ok(),
            relationships,
        }
    }

    #[cfg(test)]
    fn with_tools_root(mut self, root: PathBuf) -> Self {
        self.tools_root = Some(root);
        self
    }

    pub(crate) fn with_extension_manager(
        mut self,
        extension_manager: Option<Arc<artist_extensions::Manager>>,
    ) -> Self {
        self.extension_manager = extension_manager;
        self
    }

    #[cfg(test)]
    fn with_pages(mut self, pages: crate::pagination::PageStore) -> Self {
        self.pages = pages;
        self
    }

    #[cfg(test)]
    fn with_dictionary(mut self, dictionary: crate::Dictionary) -> Self {
        self.dictionary = Some(dictionary);
        self
    }
}

/// `find` over the same typed noun space as [`VirtualReadTool`].
///
/// Real-path discovery deliberately remains delegated to FFF. Runtime session
/// resources are already a compact durable set, so matching their canonical
/// paths directly is both deterministic and avoids manufacturing a shadow
/// filesystem index whose ranking could diverge from the registry.
#[derive(Clone)]
pub(crate) struct VirtualFindTool {
    real: FindTool,
    sessions: SessionHub,
}

/// Content search for typed session resources. The resource snapshot is the
/// canonical searchable projection; no host-path lookup is attempted.
#[derive(Clone)]
pub(crate) struct VirtualGrepTool {
    real: GrepTool,
    sessions: SessionHub,
}

impl VirtualGrepTool {
    pub fn new(real: GrepTool, sessions: SessionHub) -> Self {
        Self { real, sessions }
    }
}

impl PortableTool for VirtualGrepTool {
    const NAME: &'static str = "grep";
    type Error = ToolError;
    type Args = GrepArgs;
    type Output = String;

    fn description(&self) -> String {
        "Search real text with FFF or canonical snapshots beneath an explicit typed Artist path. Virtual search never reads a host filesystem path and never follows relationship pointers; read relation:// or a resource's /parent, /children, /predecessor, or /successor projection explicitly.".into()
    }

    fn parameters(&self) -> serde_json::Value {
        self.real.parameters()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let Some(path) = args.path.as_deref() else {
            return self.real.call(args).await;
        };
        match ResourcePath::parse(path).map_err(|error| ToolError::Message(error.to_string()))? {
            ResourcePath::Real(_) => self.real.call(args).await,
            ResourcePath::Virtual { scheme, segments } => {
                self.grep_virtual(scheme, &segments, &args)
            }
        }
    }
}

impl VirtualGrepTool {
    fn grep_virtual(
        &self,
        scheme: ResourceScheme,
        segments: &[String],
        args: &GrepArgs,
    ) -> Result<String, ToolError> {
        let Some(kind) = session_kind(scheme) else {
            // A declared namespace with no installed resolver is still a
            // valid path. It simply has no searchable canonical projection in
            // this process; never reinterpret it as a host path.
            if segments.is_empty() {
                return Ok("No matches found.".into());
            }
            return Err(ToolError::Message(format!(
                "{}:// is a typed virtual root but its grep resolver is not installed in this Artist runtime; use a runtime with the {} resolver",
                scheme, scheme
            )));
        };
        if segments.len() > 1 {
            return Err(ToolError::Message(format!(
                "{}://{}/{} has no searchable projection in this runtime; read {}://{} for its canonical snapshot",
                scheme,
                segments[0],
                segments[1..].join("/"),
                scheme,
                segments[0]
            )));
        }
        let mode = args.match_mode.as_deref().unwrap_or("auto");
        if !matches!(mode, "auto" | "smart" | "literal" | "regex" | "fuzzy") {
            return Err(ToolError::Message(format!("invalid match mode: {mode}")));
        }
        let insensitive = match args.case.as_deref().unwrap_or("smart") {
            "smart" => !args.query.chars().any(char::is_uppercase),
            "insensitive" => true,
            "sensitive" => false,
            other => return Err(ToolError::Message(format!("invalid case mode: {other}"))),
        };
        let regex = if mode == "regex" {
            Some(
                regex::RegexBuilder::new(&args.query)
                    .case_insensitive(insensitive)
                    .build()
                    .map_err(|error| ToolError::Message(format!("invalid regex: {error}")))?,
            )
        } else {
            None
        };
        let glob = args
            .glob
            .as_deref()
            .map(globset::Glob::new)
            .transpose()
            .map_err(|error| ToolError::Message(format!("invalid glob: {error}")))?
            .map(|glob| glob.compile_matcher());
        let limit = args.limit.unwrap_or(20).min(100);
        let records = self
            .sessions
            .registry()
            .list()
            .map_err(|error| ToolError::Message(error.to_string()))?;
        let mut literal = Vec::new();
        // Keep a source-ordered candidate list. FFF's fuzzy engine supplies
        // the matching semantics, but grep deliberately leaves matching lines
        // in source order (only file chunks are rankable).
        let mut fuzzy_candidates = Vec::new();
        for record in records
            .into_iter()
            .filter(|record| record_matches_scheme(record, scheme, kind))
        {
            if !segments.is_empty() && resource_segment_for_record(scheme, &record) != segments[0] {
                continue;
            }
            let path = resource_path_for_record(scheme, &record);
            if glob
                .as_ref()
                .is_some_and(|glob| !glob.is_match(Path::new(&path)))
            {
                continue;
            }
            let text = serde_json::to_string_pretty(&serde_json::json!({
                "path": path,
                "kind": record.kind,
                "lifecycle": record.lifecycle,
                "cancelRequested": record.cancel_requested,
                "snapshot": record.snapshot,
            }))
            .map_err(|error| ToolError::Message(error.to_string()))?;
            for (line_index, line) in text.lines().enumerate() {
                let line_number = line_index + 1;
                let literal_match = match &regex {
                    Some(regex) => regex.is_match(line),
                    None => contains_literal(line, &args.query, insensitive),
                };
                if literal_match {
                    literal.push(format!("{path}:{line_number}:1: {line}"));
                }
                if !matches!(mode, "regex" | "literal") {
                    fuzzy_candidates.push(format!("{path}:{line_number}:1: {line}"));
                }
            }
        }
        let fuzzy = if matches!(mode, "auto" | "smart" | "fuzzy") {
            fff_fuzzy_virtual_lines(&args.query, &fuzzy_candidates, insensitive)
        } else {
            Vec::new()
        };
        let mut matches = if matches!(mode, "auto" | "smart") && literal.is_empty() {
            fuzzy
        } else if mode == "fuzzy" {
            fuzzy
        } else {
            literal
        };
        let more = matches.len() > limit;
        matches.truncate(limit);
        if more {
            matches.push(format!("[truncated: showing at most {limit} matches]"));
        }
        Ok(if matches.is_empty() {
            "No matches found.".into()
        } else {
            matches.join("\n")
        })
    }
}

fn contains_literal(line: &str, query: &str, insensitive: bool) -> bool {
    if insensitive {
        line.to_lowercase().contains(&query.to_lowercase())
    } else {
        line.contains(query)
    }
}

impl VirtualFindTool {
    pub fn new(real: FindTool, sessions: SessionHub) -> Self {
        Self { real, sessions }
    }
}

impl PortableTool for VirtualFindTool {
    const NAME: &'static str = "find";
    type Error = ToolError;
    type Args = FindArgs;
    type Output = String;

    fn description(&self) -> String {
        "Ranked discovery over real files or typed Artist resources. Pass a virtual scheme path to search that canonical runtime namespace. Relationship pointers are not followed implicitly; they are explicit one-hop read projections.".into()
    }

    fn parameters(&self) -> serde_json::Value {
        self.real.parameters()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let Some(path) = args.path.as_deref() else {
            return self.real.call(args).await;
        };
        match ResourcePath::parse(path).map_err(|error| ToolError::Message(error.to_string()))? {
            ResourcePath::Real(_) => self.real.call(args).await,
            ResourcePath::Virtual { scheme, segments } => {
                self.find_virtual(scheme, &segments, &args)
            }
        }
    }
}

impl VirtualFindTool {
    fn find_virtual(
        &self,
        scheme: ResourceScheme,
        segments: &[String],
        args: &FindArgs,
    ) -> Result<String, ToolError> {
        let Some(kind) = session_kind(scheme) else {
            if segments.is_empty() {
                return Ok("No resources found.".into());
            }
            return Err(ToolError::Message(format!(
                "{}:// is a typed virtual root but its find resolver is not installed in this Artist runtime; use a runtime with the {} resolver",
                scheme, scheme
            )));
        };
        if segments.len() > 1 {
            return Err(ToolError::Message(format!(
                "{}://{}/{} has no traversable projection in this runtime; read {}://{} for its canonical snapshot",
                scheme,
                segments[0],
                segments[1..].join("/"),
                scheme,
                segments[0]
            )));
        }
        let glob = args
            .glob
            .as_deref()
            .map(globset::Glob::new)
            .transpose()
            .map_err(|error| ToolError::Message(format!("invalid glob: {error}")))?
            .map(|glob| glob.compile_matcher());
        let query = args.query.to_ascii_lowercase();
        let limit = args.limit.unwrap_or(20).min(100);
        let mut matches = self
            .sessions
            .registry()
            .list()
            .map_err(|error| ToolError::Message(error.to_string()))?
            .into_iter()
            .filter(|record| record_matches_scheme(record, scheme, kind))
            .filter(|record| {
                segments.is_empty() || resource_segment_for_record(scheme, record) == segments[0]
            })
            .map(|record| {
                let path = resource_path_for_record(scheme, &record);
                let score = virtual_path_match_score(&query, &path);
                (path, record.last_seen, score)
            })
            .filter(|(path, _, score)| {
                score.is_some() && glob.as_ref().is_none_or(|g| g.is_match(Path::new(path)))
            })
            .collect::<Vec<_>>();
        // `find` is frecency-ranked. The fuzzy score selects the traversed
        // resources; explicit opens update `last_seen`, while discovery does
        // not, so ordering never turns a search into an interaction.
        matches.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(&right.0)));
        let more = matches.len() > limit;
        matches.truncate(limit);
        let mut rendered = matches
            .into_iter()
            .map(|(path, _, score)| format!("{path}\t(score {})", score.unwrap_or_default()))
            .collect::<Vec<_>>();
        if more {
            rendered.push(format!("[truncated: showing at most {limit} results]"));
        }
        Ok(if rendered.is_empty() {
            "No resources found.".into()
        } else {
            rendered.join("\n")
        })
    }
}

/// Render a virtual text resource without ever allowing an offset to float
/// across a newer transcript.  Anchors are first calculated for the complete
/// logical text, then the rendered logical lines are windowed, preserving the
/// same addresses a full read would have returned.
fn paged_virtual_text(
    source: &str,
    path: &str,
    args: &ReadArgs,
) -> Result<rig_core::tool::ToolOutput, ToolError> {
    const DEFAULT_LINES: usize = 200;

    let revision = artist_tools::resource_path::virtual_text_revision(source);
    let offset = args.offset.unwrap_or(1).max(1);
    if offset > 1 {
        let supplied = args
            .revision
            .as_deref()
            .ok_or_else(|| ToolError::stale_revision(path, None, Some(revision.clone())))?;
        if supplied != revision {
            return Err(ToolError::stale_revision(
                path,
                Some(supplied.to_owned()),
                Some(revision),
            ));
        }
    }
    let anchored = artist_tools::resource_path::render_anchored_virtual_text(source);
    let lines = anchored.lines().collect::<Vec<_>>();
    let limit = args.limit.unwrap_or(DEFAULT_LINES).max(1);
    let start = offset.saturating_sub(1);
    let window = lines
        .iter()
        .skip(start)
        .take(limit)
        .copied()
        .collect::<Vec<_>>();
    let mut output = format!("[revision: {revision}]\n{}", window.join("\n"));
    if start.saturating_add(window.len()) < lines.len() {
        let next = offset + window.len();
        output.push_str(&format!(
            "\n[truncated: continue with read(path=\"{path}\", offset={next}, revision=\"{revision}\")]"
        ));
    }
    Ok(rig_core::tool::ToolOutput::one(ToolResultContent::text(
        output,
    )))
}

/// FFF uses neo-frizbee as its typo-tolerant fuzzy engine. Virtual resources
/// are not host files and therefore cannot be inserted into the FFF file
/// index; apply that same engine directly to their canonical snapshot lines.
/// The caller retains these matches in source order as required by grep.
fn fff_fuzzy_virtual_lines(query: &str, lines: &[String], insensitive: bool) -> Vec<String> {
    if query.is_empty() {
        return lines.to_vec();
    }
    let max_typos = (query.len() / 3).min(2) as u16;
    let casing = if insensitive {
        neo_frizbee::CaseMatching::Ignore
    } else {
        neo_frizbee::CaseMatching::Respect
    };
    let mut matcher = neo_frizbee::Matcher::new(
        query,
        &neo_frizbee::Config {
            max_typos: Some(max_typos),
            casing,
            // Source ordering is imposed after matching rather than relying
            // on the engine's score ordering.
            sort: false,
            scoring: neo_frizbee::Scoring {
                exact_match_bonus: 100,
                prefix_bonus: 0,
                capitalization_bonus: if insensitive { 0 } else { 4 },
                ..neo_frizbee::Scoring::default()
            },
            ..Default::default()
        },
    );
    let mut matched = vec![false; lines.len()];
    for found in matcher.match_iter(lines.iter()) {
        if let Some(slot) = matched.get_mut(found.index as usize) {
            *slot = true;
        }
    }
    lines
        .iter()
        .zip(matched)
        .filter_map(|(line, matched)| matched.then(|| line.clone()))
        .collect()
}

/// Tiny deterministic path ranking for the compact session namespace. Every
/// query character must occur in order; closer characters rank higher.
fn virtual_path_match_score(query: &str, candidate: &str) -> Option<u64> {
    if query.is_empty() {
        return Some(0);
    }
    let candidate = candidate.to_ascii_lowercase();
    let mut cursor = 0;
    let mut gaps = 0_u64;
    for needle in query.chars().filter(|character| !character.is_whitespace()) {
        let tail = &candidate[cursor..];
        let (offset, matched) = tail.char_indices().find(|(_, hay)| *hay == needle)?;
        gaps = gaps.saturating_add(offset as u64);
        cursor += offset + matched.len_utf8();
    }
    Some(10_000_u64.saturating_sub(gaps))
}

impl PortableTool for VirtualReadTool {
    const NAME: &'static str = "read";
    type Error = ToolError;
    type Args = ReadArgs;
    type Output = rig_core::tool::ToolOutput;

    fn description(&self) -> String {
        "Read a real file or typed Artist path. Scheme roots list immediate children; session paths return a stateless snapshot with its revision. Real text files return anchored lines and a revision.".into()
    }

    fn parameters(&self) -> serde_json::Value {
        self.real.parameters()
    }

    fn map_error(&self, error: ToolError) -> rig_core::tool::ToolExecutionError {
        error.into_execution_error()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        match ResourcePath::parse(&args.path)
            .map_err(|error| ToolError::Message(error.to_string()))?
        {
            ResourcePath::Real(_) => self.real.call(args).await,
            ResourcePath::Virtual { scheme, segments } => {
                self.read_virtual(scheme, &segments, &args).await
            }
        }
    }
}

impl VirtualReadTool {
    async fn read_virtual(
        &self,
        scheme: ResourceScheme,
        segments: &[String],
        args: &ReadArgs,
    ) -> Result<rig_core::tool::ToolOutput, ToolError> {
        if scheme == ResourceScheme::Skill {
            return self.read_skill(segments, args);
        }
        if scheme == ResourceScheme::Profile {
            return self.read_profile(segments, args);
        }
        if scheme == ResourceScheme::Tools {
            return self.read_tools(segments, args);
        }
        if scheme == ResourceScheme::Memory {
            return self.read_memory(segments, args);
        }
        if scheme == ResourceScheme::Artifact {
            return self.read_artifact(segments, args);
        }
        if scheme == ResourceScheme::Dict {
            return self.read_dictionary(segments, args);
        }
        if scheme == ResourceScheme::Relation {
            return self.read_relationship(segments, args);
        }
        if scheme == ResourceScheme::Rules {
            return self.read_rules(segments, args);
        }
        if scheme == ResourceScheme::Forge {
            return self.read_forge(segments, args).await;
        }
        if scheme == ResourceScheme::Computer && segments.first().is_some_and(|part| part == "use")
        {
            return self.read_computer_capability(segments, args);
        }
        let kind = session_kind(scheme);
        if segments.is_empty() && kind.is_none() {
            return Ok(rig_core::tool::ToolOutput::text(format!(
                "[{}:// has no immediate children in this runtime]",
                scheme
            )));
        }
        let Some(kind) = kind else {
            return Err(ToolError::Message(format!(
                "{}://{} is a typed virtual path but its resolver is not installed in this Artist runtime; read {}:// for immediate children",
                scheme,
                segments.join("/"),
                scheme
            )));
        };
        if scheme == ResourceScheme::Agent && segments.len() == 2 && segments[1] == "todo" {
            return self.read_agent_todo(&segments[0], kind, args);
        }
        if scheme == ResourceScheme::Agent && segments.len() >= 2 && segments[1] == "yields" {
            return self.read_agent_yields(&segments[0], &segments[2..], kind, args);
        }
        if segments.len() == 2
            && matches!(
                segments[1].as_str(),
                "parent" | "children" | "predecessor" | "successor"
            )
        {
            return self.read_resource_relationship_projection(scheme, &segments[0], &segments[1]);
        }
        if segments.len() > 1 {
            return Err(ToolError::Message(format!(
                "{}://{}/{} is not a readable projection; read {}://{} for its canonical snapshot",
                scheme,
                segments[0],
                segments[1..].join("/"),
                scheme,
                segments[0]
            )));
        }
        let records = self
            .sessions
            .registry()
            .list()
            .map_err(|error| ToolError::Message(error.to_string()))?;
        if segments.is_empty() {
            let listing = records
                .into_iter()
                .filter(|record| record_matches_scheme(record, scheme, kind))
                .map(|record| resource_path_for_record(scheme, &record))
                .collect::<Vec<_>>();
            return Ok(rig_core::tool::ToolOutput::text(if listing.is_empty() {
                format!("[{}:// has no immediate children]", scheme)
            } else {
                listing.join("\n")
            }));
        }
        let id = &segments[0];
        let registry_id = registry_id_for_resource(scheme, id);
        // An explicit resource open is the only path that updates its durable
        // recency. `find`/`grep` and root listings deliberately use `list`,
        // whose discovery pass never changes this signal.
        let record = self
            .sessions
            .registry()
            .get(&registry_id)
            .map_err(|error| ToolError::Message(error.to_string()))?
            .filter(|record| record_matches_scheme(record, scheme, kind))
            .ok_or_else(|| {
                ToolError::Message(format!(
                    "unknown {} resource {}://{}; read {}:// for immediate children",
                    scheme, scheme, id, scheme
                ))
            })?;
        if scheme == ResourceScheme::Agent {
            let mut children = vec![format!("agent://{}/todo", record.id)];
            if record
                .snapshot
                .get("yields")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|yields| !yields.is_empty())
            {
                children.push(format!("agent://{}/yields", record.id));
            }
            children.extend([
                format!("agent://{}/parent", record.id),
                format!("agent://{}/children", record.id),
                format!("agent://{}/predecessor", record.id),
                format!("agent://{}/successor", record.id),
            ]);
            return Ok(rig_core::tool::ToolOutput::text(children.join("\n")));
        }
        if scheme == ResourceScheme::Bash {
            let transcript = record
                .snapshot
                .get("output")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            return paged_virtual_text(transcript, &args.path, args);
        }
        let payload = serde_json::json!({
            "path": resource_path_for_record(scheme, &record),
            "kind": record.kind,
            "lifecycle": record.lifecycle,
            "cancelRequested": record.cancel_requested,
            "snapshot": record.snapshot,
        });
        let bytes =
            serde_json::to_vec(&payload).map_err(|error| ToolError::Message(error.to_string()))?;
        let revision = Sha256::digest(&bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let rendered = serde_json::to_string_pretty(&payload)
            .map_err(|error| ToolError::Message(error.to_string()))?;
        Ok(rig_core::tool::ToolOutput::one(ToolResultContent::text(
            format!("[revision: {revision}]\n{rendered}"),
        )))
    }

    /// Addressable relationship projections keep orientation explicit: an edge
    /// `source --child--> target` makes `target/parent` and `source/children`
    /// readable. Successor/predecessor use the same directed convention.
    fn read_resource_relationship_projection(
        &self,
        scheme: ResourceScheme,
        id: &str,
        projection: &str,
    ) -> Result<rig_core::tool::ToolOutput, ToolError> {
        let kind = session_kind(scheme).ok_or_else(|| {
            ToolError::Message(format!(
                "{scheme}://{id} does not support relationship projections"
            ))
        })?;
        let registry_id = registry_id_for_resource(scheme, id);
        let exists = self
            .sessions
            .registry()
            .get(&registry_id)
            .map_err(|error| ToolError::Message(error.to_string()))?
            .is_some_and(|record| record_matches_scheme(&record, scheme, kind));
        if !exists {
            return Err(ToolError::Message(format!(
                "unknown {} resource {}://{}; read {}:// for immediate children",
                scheme, scheme, id, scheme
            )));
        }
        let path = format!("{scheme}://{id}");
        let (source, target, edge_kind) = match projection {
            "parent" => (None, Some(path.as_str()), Some("child")),
            "children" => (Some(path.as_str()), None, Some("child")),
            "predecessor" => (None, Some(path.as_str()), Some("successor")),
            "successor" => (Some(path.as_str()), None, Some("successor")),
            _ => unreachable!("projection is checked by read_virtual"),
        };
        let mut paths = self
            .relationships
            .list(source, target, edge_kind)
            .map_err(|error| ToolError::Message(error.to_string()))?
            .into_iter()
            .map(|edge| {
                if source.is_some() {
                    edge.target
                } else {
                    edge.source
                }
            })
            .collect::<Vec<_>>();
        paths.sort();
        paths.dedup();
        Ok(rig_core::tool::ToolOutput::text(if paths.is_empty() {
            format!("[{path}/{projection} has no immediate children]")
        } else {
            paths.join("\n")
        }))
    }

    async fn read_forge(
        &self,
        segments: &[String],
        args: &ReadArgs,
    ) -> Result<rig_core::tool::ToolOutput, ToolError> {
        if segments.is_empty() {
            return Ok(rig_core::tool::ToolOutput::text(
                "[forge:// has no immediate children; read forge://github/<owner>/<repo>]",
            ));
        }
        let path = format!("forge://{}", segments.join("/"));
        let rendered = crate::forge_tool::read_path(&path)
            .await
            .map_err(|error| ToolError::Message(error.to_string()))?;
        paged_virtual_text(&rendered, &args.path, args)
    }

    fn read_rules(
        &self,
        segments: &[String],
        args: &ReadArgs,
    ) -> Result<rig_core::tool::ToolOutput, ToolError> {
        let mut diagnostics = Vec::new();
        let rules = artist_rules::discovery::discover(self.real.0.root(), &mut diagnostics);
        if segments.is_empty() {
            let mut paths = rules
                .iter()
                .map(|rule| format!("rules://{}", rule.id))
                .collect::<Vec<_>>();
            paths.sort();
            if !diagnostics.is_empty() {
                paths.push(format!(
                    "[rule discovery diagnostics: {}]",
                    diagnostics.join("; ")
                ));
            }
            return Ok(rig_core::tool::ToolOutput::text(if paths.is_empty() {
                "[rules:// has no immediate children]".into()
            } else {
                paths.join("\n")
            }));
        }
        if segments.len() != 1 {
            return Err(ToolError::Message(
                "rules are addressed as rules://<rule-id>".into(),
            ));
        }
        let rule = rules
            .iter()
            .find(|rule| rule.id.0 == segments[0])
            .ok_or_else(|| {
                ToolError::Message(format!(
                    "unknown rule rules://{}; read rules:// for immediate children",
                    segments[0]
                ))
            })?;
        let source = rule
            .source
            .as_ref()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .unwrap_or_else(|| format!("{rule:#?}"));
        paged_virtual_text(&source, &args.path, args)
    }

    fn read_relationship(
        &self,
        segments: &[String],
        args: &ReadArgs,
    ) -> Result<rig_core::tool::ToolOutput, ToolError> {
        if segments.is_empty() {
            let listing = self
                .relationships
                .list(None, None, None)
                .map_err(|error| ToolError::Message(error.to_string()))?
                .into_iter()
                .map(|edge| format!("relation://{}", edge.id))
                .collect::<Vec<_>>();
            return Ok(rig_core::tool::ToolOutput::text(if listing.is_empty() {
                "[relation:// has no immediate children]".into()
            } else {
                listing.join("\n")
            }));
        }
        if segments.len() != 1 {
            return Err(ToolError::Message(
                "relationship edges are addressed as relation://<edge-id>".into(),
            ));
        }
        let edge = self
            .relationships
            .get(&segments[0])
            .map_err(|error| ToolError::Message(error.to_string()))?
            .ok_or_else(|| {
                ToolError::Message(format!(
                    "unknown relationship relation://{}; read relation:// for immediate children",
                    segments[0]
                ))
            })?;
        let rendered = serde_json::to_string_pretty(&edge)
            .map_err(|error| ToolError::Message(error.to_string()))?;
        paged_virtual_text(&rendered, &args.path, args)
    }

    fn read_agent_todo(
        &self,
        artist: &str,
        kind: &str,
        args: &ReadArgs,
    ) -> Result<rig_core::tool::ToolOutput, ToolError> {
        let exists = self
            .sessions
            .registry()
            .list()
            .map_err(|error| ToolError::Message(error.to_string()))?
            .into_iter()
            .any(|record| record.kind == kind && record.id == artist);
        if !exists {
            return Err(ToolError::Message(format!(
                "unknown agent resource agent://{artist}; read agent:// for immediate children"
            )));
        }
        let text = crate::todo::render(&self.todos.get(artist));
        paged_virtual_text(&text, &args.path, args)
    }

    fn read_computer_capability(
        &self,
        segments: &[String],
        args: &ReadArgs,
    ) -> Result<rig_core::tool::ToolOutput, ToolError> {
        match segments {
            [use_root] if use_root == "use" => {
                let paths = crate::computer_tool::capability_paths();
                Ok(rig_core::tool::ToolOutput::text(paths.join("\n")))
            }
            [use_root, action] if use_root == "use" => {
                let document = crate::computer_tool::capability_document(action).ok_or_else(|| {
                    ToolError::Message(format!(
                        "unknown computer capability computer://use/{action}; read computer://use for runnable capability paths"
                    ))
                })?;
                let source = serde_json::to_string_pretty(&document)
                    .map_err(|error| ToolError::Message(error.to_string()))?;
                paged_virtual_text(&source, &args.path, args)
            }
            _ => Err(ToolError::Message(format!(
                "computer://{} is not a capability projection; read computer://use for runnable capability paths",
                segments.join("/")
            ))),
        }
    }

    fn read_agent_yields(
        &self,
        artist: &str,
        tail: &[String],
        kind: &str,
        args: &ReadArgs,
    ) -> Result<rig_core::tool::ToolOutput, ToolError> {
        let record = self
            .sessions
            .registry()
            .get(artist)
            .map_err(|error| ToolError::Message(error.to_string()))?
            .filter(|record| record.kind == kind)
            .ok_or_else(|| {
                ToolError::Message(format!(
                    "unknown agent resource agent://{artist}; read agent:// for immediate children"
                ))
            })?;
        let yields = record
            .snapshot
            .get("yields")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        if tail.is_empty() {
            let paths = yields
                .iter()
                .filter_map(|yielded| yielded.get("sequence").and_then(serde_json::Value::as_u64))
                .map(|sequence| format!("agent://{artist}/yields/{sequence}"))
                .collect::<Vec<_>>();
            return Ok(rig_core::tool::ToolOutput::text(if paths.is_empty() {
                format!("[agent://{artist}/yields has no immediate children]")
            } else {
                paths.join("\n")
            }));
        }
        if tail.len() != 1 {
            return Err(ToolError::Message(format!(
                "agent://{artist}/yields/{} is not a readable projection; read agent://{artist}/yields",
                tail.join("/")
            )));
        }
        let sequence = tail[0].parse::<u64>().map_err(|_| {
            ToolError::Message(format!(
                "agent yield `{}` is not a numeric sequence; read agent://{artist}/yields",
                tail[0]
            ))
        })?;
        let yielded = yields
            .into_iter()
            .find(|yielded| {
                yielded.get("sequence").and_then(serde_json::Value::as_u64) == Some(sequence)
            })
            .ok_or_else(|| {
                ToolError::Message(format!(
                    "unknown agent yield agent://{artist}/yields/{sequence}; read agent://{artist}/yields"
                ))
            })?;
        let source = serde_json::to_string_pretty(&yielded)
            .map_err(|error| ToolError::Message(error.to_string()))?;
        paged_virtual_text(&source, &args.path, args)
    }

    fn read_skill(
        &self,
        segments: &[String],
        args: &ReadArgs,
    ) -> Result<rig_core::tool::ToolOutput, ToolError> {
        if segments.is_empty() {
            let skills = self.resources.skills_for(&self.profile);
            return Ok(rig_core::tool::ToolOutput::text(if skills.is_empty() {
                "[skill:// has no immediate children]".into()
            } else {
                skills
                    .into_iter()
                    .map(|skill| format!("skill://{}", skill.name))
                    .collect::<Vec<_>>()
                    .join("\n")
            }));
        }
        if segments.len() != 1 {
            return Err(ToolError::Message(format!(
                "skill://{}/{} is not a readable projection; read skill://{}",
                segments[0],
                segments[1..].join("/"),
                segments[0]
            )));
        }
        let source = self
            .resources
            .read_skill(&self.profile, &segments[0])
            .map_err(ToolError::Message)?;
        paged_virtual_text(&source, &args.path, args)
    }

    fn read_profile(
        &self,
        segments: &[String],
        args: &ReadArgs,
    ) -> Result<rig_core::tool::ToolOutput, ToolError> {
        if segments.is_empty() {
            let names = self.profiles.names();
            return Ok(rig_core::tool::ToolOutput::text(if names.is_empty() {
                "[profile:// has no immediate children]".into()
            } else {
                names
                    .into_iter()
                    .map(|name| format!("profile://{name}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            }));
        }
        if segments.len() != 1 {
            return Err(ToolError::Message(format!(
                "profile://{}/{} is not a readable projection; read profile://{}",
                segments[0],
                segments[1..].join("/"),
                segments[0]
            )));
        }
        let profile = self
            .profiles
            .get(&segments[0])
            .map_err(ToolError::Message)?;
        let payload = serde_json::json!({
            "path": format!("profile://{}", profile.name),
            "name": profile.name,
            "description": profile.description,
            "instructions": profile.instructions,
            "yieldSchema": profile.yield_schema,
            "source": profile.source.map(|source| source.display().to_string()),
        });
        let source = serde_json::to_string_pretty(&payload)
            .map_err(|error| ToolError::Message(error.to_string()))?;
        paged_virtual_text(&source, &args.path, args)
    }

    fn read_tools(
        &self,
        segments: &[String],
        args: &ReadArgs,
    ) -> Result<rig_core::tool::ToolOutput, ToolError> {
        let Some(root) = &self.tools_root else {
            return Ok(rig_core::tool::ToolOutput::text(
                "[tools:// has no configured extension root]",
            ));
        };
        let registry = self
            .extension_manager
            .as_ref()
            .map(|manager| manager.registry_snapshot())
            .unwrap_or_else(|| artist_extensions::Registry::load(root.clone()));
        if segments.is_empty() {
            return Ok(rig_core::tool::ToolOutput::text(
                if registry.extensions.is_empty() {
                    "[tools:// has no immediate children]".into()
                } else {
                    registry
                        .extensions
                        .iter()
                        .map(|extension| format!("tools://{}", extension.manifest.id))
                        .collect::<Vec<_>>()
                        .join("\n")
                },
            ));
        }
        let extension = registry
            .extensions
            .iter()
            .find(|extension| extension.manifest.id == segments[0])
            .ok_or_else(|| {
                ToolError::Message(format!(
                    "unknown tools resource tools://{}; read tools:// for immediate children",
                    segments[0]
                ))
            })?;
        if segments.len() == 1 {
            let mut children = vec![
                format!("tools://{}/manifest", extension.manifest.id),
                format!("tools://{}/module", extension.manifest.id),
            ];
            if !extension.manifest.tools.is_empty() {
                children.push(format!("tools://{}/tool", extension.manifest.id));
            }
            return Ok(rig_core::tool::ToolOutput::text(children.join("\n")));
        }
        if segments.len() == 2 && segments[1] == "tool" {
            let paths = extension
                .manifest
                .tools
                .iter()
                .filter(|tool| !tool.name.is_empty() && !tool.name.contains('/'))
                .map(|tool| format!("tools://{}/tool/{}", extension.manifest.id, tool.name))
                .collect::<Vec<_>>();
            return Ok(rig_core::tool::ToolOutput::text(if paths.is_empty() {
                format!(
                    "[tools://{}/tool has no immediate children]",
                    extension.manifest.id
                )
            } else {
                paths.join("\n")
            }));
        }
        if segments.len() == 3 && segments[1] == "tool" {
            let tool = extension
                .manifest
                .tools
                .iter()
                .find(|tool| tool.name == segments[2])
                .ok_or_else(|| {
                    ToolError::Message(format!(
                        "unknown extension tool tools://{}/tool/{}; read tools://{}/tool",
                        extension.manifest.id, segments[2], extension.manifest.id
                    ))
                })?;
            let content = serde_json::to_string_pretty(&serde_json::json!({
                "path": format!("tools://{}/tool/{}", extension.manifest.id, tool.name),
                "name": tool.name,
                "description": tool.description,
                "parameters": tool.parameters,
                "run": "run this exact path with the JSON arguments field",
            }))
            .map_err(|error| ToolError::Message(error.to_string()))?;
            return paged_virtual_text(&content, &args.path, args);
        }
        if segments.len() != 2 {
            return Err(ToolError::Message(format!(
                "tools://{}/{} is not a readable projection; read tools://{}",
                segments[0],
                segments[1..].join("/"),
                segments[0]
            )));
        }
        let content = match segments[1].as_str() {
            "manifest" => {
                let content = serde_json::to_string_pretty(&extension.manifest)
                    .map_err(|error| ToolError::Message(error.to_string()))?;
                content
            }
            "module" => {
                let bytes = std::fs::read(&extension.wasm)
                    .map_err(|error| ToolError::Message(error.to_string()))?;
                let digest = Sha256::digest(&bytes)
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>();
                let content = serde_json::to_string_pretty(&serde_json::json!({
                    "path": format!("tools://{}/module", extension.manifest.id),
                    "bytes": bytes.len(),
                    "sha256": digest,
                    "mediaType": "application/wasm",
                }))
                .map_err(|error| ToolError::Message(error.to_string()))?;
                content
            }
            other => {
                return Err(ToolError::Message(format!(
                    "unknown tools projection `{other}`; read tools://{} for immediate children",
                    extension.manifest.id
                )));
            }
        };
        paged_virtual_text(&content, &args.path, args)
    }

    fn read_memory(
        &self,
        segments: &[String],
        args: &ReadArgs,
    ) -> Result<rig_core::tool::ToolOutput, ToolError> {
        if segments.is_empty() {
            return Ok(rig_core::tool::ToolOutput::text(
                "memory://diagnostics\nmemory://documents\nmemory://facts\nmemory://labels\nmemory://rules\nmemory://findings\nmemory://bayesian-priors\nmemory://bayesian-observations\nmemory://bayesian-estimates\nmemory://candidates",
            ));
        }
        let (directory, label) = match segments[0].as_str() {
            "diagnostics" => (
                artist_session::muse_diagnostics_dir(self.real.0.root()),
                "diagnostics",
            ),
            "documents" => (
                artist_session::muse_documents_dir(self.real.0.root()),
                "documents",
            ),
            "candidates" => (
                artist_session::muse_candidates_dir(self.real.0.root()),
                "candidates",
            ),
            "facts" => (artist_session::muse_facts_dir(self.real.0.root()), "facts"),
            "labels" => (
                artist_session::muse_labels_dir(self.real.0.root()),
                "labels",
            ),
            "rules" => (artist_session::muse_rules_dir(self.real.0.root()), "rules"),
            "findings" => (
                artist_session::muse_findings_dir(self.real.0.root()),
                "findings",
            ),
            "bayesian-priors" => (
                artist_session::muse_bayesian_priors_dir(self.real.0.root()),
                "bayesian-priors",
            ),
            "bayesian-observations" => (
                artist_session::muse_bayesian_observations_dir(self.real.0.root()),
                "bayesian-observations",
            ),
            "bayesian-estimates" => (
                artist_session::muse_bayesian_estimates_dir(self.real.0.root()),
                "bayesian-estimates",
            ),
            other => {
                return Err(ToolError::Message(format!(
                    "unknown memory projection `{other}`; read memory:// for immediate children"
                )));
            }
        };
        if segments.len() == 1 {
            let mut ids = std::fs::read_dir(&directory)
                .ok()
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .filter_map(|entry| {
                    let path = entry.path();
                    (path.is_file()
                        && path
                            .extension()
                            .is_some_and(|extension| extension == "json"))
                    .then(|| {
                        path.file_stem()
                            .and_then(|stem| stem.to_str())
                            .map(str::to_owned)
                    })
                    .flatten()
                })
                .collect::<Vec<_>>();
            ids.sort();
            return Ok(rig_core::tool::ToolOutput::text(if ids.is_empty() {
                format!("[memory://{label} has no immediate children]")
            } else {
                ids.into_iter()
                    .map(|id| format!("memory://{label}/{id}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            }));
        }
        if segments.len() != 2 {
            return Err(ToolError::Message(format!(
                "memory://{label}/{} is not a readable projection; read memory://{label}",
                segments[1..].join("/")
            )));
        }
        let path = directory.join(format!("{}.json", segments[1]));
        let source = std::fs::read_to_string(&path).map_err(|error| {
            ToolError::Message(format!(
                "unknown Muse {label} memory://{label}/{}: {error}",
                segments[1],
            ))
        })?;
        paged_virtual_text(&source, &args.path, args)
    }

    fn read_artifact(
        &self,
        segments: &[String],
        args: &ReadArgs,
    ) -> Result<rig_core::tool::ToolOutput, ToolError> {
        if segments.is_empty() {
            let artifacts = self
                .pages
                .artifacts()
                .map_err(|error| ToolError::Message(error.to_string()))?;
            return Ok(rig_core::tool::ToolOutput::text(if artifacts.is_empty() {
                "[artifact:// has no immediate children]".into()
            } else {
                artifacts
                    .into_iter()
                    .map(|artifact| format!("artifact://{}", artifact.id))
                    .collect::<Vec<_>>()
                    .join("\n")
            }));
        }
        if segments.len() != 1 {
            return Err(ToolError::Message(format!(
                "artifact://{}/{} is not a readable projection; read artifact://{}",
                segments[0],
                segments[1..].join("/"),
                segments[0]
            )));
        }
        let artifact = self
            .pages
            .artifact(&segments[0])
            .map_err(|error| ToolError::Message(error.to_string()))?
            .ok_or_else(|| {
                ToolError::Message(format!(
                    "unknown artifact resource artifact://{}; read artifact:// for immediate children",
                    segments[0]
                ))
            })?;
        let source = serde_json::to_string_pretty(&artifact)
            .map_err(|error| ToolError::Message(error.to_string()))?;
        let metadata = paged_virtual_text(&source, &args.path, args)?;
        let Some(media_type) = artifact
            .media_type
            .as_deref()
            .and_then(Self::artifact_image_type)
        else {
            return Ok(metadata);
        };
        let Some(bytes) = self
            .pages
            .media_bytes(&artifact.id)
            .map_err(|error| ToolError::Message(error.to_string()))?
        else {
            return Ok(metadata);
        };
        use base64::Engine as _;
        use rig_core::completion::message::{DocumentSourceKind, Image};
        let mut content = metadata.into_content().into_iter().collect::<Vec<_>>();
        content.push(ToolResultContent::Image(Image {
            data: DocumentSourceKind::Base64(
                base64::engine::general_purpose::STANDARD.encode(bytes),
            ),
            media_type: Some(media_type),
            detail: None,
            additional_params: None,
        }));
        Ok(rig_core::tool::ToolOutput::content(
            rig_core::OneOrMany::many(content)
                .expect("artifact metadata with visual attachment is non-empty"),
        ))
    }

    /// Image MIME types whose visual attachment representation is supported by the
    /// current provider abstraction. Other media remains safely addressable as
    /// metadata until its attachment type has an explicit model-facing contract.
    fn artifact_image_type(mime: &str) -> Option<rig_core::completion::message::ImageMediaType> {
        use rig_core::completion::message::ImageMediaType;
        match mime {
            "image/png" => Some(ImageMediaType::PNG),
            "image/jpeg" | "image/jpg" => Some(ImageMediaType::JPEG),
            "image/gif" => Some(ImageMediaType::GIF),
            "image/webp" => Some(ImageMediaType::WEBP),
            "image/svg+xml" => Some(ImageMediaType::SVG),
            "image/heic" => Some(ImageMediaType::HEIC),
            "image/heif" => Some(ImageMediaType::HEIF),
            _ => None,
        }
    }

    fn read_dictionary(
        &self,
        segments: &[String],
        args: &ReadArgs,
    ) -> Result<rig_core::tool::ToolOutput, ToolError> {
        let Some(dictionary) = &self.dictionary else {
            return Ok(rig_core::tool::ToolOutput::text(
                "[dict:// is unavailable because Artist has no config directory]",
            ));
        };
        if segments.is_empty() {
            let entries = dictionary
                .entries()
                .map_err(|error| ToolError::Message(error.to_string()))?;
            return Ok(rig_core::tool::ToolOutput::text(if entries.is_empty() {
                "[dict:// has no immediate children]".into()
            } else {
                entries
                    .into_iter()
                    .map(|(reference, _)| format!("dict://{reference}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            }));
        }
        // A TECA-prefix reference contains `/`, which the virtual path parser
        // splits into segments; rejoin them to reconstruct the opaque `§...`
        // reference before resolving against the known streams.
        let reference = segments.join("/");
        let value = dictionary
            .resolve(&reference)
            .map_err(|error| ToolError::Message(error.to_string()))?;
        paged_virtual_text(&value, &args.path, args)
    }
}

pub(crate) fn session_kind(scheme: ResourceScheme) -> Option<&'static str> {
    match scheme {
        ResourceScheme::Agent => Some("subagent"),
        ResourceScheme::Bash => Some("bash"),
        ResourceScheme::Ask => Some("ask"),
        ResourceScheme::Canvas => Some("canvas"),
        ResourceScheme::Computer => Some("computer"),
        // Native processes use the terminal backend and registry kind, but
        // their public path is deliberately distinct from `bash://`.
        ResourceScheme::Process => Some("bash"),
        ResourceScheme::Eval => Some("eval"),
        ResourceScheme::Debug => Some("debug"),
        ResourceScheme::Artifact
        | ResourceScheme::Dict
        | ResourceScheme::Memory
        | ResourceScheme::Profile
        | ResourceScheme::Skill
        | ResourceScheme::Tools
        | ResourceScheme::Forge
        | ResourceScheme::Code
        | ResourceScheme::Relation
        | ResourceScheme::Rules => None,
    }
}

fn record_matches_scheme(
    record: &artist_registry::SessionRecord,
    scheme: ResourceScheme,
    kind: &str,
) -> bool {
    match scheme {
        ResourceScheme::Bash => {
            record.kind == kind
                && record
                    .snapshot
                    .get("resourceType")
                    .and_then(serde_json::Value::as_str)
                    != Some("process")
        }
        ResourceScheme::Process => {
            record.kind == kind
                && record
                    .snapshot
                    .get("resourceType")
                    .and_then(serde_json::Value::as_str)
                    == Some("process")
        }
        _ => record.kind == kind,
    }
}

fn resource_segment_for_record(
    scheme: ResourceScheme,
    record: &artist_registry::SessionRecord,
) -> String {
    match scheme {
        ResourceScheme::Process => record
            .id
            .strip_prefix("process:")
            .unwrap_or(&record.id)
            .to_owned(),
        ResourceScheme::Eval => record
            .id
            .strip_prefix("eval:")
            .unwrap_or(&record.id)
            .to_owned(),
        ResourceScheme::Debug => record
            .id
            .strip_prefix("debug:")
            .unwrap_or(&record.id)
            .to_owned(),
        _ => record.id.clone(),
    }
}

fn resource_path_for_record(
    scheme: ResourceScheme,
    record: &artist_registry::SessionRecord,
) -> String {
    format!(
        "{}://{}",
        scheme,
        resource_segment_for_record(scheme, record)
    )
}

fn registry_id_for_resource(scheme: ResourceScheme, segment: &str) -> String {
    match scheme {
        ResourceScheme::Process if !segment.starts_with("process:") => format!("process:{segment}"),
        ResourceScheme::Eval if !segment.starts_with("eval:") => format!("eval:{segment}"),
        ResourceScheme::Debug if !segment.starts_with("debug:") => format!("debug:{segment}"),
        _ => segment.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_tool(
        workspace: artist_tools::Workspace,
        root: &std::path::Path,
        hub: SessionHub,
        todos: crate::todo::TodoStore,
    ) -> VirtualReadTool {
        let profiles = crate::profiles::Profiles::discover(root);
        let profile = profiles.get("default").unwrap();
        VirtualReadTool::new(
            ReadTool(workspace),
            hub,
            todos,
            crate::resources::Resources::discover(root),
            profiles,
            profile,
            crate::pagination::PageStore::memory(),
            crate::relationships::RelationshipStore::for_project(root).unwrap(),
        )
    }

    #[tokio::test]
    async fn session_scheme_root_lists_children_and_snapshot_is_revisioned() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let workspace = artist_tools::Workspace::open(root.path(), state.path(), "goethe").unwrap();
        let hub = SessionHub::standard(root.path(), "goethe", None);
        hub.registry()
            .create_exact(
                "job",
                "bash",
                "goethe",
                None,
                serde_json::json!({"output":"ok"}),
            )
            .unwrap();
        let read = read_tool(
            workspace,
            root.path(),
            hub,
            crate::todo::TodoStore::default(),
        );
        let root_output = read
            .call(ReadArgs {
                path: "bash://".into(),
                offset: None,
                limit: None,
                revision: None,
            })
            .await
            .unwrap();
        assert!(root_output.render().contains("bash://job"));
        let snapshot = read
            .call(ReadArgs {
                path: "bash://job".into(),
                offset: None,
                limit: None,
                revision: None,
            })
            .await
            .unwrap();
        let rendered = snapshot.render();
        assert!(rendered.contains("[revision: "));
        assert!(rendered.contains(": ok"));
    }

    #[tokio::test]
    async fn native_processes_have_a_distinct_typed_root_not_a_bash_alias() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let workspace = artist_tools::Workspace::open(root.path(), state.path(), "goethe").unwrap();
        let hub = SessionHub::standard(root.path(), "goethe", None);
        hub.registry()
            .create_exact(
                "process:worker",
                "bash",
                "goethe",
                None,
                serde_json::json!({"resourceType":"process", "output":"native"}),
            )
            .unwrap();
        let read = read_tool(
            workspace,
            root.path(),
            hub,
            crate::todo::TodoStore::default(),
        );
        let process_root = read
            .call(ReadArgs {
                path: "process://".into(),
                offset: None,
                limit: None,
                revision: None,
            })
            .await
            .unwrap()
            .render();
        assert!(process_root.contains("process://worker"), "{process_root}");
        assert!(!process_root.contains("process:worker"), "{process_root}");
        let bash_root = read
            .call(ReadArgs {
                path: "bash://".into(),
                offset: None,
                limit: None,
                revision: None,
            })
            .await
            .unwrap()
            .render();
        assert!(!bash_root.contains("worker"), "{bash_root}");
        let snapshot = read
            .call(ReadArgs {
                path: "process://worker".into(),
                offset: None,
                limit: None,
                revision: None,
            })
            .await
            .unwrap()
            .render();
        assert!(
            snapshot.contains("\"path\": \"process://worker\""),
            "{snapshot}"
        );
    }

    #[tokio::test]
    async fn bash_continuation_requires_the_current_transcript_revision() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let workspace = artist_tools::Workspace::open(root.path(), state.path(), "goethe").unwrap();
        let hub = SessionHub::standard(root.path(), "goethe", None);
        hub.registry()
            .create_exact(
                "job",
                "bash",
                "goethe",
                None,
                serde_json::json!({"output":"one\ntwo\nthree"}),
            )
            .unwrap();
        let read = read_tool(
            workspace,
            root.path(),
            hub,
            crate::todo::TodoStore::default(),
        );
        let first = read
            .call(ReadArgs {
                path: "bash://job".into(),
                offset: None,
                limit: Some(1),
                revision: None,
            })
            .await
            .unwrap()
            .render();
        let revision = first
            .lines()
            .next()
            .and_then(|line| line.strip_prefix("[revision: "))
            .and_then(|line| line.strip_suffix(']'))
            .unwrap();
        assert!(first.contains("offset=2"));
        let missing = read
            .call(ReadArgs {
                path: "bash://job".into(),
                offset: Some(2),
                limit: None,
                revision: None,
            })
            .await
            .unwrap_err();
        assert!(missing.to_string().contains("stale_revision"));
        let next = read
            .call(ReadArgs {
                path: "bash://job".into(),
                offset: Some(2),
                limit: None,
                revision: Some(revision.to_owned()),
            })
            .await
            .unwrap()
            .render();
        assert!(next.contains("two"));
    }

    #[tokio::test]
    async fn every_declared_scheme_root_is_readable_before_its_resolver_is_installed() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let workspace = artist_tools::Workspace::open(root.path(), state.path(), "goethe").unwrap();
        let read = read_tool(
            workspace,
            root.path(),
            SessionHub::standard(root.path(), "goethe", None),
            crate::todo::TodoStore::default(),
        );
        for scheme in [
            ResourceScheme::Artifact,
            ResourceScheme::Process,
            ResourceScheme::Eval,
            ResourceScheme::Debug,
            ResourceScheme::Forge,
            ResourceScheme::Code,
            ResourceScheme::Relation,
            ResourceScheme::Rules,
            ResourceScheme::Dict,
            ResourceScheme::Memory,
            ResourceScheme::Profile,
            ResourceScheme::Skill,
            ResourceScheme::Tools,
        ] {
            let output = read
                .call(ReadArgs {
                    path: format!("{scheme}://"),
                    offset: None,
                    limit: None,
                    revision: None,
                })
                .await
                .unwrap();
            let rendered = output.render();
            assert!(
                rendered.contains("no immediate children")
                    || rendered.contains("skill://")
                    || rendered.contains("profile://")
                    || rendered.contains("memory://"),
                "unexpected root projection for {scheme}: {rendered}"
            );
        }
    }

    #[tokio::test]
    async fn computer_capability_catalog_is_addressable_without_a_live_surface() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let workspace = artist_tools::Workspace::open(root.path(), state.path(), "goethe").unwrap();
        let read = read_tool(
            workspace,
            root.path(),
            SessionHub::standard(root.path(), "goethe", None),
            crate::todo::TodoStore::default(),
        );
        let catalog = read
            .call(ReadArgs {
                path: "computer://use".into(),
                offset: None,
                limit: None,
                revision: None,
            })
            .await
            .unwrap()
            .render();
        assert!(catalog.contains("computer://use/observe"));
        assert!(catalog.contains("computer://use/do"));
        let capability = read
            .call(ReadArgs {
                path: "computer://use/observe".into(),
                offset: None,
                limit: None,
                revision: None,
            })
            .await
            .unwrap()
            .render();
        assert!(capability.contains("\"rung\": 0"));
        assert!(capability.contains("requiresSurface"));
        let first_page = read
            .call(ReadArgs {
                path: "computer://use/observe".into(),
                offset: None,
                limit: Some(1),
                revision: None,
            })
            .await
            .unwrap()
            .render();
        let revision = first_page
            .lines()
            .next()
            .and_then(|line| line.strip_prefix("[revision: "))
            .and_then(|line| line.strip_suffix(']'))
            .unwrap();
        assert!(first_page.contains("offset=2"));
        let stale = read
            .call(ReadArgs {
                path: "computer://use/observe".into(),
                offset: Some(2),
                limit: None,
                revision: Some("not-the-revision".into()),
            })
            .await
            .unwrap_err();
        assert!(stale.to_string().contains("stale_revision"));
        let next = read
            .call(ReadArgs {
                path: "computer://use/observe".into(),
                offset: Some(2),
                limit: None,
                revision: Some(revision.into()),
            })
            .await
            .unwrap()
            .render();
        assert!(next.contains("requiresSurface"));
    }

    #[tokio::test]
    async fn agent_todo_is_a_read_only_projection_beneath_its_canonical_path() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let workspace = artist_tools::Workspace::open(root.path(), state.path(), "goethe").unwrap();
        let hub = SessionHub::standard(root.path(), "goethe", None);
        hub.registry()
            .create_exact(
                "goethe",
                "subagent",
                "goethe",
                None,
                serde_json::Value::Null,
            )
            .unwrap();
        let todos = crate::todo::TodoStore::default();
        todos.set(
            "goethe",
            vec![artist_session::TodoItem {
                text: "implement the projection".into(),
                status: artist_session::TodoStatus::default(),
                children: Vec::new(),
            }],
        );
        let read = read_tool(workspace, root.path(), hub, todos);
        let output = read
            .call(ReadArgs {
                path: "agent://goethe/todo".into(),
                offset: None,
                limit: None,
                revision: None,
            })
            .await
            .unwrap()
            .render();
        assert!(output.contains("[revision: "));
        assert!(output.contains("implement the projection"));
    }

    #[tokio::test]
    async fn relationship_projections_are_readable_on_canonical_resources() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let workspace = artist_tools::Workspace::open(root.path(), state.path(), "ada").unwrap();
        let hub = SessionHub::standard(root.path(), "ada", None);
        for artist in ["ada", "bea", "cy"] {
            hub.registry()
                .create_exact(artist, "subagent", "ada", None, serde_json::Value::Null)
                .unwrap();
        }
        let relationships =
            crate::relationships::RelationshipStore::for_project(root.path()).unwrap();
        relationships
            .add("agent://ada", "child", "agent://bea")
            .unwrap();
        relationships
            .add("agent://bea", "successor", "agent://cy")
            .unwrap();
        let read = read_tool(
            workspace,
            root.path(),
            hub,
            crate::todo::TodoStore::default(),
        );
        let parent = read
            .call(ReadArgs {
                path: "agent://bea/parent".into(),
                offset: None,
                limit: None,
                revision: None,
            })
            .await
            .unwrap()
            .render();
        assert_eq!(parent, "agent://ada");
        let children = read
            .call(ReadArgs {
                path: "agent://ada/children".into(),
                offset: None,
                limit: None,
                revision: None,
            })
            .await
            .unwrap()
            .render();
        assert_eq!(children, "agent://bea");
        let predecessor = read
            .call(ReadArgs {
                path: "agent://cy/predecessor".into(),
                offset: None,
                limit: None,
                revision: None,
            })
            .await
            .unwrap()
            .render();
        assert_eq!(predecessor, "agent://bea");
    }

    #[tokio::test]
    async fn completed_agent_yields_are_read_only_canonical_children() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let workspace = artist_tools::Workspace::open(root.path(), state.path(), "goethe").unwrap();
        let hub = SessionHub::standard(root.path(), "goethe", None);
        let record = hub
            .registry()
            .create_exact(
                "goethe",
                "subagent",
                "goethe",
                None,
                serde_json::json!({
                    "yields": [{"sequence": 1, "profile": "reviewer", "value": {"answer": "done"}}]
                }),
            )
            .unwrap();
        hub.registry()
            .finish(&record.id, artist_registry::SessionStatus::Completed, None)
            .unwrap();
        let read = read_tool(
            workspace,
            root.path(),
            hub,
            crate::todo::TodoStore::default(),
        );

        let agent = read
            .call(ReadArgs {
                path: "agent://goethe".into(),
                offset: None,
                limit: None,
                revision: None,
            })
            .await
            .unwrap()
            .render();
        assert!(agent.contains("agent://goethe/todo"));
        assert!(agent.contains("agent://goethe/yields"));

        let listing = read
            .call(ReadArgs {
                path: "agent://goethe/yields".into(),
                offset: None,
                limit: None,
                revision: None,
            })
            .await
            .unwrap()
            .render();
        assert!(listing.contains("agent://goethe/yields/1"));
        let yielded = read
            .call(ReadArgs {
                path: "agent://goethe/yields/1".into(),
                offset: None,
                limit: None,
                revision: None,
            })
            .await
            .unwrap()
            .render();
        assert!(yielded.contains("[revision: "));
        assert!(yielded.contains("done"));
    }

    #[tokio::test]
    async fn skill_paths_list_and_read_the_discovered_skill_source() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let skill = root.path().join(".artist/skills/inspect/SKILL.md");
        std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
        std::fs::write(
            &skill,
            "---\nname: inspect\ndescription: Inspect a target safely.\n---\n\nRead the target first.\n",
        )
        .unwrap();
        let workspace = artist_tools::Workspace::open(root.path(), state.path(), "goethe").unwrap();
        let read = read_tool(
            workspace,
            root.path(),
            SessionHub::standard(root.path(), "goethe", None),
            crate::todo::TodoStore::default(),
        );
        let listing = read
            .call(ReadArgs {
                path: "skill://".into(),
                offset: None,
                limit: None,
                revision: None,
            })
            .await
            .unwrap()
            .render();
        assert!(listing.contains("skill://inspect"));
        let source = read
            .call(ReadArgs {
                path: "skill://inspect".into(),
                offset: None,
                limit: None,
                revision: None,
            })
            .await
            .unwrap()
            .render();
        assert!(source.contains("[revision: "));
        assert!(source.contains("Read the target first."));
    }

    #[tokio::test]
    async fn profile_paths_list_and_read_resolved_profile_snapshots() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let workspace = artist_tools::Workspace::open(root.path(), state.path(), "goethe").unwrap();
        let read = read_tool(
            workspace,
            root.path(),
            SessionHub::standard(root.path(), "goethe", None),
            crate::todo::TodoStore::default(),
        );
        let listing = read
            .call(ReadArgs {
                path: "profile://".into(),
                offset: None,
                limit: None,
                revision: None,
            })
            .await
            .unwrap()
            .render();
        assert!(listing.contains("profile://default"));
        let snapshot = read
            .call(ReadArgs {
                path: "profile://default".into(),
                offset: None,
                limit: None,
                revision: None,
            })
            .await
            .unwrap()
            .render();
        assert!(snapshot.contains("[revision: "));
        assert!(snapshot.contains("\"name\": \"default\""));
        let first = read
            .call(ReadArgs {
                path: "profile://default".into(),
                offset: None,
                limit: Some(1),
                revision: None,
            })
            .await
            .unwrap()
            .render();
        let revision = first
            .lines()
            .next()
            .and_then(|line| line.strip_prefix("[revision: "))
            .and_then(|line| line.strip_suffix(']'))
            .unwrap();
        let next = read
            .call(ReadArgs {
                path: "profile://default".into(),
                offset: Some(2),
                limit: None,
                revision: Some(revision.into()),
            })
            .await
            .unwrap()
            .render();
        assert!(next.contains("\"name\": \"default\""));
    }

    #[tokio::test]
    async fn tools_paths_expose_manifest_and_binary_metadata_without_binary_text() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let extension = root.path().join("extensions/demo");
        std::fs::create_dir_all(&extension).unwrap();
        std::fs::write(
            extension.join("extension.toml"),
            "id = 'demo'\ndescription = 'Demo extension'\n\n[[tools]]\nname = 'inspect'\ndescription = 'Inspect the project'\n",
        )
        .unwrap();
        std::fs::write(extension.join("extension.wasm"), b"\0asm demo bytes").unwrap();
        let workspace = artist_tools::Workspace::open(root.path(), state.path(), "goethe").unwrap();
        let read = read_tool(
            workspace,
            root.path(),
            SessionHub::standard(root.path(), "goethe", None),
            crate::todo::TodoStore::default(),
        )
        .with_tools_root(root.path().join("extensions"));
        let listing = read
            .call(ReadArgs {
                path: "tools://".into(),
                offset: None,
                limit: None,
                revision: None,
            })
            .await
            .unwrap()
            .render();
        assert!(listing.contains("tools://demo"));
        let manifest = read
            .call(ReadArgs {
                path: "tools://demo/manifest".into(),
                offset: None,
                limit: None,
                revision: None,
            })
            .await
            .unwrap()
            .render();
        assert!(manifest.contains("Demo extension"));
        let tool_listing = read
            .call(ReadArgs {
                path: "tools://demo/tool".into(),
                offset: None,
                limit: None,
                revision: None,
            })
            .await
            .unwrap()
            .render();
        assert!(tool_listing.contains("tools://demo/tool/inspect"));
        let tool = read
            .call(ReadArgs {
                path: "tools://demo/tool/inspect".into(),
                offset: None,
                limit: None,
                revision: None,
            })
            .await
            .unwrap()
            .render();
        assert!(tool.contains("run this exact path"));
        let module = read
            .call(ReadArgs {
                path: "tools://demo/module".into(),
                offset: None,
                limit: None,
                revision: None,
            })
            .await
            .unwrap()
            .render();
        assert!(module.contains("application/wasm"));
        assert!(!module.contains("demo bytes"));
    }

    #[tokio::test]
    async fn memory_diagnostics_are_addressable_without_becoming_host_paths() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let diagnostic =
            artist_session::write_muse_diagnostic(root.path(), "read", "bad source").unwrap();
        let workspace = artist_tools::Workspace::open(root.path(), state.path(), "goethe").unwrap();
        let read = read_tool(
            workspace,
            root.path(),
            SessionHub::standard(root.path(), "goethe", None),
            crate::todo::TodoStore::default(),
        );
        let listing = read
            .call(ReadArgs {
                path: "memory://diagnostics".into(),
                offset: None,
                limit: None,
                revision: None,
            })
            .await
            .unwrap()
            .render();
        assert!(listing.contains(&diagnostic.id));
        let rendered = read
            .call(ReadArgs {
                path: format!("memory://diagnostics/{}", diagnostic.id),
                offset: None,
                limit: None,
                revision: None,
            })
            .await
            .unwrap()
            .render();
        assert!(rendered.contains("[revision: "));
        assert!(rendered.contains("bad source"));
    }

    #[tokio::test]
    async fn retained_muse_documents_are_addressable_under_memory() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let event = artist_session::Envelope {
            v: artist_session::SCHEMA_VERSION,
            seq: 1,
            ts: 1,
            session: "session".into(),
            run: None,
            lineage: artist_session::MAIN_LINEAGE.into(),
            kind: "turn.user".into(),
            payload: serde_json::json!({"content":[{"type":"text","text":"Persist me."}]}),
        };
        let snapshot = muse_registry::PackageRegistry::new()
            .snapshot_all()
            .unwrap();
        let ingestion = artist_session::ingest_for_muse(root.path(), "read", &[event], snapshot);
        let document = ingestion
            .documents
            .first()
            .expect("stored document")
            .id
            .to_string();
        let workspace = artist_tools::Workspace::open(root.path(), state.path(), "goethe").unwrap();
        let read = read_tool(
            workspace,
            root.path(),
            SessionHub::standard(root.path(), "goethe", None),
            crate::todo::TodoStore::default(),
        );

        let listing = read
            .call(ReadArgs {
                path: "memory://documents".into(),
                offset: None,
                limit: None,
                revision: None,
            })
            .await
            .unwrap()
            .render();
        assert!(listing.contains(&document));
        let source = read
            .call(ReadArgs {
                path: format!("memory://documents/{document}"),
                offset: None,
                limit: None,
                revision: None,
            })
            .await
            .unwrap()
            .render();
        assert!(source.contains("muse-occurrence-7"));
    }

    #[tokio::test]
    async fn retained_muse_candidates_are_addressable_under_memory() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let event = artist_session::Envelope {
            v: artist_session::SCHEMA_VERSION,
            seq: 1,
            ts: 1,
            session: "session".into(),
            run: None,
            lineage: artist_session::MAIN_LINEAGE.into(),
            kind: "turn.user".into(),
            payload: serde_json::json!({"content":[{"type":"text","text":"Persist me."}]}),
        };
        let snapshot = muse_registry::PackageRegistry::new()
            .snapshot_all()
            .unwrap();
        let ingestion = artist_session::ingest_for_muse(root.path(), "read", &[event], snapshot);
        let mut candidate = artist_session::MuseOntologyCandidate {
            id: String::new(),
            candidate: artist_session::MuseOntologyCandidateKind::Concept {
                id: muse_core::ConceptId::from("artist:ReviewedEvent"),
                parents: std::collections::BTreeSet::from([muse_core::ConceptId::from(
                    "artist:ArtistSessionEventRecord",
                )]),
            },
            declared_rules: vec![muse_ontology::Axiom::Subsumption {
                child: muse_core::ConceptId::from("artist:ReviewedEvent"),
                parent: muse_core::ConceptId::from("artist:ArtistSessionEventRecord"),
            }],
            source_documents: std::collections::BTreeSet::from([ingestion.documents[0].id.clone()]),
            examples: vec!["the event was reviewed".into()],
            tests: vec!["candidate remains inert".into()],
        };
        candidate.id = candidate.content_id().unwrap();
        artist_session::write_muse_candidate(root.path(), &candidate).unwrap();
        let workspace = artist_tools::Workspace::open(root.path(), state.path(), "goethe").unwrap();
        let read = read_tool(
            workspace,
            root.path(),
            SessionHub::standard(root.path(), "goethe", None),
            crate::todo::TodoStore::default(),
        );
        let listing = read
            .call(ReadArgs {
                path: "memory://candidates".into(),
                offset: None,
                limit: None,
                revision: None,
            })
            .await
            .unwrap()
            .render();
        assert!(listing.contains(&candidate.id));
        let source = read
            .call(ReadArgs {
                path: format!("memory://candidates/{}", candidate.id),
                offset: None,
                limit: None,
                revision: None,
            })
            .await
            .unwrap()
            .render();
        assert!(source.contains("candidate remains inert"));
    }

    #[tokio::test]
    async fn retained_muse_explicit_facts_are_addressable_under_memory() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let event = artist_session::Envelope {
            v: artist_session::SCHEMA_VERSION,
            seq: 1,
            ts: 1,
            session: "session".into(),
            run: None,
            lineage: artist_session::MAIN_LINEAGE.into(),
            kind: "turn.user".into(),
            payload: serde_json::json!({"content":[{"type":"text","text":"Persist me."}]}),
        };
        let snapshot = muse_registry::PackageRegistry::new()
            .snapshot_all()
            .unwrap();
        let ingestion = artist_session::ingest_for_muse(root.path(), "read", &[event], snapshot);
        let document = ingestion.documents[0].id.to_string();
        assert_eq!(ingestion.facts[0].document.to_string(), document);
        let workspace = artist_tools::Workspace::open(root.path(), state.path(), "goethe").unwrap();
        let read = read_tool(
            workspace,
            root.path(),
            SessionHub::standard(root.path(), "goethe", None),
            crate::todo::TodoStore::default(),
        );
        let listing = read
            .call(ReadArgs {
                path: "memory://facts".into(),
                offset: None,
                limit: None,
                revision: None,
            })
            .await
            .unwrap()
            .render();
        assert!(listing.contains(&document));
        let source = read
            .call(ReadArgs {
                path: format!("memory://facts/{document}"),
                offset: None,
                limit: None,
                revision: None,
            })
            .await
            .unwrap()
            .render();
        assert!(source.contains("artist:eventRecordSession"));
    }

    #[tokio::test]
    async fn artifacts_are_addressable_by_metadata_without_payload_text() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let pages = crate::pagination::PageStore::memory();
        let secret = "artifact-private".repeat(10_000);
        pages
            .paginate("read", &serde_json::json!({"value": secret}), &secret)
            .unwrap()
            .unwrap();
        let artifact_id = pages.artifacts().unwrap().remove(0).id;
        let workspace = artist_tools::Workspace::open(root.path(), state.path(), "goethe").unwrap();
        let read = read_tool(
            workspace,
            root.path(),
            SessionHub::standard(root.path(), "goethe", None),
            crate::todo::TodoStore::default(),
        )
        .with_pages(pages);
        let listing = read
            .call(ReadArgs {
                path: "artifact://".into(),
                offset: None,
                limit: None,
                revision: None,
            })
            .await
            .unwrap()
            .render();
        assert!(listing.contains(&artifact_id));
        let metadata = read
            .call(ReadArgs {
                path: format!("artifact://{artifact_id}"),
                offset: None,
                limit: None,
                revision: None,
            })
            .await
            .unwrap()
            .render();
        assert!(metadata.contains("[revision: "));
        assert!(metadata.contains("\"tool\": \"read\""));
        assert!(!metadata.contains("artifact-private"));
    }

    #[tokio::test]
    async fn reading_an_image_artifact_returns_metadata_and_a_visual_attachment() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let pages = crate::pagination::PageStore::memory();
        let artifact_id = pages
            .capture_media("computer:screenshot", "image/png", b"not-a-real-png")
            .unwrap();
        let workspace = artist_tools::Workspace::open(root.path(), state.path(), "goethe").unwrap();
        let read = read_tool(
            workspace,
            root.path(),
            SessionHub::standard(root.path(), "goethe", None),
            crate::todo::TodoStore::default(),
        )
        .with_pages(pages);
        let result = read
            .call(ReadArgs {
                path: format!("artifact://{artifact_id}"),
                offset: None,
                limit: None,
                revision: None,
            })
            .await
            .unwrap();
        assert!(result.render().contains("computer:screenshot"));
        assert!(result.into_content().iter().any(|block| matches!(
            block,
            ToolResultContent::Image(image)
                if image.media_type
                    == Some(rig_core::completion::message::ImageMediaType::PNG)
        )));
    }

    #[tokio::test]
    async fn dictionary_references_are_durable_and_readable_as_typed_paths() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let dictionary = crate::Dictionary::at(root.path().join("dictionary.json")).unwrap();
        let reference = dictionary.intern("shared exact value").unwrap();
        let workspace = artist_tools::Workspace::open(root.path(), state.path(), "goethe").unwrap();
        let read = read_tool(
            workspace,
            root.path(),
            SessionHub::standard(root.path(), "goethe", None),
            crate::todo::TodoStore::default(),
        )
        .with_dictionary(dictionary);
        let listing = read
            .call(ReadArgs {
                path: "dict://".into(),
                offset: None,
                limit: None,
                revision: None,
            })
            .await
            .unwrap()
            .render();
        assert!(listing.contains(&reference));
        let value = read
            .call(ReadArgs {
                path: format!("dict://{reference}"),
                offset: None,
                limit: None,
                revision: None,
            })
            .await
            .unwrap()
            .render();
        assert!(value.contains("[revision: "));
        assert!(value.contains("shared exact value"));
    }

    #[tokio::test]
    async fn find_searches_a_session_scheme_without_touching_real_paths() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let workspace = artist_tools::Workspace::open(root.path(), state.path(), "goethe").unwrap();
        let hub = SessionHub::standard(root.path(), "goethe", None);
        hub.registry()
            .create_exact(
                "compile-log",
                "bash",
                "goethe",
                None,
                serde_json::Value::Null,
            )
            .unwrap();
        let find = VirtualFindTool::new(FindTool(workspace), hub);
        let output = find
            .call(FindArgs {
                query: "cmlg".into(),
                path: Some("bash://".into()),
                glob: None,
                limit: None,
            })
            .await
            .unwrap();
        assert!(output.contains("bash://compile-log"), "{output}");
    }

    #[tokio::test]
    async fn virtual_find_uses_explicit_open_frecency_not_lexical_score() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let workspace = artist_tools::Workspace::open(root.path(), state.path(), "goethe").unwrap();
        let hub = SessionHub::standard(root.path(), "goethe", None);
        hub.registry()
            .create_exact("first", "bash", "goethe", None, serde_json::Value::Null)
            .unwrap();
        hub.registry()
            .create_exact("second", "bash", "goethe", None, serde_json::Value::Null)
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        hub.registry().get("first").unwrap();
        let find = VirtualFindTool::new(FindTool(workspace), hub);
        let output = find
            .call(FindArgs {
                query: String::new(),
                path: Some("bash://".into()),
                glob: None,
                limit: None,
            })
            .await
            .unwrap();
        assert!(
            output.find("bash://first") < output.find("bash://second"),
            "{output}"
        );
    }

    #[tokio::test]
    async fn grep_searches_the_canonical_session_snapshot() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let workspace = artist_tools::Workspace::open(root.path(), state.path(), "goethe").unwrap();
        let hub = SessionHub::standard(root.path(), "goethe", None);
        hub.registry()
            .create_exact(
                "job",
                "bash",
                "goethe",
                None,
                serde_json::json!({"message":"needle"}),
            )
            .unwrap();
        let grep = VirtualGrepTool::new(GrepTool(workspace), hub);
        let output = grep
            .call(GrepArgs {
                query: "needle".into(),
                path: Some("bash://job".into()),
                glob: None,
                match_mode: None,
                case: None,
                context: None,
                limit: None,
            })
            .await
            .unwrap();
        assert!(output.contains("bash://job"), "{output}");
        assert!(output.contains("needle"), "{output}");
    }

    #[tokio::test]
    async fn virtual_grep_auto_falls_back_to_fff_fuzzy_matching() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let workspace = artist_tools::Workspace::open(root.path(), state.path(), "goethe").unwrap();
        let hub = SessionHub::standard(root.path(), "goethe", None);
        hub.registry()
            .create_exact(
                "job",
                "bash",
                "goethe",
                None,
                serde_json::json!({"message":"needle"}),
            )
            .unwrap();
        let grep = VirtualGrepTool::new(GrepTool(workspace), hub);
        let output = grep
            .call(GrepArgs {
                query: "neelde".into(),
                path: Some("bash://job".into()),
                glob: None,
                match_mode: None,
                case: None,
                context: None,
                limit: None,
            })
            .await
            .unwrap();
        assert!(output.contains("needle"), "{output}");
    }
}
