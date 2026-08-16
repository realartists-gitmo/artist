//! Read-only repository projections for `repo://` resources.
//!
//! The vendored `artist-ast` fork supplies structured multi-language
//! declarations. This adapter translates those declarations into repository
//! resources and Teca-compatible file anchors. It is deliberately read-only:
//! source edits remain native-path `edit` requests, anchored by the kernel.

use crate::{
    AnchorSet, AnchoredLine, AnchoredText, ClaimDecision, DynamicClaimProvider,
    DynamicResourceProvider, DynamicValue, DynamicVerbResult, KernelError, Pattern,
    ResourceAddress, ResourceFuture, ResourceUri, SearchService, StructuralAnalyzer,
    StructuralLine, VerbId, is_file_uri, normalize,
};
use artist_ast::{JsonValue as Value, from_str, json, to_value, to_vec_pretty};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};
use url::Url;

pub struct RepositoryHandler {
    root: PathBuf,
    project: String,
    structure: StructuralAnalyzer,
    search: SearchService,
    file_projections: bool,
    file_symbol_projections: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepositoryVerbBindings {
    pub read: VerbId,
    pub find: VerbId,
    pub grep: VerbId,
}

pub struct RepositoryResourceProvider {
    handler: Arc<RepositoryHandler>,
    bindings: RepositoryVerbBindings,
}

impl RepositoryResourceProvider {
    pub fn new(handler: Arc<RepositoryHandler>, bindings: RepositoryVerbBindings) -> Self {
        Self { handler, bindings }
    }
}

impl RepositoryHandler {
    pub fn new(root: impl AsRef<Path>) -> Result<Self, KernelError> {
        let root = fs::canonicalize(root.as_ref()).map_err(|error| KernelError::Handler {
            message: format!(
                "canonicalize repository root {}: {error}",
                root.as_ref().display()
            ),
        })?;
        if !root.is_dir() {
            return Err(KernelError::Handler {
                message: format!("repository root is not a directory: {}", root.display()),
            });
        }
        let project = root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("project")
            .to_owned();
        Ok(Self {
            root,
            project,
            structure: StructuralAnalyzer::default(),
            search: SearchService::new(),
            file_projections: true,
            file_symbol_projections: true,
        })
    }

    /// Keep `file://` AST compatibility available for standalone native
    /// kernels, while allowing the WASM AST resource to own those projections
    /// in the application kernel. `repo://` remains handled in both modes.
    pub fn without_file_projections(mut self) -> Self {
        self.file_projections = false;
        self
    }

    pub fn without_file_symbol_projections(mut self) -> Self {
        self.file_symbol_projections = false;
        self
    }

    pub fn project(&self) -> &str {
        &self.project
    }

    fn url(target: &ResourceAddress) -> Result<&Url, KernelError> {
        target
            .as_uri()
            .map(|uri| uri.as_ref())
            .ok_or_else(|| KernelError::InvalidRequest {
                message: "repository resources require a URI target".to_owned(),
            })
    }

    fn is_local(target: &ResourceAddress) -> bool {
        is_file_uri(target)
    }

    fn claims_file_projection(&self, uri: &ResourceUri) -> bool {
        if !self.file_projections || uri.scheme() != "file" || uri.query().is_some() {
            return false;
        }
        let Ok(path) = uri.as_ref().to_file_path() else {
            return false;
        };
        if path.is_file() {
            return false;
        }
        let mut current = path;
        while let Some(parent) = current.parent() {
            if parent.is_file() {
                return self.file_symbol_projections
                    || !current.to_string_lossy().contains("/symbols");
            }
            if parent == self.root {
                break;
            }
            current = parent.to_owned();
        }
        false
    }

    fn file_and_suffix(
        &self,
        target: &ResourceAddress,
    ) -> Result<(PathBuf, Vec<String>), KernelError> {
        let url = Self::url(target)?;
        if Self::is_local(target) {
            let mut candidate = url
                .to_file_path()
                .map_err(|_| KernelError::InvalidRequest {
                    message: format!("invalid local file URI: {target}"),
                })?;
            let mut suffix = Vec::new();
            while !candidate.is_file() {
                let Some(name) = candidate
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(str::to_owned)
                else {
                    break;
                };
                suffix.push(name);
                if !candidate.pop() {
                    break;
                }
            }
            if candidate.is_file() {
                let canonical =
                    fs::canonicalize(&candidate).map_err(|error| KernelError::Handler {
                        message: format!("resolve local file {}: {error}", candidate.display()),
                    })?;
                suffix.reverse();
                return Ok((canonical, suffix));
            }
            return Err(KernelError::NotFound {
                uri: target.to_string(),
            });
        }
        if url.host_str().is_some_and(|host| host != self.project) {
            return Err(KernelError::NotFound {
                uri: target.to_string(),
            });
        }
        let segments = url
            .path_segments()
            .map(|parts| parts.map(str::to_owned).collect::<Vec<_>>())
            .unwrap_or_default();
        for split in (1..=segments.len()).rev() {
            let candidate = self
                .root
                .join(segments[..split].iter().collect::<PathBuf>());
            if candidate.is_file() {
                let canonical =
                    fs::canonicalize(&candidate).map_err(|error| KernelError::Handler {
                        message: format!(
                            "resolve repository file {}: {error}",
                            candidate.display()
                        ),
                    })?;
                if !canonical.starts_with(&self.root) {
                    break;
                }
                return Ok((canonical, segments[split..].to_vec()));
            }
        }
        Err(KernelError::NotFound {
            uri: target.to_string(),
        })
    }

    fn read_resource(&self, target: &ResourceAddress) -> Result<Value, KernelError> {
        let original_target = target.clone();
        let target = normalize(target)?;
        let _analysis_guard = artist_ast::graph_cache::analysis_lock();
        let snapshot =
            artist_ast::snapshot::RepositorySnapshot::capture(&self.root).map_err(|e| {
                KernelError::Handler {
                    message: format!("capture repository snapshot: {e}"),
                }
            })?;
        let url = Self::url(&target)?;
        let root_suffix = url.path().trim_matches('/');
        if matches!(
            root_suffix,
            "deps" | "reverse-deps" | "surface" | "cycles" | "graph"
        ) {
            return match root_suffix {
                "surface" => self.surface_resource(snapshot.root(), &target),
                "deps" => self.graph_resource(snapshot.root(), &[], false, &target),
                "reverse-deps" => self.graph_resource(snapshot.root(), &[], true, &target),
                "cycles" | "graph" => self.graph_resource(snapshot.root(), &[], false, &target),
                _ => unreachable!(),
            };
        }
        if url.scheme() == "repo" {
            let segments = url
                .path_segments()
                .map(|s| s.collect::<Vec<_>>())
                .unwrap_or_default();
            if segments.first() == Some(&"symbols") {
                let tail = &segments[1..];
                let relation = tail.iter().position(|part| {
                    matches!(
                        *part,
                        "callers"
                            | "callees"
                            | "impact"
                            | "trace"
                            | "implements"
                            | "implementations"
                    )
                });
                let end = relation.unwrap_or(tail.len());
                let symbol = tail[..end].join(".");
                if !symbol.is_empty() {
                    if let Some(index) = relation {
                        let rel = tail[index..]
                            .iter()
                            .map(|s| (*s).to_owned())
                            .collect::<Vec<_>>();
                        return match rel[0].as_str() {
                            "callers" => {
                                self.call_resource(snapshot.root(), &[symbol], true, &target)
                            }
                            "callees" => {
                                self.call_resource(snapshot.root(), &[symbol], false, &target)
                            }
                            "impact" => self.impact_resource(snapshot.root(), &symbol, &target),
                            "implements" | "implementations" => {
                                self.implements_resource(snapshot.root(), &symbol, &target)
                            }
                            "trace" if rel.len() == 3 && rel[1] == "to" => {
                                self.trace_resource(snapshot.root(), &symbol, &rel[2], &target)
                            }
                            "trace" => {
                                return Err(KernelError::InvalidRequest {
                                    message: "trace URI requires symbols/<from>/trace/to/<to>"
                                        .to_owned(),
                                });
                            }
                            _ => unreachable!(),
                        };
                    }
                    return self.project_symbol_resource(snapshot.root(), &symbol, &target);
                }
            }
        }
        let (file, suffix) = self.file_and_suffix(&target)?;
        let snapshot = if Self::is_local(&target) {
            artist_ast::snapshot::RepositorySnapshot::capture_for_file(&self.root, &file).map_err(
                |e| KernelError::Handler {
                    message: format!("capture file snapshot: {e}"),
                },
            )?
        } else {
            snapshot
        };
        let analysis_file = snapshot.map_path(&file);
        let source = fs::read(&analysis_file).map_err(|error| KernelError::Handler {
            message: format!("read {}: {error}", analysis_file.display()),
        })?;
        if suffix.is_empty() {
            return Ok(
                json!({"type":"source", "path": relative(&self.root, &file), "content": String::from_utf8_lossy(&source), "symbols": symbols(&self.root, &analysis_file, &source, &self.structure, &self.project, &self.file_projection_base(&original_target, &file))?}),
            );
        }
        if suffix.first().is_some_and(|segment| segment == "symbols") {
            let entries = symbols(
                &self.root,
                &analysis_file,
                &source,
                &self.structure,
                &self.project,
                &self.file_projection_base(&original_target, &file),
            )?;
            let symbol_tail = &suffix[1..];
            if let Some((index, relation)) = symbol_tail.iter().enumerate().find(|(_, part)| {
                matches!(
                    part.as_str(),
                    "callers" | "callees" | "impact" | "trace" | "implements" | "implementations"
                )
            }) {
                let symbol = symbol_tail[..index].join(".");
                let rest = &symbol_tail[index..];
                return match relation.as_str() {
                    "callers" => self.call_resource(snapshot.root(), &[symbol], true, &target),
                    "callees" => self.call_resource(snapshot.root(), &[symbol], false, &target),
                    "impact" => self.impact_resource(snapshot.root(), &symbol, &target),
                    "implements" | "implementations" => {
                        self.implements_resource(snapshot.root(), &symbol, &target)
                    }
                    "trace" if rest.len() == 3 && rest[1] == "to" => {
                        self.trace_resource(snapshot.root(), &symbol, &rest[2], &target)
                    }
                    "trace" => {
                        return Err(KernelError::InvalidRequest {
                            message: "trace URI requires symbols/<from>/trace/to/<to>".to_owned(),
                        });
                    }
                    _ => unreachable!(),
                };
            }
            return symbol_resource(entries, symbol_tail, &target);
        }
        if suffix.first().is_some_and(|segment| segment == "map") {
            return self.map_resource(&analysis_file, &source, &target);
        }
        if suffix.first().is_some_and(|segment| segment == "show") {
            let name = suffix[1..].join(".");
            return self.show_resource(&analysis_file, &source, &name, &target);
        }
        if suffix
            .first()
            .is_some_and(|segment| segment == "implements")
        {
            return self.implements_resource(snapshot.root(), &suffix[1..].join("::"), &target);
        }
        if suffix.first().is_some_and(|segment| segment == "surface") {
            return self.surface_resource(&analysis_file, &target);
        }
        if suffix.first().is_some_and(|segment| segment == "deps") {
            return self.graph_resource(snapshot.root(), &suffix[1..], false, &target);
        }
        if suffix
            .first()
            .is_some_and(|segment| segment == "reverse-deps")
        {
            return self.graph_resource(snapshot.root(), &suffix[1..], true, &target);
        }
        Err(KernelError::NotFound {
            uri: target.to_string(),
        })
    }

    fn project_symbol_resource(
        &self,
        analysis_root: &Path,
        name: &str,
        target: &ResourceAddress,
    ) -> Result<Value, KernelError> {
        let mut items = Vec::new();
        for parsed in artist_ast::walk_and_parse(&[analysis_root.to_owned()], None) {
            for found in artist_ast::core::find_symbols(&parsed, name) {
                items.push(json!({"file": relative(analysis_root, &parsed.path), "symbol": found}));
            }
        }
        if items.is_empty() {
            return Err(KernelError::NotFound {
                uri: target.to_string(),
            });
        }
        Ok(json!({"type":"symbol", "items":items}))
    }

    fn file_projection_base(&self, original: &ResourceAddress, file: &Path) -> String {
        if original.as_uri().is_some_and(|uri| uri.scheme() == "file") {
            return Url::from_file_path(file)
                .map(|url| url.to_string())
                .unwrap_or_else(|_| file.display().to_string());
        }
        format!("repo://{}/{}", self.project, relative(&self.root, file))
    }

    fn map_resource(
        &self,
        file: &Path,
        source: &[u8],
        target: &ResourceAddress,
    ) -> Result<Value, KernelError> {
        let text = std::str::from_utf8(source).map_err(|_| KernelError::Handler {
            message: "AST map requires UTF-8 source".to_owned(),
        })?;
        let parsed = artist_ast::parse_source(file, text).ok_or_else(|| KernelError::NotFound {
            uri: target.to_string(),
        })?;
        Ok(from_str(&artist_ast::core::render_json_map(
            &[parsed],
            &artist_ast::core::MapOptions::default(),
            false,
        ))
        .unwrap_or_else(|_| json!({"type":"map","items":[]})))
    }

    fn show_resource(
        &self,
        file: &Path,
        source: &[u8],
        name: &str,
        target: &ResourceAddress,
    ) -> Result<Value, KernelError> {
        let text = std::str::from_utf8(source).map_err(|_| KernelError::Handler {
            message: "AST show requires UTF-8 source".to_owned(),
        })?;
        let parsed = artist_ast::parse_source(file, text).ok_or_else(|| KernelError::NotFound {
            uri: target.to_string(),
        })?;
        let matches = artist_ast::core::find_symbols(&parsed, name);
        if matches.is_empty() {
            return Err(KernelError::NotFound {
                uri: target.to_string(),
            });
        }
        Ok(json!({"type":"show", "items":matches}))
    }

    fn implements_resource(
        &self,
        analysis_root: &Path,
        name: &str,
        _target: &ResourceAddress,
    ) -> Result<Value, KernelError> {
        let graph = artist_ast::graph_cache::get_or_init(analysis_root).map_err(|e| {
            KernelError::Handler {
                message: e.to_string(),
            }
        })?;
        let mut parsed = Vec::new();
        for path in graph.deps.forward.keys() {
            if let Ok(bytes) = fs::read(path)
                && let Ok(text) = std::str::from_utf8(&bytes)
                && let Some(result) = artist_ast::parse_source(path, text)
            {
                parsed.push(result);
            }
        }
        let items = artist_ast::core::find_implementations(&parsed, name, true);
        Ok(json!({"type":"implements", "items":items}))
    }

    fn surface_resource(
        &self,
        file: &Path,
        _target: &ResourceAddress,
    ) -> Result<Value, KernelError> {
        let options = artist_ast::surface::SurfaceOptions::default();
        let entries = artist_ast::surface::resolve_surface(file, &options).map_err(|e| {
            KernelError::Handler {
                message: e.to_string(),
            }
        })?;
        Ok(json!({"type":"surface", "items":entries}))
    }

    fn graph_resource(
        &self,
        analysis_root: &Path,
        suffix: &[String],
        reverse: bool,
        _target: &ResourceAddress,
    ) -> Result<Value, KernelError> {
        let graph = artist_ast::graph_cache::get_or_init(analysis_root).map_err(|e| {
            KernelError::Handler {
                message: e.to_string(),
            }
        })?;
        let scope = suffix.join("/");
        let paths = if reverse {
            graph.deps.reverse_adjacency().into_iter().filter(|(path, _)| scope.is_empty() || graph.deps.rel(path).starts_with(&scope)).map(|(path, sources)| json!({"path": graph.deps.rel(&path), "sources": sources.iter().map(|p| graph.deps.rel(p)).collect::<Vec<_>>() })).collect::<Vec<_>>()
        } else {
            graph.deps.forward.iter().filter(|(path, _)| scope.is_empty() || graph.deps.rel(path).starts_with(&scope)).map(|(path, edges)| json!({"path": graph.deps.rel(path), "targets": edges.iter().map(|e| json!({"path": graph.deps.rel(&e.target), "line":e.line, "kind":e.kind})).collect::<Vec<_>>() })).collect::<Vec<_>>()
        };
        Ok(
            json!({"type": if reverse {"reverse-deps"} else {"deps"}, "items":paths, "stats":graph.deps.stats}),
        )
    }

    fn call_resource(
        &self,
        analysis_root: &Path,
        suffix: &[String],
        callers: bool,
        _target: &ResourceAddress,
    ) -> Result<Value, KernelError> {
        let graph =
            artist_ast::graph_cache::ensure_with_calls(analysis_root, false).map_err(|e| {
                KernelError::Handler {
                    message: e.to_string(),
                }
            })?;
        let calls = graph.calls.as_ref().ok_or_else(|| KernelError::Handler {
            message: "call graph unavailable".to_owned(),
        })?;
        let name = suffix.join(".");
        let qns = artist_ast::calls::cli_helpers::resolve_target_qns(calls, &name);
        let mut items = Vec::new();
        for qn in qns {
            let hits = if callers {
                artist_ast::calls::traverse::callers(calls, &qn, 8, usize::MAX, |_| true)
            } else {
                artist_ast::calls::traverse::callees(calls, &qn, 8)
            };
            for hit in hits {
                items.push(json!({"depth":hit.depth,"source":hit.edge.source.0,"target":hit.edge.target,"line":hit.edge.line,"confidence":hit.edge.confidence}));
            }
        }
        Ok(json!({"type":if callers {"callers"} else {"callees"}, "items":items}))
    }

    fn trace_resource(
        &self,
        analysis_root: &Path,
        source: &str,
        destination: &str,
        _target: &ResourceAddress,
    ) -> Result<Value, KernelError> {
        let from = source.to_owned();
        let to = destination.to_owned();
        let graph =
            artist_ast::graph_cache::ensure_with_calls(analysis_root, false).map_err(|e| {
                KernelError::Handler {
                    message: e.to_string(),
                }
            })?;
        let calls = graph.calls.as_ref().ok_or_else(|| KernelError::Handler {
            message: "call graph unavailable".to_owned(),
        })?;
        let froms = artist_ast::calls::cli_helpers::resolve_target_qns(calls, &from);
        let tos = artist_ast::calls::cli_helpers::resolve_target_qns(calls, &to);
        let path = artist_ast::calls::trace::find_path(calls, &froms, &tos, 8)
            .map(|found| json!({"start":found.start.0,"hops":found.hops.into_iter().map(|hop| json!({"qname":hop.qn.0,"source":hop.via.source.0,"line":hop.via.line})).collect::<Vec<_>>() }));
        Ok(json!({"type":"trace","from":from,"to":to,"result":path,"stats":calls.stats}))
    }

    fn impact_resource(
        &self,
        analysis_root: &Path,
        name: &str,
        _target: &ResourceAddress,
    ) -> Result<Value, KernelError> {
        let opts = artist_ast::impact::ImpactOptions {
            depth: 8,
            limit: 1000,
            mode: artist_ast::impact::ImpactMode::All,
            include_ambiguous: false,
            tests: false,
            exclude_tests: false,
            json: true,
            pretty: false,
        };
        let reports = artist_ast::impact::report(name, analysis_root, &opts)
            .map_err(|e| KernelError::Handler { message: e })?;
        Ok(json!({"type":"impact","items":reports}))
    }
}

impl DynamicClaimProvider for RepositoryResourceProvider {
    fn claim(&self, verb: &VerbId, uri: &ResourceUri) -> ClaimDecision {
        let supported = verb == &self.bindings.read
            || verb == &self.bindings.find
            || verb == &self.bindings.grep;
        if supported && (uri.scheme() == "repo" || self.handler.claims_file_projection(uri)) {
            ClaimDecision::Handle
        } else {
            ClaimDecision::Pass
        }
    }
}

impl DynamicResourceProvider for RepositoryResourceProvider {
    fn verb_definitions(&self) -> Vec<crate::VerbDefinition> {
        [
            (&self.bindings.read, "read"),
            (&self.bindings.find, "find"),
            (&self.bindings.grep, "grep"),
        ]
        .into_iter()
        .map(|(identity, function)| {
            crate::VerbDefinition::new(
                identity.clone(),
                function,
                function,
                format!("Repository {function} provider"),
            )
        })
        .collect()
    }

    fn invoke<'a>(
        &'a self,
        verb: &'a VerbId,
        uri: &'a ResourceUri,
        input: DynamicValue,
    ) -> ResourceFuture<'a> {
        Box::pin(async move {
            let output = if verb == &self.bindings.read {
                self.handler.dynamic_read(uri.clone(), &input)?
            } else if verb == &self.bindings.find {
                self.handler
                    .dynamic_find(uri.clone(), repository_string_field(&input, "query")?)?
            } else if verb == &self.bindings.grep {
                self.handler
                    .dynamic_grep(uri.clone(), repository_string_field(&input, "pattern")?)?
            } else {
                return Err(KernelError::UnsupportedVerb {
                    verb: verb.to_string(),
                    uri: uri.to_string(),
                });
            };
            Ok(DynamicVerbResult {
                verb: verb.clone(),
                function: verb.function().to_owned(),
                output,
            })
        })
    }
}

impl RepositoryHandler {
    fn dynamic_read(
        &self,
        uri: ResourceUri,
        input: &DynamicValue,
    ) -> Result<DynamicValue, KernelError> {
        let target = ResourceAddress::uri(uri.clone());
        let (file, suffix) = self.file_and_suffix(&target).unwrap_or_else(|_| {
            (
                self.root.join(".artist-projection.json"),
                vec!["projection".to_owned()],
            )
        });
        let source = if !suffix.is_empty() {
            let value = self.read_resource(&target)?;
            to_vec_pretty(&value).map_err(|error| KernelError::Handler {
                message: format!("serialize repository projection: {error}"),
            })?
        } else {
            fs::read(&file).map_err(|error| KernelError::Handler {
                message: format!("read {}: {error}", file.display()),
            })?
        };
        let text = repository_anchored_text(uri, &file, &source, &self.structure)?;
        let (at, before, after) = repository_read_options(input)?;
        Ok(DynamicValue::Variant(
            "lines".to_owned(),
            Some(Box::new(repository_text(select_repository_read_window(
                text,
                at.as_ref(),
                before,
                after,
            )?))),
        ))
    }

    fn dynamic_find(&self, uri: ResourceUri, query: String) -> Result<DynamicValue, KernelError> {
        let paths = self.find_paths(&ResourceAddress::uri(uri), &query)?;
        let mut seen = std::collections::HashSet::new();
        Ok(DynamicValue::Record(BTreeMap::from([(
            "uris".to_owned(),
            DynamicValue::List(
                paths
                    .into_iter()
                    .filter(|(path, _)| seen.insert(path.clone()))
                    .map(|(path, directory)| {
                        ResourceUri::parse(&format!("{}{}", path, if directory { "/" } else { "" }))
                            .map(DynamicValue::ResourceUri)
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            ),
        )])))
    }

    fn dynamic_grep(&self, uri: ResourceUri, pattern: String) -> Result<DynamicValue, KernelError> {
        let target = ResourceAddress::uri(uri.clone());
        let root = self.file_and_suffix(&target)?.0;
        let mut matches =
            self.search
                .grep_file(&root, &Pattern::parse(&pattern)?, &self.structure)?;
        for item in &mut matches {
            if uri.scheme() == "repo" {
                let file_uri =
                    item.uri
                        .as_ref()
                        .to_file_path()
                        .map_err(|_| KernelError::InvalidUri {
                            message: item.uri.to_string(),
                        })?;
                item.uri = ResourceUri::parse(&format!(
                    "repo://{}/{}",
                    self.project,
                    relative(&self.root, &file_uri)
                ))?;
            }
        }
        Ok(DynamicValue::Record(BTreeMap::from([(
            "matches".to_owned(),
            DynamicValue::List(matches.into_iter().map(repository_text).collect()),
        )])))
    }
}

fn repository_string_field(input: &DynamicValue, name: &str) -> Result<String, KernelError> {
    let DynamicValue::Record(fields) = input else {
        return Err(KernelError::InvalidRequest {
            message: "repository dynamic input must be a record".to_owned(),
        });
    };
    match fields.get(name) {
        Some(DynamicValue::String(value)) => Ok(value.clone()),
        _ => Err(KernelError::InvalidRequest {
            message: format!("repository dynamic field {name} must be a string"),
        }),
    }
}

fn repository_line(line: AnchoredLine) -> DynamicValue {
    DynamicValue::Record(BTreeMap::from([
        (
            "anchor".to_owned(),
            DynamicValue::String(line.anchor.to_string()),
        ),
        ("text".to_owned(), DynamicValue::String(line.text)),
        (
            "ending".to_owned(),
            DynamicValue::Enum(format!("{:?}", line.ending).to_lowercase()),
        ),
    ]))
}

fn repository_text(text: AnchoredText) -> DynamicValue {
    DynamicValue::Record(BTreeMap::from([
        ("uri".to_owned(), DynamicValue::ResourceUri(text.uri)),
        (
            "lines".to_owned(),
            DynamicValue::List(text.lines.into_iter().map(repository_line).collect()),
        ),
    ]))
}

fn select_repository_read_window(
    mut text: AnchoredText,
    at: Option<&crate::Position>,
    before: Option<u32>,
    after: Option<u32>,
) -> Result<AnchoredText, KernelError> {
    const DEFAULT_READ_WINDOW: usize = 200;
    let at = at.unwrap_or(&crate::Position::Top);
    if matches!(at, crate::Position::Top) && before.is_some()
        || matches!(at, crate::Position::Bottom) && after.is_some()
    {
        return Err(KernelError::InvalidRequest {
            message: "read window is invalid for its position".to_owned(),
        });
    }
    let count = text.lines.len();
    let index = match at {
        crate::Position::Top => 0,
        crate::Position::Bottom => count,
        crate::Position::At(anchor) => text
            .lines
            .iter()
            .position(|line| &line.anchor == anchor)
            .ok_or_else(|| KernelError::StaleAnchor {
                message: format!("read anchor does not resolve: {anchor}"),
            })?,
    };
    let (before, after) = match at {
        crate::Position::Top => (0, after.map_or(DEFAULT_READ_WINDOW, |value| value as usize)),
        crate::Position::Bottom => (
            before.map_or(DEFAULT_READ_WINDOW, |value| value as usize),
            0,
        ),
        crate::Position::At(_) => (
            before.map_or(0, |value| value as usize),
            after.map_or(DEFAULT_READ_WINDOW, |value| value as usize),
        ),
    };
    let (start, end) = match at {
        crate::Position::Bottom => (count.saturating_sub(before), count),
        _ => (
            index.saturating_sub(before),
            index.saturating_add(after.saturating_add(1)).min(count),
        ),
    };
    text.lines = text.lines[start..end].to_vec();
    Ok(text)
}

fn repository_read_options(
    input: &DynamicValue,
) -> Result<(Option<crate::Position>, Option<u32>, Option<u32>), KernelError> {
    let DynamicValue::Record(fields) = input else {
        return Ok((None, None, None));
    };
    let at = match fields.get("at") {
        None | Some(DynamicValue::Option(None)) => None,
        Some(DynamicValue::Option(Some(value))) => Some(value.as_ref()),
        Some(value) => Some(value),
    };
    let at = at
        .map(|value| match value {
            DynamicValue::Variant(name, payload) => match (name.as_str(), payload.as_deref()) {
                ("top", None) => Ok(crate::Position::Top),
                ("bottom", None) => Ok(crate::Position::Bottom),
                ("at", Some(DynamicValue::String(anchor))) => {
                    Ok(crate::Position::At(crate::Anchor::from_tokens(
                        anchor
                            .trim_start_matches('#')
                            .split('.')
                            .map(str::to_owned)
                            .collect(),
                    )))
                }
                _ => Err(KernelError::InvalidRequest {
                    message: "invalid read position".into(),
                }),
            },
            DynamicValue::String(value) if value == "top" => Ok(crate::Position::Top),
            DynamicValue::String(value) if value == "bottom" => Ok(crate::Position::Bottom),
            DynamicValue::String(value) => Ok(crate::Position::At(crate::Anchor::from_tokens(
                value
                    .trim_start_matches('#')
                    .split('.')
                    .map(str::to_owned)
                    .collect(),
            ))),
            _ => Err(KernelError::InvalidRequest {
                message: "invalid read position".into(),
            }),
        })
        .transpose()?;
    let number = |name: &str| match fields.get(name) {
        None | Some(DynamicValue::Option(None)) => Ok(None),
        Some(DynamicValue::Option(Some(value))) => match value.as_ref() {
            DynamicValue::U32(v) => Ok(Some(*v)),
            DynamicValue::U64(v) if *v <= u32::MAX as u64 => Ok(Some(*v as u32)),
            _ => Err(KernelError::InvalidRequest {
                message: format!("read {name} must be u32"),
            }),
        },
        Some(DynamicValue::U32(v)) => Ok(Some(*v)),
        Some(DynamicValue::U64(v)) if *v <= u32::MAX as u64 => Ok(Some(*v as u32)),
        _ => Err(KernelError::InvalidRequest {
            message: format!("read {name} must be u32"),
        }),
    };
    Ok((at, number("before")?, number("after")?))
}

impl RepositoryHandler {
    fn find_paths(
        &self,
        target: &ResourceAddress,
        query: &str,
    ) -> Result<Vec<(String, bool)>, KernelError> {
        let url = Self::url(target)?;
        if url.scheme() == "file" {
            let (file, _) = self.file_and_suffix(target)?;
            if file.is_dir() {
                return Ok(self
                    .search
                    .find_files(&file, &Pattern::parse(query)?)?
                    .into_iter()
                    .map(|path| (path.display().to_string(), path.is_dir()))
                    .collect());
            }
            return Ok(vec![(file.display().to_string(), file.is_dir())]);
        }
        let prefix = url.path().trim_matches('/');
        let output = self
            .search
            .find_files(&self.root, &Pattern::parse(query)?)?
            .into_iter()
            .filter_map(|path| {
                let relative_path = relative(&self.root, &path);
                (prefix.is_empty() || relative_path.starts_with(prefix)).then(|| {
                    (
                        format!("repo://{}/{}", self.project, relative_path),
                        path.is_dir(),
                    )
                })
            })
            .collect::<Vec<_>>();
        Ok(output)
    }

    #[cfg(test)]
    fn grep(&self, target: &ResourceAddress, args: &Value) -> Result<Value, KernelError> {
        let pattern = args.get("pattern").and_then(Value::as_str).ok_or_else(|| {
            KernelError::InvalidRequest {
                message: "repository grep requires args.pattern".to_owned(),
            }
        })?;
        let root = if Self::is_local(target) {
            self.file_and_suffix(target)?.0
        } else {
            self.root.clone()
        };
        let mut matches =
            self.search
                .grep_file(&root, &Pattern::parse(pattern)?, &self.structure)?;
        if !Self::is_local(target) {
            for item in &mut matches {
                let relative_path = item.uri.path().trim_start_matches('/');
                item.uri = crate::ResourceUri::parse(&format!(
                    "repo://{}/{}",
                    self.project, relative_path
                ))?;
            }
        }
        Ok(
            json!({"matches": matches, "paths": matches.iter().map(|item| item.uri.to_string()).collect::<Vec<_>>(), "pattern": pattern}),
        )
    }
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/")
}

fn repository_anchored_text(
    uri: crate::ResourceUri,
    path: &Path,
    source: &[u8],
    structure: &StructuralAnalyzer,
) -> Result<AnchoredText, KernelError> {
    let (_, lines) = structure.analyze(path, source);
    if lines.is_empty() {
        return Ok(AnchoredText {
            uri,
            lines: Vec::new(),
        });
    }
    let anchors = AnchorSet::from_inputs(
        &lines
            .iter()
            .map(StructuralLine::anchor_input)
            .collect::<Vec<_>>(),
    )
    .map_err(|error| KernelError::InvalidAnchor {
        message: error.to_string(),
    })?;
    let anchored = lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            let ending = if source
                .get(line.end_byte..)
                .is_some_and(|tail| tail.starts_with(b"\r\n"))
            {
                crate::LineEnding::Crlf
            } else if source
                .get(line.end_byte..)
                .is_some_and(|tail| tail.starts_with(b"\n"))
            {
                crate::LineEnding::Lf
            } else if source
                .get(line.end_byte..)
                .is_some_and(|tail| tail.starts_with(b"\r"))
            {
                crate::LineEnding::Cr
            } else {
                crate::LineEnding::None
            };
            AnchoredLine {
                anchor: anchors.items()[index].anchor.clone(),
                text: String::from_utf8_lossy(&line.line_text).into_owned(),
                ending,
            }
        })
        .collect();
    Ok(AnchoredText {
        uri,
        lines: anchored,
    })
}

fn symbols(
    _root: &Path,
    path: &Path,
    source: &[u8],
    structure: &StructuralAnalyzer,
    _project: &str,
    file_uri: &str,
) -> Result<Vec<Value>, KernelError> {
    let (_line_provider, lines) = structure.analyze(path, source);
    let anchors = if lines.is_empty() {
        None
    } else {
        AnchorSet::from_inputs(
            &lines
                .iter()
                .map(StructuralLine::anchor_input)
                .collect::<Vec<_>>(),
        )
        .ok()
    };
    let mut output = Vec::new();
    if let Ok(text) = std::str::from_utf8(source)
        && let Some(parsed) = artist_ast::parse_source(path, text)
    {
        let provider = format!("ast-bro:{}", parsed.language);
        for declaration in &parsed.declarations {
            collect_declaration(
                declaration,
                &provider,
                &lines,
                anchors.as_ref(),
                file_uri,
                &mut output,
            );
        }
    } else {
        // Unsupported or non-UTF-8 files still remain readable and retain
        // their kernel line-addressing provider; they simply have no AST
        // symbol projection.
    }
    Ok(output)
}

fn collect_declaration(
    declaration: &artist_ast::core::Declaration,
    provider: &str,
    lines: &[StructuralLine],
    anchors: Option<&AnchorSet>,
    file_uri: &str,
    output: &mut Vec<Value>,
) {
    let mut value = to_value(declaration).unwrap_or_else(|_| json!({}));
    if let Value::Object(fields) = &mut value {
        let anchor = declaration_anchor(declaration.start_line, lines, anchors)
            .map(|anchor| format!("{file_uri}{anchor}"));
        fields.insert(
            "span".to_owned(),
            json!([declaration.start_line, declaration.end_line]),
        );
        fields.insert(
            "def".to_owned(),
            anchor.map(Value::String).unwrap_or(Value::Null),
        );
        fields.insert("provider".to_owned(), Value::String(provider.to_owned()));
    }
    output.push(value);
    for child in &declaration.children {
        collect_declaration(child, provider, lines, anchors, file_uri, output);
    }
}

fn declaration_anchor(
    start_line: usize,
    lines: &[StructuralLine],
    anchors: Option<&AnchorSet>,
) -> Option<String> {
    let index = start_line.checked_sub(1)?;
    lines.get(index)?;
    anchors?
        .items()
        .get(index)
        .map(|item| item.anchor.to_string())
}

fn symbol_resource(
    entries: Vec<Value>,
    suffix: &[String],
    target: &ResourceAddress,
) -> Result<Value, KernelError> {
    if suffix.is_empty() {
        return Ok(json!({"type":"symbols", "items": entries}));
    }
    let name = suffix.join("::");
    let matches = entries
        .into_iter()
        .filter(|entry| entry["name"] == name || entry["name"] == *suffix.last().unwrap())
        .collect::<Vec<_>>();
    if matches.len() == 1 {
        return Ok(json!({"type":"symbol", "node": matches[0]}));
    }
    if matches.is_empty() {
        Err(KernelError::NotFound {
            uri: target.to_string(),
        })
    } else {
        Err(KernelError::InvalidRequest {
            message: format!("symbol path is ambiguous: {name}"),
        })
    }
}
